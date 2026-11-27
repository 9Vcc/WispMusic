//! Installed provider plugins and the providers they contribute.
//!
//! The catalog reads both the legacy `<data-dir>/wisp/plugins/<id>/CURRENT/<wisp.json>` layout
//! and flat `<data-dir>/wisp/plugins/*.wisp` packages. A flat package is parsed in memory: the
//! wasm and icon are never extracted to disk. `<id>.current` chooses the active flat version and
//! `<id>.disabled` switches every version of that plugin off. The host ships no streaming
//! provider of its own: an enabled installed plugin whose slug has a host bridge is the only way
//! a streaming provider joins the list.

pub mod runtime;
pub mod wasm_provider;

#[cfg(test)]
mod test_support;

pub use runtime::PluginHealth;

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use semver::Version;
use wisp_plugin_api::{ABI_MAJOR, Manifest};
use wisp_plugin_pack::{VerifyOptions, load as load_package};

use music::MusicProvider;

#[derive(Clone, Debug)]
pub struct PluginInfo {
    pub id: String,
    pub slug: String,
    pub name: &'static str,
    /// The name as the manifest wrote it, translations included, so a surface can show the
    /// active language instead of the default.
    pub name_i18n: wisp_plugin_api::LocalizedText,
    pub version: String,
    pub abi: String,
    pub publisher: String,
    pub icon: Option<String>,
    /// The path the UI gives the image loader: an embedded icon path, or a runtime asset key
    /// for an installed package's own file.
    pub icon_asset: Option<&'static str>,
    /// The installed icon file, which the host registers with the image loader.
    pub icon_file: Option<PathBuf>,
    /// Icon bytes read straight from a flat `.wisp` package.
    pub icon_bytes: Option<Vec<u8>>,
    pub brand: Option<String>,
    pub path: PathBuf,
    /// Set for a flat `.wisp` package; `None` for the legacy version-directory layout.
    pub package: Option<PathBuf>,
    /// Whether the installed plugin is switched on. A `DISABLED` marker (legacy) or
    /// `<id>.disabled` sidecar (flat) keeps it installed while removing it from the provider list.
    pub enabled: bool,
    /// The newest installed version below the active one, for the rollback action.
    pub previous: Option<String>,
}

impl PluginInfo {
    fn from_manifest(manifest: &Manifest, path: PathBuf) -> Self {
        let display = manifest.name.default.trim();
        let display = match display.is_empty() {
            true => manifest.slug.clone(),
            false => display.to_owned(),
        };
        let (icon_asset, icon_file) = icon_asset(manifest, &path);
        let previous = previous_version(&path, &manifest.version);
        let enabled = !path
            .parent()
            .is_some_and(|root| root.join("DISABLED").exists());
        Self {
            id: manifest.id.clone(),
            slug: manifest.slug.clone(),
            name: leak(display),
            name_i18n: manifest.name.clone(),
            version: manifest.version.clone(),
            abi: format!("{}.{}", manifest.abi.major, manifest.abi.minor),
            publisher: manifest.publisher.name.clone(),
            icon: manifest.assets.icon.clone(),
            icon_asset,
            icon_file,
            icon_bytes: None,
            brand: manifest.assets.brand.clone(),
            path,
            package: None,
            enabled,
            previous,
        }
    }

    fn from_package(
        manifest: &Manifest,
        package: PathBuf,
        icon: Option<Vec<u8>>,
        enabled: bool,
        previous: Option<String>,
    ) -> Self {
        let display = manifest.name.default.trim();
        let display = match display.is_empty() {
            true => manifest.slug.clone(),
            false => display.to_owned(),
        };
        let icon_asset = icon.as_ref().map(|_| {
            let name = manifest
                .assets
                .icon
                .as_deref()
                .and_then(|path| path.rsplit('/').next())
                .unwrap_or("icon.svg");
            leak(format!("plugin-icons/{}/{name}", manifest.slug))
        });
        Self {
            id: manifest.id.clone(),
            slug: manifest.slug.clone(),
            name: leak(display),
            name_i18n: manifest.name.clone(),
            version: manifest.version.clone(),
            abi: format!("{}.{}", manifest.abi.major, manifest.abi.minor),
            publisher: manifest.publisher.name.clone(),
            icon: manifest.assets.icon.clone(),
            icon_asset,
            icon_file: None,
            icon_bytes: icon,
            brand: manifest.assets.brand.clone(),
            path: package.clone(),
            package: Some(package),
            enabled,
            previous,
        }
    }
}

