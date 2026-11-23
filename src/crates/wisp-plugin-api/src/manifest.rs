use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The ABI version a plugin was built against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AbiVersion {
    pub major: u16,
    pub minor: u16,
}

/// A name in the manifest's default locale plus optional translations.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalizedText {
    pub default: String,
    #[serde(flatten)]
    pub others: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Publisher {
    pub id: String,
    pub name: String,
}

/// What the plugin declares it can answer. The host renders capabilities from this block and
/// never trusts a runtime reply that contradicts it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Declares {
    /// v1 has exactly one kind: `provider`.
    pub kind: Vec<String>,
    /// Op names the plugin answers. Anything else falls back to the host default.
    pub ops: Vec<String>,
    /// Sign-in methods the plugin offers, in the order the host lists them: `qr`, `phone`,
    /// `password`, `qq`, `wechat`, `secret`. Empty keeps the host's v1 defaults.
    pub sign_in: Vec<String>,
    pub capabilities: CapabilitiesDto,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CapabilitiesDto {
    pub follow_artists: bool,
    pub radio: bool,
    /// The provider understands the FM mode and song-pool switches.
    pub radio_modes: bool,
    /// The provider offers a daily account check-in.
    pub checkin: bool,
    pub playcounts: bool,
    /// The provider's home feed can be asked for one set of shelves at a time through
    /// `HomeArgs.parts`, so the page can draw its first shelf before the rest arrives.
    pub home_parts: bool,
    /// The provider can switch an owned playlist between public and private.
    #[serde(default = "default_true")]
    pub playlist_visibility: bool,
    pub qualities: Vec<OfferDto>,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferDto {
    pub quality: String,
    pub encoding: EncodingDto,
    #[serde(default)]
    pub up_to: bool,
}

/// An audio encoding as it crosses the ABI. Mirrors the host's `Encoding` without borrowing
/// host types.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EncodingDto {
    Flac { depth: u32, rate: u32 },
    Mp3 { kbps: u32 },
    Aac { kbps: u32 },
}

/// The only hosts a plugin may reach. The host validates every `http_request` against this
/// block; no wildcard is accepted in v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkPolicy {
    pub hosts: Vec<String>,
    pub schemes: Vec<String>,
    pub redirects: u8,
    pub max_body_bytes: u64,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            hosts: Vec::new(),
            schemes: vec!["https".to_owned()],
            redirects: 1,
            max_body_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoragePolicy {
    pub private_dir: bool,
    pub max_bytes: u64,
}

impl Default for StoragePolicy {
    fn default() -> Self {
        Self {
            private_dir: true,
            max_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Assets {
    pub icon: Option<String>,
    pub brand: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct I18n {
    pub fluent: Option<String>,
}

/// `wisp.json`: the metadata every package carries at its root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Immutable identity of the plugin. Changing it is a new plugin.
    pub id: String,
    /// The stable session/settings key. Changing it does not migrate anything.
    pub slug: String,
    pub name: LocalizedText,
    pub version: String,
    pub abi: AbiVersion,
    pub min_app_version: String,
    pub entry: String,
    pub publisher: Publisher,
    pub license: String,
    #[serde(default)]
    pub homepage: Option<String>,
    pub declares: Declares,
    pub network: NetworkPolicy,
    #[serde(default)]
    pub secrets: Vec<String>,
    pub storage: StoragePolicy,
    pub assets: Assets,
    #[serde(default)]
    pub i18n: Option<I18n>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r##"{
      "id": "netease",
      "slug": "netease",
      "name": { "default": "NetEase", "zh-Hans": "网易云音乐" },
      "version": "1.0.0",
      "abi": { "major": 1, "minor": 0 },
      "min_app_version": "0.52.0",
      "entry": "lib.wasm",
      "publisher": { "id": "wisp-official", "name": "WispMusic" },
      "license": "AGPL-3.0-or-later",
      "declares": {
        "kind": ["provider"],
        "ops": ["search", "stream_resolve"],
        "capabilities": {
          "follow_artists": true,
          "qualities": [
            { "quality": "hires", "encoding": { "kind": "flac", "depth": 24, "rate": 96000 } }
          ]
        }
      },
      "network": { "hosts": ["music.163.com"], "max_body_bytes": 8388608 },
      "secrets": ["cookie"],
      "storage": { "private_dir": true, "max_bytes": 16777216 },
      "assets": { "icon": "icons/netease.svg", "brand": "#c20c11" },
      "i18n": { "fluent": "i18n/" }
    }"##;

    #[test]
    fn a_manifest_round_trips() {
        let manifest: Manifest = serde_json::from_str(EXAMPLE).expect("the manifest parses");
        assert_eq!(manifest.slug, "netease");
        assert_eq!(manifest.name.default, "NetEase");
        assert_eq!(manifest.name.others["zh-Hans"], "网易云音乐");
        assert_eq!(manifest.abi, AbiVersion { major: 1, minor: 0 });
        assert_eq!(manifest.declares.kind, ["provider"]);
        assert_eq!(manifest.declares.capabilities.qualities.len(), 1);
        assert_eq!(
            manifest.declares.capabilities.qualities[0].encoding,
            EncodingDto::Flac {
                depth: 24,
                rate: 96_000
            }
        );
        assert_eq!(manifest.network.redirects, 1);
        assert_eq!(manifest.network.schemes, ["https"]);

        let text = serde_json::to_string(&manifest).expect("the manifest serializes");
        let again: Manifest = serde_json::from_str(&text).expect("the manifest round trips");
        assert_eq!(again, manifest);
    }
}
