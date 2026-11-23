//! Package format, signing and installation for WispMusic provider plugins.
//!
//! `.wisp` is the signed distribution container; `.zip` is a development side-load format with
//! the same manifest, file tree and signature payload. The signature canonicalizes each entry's
//! name, raw length and SHA-256 rather than its container offsets, so both formats verify against
//! the same digest.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use ring::signature::{ED25519, Ed25519KeyPair, UnparsedPublicKey};
use semver::Version;
use sha2::{Digest, Sha256};
use wisp_plugin_api::{ABI_MAJOR, Manifest};
#[cfg(feature = "instantiate")]
use wisp_plugin_host::Runtime;

pub const WISP_MAGIC: &[u8; 4] = b"WISP";
pub const FORMAT_VERSION: u16 = 1;

const FLAG_SIGNED: u16 = 1;
const FLAG_ZSTD: u16 = 2;
const FLAG_DEBUG: u16 = 4;
const KNOWN_FLAGS: u16 = FLAG_SIGNED | FLAG_ZSTD | FLAG_DEBUG;

const HEADER_BYTES: usize = 48;
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TOTAL_RAW_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_NAME_BYTES: usize = 512;
const MAX_REDIRECTS: u8 = 1;
const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Store = 0,
    Deflate = 1,
}

impl Method {
    fn from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Store),
            1 => Ok(Self::Deflate),
            other => bail!("unknown entry compression method {other}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PackageEntry {
    pub name: String,
    pub raw: Vec<u8>,
    pub compressed: Vec<u8>,
    pub method: Method,
    pub sha256: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct Package {
    pub manifest: Manifest,
    pub manifest_bytes: Vec<u8>,
    pub signature: Vec<u8>,
    pub signed: bool,
    pub entries: Vec<PackageEntry>,
}

impl Package {
    pub fn entry(&self, name: &str) -> Option<&PackageEntry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    pub fn verify_signature(&self, public_key: &[u8]) -> Result<()> {
        ensure!(self.signed, "the package is not signed");
        verify_signature(
            &self.manifest_bytes,
            &signing_entries(&self.entries),
            &self.signature,
            public_key,
        )
    }
}

#[derive(Clone, Debug)]
struct SigningEntry {
    name: String,
    raw_len: u64,
    sha256: [u8; 32],
}

fn signing_entries(entries: &[PackageEntry]) -> Vec<SigningEntry> {
    entries
        .iter()
        .map(|entry| SigningEntry {
            name: entry.name.clone(),
            raw_len: entry.raw.len() as u64,
            sha256: entry.sha256,
        })
        .collect()
}

#[derive(Clone, Debug, Default)]
pub struct VerifyOptions {
    pub public_key: Option<Vec<u8>>,
    pub allow_unsigned: bool,
    pub allow_zip: bool,
    /// Accepts a signed package without checking its signature. Used by hosts that deliberately
    /// install unauthenticated development packages.
    pub skip_signature: bool,
}

#[derive(Clone, Debug)]
pub struct InstallOptions {
    pub plugins_dir: PathBuf,
    pub verify: VerifyOptions,
    pub allow_downgrade: bool,
}

#[derive(Clone, Debug)]
pub struct InstalledPlugin {
    pub id: String,
    pub slug: String,
    pub version: String,
    pub abi: String,
    pub publisher: String,
    pub path: PathBuf,
}

/// Reads a source tree, signs it and returns the in-memory package. `private_key` accepts a
/// 32-byte Ed25519 seed (raw or 64 hex characters) or a PKCS#8 document. `None` produces an
/// unsigned development package.
pub fn pack_dir(dir: &Path, private_key: Option<&[u8]>) -> Result<Package> {
    let manifest_path = dir.join("wisp.json");
    let manifest_bytes = fs::read(&manifest_path)
        .with_context(|| format!("cannot read {}", manifest_path.display()))?;
    ensure!(
        manifest_bytes.len() <= MAX_MANIFEST_BYTES,
        "wisp.json exceeds {MAX_MANIFEST_BYTES} bytes"
    );
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .context("wisp.json is not a valid plugin manifest")?;
    validate_manifest(&manifest)?;

    let entries = collect_entries(dir)?;
    ensure!(
        entries.iter().any(|entry| entry.name == manifest.entry),
        "the manifest entry `{}` is missing from the source tree",
        manifest.entry
    );
    ensure!(
        entries
            .iter()
            .all(|entry| entry.name != "wisp.json" && entry.name != "wisp.sig"),
        "wisp.json and wisp.sig must live at the package root, not in the entry table"
    );
    let (signature, signed) = match private_key {
        Some(key) => {
            let key_pair = private_key_pair(key)?;
            let digest = signing_digest(&manifest_bytes, &signing_entries(&entries));
            (key_pair.sign(&digest).as_ref().to_vec(), true)
        }
        None => (Vec::new(), false),
    };
    Ok(Package {
        manifest,
        manifest_bytes,
        signature,
        signed,
        entries,
    })
}

pub fn write_wisp(package: &Package, path: &Path) -> Result<()> {
    let bytes = encode_wisp(package)?;
    atomic_write(path, &bytes)
}

pub fn write_zip(package: &Package, path: &Path) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = temporary_path(path);
    let file = File::create(&temporary)
        .with_context(|| format!("cannot create {}", temporary.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let manifest_options = zip_options(zip::CompressionMethod::Stored);
    zip.start_file("wisp.json", manifest_options)
        .context("cannot add wisp.json to the zip")?;
    zip.write_all(&package.manifest_bytes)?;

    let signature_options = zip_options(zip::CompressionMethod::Stored);
    if package.signed {
        zip.start_file("wisp.sig", signature_options)
            .context("cannot add wisp.sig to the zip")?;
        zip.write_all(&package.signature)?;
    }

    let mut entries: Vec<&PackageEntry> = package.entries.iter().collect();
    entries.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
    for entry in entries {
        let method = if entry.name.ends_with(".wasm") {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };
        zip.start_file(&entry.name, zip_options(method))
            .with_context(|| format!("cannot add {} to the zip", entry.name))?;
        zip.write_all(&entry.raw)?;
    }
    let finished = zip.finish()?;
    drop(finished);
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("cannot replace {}", path.display()))?;
    }
    fs::rename(&temporary, path)
        .with_context(|| format!("cannot move the zip into {}", path.display()))?;
    Ok(())
}

