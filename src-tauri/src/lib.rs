mod audio;
mod cache;
mod dev_watchdog;
mod download;
mod local_lyric;
mod local_music;
mod ncm;
mod process_memory;
mod smtc;
mod thumbbar;
mod unm;

use percent_encoding::percent_decode_str;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    utils::config::WindowConfig,
    webview::WebviewWindowBuilder,
    Emitter, Manager, WindowEvent,
};

/// mini 模式是否激活（进入 mini 后为 true，恢复主窗口后为 false）。
/// 激活期间：托盘左击呼出 mini 窗口而非主窗口；mini 窗口被"关闭"仅隐藏，
/// 主窗口保持隐藏，托盘可再次呼出。
static MINI_MODE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 托盘菜单当前是否正显示着（show_tray_menu_at 置位，失焦/销毁清零）。
/// 用静态标记而不是去查窗口可见性：托盘事件处理器在主线程上跑，
/// 这里只需要一个无歧义的、不可能死锁的判断依据。
static TRAY_MENU_SHOWN: AtomicBool = AtomicBool::new(false);

/// 按需创建的辅助窗口规格（mini-player / desktop-lyric）。
/// 它们不在 tauri.conf.json 里静态声明，否则启动时每个都会建一个 webview
/// （各自的 JS 堆），即使用户从不打开。规格放在独立 JSON 里而不是写在 Rust
/// 常量中：这样窗口选项与静态声明时的写法同源，由 from_config 原样套用，
/// 不会因为抄漏某个选项（transparent / skipTaskbar / shadow 等）而行为不一致。
const AUX_WINDOW_SPECS: &str = include_str!("../aux-windows.json");

#[derive(serde::Deserialize)]
struct AuxWindowSpecFile {
    windows: Vec<WindowConfig>,
}

fn aux_window_spec(label: &str) -> Option<WindowConfig> {
    let specs: AuxWindowSpecFile = serde_json::from_str(AUX_WINDOW_SPECS)
        .map_err(|err| eprintln!("[window] parse aux-windows.json failed: {err}"))
        .ok()?;
    specs
        .windows
        .into_iter()
        .find(|spec| spec.label.as_str() == label)
}

/// 取辅助窗口；不存在则按 aux-windows.json 的规格现场创建。
///
/// 这里就是**实例缓存**：Tauri 的窗口注册表按 label 存活实例，存在即复用；
/// 被销毁后 get_webview_window 返回 None，下次调用会按规格重建（与 EchoMusic
/// 的 ensureXxxWindow 同一形态：缓存命中直接返回，未命中才建）。
/// 只在「确实要显示该窗口」的路径上调用——纯推送状态的路径不能调它，
/// 否则一个 emit 就会把窗口建出来，懒创建失去意义。
///
/// **调用方必须是 async command**：tauri 的 from_config 文档写明，Windows 上
/// 在同步命令或事件处理器里创建 webview 会死锁（wry#583，主线程等自己做窗口
/// 创建的消息循环）。异步命令跑在运行时线程上，正是官方推荐的写法。
fn ensure_aux_window(app: &tauri::AppHandle, label: &str) -> Result<tauri::WebviewWindow, String> {
    if let Some(window) = app.get_webview_window(label) {
        return Ok(window);
    }

    let spec = aux_window_spec(label).ok_or_else(|| format!("no window spec for {label}"))?;
    WebviewWindowBuilder::from_config(app, &spec)
        .map_err(|err| format!("build {label} window failed: {err}"))?
        .build()
        .map_err(|err| format!("create {label} window failed: {err}"))
}

/// 销毁辅助窗口（不存在则忽略）。
///
/// 这是「懒创建 + 实例缓存 + 用完即毁」的最后一层：销毁后渲染进程、JS 堆、
/// DOM、WebGL 资源随窗口一起释放，而不是常驻到进程退出。下次需要时由
/// ensure_aux_window 按规格重建。Tauri 的 destroy 经事件循环代理执行，
/// 在事件处理器或命令线程里调用都安全（它不是创建，没有 wry#583 的问题）。
fn destroy_aux_window(app: &tauri::AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.destroy();
    }
}

