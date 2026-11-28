//! 私人漫游：Wisp 的沉浸式 FM 页——封面居中、两侧露相邻两张，下面是播放/不喜欢/换一批。
//!
//! 播放真源始终是全局队列：页面只是它的视图。与 Wisp 原版一致，FM 是无限流——
//! 一批曲子入队从第一首播，队尾耗尽时电台续歌（`Playback::radio`）自动补下一批，
//! 「不喜欢」作用于当前曲并切下一首。

use std::collections::HashSet;
use std::rc::Rc;

use crate::chrome::Chrome;
use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Context, Entity, Render, SharedString, Task,
    Window, div, img, linear_color_stop, linear_gradient, px, svg,
};
use i18n::t;
use music::Track;
use state::{Home, Io, Playback, Queue, QueuePosition, Session, Wisp};
use ui::{ActiveTheme as _, Artwork, Button, Scrollbar, Scroller, SpinningArtwork, growing, mix};

/// 每日推荐还没有曲子时的空态：刷新入口就在这一页上。
pub(crate) struct DailyEmpty {
    home: Entity<Home>,
}

impl DailyEmpty {
    pub(crate) fn new(home: Entity<Home>, cx: &mut Context<Self>) -> Self {
        cx.observe(&home, |_, _, cx| cx.notify()).detach();
        Self { home }
    }
}

impl Render for DailyEmpty {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let loading = self.home.read(cx).is_loading(cx);

        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(16.))
            .size_full()
            .child(
                div()
                    .size(px(72.))
                    .rounded(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme.secondary)
                    .child(img(icons::path("icons/disc-3.svg")).size(px(30.))),
            )
            .child(
                div()
                    .text_size(px(18.))
                    .text_color(theme.foreground)
                    .child(match loading {
                        true => t!("daily-loading"),
                        false => t!("daily-empty"),
                    }),
            )
            .when(!loading, |this| {
                this.child(
                    Button::new("daily-refresh")
                        .outline()
                        .icon("icons/refresh-cw.svg")
                        .label(t!("daily-refresh"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.home.update(cx, |home, cx| home.refresh(cx));
                        })),
                )
            })
            .into_any_element()
    }
}

const PAGE_PAD: f32 = 24.;
/// The FM header's expanded content height and the compact height it shrinks to while the
/// page scrolls. The content starts `HEADER_TOP` below it.
const HEADER_H: f32 = 144.;
const HEADER_TOP: f32 = 8.;
const HEADER_COLLAPSED: f32 = 56.;
const HEADER_SCROLL: f32 = HEADER_H + HEADER_TOP - HEADER_COLLAPSED;
const CARD: f32 = 248.;
/// How much the dislike and play controls grow under the pointer.
const CONTROL_GROW: f32 = 1.08;
/// The FM mode strip fills the card's inner width. Its four slots are fixed so the active
/// pill can slide between them.
const MODE_TRACK_W: f32 = CARD - 32.;
const MODE_PAD: f32 = 3.;
const MODE_GAP: f32 = 2.;
const MODE_SEG_W: f32 = (MODE_TRACK_W - 2. * MODE_PAD - 3. * MODE_GAP) / 4.;
/// The song-pool strip keeps a fixed width so its pill has the same slide.
const POOL_TRACK_W: f32 = 168.;
const POOL_PAD: f32 = 3.;
const POOL_GAP: f32 = 2.;
const POOL_SEG_W: f32 = (POOL_TRACK_W - 2. * POOL_PAD - 2. * POOL_GAP) / 3.;
/// The playing cover is a step larger than the ones stacked behind it.
const CURRENT_COVER: f32 = 232.;
const SIDE_COVER: f32 = 204.;
/// How far each cover behind the playing one is set back in the stack, to the right.
const STACK_STEP: f32 = 30.;
/// How many covers stand on the stack: the playing one and the twenty-nine queued behind
/// it. The stack keeps them all whatever the window width, so the same pile rolls on every
/// screen.
const COVERS: i32 = 30;
const CURRENT_LEFT: f32 = CARD - 88.;
/// Where the stack starts in the hero, and the room it takes.
const STACK_TOP: f32 = 8.;
const STACK_WIDTH: f32 = CURRENT_COVER + (COVERS as f32 - 1.) * STACK_STEP;
const STACK_HEIGHT: f32 = CURRENT_COVER;
const HERO_H: f32 = STACK_TOP + STACK_HEIGHT + STACK_TOP;
const NOW_COVER: f32 = 180.;

fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

#[derive(Clone)]
struct Side {
    track: Track,
    uid: Option<String>,
    pick: Option<usize>,
}

fn cover_id(track: &Track) -> String {
    track.id.clone().unwrap_or_else(|| track.name.clone())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CoverSpin {
    Still,
    Playing,
    Exit,
}

const ROLL_OUT_MS: u64 = 560;
const ROLL_SHIFT_MS: u64 = 900;
const ROLL_ENTER_DELAY_MS: u64 = 560;
const ROLL_ENTER_MS: u64 = 900;
const ROLL_TOTAL_MS: u64 = 1_500;
/// How long the playing cover takes for one whole turn.
const PLAYING_TURN_MS: u64 = 6_000;

/// How large the cover in `slot` is drawn: the playing one is the top of the stack, larger
/// than the covers set back behind it.
fn slot_size(slot: i32) -> f32 {
    match slot {
        0 => CURRENT_COVER,
        _ => SIDE_COVER,
    }
}

/// How far the cover in `slot` sits from the playing cover's corner, set back into the
/// stack to the right and down. A slot before the playing one has rolled off the stack.
fn slot_left(slot: i32) -> f32 {
    match slot {
        slot if slot < 0 => -CURRENT_COVER,
        0 => 0.,
        slot => (CURRENT_COVER - SIDE_COVER) + slot as f32 * STACK_STEP,
    }
}

fn slot_top(slot: i32) -> f32 {
    match slot {
        0 => 0.,
        _ => (CURRENT_COVER - SIDE_COVER) / 2.,
    }
}

pub(crate) struct RadioHeader {
    view: Entity<RadioView>,
}

impl RadioHeader {
    pub(crate) fn new(view: Entity<RadioView>, cx: &mut Context<Self>) -> Self {
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        Self { view }
    }
}

impl Render for RadioHeader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = Chrome::page(window, cx);
        self.view.update(cx, |view, cx| {
            let scrolled = view.scrollbar.read(cx).offset().max(px(0.));
            let progress = (scrolled.as_f32() / HEADER_SCROLL).clamp(0., 1.);
            view.sliver_header(progress, page, window, cx)
        })
    }
}

