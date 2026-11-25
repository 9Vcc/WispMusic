#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
#[cfg(windows)]
mod app_icon;
mod assets;
mod console;
mod dock;
mod http;
mod logging;
mod memory;
mod single;
#[cfg(windows)]
mod thumbbar;
mod tray;
mod tray_menu;

use std::rc::Rc;
#[cfg(windows)]
mod window_title;

use std::path::PathBuf;
use std::process::{ExitCode, exit};
use std::sync::Arc;
use std::time::Duration;

use cli::commands::CommandType;
use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Global, Pixels, QuitMode, Size, TitlebarOptions,
    WindowBounds, WindowOptions, px, size,
};
use music::LyricsProvider;
use music::link::Linked;
use router::{Destination, Screen};
use state::{Opened, Outcome, Wisp, Toasts};
use ui::ActiveTheme as _;
use ui::ThemeKind;
use views::Root;

const LEAST_SIZE: Size<Pixels> = size(px(480.), px(400.));
const FIRST_SIZE: Size<Pixels> = size(px(920.), px(640.));
/// Some file managers launch a fresh process per selected file instead of one with every path,
/// so a multi-file "Open With" arrives here as several single-file hand-offs within milliseconds
/// of each other. The first one plays at once and the rest join its queue. This is how long the
/// burst has to stay quiet before it's considered over, so every file lands in one queue
/// instead of racing the others in.
const OPEN_COALESCE: Duration = Duration::from_millis(250);

