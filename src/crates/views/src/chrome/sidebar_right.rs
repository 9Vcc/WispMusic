use gpui::prelude::*;
use gpui::{
    AnimationExt as _, App, Context, Entity, Pixels, Render, StyleRefinement, Window, div, px,
};
use state::{AppSettings, Playback, Queue, SideTab, Wisp};
use ui::{ActiveTheme as _, Entrance, MIN_CONTENT, Panel, Room, Side, entering, veiled};

use crate::chrome::Aside;
use crate::chrome::sidebar_motion::SidebarMotion;

const MIN_WIDTH: Pixels = px(240.);
const MAX_WIDTH: Pixels = px(560.);

fn fills_content(width: Pixels) -> bool {
    !Room::of(width).fits(Room::Wide)
}

/// The right sidebar: the queue and the lyrics, which share one view.
pub(crate) struct SidebarRight {
    aside: Entity<Aside>,
    settings: Entity<AppSettings>,
    playback: Entity<Playback>,
    width: Pixels,
    open: bool,
    motion: SidebarMotion,
    tab: SideTab,
    /// The entrance of the tab the open sidebar just switched to, the one a page makes as the
    /// listener moves to it, while it plays.
    entrance: Option<Entrance>,
}

impl SidebarRight {
    pub(crate) fn new(
        queue: Entity<Queue>,
        playback: Entity<Playback>,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = Wisp::global(cx).settings.clone();
        let width = px(settings.read(cx).sidebar_right_width()).clamp(MIN_WIDTH, MAX_WIDTH);
        let open = settings.read(cx).sidebar_right_open();
        let tab = settings.read(cx).sidebar_right_tab();
        let playback_for_fm = playback.clone();
        let aside = cx.new(|cx| Aside::new(queue, playback, tab, cx));
        // Private FM has no finite playlist to show. Hide its queue as soon as FM starts,
        // even if the panel was already open on it.
        cx.observe(&playback_for_fm, |this, _, cx| {
            if this.open && this.tab == SideTab::Queue && this.playback.read(cx).fm_session() {
                this.close(cx);
            }
        })
        .detach();
        cx.observe(&settings, |this, settings, cx| {
            if this.tab != SideTab::Lyrics || settings.read(cx).lyrics_enabled() {
                return;
            }
            this.tab = SideTab::Queue;
            if this.open {
                this.enter(cx);
            }
            this.aside
                .update(cx, |aside, cx| aside.show(SideTab::Queue, cx));
            settings.update(cx, |settings, cx| {
                settings.set_sidebar_right_tab(SideTab::Queue, cx)
            });
            cx.notify();
        })
        .detach();

        Self {
            aside,
            settings,
            playback: playback_for_fm,
            width,
            open,
            motion: SidebarMotion::settled(open),
            tab,
            entrance: None,
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn available(window: &Window) -> bool {
        !fills_content(window.viewport_size().width)
    }

    pub(crate) fn covers_content(&self, _window: &Window) -> bool {
        false
    }

    pub(crate) fn occupied_width(&self, window: &Window) -> Pixels {
        match Self::available(window) {
            false => Pixels::ZERO,
            true => self.width * self.motion.amount(std::time::Instant::now()),
        }
    }

    pub(crate) fn is_animating(&self, window: &Window) -> bool {
        Self::available(window) && self.motion.is_animating(std::time::Instant::now())
    }

    /// The width this sidebar occupies once its current open state has landed.
    pub(crate) fn settled_breadth(&self) -> Pixels {
        match self.open {
            true => self.width,
            false => Pixels::ZERO,
        }
    }

    /// The width the left sidebar has to leave free for this one. It counts an open sidebar
    /// even while the window is too narrow to show it, so the left sidebar collapses first and
    /// stays collapsed when this one hides in turn.
    pub(crate) fn reserved_width(&self) -> Pixels {
        match self.open {
            false => Pixels::ZERO,
            true => self.width,
        }
    }

    pub(crate) fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.tab == SideTab::Queue && self.playback.read(cx).fm_session() {
            return;
        }
        self.open = !self.open;
        if self.open {
            let tab = self.tab;
            self.aside.update(cx, |aside, cx| aside.show(tab, cx));
        }
        self.remember(cx);
        cx.notify();
    }

    pub(crate) fn show(&mut self, tab: SideTab, cx: &mut Context<Self>) {
        if tab == SideTab::Lyrics && !self.settings.read(cx).lyrics_enabled() {
            return;
        }
        if tab == SideTab::Queue && self.playback.read(cx).fm_session() {
            return;
        }
        if self.open && self.tab == tab {
            self.close(cx);
            return;
        }
        if self.tab != tab {
            self.settings
                .update(cx, |settings, cx| settings.set_sidebar_right_tab(tab, cx));
        }
        self.aside.update(cx, |aside, cx| aside.show(tab, cx));
        // A sidebar that opens on the tab slides out with it instead.
        if self.open && self.tab != tab {
            self.enter(cx);
        }
        self.tab = tab;
        self.open = true;
        self.remember(cx);
        cx.notify();
    }

    pub(crate) fn close(&mut self, cx: &mut Context<Self>) {
        self.aside.update(cx, |aside, cx| aside.dismiss(cx));
        self.open = false;
        self.remember(cx);
        cx.notify();
    }

    fn remember(&self, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.set_sidebar_right_open(self.open, cx)
        });
    }

    fn persist(&self, cx: &mut Context<Self>) {
        let width = self.width / px(1.);
        self.settings.update(cx, |settings, cx| {
            settings.set_sidebar_right_width(width, cx)
        });
    }

    /// Starts the entrance over the tab just switched to, unless the listener reduces motion.
    fn enter(&mut self, cx: &App) {
        self.entrance = (!cx.reduce_motion()).then(Entrance::start);
    }

    /// How much of the tab just switched to is still hidden, from 1 as the switch starts to 0
    /// once its entrance ends. It asks for another frame while the entrance runs.
    fn hidden(&mut self, window: &mut Window, cx: &App) -> f32 {
        let Some(entrance) = self.entrance else {
            return 0.;
        };
        if cx.reduce_motion() || !entrance.running() {
            self.entrance = None;
            return 0.;
        }
        window.request_animation_frame();
        entrance.hidden()
    }
}

