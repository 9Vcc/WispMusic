use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{
    AnyElement, AnyView, App, Bounds, ClickEvent, Context, Entity, FocusHandle, FontWeight,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Render, ScrollEvent, SharedString, SpringConfig, SpringState, Task,
    TextShadow,
};
use gpui::{Div, ElementId, Window, anchored, canvas, deferred, div, phi, point, px, svg};
use i18n::t;
use input::{ToggleLyrics, ToggleQueue, ToggleWindowFullscreen, WORKSPACE_CONTEXT};
use router::{Destination, back, navigate};
use state::{
    AppSettings, Cover, FullscreenControlsAutohide, Lyrics, Playback, Queue, SideTab, Wisp,
};
use ui::{
    ActiveTheme as _, Artwork, Badge, Button, Flight, InlineLink, InlineLinks, Modal, Motion,
    Motioned as _, Popovers, Popup, Room, Scrollbar, Scroller, Scrubber, ScrubberState, Springs,
    Sprung, TabBar, Tails as _, Text, Tipped as _, Visualizer, Zoom, clock, entering, growing,
    snapped,
};
use ui::{Perch, tabular};

use crate::chrome::{Aside, OutputPicker, PlayerBar, SpeedButton, TitleBarOptions};
use crate::shared::menus::ItemMenu;
use crate::shared::transport::{
    COVER_FLIGHT, CoverFade, ShellSwap, fraction, like, moved, play_mode, transport,
};
use crate::shared::visualizer::VisualizerDrive;
use crate::shared::volume::{
    RisingVolume, VOLUME_BUBBLE_LIFT, VOLUME_RISE, VolumeHost, rising_volume,
};
use crate::shared::{self, ambient};
use crate::shells::Shell;
use ui::{VeilEdge, veil};

const COVER_TALL: f32 = 0.46;
const COVER_WIDE: f32 = 0.34;
const COVER_TALL_TIGHT: f32 = 0.6;
const COVER_WIDE_TIGHT: f32 = 0.86;
const COVER_TALL_REST: f32 = 0.56;
const COVER_WIDE_REST: f32 = 0.4;
const COVER_TALL_TIGHT_REST: f32 = 0.72;
const COVER_MIN: f32 = 96.;
const COVER_MAX: f32 = 520.;
const COVER_MAX_REST: f32 = 560.;
const COVER_LAYER_PAD: f32 = 2.;
/// The page's own `gap_5`, bottom padding and the meta block's `gap_1`, in rems, so the cover can be
/// fitted against the room actually left over. GPUI's spacing scale is a quarter rem a step.
const COLUMN_GAP: f32 = 1.25;
const PAGE_PAD: f32 = 1.5;
const META_GAP: f32 = 0.25;
/// The page's side padding and the gap between the cover column and a panel from `Room::Wide`
/// up, in rems, which set how wide the split column is.
const PAGE_SIDE: f32 = 2.;
const ROW_GAP: f32 = 2.;
const DOCK: f32 = 1.15;
const DOCK_FULL: f32 = 1.7;
const SINK: f32 = 24.;
/// The name the seek bar with the transport is seen under, so it slides to where a panel
/// moves it.
const DECK_FLIGHT: &str = "fullscreen-deck";
/// The space between the pill and the leave button in the corner, and between the transport and
/// the volume where the volume follows it.
const CORNER_GAP: f32 = 8.;
/// A closed panel fading out, a little quicker than one comes in.
const PANEL_LEAVE: SpringConfig = ui::spring(0.24, 0.);
/// How far the page's foot lifts the row along the bottom, which puts it level with the
/// corner.
const DECK_LIFT: f32 = 2.;
const PILL_GAP: f32 = 2.;
/// How far above the bar along the bottom the pill floats below `Room::Wide`.
const PILL_LIFT: f32 = 10.;
const SEEK_MAX: f32 = 420.;
/// [wisp] 歌词偏移的步进（毫秒），与原版一致。
const LYRIC_NUDGE: i32 = 500;
/// [wisp] 歌词工具圆钮悬浮时的放大倍数（轻微）。
const LYRIC_TOOL_GROW: f32 = 1.06;
const CLOCK_SHORT: f32 = 3.4;
const CLOCK_LONG: f32 = 5.4;
const VISUALIZER_MIN: f32 = 160.;
/// How tall the band under the controls is, as a share of the window. It carries the seek bar,
/// the transport and the meta line over a visualizer that reaches the bottom edge, so it runs
/// a good deal taller than the chrome itself and fades out well above it.
const VEIL: f32 = 0.34;
/// How hard that band blurs what passes under it. The settings header has flat page under it
/// and gets by on a pixel; a bar of the visualizer is a hard edge and needs a real radius
/// before it stops reading through the text over it.
const VEIL_BLUR: Pixels = px(12.);
/// How far ahead of the spectrum that band runs as the two come and go: it is at full strength
/// by the time this share of the spectrum shows. A band that only kept pace would let the bars
/// read sharp through it for as long as either was partly there.
const VEIL_LEAD: f32 = 10.;
/// The blur of each shadow stacked under the title and the artist line. A tight one gives the
/// shadow a core and a wider one softens its edge.
const TEXT_SHADOW: [f32; 2] = [2., 8.];
/// How much of the page colour the title and artist shadows carry. The text reads on the field
/// without them, so they only hold it where something bright passes behind.
const TEXT_SHADOW_STRENGTH: f32 = 0.45;
/// How far the widest text shadow spreads past its glyphs: three of its deviations, as far as the
/// renderer carries a blur. A gliding layer keeps this much room so it never cuts a shadow off.
const SHADOW_BLEED: f32 = TEXT_SHADOW[1] * 3.;
/// How hard the docked controls blur once they are all but hidden. The dock shrinks over them as
/// they leave, so without it the last frames show the tops of the seek row cut off by its edge.
const DOCK_BLUR: Pixels = px(8.);
/// Below this the dock's blur is not worth a layer of its own.
const DOCK_BLUR_LEAST: Pixels = px(0.05);
const REST: Duration = Duration::from_millis(1000);
const WAKE_DEBOUNCE: Duration = Duration::from_millis(400);
/// How far the pointer has to travel, in pixels, before it wakes the controls, so the drift a
/// resting hand gives a trackpad or a sensitive mouse leaves them asleep.
const WAKE_TRAVEL: f64 = 3.;
const SPRING_REST: f32 = 0.001;
const SPRING_STALL: Duration = Duration::from_millis(64);

pub struct FullscreenView {
    lyrics: Entity<Lyrics>,
    /// [wisp] 歌词版本选择对话框是否打开，以及对话框里选中的候选。
    version_dialog: bool,
    version_pick: Option<usize>,
    /// [wisp] 版本列表的滚动条。
    version_scroll: Entity<Scrollbar>,
    playback: Entity<Playback>,
    cover: Entity<Cover>,
    settings: Entity<AppSettings>,
    aside: Entity<Aside>,
    collapse: Entity<Collapse>,
    panel: Option<SideTab>,
    /// How much of the content the pill just switched to is still hidden, springing from 1 to 0
    /// at the pace the cover travels, so a panel fades in while the cover slides aside. A spring
    /// steps from the first frame the content draws, so a slow first layout cannot eat the
    /// entrance. The pill itself never takes part.
    switch: Option<Sprung>,
    seek: ScrubberState,
    pending: Option<f32>,
    over_seek: Option<f32>,
    volume: RisingVolume,
    /// The speed picker the narrow bar carries where the player bar has its own.
    speed: Entity<SpeedButton>,
    over_pill: bool,
    over_transport: bool,
    over_leave: bool,
    large: Option<SharedString>,
    full: Option<SharedString>,
    revision: usize,
    track_menu: ItemMenu,
    context_menu: Option<(music::Track, Point<Pixels>)>,
    last_moved: Instant,
    /// Where the pointer was when it last woke the controls, which motion is measured from.
    last_pos: Option<Point<Pixels>>,
    inside: bool,
    /// Whether the pointer left the window and has not moved inside it since. macOS answers
    /// `is_window_hovered` with whether the window has focus, so only the exit event tells there.
    left: bool,
    awake: bool,
    hidden: SpringState,
    spring_beat: Instant,
    rest: Option<Task<()>>,
    focus: FocusHandle,
    visualizer: VisualizerDrive,
    root_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// How far the cover is scaled as it flies in from the player bar.
    cover_zoom: Rc<Cell<Zoom>>,
    /// Whether the player is the shell on show. While it fades out over the workspace its cover
    /// flies back into the player bar.
    on_show: bool,
    /// The player's own top left corner, which a fading panel is placed against.
    root_origin: Rc<Cell<Point<Pixels>>>,
    /// Where the panel last drew, which is where it fades out once the pill closes it.
    panel_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// How much of a panel the pill just closed is still showing, springing from 1 to 0.
    leaving: Option<Sprung>,
    meta_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The height the controls take with nothing clamping them, measured a frame ago. The dock
    /// shrinks from it while it fades, so the column moves in step with the cover.
    dock_natural: Rc<Cell<Pixels>>,
    /// How wide the corner drew with everything in it, measured a frame ago. Below `Room::Wide`
    /// the seek bar shares the corner's row and ends this far from it.
    corner_width: Rc<Cell<Pixels>>,
    /// The speaker button that moves playback between output devices.
    output: Entity<OutputPicker>,
}

/// The player's way out, at the start of the title bar on every page: the cover, the lyrics
/// and the queue alike.
struct Collapse;

impl Render for Collapse {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("fullscreen-collapse")
            .ghost()
            .small()
            .icon("icons/chevron-down.svg")
            .tooltip_trailing("lyric-collapse")
            .on_click(|_, _, cx| {
                cx.stop_propagation();
                back(cx);
            })
    }
}

impl VolumeHost for FullscreenView {
    fn volume(&mut self) -> &mut RisingVolume {
        &mut self.volume
    }

    fn playback(&self) -> &Entity<Playback> {
        &self.playback
    }
}

impl FullscreenView {
    /// The height the dock along the bottom is given, which the toasts hang above.
    pub(crate) fn dock_height(cx: &App) -> Pixels {
        cx.theme().metrics.player_bar * DOCK_FULL
    }

