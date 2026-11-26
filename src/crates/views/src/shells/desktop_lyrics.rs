//! The desktop lyrics window: a transparent, always-on-top strip that follows playback.
//!
//! It is its own tool window, so it stays above other applications and out of the taskbar,
//! and it never takes focus when it opens. The settings toggle is the only source of truth:
//! the installer watches it and opens or closes this window accordingly.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App, Bounds, Context, CursorStyle,
    Entity, FontWeight, Global, Hsla, IntoElement, MouseButton, MouseMoveEvent, Render,
    SharedString, Task, TextShadow, Window, WindowBackgroundAppearance, WindowBounds,
    WindowControlArea, WindowKind, WindowOptions, black, div, point, px, relative, size, white,
};
use i18n::t;
use music::{Lyrics as MusicLyrics, LyricsLine, LyricsWord};
use state::{AppSettings, Io, Lyrics, Playback, PlaybackState, Wisp};
use ui::Button;

use crate::chrome::stroke_lyrics::{
    FlyStroke, Hanzi, Pose, STROKE_SCATTER_MS, StrokeCell, Timing, fetch_hanzi, fit_timeline,
    flatten_line, fly_strokes, instant_line, is_strokable, next_random,
};

use crate::shared::transport::{next, previous, toggle};
use crate::shared::typeface;

const WIDTH: f32 = 1000.;
/// Vertical windows trade the wide strip for a tall one, two columns wide.
const VERTICAL_HEIGHT: f32 = 620.;
/// Room above the taskbar for the window's bottom edge.
const MARGIN: f32 = 56.;
const HEADER: f32 = 48.;
const EDGE: f32 = 16.;
const RADIUS: f32 = 12.;
const MIN_FONT: f32 = 20.;
const MAX_FONT: f32 = 96.;
/// Line slot height, from the reference.
const SLOT_RATIO: f32 = 1.35;
/// The window height, horizontal, and width, vertical, that show the smallest and the largest
/// lyrics. The window is the one size control: its sizing axis maps linearly onto the font,
/// taken from EchoMusic, and the other axis only decides how much of a line fits.
const STRIP_MIN: f32 = 140.;
const STRIP_MAX: f32 = 360.;
const COLUMN_MIN: f32 = 120.;
const COLUMN_MAX: f32 = 520.;
/// The smallest the window may be dragged to, in logical pixels.
const WINDOW_MIN: f32 = 80.;
/// The corner a locked strip watches for the pointer: it covers the unlock bubble at any
/// interface size, and its inset matches where the bubble sits.
const UNLOCK: f32 = 40.;
const UNLOCK_INSET: f32 = 8.;
/// The grab zones along the edges, the bigger corners that take precedence, and the inset that
/// keeps the two apart. A transparent window cannot wear a native sizing frame, so the strip
/// resizes itself from these.
const HANDLE: f32 = 8.;
const CORNER: f32 = 14.;
const HANDLE_INSET: f32 = 12.;
/// The pair slides out over this long after its second line ends.
const SWAP: Duration = Duration::from_millis(220);
/// The dark plate that fades in under the lyrics on hover, and the heavier one that backs
/// the strip while it is being moved or resized.
const HOVER_ALPHA: f32 = 0.6;
const INTERACT_ALPHA: f32 = 0.7;
const HOVER_FADE: Duration = Duration::from_millis(300);
/// How often the pointer is checked against the window, catching leaves the hover events miss.
const HOVER_POLL: Duration = Duration::from_millis(200);
/// One breath of the interlude dots.
const BREATH: Duration = Duration::from_millis(2400);
/// Repaint cadence for the word sweep and the drag resize, one frame per tick.
const SWEEP_FRAME: Duration = Duration::from_millis(16);
const SWEEP_LEAST: Duration = Duration::from_millis(180);
const SWEEP_STRETCH: f32 = 1.4;
const SWEPT: f32 = 0.98;
/// How much of a word the leading edge fades over, relative to its text size.
const SWEEP_EDGE: f32 = 0.25;
/// How bright a character stays before the sweep reaches it, so the fill reads as it grows.
const UNSUNG: f32 = 0.32;

#[derive(Clone, Copy)]
struct MainHost(AnyWindowHandle);

impl Global for MainHost {}

#[derive(Clone, Copy)]
struct DesktopWindow(AnyWindowHandle);

impl Global for DesktopWindow {}

struct Installed;

impl Global for Installed {}

/// Watches the desktop-lyrics setting and opens or closes the window. The main window's handle
/// places the strip on the display the app is on.
pub fn install(main: AnyWindowHandle, cx: &mut App) {
    cx.set_global(MainHost(main));
    if cx.try_global::<Installed>().is_some() {
        sync(cx);
        return;
    }
    cx.set_global(Installed);
    let settings = Wisp::global(cx).settings.clone();
    cx.observe(&settings, |_, cx| {
        // The setting can be changed from this window, which is already on the update stack.
        cx.defer(sync);
    })
    .detach();
    sync(cx);
}

fn sync(cx: &mut App) {
    let on = Wisp::global(cx).settings.read(cx).desktop_lyrics();
    match (on, cx.try_global::<DesktopWindow>().copied()) {
        (true, None) => open(cx),
        (false, Some(window)) => {
            cx.remove_global::<DesktopWindow>();
            window
                .0
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        }
        _ => {}
    }
}

fn open(cx: &mut App) {
    let Some(host) = cx.try_global::<MainHost>().copied() else {
        return;
    };
    let playback = Wisp::global(cx).playback.clone();
    let lyrics = Wisp::global(cx).lyrics.clone();
    let settings = Wisp::global(cx).settings.clone();
    let font = settings.read(cx).desktop_lyrics_font_size();
    let vertical = settings.read(cx).desktop_lyrics_vertical();
    let height = design_height(font, vertical);

    let frame = host.0.update(cx, |_, window, _| window.bounds()).ok();
    let displays = cx.displays();
    let area = frame
        .as_ref()
        .and_then(|frame| {
            displays
                .iter()
                .find(|display| display.visible_bounds().contains(&frame.center()))
                .map(|display| display.visible_bounds())
        })
        .or_else(|| displays.first().map(|display| display.visible_bounds()));

    let (origin, size) = match area {
        Some(area) => {
            let available = area.size.width.as_f32();
            let width = design_width(font, vertical).min((available - 64.).max(320.));
            let height = height.min(area.size.height.as_f32());
            let x = area.origin.x.as_f32() + (available - width) / 2.;
            let y = (area.origin.y.as_f32() + area.size.height.as_f32() - height - MARGIN)
                .max(area.origin.y.as_f32());
            (point(px(x), px(y)), size(px(width), px(height)))
        }
        None => (
            point(px(100.), px(100.)),
            size(px(design_width(font, vertical)), px(height)),
        ),
    };
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size })),
            window_background: WindowBackgroundAppearance::Transparent,
            kind: WindowKind::PopUp,
            titlebar: None,
            is_resizable: true,
            is_minimizable: false,
            is_movable: true,
            app_owns_titlebar_drag: true,
            focus: false,
            show: true,
            // This window animates while another app is focused, so it must not take
            // the default inactive-window frame cap.
            inactive_frame_interval: None,
            ..Default::default()
        },
        |window, cx| {
            window.on_window_should_close(cx, {
                let settings = settings.clone();
                move |_, cx| {
                    clear_moving();
                    settings.update(cx, |settings, cx| settings.set_desktop_lyrics(false, cx));
                    cx.remove_global::<DesktopWindow>();
                    true
                }
            });
            cx.new(|cx| DesktopLyric::new(playback, lyrics, settings, cx))
        },
    );
    if let Ok(handle) = opened {
        cx.set_global(DesktopWindow(handle.into()));
    }
}

#[cfg(target_os = "windows")]
fn hwnd(window: &Window) -> Option<*mut std::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(handle.hwnd.get() as *mut std::ffi::c_void)
}

/// Hides the DWM frame, which otherwise draws an inactive border and shadow around the
/// transparent strip, and watches the window's move loop so the view can show its plate.
#[cfg(target_os = "windows")]
fn hide_frame(window: &Window) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_NCRENDERING_POLICY,
        DwmSetWindowAttribute,
    };
    use windows_sys::Win32::UI::Shell::SetWindowSubclass;
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    clear_moving();
    let policy = DWMNCRP_DISABLED;
    let border = DWMWA_COLOR_NONE;
    unsafe {
        SetWindowSubclass(hwnd, Some(frame_subclass), FRAME_SUBCLASS, 0);
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_NCRENDERING_POLICY as u32,
            &policy as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&policy) as u32,
        );
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as u32,
            &border as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&border) as u32,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn hide_frame(_window: &Window) {}

/// Set while Windows runs the move loop, so the view can show its plate and outline.
#[cfg(target_os = "windows")]
static MOVING: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
fn moving() -> bool {
    MOVING.load(Ordering::Relaxed)
}

#[cfg(not(target_os = "windows"))]
fn moving() -> bool {
    false
}

#[cfg(target_os = "windows")]
fn clear_moving() {
    MOVING.store(false, Ordering::Relaxed);
}

#[cfg(not(target_os = "windows"))]
fn clear_moving() {}

#[cfg(target_os = "windows")]
const FRAME_SUBCLASS: usize = 1;

/// The private message that hands the window loop a new rect. Sizing straight from a gpui
/// update would re-enter the app from the resize callback it provokes, which the app cannot
/// take, so the drag leaves the rect here and lets the message loop apply it instead.
#[cfg(target_os = "windows")]
const RESIZE_MESSAGE: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 1;

#[cfg(target_os = "windows")]
static PENDING_RECT: std::sync::Mutex<Option<(i32, i32, i32, i32)>> = std::sync::Mutex::new(None);

/// Keeps the client area over the whole window, reports the move loop, and applies the rects
/// a drag hands over. Sizing is the view's own: setting `WS_THICKFRAME` would hand the drag to
/// the OS, whose modal loop stutters on a layered window and whose frame the strip does not
/// want.
#[cfg(target_os = "windows")]
unsafe extern "system" fn frame_subclass(
    handle: *mut std::ffi::c_void,
    message: u32,
    wparam: usize,
    lparam: isize,
    _subclass: usize,
    _data: usize,
) -> isize {
    use windows_sys::Win32::UI::Shell::DefSubclassProc;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE,
        WM_NCCALCSIZE,
    };

    unsafe {
        match message {
            WM_ENTERSIZEMOVE => {
                MOVING.store(true, Ordering::Relaxed);
                DefSubclassProc(handle, message, wparam, lparam)
            }
            WM_EXITSIZEMOVE => {
                MOVING.store(false, Ordering::Relaxed);
                DefSubclassProc(handle, message, wparam, lparam)
            }
            WM_NCCALCSIZE if wparam != 0 => 0,
            RESIZE_MESSAGE => {
                let rect = PENDING_RECT
                    .lock()
                    .ok()
                    .and_then(|mut pending| pending.take());
                if let Some((x, y, width, height)) = rect {
                    SetWindowPos(
                        handle,
                        std::ptr::null_mut(),
                        x,
                        y,
                        width,
                        height,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    );
                }
                0
            }
            _ => DefSubclassProc(handle, message, wparam, lparam),
        }
    }
}

