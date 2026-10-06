use futures_util::StreamExt;
use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, RwLock,
};
use std::time::Duration;
use tauri::{AppHandle, Manager, State};

pub struct CacheState {
    pub downloading: Arc<Mutex<HashSet<String>>>,
}

/// 默认缓存上限（10 GiB，与前端的默认设置一致）。此前默认 `u64::MAX` 表示
/// 「前端设限命令到达前永不清理」，启动早期或设置水合失败时缓存即无上限增长；
/// 现在默认值本身就是兜底，前端随后按用户设置覆盖。
const DEFAULT_AUDIO_CACHE_LIMIT_BYTES: u64 = 10 * 1024 * 1024 * 1024;
static AUDIO_CACHE_LIMIT_BYTES: AtomicU64 =
    AtomicU64::new(DEFAULT_AUDIO_CACHE_LIMIT_BYTES);

/// 用户自定义的缓存根目录（覆盖默认的 `app_cache_dir()`）。
///
/// `None` 表示使用系统默认位置；设置后 `audio_cache` 与 `native_audio` 两个子目录
/// 都挂在它下面。所有缓存路径解析（写入 / 查询 / 统计 / 清理）都经过
/// [`cache_root_dir`]，因此这里切换一次即可让整条链路改道，
/// 不需要改任何调用方。
static CACHE_ROOT_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// 缓存目录信息快照，供设置页展示当前位置与占用。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioCacheDirInfo {
    /// 当前生效的缓存根目录（不含 audio_cache / native_audio 子目录）
    pub path: String,
    /// 系统默认的缓存根目录，供前端展示「恢复默认」目标
    pub default_path: String,
    /// 是否已切换到用户自定义目录
    pub is_custom: bool,
    /// 当前缓存占用总字节数
    pub total_size: u64,
}

struct CacheFile {
    path: PathBuf,
    size: u64,
    accessed: std::time::SystemTime,
}

impl CacheState {
    pub fn new() -> Self {
        Self {
            downloading: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}

/// 读取用户自定义缓存根目录（可克隆的快照）。
///
/// 锁中毒时直接取回内层值：缓存目录解析处在每次播放/校验的热路径上，
/// 一次中毒不应让后续所有缓存操作全部失败。
fn cache_root_override() -> Option<PathBuf> {
    CACHE_ROOT_OVERRIDE
        .read()
        .unwrap_or_else(|err| err.into_inner())
        .clone()
}

/// 缓存根目录：用户自定义优先，否则回落到系统 app cache 目录。
fn cache_root_dir(app: &AppHandle) -> Option<PathBuf> {
    cache_root_override().or_else(|| app.path().app_cache_dir().ok())
}

fn get_named_cache_dir(app: &AppHandle, name: &str) -> Option<PathBuf> {
    cache_root_dir(app).map(|mut p| {
        p.push(name);
        p
    })
}

fn get_cache_dir(app: &AppHandle) -> Option<PathBuf> {
    get_named_cache_dir(app, "audio_cache").map(|p| {
        let _ = std::fs::create_dir_all(&p);
        p
    })
}

/// `native_audio` 原生缓存目录（供 `audio::source` 复用同一份解析逻辑）。
pub(crate) fn native_audio_cache_dir(app: &AppHandle) -> Option<PathBuf> {
    get_named_cache_dir(app, "native_audio").map(|p| {
        let _ = std::fs::create_dir_all(&p);
        p
    })
}

fn get_native_cache_dir(app: &AppHandle) -> Option<PathBuf> {
    native_audio_cache_dir(app)
}

fn get_cache_file_path(app: &AppHandle, song_id: i64, level: &str, ext: &str) -> Option<PathBuf> {
    let mut dir = get_cache_dir(app)?;
    dir.push(format!("{}_{}.{}", song_id, level, ext));
    Some(dir)
}

/// `audio_cache/{songId}_{level}.{ext}`——播放缓存的主键。
///
/// 键里**不能放 URL**：播放地址带签名与时效（`expi`），同一首歌每次解析出来的
/// 字符串都可能不同，用 URL 做键等于每次播放都算作一个全新文件，缓存永远不
/// 命中，也就永远退化成「重新拉整首流」。歌曲 + 音质是稳定且唯一的。
pub(crate) fn song_audio_cache_path(
    app: &AppHandle,
    song_id: i64,
    level: &str,
    url: &str,
) -> Result<PathBuf, String> {
    get_cache_file_path(app, song_id, level, crate::audio::remote_cache_ext(url))
        .ok_or_else(|| "No cache dir".to_string())
}

/// `native_audio/{url hash}.{ext}`——URL 维度的原生缓存。
///
/// 仅用于调用方没给出 songId/level 的情况，以及兼容改造前落下的原生缓存文件。
pub(crate) fn url_audio_cache_path(app: &AppHandle, url: &str) -> Result<PathBuf, String> {
    let mut dir = get_native_cache_dir(app).ok_or("No cache dir")?;
    let _ = std::fs::create_dir_all(&dir);
    dir.push(crate::audio::remote_cache_file_name(url));
    Ok(dir)
}

fn extract_ext(url: &str) -> String {
    crate::audio::remote_cache_ext(url).to_string()
}

fn collect_cache_files(dir: PathBuf, label: &str) -> Result<Vec<CacheFile>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(&dir).map_err(|err| format!("read {label} failed: {err}"))?;
    let mut files = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_file() {
            // 淘汰序的时间键取 accessed 与 modified 的较大者：Windows/NTFS
            // 默认关闭最后访问时间更新，accessed 常年停在文件创建时刻，
            // 单用它等于让淘汰顺序变成 read_dir 的任意序（可能删掉刚播过的歌）；
            // modified 至少反映“何时写入缓存”，取较大者即可在能追踪访问时间的
            // 环境退化成真 LRU，在不能追踪的环境退化成 FIFO
            let accessed = metadata
                .accessed()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let modified = metadata
                .modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            files.push(CacheFile {
                path,
                size: metadata.len(),
                accessed: accessed.max(modified),
            });
        }
    }

