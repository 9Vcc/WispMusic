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

use gpui::{Font, FontFallbacks, FontWeight, SharedString, TextSystem, font, px};
use state::SYSTEM_FONT;

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

// [wisp] 全局主字体：内嵌的阿里巴巴普惠体 3.0（`src/assets/fonts/`，由
// `wisp/build.rs` 扫成 `FONTS` 表注册进 TextSystem）。它自带拉丁与中日韩字形，
// 中英文共用一套笔画与字面，观感统一；系统自带的雅黑/宋体在部分字号与字重下会
// 丢笔画、发虚。
//
// 这里必须是**主字体**，不能只当回退项：gpui_windows 生成回退映射时是从
// **系统**字体集合里按族名查字的（`direct_write.rs::generate_font_fallbacks`），
// 内嵌字体只活在 custom 集合里，系统里没装就查不到、那条映射会被静默跳过。
// 所以把普惠体写进 `SCRIPTS` 是无效的（此前即如此，中文实际一直走雅黑）。
//
// 族名取 typographic family（nameID 16）而**不是** nameID 1：实测注册进
// DirectWrite 后，字体集合报告的名字是本地化的「阿里巴巴普惠体 3.0」，而
// `Alibaba PuHuiTi 3.0 65 Medium` 这种带字重后缀的 nameID 1 反而查不到——排版
// 宽度与兜底字体一致，说明静默回落了。同一份实测还确认 10 个字重被归为**一个**
// 字族，能按 weight 选到不同字面，所以字重差异必须靠内嵌多份 ttf 来给：
// 目前内嵌 65 Medium（500，正文）/ 75 SemiBold（600）/ 85 Bold（700）三档，
// 品牌名与标题用的 SEMIBOLD/BOLD 就落到真字面上，而不是由 500 一档顶替。
pub(crate) const UI_FONT: &str = "Alibaba PuHuiTi 3.0";

// [wisp] 主字体缺字时的回退链，按语种排优先级。普惠体覆盖拉丁与中日韩，
// 这里补的是阿拉伯/希伯来/泰/天城体与 emoji，以及各平台的中文兜底。
const SCRIPTS: [&str; 18] = [
    "Source Han Sans",
    "Noto Sans CJK JP",
    "Noto Sans CJK SC",
    "Noto Sans CJK TC",
    "Noto Sans CJK KR",
    "Noto Sans Arabic",
    "Noto Sans Hebrew",
    "Noto Sans Thai",
    "Noto Sans Devanagari",
    "Hiragino Sans",
    "PingFang SC",
    "Apple SD Gothic Neo",
    "Yu Gothic UI",
    "Microsoft YaHei UI",
    "Malgun Gothic",
    "Noto Color Emoji",
    "Apple Color Emoji",
    "Segoe UI Emoji",
];

pub(crate) fn ui_font(chosen: &str) -> Font {
    match chosen == SYSTEM_FONT {
        true => Font {
            fallbacks: Some(scripts()),
            ..font(UI_FONT)
        },
        false => Font {
            fallbacks: Some(scripts()),
            ..font(SharedString::from(chosen.to_owned()))
        },
    }
}

/// The system fonts drawn for whatever `UI_FONT` and a chosen face leave uncovered. One list
/// for both, since an embedded face cannot serve as a fallback: gpui_windows resolves the
/// names below against the **system** collection only, where the bundled font is absent.
fn scripts() -> FontFallbacks {
    static SCRIPTS_FALLBACK: std::sync::OnceLock<FontFallbacks> = std::sync::OnceLock::new();
    SCRIPTS_FALLBACK
        .get_or_init(|| {
            FontFallbacks::from_fonts(SCRIPTS.iter().map(|name| (*name).to_owned()).collect())
        })
        .clone()
}

/// The family lyrics draw in: a Kai face bundled beside the interface font. The stroke effect
/// draws vector paths rather than glyphs, so a font only changes the text of a lyric line and
/// the letters that effect falls back to for characters it has no strokes for.
pub(crate) const LYRICS_FONT: &str = "Alimama DongFangDaKai";

/// The bundled lyric face with the same system fallbacks the interface font carries.
pub(crate) fn lyrics() -> Font {
    Font {
        fallbacks: Some(scripts()),
        ..font(LYRICS_FONT)
    }
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

    /// The embedded lyric face must be found by the name the lyrics ask for, or every lyric line
    /// would quietly fall back to the interface font.
    #[test]
    fn the_lyric_font_registers_under_its_name() {
        let text = TextSystem::new(gpui_platform::current_platform(false).text_system());
        text.add_fonts(vec![std::borrow::Cow::Borrowed(
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/fonts/AlimamaDongFangDaKai-Regular.ttf"
            ))
            .as_slice(),
        )])
        .unwrap();
        let face = |family: &str| {
            let id = text.resolve_font(&Font {
                family: family.to_owned().into(),
                ..Default::default()
            });
            (
                text.em_width(id, px(100.)).ok(),
                text.ascent(id, px(100.)),
                text.descent(id, px(100.)),
            )
        };
        assert_ne!(
            face(LYRICS_FONT),
            face("No Such Family Zzz 12345"),
            "the lyric face resolved to the fallback instead"
        );
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
