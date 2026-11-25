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
    Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App, Bounds, Context, Entity,
    FontWeight, Global, IntoElement, MouseButton, MouseMoveEvent, Render, SharedString, Task,
    TextShadow, Window,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowKind, WindowOptions, black,
    div, point, px, relative, size, white,
};
use i18n::t;
use music::{Lyrics as MusicLyrics, LyricsLine, LyricsWord};
use state::{AppSettings, Io, Lyrics, Playback, PlaybackState, Wisp};
use ui::Button;

use crate::chrome::stroke_lyrics::{
    FlyStroke, Hanzi, Pose, StrokeCell, Timing, fetch_hanzi, fit_timeline, flatten_line,
    fly_strokes, instant_line, is_strokable, next_random, STROKE_SCATTER_MS,
};

use crate::shared::transport::{next, previous, toggle};

const WIDTH: f32 = 1000.;
/// Vertical windows trade the wide strip for a tall one, two columns wide.
const VERTICAL_HEIGHT: f32 = 620.;
const VERTICAL_MIN_WIDTH: f32 = 300.;
const VERTICAL_MAX_WIDTH: f32 = 600.;
const VERTICAL_RATIO: f32 = 6.5;
/// Room above the taskbar for the window's bottom edge.
const MARGIN: f32 = 56.;
const HEADER: f32 = 48.;
const EDGE: f32 = 16.;
/// Keep the vertical toolbar and the right column clear of each other.
const VERTICAL_SAFE: f32 = 48.;
const RADIUS: f32 = 12.;
const MIN_FONT: f32 = 24.;
const MAX_FONT: f32 = 64.;
const FONT_STEP: f32 = 2.;
/// Line slot height and the stage height a font size designs for, from the reference.
const SLOT_RATIO: f32 = 1.35;
const DESIGN_RATIO: f32 = 3.4;
const DESIGN_MIN: f32 = 140.;
const DESIGN_MAX: f32 = 320.;
/// The pair slides out over this long after its second line ends.
const SWAP: Duration = Duration::from_millis(220);
/// The dark plate that fades in under the lyrics on hover, and the heavier one that backs
/// the strip while it is being moved or resized.
const HOVER_ALPHA: f32 = 0.6;
const INTERACT_ALPHA: f32 = 0.7;
const HOVER_FADE: Duration = Duration::from_millis(300);
/// How often the pointer is checked against the window, catching leaves the hover events miss.
const HOVER_POLL: Duration = Duration::from_millis(200);
/// The lyrics ease towards the size their window now calls for, rather than snapping.
const FONT_SETTLE: Duration = Duration::from_millis(220);
/// Repaint cadence for the word sweep and the size transition.
const SWEEP_FRAME: Duration = Duration::from_millis(16);
const SWEEP_LEAST: Duration = Duration::from_millis(180);
const SWEEP_STRETCH: f32 = 1.4;
const SWEPT: f32 = 0.98;
/// How much of a word the leading edge fades over, relative to its text size.
const SWEEP_EDGE: f32 = 0.25;

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
            ..Default::default()
        },
        |window, cx| {
            window.on_window_should_close(cx, {
                let settings = settings.clone();
                move |_, cx| {
                    clear_interaction();
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
/// transparent strip.
#[cfg(target_os = "windows")]
fn hide_frame(window: &Window) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_NCRENDERING_POLICY,
        DwmSetWindowAttribute,
    };
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    make_resizable(hwnd);
    let policy = DWMNCRP_DISABLED;
    let border = DWMWA_COLOR_NONE;
    unsafe {
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

/// Set while Windows runs the move or resize loop, so the view can show its outline.
#[cfg(target_os = "windows")]
static INTERACTING: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
fn interacting() -> bool {
    INTERACTING.load(Ordering::Relaxed)
}

#[cfg(not(target_os = "windows"))]
fn interacting() -> bool {
    false
}

#[cfg(target_os = "windows")]
fn clear_interaction() {
    INTERACTING.store(false, Ordering::Relaxed);
}

#[cfg(not(target_os = "windows"))]
fn clear_interaction() {}

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

#[cfg(target_os = "windows")]
const RESIZE_SUBCLASS: usize = 1;

/// Adds the sizing frame back and owns its hit testing: the client area covers the whole
/// window, and the outer few pixels answer as resize edges instead of as content.
#[cfg(target_os = "windows")]
fn make_resizable(handle: *mut std::ffi::c_void) {
    use windows_sys::Win32::UI::Shell::SetWindowSubclass;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WS_THICKFRAME,
    };
    clear_interaction();
    unsafe {
        SetWindowSubclass(handle, Some(resize_subclass), RESIZE_SUBCLASS, 0);
        let style = GetWindowLongPtrW(handle, GWL_STYLE);
        SetWindowLongPtrW(handle, GWL_STYLE, style | WS_THICKFRAME as isize);
        SetWindowPos(
            handle,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
        );
    }
}

#[cfg(target_os = "windows")]
fn resize_hit(code: u32) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
    };
    matches!(
        code,
        HTLEFT
            | HTRIGHT
            | HTTOP
            | HTTOPLEFT
            | HTTOPRIGHT
            | HTBOTTOM
            | HTBOTTOMLEFT
            | HTBOTTOMRIGHT
    )
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn resize_subclass(
    handle: *mut std::ffi::c_void,
    message: u32,
    wparam: usize,
    lparam: isize,
    _subclass: usize,
    _data: usize,
) -> isize {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::Shell::DefSubclassProc;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, GetClientRect, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT,
        HTTOP, HTTOPLEFT, HTTOPRIGHT, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_NCCALCSIZE,
        WM_NCHITTEST, WM_NCLBUTTONDOWN,
    };

    unsafe {
        match message {
            WM_ENTERSIZEMOVE => {
                INTERACTING.store(true, Ordering::Relaxed);
                DefSubclassProc(handle, message, wparam, lparam)
            }
            WM_EXITSIZEMOVE => {
                INTERACTING.store(false, Ordering::Relaxed);
                DefSubclassProc(handle, message, wparam, lparam)
            }
            WM_NCCALCSIZE if wparam != 0 => 0,
            WM_NCHITTEST => {
                let mut point = POINT {
                    x: (lparam as u32 & 0xffff) as u16 as i16 as i32,
                    y: ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32,
                };
                let mut rect = RECT::default();
                if GetClientRect(handle, &mut rect) == 0
                    || ScreenToClient(handle, &mut point) == 0
                {
                    return DefSubclassProc(handle, message, wparam, lparam);
                }
                let dpi = GetDpiForWindow(handle);
                let border = (8. * dpi as f32 / 96.).round().max(4.) as i32;
                let left = point.x < border;
                let right = point.x >= rect.right - border;
                let top = point.y < border;
                let bottom = point.y >= rect.bottom - border;
                let edge = match (left, right, top, bottom) {
                    (true, _, true, _) => HTTOPLEFT,
                    (_, true, true, _) => HTTOPRIGHT,
                    (true, _, _, true) => HTBOTTOMLEFT,
                    (_, true, _, true) => HTBOTTOMRIGHT,
                    (true, _, _, _) => HTLEFT,
                    (_, true, _, _) => HTRIGHT,
                    (_, _, true, _) => HTTOP,
                    (_, _, _, true) => HTBOTTOM,
                    _ => return DefSubclassProc(handle, message, wparam, lparam),
                };
                edge as isize
            }
            WM_NCLBUTTONDOWN if resize_hit(wparam as u32) => {
                DefWindowProcW(handle, message, wparam, lparam);
                0
            }
            _ => DefSubclassProc(handle, message, wparam, lparam),
        }
    }
}

fn design_width(font: f32, vertical: bool) -> f32 {
    match vertical {
        true => (font * VERTICAL_RATIO)
            .round()
            .clamp(VERTICAL_MIN_WIDTH, VERTICAL_MAX_WIDTH),
        false => WIDTH,
    }
}

/// The window height a font size designs for: two 1.35em slots plus the header, or a tall
/// column that fits a whole line.
fn design_height(font: f32, vertical: bool) -> f32 {
    match vertical {
        true => VERTICAL_HEIGHT,
        false => (font * DESIGN_RATIO).round().clamp(DESIGN_MIN, DESIGN_MAX) + HEADER,
    }
}

/// The size lyrics draw at: the saved size times how far the window has strayed from the
/// geometry its size designs for. A window made shorter or narrower brings smaller lyrics.
fn effective_font(setting: f32, width: f32, height: f32, vertical: bool) -> f32 {
    let design_width = design_width(setting, vertical);
    let design_height = design_height(setting, vertical);
    let (base, actual) = match vertical {
        true => (design_width, width),
        false => (design_height - HEADER, height - HEADER),
    };
    if base <= 0. || actual <= 0. || width <= 0. || height <= 0. {
        return setting;
    }
    let factor = match vertical {
        true => (actual / base).min(height / design_height),
        false => (actual / base).min(width / design_width),
    };
    (setting * factor).round().clamp(16., 96.)
}

/// Resizes the strip for a new font size or direction, keeping its centre where it was.
fn fit_to_shape(window: &mut Window, font: f32, vertical: bool, cx: &App) {
    place_window(
        window,
        design_width(font, vertical),
        design_height(font, vertical),
        cx,
    );
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
    if let Some(hwnd) = hwnd(window) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
        };
        let scale = window.scale_factor();
        unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                (x * scale).round() as i32,
                (y * scale).round() as i32,
                (width * scale).round() as i32,
                (height * scale).round() as i32,
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
        return;
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (x, y);
    window.resize(size(px(width), px(height)));
}