    Ok(files)
}

fn legacy_audio_cache_files(app: &AppHandle) -> Result<Vec<CacheFile>, String> {
    let dir = get_cache_dir(app).ok_or("No cache dir")?;
    collect_cache_files(dir, "audio cache")
}

fn all_audio_cache_files(app: &AppHandle) -> Result<Vec<CacheFile>, String> {
    let mut files = legacy_audio_cache_files(app)?;

    if let Some(dir) = get_native_cache_dir(app) {
        files.extend(collect_cache_files(dir, "native audio cache")?);
    }

    Ok(files)
}

fn enforce_files_limit(mut files: Vec<CacheFile>, max_bytes: u64, preserve_path: Option<&Path>) {
    files.sort_by(|a, b| a.accessed.cmp(&b.accessed));

    let mut current_size: u64 = files.iter().map(|file| file.size).sum();

    for file in files {
        if current_size <= max_bytes {
            break;
        }
        if preserve_path.is_some_and(|path| file.path.as_path() == path) {
            continue;
        }
        if std::fs::remove_file(file.path).is_ok() {
            current_size = current_size.saturating_sub(file.size);
        }
    }
}

fn enforce_audio_cache_limit_impl(
    app: &AppHandle,
    max_bytes: u64,
    preserve_path: Option<&Path>,
) -> Result<(), String> {
    enforce_files_limit(all_audio_cache_files(app)?, max_bytes, preserve_path);
    Ok(())
}

pub(crate) fn enforce_configured_audio_cache_limit_excluding(
    app: &AppHandle,
    preserve_path: &Path,
) -> Result<(), String> {
    let max_bytes = AUDIO_CACHE_LIMIT_BYTES.load(Ordering::Relaxed);
    enforce_audio_cache_limit_impl(app, max_bytes, Some(preserve_path))
}

#[tauri::command]
pub fn check_audio_cache(
    app: AppHandle,
    song_id: i64,
    level: String,
    url: String,
) -> Option<String> {
    // 主键：歌曲 + 音质。命中的文件只可能是完整下载后 rename 进来的，
    // 因此不存在「读到半截文件」的情况，无需再查 downloading 标记。
    if let Ok(path) = song_audio_cache_path(&app, song_id, &level, &url) {
        if path.exists() {
            return Some(path.to_string_lossy().to_string());
        }
    }

    // 兼容 URL 维度命名（改造前由原生流式下载落下）：给出 URL 时一并查，
    // 避免这些已经下好的整首文件被判为未缓存而重新拉流。
    if !url.is_empty() {
        if let Ok(path) = url_audio_cache_path(&app, &url) {
            if path.exists() {
                return Some(path.to_string_lossy().to_string());
            }
        }
    }

    None
}

