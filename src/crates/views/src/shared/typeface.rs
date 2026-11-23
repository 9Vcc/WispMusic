//! Tells a font family that DirectWrite can draw from one whose metrics would crash the
//! renderer before it draws a single glyph.
//!
//! `DirectWriteTextSystem::font_metrics` computes the glyph box as
//! `glyphBoxRight - glyphBoxLeft` in `i16` (gpui_windows/src/direct_write.rs:681). A face whose
//! `DWRITE_FONT_METRICS` reports the right edge left of the left edge makes that a subtraction
//! with overflow, which a debug build panics on and a release build wraps into a negative width.
//! Fonts of that kind are not hypothetical: a third-party CJK face installed under
//! `%LOCALAPPDATA%\Microsoft\Windows\Fonts` has exactly this shape, and choosing it from the
//! typeface picker took the whole app down the moment the first line was painted.
//!
//! The picker lists every system family, so it has to leave these out. There is no cheap way to
//! ask DirectWrite whether a box is inverted — the metrics are only reachable through
//! `TextSystem::bounding_box`, which is where the panic happens — so each family is probed once
//! inside `catch_unwind` while the list is built, off the UI thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{Font, FontWeight, TextSystem, font, px};

/// Set while a family is being probed. Asking a broken face for its box panics (that is the
/// whole point of the probe), and the app-wide panic hook would otherwise write a full backtrace
/// to the log for every one of them — a couple hundred fonts are walked when the picker fills in.
/// The hook skips its backtrace while this is set; `catch_unwind` below handles the panic itself.
pub static PROBING: AtomicBool = AtomicBool::new(false);

/// Whether a panic caught right now is one the typeface probe raised.
pub fn probing() -> bool {
    PROBING.load(Ordering::Relaxed)
}

/// A face whose box cannot be measured would take the renderer down. Asking for the bounding box
/// is the only way to find that out, and in a debug build the answer is a panic rather than a
/// number, so the probe has to catch it.
pub(crate) fn draws(text: &TextSystem, family: &str) -> bool {
    // One weight is enough: the shape comes from the face DirectWrite picks, and a family whose
    // every weight is inverted is the only kind worth worrying about. Asking for a weight the
    // family does not carry would just resolve to the nearest one anyway.
    let requested = Font {
        weight: FontWeight::NORMAL,
        ..font(family.to_owned())
    };
    PROBING.store(true, Ordering::Relaxed);
    let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let id = text.resolve_font(&requested);
        let bounds = text.bounding_box(id, px(16.));
        let width = f32::from(bounds.size.width);
        width.is_finite() && width > 0.
    }));
    PROBING.store(false, Ordering::Relaxed);
    answer.unwrap_or(false)
}

/// `names` with the families whose metrics would crash the renderer taken out.
pub(crate) fn drawable(text_system: &Arc<TextSystem>, names: Vec<String>) -> Vec<String> {
    names
        .into_iter()
        .filter(|name| draws(text_system, name))
        .collect()
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    fn bundled() -> Arc<TextSystem> {
        let platform = gpui_platform::current_platform(false);
        Arc::new(TextSystem::new(platform.text_system()))
    }

    /// The family the app draws itself in has to survive the probe, or the picker would leave out
    /// the face every window is already using.
    #[test]
    fn the_app_font_is_drawable() {
        assert!(draws(&bundled(), "Alibaba PuHuiTi 3.0"));
    }

    /// A name DirectWrite knows nothing about resolves to the fallback face, which draws. The
    /// probe must not read that as broken, or it would throw away every unfamiliar name.
    #[test]
    fn an_unknown_family_falls_back_rather_than_failing() {
        assert!(draws(&bundled(), "No Such Family Zzz 12345"));
    }

    /// The probe leaves the flag clear whether it passed or caught a panic, so an unrelated crash
    /// later still gets its backtrace written.
    #[test]
    fn the_probe_clears_its_flag() {
        assert!(draws(&bundled(), "No Such Family Zzz 12345"));
        assert!(!probing());
    }
}
