use std::time::Instant;

use gpui::prelude::*;
use gpui::{
    Context, IntoElement, MouseButton, Render, Window, WindowControlArea, canvas, div, px,
};

use crate::brand::{FADE_END, FADE_START, TOTAL, MarkPalette, paint_mark};
use ui::ActiveTheme as _;

/// How early the hidden UI starts building before the cross-fade, so its first layout is not a
/// hitch on the frame the fade begins.
const PREBUILD_LEAD: f32 = 0.20;
/// How much the mark closes into the page while the two layers cross-fade.
const MARK_SCALE: f32 = 0.035;
/// A small lift keeps the mark from feeling pinned to the centre as the page arrives.
const MARK_RISE: f32 = 6.;

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

pub struct Splash {
    start: Instant,
    done: bool,
}

impl Splash {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            start: Instant::now(),
            done: false,
        }
    }

    fn elapsed(&self) -> f32 {
        self.start.elapsed().as_secs_f32()
    }

    pub fn done(&self) -> bool {
        self.done
    }

    /// Whether the mark is still alone on the theme field; from here the hidden UI is laid out.
    pub fn animating(&self) -> bool {
        !self.done && self.elapsed() < FADE_START - PREBUILD_LEAD
    }

    /// How far the UI behind has taken over, from 0 while the mark assembles to 1 at the end.
    pub fn reveal(&self) -> f32 {
        if self.done {
            return 1.;
        }
        let elapsed = self.elapsed();
        if elapsed <= FADE_START {
            return 0.;
        }
        smoothstep(((elapsed - FADE_START) / (FADE_END - FADE_START)).clamp(0., 1.))
    }
}

impl Render for Splash {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = || div().absolute().inset_0().into_any_element();
        if self.done {
            return empty();
        }
        let t = self.elapsed();
        if t >= TOTAL {
            self.done = true;
            return empty();
        }
        // Frames come from the display rather than a timer, so the motion stays on vsync.
        window.request_animation_frame();

        let viewport = window.viewport_size();
        let shortest = viewport.width.min(viewport.height) / px(1.);
        let reveal = self.reveal();
        let side = px(shortest * 0.66 * (1. - MARK_SCALE * reveal));
        let palette = MarkPalette::launch(cx.theme());
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .opacity(1. - reveal)
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| paint_mark(bounds, t, palette, window),
                )
                .size(side)
                .mt(px(-MARK_RISE * reveal))
                .flex_none(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crossfade_is_held_then_centred_then_settled() {
        assert_eq!(smoothstep(-1.), 0.);
        assert_eq!(smoothstep(0.), 0.);
        assert_eq!(smoothstep(0.5), 0.5);
        assert_eq!(smoothstep(1.), 1.);
        assert_eq!(smoothstep(2.), 1.);
    }
}