fn main() -> ExitCode {
    memory::tune();
    if let Some(code) = webview::probed() {
        exit(code);
    }
    // A command for the running Wisp is sent and answered here, without a log file, a
    // runtime or a window. Older macOS adds a `-psn_` argument to a Finder launch.
    let args = std::env::args_os()
        .filter(|arg| !arg.to_string_lossy().starts_with("-psn_"))
        .collect();
    let command = cli::parse(args, state::version());
    if !matches!(command, Ok(CommandType::Launch(_))) {
        console::attach();
    }
    let args = match command {
        Ok(CommandType::Launch(items)) => items,
        Ok(CommandType::Local(local)) => return cli::local(local),
        Ok(CommandType::Remote(request, format)) => {
            return match single::connect(request.streams()) {
                Ok(mut stream) => cli::run(&mut stream, &request, format),
                Err(_) => cli::not_running(),
            };
        }
        Err(refusal) => return refusal.exit(),
    };
    let portable = paths::init();
    logging::init();
    match &portable {
        Some(root) => log::info!("paths: using {}", root.display()),
        None => log::info!("paths: using the platform data directories"),
    }

    let (sender, mut links) = tokio::sync::mpsc::unbounded_channel::<Vec<String>>();
    let (asked, mut commands) = tokio::sync::mpsc::unbounded_channel::<single::Asked>();
    match single::claim(&args, sender.clone(), asked) {
        single::Instance::First => {}
        single::Instance::Running => return ExitCode::SUCCESS,
        single::Instance::Failed => exit(1),
    }
    // reqwest is built without a crypto provider, and another dependency still compiles
    // aws-lc-rs in beside ring, so rustls cannot pick one by itself. Every client uses ring from
    // here on. A second install only means another crate got there first.
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let io = match state::Io::new() {
        Ok(io) => io,
        Err(error) => {
            eprintln!("wisp: cannot start runtime: {error:#}");
            return ExitCode::FAILURE;
        }
    };
    // reqwest 的 DNS 解析绑到这个 runtime：请求 future 无论在哪个执行器上被
    // poll（tokio、gpui 主线程、后台），域名解析都发生在 tokio 里，不会再
    // 出现「no reactor running」的崩溃。
    music::bind_dns_runtime(io.handle());

    let app = gpui_platform::application()
        .with_assets(assets::Assets)
        .with_http_client(Arc::new(http::Client::new(io.handle())));
    let opened_urls_sender = sender.clone();
    app.on_open_urls(move |opened| {
        opened_urls_sender.send(opened).ok();
    });
    app.on_reopen(show_window);

    app.run(move |cx: &mut App| {
        if let Err(error) = assets::Assets.load_fonts(cx) {
            log::error!("wisp: cannot load bundled fonts: {error:#}");
        }

        let database = storage::Database::standard();
        // [wisp] provider 列表由已装插件 manifest 驱动：内置网易云仍是同 slug 的 fallback，
        // 没有 host bridge 的插件只进管理列表，不会被当成可用 provider。
        let plugins_dir = music::plugins::plugins_dir();
        let plugins = music::plugins::Catalog::scan(&plugins_dir);
        let providers = plugins.providers();
        let local_provider: Arc<dyn music::MusicProvider> =
            Arc::new(music::local::LocalProvider::new(
                paths::cache_dir()
                    .unwrap_or_else(std::env::temp_dir)
                    .join("wisp"),
                database.clone(),
                storage::Cache::standard(),
            ));
        // [wisp] 歌词源是宿主能力：本地与内置云端源始终可用，不随插件启停变化。
        let mut lyrics: Vec<Arc<dyn LyricsProvider>> =
            vec![Arc::new(music::local::LocalLyrics)];
        lyrics.extend(music::builtin::lyrics_sources());
        state::init(cx, io, database, providers, local_provider, lyrics, plugins, plugins_dir);
        // [wisp] 解灰设置推送到 music 侧共享配置：启动一次，之后设置变化跟随。
        {
            let settings = Wisp::global(cx).settings.clone();
            music::builtin::set_unblock_config(settings.read(cx).unblock_config());
            cx.observe(&settings, |settings, cx| {
                music::builtin::set_unblock_config(settings.read(cx).unblock_config());
            })
            .detach();
        }
        #[cfg(target_os = "windows")]
        state::install_rounded_window_hook(set_corner_preference, cx);
        let startup = Wisp::global(cx).settings.read(cx).startup().to_owned();
        let start = Screen::from_id(&startup)
            .unwrap_or(Screen::Home)
            .destination();
        router::init(start, cx);
        let (look, overrides, language, pack, stillness, pace, remembered) = {
            let settings = Wisp::global(cx).settings.read(cx);
            (
                settings.look(),
                settings.theme_overrides(),
                settings.language().to_owned(),
                settings.icons().to_owned(),
                settings.stillness(),
                settings.pace(),
                settings.system_theme(),
            )
        };
        i18n::set(i18n::resolve(&language));
        icons::set(&pack);
        ui::motion::apply(stillness, pace, cx);
        ui::refresh_primary_paste(cx);
        // linux answers late
        let reported = match cfg!(any(target_os = "linux", target_os = "freebsd")) {
            true => None,
            false => Some(ThemeKind::reported(cx)),
        };
        ThemeKind::assume(reported.unwrap_or(remembered));
        if let Some(reported) = reported.filter(|reported| *reported != remembered) {
            Wisp::global(cx)
                .settings
                .clone()
                .update(cx, |settings, cx| settings.set_system_theme(reported, cx));
        }
        ui::Theme::init(look, &overrides, cx);

        let lingers = tray::install(
            show_window,
            toggle_window,
            |cursor_x, cursor_y, cx| {
                tray_menu::open_tray_menu(
                    cursor_x,
                    cursor_y,
                    // 显示主窗口前先关掉菜单窗口，避免 activate 到菜单自己。
                    Rc::new(|cx| {
                        tray_menu::close_tray_menu(cx);
                        show_window(cx);
                    }),
                    cx,
                );
            },
            cx,
        );
        if lingers {
            cx.set_quit_mode(QuitMode::Explicit);
        }
        actions::register(lingers, cx);
        memory::watch(cx);

        open_window(cx, true);
        let session = Wisp::global(cx).session.clone();
        session.update(cx, |session, cx| session.restore(cx));

        // A cold launch with files or links goes through the same batch a hot hand-off uses, now
        // that state exists to open them into. A link waits there for its provider to sign in.
        cx.subscribe(
            &Wisp::global(cx).links.clone(),
            |_, opened: &Opened, cx| {
                if let Some(destination) = destination(&opened.0) {
                    router::navigate(destination, cx);
                }
            },
        )
        .detach();
        if !args.is_empty() {
            sender.send(args.clone()).ok();
        }

        // Commands skip the open batching, since nothing arrives in bursts there.
        let control = Wisp::global(cx).control.clone();
        cx.spawn(async move |cx| {
            while let Some(asked) = commands.recv().await {
                control.update(cx, |control, cx| match asked {
                    single::Asked::Once(request, reply) => control.answer(request, reply, cx),
                    single::Asked::Watch(watch, sink) => control.watch(watch, sink, cx),
                });
            }
        })
        .detach();

        cx.spawn(async move |cx| {
            let mut pending: Vec<String> = Vec::new();
            while let Some(items) = links.recv().await {
                // The first hand-off acts at once, the rest of the burst joins the queue.
                cx.update(|cx| follow(&items, false, cx));
                loop {
                    cx.background_executor().timer(OPEN_COALESCE).await;
                    let mut more = false;
                    while let Ok(items) = links.try_recv() {
                        pending.extend(items);
                        more = true;
                    }
                    if !more {
                        break;
                    }
                    let batch = std::mem::take(&mut pending);
                    cx.update(|cx| follow(&batch, true, cx));
                }
            }
        })
        .detach();

        cx.activate(true);
    });
    ExitCode::SUCCESS
}

