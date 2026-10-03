//! 安装环境下的文件日志。
//!
//! 为什么要在进程里自己做一份：release 构建是 GUI 子系统（见 `main.rs` 的
//! `windows_subsystem = "windows"`），没有控制台，`println!` / `eprintln!` 的输出
//! 全部被丢弃。于是「安装版用户遇到播放卡死」这类问题在现场一点痕迹都留不下——
//! 本次那个 rodio 的 `unreachable!()` panic 就是靠 dev 控制台才抓到的，换成安装版
//! 就只剩一片安静。
//!
//! 目录取 `<exe 所在目录>/logs`：与程序放在一起，用户和开发者都能直接拿到，卸载时
//! 跟着程序一起消失。那里建不出来就回落到系统日志目录（Tauri 的 `app_log_dir`）——
//! 装在 `Program Files` 下又以非管理员运行时，exe 目录确实不可写，所以回落不是
//! 可选优化而是必经分支；最终采用了哪个目录会写进日志的第一行，不用猜。

use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use tauri::{AppHandle, Manager};

/// 应用日志文件名。解灰 sidecar 的 `unm-server.log` 落在同一个目录（见 unm.rs）
pub const FILE_NAME: &str = "wisp.log";

/// 单文件上限：超过就转成 `wisp.log.1`，只留一份历史。
/// 不这么做的话，没人主动清理的机器上日志会一直堆到占满磁盘。
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

struct LogSink {
    directory: PathBuf,
    file: Option<File>,
    written: u64,
    limit_bytes: u64,
}

impl LogSink {
    fn open(directory: &Path, limit_bytes: u64) -> Self {
        let path = directory.join(FILE_NAME);
        let file = OpenOptions::new().create(true).append(true).open(&path).ok();
        let written = file
            .as_ref()
            .and_then(|handle| handle.metadata().ok())
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        Self {
            directory: directory.to_path_buf(),
            file,
            written,
            limit_bytes,
        }
    }

    /// 写一行。逐条 flush：崩溃前最后那几行往往正是最有用的，不能留在缓冲区里。
    fn write_line(&mut self, stamp: &str, level: &str, message: &str) {
        let line = format!("{stamp} [{level}] {message}\n");
        if self.written.saturating_add(line.len() as u64) > self.limit_bytes {
            self.rotate();
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if file.write_all(line.as_bytes()).is_ok() {
            let _ = file.flush();
            self.written += line.len() as u64;
        }
    }

    fn rotate(&mut self) {
        let path = self.directory.join(FILE_NAME);
        let archived = self.directory.join(format!("{FILE_NAME}.1"));
        // 先放掉自己的句柄：Windows 上重命名一个正被自己打开的文件并不总成立
        self.file = None;
        let _ = std::fs::remove_file(&archived);
        let _ = std::fs::rename(&path, &archived);
        // 转不动也照样继续写原文件：宁可单个文件大一点，也不能把日志关掉
        self.file = OpenOptions::new().create(true).append(true).open(&path).ok();
        self.written = self
            .file
            .as_ref()
            .and_then(|handle| handle.metadata().ok())
            .map(|metadata| metadata.len())
            .unwrap_or(0);
    }
}

static SINK: OnceLock<Mutex<Option<LogSink>>> = OnceLock::new();

/// 纯判据：安装目录建得起来就用它，否则用系统日志目录。
/// 拆出来是为了两条分支都能单测覆盖，而不是依赖本机权限。
fn pick_directory(install_writable: bool, install: &Path, fallback: &Path) -> PathBuf {
    if install_writable {
        install.to_path_buf()
    } else {
        fallback.to_path_buf()
    }
}

/// 时间戳。用本地时间（用户报障时说的是墙上时钟，UTC 会逼人多算一次时区）。
/// 只有 `GetLocalTime` 这一个调用，不值得为它引入日期库。
#[cfg(windows)]
fn now_stamp() -> String {
    // windows 0.44 里 GetLocalTime 挂在 SystemInformation 下，返回的 SYSTEMTIME
    // 类型在 Foundation（该 feature 本模块已启用）
    use windows::Win32::System::SystemInformation::GetLocalTime;

    let time = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        time.wYear,
        time.wMonth,
        time.wDay,
        time.wHour,
        time.wMinute,
        time.wSecond,
        time.wMilliseconds
    )
}

#[cfg(not(windows))]
fn now_stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    format!("epoch+{ms}ms")
}

