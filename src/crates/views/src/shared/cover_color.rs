//! Turns a cover's leading hue into a lyric colour that reads on the surface the lyrics are
//! drawn over. Greyscale art names no hue, and then the lyrics keep their own colour.

use gpui::Hsla;

/// A cover accent lifted or deepened to sit legibly on a dark or light surface. `None` for
/// art without a real hue, so callers can keep their own foreground.
pub(crate) fn accent(colour: Hsla, dark: bool) -> Option<Hsla> {
    if colour.s < 0.08 {
        return None;
    }
    let (saturation, lightness) = match dark {
        true => (colour.s.clamp(0.45, 0.85), colour.l.clamp(0.55, 0.72)),
        false => (colour.s.clamp(0.55, 0.9), colour.l.clamp(0.42, 0.55)),
    };
    Some(Hsla {
        h: colour.h,
        s: saturation,
        l: lightness,
        a: 1.,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grey_cover_names_no_accent() {
        assert!(
            accent(
                Hsla {
                    h: 0.2,
                    s: 0.02,
                    l: 0.5,
                    a: 1.,
                },
                true
            )
            .is_none()
        );
    }

    #[test]
    fn a_cover_hue_is_lifted_for_a_dark_surface() {
        let colour = accent(
            Hsla {
                h: 0.3,
                s: 0.1,
                l: 0.2,
                a: 1.,
            },
            true,
        )
        .unwrap();
        assert!(colour.s >= 0.45);
        assert!(colour.l >= 0.55);
    }

    #[test]
    fn a_cover_hue_is_deepened_for_a_light_surface() {
        let colour = accent(
            Hsla {
                h: 0.3,
                s: 0.95,
                l: 0.9,
                a: 1.,
            },
            false,
        )
        .unwrap();
        assert!(colour.s <= 0.9);
        assert!(colour.l <= 0.55);
    }
}
