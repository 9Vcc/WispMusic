//! Verifies the shipped font configuration against a real Windows text stack: the three
//! bundled Alibaba PuHuiTi faces must register as one family and lay out at three distinct
//! widths as the weight rises. `views` reaches its fonts through `wisp`'s asset table, so the
//! files are read straight off disk here instead.

#![cfg(target_os = "windows")]

use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use gpui::{Font, FontWeight, SharedString, TextRun, WindowTextSystem, black, font, px};

fn faces() -> Vec<Cow<'static, [u8]>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .expect("the bundled fonts are there")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            name.starts_with("AlibabaPuHuiTi") && name.ends_with(".ttf")
        })
        .collect();
    paths.sort();
    assert_eq!(
        paths.len(),
        3,
        "expected the three bundled faces: {paths:?}"
    );
    paths
        .into_iter()
        .map(|path| Cow::Owned(std::fs::read(path).unwrap()))
        .collect()
}

#[test]
fn the_bundled_family_lays_out_three_weights() {
    let platform = gpui_platform::current_platform(false);
    let text = Arc::new(gpui::TextSystem::new(platform.text_system()));
    let window_text = WindowTextSystem::new(text.clone());
    text.add_fonts(faces()).unwrap();

    let names = text.all_font_names();
    assert!(
        names
            .iter()
            .any(|name| name.contains("普惠体") || name.contains("PuHuiTi 3.0")),
        "the bundled family never registered; names looked like {:?}",
        &names[..names.len().min(8)]
    );

    let sample = SharedString::from("热门排行榜Ag");
    let width = |weight: FontWeight| {
        let run = TextRun {
            len: sample.len(),
            font: Font {
                weight,
                ..font("Alibaba PuHuiTi 3.0")
            },
            color: black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        f32::from(
            window_text
                .shape_line(sample.clone(), px(16.), &[run], None)
                .width(),
        )
    };

    let body = width(FontWeight::NORMAL);
    let heading = width(FontWeight::SEMIBOLD);
    let strong = width(FontWeight::BOLD);

    assert!(
        body < heading && heading < strong,
        "the three faces should get wider with weight, got {body} / {heading} / {strong}"
    );
}

/// The family must be named by its typographic name. The per-weight names found in the TTF
/// `name` table (`Alibaba PuHuiTi 3.0 65 Medium` and friends) do not match anything once the
/// faces are registered, and DirectWrite silently falls back instead of failing, so a wrong
/// constant would only show up as text in the wrong typeface. This pins the difference.
#[test]
fn the_per_weight_name_is_not_the_family() {
    let platform = gpui_platform::current_platform(false);
    let text = Arc::new(gpui::TextSystem::new(platform.text_system()));
    let window_text = WindowTextSystem::new(text.clone());
    text.add_fonts(faces()).unwrap();

    let sample = SharedString::from("热门排行榜Ag");
    let width = |family: &str| {
        let run = TextRun {
            len: sample.len(),
            font: font(family),
            color: black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        f32::from(
            window_text
                .shape_line(sample.clone(), px(16.), &[run], None)
                .width(),
        )
    };

    let family = width("Alibaba PuHuiTi 3.0");
    let weight_name = width("Alibaba PuHuiTi 3.0 65 Medium");
    let absent = width("No Such Family Zzz 12345");

    assert_ne!(
        family, absent,
        "the family name should reach the bundled face rather than the fallback"
    );
    assert_eq!(
        weight_name, absent,
        "the per-weight name is expected to miss and fall back; if this starts matching, the \
         names in the sources can go back to the per-weight form"
    );
}
