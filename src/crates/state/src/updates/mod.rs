mod build;
mod install;

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, Result};
use gpui::{Context, Entity, SharedString, Subscription, Task};
use i18n::t;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{AppSettings, Io, join};
use install::Install;

mod updater_pub {
    include!(concat!(env!("OUT_DIR"), "/updater_pub.rs"));
}

/// The endpoint override for local drills, without changing which key a build trusts.
const ENDPOINT_ENV: &str = "WISP_UPDATE_URL";
/// How often a running copy asks the manifest again, so a window left open for a day still
/// hears about a release.
const POLL: Duration = Duration::from_secs(30 * 60);
/// How many leading hex digits of a commit hash the version row shows, as `git` abbreviates it.
const SHORT_COMMIT: usize = 7;
const AGENT: &str = concat!(
    "wisp/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/9Vcc/WispMusic)"
);

/// The builds Wisp updates to. Stable follows tagged releases, and Nightly follows the build
/// of main the nightly workflow publishes every night.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Nightly,
}

impl Channel {
    pub const ALL: [Self; 2] = [Self::Stable, Self::Nightly];

    /// The channel the running build came from.
    pub fn running() -> Self {
        build::Build::current().channel
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Nightly => "nightly",
        }
    }

    /// Reads a stored id. An empty or unknown one means the channel of the running build.
    pub fn from_id(id: &str) -> Self {
        match id {
            "stable" => Self::Stable,
            "nightly" => Self::Nightly,
            _ => Self::running(),
        }
    }

    pub fn label(self) -> SharedString {
        match self {
            Self::Stable => t!("update-channel-stable"),
            Self::Nightly => t!("update-channel-nightly"),
        }
    }

    fn url(self) -> String {
        if let Ok(endpoint) = std::env::var(ENDPOINT_ENV)
            && !endpoint.trim().is_empty()
        {
            return endpoint;
        }
        match self {
            Self::Stable => updater_pub::ENDPOINT.to_owned(),
            Self::Nightly => updater_pub::NIGHTLY_ENDPOINT.to_owned(),
        }
    }
}

/// The package's displayed version, falling back to the compiled version for portable copies.
pub fn version() -> &'static str {
    &build::Build::current().version
}

/// The shortened hash of the commit a nightly was built from, or `None` for a stable release
/// and for a build that does not know its commit.
pub fn commit() -> Option<&'static str> {
    let build = build::Build::current();
    if build.channel != Channel::Nightly {
        return None;
    }
    let commit = build.commit.as_deref()?;
    Some(commit.get(..SHORT_COMMIT).unwrap_or(commit))
}

/// Whether this copy of Wisp installs its own updates, which is when a channel can be chosen.
/// A copy a package manager installed, or a portable one, does not.
pub fn updates_itself() -> bool {
    Install::current().updates_itself()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Release {
    version: String,
    /// The installer this release updates from.
    package: Asset,
    /// The package hash the signature covers, lowercase hex.
    sha256: String,
    signature: String,
}

/// What the last manual check found, for the settings row that asked for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckFeedback {
    Latest,
    Updating,
    Failed,
}

pub struct Updates {
    http: reqwest::Client,
    io: Io,
    task: Option<Task<()>>,
    /// The channel chosen in settings when Wisp last looked, so a different choice looks again.
    channel: Channel,
    /// Whether the check under way was asked for from the settings row.
    manual_check: bool,
    /// Whether a manual check is waiting on an answer.
    checking: bool,
    feedback: Option<CheckFeedback>,
    _poll: Task<()>,
    _settings: Subscription,
}

/// A stable build only follows stable releases now that there is one published channel. A
/// nightly build may still choose between them.
fn channel_for(settings: &Entity<AppSettings>, cx: &mut Context<Updates>) -> Channel {
    match Channel::running() {
        Channel::Stable => Channel::Stable,
        Channel::Nightly => settings.read(cx).update_channel(),
    }
}

