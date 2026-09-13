use serde::Serialize;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter, State};

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
    // 注册 pause flag
    let pause_flag = Arc::new(AtomicBool::new(false));
    {
        let mut registry = state.lock().unwrap();
        registry.insert(
            song_id,
            DownloadEntry {
                pause_flag: pause_flag.clone(),
            },
        );
    }

    let offset = resume_from.unwrap_or(0);

    let client = reqwest::Client::new();
    let mut request = client.get(&url);

    // 续传时加 Range 请求头
    if offset > 0 {
        request = request.header("Range", format!("bytes={}-", offset));
    }

    let response = request.send().await.map_err(|e| e.to_string())?;

    // Content-Length 是剩余字节数，total = offset + 剩余
    let remaining = response.content_length().unwrap_or(0);
    let total = offset + remaining;

    // 确保父目录存在
    if let Some(parent) = std::path::Path::new(&save_path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // 续传时追加写，首次下载时新建
    let mut file = if offset > 0 {
        OpenOptions::new()
            .append(true)
            .open(&save_path)
            .map_err(|e| e.to_string())?
    } else {
        std::fs::File::create(&save_path).map_err(|e| e.to_string())?
    };

    let mut downloaded = offset;
    let mut stream = response.bytes_stream();
    let mut last_emit = Instant::now();
    let mut bytes_since_last = 0u64;

    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        // 检查暂停 flag
        if pause_flag.load(Ordering::Relaxed) {
            state.lock().unwrap().remove(&song_id);
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

    state.lock().unwrap().remove(&song_id);
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