    pub fn new(playback: Entity<Playback>, queue: Entity<Queue>, cx: &mut Context<Self>) -> Self {
        let lyrics = Wisp::global(cx).lyrics.clone();
        cx.observe(&lyrics, |_, _, cx| cx.notify()).detach();
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
        // the seek bar and the lyrics follow the position as it moves
        let ticks = playback.read(cx).ticks().clone();
        cx.observe(&ticks, |_, _, cx| cx.notify()).detach();
        let cover = Wisp::global(cx).cover.clone();
        cx.observe(&cover, |_, _, cx| cx.notify()).detach();
        let climax = Wisp::global(cx).climax.clone();
        cx.observe(&climax, |_, _, cx| cx.notify()).detach();
        let library = Wisp::global(cx).library.clone();
        cx.observe(&library, |_, _, cx| cx.notify()).detach();
        // The meta drops its text shadows while the player opens or closes.
        cx.observe_global::<ShellSwap>(|_, cx| cx.notify()).detach();
        let settings = Wisp::global(cx).settings.clone();
        cx.observe(&settings, |this, settings, cx| {
            if this.panel == Some(SideTab::Lyrics) && !settings.read(cx).lyrics_enabled() {
                this.panel = None;
            }
            cx.notify();
        })
        .detach();
        let panel = settings.read(cx).fullscreen_tab();
        let shown = panel.unwrap_or_else(|| match settings.read(cx).lyrics_enabled() {
            true => SideTab::Lyrics,
            false => SideTab::Queue,
        });
        let collapse = cx.new(|_| Collapse);
        let aside = cx.new(|cx| Aside::new(queue.clone(), playback.clone(), shown, cx));
        aside.update(cx, |aside, cx| aside.strip(cx));
        let me = cx.entity_id();
        let playlist_scrollbar = cx.new(|_| Scrollbar::inset().watching(me));
        let version_scroll = cx.new(|_| Scrollbar::inset().watching(me));
        let popovers = Popovers::default();
        let outputs = Wisp::global(cx).outputs.clone();
        let output = cx.new(|cx| OutputPicker::compact(outputs, popovers, cx));
        let speed = cx.new(|cx| SpeedButton::named("fullscreen-speed", cx));

        Self {
            lyrics,
            version_dialog: false,
            version_pick: None,
            version_scroll,
            playback,
            cover,
            settings,
            aside,
            collapse,
            panel,
            switch: None,
            seek: ScrubberState::new("fullscreen-seek"),
            pending: None,
            over_seek: None,
            volume: RisingVolume::new("fullscreen-volume-slider"),
            speed,
            over_pill: false,
            over_transport: false,
            over_leave: false,
            large: None,
            full: None,
            revision: 0,
            track_menu: ItemMenu::new(playlist_scrollbar, cx),
            context_menu: None,
            last_moved: Instant::now(),
            last_pos: None,
            inside: true,
            left: false,
            awake: true,
            hidden: SpringState {
                position: 0.,
                velocity: 0.,
            },
            spring_beat: Instant::now(),
            rest: None,
            focus: cx.focus_handle(),
            visualizer: VisualizerDrive::default(),
            root_bounds: Rc::new(Cell::new(Bounds::default())),
            cover_zoom: Rc::default(),
            on_show: false,
            root_origin: Rc::default(),
            panel_bounds: Rc::new(Cell::new(Bounds::default())),
            leaving: None,
            meta_bounds: Rc::new(Cell::new(Bounds::default())),
            dock_natural: Rc::new(Cell::new(Pixels::ZERO)),
            corner_width: Rc::new(Cell::new(Pixels::ZERO)),
            output,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Whether a cover is in the air between the player bar and this player, so the bar leaves its
    /// own undrawn until the flight lands. Below `Room::Wide` a panel open has no big cover to
    /// fly, since the bar along the bottom carries a small one instead.
    pub(crate) fn cover_flies(&self, window: &Window) -> bool {
        Room::of(window.viewport_size().width).fits(Room::Wide) || self.panel.is_none()
    }

    /// Whether the player bar's cover should give its place to the title, which it does when
    /// the artwork tab is on below `Room::Wide`, where the bar's title stands in the fullscreen
    /// player's bar with no cover beside it.
    pub(crate) fn tucks_cover(&self, window: &Window) -> bool {
        !Room::of(window.viewport_size().width).fits(Room::Wide) && self.panel.is_none()
    }

    /// Starts the idle clock as the view opens and stops it as the view closes, so it never
    /// hides the cursor over the workspace. Either way the view next shows with its controls up.
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.on_show = shown;
        match shown {
            true => {
                self.flip(true);
                self.hidden = SpringState {
                    position: 0.,
                    velocity: 0.,
                };
                self.unpark();
                self.last_pos = None;
                self.stir(cx);
            }
            false => self.rest = None,
        }
        cx.notify();
    }

    /// Selects the fullscreen panel and restarts the controls' idle timer.
    fn show(&mut self, panel: Option<SideTab>, cx: &mut Context<Self>) {
        if panel != self.panel && !cx.reduce_motion() {
            let visible = self
                .switch
                .as_ref()
                .map_or(1., |switch| 1. - switch.position().clamp(0., 1.));
            self.switch = Some(Sprung::at(1.));
            self.leaving = panel.is_none().then(|| Sprung::at(visible));
        }
        self.panel = panel;
        // The transport may move between the dock and the strip, and the row it leaves never
        // says the pointer left it.
        self.over_transport = false;
        self.settings
            .update(cx, |settings, cx| settings.set_fullscreen_tab(panel, cx));
        if let Some(tab) = panel {
            self.aside.update(cx, |aside, cx| aside.show(tab, cx));
        }
    }

    /// Flips the fullscreen panel to `tab` from a key press. The cover comes back when `tab`
    /// is already showing.
    fn toggle_panel(&mut self, tab: SideTab, cx: &mut Context<Self>) {
        if tab == SideTab::Lyrics && !self.settings.read(cx).lyrics_enabled() {
            return;
        }
        let panel = match self.panel == Some(tab) {
            true => None,
            false => Some(tab),
        };
        self.show(panel, cx);
    }

    fn stir(&mut self, cx: &mut Context<Self>) {
        if !self.awake {
            self.flip(true);
            cx.notify();
        }
        self.last_moved = Instant::now();
        self.rest = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REST).await;
            this.update(cx, |this, cx| {
                if this.last_moved.elapsed() < REST {
                    return;
                }
                if this.busy(cx) {
                    this.stir(cx);
                    return;
                }
                this.flip(false);
                // The pointer going idle takes the cursor with it; any motion
                // brings it back on its own.
                cx.hide_cursor();
                cx.notify();
            })
            .ok();
        }));
    }

    fn hover(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !hovered(window) {
            return;
        }
        let pad = cx.theme().metrics.pad;
        let seek = self.seek.hovered(event.position, pad);
        if moved(self.over_seek, seek) {
            self.over_seek = seek;
            cx.notify();
        }
        if self.left {
            self.left = false;
            cx.notify();
        }
        let travel = self
            .last_pos
            .map_or(f64::INFINITY, |last| (event.position - last).magnitude());
        if travel < WAKE_TRAVEL {
            if !self.awake {
                cx.hide_cursor();
            }
            return;
        }
        self.last_pos = Some(event.position);
        self.poke(cx);
    }

    fn poke(&mut self, cx: &mut Context<Self>) {
        if self.awake && self.last_moved.elapsed() < WAKE_DEBOUNCE {
            return;
        }
        self.stir(cx);
    }

    /// Covers the whole view while the controls sleep and the cursor is hidden, so nothing under
    /// the pointer stays hovered and a click only wakes the controls instead of landing blind.
    /// Scrolling passes through, and the layer takes over the root's own pointer listeners, which
    /// no longer count as hovered beneath it.
    fn dormant(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .inset_0()
            .block_mouse_except_scroll()
            .on_mouse_move(cx.listener(Self::hover))
            .on_mouse_exit(cx.listener(|this, _: &MouseExitEvent, _, cx| {
                this.left = true;
                cx.notify();
            }))
            .on_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, _, cx| this.rouse(cx)))
    }

    /// Wakes the sleeping controls from a press, keeping the press from whatever sits under the
    /// hidden pointer.
    fn rouse(&mut self, cx: &mut Context<Self>) {
        cx.stop_propagation();
        cx.show_cursor();
        self.stir(cx);
    }

    /// Whether anything under the pointer should hold the chrome awake. A hover flag only counts
    /// while the pointer is over the window: one set when it left stays set, and a stale flag
    /// would keep the controls up for good. A track menu drawn over the view holds it too, since
    /// the pointer over it never reaches the view and the cursor would hide under it.
    fn busy(&self, cx: &App) -> bool {
        let parked = self.volume.parked()
            || self.over_pill
            || self.over_transport
            || self.over_leave
            || self.over_seek.is_some()
            || self.scrollbar_held(cx)
            || self.speed.read(cx).open();
        (self.inside && parked)
            || self.volume.held
            || self.pending.is_some()
            || self.context_menu.is_some()
            || (self.panel.is_some() && self.aside.read(cx).menu_open())
    }

    /// Clears hover state when the fullscreen view opens or closes. Its elements report their
    /// current hover state when they are mounted again.
    fn unpark(&mut self) {
        self.volume.unpark();
        self.over_pill = false;
        self.over_transport = false;
        self.over_leave = false;
        self.over_seek = None;
    }

    /// Follows the pointer in and out of the window, once a frame. A hover flag clears on an
    /// event the pointer has to be present for, so the ones still set when it left the window are
    /// dropped here instead. Crossing the window edge repaints on its own, so this is never late.
    fn watch(&mut self, window: &Window, cx: &mut Context<Self>) {
        let inside = hovered(window) && !self.left;
        if inside == self.inside {
            return;
        }
        self.inside = inside;
        if !inside {
            self.unpark();
            self.last_pos = None;
            self.stir(cx);
        }
    }

    /// Whether the pointer is parked on the lyrics panel's scrollbar. Read at
    /// rest-expiry time, so no subscription is needed: a parked pointer simply
    /// re-arms the timer instead of letting the chrome go idle under it.
    fn scrollbar_held(&self, cx: &App) -> bool {
        self.panel.is_some() && self.aside.read(cx).scrollbar_active(cx)
    }

    fn flip(&mut self, awake: bool) {
        if self.awake == awake {
            return;
        }
        self.awake = awake;
        self.spring_beat = Instant::now();
    }

    /// Steps the idle spring and answers how far the view has sunk toward rest, 0 awake to
    /// 1 asleep. It follows the pointer alone and never the visibility setting, so the leave
    /// button can ride it whatever the setting says about the controls.
    fn hidden(&mut self, window: &mut Window, cx: &App) -> f32 {
        let target = match self.awake {
            true => 0.,
            false => 1.,
        };

        if cx.reduce_motion() {
            self.hidden = SpringState {
                position: target,
                velocity: 0.,
            };
            self.spring_beat = Instant::now();
            return target;
        }

        let now = Instant::now();
        let elapsed = now.duration_since(self.spring_beat).min(SPRING_STALL);
        self.spring_beat = now;
        self.hidden = Springs::RESPONSIVE.step(self.hidden, target, elapsed.as_secs_f32());
        if Springs::RESPONSIVE.is_settled(self.hidden, target, SPRING_REST) {
            self.hidden = SpringState {
                position: target,
                velocity: 0.,
            };
        } else {
            window.request_animation_frame();
        }
        self.hidden.position.clamp(0., 1.)
    }

    /// How much of the tab just shown is still hidden, from 1 as the switch starts to 0 once it
    /// ends. It asks for another frame while the switch runs.
    fn switching(&mut self, window: &mut Window, cx: &App) -> f32 {
        let Some(switch) = self.switch.as_mut() else {
            return 0.;
        };
        let hidden = switch.follow(0., Springs::FLIGHT, SPRING_REST, window, cx);
        if hidden <= 0. {
            self.switch = None;
        }
        hidden.clamp(0., 1.)
    }

    fn commit_seek(&mut self, cx: &mut Context<Self>) {
        let Some(fraction) = self.pending.take() else {
            return;
        };
        self.playback
            .update(cx, |playback, cx| playback.seek_fraction(fraction, cx));
    }

    fn artwork(
        &mut self,
        layout_side: Pixels,
        raster_side: Pixels,
        presentation_scale: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // The cover leaves the player bar with that cover's tight corners, which round out to
        // its own as it lands, and tighten again on the way back. A panel coming or going only
        // scales it between two sizes of its own, so it keeps its own corners the whole way.
        let (tight, round) = (Artwork::rounding(cx), cx.theme().radius * 2.);
        let zoom = self.cover_zoom.get();
        let radius = match (self.on_show, self.switch.is_some()) {
            (true, true) => zoom.radius(round, round),
            (true, false) => zoom.radius(tight, round),
            (false, _) => zoom.radius(round, tight),
        };
        let pad = px(COVER_LAYER_PAD);
        let inset = (layout_side - raster_side) / 2. - pad;
        let (album, small, local_id, named) = match self.playback.read(cx).shown_track(cx) {
            Some(track) => {
                let local = track.id.as_deref().is_some_and(music::is_local_id);
                let named = album_name(&track);
                (track.album_id, track.cover, local, named)
            }
            None => (None, None, false, SharedString::default()),
        };
        let cover = self.cover.read(cx);
        let cover_large = album.as_deref().and_then(|album| cover.large_for(album));
        let cover_full = album.as_deref().and_then(|album| cover.full_for(album));
        let local = local_id || small.as_ref().is_some_and(|url| url.starts_with("file://"));
        // Only upgrade resolution for non-local tracks, since a local cover is always
        // the full resolution picture. The track's own embedded cover is prioritized
        // over the cover of its album. Each larger cover fades in over the one below, so the
        // thumbnail shows only until the large cover lands.
        let upgrades = |url: &&str| !local && small.is_some() && Some(*url) != small.as_deref();
        let large = cover_large.filter(upgrades);
        let full = cover_full
            .filter(upgrades)
            .filter(|url| Some(*url) != large);

        if self.large.as_deref() != large || self.full.as_deref() != full {
            self.large = large.map(SharedString::from);
            self.full = full.map(SharedString::from);
            self.revision += 1;
        }
        let large = self.large.clone();
        let full = self.full.clone();
        let revision = self.revision;
        let waiting = !local && album.is_some() && cover_large.is_none();

        // As the player opens, its cover comes into view with the rest of it while it flies.
        let fade = cx.try_global::<CoverFade>().map_or(1., |fade| fade.0);
        let cover = div()
            .id("fullscreen-artwork")
            .when(fade < 1., |this| this.opacity(fade))
            .relative()
            .size(layout_side)
            .flex_none()
            .when_some(album, |this, album| {
                this.role(Role::Link)
                    .aria_label(named)
                    .cursor_pointer()
                    .on_click(move |_, _, cx| open_album(&album, cx))
            })
            .child(
                div()
                    .absolute()
                    .top(inset)
                    .left(inset)
                    .size(raster_side + pad * 2.)
                    .child(
                        div().absolute().top(pad).left(pad).child(
                            Artwork::new(small)
                                .size(raster_side)
                                .corner_radius(radius)
                                .soft(waiting),
                        ),
                    )
                    .children(
                        [("cover-large", large), ("cover-full", full)]
                            .into_iter()
                            .filter_map(|(layer, url)| {
                                let url = url?;
                                Some(
                                    div()
                                        .absolute()
                                        .top(pad)
                                        .left(pad)
                                        .child(
                                            Artwork::new(Some(url))
                                                .size(raster_side)
                                                .corner_radius(radius)
                                                .overlay(),
                                        )
                                        .motion((layer, revision), Motion::Slow, |art, t| {
                                            art.opacity(t)
                                        }),
                                )
                            }),
                    ),
            );
        // The cover flies in from the player bar as fullscreen opens, and slides and grows to
        // its new place when a panel comes or goes, above everything while it travels.
        // The controls falling asleep move the cover on a spring of their own, so layout only
        // sends it flying while a panel is coming or going. The raster is drawn at its resting
        // size through the flight's own layer, which keeps room for how far it reaches past
        // its layout, so a flying cover takes one layer and not two.
        Flight::new("fullscreen-cover", cover)
            .transfer_spring(Springs::ARTWORK_FLIGHT)
            .lands_from(COVER_FLIGHT)
            // Once the player closes, its cover flies back down onto the player bar's.
            .heads_to((!self.on_show).then_some(COVER_FLIGHT))
            .reports(self.cover_zoom.clone())
            .follows(self.switch.is_some())
            .reach((raster_side - layout_side) / 2. + pad)
            .presents_at(presentation_scale)
            .above()
    }

    fn open_context_menu(
        &mut self,
        track: music::Track,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.track_menu.reset(cx);
        self.context_menu = Some((track, position));
        cx.notify();
    }

    fn meta(
        &self,
        hide: f32,
        lift: Pixels,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = *cx.theme();
        let frosted = ambient::shown(cx);
        let shown = self.playback.read(cx).shown_track(cx);
        let track = shown.as_ref();
        let title = match track {
            Some(track) => SharedString::from(track.name.clone()),
            None => t!("player-nothing-playing"),
        };
        let album = track.and_then(|track| track.album_id.clone());
        let opens = album.is_some();
        let explicit = track.is_some_and(|track| track.explicit);
        let meta_bounds = self.meta_bounds.clone();
        // Both lines carry a shadow in the page colour, so they stay legible where a visualizer
        // peak or bright ambient art passes behind them. The title ellipsizes without
        // `truncate`, whose `overflow_hidden` would clip the shadow to the line box. Every shadow
        // is a blur layer of its own, so they wait out the player opening or closing, while the
        // meta is still fading and every frame already carries the most layers.
        let shadowed = shared::effects()
            && cx.try_global::<ShellSwap>().copied().unwrap_or_default() == ShellSwap::Settled;
        let shadow = TEXT_SHADOW.map(|blur| {
            TextShadow::new(theme.background.opacity(TEXT_SHADOW_STRENGTH)).blur(px(blur))
        });
        let trailing = |track: Option<&music::Track>, cx: &App| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .when(explicit, |this| {
                    this.child(explicit_badge("fullscreen-explicit", track.is_some()))
                })
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .opacity(1. - hide)
                        .child(like(track, &self.playback, cx).when(frosted, Button::frosted)),
                )
        };

        div()
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap_1()
            .w_full()
            .min_w_0()
            .map(|this| glide(this, lift, px(SHADOW_BLEED), window))
            .child(
                canvas(move |bounds, _, _| meta_bounds.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(SHADOW_BLEED))
                    .bottom(px(SHADOW_BLEED)),
            )
            .child(
                // Three-part row with equal flex sides, so the title stays truly centred:
                // the heart and the explicit badge live in the right cell and never shift it.
                // The left cell holds an invisible copy of the right one, so both sides floor
                // at the same width and a long title truncates instead of pushing the heart out
                // of the row. The room around the title comes from the row gap, since padding
                // on a side would make that side the wider one.
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .min_w_0()
                    .child(
                        div()
                            .id("fullscreen-title-balance")
                            .flex()
                            .flex_1()
                            .justify_end()
                            .invisible()
                            .child(trailing(None, cx)),
                    )
                    .child(
                        div()
                            .id("fullscreen-title")
                            .map(|this| match opens {
                                true => this.role(Role::Link).aria_label(title.clone()),
                                false => this.role(Role::Label).aria_value(title.clone()),
                            })
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(theme.text(Text::Title))
                            .font_weight(FontWeight::SEMIBOLD)
                            .when_some(album, |this, album| {
                                this.cursor_pointer()
                                    .hover(|style| style.underline())
                                    .on_click(move |_, _, cx| open_album(&album, cx))
                            })
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    let Some(track) = this.playback.read(cx).shown_track(cx) else {
                                        return;
                                    };
                                    window.prevent_default();
                                    this.open_context_menu(track, event.position, cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .when(shadowed, |this| this.text_shadow(shadow))
                            .child(title),
                    )
                    .child(div().flex().flex_1().child(trailing(track, cx))),
            )
            .when_some(track, |this, track| {
                this.child(
                    div().flex().w_full().min_w_0().justify_center().child(
                        InlineLinks::new(
                            "fullscreen-artists",
                            track.artist_refs.iter().map(|artist| {
                                InlineLink::new(
                                    artist.name.clone(),
                                    artist.id.clone().map(Into::into),
                                )
                            }),
                            track.artists.clone(),
                            shared::quiet(&theme),
                        )
                        .text_size(theme.text(Text::Body))
                        .truncate()
                        .when(shadowed, |this| this.text_shadow(shadow))
                        .on_click(|id, cx| navigate(Destination::Artist(id), cx)),
                    ),
                )
            })
    }

    fn strip(
        &self,
        hide: f32,
        cover: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = *cx.theme();
        let frosted = ambient::shown(cx);
        let side = ui::snapped(theme.metrics.row, window);
        let shown = self.playback.read(cx).shown_track(cx);
        let track = shown.as_ref();
        let title = match track {
            Some(track) => SharedString::from(track.name.clone()),
            None => t!("player-nothing-playing"),
        };
        let album = track.and_then(|track| track.album_id.clone());
        let opens = album.is_some();
        let named = track.map(album_name).unwrap_or_default();
        let explicit = track.is_some_and(|track| track.explicit);

        div()
            .flex()
            .items_center()
            .gap_3()
            .w_full()
            .min_w_0()
            .when(cover, |this| {
                this.child(
                    div()
                        .id("strip-artwork")
                        .when_some(album.clone(), |this, album| {
                            this.role(Role::Link)
                                .aria_label(named)
                                .cursor_pointer()
                                .on_click(move |_, _, cx| open_album(&album, cx))
                        })
                        .child(Artwork::new(track.and_then(|t| t.cover.clone())).size(side)),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .id("strip-title")
                                    .map(|this| match opens {
                                        true => this.role(Role::Link).aria_label(title.clone()),
                                        false => this.role(Role::Label).aria_value(title.clone()),
                                    })
                                    .min_w_0()
                                    .truncate()
                                    .tails()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .when_some(album, |this, album| {
                                        this.cursor_pointer()
                                            .hover(|style| style.underline())
                                            .on_click(move |_, _, cx| open_album(&album, cx))
                                    })
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(
                                            move |this, event: &MouseDownEvent, window, cx| {
                                                let Some(track) =
                                                    this.playback.read(cx).shown_track(cx)
                                                else {
                                                    return;
                                                };
                                                window.prevent_default();
                                                this.open_context_menu(track, event.position, cx);
                                                cx.stop_propagation();
                                            },
                                        ),
                                    )
                                    .child(title),
                            )
                            .when(explicit, |this| {
                                this.child(explicit_badge("strip-explicit", true))
                            })
                            .child(div().flex().flex_none().opacity(1. - hide).child(
                                like(track, &self.playback, cx).when(frosted, Button::frosted),
                            )),
                    )
                    .when_some(track, |this, track| {
                        this.child(
                            InlineLinks::new(
                                "strip-artists",
                                track.artist_refs.iter().map(|artist| {
                                    InlineLink::new(
                                        artist.name.clone(),
                                        artist.id.clone().map(Into::into),
                                    )
                                }),
                                track.artists.clone(),
                                shared::quiet(&theme),
                            )
                            .text_size(theme.text(Text::Small))
                            .truncate()
                            .on_click(|id, cx| navigate(Destination::Artist(id), cx)),
                        )
                    }),
            )
            // The strip only shows below `Room::Wide`, where it carries the transport at its
            // end the way the stacked player bar does. The buttons go with the controls.
            .when(hide < 1., |this| {
                this.child(
                    self.transport_row("fullscreen-strip-transport", false, cx)
                        .flex_none()
                        .opacity(1. - hide),
                )
            })
    }

    /// The transport buttons in a row that tells the view while the pointer is over it.
    fn transport_row(
        &self,
        id: &'static str,
        big: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .on_hover(cx.listener(|this, hovering: &bool, _, cx| {
                this.over_transport = *hovering;
                cx.notify();
            }))
            .child(transport(&self.playback, big, cx))
    }

    fn seek(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let muted = theme.muted_foreground;
        let empty = muted.opacity(0.3);
        let text = theme.text(Text::Tiny);
        let playback = self.playback.read(cx);
        let seekable = playback.shown_track(cx).is_some();
        let (elapsed, total) = playback.shown_time(cx);
        let progress = self.pending.unwrap_or_else(|| fraction(elapsed, total));
        let ticks = Wisp::global(cx).climax.read(cx).ticks(total);
        let width = text
            * match total.as_secs() >= 3600 {
                true => CLOCK_LONG,
                false => CLOCK_SHORT,
            };

        let bubble = self
            .over_seek
            .or(self.pending)
            .map(|at| (at, clock(total.mul_f32(at))));

        let label = move |id: &'static str, value: Duration, align_end: bool| {
            ui::spoken(id, clock(value))
                .w(width)
                .flex_none()
                .whitespace_nowrap()
                .text_size(text)
                .text_color(muted)
                .when_else(align_end, |this| this.text_right(), |this| this.text_left())
        };

        div()
            .id("fullscreen-seek")
            .role(Role::Group)
            .aria_label(t!("song-position"))
            .flex()
            .items_center()
            .gap_2()
            .w_full()
            .child(label("fullscreen-elapsed", elapsed, true))
            .child(
                div().flex_1().min_w_0().child(
                    Scrubber::new(&self.seek, progress)
                        .name(t!("song-position"))
                        .value(clock(elapsed))
                        .colors(theme.progress_bar, empty, theme.foreground)
                        .enabled(seekable)
                        .ticks(ticks)
                        .when_some(bubble, |this, (at, text)| this.bubble(at, text))
                        .on_move(cx.listener(|this, fraction: &f32, _, cx| {
                            this.pending = Some(*fraction);
                            cx.notify();
                        }))
                        .on_release(
                            cx.listener(|this, _: &MouseUpEvent, _, cx| this.commit_seek(cx)),
                        ),
                ),
            )
            .child(label("fullscreen-total", total, false))
    }

    /// The seek bar and the transport. From `Room::Wide` up they stack in the middle, the
    /// transport under the seek bar with the volume at the row's right end. A narrower window
    /// has no room for that row beside the corner, so the controls lie the way the stacked
    /// player bar does: the transport on a row of its own, and the seek bar along the bottom,
    /// level with the corner and ending short of it. With a panel open the strip above
    /// carries the transport instead, and the corner takes the volume. Centred controls are at
    /// most `deck` wide, so they keep one width whether they sit in the column or at the bottom.
    fn controls(
        &self,
        room: Room,
        deck: Pixels,
        hide: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = *cx.theme();
        let centred = room.fits(Room::Wide);
        let stack = div().flex().flex_col().items_center().gap_3().w_full();
        let stack = match centred {
            // [wisp] 音量贴着 transport 的循环键排在右侧；左侧放等宽的隐形
            // 占位，播放键因此与上方曲名严格居中（不再被音量挤偏）。
            true => stack.child(self.seek(cx)).child(
                div()
                    .flex()
                    .w_full()
                    .items_center()
                    .justify_center()
                    .gap(px(16.))
                    .child(div().flex_none().w(px(32.)).invisible())
                    .child(
                        self.transport_row("fullscreen-transport", true, cx)
                            .flex_none(),
                    )
                    .child(play_mode(&self.playback, ambient::shown(cx), cx))
                    .child(self.sound(cx)),
            ),
            false => {
                let clear = self.corner_width.get() + px(CORNER_GAP) * 2.;
                stack
                    .when(self.panel.is_none(), |this| {
                        this.child(
                            self.transport_row("fullscreen-transport", true, cx)
                                .w_full()
                                .justify_center(),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .w_full()
                            .h(theme.metrics.control_small)
                            .pr(clear)
                            .child(self.seek(cx)),
                    )
            }
        };

        div()
            .id("fullscreen-controls")
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .w_full()
            .when(centred, |this| this.max_w(deck))
            .flex_none()
            .on_click(|_, _, cx| cx.stop_propagation())
            // The seek bar and the transport travel as one when a panel moves them between
            // the bottom of the window and the column under the cover.
            .child(travelling(
                "fullscreen-deck",
                DECK_FLIGHT,
                self.switch.is_some(),
                self.on_show,
                // The volume slider rises out of the transport and carries a bubble of its
                // own, which is the farthest anything here paints past the deck.
                px(VOLUME_RISE) + Scrubber::bubble_reach(VOLUME_BUBBLE_LIFT, cx),
                stack.opacity(1. - hide),
            ))
    }

    /// The transport controls under the cover or along the bottom. A dock that `collapses` gives
    /// its height back to the page as it hides. One beside the cover keeps its footprint and
    /// only fades, since the column around it settles by `glide` instead of through layout.
    fn dock(
        &self,
        cap: Pixels,
        hide: f32,
        collapses: bool,
        room: Room,
        deck: Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // The clamp runs from the controls' own height once it is known. From a taller cap the
        // dock would hold still through the start of the fade and catch up after. A dock that
        // keeps its footprint reserves it only once the controls are gone, because the height is
        // a frame old and a switch between the full and split layouts would stretch the column
        // by the difference until something renders again.
        let natural = self.dock_natural.get();
        let cap = match natural > Pixels::ZERO {
            true => natural,
            false => cap,
        };
        let measured = self.dock_natural.clone();
        div()
            .flex()
            .flex_none()
            .w_full()
            .items_start()
            .justify_center()
            .map(|this| match collapses {
                true => this.when(hide > 0., |this| {
                    this.max_h(cap * (1. - hide)).overflow_hidden()
                }),
                false => this.when(hide >= 1., |this| this.min_h(cap)),
            })
            .map(|this| match DOCK_BLUR * hide {
                blur if blur > DOCK_BLUR_LEAST && hide < 1. && shared::effects() => this.blur(blur),
                _ => this,
            })
            .when(hide < 1., |this| {
                this.child(
                    div()
                        .relative()
                        .w_full()
                        .flex()
                        .justify_center()
                        .top(px(SINK) * hide)
                        .child(
                            canvas(
                                move |bounds, _, _| measured.set(bounds.size.height),
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        )
                        .child(self.controls(room, deck, hide, cx)),
                )
            })
    }

    fn pill(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let frosted = ambient::shown(cx);
        let lyrics = self.settings.read(cx).lyrics_enabled();
        let gap = px(PILL_GAP);
        let linger = cx.listener(|this: &mut Self, hovering: &bool, window, cx| {
            this.over_pill = *hovering;
            if *hovering && hovered(window) && !this.left {
                this.poke(cx);
            }
            cx.notify();
        });
        let tab = move |id: &'static str, icon: &'static str, hint: &'static str, panel| {
            let showing = self.panel == panel;

            Button::new(id)
                .ghost()
                .when(frosted, Button::frosted)
                .small()
                .icon(icon)
                .tooltip_above(hint)
                .selected(showing)
                .rounded(theme.radius)
                .tint(match showing {
                    true => theme.foreground,
                    false => theme.muted_foreground,
                })
                .on_click(cx.listener(move |this, _, _, cx| this.show(panel, cx)))
        };

        div()
            .id("fullscreen-pill")
            .flex_none()
            .on_hover(linger)
            .child(
                TabBar::new("fullscreen-pill-bar")
                    .name(t!("fullscreen-panels"))
                    .when(frosted, TabBar::blurred)
                    .rounded(theme.radius + gap)
                    .items(
                        [
                            Some(tab(
                                "fullscreen-artwork-tab",
                                "icons/disc-3.svg",
                                "fullscreen-artwork",
                                None,
                            )),
                            lyrics.then(|| {
                                tab(
                                    "fullscreen-lyrics",
                                    "icons/mic-vocal.svg",
                                    "lyrics-title",
                                    Some(SideTab::Lyrics),
                                )
                            }),
                            Some(tab(
                                "fullscreen-queue",
                                "icons/list-music.svg",
                                "queue-title",
                                Some(SideTab::Queue),
                            )),
                        ]
                        .into_iter()
                        .flatten(),
                    ),
            )
    }

    fn sound(&self, cx: &mut Context<Self>) -> impl IntoElement {
        rising_volume(&self.volume, &self.playback, ambient::shown(cx), cx)
    }

    /// The desk lyric strip toggle, placed where the player bar has its own in the narrow bar.
    fn desktop_lyrics_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let on = self.settings.read(cx).desktop_lyrics();
        Button::new("fullscreen-desktop-lyrics")
            .ghost()
            .small()
            .icon("icons/monitor.svg")
            .tooltip_above("player-desktop-lyrics")
            .selected(on)
            .tint(match on {
                true => theme.foreground,
                false => theme.muted_foreground,
            })
            .on_click(cx.listener(|this, _, _, cx| {
                let on = this.settings.read(cx).desktop_lyrics();
                this.settings
                    .update(cx, |settings, cx| settings.set_desktop_lyrics(!on, cx));
            }))
            .into_any_element()
    }

    fn mini_button(&self) -> AnyElement {
        Button::new("fullscreen-mini")
            .ghost()
            .small()
            .icon("icons/picture-in-picture.svg")
            .tooltip_above("player-mini")
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                crate::shells::mini::enter(window, cx)
            })
            .into_any_element()
    }

    /// [wisp] 歌词页右侧的悬浮工具条：每颗按钮独立的圆形控件（与原版同款），
    /// 不共用胶囊容器；选择歌词、偏移 ±0.5s、重置。
    /// [wisp] 歌词页左上角的收起键，形态对齐 EchoMusic 的歌词页返回键；随控制条一起淡出。
    fn lyric_tools(
        &self,
        hide: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let lyrics = self.lyrics.read(cx);
        let offset = lyrics.offset_ms();
        let picked = lyrics.picked();
        let has_lyrics = lyrics.current().is_some();
        let stroke = self.settings.read(cx).stroke_lyrics();
        let me = cx.entity().downgrade();

        // 材质与底部 pill 的 frosted 按钮一致：popover 半透明底 + backdrop 模糊
        // + 白描边；形状仍是独立圆形，悬浮轻微放大。模糊关掉时退回不透明 popover。
        let theme = *cx.theme();
        let frosted = ambient::shown(cx) && ui::blurring(cx);
        let narrow = !Room::of(window.viewport_size().width).fits(Room::Wide);
        let bottom = match narrow {
            true => PlayerBar::height(window, cx),
            false => Pixels::ZERO,
        };
        let mut tool = |id: &'static str, icon: &'static str, hint: SharedString| {
            let key: ElementId = id.into();
            let button = div()
                .id(id)
                .size(px(40.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(match frosted {
                    true => theme.popover.opacity(0.35),
                    false => theme.popover,
                })
                .when(frosted, |this| this.backdrop_blur(ui::GLASS_BLUR))
                .border_1()
                .border_color(gpui::white().opacity(0.28))
                .shadow_md()
                .hover(move |this| {
                    this.bg(match frosted {
                        true => theme.popover.opacity(0.5),
                        false => theme.popover,
                    })
                })
                .child(
                    // svg 才能按 text_color 着色，img 会把图标当成位图（永远黑色）。
                    svg()
                        .path(icons::path(icon))
                        .size(px(20.))
                        .text_color(gpui::white()),
                )
                .tip_label(hint, Perch::Trailing);
            growing(button, &key, LYRIC_TOOL_GROW, window, cx)
        };

        div()
            .absolute()
            .right(px(14.))
            .top_0()
            .bottom(bottom)
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .opacity(1. - hide)
            .when(has_lyrics, |column| {
                column
                    // 偏移非 0 时在顶部显示当前值（独立小胶囊）。
                    .when(offset != 0, |column| {
                        column.child(
                            div()
                                .rounded_full()
                                .px(px(8.))
                                .py(px(3.))
                                .bg(match frosted {
                                    true => theme.popover.opacity(0.35),
                                    false => theme.popover,
                                })
                                .when(frosted, |this| this.backdrop_blur(ui::GLASS_BLUR))
                                .border_1()
                                .border_color(theme.border)
                                .text_size(px(11.))
                                .text_color(theme.popover_foreground)
                                .font_features(tabular())
                                .child(SharedString::from(format!("{offset}ms"))),
                        )
                    })
                    .child({
                        let settings = self.settings.clone();
                        let hint = match stroke {
                            true => t!("lyric-stroke-disable"),
                            false => t!("lyric-stroke-enable"),
                        };
                        let button = tool("lyric-tool-stroke", "icons/pencil.svg", hint).on_click(
                            move |_, _, cx| {
                                cx.stop_propagation();
                                settings.update(cx, |settings, cx| {
                                    settings.set_stroke_lyrics(!stroke, cx)
                                });
                            },
                        );
                        div().relative().child(button).when(stroke, |this| {
                            this.child(
                                div()
                                    .absolute()
                                    .right(px(3.))
                                    .bottom(px(3.))
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(gpui::white().opacity(0.92))
                                    .border_1()
                                    .border_color(gpui::black().opacity(0.5)),
                            )
                        })
                    })
                    .child({
                        let button =
                            tool("lyric-tool-choose", "icons/list.svg", t!("lyric-choose"))
                                .on_click({
                                    let me = me.clone();
                                    move |_, _, cx| {
                                        cx.stop_propagation();
                                        me.update(cx, |this, cx| {
                                            this.version_dialog = true;
                                            this.version_pick = Some(this.lyrics.read(cx).chosen());
                                            cx.notify();
                                        })
                                        .ok();
                                    }
                                });
                        div().relative().child(button).when(picked, |this| {
                            this.child(
                                div()
                                    .absolute()
                                    .right(px(3.))
                                    .bottom(px(3.))
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(gpui::rgb(0x34d399))
                                    .border_1()
                                    .border_color(gpui::black().opacity(0.5)),
                            )
                        })
                    })
                    .child(
                        tool(
                            "lyric-tool-back",
                            "icons/undo-2.svg",
                            t!("lyric-offset-back"),
                        )
                        .on_click({
                            let lyrics = self.lyrics.clone();
                            move |_, _, cx| {
                                cx.stop_propagation();
                                lyrics
                                    .update(cx, |lyrics, cx| lyrics.nudge_offset(-LYRIC_NUDGE, cx));
                            }
                        }),
                    )
                    .child(
                        tool(
                            "lyric-tool-forward",
                            "icons/redo-2.svg",
                            t!("lyric-offset-forward"),
                        )
                        .on_click({
                            let lyrics = self.lyrics.clone();
                            move |_, _, cx| {
                                cx.stop_propagation();
                                lyrics
                                    .update(cx, |lyrics, cx| lyrics.nudge_offset(LYRIC_NUDGE, cx));
                            }
                        }),
                    )
                    // 重置在偏移非 0 时现身，其余时候隐形占位保持按钮列不跳。
                    .child(
                        tool(
                            "lyric-tool-reset",
                            "icons/rotate-ccw-clock.svg",
                            t!("lyric-offset-reset"),
                        )
                        .when(offset == 0, |this| this.invisible())
                        .on_click({
                            let lyrics = self.lyrics.clone();
                            move |_, _, cx| {
                                cx.stop_propagation();
                                lyrics.update(cx, |lyrics, cx| lyrics.reset_offset(cx));
                            }
                        }),
                    )
            })
    }

    /// [wisp] 歌词版本对话框（对照原版的双栏浮窗）：左列候选卡（标题、来源、
    /// 时长、使用中标记），右列选中候选的歌词预览；底部应用 / 恢复自动。
    fn version_dialog_panel(&self, _window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let lyrics = self.lyrics.read(cx);
        let chosen = lyrics.chosen();
        let picked = lyrics.picked();
        let pick = self.version_pick.unwrap_or(chosen);
        let track = self.playback.read(cx).shown_track(cx);
        let subtitle = match &track {
            Some(track) => SharedString::from(match track.artists.is_empty() {
                true => track.name.clone(),
                false => format!("{} · {}", track.name, track.artists),
            }),
            None => SharedString::from(""),
        };

        let card = |index: usize, hit: &music::LyricsHit| {
            let selected = index == pick;
            let source = SharedString::from(hit.source.to_owned());
            let title = match hit.title.trim().is_empty() {
                true => t!("lyric-version-number", count = index + 1),
                false => SharedString::from(hit.title.clone()),
            };
            let time = hit
                .duration
                .map(|duration| {
                    SharedString::from(format!(
                        "{}:{:02}",
                        duration.as_secs() / 60,
                        duration.as_secs() % 60
                    ))
                })
                .unwrap_or_else(|| SharedString::from(""));
            div()
                .id(("lyric-version-card", index))
                .relative()
                .flex()
                .flex_col()
                .gap(px(6.))
                .px(px(12.))
                .py(px(10.))
                .rounded(theme.radius)
                .border_1()
                .border_color(match selected {
                    true => theme.primary,
                    false => theme.border,
                })
                .when(selected, |this| this.bg(theme.primary.opacity(0.1)))
                .hover(move |style| style.bg(theme.table_hover))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.version_pick = Some(index);
                    cx.notify();
                }))
                .child(
                    div()
                        .pr(px(22.))
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(title),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .min_w_0()
                                // 来源徽章：官方推荐用主题色，其余灰底（原版同款）。
                                .when_some(hit.badge.clone(), |this, badge| {
                                    let official = badge.contains("官方");
                                    this.child(
                                        div()
                                            .flex_none()
                                            .text_size(px(10.))
                                            .px(px(6.))
                                            .py(px(2.))
                                            .rounded_full()
                                            .when(official, |this| {
                                                this.bg(theme.primary.opacity(0.14))
                                                    .text_color(theme.primary)
                                            })
                                            .when(!official, |this| {
                                                this.bg(theme.foreground.opacity(0.06))
                                                    .text_color(theme.muted_foreground)
                                            })
                                            .child(badge),
                                    )
                                })
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(px(10.))
                                        .text_color(theme.muted_foreground)
                                        .child(source),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(px(11.))
                                        .text_color(theme.muted_foreground)
                                        .child(time),
                                ),
                        )
                        // 推荐星级：score 0-100 折成 5 颗星，实心主题色、空心灰。
                        .when_some(hit.score, |this, score| {
                            let filled = ((score as f32 / 20.).round() as usize).min(5);
                            this.child(
                                div().flex_none().text_size(px(10.)).child(
                                    div()
                                        .flex()
                                        .child(
                                            div()
                                                .text_color(theme.primary)
                                                .child(SharedString::from("★".repeat(filled))),
                                        )
                                        .child(
                                            div()
                                                .text_color(theme.muted_foreground.opacity(0.5))
                                                .child(SharedString::from("★".repeat(5 - filled))),
                                        ),
                                ),
                            )
                        }),
                )
                // 使用中：右上角主题色圆形勾选徽标（原版同款）。
                .when(index == chosen, |this| {
                    this.child(
                        div()
                            .absolute()
                            .right(px(8.))
                            .top(px(8.))
                            .size(px(16.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme.primary)
                            .child(
                                svg()
                                    .path(icons::path("icons/check.svg"))
                                    .size(px(10.))
                                    .text_color(theme.primary_foreground),
                            ),
                    )
                })
        };

        let cards: Vec<_> = lyrics
            .hits()
            .iter()
            .enumerate()
            .map(|(index, hit)| card(index, hit))
            .collect();

        // 预览行：取选中候选的全部行（空行跳过但保留行号），当前使用中的版本
        // 把播放中的那行加粗，与原版的实时预览一致。
        // 播放中的行：只在预览的就是当前版本时才有意义，按含偏移的播放位置
        // 找最后一个已经开始的行（逐行时间轴），纯文本歌词没有时间轴就不标。
        let active = (pick == chosen)
            .then(|| {
                let position = lyrics.offset_time();
                lyrics.hits().get(pick).and_then(|hit| match &hit.lyrics {
                    music::Lyrics::Synced { lines } => {
                        lines.iter().rposition(|line| line.start <= position)
                    }
                    music::Lyrics::Plain { .. } => None,
                })
            })
            .flatten();
        let mut preview_rows: Vec<AnyElement> = Vec::new();
        if let Some(hit) = lyrics.hits().get(pick) {
            let lines: Vec<(&str, Option<String>)> = match &hit.lyrics {
                music::Lyrics::Plain { text, .. } => {
                    text.lines().map(|line| (line, None)).collect()
                }
                music::Lyrics::Synced { lines } => lines
                    .iter()
                    .map(|line| (line.text.as_str(), None))
                    .collect(),
            };
            for (index, (line, _)) in lines.iter().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let current = active == Some(index);
                preview_rows.push(
                    div()
                        .flex_none()
                        .truncate()
                        .text_size(px(13.))
                        .when(current, |this| {
                            this.font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                        })
                        .when(!current, |this| this.text_color(theme.muted_foreground))
                        .child(SharedString::from((*line).to_owned()))
                        .into_any_element(),
                );
            }
        }
        let preview_count = preview_rows.len();
        let selected_hit = lyrics.hits().get(pick);
        let preview_head = selected_hit.map(|hit| {
            (
                match hit.title.trim().is_empty() {
                    true => t!("lyric-version-number", count = pick + 1),
                    false => SharedString::from(hit.title.clone()),
                },
                SharedString::from(hit.source.to_owned()),
            )
        });

        Modal::new("lyric-version-dialog", t!("lyric-choose"))
            .w(px(720.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p(px(12.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(subtitle),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .h(px(380.))
                            // 左列：候选列表，外套一层带描边和淡底的容器（原版同款），
                            // 列表本身走 Scroller，滚动条细而自动隐藏。
                            .child(
                                div()
                                    .w(px(300.))
                                    .h_full()
                                    .rounded(theme.radius)
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(theme.foreground.opacity(0.02))
                                    // 左/上/下各 8px，右边只留 2px：滚动条贴容器右缘，
                                    // 不再压着卡片的右边框。
                                    .pt(px(8.))
                                    .pb(px(8.))
                                    .pl(px(8.))
                                    .pr(px(2.))
                                    .child(
                                        Scroller::new("lyric-version-list", &self.version_scroll)
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .h_full()
                                            .when(picked, |this| {
                                                this.child(
                                                    div()
                                                        .id("lyric-version-auto")
                                                        .flex()
                                                        .items_center()
                                                        .px(px(10.))
                                                        .py(px(8.))
                                                        .rounded(theme.radius)
                                                        .border_1()
                                                        .border_color(theme.border)
                                                        .cursor_pointer()
                                                        .hover(move |style| {
                                                            style.bg(theme.table_hover)
                                                        })
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.lyrics.update(cx, |lyrics, cx| {
                                                                lyrics.choose_auto(cx)
                                                            });
                                                            this.version_dialog = false;
                                                            cx.notify();
                                                        }))
                                                        .child(
                                                            div()
                                                                .text_color(theme.foreground)
                                                                .child(t!("lyric-version-auto")),
                                                        ),
                                                )
                                            })
                                            .children(cards),
                                    ),
                            )
                            // 右列：预览头（标题/来源/行数）+ 可滚动的歌词正文。
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .rounded(theme.radius)
                                    .border_1()
                                    .border_color(theme.border)
                                    .overflow_hidden()
                                    .when_some(preview_head, |this, (title, source)| {
                                        this.child(
                                            div()
                                                .flex_none()
                                                .flex()
                                                .items_start()
                                                .justify_between()
                                                .gap_2()
                                                .px(px(12.))
                                                .py(px(10.))
                                                .border_b_1()
                                                .border_color(theme.border)
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap(px(2.))
                                                        .min_w_0()
                                                        .child(
                                                            div()
                                                                .truncate()
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(theme.foreground)
                                                                .child(title),
                                                        )
                                                        .child(
                                                            div()
                                                                .truncate()
                                                                .text_size(px(11.))
                                                                .text_color(theme.muted_foreground)
                                                                .child(source),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .text_size(px(10.))
                                                        .px(px(8.))
                                                        .py(px(2.))
                                                        .rounded_full()
                                                        .bg(theme.foreground.opacity(0.06))
                                                        .text_color(theme.muted_foreground)
                                                        .child(t!(
                                                            "lyric-version-lines",
                                                            count = preview_count
                                                        )),
                                                ),
                                        )
                                    })
                                    .child(
                                        div()
                                            .id("lyric-version-preview")
                                            .flex()
                                            .flex_col()
                                            .gap(px(6.))
                                            .flex_1()
                                            .min_h_0()
                                            .overflow_y_scroll()
                                            .px(px(12.))
                                            .py(px(10.))
                                            .children(preview_rows),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .when(picked, |this| {
                                this.child(
                                    div()
                                        .rounded(theme.radius)
                                        .bg(theme.foreground.opacity(0.08))
                                        .child(
                                            Button::new("lyric-version-restore")
                                                .ghost()
                                                .label(t!("lyric-version-restore"))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.lyrics.update(cx, |lyrics, cx| {
                                                        lyrics.choose_auto(cx)
                                                    });
                                                    this.version_dialog = false;
                                                    cx.notify();
                                                })),
                                        ),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Button::new("lyric-version-cancel")
                                            .ghost()
                                            .label(t!("lyric-version-cancel"))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.version_dialog = false;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("lyric-version-apply")
                                            .primary()
                                            .label(t!("lyric-version-apply"))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.lyrics.update(cx, |lyrics, cx| {
                                                    lyrics.choose(pick, cx)
                                                });
                                                this.version_dialog = false;
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    ),
            )
            .close_button()
            .on_dismiss(cx.listener(|this, _, _, cx| {
                this.version_dialog = false;
                cx.notify();
            }))
    }

    fn menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (track, position) = self.context_menu.clone()?;

        Some(
            Popup::new(position, self.track_menu.for_track(&track, cx)).on_close(cx.listener(
                |this, _, _, cx| {
                    this.context_menu = None;
                    cx.notify();
                },
            )),
        )
    }

    /// The bar along the bottom below `Room::Wide`, laid out the way the stacked player bar is: the
    /// cover and title with the transport at their end, then the seek bar with the play mode, the
    /// volume and the leave button. The pill floats above it. Its height is the player bar's, so
    /// opening or closing the player leaves every control where it stood.
    fn narrow_bar(
        &self,
        hide: f32,
        idle: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = *cx.theme();
        let transparent = self.settings.read(cx).player_bar_translucent();
        let height = PlayerBar::height(window, cx);
        let dozing = |element: AnyElement, by: f32| {
            div()
                .flex_none()
                .opacity(1. - by)
                .when(by >= 1., |this| this.invisible())
                .child(element)
                .into_any_element()
        };
        let seek = div()
            .flex_1()
            .min_w_0()
            .opacity(1. - hide)
            .when(hide >= 1., |this| this.invisible())
            .child(self.seek(cx));
        let mode = dozing(play_mode(&self.playback, false, cx).into_any_element(), hide);
        let sound = dozing(
            rising_volume(&self.volume, &self.playback, true, cx).into_any_element(),
            hide,
        );
        let speed = dozing(self.speed.clone().into_any_element(), hide);
        let lyrics = self.settings.read(cx).lyrics_enabled();
        let leave = Button::new("leave-fullscreen")
            .ghost()
            .small()
            .icon("icons/minimize.svg")
            .tooltip_above("player-fullscreen-leave")
            .on_click(|_, _, cx| back(cx));
        let leave = match cx.try_global::<ShellSwap>().copied().unwrap_or_default() {
            ShellSwap::Opening => deferred(leave).into_any_element(),
            ShellSwap::Closing => leave.invisible().into_any_element(),
            ShellSwap::Settled => leave.into_any_element(),
        };
        let leave = div()
            .flex_none()
            .relative()
            .top(px(SINK) * idle)
            .opacity(1. - idle)
            .when(idle >= 1., |this| this.invisible())
            .child(leave);

        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let radius = crate::chrome::window_radius(self.settings.read(cx), cx, window);
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let radius: Option<Pixels> = None;

        div()
            .id("fullscreen-bar")
            .role(Role::Toolbar)
            .aria_label(t!("settings-page-player-bar"))
            .occlude()
            .on_click(|_, _, cx| cx.stop_propagation())
            .w_full()
            .flex_none()
            .h(height)
            .px_5()
            .py_2()
            .flex()
            .flex_col()
            .justify_center()
            .gap_2()
            .when_some(radius, |this, radius| this.rounded_b(radius))
            .when_else(
                transparent,
                |this| ui::bar_surface(this, cx).text_color(theme.popover_foreground),
                |this| this.when(!theme.transparent, |this| this.bg(theme.secondary)),
            )
            .border_t_1()
            .border_color(theme.border)
            .child(self.strip(hide, self.panel.is_some(), window, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .child(seek)
                    .child(speed)
                    .child(mode)
                    .child(sound)
                    .when(lyrics, |this| {
                        this.child(dozing(self.desktop_lyrics_button(cx), hide))
                    })
                    .child(dozing(self.mini_button(), hide))
                    .child(leave),
            )
    }

    /// The pill that picks what shows beside the cover and the output button, floating above the
    /// bar below `Room::Wide` so neither moves with the controls.
    fn floating_tools(
        &self,
        hide: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sunk = |element: Div, by: f32| {
            element
                .relative()
                .top(px(SINK) * by)
                .opacity(1. - by)
                .when(by >= 1., |this| this.invisible())
                .block_mouse_except_scroll()
        };
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(PlayerBar::height(window, cx) + px(PILL_LIFT))
            .flex()
            .items_center()
            .px_5()
            .child(
                div()
                    .flex_1()
                    .child(sunk(div().child(self.output_button(cx)), hide)),
            )
            .child(sunk(div().flex_none().child(self.pill(cx)), hide))
            .child(div().flex_1())
    }

    /// The output device button in the glass capsule the panel pill wears, for the bottom bar and
    /// the corner that both stand it in.
    fn output_button(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let frosted = ambient::shown(cx) && ui::blurring(cx);
        let capsule = div()
            .flex_none()
            .flex()
            .items_center()
            .p_1()
            .rounded(theme.radius + px(PILL_GAP))
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary);
        let capsule = match frosted {
            true => ui::glass(capsule, cx).shadow_sm(),
            false => capsule,
        };
        capsule.child(self.output.clone())
    }

    /// The bottom left corner: the output device button in the same glass capsule as the
    /// panel pill on the right, so the two corners read as one row.
    fn output_corner(
        &self,
        hide: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = *cx.theme();
        let drop = PlayerBar::fullscreen_drop(window, cx);
        let sunk = |element: Div, by: f32| element.relative().top(px(SINK) * by).opacity(1. - by);

        div()
            .id("fullscreen-output-corner")
            .absolute()
            .bottom(-drop)
            .left_5()
            .h(snapped(theme.metrics.player_bar, window))
            .flex()
            .items_center()
            .when(hide < 1., |this| {
                this.child(sunk(div().child(self.output_button(cx)), hide))
            })
    }

    /// The bottom right corner: the pill that picks what shows beside the cover, and the leave
    /// button, in one row that never moves with the layout, so a panel coming or going
    /// leaves the pill under the pointer that switched it. The row is centred on a player bar's
    /// height, lowered by as much as the chrome player bar lowers its own fullscreen button in
    /// the layout it has at this width, so that button and the leave button share one spot. The pill sinks and fades
    /// with the controls. The leave button follows the idle spring instead, so a
    /// pointer move still brings it back when the controls are set to stay hidden. While the
    /// player opens the leave button paints above its fade and zoom, and while it closes the
    /// player bar's button paints there instead. The row occludes, so hovering it never
    /// reaches whatever sits behind it.
    fn corner(
        &self,
        hide: f32,
        idle: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = *cx.theme();
        let frosted = ambient::shown(cx);
        let sunk = |element: Div, by: f32| element.relative().top(px(SINK) * by).opacity(1. - by);
        let drop = PlayerBar::fullscreen_drop(window, cx);
        let narrow = !Room::of(window.viewport_size().width).fits(Room::Wide);
        let width = self.corner_width.clone();
        let swap = cx.try_global::<ShellSwap>().copied().unwrap_or_default();

        div()
            .id("fullscreen-corner")
            .occlude()
            .absolute()
            .bottom(-drop)
            .right_5()
            .h(snapped(theme.metrics.player_bar, window))
            .flex()
            .items_center()
            .gap(px(CORNER_GAP))
            .on_hover(cx.listener(|this, hovering: &bool, _, cx| {
                this.over_leave = *hovering;
                cx.notify();
            }))
            // The seek bar reads how wide the whole corner is, so only a corner with everything
            // in it says.
            .when(hide < 1. && idle < 1., |this| {
                this.child(
                    canvas(
                        move |bounds, _, _| width.set(bounds.size.width),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
            })
            // Below `Room::Wide` the transport row has no end to hold the volume, so it
            // stands here with the pill.
            .when(narrow && hide < 1., |this| {
                this.child(sunk(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(play_mode(&self.playback, frosted, cx))
                        .child(self.sound(cx)),
                    hide,
                ))
            })
            .when(hide < 1., |this| {
                this.child(sunk(div().child(self.pill(cx)), hide))
            })
            .when(idle < 1., |this| {
                let leave = Button::new("leave-fullscreen")
                    .ghost()
                    .when(frosted, Button::frosted)
                    .small()
                    .icon("icons/minimize.svg")
                    .tooltip_above("player-fullscreen-leave")
                    .on_click(|_, _, cx| back(cx));
                let leave = match swap {
                    ShellSwap::Opening => deferred(leave).into_any_element(),
                    ShellSwap::Closing => leave.invisible().into_any_element(),
                    ShellSwap::Settled => leave.into_any_element(),
                };
                this.child(sunk(div().child(leave), idle))
            })
    }
}

fn open_album(album: &str, cx: &mut App) {
    navigate(Destination::Album(album.into()), cx);
}

/// What a cover that opens the track's album is called: the album's name, or View album when
/// the track does not name one.
fn album_name(track: &music::Track) -> SharedString {
    match track.album.is_empty() {
        true => t!("song-view-album"),
        false => SharedString::from(track.album.clone()),
    }
}

/// The explicit mark beside a title. `named` puts it in the accessibility tree, so the invisible
/// copy that only balances the title row can stay out of it.
fn explicit_badge(id: &'static str, named: bool) -> impl IntoElement {
    let badge = div().flex_none().child(Badge::explicit());
    match named {
        true => badge
            .id(id)
            .role(Role::Image)
            .aria_label(t!("song-explicit"))
            .into_any_element(),
        false => badge.into_any_element(),
    }
}

impl Shell for FullscreenView {
    fn title_bar(&self, _content: Option<AnyView>, cx: &App) -> TitleBarOptions {
        TitleBarOptions {
            navigation: false,
            sidebar_open: false,
            sidebar_right: None,
            border: false,
            content: None,
            // The cover, the lyrics and the queue are all pages of the player, so each keeps
            // the same way out at the start of the bar.
            left: Some(self.collapse.clone().into()),
            transparent: ambient::shown(cx),
        }
    }
}

impl Render for FullscreenView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let viewport = window.viewport_size();
        let room = Room::of(viewport.width);
        let narrow = !room.fits(Room::Wide);
        let split = room.fits(Room::Wide) && self.panel.is_some();
        self.watch(window, cx);
        let idle = self.hidden(window, cx);
        let switching = self.switching(window, cx);
        let leaving = self.leaving.as_mut().map(|leaving| {
            leaving
                .follow(0., PANEL_LEAVE, SPRING_REST, window, cx)
                .clamp(0., 1.)
        });
        if leaving.is_none_or(|left| left <= 0.) {
            self.leaving = None;
        }
        let leaving = leaving.filter(|left| *left > 0.);
        let hide = match self.settings.read(cx).fullscreen_controls_autohide() {
            FullscreenControlsAutohide::Automatic => idle,
            FullscreenControlsAutohide::AlwaysShown => 0.,
            FullscreenControlsAutohide::AlwaysHidden => 1.,
        };
        let shown = hide < 1.;
        let (tall, wide, ceiling, tall_rest, wide_rest, ceiling_rest) = match room.fits(Room::Wide)
        {
            true => (
                COVER_TALL,
                COVER_WIDE,
                px(COVER_MAX),
                COVER_TALL_REST,
                COVER_WIDE_REST,
                px(COVER_MAX_REST),
            ),
            false => (
                COVER_TALL_TIGHT,
                COVER_WIDE_TIGHT,
                viewport.width,
                COVER_TALL_TIGHT_REST,
                COVER_WIDE_TIGHT,
                viewport.width,
            ),
        };
        // Everything the cover shares the column with, measured rather than guessed: the title
        // bar above it, the gap down to the meta line, the meta itself, the gap under it and
        // the page's bottom padding. Neither meta row sets a line height, so both stand at
        // GPUI's own leading. A window too short for all of it has to shrink the cover, since
        // the column centres what it cannot fit and the top edge is what goes.
        let rem = window.rem_size();
        let line = |step: Text| phi().to_pixels(theme.text(step).into(), rem).round();
        let meta = match narrow {
            true => Pixels::ZERO,
            false => line(Text::Title) + rem * META_GAP + line(Text::Body),
        };
        // The controls fill the split column, so beside no panel they stop at the column's width
        // too and a panel coming or going never stretches them while they fly.
        let column = (viewport.width - rem * (PAGE_SIDE * 2. + ROW_GAP)) / 2.;
        let deck = px(SEEK_MAX).min(column).max(Pixels::ZERO);
        // The page ends as much nearer the window's bottom edge as the corner sits lower, so
        // the transport along the bottom stays level with the corner. Below `Room::Wide` the
        // bar takes the player bar's own height in the flow, and the pill floats above it.
        let bar = PlayerBar::height(window, cx);
        let pill = theme.metrics.control_small + px(PILL_GAP * 2. + 2.);
        let foot = match narrow {
            true => {
                bar + match self.panel {
                    Some(_) => Pixels::ZERO,
                    None => pill + px(PILL_LIFT * 2.),
                }
            }
            false => rem * PAGE_PAD + px(DECK_LIFT) - PlayerBar::fullscreen_drop(window, cx),
        };
        let stack = theme.metrics.title_bar + rem * (COLUMN_GAP * 2.) + foot + meta;
        // The dock reserves the cap it clamps itself to while fading, and nothing at all once
        // it is hidden, which is the whole difference between the awake and the resting fit.
        let dock = match narrow {
            true => Pixels::ZERO,
            false => {
                theme.metrics.player_bar
                    * match split {
                        true => DOCK,
                        false => DOCK_FULL,
                    }
            }
        };
        let fit = |tall: f32, wide: f32, reserve: Pixels, ceiling: Pixels| {
            (viewport.height * tall)
                .min(viewport.height - reserve)
                .min(viewport.width * wide)
                .min(ceiling)
                .max(px(COVER_MIN))
        };
        let near = fit(tall, wide, stack + dock, ceiling);
        // The resting cover is the raster, scaled down while the controls are up, so the layer
        // the compositor scales is that much wider than the cover on screen and reaches well
        // past it on every side. A layer is clipped to the window before it is scaled, so one
        // that overhangs the top edge loses a strip off the cover itself, which is the whole
        // reason the resting size can never reach further up than the room above the cover.
        let slack = (viewport.height - stack - dock - near).max(Pixels::ZERO);
        let above = theme.metrics.title_bar + slack / 2. - px(COVER_LAYER_PAD);
        let far = fit(tall_rest, wide_rest, stack, ceiling_rest)
            .min(near + above * 2.)
            .max(near);
        let presented_side = near + (far - near) * hide;
        // The flex item must never change size when the idle state flips: even a one-frame
        // near/far swap makes the centred column relayout. Keep its awake footprint forever and
        // animate only the fixed large raster surface in the compositor.
        let side = snapped(near, window);
        let raster_side = snapped(far, window);
        let cover_scale = presentation_scale(presented_side, raster_side);
        let staged = self.panel.is_none() || split;
        // The column settles by half of what the dock gives up and the meta by half of what the
        // cover grows, both straight along `hide` to a rest that sits on the device grid, so
        // neither steps a whole pixel at a time through layout in the slow tail of the spring.
        let settle = |rest: Pixels| snapped(rest, window) * hide;
        let shift = match staged && !narrow {
            true => settle(self.dock_natural.get() / 2.),
            false => Pixels::ZERO,
        };
        let lift = match narrow {
            true => Pixels::ZERO,
            false => settle((far - side) / 2.),
        };
        // The cover grows past the column's top and the meta sinks past its bottom while the
        // column glides, so its layer keeps room for both. A split column already fills the
        // height and has the slack around its content to grow into.
        let bleed = match split {
            true => Pixels::ZERO,
            false => snapped((far - side) / 2., window) + px(COVER_LAYER_PAD + SHADOW_BLEED),
        };

        let style = self.settings.read(cx).visualizer_style();
        let visualizer_on = self
            .settings
            .read(cx)
            .visualizer_screens()
            .contains(self.panel)
            && style.shown();
        match visualizer_on
            .then(|| self.playback.read(cx).spectrum(cx))
            .flatten()
        {
            Some(spectrum) => self.visualizer.show(cx.entity_id(), spectrum, window),
            None => self.visualizer.hide(),
        }
        // The spectrum fades in and out, and the band over it with it, so both are drawn for
        // as long as any of the spectrum shows rather than for as long as it is wanted.
        let visualizer_presence = self.visualizer.presence();
        let bottom = |bounds: Bounds<Pixels>| bounds.origin.y + bounds.size.height;
        // The peaks stop an inset short of the artist line, so the glow never reaches the text.
        let visualizer_max =
            (bottom(self.root_bounds.get()) - bottom(self.meta_bounds.get()) - theme.metrics.inset)
                .max(px(VISUALIZER_MIN));
        let root_bounds = self.root_bounds.clone();
        // The visualizer and its veil sit flush with the window's bottom corners.
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let corner = crate::chrome::window_radius(self.settings.read(cx), cx, window);
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let corner: Option<Pixels> = None;
        let visualizer_tint = match ambient::shown(cx) {
            true => theme.tint.map(|tint| theme.primary_of(tint)),
            false => None,
        };
        // Over a panel the visualizer covers the panel's text, so it is drawn between the panel
        // and the cover column, which keeps the controls in that column above it. There it is
        // anchored to the root's own place in the window, which no panel moves, so it stands
        // where it stood behind. The canvas behind `root_bounds` has no insets of its own and
        // starts inside the root's padding, so only its size is the root's and the corner
        // comes from `root_origin`.
        let root = Bounds::new(self.root_origin.get(), self.root_bounds.get().size);
        let over =
            self.panel.is_some() && visualizer_presence > 0. && root.size.height > Pixels::ZERO;
        let mut spectrum: Vec<AnyElement> = Vec::new();
        if visualizer_presence > 0. {
            spectrum.push(
                Visualizer::new(self.visualizer.levels(), visualizer_max)
                    .opacity(visualizer_presence)
                    .style_kind(style)
                    .when_some(visualizer_tint, |this, tint| this.tint(tint))
                    .when_some(corner, |this, radius| this.corner_radius(radius))
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .into_any_element(),
            );
        }
        // Straight over the visualizer and under the controls: a backdrop blurs only what is
        // painted before it, so the bars stop reading through the transport while the title
        // and the artist line keep their own edges. The band fades with the controls, so a
        // hidden set takes it along, and with the spectrum, so it lasts for as long as bars
        // still show under the controls.
        if visualizer_presence > 0. && shown && shared::effects() {
            let band = viewport.height * VEIL;
            spectrum.push(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(band)
                    .opacity((1. - hide) * (visualizer_presence * VEIL_LEAD).min(1.))
                    .child(veil(
                        VeilEdge::Bottom,
                        band,
                        VEIL_BLUR,
                        theme.background,
                        corner,
                        window,
                    ))
                    .into_any_element(),
            );
        }
        let (behind, between) = match over {
            true => (
                Vec::new(),
                Some(
                    anchored().position(root.origin).snap_to_window().child(
                        div()
                            .relative()
                            .w(root.size.width)
                            .h(root.size.height)
                            .children(spectrum),
                    ),
                ),
            ),
            false => (spectrum, None),
        };
        let perches = match self.panel.is_some() {
            true => self.aside.update(cx, |aside, cx| aside.perches(window, cx)),
            false => Vec::new(),
        };

        div()
            .id("fullscreen")
            .role(Role::Main)
            .aria_label(t!("player-fullscreen"))
            .key_context(super::shell_context(WORKSPACE_CONTEXT))
            .track_focus(&self.focus)
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .gap_5()
            // Below `Room::Wide` the page keeps the player bar's own side padding, which is the
            // corner's too, so the strip and the row along the bottom start and end where the
            // bar's do.
            .when_else(
                room.fits(Room::Wide),
                |this| this.px_8(),
                |this| this.px_0(),
            )
            .when(narrow, |this| this.gap_0())
            .pb(match narrow {
                true => Pixels::ZERO,
                false => foot,
            })
            .on_click(|event: &ClickEvent, window, cx| {
                if event.click_count() == 2 {
                    window.dispatch_action(Box::new(ToggleWindowFullscreen), cx);
                }
            })
            .on_mouse_move(cx.listener(Self::hover))
            .on_mouse_exit(cx.listener(|this, _: &MouseExitEvent, _, cx| {
                this.left = true;
                cx.notify();
            }))
            // Capture phase: every click stirs the idle timer, even one a
            // control underneath swallows for itself. Asleep, it only wakes, which covers the
            // frame before the dormant layer first paints.
            .capture_any_mouse_down(cx.listener(
                |this, _: &MouseDownEvent, _, cx| match this.awake {
                    true => this.poke(cx),
                    false => this.rouse(cx),
                },
            ))
            // A trackpad reports fingers that only touch it as a scroll that moves nothing.
            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                if event.delta.pixel_delta(window.line_height()) != Point::default() {
                    if !this.awake {
                        cx.show_cursor();
                    }
                    this.poke(cx);
                }
            }))
            .on_key_down(cx.listener(|this, _: &KeyDownEvent, _, cx| this.poke(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleLyrics, _, cx| this.toggle_panel(SideTab::Lyrics, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleQueue, _, cx| this.toggle_panel(SideTab::Queue, cx)),
            )
            .child(
                canvas(move |bounds, _, _| root_bounds.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child({
                let root_origin = self.root_origin.clone();
                canvas(
                    move |bounds, _, _| root_origin.set(bounds.origin),
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
            })
            .children(behind)
            .child({
                let column = staged.then(|| {
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_5()
                        .min_w_0()
                        .when_else(split, |this| this.flex_1().h_full(), |this| this.w_full())
                        .map(|this| glide(this, shift, bleed, window))
                        // A split column carries the pill in its dock, and its cover
                        // holds still while the panel beside it enters.
                        .when(!split, |this| entering(this, switching))
                        .child(self.artwork(side, raster_side, cover_scale, cx))
                        // The title and artist travel with the cover when a panel
                        // comes or goes. Below `Room::Wide` the bar along the bottom carries
                        // them, and an empty line under the cover only marks where the
                        // visualizer stops.
                        .when_else(
                            narrow,
                            |this| {
                                let meta_bounds = self.meta_bounds.clone();
                                this.child(
                                    div().relative().w_full().h(px(0.)).child(
                                        canvas(
                                            move |bounds, _, _| meta_bounds.set(bounds),
                                            |_, _, _, _| {},
                                        )
                                        .absolute()
                                        .size_full(),
                                    ),
                                )
                            },
                            |this| {
                                this.child(lifted(
                                    Flight::new(
                                        "fullscreen-meta",
                                        self.meta(hide, lift, window, cx),
                                    )
                                    .unscaled()
                                    .follows(self.switch.is_some()),
                                    // The column's own layer would clip the line while it
                                    // is still outside the column's new bounds.
                                    self.on_show,
                                ))
                            },
                        )
                        .when(split, |this| {
                            this.child(self.dock(
                                theme.metrics.player_bar * DOCK,
                                hide,
                                false,
                                room,
                                deck,
                                cx,
                            ))
                        })
                        .into_any_element()
                });
                let panel = self.panel.is_some().then(|| {
                    div()
                        .id("fullscreen-panel")
                        .relative()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .h_full()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child({
                            let panel_bounds = self.panel_bounds.clone();
                            canvas(
                                move |bounds, _, _| panel_bounds.set(bounds),
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .size_full()
                        })
                        .child(entering(
                            div().size_full().min_h_0().child(self.aside.clone()),
                            switching,
                        ))
                        .into_any_element()
                });
                let row = div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_8()
                    .when(narrow, |this| this.px_5());
                // Children paint in order, so the reversed row lists the panel first and still
                // stands the column on the left.
                match over {
                    false => row.children(column).children(panel),
                    true => row
                        .flex_row_reverse()
                        .children(panel)
                        .children(between)
                        .children(column),
                }
            })
            // A panel the pill closed fades out where it stood while the cover takes the middle.
            .when_some(leaving.filter(|_| self.panel.is_none()), |this, left| {
                let at = self.panel_bounds.get();
                let origin = at.origin - self.root_origin.get();
                this.child(
                    div()
                        .absolute()
                        .left(origin.x)
                        .top(origin.y)
                        .w(at.size.width)
                        .h(at.size.height)
                        .opacity(left)
                        .child(self.aside.clone()),
                )
            })
            // The panel's floating buttons stand over it here, above the visualizer and the band
            // over it, which would otherwise cover and blur them.
            .when(!perches.is_empty(), |this| {
                let at = self.panel_bounds.get();
                let origin = at.origin - self.root_origin.get();
                this.child(entering(
                    div()
                        .absolute()
                        .left(origin.x)
                        .top(origin.y)
                        .w(at.size.width)
                        .h(at.size.height)
                        .children(perches),
                    switching,
                ))
            })
            .when(!narrow && !split && self.panel.is_some(), |this| {
                this.child(self.strip(hide, true, window, cx))
            })
            .when(narrow, |this| {
                this.child(self.narrow_bar(hide, idle, window, cx))
            })
            .when(!narrow && !split, |this| {
                this.child(self.dock(
                    theme.metrics.player_bar * DOCK_FULL,
                    hide,
                    !staged,
                    room,
                    deck,
                    cx,
                ))
            })
            .when(!narrow && (idle < 1. || hide < 1.), |this| {
                this.child(self.corner(hide, idle, window, cx))
            })
            .when(!narrow, |this| {
                this.child(self.output_corner(hide, window, cx))
            })
            .when(narrow && hide < 1., |this| {
                this.child(self.floating_tools(hide, window, cx))
            })
            .when(self.panel == Some(SideTab::Lyrics), |this| {
                this.child(self.lyric_tools(hide, window, cx))
            })
            .when(self.version_dialog, |this| {
                this.child(self.version_dialog_panel(window, cx))
            })
            .when(!self.awake && self.on_show, |this| {
                this.child(self.dormant(cx))
            })
            .children(self.menu(cx))
    }
}

/// Wraps a control that a panel moves to another part of the layout, so it slides there from
/// where the one it replaces just stood rather than jumping. It keeps its size as it goes and
/// draws above the layers it crosses while the player is `on_show`. It flies only while
/// `switching`, as a panel comes or goes. Any other move is the controls' own, falling asleep
/// or waking, and follows layout at once. `reach` is how far its bubbles and pop-overs paint
/// past it, which the flying layer keeps room for until the flight has fully landed.
fn travelling(
    id: &'static str,
    name: &'static str,
    switching: bool,
    on_show: bool,
    reach: Pixels,
    element: impl IntoElement,
) -> Flight {
    lifted(
        Flight::new(id, element)
            .seen_as(name)
            .lands_from(name)
            .recent()
            .follows(switching)
            .unscaled()
            .reach(reach),
        on_show,
    )
}

/// Draws a flying control above the window only while the player is `on_show`. A player that
/// is closing fades out as one, and a control drawn above it would stay behind at full strength
/// until its flight landed.
fn lifted(flight: Flight, on_show: bool) -> Flight {
    match on_show {
        true => flight.above(),
        false => flight,
    }
}

/// Places `element` `offset` below where layout puts it: the whole device pixels through layout
/// and the fraction through the compositor, so a slow offset glides rather than stepping a pixel
/// at a time. An offset that sits on the device grid takes no layer. The layer clips to the
/// element's bounds, so `bleed` pads them above and below by as far as the children paint past
/// them, and matching negative margins give that room back to layout. The padding stays at rest
/// too, so nothing moves when the layer comes and goes.
fn glide<E: Styled>(element: E, offset: Pixels, bleed: Pixels, window: &Window) -> E {
    let grid = snapped(offset, window);
    let element = element.py(bleed).my(Pixels::ZERO - bleed).top(grid);
    match offset == grid {
        true => element,
        false => element.layer_translate(point(px(0.), offset - grid)),
    }
}

fn presentation_scale(presented: Pixels, layout: Pixels) -> f32 {
    presented.as_f32() / layout.as_f32()
}

/// Whether the pointer is over the window, focused or not, so hovering an unfocused player still
/// wakes its controls. macOS answers `is_window_hovered` with focus, so there the view's exit
/// event alone tells.
fn hovered(window: &Window) -> bool {
    cfg!(target_os = "macos") || window.is_window_hovered()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_spring_preserves_a_subpixel_size() {
        let presented = px(200.25);
        let layout = px(200.);
        let scale = presentation_scale(presented, layout);

        assert!((layout.as_f32() * scale - presented.as_f32()).abs() < 0.001);
    }
}
