//! The desktop lyrics window: a transparent, always-on-top strip that follows playback.
//!
//! It is its own tool window, so it stays above other applications and out of the taskbar,
//! and it never takes focus when it opens. The settings toggle is the only source of truth:
//! the installer watches it and opens or closes this window accordingly.

use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, Entity, FontWeight, Global,
    IntoElement, MouseButton, Render, SharedString, TextShadow, Window, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions, black, div, point, px, relative, size, white,
};
use i18n::t;
use music::{Lyrics as MusicLyrics, LyricsLine};
use state::{AppSettings, Lyrics, Playback, Wisp};
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
    let frame = host
        .0
        .update(cx, |_, window, _| window.bounds())
        .ok();
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
        None => (
            point(px(100.), px(100.)),
            size(px(WIDTH), px(HEIGHT)),
        ),
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

struct DesktopLyric {
    playback: Entity<Playback>,
    lyrics: Entity<Lyrics>,
    settings: Entity<AppSettings>,
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
        }
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
            .on_mouse_down(MouseButton::Left, |_, window, _| {
                window.start_window_move()
            })
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
                        this.settings.update(cx, |settings, cx| {
                            settings.set_desktop_lyrics(false, cx)
                        });
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
    let mut line_div = div()
        .whitespace_nowrap()
        .text_size(px(size))
        .line_height(relative(1.2))
        .text_shadow([shadow()])
        .text_color(match active {
            true => white(),
            false => white().opacity(0.45),
        })
        .when(active, |this| this.font_weight(FontWeight::SEMIBOLD));

    if let Some(words) = line.words.as_ref().filter(|_| active) {
        return words
            .iter()
            .fold(
                div()
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .text_size(px(size))
                    .line_height(relative(1.2))
                    .text_shadow([shadow()])
                    .font_weight(FontWeight::SEMIBOLD),
                |row, word| {
                    let lit = at >= word.start;
                    row.child(
                        div()
                            .text_shadow([shadow()])
                            .text_color(match lit {
                                true => white(),
                                false => white().opacity(0.3),
                            })
                            .child(word.text.clone()),
                    )
                },
            )
            .into_any_element();
    }
    line_div = line_div.child(line.text.clone());
    line_div.into_any_element()
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let size = self.settings.read(cx).desktop_lyrics_font_size();
        let next_size = (size * 0.72).max(18.);
        let (track, position) = {
            let playback = self.playback.read(cx);
            (playback.shown_track(cx), playback.live_position())
        };
        let (lyric, offset) = {
            let lyrics = self.lyrics.read(cx);
            (lyrics.current().map(|hit| hit.lyrics.clone()), lyrics.offset_ms())
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
                let current = match active {
                    Some(index) => line_text(&lines[index], at, true, size),
                    None => plain_line("♪", false, size),
                };
                let next_line = match active {
                    Some(index) => lines
                        .get(index + 1)
                        .map(|line| line_text(line, at, false, next_size))
                        .unwrap_or_else(|| plain_line("", false, next_size)),
                    None => lines
                        .first()
                        .map(|line| line_text(line, at, false, next_size))
                        .unwrap_or_else(|| plain_line("", false, next_size)),
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
                    .map(|line| plain_line(line, false, next_size))
                    .unwrap_or_else(|| plain_line("", false, next_size));
                (current, next_line)
            }
            _ => (
                plain_line(&title, true, size),
                plain_line(&artist, false, next_size),
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
                    .on_mouse_down(MouseButton::Left, |_, window, _| {
                        window.start_window_move()
                    })
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