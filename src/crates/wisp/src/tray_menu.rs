//! 自绘托盘菜单的浮层窗口。
//!
//! 窗口**常驻**：右键托盘时不新建窗口，只把它移动到光标处并显示；关闭时
//! 只是隐藏（ShowWindow(SW_HIDE)）。这样「只有一份菜单」的保证来自结构本身，
//! 不依赖销毁是否成功——原版 Tauri 的 tray-menu 也是常驻复用窗口。
//!
//! 显示与隐藏都走 Win32：gpui 的窗口开关在这个版本上依赖帧调度，交互中
//! 或窗口正忙时不可靠；ShowWindow/SetWindowPos 是同步的、立即生效的。

use gpui::{
    App, AppContext, Bounds, Entity, Global, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, point, px, size,
};
use state::Wisp;
use std::rc::Rc;
use views::TrayMenuView;
use views::{MENU_HEIGHT, MENU_WIDTH};

/// 菜单与光标的垂直间隔（原版同款）。
const GAP: f32 = 10.;

/// 常驻的菜单窗口：句柄、Win32 句柄与前台状态。
pub(crate) struct TrayMenuWindow {
    pub handle: gpui::WindowHandle<TrayMenuView>,
    /// 菜单视图的实体：重置视图状态走实体更新，不借用窗口状态。
    pub view: Entity<TrayMenuView>,
    #[cfg(target_os = "windows")]
    pub hwnd: windows_sys::Win32::Foundation::HWND,
    /// 菜单是否曾经成为系统前台窗口。Windows 的前台锁定可能让弹出的菜单
    /// 从未获得前台——这种情况下不能自动关（否则一呼出就闪退）。
    pub was_foreground: bool,
    /// 菜单在屏幕上的物理矩形（左上 x/y、宽、高），用来判断点击落在外面。
    #[cfg(target_os = "windows")]
    pub rect: (i32, i32, i32, i32),
    /// 菜单弹出时系统前台的窗口（一般是资源管理器）。之后前台换成别的
    /// 窗口，就说明用户切走了，菜单该收起。
    #[cfg(target_os = "windows")]
    pub foreground_at_show: windows_sys::Win32::Foundation::HWND,
    /// 菜单当前是否显示。轮询在菜单隐藏时待命，不退出——否则第二次
    /// 呼出就没有人盯着失焦了。
    pub visible: bool,
}

impl Global for TrayMenuWindow {}

/// 隐藏菜单窗口（常驻复用，不销毁）。
pub(crate) fn close_tray_menu(cx: &mut App) {
    let Some(window) = cx.try_global::<TrayMenuWindow>() else {
        return;
    };
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        unsafe {
            ShowWindow(window.hwnd, SW_HIDE);
        }
        // 存的句柄可能是空的（创建时 show:false，平台窗口尚未就绪）——
        // 那就等下次显示时刷新句柄；这里不再借用窗口（忙时会失败）。
    }
    #[cfg(not(target_os = "windows"))]
    {
        let handle = window.handle.clone();
        handle
            .update(cx, |_, window, cx| {
                window.remove_window();
                cx.notify();
            })
            .ok();
    }
    // 前台状态复位：下次呼出重新记。轮询任务继续待命（visible=false）。
    let mut state = cx.remove_global::<TrayMenuWindow>();
    state.was_foreground = false;
    state.visible = false;
    cx.set_global(state);
}

/// 打开（显示）托盘菜单。`cursor_x/cursor_y` 是托盘点击处的物理像素坐标。
pub(crate) fn open_tray_menu(
    cursor_x: f64,
    cursor_y: f64,
    show_main: Rc<dyn Fn(&mut App)>,
    cx: &mut App,
) {
    // 已有窗口：重置视图状态，移动到新位置并显示（不新建，绝不共存）。
    if let Some(state) = cx.try_global::<TrayMenuWindow>() {
        let handle = state.handle.clone();
        // 实体更新不借用窗口（借用会撞上渲染，报 RefCell already borrowed）。
        // 先把实体 clone 出来：try_global 的不可变借用还在，不能直接用 cx。
        let view = state.view.clone();
        view.update(cx, |view: &mut TrayMenuView, cx| view.shown(cx));
        show_at(handle, cursor_x, cursor_y, cx);
        return;
    }

    // 窗口创建挪到本帧之后：此刻栈上正持有 App 借用，建窗口会立刻触发一次
    // 绘制，绘制又要借 App —— gpui 报「RefCell already borrowed」。
    let show_main = show_main.clone();
    cx.defer(move |cx| {
        create_menu_window(cursor_x, cursor_y, show_main, cx);
    });
}

