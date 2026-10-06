use super::mp4_decoder::{is_iso_bmff, Mp4Decoder};
use super::streaming::{
    content_length, content_type, decode_streaming_reader, decode_with_panic_guard,
    detect_stream_format, BoxedAudioSource, StreamFormat, StreamingBuffer, StreamingReader,
};
use crate::http_client::shared_client;
use futures_util::StreamExt;
use lofty::{
    file::TaggedFileExt,
    probe::Probe,
    tag::{ItemKey, Tag},
};
use percent_encoding::percent_decode_str;
use rodio::{Decoder, Source};
use std::{
    fs::File,
    io::{BufReader, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Weak,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::AppHandle;

#[derive(Clone)]
pub(super) enum AudioSource {
    File {
        path: PathBuf,
        label: String,
        replay_gain: Option<ReplayGainInfo>,
    },
    Stream {
        reader: StreamingReader,
        format: StreamFormat,
        label: String,
        replay_gain: Option<ReplayGainInfo>,
    },
}

impl AudioSource {
    pub(super) fn label(&self) -> String {
        match self {
            AudioSource::File { label, .. } => label.clone(),
            AudioSource::Stream { label, .. } => label.clone(),
        }
    }

    pub(super) fn replay_gain(&self) -> Option<ReplayGainInfo> {
        match self {
            AudioSource::File { replay_gain, .. } => *replay_gain,
            AudioSource::Stream { replay_gain, .. } => *replay_gain,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct ReplayGainInfo {
    pub(super) gain_db: f32,
    pub(super) peak: Option<f32>,
}

/// 解析音源。`song_id` / `level` 是**播放缓存的键**（歌曲 + 音质）：
/// 给出时远程音源会落到 `audio_cache/{songId}_{level}.{ext}`，与 `check_audio_cache`
/// 的查询口径一致——两处键不一致会让同一首歌被反复完整下载。
/// 本地文件与 `unblock:` 前缀的调用方可传 `None`，此时退回 URL 维度命名。
pub(super) async fn resolve_audio_source(
    app: &AppHandle,
    source: String,
    latest_request_id: Arc<AtomicU64>,
    request_id: u64,
    song_id: Option<i64>,
    level: Option<&str>,
) -> Result<Option<AudioSource>, String> {
    if is_stale_request(&latest_request_id, request_id) {
        return Ok(None);
    }

    if let Some(source) = source.strip_prefix("unblock:") {
        // 解灰地址允许流式起播，但要先确认源站认 Range（第三个参数）。第三方 CDN 里
        // 有一批根本不认 Range：这种源一旦流式起播，用户拖进度就只能等顺序下载爬到那个
        // 字节偏移（几十秒），等于「起播快一次、之后每下都拖不动」，不划算。
        //
        // 此前恒为 false → 必须整首下完才出声，而第三方 CDN 通常只有几百 KB/s，一首无损
        // 要等十几到几十秒（灰歌转圈久的主因）。另外两条前提也已核实：格式判定看魔数
        // 优先于 URL 扩展名（`detect_stream_format`，第三方地址的扩展名经常与实际内容
        // 不符）；读等待有停滞上限，卡不住音频线程。
        // 代价：流被中途掐断时用户会听到「断了」而不是「没起来」，换来的是秒开且可拖。
        return resolve_remote_audio_source(
            app,
            source,
            latest_request_id,
            request_id,
            true,
            true,
            song_id,
            level,
        )
        .await;
    }

    if source.starts_with("http://") || source.starts_with("https://") {
        return resolve_remote_audio_source(
            app,
            &source,
            latest_request_id,
            request_id,
            true,
            false,
            song_id,
            level,
        )
        .await;
    }

    let path = decode_source_path(&source);
    if !path.exists() {
        return Err(format!("audio file does not exist: {}", path.display()));
    }

    if is_stale_request(&latest_request_id, request_id) {
        return Ok(None);
    }

    Ok(Some(AudioSource::File {
        label: path.to_string_lossy().to_string(),
        replay_gain: read_replay_gain(&path),
        path,
    }))
}

/// 流式下载单个分块之间的最长空转等待。CDN 偶发抖动远小于此值；超过即判定
/// 死流，报错停止，避免下载任务被永久占住、进而饿死音频回调要的缓冲。
const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(20);

/// 远程音源的落盘目标路径。
///
/// 给了 songId + level 就用稳定的「歌曲 + 音质」键——播放地址每次解析都可能
/// 带不同的签名/时效参数，URL 维度命名会让同一首歌每次都算作新文件；
/// 没有歌曲上下文的调用方（直接播一个原始地址）才退回 URL 维度命名。
fn remote_final_path(
    app: &AppHandle,
    url: &str,
    song_id: Option<i64>,
    level: Option<&str>,
) -> Result<PathBuf, String> {
    match (song_id, level.filter(|level| !level.is_empty())) {
        (Some(song_id), Some(level)) => {
            crate::cache::song_audio_cache_path(app, song_id, level, url)
        }
        _ => crate::cache::url_audio_cache_path(app, url),
    }
}

/// 探测源站是否真的认 Range：发一个 `bytes=0-0`，只接受 206。
///
/// 不看 `Accept-Ranges` 响应头——不少 CDN 声称支持却对 Range 请求回 200 全量，也有
/// 不带头但正常响应 206 的；只有真发一次才作数。成本是一次 RTT（几十到几百毫秒），
/// 只在「缓存没命中、又打算流式起播」时付，且 3s 内没结果就当不支持。
async fn remote_supports_range(url: &str) -> bool {
    let probe = shared_client()
        .get(url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .timeout(Duration::from_secs(3))
        .send()
        .await;
    matches!(
        probe,
        Ok(response) if response.status() == reqwest::StatusCode::PARTIAL_CONTENT
    )
}

async fn resolve_remote_audio_source(
    app: &AppHandle,
    url: &str,
    latest_request_id: Arc<AtomicU64>,
    request_id: u64,
    allow_streaming: bool,
    probe_range_before_streaming: bool,
    song_id: Option<i64>,
    level: Option<&str>,
) -> Result<Option<AudioSource>, String> {
    let final_path = remote_final_path(app, url, song_id, level)?;

    if final_path.exists() && final_path.metadata().map(|m| m.len()).unwrap_or(0) > 0 {
        return Ok(Some(AudioSource::File {
            replay_gain: read_replay_gain(&final_path),
            path: final_path,
            label: url.to_string(),
        }));
    }

    if is_stale_request(&latest_request_id, request_id) {
        return Ok(None);
    }

    let response = shared_client()
        .get(url)
        .send()
        .await
        .map_err(|err| format!("request audio url failed: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("request audio url returned {}", response.status()));
    }
    let content_type = content_type(response.headers());
    let content_length = content_length(response.headers());
    let mut stream = response.bytes_stream();
    let first_chunk = match tokio::time::timeout(CHUNK_IDLE_TIMEOUT, stream.next()).await {
        Ok(Some(chunk)) => {
            chunk.map_err(|err| format!("read audio response failed: {err}"))?
        }
        Ok(None) => return Err("audio response was empty".to_string()),
        Err(_) => return Err("audio response first chunk timed out".to_string()),
    };

    let temp_path = final_path.with_extension(format!("part-{}", unique_suffix()));

    if allow_streaming {
        if let Some(stream_format) =
            detect_stream_format(url, content_type.as_deref(), &first_chunk)
        {
            // 只有拿到 Content-Length 才允许流式起播，两个理由指向同一个缺口：
            // 长度未知时，解码器初始化阶段要往末尾 seek（ISO BMFF 必须有总长度；
            // symphonia 探测某些格式时也会 seek 去估长度），而 `StreamingReader`
            // 对「等不到长度」只能返回错误——rodio 0.20.1 把初始化期的 SeekError
            // 当作 `unreachable!()` panic 处理（`decoder/symphonia.rs:45`），那会
            // 当场打死音频线程：没声音、切不动输出设备，而且不会留下任何慢命令日志。
            // 真机日志里抓到的正是这条 panic。缺 Content-Length 的流本就是少数
            // （CDN 一般都会带），退回完整下载更安全。
            let streamable = content_length.is_some();
            // 需要先探测的源（解灰地址）在这一步确认能随机跳转才允许流式起播。
            // 放在这里而不是解析入口处，是为了不打断已经建立的主请求：无论探针结果
            // 如何，first_chunk 与 stream 都照原样交给下面的完整下载路径。
            let streamable = streamable
                && (!probe_range_before_streaming || remote_supports_range(url).await);
            if streamable {
                // 把最终落盘路径交给缓冲：顺序下载完成后它会打开该文件接管读取，
                // 释放整份内存副本（见 StreamingBuffer::finish）
                let shared = StreamingBuffer::new(content_length, Some(final_path.clone()));
                shared.append(&first_chunk)?;
                let reader = StreamingReader::new(
                    Arc::clone(&shared),
                    Arc::clone(&latest_request_id),
                    request_id,
                );
                let first_chunk_for_file = first_chunk.clone();
                let latest_request_id_for_download = Arc::clone(&latest_request_id);
                let app_for_cache_limit = app.clone();

                // Range 快进监视：reader seek 到未下载边界之外时，从目标字节
                // 位置发起新的范围请求灌入 tail，避免顺序等待整段下载数据
                tauri::async_runtime::spawn(run_fast_forward_watch(
                    url.to_string(),
                    Arc::downgrade(&shared),
                    Arc::clone(&latest_request_id),
                    request_id,
                ));

                tauri::async_runtime::spawn(async move {
                    let result = async {
                        if is_stale_request(&latest_request_id_for_download, request_id) {
                            return Err("streaming audio load cancelled".to_string());
                        }

                        let mut file = File::create(&temp_path)
                            .map_err(|err| format!("create temp audio file failed: {err}"))?;
                        file.write_all(&first_chunk_for_file)
                            .map_err(|err| format!("write temp audio file failed: {err}"))?;

                        loop {
                            if is_stale_request(&latest_request_id_for_download, request_id) {
                                return Err("streaming audio load cancelled".to_string());
                            }

                            // 连接中途僵死时 next() 可能永不返回，让整条流式
                            // 下载任务被永久占住；加空闲超时把死流转成普通错误
                            let next =
                                match tokio::time::timeout(CHUNK_IDLE_TIMEOUT, stream.next()).await
                                {
                                    Ok(next) => next,
                                    Err(_) => {
                                        return Err(
                                            "streaming audio read idle timeout".to_string()
                                        )
                                    }
                                };
                            let Some(chunk) = next else {
                                break;
                            };

                            let chunk = chunk.map_err(|err| {
                                format!("read streaming audio response failed: {err}")
                            })?;
                            shared.append(&chunk)?;
                            file.write_all(&chunk)
                                .map_err(|err| format!("write temp audio file failed: {err}"))?;
                        }

                        file.flush()
                            .map_err(|err| format!("flush temp audio file failed: {err}"))?;
                        drop(file);

                        if let Err(err) = std::fs::rename(&temp_path, &final_path) {
                            if final_path.exists()
                                && final_path.metadata().map(|m| m.len()).unwrap_or(0) > 0
                            {
                                let _ = std::fs::remove_file(&temp_path);
                                let _ =
                                    crate::cache::enforce_configured_audio_cache_limit_excluding(
                                        &app_for_cache_limit,
                                        &final_path,
                                    );
                                return Ok(());
                            }
                            return Err(format!("commit temp audio file failed: {err}"));
                        }

                        let _ = crate::cache::enforce_configured_audio_cache_limit_excluding(
                            &app_for_cache_limit,
                            &final_path,
                        );
                        Ok(())
                    }
                    .await;

                    match result {
                        Ok(()) => shared.finish(),
                        Err(error) => {
                            let _ = std::fs::remove_file(&temp_path);
                            shared.fail(error);
                        }
                    }
                });

                // 这里不再预建一次解码器做校验：那次构造（含 m4a 的 moov 解析、
                // 可能触发的 Range 快进）建完就丢，而真正播放用的解码器会在
                // rebuild_sink 里再建一次——同一首歌白白付两遍解码器初始化。
                // 格式与实际内容不符时由 rebuild_sink 报错，错误照样能回到前端
                // （controller 会补发 Stop），只是发生时机后移了。
                if is_stale_request(&latest_request_id, request_id) {
                    return Ok(None);
                }

                return Ok(Some(AudioSource::Stream {
                    reader,
                    format: stream_format,
                    label: url.to_string(),
                    replay_gain: None,
                }));
            }
        }
    }

    let mut file =
        File::create(&temp_path).map_err(|err| format!("create temp audio file failed: {err}"))?;
    file.write_all(&first_chunk)
        .map_err(|err| format!("write temp audio file failed: {err}"))?;

    loop {
        if is_stale_request(&latest_request_id, request_id) {
            drop(file);
            let _ = std::fs::remove_file(&temp_path);
            return Ok(None);
        }

        // 与流式分支同样兜住半开连接：next() 可能永不返回，否则任务连同
        // File 句柄与 .part 文件会被死连接永久占住
        let next = match tokio::time::timeout(CHUNK_IDLE_TIMEOUT, stream.next()).await {
            Ok(next) => next,
            Err(_) => {
                drop(file);
                let _ = std::fs::remove_file(&temp_path);
                return if is_stale_request(&latest_request_id, request_id) {
                    Ok(None)
                } else {
                    Err("read audio response idle timeout".to_string())
                };
            }
        };
        let Some(chunk) = next else {
            break;
        };

        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(err) => {
                let _ = std::fs::remove_file(&temp_path);
                return Err(format!("read audio response failed: {err}"));
            }
        };
        if let Err(err) = file.write_all(&chunk) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(format!("write temp audio file failed: {err}"));
        }
    }

    if let Err(err) = file.flush() {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!("flush temp audio file failed: {err}"));
    }
    drop(file);

    if let Err(err) = std::fs::rename(&temp_path, &final_path) {
        if final_path.exists() && final_path.metadata().map(|m| m.len()).unwrap_or(0) > 0 {
            let _ = std::fs::remove_file(&temp_path);
            let _ = crate::cache::enforce_configured_audio_cache_limit_excluding(app, &final_path);
            return Ok(Some(AudioSource::File {
                replay_gain: read_replay_gain(&final_path),
                path: final_path,
                label: url.to_string(),
            }));
        }
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!("commit temp audio file failed: {err}"));
    }

    let _ = crate::cache::enforce_configured_audio_cache_limit_excluding(app, &final_path);

    if is_stale_request(&latest_request_id, request_id) {
        return Ok(None);
    }

    Ok(Some(AudioSource::File {
        replay_gain: read_replay_gain(&final_path),
        path: final_path,
        label: url.to_string(),
    }))
}

pub(super) fn probe_source_duration(source: &AudioSource) -> Result<Option<Duration>, String> {
    match source {
        AudioSource::File { path, .. } => {
            let decoder = decode_file_source(path)?;
            Ok(decoder.total_duration())
        }
        AudioSource::Stream {
            reader,
            format,
            label,
            ..
        } => {
            let decoder = decode_streaming_reader(reader.clone(), *format).map_err(|err| {
                format!(
                    "decode preload streaming {} audio {label} failed: {err}",
                    format.label()
                )
            })?;
            Ok(decoder.total_duration())
        }
    }
}

/// 打开并解码本地音频文件。ISO BMFF（m4a/mp4 等）必须携带文件长度交给
/// Mp4Decoder，否则 symphonia 初始化会触发 rodio 的 unreachable!() panic；
/// 其余格式沿用 rodio 默认解码。
pub(super) fn decode_file_source(path: &std::path::Path) -> Result<BoxedAudioSource, String> {
    let file = File::open(path)
        .map_err(|err| format!("open audio file {} failed: {err}", path.display()))?;
    let mut reader = BufReader::new(file);

    let is_mp4 = is_iso_bmff(&mut reader)
        .map_err(|err| format!("read audio file {} failed: {err}", path.display()))?;

    if is_mp4 {
        let byte_len = reader
            .get_ref()
            .metadata()
            .map_err(|err| format!("read audio file {} metadata failed: {err}", path.display()))?
            .len();
        let decoder = Mp4Decoder::new(reader, byte_len, "m4a")
            .map_err(|err| format!("decode audio file {} failed: {err}", path.display()))?;
        return Ok(Box::new(decoder));
    }

    // 通用路径同样可能因探测回落到 isomp4 读取器触发 rodio 的
    // unreachable!() panic（如非 ftyp 开头的 ISO BMFF 文件），加 panic guard 兜底
    let decoder = decode_with_panic_guard(|| Ok(Box::new(Decoder::new(reader)?) as BoxedAudioSource))
        .map_err(|err| format!("decode audio file {} failed: {err}", path.display()))?;
    Ok(decoder)
}

#[derive(Debug)]
enum RangeStreamOutcome {
    Finished,
    Superseded,
    Failed(String),
}

/// 监听 reader 的快进请求并向源站发起 Range 请求，把目标位置之后的数据
/// 灌入 StreamingBuffer 的 tail 区域。原顺序下载不受影响，继续写落盘缓存，
/// 因此缓存文件始终完整有效。
///
/// 只持缓冲的 `Weak`：本任务 park 在 `rx.changed()` 期间绝不能是强持有者。
/// watch 的 Sender 就住在 StreamingBuffer 内部，若任务同时持有 Arc 与 Receiver，
/// 会形成自环——Sender 永不被 drop，`rx.changed()` 也永不返回 Err；切歌后
/// reader 与下载任务都已退出，本任务却永久 park，把整份流缓冲（data 预留
/// 最多 24MB + tail 最多 32MB）钉死在堆上，表现为越切歌内存越高。
/// 用 Weak 后：reader 与下载任务都释放时缓冲被 drop → Sender 随之 drop →
/// `rx.changed()` 立即返回 Err → 本任务退出并释放 url 等自身持有。
/// 需要写缓冲时再临时 `upgrade()`，用完即放，短时强引用不会阻塞回收。
async fn run_fast_forward_watch(
    url: String,
    shared: Weak<super::streaming::StreamingBuffer>,
    latest_request_id: Arc<AtomicU64>,
    request_id: u64,
) {
    let Some(buffer) = shared.upgrade() else {
        return;
    };
    let mut rx = buffer.fast_forward_receiver();
    drop(buffer);

    loop {
        if is_stale_request(&latest_request_id, request_id) {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
        let Some(target) = *rx.borrow_and_update() else {
            continue;
        };
        let Some(buffer) = shared.upgrade() else {
            return;
        };

        buffer.reset_tail(target);
        let outcome = stream_range_into_tail(
            &url,
            target,
            &buffer,
            &mut rx,
            &latest_request_id,
            request_id,
        )
        .await;

        match outcome {
            RangeStreamOutcome::Finished | RangeStreamOutcome::Superseded => {}
            RangeStreamOutcome::Failed(err) => {
                // Range 不可用（源站不支持/网络错误）：标记 tail 结束，
                // reader 退回等待主顺序下载推进，行为与旧版一致
                buffer.tail_finished();
                if is_stale_request(&latest_request_id, request_id) {
                    return;
                }
                let _ = err;
            }
        }
        // 回到 park 前必须放掉强引用（迭代末尾本会自动 drop，显式写出防止
        // 后续重构把 await 移到作用域内又造出自环）
        drop(buffer);
    }
}

async fn stream_range_into_tail(
    url: &str,
    target: u64,
    shared: &Arc<super::streaming::StreamingBuffer>,
    rx: &mut tokio::sync::watch::Receiver<Option<u64>>,
    latest_request_id: &Arc<AtomicU64>,
    request_id: u64,
) -> RangeStreamOutcome {
    use futures_util::StreamExt;

    let response = match shared_client()
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={target}-"))
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => return RangeStreamOutcome::Failed(format!("range request failed: {err}")),
    };

    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return RangeStreamOutcome::Failed(format!(
            "range request returned {} instead of 206",
            response.status()
        ));
    }

    let mut stream = response.bytes_stream();
    loop {
        tokio::select! {
            res = tokio::time::timeout(CHUNK_IDLE_TIMEOUT, stream.next()) => {
                // 半开连接下 next() 可能永不返回，超时后按失败退出，
                // 让 reader 退回等待主顺序下载推进
                let chunk = match res {
                    Ok(chunk) => chunk,
                    Err(_) => {
                        return RangeStreamOutcome::Failed(
                            "range stream idle timeout".to_string(),
                        )
                    }
                };
                let Some(chunk) = chunk else {
                    return RangeStreamOutcome::Finished;
                };
                if is_stale_request(latest_request_id, request_id) {
                    return RangeStreamOutcome::Superseded;
                }
                match chunk {
                    Ok(bytes) => {
                        if let Err(err) = shared.append_tail(&bytes) {
                            return RangeStreamOutcome::Failed(err);
                        }
                    }
                    Err(err) => {
                        return RangeStreamOutcome::Failed(format!(
                            "read range stream failed: {err}"
                        ));
                    }
                }
            }
            changed = rx.changed() => {
                if changed.is_err() {
                    return RangeStreamOutcome::Superseded;
                }
                match *rx.borrow_and_update() {
                    // 更新的快进目标出现，让外层用新目标重启 Range 流
                    Some(_) => return RangeStreamOutcome::Superseded,
                    // 请求被取消（reader 已被主缓冲满足），停掉本流即可
                    None => return RangeStreamOutcome::Superseded,
                }
            }
        }
    }
}

fn read_replay_gain(path: &PathBuf) -> Option<ReplayGainInfo> {
    let tagged_file = Probe::open(path).ok()?.read().ok()?;
    let tag = tagged_file.primary_tag().or(tagged_file.first_tag())?;

    let gain_db = read_replay_gain_db(tag, &ItemKey::ReplayGainTrackGain)
        .or_else(|| read_replay_gain_db(tag, &ItemKey::ReplayGainAlbumGain))?;
    let peak = read_replay_gain_peak(tag, &ItemKey::ReplayGainTrackPeak)
        .or_else(|| read_replay_gain_peak(tag, &ItemKey::ReplayGainAlbumPeak));

    Some(ReplayGainInfo { gain_db, peak })
}

fn read_replay_gain_db(tag: &Tag, key: &ItemKey) -> Option<f32> {
    parse_replay_gain_number(tag.get_string(key)?)
}

fn read_replay_gain_peak(tag: &Tag, key: &ItemKey) -> Option<f32> {
    parse_replay_gain_number(tag.get_string(key)?).filter(|peak| *peak > 0.0)
}

fn parse_replay_gain_number(value: &str) -> Option<f32> {
    let mut token = String::new();
    let mut started = false;

    for ch in value.chars() {
        if ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == '+' {
            token.push(ch);
            started = true;
        } else if started {
            break;
        }
    }

    token.parse::<f32>().ok().filter(|value| value.is_finite())
}

pub(super) fn native_audio_cache_size(app: &AppHandle) -> Result<u64, String> {
    Ok(native_audio_cache_files(app)?
        .iter()
        .map(|(_, size, _)| *size)
        .sum())
}

pub(super) fn clear_native_audio_cache(app: &AppHandle) -> Result<(), String> {
    for (path, _, _) in native_audio_cache_files(app)? {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

pub(super) fn enforce_native_audio_cache_limit(
    app: &AppHandle,
    max_bytes: u64,
) -> Result<(), String> {
    let mut files = native_audio_cache_files(app)?;
    files.sort_by(|a, b| a.2.cmp(&b.2));

    let mut current_size: u64 = files.iter().map(|(_, size, _)| *size).sum();
    for (path, size, _) in files {
        if current_size <= max_bytes {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            current_size = current_size.saturating_sub(size);
        }
    }

    Ok(())
}

fn native_audio_cache_dir(app: &AppHandle) -> Result<PathBuf, String> {
    // 解析逻辑收敛到 cache.rs：那里同时承载用户自定义缓存根目录的覆盖，
    // 两处各写一份会让「改了缓存位置但原生缓存仍写旧目录」复发
    crate::cache::native_audio_cache_dir(app).ok_or_else(|| "No cache dir".to_string())
}

fn native_audio_cache_files(app: &AppHandle) -> Result<Vec<(PathBuf, u64, SystemTime)>, String> {
    let dir = native_audio_cache_dir(app)?;
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries =
        std::fs::read_dir(&dir).map_err(|err| format!("read native audio cache failed: {err}"))?;
    let mut files = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_file() {
            files.push((
                path,
                metadata.len(),
                metadata.accessed().unwrap_or(SystemTime::UNIX_EPOCH),
            ));
        }
    }

    Ok(files)
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn is_stale_request(latest_request_id: &AtomicU64, request_id: u64) -> bool {
    latest_request_id.load(Ordering::SeqCst) != request_id
}

fn decode_source_path(source: &str) -> PathBuf {
    let raw = source
        .strip_prefix("file://")
        .or_else(|| source.strip_prefix("localmusic://localhost/"))
        .or_else(|| source.strip_prefix("asset://localhost/"))
        .unwrap_or(source);

    let mut decoded = percent_decode_str(raw).decode_utf8_lossy().to_string();
    if cfg!(windows)
        && decoded.starts_with('/')
        && decoded.len() > 3
        && decoded.as_bytes().get(2) == Some(&b':')
    {
        decoded.remove(0);
    }

    PathBuf::from(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_replay_gain_numbers_with_units() {
        assert_eq!(parse_replay_gain_number("-7.25 dB"), Some(-7.25));
        assert_eq!(parse_replay_gain_number("+2.0 dB"), Some(2.0));
        assert_eq!(parse_replay_gain_number("0.987654"), Some(0.987654));
        assert_eq!(parse_replay_gain_number("not tagged"), None);
    }
}
