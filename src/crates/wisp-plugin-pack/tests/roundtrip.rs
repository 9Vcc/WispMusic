use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use ring::signature::{Ed25519KeyPair, KeyPair};
use tempfile::TempDir;
use wisp_plugin_api::ABI_MAJOR;
use wisp_plugin_pack::{
    InstallOptions, VerifyOptions, install, list, load, pack_dir, read_wisp, rollback, write_wisp,
    write_zip,
};

const SEED: [u8; 32] = [7; 32];

fn manifest(version: &str) -> String {
    format!(
        r##"{{
  "id": "demo",
  "slug": "demo",
  "name": {{ "default": "Demo" }},
  "version": "{version}",
  "abi": {{ "major": {ABI_MAJOR}, "minor": 0 }},
  "min_app_version": "0.51.0",
  "entry": "lib.wasm",
  "publisher": {{ "id": "wisp-official", "name": "WispMusic" }},
  "license": "MIT",
  "declares": {{ "kind": ["provider"], "ops": ["search"] }},
  "network": {{ "hosts": ["music.163.com"], "schemes": ["https"] }},
  "secrets": ["cookie"],
  "storage": {{ "private_dir": true, "max_bytes": 16777216 }},
  "assets": {{ "icon": "icons/demo.svg" }}
}}"##
    )
}

fn wasm() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (data (i32.const 8) "\02\00\00\00{}")
            (func (export "wisp_alloc") (param i32) (result i32) i32.const 1024)
            (func (export "wisp_free") (param i32))
            (func (export "wisp_describe") (result i32) i32.const 8)
            (func (export "wisp_call") (param i32 i32 i32) (result i32) i32.const 8)
        )"#,
    )
    .expect("the test wasm compiles")
}

fn source_tree(dir: &Path, version: &str) -> Result<()> {
    fs::create_dir_all(dir.join("icons"))?;
    fs::write(dir.join("wisp.json"), manifest(version))?;
    fs::write(dir.join("lib.wasm"), wasm())?;
    fs::write(dir.join("icons/demo.svg"), "<svg/>")?;
    fs::write(dir.join("LICENSE"), "MIT")?;
    Ok(())
}

fn key_pair() -> Ed25519KeyPair {
    Ed25519KeyPair::from_seed_unchecked(&SEED).expect("the test seed is valid")
}

fn public_key() -> Vec<u8> {
    key_pair().public_key().as_ref().to_vec()
}

fn verify_options() -> VerifyOptions {
    VerifyOptions {
        public_key: Some(public_key()),
        allow_unsigned: false,
        allow_zip: false,
        skip_signature: false,
    }
}

fn signed_package(dir: &Path, version: &str) -> Result<wisp_plugin_pack::Package> {
    source_tree(dir, version)?;
    pack_dir(dir, Some(&SEED))
}

#[test]
fn a_signed_container_round_trips_and_is_deterministic() -> Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    let package = signed_package(&source, "1.0.0")?;
    let first = temp.path().join("first.wisp");
    let second = temp.path().join("second.wisp");

    write_wisp(&package, &first)?;
    write_wisp(&package, &second)?;
    assert_eq!(fs::read(&first)?, fs::read(&second)?);

    let loaded = read_wisp(&first, &verify_options())?;
    assert_eq!(loaded.manifest.id, "demo");
    assert_eq!(loaded.manifest.version, "1.0.0");
    assert!(loaded.signed);
    assert_eq!(loaded.entries.len(), 3);
    assert!(loaded.entry("lib.wasm").is_some());
    assert!(loaded.entry("icons/demo.svg").is_some());
    assert_eq!(loaded.manifest_bytes, fs::read(source.join("wisp.json"))?);
    Ok(())
}

#[test]
fn a_zip_side_load_round_trips_in_dev_mode() -> Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    let package = signed_package(&source, "1.0.0")?;
    let zip = temp.path().join("demo.zip");
    let second = temp.path().join("demo-second.zip");
    write_zip(&package, &zip)?;
    write_zip(&package, &second)?;
    assert_eq!(fs::read(&zip)?, fs::read(&second)?);

    let options = VerifyOptions {
        public_key: Some(public_key()),
        allow_unsigned: false,
        allow_zip: true,
        skip_signature: false,
    };
    let loaded = load(&zip, &options)?;
    assert_eq!(loaded.manifest.version, "1.0.0");
    assert_eq!(
        loaded.entry("lib.wasm").expect("the wasm entry").raw,
        wasm()
    );

    let error = load(&zip, &verify_options()).expect_err("zip needs dev mode");
    assert!(error.to_string().contains("--dev"), "{error:#}");
    Ok(())
}

