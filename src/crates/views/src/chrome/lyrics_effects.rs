use gpui::{Bounds, Pixels, Point};

pub(super) fn parallax_offset(
    pointer: Point<Pixels>,
    bounds: Bounds<Pixels>,
    inside: bool,
) -> Point<Pixels> {
    if !inside || bounds.size.width <= Pixels::ZERO || bounds.size.height <= Pixels::ZERO {
        return Point::default();
    }
    let center = bounds.center();
    let x = ((pointer.x - center.x).as_f32() / (bounds.size.width.as_f32() * 0.5)).clamp(-1., 1.);
    let y = ((pointer.y - center.y).as_f32() / (bounds.size.height.as_f32() * 0.5)).clamp(-1., 1.);
    Point::new(gpui::px(-x * 6.), gpui::px(-y * 4.))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px, size};

    #[test]
    fn parallax_is_centered_and_bounded() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(100.)));
        assert_eq!(parallax_offset(bounds.center(), bounds, true), Point::default());
        let corner = parallax_offset(point(px(200.), px(100.)), bounds, true);
        assert!((corner.x.as_f32() + 6.).abs() < 0.001);
        assert!((corner.y.as_f32() + 4.).abs() < 0.001);
        assert_eq!(parallax_offset(point(px(200.), px(100.)), bounds, false), Point::default());
    }
}