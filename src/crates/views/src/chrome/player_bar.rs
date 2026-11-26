use router::{Destination, Link, navigate};
use std::time::Duration;
use ui::ActiveTheme as _;
use ui::Tails as _;
use ui::Tipped as _;

use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point,
    Render, SharedString, StyledText,
};
use gpui::{Window, deferred, div, px};
use i18n::t;
use input::{ToggleLyrics, ToggleQueue};
use state::{AppSettings, Playback, PlayerBarLayout, Queue, SideTab, Wisp};
use ui::{
    Artwork, Badge, Button, Flight, InlineLink, InlineLinks, Perch, Pinnable, Popup, Room,
    Scrollbar,
};

use crate::chrome::SpeedButton;
use crate::chrome::timeline::Timeline;
use crate::chrome::{Chrome, SidebarRight};
use crate::shared::marquee::{MarqueeState, marquee};
use crate::shared::menus::ItemMenu;
use crate::shared::pins::Pinned as _;
use crate::shared::transport::{
    COVER_FLIGHT, CoverAway, CoverRoom, ShellSwap, climax, like, play_mode, transport,
    transport_controls,
};
use crate::shared::volume::{RisingVolume, VolumeHost, rising_volume};

const SEEK_MAX: f32 = 560.;
/// How wide a small icon button draws: `SMALL_INSET` on each side of a 16 px icon. Every button
/// on the classic bar's right side is one, the sessions picker included.
const ICON_BUTTON: f32 = 32.;
/// The classic bar's spacing in rems: its `px_5` at either end, its `gap_4` between the three
/// columns, the right side's `gap_2` between its groups, the `gap_1` between the panel buttons
/// and the volume group's `gap_1` between the mute button and the slider.
const BAR_PAD: f32 = 1.25;
const COLUMN_GAP: f32 = 1.;
const SIDE_GAP: f32 = 0.5;
const TOOL_GAP: f32 = 0.25;
/// The width the classic bar's middle column holds while the volume slider gives way beside
/// it, the least it shrinks to before the volume folds into a button, and the least it takes
/// once the volume has folded, about the transport's own width over a short seek bar.
const SEEK_KEEP: f32 = 300.;
const SEEK_LEAST: f32 = 260.;
const SEEK_FLOOR: f32 = 220.;
/// The room a side of the classic bar keeps for the track's title once the volume has folded,
/// as a share of the cover metric.
const TITLE_ROOM: f32 = 1.4;
/// The stacked bar's `gap_2` between its two rows, in rems.
const STACK_GAP: f32 = 0.5;
/// The gap between the cover and the title in a now playing row, in rems.
const COVER_GAP: f32 = 0.75;

/// How the bar draws its volume control.
#[derive(Clone, Copy)]
enum Volume {
    /// The mute button with a slider this wide beside it.
    Slider(Pixels),
    /// The mute button alone, with a slider that rises over it on hover.
    Rising,
}

pub(crate) struct PlayerBar {
    playback: Entity<Playback>,
    settings: Entity<AppSettings>,
    track_menu: ItemMenu,
    speed: Entity<SpeedButton>,
    context_menu: Option<(music::Track, Point<Pixels>)>,
    /// The clocks and the seek bar, which follow the position without redrawing the bar.
    timeline: Entity<Timeline>,
    title_marquee: MarqueeState,
    artist_marquee: MarqueeState,
    /// over the button where the bar has no room for one beside it.
    rising: RisingVolume,
}

impl VolumeHost for PlayerBar {
    fn volume(&mut self) -> &mut RisingVolume {
        &mut self.rising
    }

    fn playback(&self) -> &Entity<Playback> {
        &self.playback
    }
}

