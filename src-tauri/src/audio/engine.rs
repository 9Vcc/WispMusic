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
use super::state::{track_position, AudioThreadBusy, AudioThreadInner, TrackState};
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
fn command_kind(command: &AudioCommand) -> &'static str {
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

    let (stream, handle) = open_output(selected_device_id.as_deref())?;

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
    let previous_stream = inner.stream.replace(stream);
    let previous_handle = inner.handle.replace(handle);
    inner.selected_device_id = selected_device_id;

    if inner.track.is_some() {
        let duration = match rebuild_sink(inner, track_position, was_playing, false) {
            Ok(duration) => duration,
            Err(err) => {
                // 回滚：rebuild_sink 只在建源成功后才停旧 sink，失败时旧 sink 与旧流
                // 都还有效，放回去即可继续在原设备上出声，不至于两边都没声音
                inner.handle = previous_handle;
                inner.stream = previous_stream;
                inner.selected_device_id = previous_device_id;
                return Err(err);
            }
        };

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

    drop(previous_handle);
    drop(previous_stream);
    Ok(snapshot_inner(inner))
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