/// 定位并显示托盘菜单（居中于鼠标、在鼠标上方）。
/// 状态推送必须跟在 show 之后，否则菜单只会显示初始默认值。
fn show_tray_menu_at(
    window: &tauri::WebviewWindow,
    app: &tauri::AppHandle,
    position: tauri::PhysicalPosition<f64>,
) -> Result<(), String> {
    let size = window
        .outer_size()
        .map_err(|err| format!("read tray-menu size failed: {err}"))?;
    let adjusted_pos = tauri::PhysicalPosition {
        x: position.x - (size.width as f64 / 2.0),   // 居中于鼠标
        y: position.y - (size.height as f64) - 10.0, // 在鼠标上方显示（针对底部任务栏）
    };
    window
        .set_position(tauri::Position::Physical(tauri::PhysicalPosition {
            x: adjusted_pos.x as i32,
            y: adjusted_pos.y as i32,
        }))
        .map_err(|err| err.to_string())?;
    window.show().map_err(|err| err.to_string())?;
    window.set_focus().map_err(|err| err.to_string())?;
    TRAY_MENU_SHOWN.store(true, Ordering::Release);

    // 菜单弹出后通知主窗口推送最新播放器状态，
    // 否则托盘菜单只显示初始默认值
    let _ = app.emit_to("main", "tray-menu-shown", ());
    Ok(())
}

/// 在独立线程里确保托盘菜单窗口存在。
///
/// **绝不能在托盘事件处理器的主线程里直接建 webview**：wry#583 会在 Windows 上
/// 死锁（from_config 的文档同样写明要用 async 命令或独立线程）。
fn ensure_tray_menu_async(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        if let Err(err) = ensure_aux_window(&app, "tray-menu") {
            eprintln!("[tray] create tray-menu failed: {err}");
        }
    });
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// 显示并聚焦主窗口（托盘菜单“显示窗口”用，来自 tray-menu 窗口的调用）
#[tauri::command]
fn tray_show_main_window(app: tauri::AppHandle) -> Result<(), String> {
    // 从托盘菜单显式要求主窗口时，退出 mini 模式
    MINI_MODE_ACTIVE.store(false, Ordering::Release);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit("app-foreground", ());
    }
    if let Some(mini) = app.get_webview_window("mini-player") {
        let _ = mini.hide();
    }
    Ok(())
}

/// 托盘菜单 → 主窗口：转发播放控制指令。
/// 以 smtc-event 事件定向送达主窗口，复用 SMTC 硬件按键的既有处理链路，
/// 避免依赖 webview 间 JS 事件广播（该链路在部分环境下不生效）。
#[tauri::command]
fn tray_player_command(
    app: tauri::AppHandle,
    payload: serde_json::Value,
) -> Result<(), String> {
    app.emit_to("main", "smtc-event", payload)
        .map_err(|e| e.to_string())
}

/// 主窗口 → 托盘菜单：推送播放器状态快照
#[tauri::command]
fn tray_push_state(
    app: tauri::AppHandle,
    state: serde_json::Value,
) -> Result<(), String> {
    app.emit_to("tray-menu", "tray-sync-state", state)
        .map_err(|e| e.to_string())
}

/// 主窗口 → mini 播放器：推送播放器状态快照（含进度/歌词）
#[tauri::command]
fn mini_push_state(
    app: tauri::AppHandle,
    state: serde_json::Value,
) -> Result<(), String> {
    app.emit_to("mini-player", "mini-sync-state", state)
        .map_err(|e| e.to_string())
}

