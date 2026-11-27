use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use gpui::{Context, EventEmitter};
use music::MusicProvider;
use semver::Version;
use wasm_provider::{Catalog, PluginHealth};
use wisp_plugin_pack::{VerifyOptions, load as load_package};

use crate::{Outcome, Toasts};

pub enum PluginsEvent {
    Changed {
        providers: Vec<Arc<dyn MusicProvider>>,
    },
}

/// The installed-plugin catalog, kept behind an entity so management actions can refresh the
/// settings list and the live provider list without rebuilding the whole app.
pub struct Plugins {
    dir: PathBuf,
    catalog: Catalog,
}

impl EventEmitter<PluginsEvent> for Plugins {}

impl Plugins {
    pub fn new(dir: PathBuf, catalog: Catalog) -> Self {
        let this = Self { dir, catalog };
        this.register_icons();
        this
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn health(&self, slug: &str) -> Option<wasm_provider::PluginHealth> {
        wasm_provider::runtime::health_for(slug)
    }

    pub fn sign_in_required(&self, slug: &str) -> bool {
        wasm_provider::runtime::sign_in_required_for(slug).unwrap_or(false)
    }

    /// Watches the runtime health of installed slots and surfaces the automatic transitions
    /// once, in the window, instead of only in the settings row.
    pub fn watch(&mut self, cx: &mut Context<Self>) {
        let mut last: HashMap<String, PluginHealth> = HashMap::new();
        for plugin in self.catalog.installed() {
            if let Some(health) = self.health(&plugin.slug) {
                last.insert(plugin.slug.clone(), health);
            }
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = this
                    .update(cx, |this, cx| {
                        let mut alerts = Vec::new();
                        for plugin in this.catalog.installed() {
                            let Some(health) = this.health(&plugin.slug) else {
                                continue;
                            };
                            let changed = last.get(&plugin.slug) != Some(&health);
                            last.insert(plugin.slug.clone(), health.clone());
                            if changed {
                                match health {
                                    PluginHealth::RolledBack => {
                                        alerts.push("toast-plugin-rolled-back")
                                    }
                                    PluginHealth::Quarantined(_) => {
                                        alerts.push("toast-plugin-quarantined")
                                    }
                                    PluginHealth::Active => {}
                                }
                            }
                        }
                        for key in alerts {
                            Toasts::show(Outcome::Warning, key, cx);
                        }
                    })
                    .is_ok();
                if !alive {
                    return;
                }
            }
        })
        .detach();
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let (catalog, providers) = build_catalog(&self.dir);
        self.adopt(catalog, providers, cx);
    }

    fn reload_async(&mut self, cx: &mut Context<Self>) {
        let dir = self.dir.clone();
        cx.spawn(async move |this, cx| {
            let (catalog, providers) = cx
                .background_executor()
                .spawn(async move { build_catalog(&dir) })
                .await;
            this.update(cx, |this, cx| this.adopt(catalog, providers, cx))
                .ok();
        })
        .detach();
    }

    fn adopt(
        &mut self,
        catalog: Catalog,
        providers: Vec<Arc<dyn MusicProvider>>,
        cx: &mut Context<Self>,
    ) {
        self.catalog = catalog;
        self.register_icons();
        cx.notify();
        cx.emit(PluginsEvent::Changed { providers });
    }

    fn register_icons(&self) {
        for plugin in self.catalog.installed() {
            let Some(asset) = plugin.icon_asset else {
                continue;
            };
            if let Some(bytes) = &plugin.icon_bytes {
                icons::register(asset, bytes.clone());
                continue;
            }
            let Some(file) = plugin.icon_file.as_deref() else {
                continue;
            };
            match fs::read(file) {
                Ok(bytes) => icons::register(asset, bytes),
                Err(error) => log::warn!(
                    "plugins: cannot read the icon for {} at {}: {error}",
                    plugin.slug,
                    file.display()
                ),
            }
        }
    }

    pub fn rollback(&mut self, slug: &str, cx: &mut Context<Self>) -> Result<String> {
        let plugin = self
            .catalog
            .find(slug)
            .cloned()
            .with_context(|| format!("{slug} is not installed"))?;
        let version = match plugin.package {
            Some(_) => wasm_provider::rollback_flat(&self.dir, &plugin.id)?,
            None => wasm_provider::rollback(&self.dir, slug)?,
        };
        self.reload_async(cx);
        Ok(version)
    }

    pub fn uninstall(&mut self, slug: &str, cx: &mut Context<Self>) -> Result<()> {
        let plugin = self
            .catalog
            .find(slug)
            .cloned()
            .with_context(|| format!("{slug} is not installed"))?;
        if plugin.package.is_some() {
            wasm_provider::uninstall_flat(&self.dir, &plugin.id)?;
        }
        let legacy = self.dir.join(&plugin.slug);
        if legacy.exists() {
            wasm_provider::uninstall(&self.dir, &plugin.slug)?;
        }
        self.reload_async(cx);
        Ok(())
    }

    pub fn set_enabled(&mut self, slug: &str, enabled: bool, cx: &mut Context<Self>) -> Result<()> {
        let plugin = self
            .catalog
            .find(slug)
            .cloned()
            .with_context(|| format!("{slug} is not installed"))?;
        if plugin.package.is_some() {
            wasm_provider::set_flat_enabled(&self.dir, &plugin.id, enabled)?;
        } else {
            wasm_provider::set_enabled(&self.dir, slug, enabled)?;
        }
        self.reload_async(cx);
        Ok(())
    }

    /// Copies a `.wisp` package into the plugin root without extracting it. The container,
    /// manifest, ABI and every entry digest are still validated; only the publisher signature is
    /// skipped. `<id>.current` selects the active flat version.
    pub fn install(&mut self, package: &Path, cx: &mut Context<Self>) -> Result<()> {
        let known = known_versions(&self.catalog);
        stage_package(&self.dir, package, &known)?;
        let (catalog, providers) = build_catalog(&self.dir);
        self.adopt(catalog, providers, cx);
        Ok(())
    }

    /// The drag-and-drop and file-picker path. Package validation, copying and provider
    /// construction all run off the GPUI thread, one package at a time so two packages for the
    /// same service cannot race over its `.current` sidecar; only the catalog swap and the
    /// summary toast return to it.
    pub fn install_many_async(&mut self, packages: Vec<PathBuf>, cx: &mut Context<Self>) {
        if packages.is_empty() {
            return;
        }
        let dir = self.dir.clone();
        let known = known_versions(&self.catalog);
        cx.spawn(async move |this, cx| {
            let prepared = cx
                .background_executor()
                .spawn(async move {
                    let mut known = known;
                    let mut installed = 0usize;
                    let mut failed = 0usize;
                    let mut first_error = None;
                    for package in packages {
                        match stage_package(&dir, &package, &known) {
                            Ok((slug, version)) => {
                                installed += 1;
                                known.retain(|(known, _)| known != &slug);
                                known.push((slug, version));
                                log::info!("plugins: installed {}", package.display());
                            }
                            Err(error) => {
                                failed += 1;
                                let detail = format!("{error:#}");
                                log::warn!(
                                    "plugins: cannot install {}: {detail}",
                                    package.display()
                                );
                                first_error.get_or_insert(detail);
                            }
                        }
                    }
                    (build_catalog(&dir), installed, failed, first_error)
                })
                .await;
            let ((catalog, providers), installed, failed, first_error) = prepared;
            this.update(cx, |this, cx| {
                this.adopt(catalog, providers, cx);
                match (installed, failed) {
                    (_, 0) => Toasts::show(Outcome::Done, "toast-plugin-installed", cx),
                    (0, _) => Toasts::show(
                        Outcome::Failed,
                        install_error_key(first_error.as_deref().unwrap_or_default()),
                        cx,
                    ),
                    _ => Toasts::show(Outcome::Failed, "toast-plugin-installed-partial", cx),
                }
            })
            .ok();
        })
        .detach();
    }
}