fn previous_version(version_dir: &Path, current: &str) -> Option<String> {
    let current = Version::parse(current).ok()?;
    let root = version_dir.parent()?;
    let versions = fs::read_dir(root)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| Version::parse(&entry.file_name().to_string_lossy()).ok());
    newest_below(&current, versions).map(|version| version.to_string())
}

fn newest_below(current: &Version, versions: impl IntoIterator<Item = Version>) -> Option<Version> {
    versions
        .into_iter()
        .filter(|version| version < current)
        .max()
}

fn icon_asset(manifest: &Manifest, path: &Path) -> (Option<&'static str>, Option<PathBuf>) {
    let Some(icon) = manifest.assets.icon.as_deref() else {
        return (None, None);
    };
    let file = path.join(icon);
    let Ok(file) = fs::canonicalize(&file) else {
        return (Some(leak(icon)), None);
    };
    if !file.starts_with(path) {
        return (Some(leak(icon)), None);
    }
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("icon.svg");
    let key = format!("plugin-icons/{}/{}", manifest.slug, name);
    (Some(leak(key)), Some(file))
}

fn leak(value: impl Into<String>) -> &'static str {
    Box::leak(value.into().into_boxed_str())
}

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    plugins: Vec<PluginInfo>,
}

impl Catalog {
    /// Reads the plugin root. A missing directory is an empty catalog, not an error: a user who
    /// never installed a plugin must see exactly the built-in app.
    pub fn scan(dir: &Path) -> Self {
        let mut plugins = Vec::new();
        let mut ids = HashSet::new();
        let mut slugs = HashSet::new();
        for plugin in scan_flat(dir) {
            if !ids.insert(plugin.id.clone()) || !slugs.insert(plugin.slug.clone()) {
                log::warn!("plugins: duplicate flat plugin {} ignored", plugin.id);
                continue;
            }
            plugins.push(plugin);
        }
        for plugin in scan_legacy(dir) {
            if ids.contains(&plugin.id) || slugs.contains(&plugin.slug) {
                log::info!(
                    "plugins: flat package shadows the legacy directory for {}",
                    plugin.id
                );
                continue;
            }
            ids.insert(plugin.id.clone());
            slugs.insert(plugin.slug.clone());
            plugins.push(plugin);
        }
        plugins.sort_by(|left, right| left.slug.cmp(&right.slug));
        Self { plugins }
    }

    pub fn installed(&self) -> impl Iterator<Item = &PluginInfo> {
        self.plugins.iter()
    }

    /// Every row the management list shows.
    pub fn all(&self) -> impl Iterator<Item = &PluginInfo> {
        self.plugins.iter()
    }

    pub fn find(&self, slug: &str) -> Option<&PluginInfo> {
        self.plugins.iter().find(|plugin| plugin.slug == slug)
    }

    /// The row that describes the provider actually in use: an enabled plugin first, then a
    /// disabled one (so a broken install still has a name to show).
    fn provider(&self, slug: &str) -> Option<&PluginInfo> {
        self.plugins
            .iter()
            .find(|plugin| plugin.slug == slug && plugin.enabled)
            .or_else(|| self.find(slug))
    }

