//! A small always-on-top player with the cover, the track, transport and a seek bar.
//!
//! It lives in its own borderless window: a tool window on Windows, so it stays out of the
//! taskbar. Entering it hides the main window. The restore button brings the main window back
//! and closes this one; the close button only hides this one, and the tray brings it back,
//! which matches the old mini player.

use gpui::prelude::*;
use gpui::{
    AnyWindowHandle, App, Bounds, Context, Entity, Global, IntoElement, MouseButton, Render,
    SharedString, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div,
    point, px, relative, size,
};
use i18n::t;
use state::{AppSettings, Playback, Wisp};
use ui::{ActiveTheme as _, Artwork, Button};

use crate::shared::transport::{next, previous, toggle};

const WIDTH: f32 = 360.;
const HEIGHT: f32 = 72.;
/// The transparent margin between the window and its rounded card.
const FRAME: f32 = 4.;
/// The card's inner spacing, cover and type, all fixed with the window.
const PAD: f32 = 12.;
const GAP: f32 = 8.;
const COVER: f32 = 44.;
const COVER_RADIUS: f32 = 8.;
const CARD_RADIUS: f32 = 14.;
const TITLE_SIZE: f32 = 13.;
const CAPTION_SIZE: f32 = 11.;
/// The seek bar's inset inside the card, its hit height, and the line inside that.
const BAR_INSET: f32 = 12.;
const BAR_HIT: f32 = 10.;
const BAR_LINE: f32 = 4.;
const BAR_WEIGHT: f32 = 3.;

#[derive(Clone, Copy)]
struct MiniSession {
    main: AnyWindowHandle,
    mini: AnyWindowHandle,
}

impl Global for MiniSession {}

pub(crate) fn enter(window: &mut Window, cx: &mut App) {
    if let Some(session) = cx.try_global::<MiniSession>().copied() {
        let alive = session
            .mini
            .update(cx, |_, window, _| surface(window))
            .is_ok();
        if alive {
            hide_native(window);
            return;
        }
        cx.remove_global::<MiniSession>();
    }
    let main = window.window_handle();
    let frame = window.bounds();
    let origin = point(
        frame.origin.x + (frame.size.width - px(WIDTH)) / 2.,
        frame.origin.y + frame.size.height / 4.,
    );
    let playback = Wisp::global(cx).playback.clone();
    let settings = Wisp::global(cx).settings.clone();
    let Ok(opened) = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin,
                size: size(px(WIDTH), px(HEIGHT)),
            })),
            window_background: WindowBackgroundAppearance::Transparent,
            kind: WindowKind::PopUp,
            titlebar: None,
            is_resizable: false,
            is_minimizable: false,
            is_movable: true,
            app_owns_titlebar_drag: true,
            focus: true,
            show: true,
            ..Default::default()
        },
        |window, cx| {
            window.on_window_should_close(cx, |_, cx| {
                reveal_main(cx);
                cx.remove_global::<MiniSession>();
                true
            });
            cx.new(|cx| MiniPlayer::new(playback, settings, cx))
        },
    ) else {
        return;
    };
    cx.set_global(MiniSession {
        main,
        mini: opened.into(),
    });
    hide_native(window);
}

/// Shows and focuses the mini player when it exists. The tray uses this while mini mode is on.
pub fn reveal(cx: &mut App) -> bool {
    let Some(session) = cx.try_global::<MiniSession>().copied() else {
        return false;
    };
    let alive = session
        .mini
        .update(cx, |_, window, _| surface(window))
        .is_ok();
    if !alive {
        cx.remove_global::<MiniSession>();
    }
    alive
}

/// Leaves mini mode: closes the mini window and shows the main one again.
pub fn dismiss(cx: &mut App) -> bool {
    let Some(session) = cx.try_global::<MiniSession>().copied() else {
        return false;
    };
    cx.remove_global::<MiniSession>();
    session
        .mini
        .update(cx, |_, window, _| window.remove_window())
        .ok();
    session
        .main
        .update(cx, |_, window, _| surface(window))
        .is_ok()
}

fn reveal_main(cx: &mut App) {
    let Some(session) = cx.try_global::<MiniSession>().copied() else {
        return;
    };
    session.main.update(cx, |_, window, _| surface(window)).ok();
}

fn leave(window: &mut Window, cx: &mut App) {
    reveal_main(cx);
    cx.remove_global::<MiniSession>();
    window.remove_window();
}

#[cfg(target_os = "windows")]
fn hwnd(window: &Window) -> Option<*mut std::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let RawWindowHandle::Win32(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw()
    else {
        return None;
    };
    Some(handle.hwnd.get() as *mut std::ffi::c_void)
}