/// 建出常驻的菜单窗口（只在第一次右键时走到）。
fn create_menu_window(
    cursor_x: f64,
    cursor_y: f64,
    show_main: Rc<dyn Fn(&mut App)>,
    cx: &mut App,
) {
    // 原版定位：水平居中于光标、垂直在光标上方，按显示器钳制。
    let scale = cursor_scale(cursor_x, cursor_y);
    let cursor = point(px((cursor_x / scale) as f32), px((cursor_y / scale) as f32));
    let visible = cx
        .displays()
        .into_iter()
        .find(|display| display.visible_bounds().contains(&cursor))
        .map(|display| display.visible_bounds());
    let mut origin = point(
        cursor.x - px(MENU_WIDTH / 2.),
        cursor.y - px(MENU_HEIGHT) - px(GAP),
    );
    if let Some(visible) = visible {
        let left = visible.origin.x;
        let top = visible.origin.y;
        let right = left + visible.size.width;
        let bottom = top + visible.size.height;
        origin.x = origin.x.clamp(left, (right - px(MENU_WIDTH)).max(left));
        origin.y = origin.y.clamp(top, (bottom - px(MENU_HEIGHT)).max(top));
    }

    let wisp = Wisp::global(cx);
    let playback = wisp.playback.clone();
    let settings = wisp.settings.clone();
    let close: Rc<dyn Fn(&mut App)> = Rc::new(|cx| close_tray_menu(cx));

    let root = cx.new(|cx| TrayMenuView::new(playback, settings, show_main, close, cx));
    // 句柄在 open_window 的回调里取——那正是唯一能拿到 &mut Window 而不
    // 与渲染冲突的时刻。
    let hwnd_cell: Rc<std::cell::Cell<windows_sys::Win32::Foundation::HWND>> =
        Rc::new(std::cell::Cell::new(std::ptr::null_mut()));
    let hwnd_for_callback = hwnd_cell.clone();
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin,
                    size: size(px(MENU_WIDTH), px(MENU_HEIGHT)),
                })),
                window_background: WindowBackgroundAppearance::Transparent,
                kind: WindowKind::PopUp,
                titlebar: None,
                is_resizable: false,
                is_movable: false,
                focus: true,
                // 常驻：先创建但不显示，呼出时用 ShowWindow 显示。
                show: false,
                ..Default::default()
            },
            |window: &mut gpui::Window, _| {
                #[cfg(target_os = "windows")]
                hwnd_for_callback.set(hwnd_of(window).unwrap_or(std::ptr::null_mut()));
                root.clone()
            },
        )
        .ok();

    let Some(handle) = handle else {
        return;
    };
    #[cfg(target_os = "windows")]
    let hwnd = hwnd_cell.get();
    cx.set_global(TrayMenuWindow {
        handle: handle.clone(),
        view: root.clone(),
        #[cfg(target_os = "windows")]
        hwnd,
        was_foreground: false,
        #[cfg(target_os = "windows")]
        rect: (0, 0, 0, 0),
        #[cfg(target_os = "windows")]
        foreground_at_show: std::ptr::null_mut(),
        visible: false,
    });

    show_at(handle, cursor_x, cursor_y, cx);
    watch_blur(handle, cx);
}

