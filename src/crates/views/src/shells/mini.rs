//! A small always-on-top player with the cover, the track, transport and a seek bar.
//!
//! It lives in its own borderless window: a tool window on Windows, so it stays out of the
//! taskbar. Entering it hides the main window. The restore button brings the main window back
//! and closes this one; the close button only hides this one, and the tray brings it back,
//! which matches the old mini player. Its fill follows the window's opacity, and the window
//! asks for a blurred desktop when blurring is on. The list expands to the live queue.

use std::ops::Range;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App, Bounds, Context, Entity,
    Global, IntoElement, MouseButton, MouseUpEvent, Pixels, Point, Render, ScrollStrategy,
    SharedString, Task, UniformListScrollHandle, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, div, point, px, relative, size, svg, uniform_list,
};
use i18n::t;
use state::{AppSettings, Playback, QueuePosition, Wisp};
use ui::{ActiveTheme as _, Artwork, Button, Scrubber, ScrubberState, clock, tabular};

use crate::shared::transport::{next, play_mode, previous, toggle};

const WIDTH: f32 = 360.;
const BAR: f32 = 72.;
const EXPANDED: f32 = 450.;
/// Matches the system corner radius DWM gives a rounded window.
const CARD_RADIUS: f32 = 8.;
/// The card's inner spacing, cover and type, all fixed with the window.
const PAD: f32 = 12.;
const GAP: f32 = 8.;
const COVER: f32 = 48.;
const COVER_RADIUS: f32 = 8.;
const TITLE_SIZE: f32 = 13.;
const CAPTION_SIZE: f32 = 11.;
/// The seek bar's inset inside the bar, its hit height, and the line inside that.
const BAR_INSET: f32 = 12.;
const BAR_EDGE: f32 = 4.;
const BAR_HIT: f32 = 10.;
const BAR_LINE: f32 = 4.;
const BAR_WEIGHT: f32 = 3.;
/// The seek line starts after the cover, as in the old mini player.
const TRACK_LEFT: f32 = PAD + COVER + GAP;
/// The expanded queue's row height, side padding and heading.
const ROW: f32 = 32.;
const ROW_PAD: f32 = 6.;
const HEAD: f32 = 24.;
/// How long the queue takes to grow out of the bar.
const REVEAL: Duration = Duration::from_millis(300);

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
    let desired = point(
        frame.origin.x + (frame.size.width - px(WIDTH)) / 2.,
        frame.origin.y + frame.size.height / 4.,
    );
    // Pick the place once, with room for the expanded panel. Folding never moves the window,
    // so repeated expands cannot drift.
    let origin = room(
        Bounds {
            origin: desired,
            size: size(px(WIDTH), px(BAR)),
        },
        cx,
    );
    let playback = Wisp::global(cx).playback.clone();
    let settings = Wisp::global(cx).settings.clone();
    let background = {
        let look = settings.read(cx).look();
        ui::backdrop(look.blur_window, look.transparent)
    };
    let Ok(opened) = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin,
                size: size(px(WIDTH), px(BAR)),
            })),
            // Same material as the main window: DWM rounds this surface, so the blur and its
            // shadow follow the corners instead of showing a square sheet behind the card.
            window_background: background,
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

/// Turns DWM's non-client frame on or off. It is hidden while the card folds so the tall
/// transparent window does not leave a border or shadow around the shrinking card.
#[cfg(target_os = "windows")]
fn set_window_frame(window: &Window, shown: bool) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMNCRP_ENABLED, DWMWA_BORDER_COLOR, DWMWA_COLOR_DEFAULT,
        DWMWA_COLOR_NONE, DWMWA_NCRENDERING_POLICY, DwmSetWindowAttribute,
    };
    let Some(handle) = hwnd(window) else {
        return;
    };
    let policy = match shown {
        true => DWMNCRP_ENABLED,
        false => DWMNCRP_DISABLED,
    };
    let border = match shown {
        true => DWMWA_COLOR_DEFAULT,
        false => DWMWA_COLOR_NONE,
    };
    unsafe {
        DwmSetWindowAttribute(
            handle,
            DWMWA_NCRENDERING_POLICY as u32,
            &policy as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&policy) as u32,
        );
        DwmSetWindowAttribute(
            handle,
            DWMWA_BORDER_COLOR as u32,
            &border as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&border) as u32,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn set_window_frame(_window: &Window, _shown: bool) {}