#[cfg(target_os = "windows")]
fn window_host(window: &Window) -> Option<usize> {
    hwnd(window).map(|handle| handle as usize)
}

#[cfg(not(target_os = "windows"))]
fn window_host(_window: &Window) -> Option<usize> {
    None
}

#[cfg(target_os = "windows")]
fn cursor_over_window(host: usize) -> bool {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};
    let mut point = POINT::default();
    let mut rect = RECT::default();
    unsafe {
        GetCursorPos(&mut point) != 0
            && GetWindowRect(host as *mut std::ffi::c_void, &mut rect) != 0
            && point.x >= rect.left
            && point.x < rect.right
            && point.y >= rect.top
            && point.y < rect.bottom
    }
}

#[cfg(not(target_os = "windows"))]
fn cursor_over_window(_host: usize) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn left_button_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    unsafe { (GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000) != 0 }
}

#[cfg(not(target_os = "windows"))]
fn left_button_down() -> bool {
    false
}

/// The pointer in screen pixels, which do not shift as the window follows a resize drag.
#[cfg(target_os = "windows")]
fn cursor_position() -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut point = POINT::default();
    unsafe { (GetCursorPos(&mut point) != 0).then_some((point.x, point.y)) }
}

/// The window rect in screen pixels, as left, top, right and bottom.
#[cfg(target_os = "windows")]
fn window_rect(window: &Window) -> Option<(i32, i32, i32, i32)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let handle = hwnd(window)?;
    let mut rect = RECT::default();
    unsafe {
        (GetWindowRect(handle, &mut rect) != 0).then_some((
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
        ))
    }
}

/// Whether the pointer is over the unlock bubble in the locked strip's top right corner.
#[cfg(target_os = "windows")]
fn cursor_over_unlock(host: usize) -> bool {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};
    let mut point = POINT::default();
    let mut rect = RECT::default();
    let scale = unsafe { GetDpiForWindow(host as *mut std::ffi::c_void) } as f32 / 96.;
    let inner = (UNLOCK_INSET * scale).round() as i32;
    let outer = ((UNLOCK + UNLOCK_INSET) * scale).round() as i32;
    unsafe {
        GetCursorPos(&mut point) != 0
            && GetWindowRect(host as *mut std::ffi::c_void, &mut rect) != 0
            && point.x >= rect.right - outer
            && point.x < rect.right - inner
            && point.y >= rect.top + inner
            && point.y < rect.top + outer
    }
}

#[cfg(not(target_os = "windows"))]
fn cursor_over_unlock(_host: usize) -> bool {
    false
}