    pub fn name(&self, slug: &str) -> Option<&'static str> {
        self.provider(slug).map(|plugin| plugin.name)
    }

    pub fn icon(&self, slug: &str) -> Option<&str> {
        self.provider(slug)?.icon.as_deref()
    }

    pub fn icon_asset(&self, slug: &str) -> Option<&'static str> {
        self.provider(slug)?.icon_asset
    }

    pub fn brand(&self, slug: &str) -> Option<&str> {
        self.provider(slug)?.brand.as_deref()
    }

    /// The providers the app starts with. Only an enabled installed plugin whose slug has a host
    /// bridge joins the list; there is no built-in streaming provider. A plugin whose slug has no
    /// host bridge yet is listed in the catalog but not offered as a provider.
    pub fn providers(&self) -> Vec<Arc<dyn MusicProvider>> {
        let mut providers = Vec::new();
        let mut seen = HashSet::new();
        for plugin in &self.plugins {
            if !plugin.enabled {
                continue;
            }
            if !seen.insert(plugin.slug.as_str()) {
                continue;
            }
            match provider_for(&plugin.slug, plugin.name, plugin.package.as_deref()) {
                Some(provider) => providers.push(provider),
                None => log::warn!("plugins: no host bridge for {}", plugin.slug),
            }
        }
        providers
    }
}

struct FlatEntry {
    manifest: Manifest,
    path: PathBuf,
    icon: Option<Vec<u8>>,
    version: Version,
}

fn flat_verify_options() -> VerifyOptions {
    VerifyOptions {
        public_key: None,
        allow_unsigned: true,
        allow_zip: false,
        skip_signature: true,
    }
}

fn scan_flat(dir: &Path) -> Vec<PluginInfo> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut groups: BTreeMap<String, Vec<FlatEntry>> = BTreeMap::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wisp"))
        {
            continue;
        }
        let package = match load_package(&path, &flat_verify_options()) {
            Ok(package) => package,
            Err(error) => {
                log::warn!("plugins: cannot read {}: {error:#}", path.display());
                continue;
            }
        };
        if package.manifest.abi.major != ABI_MAJOR {
            log::warn!(
                "plugins: {} needs ABI {}, skipping",
                package.manifest.slug,
                package.manifest.abi.major
            );
            continue;
        }
        if package.manifest.slug.is_empty() || package.manifest.id.is_empty() {
            log::warn!("plugins: {} has an empty id or slug", path.display());
            continue;
        }
        let Ok(version) = Version::parse(&package.manifest.version) else {
            log::warn!(
                "plugins: {} has an invalid version {}",
                path.display(),
                package.manifest.version
            );
            continue;
        };
        log::info!(
            "plugins: loaded flat package {} {} from {}",
            package.manifest.slug,
            package.manifest.version,
            path.display()
        );
        let icon = package
            .manifest
            .assets
            .icon
            .as_deref()
            .and_then(|name| package.entry(name))
            .map(|entry| entry.raw.clone());
        groups
            .entry(package.manifest.id.clone())
            .or_default()
            .push(FlatEntry {
                manifest: package.manifest,
                path,
                icon,
                version,
            });
    }

    let mut plugins = Vec::new();
    for (id, mut versions) in groups {
        versions.sort_by(|left, right| left.version.cmp(&right.version));
        let chosen = fs::read_to_string(dir.join(format!("{id}.current")))
            .ok()
            .map(|name| name.trim().to_owned())
            .and_then(|name| {
                versions.iter().position(|entry| {
                    entry
                        .path
                        .file_name()
                        .is_some_and(|file| file.to_string_lossy() == name)
                })
            })
            .unwrap_or_else(|| versions.len().saturating_sub(1));
        let active = versions.remove(chosen.min(versions.len().saturating_sub(1)));
        let previous = versions
            .iter()
            .filter(|entry| entry.version < active.version)
            .max_by(|left, right| left.version.cmp(&right.version))
            .map(|entry| entry.version.to_string());
        let enabled = !dir.join(format!("{id}.disabled")).exists();
        plugins.push(PluginInfo::from_package(
            &active.manifest,
            active.path,
            active.icon,
            enabled,
            previous,
        ));
    }
    plugins
}

