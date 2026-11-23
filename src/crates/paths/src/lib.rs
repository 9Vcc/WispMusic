//! Where the app keeps its data.
//!
//! The preferred location is a `wisp` folder next to the executable, so a portable install keeps
//! settings, state, cache, vault, plugins and logs together and never needs `%LOCALAPPDATA%`.
//! A first launch copies what an older install left in the platform folders. When the executable
//! folder is not writable (for example under Program Files), every lookup falls back to the
//! platform directories exactly as before.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const FOLDER: &str = "wisp";
const MARKER: &str = ".migrated";

static BASE: OnceLock<PathBuf> = OnceLock::new();

/// Picks the data root once. Returns the portable root when one was chosen. Call this before the
/// logger, the database or any other writer opens a path.
pub fn init() -> Option<PathBuf> {
    if let Some(base) = BASE.get() {
        return Some(base.join(FOLDER));
    }
    let base = portable_base()?;
    let root = base.join(FOLDER);
    migrate(&root);
    let _ = BASE.set(base.clone());
    Some(root)
}

fn portable_base() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut base = exe.parent()?.to_path_buf();
    // A cross-compiled dev run lives in `target/<profile>`; keep its data outside `target/`
    // so `cargo clean` does not take the vault with it.
    if base
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "debug" || name == "release")
        && base
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "target")
        && let Some(root) = base.parent().and_then(|target| target.parent())
    {
        base = root.to_path_buf();
    }
    let root = base.join(FOLDER);
    fs::create_dir_all(&root).ok()?;
    let probe = root.join(format!(".write-test-{}", std::process::id()));
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            Some(base)
        }
        Err(_) => None,
    }
}

/// Copies entries that the portable folder does not already have. Existing portable files win, so
/// a second launch never overwrites newer data with an old platform copy. The platform folders
/// are left untouched as a backup.
fn migrate(root: &Path) {
    let marker = root.join(MARKER);
    if marker.exists() {
        return;
    }
    let mut sources: Vec<PathBuf> = Vec::new();
    for source in [
        dirs::data_local_dir().map(|dir| dir.join(FOLDER)),
        dirs::config_dir().map(|dir| dir.join(FOLDER)),
        dirs::cache_dir().map(|dir| dir.join(FOLDER)),
        dirs::state_dir().map(|dir| dir.join(FOLDER)),
    ]
    .into_iter()
    .flatten()
    {
        if source == root || root.starts_with(&source) || sources.contains(&source) {
            continue;
        }
        if source.is_dir() {
            sources.push(source);
        }
    }
    for source in sources {
        copy_missing(&source, root);
    }
    let _ = fs::write(marker, b"");
}

fn copy_missing(source: &Path, destination: &Path) {
    let Ok(entries) = fs::read_dir(source) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == MARKER || name.to_string_lossy().starts_with(".write-test-") {
            continue;
        }
        let target = destination.join(&name);
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if target.exists() && !target.is_dir() {
                continue;
            }
            if fs::create_dir_all(&target).is_ok() {
                copy_missing(&entry.path(), &target);
            }
        } else if kind.is_file() && !target.exists() {
            let _ = fs::copy(entry.path(), target);
        }
    }
}

fn base() -> Option<PathBuf> {
    BASE.get().cloned()
}

pub fn data_local_dir() -> Option<PathBuf> {
    base().or_else(dirs::data_local_dir)
}

pub fn cache_dir() -> Option<PathBuf> {
    base().or_else(dirs::cache_dir)
}

pub fn config_dir() -> Option<PathBuf> {
    base().or_else(dirs::config_dir)
}

pub fn state_dir() -> Option<PathBuf> {
    base().or_else(|| dirs::state_dir().or_else(dirs::cache_dir))
}

/// The portable `wisp` folder, when one is in use.
pub fn portable_root() -> Option<PathBuf> {
    base().map(|base| base.join(FOLDER))
}