impl PlayerBar {
    pub fn new(playback: Entity<Playback>, queue: Entity<Queue>, cx: &mut Context<Self>) -> Self {
        let library = Wisp::global(cx).library.clone();
        let settings = Wisp::global(cx).settings.clone();
        let climax = Wisp::global(cx).climax.clone();
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
        cx.observe(&queue, |_, _, cx| cx.notify()).detach();
        cx.observe(&library, |_, _, cx| cx.notify()).detach();
        cx.observe(&climax, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe_global::<ShellSwap>(|_, cx| cx.notify()).detach();
        cx.observe_global::<CoverRoom>(|_, cx| cx.notify()).detach();

        let me = cx.entity_id();
        let playlist_scrollbar = cx.new(|_| Scrollbar::inset().watching(me));
        let speed = cx.new(SpeedButton::new);
        let timeline = cx.new(|cx| Timeline::new(playback.clone(), cx));

        Self {
            playback,
            settings,
            track_menu: ItemMenu::new(playlist_scrollbar, cx),
            speed,
            context_menu: None,
            timeline,
            title_marquee: MarqueeState::new(),
            artist_marquee: MarqueeState::new(),
            rising: RisingVolume::new("volume-rise"),
        }
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

    /// Opens the track menu on whatever is playing when the click lands, so a render does not
    /// have to hold a copy of the track for a menu that is rarely asked for.
    fn open_playing_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(track) = self.playback.read(cx).shown_track(cx) else {
            return;
        };
        self.open_context_menu(track, position, cx);
    }

    fn hover(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.timeline
            .update(cx, |timeline, cx| timeline.hover(event.position, cx));
    }

    /// 音量控件与 Wisp 现版一致：只留一颗静音按钮，指针停在上面时升起竖向滑条。
    fn sound(&self, cx: &mut Context<Self>) -> AnyElement {
        rising_volume(&self.rising, &self.playback, true, cx).into_any_element()
    }

    /// The buttons that open the right panel's queue and lyrics. Private FM is an endless
    /// stream, so its queue is not a finite playlist and gets no entry, as in the old player.
    fn side_buttons(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let fm = self.playback.read(cx).fm_session();
        let (open, tab, lyrics) = {
            let settings = self.settings.read(cx);
            (
                settings.sidebar_right_open(),
                settings.sidebar_right_tab(),
                settings.lyrics_enabled(),
            )
        };

        let button = move |id: &'static str, icon: &'static str, hint: &'static str, side| {
            let showing = open && tab == side;

            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .tooltip_above(hint)
                .selected(showing)
                .tint(match showing {
                    true => theme.foreground,
                    false => theme.muted_foreground,
                })
                .on_click(move |_, window, cx| {
                    let action: Box<dyn gpui::Action> = match side {
                        SideTab::Queue => Box::new(ToggleQueue),
                        SideTab::Lyrics => Box::new(ToggleLyrics),
                    };
                    window.dispatch_action(action, cx);
                })
        };

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .when(lyrics, |this| {
                this.child(button(
                    "player-lyrics",
                    "icons/mic-vocal.svg",
                    "lyrics-title",
                    SideTab::Lyrics,
                ))
            })
            .when(!fm, |this| {
                this.child(button(
                    "player-queue",
                    "icons/list-music.svg",
                    "queue-title",
                    SideTab::Queue,
                ))
            })
            .into_any_element()
    }

