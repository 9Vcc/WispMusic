use unic_langid::{LanguageIdentifier, langid};

mod generated {
    include!(concat!(env!("OUT_DIR"), "/locales.rs"));
}

pub const AUTO: &str = "auto";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Language {
    English,
    ChineseSimplified,
}

impl Language {
    pub const ALL: [Self; 2] = [Self::English, Self::ChineseSimplified];

    pub fn id(self) -> &'static str {
        match self {
            Self::English => "en-US",
            Self::ChineseSimplified => "zh-CN",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::ChineseSimplified => "简体中文",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|language| language.id() == id)
    }

    pub fn detect() -> Self {
        sys_locale::get_locale().map_or(Self::English, |locale| Self::closest(&locale))
    }

    /// The shipped language nearest to a system locale such as `zh_CN.UTF-8`. A language
    /// shipped for more than one region is matched on the region first, then on the language
    /// alone. Anything else falls back to English.
    fn closest(locale: &str) -> Self {
        let wanted = parts(locale);

        Self::ALL
            .into_iter()
            .find(|language| parts(language.id()) == wanted)
            .or_else(|| {
                Self::ALL
                    .into_iter()
                    .find(|language| parts(language.id()).0 == wanted.0)
            })
            .unwrap_or(Self::English)
    }

    pub(crate) fn tag(self) -> LanguageIdentifier {
        match self {
            Self::English => langid!("en-US"),
            Self::ChineseSimplified => langid!("zh-CN"),
        }
    }

    pub(crate) fn sources(self) -> &'static [(&'static str, &'static str)] {
        generated::sources(self.id())
    }
}

pub fn resolve(id: &str) -> Language {
    Language::from_id(id).unwrap_or_else(Language::detect)
}

/// The language and the region of a locale tag, lowercased and uppercased. The encoding and
/// modifier a POSIX locale carries are dropped, and so is a script such as `Hans`.
fn parts(tag: &str) -> (String, Option<String>) {
    let tag = tag.split(['.', '@']).next().unwrap_or_default();
    let mut subtags = tag.split(['-', '_']);
    let language = subtags.next().unwrap_or_default().to_lowercase();
    let region = subtags
        .find(|subtag| {
            subtag.len() == 2 && subtag.chars().all(|letter| letter.is_ascii_alphabetic())
        })
        .map(str::to_uppercase);
    (language, region)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_picks_its_own_variant() {
        assert_eq!(Language::closest("zh-CN"), Language::ChineseSimplified);
        assert_eq!(Language::closest("zh-Hans-CN"), Language::ChineseSimplified);
        assert_eq!(Language::closest("en-US"), Language::English);
    }

    #[test]
    fn an_unshipped_region_falls_back_to_the_language() {
        assert_eq!(Language::closest("zh"), Language::ChineseSimplified);
        assert_eq!(Language::closest("zh-TW"), Language::ChineseSimplified);
        assert_eq!(Language::closest("en-GB"), Language::English);
    }

    #[test]
    fn an_unknown_language_falls_back_to_english() {
        assert_eq!(Language::closest("nl-NL"), Language::English);
        assert_eq!(Language::closest("de-AT"), Language::English);
        assert_eq!(Language::closest(""), Language::English);
    }
}