fn known_versions(catalog: &Catalog) -> Vec<(String, Version)> {
    catalog
        .installed()
        .filter_map(|plugin| {
            Version::parse(&plugin.version)
                .ok()
                .map(|version| (plugin.slug.clone(), version))
        })
        .collect()
}

fn build_catalog(dir: &Path) -> (Catalog, Vec<Arc<dyn MusicProvider>>) {
    let catalog = Catalog::scan(dir);
    let providers = catalog.providers();
    (catalog, providers)
}

fn stage_package(
    dir: &Path,
    package: &Path,
    known: &[(String, Version)],
) -> Result<(String, Version)> {
    let source = fs::canonicalize(package)
        .with_context(|| format!("cannot resolve {}", package.display()))?;
    let loaded = load_package(
        &source,
        &VerifyOptions {
            public_key: None,
            allow_unsigned: true,
            allow_zip: false,
            skip_signature: true,
        },
    )?;
    let version = Version::parse(&loaded.manifest.version)
        .context("the package version is not a semantic version")?;
    if let Some((_, current)) = known.iter().find(|(slug, _)| slug == &loaded.manifest.slug)
        && version < *current
    {
        bail!(
            "refusing to install {} {} over {current}",
            loaded.manifest.id,
            loaded.manifest.version
        );
    }
    fs::create_dir_all(dir)?;
    let file_name = format!("{}-{}.wisp", loaded.manifest.id, loaded.manifest.version);
    let target = dir.join(&file_name);
    if target.exists() {
        let existing = fs::canonicalize(&target).unwrap_or_else(|_| target.clone());
        if existing != source {
            bail!(
                "{} {} is already installed and version files are immutable",
                loaded.manifest.id,
                loaded.manifest.version
            );
        }
    } else {
        copy_atomic(&source, &target)?;
    }
    atomic_write(
        &dir.join(format!("{}.current", loaded.manifest.id)),
        format!("{file_name}\n").as_bytes(),
    )?;
    Ok((loaded.manifest.slug, version))
}

fn install_error_key(detail: &str) -> &'static str {
    if detail.contains("ABI") {
        "toast-plugin-incompatible"
    } else {
        "toast-plugin-failed"
    }
}

#[cfg(test)]
mod tests {
    use super::install_error_key;

    #[test]
    fn abi_errors_get_the_incompatible_copy() {
        assert_eq!(
            install_error_key("the plugin needs ABI 2, but this app supports 1"),
            "toast-plugin-incompatible"
        );
        assert_eq!(install_error_key("disk full"), "toast-plugin-failed");
    }
}

fn copy_atomic(source: &Path, target: &Path) -> Result<()> {
    let temporary = target.with_file_name(format!(
        ".{}.tmp-{}",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("package"),
        std::process::id()
    ));
    fs::copy(source, &temporary).with_context(|| format!("cannot copy {}", source.display()))?;
    fs::rename(&temporary, target)
        .with_context(|| format!("cannot move {} into place", temporary.display()))?;
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_file_name(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("current"),
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
