use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

use tauri::AppHandle;

use super::command::{AudioCommand, ResponseTx};
use super::engine::{run_audio_thread, safe_duration_from_secs};
use super::snapshot::{current_snapshot, duration_secs};
use super::source::{probe_source_duration, resolve_audio_source};
pub use super::types::{AudioSnapshot, PreloadResult};

/// 命令队列容量。音频线程逐个串行执行命令（一次 seek/切设备可能占住数秒），
/// 期间 UI 命令只入队；不设上限时拖动类操作可无限堆积。256 条远高于正常
/// 突发（单条仅百字节量级），只在音频线程已被长操作占住时才有意义地生效。
const AUDIO_COMMAND_QUEUE_CAPACITY: usize = 256;

/// 等待音频线程回包的上限。与流式读取的空闲超时同量级：seek 在僵死流上
/// 最坏要等约 20s 才报错。超时后不再让调用方（blocking 池任务）无限 park，
/// 而是尽快返回错误——UI 靠 250ms 快照事件自会校正状态。
const AUDIO_COMMAND_RESPONSE_TIMEOUT: Duration = Duration::from_secs(20);

pub struct AudioState {
    tx: mpsc::SyncSender<AudioCommand>,
    snapshot: Arc<Mutex<AudioSnapshot>>,
    latest_request_id: Arc<AtomicU64>,
}

impl AudioState {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel(AUDIO_COMMAND_QUEUE_CAPACITY);
        let snapshot = Arc::new(Mutex::new(AudioSnapshot::idle(0.7, 0)));
        let snapshot_for_thread = Arc::clone(&snapshot);

        std::thread::spawn(move || run_audio_thread(rx, snapshot_for_thread));