pub fn load(path: &Path, options: &VerifyOptions) -> Result<Package> {
    let mut file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut magic = [0u8; 4];
    let read = file.read(&mut magic)?;
    ensure!(read == 4, "the package is too short");
    match &magic {
        WISP_MAGIC => read_wisp(path, options),
        b"PK\x03\x04" => {
            ensure!(
                options.allow_zip,
                "zip side-load packages are only accepted with --dev"
            );
            read_zip(path, options)
        }
        _ => bail!("the file is neither a .wisp container nor a zip archive"),
    }
}

pub fn read_wisp(path: &Path, options: &VerifyOptions) -> Result<Package> {
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    decode_wisp(&bytes, options).with_context(|| format!("cannot verify {}", path.display()))
}

pub fn read_zip(path: &Path, options: &VerifyOptions) -> Result<Package> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).with_context(|| format!("cannot read {}", path.display()))?;
    ensure!(
        archive.len() <= MAX_ENTRIES + 2,
        "the zip has too many entries"
    );

    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut signature: Option<Vec<u8>> = None;
    let mut entries = Vec::new();
    let mut seen = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .with_context(|| format!("cannot read zip entry #{index}"))?;
        let name = file.name().to_owned();
        if file.is_dir() {
            continue;
        }
        if name == "wisp.json" {
            ensure!(
                manifest_bytes.is_none(),
                "the zip has more than one wisp.json"
            );
            manifest_bytes = Some(read_limited(&mut file, MAX_MANIFEST_BYTES as u64, &name)?);
            continue;
        }
        if name == "wisp.sig" {
            ensure!(signature.is_none(), "the zip has more than one wisp.sig");
            signature = Some(read_limited(&mut file, 64, &name)?);
            continue;
        }
        validate_entry_name(&name)?;
        ensure!(
            seen.insert(name.clone(), ()).is_none(),
            "duplicate zip entry `{name}`"
        );
        let raw = read_limited(&mut file, MAX_ENTRY_BYTES, &name)?;
        let sha256 = sha256(&raw);
        entries.push(PackageEntry {
            name,
            raw,
            compressed: Vec::new(),
            method: Method::Store,
            sha256,
        });
    }
    entries.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
    let manifest_bytes = manifest_bytes.context("the zip has no root wisp.json")?;
    let signature = signature.unwrap_or_default();
    let signed = !signature.is_empty();
    verify_signature_policy(
        &manifest_bytes,
        &signing_entries(&entries),
        &signature,
        signed,
        options,
    )?;
    let manifest: Manifest =
        serde_json::from_slice(&manifest_bytes).context("wisp.json is not a valid manifest")?;
    validate_manifest(&manifest)?;
    verify_entry_references(&manifest, &entries)?;
    Ok(Package {
        manifest,
        manifest_bytes,
        signature,
        signed,
        entries,
    })
}