#[derive(Clone, Copy)]
struct Swap {
    from: usize,
    started: Instant,
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
    /// The size on screen, easing towards what the window measures.
    font: Option<f32>,
    font_goal: f32,
    font_from: f32,
    font_started: Option<Instant>,
    hovering: bool,
    hover_changed: bool,
    hover_epoch: u32,
    stroke: Option<StrokePair>,
    stroke_pending: Option<usize>,
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
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(HOVER_POLL).await;
            if this.update(cx, |this, cx| this.poll_hover(cx)).is_err() {
                break;
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
            font: None,
            font_goal: 0.,
            font_from: 0.,
            font_started: None,
            hovering: false,
            hover_changed: false,
            hover_epoch: 0,
            stroke: None,
            stroke_pending: None,
            hanzi: HashMap::new(),
            fetching: HashSet::new(),
            stroke_tasks: Vec::new(),
        }
    }

    fn set_hovered(&mut self, want: bool, cx: &mut Context<Self>) {
        if self.hovering == want {
            return;
        }
        self.hovering = want;
        self.hover_changed = true;
        self.hover_epoch = self.hover_epoch.wrapping_add(1);
        cx.notify();
    }

    /// Keeps hover and the move/resize outline honest when the pointer leaves without a leave
    /// message, which happens when the window moves under the cursor during a drag.
    fn poll_hover(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.host else {
            return;
        };
        let inside = cursor_over_window(host);
        let held = left_button_down();
        if !held && interacting() {
            clear_interaction();
        }
        let want = inside || (held && interacting());
        self.set_hovered(want, cx);
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
        if self.stroke.as_ref().is_some_and(|scene| scene.first == pair) {
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
            let last_offset = timings
                .last()
                .map(|timing| timing.trigger_ms)
                .unwrap_or(0.)
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

    fn change_font(&mut self, delta: f32, cx: &mut Context<Self>) {
        let size = self.settings.read(cx).desktop_lyrics_font_size();
        self.settings.update(cx, |settings, cx| {
            settings.set_desktop_lyrics_font_size((size + delta).clamp(MIN_FONT, MAX_FONT), cx)
        });
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

    fn display(&self, vertical: bool, size: f32, cx: &mut Context<Self>) -> AnyElement {
        let font = SharedString::from(format!("{}", size.round() as i32));
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
                Button::new("desktop-lyrics-font-down")
                    .ghost()
                    .small()
                    .icon("icons/minus.svg")
                    .tint(white())
                    .tooltip_above("desktop-lyrics-font-down")
                    .on_click(cx.listener(|this, _, _, cx| this.change_font(-FONT_STEP, cx))),
            )
            .child(
                div()
                    .flex_none()
                    .min_w(px(20.))
                    .text_center()
                    .text_size(px(11.))
                    .text_color(white().opacity(0.7))
                    .child(font),
            )
            .child(
                Button::new("desktop-lyrics-font-up")
                    .ghost()
                    .small()
                    .icon("icons/plus.svg")
                    .tint(white())
                    .tooltip_above("desktop-lyrics-font-up")
                    .on_click(cx.listener(|this, _, _, cx| this.change_font(FONT_STEP, cx))),
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

    fn header(&self, title: SharedString, size: f32, cx: &mut Context<Self>) -> AnyElement {
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
            .child(self.display(false, size, cx))
            .into_any_element()
    }

    fn vertical_header(&self, size: f32, cx: &mut Context<Self>) -> AnyElement {
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
            .child(self.display(true, size, cx))
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
        shadows.push(
            TextShadow::new(black().opacity(0.55)).offset(px(edge * x), px(edge * y)),
        );
    }
    for (x, y) in [(0.7, 0.7), (-0.7, 0.7), (0.7, -0.7), (-0.7, -0.7)] {
        shadows.push(
            TextShadow::new(black().opacity(0.4))
                .offset(px(edge * x), px(edge * y)),
        );
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
    fn color(self) -> gpui::Hsla {
        match self {
            Self::Singing | Self::Sung => white(),
            Self::Preview => white().opacity(0.45),
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

fn line_text(line: &LyricsLine, at: Duration, role: LineRole, size: f32) -> AnyElement {
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
            .font_weight(FontWeight::SEMIBOLD);
        let row = fragments.into_iter().fold(row, |row, (text, word)| {
            let (start, end) = word_window(line.start, words, word);
            let share = swept(start, end, at, word + 1 >= words.len());
            let cell = div()
                .relative()
                .flex_none()
                .whitespace_nowrap()
                .text_color(white().opacity(0.32))
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
                        .text_color(white())
                        .when(share < 1., |this| this.fade_sides(px(0.), px(edge)))
                        .child(
                            div()
                                .whitespace_nowrap()
                                .child(SharedString::from(text)),
                        ),
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
        .text_color(role.color())
        .font_weight(FontWeight::SEMIBOLD)
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

fn plain_line(text: &str, role: LineRole, size: f32) -> AnyElement {
    div()
        .whitespace_nowrap()
        .text_size(px(size))
        .line_height(relative(SLOT_RATIO))
        .text_shadow(shadows(size))
        .text_color(role.color())
        .font_weight(FontWeight::SEMIBOLD)
        .child(SharedString::from(text.to_owned()))
        .into_any_element()
}

#[derive(Clone, Copy)]
struct Stage<'a> {
    lines: &'a [LyricsLine],
    at: Duration,
    font: f32,
    slot: f32,
}

fn pair_row(stage: &Stage, index: usize, top: f32, opacity: f32, tail: bool) -> AnyElement {
    let text = stage
        .lines
        .get(index)
        .map(|line| line_text(line, stage.at, LineRole::at(line, stage.at), stage.font))
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

fn plain_row(text: String, role: LineRole, font: f32, slot: f32, top: f32, tail: bool) -> AnyElement {
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
        .child(plain_line(&text, role, font))
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
            let chars = word.text.chars().filter(|ch| !ch.is_whitespace()).collect::<Vec<_>>();
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
) -> AnyElement {
    let width = (font * 1.2).max(1.);
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
        .text_color(role.color())
        .font_weight(FontWeight::SEMIBOLD)
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
                        .text_color(white())
                        .when(share < 1., |this| this.fade_edges(px(0.), px(font * SWEEP_EDGE)))
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
    align_end: bool,
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
        .when(align_end, |this| this.items_end())
        .when(!align_end, |this| this.items_start())
        .gap(px((font * 0.12).max(2.)));
    let chunks = timings.chunks(per).collect::<Vec<_>>();
    for chunk in chunks {
        let mut column = div().flex().flex_col().items_center();
        for timing in chunk {
            let tail = std::ptr::eq(timing, &timings[last]);
            column = column.child(vertical_char(timing, at, role, font, slot, tail));
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
}

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
        .map(|line| vertical_line(line, stage.at, left_role, stage.font, stage.height, false))
        .unwrap_or_else(|| div().into_any_element());
    let right = second
        .map(|line| vertical_line(line, stage.at, right_role, stage.font, stage.height, true))
        .unwrap_or_else(|| div().into_any_element());
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
                .justify_start()
                .overflow_hidden()
                .pl(px(EDGE / 2.))
                .pr(px(4.))
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
                .justify_end()
                .overflow_hidden()
                .pl(px(4.))
                .pr(px(VERTICAL_SAFE))
                .child(right),
        )
        .into_any_element()
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

fn vertical_scene(
    stage: VerticalStage<'_>,
    pair: usize,
    swap: Option<(usize, f32)>,
    slide: f32,
    roles: Option<(LineRole, LineRole)>,
) -> AnyElement {
    let mut scene = div().relative().size_full();
    if let Some((from, eased)) = swap {
        // Both pair columns leave towards their own edge, and the incoming pair
        // arrives from the same side.
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
                let progress =
                    ((elapsed - stroke.delay_ms) / stroke.duration_ms).clamp(0., 1.);
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

fn stroke_cell(
    item: &StrokeChar,
    cell: f32,
    color: gpui::Hsla,
    out_at: Option<Instant>,
    now: Instant,
) -> AnyElement {
    let (poses, alpha) = stroke_poses(item, out_at, now);
    let content = match item.data.as_ref() {
        Some(hanzi) => StrokeCell {
            hanzi: hanzi.clone(),
            size: px(cell),
            poses,
            color: color.opacity(alpha),
        }
        .into_any_element(),
        None => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .opacity(alpha)
            .text_size(px((cell * 0.62).max(8.)))
            .text_color(color)
            .child(SharedString::from(item.timing.text.to_string()))
            .into_any_element(),
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
    cell: f32,
    gap: f32,
    color: gpui::Hsla,
    out_at: Option<Instant>,
    now: Instant,
) -> AnyElement {
    let mut row = div().flex().items_center().gap(px(gap));
    for item in &line.chars {
        row = row.child(stroke_cell(item, cell, color, out_at, now));
    }
    row.into_any_element()
}

fn stroke_column(
    line: &StrokeLine,
    cell: f32,
    gap: f32,
    color: gpui::Hsla,
    out_at: Option<Instant>,
    now: Instant,
) -> AnyElement {
    let mut column = div().flex().flex_col().items_center().gap(px(gap));
    for item in &line.chars {
        column = column.child(stroke_cell(item, cell, color, out_at, now));
    }
    column.into_any_element()
}

fn stroke_view(
    pair: &StrokePair,
    font: f32,
    width: f32,
    height: f32,
    vertical: bool,
    now: Instant,
) -> AnyElement {
    let max_chars = pair
        .lines
        .iter()
        .map(|line| line.chars.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let color = white();
    if vertical {
        let cell = font
            .min((height - EDGE) * 0.94 / max_chars as f32)
            .min((width - VERTICAL_SAFE - EDGE) / 2. / 1.4)
            .max(12.);
        let gap = (cell * 0.14).max(4.);
        let mut scene = div().size_full().flex().items_stretch();
        for offset in 0..2 {
            let line = &pair.lines[offset];
            let cells = stroke_column(line, cell, gap, color, pair.out_at, now);
            let column = div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .when(offset == 0, |this| this.items_start().justify_start())
                .when(offset == 1, |this| this.items_end().justify_end())
                .gap(px(gap))
                .pt(px(EDGE / 2.))
                .pb(px(EDGE / 2.))
                .when(offset == 0, |this| this.pl(px(EDGE / 2.)));
            let column = match offset {
                1 => column.pr(px(VERTICAL_SAFE)),
                _ => column,
            };
            scene = scene.child(column.child(cells));
        }
        return scene.into_any_element();
    }
    let cell = font
        .min(height * 0.42)
        .min(width * 0.92 / max_chars as f32)
        .max(12.);
    let gap = (cell * 0.14).max(4.);
    let mut scene = div().size_full().flex().flex_col();
    for offset in 0..2 {
        let line = &pair.lines[offset];
        let cells = stroke_cells(line, cell, gap, color, pair.out_at, now);
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

fn placeholder_lyric(font: f32) -> AnyElement {
    div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(font))
        .text_color(white().opacity(0.6))
        .text_shadow(shadows(font))
        .child("♪")
        .into_any_element()
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
        if !stroke_mode {
            self.stroke = None;
            self.stroke_pending = None;
        }
        let bounds = window.bounds();
        let shape_changed = self.fit != Some((setting_font, vertical));
        // While the window still has its old shape, draw at the saved size instead of
        // reading a huge horizontal size out of a tall window (or the reverse).
        let goal = match shape_changed {
            true => setting_font,
            false => effective_font(
                setting_font,
                bounds.size.width.as_f32(),
                bounds.size.height.as_f32(),
                vertical,
            ),
        };
        let now = Instant::now();
        match self.font {
            None => {
                self.font = Some(goal);
                self.font_goal = goal;
            }
            Some(current) => {
                if (goal - self.font_goal).abs() >= 0.5 {
                    self.font_goal = goal;
                    self.font_from = current;
                    self.font_started = Some(now);
                }
            }
        }
        if let Some(started) = self.font_started {
            let progress =
                ((now - started).as_secs_f32() / FONT_SETTLE.as_secs_f32()).clamp(0., 1.);
            if progress >= 1. {
                self.font = Some(self.font_goal);
                self.font_started = None;
            } else {
                let eased = ui::ease_out_cubic(progress);
                self.font = Some(self.font_from + (self.font_goal - self.font_from) * eased);
                self.pump(cx);
            }
        } else if self.font != Some(self.font_goal) {
            self.font = Some(self.font_goal);
        }
        // Whole-pixel sizes rasterize cleanly; the eased value above only paces the steps.
        let font = self.font.unwrap_or(goal).round().max(1.);
        let slot = font * SLOT_RATIO;
        // Changing the saved size or direction resizes the strip once; the effective size
        // then follows whatever the window measures.
        if shape_changed {
            self.fit = Some((setting_font, vertical));
            let target_width = design_width(setting_font, vertical);
            let target_height = design_height(setting_font, vertical);
            if (bounds.size.width.as_f32() - target_width).abs() > 1.
                || (bounds.size.height.as_f32() - target_height).abs() > 1.
            {
                cx.defer_in(window, move |_, window, cx| {
                    fit_to_shape(window, setting_font, vertical, cx);
                    // SetWindowPos settles the native size; read it back before the next
                    // frame so layout does not keep drawing at the old shape.
                    window.bounds_changed(cx);
                    cx.notify();
                });
            }
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
                    if self
                        .swap
                        .is_some_and(|swap| swap.started.elapsed() >= SWAP)
                    {
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
                                let swap = self
                                    .swap
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
                                    },
                                    pair,
                                    swap,
                                    slot,
                                    None,
                                ));
                            }
                            None => placeholder = Some(placeholder_lyric(font)),
                        }
                    } else {
                        let stage = Stage {
                            lines: &lines,
                            at,
                            font,
                            slot,
                        };
                        match self.pair.filter(|pair| *pair < lines.len()) {
                            Some(pair) => {
                                if let Some(swap) = self.swap.filter(|swap| swap.from < lines.len()) {
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
                            None => placeholder = Some(placeholder_lyric(font)),
                        }
                    }
                } else if self.stroke.is_none() {
                    placeholder = Some(placeholder_lyric(font));
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
                    ));
                    if !second.is_empty() {
                        rows.push(plain_row(
                            second,
                            LineRole::Preview,
                            font,
                            slot,
                            slot,
                            true,
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
                    ));
                    rows.push(plain_row(
                        artist.to_string(),
                        LineRole::Preview,
                        font,
                        slot,
                        slot,
                        true,
                    ));
                }
            }
        }

        let interacting = interacting();
        if interacting {
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
        let vertical_content = vertical_stage.unwrap_or_else(|| placeholder_lyric(font));
        let vertical_stage_element = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .overflow_hidden()
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .child(match showing_stroke {
                true => stroke_view(
                    self.stroke.as_ref().expect("checked above"),
                    font,
                    bounds.size.width.as_f32(),
                    bounds.size.height.as_f32(),
                    true,
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
            .overflow_hidden()
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .child(match showing_stroke {
                true => stroke_view(
                    self.stroke.as_ref().expect("checked above"),
                    font,
                    bounds.size.width.as_f32(),
                    (bounds.size.height.as_f32() - HEADER).max(0.),
                    false,
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
                    if !hover_changed && !interacting {
                        return this;
                    }
                    let alpha = match (interacting, hovering) {
                        (true, _) => INTERACT_ALPHA,
                        (false, true) => HOVER_ALPHA * delta,
                        (false, false) => HOVER_ALPHA * (1. - delta),
                    };
                    this.bg(black().opacity(alpha))
                },
            )
            .when(vertical, |this| {
                this.child(vertical_stage_element)
                    .child(self.vertical_header(setting_font, cx))
            })
            .when(!vertical, |this| {
                this.child(self.header(header_title, setting_font, cx))
                    .child(horizontal_stage)
            })
            .when(interacting, |this| {
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
            words: Some(vec![
                word("hold", 1000, 1500),
                word("on", 1600, 2000),
            ]),
            secondary: Vec::new(),
            voice: Default::default(),
        };
        assert_eq!(LineRole::at(&line, Duration::from_millis(500)), LineRole::Preview);
        assert_eq!(LineRole::at(&line, Duration::from_millis(1200)), LineRole::Singing);
        assert_eq!(LineRole::at(&line, Duration::from_millis(1800)), LineRole::Singing);
        assert_eq!(LineRole::at(&line, Duration::from_millis(2200)), LineRole::Sung);
    }

    #[test]
    fn the_font_follows_the_window_but_not_under_sixteen() {
        assert_eq!(design_height(40., false), 188.);
        assert_eq!(design_height(64., false), 266.);
        assert_eq!(design_height(40., true), 620.);
        assert_eq!(design_width(40., true), 300.);
        assert_eq!(effective_font(40., 1000., 188., false), 40.);
        assert!(effective_font(40., 1000., 120., false) < 40.);
        assert_eq!(effective_font(40., 500., 188., false), 20.);
        assert_eq!(effective_font(40., 200., 60., false), 16.);
        assert_eq!(effective_font(40., 400., 620., true), 40.);
        assert!(effective_font(40., 200., 620., true) < 40.);
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
