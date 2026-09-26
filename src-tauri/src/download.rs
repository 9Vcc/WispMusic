use futures_util::StreamExt;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

/// 下载分块之间的最长空转等待。比流式播放的 CHUNK_IDLE_TIMEOUT（20s）宽松，
/// 避免慢速源上误杀用户主动发起的下载；超过即判定死流并报错，而不是把任务
/// 连同文件句柄永久挂着
const DOWNLOAD_CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// 下载用客户端：只约束连接建立（死地址/DNS 挂起），不设整体 timeout——
/// 整体超时会掐断正常的大文件长下载
fn download_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_default()
}

// ---- 全局下载 Registry ----

pub struct DownloadEntry {
    pub pause_flag: Arc<AtomicBool>,
}

pub type DownloadRegistry = Mutex<HashMap<i64, DownloadEntry>>;

pub fn new_registry() -> DownloadRegistry {
    Mutex::new(HashMap::new())
}

// ---- Event Payloads ----

#[derive(Serialize, Clone)]
pub struct DownloadProgress {
    pub song_id: i64,
    pub downloaded: u64,
    pub total: u64,
    pub speed: u64,
}

#[derive(Serialize, Clone)]
pub struct DownloadComplete {
    pub song_id: i64,
}

#[derive(Serialize, Clone)]
pub struct DownloadPaused {
    pub song_id: i64,
    pub bytes_written: u64,
}

#[derive(Serialize, Clone)]
pub struct DownloadError {
    pub song_id: i64,
    pub message: String,
}

// ---- Tauri Commands ----

/// 获取默认下载目录
#[tauri::command]
pub fn get_default_download_dir() -> Result<String, String> {
    let music_dir = dirs::audio_dir().ok_or_else(|| "无法获取用户音乐目录".to_string())?;
    let wisp_dir = music_dir.join("WispMusic");
    Ok(wisp_dir.to_str().unwrap().to_string())
}

/// 确保目录存在，不存在则创建
#[tauri::command]
pub fn ensure_dir_exists(path: String) -> Result<(), String> {
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())
}

/// 流式下载歌曲，支持从指定 offset 续传
/// resume_from: 续传时传入已下载的字节数；首次下载传 0 或 None
#[tauri::command]
pub async fn download_song(
    app: AppHandle,
    state: State<'_, DownloadRegistry>,
    url: String,
    save_path: String,
    song_id: i64,
    resume_from: Option<u64>,
) -> Result<(), String> {
    // 注册 pause flag。同 song_id 已有任务在跑时直接拒绝：两条任务会同时写
    // 同一个 save_path（写坏文件），且后插入的 pause_flag 会顶掉旧的，
    // 旧任务从此再也暂停不掉
    let pause_flag = Arc::new(AtomicBool::new(false));
    {
        let mut registry = state.lock().unwrap();
        if registry.contains_key(&song_id) {
            return Err(format!("song_id={song_id} 的下载任务已在进行中"));
        }
        registry.insert(
            song_id,
            DownloadEntry {
                pause_flag: pause_flag.clone(),
            },
        );
    }

    let result = download_song_inner(
        &app,
        &state,
        &url,
        &save_path,
        song_id,
        resume_from.unwrap_or(0),
        &pause_flag,
    )
    .await;

    // 成功、失败、暂停三条路径都要摘掉注册项，否则错误路径上的 registry
    // 只增不减（每条都持有 pause_flag 与字符串）
    state.lock().unwrap().remove(&song_id);
    result
}

async fn download_song_inner(
    app: &AppHandle,
    registry: &DownloadRegistry,
    url: &str,
    save_path: &str,
    song_id: i64,
    offset: u64,
    pause_flag: &AtomicBool,
) -> Result<(), String> {
    let client = download_client();
    let mut request = client.get(url);

    // 续传时加 Range 请求头
    if offset > 0 {
        request = request.header("Range", format!("bytes={}-", offset));
    }

    let response = request.send().await.map_err(|e| e.to_string())?;

    // Content-Length 是剩余字节数，total = offset + 剩余
    let remaining = response.content_length().unwrap_or(0);
    let total = offset + remaining;

    // 确保父目录存在
    if let Some(parent) = std::path::Path::new(save_path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // 续传时追加写，首次下载时新建
    let mut file = if offset > 0 {
        OpenOptions::new()
            .append(true)
            .open(save_path)
            .map_err(|e| e.to_string())?
    } else {
        std::fs::File::create(save_path).map_err(|e| e.to_string())?
    };

    let mut downloaded = offset;
    let mut stream = response.bytes_stream();
    let mut last_emit = Instant::now();
    let mut bytes_since_last = 0u64;

    loop {
        // 死流兜底：分块之间超过空闲上限即报错退出，避免任务与句柄永久挂住
        let next = match tokio::time::timeout(DOWNLOAD_CHUNK_IDLE_TIMEOUT, stream.next()).await {
            Ok(next) => next,
            Err(_) => return Err("下载响应空闲超时，已停止".to_string()),
        };
        let Some(chunk) = next else {
            break;
        };

        // 检查暂停 flag
        if pause_flag.load(Ordering::Relaxed) {
            registry.lock().unwrap().remove(&song_id);
            let _ = app.emit(
                "download-paused",
                DownloadPaused {
                    song_id,
                    bytes_written: downloaded,
                },
            );
            return Ok(());
        }

        let chunk = chunk.map_err(|e| e.to_string())?;
        file.write_all(&chunk).map_err(|e| e.to_string())?;

        downloaded += chunk.len() as u64;
        bytes_since_last += chunk.len() as u64;

        let elapsed = last_emit.elapsed().as_millis();
        if elapsed >= 300 {
            let speed = (bytes_since_last as f64 / elapsed as f64 * 1000.0) as u64;
            let _ = app.emit(
                "download-progress",
                DownloadProgress {
                    song_id,
                    downloaded,
                    total,
                    speed,
                },
            );
            last_emit = Instant::now();
            bytes_since_last = 0;
        }
    }

    let _ = app.emit("download-complete", DownloadComplete { song_id });
    Ok(())
}

/// 暂停下载：设置 pause flag，Rust 下载循环会在下一个 chunk 前检测到并停止
#[tauri::command]
pub fn pause_download(state: State<'_, DownloadRegistry>, song_id: i64) -> Result<(), String> {
    let registry = state.lock().unwrap();
    if let Some(entry) = registry.get(&song_id) {
        entry.pause_flag.store(true, Ordering::Relaxed);
        Ok(())
    } else {
        Err(format!("找不到 song_id={} 的下载任务", song_id))
    }
}