/// 初始化日志（幂等）：决定目录、打开文件、装 panic hook，返回最终目录。
pub fn init(app: &AppHandle) -> PathBuf {
    if let Some(existing) = directory() {
        return existing;
    }

    let install_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.join("logs")))
        .unwrap_or_else(|| PathBuf::from("logs"));
    let install_writable = std::fs::create_dir_all(&install_dir).is_ok();
    let fallback_dir = app
        .path()
        .app_log_dir()
        .unwrap_or_else(|_| install_dir.clone());
    let chosen = pick_directory(install_writable, &install_dir, &fallback_dir);

    if SINK
        .set(Mutex::new(Some(LogSink::open(&chosen, MAX_LOG_BYTES))))
        .is_err()
    {
        return directory().unwrap_or(chosen);
    }

    install_panic_hook();
    log_line(
        "INFO",
        &format!(
            "日志目录：{}{}",
            chosen.display(),
            if install_writable {
                ""
            } else {
                "（安装目录不可写，已回落到系统日志目录）"
            }
        ),
    );
    chosen
}

/// 当前采用的日志目录；未初始化时 None（早期启动阶段与单测里都会走到这条）
pub fn directory() -> Option<PathBuf> {
    SINK.get()?
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|sink| sink.directory.clone())
}

/// 供「查看日志」界面列出的文件信息
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFileInfo {
    /// 文件名（不含路径）——读取时只接受这里出现过的名字
    pub name: String,
    pub size_bytes: u64,
}

/// 目录下的日志文件（`*.log` 与轮转出的 `*.log.1`），按大小倒序：
/// 主日志通常最大排最前，历史与 sidecar 日志跟在后面。
pub fn files() -> Vec<LogFileInfo> {
    match directory() {
        Some(dir) => list_files_in(&dir),
        None => Vec::new(),
    }
}

fn list_files_in(directory: &Path) -> Vec<LogFileInfo> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut list: Vec<LogFileInfo> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".log") && !name.ends_with(".log.1") {
                return None;
            }
            let size_bytes = entry.metadata().ok()?.len();
            Some(LogFileInfo { name, size_bytes })
        })
        .collect();
    list.sort_by(|a, b| {
        b.size_bytes
            .cmp(&a.size_bytes)
            .then_with(|| a.name.cmp(&b.name))
    });
    list
}

/// 尾部读取一次最多读多少字节。日志单文件上限 5MB，整个读进内存去切尾部
/// 是没必要的开销——末尾这么多字节足够覆盖几千行，够看了。
const TAIL_READ_BYTES: u64 = 256 * 1024;

/// 读取某个日志文件的末尾若干行。
///
/// `name` 必须是 `files()` 给出过的名字：这里只接受目录内的既有日志文件名，
/// 不然前端传个 `../../windows.log` 就能读到用户其它文件。
pub fn tail(name: &str, max_lines: usize) -> Result<String, String> {
    let directory = directory().ok_or_else(|| "日志尚未初始化".to_string())?;
    read_tail_in(&directory, name, max_lines)
}

fn read_tail_in(directory: &Path, name: &str, max_lines: usize) -> Result<String, String> {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(format!("不是可读取的日志文件: {name}"));
    }
    if !list_files_in(directory)
        .iter()
        .any(|file| file.name == name)
    {
        return Err(format!("不是可读取的日志文件: {name}"));
    }
    let path = directory.join(name);

    let mut file = File::open(&path).map_err(|err| format!("打开日志失败: {err}"))?;
    let len = file
        .metadata()
        .map_err(|err| format!("读取日志信息失败: {err}"))?
        .len();
    let start = len.saturating_sub(TAIL_READ_BYTES);
    if start > 0 {
        file.seek(SeekFrom::Start(start))
            .map_err(|err| format!("定位日志尾部失败: {err}"))?;
    }
    let mut bytes = Vec::with_capacity((len - start) as usize + 1);
    file.read_to_end(&mut bytes)
        .map_err(|err| format!("读取日志失败: {err}"))?;

    // 截断点大概率落在某一行的中间，丢掉开头那半行再按行数切
    if start > 0 {
        if let Some(break_at) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=break_at);
        }
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    let first = lines.len().saturating_sub(max_lines);
    Ok(lines[first..].join("\n"))
}

/// 写一行日志。dev 下同时打到 stderr，保持原有控制台可见性。
pub fn log_line(level: &str, message: &str) {
    #[cfg(debug_assertions)]
    eprintln!("[{level}] {message}");

    let Some(sink_mutex) = SINK.get() else {
        return;
    };
    let mut guard = sink_mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(sink) = guard.as_mut() else {
        return;
    };
    sink.write_line(&now_stamp(), level, message);
}

/// 把 panic 记进日志文件。
///
/// 这是本模块存在的最大理由：安装版没有控制台，线程 panic（例如音频线程里那次
/// rodio `unreachable!()`）在用户机器上发生完就消失了，连"发生过"都无从证明。
/// 位置信息也一起记——没有 backtrace 时，文件:行号足够定位。
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|at| format!("（{}:{}）", at.file(), at.line()))
            .unwrap_or_default();
        log_line("PANIC", &format!("{info}{location}"));
    }));
}

/// 记一条普通信息
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::logger::log_line("INFO", &format!($($arg)*)) };
}