/// Opens what a launch or a hand-off passed: a provider's link opens on that provider, and
/// anything else is read as a file to play. The first batch of an open starts playing at once,
/// later batches of the same open join its queue. An address no provider knows is refused with
/// a toast rather than tried as a file.
fn follow(items: &[String], tail: bool, cx: &mut App) {
    if !tail {
        show_window(cx);
    }
    let links = Wisp::global(cx).links.clone();
    let mut paths: Vec<PathBuf> = Vec::new();
    for item in items {
        if links.update(cx, |links, cx| links.open(item, cx)) {
            continue;
        }
        match cli::item::is_address(item) {
            true => Toasts::show(Outcome::Failed, "toast-link-unreadable", cx),
            false => paths.push(cli::item::local_path(item)),
        }
    }
    if !tail
        && !paths.is_empty()
        && Wisp::global(cx)
            .settings
            .read(cx)
            .now_playing_on_file_open()
    {
        router::navigate(Destination::Fullscreen, cx);
    }
    if !paths.is_empty() {
        let playback = Wisp::global(cx).playback.clone();
        playback.update(cx, |playback, cx| playback.open_paths(paths, tail, cx));
    }
}

/// The page a link opens on, for the kinds of link that open a page.
fn destination(linked: &Linked) -> Option<Destination> {
    match linked {
        Linked::Track(id) => Some(Destination::Song(id.clone().into())),
        Linked::Album(id) => Some(Destination::Album(id.clone().into())),
        Linked::Artist(id) => Some(Destination::Artist(id.clone().into())),
        Linked::Playlist(id) => Some(Destination::Playlist(id.clone().into())),
        Linked::User(id) => Some(Destination::User(id.clone().into())),
        Linked::Short => None,
    }
}

fn show_window(cx: &mut App) {
    // [wisp] mini 模式开着时，呼出主窗口即退出 mini，先把 mini 关掉。
    if views::dismiss_mini(cx) {
        cx.activate(true);
        return;
    }
    // [wisp] 只对主窗口操作：托盘菜单窗口打开时 windows().first() 会拿到它。
    let Some(main) = cx.try_global::<MainWindow>() else {
        dock::show(true);
        open_window(cx, false);
        return;
    };
    let handle = main.0.clone();
    // 主窗口被用户关闭（remove_window）后全局里还留着句柄，update 会失败——
    // 清掉句柄重新开窗，托盘的呼出才有效。
    let activated = handle.update(cx, |_, window, _| window.activate_window());
    if activated.is_err() {
        cx.remove_global::<MainWindow>();
        open_window(cx, false);
    }
    cx.activate(true);
}

/// What a tray click does to the main window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowAction {
    Show,
    Hide,
}

/// Shows the window when it is hidden, minimized or closed, and hides it when it is visible.
fn toggle_window(cx: &mut App) {
    // [wisp] mini 模式激活时（包括 mini 被暂时隐藏），托盘左击唤回 mini，而不是主窗口。
    if views::reveal_mini(cx) {
        cx.activate(true);
        return;
    }
    // and_then 的闭包会持有 cx 的借用，与里面 update 的可变借用冲突；
    // 改成 match，把 handle clone 出来后借用即结束。
    let visible = match cx.try_global::<MainWindow>() {
        Some(main) => {
            let handle = main.0.clone();
            match handle.update(cx, |_, window, _| {
                Some(window.is_visible() && !window.is_maximized())
            }) {
                Ok(visible) => visible,
                // 主窗已销毁：按不存在处理，呼出即重开。
                Err(_) => {
                    cx.remove_global::<MainWindow>();
                    None
                }
            }
        }
        None => None,
    };
    match window_action(visible) {
        WindowAction::Show => show_window(cx),
        WindowAction::Hide => {
            if let Some(main) = cx.try_global::<MainWindow>() {
                let handle = main.0.clone();
                handle
                    .update(cx, |_, window, _| window.minimize_window())
                    .ok();
            }
        }
    }
}