fn decode_wisp(bytes: &[u8], options: &VerifyOptions) -> Result<Package> {
    ensure!(bytes.len() >= HEADER_BYTES, "the .wisp header is truncated");
    ensure!(&bytes[..4] == WISP_MAGIC, "the .wisp magic is missing");
    let format = u16::from_le_bytes(bytes[4..6].try_into().expect("two bytes"));
    ensure!(
        format == FORMAT_VERSION,
        "unsupported .wisp format version {format}; update WispMusic"
    );
    let flags = u16::from_le_bytes(bytes[6..8].try_into().expect("two bytes"));
    ensure!(
        flags & !KNOWN_FLAGS == 0,
        "the .wisp header carries unknown flags"
    );
    ensure!(
        flags & FLAG_ZSTD == 0,
        "zstd-compressed containers are not supported yet"
    );
    let manifest_len = read_u32(bytes, 8) as usize;
    ensure!(
        manifest_len <= MAX_MANIFEST_BYTES,
        "the manifest exceeds {MAX_MANIFEST_BYTES} bytes"
    );
    let manifest_sha = &bytes[12..44];
    let sig_len = read_u32(bytes, 44) as usize;
    let signed = flags & FLAG_SIGNED != 0;
    ensure!(
        signed == (sig_len == 64),
        "the signature flag and signature length disagree"
    );
    ensure!(
        !signed || sig_len == 64,
        "a signed .wisp must carry a 64-byte Ed25519 signature"
    );
    let manifest_start = HEADER_BYTES;
    let manifest_end = manifest_start
        .checked_add(manifest_len)
        .context("the manifest length overflows")?;
    ensure!(
        manifest_end <= bytes.len(),
        "the manifest extends past the end of the file"
    );
    let manifest_bytes = &bytes[manifest_start..manifest_end];
    ensure!(
        Sha256::digest(manifest_bytes).as_slice() == manifest_sha,
        "the manifest SHA-256 does not match the header"
    );
    let signature_start = manifest_end;
    let signature_end = signature_start
        .checked_add(sig_len)
        .context("the signature length overflows")?;
    ensure!(
        signature_end <= bytes.len(),
        "the signature extends past the end of the file"
    );
    let signature = &bytes[signature_start..signature_end];

    let entry_count = read_u32_at(bytes, signature_end)? as usize;
    ensure!(
        entry_count <= MAX_ENTRIES,
        "the package has too many entries"
    );
    let mut cursor = signature_end + 4;
    let mut entries = Vec::with_capacity(entry_count);
    let mut names = BTreeMap::new();
    let mut previous_name: Option<Vec<u8>> = None;
    let mut total_raw = 0u64;
    for _ in 0..entry_count {
        let name_len = read_u16_at(bytes, cursor)? as usize;
        cursor += 2;
        ensure!(
            name_len > 0 && name_len <= MAX_NAME_BYTES,
            "invalid entry name length"
        );
        let name_bytes = bytes
            .get(cursor..cursor + name_len)
            .context("an entry name extends past the end of the file")?;
        cursor += name_len;
        let name = std::str::from_utf8(name_bytes)
            .context("an entry name is not UTF-8")?
            .to_owned();
        validate_entry_name(&name)?;
        ensure!(
            name != "wisp.json" && name != "wisp.sig",
            "the manifest and signature are not entries"
        );
        ensure!(
            names.insert(name.clone(), ()).is_none(),
            "duplicate entry name `{name}`"
        );
        if let Some(previous) = &previous_name {
            ensure!(
                previous.as_slice() < name.as_bytes(),
                "entries must be sorted by name"
            );
        }
        previous_name = Some(name.as_bytes().to_vec());

        let offset = read_u64_at(bytes, cursor)?;
        cursor += 8;
        let compressed_len = read_u64_at(bytes, cursor)?;
        cursor += 8;
        let raw_len = read_u64_at(bytes, cursor)?;
        cursor += 8;
        ensure!(
            raw_len <= MAX_ENTRY_BYTES,
            "entry `{name}` exceeds {MAX_ENTRY_BYTES} raw bytes"
        );
        total_raw = total_raw
            .checked_add(raw_len)
            .context("the total raw size overflows")?;
        ensure!(
            total_raw <= MAX_TOTAL_RAW_BYTES,
            "the package exceeds {MAX_TOTAL_RAW_BYTES} raw bytes"
        );
        let sha_bytes = bytes
            .get(cursor..cursor + 32)
            .context("an entry SHA-256 extends past the end of the file")?;
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(sha_bytes);
        cursor += 32;
        let method = Method::from_u8(*bytes.get(cursor).context("an entry method is missing")?)?;
        cursor += 1;
        entries.push(PendingEntry {
            name,
            offset,
            compressed_len,
            raw_len,
            sha256,
            method,
        });
    }

    let payload_start = cursor;
    let payload_len = bytes.len() - payload_start;
    for entry in &entries {
        let offset = usize::try_from(entry.offset).context("an entry offset is too large")?;
        let compressed_len =
            usize::try_from(entry.compressed_len).context("an entry length is too large")?;
        ensure!(
            offset
                .checked_add(compressed_len)
                .is_some_and(|end| end <= payload_len),
            "entry `{}` points past the payload",
            entry.name
        );
    }

    verify_signature_policy(
        manifest_bytes,
        &entries
            .iter()
            .map(|entry| SigningEntry {
                name: entry.name.clone(),
                raw_len: entry.raw_len,
                sha256: entry.sha256,
            })
            .collect::<Vec<_>>(),
        signature,
        signed,
        options,
    )?;

    let manifest: Manifest =
        serde_json::from_slice(manifest_bytes).context("wisp.json is not a valid manifest")?;
    validate_manifest(&manifest)?;

    let mut package_entries = Vec::with_capacity(entries.len());
    for entry in &entries {
        let offset = usize::try_from(entry.offset).context("an entry offset is too large")?;
        let compressed_len =
            usize::try_from(entry.compressed_len).context("an entry length is too large")?;
        let compressed = &bytes[payload_start + offset..payload_start + offset + compressed_len];
        let raw = decompress(compressed, entry.method, entry.raw_len, &entry.name)?;
        ensure!(
            sha256(&raw) == entry.sha256,
            "entry `{}` does not match its SHA-256",
            entry.name
        );
        package_entries.push(PackageEntry {
            name: entry.name.clone(),
            raw,
            compressed: compressed.to_vec(),
            method: entry.method,
            sha256: entry.sha256,
        });
    }
    verify_entry_references(&manifest, &package_entries)?;
    Ok(Package {
        manifest,
        manifest_bytes: manifest_bytes.to_vec(),
        signature: signature.to_vec(),
        signed,
        entries: package_entries,
    })
}

