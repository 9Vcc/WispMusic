use std::cell::RefCell;
use std::rc::Rc;

use crate::chrome::Chrome;
use crate::shared::menus::{Item, ItemMenu};
use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Context, ElementId, Entity, MouseDownEvent,
    Pixels, Point, Render, ScrollHandle, SharedString, WeakEntity, Window, div, px,
};
use i18n::t;
use music::GenreItem;
use router::{Destination, navigate};
use state::{Home, Network, Origin, Playback, Wisp};
use ui::{
    ActiveTheme as _, Button, GLASS_BLUR, Lazy, Mode, Perch, Popup, Scrollbar, Scroller, Springs,
    Sprung, Tipped, blurring, growing, mix,
};

use crate::shared::cells;
use crate::shared::picks::{Picks, Shape};
use crate::shared::shelves::Shelves;
use crate::shared::tracks::{PlaybackStatus, playback_status};
use crate::shared::trouble;

const STEADY: Pixels = px(0.5);
/// The provider switch draws icon-only segments; these keep the thumb over the active one.
const SWITCH_ICON: f32 = 30.;
const SWITCH_GAP: f32 = 2.;
const SWITCH_PAD: f32 = 3.;
const SWITCH_STEP: f32 = SWITCH_ICON + SWITCH_GAP;
/// How close, in slots, the provider thumb's spring has to come before it counts as landed.
const SWITCH_REST: f32 = 0.01;

pub(crate) struct HomeView {
    home: Entity<Home>,
    playback: Entity<Playback>,
    playback_status: PlaybackStatus,
    shelves: Entity<Shelves>,
    width: Pixels,
    quick_picks: Pager,
    scrollbar: Entity<Scrollbar>,
    /// The submenu state of a track's context menu.
    menus: ItemMenu,
    context_menu: Option<(Item, Point<Pixels>)>,
    /// [wisp] 指针停在哪张入口卡上（"daily" / "fm"），驱动底色渐变。
    hovered_card: Option<&'static str>,
}

#[derive(Default)]
struct Pager {
    columns: usize,
    page: usize,
}

impl Pager {
    fn fit(&mut self, columns: usize, pages: usize) -> usize {
        if self.columns != columns {
            self.columns = columns;
            self.page = 0;
        }
        self.page = self.page.min(pages.saturating_sub(1));
        self.page
    }
}

/// How much of the theme's accent the card's play button keeps over the glass, as the cover
/// cards' hover play button does.
const PLAY_FILL: f32 = 0.55;
/// How far the play button grows under the pointer.
const PLAY_GROW: f32 = 1.08;

/// 入口卡上的立即播放按钮：材质与封面悬浮播放键一致（半透明主题色 + 毛玻璃 + 阴影），
/// 但保持方形圆角，并在悬浮时弹簧放大。
fn play_button(
    id: &'static str,
    playing: bool,
    window: &mut Window,
    cx: &mut Context<HomeView>,
) -> AnyElement {
    let icon = match playing {
        true => "icons/pause-filled.svg",
        false => "icons/play-filled.svg",
    };
    let hint = match playing {
        true => "play-pause",
        false => "common-play",
    };
    let fill = cx.theme().cover_fill(None);
    // 点按钮只播歌单／开始漫游，不再跳转页面：先把点击拦在这里，别冒泡到卡片。
    let press: Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App)> = match id {
        "daily-play" => Box::new(cx.listener(|this, _, _, cx| {
            cx.stop_propagation();
            let playback = this.playback.clone();
            let origin =
                Origin::playlist("daily-recommend".to_owned()).named("每日推荐".to_owned());
            if playback.read(cx).engaged(&origin, cx) {
                playback.update(cx, |playback, cx| playback.toggle_play(cx));
                return;
            }
            let tracks = (*this.home.read(cx).daily_recommend()).clone();
            if tracks.is_empty() {
                return;
            }
            playback.update(cx, |playback, cx| {
                playback.start(tracks, 0, Some(origin), cx)
            });
        })),
        _ => Box::new(cx.listener(|this, _, _, cx| {
            cx.stop_propagation();
            let playback = this.playback.clone();
            match this.playback.read(cx).fm_session() {
                true => playback.update(cx, |playback, cx| playback.toggle_play(cx)),
                false => playback.update(cx, |playback, cx| playback.start_roaming(cx)),
            }
        })),
    };

    let button = Button::new(id)
        .primary()
        .icon(icon)
        .tooltip(hint)
        .size(px(36.))
        .rounded(px(10.))
        .fill(fill.background.opacity(PLAY_FILL), fill.hover)
        .tint(fill.foreground)
        .when(blurring(cx), |button| button.backdrop_blur(GLASS_BLUR))
        .shadow_sm()
        .on_click(move |event, window, app| press(event, window, app));
    let key = ElementId::from(id);
    growing(
        div()
            .id(SharedString::from(format!("home-play-{id}")))
            .child(button),
        &key,
        PLAY_GROW,
        window,
        cx,
    )
    .into_any_element()
}

