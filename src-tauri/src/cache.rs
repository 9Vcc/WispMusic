use futures_util::StreamExt;
use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
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

fn get_named_cache_dir(app: &AppHandle, name: &str) -> Option<PathBuf> {
    app.path().app_cache_dir().ok().map(|mut p| {
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

fn get_native_cache_dir(app: &AppHandle) -> Option<PathBuf> {
    get_named_cache_dir(app, "native_audio")
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
        eprintln!("[cache] 启动清扫孤儿临时文件 {removed} 个");
    }
}