/// 进入 mini 模式：mini 窗口定位到主窗口中心偏上，显示并隐藏主窗口。
/// 必须是 async：内部可能现场创建 mini 窗口（见 ensure_aux_window）
#[tauri::command]
async fn mini_enter(app: tauri::AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or("main window not found")?;
    // 首次进入 mini 模式时才创建 mini 窗口（见 ensure_aux_window）
    let mini = ensure_aux_window(&app, "mini-player")?;

    // 定位到主窗口视口中心偏上（物理坐标），用户拖动后本次会话位置保留
    // （窗口仅 hide/show 不销毁，再次进入时不再强制归位）
    if !mini.is_visible().unwrap_or(true) {
        let pos = main.outer_position().map_err(|e| e.to_string())?;
        let size = main.inner_size().map_err(|e| e.to_string())?;
        let mini_size = mini.outer_size().unwrap_or(tauri::PhysicalSize::new(
            (360.0
                * main
                    .current_monitor()
                    .ok()
                    .flatten()
                    .map(|m| m.scale_factor())
                    .unwrap_or(1.0)) as u32,
            72,
        ));
        let x = pos.x + (size.width.saturating_sub(mini_size.width) as i32) / 2;
        let y = pos.y + (size.height as i32) / 4;
        let _ = mini.set_position(tauri::PhysicalPosition::new(x, y));
    }

    mini.show().map_err(|e| e.to_string())?;
    let _ = mini.set_focus();
    let _ = main.hide();
    MINI_MODE_ACTIVE.store(true, Ordering::Release);

    // mini 窗口（重新）显示后立即拿一次完整快照，避免显示旧状态。
    // 刚创建的窗口此时 JS 还没挂载、监听器未注册，这次推送会丢，
    // 由窗口挂载后的 aux_window_ready 再要一次兜底
    let _ = app.emit_to("main", "mini-entered", ());
    Ok(())
}

/// 退出 mini 模式：恢复主窗口并隐藏 mini 窗口
#[tauri::command]
fn mini_restore_main(app: tauri::AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or("main window not found")?;

    MINI_MODE_ACTIVE.store(false, Ordering::Release);
    main.show().map_err(|e| e.to_string())?;
    let _ = main.set_focus();
    let _ = main.emit("app-foreground", ());
    // 退出 mini 模式 = mini 窗口不再使用：销毁而不是隐藏，把渲染进程还回去。
    // 下次进 mini 模式由 mini_enter 按规格重建（ Destroyed 前窗口内的状态本就
    // 不需要保留，重新显示时会由 aux_window_ready 重新拿一次完整快照）。
    destroy_aux_window(&app, "mini-player");
    Ok(())
}

