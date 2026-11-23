pub(crate) mod about;
pub(crate) mod adaptive;
pub(crate) mod album_grid;
pub(crate) mod ambient;
pub(crate) mod animation;
pub(crate) mod blend;
pub(crate) mod cards;
pub(crate) mod cells;
pub(crate) mod confirm;
pub(crate) mod cover_viewer;
pub(crate) mod device_code;
pub(crate) mod device_menu;
pub(crate) mod equalizer;
pub(crate) mod hero;
pub(crate) mod local;
pub(crate) mod menus;
pub(crate) mod nav;
pub(crate) mod page;
pub(crate) mod picks;
pub(crate) mod pins;
pub(crate) mod playlist_editor;
pub(crate) mod popups;
pub(crate) mod powerbar;
pub(crate) mod shelves;
pub(crate) mod smart_playlist;
pub(crate) mod steps;
pub(crate) mod tag_editor;
pub(crate) mod text;
pub(crate) mod theme_miniature;
pub(crate) mod track_card;
pub(crate) mod tracks;
pub(crate) mod transport;
pub(crate) mod trouble;
pub(crate) mod typeface;
pub(crate) mod visualizer;
pub(crate) mod volume;

pub(crate) use ui::effects;

use state::Wisp;

/// 网易云音乐的红。
const NETEASE: u32 = 0xc20c11;
/// How much of the foreground text keeps where the theme would grey it over the ambient field.
/// The theme's grey is about that share of white over the page colour.
const QUIET: f32 = 0.56;

/// The provider's mark, from its manifest. An installed plugin's icon is a runtime asset; a
/// built-in manifest points at the embedded icon pack. Anything missing falls back to the
/// generic music mark.
pub(crate) fn provider_logo(slug: &str, cx: &gpui::App) -> &'static str {
    if let Some(asset) = Wisp::global(cx).plugins.read(cx).catalog().icon_asset(slug)
        && (icons::has(asset) || icons::asset(asset).is_some())
    {
        return asset;
    }
    match slug {
        "netease" => "icons/netease.svg",
        _ => "icons/music.svg",
    }
}

/// The brand colour of the provider `slug`'s mark, which for anything without one of its own is
/// the foreground.
pub(crate) fn provider_brand(slug: &str, cx: &gpui::App) -> gpui::Hsla {
    if let Some(brand) = Wisp::global(cx).plugins.read(cx).catalog().brand(slug)
        && let Some(color) = hex_colour(brand)
    {
        return color;
    }
    match slug {
        "netease" => gpui::rgb(NETEASE).into(),
        _ => ui::ActiveTheme::theme(cx).foreground,
    }
}

fn hex_colour(value: &str) -> Option<gpui::Hsla> {
    let hex = value.trim().strip_prefix('#')?;
    let (red, green, blue) = match hex.len() {
        3 => {
            let mut digits = hex.chars().map(|digit| digit.to_digit(16));
            (
                digits.next()?? * 17,
                digits.next()?? * 17,
                digits.next()?? * 17,
            )
        }
        6 | 8 => (
            u32::from_str_radix(&hex[..2], 16).ok()?,
            u32::from_str_radix(&hex[2..4], 16).ok()?,
            u32::from_str_radix(&hex[4..6], 16).ok()?,
        ),
        _ => return None,
    };
    Some(gpui::rgb((red << 16) | (green << 8) | blue).into())
}

/// The foreground at `QUIET`, in place of the theme's grey for text over the ambient field:
/// unsung lyrics, quieter lanes and the artist line. A flat grey stands apart from the colours
/// behind it, while translucent light takes them on.
pub(crate) fn quiet(theme: &ui::Theme) -> gpui::Hsla {
    theme.foreground.opacity(QUIET)
}