    /// The button that opens the fullscreen player. It takes over from the player's leave
    /// button the moment the player starts closing and paints above its fade, and it paints
    /// nothing while the player opens over it.
    fn fullscreen_button(&self, cx: &gpui::App) -> AnyElement {
        let button = Button::new("toggle-fullscreen")
            .ghost()
            .small()
            .icon("icons/maximize.svg")
            .tooltip_above("player-fullscreen")
            // 用按下而不是合成的点击：切歌瞬间的封面飞行动画会重建元素，
            // 松开时的 click 偶尔落到已被替换的元素上，全屏就偶现不响应。
            // 直接走路由，不经过焦点上的 action 派发，搜索输入等焦点不会吞掉它。
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                navigate(Destination::Fullscreen, cx)
            });
        match cx.try_global::<ShellSwap>().copied().unwrap_or_default() {
            ShellSwap::Closing => deferred(button).into_any_element(),
            ShellSwap::Opening => button.invisible().into_any_element(),
            ShellSwap::Settled => button.into_any_element(),
        }
    }

    fn desktop_lyrics_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let on = self.settings.read(cx).desktop_lyrics();
        Button::new("player-desktop-lyrics")
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
        Button::new("player-mini")
            .ghost()
            .small()
            .icon("icons/picture-in-picture.svg")
            .tooltip_above("player-mini")
            // Opening the mini window borrows the app, which cannot happen while this press is
            // still being dispatched. The next foreground task runs after the event has unwound.
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                let main = window.window_handle();
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(1))
                        .await;
                    main.update(cx, |_, window, cx| crate::shells::mini::enter(window, cx))
                        .ok();
                })
                .detach();
            })
            .into_any_element()
    }

    fn now_playing(
        &self,
        room: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let label = match self.playback.read(cx).shown_track(cx) {
            Some(track) => SharedString::from(format!("{} — {}", track.name, track.artists)),
            None => t!("player-nothing-playing"),
        };
        div()
            .id("now-playing")
            .role(Role::Group)
            .aria_label(label)
            .flex()
            .items_center()
            .gap_3()
            .flex_1()
            .min_w_0()
            .child(Self::tuck(
                self.artwork(window, cx),
                Self::cover_room(cx),
                window,
                cx,
            ))
            .when(room, |this| this.child(self.track_details(false, cx)))
    }

    /// How much room the cover takes beside the title right now, which [`CoverRoom`] sets.
    fn cover_room(cx: &App) -> f32 {
        cx.try_global::<CoverRoom>()
            .map_or(1., |room| room.0.clamp(0., 1.))
    }

    /// Tucks the cover that starts a now playing row away by `room`. It keeps its size and spot
    /// and fades, while what follows it slides over its place, so a cover a flight targets never
    /// changes its bounds. With no room left it answers no pointer either.
    fn tuck(
        cover: gpui::Stateful<gpui::Div>,
        room: f32,
        window: &Window,
        cx: &App,
    ) -> gpui::Stateful<gpui::Div> {
        if room >= 1. {
            return cover;
        }
        let taken = Self::cover_side(window, cx) + window.rem_size() * COVER_GAP;
        cover
            .mr(Pixels::ZERO - taken * (1. - room))
            .opacity(room)
            .when(room <= 0., |this| this.invisible())
    }

    /// How big the playing cover draws at this width.
    fn cover_side(window: &Window, cx: &App) -> Pixels {
        let theme = cx.theme();
        let layout = Wisp::global(cx).settings.read(cx).player_bar_layout();
        let side = match layout == PlayerBarLayout::Compact && !Self::stacked(window, layout) {
            true => theme.metrics.thumb,
            false => theme.metrics.row,
        };
        ui::snapped(side, window)
    }

    /// The playing cover keeps its album link and context menu in every layout.
    fn artwork(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let artwork = Self::cover_side(window, cx);
        let away = cx.try_global::<CoverAway>().is_some_and(|away| away.0);
        let shown = self.playback.read(cx).shown_track(cx);
        let track = shown.as_ref();
        let cover = track.and_then(|track| track.cover.clone());
        let album = track.map(|track| match track.album.is_empty() {
            true => t!("song-view-album"),
            false => SharedString::from(track.album.clone()),
        });
        div()
            .id("now-playing-artwork")
            .when_some(track.and_then(|track| track.pin()), Pinnable::pin)
            // [wisp] 点封面进全屏播放器；专辑页改由点歌名进（见 track_details）。
            .when(track.is_some(), |this| {
                this.cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        navigate(Destination::Fullscreen, cx)
                    })
            })
            .when_some(album, |this, album| this.aria_label(album))
            .when(track.is_some(), |this| {
                this.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        window.prevent_default();
                        this.open_playing_menu(event.position, cx);
                    }),
                )
            })
            .child(
                // The fullscreen player's cover flies to and from this one, which stays undrawn
                // while that one is in the air.
                Flight::new(
                    "now-playing-cover",
                    div()
                        .when(away, |this| this.invisible())
                        .child(Artwork::new(cover).size(artwork)),
                )
                .seen_as(COVER_FLIGHT),
            )
            .flex_none()
    }

    /// Track and artist links, explicit badge and favorite action, optionally centered without
    /// moving the cover or transport with them.
    fn track_details(&self, centered: bool, cx: &mut Context<Self>) -> gpui::Div {
        let theme = *cx.theme();
        let muted = theme.muted_foreground;
        let artists = theme.text(ui::Text::Small);
        let shown = self.playback.read(cx).shown_track(cx);
        let track = shown.as_ref();
        let explicit = track.is_some_and(|track| track.explicit);
        let title = match track {
            Some(track) if !centered => {
                let name = SharedString::from(track.name.clone());
                let album_id = track.album_id.clone();
                let pin = track.pin();
                let weak = cx.entity().downgrade();
                let key = format!("{}-{:?}", track.name, album_id);
                marquee(
                    &self.title_marquee,
                    "now-playing-track",
                    &key,
                    move |copy| {
                        let name = name.clone();
                        let album_id = album_id.clone();
                        div()
                            .id(("now-playing-track", copy))
                            .whitespace_nowrap()
                            .when_some(pin.clone(), Pinnable::pin)
                            .map(|this| match album_id {
                                Some(album_id) => this
                                    .aria_label(name.clone())
                                    .hover(|style| style.underline())
                                    .link(Destination::Album(album_id.into())),
                                None => this.role(Role::Label).aria_value(name.clone()),
                            })
                            .on_mouse_down(MouseButton::Right, {
                                let weak = weak.clone();
                                move |event: &MouseDownEvent, window, cx| {
                                    window.prevent_default();
                                    weak.update(cx, |this, cx| {
                                        this.open_playing_menu(event.position, cx)
                                    })
                                    .ok();
                                }
                            })
                            .child(name)
                            .into_any_element()
                    },
                    cx,
                )
            }
            Some(track) => {
                let name = SharedString::from(track.name.clone());
                let text = StyledText::new(name.clone());
                let layout = text.layout().clone();
                div()
                    .id("now-playing-track")
                    .when_some(track.pin(), Pinnable::pin)
                    .map(|this| match track.album_id.clone() {
                        Some(album_id) => this
                            .aria_label(name.clone())
                            .hover(|style| style.underline())
                            .link(Destination::Album(album_id.into())),
                        None => this.role(Role::Label).aria_value(name.clone()),
                    })
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            window.prevent_default();
                            this.open_playing_menu(event.position, cx);
                        }),
                    )
                    .child(text)
                    .tip_clipped(name, layout, Perch::Above)
                    .min_w_0()
                    .truncate()
                    .tails()
                    .into_any_element()
            }
            None => div()
                .id("now-playing-album")
                .role(Role::Status)
                .aria_label(t!("player-nothing-playing"))
                .child(t!("player-nothing-playing"))
                .min_w_0()
                .text_color(muted)
                .truncate()
                .into_any_element(),
        };
        let actions = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .when(explicit, |this| {
                this.child(div().flex_none().child(Badge::explicit()))
            })
            .child(like(track, &self.playback, cx));
        let headline = match centered {
            true => {
                // Equal clearance on both sides keeps the title itself centered. Actions sit
                // outside its measured width and remain reachable even for a truncated title.
                let clearance = theme.metrics.control_small
                    * match explicit {
                        true => 2.,
                        false => 1.,
                    }
                    + theme.metrics.pad;
                div().w_full().flex().justify_center().px(clearance).child(
                    div()
                        .relative()
                        .flex()
                        .items_center()
                        .min_w_0()
                        .min_h(theme.text(ui::Text::Body) * ui::LEADING)
                        .child(title)
                        .child(
                            actions
                                .absolute()
                                .left(gpui::relative(1.))
                                .ml(theme.metrics.pad * 0.5)
                                .top_0()
                                .bottom_0(),
                        ),
                )
            }
            // The row is one line of the title tall, so the heart beside it reaches past it rather
            // than pushing the artist line down.
            false => div()
                .w_full()
                .flex()
                .min_w_0()
                .items_center()
                .h(theme.text(ui::Text::Body) * ui::LEADING)
                .gap_1()
                // Let the title take only what it needs, so the heart sits beside it instead
                // of at the far edge of a stretched row. It still shrinks for long titles.
                .child(div().min_w_0().max_w_full().child(title))
                .child(actions),
        };
        let subtitle = match (centered, track) {
            (false, Some(track)) => {
                let refs = track.artist_refs.clone();
                let text = SharedString::from(track.artists.clone());
                let key = text.clone();
                let color = muted;
                let size = artists;
                let content = marquee(
                    &self.artist_marquee,
                    "now-playing-artist",
                    key.as_ref(),
                    move |copy| {
                        InlineLinks::new(
                            SharedString::from(format!("now-playing-artist-{copy}")),
                            refs.iter().map(|artist| {
                                InlineLink::new(
                                    artist.name.clone(),
                                    artist.id.clone().map(Into::into),
                                )
                            }),
                            text.clone(),
                            color,
                        )
                        .text_size(size)
                        .on_click(|id, cx| {
                            navigate(Destination::Artist(id), cx);
                        })
                        .into_any_element()
                    },
                    cx,
                );
                Some(content)
            }
            (true, Some(track)) => Some(
                InlineLinks::new(
                    "now-playing-artist",
                    track.artist_refs.iter().map(|artist| {
                        InlineLink::new(artist.name.clone(), artist.id.clone().map(Into::into))
                    }),
                    track.artists.clone(),
                    muted,
                )
                .text_size(artists)
                .justify_center()
                .truncate()
                .on_click(|id, cx| {
                    navigate(Destination::Artist(id), cx);
                })
                .into_any_element(),
            ),
            _ => None,
        };
        div()
            .flex()
            .flex_col()
            .line_height(gpui::relative(ui::LEADING))
            .gap(theme.metrics.pad / 2.)
            .when(centered, |this| this.items_center())
            .justify_center()
            .flex_1()
            .min_w_0()
            .child(headline)
            .when_some(subtitle, |this, subtitle| this.child(subtitle))
    }
}