impl Updates {
    pub fn new(settings: Entity<AppSettings>, io: Io, cx: &mut Context<Self>) -> Self {
        let channel = channel_for(&settings, cx);
        let watch = cx.observe(&settings, |this: &mut Self, settings, cx| {
            let channel = channel_for(&settings, cx);
            if channel == this.channel {
                return;
            }
            this.channel = channel;
            this.task = None;
            this.manual_check = false;
            this.checking = false;
            this.feedback = None;
            cx.notify();
            this.look(cx);
        });
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this.update(cx, |this, cx| this.look(cx)).is_err() {
                    break;
                }
            }
        });
        let mut updates = Self {
            http: reqwest::Client::builder()
                .dns_resolver(music::dns::dns())
                .build()
                .expect("reqwest client"),
            io,
            task: None,
            channel,
            manual_check: false,
            checking: false,
            feedback: None,
            _poll: poll,
            _settings: watch,
        };
        updates.look(cx);
        updates
    }

    /// Whether a manual check is waiting on an answer, for the settings row's spinner.
    pub fn checking(&self) -> bool {
        self.checking
    }

    /// What the last manual check found.
    pub fn feedback(&self) -> Option<CheckFeedback> {
        self.feedback
    }

    /// Asks the manifest again now, from the settings page.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        if !updates_itself() {
            return;
        }
        self.manual_check = true;
        self.checking = true;
        self.feedback = None;
        cx.notify();
        self.look(cx);
    }

    /// Looks for a newer build and updates to it without asking. A found release is downloaded
    /// and verified, handed to the installer, and this process quits so the installer can put
    /// the new build in place and start it. A failure stays in the log and the next look retries.
    fn look(&mut self, cx: &mut Context<Self>) {
        if self.task.is_some() || !updates_itself() {
            return;
        }
        let channel = self.channel;
        let http = self.http.clone();
        let io = self.io.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let lookup_http = http.clone();
            let found = join(io.spawn(async move { latest(&lookup_http, channel).await })).await;
            this.update(cx, |this, cx| {
                this.task = None;
                this.checking = false;
                let manual = std::mem::take(&mut this.manual_check);
                match found {
                    Ok(Some(release)) => {
                        if manual {
                            this.feedback = Some(CheckFeedback::Updating);
                            cx.notify();
                        }
                        this.install(release, cx);
                    }
                    Ok(None) => {
                        if manual {
                            this.feedback = Some(CheckFeedback::Latest);
                            cx.notify();
                        }
                    }
                    Err(error) => {
                        log::warn!("updates: cannot ask the update source: {error:#}");
                        if manual {
                            this.feedback = Some(CheckFeedback::Failed);
                            cx.notify();
                        }
                    }
                }
            })
            .ok();
        }));
    }

    /// Downloads and verifies `release`, then quietly replaces this build and exits. The update
    /// card is gone: finding a release is already the decision to install it.
    fn install(&mut self, release: Release, cx: &mut Context<Self>) {
        let Release {
            version,
            package,
            sha256,
            signature,
            ..
        } = release;
        log::info!("updates: {version} is available, installing silently");
        let http = self.http.clone();
        let io = self.io.clone();
        let installed = version.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let fetched = join(io.spawn(async move {
                download(&http, &package, &sha256, &signature, &version).await
            }))
            .await;
            let applied = match fetched {
                Ok(path) => join(io.spawn_blocking(move || Install::current().apply(&path))).await,
                Err(error) => Err(error),
            };
            match applied {
                Ok(()) => {
                    log::info!("updates: {installed} handed to the installer, restarting");
                    cx.update(|cx| cx.quit());
                }
                Err(error) => {
                    log::warn!(
                        "updates: cannot install {installed}: {error:#}; the next check retries"
                    );
                    this.update(cx, |this, cx| {
                        this.task = None;
                        this.checking = false;
                        this.manual_check = false;
                        this.feedback = Some(CheckFeedback::Failed);
                        cx.notify();
                    })
                    .ok();
                }
            }
        }));
    }
}

#[derive(Deserialize)]
struct ManifestJson {
    version: String,
    platforms: std::collections::HashMap<String, PlatformJson>,
}

#[derive(Deserialize)]
struct PlatformJson {
    url: String,
    sha256: String,
    #[serde(default)]
    signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Asset {
    name: String,
    url: String,
}

/// The platform section the updater manifest publishes for this build.
fn platform_key() -> Option<&'static str> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Some("windows-x86_64")
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        Some("windows-aarch64")
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Some("darwin-aarch64")
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        Some("darwin-x86_64")
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        Some("linux-x86_64")
    }
    #[cfg(not(any(
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64"),
    )))]
    {
        None
    }
}