#[test]
fn an_unsigned_entry_sha_mismatch_is_rejected() -> Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    fs::create_dir_all(&source)?;
    fs::write(source.join("wisp.json"), manifest("1.0.0"))?;
    fs::write(source.join("lib.wasm"), wasm())?;
    fs::create_dir_all(source.join("icons"))?;
    fs::write(source.join("icons/demo.svg"), "<svg/>")?;
    let package = pack_dir(&source, None)?;
    let path = temp.path().join("unsigned.wisp");
    write_wisp(&package, &path)?;

    let mut bytes = fs::read(&path)?;
    let wasm = wasm();
    let at = bytes
        .windows(wasm.len())
        .position(|window| window == wasm)
        .expect("the stored wasm is in the payload");
    bytes[at + wasm.len() / 2] ^= 1;
    fs::write(&path, &bytes)?;

    let error = read_wisp(
        &path,
        &VerifyOptions {
            public_key: None,
            allow_unsigned: true,
            allow_zip: false,
            skip_signature: false,
        },
    )
    .expect_err("the entry digest changed");
    assert!(format!("{error:#}").contains("SHA-256"), "{error:#}");
    Ok(())
}

#[test]
fn an_incompatible_abi_is_rejected_at_pack_time() -> Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    source_tree(&source, "1.0.0")?;
    let manifest =
        fs::read_to_string(source.join("wisp.json"))?.replace("\"major\": 1", "\"major\": 2");
    fs::write(source.join("wisp.json"), manifest)?;
    let error = pack_dir(&source, Some(&SEED)).expect_err("ABI 2 is not supported");
    assert!(format!("{error:#}").contains("ABI"), "{error:#}");
    Ok(())
}

#[test]
fn a_tampered_manifest_is_rejected_before_json_parsing() -> Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    let package = signed_package(&source, "1.0.0")?;
    let path = temp.path().join("demo.wisp");
    write_wisp(&package, &path)?;
    let mut bytes = fs::read(&path)?;
    bytes[48 + 2] ^= 1;
    fs::write(&path, &bytes)?;

    let error = read_wisp(&path, &verify_options()).expect_err("the manifest changed");
    assert!(format!("{error:#}").contains("SHA-256"), "{error:#}");
    Ok(())
}

#[test]
fn a_tampered_payload_or_signature_is_rejected() -> Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    let package = signed_package(&source, "1.0.0")?;
    let path = temp.path().join("demo.wisp");
    write_wisp(&package, &path)?;
    let original = fs::read(&path)?;

    let mut payload = original.clone();
    *payload.last_mut().expect("the package has a payload") ^= 1;
    fs::write(&path, &payload)?;
    assert!(read_wisp(&path, &verify_options()).is_err());

    let mut signature = original;
    let manifest_len =
        u32::from_le_bytes(signature[8..12].try_into().expect("four bytes")) as usize;
    signature[48 + manifest_len] ^= 1;
    fs::write(&path, &signature)?;
    let error = read_wisp(&path, &verify_options()).expect_err("the signature changed");
    assert!(format!("{error:#}").contains("signature"), "{error:#}");
    Ok(())
}

#[test]
fn an_unknown_format_version_is_rejected() -> Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    let package = signed_package(&source, "1.0.0")?;
    let path = temp.path().join("demo.wisp");
    write_wisp(&package, &path)?;
    let mut bytes = fs::read(&path)?;
    bytes[4..6].copy_from_slice(&2u16.to_le_bytes());
    fs::write(&path, &bytes)?;

    let error = read_wisp(&path, &verify_options()).expect_err("version 2 is unknown");
    assert!(
        format!("{error:#}").contains("unsupported .wisp format version"),
        "{error:#}"
    );
    Ok(())
}

#[test]
fn a_zip_entry_cannot_escape_the_install_root() -> Result<()> {
    let temp = TempDir::new()?;
    let path = temp.path().join("evil.zip");
    let file = fs::File::create(&path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default());
    zip.start_file("wisp.json", options)?;
    zip.write_all(manifest("1.0.0").as_bytes())?;
    zip.start_file("lib.wasm", options)?;
    zip.write_all(&wasm())?;
    zip.start_file("../evil", options)?;
    zip.write_all(b"escape")?;
    zip.finish()?;

    let options = VerifyOptions {
        public_key: None,
        allow_unsigned: true,
        allow_zip: true,
        skip_signature: false,
    };
    let error = load(&path, &options).expect_err("traversal must be refused");
    assert!(format!("{error:#}").contains("path segment"), "{error:#}");
    Ok(())
}