impl PlayerBar {
    /// Splits the classic bar's row: how wide the seek bar's column in the middle is, and how
    /// the volume draws. The two sides always share what is left evenly, so the middle stays
    /// centred. While the middle has `SEEK_KEEP` the row splits in even thirds. Narrower, the
    /// volume slider gives way first, down to `VOLUME_TIGHT`, then the middle, down to
    /// `SEEK_LEAST`, and only then does the volume fold into a button whose slider rises on
    /// hover. From there the middle gives the track's title `TITLE_ROOM`, down to
    /// `SEEK_FLOOR`. `tools` counts the icon buttons before the volume.
    fn classic(window: &Window, tools: usize, cx: &gpui::App) -> Pixels {
        let title = cx.theme().metrics.cover * TITLE_ROOM;
        let rem = window.rem_size();
        let button = px(ICON_BUTTON);
        let row = window.viewport_size().width - rem * (BAR_PAD + COLUMN_GAP) * 2.;
        let tools = button * tools as f32 + rem * TOOL_GAP * tools.saturating_sub(1) as f32;
        // The right side: the icons, the mute and fullscreen buttons and the gaps between them.
        let rising = tools + button * 2. + rem * SIDE_GAP * 2.;
        let keep = (row / 3.).max(px(SEEK_KEEP)).min(px(SEEK_MAX));
        (row - title * 2.)
            .max(px(SEEK_FLOOR))
            .min(keep)
            .min(row - rising * 2.)
    }