#[tauri::command]
pub async fn cache_audio(
    app: AppHandle,
    state: State<'_, CacheState>,
    song_id: i64,
    level: String,
    url: String,
) -> Result<(), String> {
    let key = format!("{}_{}", song_id, level);

    {
        let mut downloading = state.downloading.lock().unwrap();
        if downloading.contains(&key) {
            return Ok(());
        }
        downloading.insert(key.clone());
    }

    let result: Result<(), String> = async {
        let ext = extract_ext(&url);
        let path = get_cache_file_path(&app, song_id, &level, &ext).ok_or("No cache dir")?;

        if path.exists() {
            return Ok(());
        }

        let temp_path = path.with_extension("tmp");

        // 与播放流共用同一个客户端（连接池复用）；只约束连接建立与分块之间
        // 空转，不设整体超时（会掐断正常的大文件下载）
        let client = crate::http_client::shared_client();
        let res = client.get(&url).send().await.map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            return Err("Failed to download".to_string());
        }

        let mut file = File::create(&temp_path).map_err(|e| e.to_string())?;
        let mut stream = res.bytes_stream();

        loop {
            let next = match tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
                Ok(next) => next,
                Err(_) => {
                    // 必须先关句柄再删：Windows 上打开着的文件删不掉
                    drop(file);
                    let _ = std::fs::remove_file(&temp_path);
                    return Err("缓存下载空闲超时".to_string());
                }
            };
            let Some(chunk) = next else {
                break;
            };
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(err) => {
                    drop(file);
                    let _ = std::fs::remove_file(&temp_path);
                    return Err(err.to_string());
                }
            };
            if let Err(err) = file.write_all(&chunk) {
                drop(file);
                let _ = std::fs::remove_file(&temp_path);
                return Err(err.to_string());
            }
        }

        drop(file);
        std::fs::rename(temp_path, &path).map_err(|e| e.to_string())?;
        enforce_configured_audio_cache_limit_excluding(&app, &path)?;
        Ok(())
    }
    .await;

    state.downloading.lock().unwrap().remove(&key);
    result
}

#[tauri::command]
pub fn get_audio_cache_size(app: AppHandle) -> Result<u64, String> {
    Ok(legacy_audio_cache_files(&app)?
        .iter()
        .map(|file| file.size)
        .sum())
}

#[tauri::command]
pub fn clear_audio_cache(app: AppHandle) -> Result<(), String> {
    for file in legacy_audio_cache_files(&app)? {
        let _ = std::fs::remove_file(file.path);
    }

    Ok(())
}

#[tauri::command]
pub fn enforce_cache_limit(app: AppHandle, max_bytes: u64) -> Result<(), String> {
    enforce_files_limit(legacy_audio_cache_files(&app)?, max_bytes, None);

    Ok(())
}

#[tauri::command]
pub fn enforce_audio_cache_limit(app: AppHandle, max_bytes: u64) -> Result<(), String> {
    enforce_audio_cache_limit_impl(&app, max_bytes, None)
}

#[tauri::command]
pub fn set_audio_cache_limit(app: AppHandle, max_bytes: u64) -> Result<(), String> {
    AUDIO_CACHE_LIMIT_BYTES.store(max_bytes, Ordering::Relaxed);
    enforce_audio_cache_limit_impl(&app, max_bytes, None)
}

/// 当前缓存目录信息（位置 / 是否为自定义 / 占用大小）。
#[tauri::command]
pub fn get_audio_cache_dir(app: AppHandle) -> Result<AudioCacheDirInfo, String> {
    let default_root = app
        .path()
        .app_cache_dir()
        .map_err(|err| format!("resolve app cache dir failed: {err}"))?;
    let custom_root = cache_root_override();
    let root = custom_root.clone().unwrap_or_else(|| default_root.clone());
    let total_size = all_audio_cache_files(&app)?
        .iter()
        .map(|file| file.size)
        .sum();

    Ok(AudioCacheDirInfo {
        path: root.to_string_lossy().to_string(),
        default_path: default_root.to_string_lossy().to_string(),
        is_custom: custom_root.is_some(),
        total_size,
    })
}

