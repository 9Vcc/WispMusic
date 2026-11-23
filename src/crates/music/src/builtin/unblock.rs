//! 解灰调度：只走纯 Rust 移植的 unm 引擎（不再使用任何第三方 HTTP 回退源）。
//!
//! 纪律：
//!   1. 只缓存成功且验音通过的结果，TTL 30 分钟、最多 60 条；
//!   2. 每个引擎自带超时，逐个尝试；
//!   3. 直链入库前必须验音（Range 请求看魔数 + 按时长粗校体积），
//!      避免第三方把 HTML / 试听片段 / 损坏 mp4 当音频交给我们。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::unm::{self, TrackInfo};
use crate::Quality;

const AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
/// A stream below this size is a spoken notice or a trial clip even when the track's duration
/// is not known yet; the Kuwo free preview is about 181 KiB.
const PLACEHOLDER_BYTES: u64 = 256 * 1024;
const CACHE_LIMIT: usize = 60;
const CACHE_TTL: Duration = Duration::from_secs(30 * 60);

/// 成功的解灰结果：(直链, 引擎名)。
type Match = (String, &'static str);

#[derive(Default)]
struct Cache {
    entries: HashMap<String, (Match, Instant)>,
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

fn key(id: i64, quality: Quality) -> String {
    // v2：只走 unm 引擎后升版，避开旧版本缓存下来的第三方坏直链
    format!("v2-unm-only|{id}|{quality:?}")
}

fn cached(id: i64, quality: Quality) -> Option<Match> {
    let mut cache = cache().lock().ok()?;
    let key = key(id, quality);
    let hit = cache.entries.get(&key)?;
    if hit.1.elapsed() >= CACHE_TTL {
        cache.entries.remove(&key);
        return None;
    }
    Some(hit.0.clone())
}

fn store(id: i64, quality: Quality, result: Match) {
    let Ok(mut cache) = cache().lock() else {
        return;
    };
    if cache.entries.len() >= CACHE_LIMIT {
        let oldest = cache
            .entries
            .iter()
            .min_by_key(|(_, (_, at))| *at)
            .map(|(key, _)| key.clone());
        if let Some(oldest) = oldest {
            cache.entries.remove(&oldest);
        }
    }
    cache.entries.insert(key(id, quality), (result, Instant::now()));
}

/// 引擎给"能 200 但不是音频"的占位地址（HTML、短试听、损坏的 mp4）时拒绝入库。
pub(super) async fn valid_audio(url: &str, duration_ms: i64) -> bool {
    if !url.starts_with("http") {
        return false;
    }
    let Some(client) = super::unm::client(AGENT) else {
        return false;
    };
    let Ok(response) = client
        .get(url)
        .header("Range", "bytes=0-8191")
        .header("Accept-Encoding", "identity")
        .send()
        .await
    else {
        return false;
    };
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return false;
    }
    // 注意：Range 请求的 Content-Length 只是分片长度（通常 8192），
    // 必须用 Content-Range 里的总大小判断体积，否则所有正常音频都会被误杀。
    let total = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.rsplit('/').next())
        .and_then(|text| text.parse::<u64>().ok())
        .or_else(|| {
            if status == 200 {
                response.content_length()
            } else {
                None
            }
        });
    if let Some(total) = total.filter(|total| *total > 0) {
        if total < PLACEHOLDER_BYTES {
            log::debug!("unblock: rejecting a placeholder stream ({total} bytes)");
            return false;
        }
        if duration_ms > 0 {
            // 低于约 32kbps 的一律视为占位/试听片段
            let minimum = (duration_ms as u64 / 1000) * 4_000;
            if total < minimum {
                log::debug!("unblock: rejecting a too-small stream ({total}/{minimum} bytes)");
                return false;
            }
        }
    }
    let Ok(bytes) = response.bytes().await else {
        return false;
    };
    let head = &bytes[..bytes.len().min(64)];
    let audio = head.starts_with(b"ID3")
        || head.starts_with(b"fLaC")
        || head.starts_with(b"OggS")
        || head.starts_with(b"RIFF")
        || head.first() == Some(&0xFF) && head.get(1).is_some_and(|b| b & 0xE0 == 0xE0)
        || head.get(4..8) == Some(b"ftyp") && head.len() >= 16;
    if !audio {
        log::debug!("unblock: rejecting a non-audio stream");
    }
    audio
}

/// 只走 unm 引擎；失败就返回 None，由调用方按正常错误路径报告。
pub(crate) async fn match_url(info: &TrackInfo, quality: Quality) -> Option<Match> {
    let id = info.id;
    if let Some(hit) = cached(id, quality) {
        log::info!("unblock: netease {id} hit the cache ({})", hit.1);
        return Some(hit);
    }

    let (url, source) = unm::match_url(info, quality).await?;
    log::info!("unblock: netease {id} matched by unm engine {source}");
    store(id, quality, (url.clone(), source));
    Some((url, source))
}