    /// The volume slider keeps its full width until the window is as narrow as the one that
    /// stacks the classic bar, so a layout that stacks sooner still matches it.
    /// Centered needs room for its three columns; smaller windows keep every control reachable
    /// in the shared responsive layout. Only the whole window's width determines the choice.
    fn stacked(window: &Window, layout: PlayerBarLayout) -> bool {
        let room = match layout {
            PlayerBarLayout::Centered => Room::Wide,
            _ => Room::Roomy,
        };
        !Chrome::window_room(window).fits(room)
    }

    /// How far below its place in a full-height bar the fullscreen button sits, which is where
    /// a stacked or compact bar puts it. The fullscreen player drops its own corner by as
    /// much, so the button stays under the pointer when the player opens and closes.
    pub(crate) fn fullscreen_drop(window: &Window, cx: &gpui::App) -> Pixels {
        let theme = *cx.theme();
        let layout = Wisp::global(cx).settings.read(cx).player_bar_layout();
        let spare = theme.metrics.player_bar - Self::height(window, cx);
        let drop = match Self::stacked(window, layout) {
            // The button's row is centred under the cover's row and the gap between the two.
            true => spare + theme.metrics.row + window.rem_size() * STACK_GAP,
            false => spare,
        };
        ui::snapped((drop / 2.).max(Pixels::ZERO), window)
    }