/// Picks the action from whether the window is visible, `None` when there is no window.
fn window_action(visible: Option<bool>) -> WindowAction {
    match visible {
        Some(true) => WindowAction::Hide,
        Some(false) | None => WindowAction::Show,
    }
}

/// The main window's handle, so the tray's show/toggle always act on the main
/// window instead of whatever window happens to be first (the tray menu itself).
pub(crate) struct MainWindow(pub AnyWindowHandle);

impl Global for MainWindow {}

fn open_window(cx: &mut App, splash: bool) {
    let Wisp {
        session,
        library,
        playback,
        queue,
        ..
    } = Wisp::global(cx);
    let (session, library, playback, queue) = (
        session.clone(),
        library.clone(),
        playback.clone(),
        queue.clone(),
    );
    let (placement, display_id) = state::window_placement(LEAST_SIZE, cx)
        .map(|(placement, display_id)| (placement, Some(display_id)))
        .unwrap_or_else(|| {
            (
                WindowBounds::Windowed(Bounds::centered(None, FIRST_SIZE, cx)),
                None,
            )
        });

    let settings = Wisp::global(cx).settings.read(cx);
    let saver = settings.saver();
    let traffic_lights = gpui::point(px(9.), px(9.));
    let look = settings.look();
    let background = ui::backdrop(look.blur_window, look.transparent);

    let handle = cx
        .open_window(
        WindowOptions {
            window_bounds: Some(placement),
            display_id,
            window_background: background,
            titlebar: Some(TitlebarOptions {
                title: Some("Wisp".into()),
                appears_transparent: true,
                traffic_light_position: Some(traffic_lights),
            }),
            inactive_frame_interval: saver.interval(),
            is_movable: true,
            is_resizable: true,
            app_owns_titlebar_drag: true,
            app_id: Some(assets::app_id().into()),
            window_min_size: Some(LEAST_SIZE),
            ..Default::default()
        },
        |window, cx| {
            window.set_rem_size(cx.theme().font_size);
            #[cfg(target_os = "windows")]
            set_corner_preference(
                window,
                Wisp::global(cx).settings.read(cx).window_rounding(),
            );
            let handle = platform_handle(window);
            state::attach_remote(handle, cx);
            #[cfg(windows)]
            if let Some(handle) = handle {
                app_icon::install(handle);
                thumbbar::install(window.window_handle().window_id(), handle, cx);
            }
            #[cfg(windows)]
            window_title::install(window.window_handle(), cx);
            state::remember_window(window, cx);
            cx.new(|cx| Root::new(session, library, playback, queue, splash, window, cx))
        },
    )
    .expect("failed to open window");
    let main = handle.into();
    cx.set_global(MainWindow(main));
    views::install_desktop_lyrics(main, cx);
}

// DWM only offers two rounded presets (no arbitrary radius), so the four `Rounding` choices
// collapse onto them: `Square` turns rounding off, `Subtle` gets the small radius, and
// `Rounded`/`Round` both get the normal one, since DWM has nothing rounder than that.
#[cfg(target_os = "windows")]
fn set_corner_preference(window: &gpui::Window, rounding: ui::Rounding) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{
        DWM_WINDOW_CORNER_PREFERENCE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
        DWMWCP_ROUND, DWMWCP_ROUNDSMALL, DwmSetWindowAttribute,
    };

    let Ok(RawWindowHandle::Win32(handle)) =
        HasWindowHandle::window_handle(window).map(|handle| handle.as_raw())
    else {
        return;
    };
    let handle = handle.hwnd.get() as *mut std::ffi::c_void;
    let preference: DWM_WINDOW_CORNER_PREFERENCE = match rounding {
        ui::Rounding::Square => DWMWCP_DONOTROUND,
        ui::Rounding::Subtle => DWMWCP_ROUNDSMALL,
        ui::Rounding::Rounded | ui::Rounding::Round => DWMWCP_ROUND,
    };
    unsafe {
        DwmSetWindowAttribute(
            handle,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &preference as *const _ as *const std::ffi::c_void,
            size_of_val(&preference) as u32,
        );
    }
}

#[cfg(target_os = "windows")]
fn platform_handle(window: &gpui::Window) -> Option<*mut std::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let RawWindowHandle::Win32(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw()
    else {
        return None;
    };
    let handle = handle.hwnd.get() as *mut std::ffi::c_void;
    hide_system_caption(handle);
    clamp_maximize(handle);
    views::install_legacy_client_area(window, true);
    Some(handle)
}