/// Makes the strip ignore the mouse, so a click falls through to whatever is behind it, or
/// takes that back while the pointer is on the unlock bubble.
#[cfg(target_os = "windows")]
fn set_click_through(window: &Window, on: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_TRANSPARENT,
    };
    let Some(handle) = hwnd(window) else {
        return;
    };
    unsafe {
        let style = GetWindowLongPtrW(handle, GWL_EXSTYLE);
        let wanted = match on {
            true => style | WS_EX_TRANSPARENT as isize,
            false => style & !(WS_EX_TRANSPARENT as isize),
        };
        if style != wanted {
            SetWindowLongPtrW(handle, GWL_EXSTYLE, wanted);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn set_click_through(_window: &Window, _on: bool) {}

/// Where forwarded mouse moves go while the strip is locked, and the hook that forwards them.
/// A click-through window gets no input of its own, so moves are posted in from a global hook
/// instead, the way Electron forwards them for EchoMusic: hover keeps working while clicks
/// fall through.
#[cfg(target_os = "windows")]
static MOVE_TARGET: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
#[cfg(target_os = "windows")]
static MOVE_HOOK: std::sync::Mutex<Option<isize>> = std::sync::Mutex::new(None);

#[cfg(target_os = "windows")]
unsafe extern "system" fn forward_mouse_move(code: i32, wparam: usize, lparam: isize) -> isize {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetClientRect, MSLLHOOKSTRUCT, PostMessageW, WM_MOUSEMOVE,
    };
    if code >= 0
        && wparam as u32 == WM_MOUSEMOVE
        && let Some(handle) = MOVE_TARGET.lock().ok().and_then(|target| *target)
    {
        let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        let mut point = info.pt;
        let mut rect = RECT::default();
        unsafe {
            if ScreenToClient(handle as *mut std::ffi::c_void, &mut point) != 0
                && GetClientRect(handle as *mut std::ffi::c_void, &mut rect) != 0
                && point.x >= 0
                && point.y >= 0
                && point.x < rect.right
                && point.y < rect.bottom
            {
                let packed = ((point.y as u32 & 0xffff) << 16) | (point.x as u32 & 0xffff);
                PostMessageW(
                    handle as *mut std::ffi::c_void,
                    WM_MOUSEMOVE,
                    0,
                    packed as isize,
                );
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

#[cfg(target_os = "windows")]
fn start_move_forwarding(window: &Window) {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_MOUSE_LL};
    let Some(handle) = hwnd(window) else {
        return;
    };
    let Ok(mut hook) = MOVE_HOOK.lock() else {
        return;
    };
    if hook.is_some() {
        return;
    }
    if let Ok(mut target) = MOVE_TARGET.lock() {
        *target = Some(handle as usize);
    }
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let installed = SetWindowsHookExW(WH_MOUSE_LL, Some(forward_mouse_move), module, 0);
        if installed.is_null() {
            log::warn!("desktop-lyrics: cannot forward mouse moves to the locked strip");
            if let Ok(mut target) = MOVE_TARGET.lock() {
                *target = None;
            }
        } else {
            *hook = Some(installed as isize);
        }
    }
}

#[cfg(target_os = "windows")]
fn stop_move_forwarding() {
    use windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx;
    let Ok(mut hook) = MOVE_HOOK.lock() else {
        return;
    };
    if let Some(installed) = hook.take() {
        unsafe { UnhookWindowsHookEx(installed as *mut std::ffi::c_void) };
    }
    if let Ok(mut target) = MOVE_TARGET.lock() {
        *target = None;
    }
}

/// Moves and resizes the window without activating it, which is what keeps the strip out of
/// the way while it is dragged. The rect is applied by the window loop, never from here, so
/// the resize callback it provokes finds the app free.
#[cfg(target_os = "windows")]
fn post_window_rect(window: &Window, rect: (i32, i32, i32, i32)) {
    use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;
    let Some(handle) = hwnd(window) else {
        return;
    };
    if let Ok(mut pending) = PENDING_RECT.lock() {
        *pending = Some(rect);
    }
    unsafe {
        PostMessageW(handle, RESIZE_MESSAGE, 0, 0);
    }
}

/// Maps a position along the window's sizing axis onto a lyric size, and back, linearly
/// between the two design ends. Rounding both ways keeps the size a step and the window it
/// designs for telling the same story.
fn font_for(size: f32, from: f32, to: f32) -> f32 {
    let share = ((size - from) / (to - from)).clamp(0., 1.);
    (MIN_FONT + share * (MAX_FONT - MIN_FONT)).round()
}

fn size_for(font: f32, from: f32, to: f32) -> f32 {
    let share = ((font - MIN_FONT) / (MAX_FONT - MIN_FONT)).clamp(0., 1.);
    (from + share * (to - from)).round()
}

fn design_width(font: f32, vertical: bool) -> f32 {
    match vertical {
        true => size_for(font, COLUMN_MIN, COLUMN_MAX),
        false => WIDTH,
    }
}

/// The window height a font size designs for, horizontal, or the tall column a vertical strip
/// keeps whatever its width.
fn design_height(font: f32, vertical: bool) -> f32 {
    match vertical {
        true => VERTICAL_HEIGHT,
        false => size_for(font, STRIP_MIN, STRIP_MAX),
    }
}

/// The lyric size the window shows: its height when horizontal, its width when vertical. The
/// other axis only decides how much of a line fits at that size.
fn window_font(width: f32, height: f32, vertical: bool) -> f32 {
    match vertical {
        true => font_for(width, COLUMN_MIN, COLUMN_MAX),
        false => font_for(height, STRIP_MIN, STRIP_MAX),
    }
}

/// Resizes the strip for a new font size, keeping its width in the horizontal layout and its
/// height in the vertical one. Turning the strip re-fits both axes; the axis the lyrics do not
/// read stays as the user dragged it.
fn fit_to_shape(window: &mut Window, font: f32, vertical: bool, turned: bool, cx: &App) {
    let bounds = window.bounds();
    let (width, height) = match (vertical, turned) {
        (true, true) => (design_width(font, true), VERTICAL_HEIGHT),
        (true, false) => (design_width(font, true), bounds.size.height.as_f32()),
        (false, true) => (WIDTH, design_height(font, false)),
        (false, false) => (bounds.size.width.as_f32(), design_height(font, false)),
    };
    place_window(window, width, height, cx);
}

fn place_window(window: &mut Window, width: f32, height: f32, cx: &App) {
    let displays = cx.displays();
    let area = displays
        .iter()
        .find(|display| display.visible_bounds().contains(&window.bounds().center()))
        .or_else(|| displays.first())
        .map(|display| display.visible_bounds());
    let (width, height) = match area {
        Some(area) => (
            width.min((area.size.width.as_f32() - 64.).max(320.)),
            height.min(area.size.height.as_f32()),
        ),
        None => (width, height),
    };
    let bounds = window.bounds();
    if (bounds.size.width.as_f32() - width).abs() < 1.
        && (bounds.size.height.as_f32() - height).abs() < 1.
    {
        return;
    }
    let centre = bounds.center();
    let (mut x, mut y) = (
        centre.x.as_f32() - width / 2.,
        centre.y.as_f32() - height / 2.,
    );
    if let Some(area) = area {
        let left = area.origin.x.as_f32();
        let top = area.origin.y.as_f32();
        let right = left + area.size.width.as_f32();
        let bottom = top + area.size.height.as_f32();
        x = x.clamp(left, (right - width).max(left));
        y = y.clamp(top, (bottom - height).max(top));
    }
    #[cfg(target_os = "windows")]
    {
        let scale = window.scale_factor();
        post_window_rect(
            window,
            (
                (x * scale).round() as i32,
                (y * scale).round() as i32,
                (width * scale).round() as i32,
                (height * scale).round() as i32,
            ),
        );
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (x, y);
        window.resize(size(px(width), px(height)));
    }
}

#[derive(Clone, Copy)]
struct Swap {
    from: usize,
    started: Instant,
}

/// Which edge or corner a resize drag moves.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResizeDir {
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    TopLeft,
}

impl ResizeDir {
    const ALL: [Self; 8] = [
        Self::Top,
        Self::TopRight,
        Self::Right,
        Self::BottomRight,
        Self::Bottom,
        Self::BottomLeft,
        Self::Left,
        Self::TopLeft,
    ];

    #[cfg(target_os = "windows")]
    fn west(self) -> bool {
        matches!(self, Self::Left | Self::TopLeft | Self::BottomLeft)
    }

    #[cfg(target_os = "windows")]
    fn east(self) -> bool {
        matches!(self, Self::Right | Self::TopRight | Self::BottomRight)
    }

    #[cfg(target_os = "windows")]
    fn north(self) -> bool {
        matches!(self, Self::Top | Self::TopLeft | Self::TopRight)
    }

    #[cfg(target_os = "windows")]
    fn south(self) -> bool {
        matches!(self, Self::Bottom | Self::BottomLeft | Self::BottomRight)
    }

    fn cursor(self) -> CursorStyle {
        match self {
            Self::Top | Self::Bottom => CursorStyle::ResizeUpDown,
            Self::Left | Self::Right => CursorStyle::ResizeLeftRight,
            Self::TopLeft | Self::BottomRight => CursorStyle::ResizeUpLeftDownRight,
            Self::TopRight | Self::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn edge(self) -> gpui::ResizeEdge {
        use gpui::ResizeEdge;
        match self {
            Self::Top => ResizeEdge::Top,
            Self::TopRight => ResizeEdge::TopRight,
            Self::Right => ResizeEdge::Right,
            Self::BottomRight => ResizeEdge::BottomRight,
            Self::Bottom => ResizeEdge::Bottom,
            Self::BottomLeft => ResizeEdge::BottomLeft,
            Self::Left => ResizeEdge::Left,
            Self::TopLeft => ResizeEdge::TopLeft,
        }
    }
}

/// A drag on one of the strip's edges: where the pointer and the window were when it started,
/// in screen pixels, so the window can follow the pointer exactly while the strip resizes.
#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
struct ResizeDrag {
    dir: ResizeDir,
    cursor: (i32, i32),
    /// Left, top, right and bottom.
    rect: (i32, i32, i32, i32),
    applied: (i32, i32, i32, i32),
}

#[cfg(target_os = "windows")]
impl ResizeDrag {
    /// The window rect a drag to `cursor` calls for, never smaller than the least size.
    fn rect_at(self, cursor: (i32, i32), scale: f32) -> (i32, i32, i32, i32) {
        let (left, top, right, bottom) = self.rect;
        let least = (WINDOW_MIN * scale).round() as i32;
        let dx = cursor.0 - self.cursor.0;
        let dy = cursor.1 - self.cursor.1;
        let width = match (self.dir.east(), self.dir.west()) {
            (true, _) => (right - left + dx).max(least),
            (_, true) => (right - left - dx).max(least),
            _ => right - left,
        };
        let height = match (self.dir.south(), self.dir.north()) {
            (true, _) => (bottom - top + dy).max(least),
            (_, true) => (bottom - top - dy).max(least),
            _ => bottom - top,
        };
        let x = match self.dir.west() {
            true => right - width,
            false => left,
        };
        let y = match self.dir.north() {
            true => bottom - height,
            false => top,
        };
        (x, y, width, height)
    }
}

struct DesktopLyric {
    playback: Entity<Playback>,
    lyrics: Entity<Lyrics>,
    settings: Entity<AppSettings>,
    framed: bool,
    host: Option<usize>,
    sweeping: Option<Task<()>>,
    /// The pair of lines on stage. It holds still while the second line sings.
    pair: Option<usize>,
    swap: Option<Swap>,
    track: Option<String>,
    fit: Option<(f32, bool)>,
    /// When the strip opened, which is where the interlude dots' breath starts.
    dots_at: Instant,
    #[cfg(target_os = "windows")]
    resize: Option<ResizeDrag>,
    #[cfg(target_os = "windows")]
    resize_task: Option<Task<()>>,
    #[cfg(target_os = "windows")]
    over_unlock: bool,
    #[cfg(target_os = "windows")]
    unlock_hot: bool,
    #[cfg(target_os = "windows")]
    click_through: Option<bool>,
    #[cfg(target_os = "windows")]
    forwarding: bool,
    hovering: bool,
    hover_changed: bool,
    hover_epoch: u32,
    stroke: Option<StrokePair>,
    stroke_pending: Option<usize>,
    cover: Option<String>,
    accent: Option<Hsla>,
    accent_task: Option<Task<()>>,
    hanzi: HashMap<char, Option<Arc<Hanzi>>>,
    fetching: HashSet<char>,
    stroke_tasks: Vec<Task<()>>,
}

struct StrokePair {
    first: usize,
    lines: [StrokeLine; 2],
    out_at: Option<Instant>,
}

struct StrokeLine {
    chars: Vec<StrokeChar>,
    instant: bool,
    fly_budget: f64,
}

struct StrokeChar {
    timing: Timing,
    data: Option<Arc<Hanzi>>,
    no_data: bool,
    fly: Option<StrokeFly>,
    assembled: bool,
    scatter: Option<StrokeScatter>,
    fallback_at: Option<Instant>,
}

struct StrokeFly {
    start: Instant,
    strokes: Vec<FlyStroke>,
}

struct StrokeScatter {
    start: Instant,
    poses: Vec<Pose>,
}

impl DesktopLyric {
    fn new(
        playback: Entity<Playback>,
        lyrics: Entity<Lyrics>,
        settings: Entity<AppSettings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let ticks = playback.read(cx).ticks().clone();
        cx.observe(&ticks, |_, _, cx| cx.notify()).detach();
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
        cx.observe(&lyrics, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        // The window can move out from under the pointer in the middle of a drag, so its
        // leave message is not enough to keep hover honest.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(HOVER_POLL).await;
                if this.update(cx, |this, cx| this.poll_hover(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            playback,
            lyrics,
            settings,
            framed: false,
            host: None,
            sweeping: None,
            pair: None,
            swap: None,
            track: None,
            fit: None,
            dots_at: Instant::now(),
            #[cfg(target_os = "windows")]
            resize: None,
            #[cfg(target_os = "windows")]
            resize_task: None,
            #[cfg(target_os = "windows")]
            over_unlock: false,
            #[cfg(target_os = "windows")]
            unlock_hot: false,
            #[cfg(target_os = "windows")]
            click_through: None,
            #[cfg(target_os = "windows")]
            forwarding: false,
            hovering: false,
            hover_changed: false,
            hover_epoch: 0,
            stroke: None,
            stroke_pending: None,
            cover: None,
            accent: None,
            accent_task: None,
            hanzi: HashMap::new(),
            fetching: HashSet::new(),
            stroke_tasks: Vec::new(),
        }
    }

    /// The unlock button being hovered is what turns click-through off for a moment, so a
    /// click can land on it; leaving the strip or unlocking puts it back.
    #[cfg(target_os = "windows")]
    fn set_unlock_hot(&mut self, hot: bool, cx: &mut Context<Self>) {
        if self.unlock_hot != hot {
            self.unlock_hot = hot;
            cx.notify();
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn set_unlock_hot(&mut self, _hot: bool, _cx: &mut Context<Self>) {}

    fn set_hovered(&mut self, want: bool, cx: &mut Context<Self>) {
        if self.hovering == want {
            return;
        }
        if !want {
            self.set_unlock_hot(false, cx);
        }
        self.hovering = want;
        self.hover_changed = true;
        self.hover_epoch = self.hover_epoch.wrapping_add(1);
        cx.notify();
    }

    /// Keeps hover honest when the pointer leaves without a leave message, which happens when
    /// the window moves under the cursor during a drag.
    fn poll_hover(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.host else {
            return;
        };
        let inside = cursor_over_window(host);
        #[cfg(target_os = "windows")]
        {
            let locked = self.settings.read(cx).desktop_lyrics_locked();
            let over = locked && inside && cursor_over_unlock(host);
            if self.over_unlock != over {
                self.over_unlock = over;
                cx.notify();
            }
        }
        let want = inside || self.resizing();
        self.set_hovered(want, cx);
    }

    /// Whether a resize drag is running; the plate stays up for the whole drag.
    #[cfg(target_os = "windows")]
    fn resizing(&self) -> bool {
        self.resize.is_some()
    }

    #[cfg(not(target_os = "windows"))]
    fn resizing(&self) -> bool {
        false
    }

    /// Starts a resize from one of the grab zones. Windows resizes the window here, from the
    /// pointer, because the OS sizing loop stalls a transparent window; other platforms hand
    /// the drag to their own frame.
    fn start_resize(&mut self, dir: ResizeDir, window: &mut Window, cx: &mut Context<Self>) {
        self.set_hovered(true, cx);
        #[cfg(target_os = "windows")]
        {
            let (Some(cursor), Some(rect)) = (cursor_position(), window_rect(window)) else {
                return;
            };
            self.resize = Some(ResizeDrag {
                dir,
                cursor,
                rect,
                applied: rect,
            });
            self.pump_resize(window, cx);
        }
        #[cfg(not(target_os = "windows"))]
        window.start_window_resize(dir.edge());
    }

    /// Drives the resize drag one frame at a time. The pointer's screen position, which the OS
    /// keeps delivering under its mouse capture, is the source of truth, so the window follows
    /// exactly even when the edge outruns it.
    #[cfg(target_os = "windows")]
    fn pump_resize(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.resize_task.is_some() {
            return;
        }
        self.resize_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SWEEP_FRAME).await;
            this.update_in(cx, |this, window, cx| {
                this.resize_task = None;
                this.step_resize(window, cx);
            })
            .ok();
        }));
    }

    #[cfg(target_os = "windows")]
    fn step_resize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.resize else {
            return;
        };
        if !left_button_down() {
            self.finish_resize(window, cx);
            return;
        }
        let Some(cursor) = cursor_position() else {
            self.finish_resize(window, cx);
            return;
        };
        let next = drag.rect_at(cursor, window.scale_factor());
        if next != drag.applied {
            post_window_rect(window, next);
            if let Some(drag) = self.resize.as_mut() {
                drag.applied = next;
            }
        }
        self.pump_resize(window, cx);
    }

    /// Ends a resize drag. The size the window came out at becomes the saved font size, so the
    /// strip reopens at the size it was left at.
    #[cfg(target_os = "windows")]
    fn finish_resize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.resize.take() else {
            return;
        };
        let final_rect = cursor_position()
            .map(|cursor| drag.rect_at(cursor, window.scale_factor()))
            .unwrap_or(drag.applied);
        if final_rect != drag.applied {
            post_window_rect(window, final_rect);
        }
        let scale = window.scale_factor();
        let vertical = self.settings.read(cx).desktop_lyrics_vertical();
        let font = window_font(
            final_rect.2 as f32 / scale,
            final_rect.3 as f32 / scale,
            vertical,
        );
        self.fit = Some((font, vertical));
        self.settings.update(cx, |settings, cx| {
            settings.set_desktop_lyrics_font_size(font, cx)
        });
        cx.notify();
    }

    /// A grab zone for one edge or corner, placed just inside the border so the pointer never
    /// has to leave the window to catch it.
    fn resize_zone(&self, dir: ResizeDir, cx: &mut Context<Self>) -> AnyElement {
        let zone = div().absolute().cursor(dir.cursor()).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.start_resize(dir, window, cx);
            }),
        );
        let zone = match dir {
            ResizeDir::Top => zone
                .top_0()
                .left(px(HANDLE_INSET))
                .right(px(HANDLE_INSET))
                .h(px(HANDLE)),
            ResizeDir::Bottom => zone
                .bottom_0()
                .left(px(HANDLE_INSET))
                .right(px(HANDLE_INSET))
                .h(px(HANDLE)),
            ResizeDir::Left => zone
                .left_0()
                .top(px(HANDLE_INSET))
                .bottom(px(HANDLE_INSET))
                .w(px(HANDLE)),
            ResizeDir::Right => zone
                .right_0()
                .top(px(HANDLE_INSET))
                .bottom(px(HANDLE_INSET))
                .w(px(HANDLE)),
            ResizeDir::TopLeft => zone.top_0().left_0().size(px(CORNER)),
            ResizeDir::TopRight => zone.top_0().right_0().size(px(CORNER)),
            ResizeDir::BottomLeft => zone.bottom_0().left_0().size(px(CORNER)),
            ResizeDir::BottomRight => zone.bottom_0().right_0().size(px(CORNER)),
        };
        zone.into_any_element()
    }

    /// Redraws at the karaoke cadence while a line is filling. Position reports arrive twice a
    /// second, which is far too coarse to carry the per-word sweep.
    fn pump(&mut self, cx: &mut Context<Self>) {
        if self.sweeping.is_some() {
            return;
        }
        self.sweeping = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SWEEP_FRAME).await;
            this.update(cx, |this, cx| {
                this.sweeping = None;
                cx.notify();
            })
            .ok();
        }));
    }

    fn request_hanzi(&mut self, ch: char, cx: &mut Context<Self>) {
        if !is_strokable(ch) || self.hanzi.contains_key(&ch) || self.fetching.contains(&ch) {
            return;
        }
        self.fetching.insert(ch);
        let task = Io::global(cx).load(
            async move { fetch_hanzi(ch).await },
            move |this: &mut Self, answer, cx| {
                this.fetching.remove(&ch);
                let data = answer.ok().flatten();
                this.hanzi.insert(ch, data.clone());
                if this.hanzi.len() > 800
                    && let Some(old) = this.hanzi.keys().copied().find(|key| *key != ch)
                {
                    this.hanzi.remove(&old);
                }
                if let Some(pair) = this.stroke.as_mut() {
                    for line in &mut pair.lines {
                        for item in &mut line.chars {
                            if item.timing.text == ch && item.data.is_none() {
                                item.data = data.clone();
                                item.no_data = data.is_none();
                            }
                        }
                    }
                }
                cx.notify();
            },
            cx,
        );
        self.stroke_tasks.push(task);
    }

    fn build_stroke_pair(&mut self, lines: &[LyricsLine], pair: usize, cx: &mut Context<Self>) {
        if self
            .stroke
            .as_ref()
            .is_some_and(|scene| scene.first == pair)
        {
            return;
        }
        let mut built = Vec::with_capacity(2);
        for offset in 0..2 {
            let index = pair + offset;
            let Some(line) = lines.get(index) else {
                built.push(StrokeLine {
                    chars: Vec::new(),
                    instant: true,
                    fly_budget: 0.,
                });
                continue;
            };
            let timings = flatten_line(line);
            let next_start = lines
                .get(index + 1)
                .map(|next| next.start)
                .unwrap_or(line.start + Duration::from_secs(5));
            let window = next_start
                .saturating_sub(line.start)
                .max(Duration::from_millis(600));
            let last_offset = timings.last().map(|timing| timing.trigger_ms).unwrap_or(0.)
                - line.start.as_secs_f64() * 1000.;
            let fly_budget = (window.as_secs_f64() * 1000. - 120. - last_offset).max(0.);
            let instant = instant_line(&timings);
            let chars = timings
                .into_iter()
                .map(|timing| {
                    let cached = self.hanzi.get(&timing.text).cloned().flatten();
                    let no_data = !is_strokable(timing.text)
                        || self
                            .hanzi
                            .get(&timing.text)
                            .is_some_and(|data| data.is_none());
                    StrokeChar {
                        timing,
                        data: cached,
                        no_data,
                        fly: None,
                        assembled: false,
                        scatter: None,
                        fallback_at: None,
                    }
                })
                .collect::<Vec<_>>();
            built.push(StrokeLine {
                chars,
                instant,
                fly_budget,
            });
        }
        let wanted = built
            .iter()
            .flat_map(|line| line.chars.iter().map(|item| item.timing.text))
            .collect::<HashSet<_>>();
        for ch in wanted {
            self.request_hanzi(ch, cx);
        }
        for index in [pair + 2, pair + 3] {
            if let Some(line) = lines.get(index) {
                for ch in line.text.chars() {
                    self.request_hanzi(ch, cx);
                }
            }
        }
        self.stroke = Some(StrokePair {
            first: pair,
            lines: [built.remove(0), built.remove(0)],
            out_at: None,
        });
        self.stroke_pending = None;
    }

    fn advance_stroke_pair(&mut self, active: Option<usize>, now: Instant) {
        match (self.pair, active) {
            (None, Some(active)) => {
                self.pair = Some(active);
                self.stroke = None;
            }
            (Some(pair), Some(active)) if active == pair || active == pair + 1 => {
                self.stroke_pending = None;
                if let Some(scene) = self.stroke.as_mut()
                    && scene.out_at.take().is_some()
                {
                    for line in &mut scene.lines {
                        for item in &mut line.chars {
                            item.scatter = None;
                        }
                    }
                }
            }
            (Some(_), Some(active)) => {
                self.stroke_pending = Some(active);
                if let Some(scene) = self.stroke.as_mut()
                    && scene.out_at.is_none()
                {
                    scene.out_at = Some(now);
                    for line in &mut scene.lines {
                        for (index, item) in line.chars.iter_mut().enumerate() {
                            if let Some(data) = item.data.as_ref() {
                                item.scatter = Some(StrokeScatter {
                                    start: now,
                                    poses: scatter_poses(
                                        item.timing.trigger_ms,
                                        index,
                                        data.stroke_count(),
                                    ),
                                });
                            }
                        }
                    }
                }
            }
            (_, None) => {
                self.pair = None;
                self.stroke = None;
                self.stroke_pending = None;
            }
        }
        if self
            .stroke
            .as_ref()
            .and_then(|scene| scene.out_at)
            .is_some_and(|at| at.elapsed().as_secs_f64() * 1000. >= STROKE_SCATTER_MS)
        {
            if let Some(next) = self.stroke_pending.take() {
                self.pair = Some(next);
            }
            self.stroke = None;
        }
    }

    fn tick_stroke(&mut self, now_ms: f64) {
        let Some(scene) = self.stroke.as_mut() else {
            return;
        };
        if scene.out_at.is_some() {
            return;
        }
        let now = Instant::now();
        for line in &mut scene.lines {
            for (index, item) in line.chars.iter_mut().enumerate() {
                if item.assembled || item.fly.is_some() || item.scatter.is_some() {
                    continue;
                }
                let delay = item.timing.trigger_ms - now_ms;
                if delay > 0. {
                    continue;
                }
                let late = -delay;
                match item.data.clone() {
                    Some(data) => {
                        if line.instant || late > 600. {
                            item.assembled = true;
                            continue;
                        }
                        let timeline = fit_timeline((line.fly_budget - late).max(0.));
                        let start = now - Duration::from_millis(late.max(0.) as u64);
                        let mut seed = (item.timing.trigger_ms as u64)
                            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                            .wrapping_add(index as u64);
                        let (strokes, _) = fly_strokes(&data, &mut seed, timeline, start);
                        item.fly = Some(StrokeFly { start, strokes });
                    }
                    None if item.no_data => {
                        item.fallback_at = Some(now);
                    }
                    None => {}
                }
            }
        }
    }

    fn transport(&self, vertical: bool, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_none()
            .when(vertical, |this| this.flex_col())
            .items_center()
            .gap(px(1.))
            .rounded_full()
            .border_1()
            .border_color(white().opacity(0.15))
            .bg(black().opacity(0.45))
            .p(px(2.))
            .child(previous(&self.playback, false, cx).tint(white()))
            .child(toggle(&self.playback, false, false, cx).tint(white()))
            .child(next(&self.playback, false, cx).tint(white()))
            .into_any_element()
    }

    fn display(&self, vertical: bool, cx: &mut Context<Self>) -> AnyElement {
        let vertical_layout = self.settings.read(cx).desktop_lyrics_vertical();
        let stroke = self.settings.read(cx).desktop_lyrics_stroke();
        div()
            .flex()
            .flex_none()
            .when(vertical, |this| this.flex_col())
            .items_center()
            .gap(px(1.))
            .rounded_full()
            .border_1()
            .border_color(white().opacity(0.15))
            .bg(black().opacity(0.45))
            .p(px(2.))
            .child(
                Button::new("desktop-lyrics-direction")
                    .ghost()
                    .small()
                    .icon(match vertical_layout {
                        true => "icons/sliders-horizontal.svg",
                        false => "icons/columns-3.svg",
                    })
                    .tint(white())
                    .tooltip_above(match vertical_layout {
                        true => "desktop-lyrics-horizontal",
                        false => "desktop-lyrics-vertical",
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        let vertical = this.settings.read(cx).desktop_lyrics_vertical();
                        this.settings.update(cx, |settings, cx| {
                            settings.set_desktop_lyrics_vertical(!vertical, cx)
                        });
                    })),
            )
            .child(
                Button::new("desktop-lyrics-stroke")
                    .ghost()
                    .small()
                    .icon("icons/pencil.svg")
                    .tint(white())
                    .tooltip_above(match stroke {
                        true => "desktop-lyrics-default",
                        false => "desktop-lyrics-stroke",
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        let stroke = this.settings.read(cx).desktop_lyrics_stroke();
                        this.settings.update(cx, |settings, cx| {
                            settings.set_desktop_lyrics_stroke(!stroke, cx)
                        });
                    })),
            )
            .child(
                Button::new("desktop-lyrics-lock")
                    .ghost()
                    .small()
                    .icon("icons/lock.svg")
                    .tint(white())
                    .tooltip_above("desktop-lyrics-lock")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings.update(cx, |settings, cx| {
                            settings.set_desktop_lyrics_locked(true, cx)
                        });
                    })),
            )
            .child(
                Button::new("desktop-lyrics-close")
                    .ghost()
                    .small()
                    .icon("icons/x.svg")
                    .tint(white())
                    .tooltip_above("desktop-lyrics-close")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings
                            .update(cx, |settings, cx| settings.set_desktop_lyrics(false, cx));
                    })),
            )
            .into_any_element()
    }

    /// The bubble a locked strip floats while the pointer is over it: the way back to moving
    /// and resizing it.
    fn unlock(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .top(px(UNLOCK_INSET))
            .right(px(UNLOCK_INSET))
            .child(
                div()
                    .flex()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .border_color(white().opacity(0.15))
                    .bg(black().opacity(0.45))
                    .p(px(2.))
                    .child(
                        Button::new("desktop-lyrics-unlock")
                            .ghost()
                            .small()
                            .icon("icons/lock-open.svg")
                            .tint(white())
                            .tooltip("desktop-lyrics-unlock")
                            .on_hover(cx.listener(|this, over: &bool, _, cx| {
                                this.set_unlock_hot(*over, cx)
                            }))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings.update(cx, |settings, cx| {
                                    settings.set_desktop_lyrics_locked(false, cx)
                                });
                            })),
                    ),
            )
            .into_any_element()
    }

    fn header(&self, title: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let title = div()
            .flex()
            .min_w_0()
            .flex_1()
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .border_color(white().opacity(0.15))
                    .bg(black().opacity(0.45))
                    .px(px(12.))
                    .py(px(4.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(white().opacity(0.75))
                            .child(title),
                    ),
            );
        div()
            .flex_none()
            .flex()
            .h(px(HEADER))
            .w_full()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .when(!self.hovering, |this| this.invisible())
            .child(title)
            .child(self.transport(false, cx))
            .child(self.display(false, cx))
            .into_any_element()
    }

    fn vertical_header(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .right(px(8.))
            .top(px(8.))
            .bottom(px(8.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .when(!self.hovering, |this| this.invisible())
            .child(self.transport(true, cx))
            .child(self.display(true, cx))
            .into_any_element()
    }
}

/// A dark rim around the glyphs, then a tight and a wide soft shadow. The rim substitutes for
/// a text stroke: a transparent window cannot use subpixel text, and the rim is what hides
/// the gray antialiasing steps over a bright wallpaper.
fn shadows(size: f32) -> Vec<TextShadow> {
    let edge = (size * 0.04).clamp(1., 2.5);
    let mut shadows = Vec::with_capacity(10);
    for (x, y) in [(1., 0.), (-1., 0.), (0., 1.), (0., -1.)] {
        shadows.push(TextShadow::new(black().opacity(0.55)).offset(px(edge * x), px(edge * y)));
    }
    for (x, y) in [(0.7, 0.7), (-0.7, 0.7), (0.7, -0.7), (-0.7, -0.7)] {
        shadows.push(TextShadow::new(black().opacity(0.4)).offset(px(edge * x), px(edge * y)));
    }
    shadows.push(TextShadow::new(black().opacity(0.55)).blur(px(2.)));
    shadows.push(TextShadow::new(black().opacity(0.35)).blur(px(7.)));
    shadows
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineRole {
    Singing,
    Sung,
    Preview,
}

impl LineRole {
    fn color(self, accent: Hsla) -> Hsla {
        match self {
            Self::Singing | Self::Sung => accent,
            Self::Preview => accent.opacity(0.45),
        }
    }

    /// A line brightens once its start passes and settles to plain white once its last word
    /// has been sung. The pair's outgoing side keeps its look while it slides away.
    fn at(line: &LyricsLine, at: Duration) -> Self {
        match at < line.start {
            true => Self::Preview,
            false => match line.sung_end().is_some_and(|end| at >= end) {
                true => Self::Sung,
                false => Self::Singing,
            },
        }
    }
}

fn line_text(
    line: &LyricsLine,
    at: Duration,
    role: LineRole,
    size: f32,
    accent: Hsla,
) -> AnyElement {
    if role == LineRole::Singing
        && let Some(words) = line.words.as_deref()
        && let Some(fragments) = word_fragments(&line.text, words)
    {
        let edge = size * SWEEP_EDGE;
        let row = div()
            .flex()
            .items_center()
            .whitespace_nowrap()
            .text_size(px(size))
            .line_height(relative(SLOT_RATIO))
            .text_shadow(shadows(size))
            .font_weight(FontWeight::NORMAL);
        let row = fragments.into_iter().fold(row, |row, (text, word)| {
            let (start, end) = word_window(line.start, words, word);
            let share = swept(start, end, at, word + 1 >= words.len());
            let cell = div()
                .relative()
                .flex_none()
                .whitespace_nowrap()
                .text_color(accent.opacity(UNSUNG))
                .child(SharedString::from(text.clone()));
            let cell = match share > 0. {
                true => cell.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(relative(share))
                        .overflow_hidden()
                        // The shadow lives on the line below. Inheriting it here would
                        // let the word's clipping box cut it into a visible rectangle.
                        .text_shadow([])
                        .text_color(accent)
                        .when(share < 1., |this| this.fade_sides(px(0.), px(edge)))
                        .child(div().whitespace_nowrap().child(SharedString::from(text))),
                ),
                false => cell,
            };
            row.child(cell)
        });
        return row.into_any_element();
    }
    div()
        .whitespace_nowrap()
        .text_size(px(size))
        .line_height(relative(SLOT_RATIO))
        .text_shadow(shadows(size))
        .text_color(role.color(accent))
        .font_weight(FontWeight::NORMAL)
        .child(line.text.clone())
        .into_any_element()
}

/// Splits a line at its word boundaries, keeping the spacing each word brings along. Answers
/// nothing when the timing does not line up with the text, which then draws as a plain line.
fn word_fragments(line: &str, words: &[LyricsWord]) -> Option<Vec<(String, usize)>> {
    let mut starts = Vec::with_capacity(words.len());
    let mut cursor = 0;
    for word in words {
        if word.text.is_empty() {
            return None;
        }
        let rest = line.get(cursor..)?;
        let start = cursor + rest.find(&word.text)?;
        starts.push(start);
        cursor = start + word.text.len();
    }
    Some(
        starts
            .iter()
            .enumerate()
            .map(|(index, start)| {
                let start = match index {
                    0 => 0,
                    _ => *start,
                };
                let end = starts.get(index + 1).copied().unwrap_or(line.len());
                (line[start..end].to_owned(), index)
            })
            .collect(),
    )
}

/// When a word starts filling and when its fill reaches the end of the text. A rest after it
/// ends the fill there instead of dragging it on.
fn word_window(line_start: Duration, words: &[LyricsWord], index: usize) -> (Duration, Duration) {
    let word = &words[index];
    let start = match index {
        0 => line_start.min(word.start),
        _ => word.start,
    };
    let sung = word.end.max(start);
    let end = match words.get(index + 1) {
        Some(next) => match sung > start {
            true => next.start.max(start).min(sung),
            false => next.start.max(start),
        },
        None => sung,
    };
    (start, end)
}

fn swept(start: Duration, end: Duration, at: Duration, tail: bool) -> f32 {
    let span = end.saturating_sub(start);
    let travel = match tail {
        true => span.max(SWEEP_LEAST),
        false => span.mul_f32(SWEEP_STRETCH).max(SWEEP_LEAST),
    };
    let share = progress(start, start + travel, at);
    let eased = 1. - (1. - share).powi(3);
    match eased >= SWEPT {
        true => 1.,
        false => eased,
    }
}

fn progress(start: Duration, end: Duration, at: Duration) -> f32 {
    if at <= start {
        return 0.;
    }
    if at >= end {
        return 1.;
    }
    let span = (end - start).as_secs_f32();
    ((at - start).as_secs_f32() / span).clamp(0., 1.)
}

fn plain_line(text: &str, role: LineRole, size: f32, accent: Hsla) -> AnyElement {
    div()
        .whitespace_nowrap()
        .text_size(px(size))
        .line_height(relative(SLOT_RATIO))
        .text_shadow(shadows(size))
        .text_color(role.color(accent))
        .font_weight(FontWeight::NORMAL)
        .child(SharedString::from(text.to_owned()))
        .into_any_element()
}

#[derive(Clone, Copy)]
struct Stage<'a> {
    lines: &'a [LyricsLine],
    at: Duration,
    font: f32,
    slot: f32,
    accent: Hsla,
}

