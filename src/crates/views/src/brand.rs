//! The app mark drawn as vectors at any size: the About/sign-in icon and the launch sequence
//! share this geometry, so the mark stays crisp and keeps its own colours.

use gpui::prelude::*;
use gpui::{
    Bounds, ContentMask, Hsla, PathBuilder, Pixels, Point, Window, canvas, div, point, px, rgb,
    size,
};

const TILE: u32 = 0x17191B;
const WHITE: u32 = 0xF2EEE6;
const STONE: u32 = 0xBEB6A8;
const BLUE: u32 = 0x4C8DFF;
const RING: u32 = 0x2D3134;

pub(crate) const TOTAL: f32 = 3.0;
pub(crate) const FADE_START: f32 = 2.65;
pub(crate) const FADE_END: f32 = 3.00;

/// The W in the icon's 1024 grid, before the 1.3x scale about its centre.
const W_POINTS: [(f32, f32); 5] = [
    (320., 436.),
    (416., 580.),
    (512., 436.),
    (608., 580.),
    (704., 436.),
];
const W_SCALE: f32 = 1.3;
const W_HALF: f32 = 84. * W_SCALE / 2.;

const BAR_X: [f32; 5] = [384., 448., 512., 576., 640.];
const BAR_H: [f32; 5] = [100., 160., 200., 160., 100.];
const BAR_CY: f32 = 822.;
const BAR_HALF: f32 = 13.;

const RING_FROM: f32 = 117.5;
const RING_TO: f32 = 422.5;
const PIP_ANGLE: f32 = 326.03;

fn phase(t: f32, start: f32, end: f32) -> f32 {
    if end <= start {
        return 1.;
    }
    ease_out_cubic(((t - start) / (end - start)).clamp(0., 1.))
}

