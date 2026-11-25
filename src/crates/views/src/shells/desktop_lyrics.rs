//! The desktop lyrics window: a transparent, always-on-top strip that follows playback.
//!
//! It is its own tool window, so it stays above other applications and out of the taskbar,
//! and it never takes focus when it opens. The settings toggle is the only source of truth:
//! the installer watches it and opens or closes this window accordingly.

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App, Bounds, Context, Entity,
    FontWeight, Global, IntoElement, MouseButton, Render, SharedString, Task, TextShadow, Window,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowKind, WindowOptions, black,
    div, point, px, relative, size, white,
};
use i18n::t;
use music::{Lyrics as MusicLyrics, LyricsLine, LyricsWord};
use state::{AppSettings, Lyrics, Playback, PlaybackState, Wisp};
use ui::Button;

use crate::shared::transport::{next, previous, toggle};

const WIDTH: f32 = 1000.;
/// Room above the taskbar for the window's bottom edge.
const MARGIN: f32 = 56.;
const HEADER: f32 = 48.;
const EDGE: f32 = 16.;
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
/// The dark plate that fades in under the lyrics on hover.
const HOVER_ALPHA: f32 = 0.6;
const HOVER_FADE: Duration = Duration::from_millis(300);
/// Repaint cadence while a line fills word by word.
const SWEEP_FRAME: Duration = Duration::from_millis(22);
const SWEEP_LEAST: Duration = Duration::from_millis(180);
const SWEEP_STRETCH: f32 = 1.4;
const SWEPT: f32 = 0.98;
const LANDING: f32 = 0.2;
/// How much of a word the leading edge fades over, relative to its text size.
const SWEEP_EDGE: f32 = 0.22;

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
    let height = design_height(font);

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
            let width = WIDTH.min((available - 64.).max(320.));
            let x = area.origin.x.as_f32() + (available - width) / 2.;
            let y = area.origin.y.as_f32() + area.size.height.as_f32() - height - MARGIN;
            (point(px(x), px(y)), size(px(width), px(height)))
        }
        None => (point(px(100.), px(100.)), size(px(WIDTH), px(height))),
    };
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size })),
            window_background: WindowBackgroundAppearance::Transparent,
            kind: WindowKind::PopUp,
            titlebar: None,
            is_resizable: false,
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

/// The window height a font size designs for: two 1.35em slots plus the header.
fn design_height(font: f32) -> f32 {
    (font * DESIGN_RATIO).round().clamp(DESIGN_MIN, DESIGN_MAX) + HEADER
}

/// The size lyrics draw at: the saved size times how far the window has strayed from the
/// geometry its size designs for. A window made shorter or narrower brings smaller lyrics.
fn effective_font(setting: f32, width: f32, height: f32) -> f32 {
    let stage = design_height(setting) - HEADER;
    let actual = (height - HEADER).max(0.);
    if stage <= 0. || actual <= 0. || width <= 0. {
        return setting;
    }
    let factor = (actual / stage).min(width / WIDTH);
    (setting * factor).round().clamp(16., 96.)
}

/// Resizes the strip for a new font size, keeping its centre where it was.
fn fit_to_font(window: &mut Window, font: f32, cx: &App) {
    place_window(window, WIDTH, design_height(font), cx);
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
    sweeping: Option<Task<()>>,
    /// The pair of lines on stage. It holds still while the second line sings.
    pair: Option<usize>,
    swap: Option<Swap>,
    track: Option<String>,
    fit_font: Option<f32>,
    hovering: bool,
    hover_changed: bool,
    hover_epoch: u32,
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
        Self {
            playback,
            lyrics,
            settings,
            framed: false,
            sweeping: None,
            pair: None,
            swap: None,
            track: None,
            fit_font: None,
            hovering: false,
            hover_changed: false,
            hover_epoch: 0,
        }
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

    fn change_font(&mut self, delta: f32, cx: &mut Context<Self>) {
        let size = self.settings.read(cx).desktop_lyrics_font_size();
        self.settings.update(cx, |settings, cx| {
            settings.set_desktop_lyrics_font_size((size + delta).clamp(MIN_FONT, MAX_FONT), cx)
        });
    }

    fn header(&self, title: SharedString, size: f32, cx: &mut Context<Self>) -> AnyElement {
        let font = SharedString::from(format!("{}", size.round() as i32));
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
        let transport = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(1.))
            .rounded_full()
            .border_1()
            .border_color(white().opacity(0.15))
            .bg(black().opacity(0.45))
            .p(px(2.))
            .child(previous(&self.playback, false, cx).tint(white()))
            .child(toggle(&self.playback, false, false, cx).tint(white()))
            .child(next(&self.playback, false, cx).tint(white()));
        let controls = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(1.))
            .rounded_full()
            .border_1()
            .border_color(white().opacity(0.15))
            .bg(black().opacity(0.45))
            .p(px(2.))
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
            );

        div()
            .flex_none()
            .flex()
            .h(px(HEADER))
            .w_full()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .invisible()
            .group_hover("desktop-lyrics", |this| this.visible())
            .child(title)
            .child(transport)
            .child(controls)
            .into_any_element()
    }
}