/// 切换缓存目录到 `path`。
///
/// 校验（绝对路径 / 可创建 / 可写）通过后立即把覆盖根目录换上，随后尽最大努力
/// 把旧目录里已有的缓存文件迁移过去——迁移只为少让用户重下，失败不阻断切换。
/// 走 `spawn_blocking`：迁移可能涉及 GB 级文件，不能占住异步运行时的工作线程。
#[tauri::command]
pub async fn set_audio_cache_dir(
    app: AppHandle,
    path: String,
) -> Result<AudioCacheDirInfo, String> {
    tauri::async_runtime::spawn_blocking(move || switch_cache_dir_blocking(&app, path.trim()))
        .await
        .map_err(|err| format!("set cache dir task failed: {err}"))?
}

/// 恢复默认缓存目录（同样会尽力把已有缓存迁回去）。
#[tauri::command]
pub async fn reset_audio_cache_dir(app: AppHandle) -> Result<AudioCacheDirInfo, String> {
    tauri::async_runtime::spawn_blocking(move || reset_cache_dir_blocking(&app))
        .await
        .map_err(|err| format!("reset cache dir task failed: {err}"))?
}

/// 一次性清理两个缓存目录，返回本次释放的字节数（供前端给出明确结果反馈）。
#[tauri::command]
pub fn clear_all_audio_cache(app: AppHandle) -> Result<u64, String> {
    let mut freed = 0u64;
    for file in all_audio_cache_files(&app)? {
        if std::fs::remove_file(&file.path).is_ok() {
            freed += file.size;
        }
    }
    Ok(freed)
}

fn cache_subdirs() -> [&'static str; 2] {
    ["audio_cache", "native_audio"]
}

/// 校验目标根目录可用：必须是绝对路径，且两个子目录都能创建并写入。
fn validate_cache_root(root: &Path) -> Result<(), String> {
    if !root.is_absolute() {
        return Err("缓存目录必须是绝对路径".to_string());
    }
    for name in cache_subdirs() {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).map_err(|err| format!("无法创建缓存目录：{err}"))?;
        // 真写一次探针：只创建目录无法排除「目录可建但不可写」（只读盘 / 权限不足）
        let probe = dir.join(".wisp-cache-write-test");
        std::fs::write(&probe, b"wisp")
            .map_err(|err| format!("缓存目录不可写：{err}"))?;
        let _ = std::fs::remove_file(&probe);
    }
    Ok(())
}

/// 把 `from_root` 下两个缓存子目录里的文件搬到 `to_root`，返回迁移文件数。
/// 同名目标已存在则跳过（不覆盖），单个文件失败不影响其余文件。
fn migrate_cache_files(from_root: &Path, to_root: &Path) -> usize {
    let mut moved = 0usize;
    for name in cache_subdirs() {
        let from = from_root.join(name);
        let Ok(entries) = std::fs::read_dir(&from) else {
            continue;
        };
        let to = to_root.join(name);
        let _ = std::fs::create_dir_all(&to);
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let src = entry.path();
            let dst = to.join(entry.file_name());
            if dst.exists() {
                continue;
            }
            // 同一卷用 rename 是零拷贝；跨卷（换盘）会失败，退回复制 + 删除。
            // 复制成功但源文件被占用（正在播放中）删不掉时不计入 migrated：
            // 目标已可读，只是旧位置留了个孤儿，不能对外谎报「已搬完」。
            if std::fs::rename(&src, &dst).is_ok() {
                moved += 1;
            } else if std::fs::copy(&src, &dst).is_ok() && std::fs::remove_file(&src).is_ok() {
                moved += 1;
            }
        }
    }
    moved
}

fn switch_cache_dir_blocking(app: &AppHandle, raw: &str) -> Result<AudioCacheDirInfo, String> {
    if raw.is_empty() {
        return Err("缓存目录不能为空".to_string());
    }

    let new_root = PathBuf::from(raw);
    validate_cache_root(&new_root)?;

    let previous_root = cache_root_dir(app);
    // 先换根：迁移期间新产生的下载直接落到新目录，旧文件随后被搬进来
    {
        let mut guard = CACHE_ROOT_OVERRIDE
            .write()
            .unwrap_or_else(|err| err.into_inner());
        *guard = Some(new_root.clone());
    }

    if let Some(previous) = previous_root {
        if previous != new_root {
            let moved = migrate_cache_files(&previous, &new_root);
            if moved > 0 {
                crate::log_info!("[cache] 切换缓存目录，迁移 {moved} 个缓存文件");
            }
        }
    }

    get_audio_cache_dir(app.clone())
}