impl HomeView {
    pub(crate) fn new(
        home: Entity<Home>,
        playback: Entity<Playback>,
        cx: &mut Context<Self>,
    ) -> Self {
        let me = cx.entity_id();
        let playlist_scrollbar = cx.new(|_| Scrollbar::inset().watching(me));
        let menus = ItemMenu::new(playlist_scrollbar, cx);

        cx.observe(&home, |this, _, cx| {
            this.menus.reset(cx);
            this.context_menu = None;
            cx.notify();
        })
        .detach();
        let chrome = Chrome::entity(cx);
        cx.observe(&chrome, |_, _, cx| cx.notify()).detach();

        let library = Wisp::global(cx).library.clone();
        cx.observe(&library, |_, _, cx| cx.notify()).detach();
        let session = Wisp::global(cx).session.clone();
        cx.observe(&session, |_, _, cx| cx.notify()).detach();

        let shelves = cx.new(|_| {
            Shelves::new("home-shelf", me, playback.clone())
                .spaced(px(20.))
                .home()
        });
        cx.observe(&shelves, |_, _, cx| cx.notify()).detach();

        let current_playback = playback_status(&playback, cx);
        cx.observe(&playback, |this, playback, cx| {
            let current = playback_status(&playback, cx);
            if this.playback_status != current {
                this.playback_status = current;
                cx.notify();
            }
        })
        .detach();

        Self {
            home,
            playback,
            playback_status: current_playback,
            shelves,
            width: Pixels::ZERO,
            quick_picks: Pager::default(),
            scrollbar: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(me)),
            menus,
            context_menu: None,
            hovered_card: None,
        }
    }

    pub(crate) fn home_state(&self) -> Entity<state::Home> {
        self.home.clone()
    }

    /// Tells the feed whether this page is the one on screen, so it lands changes to what is
    /// drawn only while nobody is looking.
    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.home
            .update(cx, |home, cx| home.set_visible(visible, cx));
    }

    /// Fetches the feed and the library it mixes from again.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.home.update(cx, |home, cx| home.refresh(cx));
    }

    /// Whether Quick picks or the shelves are still on their way.
    pub(crate) fn is_loading(&self, cx: &App) -> bool {
        let home = self.home.read(cx);
        home.is_loading(cx) || home.is_feeding()
    }

    /// The Quick picks deck, the one deck on the page: what the provider lists as played lately
    /// and its picks after them, a skeleton while they are on their way.
    fn quick_picks(&mut self, width: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let items = self.home.read(cx).quick_picks();
        let shape = Shape::new(width, items.len());
        let pages = shape.pages;
        let page = self.quick_picks.fit(shape.columns, shape.pages);
        let opened = items.clone();
        let home = cx.entity().downgrade();

        Picks::mixed("quick-picks", items, self.playback.clone(), width, page)
            .title("home-quick-picks")
            .vacancy("home-quick-picks-empty")
            .loading(self.home.read(cx).is_loading(cx))
            .on_previous(cx.listener(|this, _, _, cx| {
                this.quick_picks.page = this.quick_picks.page.saturating_sub(1);
                this.context_menu = None;
                cx.notify();
            }))
            .on_next(cx.listener(move |this, _, _, cx| {
                this.quick_picks.page = (this.quick_picks.page + 1).min(pages.saturating_sub(1));
                this.context_menu = None;
                cx.notify();
            }))
            .on_context_menu(move |place, event, _, cx| {
                open_item_menu(&home, &opened, place, event, cx);
            })
            .into_any_element()
    }

    /// 今天几月几日（不引 chrono，按 civil-from-days 换算）
    fn civil_today() -> (i64, i64) {
        use std::time::{SystemTime, UNIX_EPOCH};
        // 按本地时区（UTC+8）算日期：直接用 UTC 会在 20 点后错成前一天。
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64 + 8 * 3_600)
            .unwrap_or(0);
        let z = secs / 86_400 + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = mp + if mp < 10 { 3 } else { -9 };
        (month, day)
    }

    fn greeting(cx: &App) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let hour = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() / 3600)
            .unwrap_or(0)
            + 8)
            % 24;
        let part = match hour {
            0..=5 => "凌晨好",
            6..=10 => "早上好",
            11..=12 => "中午好",
            13..=17 => "下午好",
            _ => "晚上好",
        };
        let name = match Wisp::global(cx).session.read(cx).state() {
            state::SessionState::SignedIn(profile) => profile.display_name.clone(),
            _ => "朋友".to_owned(),
        };
        format!("Hi, {name} {part}")
    }

    /// The connected services as one icon slider, so the API switches from the home page in a
    /// click instead of opening the account menu. One service has nothing to switch to, so the
    /// slider stays out until a second account is stored. The lit segment is one thumb that
    /// springs to the service just picked, turning from wherever it stands if the pick changes
    /// again while it is still moving.
    fn providers_switch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let session = Wisp::global(cx).session.clone();
        let connected: Vec<_> = session.read(cx).connected().collect();
        if connected.len() < 2 {
            return None;
        }
        let active = connected.iter().position(|info| info.active);
        let theme = *cx.theme();

        // The thumb's place is a spring kept in keyed element state, the same way a button
        // keeps its resizing width: it survives every re-render, steps on the wall clock each
        // frame it draws, and asks for the next frame itself while it moves. An animation
        // wrapper keyed by a fresh id needs the element-state dance to line up exactly with
        // the re-render that carries the new slot; a spring the view keeps across frames
        // cannot lose the slide to a re-render at all.
        let thumb = active.map(|slot| {
            let slide = window.use_keyed_state("home-provider-slide", cx, |_, _| {
                RefCell::new(Sprung::at(slot as f32))
            });
            let at = slide.read(cx).borrow_mut().follow(
                slot as f32,
                Springs::RESPONSIVE,
                SWITCH_REST,
                window,
                cx,
            );
            div()
                .absolute()
                .top(px(SWITCH_PAD))
                .left(px(SWITCH_PAD + at * SWITCH_STEP))
                .size(px(SWITCH_ICON))
                .rounded(theme.radius)
                .bg(theme.primary.opacity(0.22))
                .border_1()
                .border_color(theme.primary.opacity(0.55))
        });

        let segments = connected.into_iter().enumerate().map(|(place, info)| {
            let slug = info.slug.clone();
            let chosen = active == Some(place);
            let pending = info.pending;
            let session = session.clone();
            div()
                .id(("home-provider", place))
                .tip_label(info.name.clone(), Perch::Below)
                .child(
                    Button::new(("home-provider", place))
                        .icon(crate::shared::provider_logo(info.slug.as_ref(), cx))
                        .name(info.name.clone())
                        .size(px(SWITCH_ICON))
                        .ghost()
                        .tint(match chosen {
                            true => theme.foreground,
                            false => theme.muted_foreground,
                        })
                        .disabled(pending)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if chosen {
                                return;
                            }
                            // The switch settles the new slot synchronously, so the notify
                            // below redraws once with the new target and the spring carries
                            // the thumb there. A switch that is refused changes nothing, and
                            // the thumb stays put.
                            session.update(cx, |session, cx| session.switch(slug.as_ref(), cx));
                            cx.notify();
                        })),
                )
        });

        Some(
            div()
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .p(px(SWITCH_PAD))
                .rounded(theme.radius)
                .bg(theme.secondary)
                .border_1()
                .border_color(theme.border)
                .children(thumb)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(SWITCH_GAP))
                        .children(segments),
                )
                .into_any_element(),
        )
    }

    /// [wisp] 首页顶部两张扁平入口卡（按参考图：彩色图标块 + 标题/副标题 + 右侧描边按钮）。
    fn cards(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        // [wisp] 透明窗口下的卡片只剩一层薄雾，必须靠背景模糊才立得住；模糊是每帧最贵的
        // 一笔绘制，所以只在真正透出桌面、且用户没关掉毛玻璃时才挂。
        let frosted = theme.transparent && ui::blurring(cx);
        let (_, day) = Self::civil_today();
        let daily = self.home.read(cx).daily_recommend();
        let first = daily.first().cloned();
        // [wisp] FM 卡片副标题：正在漫游时显示当前曲，其余时候是固定文案。
        // 以前取的是「最近听的一首歌」，看起来像副标题串成了歌名。
        let fm_subtitle: gpui::SharedString = match self.playback.read(cx).track() {
            Some(track) if self.playback.read(cx).fm_session() => {
                t!("home-fm-now", name = track.name.as_str())
            }
            _ => t!("radio-tagline"),
        };
        let playing = self.playback.read(cx).control().unwrap_or(false);
        // 两张卡各自显示自己的播放状态：每日推荐在播时它亮，私人 FM 会话在播时它亮，
        // 不要用全局的在播状态（否则其他歌单也会让漫游按钮变成暂停）。
        let daily_origin =
            Origin::playlist("daily-recommend".to_owned()).named("每日推荐".to_owned());
        let daily_playing = {
            let playback = self.playback.read(cx);
            playback.engaged(&daily_origin, cx) && playing
        };
        let fm_playing = playing && self.playback.read(cx).fm_session();

        let daily_card = {
            let daily_hover = self.hovered_card == Some("daily");
            // [wisp] 卡片底色由 theme.card() 给：不透明窗口上就是原来那个 94% 的页面色，
            // 窗口调透后换成一块跟随透明度固定的玻璃面板，不再是一块死黑/死白的板子。
            let resting = theme.card(frosted);
            let hovered = theme.card_hover(frosted);
            div()
                .id("home-daily-card")
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(72.))
                .flex()
                .items_center()
                .gap(px(14.))
                .px(px(14.))
                .rounded(px(12.))
                .border_1()
                .border_color(theme.border.alpha(0.7))
                .bg(resting)
                // [wisp] 幽灵到只剩薄雾时，靠背后那层模糊把卡片和壁纸分开；不模糊就是一格
                // 半透明的色块，桌面上的图标会直接穿在文字后面。
                .when(frosted, |this| this.backdrop_blur(ui::GLASS_BLUR))
                .cursor_pointer()
                .on_click(cx.listener(|_, _, _, cx| navigate(Destination::DailyRecommend, cx)))
                .on_hover(cx.listener(|this, over: &bool, _, cx| {
                    this.hovered_card = over.then_some("daily");
                    cx.notify();
                }))
                .with_animation(
                    SharedString::from(format!("home-card-daily-{daily_hover}")),
                    Animation::new(std::time::Duration::from_millis(170))
                        .with_easing(ui::motion::ease_out_cubic),
                    move |this, delta| {
                        // 离开时不做过渡：直接回到常态，避免重绘时先上浮再落回的抖动。
                        if !daily_hover {
                            return this
                                .bg(resting)
                                .border_color(theme.border.alpha(0.7))
                                .top(px(0.));
                        }
                        this.bg(mix(resting, hovered, delta))
                            .border_color(mix(theme.border.alpha(0.7), theme.border, delta))
                            // 悬浮时整张卡轻轻上浮。
                            .top(px(-3. * delta))
                    },
                )
                .child(
                    div()
                        .size(px(40.))
                        .rounded(px(9.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::rgb(0x1db954))
                        .text_color(gpui::white())
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(day.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_color(theme.foreground)
                                .font_weight(gpui::FontWeight::BOLD)
                                .child(t!("home-daily")),
                        )
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(t!("home-daily-detail")),
                        ),
                )
                .when_some(first.clone(), |this, _| {
                    this.child(play_button("daily-play", daily_playing, window, cx))
                })
        };

        let fm_card = {
            let fm_hover = self.hovered_card == Some("fm");
            let resting = theme.card(frosted);
            let hovered = theme.card_hover(frosted);
            div()
                .id("home-fm-card")
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(72.))
                .flex()
                .items_center()
                .gap(px(14.))
                .px(px(14.))
                .rounded(px(12.))
                .border_1()
                .border_color(theme.border.alpha(0.7))
                .bg(resting)
                .when(frosted, |this| this.backdrop_blur(ui::GLASS_BLUR))
                .cursor_pointer()
                .on_click(cx.listener(|_, _, _, cx| navigate(Destination::Radio, cx)))
                .on_hover(cx.listener(|this, over: &bool, _, cx| {
                    this.hovered_card = over.then_some("fm");
                    cx.notify();
                }))
                .with_animation(
                    SharedString::from(format!("home-card-fm-{fm_hover}")),
                    Animation::new(std::time::Duration::from_millis(170))
                        .with_easing(ui::motion::ease_out_cubic),
                    move |this, delta| {
                        if !fm_hover {
                            return this
                                .bg(resting)
                                .border_color(theme.border.alpha(0.7))
                                .top(px(0.));
                        }
                        this.bg(mix(resting, hovered, delta))
                            .border_color(mix(theme.border.alpha(0.7), theme.border, delta))
                            .top(px(-3. * delta))
                    },
                )
                .child(
                    div()
                        .size(px(40.))
                        .rounded(px(9.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::rgb(0x3a8ee6))
                        .text_color(gpui::white())
                        .font_weight(gpui::FontWeight::BOLD)
                        .child("FM"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_color(theme.foreground)
                                .font_weight(gpui::FontWeight::BOLD)
                                .child(t!("home-radio")),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.muted_foreground)
                                .child(fm_subtitle),
                        ),
                )
                .child(play_button("fm-toggle", fm_playing, window, cx))
        };

        // 窄窗口下两张卡并排会把文字挤扁、右侧溢出：内容区不够宽就上下堆叠。
        let stacked = self.width < px(560.);
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_end()
                    .gap(px(16.))
                    .when(self.width < px(640.), |this| this.flex_col().items_start())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(theme.foreground)
                                    .child(Self::greeting(cx)),
                            )
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(t!("home-radio-detail")),
                            ),
                    )
                    .when_some(self.providers_switch(window, cx), |this, switch| {
                        this.child(switch)
                    }),
            )
            .child(
                div()
                    .flex()
                    .when(stacked, |this| this.flex_col())
                    .gap(px(16.))
                    .child(daily_card)
                    .child(fm_card),
            )
            .into_any_element()
    }

    /// The page home shows in place of its shelves: the No connection state the moment the
    /// network is gone, since every row here comes from the provider, and the reason its feed
    /// failed otherwise. Asking for the feed again is the retry, and it starts the pauses
    /// between tries over.
    fn failure(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let reason = match Network::lost(cx) {
            true => None,
            false => Some(self.home.read(cx).error()?.to_owned()),
        };
        let home = self.home.clone();

        Some(
            trouble::lost(
                "home-lost",
                t!("trouble-not-loaded"),
                reason.as_deref(),
                move |_, _, cx| {
                    home.update(cx, |home, cx| home.retry(cx));
                },
                cx,
            )
            .size_full()
            .into_any_element(),
        )
    }
}