impl Render for SidebarRight {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = std::time::Instant::now();
        let open = self.open;
        let animate = !cx.reduce_motion();
        if self.motion.needs_retarget(open, animate) {
            self.motion.retarget(open, now, animate);
            cx.notify();
        }
        let amount = self.motion.amount(now);
        if self.motion.is_animating(now) {
            window.request_animation_frame();
        }
        let animated_width = self.width * amount;
        if amount <= 0. || !Self::available(window) {
            return div().into_any_element();
        }

        let theme = *cx.theme();
        // The tab switched to enters the way a page does: a scrim of the sidebar's own colour
        // fades off it, so the queue and lyrics stay cached. A see-through window has no such
        // colour, so there the tab carries the fade itself and is drawn uncached while it runs,
        // since a cached view replays the opacity it was first painted with.
        let hidden = self.hidden(window, cx);
        let dissolving = theme.transparent && hidden > 0.;
        let tab = match dissolving {
            true => self.aside.clone().into_any_element(),
            false => self
                .aside
                .clone()
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
        };

        let panel = Panel::new("sidebar-right", Side::Right, animated_width)
            .w(animated_width)
            .opacity(amount)
            .limits(MIN_WIDTH, MAX_WIDTH)
            .reach(super::cap(MIN_WIDTH, MAX_WIDTH, MIN_CONTENT, window))
            .grip_above(super::Chrome::bottom(cx))
            .on_resize(cx.listener(|this, width: &Pixels, _, cx| {
                this.width = *width;
                this.persist(cx);
                cx.notify();
            }))
            .bg(theme.sidebar)
            .border_color(gpui::transparent_black())
            .child(
                div()
                    .relative()
                    .size_full()
                    .min_h_0()
                    .when(hidden > 0., |this| this.overflow_hidden())
                    .child(
                        div()
                            .size_full()
                            // At rest the tab is drawn bare, outside any layer, so whatever
                            // frosts in it keeps its backdrop.
                            .when(hidden > 0., |this| match theme.transparent {
                                true => entering(this, hidden),
                                false => veiled(this, hidden),
                            })
                            .child(tab),
                    )
                    .when(hidden > 0. && !theme.transparent, |this| {
                        this.child(
                            div()
                                .absolute()
                                .inset_0()
                                .bg(theme.background)
                                .opacity(hidden),
                        )
                    }),
            );
        panel
            .with_animation(
                gpui::SharedString::from(format!("sidebar-right-{open}")),
                gpui::Animation::new(ui::motion::paced(std::time::Duration::from_millis(200)))
                    .with_easing(ui::motion::ease_out_cubic),
                move |this, delta| this.opacity(if open { delta } else { 1. - delta }),
            )
            .into_any_element()
    }
}