    /// The height the workspace reserves, shared by the overlay's bottom scroll clearance.
    pub(crate) fn height(window: &Window, cx: &gpui::App) -> Pixels {
        let theme = *cx.theme();
        let layout = Wisp::global(cx).settings.read(cx).player_bar_layout();
        let height = match (Self::stacked(window, layout), layout) {
            (true, _) => theme.metrics.player_bar + theme.metrics.pad * 3.,
            (false, PlayerBarLayout::Compact) => theme.metrics.row + theme.metrics.pad * 2.,
            _ => theme.metrics.player_bar,
        };
        ui::snapped(height, window)
    }
}

impl Render for PlayerBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let span = Chrome::window_room(window);
        let layout = self.settings.read(cx).player_bar_layout();
        let stacked = Self::stacked(window, layout);
        let transparent = self.settings.read(cx).player_bar_translucent();
        let height = Self::height(window, cx);
        // Equal wings keep the title centered while reserving the original volume slider width.
        let lyrics = self.settings.read(cx).lyrics_enabled();
        let wing_controls = 5. + if lyrics { 1. } else { 0. };
        let centered_wing = theme.metrics.control * wing_controls + theme.metrics.pad * 3.;
        let show_track = span.fits(Room::Snug);
        let fm = self.playback.read(cx).fm_session();
        let sides = match SidebarRight::available(window) {
            true => Some(self.side_buttons(cx)),
            false => None,
        };
        // [wisp] 共享收听按钮的位置改放变速控件，始终可用。
        let picker = true;
        // [wisp] 播放模式键排在音量左侧，也算一颗音量前的图标按钮；mini 键同理。
        let tools = 1
            + usize::from(picker)
            + 1
            + usize::from(lyrics)
            + match sides.is_some() {
                true => usize::from(lyrics) + usize::from(!fm),
                false => 0,
            };
        let buttons = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .when(picker, |this| this.child(self.speed.clone()))
            .children(sides);

        let seek = self.timeline.clone();

        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let radius = crate::chrome::window_radius(self.settings.read(cx), cx, window);
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let radius: Option<Pixels> = None;

        let base = div()
            .id("player-bar")
            .role(Role::Toolbar)
            .aria_label(t!("settings-page-player-bar"))
            .flex()
            .w_full()
            .h(height)
            .flex_none()
            .px_5()
            .when(stacked, |this| this.py_2())
            .when_some(radius, |this, radius| this.rounded_b(radius))
            .when_else(
                transparent,
                |this| ui::bar_surface(this, cx).text_color(theme.popover_foreground),
                |this| this.when(!theme.transparent, |this| this.bg(theme.secondary)),
            )
            .border_t_1()
            .border_color(theme.border)
            .occlude()
            .on_mouse_move(cx.listener(Self::hover));

        let context_menu = self.context_menu.clone().map(|(track, position)| {
            Popup::new(position, self.track_menu.for_track(&track, cx)).on_close(cx.listener(
                |this, _, _, cx| {
                    this.context_menu = None;
                    cx.notify();
                },
            ))
        });

        let content = match (stacked, layout) {
            (true, _) => base
                .flex_col()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .w_full()
                        .child(self.now_playing(show_track, window, cx))
                        .child(transport(&self.playback, false, cx)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .w_full()
                        .child(div().flex_1().min_w_0().child(seek))
                        .child(buttons)
                        .child(play_mode(&self.playback, false, cx))
                        .child(self.sound(cx))
                        .when(lyrics, |this| this.child(self.desktop_lyrics_button(cx)))
                        .child(self.mini_button())
                        .child(self.fullscreen_button(cx)),
                ),
            (false, PlayerBarLayout::Centered) => base
                .items_center()
                .py(theme.metrics.pad)
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w(centered_wing)
                        .h_full()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(self.artwork(window, cx))
                        .child(transport(&self.playback, false, cx)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .max_w(px(SEEK_MAX))
                        .h_full()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .child(self.track_details(true, cx).w_full())
                        .child(div().flex_none().w_full().child(seek)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(centered_wing)
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap_2()
                        .child(buttons)
                        .child(play_mode(&self.playback, false, cx))
                        .child(self.sound(cx))
                        .when(lyrics, |this| this.child(self.desktop_lyrics_button(cx)))
                        .child(self.mini_button())
                        .child(self.fullscreen_button(cx)),
                ),
            (false, PlayerBarLayout::Compact) => base
                .items_center()
                .gap_3()
                .child(
                    self.now_playing(show_track, window, cx)
                        .max_w(theme.metrics.cover * 1.6),
                )
                .child(transport(&self.playback, false, cx))
                .child(div().flex_1().min_w_0().child(seek))
                .child(buttons)
                .child(play_mode(&self.playback, false, cx))
                .child(self.sound(cx))
                .when(lyrics, |this| this.child(self.desktop_lyrics_button(cx)))
                .child(self.mini_button())
                .child(self.fullscreen_button(cx)),
            // The two sides share what the middle leaves them evenly, so the transport and the
            // seek bar stay centred. `classic` works out how wide the middle is and how the
            // volume draws.
            _ => {
                let middle = Self::classic(window, tools, cx);
                // The climax key sits right beside `next`, but it must not shift the
                // transport: only prev/play/next take part in centring, and the key is placed
                // one gap past the next button's right edge.
                let flame = px(ICON_BUTTON) * 1.5 + window.rem_size();
                base.items_center()
                    .gap_4()
                    .child(self.now_playing(show_track, window, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_1()
                            .flex_none()
                            .w(middle)
                            .child(
                                div()
                                    .relative()
                                    .w_full()
                                    .flex()
                                    .justify_center()
                                    .child(transport_controls(&self.playback, cx))
                                    .child(
                                        div()
                                            .absolute()
                                            .left(gpui::relative(0.5))
                                            .ml(flame)
                                            .top_0()
                                            .bottom_0()
                                            .flex()
                                            .items_center()
                                            .child(climax(&self.playback, false, cx)),
                                    ),
                            )
                            .child(seek),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(buttons)
                            .child(play_mode(&self.playback, false, cx))
                            .child(self.sound(cx))
                            .when(lyrics, |this| this.child(self.desktop_lyrics_button(cx)))
                            .child(self.mini_button())
                            .child(self.fullscreen_button(cx)),
                    )
            }
        };

        content
            .relative()
            .when_some(context_menu, |this, menu| this.child(menu))
    }
}