fn reset_cache_dir_blocking(app: &AppHandle) -> Result<AudioCacheDirInfo, String> {
    let previous_root = cache_root_dir(app);
    {
        let mut guard = CACHE_ROOT_OVERRIDE
            .write()
            .unwrap_or_else(|err| err.into_inner());
        *guard = None;
    }

    let default_root = app
        .path()
        .app_cache_dir()
        .map_err(|err| format!("resolve app cache dir failed: {err}"))?;
    for name in cache_subdirs() {
        let _ = std::fs::create_dir_all(default_root.join(name));
    }

    if let Some(previous) = previous_root {
        if previous != default_root {
            let moved = migrate_cache_files(&previous, &default_root);
            if moved > 0 {
                crate::log_info!("[cache] 恢复默认缓存目录，迁回 {moved} 个缓存文件");
            }
        }
    }

    get_audio_cache_dir(app.clone())
}

/// 启动清扫孤儿临时文件，两个缓存目录各扫一遍：
/// · `native_audio/*.part-*`：原生音频流式落盘的中间文件（下载中途切歌/进程
///   被强退即残留，此后无人回收）；
/// · `audio_cache/*.tmp`：旧缓存路径的中间文件（写失败或崩溃时残留）。
/// 应用单实例，启动时刻不可能有进行中的下载，遇到即删。
pub(crate) fn cleanup_orphan_temp_files(app: &AppHandle) {
    let mut removed = 0usize;
    for dir in [get_native_cache_dir(app), get_cache_dir(app)]
        .into_iter()
        .flatten()
    {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains(".part-") || name.ends_with(".tmp") {
                if std::fs::remove_file(entry.path()).is_ok() {
                    removed += 1;
                }
            }
        }
    }
    if removed > 0 {
        crate::log_info!("[cache] 启动清扫孤儿临时文件 {removed} 个");
    }
}

#[cfg(test)]
mod cache_dir_tests {
    use super::*;

    /// 生成一个唯一临时目录路径（不创建）。测试并行跑时避免互相踩到同名目录。
    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("wisp-cache-test-{tag}-{nanos}"))
    }

    #[test]
    fn migrate_moves_files_from_both_subdirs() {
        let from = unique_dir("from");
        let to = unique_dir("to");
        std::fs::create_dir_all(from.join("audio_cache")).unwrap();
        std::fs::write(from.join("audio_cache").join("1_l.mp3"), b"a").unwrap();
        std::fs::create_dir_all(from.join("native_audio")).unwrap();
        std::fs::write(from.join("native_audio").join("hash.mp3"), b"b").unwrap();

        assert_eq!(migrate_cache_files(&from, &to), 2);

        // 换目录后必须能在新位置读到原文件（等价于「缓存不丢」）
        assert_eq!(
            std::fs::read(to.join("audio_cache").join("1_l.mp3")).unwrap(),
            b"a"
        );
        assert_eq!(
            std::fs::read(to.join("native_audio").join("hash.mp3")).unwrap(),
            b"b"
        );
        assert!(!from.join("audio_cache").join("1_l.mp3").exists());

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    #[test]
    fn migrate_skips_files_already_present_at_target() {
        let from = unique_dir("from");
        let to = unique_dir("to");
        std::fs::create_dir_all(from.join("audio_cache")).unwrap();
        std::fs::write(from.join("audio_cache").join("same.mp3"), b"new").unwrap();
        std::fs::create_dir_all(to.join("audio_cache")).unwrap();
        std::fs::write(to.join("audio_cache").join("same.mp3"), b"old").unwrap();

        assert_eq!(migrate_cache_files(&from, &to), 0);
        // 目标已有同名文件时不得覆盖（避免把新目录里刚下好的缓存打回旧内容）
        assert_eq!(
            std::fs::read(to.join("audio_cache").join("same.mp3")).unwrap(),
            b"old"
        );

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    #[test]
    fn migrate_is_noop_when_source_missing() {
        let from = unique_dir("missing");
        let to = unique_dir("to");
        assert_eq!(migrate_cache_files(&from, &to), 0);
        let _ = std::fs::remove_dir_all(&to);
    }

    #[test]
    fn validate_rejects_relative_root() {
        assert!(validate_cache_root(Path::new("relative/cache")).is_err());
    }

    #[test]
    fn validate_accepts_writable_absolute_root() {
        let root = unique_dir("writable");
        assert!(validate_cache_root(&root).is_ok());
        // 探针文件必须被清理干净，不留下脏文件
        assert!(!root.join("audio_cache").join(".wisp-cache-write-test").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