/// 记一条不影响功能但值得知道的事（降级、跳过、回落）
#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::logger::log_line("WARN", &format!($($arg)*)) };
}

/// 记一条已经影响用户的事（载入失败、设备切换失败、清扫异常）
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => { $crate::logger::log_line("ERROR", &format!($($arg)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "wisp-log-test-{}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp log dir");
        path
    }

    #[test]
    fn install_dir_wins_when_writable_and_falls_back_otherwise() {
        let install = Path::new("D:/app/logs");
        let fallback = Path::new("C:/users/appdata/logs");

        assert_eq!(pick_directory(true, install, fallback), install.to_path_buf());
        assert_eq!(
            pick_directory(false, install, fallback),
            fallback.to_path_buf(),
            "装在 Program Files 且非管理员运行时必须还能有地方写日志"
        );
    }

    #[test]
    fn lines_are_written_and_rotated_once_past_the_limit() {
        let dir = temp_dir("rotate");
        // 上限取得小，才能在不写几十兆的前提下验证轮转。两行短日志约 85 字节，
        // 第三行约 78 字节——所以 120 这个值意味着「恰好写到第三行才越线」。
        let mut sink = LogSink::open(&dir, 120);

        sink.write_line("2026-10-03 16:00:00.000", "INFO", "first line");
        sink.write_line("2026-10-03 16:00:01.000", "INFO", "second line");
        assert!(
            !dir.join(format!("{FILE_NAME}.1")).exists(),
            "没到上限不该改名"
        );

        sink.write_line(
            "2026-10-03 16:00:02.000",
            "ERROR",
            "third line is long enough to cross the limit",
        );

        let current = std::fs::read_to_string(dir.join(FILE_NAME)).expect("当前日志文件应存在");
        let archived =
            std::fs::read_to_string(dir.join(format!("{FILE_NAME}.1"))).expect("轮转后应有历史文件");

        assert!(archived.contains("first line"), "旧内容应进 wisp.log.1");
        assert!(archived.contains("second line"));
        assert!(
            !current.contains("first line") && !current.contains("second line"),
            "转走之后当前文件不该还留着旧内容"
        );
        assert!(current.contains("third line"), "超限那一行本身仍要写得进去");
        assert!(
            sink.written <= 120,
            "轮转后计数要归位，否则之后每行都会再触发一次改名"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotating_twice_keeps_only_one_history_file() {
        let dir = temp_dir("single-history");
        let mut sink = LogSink::open(&dir, 48);

        for i in 0..6 {
            sink.write_line("2026-10-03 16:00:00.000", "INFO", &format!("line {i} padding padding"));
        }

        assert!(dir.join(FILE_NAME).is_file());
        assert!(dir.join(format!("{FILE_NAME}.1")).is_file());
        assert!(
            !dir.join(format!("{FILE_NAME}.2")).exists(),
            "只留一份历史，避免长期无人清理时越堆越多"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn logging_before_init_is_silent_and_never_panics() {
        // 未初始化（或初始化失败）时调用不能把宿主线程带倒：
        // 日志是辅助设施，它自己不能成为新的故障点
        log_line("INFO", "这条在没有任何 sink 的情况下也必须安全");
    }

    #[test]
    fn lists_only_log_files_biggest_first() {
        let dir = temp_dir("listing");
        std::fs::write(dir.join(FILE_NAME), "x".repeat(300)).expect("写主日志");
        std::fs::write(dir.join(format!("{FILE_NAME}.1")), "y".repeat(100)).expect("写历史");
        std::fs::write(dir.join("unm-server.log"), "z".repeat(200)).expect("写 sidecar 日志");
        std::fs::write(dir.join("notes.txt"), "不该出现").expect("写非日志文件");

        let listed = list_files_in(&dir);
        let names: Vec<&str> = listed.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names, vec![FILE_NAME, "unm-server.log", "wisp.log.1"]);
        assert_eq!(listed[0].size_bytes, 300);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_returns_the_last_lines_and_refuses_anything_else() {
        let dir = temp_dir("tail");
        let mut content = String::new();
        for index in 0..500 {
            content.push_str(&format!("line {index}\n"));
        }
        std::fs::write(dir.join(FILE_NAME), &content).expect("写日志");

        let tail = read_tail_in(&dir, FILE_NAME, 3).expect("应能读到尾部");
        assert_eq!(tail.lines().count(), 3);
        assert!(tail.ends_with("line 499"), "取到的应是结尾几行：{tail}");

        // 只允许列出来的那几个名字：否则前端传个 ../ 就能读到用户其它文件
        for rejected in ["../wisp.log", "sub/wisp.log", "..", "notes.txt", "wisp.log.2"] {
            assert!(
                read_tail_in(&dir, rejected, 10).is_err(),
                "{rejected} 不该被允许读取"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
