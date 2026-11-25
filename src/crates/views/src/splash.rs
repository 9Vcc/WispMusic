use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{Context, IntoElement, Render, Window, canvas, div, px};

use crate::brand::{FADE_END, FADE_START, TOTAL, ease_out_cubic, paint_mark};

pub struct Splash {
    elapsed: f32,
    done: bool,
}

impl Splash {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let start = Instant::now();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let finished = this
                    .update(cx, |this, cx| {
                        this.elapsed = start.elapsed().as_secs_f32();
                        if this.elapsed >= TOTAL {
                            this.done = true;
                        }
                        cx.notify();
                        this.done
                    })
                    .unwrap_or(true);
                if finished {
                    break;
                }
            }
        })
        .detach();
        Self {
            elapsed: 0.,
            done: false,
        }
    }

    pub fn done(&self) -> bool {
        self.done
    }

    /// Whether the mark is still assembling over the bare field, before the UI fades in.
    pub fn animating(&self) -> bool {
        !self.done && self.elapsed < FADE_START
    }

    /// How far the UI behind has taken over, from 0 while the mark assembles to 1 at the end.
    pub fn reveal(&self) -> f32 {
        if self.done {
            return 1.;
        }
        if self.elapsed <= FADE_START {
            return 0.;
        }
        ease_out_cubic(((self.elapsed - FADE_START) / (FADE_END - FADE_START)).clamp(0., 1.))
    }
}

impl Render for Splash {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if self.done {
            return div().absolute().inset_0().into_any_element();
        }
        let t = self.elapsed;
        let viewport = window.viewport_size();
        let shortest = viewport.width.min(viewport.height) / px(1.);
        let side = px(shortest * 0.66);
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .opacity(1. - self.reveal())
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| paint_mark(bounds, t, window),
                )
                .size(side)
                .flex_none(),
            )
            .into_any_element()
    }
}