fn pair_row(stage: &Stage, index: usize, top: f32, opacity: f32, tail: bool) -> AnyElement {
    let text = stage
        .lines
        .get(index)
        .map(|line| {
            line_text(
                line,
                stage.at,
                LineRole::at(line, stage.at),
                stage.font,
                stage.accent,
            )
        })
        .unwrap_or_else(|| div().into_any_element());
    div()
        .absolute()
        .left_0()
        .right_0()
        .top(px(top))
        .h(px(stage.slot))
        .px(px(EDGE))
        .flex()
        .items_center()
        .when(tail, |this| this.justify_end())
        .when(!tail, |this| this.justify_start())
        .overflow_hidden()
        .opacity(opacity)
        .child(text)
        .into_any_element()
}

fn plain_row(
    text: String,
    role: LineRole,
    font: f32,
    slot: f32,
    top: f32,
    tail: bool,
    accent: Hsla,
) -> AnyElement {
    div()
        .absolute()
        .left_0()
        .right_0()
        .top(px(top))
        .h(px(slot))
        .px(px(EDGE))
        .flex()
        .items_center()
        .when(tail, |this| this.justify_end())
        .when(!tail, |this| this.justify_start())
        .overflow_hidden()
        .child(plain_line(&text, role, font, accent))
        .into_any_element()
}

