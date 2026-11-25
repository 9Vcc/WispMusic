//! A marquee for a single line that is too wide for its column: cruise one copy plus a gap,
//! then repeat from a seamlessly aligned second copy.
//!
//! The caller keeps one [`MarqueeState`] per line and builds the line with a small factory.
//! The factory is called for four copies: the visible static one, an invisible measure, and
//! the two copies of the rolling track. Interactive copies must give each one its own id.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, IntoElement, SharedString, canvas, div, px,
};

/// The space between the two copies of a rolling line.
const GAP: f32 = 32.;
const SPEED: f32 = 50.;
/// Widths closer than this are considered unchanged, so paint-time measuring cannot spin.
const TOLERANCE: f32 = 0.5;

/// Widths measured for one marquee line. Keep it in the owning view.
#[derive(Clone, Default)]
pub(crate) struct MarqueeState {
    available: Rc<Cell<f32>>,
    natural: Rc<Cell<f32>>,
}

impl MarqueeState {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

/// Renders `content` once in flow, once invisible for measuring, and twice in the scrolling
/// track. `key` restarts the animation when the line's identity changes.
pub(crate) fn marquee<T: 'static>(
    state: &MarqueeState,
    id: &'static str,
    key: &str,
    content: impl Fn(usize) -> AnyElement,
    cx: &mut Context<T>,
) -> AnyElement {
    let natural = state.natural.get();
    let available = state.available.get();
    let scrolling = available > 0. && natural > available + 1.;
    let parent = cx.entity_id();
    let measured = state.natural.clone();
    let available_cell = state.available.clone();
    let container = div()
        .relative()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .child(
            div()
                .w_full()
                .whitespace_nowrap()
                .when(scrolling, |this| this.invisible())
                .child(content(0))
                .child(
                    canvas(
                        move |bounds, _, cx| {
                            let width = bounds.size.width.as_f32();
                            if (available_cell.get() - width).abs() > TOLERANCE {
                                available_cell.set(width);
                                cx.notify(parent);
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                ),
        )
        .child(
            div()
                .absolute()
                .left(px(0.))
                .top(px(0.))
                .whitespace_nowrap()
                .invisible()
                .child(content(1))
                .child(
                    canvas(
                        move |bounds, _, cx| {
                            let width = bounds.size.width.as_f32();
                            if (measured.get() - width).abs() > TOLERANCE {
                                measured.set(width);
                                cx.notify(parent);
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                ),
        );

    if !scrolling {
        return container.into_any_element();
    }

    let distance = natural + GAP;
    let total = distance / SPEED;
    let row = div()
        .absolute()
        .left(px(0.))
        .top(px(0.))
        .flex()
        .whitespace_nowrap()
        .child(div().flex_none().child(content(2)))
        .child(div().w(px(GAP)))
        .child(div().flex_none().child(content(3)));
    container
        .child(row.with_animation(
            SharedString::from(format!(
                "{id}-{key}-{}-{}",
                natural.round(),
                available.round()
            )),
            Animation::new(Duration::from_secs_f32(total)).repeat(),
            move |this, delta| this.left(px(-distance * delta)),
        ))
        .into_any_element()
}