/// Restores the window material after an animation that had to hide it.
fn restore_material(window: &mut Window, cx: &App) {
    let theme = *cx.theme();
    window.set_background_appearance(ui::backdrop(theme.blur_window, theme.transparent));
    set_window_frame(window, true);
}

/// Where the expanded window fits around `frame`, keeping its top unless the screen runs out.
fn room(frame: Bounds<Pixels>, cx: &App) -> Point<Pixels> {
    let displays = cx.displays();
    let area = displays
        .iter()
        .find(|display| display.visible_bounds().contains(&frame.center()))
        .or_else(|| displays.first())
        .map(|display| display.visible_bounds());
    let Some(area) = area else {
        return frame.origin;
    };
    let mut origin = frame.origin;
    origin.x = origin.x.clamp(
        area.origin.x,
        (area.origin.x + area.size.width - px(WIDTH)).max(area.origin.x),
    );
    origin.y = origin.y.max(area.origin.y);
    if origin.y + px(EXPANDED) > area.origin.y + area.size.height {
        origin.y = (area.origin.y + area.size.height - px(EXPANDED)).max(area.origin.y);
    }
    origin
}

struct MiniPlayer {
    playback: Entity<Playback>,
    expanded: bool,
    /// Whether the queue is mounted. It stays mounted while folding so the shrinking card
    /// clips it instead of exposing a bare bar over the backdrop.
    panel: bool,
    /// Whether the card is folding with the window material hidden.
    folding: bool,
    /// Restarts the card's height animation on every fold.
    epoch: u64,
    /// Shrinks the window after a fold; the card itself animates inside the still-expanded
    /// window, so the OS is resized once per fold rather than every frame.
    finish: Option<Task<()>>,
    /// The queue's scroll, kept between renders so it does not jump to the top.
    scroll: UniformListScrollHandle,
    seek: ScrubberState,
    volume: ScrubberState,
    pending_seek: Option<f32>,
    /// What the window material and its corners were last told, so they only change with the
    /// settings.
    background: Option<(bool, bool)>,
    #[cfg(target_os = "windows")]
    rounding: Option<ui::Rounding>,
}