/// 私人漫游：页面跟着播放走——中央是正在播的（没在播时是预览的第一首），
/// 右侧是候选或队列里的下一批。
pub(crate) struct RadioView {
    session: Entity<Session>,
    playback: Entity<Playback>,
    queue: Entity<Queue>,
    io: Io,
    /// 还没开始漫游时预览的候选批次；开始后队列接管，它只用来「开始漫游」。
    picks: Rc<Vec<Track>>,
    /// The provider token that continues this FM station after the batch is played.
    next: Option<String>,
    task: Option<Task<()>>,
    scrollbar: Entity<Scrollbar>,
    /// 正在取一批，取到之前显示挑选中。
    loading: bool,
    /// The listener pressed play before a batch was ready; the fetch starts it on arrival.
    want_play: bool,
    /// A start was asked for and the queue has not begun playing yet.
    starting: bool,
    /// The carousel at rest: current cover first, then the side covers in play order.
    covers: Vec<Side>,
    /// The strip the current roll started from; empty when the carousel is still.
    covers_from: Vec<Side>,
    /// The cover entering at the right end of that roll.
    covers_extra: Option<Side>,
    cover_key: Option<String>,
    cover_seq: u64,
    cover_task: Option<Task<()>>,
    /// The provider the page's preview and FM session belong to.
    provider: Option<String>,
    fm_provider: Option<String>,
    fm_session_seen: bool,
    /// The last selected FM mode and song pool, kept so the active pill can slide from there.
    mode_slide_from: usize,
    mode_slide_seq: u64,
    pool_slide_from: usize,
    pool_slide_seq: u64,
}

impl RadioView {
    pub(crate) fn new(
        session: Entity<Session>,
        playback: Entity<Playback>,
        queue: Entity<Queue>,
        io: Io,
        cx: &mut Context<Self>,
    ) -> Self {
        let me = cx.entity_id();
        let provider = session.read(cx).provider_slug().map(str::to_owned);
        let fm_session_seen = playback.read(cx).fm_session();
        let fm_provider = fm_session_seen.then(|| provider.clone()).flatten();
        cx.observe(&playback, |this, playback, cx| {
            let playback = playback.read(cx);
            let fm = playback.fm_session();
            if this.starting && fm && playback.track().is_some() {
                this.starting = false;
                this.want_play = false;
            }
            if fm != this.fm_session_seen {
                this.fm_session_seen = fm;
                this.fm_provider = fm.then(|| this.provider.clone()).flatten();
            }
            if !fm {
                this.fm_provider = None;
            }
            cx.notify();
        })
        .detach();
        cx.observe(&queue, |_, _, cx| cx.notify()).detach();
        let watched = session.clone();
        cx.observe(&watched, |this, session, cx| {
            let provider = session.read(cx).provider_slug().map(str::to_owned);
            if provider != this.provider {
                this.provider = provider;
                this.reset_for_provider(cx);
            } else if !this.loading
                && this.picks.is_empty()
                && !this.roaming(cx)
                && session.read(cx).client().is_some()
            {
                this.fetch(false, cx);
            }
            cx.notify();
        })
        .detach();
        let mut this = Self {
            session,
            playback,
            queue,
            io,
            picks: Rc::new(Vec::new()),
            next: None,
            task: None,
            scrollbar: cx.new(|_| Scrollbar::new(gpui::ScrollHandle::new()).watching(me)),
            loading: false,
            want_play: false,
            starting: false,
            covers: Vec::new(),
            covers_from: Vec::new(),
            covers_extra: None,
            cover_key: None,
            cover_seq: 0,
            cover_task: None,
            provider,
            fm_provider,
            fm_session_seen,
            mode_slide_from: 0,
            mode_slide_seq: 0,
            pool_slide_from: 0,
            pool_slide_seq: 0,
        };
        let scrollbar = this.scrollbar.clone();
        cx.observe(&scrollbar, |_, _, cx| cx.notify()).detach();
        this.fetch(false, cx);
        this
    }

    /// Switches the page to another provider: the old preview and carousel belong to the
    /// previous one, so they are dropped before the new batch is requested.
    fn reset_for_provider(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.loading = false;
        self.want_play = false;
        self.starting = false;
        self.picks = Rc::new(Vec::new());
        self.next = None;
        self.covers.clear();
        self.covers_from.clear();
        self.covers_extra = None;
        self.cover_key = None;
        self.cover_seq = self.cover_seq.wrapping_add(1);
        self.cover_task = None;
        self.fm_provider = None;
        self.fetch(false, cx);
    }

    /// 是否已经在漫游：只有 FM 会话里的歌才算，普通歌单正在播放不算。
    fn roaming(&self, cx: &App) -> bool {
        let playback = self.playback.read(cx);
        playback.fm_session()
            && playback.track().is_some()
            && self.fm_provider.is_some()
            && self.fm_provider.as_deref() == self.session.read(cx).provider_slug()
    }

