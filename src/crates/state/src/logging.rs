use std::path::PathBuf;

const FILE: &str = "wisp.log";

/// The file the running Wisp appends its log to, `wisp.log` under the platform's
/// state folder, or the cache folder where the platform has none. `None` when neither is known.
pub fn log_file() -> Option<PathBuf> {
    let root = paths::state_dir()?;

    Some(root.join("wisp").join(FILE))
}
