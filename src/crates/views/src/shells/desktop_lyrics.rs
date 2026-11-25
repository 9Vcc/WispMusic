//! The desktop lyrics window: a transparent, always-on-top strip that follows playback.
//!
//! It is its own tool window, so it stays above other applications and out of the taskbar,
//! and it never takes focus when it opens. The settings toggle is the only source of truth:
//! the installer watches it and opens or closes this window accordingly.

use std::time::Duration;

use gpui::Task;
use gpui::prelude::*;
use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, Entity, FontWeight, Global, IntoElement,
    MouseButton, Render, SharedString, TextShadow, Window, WindowBackgroundAppearance,
    WindowBounds, WindowControlArea, WindowKind, WindowOptions, black, div, point, px, relative,
    size, white,
};
use i18n::t;
use music::{Lyrics as MusicLyrics, LyricsLine, LyricsWord};
use state::{AppSettings, Lyrics, Playback, PlaybackState, Wisp};
use ui::Button;

use crate::shared::transport::{next, previous, toggle};

const WIDTH: f32 = 1000.;
const HEIGHT: f32 = 188.;
/// Room above the taskbar for the window's bottom edge.
const MARGIN: f32 = 56.;
const HEADER: f32 = 48.;
const MIN_FONT: f32 = 24.;
const MAX_FONT: f32 = 48.;
const FONT_STEP: f32 = 2.;
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
            let y = area.origin.y.as_f32() + area.size.height.as_f32() - HEIGHT - MARGIN;
            (point(px(x), px(y)), size(px(width), px(HEIGHT)))
        }
        None => (point(px(100.), px(100.)), size(px(WIDTH), px(HEIGHT))),
    };

    let playback = Wisp::global(cx).playback.clone();
    let lyrics = Wisp::global(cx).lyrics.clone();
    let settings = Wisp::global(cx).settings.clone();
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

/// Hides the DWM frame, which otherwise draws an inactive border and shadow around the
/// transparent strip.
#[cfg(target_os = "windows")]
fn hide_frame(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_NCRENDERING_POLICY,
        DwmSetWindowAttribute,
    };
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as *mut std::ffi::c_void;
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

struct DesktopLyric {
    playback: Entity<Playback>,
    lyrics: Entity<Lyrics>,
    settings: Entity<AppSettings>,
    framed: bool,
    sweeping: Option<Task<()>>,
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
                            .text_size(px(12.))
                            .text_color(white().opacity(0.75))
                            .child(title),
                    ),
            );
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
            .child(previous(&self.playback, false, cx).tint(white()))
            .child(toggle(&self.playback, false, false, cx).tint(white()))
            .child(next(&self.playback, false, cx).tint(white()))
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
            .child(controls)
            .into_any_element()
    }
}

fn shadow() -> TextShadow {
    TextShadow::new(black().opacity(0.7)).blur(px(6.))
}

fn line_text(line: &LyricsLine, at: Duration, active: bool, size: f32) -> AnyElement {
    if active
        && let Some(words) = line.words.as_deref()
        && let Some(fragments) = word_fragments(&line.text, words)
    {
        let edge = size * SWEEP_EDGE;
        let row = div()
            .flex()
            .items_center()
            .whitespace_nowrap()
            .text_size(px(size))
            .line_height(relative(1.2))
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
        .line_height(relative(1.2))
        .text_shadow([shadow()])
        .text_color(match active {
            true => white(),
            false => white().opacity(0.45),
        })
        .when(active, |this| this.font_weight(FontWeight::SEMIBOLD))
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

fn plain_line(text: &str, active: bool, size: f32) -> AnyElement {
    div()
        .whitespace_nowrap()
        .text_size(px(size))
        .line_height(relative(1.2))
        .text_shadow([shadow()])
        .text_color(match active {
            true => white(),
            false => white().opacity(0.45),
        })
        .when(active, |this| this.font_weight(FontWeight::SEMIBOLD))
        .child(SharedString::from(text.to_owned()))
        .into_any_element()
}

impl Render for DesktopLyric {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.framed {
            self.framed = true;
            hide_frame(window);
        }
        let size = self.settings.read(cx).desktop_lyrics_font_size();
        let (track, position, playing) = {
            let playback = self.playback.read(cx);
            (
                playback.shown_track(cx),
                playback.live_position(),
                matches!(playback.state(), PlaybackState::Playing),
            )
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

        let (current, next_line) = match lyric {
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
                let current = match active {
                    Some(index) => line_text(&lines[index], at, true, size),
                    None => plain_line("♪", false, size),
                };
                let next_line = match active {
                    Some(index) => lines
                        .get(index + 1)
                        .map(|line| line_text(line, at, false, size))
                        .unwrap_or_else(|| plain_line("", false, size)),
                    None => lines
                        .first()
                        .map(|line| line_text(line, at, false, size))
                        .unwrap_or_else(|| plain_line("", false, size)),
                };
                (current, next_line)
            }
            Some(MusicLyrics::Plain { text, .. }) => {
                let mut lines = text.lines().filter(|line| !line.trim().is_empty());
                let current = lines
                    .next()
                    .map(|line| plain_line(line, true, size))
                    .unwrap_or_else(|| plain_line("", true, size));
                let next_line = lines
                    .next()
                    .map(|line| plain_line(line, false, size))
                    .unwrap_or_else(|| plain_line("", false, size));
                (current, next_line)
            }
            _ => (
                plain_line(&title, true, size),
                plain_line(&artist, false, size),
            ),
        };

        div()
            .id("desktop-lyrics")
            .group("desktop-lyrics")
            .size_full()
            .flex()
            .flex_col()
            .child(self.header(header_title, size, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(4.))
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
                    .px(px(32.))
                    .child(
                        div()
                            .w_full()
                            .overflow_hidden()
                            .flex()
                            .justify_start()
                            .child(current),
                    )
                    .child(
                        div()
                            .w_full()
                            .overflow_hidden()
                            .flex()
                            .justify_end()
                            .child(next_line),
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
    fn the_sweep_finishes_inside_its_window() {
        let start = Duration::from_millis(500);
        let end = Duration::from_millis(1500);
        assert_eq!(swept(start, end, start, true), 0.);
        assert_eq!(swept(start, end, end, true), 1.);
        let middle = swept(start, end, start + (end - start) / 2, true);
        assert!(middle > 0. && middle < 1.);
    }
}