fn verify_signature_policy(
    manifest_bytes: &[u8],
    entries: &[SigningEntry],
    signature: &[u8],
    signed: bool,
    options: &VerifyOptions,
) -> Result<()> {
    if options.skip_signature {
        return Ok(());
    }
    if signed {
        let key = options
            .public_key
            .as_deref()
            .context("a trusted public key is required to verify this package")?;
        return verify_signature(manifest_bytes, entries, signature, key);
    }
    ensure!(
        options.allow_unsigned,
        "the package is unsigned; pass --dev to accept an unsigned side-load"
    );
    Ok(())
}

fn verify_signature(
    manifest_bytes: &[u8],
    entries: &[SigningEntry],
    signature: &[u8],
    public_key: &[u8],
) -> Result<()> {
    ensure!(
        signature.len() == 64,
        "the Ed25519 signature must be 64 bytes"
    );
    let public_key = load_public_key(public_key)?;
    let digest = signing_digest(manifest_bytes, entries);
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(&digest, signature)
        .map_err(|error| anyhow::anyhow!("the package signature is invalid: {error:?}"))?;
    Ok(())
}

fn signing_digest(manifest_bytes: &[u8], entries: &[SigningEntry]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(Sha256::digest(manifest_bytes));
    let mut sorted: Vec<&SigningEntry> = entries.iter().collect();
    sorted.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
    for entry in sorted {
        hasher.update((entry.name.len() as u16).to_le_bytes());
        hasher.update(entry.name.as_bytes());
        hasher.update(entry.raw_len.to_le_bytes());
        hasher.update(entry.sha256);
    }
    hasher.finalize().into()
}

fn verify_entry_references(manifest: &Manifest, entries: &[PackageEntry]) -> Result<()> {
    ensure!(
        entries.iter().any(|entry| entry.name == manifest.entry),
        "the manifest entry `{}` is missing",
        manifest.entry
    );
    if let Some(icon) = &manifest.assets.icon {
        validate_entry_name(icon)?;
        ensure!(
            entries.iter().any(|entry| entry.name == *icon),
            "the manifest icon `{icon}` is missing"
        );
    }
    Ok(())
}

