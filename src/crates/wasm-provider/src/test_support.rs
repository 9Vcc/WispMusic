//! Test-only package fixtures.

use std::collections::BTreeMap;

use wisp_plugin_api::manifest::{
    AbiVersion, Assets, Declares, LocalizedText, Manifest, NetworkPolicy, Publisher, StoragePolicy,
};

pub(crate) fn manifest(slug: &str) -> Manifest {
    Manifest {
        id: slug.to_owned(),
        slug: slug.to_owned(),
        name: LocalizedText {
            default: format!("{slug} display"),
            others: BTreeMap::new(),
        },
        version: "1.2.3".to_owned(),
        abi: AbiVersion { major: 1, minor: 0 },
        min_app_version: "0.0.0".to_owned(),
        entry: "lib.wasm".to_owned(),
        publisher: Publisher {
            id: "test".to_owned(),
            name: "Test".to_owned(),
        },
        license: "AGPL-3.0-or-later".to_owned(),
        homepage: None,
        declares: Declares {
            kind: vec!["provider".to_owned()],
            ops: vec!["sign_in".to_owned()],
            ..Declares::default()
        },
        network: NetworkPolicy {
            hosts: vec!["example.com".to_owned()],
            ..NetworkPolicy::default()
        },
        secrets: Vec::new(),
        storage: StoragePolicy::default(),
        assets: Assets::default(),
        i18n: None,
    }
}