fn scan_legacy(dir: &Path) -> Vec<PluginInfo> {
    let mut plugins = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return plugins,
        Err(error) => {
            log::warn!("plugins: cannot read {}: {error}", dir.display());
            return plugins;
        }
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let root = entry.path();
        let version = match fs::read_to_string(root.join("CURRENT")) {
            Ok(version) => version.trim().to_owned(),
            Err(_) => continue,
        };
        if version.is_empty() || version.contains(['/', '\\']) || version.contains("..") {
            log::warn!("plugins: {} has an invalid CURRENT", root.display());
            continue;
        }
        let version_dir = root.join(&version);
        let canonical = match fs::canonicalize(&version_dir) {
            Ok(path) => path,
            Err(_) => continue,
        };
        let root = match fs::canonicalize(&root) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if !canonical.starts_with(&root) {
            log::warn!(
                "plugins: {version} in {} points outside the plugin root",
                root.display()
            );
            continue;
        }
        let manifest_path = canonical.join("wisp.json");
        let bytes = match fs::read(&manifest_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!("plugins: cannot read {}: {error}", manifest_path.display());
                continue;
            }
        };
        let manifest: Manifest = match serde_json::from_slice(&bytes) {
            Ok(manifest) => manifest,
            Err(error) => {
                log::warn!(
                    "plugins: {} has an invalid manifest: {error}",
                    manifest_path.display()
                );
                continue;
            }
        };
        if manifest.abi.major != ABI_MAJOR {
            log::warn!(
                "plugins: {} needs ABI {}, skipping",
                manifest.slug,
                manifest.abi.major
            );
            continue;
        }
        if manifest.slug.is_empty() {
            log::warn!("plugins: {} has an empty slug", manifest_path.display());
            continue;
        }
        if plugins
            .iter()
            .any(|plugin: &PluginInfo| plugin.slug == manifest.slug)
        {
            log::warn!("plugins: duplicate slug {} ignored", manifest.slug);
            continue;
        }
        plugins.push(PluginInfo::from_manifest(&manifest, canonical));
    }
    plugins
}

fn provider_for(
    slug: &str,
    name: &'static str,
    package: Option<&Path>,
) -> Option<Arc<dyn MusicProvider>> {
    let static_slug: &'static str = match slug {
        "netease" => "netease",
        other => Box::leak(other.to_owned().into_boxed_str()),
    };
    package.and_then(|package| {
        match wasm_provider::WasmProvider::from_package(package, static_slug, name) {
            Ok(provider) => Some(provider),
            Err(error) => {
                log::warn!("plugins: cannot create the provider for {slug}: {error:#}");
                None
            }
        }
    })
}

/// `%LOCALAPPDATA%\wisp\plugins` on Windows and the equivalent data directory elsewhere. The
/// pack CLI writes here unless `--plugins-dir` says otherwise.
pub fn plugins_dir() -> PathBuf {
    paths::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("wisp")
        .join("plugins")
}

/// Points `CURRENT` at the newest installed version below the active one.
pub fn rollback(dir: &Path, slug: &str) -> Result<String> {
    let root = plugin_root(dir, slug)?;
    let current =
        read_current(&root).with_context(|| format!("{slug} has no installed version"))?;
    let previous = previous_version(&root.join(current.to_string()), &current.to_string())
        .with_context(|| format!("{slug} has no previous version to roll back to"))?;
    let previous = Version::parse(&previous).context("the previous plugin version is invalid")?;
    write_current(&root, &previous)?;
    Ok(previous.to_string())
}

pub fn set_enabled(dir: &Path, slug: &str, enabled: bool) -> Result<()> {
    let root = plugin_root(dir, slug)?;
    let marker = root.join("DISABLED");
    if enabled {
        match fs::remove_file(&marker) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("cannot remove {}", marker.display())),
        }
    } else {
        fs::write(&marker, b"disabled\n")
            .with_context(|| format!("cannot write {}", marker.display()))
    }
}

/// Removes every installed version and `CURRENT`, keeping the plugin's private `data` directory:
/// an uninstall should not silently take the user's local plugin data with it.
pub fn uninstall(dir: &Path, slug: &str) -> Result<()> {
    let root = plugin_root(dir, slug)?;
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        if entry.file_name() == "data" {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(&path)
                .with_context(|| format!("cannot remove {}", path.display()))?;
        } else {
            fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
        }
    }
    Ok(())
}