impl Render for HomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(failure) = self.failure(cx) {
            return div()
                .id("home-failure")
                .role(Role::Alert)
                .size_full()
                .child(failure)
                .into_any_element();
        }

        let theme = *cx.theme();
        let available = cells::content_width(window, theme.metrics.inset * 2., cx);
        if (available - self.width).abs() >= STEADY {
            self.width = available;
        }
        let context_menu = self.context_menu.clone().map(|(item, position)| {
            let menu = item.menu(&self.menus, self.playback.clone(), false, cx);
            Popup::new(position, menu).on_close(cx.listener(|this, _, _, cx| {
                this.context_menu = None;
                cx.notify();
            }))
        });

        let view = cx.entity();
        let cards = Lazy::new("home-wisp-cards", move |window, cx| {
            view.update(cx, |this, cx| this.cards(window, cx))
        });
        let sections = self.home.read(cx).sections();
        let feeding = self.home.read(cx).is_feeding();
        let width = self.width;
        let shelves = self
            .shelves
            .update(cx, |shelves, cx| match sections.is_empty() && feeding {
                true => shelves.pending(width, cx),
                false => vec![shelves.render(sections, Mode::Grid, width, window, cx)],
            });

        Scroller::new("home-page", &self.scrollbar)
            .hold()
            .bottom_inset(crate::chrome::Chrome::track_bottom(cx))
            // [wisp] 对齐原版主页的横向 px-8 留白；纵向收紧，小窗下更紧凑
            .py(px(20.))
            .px(px(32.) + crate::chrome::Chrome::margin(window, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    // [wisp] 卡片区与分栏、分栏之间统一 20px 的纵向节奏
                    .gap_5()
                    .child(ui::held("home-wisp-cards", cards))
                    .children(shelves),
            )
            .when_some(context_menu, |this, menu| this.child(menu))
            .into_any_element()
    }
}

fn open_item_menu(
    home: &WeakEntity<HomeView>,
    items: &Rc<Vec<GenreItem>>,
    place: usize,
    event: &MouseDownEvent,
    cx: &mut gpui::App,
) {
    let Some(item) = items.get(place).and_then(Item::of) else {
        return;
    };
    let Some(home) = home.upgrade() else {
        return;
    };
    home.update(cx, |this, cx| {
        this.menus.reset(cx);
        this.context_menu = Some((item, event.position));
        cx.notify();
    });
}