fn validate_manifest(manifest: &Manifest) -> Result<()> {
    validate_token("id", &manifest.id)?;
    validate_token("slug", &manifest.slug)?;
    ensure!(
        manifest.abi.major == ABI_MAJOR,
        "the plugin needs ABI {}, but this app supports {ABI_MAJOR}",
        manifest.abi.major
    );
    Version::parse(&manifest.version)
        .with_context(|| format!("`{}` is not a semantic version", manifest.version))?;
    Version::parse(&manifest.min_app_version).with_context(|| {
        format!(
            "`{}` is not a semantic app version",
            manifest.min_app_version
        )
    })?;
    validate_entry_name(&manifest.entry)?;
    ensure!(
        !manifest.declares.kind.is_empty()
            && manifest.declares.kind.iter().all(|kind| kind == "provider"),
        "v1 only supports provider plugins"
    );
    ensure!(
        !manifest.network.hosts.is_empty(),
        "a provider must declare at least one network host"
    );
    ensure!(
        !manifest.network.schemes.is_empty(),
        "a provider must declare at least one network scheme"
    );
    for scheme in &manifest.network.schemes {
        ensure!(
            scheme == "http" || scheme == "https",
            "network scheme `{scheme}` is not supported"
        );
    }
    for host in &manifest.network.hosts {
        validate_manifest_host(host)?;
    }
    ensure!(
        manifest.network.redirects <= MAX_REDIRECTS,
        "network.redirects must be at most {MAX_REDIRECTS}"
    );
    ensure!(
        manifest.network.max_body_bytes > 0 && manifest.network.max_body_bytes <= MAX_BODY_BYTES,
        "network.max_body_bytes must be between 1 and {MAX_BODY_BYTES}"
    );
    ensure!(
        manifest.storage.max_bytes > 0,
        "storage.max_bytes must be positive"
    );
    for key in &manifest.secrets {
        validate_token("secret name", key)?;
    }
    if let Some(homepage) = &manifest.homepage {
        ensure!(
            homepage.starts_with("https://"),
            "the homepage must use HTTPS"
        );
    }
    Ok(())
}

fn validate_manifest_host(host: &str) -> Result<()> {
    ensure!(!host.is_empty(), "a network host must not be empty");
    ensure!(
        !host.contains('*')
            && !host.contains('/')
            && !host.contains('\\')
            && !host.contains('@')
            && !host.contains(':'),
        "network host `{host}` must be an exact DNS name without a scheme, port or wildcard"
    );
    validate_dns_name(host)
}

fn validate_dns_name(host: &str) -> Result<()> {
    ensure!(host.is_ascii(), "network host `{host}` must be ASCII");
    ensure!(
        !host.starts_with('.') && !host.ends_with('.') && !host.contains(".."),
        "network host `{host}` has an empty label"
    );
    for label in host.split('.') {
        ensure!(
            label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
            "network host `{host}` is not a valid DNS name"
        );
    }
    ensure!(
        host != "localhost" && !host.ends_with(".localhost") && !host.ends_with(".local"),
        "network host `{host}` points at a local machine"
    );
    Ok(())
}

fn validate_token(name: &str, value: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{name} must not be empty");
    ensure!(
        value != "." && value != "..",
        "{name} must not be a path segment"
    );
    ensure!(value.len() <= 64, "{name} is too long");
    ensure!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')),
        "{name} `{value}` contains an invalid character"
    );
    Ok(())
}

fn validate_entry_name(name: &str) -> Result<()> {
    ensure!(!name.is_empty(), "an entry name must not be empty");
    ensure!(name.len() <= MAX_NAME_BYTES, "an entry name is too long");
    ensure!(!name.starts_with('/'), "entry `{name}` is absolute");
    ensure!(!name.contains('\\'), "entry `{name}` uses a backslash");
    ensure!(
        !name.bytes().any(|byte| byte < 0x20
            || byte == 0x7f
            || matches!(byte, b':' | b'*' | b'?' | b'"' | b'<' | b'>' | b'|')),
        "entry `{name}` contains a forbidden character"
    );
    for segment in name.split('/') {
        ensure!(
            !segment.is_empty() && segment != "." && segment != "..",
            "entry `{name}` contains an empty, `.` or `..` path segment"
        );
        ensure!(
            !segment.ends_with('.') && !segment.ends_with(' '),
            "entry `{name}` has a segment ending in a dot or space"
        );
        let upper = segment.to_ascii_uppercase();
        let stem = upper.split('.').next().unwrap_or(&upper);
        ensure!(
            !matches!(
                stem,
                "CON"
                    | "PRN"
                    | "AUX"
                    | "NUL"
                    | "COM1"
                    | "COM2"
                    | "COM3"
                    | "COM4"
                    | "COM5"
                    | "COM6"
                    | "COM7"
                    | "COM8"
                    | "COM9"
                    | "LPT1"
                    | "LPT2"
                    | "LPT3"
                    | "LPT4"
                    | "LPT5"
                    | "LPT6"
                    | "LPT7"
                    | "LPT8"
                    | "LPT9"
            ),
            "entry `{name}` uses a reserved Windows device name"
        );
    }
    Ok(())
}