/// Flat packages of one plugin, sorted by version.
pub fn flat_versions(dir: &Path, id: &str) -> Result<Vec<(Version, PathBuf)>> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        bail!("`{id}` is not a valid plugin id");
    }
    let entries = fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))?;
    let mut versions = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wisp"))
        {
            continue;
        }
        let Ok(package) = load_package(&path, &flat_verify_options()) else {
            continue;
        };
        if package.manifest.id != id {
            continue;
        }
        if let Ok(version) = Version::parse(&package.manifest.version) {
            versions.push((version, path));
        }
    }
    versions.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(versions)
}

pub fn rollback_flat(dir: &Path, id: &str) -> Result<String> {
    let versions = flat_versions(dir, id)?;
    if versions.is_empty() {
        bail!("{id} has no flat package installed");
    }
    let current = fs::read_to_string(dir.join(format!("{id}.current"))).ok();
    let current = current.as_deref().map(str::trim);
    let active = current
        .and_then(|name| {
            versions.iter().position(|(_, path)| {
                path.file_name()
                    .is_some_and(|file| file.to_string_lossy() == name)
            })
        })
        .unwrap_or(versions.len() - 1);
    let previous = versions[..active]
        .iter()
        .rev()
        .find(|(version, _)| *version < versions[active].0)
        .with_context(|| format!("{id} has no previous flat version"))?;
    let name = previous
        .1
        .file_name()
        .context("the flat package has no file name")?
        .to_string_lossy()
        .into_owned();
    atomic_write(
        &dir.join(format!("{id}.current")),
        format!("{name}\n").as_bytes(),
    )?;
    Ok(previous.0.to_string())
}

pub fn set_flat_enabled(dir: &Path, id: &str, enabled: bool) -> Result<()> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        bail!("`{id}` is not a valid plugin id");
    }
    let marker = dir.join(format!("{id}.disabled"));
    if enabled {
        match fs::remove_file(&marker) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("cannot remove {}", marker.display())),
        }
    } else {
        fs::write(&marker, b"disabled\n")
            .with_context(|| format!("cannot write {}", marker.display()))
    }
}

pub fn uninstall_flat(dir: &Path, id: &str) -> Result<()> {
    for (_, path) in flat_versions(dir, id)? {
        fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
    }
    for suffix in ["current", "disabled"] {
        let path = dir.join(format!("{id}.{suffix}"));
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("cannot remove {}", path.display()));
            }
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_file_name(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("plugins"),
        std::process::id()
    ));
    fs::write(&temporary, bytes)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("cannot replace {}", path.display()))?;
    }
    fs::rename(&temporary, path)
        .with_context(|| format!("cannot move {} into place", temporary.display()))?;
    Ok(())
}

fn plugin_root(dir: &Path, slug: &str) -> Result<PathBuf> {
    if slug.is_empty() || slug.contains(['/', '\\']) || slug.contains("..") {
        bail!("`{slug}` is not a valid plugin slug");
    }
    let base =
        fs::canonicalize(dir).with_context(|| format!("cannot resolve {}", dir.display()))?;
    let root = fs::canonicalize(dir.join(slug))
        .with_context(|| format!("cannot resolve the {slug} plugin directory"))?;
    if !root.starts_with(&base) {
        bail!("the {slug} plugin directory points outside the plugin root");
    }
    Ok(root)
}

fn read_current(root: &Path) -> Option<Version> {
    let text = fs::read_to_string(root.join("CURRENT")).ok()?;
    Version::parse(text.trim()).ok()
}

