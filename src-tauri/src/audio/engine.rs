use rodio::Sink;
use std::{
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::AppHandle;

use super::command::AudioCommand;
use super::device::{normalize_device_id, open_output};
use super::event::{emit_snapshot, respond_and_emit};
use super::pipeline::{rebuild_sink, reset_audio_analysis, stop_fading_sinks, update_fades};
use super::snapshot::{idle_snapshot, snapshot_inner, store_snapshot};
use super::source::AudioSource;
use super::state::{
    retired_output_is_releasable, track_position, AudioThreadBusy, AudioThreadInner, RetiredOutput,
    TrackState, RETIRED_OUTPUT_FORCE_GRACE,
};
use super::types::AudioSnapshot;

const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
const CROSSFADE_MAX_DURATION: Duration = Duration::from_secs(12);

/// 单条命令占用音频线程超过这个时长就打一条日志。
///
/// 音频线程是**一条命令串行执行**的：它被谁占住，后面所有命令（含切换输出设备、
/// 暂停、seek）就只能排队等，而调用方的回包超时是 20s——用户看到的就是「卡住没声音、
/// 设备也切不动」。没有这条日志时，那种状态只能从外面猜是哪类命令咬住了线程；
/// 有了它就有一条直接的归因证据（2026-10-03 的压测就是这么定位的）。
const SLOW_COMMAND_LOG_THRESHOLD: Duration = Duration::from_secs(2);

/// `Sink::try_seek` 等 mixer 线程回信的上限。
///
/// rodio 的 `try_seek` 把 SeekOrder 投进控件后就 `feedback.recv()` 阻塞等结果
/// （sink.rs:249-261），而回信只在 mixer 线程处理样本的间隙给出。mixer 一旦卡在
/// 我们的 `StreamingReader::read` 里（僵死流最坏 25s），这条 Seek 就把**整条音频
/// 线程**占死：此后每条 `player_*` 都要等满 20s 回包超时，界面上是进度还在走、
/// 声音没有、连输出设备都切不动。
///
/// 取 3s：实测正常跳转只要 4~23ms，跨 CDN 的 Range 跳转也远在此之内；关键是它
/// 必须明显小于回包超时，超时后按「跳转没成」走既有的重建管线回退路径。
const SEEK_FEEDBACK_TIMEOUT: Duration = Duration::from_secs(3);

/// 底层播放位置停住多久算「无声」。
///
/// 心跳每 250ms 一拍，rodio 每 5ms 由 mixer 回写一次位置（sink.rs:126-146）。
/// 位置不动就说明 mixer 没在消费样本——通常是它卡在我们的流读取里等网络。
/// 取 2s：正常缓冲期间位置每拍都在走，2s 足够排除偶发抖动，又短到能在用户还没
/// 放弃时留下一条归因日志。
const SILENCE_LOG_THRESHOLD: Duration = Duration::from_secs(2);

/// 喂一拍 mixer 回写的播放位置，返回 `Some(停滞时长)` 表示「刚刚判定为无声」。
///
/// 只在刚进入停滞时返回一次（`silence_reported` 去重，免得每拍刷屏），位置重新
/// 推进后自动再武装。判定与状态分离、`now` 由调用方传入，是为了让这条不变量能
/// 被单测锁死（不必真等满阈值）。
fn note_sink_progress(
    inner: &mut AudioThreadInner,
    position: Duration,
    now: Instant,
) -> Option<Duration> {
    if inner.sink_position != Some(position) {
        inner.sink_position = Some(position);
        inner.sink_position_changed_at = Some(now);
        inner.silence_reported = false;
        return None;
    }

    let stalled_for = now
        .saturating_duration_since(inner.sink_position_changed_at.unwrap_or(now));
    if inner.silence_reported || stalled_for < SILENCE_LOG_THRESHOLD {
        return None;
    }

    inner.silence_reported = true;
    Some(stalled_for)
}

pub(super) fn run_audio_thread(
    rx: mpsc::Receiver<AudioCommand>,
    snapshot_state: Arc<Mutex<AudioSnapshot>>,
    busy: Arc<Mutex<AudioThreadBusy>>,
) {
    let mut inner = AudioThreadInner::new();
    let mut last_app: Option<AppHandle> = None;

    loop {
        mark_loop(&busy);
        match rx.recv_timeout(PROGRESS_INTERVAL) {
            Ok(command) => handle_command(command, &mut inner, &snapshot_state, &mut last_app, &busy),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // 就绪前硬切的兜底：新音源超时未到位时把还在响的旧歌停掉，
                // 否则网络挂死会让旧歌一直唱下去、看起来像点歌没生效。
                // 与是否已有 app 句柄无关，心跳里每次都要检查。
                if let Some(snapshot) = expire_pending_handoff(&mut inner) {
                    store_snapshot(&snapshot_state, snapshot.clone());
                    if let Some(app) = last_app.as_ref() {
                        emit_snapshot(app, "state", snapshot);
                    }
                }
                if let Some(app) = last_app.as_ref() {
                    tick_progress(app, &mut inner, &snapshot_state);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// 记一笔「线程此刻正被哪条命令占住」。锁只在进入/离开命令时握一下，
/// 被占住的那段时间锁是放开的——诊断命令因此永远问得出来。
fn mark_loop(busy: &Arc<Mutex<AudioThreadBusy>>) {
    if let Ok(mut guard) = busy.lock() {
        guard.looped_at = Instant::now();
        guard.loops += 1;
    }
}

fn handle_command(
    command: AudioCommand,
    inner: &mut AudioThreadInner,
    snapshot_state: &Arc<Mutex<AudioSnapshot>>,
    last_app: &mut Option<AppHandle>,
    busy: &Arc<Mutex<AudioThreadBusy>>,
) {
    let kind = command_kind(&command);
    let started = Instant::now();
    if let Ok(mut guard) = busy.lock() {
        guard.current = Some((kind, started));
    }
    dispatch_command(command, inner, snapshot_state, last_app);
    let elapsed = started.elapsed();
    if let Ok(mut guard) = busy.lock() {
        guard.current = None;
        guard.last_done = Some((kind, elapsed, Instant::now()));
    }
    if elapsed >= SLOW_COMMAND_LOG_THRESHOLD {
        crate::log_warn!(
            "[AUDIO] 命令占住音频线程 {:.1}s: {}（这段时间里其它命令都在排队）",
            elapsed.as_secs_f64(),
            kind
        );
    }
}

/// 命令种类标签，只用于日志归因（不改变任何行为）。
pub(super) fn command_kind(command: &AudioCommand) -> &'static str {
    match command {
        AudioCommand::BeginLoad { .. } => "BeginLoad",
        AudioCommand::Load { .. } => "Load",
        AudioCommand::Play { .. } => "Play",
        AudioCommand::Pause { .. } => "Pause",
        AudioCommand::Seek { .. } => "Seek",
        AudioCommand::SetVolume { .. } => "SetVolume",
        AudioCommand::SetSpeed { .. } => "SetSpeed",
        AudioCommand::Stop { .. } => "Stop",
        AudioCommand::SetOutputDevice { .. } => "SetOutputDevice",
        AudioCommand::SetReplayGain { .. } => "SetReplayGain",
        AudioCommand::SetEqualizer { .. } => "SetEqualizer",
        AudioCommand::SetCrossfade { .. } => "SetCrossfade",
    }
}

fn dispatch_command(
    command: AudioCommand,
    inner: &mut AudioThreadInner,
    snapshot_state: &Arc<Mutex<AudioSnapshot>>,
    last_app: &mut Option<AppHandle>,
) {
    match command {
        AudioCommand::BeginLoad {
            app,
            request_id,
            respond_to,
        } => {
            *last_app = Some(app.clone());
            let result = begin_load_inner(inner, request_id);
            respond_and_emit(&app, "state", result, respond_to, snapshot_state);
        }
        AudioCommand::Load {
            app,
            source,
            autoplay,
            request_id,
            volume,
            speed,
            respond_to,
        } => {
            *last_app = Some(app.clone());
            let result = load_source(inner, source, autoplay, request_id, volume, speed);
            respond_and_emit(&app, "loaded", result, respond_to, snapshot_state);
        }
        AudioCommand::Play { app, respond_to } => {
            *last_app = Some(app.clone());
            let result = play_inner(inner);
            respond_and_emit(&app, "state", result, respond_to, snapshot_state);
        }
        AudioCommand::Pause { app, respond_to } => {
            *last_app = Some(app.clone());
            let result = pause_inner(inner);
            respond_and_emit(&app, "state", result, respond_to, snapshot_state);
        }
        AudioCommand::Seek {
            app,
            position,
            respond_to,
        } => {
            *last_app = Some(app.clone());
            let result = seek_inner(inner, position);
            respond_and_emit(&app, "progress", result, respond_to, snapshot_state);
        }
        AudioCommand::SetVolume {
            app,
            volume,
            respond_to,
        } => {
            *last_app = Some(app.clone());
            let result = set_volume_inner(inner, volume);
            respond_and_emit(&app, "state", result, respond_to, snapshot_state);
        }
        AudioCommand::SetSpeed {
            app,
            speed,
            respond_to,
        } => {
            *last_app = Some(app.clone());
            let result = set_speed_inner(inner, speed);
            respond_and_emit(&app, "state", result, respond_to, snapshot_state);
        }
        AudioCommand::Stop {
            app,
            request_id,
            respond_to,
        } => {
            *last_app = Some(app.clone());
            let result = stop_inner(inner, request_id);
            respond_and_emit(&app, "state", result, respond_to, snapshot_state);
        }
        AudioCommand::SetOutputDevice {
            app,
            device_id,
            respond_to,
        } => {
            *last_app = Some(app.clone());
            let result = set_output_device_inner(inner, device_id);
            respond_and_emit(&app, "state", result, respond_to, snapshot_state);
        }
        AudioCommand::SetReplayGain {
            app,
            enabled,
            preamp_db,
            respond_to,
        } => {
            *last_app = Some(app.clone());
            let result = set_replay_gain_inner(inner, enabled, preamp_db);
            respond_and_emit(&app, "state", result, respond_to, snapshot_state);
        }
        AudioCommand::SetEqualizer {
            app,
            enabled,
            gains_db,
            respond_to,
        } => {
            *last_app = Some(app.clone());
            let result = set_equalizer_inner(inner, enabled, gains_db);
            respond_and_emit(&app, "state", result, respond_to, snapshot_state);
        }
        AudioCommand::SetCrossfade {
            app,
            duration,
            respond_to,
        } => {
            *last_app = Some(app.clone());
            let result = set_crossfade_inner(inner, duration);
            respond_and_emit(&app, "state", result, respond_to, snapshot_state);
        }
    }
}

pub(super) fn safe_duration_from_secs(secs: f64) -> Duration {
    if secs.is_finite() && secs > 0.0 {
        Duration::from_secs_f64(secs)
    } else {
        Duration::ZERO
    }
}

fn normalize_crossfade_duration(duration: Duration) -> Duration {
    duration.min(CROSSFADE_MAX_DURATION)
}

fn begin_load_inner(
    inner: &mut AudioThreadInner,
    request_id: u64,
) -> Result<AudioSnapshot, String> {
    // 迟到防护：本次 BeginLoad/Stop 的代次已落后于音频线程见过的最新代次，
    // 说明用户又点了更靠后的歌（详见 should_ignore_stale_command 的注释）。
    if should_ignore_stale_command(request_id, inner.request_id) {
        return Ok(snapshot_inner(inner));
    }

    let has_playing_sink = inner.sink.is_some()
        && inner
            .track
            .as_ref()
            .map(|track| track.is_playing)
            .unwrap_or(false);

    // 有正在出声的旧歌就**不立刻停**：让它在解析新音源的整段时间里继续响，
    // 直到 rebuild_sink 把新 sink 建好并 append 完成才停旧 sink（见 pipeline.rs）。
    // 旧实现在这里无条件 stop，于是「点歌 → 出声」的全部耗时都落在静音窗口里，
    // 网络稍慢就能听见一段空白。crossfade 时长与是否硬切无关，硬切只是
    // should_crossfade=false 的那条路径，同样在就绪后停旧源。
    //
    // 代价是并发多出一个 sink（最多持续 HANDOFF_GRACE），换来的是切歌全程有声。
    if !has_playing_sink {
        return stop_inner(inner, request_id);
    }

    inner.request_id = request_id;
    inner.handoff_deadline = Some(Instant::now() + HANDOFF_GRACE);
    Ok(idle_snapshot(inner, request_id))
}

/// 新音源迟迟不到时旧歌可以继续响的宽限期。
///
/// 上限不能太长：「点歌后旧歌还在唱」在慢网络上会被误读成没点中，
/// 且这段时间里用户听到的和 UI 显示的（加载中）是矛盾的。
const HANDOFF_GRACE: Duration = Duration::from_millis(1500);

/// 迟到的旧命令是否该被丢弃（不得据此改写代次）。
///
/// command 队列按**入队顺序**串行执行，而入队顺序**不等于**前端调用顺序：
/// 快速切歌「先点 A 再点 B」时，A 的 `player_load` 完全可能晚于 B 才把
/// `BeginLoad` / `Stop` 排进队列。若这些命令无条件写 `inner.request_id`，
/// 代次就会被倒拨回 A，随后 B 真正的 `Load`（判据 `request_id < inner.request_id`）
/// 被当成过期丢弃——**最终选中的 B 永远进不了播放**，前端停在加载转圈、
/// 音频仍是更早那首。代次必须单调不减，落后的命令直接忽略。
fn should_ignore_stale_command(request_id: u64, current_request_id: u64) -> bool {
    request_id < current_request_id
}

/// 宽限期到点后是否该主动停掉旧歌。
///
/// 判据是 `track.request_id`：`Load` 会把这个音轨整条换成新代号，因此
/// 「track 仍是旧代号」精确等价于「本次 BeginLoad 对应的新音源尚未就位」。
/// 只看指针是否为空是不够的——用户期间又点了别的歌时，管线已归新请求所有。
fn should_expire_handoff(
    deadline: Option<Instant>,
    now: Instant,
    track_request_id: Option<u64>,
    current_request_id: u64,
) -> bool {
    let Some(deadline) = deadline else {
        return false;
    };
    if now < deadline {
        return false;
    }
    matches!(track_request_id, Some(id) if id != current_request_id)
}

/// 在音频线程的 250ms 心跳里检查宽限期（不额外起定时任务，也就不会与
/// 命令处理产生竞态）。返回需要广播的状态快照。
fn expire_pending_handoff(inner: &mut AudioThreadInner) -> Option<AudioSnapshot> {
    let track_request_id = inner.track.as_ref().map(|track| track.request_id);
    if !should_expire_handoff(
        inner.handoff_deadline,
        Instant::now(),
        track_request_id,
        inner.request_id,
    ) {
        return None;
    }

    inner.handoff_deadline = None;
    stop_inner(inner, inner.request_id).ok()
}

fn load_source(
    inner: &mut AudioThreadInner,
    source: AudioSource,
    autoplay: bool,
    request_id: u64,
    volume: Option<f32>,
    speed: Option<f32>,
) -> Result<AudioSnapshot, String> {
    // 迟到防护：本次 Load 的代次已落后于音频线程见过的最新代次，说明期间
    // 用户又点了别的歌（新的 BeginLoad/Stop 已把 request_id 推进）。被放弃的
    // 请求在网络解析里耗了一段时间后，它的 Load 仍可能排进命令队列——若照常
    // 执行会重建管线、把 inner.request_id 倒拨回旧值，与随后真正的新请求
    // 抢 sink，表现为偶发闪断/状态串味。直接丢弃即可，当前状态照常返回。
    if should_ignore_stale_command(request_id, inner.request_id) {
        return Ok(snapshot_inner(inner));
    }

    inner.request_id = request_id;
    // 新音源已就位：宽限期兜底不再需要
    inner.handoff_deadline = None;

    // 音量 / 倍速必须在重建管线**之前**落定：rebuild_sink 建 sink 时就用
    // inner.volume 与 inner.speed 决定首帧音量、并把 TempoSource 按该倍率装配。
    // 放在起播之后再补命令的话，首帧总是先按旧值出声，随后才被纠正（听感上
    // 是"起播瞬间音量/语速不对然后跳一下"）。
    if let Some(volume) = volume {
        inner.volume = volume.clamp(0.0, 1.5);
    }
    if let Some(speed) = speed {
        let speed = speed.clamp(super::tempo::MIN_SPEED, super::tempo::MAX_SPEED);
        // 新 track 从 0 起播，无需像 set_speed_inner 那样先按旧速度结算位置
        inner.speed = speed;
        inner.speed_control.request(speed);
    }

    inner.track = Some(TrackState {
        source,
        request_id,
        duration: None,
        base_position: Duration::ZERO,
        started_at: if autoplay { Some(Instant::now()) } else { None },
        is_playing: autoplay,
    });

    let duration = match rebuild_sink(inner, Duration::ZERO, autoplay, true) {
        Ok(duration) => duration,
        Err(error) => {
            let _ = stop_inner(inner, request_id);
            return Err(error);
        }
    };
    if let Some(track) = inner.track.as_mut() {
        track.duration = duration;
    }

    Ok(snapshot_inner(inner))
}

fn play_inner(inner: &mut AudioThreadInner) -> Result<AudioSnapshot, String> {
    if inner.track.is_none() {
        return Err("no loaded audio source".to_string());
    }

    if inner.sink.is_none() {
        let position = inner
            .track
            .as_ref()
            .map(|track| track_position(track, inner.speed))
            .unwrap_or(Duration::ZERO);
        let duration = rebuild_sink(inner, position, true, false)?;
        if let Some(track) = inner.track.as_mut() {
            track.duration = duration.or(track.duration);
        }
    } else if let Some(sink) = inner.sink.as_ref() {
        sink.play();
    }

    if let Some(track) = inner.track.as_mut() {
        track.base_position = track_position(track, inner.speed);
        track.started_at = Some(Instant::now());
        track.is_playing = true;
    }

    Ok(snapshot_inner(inner))
}

fn pause_inner(inner: &mut AudioThreadInner) -> Result<AudioSnapshot, String> {
    if let Some(track) = inner.track.as_mut() {
        track.base_position = track_position(track, inner.speed);
        track.started_at = None;
        track.is_playing = false;
    }
    if let Some(sink) = inner.sink.as_ref() {
        sink.pause();
    }

    Ok(snapshot_inner(inner))
}

/// 在另一条线程里等 `try_seek` 的回信，到点没等到就放弃——音频线程绝不因它停摆。
///
/// 放弃之后这次跳转仍可能在 mixer 恢复时生效，但调用方紧接着会重建管线并
/// `stop()` 掉旧 sink，一条迟到的跳转落在已停播的 sink 上没有影响；等待线程
/// 只是临时多持有一份 `Arc<Sink>`，回信到手（或 sink 被它 drop）后就退出。
fn seek_sink_with_timeout(sink: &Arc<Sink>, position: Duration) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let seeking = Arc::clone(sink);
    std::thread::spawn(move || {
        // 调用方已超时返回时发送失败，线程随之退出
        let _ = tx.send(seeking.try_seek(position));
    });

    match rx.recv_timeout(SEEK_FEEDBACK_TIMEOUT) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(format!("sink seek failed: {error}")),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err("sink seek did not report back in time".to_string())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("sink seek thread ended without a result".to_string())
        }
    }
}

fn seek_inner(
    inner: &mut AudioThreadInner,
    mut position: Duration,
) -> Result<AudioSnapshot, String> {
    let Some(track) = inner.track.as_ref() else {
        return Err("no loaded audio source".to_string());
    };

    if let Some(duration) = track.duration {
        position = position.min(duration);
    }
    let was_playing = track.is_playing;

    // 快路径：在当前 Sink 上做 demuxer 级 seek（毫秒级），避免重开文件、
    // 重建管线和 skip_duration 的「从头解码再丢弃」。
    let sink_can_seek = inner
        .sink
        .as_ref()
        .map(|sink| !sink.empty())
        .unwrap_or(false);
    if sink_can_seek {
        let sink = inner.sink.as_ref().expect("checked above");
        reset_audio_analysis(inner);
        if seek_sink_with_timeout(sink, position).is_ok() {
            if let Some(track) = inner.track.as_mut() {
                track.base_position = position;
                track.started_at = if was_playing {
                    Some(Instant::now())
                } else {
                    None
                };
                track.is_playing = was_playing;
            }
            return Ok(snapshot_inner(inner));
        }
    }

    // 回退路径：sink 为空或底层源不支持 seek 时，重建管线并解码跳转
    let duration = rebuild_sink(inner, position, was_playing, false)?;

    if let Some(track) = inner.track.as_mut() {
        track.duration = duration.or(track.duration);
        track.base_position = position;
        track.started_at = if was_playing {
            Some(Instant::now())
        } else {
            None
        };
        track.is_playing = was_playing;
    }

    Ok(snapshot_inner(inner))
}

fn set_volume_inner(inner: &mut AudioThreadInner, volume: f32) -> Result<AudioSnapshot, String> {
    inner.volume = volume.clamp(0.0, 1.5);
    if let Some(sink) = inner.sink.as_ref() {
        if let Some(fade_in) = inner.fade_in.as_mut() {
            fade_in.target_volume = inner.volume;
        } else {
            sink.set_volume(inner.volume);
        }
    }

    Ok(snapshot_inner(inner))
}

fn set_speed_inner(inner: &mut AudioThreadInner, speed: f32) -> Result<AudioSnapshot, String> {
    let speed = speed.clamp(super::tempo::MIN_SPEED, super::tempo::MAX_SPEED);
    // 用旧速度结算已播放时长并重锚定，避免速度切换瞬间进度跳变
    if let Some(track) = inner.track.as_mut() {
        track.base_position = track_position(track, inner.speed);
        track.started_at = if track.is_playing {
            Some(Instant::now())
        } else {
            None
        };
    }
    inner.speed = speed;
    inner.speed_control.request(speed);

    Ok(snapshot_inner(inner))
}

/// 切换输出设备。
///
/// 顺序很关键：**先把新流换进来，但不停旧 sink**，让旧设备继续发声，直到新 sink
/// 在新设备上建好并定位完成（停旧 sink 由 rebuild_sink 内部负责）。旧实现是
/// 先 stop 旧 sink 再重建，于是「枚举设备 + 打开 WASAPI 流 + 重建解码器 + 定位」
/// 的全部耗时都落在静音窗口里。旧 OutputStream 也必须活到旧 sink 停播之后——
/// 它的混音线程才是旧 sink 停播动作的执行者。
fn set_output_device_inner(
    inner: &mut AudioThreadInner,
    device_id: Option<String>,
) -> Result<AudioSnapshot, String> {
    let selected_device_id = normalize_device_id(device_id);
    if selected_device_id == inner.selected_device_id {
        return Ok(snapshot_inner(inner));
    }
    let target_label = selected_device_id
        .clone()
        .unwrap_or_else(|| "<默认设备>".to_string());

    // 这条命令是现场量到「占住音频线程 20~30s」的那一条，所以分段计时写进日志：
    // 开新流多久、在新流上重建管线多久。旧流的释放不再计入（挪到后台线程去了）。
    let open_started = Instant::now();
    let (stream, handle) = open_output(selected_device_id.as_deref())?;
    let open_elapsed = open_started.elapsed();

    let track_position = inner
        .track
        .as_ref()
        .map(|track| track_position(track, inner.speed))
        .unwrap_or(Duration::ZERO);
    let was_playing = inner
        .track
        .as_ref()
        .map(|track| track.is_playing)
        .unwrap_or(false);

    let previous_device_id = inner.selected_device_id.clone();
    // rebuild_sink 会把旧 sink 换下来并 stop 它；那块 sink 是「旧设备的 mixer 还在不在
    // 读我们的流」唯一看得见的凭证，留一份给 release_retired_outputs 判断何时能安全结束
    let previous_sink = inner.sink.clone();
    let previous_stream = inner.stream.replace(stream);
    let previous_handle = inner.handle.replace(handle);
    inner.selected_device_id = selected_device_id;

    let mut rebuild_elapsed = Duration::ZERO;
    if inner.track.is_some() {
        let rebuild_started = Instant::now();
        let duration = match rebuild_sink(inner, track_position, was_playing, false) {
            Ok(duration) => duration,
            Err(err) => {
                // 回滚：rebuild_sink 只在建源成功后才停旧 sink，失败时旧 sink 与旧流
                // 都还有效，放回去即可继续在原设备上出声，不至于两边都没声音
                inner.handle = previous_handle;
                inner.stream = previous_stream;
                inner.selected_device_id = previous_device_id;
                crate::log_warn!(
                    "[AUDIO] 切换输出设备 {target_label} 失败：开新流 {:.2}s，重建管线 {:.2}s，{err}",
                    open_elapsed.as_secs_f64(),
                    rebuild_started.elapsed().as_secs_f64()
                );
                return Err(err);
            }
        };
        rebuild_elapsed = rebuild_started.elapsed();

        if let Some(track) = inner.track.as_mut() {
            track.duration = duration.or(track.duration);
            track.base_position = track_position;
            track.started_at = if was_playing {
                Some(Instant::now())
            } else {
                None
            };
            track.is_playing = was_playing;
        }
    }

    // 旧设备先挂起来，不在这条命令线程上结束它：它的 Drop 要等 mixer 收尾，而那个线程
    // 可能正卡在我们那条没数据的流里（现场量到「切换输出设备占住 35.1s」就是这么来的）。
    // 交给 release_retired_outputs：等它自己报告 sink empty 之后再释放。
    inner.retired_outputs.push(RetiredOutput {
        stream: previous_stream,
        handle: previous_handle,
        sink: previous_sink,
        retired_at: Instant::now(),
    });
    release_retired_outputs(inner);
    crate::log_info!(
        "[AUDIO] 切换输出设备 {target_label}：开新流 {:.2}s，重建管线 {:.2}s",
        open_elapsed.as_secs_f64(),
        rebuild_elapsed.as_secs_f64()
    );
    Ok(snapshot_inner(inner))
}

/// 释放挂起来的旧输出设备。
///
/// 判据是那块旧 sink 是否已经 empty：mixer 把它手上的源处理完（含 stop 后丢弃）之后，
/// `cpal::Stream` 的 Drop 就不再等任何人，这时结束它是安全的。超过宽限期仍没空出来
/// （流真的死了）也强行释放，否则每切一次设备就永久漏一条 mixer 线程和一个设备句柄。
fn release_retired_outputs(inner: &mut AudioThreadInner) {
    let now = Instant::now();
    for retired in std::mem::take(&mut inner.retired_outputs) {
        let sink_empty = retired
            .sink
            .as_ref()
            .map(|sink| sink.empty())
            .unwrap_or(true);
        let waited = now.saturating_duration_since(retired.retired_at);
        if !retired_output_is_releasable(sink_empty, waited, RETIRED_OUTPUT_FORCE_GRACE) {
            inner.retired_outputs.push(retired);
            continue;
        }
        let elapsed = super::device::release_output(retired.handle, retired.stream);
        if waited >= Duration::from_secs(1) || elapsed >= Duration::from_millis(300) {
            crate::log_info!(
                "[AUDIO] 释放旧输出设备：挂了 {:.1}s，实际结束又花了 {:.2}s",
                waited.as_secs_f64(),
                elapsed.as_secs_f64()
            );
        }
    }
}

fn set_replay_gain_inner(
    inner: &mut AudioThreadInner,
    enabled: bool,
    preamp_db: f32,
) -> Result<AudioSnapshot, String> {
    inner.dsp.set_replay_gain(enabled, preamp_db);
    Ok(snapshot_inner(inner))
}

fn set_equalizer_inner(
    inner: &mut AudioThreadInner,
    enabled: bool,
    gains_db: Vec<f32>,
) -> Result<AudioSnapshot, String> {
    inner.dsp.set_equalizer(enabled, gains_db);
    Ok(snapshot_inner(inner))
}

fn set_crossfade_inner(
    inner: &mut AudioThreadInner,
    duration: Duration,
) -> Result<AudioSnapshot, String> {
    inner.crossfade_duration = normalize_crossfade_duration(duration);
    Ok(snapshot_inner(inner))
}

fn stop_inner(inner: &mut AudioThreadInner, request_id: u64) -> Result<AudioSnapshot, String> {
    // 与 begin_load_inner 同源的迟到防护：迟到的旧 Stop 不得把代次倒拨，
    // 否则它会把已经属于新请求的管线/音轨按旧代号清掉（或让新请求的 Load 变"过期"）。
    // 丢弃这条命令、原样返回当前状态即可。
    if should_ignore_stale_command(request_id, inner.request_id) {
        return Ok(snapshot_inner(inner));
    }

    inner.request_id = request_id;
    inner.handoff_deadline = None;
    if let Some(sink) = inner.sink.take() {
        sink.stop();
    }
    stop_fading_sinks(inner);
    inner.track = None;
    Ok(snapshot_inner(inner))
}

fn tick_progress(
    app: &AppHandle,
    inner: &mut AudioThreadInner,
    snapshot_state: &Arc<Mutex<AudioSnapshot>>,
) {
    update_fades(inner);
    // 旧输出设备的释放放在心跳里：一旦发现它已经不再读我们的流（旧 sink empty）就结束掉
    release_retired_outputs(inner);

    if inner.track.is_none() {
        return;
    }

    let snapshot = snapshot_inner(inner);
    let sink_empty = inner
        .sink
        .as_ref()
        .map(|sink| sink.empty())
        .unwrap_or(false);
    let is_playing = inner
        .track
        .as_ref()
        .map(|track| track.is_playing)
        .unwrap_or(false);

    // 「界面在播放、实际没出声」的证据：mixer 回写的位置停住，而墙钟计时照走。
    // 音频线程本身还在转（否则到不了这里），所以卡的是 mixer 线程那一侧。
    if is_playing && !sink_empty {
        if let Some(sink) = inner.sink.as_ref() {
            let stalled_for = note_sink_progress(inner, sink.get_pos(), Instant::now());
            if let Some(stalled_for) = stalled_for {
                crate::log_warn!(
                    "[AUDIO] 疑似无声：界面按墙钟在播放，但底层播放位置已 {:.1}s 没有推进（mixer 没在消费样本，多半卡在流读取等网络）",
                    stalled_for.as_secs_f64()
                );
            }
        }
    }

    let ended = should_mark_ended(
        sink_empty,
        is_playing,
        snapshot.current_time,
        snapshot.duration,
    );

    if ended {
        if let Some(track) = inner.track.as_mut() {
            track.is_playing = false;
            track.started_at = None;
            track.base_position = track
                .duration
                .unwrap_or_else(|| safe_duration_from_secs(snapshot.current_time));
        }
        if let Some(sink) = inner.sink.take() {
            sink.stop();
        }
        let ended_snapshot = snapshot_inner(inner);
        store_snapshot(snapshot_state, ended_snapshot.clone());
        emit_snapshot(app, "ended", ended_snapshot);
    } else {
        store_snapshot(snapshot_state, snapshot.clone());
        emit_snapshot(app, "progress", snapshot);
    }
}

/// 心跳里判定「自然播放结束」。
///
/// `duration <= 0`（时长未知）时**绝不**判结束：错误终止的流（网络中断、
/// 解码失败）同样表现为 sink 变空，若时长未知也照判 ended，会被误当成放完
/// 而自动跳歌——用户感知是「恢复播放后莫名转圈然后切到下一首」。
fn should_mark_ended(
    sink_empty: bool,
    is_playing: bool,
    current_time: f64,
    duration: f64,
) -> bool {
    if !sink_empty || !is_playing || current_time <= 0.25 || duration <= 0.0 {
        return false;
    }
    current_time + 0.25 >= duration
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 快速切歌的核心不变量：迟到的旧命令（BeginLoad / Stop）必须被忽略，
    /// 不得把音频线程的代次倒拨。
    ///
    /// 复现「先点 A 再点 B，而 A 的 player_load 晚于 B 才入队」：
    /// command 队列按入队顺序串行，因此 A 的命令会在 B 之后执行。
    /// 若无条件写 inner.request_id，代次被倒拨回 A，随后 B 真正的 Load
    /// （判据 `request_id < inner.request_id`）被当成过期丢弃
    /// ——最终选中的 B 永远不播，前端停在加载转圈。这正是本次要修的根因。
    #[test]
    fn stale_command_is_ignored_so_epoch_never_reverts() {
        // B(2) 已执行、代次为 2：迟到的 A(1) 必须被忽略
        assert!(should_ignore_stale_command(1, 2));
        // B(2) 自身不得被忽略
        assert!(!should_ignore_stale_command(2, 2));
        // 前进方向照常受理
        assert!(!should_ignore_stale_command(3, 2));
        // 首次请求（代次 0）永远受理
        assert!(!should_ignore_stale_command(1, 0));
    }

    /// 端到端：迟到命令不得倒拨代次，最新请求仍能被受理。
    #[test]
    fn late_begin_and_stop_do_not_revert_epoch() {
        let mut inner = AudioThreadInner::new();

        // B 先执行
        begin_load_inner(&mut inner, 2).expect("begin 2");
        assert_eq!(inner.request_id, 2);
        // A 迟到
        begin_load_inner(&mut inner, 1).expect("stale begin ignored");
        assert_eq!(inner.request_id, 2, "迟到 BeginLoad 不得倒拨代次");

        // 迟到的旧 Stop 同样不得倒拨
        stop_inner(&mut inner, 1).expect("stale stop ignored");
        assert_eq!(inner.request_id, 2, "迟到 Stop 不得倒拨代次");

        // 最新请求 B 的 Load 判据不应命中（不会被视为过期）
        assert!(!(2 < inner.request_id));
    }

    /// WebView 重新加载后前端本地计数从 1 重新开始，而音频线程的代次停在旧值：
    /// 此后**每一条**命令都命中迟到防护被丢弃（音源不解析、Load 不下发），
    /// 表现为点歌全无反应、加载转圈永不落幕。以原生代次作基线后首个点击即可受理。
    #[test]
    fn reloaded_webview_must_seed_its_counter_from_the_native_epoch() {
        let mut inner = AudioThreadInner::new();
        begin_load_inner(&mut inner, 12).expect("begin 12");

        // 重载后从头计数：1 落后于代次 12 → 被当成迟到命令，播放器永久失灵
        assert!(should_ignore_stale_command(1, inner.request_id));

        // 取原生代次作基线后继续递增：本次点击被受理
        let seeded = inner.request_id + 1;
        assert!(!should_ignore_stale_command(seeded, inner.request_id));
        // 而对齐期间到达的迟到命令仍然照丢弃
        assert!(should_ignore_stale_command(1, seeded));
    }

    /// 正常前进方向不受影响。
    #[test]
    fn newer_request_always_advances_epoch() {
        let mut inner = AudioThreadInner::new();
        begin_load_inner(&mut inner, 1).expect("begin 1");
        begin_load_inner(&mut inner, 5).expect("begin 5");
        assert_eq!(inner.request_id, 5);
        stop_inner(&mut inner, 9).expect("stop 9");
        assert_eq!(inner.request_id, 9);
    }

    #[test]
    fn normalizes_crossfade_duration() {
        assert_eq!(normalize_crossfade_duration(Duration::ZERO), Duration::ZERO);
        assert_eq!(
            normalize_crossfade_duration(Duration::from_secs(30)),
            CROSSFADE_MAX_DURATION
        );
    }

    #[test]
    fn expires_handoff_only_after_grace_with_new_source_pending() {
        let now = Instant::now();
        let deadline = Some(now + HANDOFF_GRACE);
        let before = now + Duration::from_millis(100);
        let after = now + HANDOFF_GRACE + Duration::from_millis(250);

        // 未到宽限期：旧歌继续响，不干预
        assert!(!should_expire_handoff(deadline, before, Some(7), 8));
        // 到点且 Load 还没把音轨换成新代号：停掉旧歌
        assert!(should_expire_handoff(deadline, after, Some(7), 8));
        // 到点但新音源已就位：不得误停刚起播的新歌
        assert!(!should_expire_handoff(deadline, after, Some(8), 8));
        // 没有在播的音轨：没有可停的对象
        assert!(!should_expire_handoff(deadline, after, None, 8));
        // 没有挂起交接（普通 Stop / 首次载入）：永不触发
        assert!(!should_expire_handoff(None, after, Some(7), 8));
    }

    /// 「界面在播放、实际没出声」的判定：底层位置不动就报一次，动了就不再报。
    /// 这条检测的存在意义是把「音频线程还在转、卡的是 mixer 那一侧」和「整条线程
    /// 停摆」区分开——后者只会由 SLOW_COMMAND 那条日志（命令跑完才打）留下证据，
    /// 卡住期间一声不响。
    #[test]
    fn frozen_mixer_position_is_reported_once_per_stall() {
        let mut inner = AudioThreadInner::new();
        let started = Instant::now();

        // 第一拍建立基线
        assert_eq!(
            note_sink_progress(&mut inner, Duration::from_millis(100), started),
            None
        );

        // 位置照常推进：不算无声
        let advanced_at = started + Duration::from_millis(250);
        assert_eq!(
            note_sink_progress(&mut inner, Duration::from_millis(400), advanced_at),
            None
        );

        // 同一位置但还没过阈值
        assert_eq!(
            note_sink_progress(
                &mut inner,
                Duration::from_millis(400),
                advanced_at + SILENCE_LOG_THRESHOLD - Duration::from_millis(1)
            ),
            None,
            "阈值之内不该报"
        );

        // 跨过阈值：报一次，报的是「距离最后一次推进」停了多久
        let over = advanced_at + SILENCE_LOG_THRESHOLD + Duration::from_millis(500);
        let stalled = note_sink_progress(&mut inner, Duration::from_millis(400), over);
        assert_eq!(stalled, Some(SILENCE_LOG_THRESHOLD + Duration::from_millis(500)));

        // 同一次停滞不得刷屏
        assert_eq!(
            note_sink_progress(&mut inner, Duration::from_millis(400), over + Duration::from_secs(30)),
            None,
            "同一次停滞只报一条"
        );

        // 恢复推进后重新武装：再次停滞可以再报
        assert_eq!(
            note_sink_progress(
                &mut inner,
                Duration::from_millis(900),
                over + Duration::from_secs(31)
            ),
            None
        );
        assert!(
            note_sink_progress(
                &mut inner,
                Duration::from_millis(900),
                over + Duration::from_secs(31) + SILENCE_LOG_THRESHOLD
            )
            .is_some(),
            "恢复过一次之后应该能再次报出来"
        );

        // 阈值必须短到能在用户还没放弃之前留下日志，又要长过正常缓冲抖动
        assert!(SILENCE_LOG_THRESHOLD >= Duration::from_millis(500));
        assert!(SILENCE_LOG_THRESHOLD < SEEK_FEEDBACK_TIMEOUT * 10);
    }

    /// 旧输出设备只能在「它的 mixer 不再读我们的流」之后结束。
    /// 反过来（在命令线程上直接 Drop）就是现场量到的「切换输出设备占住 35.1s」。
    #[test]
    fn a_retired_device_is_released_only_when_its_mixer_is_done() {
        use rodio::buffer::SamplesBuffer;

        let mut inner = AudioThreadInner::new();

        // 还在被 mixer 消费的旧 sink：不能在这儿等它
        let (busy_sink, _queue) = Sink::new_idle();
        busy_sink.append(SamplesBuffer::new(1, 44_100, vec![0i16; 44_100 * 60]));
        inner.retired_outputs.push(RetiredOutput {
            stream: None,
            handle: None,
            sink: Some(Arc::new(busy_sink)),
            retired_at: Instant::now(),
        });
        release_retired_outputs(&mut inner);
        assert_eq!(
            inner.retired_outputs.len(),
            1,
            "mixer 还在用它时必须挂着"
        );

        // 已经空出来的旧 sink：当场释放
        let (idle_sink, _idle_queue) = Sink::new_idle();
        inner.retired_outputs.push(RetiredOutput {
            stream: None,
            handle: None,
            sink: Some(Arc::new(idle_sink)),
            retired_at: Instant::now(),
        });
        release_retired_outputs(&mut inner);
        assert_eq!(
            inner.retired_outputs.len(),
            1,
            "只该结束空出来的那一块，剩下的继续挂"
        );

        // 到宽限期：没空出来也必须放行，否则每切一次设备就永久漏一条线程 + 一个设备句柄
        inner.retired_outputs[0].retired_at = Instant::now() - RETIRED_OUTPUT_FORCE_GRACE;
        release_retired_outputs(&mut inner);
        assert!(
            inner.retired_outputs.is_empty(),
            "超过宽限期要强行结束，不能无限期挂着"
        );

        // 判定本身
        assert!(retired_output_is_releasable(true, Duration::ZERO, RETIRED_OUTPUT_FORCE_GRACE));
        assert!(!retired_output_is_releasable(
            false,
            Duration::from_secs(1),
            RETIRED_OUTPUT_FORCE_GRACE
        ));
        assert!(retired_output_is_releasable(
            false,
            RETIRED_OUTPUT_FORCE_GRACE,
            RETIRED_OUTPUT_FORCE_GRACE
        ));
    }

    #[test]
    fn marks_ended_only_when_sink_empty_near_known_duration() {
        // 正常自然结束：sink 空 + 播放中 + 接近已知时长
        assert!(should_mark_ended(true, true, 219.9, 220.0));
        assert!(should_mark_ended(true, true, 220.0, 220.0));

        // sink 还有数据 / 未在播放：不算结束
        assert!(!should_mark_ended(false, true, 219.9, 220.0));
        assert!(!should_mark_ended(true, false, 219.9, 220.0));

        // 离结尾还远：错误终止的流（网络中断）不得被判成放完
        assert!(!should_mark_ended(true, true, 30.0, 220.0));

        // 时长未知（duration <= 0）：一律不判结束——错误终止与正常结束
        // 在 sink 层不可区分，误判会触发自动跳歌
        assert!(!should_mark_ended(true, true, 100.0, 0.0));

        // 刚起播就断流：位置过小不判
        assert!(!should_mark_ended(true, true, 0.1, 220.0));
    }

    /// 「进度在走、没声音、连输出设备都切不动」的根因守卫：`Sink::try_seek` 会阻塞
    /// 等 mixer 线程回信，mixer 卡在流读取里时这条等待没有上限，音频线程跟着停摆。
    /// 这里造一个**没人消费的 sink**（等价于 mixer 永远不回信），要求它必须在时限内
    /// 放弃并报错——绝不许无限等下去。
    #[test]
    fn seek_gives_up_instead_of_waiting_for_a_stalled_mixer() {
        use rodio::buffer::SamplesBuffer;

        // queue 接收端不读 = mixer 线程没有推进，seek 的回信永远不来
        let (sink, _queue) = Sink::new_idle();
        sink.append(SamplesBuffer::new(1, 44_100, vec![0i16; 44_100 * 60]));
        let sink = Arc::new(sink);

        let started = Instant::now();
        let error = seek_sink_with_timeout(&sink, Duration::from_secs(30))
            .expect_err("mixer 不回信时 try_seek 必须超时放弃");
        let elapsed = started.elapsed();

        assert!(
            error.contains("did not report back"),
            "超时应给出「没等到回信」的原因，实际：{error}"
        );
        assert!(
            elapsed >= SEEK_FEEDBACK_TIMEOUT,
            "不该比时限更早放弃，实际 {elapsed:?}"
        );
        // 时限必须明显短于命令回包超时（20s），否则调用方早就当成彻底卡死了
        assert!(SEEK_FEEDBACK_TIMEOUT < Duration::from_secs(20));
    }
}
