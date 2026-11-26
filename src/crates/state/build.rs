use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const KIND: &str = "json";

fn main() {
    let root = workspace();

    let themes = root.join("assets/themes");
    let mut bundled = embed::folder(&themes, KIND);
    for (family, variants) in embed::tree(&themes, KIND) {
        bundled.extend(variants.into_iter().map(|item| embed::Item {
            name: format!("{family}/{}", item.name),
            file: item.file,
        }));
    }
    let source = format!(
        "pub(super) static BUNDLED: &[(&str, &[u8])] = {};\n",
        embed::embedded(&bundled, |item| item.name.clone()),
    );

    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets the output")).join("themes.rs");
    fs::write(&out, source).expect("cannot write the theme registry");
    println!("cargo:rerun-if-changed=build.rs");
    emit_updater(
        root.parent()
            .expect("the assets root sits under the project"),
    );
}

/// The CNB update manifest URLs, compiled into the binary. The signing key is the Tauri
/// updater's minisign key, embedded as a constant in the updates module.
fn emit_updater(root: &Path) {
    println!("cargo:rerun-if-env-changed=WISP_UPDATE_ENDPOINT");
    println!("cargo:rerun-if-env-changed=WISP_UPDATE_NIGHTLY_ENDPOINT");
    let _ = root;
    let default = "https://cnb.cool/MR9vcc/wisp-updater/-/git/raw/main/update-gpui.json";
    let endpoint = env::var("WISP_UPDATE_ENDPOINT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_owned());
    let nightly = env::var("WISP_UPDATE_NIGHTLY_ENDPOINT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| endpoint.replace("update-gpui.json", "update-gpui-nightly.json"));
    let source = format!(
        "pub const ENDPOINT: &str = {:?};
pub const NIGHTLY_ENDPOINT: &str = {:?};
",
        endpoint, nightly
    );
    let out =
        PathBuf::from(env::var("OUT_DIR").expect("cargo sets the output")).join("updater_pub.rs");
    fs::write(out, source).expect("cannot write the updater endpoint");
}

/// The workspace's asset root, taken from the crate's own manifest directory without touching
/// the filesystem, since `canonicalize` fails on a shared drive inside a Windows VM.
fn workspace() -> PathBuf {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo names the crate"));
    manifest
        .ancestors()
        .nth(2)
        .expect("the crate lives two levels under the workspace")
        .to_path_buf()
}