    fn playing(&self, cx: &App) -> bool {
        // The page owns one control: it shows FM's transport, never another queue's.
        self.roaming(cx) && self.playback.read(cx).control().unwrap_or(false)
    }

    fn modes_available(&self, cx: &App) -> bool {
        self.session.read(cx).capabilities().radio_modes
    }

    fn mode_title(&self, mode: &str) -> SharedString {
        match mode {
            "small" => t!("radio-mode-title-small"),
            "peak" => t!("radio-mode-title-peak"),
            "radio" => t!("radio-mode-title-radio"),
            _ => t!("radio-mode-title-normal"),
        }
    }

    fn card_title(&self, cx: &App) -> SharedString {
        match self.modes_available(cx) {
            true => self.mode_title(Wisp::global(cx).settings.read(cx).fm_mode()),
            false => t!("radio-card-title"),
        }
    }

    fn set_mode(&mut self, mode: &'static str, cx: &mut Context<Self>) {
        let settings = Wisp::global(cx).settings.clone();
        let current = settings.read(cx).fm_mode().to_owned();
        if current == mode {
            return;
        }
        self.mode_slide_from = match current.as_str() {
            "small" => 1,
            "peak" => 2,
            "radio" => 3,
            _ => 0,
        };
        self.mode_slide_seq = self.mode_slide_seq.wrapping_add(1);
        settings.update(cx, |settings, cx| settings.set_fm_mode(mode, cx));
        self.fetch(true, cx);
        cx.notify();
    }

    fn set_pool(&mut self, pool: u32, cx: &mut Context<Self>) {
        let settings = Wisp::global(cx).settings.clone();
        if settings.read(cx).fm_song_pool() == pool {
            return;
        }
        self.pool_slide_from = settings.read(cx).fm_song_pool().min(2) as usize;
        self.pool_slide_seq = self.pool_slide_seq.wrapping_add(1);
        settings.update(cx, |settings, cx| settings.set_fm_song_pool(pool, cx));
        self.fetch(true, cx);
        cx.notify();
    }