impl MiniPlayer {
    fn new(
        playback: Entity<Playback>,
        settings: Entity<AppSettings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let ticks = playback.read(cx).ticks().clone();
        let queue = Wisp::global(cx).queue.clone();
        cx.observe(&ticks, |_, _, cx| cx.notify()).detach();
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
        cx.observe(&queue, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        Self {
            playback,
            expanded: false,
            panel: false,
            folding: false,
            epoch: 0,
            finish: None,
            scroll: UniformListScrollHandle::new(),
            seek: ScrubberState::new("mini-seek"),
            volume: ScrubberState::new("mini-volume"),
            pending_seek: None,
            background: None,
            #[cfg(target_os = "windows")]
            rounding: None,
        }
    }

    /// Keeps the window material and its corners in step with the appearance settings, the way
    /// the main window does.
    fn sync_window(&mut self, window: &mut Window, cx: &App) {
        let theme = *cx.theme();
        let key = (theme.blur_window, theme.transparent);
        // A fold hides the material on purpose; the finishing task restores it.
        if !self.folding && self.background != Some(key) {
            self.background = Some(key);
            window.set_background_appearance(ui::backdrop(theme.blur_window, theme.transparent));
        }
        #[cfg(target_os = "windows")]
        {
            let rounding = Wisp::global(cx).settings.read(cx).window_rounding();
            if self.rounding != Some(rounding) {
                self.rounding = Some(rounding);
                state::apply_window_rounding(window, rounding, cx);
            }
        }
    }

    fn set_expanded(&mut self, expanded: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.expanded == expanded {
            return;
        }
        self.expanded = expanded;
        self.epoch = self.epoch.wrapping_add(1);
        if expanded {
            self.panel = true;
            self.finish = None;
            if self.folding {
                self.folding = false;
                restore_material(window, cx);
            }
            let sections = self.playback.read(cx).shown_queue(cx).sections;
            if sections.current {
                self.scroll
                    .scroll_to_item(sections.past, ScrollStrategy::Center);
            }
            // The window is made tall once; the card height animates inside it. The OS is
            // never resized every frame, which re-entered GPUI and stalled the motion.
            window.resize(size(px(WIDTH), px(EXPANDED)));
            cx.notify();
            return;
        }
        // Fold with the window still tall: the card shrinks inside it while the material is
        // hidden, so the uncovered part shows the desktop rather than an acrylic strip. The
        // window is shrunk once the card has finished, then the material comes back.
        self.folding = true;
        window.set_background_appearance(WindowBackgroundAppearance::Transparent);
        set_window_frame(window, false);
        let handle = window.window_handle();
        let epoch = self.epoch;
        self.finish = Some(cx.spawn(async move |this, cx| {
            // Shrink the window just before the card bottoms out, so the frame never sits
            // around an empty strip after the card has folded. The last sliver of the card
            // animation is clipped by the window and is invisible.
            cx.background_executor()
                .timer(REVEAL.saturating_sub(Duration::from_millis(70)))
                .await;
            let folding = this
                .update(cx, |this, _| {
                    (this.epoch == epoch && !this.expanded && this.panel && this.folding)
                        .then_some(())
                })
                .ok()
                .flatten()
                .is_some();
            if !folding {
                return;
            }
            handle
                .update(cx, |_, window, _| {
                    window.resize(size(px(WIDTH), px(BAR)));
                })
                .ok();
            for _ in 0..40 {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let height = handle
                    .update(cx, |_, window, _| window.viewport_size().height)
                    .ok();
                if height.is_some_and(|height| height.as_f32() <= BAR + 1.) {
                    break;
                }
            }
            let finished = this
                .update(cx, |this, _| {
                    this.epoch == epoch && !this.expanded && this.panel
                })
                .unwrap_or(false);
            if !finished {
                return;
            }
            // Bring the material back before the card turns translucent again, so the folded
            // bar never paints one frame over the bare desktop.
            handle
                .update(cx, |_, window, cx| restore_material(window, cx))
                .ok();
            this.update(cx, |this, cx| {
                if this.epoch == epoch && !this.expanded {
                    this.panel = false;
                    this.pending_seek = None;
                    this.folding = false;
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let playback = self.playback.read(cx);
        let track = playback.shown_track(cx);
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

        div()
            .id("mini-bar")
            .relative()
            .flex_none()
            .h(px(BAR))
            .w_full()
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
                    .child(
                        Artwork::new(cover)
                            .size(px(COVER))
                            .corner_radius(px(COVER_RADIUS)),
                    )
                    .child(
                        div()
                            .group("mini-info")
                            .relative()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex_col()
                            .justify_center()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(1.))
                                    .group_hover("mini-info", |style| style.opacity(0.))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(TITLE_SIZE))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme.foreground)
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
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .invisible()
                                    .group_hover("mini-info", |style| style.visible())
                                    .child(previous(&self.playback, false, cx))
                                    .child(toggle(&self.playback, false, false, cx))
                                    .child(next(&self.playback, false, cx)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .child(self.shuffle_button("mini-shuffle", cx))
                            .child(
                                Button::new("mini-expand")
                                    .ghost()
                                    .small()
                                    .icon("icons/list-music.svg")
                                    .tooltip_above(match self.expanded {
                                        true => "mini-collapse",
                                        false => "mini-expand",
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.set_expanded(!this.expanded, window, cx)
                                    })),
                            )
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
            .when(!self.expanded, |this| this.child(self.bar_progress(cx)))
    }

    fn shuffle_button(&self, id: &'static str, cx: &mut Context<Self>) -> Button {
        let theme = *cx.theme();
        let shuffle = self.playback.read(cx).shown_shuffle(cx);
        let playback = self.playback.clone();
        Button::new(id)
            .ghost()
            .small()
            .icon("icons/shuffle.svg")
            .tooltip_above("player-shuffle")
            .selected(shuffle)
            .tint(match shuffle {
                true => theme.primary,
                false => theme.muted_foreground,
            })
            .on_click(move |_, _, cx| {
                playback.update(cx, |playback, cx| playback.toggle_shuffle(cx))
            })
    }

    fn bar_progress(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let playback = self.playback.read(cx);
        let (position, duration) = playback.shown_time(cx);
        let ratio = match duration.as_secs_f32() > 0. {
            true => (position.as_secs_f32() / duration.as_secs_f32()).clamp(0., 1.),
            false => 0.,
        };
        let playback = self.playback.clone();

        div()
            .id("mini-progress")
            .absolute()
            .left(px(TRACK_LEFT))
            .right(px(BAR_INSET))
            .bottom(px(BAR_EDGE))
            .h(px(BAR_HIT))
            .role(gpui::accesskit::Role::Slider)
            .aria_label(t!("song-position"))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                cx.stop_propagation();
                let width = window.viewport_size().width;
                let left = px(TRACK_LEFT);
                let right = width - px(BAR_INSET);
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
                    .bg(theme.foreground.opacity(0.12)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(BAR_LINE))
                    .h(px(BAR_WEIGHT))
                    .w(relative(ratio))
                    .rounded_full()
                    .bg(theme.primary),
            )
    }

    fn seek_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let (elapsed, total) = self.playback.read(cx).shown_time(cx);
        let base = match total.as_secs_f32() > 0. {
            true => (elapsed.as_secs_f32() / total.as_secs_f32()).clamp(0., 1.),
            false => 0.,
        };
        let fraction = self.pending_seek.unwrap_or(base);
        let shown = total.mul_f32(fraction);
        let caption = |text: SharedString, align_end: bool| {
            div()
                .w(px(32.))
                .flex_none()
                .whitespace_nowrap()
                .text_size(px(CAPTION_SIZE))
                .font_features(tabular())
                .text_color(theme.muted_foreground)
                .when_else(align_end, |this| this.text_right(), |this| this.text_left())
                .child(text)
        };

        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(ROW_PAD))
            .pt(px(2.))
            .child(caption(clock(shown), true))
            .child(
                div().flex_1().min_w_0().child(
                    Scrubber::new(&self.seek, fraction)
                        .name(t!("song-position"))
                        .value(clock(shown))
                        .colors(
                            theme.progress_bar,
                            theme.foreground.opacity(0.12),
                            theme.foreground,
                        )
                        .enabled(!total.is_zero())
                        .on_move(cx.listener(|this, fraction: &f32, _, cx| {
                            this.pending_seek = Some(*fraction);
                            cx.notify();
                        }))
                        .on_release(cx.listener(|this, _: &MouseUpEvent, _, cx| {
                            let Some(fraction) = this.pending_seek.take() else {
                                return;
                            };
                            this.playback
                                .update(cx, |playback, cx| playback.seek_fraction(fraction, cx));
                            cx.notify();
                        })),
                ),
            )
            .child(caption(clock(total), false))
            .into_any_element()
    }

    fn footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let (volume, muted) = {
            let playback = self.playback.read(cx);
            (playback.shown_volume(cx), playback.muted(cx))
        };
        let volume = match muted {
            true => 0.,
            false => volume,
        };
        let percent = SharedString::from(format!("{}%", (volume * 100.).round() as i32));
        let playback = self.playback.clone();

        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(ROW_PAD))
            .pt(px(2.))
            .child(self.shuffle_button("mini-shuffle-foot", cx))
            .child(play_mode(&self.playback, false, cx))
            .child(div().flex_1())
            .child(
                Button::new("mini-mute")
                    .ghost()
                    .small()
                    .icon(match muted {
                        true => "icons/volume-x.svg",
                        false => "icons/volume-2.svg",
                    })
                    .tooltip_above(match muted {
                        true => "player-unmute",
                        false => "player-mute",
                    })
                    .on_click(move |_, _, cx| {
                        playback.update(cx, |playback, cx| playback.toggle_muted(cx))
                    }),
            )
            .child(
                div().w(px(88.)).child(
                    Scrubber::new(&self.volume, volume)
                        .name(t!("player-volume"))
                        .value(percent.clone())
                        .colors(
                            theme.progress_bar,
                            theme.foreground.opacity(0.12),
                            theme.foreground,
                        )
                        .on_move(cx.listener(|this, fraction: &f32, _, cx| {
                            this.playback
                                .update(cx, |playback, cx| playback.set_volume(*fraction, cx));
                        })),
                ),
            )
            .child(
                div()
                    .w(px(34.))
                    .flex_none()
                    .text_right()
                    .text_size(px(CAPTION_SIZE))
                    .font_features(tabular())
                    .text_color(theme.muted_foreground)
                    .child(percent),
            )
            .into_any_element()
    }