/// The Tauri updater's minisign public key, the same one the archived Tauri clients trust. It
/// is the base64 text of the `.pub` file, so a GPUI build can verify the very signatures that
/// update a Tauri install to this one.
const TAURI_PUBLIC_KEY: &str =
    "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDIxOUQ4M0Y3REU3QzE3MEMKUldRTUYzemU5NE9kSWZ2ekcrNHpBRysxZFpuU0FVL2w0NlNCZm1ucnpIUTJYMlNQUXliejlZUDYK";

fn public_key() -> Option<&'static minisign_verify::PublicKey> {
    static KEY: std::sync::OnceLock<Option<minisign_verify::PublicKey>> =
        std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        use base64::Engine as _;
        let text = base64::engine::general_purpose::STANDARD
            .decode(TAURI_PUBLIC_KEY)
            .ok()?;
        let text = std::str::from_utf8(&text).ok()?;
        let key_line = text
            .lines()
            .find(|line| line.starts_with("RWQ"))?
            .trim();
        minisign_verify::PublicKey::from_base64(key_line).ok()
    })
    .as_ref()
}

/// The manifest carries the `.sig` file's base64 text; minisign wants the lines inside it.
fn decode_signature(value: &str) -> Option<String> {
    use base64::Engine as _;
    let value = value.trim();
    if value.starts_with("untrusted comment") {
        return Some(value.to_owned());
    }
    let raw = base64::engine::general_purpose::STANDARD.decode(value).ok()?;
    let raw = String::from_utf8(raw).ok()?;
    raw.starts_with("untrusted comment").then_some(raw)
}

/// Whether `signature` is the publisher's signature over the installer bytes, checked with the
/// Tauri updater's key.
fn signed(data: &[u8], signature: &str) -> bool {
    let Some(public) = public_key() else {
        log::warn!("updates: the Tauri updater public key is not readable");
        return false;
    };
    let Some(raw) = decode_signature(signature) else {
        log::warn!("updates: the manifest signature is not a minisign signature");
        return false;
    };
    let Ok(signature) = minisign_verify::Signature::decode(&raw) else {
        return false;
    };
    // Tauri's signer emits the classic (non-prehashed) form; accept legacy too.
    public.verify(data, &signature, true).is_ok()
}

/// The newest build on `channel`, or `None` when the running build should stay.
async fn latest(http: &reqwest::Client, channel: Channel) -> Result<Option<Release>> {
    let Some(platform) = platform_key() else {
        return Ok(None);
    };
    let endpoint = channel.url();
    let response = http
        .get(&endpoint)
        .header("User-Agent", AGENT)
        .send()
        .await
        .context("cannot reach the update manifest")?
        .error_for_status()
        .context("the update manifest is missing")?;
    let bytes = response
        .bytes()
        .await
        .context("cannot read the update manifest")?;
    let manifest: ManifestJson =
        serde_json::from_slice(&bytes).context("cannot parse the update manifest")?;

    if !wanted(channel, &manifest.version) {
        return Ok(None);
    }
    let Some(platform) = manifest.platforms.get(platform) else {
        log::debug!("updates: the manifest names no build for this platform");
        return Ok(None);
    };
    let sha256 = platform.sha256.trim().to_ascii_lowercase();
    if decode_signature(&platform.signature).is_none() {
        log::warn!("updates: the manifest signature is not a minisign signature");
        return Ok(None);
    }
    let name = platform
        .url
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    Ok(Some(Release {
        version: manifest.version,
        package: Asset {
            name,
            url: platform.url.clone(),
        },
        sha256,
        signature: platform.signature.clone(),
    }))
}