fn collect_entries(dir: &Path) -> Result<Vec<PackageEntry>> {
    let mut files = Vec::new();
    for item in walkdir::WalkDir::new(dir).follow_links(false) {
        let item = item.with_context(|| format!("cannot walk {}", dir.display()))?;
        if item.file_type().is_symlink() {
            bail!(
                "symbolic links are not allowed in a plugin package: {}",
                item.path().display()
            );
        }
        if !item.file_type().is_file() {
            continue;
        }
        let name = relative_name(dir, item.path())?;
        if name == "wisp.json" || name == "wisp.sig" {
            continue;
        }
        files.push((name, item.path().to_path_buf()));
    }
    files.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    let mut total_raw = 0u64;
    let mut entries = Vec::with_capacity(files.len());
    for (name, path) in files {
        let raw = fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
        let raw_len = raw.len() as u64;
        ensure!(
            raw_len <= MAX_ENTRY_BYTES,
            "entry `{name}` exceeds {MAX_ENTRY_BYTES} raw bytes"
        );
        total_raw = total_raw
            .checked_add(raw_len)
            .context("the total raw size overflows")?;
        ensure!(
            total_raw <= MAX_TOTAL_RAW_BYTES,
            "the source tree exceeds {MAX_TOTAL_RAW_BYTES} raw bytes"
        );
        let (method, compressed) = compress(&name, &raw)?;
        entries.push(PackageEntry {
            sha256: sha256(&raw),
            name,
            raw,
            compressed,
            method,
        });
    }
    Ok(entries)
}

fn relative_name(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .with_context(|| format!("{} is outside {}", path.display(), root.display()))?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                let part = part
                    .to_str()
                    .with_context(|| format!("{} is not valid UTF-8", path.display()))?;
                parts.push(part);
            }
            _ => bail!("{} is not a plain relative path", path.display()),
        }
    }
    let name = parts.join("/");
    validate_entry_name(&name)?;
    Ok(name)
}

fn compress(name: &str, raw: &[u8]) -> Result<(Method, Vec<u8>)> {
    if name.ends_with(".wasm") {
        return Ok((Method::Store, raw.to_vec()));
    }
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(9));
    encoder.write_all(raw)?;
    let compressed = encoder.finish()?;
    Ok((Method::Deflate, compressed))
}

fn decompress(compressed: &[u8], method: Method, raw_len: u64, name: &str) -> Result<Vec<u8>> {
    match method {
        Method::Store => {
            ensure!(
                compressed.len() as u64 == raw_len,
                "stored entry `{name}` has the wrong length"
            );
            Ok(compressed.to_vec())
        }
        Method::Deflate => {
            ensure!(
                raw_len <= MAX_ENTRY_BYTES,
                "entry `{name}` exceeds the raw size limit"
            );
            let mut decoder = DeflateDecoder::new(compressed);
            let mut raw = Vec::with_capacity(raw_len.min(1024 * 1024) as usize);
            let read = decoder
                .by_ref()
                .take(raw_len + 1)
                .read_to_end(&mut raw)
                .with_context(|| format!("cannot inflate entry `{name}`"))?;
            ensure!(
                read as u64 == raw_len,
                "entry `{name}` inflates to a different length"
            );
            Ok(raw)
        }
    }
}

fn encode_wisp(package: &Package) -> Result<Vec<u8>> {
    ensure!(
        package.signature.len() == if package.signed { 64 } else { 0 },
        "the package signature has the wrong length"
    );
    ensure!(
        package.manifest_bytes.len() <= MAX_MANIFEST_BYTES,
        "the manifest exceeds the container limit"
    );
    ensure!(
        package.entries.len() <= MAX_ENTRIES,
        "the package has too many entries"
    );
    let mut entries: Vec<&PackageEntry> = package.entries.iter().collect();
    entries.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
    for (index, entry) in entries.iter().enumerate() {
        validate_entry_name(&entry.name)?;
        if index > 0 {
            ensure!(
                entries[index - 1].name.as_bytes() < entry.name.as_bytes(),
                "duplicate entry name `{}`",
                entry.name
            );
        }
        ensure!(
            entry.name.len() <= u16::MAX as usize,
            "an entry name is too long for the container"
        );
        ensure!(
            entry.sha256 == sha256(&entry.raw),
            "entry `{}` has a stale SHA-256",
            entry.name
        );
        match entry.method {
            Method::Store => ensure!(
                entry.compressed == entry.raw,
                "stored entry `{}` is not stored verbatim",
                entry.name
            ),
            Method::Deflate => {}
        }
    }
    let manifest_sha = sha256(&package.manifest_bytes);
    let flags = if package.signed { FLAG_SIGNED } else { 0 };
    let mut out = Vec::new();
    out.extend_from_slice(WISP_MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(package.manifest_bytes.len())
            .context("the manifest is too large")?
            .to_le_bytes(),
    );
    out.extend_from_slice(&manifest_sha);
    out.extend_from_slice(
        &u32::try_from(package.signature.len())
            .context("the signature is too large")?
            .to_le_bytes(),
    );
    out.extend_from_slice(&package.manifest_bytes);
    out.extend_from_slice(&package.signature);
    out.extend_from_slice(
        &u32::try_from(entries.len())
            .context("too many entries")?
            .to_le_bytes(),
    );
    let mut offset = 0u64;
    for entry in &entries {
        out.extend_from_slice(
            &u16::try_from(entry.name.len())
                .context("an entry name is too long")?
                .to_le_bytes(),
        );
        out.extend_from_slice(entry.name.as_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&(entry.compressed.len() as u64).to_le_bytes());
        out.extend_from_slice(&(entry.raw.len() as u64).to_le_bytes());
        out.extend_from_slice(&entry.sha256);
        out.push(entry.method as u8);
        offset = offset
            .checked_add(entry.compressed.len() as u64)
            .context("the payload size overflows")?;
    }
    for entry in &entries {
        out.extend_from_slice(&entry.compressed);
    }
    Ok(out)
}

