mod alloc_stats;
mod audio;
mod cache;
mod dev_watchdog;
mod download;
mod http_client;
mod local_lyric;
mod local_music;
mod logger;
mod ncm;
mod process_memory;
mod smtc;
mod thumbbar;
mod unm;

/// Rust 堆占用记账（诊断用，见 alloc_stats）。常态常开：内存报告要能覆盖
/// release 构建，而 release 正是需要诊断的那个
#[global_allocator]
static GLOBAL_ALLOCATOR: alloc_stats::CountingAllocator = alloc_stats::CountingAllocator;

use percent_encoding::percent_decode_str;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// 主窗口是否显示过（前端 show 成功后主动上报，或 Rust 侧任一呼出路径）。
///
/// 唯一用途是给启动兜底线程一个**语义正确**的判据。兜底要解决的是「前端首帧
/// 异常，窗口从头到尾没出现过」，而不是「此刻窗口不可见」。主窗口的关闭语义是
/// 「藏进托盘」——CloseRequested 里 prevent_close + 延迟 hide，此时窗口本来就
/// 不可见；若兜底只看可见性，启动 5s 内关掉窗口就会把它重新显示出来，而这时
/// App 已收到 app-background、渲染的是空 div，桌面上就留下一块空白材质窗口。
static MAIN_SHOWN: AtomicBool = AtomicBool::new(false);

/// 主窗口显示代号：每次「呼出主窗口」或「主窗口转入后台」都自增。
///
/// CloseRequested 里的隐藏挂在 50ms 后的独立线程上（先把内容切成空 div 再 hide），
/// 这 50ms 内用户完全可能又通过托盘/二次启动把窗口呼出来。延迟线程记住自己发起
/// 时的代号、hide 前比对：不一致说明这次隐藏已过期，跳过，否则会把刚呼出的窗口打掉。
static MAIN_EPOCH: AtomicU64 = AtomicU64::new(0);

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
        .map_err(|err| log_error!("[window] parse aux-windows.json failed: {err}"))
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

/// 呼出主窗口：推进显示代号 → show → 聚焦 → 定向通知前端回到前台态。
///
/// **所有呼出主窗口的路径都必须走这里**：代号是延迟隐藏判断自己是否过期的唯一
/// 依据（见 MAIN_EPOCH），漏掉一条路径就会出现「刚呼出又被上一次关闭的延迟 hide
/// 打掉」的反向故障。app-foreground 定向发给 main 而不是广播，与 app-background
/// 对称（广播会惊动 mini / tray-menu 窗口）。
///
/// `focus = false` 用于启动兜底：窗口首次出现不该抢走用户当前正在用的窗口焦点。
fn show_main_window(app: &tauri::AppHandle, focus: bool) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("main window not found")?;

    MAIN_EPOCH.fetch_add(1, Ordering::AcqRel);
    MAIN_SHOWN.store(true, Ordering::Release);

    window.show().map_err(|err| err.to_string())?;
    if focus {
        let _ = window.set_focus();
    }
    let _ = app.emit_to("main", "app-foreground", ());
    Ok(())
}

/// 前端主窗口显示成功后上报：给启动兜底线程「启动显示已完成」的信号。
/// 前端异常时不会调用，兜底仍会按 5s 把窗口显示出来（原有用途不变）。
#[tauri::command]
fn main_mark_shown() {
    MAIN_SHOWN.store(true, Ordering::Release);
}

/// 当前采用的日志目录（绝对路径字符串）。
///
/// 放在 Rust 侧而不是前端拼 `%APPDATA%`：安装版优先写 exe 同级的 `logs`，装到
/// `Program Files` 下不可写时才回落到系统日志目录（见 `logger.rs`）——前端按
/// 任一固定位置显示都会在其中一种安装方式下指错地方。
#[tauri::command]
fn log_directory() -> Option<String> {
    logger::directory().map(|dir| dir.display().to_string())
}

/// 可看的日志文件列表（主日志、轮转出的历史、解灰 sidecar 日志）
#[tauri::command]
fn log_files() -> Vec<logger::LogFileInfo> {
    logger::files()
}