#[derive(Clone)]
struct CharTiming {
    text: char,
    start: Duration,
    end: Duration,
}

/// Splits a line into one timing per visible character, using the word timing when there is
/// one and spreading the line otherwise.
fn char_timings(line: &LyricsLine) -> Vec<CharTiming> {
    let mut timings = Vec::new();
    if let Some(words) = line.words.as_deref().filter(|words| !words.is_empty()) {
        for word in words {
            let chars = word
                .text
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<Vec<_>>();
            let count = chars.len() as f64;
            if count == 0. {
                continue;
            }
            let span = word.end.saturating_sub(word.start);
            for (index, text) in chars.into_iter().enumerate() {
                let start = word.start + span.mul_f64(index as f64 / count);
                let end = word.start + span.mul_f64((index + 1) as f64 / count);
                timings.push(CharTiming {
                    text,
                    start,
                    end: end.max(start + Duration::from_millis(1)),
                });
            }
        }
        return timings;
    }
    let chars = line
        .text
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<Vec<_>>();
    let count = chars.len() as f64;
    if count == 0. {
        return timings;
    }
    let span = line
        .end
        .map(|end| end.saturating_sub(line.start))
        .unwrap_or(Duration::from_secs(4));
    for (index, text) in chars.into_iter().enumerate() {
        let start = line.start + span.mul_f64(index as f64 / count);
        let end = line.start + span.mul_f64((index + 1) as f64 / count);
        timings.push(CharTiming {
            text,
            start,
            end: end.max(start + Duration::from_millis(1)),
        });
    }
    timings
}

fn vertical_char(
    timing: &CharTiming,
    at: Duration,
    role: LineRole,
    font: f32,
    slot: f32,
    tail: bool,
    accent: Hsla,
) -> AnyElement {
    let width = (font * 1.2).max(1.);
    // The current line's unsung characters stay dim, as they do in the horizontal sweep, so
    // the bright fill is what marks how far the line has been sung.
    let base_color = match role {
        LineRole::Singing => accent.opacity(UNSUNG),
        _ => role.color(accent),
    };
    let base = div()
        .relative()
        .flex_none()
        .w(px(width))
        .h(px(slot))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(font))
        .line_height(relative(1.))
        .text_shadow(shadows(font))
        .text_color(base_color)
        .font_weight(FontWeight::NORMAL)
        .child(SharedString::from(timing.text.to_string()));
    let base = match role == LineRole::Singing {
        true => {
            let share = swept(timing.start, timing.end, at, tail);
            match share > 0. {
                true => base.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .w_full()
                        .h(relative(share))
                        .overflow_hidden()
                        .text_shadow([])
                        .text_color(accent)
                        .when(share < 1., |this| {
                            this.fade_edges(px(0.), px(font * SWEEP_EDGE))
                        })
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .w_full()
                                .h(px(slot))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(font))
                                .line_height(relative(1.))
                                .child(SharedString::from(timing.text.to_string())),
                        ),
                ),
                false => base,
            }
        }
        false => base,
    };
    base.into_any_element()
}