/// 把常驻窗口移到光标处并显示，同时抢到前台。
fn show_at(
    handle: gpui::WindowHandle<TrayMenuView>,
    cursor_x: f64,
    cursor_y: f64,
    cx: &mut App,
) {
    let scale = cursor_scale(cursor_x, cursor_y);
    let cursor = point(px((cursor_x / scale) as f32), px((cursor_y / scale) as f32));
    let visible = cx
        .displays()
        .into_iter()
        .find(|display| display.visible_bounds().contains(&cursor))
        .map(|display| display.visible_bounds());
    let mut origin = point(
        cursor.x - px(MENU_WIDTH / 2.),
        cursor.y - px(MENU_HEIGHT) - px(GAP),
    );
    if let Some(visible) = visible {
        let left = visible.origin.x;
        let top = visible.origin.y;
        let right = left + visible.size.width;
        let bottom = top + visible.size.height;
        origin.x = origin.x.clamp(left, (right - px(MENU_WIDTH)).max(left));
        origin.y = origin.y.clamp(top, (bottom - px(MENU_HEIGHT)).max(top));
    }
    let physical = |value: f32| ((value as f64 * scale) as i32) as i32;
    let rect = (
        physical(f64::from(origin.x) as f32),
        physical(f64::from(origin.y) as f32),
        physical(MENU_WIDTH),
        physical(MENU_HEIGHT),
    );

    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SW_SHOW,
        };
        // 句柄在全局里：显示全程不借用 gpui 窗口（借用会撞上渲染并报错）。
        let hwnd = cx
            .try_global::<TrayMenuWindow>()
            .map(|state| state.hwnd)
            .unwrap_or(std::ptr::null_mut());
        if !hwnd.is_null() {
            // Win32 调用挪出 gpui 的调用栈：ShowWindow/SetForegroundWindow 会
            // 立刻触发窗口过程，若此刻正在绘制就会重入，gpui 会记借用错误。
            // HWND 是裸指针（不 Send）：用地址整数带进任务，进去再转回。
            let hwnd_addr = hwnd as usize;
            cx.background_executor()
                .spawn(async move {
                    let hwnd = hwnd_addr as windows_sys::Win32::Foundation::HWND;
                    unsafe {
                        // 尺寸一起给：常驻窗口创建时还没定尺寸，只改位置会让它
                        // 保持默认的全屏大小，菜单于是铺满屏幕。
                        SetWindowPos(
                            hwnd,
                            HWND_TOPMOST,
                            rect.0,
                            rect.1,
                            rect.2,
                            rect.3,
                            SWP_NOACTIVATE,
                        );
                        ShowWindow(hwnd, SW_SHOW);
                    }
                    // 抢前台：解除 Windows 的前台锁，否则菜单拿不到焦点。
                    force_foreground(hwnd);
                })
                .detach();
            // 记下菜单的物理矩形与弹出时的前台窗口：点击落在外面、
            // 或前台换成别的窗口，都据此收起。
            let mut state = cx.remove_global::<TrayMenuWindow>();
            state.rect = rect;
            state.foreground_at_show = current_foreground();
            state.visible = true;
            cx.set_global(state);
            return;
        }
    }

    // 句柄还没就绪（极少见）：这次先不显示，等下一次呼出时再试——
    // 这里不借用窗口，避免与渲染撞车。
    let _ = handle;
}

/// 监视前台窗口：菜单失去前台就隐藏。
///
/// 判定不用 gpui 的 active 标志（PopUp 上不可靠），直接问 Win32；且只有
/// 「曾经在前台、现在不在」才隐藏——从未成为前台（前台锁定）时保持打开。
fn watch_blur(handle: gpui::WindowHandle<TrayMenuView>, cx: &mut App) {
    let tick: gpui::Task<anyhow::Result<()>> = cx.spawn(async move |cx| loop {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(150))
            .await;
        // 轮询里的 cx 是 AsyncApp，对全局/窗口的操作要进 cx.update 拿到 &mut App。
        let done = cx.update(|cx| {
            // 只认自己打开的那扇窗：换了窗口就退场。
            let Some(state) = cx.try_global::<TrayMenuWindow>() else {
                return true;
            };
            if state.handle != handle {
                return true;
            }
            // 菜单隐藏时待命：任务要留着，第二次呼出还得靠它盯着失焦。
            if !state.visible {
                return false;
            }
            let was_foreground = state.was_foreground;
            #[cfg(target_os = "windows")]
            let foreground_at_show = state.foreground_at_show;
            #[cfg(target_os = "windows")]
            let menu_hwnd = state.hwnd;
            // 直接用 Win32 句柄判前台，不去借用 gpui 窗口：轮询撞上渲染/
            // 事件处理时 update 会失败并刷一串「RefCell already borrowed」。
            #[cfg(target_os = "windows")]
            let foreground = foreground_is_menu(state.hwnd);
            #[cfg(not(target_os = "windows"))]
            let foreground = true;
            let mut state = cx.remove_global::<TrayMenuWindow>();
            state.was_foreground = foreground || was_foreground;
            // 点击落在菜单外面（且这一拍刚好按下左键）就隐藏：托盘点击后
            // 本进程往往拿不到前台，光靠失焦事件永远等不到。
            #[cfg(target_os = "windows")]
            {
                if click_outside(state.rect) {
                    cx.set_global(state);
                    close_tray_menu(cx);
                    return false;
                }
            }
            cx.set_global(state);
            // 前台换成了第三个窗口（既不是菜单、也不是弹出时的那个）：
            // 用户切走了，菜单收起。
            #[cfg(target_os = "windows")]
            if !foreground {
                let now = current_foreground();
                if !now.is_null()
                    && now != menu_hwnd
                    && now != foreground_at_show
                {
                    close_tray_menu(cx);
                    return false;
                }
            }
            match (foreground, was_foreground) {
                (true, _) => false,
                (false, true) => {
                    // 曾经在前台、现在不在：失焦，隐藏。
                    close_tray_menu(cx);
                    false
                }
                (false, false) => false,
            }
        });
        if done {
            return Ok(());
        }
    });
    // 轮询任务的生命周期就是菜单窗口本身；全局被清后 tick 自行退出。
    std::mem::forget(tick);
}