/// 在一次系统调用内同时设置窗口位置与尺寸（物理像素）。
/// 分离的 set_size/set_position 两次 IPC 之间，窗口会以旧坐标渲染新尺寸，
/// 产生可见跳动；Win32 SetWindowPos 原子完成。mini 播放器与桌面歌词共用。
fn set_window_bounds_atomic(
    window: &tauri::WebviewWindow,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::HasWindowHandle;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
        };

        let scale = window.scale_factor().map_err(|e| e.to_string())?;
        let hwnd = match window.window_handle().map_err(|e| e.to_string())?.as_raw() {
            raw_window_handle::RawWindowHandle::Win32(h) => {
                HWND(h.hwnd.get() as isize)
            }
            _ => return Err("Not a Win32 window".to_string()),
        };

        let result = unsafe {
            SetWindowPos(
                hwnd,
                HWND(0),
                (x * scale).round() as i32,
                (y * scale).round() as i32,
                (width * scale).round() as i32,
                (height * scale).round() as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
        result.ok().map_err(|e| e.to_string())
    }

    #[cfg(not(target_os = "windows"))]
    {
        window
            .set_position(tauri::LogicalPosition::new(x, y))
            .map_err(|e| e.to_string())?;
        window
            .set_size(tauri::LogicalSize::new(width, height))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// mini 播放器展开/收起：一次系统调用内同时设置位置和尺寸（原子 SetWindowPos）。
#[tauri::command]
fn mini_set_bounds(
    app: tauri::AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    // 窗口不存在说明没进过 mini 模式（或已退出），此时无需调整尺寸
    let Some(window) = app.get_webview_window("mini-player") else {
        return Ok(());
    };
    set_window_bounds_atomic(&window, x, y, width, height)
}

/// 辅助窗口挂载完成后调用：通知主窗口重新推送一次完整快照。
/// 懒创建后窗口首次显示时其 JS 监听器尚未注册，show 时刻的推送会丢，
/// 必须由窗口自己就绪后再要一次（与桌面歌词 ready 同源，事件名复用既有的
/// mini-entered / tray-menu-shown，主窗口侧无需新增处理分支）。
#[tauri::command]
fn aux_window_ready(app: tauri::AppHandle, label: String) -> Result<(), String> {
    let event = match label.as_str() {
        "mini-player" => "mini-entered",
        "tray-menu" => "tray-menu-shown",
        "desktop-lyric" => "desktop-lyric-shown",
        _ => return Err(format!("unsupported aux window: {label}")),
    };
    app.emit_to("main", event, ()).map_err(|e| e.to_string())
}

// ===================== 桌面歌词悬浮控件 =====================
// 独立置顶透明窗口：主窗口负责推状态（歌词/进度/控件设置），
// 窗口几何（初始落位/横竖切换缩放）由这里的原子操作完成。

/// 主窗口当前所在显示器的物理几何（桌面歌词落位/钳制的基准）。
fn desktop_lyric_anchor_monitor(
    app: &tauri::AppHandle,
) -> Option<(tauri::PhysicalPosition<i32>, tauri::PhysicalSize<u32>, f64)> {
    let main = app.get_webview_window("main")?;
    let monitor = main
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten())?;
    Some((
        *monitor.position(),
        *monitor.size(),
        monitor.scale_factor(),
    ))
}

/// 显示桌面歌词窗口：落在主窗口所在显示器的底部居中（预留下方任务栏余量），
/// 尺寸由前端按方向/字号计算后传入（逻辑像素）。不抢焦点——桌面歌词
/// 必须在不打断用户当前工作的前提下出现。
/// 必须是 async：首次打开时会现场创建桌面歌词窗口（见 ensure_aux_window）
#[tauri::command]
async fn desktop_lyric_show(
    app: tauri::AppHandle,
    width: f64,
    height: f64,
) -> Result<(), String> {
    // 首次打开桌面歌词时才创建窗口（见 ensure_aux_window）
    let window = ensure_aux_window(&app, "desktop-lyric")?;

    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let w_phys = (width * scale).round().max(1.0) as i32;
    let h_phys = (height * scale).round().max(1.0) as i32;

    // 已在屏上（方向/字号变更触发的重显示）→ 原地缩放，不重新落位
    if window.is_visible().unwrap_or(false) {
        return desktop_lyric_resize(app.clone(), width, height).await;
    }

    let (x_phys, y_phys) = match desktop_lyric_anchor_monitor(&app) {
        Some((mon_pos, mon_size, mon_scale)) => {
            let margin_phys = (56.0 * mon_scale).round() as i32; // 任务栏上方余量
            let x = mon_pos.x + ((mon_size.width as i32) - w_phys) / 2;
            let y = mon_pos.y + (mon_size.height as i32) - h_phys - margin_phys;
            (x.max(mon_pos.x), y.max(mon_pos.y))
        }
        None => (100, 100),
    };

    set_window_bounds_atomic(
        &window,
        x_phys as f64 / scale,
        y_phys as f64 / scale,
        width,
        height,
    )?;
    window.show().map_err(|e| e.to_string())?;
    let _ = app.emit_to("main", "desktop-lyric-shown", ());
    Ok(())
}

/// 关闭桌面歌词窗口。
///
/// 这个命令只会在**用户把开关关掉**时被调用（见 desktopLyricSync），所以语义
/// 就是「不再使用」→ 销毁而不是隐藏，让渲染进程与歌词窗口的 JS 堆一并释放；
/// 下次开启由 desktop_lyric_show 按规格重建，窗口挂载后的 ready 回执会重新
/// 拿一次全量快照。从未打开过时窗口不存在，无需处理。
#[tauri::command]
fn desktop_lyric_hide(app: tauri::AppHandle) -> Result<(), String> {
    destroy_aux_window(&app, "desktop-lyric");
    Ok(())
}

/// 横竖切换/字号变更时的窗口缩放：保持窗口中心点不动（物理坐标），
/// 并钳制在当前显示器范围内，一次 SetWindowPos 原子完成。
///
/// 窗口不存在时直接忽略：现在窗口会被销毁（开关关闭 / Alt+F4），而 JS 侧的
/// 可见性是近似记账，若在这里「补建」就可能把用户刚关掉的窗口又悄悄建回来。
/// 是否显示只由 desktop_lyric_show 决定。
#[tauri::command]
async fn desktop_lyric_resize(
    app: tauri::AppHandle,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let Some(window) = app.get_webview_window("desktop-lyric") else {
        return Ok(());
    };

    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let pos = window.outer_position().map_err(|e| e.to_string())?;
    let size = window.inner_size().map_err(|e| e.to_string())?;

    let w_phys = (width * scale).round().max(1.0) as i32;
    let h_phys = (height * scale).round().max(1.0) as i32;
    let cx = pos.x + (size.width as i32) / 2;
    let cy = pos.y + (size.height as i32) / 2;

    let (mut x, mut y) = (cx - w_phys / 2, cy - h_phys / 2);
    if let Some(mon) = window.current_monitor().ok().flatten() {
        let mon_x = mon.position().x;
        let mon_y = mon.position().y;
        let mon_w = mon.size().width as i32;
        let mon_h = mon.size().height as i32;
        x = x.clamp(mon_x, (mon_x + mon_w - w_phys).max(mon_x));
        y = y.clamp(mon_y, (mon_y + mon_h - h_phys).max(mon_y));
    }

    set_window_bounds_atomic(
        &window,
        x as f64 / scale,
        y as f64 / scale,
        width,
        height,
    )
}

/// 主窗口 → 桌面歌词窗口：推送快照（歌词全文/进度/控件设置）
#[tauri::command]
fn desktop_lyric_push_state(
    app: tauri::AppHandle,
    state: serde_json::Value,
) -> Result<(), String> {
    app.emit_to("desktop-lyric", "desktop-lyric-sync-state", state)
        .map_err(|e| e.to_string())
}

/// 桌面歌词窗口 → 主窗口：转发控件操作（关闭/切换方向/切换动效/调字号/ready）
#[tauri::command]
fn desktop_lyric_command(
    app: tauri::AppHandle,
    payload: serde_json::Value,
) -> Result<(), String> {
    app.emit_to("main", "desktop-lyric-event", payload)
        .map_err(|e| e.to_string())
}

/// 隐藏 mini 窗口但保持 mini 模式激活（主窗口仍隐藏，托盘左击可再次呼出）
///
/// **这里必须只隐藏、不能销毁**：MINI_MODE_ACTIVE 为 true 时托盘左击与二次启动
/// 都按「窗口已存在」直接呼出（只 get 不 ensure），销毁会让这两条路径静默失效。
/// 窗口的销毁时机是退出 mini 模式（mini_restore_main）。
#[tauri::command]
fn mini_close(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(mini) = app.get_webview_window("mini-player") {
        mini.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 诊断：设了 WISP_CDP_PORT 就为 WebView2 打开远程调试端口，用 CDP 抓堆快照
    // （内存面板显示 V8 已用堆一百多 MB，而 JS 侧归因只有几 MB，剩下的成分必须
    // 靠快照才能看清）。默认不设即关闭——正式用户不可能同时开了端口在跑。
    let browser_arguments = match std::env::var("WISP_CDP_PORT") {
        Ok(port) if !port.trim().is_empty() => format!(
            "--disable-features=HardwareMediaKeyHandling --remote-debugging-port={} --remote-allow-origins=*",
            port.trim()
        ),
        _ => "--disable-features=HardwareMediaKeyHandling".to_string(),
    };
    std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", browser_arguments);

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            // 二次启动唤起应用：mini 模式激活时呼出 mini 窗口，否则主窗口
            if MINI_MODE_ACTIVE.load(Ordering::Acquire) {
                if let Some(mini) = app.get_webview_window("mini-player") {
                    let _ = mini.show();
                    let _ = mini.set_focus();
                    let _ = app.emit_to("main", "mini-entered", ());
                }
                return;
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
                let _ = window.emit("app-foreground", ());
            }
        }))
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_store::Builder::default().build())
        .register_uri_scheme_protocol("localmusic", |_app, request| {
            let uri = request.uri().to_string();
            let encoded_path = uri.strip_prefix("localmusic://localhost/").unwrap_or("");
            let path = percent_decode_str(encoded_path)
                .decode_utf8_lossy()
                .to_string();

            match std::fs::read(&path) {
                Ok(file_bytes) => {
                    let mime = match path
                        .rsplit('.')
                        .next()
                        .unwrap_or("")
                        .to_lowercase()
                        .as_str()
                    {
                        "mp3" => "audio/mpeg",
                        "flac" => "audio/flac",
                        "m4a" | "aac" => "audio/mp4",
                        "wav" => "audio/wav",
                        "ogg" => "audio/ogg",
                        "wma" => "audio/x-ms-wma",
                        _ => "application/octet-stream",
                    };
                    tauri::http::Response::builder()
                        .header("Content-Type", mime)
                        .header("Access-Control-Allow-Origin", "*")
                        .body(file_bytes)
                        .unwrap()
                }
                Err(e) => tauri::http::Response::builder()
                    .status(404)
                    .body(format!("File not found: {}", e).into_bytes())
                    .unwrap(),
            }
        })
        .setup(|app| {
            // dev 专属：父进程（tauri dev / cargo）结束时本进程跟着退出，
            // 避免终端侧结束 dev 后留下孤儿实例。release 下是空实现。
            dev_watchdog::spawn_watchdog(app.handle().clone());

            // 主窗口 visible: false 启动，由前端在首帧就绪后显示；
            // 这里兜底：前端异常未能显示时 5s 后强制显示，避免应用"隐形"
            if let Some(window) = app.get_webview_window("main") {
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    if !window.is_visible().unwrap_or(true) {
                        let _ = window.show();
                    }
                });
            }

            // 托盘显式用 128×128 源图：default_window_icon 只有 32×32，
            // 高 DPI 托盘槽位（150%~250% 需要 24~40px）会放大导致模糊
            let tray_image = tauri::image::Image::from_bytes(include_bytes!("../icons/128x128.png"))
                .expect("failed to decode tray icon 128x128.png");
            let _tray = TrayIconBuilder::new()
                .icon(tray_image)
                .tooltip("WispMusic")
                .on_tray_icon_event(|tray, event| match event {
                    // 预热：鼠标一进托盘图标就开始建窗口（懒创建），等真正右键时
                    // 多半已经就绪，菜单弹出不再有建 webview 的等待
                    TrayIconEvent::Enter { .. } => {
                        ensure_tray_menu_async(tray.app_handle().clone());
                    }
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } => {
                        let app = tray.app_handle();
                        // mini 模式激活时托盘左击呼出 mini 窗口（可能刚被关闭隐藏）
                        if MINI_MODE_ACTIVE.load(Ordering::Acquire) {
                            if let Some(mini) = app.get_webview_window("mini-player") {
                                let _ = mini.show();
                                let _ = mini.set_focus();
                                // 窗口可能隐藏了一段时间，重新拿一次完整快照
                                let _ = app.emit_to("main", "mini-entered", ());
                            }
                            return;
                        }
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                            let _ = window.emit("app-foreground", ());
                        }
                    }
                    TrayIconEvent::Click {
                        button: MouseButton::Right,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } => {
                        let app = tray.app_handle();
                        match app.get_webview_window("tray-menu") {
                            Some(window) => {
                                if let Err(err) = show_tray_menu_at(&window, app, position) {
                                    eprintln!("[tray] show tray-menu failed: {err}");
                                }
                            }
                            // 悬停预热没赶上（比如进图标立刻就点了）：在线程里现建，
                            // 建好后自己定位显示。主线程内建 webview 会死锁（wry#583）
                            None => {
                                let app_for_create = app.clone();
                                std::thread::spawn(move || {
                                    // 预热线程可能恰好也在建：先建的那个赢，输的一方
                                    // ensure 会报「已存在」，此时再取一次照常显示
                                    let window =
                                        match ensure_aux_window(&app_for_create, "tray-menu") {
                                            Ok(window) => Some(window),
                                            Err(err) => {
                                                eprintln!(
                                                    "[tray] create tray-menu failed: {err}"
                                                );
                                                app_for_create
                                                    .get_webview_window("tray-menu")
                                            }
                                        };

                                    if let Some(window) = window {
                                        if let Err(err) = show_tray_menu_at(
                                            &window,
                                            &app_for_create,
                                            position,
                                        ) {
                                            eprintln!("[tray] show tray-menu failed: {err}");
                                        }
                                    }
                                });
                            }
                        }
                    }
                    TrayIconEvent::Leave { .. } => {
                        // 预热了但没点（或菜单已关）：回收掉，
                        // 保持「不使用时没有辅助窗口」的形态
                        if !TRAY_MENU_SHOWN.load(Ordering::Acquire) {
                            destroy_aux_window(tray.app_handle(), "tray-menu");
                        }
                    }
                    _ => {}
                })
                .build(app)?;

            smtc::init_smtc(app.handle());

            let window = app
                .get_webview_window("main")
                .expect("Failed to get main window");
            let hwnd = {
                use raw_window_handle::HasWindowHandle;
                let raw = window.window_handle().expect("Failed to get window handle");
                match raw.as_raw() {
                    raw_window_handle::RawWindowHandle::Win32(h) => {
                        windows::Win32::Foundation::HWND(h.hwnd.get() as isize)
                    }
                    _ => panic!("Not a Win32 window"),
                }
            };
            thumbbar::setup(hwnd, app.handle().clone());

            Ok(())
        })
        .on_window_event(|window, event| match event {
            // 窗口销毁策略（懒创建 + 实例缓存 + 用完即毁）：
            // · main：唯一 eager 且常驻的窗口，关闭=藏进托盘，永不销毁；
            // · mini-player：mini 模式期间的关闭只隐藏（托盘左击还要再呼出），
            //   退出 mini 模式（mini_restore_main）时销毁；
            // · desktop-lyric：用户关掉（开关或 Alt+F4）即销毁；
            // · tray-menu：失焦即用完，销毁（Enter 预热会兜住下次弹出的冷启动）。
            //   销毁后下次需要时由 ensure_aux_window 按规格重建，调用方无感。
            WindowEvent::CloseRequested { api, .. } => {
                if window.label() == "mini-player" {
                    // mini 窗口被关闭（Alt+F4 等）→ 仅隐藏并保持 mini 模式，
                    // 托盘左击可再次呼出
                    api.prevent_close();
                    let _ = window.hide();
                    return;
                }
                if window.label() == "desktop-lyric" {
                    // 用户关掉桌面歌词 = 不再使用：先同步开关设置回 false
                    // （设置是唯一可信源），再销毁。不 prevent_close，
                    // 默认关闭行为就是销毁窗口本身
                    let _ = window
                        .app_handle()
                        .emit_to("main", "desktop-lyric-hidden", ());
                    return;
                }
                if window.label() == "tray-menu" {
                    // 托盘菜单被系统关掉（Alt+F4）= 用完，销毁
                    TRAY_MENU_SHOWN.store(false, Ordering::Release);
                    destroy_aux_window(window.app_handle(), "tray-menu");
                    return;
                }
                api.prevent_close();
                let _ = window.emit("app-background", ());

                std::thread::spawn({
                    let window = window.clone();
                    move || {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        let _ = window.hide();
                    }
                });
            }
            WindowEvent::Focused(focused) => {
                // 托盘菜单失焦 = 这次弹出用完了：销毁而不是隐藏。
                // 下次鼠标进托盘图标（Enter）会预热重建，弹出不带冷启动
                if !focused && window.label() == "tray-menu" {
                    TRAY_MENU_SHOWN.store(false, Ordering::Release);
                    destroy_aux_window(window.app_handle(), "tray-menu");
                }
            }
            _ => {}
        })
        .manage(audio::AudioState::new())
        .manage(download::new_registry())
        .manage(cache::CacheState::new())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            tray_show_main_window,
            tray_player_command,
            tray_push_state,
            mini_push_state,
            mini_enter,
            mini_restore_main,
            mini_close,
            mini_set_bounds,
            aux_window_ready,
            desktop_lyric_show,
            desktop_lyric_hide,
            desktop_lyric_resize,
            desktop_lyric_push_state,
            desktop_lyric_command,
            ncm::ncm_fetch,
            unm::unm_match,
            smtc::smtc_update_metadata,
            smtc::smtc_update_playback,
            cache::check_audio_cache,
            cache::cache_audio,
            cache::get_audio_cache_size,
            cache::clear_audio_cache,
            cache::enforce_cache_limit,
            cache::enforce_audio_cache_limit,
            cache::set_audio_cache_limit,
            audio::player_load,
            audio::player_preload,
            audio::player_play,
            audio::player_pause,
            audio::player_seek,
            audio::player_set_volume,
            audio::player_set_speed,
            audio::player_stop,
            audio::player_get_state,
            audio::list_audio_output_devices,
            audio::player_set_output_device,
            audio::player_set_replay_gain,
            audio::player_set_equalizer,
            audio::player_set_crossfade,
            audio::get_native_audio_cache_size,
            audio::clear_native_audio_cache,
            audio::enforce_native_audio_cache_limit,
            download::get_default_download_dir,
            download::ensure_dir_exists,
            download::download_song,
            download::pause_download,
            local_music::scan_local_music,
            local_music::get_default_music_dir,
            local_lyric::read_text_file,
            local_lyric::find_sibling_lyric,
            process_memory::get_process_memory_report,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod window_lifecycle_tests {
    use super::*;

    const TAURI_CONF: &str = include_str!("../tauri.conf.json");
    const CAPABILITIES: &str = include_str!("../capabilities/default.json");

    fn parsed_specs() -> AuxWindowSpecFile {
        serde_json::from_str(AUX_WINDOW_SPECS).expect("aux-windows.json 应可解析")
    }

    #[test]
    fn aux_windows_are_not_declared_statically() {
        // 两者互斥：静态声明就会在启动时把 webview 建出来，懒创建即失效。
        // 这条断言用来防止以后有人「顺手」把窗口加回 tauri.conf.json
        for spec in parsed_specs().windows {
            let label = spec.label.as_str();
            assert!(
                !TAURI_CONF.contains(&format!("\"{label}\"")),
                "{label} 仍静态声明在 tauri.conf.json，启动时会白建一个 webview"
            );
        }
    }

    #[test]
    fn known_aux_windows_have_specs() {
        // 三个辅助窗口都必须有按需创建规格，缺了会被 ensure_aux_window 拒绝
        for label in ["tray-menu", "mini-player", "desktop-lyric"] {
            assert!(
                aux_window_spec(label).is_some(),
                "{label} 缺少按需创建规格"
            );
        }
    }

    #[test]
    fn every_aux_window_label_is_allowed_by_capabilities() {
        // capabilities 的 windows 数组漏配 → 该窗口的 core 命令全被 ACL 拒，
        // 表现为窗口空白，是这套结构里最容易踩且最难排查的一环
        let caps: serde_json::Value =
            serde_json::from_str(CAPABILITIES).expect("capabilities/default.json 应可解析");
        let allowed: Vec<String> = caps["windows"]
            .as_array()
            .expect("capabilities 应有 windows 数组")
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();

        for spec in parsed_specs().windows {
            assert!(
                allowed.contains(&spec.label),
                "capabilities/default.json 的 windows 缺少 {}",
                spec.label
            );
        }
    }

    #[test]
    fn aux_window_specs_keep_expected_geometry() {
        let tray = aux_window_spec("tray-menu").expect("tray-menu 规格应存在");
        assert_eq!((tray.width, tray.height), (200.0, 412.0));
        assert!(tray.always_on_top, "托盘菜单需置顶");
        assert!(tray.skip_taskbar, "托盘菜单不进任务栏");
        assert!(tray.transparent, "托盘菜单需透明底");
        assert!(!tray.decorations && !tray.resizable);
        assert!(tray.window_effects.is_some(), "托盘菜单需要 acrylic 材质");

        let mini = aux_window_spec("mini-player").expect("mini-player 规格应存在");
        assert_eq!((mini.width, mini.height), (360.0, 72.0));
        assert!(mini.always_on_top, "mini 需置顶");
        assert!(mini.skip_taskbar, "mini 不进任务栏");
        assert!(mini.transparent, "mini 需透明底");
        assert!(!mini.decorations, "mini 无系统边框");
        assert!(!mini.resizable, "mini 尺寸由展开状态接管");

        let lyric = aux_window_spec("desktop-lyric").expect("desktop-lyric 规格应存在");
        assert_eq!((lyric.width, lyric.height), (1000.0, 188.0));
        assert!(lyric.resizable, "桌面歌词需系统边缘缩放");
        assert!(lyric.always_on_top && lyric.skip_taskbar && lyric.transparent);
        assert!(!lyric.shadow, "桌面歌词不要系统投影");
    }
}