fn shadow() -> TextShadow {
    TextShadow::new(black().opacity(0.7)).blur(px(6.))
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
            .text_shadow([shadow()])
            .font_weight(FontWeight::SEMIBOLD);
        let row = fragments.into_iter().fold(row, |row, (text, word)| {
            let (start, end) = word_window(line.start, words, word);
            let share = swept(start, end, at, word + 1 >= words.len());
            let landing = match share < 1. {
                true => ((1. - share) / LANDING).min(1.),
                false => 0.,
            };
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
                        .text_color(white())
                        .when(landing > 0., |this| {
                            this.fade_sides(px(0.), px(edge * landing))
                        })
                        .child(
                            div()
                                .whitespace_nowrap()
                                .text_shadow([shadow()])
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
        .text_shadow([shadow()])
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
        .text_shadow([shadow()])
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

fn placeholder_lyric(font: f32) -> AnyElement {
    div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(font))
        .text_color(white().opacity(0.6))
        .text_shadow([shadow()])
        .child("♪")
        .into_any_element()
}

impl Render for DesktopLyric {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.framed {
            self.framed = true;
            hide_frame(window);
        }
        let setting_font = self.settings.read(cx).desktop_lyrics_font_size();
        let bounds = window.bounds();
        let font = effective_font(
            setting_font,
            bounds.size.width.as_f32(),
            bounds.size.height.as_f32(),
        );
        let slot = font * SLOT_RATIO;
        // Changing the saved size resizes the strip once; the effective size then follows
        // whatever the window measures.
        if self.fit_font != Some(setting_font) {
            self.fit_font = Some(setting_font);
            if (bounds.size.height.as_f32() - design_height(setting_font)).abs() > 1. {
                cx.defer_in(window, move |_, window, cx| {
                    fit_to_font(window, setting_font, cx)
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
        let mut placeholder = None;
        match lyric {
            Some(MusicLyrics::Synced { lines }) => {
                let active = music::lyrics::active(&lines, at);
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
                let stage = Stage {
                    lines: &lines,
                    at,
                    font,
                    slot,
                };
                match self.pair.filter(|pair| *pair < lines.len()) {
                    Some(pair) => {
                        if let Some(swap) = self.swap.filter(|swap| swap.from < lines.len()) {
                            let progress =
                                (swap.started.elapsed().as_secs_f32() / SWAP.as_secs_f32())
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
            Some(MusicLyrics::Plain { text, .. }) => {
                self.pair = None;
                self.swap = None;
                let mut lines = text.lines().filter(|line| !line.trim().is_empty());
                rows.push(plain_row(
                    lines.next().unwrap_or_default().to_owned(),
                    LineRole::Singing,
                    font,
                    slot,
                    0.,
                    false,
                ));
                if let Some(line) = lines.next() {
                    rows.push(plain_row(
                        line.to_owned(),
                        LineRole::Preview,
                        font,
                        slot,
                        slot,
                        true,
                    ));
                }
            }
            _ => {
                self.pair = None;
                self.swap = None;
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
        let stage_height = slot * 2.;

        div()
            .id("desktop-lyrics")
            .group("desktop-lyrics")
            .size_full()
            .flex()
            .flex_col()
            .rounded(px(RADIUS))
            .overflow_hidden()
            .on_hover(cx.listener(|this, over: &bool, _, cx| {
                if this.hovering != *over {
                    this.hovering = *over;
                    this.hover_changed = true;
                    this.hover_epoch = this.hover_epoch.wrapping_add(1);
                    cx.notify();
                }
            }))
            .with_animation(
                plate,
                Animation::new(HOVER_FADE).with_easing(ui::ease_out_cubic),
                move |this, delta| {
                    if !hover_changed {
                        return this;
                    }
                    let alpha = match hovering {
                        true => HOVER_ALPHA * delta,
                        false => HOVER_ALPHA * (1. - delta),
                    };
                    this.bg(black().opacity(alpha))
                },
            )
            .child(self.header(header_title, setting_font, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .h(px(stage_height))
                            .overflow_hidden()
                            .children(rows)
                            .when_some(placeholder, |this, node| this.child(node)),
                    ),
            )
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
        assert_eq!(design_height(40.), 188.);
        assert_eq!(design_height(64.), 266.);
        assert_eq!(effective_font(40., 1000., 188.), 40.);
        assert!(effective_font(40., 1000., 120.) < 40.);
        assert_eq!(effective_font(40., 500., 188.), 20.);
        assert_eq!(effective_font(40., 200., 60.), 16.);
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