/// One line as vertical columns: characters stack downwards, and a long line continues in the
/// next column to the left, the way vertical writing flows.
fn vertical_line(
    line: &LyricsLine,
    at: Duration,
    role: LineRole,
    font: f32,
    height: f32,
    accent: Hsla,
) -> AnyElement {
    let timings = char_timings(line);
    if timings.is_empty() {
        return div().into_any_element();
    }
    let slot = font * SLOT_RATIO;
    let per = ((height / slot).floor() as usize).max(1);
    let last = timings.len().saturating_sub(1);
    let mut columns = div()
        .flex()
        .flex_row_reverse()
        .items_start()
        .gap(px((font * 0.12).max(2.)));
    let chunks = timings.chunks(per).collect::<Vec<_>>();
    for chunk in chunks {
        let mut column = div().flex().flex_col().items_center();
        for timing in chunk {
            let tail = std::ptr::eq(timing, &timings[last]);
            column = column.child(vertical_char(timing, at, role, font, slot, tail, accent));
        }
        columns = columns.child(column);
    }
    columns.into_any_element()
}

#[derive(Clone, Copy)]
struct VerticalStage<'a> {
    lines: &'a [LyricsLine],
    at: Duration,
    font: f32,
    height: f32,
    accent: Hsla,
}

/// One pair of vertical lines: the first sits top-left, the second bottom-right, and both pull
/// in towards the middle so the two columns stay close instead of hugging the outer edges.
fn vertical_pair(
    stage: VerticalStage<'_>,
    pair: usize,
    opacity: f32,
    offsets: (f32, f32),
    roles: Option<(LineRole, LineRole)>,
) -> AnyElement {
    let first = stage.lines.get(pair);
    let second = stage.lines.get(pair + 1);
    let (left_role, right_role) = roles.unwrap_or_else(|| {
        (
            first.map_or(LineRole::Preview, |line| LineRole::at(line, stage.at)),
            second.map_or(LineRole::Preview, |line| LineRole::at(line, stage.at)),
        )
    });
    let left = first
        .map(|line| {
            vertical_line(
                line,
                stage.at,
                left_role,
                stage.font,
                stage.height,
                stage.accent,
            )
        })
        .unwrap_or_else(|| div().into_any_element());
    let right = second
        .map(|line| {
            vertical_line(
                line,
                stage.at,
                right_role,
                stage.font,
                stage.height,
                stage.accent,
            )
        })
        .unwrap_or_else(|| div().into_any_element());
    let gap = (stage.font * 0.3).max(6.);
    div()
        .absolute()
        .size_full()
        .flex()
        .items_stretch()
        .opacity(opacity)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .relative()
                .left(px(offsets.0))
                .flex()
                .items_start()
                .justify_end()
                .overflow_hidden()
                .pl(px(EDGE / 2.))
                .pr(px(gap / 2.))
                .child(left),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .relative()
                .left(px(offsets.1))
                .flex()
                .items_end()
                .justify_start()
                .overflow_hidden()
                .pl(px(gap / 2.))
                .pr(px(EDGE / 2.))
                .child(right),
        )
        .into_any_element()
}

/// The two lines leave towards their own side while the incoming pair arrives from it.
fn vertical_scene(
    stage: VerticalStage<'_>,
    pair: usize,
    swap: Option<(usize, f32)>,
    slide: f32,
    roles: Option<(LineRole, LineRole)>,
) -> AnyElement {
    let mut scene = div().relative().size_full();
    if let Some((from, eased)) = swap {
        let out = (-slide * eased, slide * eased);
        let back = -slide * (1. - eased);
        scene = scene
            .child(vertical_pair(stage, from, 1. - eased, out, roles))
            .child(vertical_pair(stage, pair, eased, (back, -back), roles));
    } else {
        scene = scene.child(vertical_pair(stage, pair, 1., (0., 0.), roles));
    }
    scene.into_any_element()
}

fn synthetic_line(text: String, start_ms: u64) -> LyricsLine {
    LyricsLine {
        start: Duration::from_millis(start_ms),
        end: Some(Duration::from_millis(start_ms + 4000)),
        text,
        romanized: None,
        words: None,
        secondary: Vec::new(),
        voice: Default::default(),
    }
}

