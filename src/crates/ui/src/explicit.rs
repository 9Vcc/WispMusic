use crate::metrics::Text;
use crate::theme::ActiveTheme as _;
use gpui::prelude::*;
use gpui::{
    App, Div, IntoElement, RenderOnce, SharedString, StyleRefinement, Window, div, px, rems,
    svg,
};

/// The badge's side, in rems.
const SIDE: f32 = 1.;
/// How much of the side an icon pack glyph takes, which leaves it the margin the drawn "E" has.
const GLYPH: f32 = 0.75;

/// A small filled square beside a title that marks what the item is, such as explicit or a local
/// file. It carries no name of its own, so the caller wraps it in a node that announces it.
#[derive(IntoElement)]
pub struct Badge {
    base: Div,
    mark: Mark,
}

/// What a badge shows.
enum Mark {
    /// The drawn "E", which sits in the exact middle of the square whatever font and scale are in
    /// use, an equal gap above and below it.
    Explicit,
    /// A glyph from the active icon pack, inset from the edges.
    Icon(SharedString),
    /// A word such as 付费, in a chip as tall as the square badges.
    Text(SharedString),
}

impl Badge {
    /// The square "E" a track marked explicit carries.
    pub fn explicit() -> Self {
        Self {
            base: div(),
            mark: Mark::Explicit,
        }
    }

    /// A badge showing `icon`, spelled `"icons/<name>.svg"`.
    pub fn icon(icon: impl Into<SharedString>) -> Self {
        Self {
            base: div(),
            mark: Mark::Icon(icon.into()),
        }
    }

    /// A short word in a chip, for a state the square glyphs have no mark for.
    pub fn text(text: impl Into<SharedString>) -> Self {
        Self {
            base: div(),
            mark: Mark::Text(text.into()),
        }
    }
}

impl Styled for Badge {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl RenderOnce for Badge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self { mut base, mark } = self;
        let theme = *cx.theme();
        let overrides = std::mem::take(base.style());
        let mut badge = base
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .bg(theme.muted)
            .rounded_xs();
        badge = match mark {
            Mark::Text(text) => badge
                .h(rems(SIDE))
                .px(px(4.))
                .text_size(theme.text(Text::Small))
                .text_color(theme.muted_foreground)
                .child(text),
            Mark::Explicit => badge
                .size(rems(SIDE))
                .child(svg().path(icons::path("icons/explicit.svg")).size(rems(SIDE)).text_color(theme.muted_foreground)),
            Mark::Icon(icon) => badge
                .size(rems(SIDE))
                .child(svg().path(icons::path(&icon)).size(rems(GLYPH)).text_color(theme.muted_foreground)),
        };
        badge.style().refine(&overrides);
        badge
    }
}