    fn mode_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let chosen = Wisp::global(cx).settings.read(cx).fm_mode().to_owned();
        let ink: gpui::Hsla = gpui::rgb(0x0b1620).into();
        let options = [
            ("normal", t!("radio-mode-normal")),
            ("small", t!("radio-mode-small")),
            ("peak", t!("radio-mode-peak")),
            ("radio", t!("radio-mode-radio")),
        ];
        let to = options
            .iter()
            .position(|(mode, _)| *mode == chosen)
            .unwrap_or(0);
        let from = match self.mode_slide_seq {
            0 => to,
            _ => self.mode_slide_from.min(options.len() - 1),
        };
        let from_left = MODE_PAD + from as f32 * (MODE_SEG_W + MODE_GAP);
        let to_left = MODE_PAD + to as f32 * (MODE_SEG_W + MODE_GAP);
        let bar = div()
            .absolute()
            .top(px(MODE_PAD))
            .left(px(from_left))
            .w(px(MODE_SEG_W))
            .h(px(24.))
            .rounded(px(6.))
            .bg(gpui::white().opacity(0.92));
        let indicator = if ui::motion::animates(cx) && from != to {
            bar.with_animation(
                SharedString::from(format!("fm-mode-slide-{}", self.mode_slide_seq)),
                ui::motion::Motion::Control.animation(),
                move |bar, t| bar.left(px(from_left + (to_left - from_left) * t)),
            )
            .into_any_element()
        } else {
            bar.left(px(to_left)).into_any_element()
        };
        div()
            .relative()
            .flex()
            .w(px(MODE_TRACK_W))
            .gap(px(MODE_GAP))
            .p(px(MODE_PAD))
            .rounded(px(8.))
            .bg(gpui::white().opacity(0.14))
            .border_1()
            .border_color(gpui::white().opacity(0.08))
            .child(indicator)
            .children(
                options
                    .into_iter()
                    .enumerate()
                    .map(|(index, (mode, label))| {
                        let selected = chosen == mode;
                        let aria = label.clone();
                        div()
                            .id(SharedString::from(format!("fm-mode-{index}")))
                            .role(Role::Button)
                            .aria_label(aria)
                            .when(selected, |this| this.aria_selected(true))
                            .flex_none()
                            .w(px(MODE_SEG_W))
                            .h(px(24.))
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .when(selected, |this| this.text_color(ink))
                            .when(!selected, |this| {
                                this.text_color(gpui::white().opacity(0.72))
                                    .hover(|style| style.bg(gpui::white().opacity(0.1)))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| this.set_mode(mode, cx)))
                            .child(label)
                    }),
            )
            .into_any_element()
    }

    fn pool_tabs(&self, cx: &mut Context<Self>, prefix: &'static str) -> AnyElement {
        let theme = *cx.theme();
        let chosen = Wisp::global(cx).settings.read(cx).fm_song_pool();
        let options = [
            (0, t!("radio-pool-taste")),
            (1, t!("radio-pool-style")),
            (2, t!("radio-pool-explore")),
        ];
        let to = chosen.min(2) as usize;
        let from = match self.pool_slide_seq {
            0 => to,
            _ => self.pool_slide_from.min(options.len() - 1),
        };
        let from_left = POOL_PAD + from as f32 * (POOL_SEG_W + POOL_GAP);
        let to_left = POOL_PAD + to as f32 * (POOL_SEG_W + POOL_GAP);
        let bar = div()
            .absolute()
            .top(px(POOL_PAD))
            .left(px(from_left))
            .w(px(POOL_SEG_W))
            .h(px(24.))
            .rounded(px(6.))
            .bg(theme.primary);
        let indicator = if ui::motion::animates(cx) && from != to {
            bar.with_animation(
                SharedString::from(format!("fm-pool-slide-{}", self.pool_slide_seq)),
                ui::motion::Motion::Control.animation(),
                move |bar, t| bar.left(px(from_left + (to_left - from_left) * t)),
            )
            .into_any_element()
        } else {
            bar.left(px(to_left)).into_any_element()
        };
        div()
            .relative()
            .flex()
            .flex_none()
            .w(px(POOL_TRACK_W))
            .gap(px(POOL_GAP))
            .p(px(POOL_PAD))
            .rounded(px(8.))
            .bg(theme.secondary)
            .border_1()
            .border_color(theme.border.opacity(0.5))
            .child(indicator)
            .children(
                options
                    .into_iter()
                    .enumerate()
                    .map(|(index, (pool, label))| {
                        let selected = chosen == pool;
                        let aria = label.clone();
                        div()
                            .id(SharedString::from(format!("{prefix}-{index}")))
                            .role(Role::Button)
                            .aria_label(aria)
                            .when(selected, |this| this.aria_selected(true))
                            .flex_none()
                            .w(px(POOL_SEG_W))
                            .h(px(24.))
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .when(selected, |this| this.text_color(theme.primary_foreground))
                            .when(!selected, |this| {
                                this.text_color(theme.muted_foreground)
                                    .hover(|style| style.bg(theme.secondary_hover))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| this.set_pool(pool, cx)))
                            .child(label)
                    }),
            )
            .into_any_element()
    }

    /// The disc and the now-playing card show the FM track, or the first preview before FM starts.
    fn current(&self, cx: &App) -> Option<Track> {
        let playback = self.playback.read(cx);
        match self.roaming(cx) {
            true => playback.shown_track(cx),
            false => self.picks.first().cloned(),
        }
    }

    /// Keeps the previous strip so the whole row can roll one slot to the left.
    fn sync_cover(&mut self, cx: &mut Context<Self>) {
        let mut target = Vec::new();
        if let Some(track) = self.current(cx) {
            target.push(Side {
                track,
                uid: None,
                pick: None,
            });
        }
        target.extend(self.side_tracks(cx));
        target.truncate(COVERS as usize);
        let key = target
            .iter()
            .map(|side| cover_id(&side.track))
            .collect::<Vec<_>>()
            .join("|");
        if self.cover_key.as_deref() == Some(key.as_str()) {
            return;
        }

        let previous = std::mem::take(&mut self.covers);
        let head_changed = previous.first().map(|side| cover_id(&side.track))
            != target.first().map(|side| cover_id(&side.track));
        // A change behind the head never cuts a roll short: the stack carries the roll
        // through and takes the new queue when it ends.
        if !head_changed && !self.covers_from.is_empty() {
            self.covers = target;
            self.cover_key = Some(key);
            return;
        }
        if !previous.is_empty() && head_changed && previous.len() > 1 && target.len() > 1 {
            self.covers_extra = target.last().cloned();
            self.covers_from = previous;
            self.cover_seq = self.cover_seq.wrapping_add(1);
            let seq = self.cover_seq;
            self.cover_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(ROLL_TOTAL_MS + 30))
                    .await;
                this.update(cx, |this, cx| {
                    if this.cover_seq == seq {
                        this.covers_from.clear();
                        this.covers_extra = None;
                        cx.notify();
                    }
                })
                .ok();
            }));
        } else {
            self.cover_task = None;
            self.covers_from.clear();
            self.covers_extra = None;
        }
        self.covers = target;
        self.cover_key = Some(key);
    }

    fn fetch(&mut self, restart: bool, cx: &mut Context<Self>) {
        let Some(client) = self.session.read(cx).client() else {
            return;
        };
        let mode = Wisp::global(cx).settings.read(cx).fm_mode().to_owned();
        let pool = Wisp::global(cx).settings.read(cx).fm_song_pool();
        let io = self.io.clone();
        self.loading = true;
        self.task = Some(cx.spawn(async move |this, cx| {
            let request_mode = mode.clone();
            let found = io
                .spawn(async move {
                    client
                        .track_radio_with("", None, Some(&request_mode), Some(pool))
                        .await
                })
                .await;
            this.update(cx, |this, cx| {
                this.task = None;
                this.loading = false;
                match found {
                    Ok(Ok((tracks, next))) => {
                        let play = this.want_play && !tracks.is_empty();
                        this.picks = Rc::new(tracks);
                        this.next = next;
                        if play {
                            this.want_play = false;
                        } else if this.want_play {
                            this.want_play = false;
                            this.starting = false;
                        }
                        if (restart && this.roaming(cx)) || play {
                            this.start_from(0, cx);
                        }
                    }
                    Ok(Err(error)) => {
                        this.starting = false;
                        this.want_play = false;
                        log::warn!("radio: cannot fetch: {error:#}");
                    }
                    Err(_) => {
                        this.starting = false;
                        this.want_play = false;
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn side_tracks(&self, cx: &App) -> Vec<Side> {
        if self.roaming(cx) {
            let queue = self.queue.read(cx);
            // Only the covers on the stack are cloned, not the whole upcoming queue. A song
            // the stack already shows is skipped, so a recommendation that repeats never
            // puts the same cover up twice, and only playable tracks come up at all.
            let mut seen = HashSet::new();
            if let Some(current) = self.current(cx) {
                seen.insert(cover_id(&current));
            }
            return queue
                .upcoming()
                .enumerate()
                .filter(|(_, track)| track.playable && seen.insert(cover_id(track)))
                .take(COVERS as usize - 1)
                .map(|(index, track)| Side {
                    track: track.clone(),
                    uid: queue.uid(QueuePosition::Upcoming(index)).map(str::to_owned),
                    pick: None,
                })
                .collect();
        }
        let mut seen = HashSet::new();
        if let Some(current) = self.current(cx) {
            seen.insert(cover_id(&current));
        }
        self.picks
            .iter()
            .enumerate()
            .filter(|(_, track)| seen.insert(cover_id(track)))
            .take(COVERS as usize - 1)
            .map(|(index, track)| Side {
                track: track.clone(),
                uid: None,
                pick: Some(index),
            })
            .collect()
    }

    fn start_from(&mut self, index: usize, cx: &mut Context<Self>) {
        let tracks = (*self.picks).clone();
        if tracks.is_empty() {
            return;
        }
        let next = self.next.clone();
        let mode = Wisp::global(cx).settings.read(cx).fm_mode().to_owned();
        let pool = Wisp::global(cx).settings.read(cx).fm_song_pool();
        self.fm_provider = self.session.read(cx).provider_slug().map(str::to_owned);
        self.playback.update(cx, |playback, cx| {
            playback.start_fm(tracks, index, next, mode, pool, cx)
        });
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.picks.is_empty() {
            self.want_play = true;
            self.starting = true;
            if !self.loading {
                self.fetch(false, cx);
            }
            cx.notify();
            return;
        }
        self.want_play = false;
        self.starting = true;
        self.start_from(0, cx);
        cx.notify();
    }

    fn play_side(&mut self, pick: Option<usize>, uid: Option<String>, cx: &mut Context<Self>) {
        match uid {
            Some(uid) => {
                self.playback
                    .update(cx, |playback, cx| playback.play_queue(&uid, cx));
            }
            None => {
                if let Some(index) = pick {
                    self.start_from(index, cx);
                }
            }
        }
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        match self.roaming(cx) {
            true => self
                .playback
                .update(cx, |playback, cx| playback.toggle_play(cx)),
            false => self.start(cx),
        }
    }

    /// 「不喜欢」：在 FM 里对当前曲上报并切下一首；没开始时把预览的队首先去掉。
    fn dislike(&mut self, cx: &mut Context<Self>) {
        if self.roaming(cx) {
            if let Some(id) = self.current(cx).and_then(|track| track.id) {
                self.playback.update(cx, |playback, cx| playback.next(cx));
                self.send_dislike(id, cx);
            }
            return;
        }
        let disliked = self.picks.first().and_then(|track| track.id.clone());
        let rest: Vec<Track> = self.picks.iter().skip(1).cloned().collect();
        self.picks = Rc::new(rest);
        if let Some(id) = disliked {
            self.send_dislike(id, cx);
        }
        if self.picks.is_empty() {
            self.fetch(false, cx);
        }
        cx.notify();
    }

    /// 让服务端少推这类歌；失败只记日志，不影响本地的换歌。
    fn send_dislike(&self, id: String, cx: &mut Context<Self>) {
        let Some(client) = self.session.read(cx).client() else {
            return;
        };
        let io = self.io.clone();
        let reported = id.clone();
        cx.spawn(async move |_, _| {
            let sent = io.spawn(async move { client.dislike(&id).await }).await;
            match sent {
                Ok(Ok(())) => {}
                Ok(Err(error)) => log::warn!("radio: cannot dislike {reported}: {error:#}"),
                Err(error) => log::warn!("radio: the dislike task ended: {error}"),
            }
        })
        .detach();
    }

    fn placeholder(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(16.))
            .min_h(px(420.))
            .child(
                div()
                    .size(px(72.))
                    .rounded(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme.primary.opacity(0.1))
                    .child(
                        svg()
                            .path(icons::path("icons/radio.svg"))
                            .size(px(30.))
                            .text_color(theme.primary),
                    ),
            )
            .child(
                div()
                    .text_size(px(22.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.foreground)
                    .child(match self.loading {
                        true => t!("radio-loading"),
                        false => t!("radio-empty"),
                    }),
            )
            .when(!self.loading, |this| {
                this.child(
                    Button::new("radio-retry")
                        .outline()
                        .label(t!("radio-refresh"))
                        .icon("icons/refresh-cw.svg")
                        .on_click(cx.listener(|this, _, _, cx| this.fetch(false, cx))),
                )
            })
            .into_any_element()
    }

    fn header(&self, progress: f32, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let current = self.current(cx);
        let ink = gpui::rgb(0x0b1620).into();
        let top = mix(ink, theme.primary, 0.34);
        let bottom = mix(ink, theme.primary, 0.08);
        let icon = lerp(120., 32., progress);
        let corner = lerp(16., 8., progress);
        let glyph = lerp(42., 18., progress);
        let title = lerp(38., 16., progress);
        let gap = lerp(24., 12., progress);
        let details = (1. - progress * 2.2).clamp(0., 1.);
        div()
            .h_full()
            .flex()
            .items_center()
            .gap(px(gap))
            .child(
                div()
                    .size(px(icon))
                    .flex_none()
                    .rounded(px(corner))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(linear_gradient(
                        160.,
                        linear_color_stop(top, 0.),
                        linear_color_stop(bottom, 1.),
                    ))
                    .border_1()
                    .border_color(gpui::white().opacity(0.1))
                    .child(
                        svg()
                            .path(icons::path("icons/radio.svg"))
                            .size(px(glyph))
                            .text_color(gpui::white()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6. * (1. - progress)))
                    .child(
                        div()
                            .text_size(px(title))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(theme.foreground)
                            .truncate()
                            .child(t!("nav-radio")),
                    )
                    .when(details > 0.01, |this| {
                        this.child(
                            div()
                                .text_size(px(13.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.muted_foreground)
                                .opacity(details)
                                .truncate()
                                .child(t!("radio-tagline")),
                        )
                        .when_some(current, |this, track| {
                            this.child(
                                div()
                                    .text_size(px(11.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.muted_foreground)
                                    .opacity(details)
                                    .truncate()
                                    .child(SharedString::from(format!(
                                        "{} · {}",
                                        track.name, track.artists
                                    ))),
                            )
                        })
                    }),
            )
            .when(self.modes_available(cx), |this| {
                this.child(self.pool_tabs(cx, "fm-pool"))
            })
            .into_any_element()
    }

    /// A sticky header whose height and contents follow the page scroll. The header itself
    /// shrinks from its expanded layout into the 56px compact row; nothing is cross-faded
    /// behind it, so no second title haunts the collapsed bar.
    fn sliver_header(
        &self,
        progress: f32,
        page: gpui::Pixels,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.theme();
        let frosted = theme.transparent && ui::blurring(cx);
        let height = HEADER_H + HEADER_TOP - (HEADER_H + HEADER_TOP - HEADER_COLLAPSED) * progress;
        // A page as wide as the content area starts at the sheet's rounded top corners, which a
        // square header background would cover.
        let flush = page >= Chrome::content(window, cx);
        let corners = Chrome::sheet_corners(cx);
        let header = self.header(progress, cx);
        div()
            .relative()
            .w_full()
            .h(px(height))
            .flex()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w(page)
                    .h_full()
                    .overflow_hidden()
                    .bg(theme.card(frosted))
                    .when(flush, |this| {
                        this.rounded_tl(corners.top_left)
                            .rounded_tr(corners.top_right)
                    })
                    .when(frosted, |this| this.backdrop_blur(ui::GLASS_BLUR))
                    .border_b_1()
                    .border_color(theme.border.opacity(0.5))
                    .child(
                        div()
                            .w_full()
                            .px(px(PAGE_PAD))
                            .h_full()
                            .pt(px(HEADER_TOP * (1. - progress)))
                            .child(header),
                    ),
            )
            .into_any_element()
    }

    fn card(
        &self,
        current: Option<&Track>,
        playing: bool,
        starting: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.theme();
        let ink: gpui::Hsla = gpui::rgb(0x0b1620).into();
        let top = mix(ink, theme.primary, 0.34);
        let subtitle = current
            .map(|track| format!("{} · {}", track.name, track.artists))
            .unwrap_or_else(|| t!("radio-tagline").to_string());
        let title = self.card_title(cx);
        let mode_tabs = self.modes_available(cx).then(|| self.mode_tabs(cx));
        let bars: Vec<AnyElement> = [11., 17., 8., 19., 13., 18., 9., 14., 8., 15.]
            .into_iter()
            .map(|height| {
                div()
                    .w(px(3.))
                    .h(px(height))
                    .rounded(px(2.))
                    .bg(gpui::white().opacity(0.4))
                    .into_any_element()
            })
            .collect();
        let icon = match (starting, playing) {
            (true, _) => "icons/loader-circle.svg",
            (false, true) => "icons/pause-filled.svg",
            (false, false) => "icons/play-filled.svg",
        };
        div()
            .occlude()
            .w(px(CARD))
            .h(px(CARD))
            .flex_none()
            .flex()
            .flex_col()
            .justify_between()
            .p(px(16.))
            .rounded(px(14.))
            .bg(linear_gradient(
                160.,
                linear_color_stop(top, 0.),
                linear_color_stop(ink, 1.),
            ))
            .border_1()
            .border_color(gpui::white().opacity(0.08))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(match mode_tabs {
                        Some(tabs) => tabs,
                        None => div()
                            .flex()
                            .child(
                                div()
                                    .px(px(8.))
                                    .h(px(22.))
                                    .rounded(px(6.))
                                    .flex()
                                    .items_center()
                                    .bg(gpui::white().opacity(0.14))
                                    .text_size(px(10.))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(gpui::white())
                                    .child("FM"),
                            )
                            .into_any_element(),
                    })
                    .child(
                        div()
                            .mt(px(2.))
                            .text_size(px(28.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(gpui::white())
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(gpui::white().opacity(0.8))
                            .truncate()
                            .child(SharedString::from(subtitle)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .child(div().flex_1().flex().items_end().gap(px(6.)).children(bars))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(growing(
                                div()
                                    .id("radio-dislike")
                                    .role(Role::Button)
                                    .aria_label(t!("radio-dislike"))
                                    .size(px(28.))
                                    .rounded(px(8.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(gpui::white().opacity(0.12)))
                                    .on_click(cx.listener(|this, _, _, cx| this.dislike(cx)))
                                    .child(
                                        svg()
                                            .path(icons::path("icons/heart-off.svg"))
                                            .size(px(22.))
                                            .text_color(gpui::white().opacity(0.88)),
                                    ),
                                &gpui::ElementId::from("radio-dislike"),
                                CONTROL_GROW,
                                window,
                                cx,
                            ))
                            .child(growing(
                                div()
                                    .id("radio-play")
                                    .role(Role::Button)
                                    .aria_label(match playing {
                                        true => t!("play-pause"),
                                        false => t!("radio-play"),
                                    })
                                    .size(px(52.))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .bg(theme.primary)
                                    .border_1()
                                    .border_color(gpui::white().opacity(0.18))
                                    .hover(|style| style.bg(theme.primary_hover))
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)))
                                    .child(
                                        svg()
                                            .path(icons::path(icon))
                                            .size(px(18.))
                                            .text_color(theme.primary_foreground),
                                    ),
                                &gpui::ElementId::from("radio-play"),
                                CONTROL_GROW,
                                window,
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    fn disc(&self, track: &Track, size: f32, spin: CoverSpin) -> gpui::Div {
        let ring: gpui::Hsla = gpui::rgb(0x1c252d).into();
        let dark: gpui::Hsla = gpui::rgb(0x070c10).into();
        let plate = match track.cover.clone() {
            Some(url) => match spin {
                CoverSpin::Playing => spin_art(
                    url,
                    size,
                    SharedString::from(format!("fm-spin-playing-{}", self.cover_seq)),
                    Animation::new(std::time::Duration::from_millis(PLAYING_TURN_MS)).repeat(),
                    |art, delta| art.turn(delta),
                ),
                CoverSpin::Exit => spin_art(
                    url,
                    size,
                    SharedString::from(format!("fm-spin-exit-{}", self.cover_seq)),
                    Animation::new(std::time::Duration::from_millis(ROLL_OUT_MS))
                        .with_easing(ui::motion::ease_out_cubic),
                    |art, delta| art.eager().turn(delta),
                ),
                CoverSpin::Still => Artwork::new(Some(url))
                    .size(px(size - 12.))
                    .circle()
                    .into_any_element(),
            },
            None => div()
                .size_full()
                .bg(gpui::rgb(0x151d24))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(icons::path("icons/disc-3.svg"))
                        .size(px(size * 0.24))
                        .text_color(gpui::white().opacity(0.36)),
                )
                .into_any_element(),
        };
        let core = div()
            .size_full()
            .rounded_full()
            .overflow_hidden()
            .child(plate);
        div()
            .size(px(size))
            .flex_none()
            .p(px(6.))
            .rounded_full()
            .bg(linear_gradient(
                135.,
                linear_color_stop(ring, 0.),
                linear_color_stop(dark, 1.),
            ))
            .border_1()
            .border_color(gpui::white().opacity(0.18))
            .child(core)
    }

    fn hero(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let current = self.current(cx);
        let playing = self.playing(cx);
        let animates = ui::motion::animates(cx);
        let spinning = playing && animates;
        let card = self.card(current.as_ref(), playing, self.starting, window, cx);

        // The covers sit in one stack: the playing cover on top, the covers queued behind it
        // set back a step each. On a skip the playing cover rolls off the pile, the stack
        // moves up one place, and the incoming cover joins at the back.
        let rolling = !self.covers_from.is_empty();
        let rolling_spin = rolling && animates;
        let from_len = self.covers_from.len();
        let mut lane_items = match rolling {
            true => {
                let mut items = self.covers_from.clone();
                if let Some(extra) = self.covers_extra.clone() {
                    items.push(extra);
                }
                items
            }
            false => self.covers.clone(),
        };
        let mut strip = div()
            .absolute()
            .left(px(CURRENT_LEFT))
            .top(px(STACK_TOP))
            .w(px(STACK_WIDTH))
            .h(px(STACK_HEIGHT))
            .overflow_hidden();
        // Deepest cover first, so the playing one paints over the stack and the cover
        // rolling off paints over everything.
        for (old_slot, side) in lane_items.drain(..).enumerate().rev() {
            let new_slot = match rolling {
                true => old_slot as i32 - 1,
                false => old_slot as i32,
            };
            let target = usize::try_from(new_slot)
                .ok()
                .and_then(|at| self.covers.get(at));
            let current = new_slot == 0 && target.is_some();
            let outgoing = rolling && old_slot == 0;
            let entering = rolling && old_slot >= from_len;
            let size = match outgoing {
                true => CURRENT_COVER,
                false => slot_size(new_slot),
            };
            let spin = match (rolling_spin, outgoing, current && spinning) {
                (true, true, _) => CoverSpin::Exit,
                (_, _, true) => CoverSpin::Playing,
                _ => CoverSpin::Still,
            };
            let inner = self.disc(&side.track, size, spin);
            let mut slot = div()
                .id(SharedString::from(format!("radio-cover-{old_slot}")))
                .absolute()
                .child(inner);
            if let Some(target) = target {
                let pick = target.pick;
                let uid = target.uid.clone();
                let label: SharedString = match current {
                    true => match playing {
                        true => t!("play-pause"),
                        false => t!("radio-play"),
                    },
                    false => SharedString::from(format!(
                        "{} · {}",
                        target.track.name, target.track.artists
                    )),
                };
                slot = slot
                    .role(Role::Button)
                    .aria_label(label)
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.92))
                    .on_click(cx.listener(move |this, _, _, cx| match current {
                        true => this.toggle(cx),
                        false => this.play_side(pick, uid.clone(), cx),
                    }));
            }
            let element = match rolling {
                true if outgoing => {
                    let to_left = CURRENT_COVER * 1.3;
                    slot.with_animation(
                        SharedString::from(format!("fm-cover-exit-{}", self.cover_seq)),
                        Animation::new(std::time::Duration::from_millis(ROLL_OUT_MS))
                            .with_easing(ui::motion::ease_out_cubic),
                        move |slot, delta| {
                            slot.left(px(to_left * delta))
                                .top(px(0.))
                                .layer_scale(1. + 0.06 * delta)
                                .opacity((1. - delta).powi(2))
                        },
                    )
                    .into_any_element()
                }
                true if entering => {
                    let to_left = slot_left(new_slot);
                    let to_top = slot_top(new_slot);
                    let start_left = to_left + 96.;
                    slot.with_animation(
                        SharedString::from(format!("fm-cover-enter-{}-{old_slot}", self.cover_seq)),
                        Animation::new(std::time::Duration::from_millis(ROLL_TOTAL_MS)),
                        move |slot, delta| {
                            let start = ROLL_ENTER_DELAY_MS as f32 / ROLL_TOTAL_MS as f32;
                            let span = ROLL_ENTER_MS as f32 / ROLL_TOTAL_MS as f32;
                            let progress = ((delta - start) / span).clamp(0., 1.);
                            let eased = 1. - (1. - progress).powi(3);
                            slot.left(px(start_left + (to_left - start_left) * eased))
                                .top(px(to_top))
                                .layer_scale(0.92 + 0.08 * eased)
                                .opacity(eased)
                        },
                    )
                    .into_any_element()
                }
                true => {
                    let from_left = slot_left(old_slot as i32);
                    let to_left = slot_left(new_slot);
                    let from_top = slot_top(old_slot as i32);
                    let to_top = slot_top(new_slot);
                    let from_scale = slot_size(old_slot as i32) / size;
                    slot.with_animation(
                        SharedString::from(format!("fm-cover-roll-{}-{old_slot}", self.cover_seq)),
                        Animation::new(std::time::Duration::from_millis(ROLL_SHIFT_MS))
                            .with_easing(ui::motion::ease_out_cubic),
                        move |slot, delta| {
                            slot.left(px(from_left + (to_left - from_left) * delta))
                                .top(px(from_top + (to_top - from_top) * delta))
                                .layer_scale(from_scale + (1. - from_scale) * delta)
                        },
                    )
                    .into_any_element()
                }
                false => slot
                    .left(px(slot_left(new_slot)))
                    .top(px(slot_top(new_slot)))
                    .into_any_element(),
            };
            strip = strip.child(element);
        }
        div()
            .relative()
            .w_full()
            .h(px(HERO_H))
            .mt(px(16.))
            .child(strip)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px((HERO_H - CARD) / 2.))
                    .child(card),
            )
            .into_any_element()
    }

    fn now_panel(&self, current: Option<Track>, cx: &App) -> AnyElement {
        let theme = *cx.theme();
        let body = match current {
            Some(track) => {
                let cover = match track.cover.clone() {
                    Some(url) => Artwork::new(Some(url))
                        .size(px(NOW_COVER))
                        .corner_radius(px(8.))
                        .into_any_element(),
                    None => div()
                        .size(px(NOW_COVER))
                        .rounded(px(8.))
                        .bg(theme.secondary)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            svg()
                                .path(icons::path("icons/disc-3.svg"))
                                .size(px(48.))
                                .text_color(theme.muted_foreground),
                        )
                        .into_any_element(),
                };
                let seconds = track.duration.as_secs();
                let mut chips = Vec::new();
                if seconds > 0 {
                    chips.push(format!("{}:{:02}", seconds / 60, seconds % 60));
                }
                if let Some(language) = track.languages.first() {
                    chips.push(language.clone());
                }
                let chips: Vec<AnyElement> = chips
                    .into_iter()
                    .map(|chip| {
                        div()
                            .px(px(10.))
                            .h(px(30.))
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .bg(theme.secondary)
                            .text_size(px(11.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(chip))
                            .into_any_element()
                    })
                    .collect();
                div()
                    .flex()
                    .items_center()
                    .gap(px(28.))
                    .child(cover)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h(px(NOW_COVER))
                            .justify_center()
                            .gap(px(14.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(px(28.))
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(theme.foreground)
                                            .truncate()
                                            .child(SharedString::from(track.name.clone())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme.muted_foreground)
                                            .truncate()
                                            .child(SharedString::from(track.artists.clone())),
                                    ),
                            )
                            .when(!track.album.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme.muted_foreground)
                                        .child(SharedString::from(track.album.clone())),
                                )
                            })
                            .when(!chips.is_empty(), |this| {
                                this.child(div().flex().gap(px(10.)).children(chips))
                            }),
                    )
                    .into_any_element()
            }
            None => div()
                .min_h(px(NOW_COVER))
                .flex()
                .items_center()
                .text_size(px(13.))
                .text_color(theme.muted_foreground)
                .child(match self.loading {
                    true => t!("radio-loading"),
                    false => t!("radio-now-empty"),
                })
                .into_any_element(),
        };
        div()
            .w_full()
            .mt(px(28.))
            .p(px(22.))
            .rounded(px(14.))
            .border_1()
            .border_color(theme.border.opacity(0.55))
            .bg(theme.card(theme.blur))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(theme.foreground)
                    .child(t!("radio-now-playing")),
            )
            .child(div().mt(px(18.)).child(body))
            .into_any_element()
    }
}

impl Render for RadioView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_cover(cx);
        let page = Chrome::page(window, cx);
        let hero = self.hero(window, cx);
        let current = self.current(cx);
        let panel = self.now_panel(current, cx);
        div()
            .relative()
            .size_full()
            .child(
                Scroller::new("radio-page", &self.scrollbar)
                    .pb(px(32.) + Chrome::track_bottom(cx))
                    .child(
                        div().w_full().flex().justify_center().child(
                            div()
                                .w(page)
                                .px(px(PAGE_PAD))
                                .pt(px(HEADER_H + HEADER_TOP))
                                .child(hero)
                                .child(panel),
                        ),
                    ),
            )
            .into_any_element()
    }
}

/// The plate of a spinning cover, turned by the same clock and easing as the slide it sits
/// on. The caller picks the key: it restarts the turn when a roll begins.
fn spin_art(
    url: String,
    size: f32,
    key: SharedString,
    animation: Animation,
    turn: impl Fn(SpinningArtwork, f32) -> SpinningArtwork + 'static,
) -> AnyElement {
    SpinningArtwork::new(Some(url))
        .size(px(size - 12.))
        .with_animation(key, animation, turn)
        .into_any_element()
}
