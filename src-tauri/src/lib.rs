mod audio;
mod cache;
mod dev_watchdog;
mod download;
mod local_lyric;
mod local_music;
mod ncm;
mod smtc;
mod thumbbar;
mod unm;

use percent_encoding::percent_decode_str;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WindowEvent,
};

/// mini 模式是否激活（进入 mini 后为 true，恢复主窗口后为 false）。
/// 激活期间：托盘左击呼出 mini 窗口而非主窗口；mini 窗口被"关闭"仅隐藏，
/// 主窗口保持隐藏，托盘可再次呼出。
static MINI_MODE_ACTIVE: AtomicBool = AtomicBool::new(false);

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

/// 进入 mini 模式：mini 窗口定位到主窗口中心偏上，显示并隐藏主窗口
#[tauri::command]
fn mini_enter(app: tauri::AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or("main window not found")?;
    let mini = app
        .get_webview_window("mini-player")
        .ok_or("mini-player window not found")?;

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

    // mini 窗口（重新）显示后立即拿一次完整快照，避免显示旧状态
    let _ = app.emit_to("main", "mini-entered", ());
    Ok(())
}

/// 退出 mini 模式：恢复主窗口并隐藏 mini 窗口
#[tauri::command]
fn mini_restore_main(app: tauri::AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or("main window not found")?;
    let mini = app
        .get_webview_window("mini-player")
        .ok_or("mini-player window not found")?;

    MINI_MODE_ACTIVE.store(false, Ordering::Release);
    main.show().map_err(|e| e.to_string())?;
    let _ = main.set_focus();
    let _ = main.emit("app-foreground", ());
    let _ = mini.hide();
    Ok(())
}

/// mini 播放器展开/收起：一次系统调用内同时设置位置和尺寸。
/// 分离的 set_size/set_position 两次 IPC 之间，窗口会以旧坐标渲染新尺寸
/// （左上角锚点先向下扩展再上移），产生可见跳动；Win32 SetWindowPos 原子完成。
#[tauri::command]
fn mini_set_bounds(
    app: tauri::AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let window = app
        .get_webview_window("mini-player")
        .ok_or("mini-player window not found")?;

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

/// 隐藏 mini 窗口但保持 mini 模式激活（主窗口仍隐藏，托盘左击可再次呼出）
#[tauri::command]
fn mini_close(app: tauri::AppHandle) -> Result<(), String> {
    let mini = app
        .get_webview_window("mini-player")
        .ok_or("mini-player window not found")?;
    mini.hide().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    std::env::set_var(
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "--disable-features=HardwareMediaKeyHandling",
    );

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
                        if let Some(window) = app.get_webview_window("tray-menu") {
                            let size = window.outer_size().unwrap();
                            let adjusted_pos = tauri::PhysicalPosition {
                                x: position.x - (size.width as f64 / 2.0),   // 居中于鼠标
                                y: position.y - (size.height as f64) - 10.0, // 在鼠标上方显示（针对底部任务栏）
                            };
                            let _ = window.set_position(tauri::Position::Physical(
                                tauri::PhysicalPosition {
                                    x: adjusted_pos.x as i32,
                                    y: adjusted_pos.y as i32,
                                },
                            ));
                            window.show().unwrap();
                            window.set_focus().unwrap();

                            // 菜单弹出后通知主窗口推送最新播放器状态，
                            // 否则托盘菜单只显示初始默认值
                            let _ = app.emit_to("main", "tray-menu-shown", ());
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
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                if window.label() == "mini-player" {
                    // mini 窗口被关闭（Alt+F4 等）→ 仅隐藏并保持 mini 模式，
                    // 托盘左击可再次呼出
                    let _ = window.hide();
                    return;
                }
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
