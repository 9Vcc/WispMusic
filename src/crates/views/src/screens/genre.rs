use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, Pixels, Render, ScrollHandle, SharedString,
    WeakEntity, Window, div, px,
};
use i18n::t;
use state::{AppSettings, GenreDetails, Playback, Wisp};
use ui::{ActiveTheme as _, Button, Mode, Popovers, Scrollbar, Scroller, Skeleton, Text, vacant};

use crate::chrome::{Chrome, Toolbar, Tooled, tools};
use crate::shared::cells;
use crate::shared::shelves::Shelves;
use crate::shared::trouble;

/// How wide the title skeleton reaches while the genre name is on its way.
const TITLE_SKELETON: Pixels = px(240.);
const STEADY: Pixels = px(0.5);
const SECTION: &str = "genre";

pub(crate) struct GenreView {
    detail: Entity<GenreDetails>,
    settings: Entity<AppSettings>,
    shelves: Entity<Shelves>,
    scrollbar: Entity<Scrollbar>,
    toolbar: Entity<Toolbar>,
    popovers: Popovers,
    mode: Mode,
    width: Pixels,
    me: WeakEntity<Self>,
}

impl GenreView {
    pub(crate) fn new(
        detail: Entity<GenreDetails>,
        playback: Entity<Playback>,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = Wisp::global(cx).settings.clone();
        let mode = settings.read(cx).view_or(SECTION, Mode::Grid);
        let id = cx.entity_id();
        let shelves = cx.new(|_| Shelves::new("genre-shelf", id, playback.clone()).filled());

        cx.observe(&detail, |this, _, cx| {
            this.shelves.update(cx, |shelves, _| shelves.reset());
            cx.notify();
        })
        .detach();
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
        cx.observe(&shelves, |_, _, cx| cx.notify()).detach();
        let chrome = Chrome::entity(cx);
        cx.observe(&chrome, |_, _, cx| cx.notify()).detach();

        let me = cx.entity();
        let toolbar = Toolbar::tooled(&me, cx);

        Self {
            detail,
            settings,
            shelves,
            scrollbar: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(id)),
            toolbar,
            popovers: Popovers::default(),
            mode,
            width: Pixels::ZERO,
            me: me.downgrade(),
        }
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.settings
            .update(cx, |settings, cx| settings.set_view(SECTION, mode, cx));
        cx.notify();
    }

    /// Starts a page at its heading rather than where the previous page left the scroll.
    pub(crate) fn reset_scroll(&mut self, cx: &mut Context<Self>) {
        self.scrollbar
            .read(cx)
            .scroll()
            .set_offset(gpui::point(px(0.), px(0.)));
        cx.notify();
    }
}

impl Tooled for GenreView {
    fn toolbar(&self) -> Entity<Toolbar> {
        self.toolbar.clone()
    }

    fn tools(&self, _cx: &App) -> Vec<AnyElement> {
        let viewed = self.me.clone();

        vec![tools::views(&self.popovers, self.mode, move |mode, cx| {
            viewed.update(cx, |view, cx| view.set_mode(mode, cx)).ok();
        })]
    }
}

impl GenreView {
    /// The page a genre that did not load shows instead of its shelves. Opening the same genre
    /// again is the retry, since a failed load leaves the page empty.
    fn failure(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let id = self.detail.read(cx).id()?.to_owned();
        let reason = match trouble::unreachable(&id, cx) {
            true => None,
            false => Some(self.detail.read(cx).error()?.to_owned()),
        };
        let detail = self.detail.clone();

        Some(
            trouble::lost(
                "genre-lost",
                t!("trouble-not-loaded"),
                reason.as_deref(),
                move |_, _, cx| {
                    let id = id.clone();
                    detail.update(cx, |detail, cx| detail.open(&id, cx));
                },
                cx,
            )
            .size_full()
            .into_any_element(),
        )
    }
}

impl Render for GenreView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(failure) = self.failure(cx) {
            return div()
                .id("genre-failure")
                .role(Role::Alert)
                .flex()
                .flex_col()
                .size_full()
                .child(failure)
                .into_any_element();
        }

        let theme = *cx.theme();
        let pad = theme.metrics.inset;
        let room = cells::content_width(window, pad * 2., cx);
        if (room - self.width).abs() >= STEADY {
            self.width = room;
        }

        let detail = self.detail.read(cx);
        let loading = detail.is_loading();
        let title = detail.name().map(str::to_owned);
        let sections = detail.sections();
        let empty = !loading && sections.is_empty();
        let more = detail.has_more();
        let loading_more = detail.is_loading_more();
        let more_error = detail.more_error().is_some();
        let (mode, width) = (self.mode, self.width);
        let shelves = self
            .shelves
            .update(cx, |shelves, cx| match loading && sections.is_empty() {
                true => shelves.pending(width, cx),
                false => vec![shelves.render(sections, mode, width, window, cx)],
            });

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                Scroller::new("genre", &self.scrollbar)
                    .hold()
                    .bottom_inset(crate::chrome::Chrome::track_bottom(cx))
                    .py(pad)
                    .px(pad + crate::chrome::Chrome::margin(window, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_8()
                            .when(title.is_none() && loading, |this| {
                                this.child(
                                    Skeleton::new()
                                        .w(TITLE_SKELETON)
                                        .h(theme.text(Text::Display)),
                                )
                            })
                            .when_some(title, |this, title| {
                                let name = SharedString::from(title);
                                this.child(
                                    div()
                                        .id("genre-title")
                                        .role(Role::Heading)
                                        .aria_level(1)
                                        .aria_label(name.clone())
                                        .text_size(theme.text(Text::Display))
                                        .font_weight(FontWeight::BOLD)
                                        .child(name),
                                )
                            })
                            .when(empty, |this| {
                                let label = t!("genre-empty");
                                this.child(
                                    div()
                                        .id("genre-empty")
                                        .role(Role::Status)
                                        .aria_label(label.clone())
                                        .child(vacant(label, cx)),
                                )
                            })
                            .children(shelves)
                            .when(more || loading_more || more_error, |this| {
                                this.child(self.more_footer(more, loading_more, more_error))
                            }),
                    ),
            )
            .into_any_element()
    }
}

impl GenreView {
    /// The footer under a paged genre: it asks for the next page as soon as it scrolls into
    /// view, and the button stays as the manual retry when that page fails.
    fn more_footer(&self, more: bool, loading: bool, failed: bool) -> AnyElement {
        let detail = self.detail.clone();
        let auto = detail.clone();
        let label = match (loading, failed) {
            (true, _) => t!("genre-loading-more"),
            (false, true) => t!("genre-load-failed"),
            _ => t!("genre-load-more"),
        };

        div()
            .w_full()
            .flex()
            .justify_center()
            .py_4()
            .on_children_prepainted(move |bounds, window, cx| {
                if !more || loading || failed {
                    return;
                }
                let visible = bounds.first().is_some_and(|bounds| {
                    bounds.origin.y < window.viewport_size().height
                        && bounds.origin.y + bounds.size.height > Pixels::ZERO
                });
                if visible {
                    auto.update(cx, |detail, cx| detail.load_more(cx));
                }
            })
            .child(
                Button::new("genre-load-more-button")
                    .small()
                    .ghost()
                    .disabled(loading)
                    .label(label)
                    .on_click(move |_, _, cx| {
                        detail.update(cx, |detail, cx| detail.load_more(cx));
                    }),
            )
            .into_any_element()
    }
}