/// 窗口的 Win32 句柄。
#[cfg(target_os = "windows")]
fn hwnd_of(window: &gpui::Window) -> Option<windows_sys::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(RawWindowHandle::Win32(raw)) = HasWindowHandle::window_handle(window).map(|h| h.as_raw())
    else {
        return None;
    };
    Some(raw.hwnd.get() as windows_sys::Win32::Foundation::HWND)
}

/// 前台窗口是否就是这个窗口（Win32 直查，绕开 gpui 的 active 跟踪）。
#[cfg(target_os = "windows")]
fn foreground_is_menu(hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
    if hwnd.is_null() {
        return true;
    }
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() == hwnd }
}

/// 把菜单窗口推到前台。托盘点击后本进程多半是后台，Windows 的前台锁会拦下
/// SetForegroundWindow——用 AttachThreadInput 把输入线程临时接到当前前台
/// 线程解锁（Win32 经典做法）。成了前台，轮询的「曾在前台→不在才关」才走得通。
#[cfg(target_os = "windows")]
fn force_foreground(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
    };

    if hwnd.is_null() {
        return;
    }
    unsafe {
        let foreground = GetForegroundWindow();
        let mut foreground_process = 0u32;
        let foreground_thread = GetWindowThreadProcessId(foreground, &mut foreground_process);
        let our_thread = GetCurrentThreadId();
        let attached = foreground_thread != 0
            && foreground_thread != our_thread
            && AttachThreadInput(our_thread, foreground_thread, 1) != 0;
        SetForegroundWindow(hwnd);
        if attached {
            AttachThreadInput(our_thread, foreground_thread, 0);
        }
    }
}

/// 光标所在显示器的缩放系数（物理/逻辑换算用），Win32 直查，拿不到按 1。
#[cfg(target_os = "windows")]
fn cursor_scale(cursor_x: f64, cursor_y: f64) -> f64 {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

    let cursor = POINT {
        x: cursor_x as i32,
        y: cursor_y as i32,
    };
    unsafe {
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        if dpi_x == 0 {
            1.
        } else {
            dpi_x as f64 / 96.
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn cursor_scale(_cursor_x: f64, _cursor_y: f64) -> f64 {
    1.
}

/// 左键这一拍是否刚按下（边沿），以及光标是否落在菜单矩形之外。
#[cfg(target_os = "windows")]
fn click_outside(rect: (i32, i32, i32, i32)) -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    unsafe {
        // 低位（0x0001）表示「自上次查询以来这个键被按下过」，调用即清除。
        // 高位（0x8000）是当前按住状态——用它做边沿会漏掉按下与抬起都在
        // 两次轮询之间的快速点击，菜单就再也关不掉了。
        let clicked = (GetAsyncKeyState(VK_LBUTTON as i32) as u16) & 0x0001 != 0;
        if !clicked {
            return false;
        }
        let mut cursor = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut cursor) == 0 {
            return false;
        }
        let (x, y, width, height) = rect;
        cursor.x < x || cursor.y < y || cursor.x > x + width || cursor.y > y + height
    }
}

/// 当前系统前台的窗口句柄。
#[cfg(target_os = "windows")]
fn current_foreground() -> windows_sys::Win32::Foundation::HWND {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }
}