#[test]
fn install_list_and_rollback_follow_current() -> Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    let plugins = temp.path().join("plugins");
    let options = |allow_downgrade| InstallOptions {
        plugins_dir: plugins.clone(),
        verify: verify_options(),
        allow_downgrade,
    };

    let package = signed_package(&source, "1.0.0")?;
    let one = temp.path().join("one.wisp");
    write_wisp(&package, &one)?;
    let installed = install(&one, &options(false))?;
    assert_eq!(installed.version, "1.0.0");
    assert_eq!(
        fs::read_to_string(plugins.join("demo/CURRENT"))?.trim(),
        "1.0.0"
    );

    let package = signed_package(&source, "2.0.0")?;
    let two = temp.path().join("two.wisp");
    write_wisp(&package, &two)?;
    install(&two, &options(false))?;
    let listed = list(&plugins)?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].version, "2.0.0");

    let package = signed_package(&source, "1.5.0")?;
    let middle = temp.path().join("middle.wisp");
    write_wisp(&package, &middle)?;
    assert!(
        install(&middle, &options(false)).is_err(),
        "a downgrade needs --allow-downgrade"
    );

    let restored = rollback(&plugins, "demo")?;
    assert_eq!(restored.version, "1.0.0");
    assert_eq!(
        fs::read_to_string(plugins.join("demo/CURRENT"))?.trim(),
        "1.0.0"
    );
    Ok(())
}

#[test]
fn pack_rejects_a_missing_entry() -> Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    fs::create_dir_all(&source)?;
    fs::write(source.join("wisp.json"), manifest("1.0.0"))?;
    let error = pack_dir(&source, Some(&SEED)).expect_err("lib.wasm is missing");
    assert!(format!("{error:#}").contains("missing"), "{error:#}");
    Ok(())
}

#[test]
fn a_manifest_slug_cannot_be_a_path_segment() -> Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    source_tree(&source, "1.0.0")?;
    fs::write(
        source.join("wisp.json"),
        manifest("1.0.0").replace(r#""slug": "demo""#, r#""slug": "..""#),
    )?;
    let error = pack_dir(&source, Some(&SEED)).expect_err(".. is not a slug");
    assert!(format!("{error:#}").contains("path segment"), "{error:#}");
    Ok(())
}

#[test]
fn an_unsigned_package_needs_the_dev_switch() -> Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    fs::create_dir_all(&source)?;
    fs::write(source.join("wisp.json"), manifest("1.0.0"))?;
    fs::write(source.join("lib.wasm"), wasm())?;
    fs::create_dir_all(source.join("icons"))?;
    fs::write(source.join("icons/demo.svg"), "<svg/>")?;
    let package = pack_dir(&source, None)?;
    let path = temp.path().join("unsigned.wisp");
    write_wisp(&package, &path)?;

    let error = read_wisp(&path, &verify_options()).expect_err("unsigned needs --dev");
    assert!(format!("{error:#}").contains("unsigned"), "{error:#}");
    let loaded = read_wisp(
        &path,
        &VerifyOptions {
            public_key: None,
            allow_unsigned: true,
            allow_zip: false,
            skip_signature: false,
        },
    )?;
    assert!(!loaded.signed);
    Ok(())
}

#[test]
fn a_signed_package_can_skip_signature_verification() -> Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    let package = signed_package(&source, "1.0.0")?;
    let path = temp.path().join("demo.wisp");
    write_wisp(&package, &path)?;

    let loaded = read_wisp(
        &path,
        &VerifyOptions {
            public_key: None,
            allow_unsigned: false,
            allow_zip: false,
            skip_signature: true,
        },
    )?;
    assert!(loaded.signed);
    assert_eq!(loaded.manifest.version, "1.0.0");
    Ok(())
}

#[test]
fn a_wrong_public_key_is_rejected() -> Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    let package = signed_package(&source, "1.0.0")?;
    let path = temp.path().join("demo.wisp");
    write_wisp(&package, &path)?;

    let other = Ed25519KeyPair::from_seed_unchecked(&[9; 32])
        .expect("the other seed is valid")
        .public_key()
        .as_ref()
        .to_vec();
    let error = read_wisp(
        &path,
        &VerifyOptions {
            public_key: Some(other),
            allow_unsigned: false,
            allow_zip: false,
            skip_signature: false,
        },
    )
    .expect_err("the signature must not verify");
    assert!(format!("{error:#}").contains("signature"), "{error:#}");
    Ok(())
}

#[test]
fn the_manifest_entry_must_be_in_the_container() -> Result<()> {
    let temp = TempDir::new()?;
    let source = temp.path().join("source");
    fs::create_dir_all(&source)?;
    fs::write(source.join("wisp.json"), manifest("1.0.0"))?;
    fs::write(source.join("lib.wasm"), wasm())?;
    fs::create_dir_all(source.join("icons"))?;
    fs::write(source.join("icons/demo.svg"), "<svg/>")?;
    let mut package = pack_dir(&source, None)?;
    package.entries.retain(|entry| entry.name != "lib.wasm");
    let path = temp.path().join("demo.wisp");
    write_wisp(&package, &path)?;

    let error = read_wisp(
        &path,
        &VerifyOptions {
            public_key: None,
            allow_unsigned: true,
            allow_zip: false,
            skip_signature: false,
        },
    )
    .expect_err("the entry is missing");
    assert!(format!("{error:#}").contains("missing"), "{error:#}");
    Ok(())
}