/// Streams the package into the temporary folder while hashing it, and keeps it only when the
/// hash matches the one the signed manifest lists.
async fn download(
    http: &reqwest::Client,
    package: &Asset,
    expected: &str,
    signature: &str,
    version: &str,
) -> Result<PathBuf> {
    let mut response = http
        .get(&package.url)
        .header("User-Agent", AGENT)
        .send()
        .await
        .context("cannot reach the update")?
        .error_for_status()
        .context("the update is missing")?;

    let path = std::env::temp_dir().join(format!("wisp-{version}-{}", package.name));
    let mut file = std::fs::File::create(&path).context("cannot keep the update")?;
    let mut hasher = Sha256::new();
    while let Some(chunk) = response.chunk().await.context("cannot read the update")? {
        hasher.update(&chunk);
        file.write_all(&chunk).context("cannot keep the update")?;
    }
    drop(file);

    let checksum: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if checksum != expected {
        std::fs::remove_file(&path).ok();
        anyhow::bail!("the update does not match its checksum");
    }
    let bytes = std::fs::read(&path).context("cannot read the update for verification")?;
    if !signed(&bytes, signature) {
        std::fs::remove_file(&path).ok();
        anyhow::bail!("the update is not signed by the updater key");
    }
    Ok(path)
}

/// Whether `offered` on `channel` should replace the running build. Within the running build's
/// own channel only a newer build counts. From the other channel any build does, since choosing
/// that channel asks to move to it.
pub(super) fn wanted(channel: Channel, offered: &str) -> bool {
    if channel != Channel::running() {
        return true;
    }
    match (ordered(offered), ordered(version())) {
        (Some(offered), Some(running)) => offered > running,
        _ => false,
    }
}

/// Splits a version into its release numbers and, for a nightly label such as
/// `0.44.2-nightly.20261026`, the date it was built. A stable version has 0 for the date.
fn ordered(version: &str) -> Option<(u64, u64, u64, u64)> {
    let (core, rest) = version
        .trim()
        .split_once(['-', '+'])
        .unwrap_or_else(|| (version.trim(), ""));
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let date = rest
        .strip_prefix("nightly.")
        .and_then(|date| date.parse().ok())
        .unwrap_or(0);
    Some((parts.next()??, parts.next()??, parts.next()??, date))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_carries_the_hash_and_signature() {
        let json = r#"{
            "version": "0.2.1",
            "notes": "- fixed",
            "platforms": {
                "windows-x86_64": {
                    "url": "https://example.com/WispMusic-0.2.1-setup.exe",
                    "sha256": "ABC123",
                    "signature": "c2ln"
                }
            }
        }"#;
        let manifest: ManifestJson = serde_json::from_str(json).expect("a manifest");
        assert_eq!(manifest.version, "0.2.1");
        let platform = manifest.platforms.get("windows-x86_64").expect("a platform");
        assert_eq!(platform.sha256, "ABC123");
        assert!(platform.url.ends_with("-setup.exe"));
    }

    #[test]
    fn a_garbage_signature_is_never_trusted() {
        assert!(!signed(b"hello", "not-base64"));
    }

    #[test]
    fn a_tauri_minisign_signature_verifies() {
        // A throwaway Tauri CLI pair that signed the bytes `hello`.
        let key = "RWTm4V1x1HWKsNMMhf61LGFdEwmLLdzDxi6LhnbsTIyveuE7Cm4ywmUc";
        let signature = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUbTRWMXgxSFdLc0ZQYnNYdE9PejZ6NmR5dWdrRndrcUFXR0VMMWY1UWlXd0x4RG00ODM5dDZGT2liZGUxU2FST0M5OXRSQ1FjTGxVSGlGYk5WSWtvY2dabk9KSTZXY2djPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1MzU5OTMwCWZpbGU6c2FtcGxlLnR4dApnRnZPOVY1anRMNk9CYnVRNjhUblFkN0pVaERvamszSG1QWWlhOFV3S2dRcEhmRG9jWkszY1RPdTZwWFI0cVU5QVRKMnE3L3puTGZtdGljV3prd0pDZz09Cg==";
        let public = minisign_verify::PublicKey::from_base64(key).expect("a public key");
        let raw = decode_signature(signature).expect("a minisign signature");
        let signature = minisign_verify::Signature::decode(&raw).expect("decoded");
        assert!(public.verify(b"hello", &signature, true).is_ok());
        assert!(public.verify(b"hell0", &signature, true).is_err());
    }

    #[test]
    fn only_a_newer_version_is_wanted() {
        assert!(ordered("0.2.2") > ordered("0.2.1"));
        assert!(!wanted(Channel::Stable, "0.0.1"));
    }
}