/// 读某个日志文件的尾部若干行；文件名必须来自 `log_files`，否则报错
#[tauri::command]
fn log_file_tail(name: String, max_lines: Option<usize>) -> Result<String, String> {
    logger::tail(&name, max_lines.unwrap_or(800).clamp(1, 5000))
}

/// 定位并显示托盘菜单（居中于鼠标、在鼠标上方）。
///
/// 托盘菜单窗口**常驻**（启动即建、隐藏复用、不销毁）：托盘右键是高频主交互，
/// 按需创建的冷启动（首次要拉起一个新的 WebView2 渲染进程，1~3 秒）会让
/// 首次右键无响应。因此这里只做「定位 + 显示 + 重置」，没有任何创建/销毁。
fn show_tray_menu_at(
    window: &tauri::WebviewWindow,
    app: &tauri::AppHandle,
    position: tauri::PhysicalPosition<f64>,
) -> Result<(), String> {
    let size = window
        .outer_size()
        .map_err(|err| format!("read tray-menu size failed: {err}"))?;

    // 居中于鼠标、显示在鼠标上方（针对底部任务栏）。
    // 任务栏在屏幕顶部/左右时这个默认位置会把菜单推出屏幕外，按光标所在
    // 显示器钳制，保证任意位置的托盘都能完整显示菜单。
    let mut x = (position.x - (size.width as f64 / 2.0)).round() as i32;
    let mut y = (position.y - (size.height as f64) - 10.0).round() as i32;
    if let Ok(Some(monitor)) = app.monitor_from_point(position.x, position.y) {
        let mon_pos = monitor.position();
        let mon_size = monitor.size();
        let max_x = mon_pos.x + mon_size.width as i32 - size.width as i32;
        let max_y = mon_pos.y + mon_size.height as i32 - size.height as i32;
        x = x.clamp(mon_pos.x, max_x.max(mon_pos.x));
        y = y.clamp(mon_pos.y, max_y.max(mon_pos.y));
    }

    window
        .set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(x, y)))
        .map_err(|err| err.to_string())?;

    // 通知托盘菜单自身重置上一次遗留的临时 UI 状态（如展开中的子菜单），
    // 并通知主窗口推送最新播放器状态。两件事都要赶在 show 之前：show 之后才发，
    // 会先亮出一帧上一次遗留的子菜单/旧播放状态
    let _ = app.emit_to("tray-menu", "tray-menu-shown", ());
    let _ = app.emit_to("main", "tray-menu-shown", ());

    window.show().map_err(|err| err.to_string())?;

    // 聚焦失败（Windows 前台锁 / 被其它进程抢焦点）必须把窗口藏回去：
    // 托盘菜单唯一的自动隐藏入口是失焦事件（见 on_window_event 的 Focused 分支），
    // 而没拿到焦点的窗口永远不会失焦——一个 alwaysOnTop 的菜单会永久挂在桌面上。
    if let Err(err) = window.set_focus() {
        let _ = window.hide();
        return Err(err.to_string());
    }
    Ok(())
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
    let _ = show_main_window(&app, true);
    if let Some(mini) = app.get_webview_window("mini-player") {
        let _ = mini.hide();
        // 主窗口据此停止 mini 状态推送（重新进入 mini 时 mini_enter 再发 mini-entered）
        let _ = app.emit_to("main", "mini-hidden", ());
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
    MINI_MODE_ACTIVE.store(false, Ordering::Release);
    show_main_window(&app, true)?;
    // 退出 mini 模式 = mini 窗口不再使用：销毁而不是隐藏，把渲染进程还回去。
    // 下次进 mini 模式由 mini_enter 按规格重建（ Destroyed 前窗口内的状态本就
    // 不需要保留，重新显示时会由 aux_window_ready 重新拿一次完整快照）。
    destroy_aux_window(&app, "mini-player");
    // 主窗口据此停止 mini 状态推送；重新进入 mini 时 mini_enter 会再发 mini-entered
    let _ = app.emit_to("main", "mini-hidden", ());
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
    // 主窗口据此停止 mini 状态推送（隐藏不等于销毁，但推送已无人接收）
    let _ = app.emit_to("main", "mini-hidden", ());
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
            let _ = show_main_window(app, true);
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
            // 第一件事就是开日志：release 没有控制台（main.rs 的 windows_subsystem），
            // 不装这份文件日志，安装版上发生的 panic 与失败会一个字都不留。
            logger::init(app.handle());
            log_info!("[app] 启动完成，版本 {}", app.package_info().version);

            // dev 专属：父进程（tauri dev / cargo）结束时本进程跟着退出，
            // 避免终端侧结束 dev 后留下孤儿实例。release 下是空实现。
            dev_watchdog::spawn_watchdog(app.handle().clone());

            // 启动后台清扫磁盘缓存：孤儿临时文件（下载中途切歌/强退残留的
            // .part-* 与 .tmp）与过期封面缓存。放后台线程，不拖慢启动；
            // 封面清扫要赶在前端本地曲库扫描之前跑（见 cleanup_stale_covers）
            let cleanup_handle = app.handle().clone();
            std::thread::spawn(move || {
                cache::cleanup_orphan_temp_files(&cleanup_handle);
                local_music::cleanup_stale_covers(&cleanup_handle);
            });

            // 主窗口 visible: false 启动，由前端在首帧就绪后显示；
            // 这里兜底：前端异常未能显示时 5s 后强制显示，避免应用"隐形"。
            //
            // 判据是 MAIN_SHOWN（启动显示是否发生过），**不是「此刻是否可见」**：
            // 主窗口的关闭语义是「藏进托盘」，只看可见性会把用户刚关掉的窗口
            // 重新显示到桌面上——而此时内容已被 app-background 清空，结果就是
            // 桌面上留下一块空白的材质窗口（transparent + acrylic）。
            let fallback_handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                if !MAIN_SHOWN.load(Ordering::Acquire) {
                    let _ = show_main_window(&fallback_handle, false);
                }
            });

            // 托盘显式用 128×128 源图：default_window_icon 只有 32×32，
            // 高 DPI 托盘槽位（150%~250% 需要 24~40px）会放大导致模糊
            let tray_image = tauri::image::Image::from_bytes(include_bytes!("../icons/128x128.png"))
                .expect("failed to decode tray icon 128x128.png");
            let _tray = TrayIconBuilder::new()
                .icon(tray_image)
                .tooltip("WispMusic")
                .on_tray_icon_event(|tray, event| match event {
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
                        let _ = show_main_window(app, true);
                    }
                    TrayIconEvent::Click {
                        button: MouseButton::Right,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } => {
                        // 托盘菜单窗口常驻（启动即建、隐藏复用），这里只做定位显示
                        let app = tray.app_handle();
                        match app.get_webview_window("tray-menu") {
                            Some(window) => {
                                if let Err(err) = show_tray_menu_at(&window, app, position) {
                                    log_error!("[tray] show tray-menu failed: {err}");
                                }
                            }
                            None => log_error!("[tray] tray-menu window not found"),
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
            // 窗口生命周期策略：
            // · main：唯一 eager 且常驻的窗口，关闭=藏进托盘，永不销毁；
            // · tray-menu：**常驻复用，永不销毁**——托盘右键是高频主交互，
            //   按需创建的冷启动（首次要拉起一个新的 WebView2 渲染进程，1~3 秒）
            //   会让首次右键无响应。所以这里只做隐藏，每次弹出时通过
            //   tray-menu-shown 事件重置遗留的临时 UI 状态；
            // · mini-player：mini 模式期间的关闭只隐藏（托盘左击还要再呼出），
            //   退出 mini 模式（mini_restore_main）时销毁；
            // · desktop-lyric：用户关掉（开关或 Alt+F4）即销毁。
            //   销毁后下次需要时由 ensure_aux_window 按规格重建，调用方无感。
            WindowEvent::CloseRequested { api, .. } => {
                if window.label() == "mini-player" {
                    // mini 窗口被关闭（Alt+F4 等）→ 仅隐藏并保持 mini 模式，
                    // 托盘左击可再次呼出
                    api.prevent_close();
                    let _ = window.hide();
                    // 与 mini_close 同：通知主窗口停止 mini 状态推送
                    let _ = window.app_handle().emit_to("main", "mini-hidden", ());
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
                // main 与 tray-menu 都不真正关闭：前者藏进托盘，后者隐藏复用
                api.prevent_close();

                if window.label() != "main" {
                    // 托盘菜单只需隐藏（它的显隐本就由失焦事件驱动，这里是兜底）
                    let _ = window.hide();
                    return;
                }

                // 只有主窗口转入后台才需要清空内容，而且必须**定向**发给 main：
                // 早期这里用的是广播版本，结果「对 tray-menu 按 Alt+F4」也会把可见的
                // 主窗口清空成空白窗口。与 app-foreground 的定向发送保持对称。
                let epoch = MAIN_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
                let _ = window.app_handle().emit_to("main", "app-background", ());

                // 延迟 50ms 再隐藏：先让前端把内容切成空 div，避免隐藏瞬间还挂着完整 UI
                // （任务栏缩略图与合成器仍会抓上一帧）。这段延迟让「隐藏」与「呼出」可能
                // 并发，所以 hide 前比对自己发起时的代号：过期说明期间又被呼出过，跳过，
                // 否则会把刚显示的窗口打掉（反向故障：窗口不见了但前端处于前台态）。
                std::thread::spawn({
                    let window = window.clone();
                    move || {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        if MAIN_EPOCH.load(Ordering::Acquire) != epoch {
                            return;
                        }
                        let _ = window.hide();
                    }
                });
            }
            WindowEvent::Focused(focused) => {
                // 托盘菜单失焦 = 这次弹出结束：隐藏复用（窗口不销毁，
                // 下次右键直接定位显示；临时 UI 状态由 tray-menu-shown 重置）
                if !focused && window.label() == "tray-menu" {
                    let _ = window.hide();
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
            main_mark_shown,
            log_directory,
            log_files,
            log_file_tail,
            desktop_lyric_show,
            desktop_lyric_hide,
            desktop_lyric_resize,
            desktop_lyric_push_state,
            desktop_lyric_command,
            ncm::ncm_fetch,
            unm::unm_match,
            unm::unm_warmup,
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
            audio::player_latest_request_id,
            audio::player_thread_status,
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

    /// 常驻窗口清单：启动即由 tauri.conf.json 创建（main 唯一可见，
    /// tray-menu 隐藏复用——托盘右键是高频主交互，不能接受冷启动）。
    /// main 在 tauri.conf.json 里省略 label（默认即 main），无法按字符串断言；
    /// 这里只列显式声明了 label 的常驻窗口。
    const STATIC_LABELED_WINDOWS: [&str; 1] = ["tray-menu"];

    /// 懒创建窗口清单：首次需要时才由 ensure_aux_window 按 aux-windows.json 建
    const LAZY_WINDOWS: [&str; 2] = ["mini-player", "desktop-lyric"];

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
    fn static_windows_are_declared() {
        // 常驻窗口必须在 tauri.conf.json 里声明，缺了启动时根本不存在
        for label in STATIC_LABELED_WINDOWS {
            assert!(
                TAURI_CONF.contains(&format!("\"{label}\"")),
                "常驻窗口 {label} 未在 tauri.conf.json 声明"
            );
        }
    }

    #[test]
    fn known_aux_windows_have_specs() {
        // 懒创建窗口必须有按需创建规格，缺了会被 ensure_aux_window 拒绝
        for label in LAZY_WINDOWS {
            assert!(
                aux_window_spec(label).is_some(),
                "{label} 缺少按需创建规格"
            );
        }
    }

    #[test]
    fn every_window_label_is_allowed_by_capabilities() {
        // capabilities 的 windows 数组漏配 → 该窗口的 core 命令全被 ACL 拒，
        // 表现为窗口空白，是这套结构里最容易踩且最难排查的一环。
        // 常驻与懒创建的窗口都要覆盖。
        let caps: serde_json::Value =
            serde_json::from_str(CAPABILITIES).expect("capabilities/default.json 应可解析");
        let allowed: Vec<String> = caps["windows"]
            .as_array()
            .expect("capabilities 应有 windows 数组")
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();

        for label in STATIC_LABELED_WINDOWS {
            assert!(
                allowed.contains(&label.to_string()),
                "capabilities/default.json 的 windows 缺少 {label}"
            );
        }
        for spec in parsed_specs().windows {
            assert!(
                allowed.contains(&spec.label),
                "capabilities/default.json 的 windows 缺少 {}",
                spec.label
            );
        }
    }

    #[test]
    fn static_tray_menu_keeps_expected_geometry() {
        let conf: serde_json::Value =
            serde_json::from_str(TAURI_CONF).expect("tauri.conf.json 应可解析");
        let windows = conf["app"]["windows"]
            .as_array()
            .expect("tauri.conf.json 应有 app.windows");
        let tray = windows
            .iter()
            .find(|window| window["label"] == "tray-menu")
            .expect("tray-menu 应静态声明在 tauri.conf.json");

        assert_eq!(tray["width"], 200);
        assert_eq!(tray["height"], 412);
        assert_eq!(tray["alwaysOnTop"], true, "托盘菜单需置顶");
        assert_eq!(tray["skipTaskbar"], true, "托盘菜单不进任务栏");
        assert_eq!(tray["transparent"], true, "托盘菜单需透明底");
        assert_eq!(tray["resizable"], false, "托盘菜单尺寸固定");
        assert_eq!(tray["decorations"], false, "托盘菜单无系统边框");
        assert!(
            tray["windowEffects"]["effects"].is_array(),
            "托盘菜单需要 acrylic 材质"
        );
    }

    #[test]
    fn aux_window_specs_keep_expected_geometry() {
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

    const LIB_SRC: &str = include_str!("lib.rs");

    /// 去掉本测试模块之后的源码：断言里写下的模式字符串本身就在 lib.rs 里，
    /// 不切掉的话「源码中不存在 X」这类断言会被断言自己的字符串满足
    fn production_source() -> &'static str {
        LIB_SRC
            .split("mod window_lifecycle_tests")
            .next()
            .unwrap_or(LIB_SRC)
    }

    /// 「关闭窗口后桌面残留空白窗口」的回归锁（源码级断言，不需要真实窗口）。
    ///
    /// 四条不变量各自对应一个已修的缺陷，任何一条被改回去都会在这里失败：
    /// 1. app-background / app-foreground 必须定向发送——广播版本会让「关掉
    ///    tray-menu」连带把**可见的**主窗口清空成空白窗口；
    /// 2. 启动兜底的判据必须是「显示过」（MAIN_SHOWN）而不是「此刻可见」——
    ///    主窗口关闭 = 藏进托盘，只看可见性会把用户刚关掉的窗口重新显示出来；
    /// 3. 延迟隐藏必须比对称号，否则会打掉延迟期间被呼出的窗口；
    /// 4. 托盘菜单聚焦失败必须回滚隐藏——它的自动隐藏只由失焦事件驱动，
    ///    没拿到焦点的 alwaysOnTop 菜单会永久挂在桌面上。
    #[test]
    fn blank_window_regressions_are_guarded() {
        let src = production_source();

        assert!(
            src.contains("emit_to(\"main\", \"app-background\""),
            "app-background 必须定向发送给 main"
        );
        assert!(
            !src.contains("emit(\"app-background\""),
            "app-background 禁止广播：关掉 tray-menu 会把可见的主窗口清空成空白窗口"
        );
        assert_eq!(
            src.matches("\"app-background\"").count(),
            1,
            "app-background 只应出现在那一处定向 emit_to 里"
        );
        assert_eq!(
            src.matches("\"app-foreground\"").count(),
            1,
            "app-foreground 只应出现在 show_main_window 的定向发送里，与 app-background 对称"
        );

        assert!(
            src.contains("if !MAIN_SHOWN.load("),
            "启动兜底必须由 MAIN_SHOWN（是否显示过）把关，而不是当前的可见性"
        );
        assert!(
            src.contains("MAIN_EPOCH.load(Ordering::Acquire) != epoch"),
            "延迟隐藏前必须比对称号，否则会打掉延迟期间刚被呼出的主窗口"
        );
        assert!(
            src.contains("if let Err(err) = window.set_focus()"),
            "托盘菜单聚焦失败必须 hide() 回滚，否则永不失焦、永不自动隐藏"
        );
    }
}
