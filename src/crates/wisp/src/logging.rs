use std::borrow::Cow;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use env_logger::{Env, Logger, Target};
use log::{Level, Log, Metadata, Record};

const CONSOLE: &str = "warn,symphonia=error,lofty=error";
const DISK: &str = "warn,symphonia=error,lofty=error,wisp=debug,wisp::memory=info,ui=debug,music=debug,state::home=debug,state::library=debug,state::lyrics=debug,state::playback=info,webview=debug";
const FILTER: &str = "SONORA_LOG";
const PREVIOUS: &str = "wisp.log.1";
const LIMIT: u64 = 16 * 1024 * 1024;
/// How many runs the log keeps, counting the one that is starting.
const SESSIONS: usize = 5;
/// The first line of every run, which is how a later start finds where each run began.
const STARTED: &str = "logging: wisp";

pub fn init() {
    let console = env_logger::Builder::from_env(Env::default().default_filter_or(CONSOLE))
        .format_timestamp(None)
        .format_module_path(false)
        .build();

    let disk = open().map(|file| {
        env_logger::Builder::from_env(Env::new().filter_or(FILTER, DISK))
            .target(Target::Pipe(Box::new(file)))
            .build()
    });

    let (level, logger): (log::LevelFilter, Box<dyn Log>) = match disk {
        None => (console.filter(), Box::new(console)),
        Some(disk) => (
            console.filter().max(disk.filter()),
            Box::new(Fan { console, disk }),
        ),
    };

    if log::set_boxed_logger(Box::new(Redacted(logger))).is_ok() {
        log::set_max_level(level);
    }
    catch_panics();

    log::debug!("{STARTED} {} started", state::version());
}

/// Writes a panic to the log before the default hook prints it. A Wisp started from a
/// desktop entry or a tray has no terminal to lose it to, so this is the only place a crash
/// leaves a trace. `RUST_BACKTRACE` still decides whether there is a backtrace to write.
fn catch_panics() {
    let default = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |panic| {
        // The typeface picker probes each system family for a measurable glyph box, and a broken
        // face makes that panic by design; `catch_unwind` there treats it as "not usable". Those
        // panics are expected, and a full backtrace for each of two hundred fonts would bury the
        // log, so nothing is written for them — not the log line, not the default hook's message.
        if views::probing_typefaces() {
            return;
        }
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed");
        // 强制抓栈：没有设 RUST_BACKTRACE 时也把崩溃位置写进日志，
        // 免得只留下一句 message、定位不到业务代码。
        let backtrace = std::backtrace::Backtrace::force_capture();
        log::error!("panic: {panic} on the {name} thread\n{backtrace}");
        log::logger().flush();
        default(panic);
    }));
}

/// The log file under the size limit: a write that would carry it past `LIMIT` rotates it
/// first and lands in a fresh file, so a flood of lines churns through the two files instead
/// of filling the disk.
struct Capped {
    path: PathBuf,
    file: File,
    written: u64,
}

impl Write for Capped {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let overflowing = self.written + buf.len() as u64 > LIMIT;
        if overflowing && self.written > 0 {
            rotate(&self.path);
            if let Some(file) = append(&self.path) {
                self.file = file;
                self.written = 0;
            }
        }
        let count = self.file.write(buf)?;
        self.written += count as u64;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Whether a record is the expected noise from probing a broken system font.
fn probe_noise(metadata: &Metadata) -> bool {
    views::probing_typefaces()
        && matches!(metadata.level(), Level::Error | Level::Warn)
        && matches!(
            metadata.target(),
            "gpui_windows::direct_write" | "gpui::text_system"
        )
}

/// Passes every record on with [`music::redact`] applied to its message, so neither the terminal
/// nor the file Wisp asks people to attach to a bug report carries a token, an account name or
/// the home folder. This covers the lines other crates write as well as Wisp's own.
struct Redacted(Box<dyn Log>);

impl Log for Redacted {
    fn enabled(&self, metadata: &Metadata) -> bool {
        !probe_noise(metadata) && self.0.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        if probe_noise(record.metadata()) || !self.0.enabled(record.metadata()) {
            return;
        }
        let text = record.args().to_string();
        match music::redact(&text) {
            Cow::Borrowed(_) => self.0.log(record),
            Cow::Owned(clean) => self.0.log(
                &Record::builder()
                    .args(format_args!("{clean}"))
                    .metadata(record.metadata().clone())
                    .module_path(record.module_path())
                    .file(record.file())
                    .line(record.line())
                    .build(),
            ),
        }
    }

    fn flush(&self) {
        self.0.flush();
    }
}

struct Fan {
    console: Logger,
    disk: Logger,
}

impl Log for Fan {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.console.enabled(metadata) || self.disk.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        self.console.log(record);
        self.disk.log(record);
    }

    fn flush(&self) {
        self.console.flush();
        self.disk.flush();
    }
}

fn open() -> Option<Capped> {
    let path = state::log_file()?;
    fs::create_dir_all(path.parent()?).ok()?;
    trim(&path);

    let outgrown = fs::metadata(&path).is_ok_and(|file| file.len() > LIMIT);
    if outgrown {
        rotate(&path);
    }

    let file = append(&path)?;
    let written = file.metadata().map(|file| file.len()).unwrap_or(0);
    Some(Capped {
        path,
        file,
        written,
    })
}

/// Cuts the log down to its last `SESSIONS - 1` runs, so the run that is starting makes
/// `SESSIONS`. Once the log holds that many, everything in `PREVIOUS` is older, so it goes too.
fn trim(path: &Path) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let marker = format!("] {STARTED} ");
    let starts: Vec<usize> = text
        .match_indices(&marker)
        .map(|(at, _)| text[..at].rfind('\n').map_or(0, |line| line + 1))
        .collect();
    let Some(&keep) = starts
        .len()
        .checked_sub(SESSIONS - 1)
        .and_then(|first| starts.get(first))
    else {
        return;
    };
    if let Some(folder) = path.parent() {
        let _ = fs::remove_file(folder.join(PREVIOUS));
    }
    if keep > 0 {
        let _ = fs::write(path, &text[keep..]);
    }
}

fn append(path: &Path) -> Option<File> {
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// Moves the file to `PREVIOUS`, dropping what was there, so however much is written the
/// two files together never exceed twice the limit.
fn rotate(path: &Path) {
    let Some(folder) = path.parent() else { return };
    let _ = fs::rename(path, folder.join(PREVIOUS));
}
