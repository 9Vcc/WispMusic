use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Div, ElementId, Hsla, MouseButton, Pixels, SharedString, StyleRefinement,
    Window, WindowControlArea, div, px, rgb, svg,
};

use i18n::t;

use crate::motion::{Springs, Sprung};

const SYSTEM_ACTS: bool = cfg!(target_os = "windows");
const DOT: Pixels = px(12.);
const GAP: Pixels = px(8.);
/// The glyph box matches the dot: the icons paint their shape across half the box, so a smaller
/// box leaves the close cross too thin to read on the bright red dot.
const GLYPH: Pixels = px(12.);
/// How far a dot grows under the pointer.
const GROW: f32 = 1.1;
/// How close a spring has to be to its target to count as landed.
const REST: f32 = 1e-4;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Close,
    Minimize,
    Maximize,
    Restore,
}

impl Control {
    fn icon(self) -> &'static str {
        match self {
            Self::Minimize => "icons/window-minimize.svg",
            Self::Maximize => "icons/window-maximize.svg",
            Self::Restore => "icons/window-restore.svg",
            Self::Close => "icons/window-close.svg",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Minimize => "traffic-light-minimize",
            Self::Maximize | Self::Restore => "traffic-light-maximize",
            Self::Close => "traffic-light-close",
        }
    }

    fn dot(self) -> Hsla {
        match self {
            Self::Close => rgb(0xff5f57).into(),
            Self::Minimize => rgb(0xfebc2e).into(),
            Self::Maximize | Self::Restore => rgb(0x28c840).into(),
        }
    }

    /// What a screen reader calls the control.
    fn name(self) -> SharedString {
        match self {
            Self::Minimize => t!("app-minimize"),
            Self::Maximize => t!("window-maximize"),
            Self::Restore => t!("window-restore"),
            Self::Close => t!("app-close-window"),
        }
    }

    fn system(self) -> bool {
        SYSTEM_ACTS && matches!(self, Self::Maximize | Self::Restore)
    }

    fn area(self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Maximize | Self::Restore => WindowControlArea::Max,
            Self::Close => WindowControlArea::Close,
        }
    }
}

/// The hover motion of one dot, kept across frames: the dot grows a little and its glyph fades in
/// while the pointer rests on it.
struct DotMotion {
    hovered: bool,
    scale: Sprung,
    glyph: Sprung,
}

impl DotMotion {
    fn new() -> Self {
        Self {
            hovered: false,
            scale: Sprung::at(1.),
            glyph: Sprung::at(0.),
        }
    }
}

/// Traffic-light styled window controls: three plain colored dots that reveal their glyph and
/// grow slightly under the pointer. The order follows the platform: close/minimize/maximize where
/// the macOS look belongs, minimize/maximize/close on Windows. The colors stay recognizable and
/// unthemed, which is the whole point of this style.
/// Position mirrors [`crate::WindowControls`]'s own `leading` flag: each platform's order is
/// written for its native edge, the leading one on macOS and the trailing one on Windows, and is
/// reversed when the controls sit on the other side.
#[derive(IntoElement)]
pub struct TrafficLightControls {
    base: Div,
    leading: bool,
}

impl TrafficLightControls {
    pub fn new(leading: bool) -> Self {
        Self {
            base: div(),
            leading,
        }
    }
}

impl Styled for TrafficLightControls {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl RenderOnce for TrafficLightControls {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let supported = window.window_controls();
        let maximized = window.is_maximized();
        let overrides = std::mem::take(self.base.style());
        let glyph_tint = Hsla {
            h: 0.,
            s: 0.,
            l: 0.,
            a: 0.7,
        };

        let maximize = supported.maximize.then_some(match maximized {
            true => Control::Restore,
            false => Control::Maximize,
        });
        let mut wanted: Vec<Control> = match cfg!(target_os = "windows") {
            true => [
                supported.minimize.then_some(Control::Minimize),
                maximize,
                Some(Control::Close),
            ]
            .into_iter()
            .flatten()
            .collect(),
            false => [
                Some(Control::Close),
                supported.minimize.then_some(Control::Minimize),
                maximize,
            ]
            .into_iter()
            .flatten()
            .collect(),
        };

        if cfg!(target_os = "windows") == self.leading {
            wanted.reverse();
        }

        let mut dots: Vec<AnyElement> = Vec::new();
        for control in wanted {
            let motion = window.use_keyed_state(
                ElementId::NamedChild(Arc::new(ElementId::from(control.id())), "motion".into()),
                cx,
                |_, _| DotMotion::new(),
            );
            let (scale, glyph) = motion.update(cx, |motion, cx| {
                let (scale_to, scale_spring) = match motion.hovered {
                    true => (GROW, Springs::PRESS),
                    false => (1., Springs::RELEASE),
                };
                let (glyph_to, glyph_spring) = match motion.hovered {
                    true => (1., Springs::PRESS),
                    false => (0., Springs::RELEASE),
                };
                (
                    motion.scale.follow(scale_to, scale_spring, REST, window, cx),
                    motion.glyph.follow(glyph_to, glyph_spring, REST, window, cx),
                )
            });

            let dot = div()
                .id(control.id())
                .role(gpui::accesskit::Role::Button)
                .aria_label(control.name())
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(DOT)
                .rounded_full()
                .bg(control.dot())
                .cursor_pointer()
                .occlude()
                .window_control_area(control.area())
                .when((scale - 1.).abs() > REST, |this| this.layer_scale(scale))
                .child(
                    svg()
                        .path(icons::path(control.icon()))
                        .id("glyph")
                        .size(GLYPH)
                        .flex_none()
                        .text_color(glyph_tint)
                        .opacity(glyph.clamp(0., 1.)),
                )
                .on_hover(move |hovered, _, cx| {
                    motion.update(cx, |motion, cx| {
                        if motion.hovered == *hovered {
                            return;
                        }
                        motion.hovered = *hovered;
                        cx.notify();
                    });
                })
                .when(!control.system(), |this| {
                    this.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            match control {
                                Control::Minimize => window.minimize_window(),
                                Control::Maximize | Control::Restore => window.zoom_window(),
                                Control::Close => window.remove_window(),
                            }
                        })
                })
                // The system handles a press on its own maximize, which a screen reader cannot
                // reach, so the click action zooms the window itself. Zooming never restores, so
                // Restore gets no click action.
                .when(control == Control::Maximize && control.system(), |this| {
                    this.on_a11y_action(gpui::accesskit::Action::Click, |_, window, _| {
                        window.zoom_window()
                    })
                });
            dots.push(dot.into_any_element());
        }

        let mut controls = self
            .base
            .flex()
            .flex_none()
            .items_center()
            .gap(GAP)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(dots);
        controls.style().refine(&overrides);
        controls
    }
}