fn scatter_poses(trigger_ms: f64, index: usize, count: usize) -> Vec<Pose> {
    let mut seed = (trigger_ms as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(index as u64)
        .wrapping_add(0x5DEE_CE66);
    (0..count)
        .map(|_| {
            let depth = next_random(&mut seed);
            let angle = next_random(&mut seed) * std::f64::consts::TAU;
            let radius = 120. + 360. * depth;
            let rot = (next_random(&mut seed) * 2. - 1.) * (24. + 48. * depth);
            Pose {
                dx: (angle.cos() * radius) as f32,
                dy: (angle.sin() * radius) as f32,
                rot: rot.to_radians() as f32,
                scale: (0.78 + 0.36 * (1. - depth)) as f32,
                alpha: 1.,
            }
        })
        .collect()
}

fn stroke_poses(item: &StrokeChar, out_at: Option<Instant>, now: Instant) -> (Vec<Pose>, f32) {
    if let Some(out_at) = out_at {
        let progress = ((now.saturating_duration_since(out_at).as_secs_f64() * 1000.)
            / STROKE_SCATTER_MS)
            .clamp(0., 1.);
        let alpha = (1. - progress) as f32;
        let Some(scatter) = item.scatter.as_ref() else {
            return (Vec::new(), alpha);
        };
        let eased = (progress * progress) as f32;
        let poses = scatter
            .poses
            .iter()
            .map(|pose| Pose {
                dx: pose.dx * eased,
                dy: pose.dy * eased,
                rot: pose.rot * eased,
                scale: 1. + (pose.scale - 1.) * eased,
                alpha,
            })
            .collect();
        return (poses, 1.);
    }
    if let Some(fly) = item.fly.as_ref() {
        let elapsed = now.saturating_duration_since(fly.start).as_secs_f64() * 1000.;
        let poses = fly
            .strokes
            .iter()
            .map(|stroke| {
                let progress = ((elapsed - stroke.delay_ms) / stroke.duration_ms).clamp(0., 1.);
                let eased = 1. - (1. - progress).powi(3);
                Pose {
                    dx: stroke.dx * (1. - eased as f32),
                    dy: stroke.dy * (1. - eased as f32),
                    rot: stroke.rot * (1. - eased as f32),
                    scale: stroke.scale + (1. - stroke.scale) * eased as f32,
                    alpha: eased as f32,
                }
            })
            .collect();
        return (poses, 1.);
    }
    if item.assembled {
        // An empty list makes StrokeCell fall back to its rest pose for every stroke.
        return (Vec::new(), 1.);
    }
    if let Some(at) = item.fallback_at {
        return (Vec::new(), if now >= at { 1. } else { 0. });
    }
    (Vec::new(), 0.)
}

/// How one line of stroke glyphs is drawn: the cell it gets and the pass's colours. The flat
/// shadow pass draws the same shapes darker underneath, and the coloured pass above it carries
/// a dark rim so the strokes read over a bright wallpaper.
#[derive(Clone, Copy)]
struct StrokeStyle {
    cell: f32,
    gap: f32,
    color: gpui::Hsla,
    /// Give a character without stroke data its dark rim, as the lyric text wears one.
    rim: bool,
    outline: Option<gpui::Hsla>,
}

fn stroke_cell(
    item: &StrokeChar,
    style: StrokeStyle,
    out_at: Option<Instant>,
    now: Instant,
) -> AnyElement {
    let StrokeStyle {
        cell,
        color,
        rim,
        outline,
        ..
    } = style;
    let (poses, alpha) = stroke_poses(item, out_at, now);
    let content = match item.data.as_ref() {
        Some(hanzi) => StrokeCell {
            hanzi: hanzi.clone(),
            size: px(cell),
            poses,
            color: color.opacity(alpha),
            outline: outline.map(|outline| outline.opacity(alpha)),
        }
        .into_any_element(),
        None => {
            let size = (cell * 0.62).max(8.);
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .opacity(alpha)
                .text_size(px(size))
                .when(rim, |this| this.text_shadow(shadows(size)))
                .text_color(color)
                .child(SharedString::from(item.timing.text.to_string()))
                .into_any_element()
        }
    };
    div()
        .flex_none()
        .w(px(cell))
        .h(px(cell))
        .child(content)
        .into_any_element()
}

fn stroke_cells(
    line: &StrokeLine,
    style: StrokeStyle,
    out_at: Option<Instant>,
    now: Instant,
) -> AnyElement {
    let mut row = div().flex().items_center().gap(px(style.gap));
    for item in &line.chars {
        row = row.child(stroke_cell(item, style, out_at, now));
    }
    row.into_any_element()
}

fn stroke_column(
    line: &StrokeLine,
    style: StrokeStyle,
    out_at: Option<Instant>,
    now: Instant,
) -> AnyElement {
    let mut column = div().flex().flex_col().items_center().gap(px(style.gap));
    for item in &line.chars {
        column = column.child(stroke_cell(item, style, out_at, now));
    }
    column.into_any_element()
}

/// A dark, blurred copy of a stroke line drawn just below it, which is what makes light
/// strokes read over light artwork. One blurred layer per line, not one filter per glyph.
fn stroke_shadow(cells: AnyElement, cell: f32) -> AnyElement {
    div()
        .absolute()
        .left_0()
        .right_0()
        .top(px((cell * 0.04).max(1.)))
        .blur(px((cell * 0.055).max(1.5)))
        .child(cells)
        .into_any_element()
}

fn stroke_view(
    pair: &StrokePair,
    font: f32,
    width: f32,
    height: f32,
    vertical: bool,
    accent: Hsla,
    now: Instant,
) -> AnyElement {
    let max_chars = pair
        .lines
        .iter()
        .map(|line| line.chars.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let color = accent;
    if vertical {
        // The first line sits top-left and the second bottom-right, both pulled in towards
        // the middle so the two columns stay close and clear of the edges.
        let chars = max_chars as f32;
        let spacing = chars + (chars - 1.) * 0.14;
        let cell = font
            .min((height - EDGE * 2.).max(0.) * 0.98 / spacing)
            .min((width - EDGE * 2.) / 2.8)
            .max(12.);
        let gap = (cell * 0.14).max(4.);
        let line_gap = (cell * 0.5).max(12.);
        let flat = StrokeStyle {
            cell,
            gap,
            color: black().opacity(0.55),
            rim: false,
            outline: None,
        };
        let lit = StrokeStyle {
            cell,
            gap,
            color,
            rim: true,
            outline: Some(black().opacity(0.55)),
        };
        let mut scene = div().size_full().flex().items_stretch();
        for offset in 0..2 {
            let line = &pair.lines[offset];
            let shadow = stroke_column(line, flat, pair.out_at, now);
            let cells = stroke_column(line, lit, pair.out_at, now);
            let cells = {
                let shadowed = stroke_shadow(shadow, cell);
                // Keep the shadow behind the coloured column without changing its layout.
                let mut layered = div().relative().child(shadowed);
                layered = layered.child(cells);
                layered.into_any_element()
            };
            let column = div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .gap(px(gap))
                .pt(px(EDGE / 2.))
                .pb(px(EDGE / 2.))
                .when(offset == 0, |this| {
                    this.items_end()
                        .justify_start()
                        .pl(px(EDGE / 2.))
                        .pr(px(line_gap / 2.))
                })
                .when(offset == 1, |this| {
                    this.items_start()
                        .justify_end()
                        .pl(px(line_gap / 2.))
                        .pr(px(EDGE / 2.))
                });
            scene = scene.child(column.child(cells));
        }
        return scene.into_any_element();
    }
    let cell = font
        .min(height * 0.42)
        .min(width * 0.92 / max_chars as f32)
        .max(12.);
    let gap = (cell * 0.14).max(4.);
    let flat = StrokeStyle {
        cell,
        gap,
        color: black().opacity(0.55),
        rim: false,
        outline: None,
    };
    let lit = StrokeStyle {
        cell,
        gap,
        color,
        rim: true,
        outline: Some(black().opacity(0.55)),
    };
    let mut scene = div().size_full().flex().flex_col();
    for offset in 0..2 {
        let line = &pair.lines[offset];
        let shadow = stroke_cells(line, flat, pair.out_at, now);
        let cells = stroke_cells(line, lit, pair.out_at, now);
        let cells = {
            let shadowed = stroke_shadow(shadow, cell);
            let mut layered = div().relative().child(shadowed);
            layered = layered.child(cells);
            layered.into_any_element()
        };
        let row = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .items_center()
            .px(px(EDGE))
            .when(offset == 0, |this| this.justify_start())
            .when(offset == 1, |this| this.justify_end())
            .child(cells);
        scene = scene.child(row);
    }
    scene.into_any_element()
}

/// Three dots that breathe in the accent colour where a line would be: the interlude mark the
/// lyrics page draws, in place of a note or a blank stage. `phase` is the breath in radians.
fn interlude_dots(font: f32, accent: Hsla, phase: f32) -> AnyElement {
    let size = (font * 0.26).clamp(6., 16.);
    let gap = (font * 0.2).clamp(6., 18.);
    let mut row = div().flex().items_center().gap(px(gap));
    for index in 0..3 {
        // A third of a breath apart, so the glow travels along the row.
        let breath = ((phase - index as f32 * 0.9).sin() * 0.5 + 0.5).powi(2);
        row = row.child(
            div()
                .flex_none()
                .size(px(size))
                .rounded_full()
                .bg(accent.opacity(0.25 + 0.65 * breath))
                .layer_scale(0.8 + 0.2 * breath),
        );
    }
    div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(row)
        .into_any_element()
}

#[cfg(target_os = "windows")]
impl Drop for DesktopLyric {
    fn drop(&mut self) {
        if self.forwarding {
            stop_move_forwarding();
        }
    }
}

impl Render for DesktopLyric {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.framed {
            self.framed = true;
            hide_frame(window);
            self.host = window_host(window);
        }
        let setting_font = self.settings.read(cx).desktop_lyrics_font_size();
        let vertical = self.settings.read(cx).desktop_lyrics_vertical();
        let stroke_mode = self.settings.read(cx).desktop_lyrics_stroke();
        // The stroke effect keeps its own size, so it can be tuned apart from the text.
        let stroke_scale = self.settings.read(cx).desktop_lyrics_stroke_scale();
        if !stroke_mode {
            self.stroke = None;
            self.stroke_pending = None;
        }
        let locked = self.settings.read(cx).desktop_lyrics_locked();
        // A locked strip lets the mouse through to whatever is behind it, except while the
        // pointer is on the unlock bubble, where a click has to reach the button. Its moves
        // are forwarded in so hovering still works, as Electron does for EchoMusic.
        #[cfg(target_os = "windows")]
        {
            if self.forwarding != locked {
                self.forwarding = locked;
                match locked {
                    true => start_move_forwarding(window),
                    false => stop_move_forwarding(),
                }
            }
            let through = locked && !(self.unlock_hot || self.over_unlock);
            if self.click_through != Some(through) {
                self.click_through = Some(through);
                set_click_through(window, through);
            }
        }
        let bounds = window.bounds();
        let shape_changed = self.fit != Some((setting_font, vertical));
        // The window is the size control: its height, horizontal, or width, vertical, decides
        // the lyric size directly. A changed saved size or direction re-fits the window once,
        // and the frame in between draws at the saved size instead of reading a huge
        // horizontal size out of a tall window (or the reverse).
        let font = match shape_changed {
            true => setting_font,
            false => window_font(
                bounds.size.width.as_f32(),
                bounds.size.height.as_f32(),
                vertical,
            ),
        }
        .round()
        .max(1.);
        let slot = font * SLOT_RATIO;
        // The interlude dots breathe on the strip's own clock, so a paused intro keeps its
        // gentle motion.
        let breath = (self.dots_at.elapsed().as_secs_f32() / BREATH.as_secs_f32()
            * std::f32::consts::TAU)
            % std::f32::consts::TAU;
        // Changing the saved size or direction resizes the strip once; the size then follows
        // whatever the window measures.
        if shape_changed {
            let turned = self.fit.is_some_and(|(_, saved)| saved != vertical);
            self.fit = Some((setting_font, vertical));
            cx.defer_in(window, move |_, window, cx| {
                // The strip takes its new rect from the window loop, which reports the new
                // size back through the bounds callback; a resize straight from here would
                // re-enter the app from that very callback.
                fit_to_shape(window, setting_font, vertical, turned, cx);
            });
        }
        let (track, position, playing) = {
            let playback = self.playback.read(cx);
            (
                playback.shown_track(cx),
                playback.live_position(),
                matches!(playback.state(), PlaybackState::Playing),
            )
        };
        let track_id = track.as_ref().and_then(|track| track.id.clone());
        if self.track != track_id {
            self.track = track_id;
            self.pair = None;
            self.swap = None;
            self.stroke = None;
            self.stroke_pending = None;
        }
        let cover_color = self.settings.read(cx).desktop_lyrics_cover_color();
        let cover = cover_color
            .then(|| track.as_ref().and_then(|track| track.cover.clone()))
            .flatten();
        if self.cover != cover {
            self.cover = cover.clone();
            match cover {
                Some(url) => {
                    let sampled = ui::palette(SharedString::from(url.clone()), cx);
                    self.accent_task = Some(cx.spawn(async move |this, cx| {
                        let palette = sampled.await;
                        let accent = palette
                            .primary
                            .and_then(|color| crate::shared::cover_color::accent(color, true));
                        this.update(cx, |this, cx| {
                            if this.cover.as_deref() == Some(url.as_str()) {
                                this.accent = accent;
                                cx.notify();
                            }
                        })
                        .ok();
                    }));
                }
                None => {
                    self.accent_task = None;
                    self.accent = None;
                }
            }
        }
        let accent = match cover_color {
            true => self.accent.unwrap_or_else(white),
            false => white(),
        };
        let (lyric, offset) = {
            let lyrics = self.lyrics.read(cx);
            (
                lyrics.current().map(|hit| hit.lyrics.clone()),
                lyrics.offset_ms(),
            )
        };
        let at = match offset {
            0 => position,
            ms if ms > 0 => position + Duration::from_millis(u64::from(ms.unsigned_abs())),
            ms => position.saturating_sub(Duration::from_millis(u64::from(ms.unsigned_abs()))),
        };
        let title = track
            .as_ref()
            .map(|track| SharedString::from(track.name.clone()))
            .unwrap_or_else(|| SharedString::from("WispMusic"));
        let artist = track
            .as_ref()
            .map(|track| SharedString::from(track.artists.clone()))
            .filter(|artists| !artists.is_empty())
            .unwrap_or_else(|| t!("player-nothing-playing"));
        let header_title = match track.as_ref() {
            Some(track) if !track.artists.is_empty() => {
                SharedString::from(format!("{} - {}", track.name, track.artists))
            }
            Some(track) => SharedString::from(track.name.clone()),
            None => SharedString::from("WispMusic"),
        };

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut vertical_stage: Option<AnyElement> = None;
        let mut placeholder = None;
        let mut stroke_animating = false;
        match lyric {
            Some(MusicLyrics::Synced { lines }) => {
                let active = music::lyrics::active(&lines, at);
                if stroke_mode {
                    self.swap = None;
                    self.advance_stroke_pair(active, Instant::now());
                    if let Some(pair) = self.pair.filter(|pair| *pair < lines.len()) {
                        self.build_stroke_pair(&lines, pair, cx);
                        self.tick_stroke(at.as_secs_f64() * 1000.);
                    }
                    stroke_animating = self
                        .stroke
                        .as_ref()
                        .is_some_and(|scene| scene.out_at.is_some())
                        || (playing && self.stroke.is_some());
                } else {
                    let filling = active
                        .and_then(|index| lines.get(index))
                        .filter(|line| at >= line.start)
                        .and_then(|line| line.words.as_deref());
                    if playing
                        && filling
                            .and_then(|words| words.last())
                            .is_some_and(|word| at < word.end)
                    {
                        self.pump(cx);
                    }
                    if self.swap.is_some_and(|swap| swap.started.elapsed() >= SWAP) {
                        self.swap = None;
                    }
                    match (self.pair, active) {
                        (Some(pair), Some(active)) if active == pair || active == pair + 1 => {}
                        (Some(pair), Some(active)) => {
                            self.swap = Some(Swap {
                                from: pair,
                                started: Instant::now(),
                            });
                            self.pair = Some(active);
                        }
                        (None, Some(active)) => self.pair = Some(active),
                        (_, None) => {
                            self.pair = None;
                            self.swap = None;
                        }
                    }
                    if self.swap.is_some() {
                        self.pump(cx);
                    }
                }
                // The stroke view owns the stage; do not build the text layers under it.
                if !stroke_mode {
                    if vertical {
                        match self.pair.filter(|pair| *pair < lines.len()) {
                            Some(pair) => {
                                let swap =
                                    self.swap
                                        .filter(|swap| swap.from < lines.len())
                                        .map(|swap| {
                                            let progress = (swap.started.elapsed().as_secs_f32()
                                                / SWAP.as_secs_f32())
                                            .clamp(0., 1.);
                                            (swap.from, 1. - (1. - progress).powi(3))
                                        });
                                vertical_stage = Some(vertical_scene(
                                    VerticalStage {
                                        lines: &lines,
                                        at,
                                        font,
                                        height: bounds.size.height.as_f32() - EDGE,
                                        accent,
                                    },
                                    pair,
                                    swap,
                                    slot,
                                    None,
                                ));
                            }
                            None => placeholder = Some(interlude_dots(font, accent, breath)),
                        }
                    } else {
                        let stage = Stage {
                            lines: &lines,
                            at,
                            font,
                            slot,
                            accent,
                        };
                        match self.pair.filter(|pair| *pair < lines.len()) {
                            Some(pair) => {
                                if let Some(swap) = self.swap.filter(|swap| swap.from < lines.len())
                                {
                                    let progress = (swap.started.elapsed().as_secs_f32()
                                        / SWAP.as_secs_f32())
                                    .clamp(0., 1.);
                                    let eased = 1. - (1. - progress).powi(3);
                                    for offset in 0..2 {
                                        let tail = offset == 1;
                                        let index = swap.from + offset;
                                        rows.push(pair_row(
                                            &stage,
                                            index,
                                            offset as f32 * slot - slot * eased,
                                            1. - eased,
                                            tail,
                                        ));
                                        let index = pair + offset;
                                        rows.push(pair_row(
                                            &stage,
                                            index,
                                            offset as f32 * slot + 2. * slot * (1. - eased),
                                            eased,
                                            tail,
                                        ));
                                    }
                                } else {
                                    for offset in 0..2 {
                                        let index = pair + offset;
                                        rows.push(pair_row(
                                            &stage,
                                            index,
                                            offset as f32 * slot,
                                            1.,
                                            offset == 1,
                                        ));
                                    }
                                }
                            }
                            None => placeholder = Some(interlude_dots(font, accent, breath)),
                        }
                    }
                } else if self.stroke.is_none() {
                    placeholder = Some(interlude_dots(font, accent, breath));
                }
            }
            Some(MusicLyrics::Plain { text, .. }) => {
                self.pair = None;
                self.swap = None;
                self.stroke = None;
                self.stroke_pending = None;
                let mut lines = text.lines().filter(|line| !line.trim().is_empty());
                let first = lines.next().unwrap_or_default().to_owned();
                let second = lines.next().unwrap_or_default().to_owned();
                if vertical {
                    let lines = vec![synthetic_line(first, 0), synthetic_line(second, 10_000)];
                    vertical_stage = Some(vertical_scene(
                        VerticalStage {
                            lines: &lines,
                            at,
                            font,
                            height: bounds.size.height.as_f32() - EDGE,
                            accent,
                        },
                        0,
                        None,
                        font * SLOT_RATIO,
                        Some((LineRole::Singing, LineRole::Preview)),
                    ));
                } else {
                    rows.push(plain_row(
                        first,
                        LineRole::Singing,
                        font,
                        slot,
                        0.,
                        false,
                        accent,
                    ));
                    if !second.is_empty() {
                        rows.push(plain_row(
                            second,
                            LineRole::Preview,
                            font,
                            slot,
                            slot,
                            true,
                            accent,
                        ));
                    }
                }
            }
            _ => {
                self.pair = None;
                self.swap = None;
                self.stroke = None;
                self.stroke_pending = None;
                if vertical {
                    let lines = vec![
                        synthetic_line(title.to_string(), 0),
                        synthetic_line(artist.to_string(), 10_000),
                    ];
                    vertical_stage = Some(vertical_scene(
                        VerticalStage {
                            lines: &lines,
                            at,
                            font,
                            height: bounds.size.height.as_f32() - EDGE,
                            accent,
                        },
                        0,
                        None,
                        font * SLOT_RATIO,
                        Some((LineRole::Singing, LineRole::Preview)),
                    ));
                } else {
                    rows.push(plain_row(
                        title.to_string(),
                        LineRole::Singing,
                        font,
                        slot,
                        0.,
                        false,
                        accent,
                    ));
                    rows.push(plain_row(
                        artist.to_string(),
                        LineRole::Preview,
                        font,
                        slot,
                        slot,
                        true,
                        accent,
                    ));
                }
            }
        }

        let plate_down = moving() || self.resizing();
        if moving() {
            self.pump(cx);
        }
        let hovering = self.hovering;
        let hover_changed = self.hover_changed;
        let plate = SharedString::from(format!(
            "desktop-lyrics-plate-{}-{}",
            self.hover_epoch,
            match (hover_changed, hovering) {
                (false, _) => "idle",
                (true, true) => "in",
                (true, false) => "out",
            }
        ));
        if stroke_animating {
            window.request_animation_frame();
        }
        let stage_height = slot * 2.;
        let showing_stroke = stroke_mode && self.stroke.is_some();
        // The interlude dots breathe, so a stage that shows them keeps asking for frames.
        let idle = match vertical {
            true => vertical_stage.is_none(),
            false => placeholder.is_some(),
        };
        let breathing = !showing_stroke && idle;
        if breathing {
            window.request_animation_frame();
        }
        let vertical_content =
            vertical_stage.unwrap_or_else(|| interlude_dots(font, accent, breath));
        let vertical_stage_element = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .font(typeface::lyrics())
            .overflow_hidden()
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, move |_, window, _| {
                if !locked {
                    window.start_window_move();
                }
            })
            .child(match showing_stroke {
                true => stroke_view(
                    self.stroke.as_ref().expect("checked above"),
                    font * stroke_scale,
                    bounds.size.width.as_f32(),
                    bounds.size.height.as_f32(),
                    true,
                    accent,
                    Instant::now(),
                ),
                false => vertical_content,
            })
            .into_any_element();
        let horizontal_stage = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .font(typeface::lyrics())
            .overflow_hidden()
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, move |_, window, _| {
                if !locked {
                    window.start_window_move();
                }
            })
            .child(match showing_stroke {
                true => stroke_view(
                    self.stroke.as_ref().expect("checked above"),
                    font * stroke_scale,
                    bounds.size.width.as_f32(),
                    (bounds.size.height.as_f32() - HEADER).max(0.),
                    false,
                    accent,
                    Instant::now(),
                ),
                false => div()
                    .relative()
                    .w_full()
                    .h(px(stage_height))
                    .overflow_hidden()
                    .children(rows)
                    .when_some(placeholder, |this, node| this.child(node))
                    .into_any_element(),
            })
            .into_any_element();

        div()
            .id("desktop-lyrics")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .rounded(px(RADIUS))
            .overflow_hidden()
            // Only a real move inside brings hover back; the pointer leaving is the
            // poll's call, so a stale hover state cannot fight it and flicker.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                let bounds = window.bounds();
                let inside = event.position.x >= px(0.)
                    && event.position.y >= px(0.)
                    && event.position.x <= bounds.size.width
                    && event.position.y <= bounds.size.height;
                if inside {
                    this.set_hovered(true, cx);
                }
            }))
            .with_animation(
                plate,
                Animation::new(HOVER_FADE).with_easing(ui::ease_out_cubic),
                move |this, delta| {
                    // A locked strip only floats its unlock button; it does not put the dark
                    // plate under the lyrics as an unlocked one does on hover.
                    if locked || (!hover_changed && !plate_down) {
                        return this;
                    }
                    let alpha = match (plate_down, hovering) {
                        (true, _) => INTERACT_ALPHA,
                        (false, true) => HOVER_ALPHA * delta,
                        (false, false) => HOVER_ALPHA * (1. - delta),
                    };
                    this.bg(black().opacity(alpha))
                },
            )
            .when(vertical, |this| {
                this.child(vertical_stage_element)
                    .when(!locked, |this| this.child(self.vertical_header(cx)))
            })
            .when(!vertical, |this| {
                this.when(!locked, |this| this.child(self.header(header_title, cx)))
                    .child(horizontal_stage)
            })
            .when(locked && self.hovering, |this| this.child(self.unlock(cx)))
            .when(moving() || self.resizing(), |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .rounded(px(RADIUS))
                        .border_2()
                        .border_color(white().opacity(0.4)),
                )
            })
            .when(!locked, |this| {
                this.children(ResizeDir::ALL.map(|dir| self.resize_zone(dir, cx)))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start_ms: u64, end_ms: u64) -> LyricsWord {
        LyricsWord {
            start: Duration::from_millis(start_ms),
            end: Duration::from_millis(end_ms),
            text: text.to_owned(),
            real_timed: true,
        }
    }

    #[test]
    fn fragments_keep_the_spacing_between_words() {
        let words = [word("Hello", 0, 500), word("world", 700, 1200)];
        let fragments = word_fragments("Hello world", &words).unwrap();
        assert_eq!(
            fragments,
            [("Hello ".to_owned(), 0), ("world".to_owned(), 1)]
        );
    }

    #[test]
    fn a_rest_ends_a_words_fill_where_the_word_ends() {
        let words = [word("hold", 100, 400), word("on", 900, 1200)];
        assert_eq!(
            word_window(Duration::ZERO, &words, 0),
            (Duration::ZERO, Duration::from_millis(400))
        );
        assert_eq!(
            word_window(Duration::from_millis(100), &words, 1),
            (Duration::from_millis(900), Duration::from_millis(1200))
        );
    }

    #[test]
    fn a_line_is_preview_then_singing_then_sung() {
        let line = LyricsLine {
            start: Duration::from_millis(1000),
            end: Some(Duration::from_millis(2000)),
            text: "hold on".to_owned(),
            romanized: None,
            words: Some(vec![word("hold", 1000, 1500), word("on", 1600, 2000)]),
            secondary: Vec::new(),
            voice: Default::default(),
        };
        assert_eq!(
            LineRole::at(&line, Duration::from_millis(500)),
            LineRole::Preview
        );
        assert_eq!(
            LineRole::at(&line, Duration::from_millis(1200)),
            LineRole::Singing
        );
        assert_eq!(
            LineRole::at(&line, Duration::from_millis(1800)),
            LineRole::Singing
        );
        assert_eq!(
            LineRole::at(&line, Duration::from_millis(2200)),
            LineRole::Sung
        );
    }

    #[test]
    fn the_window_size_is_the_lyric_size() {
        // The strip's height sets the font, its width only how much of a line fits.
        assert_eq!(window_font(1000., STRIP_MIN, false), MIN_FONT);
        assert_eq!(window_font(1000., STRIP_MAX, false), MAX_FONT);
        assert_eq!(window_font(500., STRIP_MAX, false), MAX_FONT);
        assert_eq!(window_font(1000., 250., false), 58.);
        assert_eq!(window_font(COLUMN_MIN, 620., true), MIN_FONT);
        assert_eq!(window_font(COLUMN_MAX, 620., true), MAX_FONT);
        assert_eq!(window_font(COLUMN_MIN, 200., true), MIN_FONT);
    }

    #[test]
    fn a_designed_size_reads_back_as_the_font_it_was_made_for() {
        for font in (20..=96).step_by(2).map(|font| font as f32) {
            assert_eq!(window_font(1000., design_height(font, false), false), font,);
            assert_eq!(
                window_font(design_width(font, true), VERTICAL_HEIGHT, true),
                font,
            );
        }
        assert_eq!(design_height(40., false), 198.);
        assert_eq!(design_width(40., true), 225.);
        assert_eq!(design_height(40., true), VERTICAL_HEIGHT);
        assert_eq!(design_width(40., false), WIDTH);
    }

    #[test]
    fn a_word_splits_into_even_character_windows() {
        let line = LyricsLine {
            start: Duration::from_millis(1000),
            end: Some(Duration::from_millis(2000)),
            text: "你好".to_owned(),
            romanized: None,
            words: Some(vec![word("你好", 1000, 2000)]),
            secondary: Vec::new(),
            voice: Default::default(),
        };
        let timings = char_timings(&line);
        assert_eq!(timings.len(), 2);
        assert_eq!(timings[0].start, Duration::from_millis(1000));
        assert_eq!(timings[1].start, Duration::from_millis(1500));
        assert_eq!(timings[1].end, Duration::from_millis(2000));
    }

    #[test]
    fn the_sweep_finishes_inside_its_window() {
        let start = Duration::from_millis(500);
        let end = Duration::from_millis(1500);
        assert_eq!(swept(start, end, start, true), 0.);
        assert_eq!(swept(start, end, end, true), 1.);
        let middle = swept(start, end, start + (end - start) / 2, true);
        assert!(middle > 0. && middle < 1.);
    }
}