fn write_current(root: &Path, version: &Version) -> Result<()> {
    let temporary = root.join(format!(".CURRENT.tmp-{}", std::process::id()));
    fs::write(&temporary, format!("{version}\n"))?;
    let current = root.join("CURRENT");
    if current.exists() {
        fs::remove_file(&current)
            .with_context(|| format!("cannot replace {}", current.display()))?;
    }
    fs::rename(&temporary, &current)
        .with_context(|| format!("cannot write {}", current.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(slug: &str) -> Manifest {
        let mut manifest = crate::test_support::manifest(slug);
        manifest.id = slug.to_owned();
        manifest.slug = slug.to_owned();
        manifest.name.default = format!("{slug} display");
        manifest.version = "1.2.3".to_owned();
        manifest.assets.icon = Some(format!("icons/{slug}.svg"));
        manifest.assets.brand = Some("#123456".to_owned());
        manifest
    }

    fn installed(slug: &str) -> PluginInfo {
        PluginInfo::from_manifest(&manifest(slug), PathBuf::from(format!("plugins/{slug}")))
    }

    fn flat_package(dir: &Path, version: &str) -> PathBuf {
        let source = dir.join(format!("source-{version}"));
        fs::create_dir_all(source.join("icons")).expect("the source tree");
        let mut manifest = manifest("netease");
        manifest.version = version.to_owned();
        fs::write(
            source.join("wisp.json"),
            serde_json::to_vec(&manifest).expect("the manifest"),
        )
        .expect("wisp.json");
        fs::write(source.join("lib.wasm"), b"not a wasm module").expect("the entry");
        fs::write(source.join("icons/netease.svg"), b"<svg/>").expect("the icon");
        let package = wisp_plugin_pack::pack_dir(&source, None).expect("the package");
        let path = dir.join(format!("netease-{version}.wisp"));
        wisp_plugin_pack::write_wisp(&package, &path).expect("the flat package");
        path
    }

    #[test]
    fn a_flat_package_becomes_a_catalog_row_with_its_icon() {
        let dir = tempfile::tempdir().expect("a temp dir");
        flat_package(dir.path(), "1.0.0");
        let catalog = Catalog::scan(dir.path());
        let plugin = catalog.find("netease").expect("the flat package is listed");
        assert_eq!(plugin.version, "1.0.0");
        assert!(plugin.enabled);
        assert!(plugin.package.is_some());
        assert_eq!(plugin.icon_asset, Some("plugin-icons/netease/netease.svg"));
        assert_eq!(plugin.icon_bytes.as_deref(), Some(&b"<svg/>"[..]));
        assert_eq!(catalog.providers().len(), 1);
    }

    #[test]
    fn the_current_sidecar_selects_a_flat_version_and_keeps_the_previous_one() {
        let dir = tempfile::tempdir().expect("a temp dir");
        flat_package(dir.path(), "1.0.0");
        let two = flat_package(dir.path(), "2.0.0");
        fs::write(
            dir.path().join("netease.current"),
            format!("{}\n", two.file_name().unwrap().to_string_lossy()),
        )
        .expect("the current sidecar");
        let catalog = Catalog::scan(dir.path());
        let plugin = catalog.find("netease").expect("the plugin");
        assert_eq!(plugin.version, "2.0.0");
        assert_eq!(plugin.previous.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn a_disabled_flat_package_stays_listed_and_stops_its_provider() {
        let dir = tempfile::tempdir().expect("a temp dir");
        flat_package(dir.path(), "1.0.0");
        fs::write(dir.path().join("netease.disabled"), b"disabled\n").expect("the marker");
        let catalog = Catalog::scan(dir.path());
        let plugin = catalog.find("netease").expect("the plugin");
        assert!(!plugin.enabled);
        assert!(catalog.providers().is_empty());
    }

    #[test]
    fn rolling_back_a_flat_package_switches_the_current_sidecar() {
        let dir = tempfile::tempdir().expect("a temp dir");
        flat_package(dir.path(), "1.0.0");
        let two = flat_package(dir.path(), "2.0.0");
        fs::write(
            dir.path().join("netease.current"),
            format!("{}\n", two.file_name().unwrap().to_string_lossy()),
        )
        .expect("the current sidecar");
        assert_eq!(
            rollback_flat(dir.path(), "netease").expect("a rollback"),
            "1.0.0"
        );
        let catalog = Catalog::scan(dir.path());
        assert_eq!(
            catalog.find("netease").expect("the plugin").version,
            "1.0.0"
        );
    }

    #[test]
    fn uninstalling_a_flat_package_removes_its_versions_and_sidecars() {
        let dir = tempfile::tempdir().expect("a temp dir");
        flat_package(dir.path(), "1.0.0");
        flat_package(dir.path(), "2.0.0");
        fs::write(dir.path().join("netease.disabled"), b"disabled\n").expect("the marker");
        uninstall_flat(dir.path(), "netease").expect("the uninstall");
        assert!(Catalog::scan(dir.path()).all().next().is_none());
        assert!(!dir.path().join("netease.current").exists());
        assert!(!dir.path().join("netease.disabled").exists());
    }

    #[test]
    fn a_flat_package_shadows_the_legacy_directory() {
        let dir = tempfile::tempdir().expect("a temp dir");
        flat_package(dir.path(), "2.0.0");
        let legacy = dir.path().join("netease");
        let version = legacy.join("1.0.0");
        fs::create_dir_all(&version).expect("the legacy tree");
        fs::write(legacy.join("CURRENT"), "1.0.0").expect("the legacy current");
        fs::write(
            version.join("wisp.json"),
            serde_json::to_vec(&manifest("netease")).expect("the manifest"),
        )
        .expect("the legacy manifest");
        let catalog = Catalog::scan(dir.path());
        assert_eq!(catalog.all().count(), 1);
        assert_eq!(
            catalog.find("netease").expect("the plugin").version,
            "2.0.0"
        );
    }

    #[test]
    fn a_missing_plugins_directory_is_empty() {
        let catalog = Catalog::scan(Path::new("__wisp_missing_plugins__"));
        assert_eq!(catalog.installed().count(), 0);
        assert!(catalog.providers().is_empty());
    }

    #[test]
    fn manifests_become_catalog_rows() {
        let catalog = Catalog {
            plugins: vec![installed("demo")],
        };
        let plugin = catalog.find("demo").expect("the plugin is listed");
        assert_eq!(plugin.version, "1.2.3");
        assert_eq!(plugin.abi, "1.0");
        assert_eq!(plugin.name, "demo display");
        assert_eq!(plugin.icon.as_deref(), Some("icons/demo.svg"));
        assert_eq!(plugin.icon_asset, Some("icons/demo.svg"));
        assert!(plugin.icon_file.is_none());
        assert_eq!(plugin.brand.as_deref(), Some("#123456"));
        assert!(
            catalog.providers().is_empty(),
            "demo has no host bridge yet"
        );
    }

    #[test]
    fn no_plugins_means_no_streaming_provider() {
        let catalog = Catalog::default();
        assert!(catalog.providers().is_empty());
        assert_eq!(catalog.all().count(), 0);
    }

    #[test]
    fn an_enabled_netease_plugin_becomes_the_provider() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let package = flat_package(dir.path(), "1.0.0");
        let mut plugin = installed("netease");
        plugin.package = Some(package);
        let catalog = Catalog {
            plugins: vec![plugin],
        };
        let providers = catalog.providers();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].slug(), "netease");
        assert_eq!(catalog.all().count(), 1);
    }

    #[test]
    fn a_disabled_plugin_contributes_no_provider() {
        let mut plugin = installed("netease");
        plugin.enabled = false;
        let catalog = Catalog {
            plugins: vec![plugin],
        };
        assert!(catalog.providers().is_empty());
        assert_eq!(
            catalog.all().count(),
            1,
            "the disabled plugin row stays visible"
        );
    }

    #[test]
    fn an_unknown_plugin_is_listed_but_does_not_become_a_provider() {
        let catalog = Catalog {
            plugins: vec![installed("kugou")],
        };
        assert!(catalog.find("kugou").is_some());
        assert!(catalog.providers().is_empty());
    }

    #[test]
    fn the_previous_version_is_the_newest_one_below_current() {
        let versions = ["1.0.0", "1.5.0", "2.0.0"]
            .into_iter()
            .map(|version| Version::parse(version).expect("a version"));
        let current = Version::parse("2.0.0").expect("a version");
        assert_eq!(
            newest_below(&current, versions).map(|version| version.to_string()),
            Some("1.5.0".to_owned())
        );
        let current = Version::parse("1.0.0").expect("a version");
        assert!(newest_below(&current, []).is_none());
    }
}