// DWM draws the caption buttons behind the client area, where an opaque window hides them
// and a transparent or blurred one shows them beside Wisp's own. They come with
// `WS_SYSMENU`, so that is the style to drop: `WS_CAPTION` has to stay, because DWM only
// animates minimize, restore and close on a window that carries it. Alt+F4 and the taskbar
// still close the window; only the Alt+Space menu goes, and Wisp's title bar has no use
// for it.
#[cfg(target_os = "windows")]
fn hide_system_caption(handle: *mut std::ffi::c_void) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WS_SYSMENU,
    };

    unsafe {
        let style = GetWindowLongPtrW(handle, GWL_STYLE);
        if style & WS_SYSMENU as isize == 0 {
            return;
        }
        SetWindowLongPtrW(handle, GWL_STYLE, style & !(WS_SYSMENU as isize));
        SetWindowPos(
            handle,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

#[cfg(target_os = "windows")]
const MAXIMIZE_SUBCLASS: usize = 1;

/// Keeps a maximized window off the taskbar, which Windows covers once `hide_system_caption`
/// drops `WS_SYSMENU`. The clamp works on the final rectangle and not in `WM_GETMINMAXINFO`,
/// because Windows only carries that size over to a larger monitor when it is at least the
/// size of the primary one.
#[cfg(target_os = "windows")]
fn clamp_maximize(handle: *mut std::ffi::c_void) {
    use windows_sys::Win32::UI::Shell::SetWindowSubclass;

    unsafe { SetWindowSubclass(handle, Some(work_area), MAXIMIZE_SUBCLASS, 0) };
}

/// Pulls each edge of a maximized window that covers its whole monitor back to that monitor's
/// work area, keeping the frame overhang Windows adds past the other edges. Fullscreen strips
/// `WS_CAPTION` but keeps `WS_MAXIMIZE`, so a window without a caption is left alone.
#[cfg(target_os = "windows")]
unsafe extern "system" fn work_area(
    handle: *mut std::ffi::c_void,
    message: u32,
    wparam: usize,
    lparam: isize,
    _subclass: usize,
    _data: usize,
) -> isize {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
    };
    use windows_sys::Win32::UI::Shell::DefSubclassProc;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, IsZoomed, SWP_NOMOVE, SWP_NOSIZE, WINDOWPOS,
        WM_WINDOWPOSCHANGING, WS_CAPTION,
    };

    unsafe {
        if message == WM_WINDOWPOSCHANGING
            && IsZoomed(handle) != 0
            && GetWindowLongPtrW(handle, GWL_STYLE) & WS_CAPTION as isize == WS_CAPTION as isize
        {
            let position = &mut *(lparam as *mut WINDOWPOS);
            let rect = RECT {
                left: position.x,
                top: position.y,
                right: position.x + position.cx,
                bottom: position.y + position.cy,
            };
            let monitor = MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST);
            let mut monitor_info: MONITORINFO = std::mem::zeroed();
            monitor_info.cbSize = size_of::<MONITORINFO>() as u32;
            if position.flags & (SWP_NOMOVE | SWP_NOSIZE) == 0
                && GetMonitorInfoW(monitor, &mut monitor_info) != 0
            {
                let (screen, work) = (monitor_info.rcMonitor, monitor_info.rcWork);
                let covers = rect.left <= screen.left
                    && rect.top <= screen.top
                    && rect.right >= screen.right
                    && rect.bottom >= screen.bottom;
                if covers {
                    position.x += work.left - screen.left;
                    position.y += work.top - screen.top;
                    position.cx -= (screen.right - screen.left) - (work.right - work.left);
                    position.cy -= (screen.bottom - screen.top) - (work.bottom - work.top);
                }
            }
        }
        DefSubclassProc(handle, message, wparam, lparam)
    }
}

#[cfg(not(target_os = "windows"))]
fn platform_handle(_window: &gpui::Window) -> Option<*mut std::ffi::c_void> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_visible_window_is_hidden() {
        assert_eq!(window_action(Some(true)), WindowAction::Hide);
    }

    #[test]
    fn a_minimized_or_closed_window_is_shown() {
        assert_eq!(window_action(Some(false)), WindowAction::Show);
        assert_eq!(window_action(None), WindowAction::Show);
    }
}
