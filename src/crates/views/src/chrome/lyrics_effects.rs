use std::time::Duration;

use gpui::Pixels;
use music::LyricsWord;

const LONG_WORD: Duration = Duration::from_millis(700);
const EDGE: f32 = 0.2;
const GLOW: f32 = 0.45;
const FLOAT: f32 = 0.08;
const SCALE: f32 = 0.15;

#[derive(Clone, Copy, Default)]
pub(super) struct WordFx {
    pub glow: bool,
    pub float: bool,
    pub scale: bool,
}

impl WordFx {
    pub fn any(self) -> bool {
        self.glow || self.float || self.scale
    }
}

#[derive(Clone, Copy)]
pub(super) struct WordMotion {
    pub glow: f32,
    pub lift: Pixels,
    pub scale: f32,
}

impl Default for WordMotion {
    fn default() -> Self {
        Self {
            glow: 0.,
            lift: Pixels::ZERO,
            scale: 1.,
        }
    }
}

pub(super) fn word_motion(
    word: &LyricsWord,
    position: Duration,
    verse: Pixels,
    fx: WordFx,
) -> WordMotion {
    if !fx.any() || !word.real_timed || word.text.trim().is_empty() {
        return WordMotion::default();
    }
    let span = word.end.saturating_sub(word.start);
    if span < LONG_WORD || span.is_zero() {
        return WordMotion::default();
    }
    let progress = ((position.saturating_sub(word.start)).as_secs_f32() / span.as_secs_f32())
        .clamp(0., 1.);
    let enter = (progress / EDGE).clamp(0., 1.);
    let leave = ((1. - progress) / EDGE).clamp(0., 1.);
    let envelope = smoothstep(enter.min(leave));
    WordMotion {
        glow: match fx.glow {
            true => GLOW * envelope,
            false => 0.,
        },
        lift: match fx.float {
            true => -verse * FLOAT * envelope,
            false => Pixels::ZERO,
        },
        scale: match fx.scale {
            true => 1. + SCALE * envelope,
            false => 1.,
        },
    }
}

fn smoothstep(value: f32) -> f32 {
    value * value * (3. - 2. * value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    fn word(start_ms: u64, end_ms: u64, real: bool) -> LyricsWord {
        LyricsWord {
            start: Duration::from_millis(start_ms),
            end: Duration::from_millis(end_ms),
            text: "word".to_owned(),
            real_timed: real,
        }
    }

    #[test]
    fn only_real_long_words_move() {
        let fx = WordFx {
            glow: true,
            float: true,
            scale: true,
        };
        assert_eq!(
            word_motion(&word(0, 699, true), Duration::from_millis(300), px(20.), fx).scale,
            1.
        );
        assert!(
            word_motion(&word(0, 700, true), Duration::from_millis(350), px(20.), fx).scale > 1.
        );
        assert_eq!(
            word_motion(&word(0, 1_000, false), Duration::from_millis(500), px(20.), fx).scale,
            1.
        );
    }

    #[test]
    fn envelope_peaks_in_the_middle_and_returns_to_rest() {
        let fx = WordFx {
            glow: true,
            float: true,
            scale: true,
        };
        let word = word(0, 1_000, true);
        let middle = word_motion(&word, Duration::from_millis(500), px(20.), fx);
        let edge = word_motion(&word, Duration::from_millis(1_000), px(20.), fx);
        assert!((middle.scale - 1.15).abs() < 0.001);
        assert!((middle.lift.as_f32() + 1.6).abs() < 0.001);
        assert!((middle.glow - 0.45).abs() < 0.001);
        assert_eq!(edge.scale, 1.);
        assert_eq!(edge.lift, Pixels::ZERO);
        assert_eq!(edge.glow, 0.);
    }
}
