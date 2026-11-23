use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use flate2::Compression;
use flate2::write::GzEncoder;

fn main() {
    fonts();

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let icon = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/windows/wisp.ico"
        );
        println!("cargo:rerun-if-changed={icon}");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon(icon);
        resource.set("ProductName", "WispMusic");
        resource.set("FileDescription", "WispMusic");
        resource.compile().expect("cannot embed the windows icon");
    }
}

fn fonts() {
    let assets = workspace().join("assets/fonts");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets the output"));

    // Font files are large and compress well; embed gzip copies and inflate them when the app
    // starts, so the binary keeps the same faces at roughly two thirds of their size.
    let mut rows = String::new();
    for item in embed::folder(&assets, "ttf") {
        let bytes = fs::read(&item.file)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", item.file.display()));
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder
            .write_all(&bytes)
            .unwrap_or_else(|error| panic!("cannot compress {}: {error}", item.file.display()));
        let compressed = out_dir.join(format!("{}.gz", item.name));
        fs::write(
            &compressed,
            encoder.finish().expect("cannot finish the font stream"),
        )
        .expect("cannot write a compressed font");
        let path = compressed.to_str().expect("a utf-8 output path");
        rows.push_str(&format!(
            "    (\"fonts/{}.ttf\", include_bytes!({path:?}).as_slice()),\n",
            item.name
        ));
    }
    let source = format!("const FONTS: &[(&str, &[u8])] = &[\n{rows}];\n");

    let out = out_dir.join("fonts.rs");
    fs::write(&out, source).expect("cannot write the font registry");
    // 必须监听字体目录：否则新增/更换字体不会触发重跑，新字体永远嵌不进包里。
    println!(
        "cargo:rerun-if-changed={}",
        assets.display()
    );
    println!("cargo:rerun-if-changed=build.rs");
}

/// The workspace root, taken from the crate's own manifest directory without touching the
/// filesystem, since `canonicalize` fails on a shared drive inside a Windows VM.
fn workspace() -> PathBuf {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo names the crate"));
    manifest
        .ancestors()
        .nth(2)
        .expect("the crate lives two levels under the workspace")
        .to_path_buf()
}
