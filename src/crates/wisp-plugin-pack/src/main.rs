use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use wisp_plugin_pack::{
    InstallOptions, VerifyOptions, install, list, load, pack_dir, rollback, write_wisp,
};

#[derive(Parser)]
#[command(
    name = "wisp-plugin-pack",
    version,
    about = "Pack, verify and install WispMusic provider plugins"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate an Ed25519 signing key pair for a plugin package.
    Keygen {
        /// Directory that receives plugin-key.bin and plugin-key.pub.
        #[arg(long)]
        out: PathBuf,
        /// Replace the files when they already exist.
        #[arg(long)]
        force: bool,
    },
    /// Build a `.wisp` from a source tree.
    Pack {
        /// Directory containing wisp.json and the plugin files.
        dir: PathBuf,
        /// Output `.wisp` path.
        #[arg(short, long)]
        out: PathBuf,
        /// Ed25519 private key: a 32-byte seed (raw or hex) or a PKCS#8 document.
        #[arg(long)]
        key: Option<PathBuf>,
        /// Build an unsigned development package. Never use this for distribution.
        #[arg(long, conflicts_with = "key")]
        unsigned: bool,
    },
    /// Check the container, signature, manifest and every entry digest.
    Verify {
        package: PathBuf,
        /// Trusted Ed25519 public key (32 raw bytes or 64 hex characters).
        #[arg(long)]
        pubkey: Option<PathBuf>,
        /// Accept zip side-loads and unsigned packages.
        #[arg(long)]
        dev: bool,
    },
    /// Verify and install a package under <plugins-dir>/<id>/<version>.
    Install {
        package: PathBuf,
        /// Trusted Ed25519 public key (32 raw bytes or 64 hex characters).
        #[arg(long)]
        pubkey: Option<PathBuf>,
        /// Plugin root; defaults to the app's data directory.
        #[arg(long)]
        plugins_dir: Option<PathBuf>,
        /// Accept zip side-loads and unsigned packages.
        #[arg(long)]
        dev: bool,
        /// Allow installing a version older than CURRENT.
        #[arg(long)]
        allow_downgrade: bool,
    },
    /// Print the installed providers and their active versions.
    List {
        #[arg(long)]
        plugins_dir: Option<PathBuf>,
    },
    /// Point CURRENT at the newest version other than the active one.
    Rollback {
        id: String,
        #[arg(long)]
        plugins_dir: Option<PathBuf>,
    },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Keygen { out, force } => {
            let seed_path = out.join("plugin-key.bin");
            let public_path = out.join("plugin-key.pub");
            if !force && (seed_path.exists() || public_path.exists()) {
                bail!(
                    "{} already holds a plugin key; pass --force to replace it",
                    out.display()
                );
            }
            fs::create_dir_all(&out)?;
            let mut seed = [0u8; 32];
            SystemRandom::new()
                .fill(&mut seed)
                .map_err(|_| anyhow::anyhow!("the system random source is unavailable"))?;
            let key_pair = Ed25519KeyPair::from_seed_unchecked(&seed)
                .map_err(|error| anyhow::anyhow!("cannot build the Ed25519 key: {error:?}"))?;
            fs::write(&seed_path, seed)?;
            fs::write(&public_path, key_pair.public_key().as_ref())?;
            println!("private key -> {}", seed_path.display());
            println!("public key  -> {}", public_path.display());
        }
        Command::Pack {
            dir,
            out,
            key,
            unsigned,
        } => {
            if !unsigned && key.is_none() {
                bail!("packing a distribution package needs --key (or --unsigned for a dev build)");
            }
            let key = key.as_deref().map(read_key).transpose()?;
            let package = pack_dir(&dir, key.as_deref())?;
            write_wisp(&package, &out)?;
            println!(
                "packed {} {} ({} entries, {}) -> {}",
                package.manifest.id,
                package.manifest.version,
                package.entries.len(),
                if package.signed { "signed" } else { "unsigned" },
                out.display()
            );
        }
        Command::Verify {
            package,
            pubkey,
            dev,
        } => {
            let options = VerifyOptions {
                public_key: pubkey.as_deref().map(read_public_key).transpose()?,
                allow_unsigned: dev,
                allow_zip: dev,
                skip_signature: false,
            };
            let package = load(&package, &options)?;
            println!(
                "verified {} {} abi={}.{} ({} entries, {})",
                package.manifest.id,
                package.manifest.version,
                package.manifest.abi.major,
                package.manifest.abi.minor,
                package.entries.len(),
                if package.signed { "signed" } else { "unsigned" }
            );
        }
        Command::Install {
            package,
            pubkey,
            plugins_dir,
            dev,
            allow_downgrade,
        } => {
            let plugins_dir = plugins_dir.unwrap_or_else(default_plugins_dir);
            let installed = install(
                &package,
                &InstallOptions {
                    plugins_dir,
                    verify: VerifyOptions {
                        public_key: pubkey.as_deref().map(read_public_key).transpose()?,
                        allow_unsigned: dev,
                        allow_zip: dev,
                        skip_signature: false,
                    },
                    allow_downgrade,
                },
            )?;
            println!(
                "installed {} {} abi={} -> {}",
                installed.id,
                installed.version,
                installed.abi,
                installed.path.display()
            );
        }
        Command::List { plugins_dir } => {
            let installed = list(&plugins_dir.unwrap_or_else(default_plugins_dir))?;
            if installed.is_empty() {
                println!("no plugins installed");
            }
            for plugin in installed {
                println!(
                    "{}\t{}\tabi={}\t{}\t{}",
                    plugin.id,
                    plugin.version,
                    plugin.abi,
                    plugin.publisher,
                    plugin.path.display()
                );
            }
        }
        Command::Rollback { id, plugins_dir } => {
            let restored = rollback(&plugins_dir.unwrap_or_else(default_plugins_dir), &id)?;
            println!(
                "rolled {} back to {} ({})",
                restored.id,
                restored.version,
                restored.path.display()
            );
        }
    }
    Ok(())
}

fn read_key(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("cannot read the key {}", path.display()))
}

fn read_public_key(path: &Path) -> Result<Vec<u8>> {
    read_key(path)
}

fn default_plugins_dir() -> PathBuf {
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(base).join("wisp").join("plugins");
    }
    if let Some(base) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(base).join("wisp").join("plugins");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("wisp")
            .join("plugins");
    }
    PathBuf::from("plugins")
}