    fn panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let listing = self.playback.read(cx).shown_queue(cx);
        let sections = listing.sections.clone();
        let past = sections.past;
        let current = sections.current;
        let total = past + usize::from(current) + sections.upcoming;

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(px(HEAD))
            .px(px(ROW_PAD))
            .text_size(px(CAPTION_SIZE))
            .text_color(theme.muted_foreground)
            .child(t!("mini-queue"))
            .child(SharedString::from(total.to_string()));

        let body = match total {
            0 => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(TITLE_SIZE))
                .text_color(theme.muted_foreground)
                .child(t!("queue-empty"))
                .into_any_element(),
            _ => div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(
                    uniform_list(
                        "mini-queue",
                        total,
                        cx.processor(move |this, range: Range<usize>, _window, cx| {
                            range
                                .map(|index| {
                                    let position = match index {
                                        at if at < past => QueuePosition::Past(at),
                                        at if current && at == past => QueuePosition::Current,
                                        at => QueuePosition::Upcoming(
                                            at - past - usize::from(current),
                                        ),
                                    };
                                    this.row(index, position, &listing, cx)
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&self.scroll)
                    .size_full(),
                )
                .into_any_element(),
        };

        div()
            .id("mini-panel")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .px(px(ROW_PAD))
            .pb(px(ROW_PAD))
            .child(header)
            .child(body)
            .child(self.seek_row(cx))
            .child(self.footer(cx))
            .into_any_element()
    }

    fn row(
        &self,
        index: usize,
        position: QueuePosition,
        listing: &state::QueueView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(track) = listing.track(position, cx) else {
            return div().into_any_element();
        };
        let uid = listing.uid(position, cx);
        let current = position == QueuePosition::Current;
        let theme = *cx.theme();
        let playback = self.playback.clone();
        let name = SharedString::from(track.name.clone());
        let artists = SharedString::from(track.artists.clone());
        let label = SharedString::from(format!("{} — {}", track.name, track.artists));
        let duration = clock(track.duration);
        let indicator = match current {
            true => svg()
                .path(icons::path("icons/play.svg"))
                .size(px(10.))
                .text_color(theme.primary)
                .into_any_element(),
            false => div()
                .text_size(px(CAPTION_SIZE))
                .text_color(theme.muted_foreground)
                .child(SharedString::from((index + 1).to_string()))
                .into_any_element(),
        };

        div()
            .id(("mini-queue-row", index))
            .role(gpui::accesskit::Role::Button)
            .aria_label(label)
            .flex()
            .flex_none()
            .h(px(ROW))
            .w_full()
            .items_center()
            .gap(px(6.))
            .px(px(ROW_PAD))
            .rounded(px(6.))
            .text_color(theme.foreground)
            .when(current, |this| this.bg(theme.table_active))
            .hover(|this| this.bg(theme.table_hover))
            .cursor_pointer()
            .on_click(move |_, _, cx| {
                let Some(uid) = uid.clone() else {
                    return;
                };
                playback.update(cx, |playback, cx| playback.play_queue(&uid, cx));
            })
            .child(
                div()
                    .w(px(14.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(indicator),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(TITLE_SIZE))
                    .when(current, |this| {
                        this.text_color(theme.primary)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                    })
                    .child(name),
            )
            .child(
                div()
                    .w(px(80.))
                    .flex_none()
                    .truncate()
                    .text_size(px(CAPTION_SIZE))
                    .text_color(theme.muted_foreground)
                    .child(artists),
            )
            .child(
                div()
                    .w(px(34.))
                    .flex_none()
                    .text_right()
                    .text_size(px(CAPTION_SIZE))
                    .text_color(theme.muted_foreground)
                    .child(duration),
            )
            .into_any_element()
    }
}

impl Render for MiniPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_window(window, cx);
        let theme = *cx.theme();
        // Folding hides the window material to keep the uncovered strip empty; the card goes
        // solid for that pass so no desktop shows through it.
        let fill = match self.folding {
            true => theme.background.opacity(1.),
            false => theme.background,
        };
        let card = div()
            .id("mini-card")
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .rounded(px(CARD_RADIUS))
            .bg(fill)
            .border_1()
            .border_color(theme.foreground.opacity(0.22))
            .text_color(theme.foreground)
            .overflow_hidden()
            .child(self.bar(cx))
            .when(self.panel, |this| this.child(self.panel(cx)));
        let card = match self.panel {
            false => card.h(px(BAR)).into_any_element(),
            true => {
                let (from, to) = match self.expanded {
                    true => (BAR, EXPANDED),
                    false => (EXPANDED, BAR),
                };
                card.with_animation(
                    SharedString::from(format!("mini-reveal-{}-{}", self.epoch, self.expanded)),
                    Animation::new(REVEAL).with_easing(ui::ease_out_cubic),
                    move |this, delta| this.h(px(from + (to - from) * delta)),
                )
                .into_any_element()
            }
        };

        div().size_full().child(card)
    }
}
