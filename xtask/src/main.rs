//! One-step release build and Windows installer.
//!
//! Run `cargo build-installer` at the repository root (the alias lives in `.cargo/config.toml`):
//! it builds the release executable, stages it, then runs NSIS. The installer is a per-user
//! setup under `%LOCALAPPDATA%\Programs\WispMusic`, matching the app's exe-adjacent `wisp` data
//! folder. Provider packages are installed by the user, not bundled.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const APP_NAME: &str = "WispMusic";
const APP_EXE: &str = "wisp.exe";

fn main() {
    match std::env::args().nth(1).unwrap_or_default().as_str() {
        "package" => package(),
        _ => {
            eprintln!("usage: cargo build-installer (or cargo run -p xtask --release -- package)");
            std::process::exit(2);
        }
    }
}

fn package() {
    let root = project_root();
    let version = read_version(&root.join("Cargo.toml"));
    let version4 = version4(&version);

    println!("[1/4] cargo build --release");
    run(&root, &cargo(), &["build", "--release"]);

    let exe = root.join("target").join("release").join(APP_EXE);
    assert!(exe.is_file(), "missing release binary: {}", exe.display());
    let stage = root
        .join("target")
        .join("package")
        .join(format!("{APP_NAME}-{version}"));

    println!("[2/4] staging {APP_NAME} {version}");
    if stage.exists() {
        fs::remove_dir_all(&stage).expect("cannot clear the staging folder");
    }
    fs::create_dir_all(&stage).expect("cannot create the staging folder");
    fs::copy(&exe, stage.join(APP_EXE)).expect("cannot stage the executable");

    println!("[3/4] makensis");
    let installer_dir = root.join("target").join("installer");
    fs::create_dir_all(&installer_dir).expect("cannot create target/installer");
    let setup = installer_dir.join(format!("{APP_NAME}-{version}-setup.exe"));
    if setup.exists() {
        fs::remove_file(&setup).expect("cannot clear the previous installer");
    }
    let nsi = root.join("installer").join("wisp.nsi");
    let icon = root.join("src").join("assets").join("windows").join("wisp.ico");
    assert!(nsi.is_file(), "missing installer script: {}", nsi.display());
    assert!(icon.is_file(), "missing installer icon: {}", icon.display());
    run(
        &root,
        &find_makensis().display().to_string(),
        &[
            format!("-DAPP_VERSION={version}"),
            format!("-DAPP_VERSION4={version4}"),
            format!("-DSTAGE_DIR={}", stage.display()),
            format!("-DOUT_FILE={}", setup.display()),
            format!("-DICON_FILE={}", icon.display()),
            nsi.display().to_string(),
        ],
    );

    assert!(setup.is_file(), "installer was not created: {}", setup.display());
    println!("[4/4] done");
    println!("installer: {}", setup.display());
}

/// xtask lives directly under the project root.
fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must sit under the project root")
        .to_path_buf()
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned())
}

/// The workspace version is the only source; the installer script carries a fallback only for a
/// direct `makensis` run.
fn read_version(cargo_toml: &Path) -> String {
    let text = fs::read_to_string(cargo_toml).expect("cannot read Cargo.toml");
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("version = \""))
        .map(|rest| rest.trim_end_matches('"').to_owned())
        .next()
        .expect("Cargo.toml has no version field")
}

/// NSIS wants four numeric parts for its file version.
fn version4(version: &str) -> String {
    let mut parts: Vec<String> = version
        .split('.')
        .take(4)
        .map(|part| {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            if digits.is_empty() { "0".to_owned() } else { digits }
        })
        .collect();
    while parts.len() < 4 {
        parts.push("0".to_owned());
    }
    parts.join(".")
}

fn find_makensis() -> PathBuf {
    if Command::new("makensis")
        .arg("/VERSION")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
    {
        return PathBuf::from("makensis");
    }

    let mut candidates = Vec::new();
    if let Ok(scoop) = std::env::var("SCOOP") {
        candidates.push(PathBuf::from(scoop).join("apps/nsis/current/makensis.exe"));
    }
    candidates.push(PathBuf::from("D:/Scoop/apps/nsis/current/makensis.exe"));
    candidates.push(PathBuf::from("C:/Program Files (x86)/NSIS/makensis.exe"));
    for path in &candidates {
        if path.is_file() {
            return path.clone();
        }
    }
    panic!("cannot find makensis: install NSIS (scoop install nsis)");
}

fn run<S: AsRef<std::ffi::OsStr>>(dir: &Path, program: &str, args: &[S]) {
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|error| panic!("cannot start {program}: {error}"));
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
}