#[cfg(target_os = "windows")]
fn hide_native(window: &mut Window) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
    if let Some(handle) = hwnd(window) {
        unsafe {
            ShowWindow(handle, SW_HIDE);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn hide_native(window: &mut Window) {
    window.minimize_window();
}

#[cfg(target_os = "windows")]
fn show_native(window: &mut Window) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOW, SetForegroundWindow, ShowWindow};
    if let Some(handle) = hwnd(window) {
        unsafe {
            ShowWindow(handle, SW_SHOW);
            SetForegroundWindow(handle);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn show_native(_window: &mut Window) {}

fn surface(window: &mut Window) {
    show_native(window);
    window.activate_window();
    window.refresh();
}

struct MiniPlayer {
    playback: Entity<Playback>,
}

impl MiniPlayer {
    fn new(
        playback: Entity<Playback>,
        settings: Entity<AppSettings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let ticks = playback.read(cx).ticks().clone();
        cx.observe(&ticks, |_, _, cx| cx.notify()).detach();
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        Self { playback }
    }
}

impl Render for MiniPlayer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let playback = self.playback.read(cx);
        let track = playback.shown_track(cx);
        let (position, duration) = playback.shown_time(cx);
        let title = track
            .as_ref()
            .map(|track| SharedString::from(track.name.clone()))
            .unwrap_or_else(|| SharedString::from("WispMusic"));
        let artist = track
            .as_ref()
            .map(|track| SharedString::from(track.artists.clone()))
            .filter(|artists| !artists.is_empty())
            .unwrap_or_else(|| t!("player-nothing-playing"));
        let cover = track.and_then(|track| track.cover);
        let ratio = match duration.as_secs_f32() > 0. {
            true => (position.as_secs_f32() / duration.as_secs_f32()).clamp(0., 1.),
            false => 0.,
        };

        div().size_full().p(px(FRAME)).child(
            div()
                .id("mini-card")
                .relative()
                .size_full()
                .rounded(px(CARD_RADIUS))
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .overflow_hidden()
                .window_control_area(gpui::WindowControlArea::Drag)
                .on_mouse_down(MouseButton::Left, |_, window, _| {
                    window.start_window_move();
                })
                .child(
                    div()
                        .flex()
                        .h_full()
                        .items_center()
                        .gap(px(GAP))
                        .pl(px(PAD))
                        .pr(px(GAP))
                        .pb(px(BAR_HIT + 2.))
                        .child(
                            Artwork::new(cover)
                                .size(px(COVER))
                                .corner_radius(px(COVER_RADIUS)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .flex_col()
                                .gap(px(1.))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(TITLE_SIZE))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme.popover_foreground)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(CAPTION_SIZE))
                                        .text_color(theme.muted_foreground)
                                        .child(artist),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .child(previous(&self.playback, false, cx))
                                .child(toggle(&self.playback, false, false, cx))
                                .child(next(&self.playback, false, cx)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .child(
                                    Button::new("mini-restore")
                                        .ghost()
                                        .small()
                                        .icon("icons/square-arrow-out-up-right.svg")
                                        .tooltip_above("mini-restore")
                                        .on_click(|_, window, cx| {
                                            cx.stop_propagation();
                                            leave(window, cx);
                                        }),
                                )
                                .child(
                                    Button::new("mini-close")
                                        .ghost()
                                        .small()
                                        .icon("icons/x.svg")
                                        .tooltip_above("mini-close")
                                        .on_click(|_, window, _| hide_native(window)),
                                ),
                        ),
                )
                .child({
                    let playback = self.playback.clone();
                    div()
                        .id("mini-progress")
                        .absolute()
                        .left(px(BAR_INSET))
                        .right(px(BAR_INSET))
                        .bottom(px(FRAME))
                        .h(px(BAR_HIT))
                        .role(gpui::accesskit::Role::Slider)
                        .aria_label(t!("song-position"))
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                            cx.stop_propagation();
                            let width = window.viewport_size().width;
                            let left = px(FRAME + BAR_INSET);
                            let right = width - px(FRAME + BAR_INSET);
                            let span = (right - left).max(px(1.));
                            let ratio = ((event.position.x - left) / span).clamp(0., 1.);
                            let duration = playback
                                .read(cx)
                                .track()
                                .map(|track| track.duration)
                                .unwrap_or_default();
                            if duration.is_zero() {
                                return;
                            }
                            playback.update(cx, |playback, cx| {
                                playback.seek(duration.mul_f32(ratio), cx)
                            });
                        })
                        .child(
                            div()
                                .absolute()
                                .left(px(0.))
                                .right(px(0.))
                                .top(px(BAR_LINE))
                                .h(px(BAR_WEIGHT))
                                .rounded_full()
                                .bg(theme.border),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(0.))
                                .top(px(BAR_LINE))
                                .h(px(BAR_WEIGHT))
                                .w(relative(ratio))
                                .rounded_full()
                                .bg(theme.popover_foreground),
                        )
                }),
        )
    }
}