fn zip_options(method: zip::CompressionMethod) -> zip::write::SimpleFileOptions {
    zip::write::SimpleFileOptions::default()
        .compression_method(method)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644)
}

fn read_limited(reader: &mut impl Read, limit: u64, name: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let read = reader.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        read as u64 <= limit,
        "zip entry `{name}` exceeds {limit} bytes"
    );
    Ok(bytes)
}

fn read_u16_at(bytes: &[u8], cursor: usize) -> Result<u16> {
    let value = bytes
        .get(cursor..cursor + 2)
        .context("a u16 field extends past the end of the file")?;
    Ok(u16::from_le_bytes(value.try_into().expect("two bytes")))
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("the header is long enough"),
    )
}

fn read_u32_at(bytes: &[u8], cursor: usize) -> Result<u32> {
    let value = bytes
        .get(cursor..cursor + 4)
        .context("a u32 field extends past the end of the file")?;
    Ok(u32::from_le_bytes(value.try_into().expect("four bytes")))
}

fn read_u64_at(bytes: &[u8], cursor: usize) -> Result<u64> {
    let value = bytes
        .get(cursor..cursor + 8)
        .context("a u64 field extends past the end of the file")?;
    Ok(u64::from_le_bytes(value.try_into().expect("eight bytes")))
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn private_key_pair(input: &[u8]) -> Result<Ed25519KeyPair> {
    let seed = decode_hex_or_raw(input, 32)?;
    if let Some(seed) = seed {
        return Ed25519KeyPair::from_seed_unchecked(&seed)
            .map_err(|error| anyhow::anyhow!("the Ed25519 seed is invalid: {error:?}"));
    }
    Ed25519KeyPair::from_pkcs8(input).map_err(|error| {
        anyhow::anyhow!("the private key is not PKCS#8 or a 32-byte seed: {error:?}")
    })
}

pub fn load_public_key(input: &[u8]) -> Result<Vec<u8>> {
    let key = decode_hex_or_raw(input, 32)?
        .context("a public key must be 32 raw bytes or 64 hex characters")?;
    Ok(key)
}

fn decode_hex_or_raw(input: &[u8], expected: usize) -> Result<Option<Vec<u8>>> {
    let text = std::str::from_utf8(input).ok().map(str::trim);
    if let Some(text) = text
        && text.len() == expected * 2
        && text.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Ok(Some(hex_decode(text)?));
    }
    Ok((input.len() == expected).then(|| input.to_vec()))
}

fn hex_decode(text: &str) -> Result<Vec<u8>> {
    ensure!(
        text.len().is_multiple_of(2),
        "the hex key has an odd length"
    );
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().as_chunks::<2>().0 {
        let high = hex_value(pair[0])?;
        let low = hex_value(pair[1])?;
        out.push((high << 4) | low);
    }
    Ok(out)
}

fn hex_value(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => bail!("the hex key contains a non-hex character"),
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = temporary_path(path);
    fs::write(&temporary, bytes)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("cannot replace {}", path.display()))?;
    }
    fs::rename(&temporary, path)
        .with_context(|| format!("cannot move {} into place", temporary.display()))?;
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("package");
    path.with_file_name(format!(".{name}.tmp-{}-{stamp}", std::process::id()))
}