pub(crate) fn ease_out_cubic(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

fn color(value: u32, alpha: f32) -> Hsla {
    let mut color: Hsla = rgb(value).into();
    color.a = alpha;
    color
}

fn fill_polygon(window: &mut Window, points: &[Point<Pixels>], color: Hsla) {
    let mut path = PathBuilder::fill();
    path.add_polygon(points, true);
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn dot(window: &mut Window, center: Point<Pixels>, radius: Pixels, color: Hsla) {
    let mut points = Vec::with_capacity(32);
    for step in 0..32 {
        let angle = step as f32 / 32. * std::f32::consts::TAU;
        points.push(point(
            center.x + radius * angle.cos(),
            center.y + radius * angle.sin(),
        ));
    }
    fill_polygon(window, &points, color);
}

fn stroke(window: &mut Window, points: &[Point<Pixels>], width: Pixels, color: Hsla) {
    let mut path = PathBuilder::stroke(width);
    path.add_polygon(points, false);
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn draw_arc(
    window: &mut Window,
    at: &impl Fn(f32, f32) -> Point<Pixels>,
    scale: f32,
    from_deg: f32,
    to_deg: f32,
    color: Hsla,
) {
    if to_deg <= from_deg + 0.01 {
        return;
    }
    let steps = (((to_deg - from_deg).abs() / 3.).ceil() as usize).max(2);
    let mut points = Vec::with_capacity(steps + 1);
    for step in 0..=steps {
        let angle = (from_deg + (to_deg - from_deg) * step as f32 / steps as f32).to_radians();
        points.push(at(
            512. + 340. * angle.cos(),
            512. + 340. * angle.sin(),
        ));
    }
    stroke(window, &points, px(7. * scale), color);
    dot(window, points[0], px(3.5 * scale), color);
    if let Some(last) = points.last() {
        dot(window, *last, px(3.5 * scale), color);
    }
}

/// The W's visited points up to `p` along its path.
fn w_polyline(p: f32) -> Vec<(f32, f32)> {
    let points: Vec<(f32, f32)> = W_POINTS
        .iter()
        .map(|&(x, y)| (512. + (x - 512.) * W_SCALE, 512. + (y - 512.) * W_SCALE))
        .collect();
    let total: f32 = (0..4)
        .map(|i| {
            let (a, b) = (points[i], points[i + 1]);
            ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
        })
        .sum();
    let mut remaining = total * p.clamp(0., 1.);
    let mut out = vec![points[0]];
    for i in 0..4 {
        let (a, b) = (points[i], points[i + 1]);
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        if remaining >= len {
            out.push(b);
            remaining -= len;
        } else {
            let t = remaining / len;
            out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
            break;
        }
    }
    out
}

/// Draws the W as capsules between visited points plus round joins, so the silhouette keeps the
/// icon's round caps without relying on gpui's mitered stroke joins.
fn draw_w(
    window: &mut Window,
    at: &impl Fn(f32, f32) -> Point<Pixels>,
    scale: f32,
    points: &[(f32, f32)],
    color: Hsla,
) {
    for pair in points.windows(2) {
        let ((x1, y1), (x2, y2)) = (pair[0], pair[1]);
        let (dx, dy) = (x2 - x1, y2 - y1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.001 {
            continue;
        }
        let (nx, ny) = (-dy / len * W_HALF, dx / len * W_HALF);
        let corners = [
            at(x1 + nx, y1 + ny),
            at(x2 + nx, y2 + ny),
            at(x2 - nx, y2 - ny),
            at(x1 - nx, y1 - ny),
        ];
        fill_polygon(window, &corners, color);
    }
    for &(x, y) in points {
        dot(window, at(x, y), px(W_HALF * scale), color);
    }
}

fn capsule(
    window: &mut Window,
    at: &impl Fn(f32, f32) -> Point<Pixels>,
    scale: f32,
    center_x: f32,
    top: f32,
    bottom: f32,
    color: Hsla,
) {
    if bottom - top > 2. * BAR_HALF {
        let corners = [
            at(center_x - BAR_HALF, top + BAR_HALF),
            at(center_x + BAR_HALF, top + BAR_HALF),
            at(center_x + BAR_HALF, bottom - BAR_HALF),
            at(center_x - BAR_HALF, bottom - BAR_HALF),
        ];
        fill_polygon(window, &corners, color);
    }
    dot(
        window,
        at(center_x, top + BAR_HALF),
        px(BAR_HALF * scale),
        color,
    );
    dot(
        window,
        at(center_x, bottom - BAR_HALF),
        px(BAR_HALF * scale),
        color,
    );
}

fn draw_pip(
    window: &mut Window,
    at: &impl Fn(f32, f32) -> Point<Pixels>,
    scale: f32,
    angle_deg: f32,
    radius_scale: f32,
    alpha: f32,
) {
    if alpha <= 0.001 {
        return;
    }
    let angle = angle_deg.to_radians();
    let x = 512. + 340. * angle.cos();
    let y = 512. + 340. * angle.sin();
    dot(
        window,
        at(x, y),
        px(24. * radius_scale * scale),
        color(BLUE, alpha),
    );
}

pub(crate) fn paint_mark(bounds: Bounds<Pixels>, t: f32, window: &mut Window) {
    let scale = bounds.size.width / px(1024.);
    let at = |x: f32, y: f32| {
        point(
            bounds.origin.x + px(x * scale),
            bounds.origin.y + px(y * scale),
        )
    };

    let faint = 0.22 * phase(t, 0., 0.20);
    if faint > 0.001 {
        draw_arc(
            window,
            &at,
            scale,
            RING_FROM,
            RING_TO,
            color(RING, faint),
        );
    }

    let p = phase(t, 0.18, 1.95);
    let pip_deg = RING_FROM + (PIP_ANGLE - RING_FROM) * p;
    if p > 0.001 {
        draw_arc(
            window,
            &at,
            scale,
            RING_FROM,
            pip_deg,
            color(RING, 1.),
        );
    }
    let rest = phase(t, 1.95, 2.40);
    if rest > 0.001 {
        draw_arc(window, &at, scale, pip_deg, RING_TO, color(RING, 1.));
    }

    if p > 0.001 {
        let points = w_polyline(p);
        let half = bounds.size.width / 2.;
        let left = Bounds {
            origin: bounds.origin,
            size: size(half, bounds.size.height),
        };
        let right = Bounds {
            origin: point(bounds.origin.x + half, bounds.origin.y),
            size: size(half, bounds.size.height),
        };
        window.with_content_mask(Some(ContentMask { bounds: left }), |window| {
            draw_w(window, &at, scale, &points, color(WHITE, 1.));
        });
        window.with_content_mask(Some(ContentMask { bounds: right }), |window| {
            draw_w(window, &at, scale, &points, color(STONE, 1.));
        });
    }

    let breathe = (std::f32::consts::PI * phase(t, 2.30, 2.65)).sin();
    for (index, (&x, &height)) in BAR_X.iter().zip(BAR_H.iter()).enumerate() {
        let order = (index as f32 - 2.).abs();
        let grow = phase(t, 0.85 + order * 0.05, 2.30 + order * 0.05);
        let height = height * grow * (1. + 0.06 * breathe);
        if height > 0.5 {
            capsule(
                window,
                &at,
                scale,
                x,
                BAR_CY - height / 2.,
                BAR_CY + height / 2.,
                color(BLUE, 1.),
            );
        }
    }

    if p > 0.001 {
        let radius_scale = 0.7 + 0.3 * ease_out_cubic(p);
        draw_pip(
            window,
            &at,
            scale,
            pip_deg,
            radius_scale,
            (p * 3.).min(1.),
        );
    }
    if p >= 1. {
        let pop = phase(t, 1.95, 2.22);
        draw_pip(
            window,
            &at,
            scale,
            PIP_ANGLE,
            1. + 0.10 * (std::f32::consts::PI * pop).sin(),
            1.,
        );
    }
}

/// The app icon: the charcoal tile with the finished mark, as the platform icons show it.
pub(crate) fn paint_icon(bounds: Bounds<Pixels>, window: &mut Window) {
    let scale = bounds.size.width / px(1024.);
    let radius = px(232. * scale);
    let left = bounds.origin.x;
    let top = bounds.origin.y;
    let right = left + bounds.size.width;
    let bottom = top + bounds.size.height;

    let mut tile = PathBuilder::fill();
    tile.move_to(point(left + radius, top));
    tile.line_to(point(right - radius, top));
    tile.curve_to(point(right, top + radius), point(right, top));
    tile.line_to(point(right, bottom - radius));
    tile.curve_to(point(right - radius, bottom), point(right, bottom));
    tile.line_to(point(left + radius, bottom));
    tile.curve_to(point(left, bottom - radius), point(left, bottom));
    tile.line_to(point(left, top + radius));
    tile.curve_to(point(left + radius, top), point(left, top));
    tile.close();
    if let Ok(tile) = tile.build() {
        window.paint_path(tile, color(TILE, 1.));
    }

    paint_mark(bounds, TOTAL, window);
}

/// The app icon at `size`, painted at the display resolution rather than scaling a bitmap.
pub(crate) fn icon(size: Pixels) -> gpui::Div {
    div().size(size).flex_none().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| paint_icon(bounds, window),
        )
        .size(size),
    )
}