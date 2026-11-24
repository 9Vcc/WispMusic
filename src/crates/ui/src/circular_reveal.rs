use std::panic::Location;

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
    LayoutId, PathBuilder, Pixels, Point, Style, Window, point, px, relative,
};

pub struct CircularReveal {
    id: ElementId,
    content: Option<AnyElement>,
    progress: f32,
    center: Point<f32>,
    mask: Hsla,
}

impl CircularReveal {
    pub fn new(
        id: impl Into<ElementId>,
        content: AnyElement,
        progress: f32,
        center: Point<f32>,
        mask: Hsla,
    ) -> Self {
        Self {
            id: id.into(),
            content: Some(content),
            progress: progress.clamp(0., 1.),
            center,
            mask,
        }
    }
}

impl IntoElement for CircularReveal {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for CircularReveal {
    type RequestLayoutState = Option<AnyElement>;
    type PrepaintState = Option<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        style.flex_shrink = 0.;
        let Some(mut content) = self.content.take() else {
            return (window.request_layout(style, [], cx), None);
        };
        let child = content.request_layout(window, cx);
        (window.request_layout(style, [child], cx), Some(content))
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        laid: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let Some(mut content) = laid.take() else {
            return None;
        };
        content.prepaint(window, cx);
        Some(content)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _laid: &mut Self::RequestLayoutState,
        content: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(content) = content {
            content.paint(window, cx);
        }
        if self.progress < 1. {
            paint_mask(bounds, self.center, self.progress, self.mask, window);
        }
    }
}

fn paint_mask(
    bounds: Bounds<Pixels>,
    center: Point<f32>,
    progress: f32,
    mask: Hsla,
    window: &mut Window,
) {
    let center = point(
        bounds.origin.x + bounds.size.width * center.x,
        bounds.origin.y + bounds.size.height * center.y,
    );
    let radius = reveal_radius(bounds, center, progress);
    let mut path = PathBuilder::fill();
    path.move_to(point(bounds.left(), bounds.top()));
    path.line_to(point(bounds.right(), bounds.top()));
    path.line_to(point(bounds.right(), bounds.bottom()));
    path.line_to(point(bounds.left(), bounds.bottom()));
    path.close();
    if radius > Pixels::ZERO {
        let top = point(center.x, center.y - radius);
        let bottom = point(center.x, center.y + radius);
        let radii = point(radius, radius);
        path.move_to(top);
        path.arc_to(radii, px(0.), false, true, bottom);
        path.arc_to(radii, px(0.), false, true, top);
        path.close();
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, mask);
    }
}

fn reveal_radius(bounds: Bounds<Pixels>, center: Point<Pixels>, progress: f32) -> Pixels {
    let corners = [
        point(bounds.left(), bounds.top()),
        point(bounds.right(), bounds.top()),
        point(bounds.right(), bounds.bottom()),
        point(bounds.left(), bounds.bottom()),
    ];
    let reach = corners
        .into_iter()
        .map(|corner| {
            let x = (corner.x - center.x).as_f32();
            let y = (corner.y - center.y).as_f32();
            px(x.hypot(y))
        })
        .fold(Pixels::ZERO, Pixels::max);
    reach * progress.clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_starts_at_zero_and_reaches_every_corner() {
        let bounds = Bounds::new(point(px(10.), px(20.)), gpui::size(px(100.), px(50.)));
        let center = point(px(60.), px(45.));
        assert_eq!(reveal_radius(bounds, center, 0.), px(0.));
        assert!((reveal_radius(bounds, center, 1.).as_f32() - 55.9017).abs() < 0.001);
        assert!((reveal_radius(bounds, center, 0.5).as_f32() - 27.95085).abs() < 0.001);
    }
}