/// Installs a verified package into `<root>/<id>/<version>` and points `CURRENT` at it.
pub fn install(package_path: &Path, options: &InstallOptions) -> Result<InstalledPlugin> {
    let package = load(package_path, &options.verify)?;
    let version = Version::parse(&package.manifest.version)
        .with_context(|| format!("`{}` is not a semantic version", package.manifest.version))?;
    let plugin_root = options.plugins_dir.join(&package.manifest.id);
    let current = read_current(&plugin_root);
    if let Some(current) = &current {
        ensure!(
            options.allow_downgrade || version >= *current,
            "refusing to downgrade {} from {current} to {version} without --allow-downgrade",
            package.manifest.id
        );
    }
    #[cfg(feature = "instantiate")]
    instantiate(&package)?;

    let version_dir = plugin_root.join(version.to_string());
    ensure!(
        !version_dir.exists(),
        "{} {version} is already installed and version directories are immutable",
        package.manifest.id
    );
    let staging = plugin_root.join(format!(
        ".staging-{version}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&staging).with_context(|| format!("cannot create {}", staging.display()))?;
    let result = (|| -> Result<()> {
        write_tree(&package, &staging)?;
        fs::create_dir_all(&plugin_root)?;
        fs::rename(&staging, &version_dir).with_context(|| {
            format!(
                "cannot move {} into {}",
                staging.display(),
                version_dir.display()
            )
        })?;
        write_current(&plugin_root, &version)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result?;
    Ok(InstalledPlugin {
        id: package.manifest.id.clone(),
        slug: package.manifest.slug.clone(),
        version: version.to_string(),
        abi: format!(
            "{}.{}",
            package.manifest.abi.major, package.manifest.abi.minor
        ),
        publisher: package.manifest.publisher.name,
        path: version_dir,
    })
}

#[cfg(feature = "instantiate")]
fn instantiate(package: &Package) -> Result<()> {
    let entry = package
        .entry(&package.manifest.entry)
        .with_context(|| format!("the manifest entry `{}` is missing", package.manifest.entry))?;
    let runtime = Runtime::new();
    let mut plugin = runtime
        .load(&entry.raw)
        .with_context(|| format!("{} does not instantiate", package.manifest.id))?;
    plugin
        .describe()
        .with_context(|| format!("{} does not answer describe()", package.manifest.id))?;
    Ok(())
}

fn write_tree(package: &Package, root: &Path) -> Result<()> {
    fs::create_dir_all(root)?;
    fs::write(root.join("wisp.json"), &package.manifest_bytes)?;
    if package.signed {
        fs::write(root.join("wisp.sig"), &package.signature)?;
    }
    for entry in &package.entries {
        validate_entry_name(&entry.name)?;
        let path = entry_path(root, &entry.name)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, &entry.raw).with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(())
}

fn entry_path(root: &Path, name: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for segment in name.split('/') {
        ensure!(
            !segment.is_empty() && segment != "." && segment != "..",
            "entry `{name}` contains an unsafe segment"
        );
        path.push(segment);
    }
    Ok(path)
}

fn read_current(plugin_root: &Path) -> Option<Version> {
    let text = fs::read_to_string(plugin_root.join("CURRENT")).ok()?;
    Version::parse(text.trim()).ok()
}

fn write_current(plugin_root: &Path, version: &Version) -> Result<()> {
    atomic_write(
        &plugin_root.join("CURRENT"),
        format!("{version}\n").as_bytes(),
    )
}

pub fn list(plugins_dir: &Path) -> Result<Vec<InstalledPlugin>> {
    let mut installed = Vec::new();
    let entries = match fs::read_dir(plugins_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(installed),
        Err(error) => return Err(error).context("cannot list the plugins directory"),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let plugin_root = entry.path();
        let Some(version) = read_current(&plugin_root) else {
            continue;
        };
        let version_dir = plugin_root.join(version.to_string());
        let manifest_bytes = match fs::read(version_dir.join("wisp.json")) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        let manifest: Manifest = match serde_json::from_slice(&manifest_bytes) {
            Ok(manifest) => manifest,
            Err(_) => continue,
        };
        installed.push(InstalledPlugin {
            id: manifest.id.clone(),
            slug: manifest.slug.clone(),
            version: version.to_string(),
            abi: format!("{}.{}", manifest.abi.major, manifest.abi.minor),
            publisher: manifest.publisher.name.clone(),
            path: version_dir,
        });
    }
    installed.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(installed)
}

pub fn rollback(plugins_dir: &Path, id: &str) -> Result<InstalledPlugin> {
    validate_token("plugin id", id)?;
    let plugin_root = plugins_dir.join(id);
    let current =
        read_current(&plugin_root).with_context(|| format!("{id} has no installed version"))?;
    let mut versions = Vec::new();
    for entry in fs::read_dir(&plugin_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(version) = Version::parse(&name) else {
            continue;
        };
        if version < current {
            versions.push(version);
        }
    }
    versions.sort();
    let previous = versions
        .pop()
        .with_context(|| format!("{id} has no previous version to roll back to"))?;
    write_current(&plugin_root, &previous)?;
    let version_dir = plugin_root.join(previous.to_string());
    let manifest: Manifest = serde_json::from_slice(&fs::read(version_dir.join("wisp.json"))?)
        .context("the previous version has an invalid wisp.json")?;
    Ok(InstalledPlugin {
        id: manifest.id.clone(),
        slug: manifest.slug.clone(),
        version: previous.to_string(),
        abi: format!("{}.{}", manifest.abi.major, manifest.abi.minor),
        publisher: manifest.publisher.name,
        path: version_dir,
    })
}

struct PendingEntry {
    name: String,
    offset: u64,
    compressed_len: u64,
    raw_len: u64,
    sha256: [u8; 32],
    method: Method,
}