        Self {
            tx,
            snapshot,
            latest_request_id: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 音频载入代次（见过的最大 requestId）。
    ///
    /// 供前端在**载入后**取作本地计数的基线：这个值活在应用进程里，而
    /// `corePlayer.playRequestId` 活在 WebView 文档里。页面重新加载后本地计数
    /// 从 1 重新数、进程侧却停在旧值，此后每一次 `player_load` 都会被
    /// `is_stale_request` 判成迟到的旧请求而丢弃（音源不解析、命令不下发），
    /// 表现为点歌没反应、加载转圈不落幕。
    pub fn latest_request_id(&self) -> u64 {
        self.latest_request_id.load(Ordering::SeqCst)
    }
}

/// 载入并起播一个音源。
///
/// `song_id` / `level` 是播放缓存键（会透传给音源解析，决定落盘文件名）；
/// `volume` / `speed` 随本次载入一起下发，避免起播后再补两条命令。
#[allow(clippy::too_many_arguments)]
pub async fn player_load(
    app: AppHandle,
    state: &AudioState,
    source: String,
    autoplay: bool,
    request_id: u64,
    song_id: Option<i64>,
    level: Option<String>,
    volume: Option<f32>,
    speed: Option<f32>,
) -> Result<AudioSnapshot, String> {
    // 代次只增不减：前端 requestId 单调递增（前提见 latest_request_id 的注释——
    // 页面重载后前端必须先取本进程代次作基线，否则新计数永远追不上 epoch），
    // 但两条 player_load 的 IPC 到达顺序不保证与调用顺序一致。若这里直接 store，
    // 一条迟到的旧请求（较小的 requestId）会把代次倒拨回旧值，随后
    // is_stale_request 便会把**真正最新**的那次请求误判成「被取代」而丢弃——
    // 表现为快速切歌后卡在加载、音频仍停在更早那首歌上。用 fetch_max 保证
    // 代次永远取到见过的最大值。
    state.latest_request_id.fetch_max(request_id, Ordering::SeqCst);

    // 等待挪到 blocking 线程后闭包必须是 'static，命令带的 AppHandle 要整体交出
    let begin_app = app.clone();
    let _ = send_command_async(state, move |respond_to| AudioCommand::BeginLoad {
        app: begin_app,
        request_id,
        respond_to,
    })
    .await?;

    let resolved_source = resolve_audio_source(
        &app,
        source,
        Arc::clone(&state.latest_request_id),
        request_id,
        song_id,
        level.as_deref(),
    )
    .await;
    let Some(source) = (match resolved_source {
        Ok(source) => source,
        Err(err) => {
            if !is_stale_request(&state.latest_request_id, request_id) {
                let stop_app = app.clone();
                let _ = send_command_async(state, move |respond_to| AudioCommand::Stop {
                    app: stop_app,
                    request_id,
                    respond_to,
                })
                .await;
            }
            return Err(err);
        }
    }) else {
        return Ok(current_snapshot(&state.snapshot));
    };

    if is_stale_request(&state.latest_request_id, request_id) {
        return Ok(current_snapshot(&state.snapshot));
    }

    send_command_async(state, move |respond_to| AudioCommand::Load {
        app,
        source,
        autoplay,
        request_id,
        volume,
        speed,
        respond_to,
    })
    .await
}

pub async fn player_preload(
    app: AppHandle,
    state: &AudioState,
    source: String,
    request_id: u64,
    song_id: Option<i64>,
    level: Option<String>,
) -> Result<PreloadResult, String> {
    let Some(source) = resolve_audio_source(
        &app,
        source,
        Arc::clone(&state.latest_request_id),
        request_id,
        song_id,
        level.as_deref(),
    )
    .await?
    else {
        return Ok(PreloadResult {
            request_id,
            is_ready: false,
            source: None,
            duration: 0.0,
        });
    };

    if is_stale_request(&state.latest_request_id, request_id) {
        return Ok(PreloadResult {
            request_id,
            is_ready: false,
            source: None,
            duration: 0.0,
        });
    }

    // 探测时长要同步构造解码器（可能触发 Range 请求与读等待），与 send_command 同理
    // 不能占住 async worker——悬停预取的密度比点歌更高，占满了同样会饿死别的下载任务
    let label = source.label();
    let duration = tauri::async_runtime::spawn_blocking(move || probe_source_duration(&source))
        .await
        .map_err(|err| format!("duration probe task failed: {err}"))??;
    Ok(PreloadResult {
        request_id,
        is_ready: true,
        source: Some(label),
        duration: duration_secs(duration),
    })
}

pub fn player_set_output_device(
    app: AppHandle,
    state: &AudioState,
    device_id: Option<String>,
) -> Result<AudioSnapshot, String> {
    send_command(state, |respond_to| AudioCommand::SetOutputDevice {
        app,
        device_id,
        respond_to,
    })
}

pub fn player_set_replay_gain(
    app: AppHandle,
    state: &AudioState,
    enabled: bool,
    preamp_db: f32,
) -> Result<AudioSnapshot, String> {
    send_command(state, |respond_to| AudioCommand::SetReplayGain {
        app,
        enabled,
        preamp_db,
        respond_to,
    })
}

pub fn player_set_equalizer(
    app: AppHandle,
    state: &AudioState,
    enabled: bool,
    gains_db: Vec<f32>,
) -> Result<AudioSnapshot, String> {
    send_command(state, |respond_to| AudioCommand::SetEqualizer {
        app,
        enabled,
        gains_db,
        respond_to,
    })
}

pub fn player_set_crossfade(
    app: AppHandle,
    state: &AudioState,
    duration_secs: f64,
) -> Result<AudioSnapshot, String> {
    let duration = safe_duration_from_secs(duration_secs);
    send_command(state, |respond_to| AudioCommand::SetCrossfade {
        app,
        duration,
        respond_to,
    })
}

pub fn player_play(app: AppHandle, state: &AudioState) -> Result<AudioSnapshot, String> {
    send_command(state, |respond_to| AudioCommand::Play { app, respond_to })
}

pub fn player_pause(app: AppHandle, state: &AudioState) -> Result<AudioSnapshot, String> {
    send_command(state, |respond_to| AudioCommand::Pause { app, respond_to })
}

pub fn player_seek(
    app: AppHandle,
    state: &AudioState,
    position_secs: f64,
) -> Result<AudioSnapshot, String> {
    let position = safe_duration_from_secs(position_secs);
    send_command(state, |respond_to| AudioCommand::Seek {
        app,
        position,
        respond_to,
    })
}

pub fn player_set_volume(
    app: AppHandle,
    state: &AudioState,
    volume: f32,
) -> Result<AudioSnapshot, String> {
    send_command(state, |respond_to| AudioCommand::SetVolume {
        app,
        volume,
        respond_to,
    })
}

pub fn player_set_speed(
    app: AppHandle,
    state: &AudioState,
    speed: f32,
) -> Result<AudioSnapshot, String> {
    send_command(state, |respond_to| AudioCommand::SetSpeed {
        app,
        speed,
        respond_to,
    })
}

pub fn player_stop(app: AppHandle, state: &AudioState) -> Result<AudioSnapshot, String> {
    let request_id = state.latest_request_id.load(Ordering::SeqCst);
    send_command(state, |respond_to| AudioCommand::Stop {
        app,
        request_id,
        respond_to,
    })
}

pub fn player_get_state(state: &AudioState) -> Result<AudioSnapshot, String> {
    Ok(current_snapshot(&state.snapshot))
}

fn is_stale_request(latest_request_id: &AtomicU64, request_id: u64) -> bool {
    latest_request_id.load(Ordering::SeqCst) != request_id
}

/// 入队一条命令，返回等回包用的接收端。
fn enqueue_command<F>(state: &AudioState, build: F) -> Result<mpsc::Receiver<Result<AudioSnapshot, String>>, String>
where
    F: FnOnce(ResponseTx) -> AudioCommand,
{
    let (respond_to, response) = mpsc::channel();
    // 队列满说明音频线程已被长操作占住且积压到上限：直接拒绝这条新命令，
    // 而不是让调用方 park 在 send 上继续堆积
    state.tx.try_send(build(respond_to)).map_err(|err| match err {
        mpsc::TrySendError::Full(_) => {
            "audio command queue is full, request dropped".to_string()
        }
        mpsc::TrySendError::Disconnected(_) => "audio thread is not available".to_string(),
    })?;
    Ok(response)
}

fn map_response_error(err: mpsc::RecvTimeoutError) -> String {
    match err {
        mpsc::RecvTimeoutError::Timeout => "audio thread did not respond in time".to_string(),
        mpsc::RecvTimeoutError::Disconnected => "audio thread dropped response".to_string(),
    }
}

/// 同步命令：调用方本来就跑在 blocking 池（见 audio.rs 的 run_audio_command），
/// 在这里 park 是安全的。
fn send_command<F>(state: &AudioState, build: F) -> Result<AudioSnapshot, String>
where
    F: FnOnce(ResponseTx) -> AudioCommand,
{
    enqueue_command(state, build)?
        .recv_timeout(AUDIO_COMMAND_RESPONSE_TIMEOUT)
        .map_err(map_response_error)?
}

/// 异步命令用的等待版本：**必须**把 `recv_timeout` 挪出 async worker。
///
/// 原先 `player_load` 直接在 async 上下文里调 `send_command`，一等就是最多 20s，
/// 把一个 tokio worker 占住。快速切歌几首就能把 worker 占满，于是给流缓冲喂数据的
/// 下载任务不再被轮询——既不报错也不出数据，而音频线程正同步读那条流，只能在
/// `StreamingReader::read` 里干等（实测：此后每条命令都 20s 超时、切歌彻底没声）。
/// 放 blocking 池等就没有这层连带效应：blocking 线程按需增长，不抢占任务调度。
async fn send_command_async<F>(
    state: &AudioState,
    build: F,
) -> Result<AudioSnapshot, String>
where
    F: FnOnce(ResponseTx) -> AudioCommand + Send + 'static,
{
    let response = enqueue_command(state, build)?;
    tauri::async_runtime::spawn_blocking(move || {
        response.recv_timeout(AUDIO_COMMAND_RESPONSE_TIMEOUT)
    })
    .await
    .map_err(|err| format!("audio command wait task failed: {err}"))?
    .map_err(map_response_error)?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 快速切歌的竞态复现：两条 player_load 的 IPC 到达顺序可能与调用顺序相反。
    /// 代次必须是**单调不减**的，否则一条迟到的旧请求会把代次倒拨，
    /// 导致真正最新的那次请求被 is_stale_request 误判为「被取代」而丢弃——
    /// 表现即「卡在加载转圈、听到的还是更早那首歌」。
    #[test]
    fn late_arriving_older_request_must_not_revert_epoch() {
        let epoch = AtomicU64::new(0);

        // 新请求 B（request_id=2）先到
        epoch.fetch_max(2, Ordering::SeqCst);
        assert!(!is_stale_request(&epoch, 2), "最新的请求不应被判为过期");

        // 迟到的是更早的请求 A（request_id=1）：不得把代次倒拨
        epoch.fetch_max(1, Ordering::SeqCst);
        assert!(
            !is_stale_request(&epoch, 2),
            "迟到的旧请求不得让最新请求变成「过期」"
        );
        assert!(
            is_stale_request(&epoch, 1),
            "迟到的旧请求自身应被判为过期而丢弃"
        );
    }

    /// store 的旧行为（无 fetch_max）会在此断言上失败——保留作对照说明为什么要改。
    #[test]
    fn fetch_max_is_what_keeps_epoch_monotonic() {
        let epoch = AtomicU64::new(0);
        epoch.fetch_max(5, Ordering::SeqCst);
        epoch.fetch_max(3, Ordering::SeqCst);
        epoch.fetch_max(4, Ordering::SeqCst);
        assert_eq!(epoch.load(Ordering::SeqCst), 5);
        // 对照：若改用 store，这里会退化到 4（被迟到的较小代次覆盖）
        let naive = AtomicU64::new(0);
        naive.store(5, Ordering::SeqCst);
        naive.store(3, Ordering::SeqCst);
        naive.store(4, Ordering::SeqCst);
        assert_eq!(naive.load(Ordering::SeqCst), 4);
    }
}
