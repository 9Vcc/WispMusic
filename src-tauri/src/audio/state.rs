use rodio::{OutputStream, OutputStreamHandle, Sink};
use std::{
    sync::{atomic::AtomicU32, Arc},
    time::{Duration, Instant},
};

use super::dsp::{DspState, SPECTRUM_BANDS};
use super::source::AudioSource;
use super::tempo::SpeedControl;

pub(super) struct TrackState {
    pub(super) source: AudioSource,
    pub(super) request_id: u64,
    pub(super) duration: Option<Duration>,
    pub(super) base_position: Duration,
    pub(super) started_at: Option<Instant>,
    pub(super) is_playing: bool,
}

/// 音频线程此刻在忙什么。
///
/// 只由音频线程自己写、由诊断命令读，**不进命令队列**——这是队列停摆时唯一还能
/// 问出真相的口子：某条命令一旦把线程占住，此后每条 `player_*` 都要等满 20s 回包
/// 超时，界面看上去是「彻底卡死、连输出设备都切不动」，而快照与这个状态都还读得到。
/// 读写两侧都只在进入/离开命令的瞬间加锁，被占住的那段时间锁是放开的，所以诊断
/// 命令不会跟着一起卡住。
pub(super) struct AudioThreadBusy {
    /// 正在处理的命令种类与开始时刻；None 表示线程空闲在等队列
    pub(super) current: Option<(&'static str, Instant)>,
    /// 最近一条处理完的命令：种类、耗时、完成时刻
    pub(super) last_done: Option<(&'static str, Duration, Instant)>,
    /// 线程最后一次回到事件循环的时刻与累计轮数（心跳停了就说明线程根本没在转）
    pub(super) looped_at: Instant,
    pub(super) loops: u64,
}

impl AudioThreadBusy {
    pub(super) fn new() -> Self {
        Self {
            current: None,
            last_done: None,
            looped_at: Instant::now(),
            loops: 0,
        }
    }
}

pub(super) struct FadingSink {
    pub(super) sink: Arc<Sink>,
    pub(super) started_at: Instant,
    pub(super) duration: Duration,
    pub(super) start_volume: f32,
}

pub(super) struct FadeInState {
    pub(super) started_at: Instant,
    pub(super) duration: Duration,
    pub(super) target_volume: f32,
}

pub(super) struct AudioThreadInner {
    pub(super) stream: Option<OutputStream>,
    pub(super) handle: Option<OutputStreamHandle>,
    /// Arc 是为了能把 `try_seek` 挪到一次性线程里去等（见 engine::seek_sink_with_timeout）：
    /// 它内部要阻塞等 mixer 线程回信，而 mixer 可能正卡在我们的流读取里。
    pub(super) sink: Option<Arc<Sink>>,
    pub(super) track: Option<TrackState>,
    pub(super) volume: f32,
    /// 变速倍率（0.1 ~ 5.0），1.0 = 原速；位置推进按此缩放
    pub(super) speed: f32,
    /// 变速控制句柄，与当前 sink 内 TempoSource 共享
    pub(super) speed_control: Arc<SpeedControl>,
    pub(super) audio_level: Arc<AtomicU32>,
    pub(super) audio_spectrum: Arc<[AtomicU32; SPECTRUM_BANDS]>,
    pub(super) dsp: Arc<DspState>,
    pub(super) crossfade_duration: Duration,
    pub(super) fading_sinks: Vec<FadingSink>,
    pub(super) fade_in: Option<FadeInState>,
    pub(super) request_id: u64,
    /// 「就绪前硬切」的宽限截止时刻：BeginLoad 保留旧 sink 等待新音源时挂上，
    /// 到点仍没等到对应的 Load 就主动停掉旧歌（见 engine::expire_pending_handoff）。
    pub(super) handoff_deadline: Option<Instant>,
    /// mixer 每 5ms 回写的播放位置，以及它最后一次发生变化的时刻。
    /// 位置不再推进而墙钟计时照走 = 界面显示在播放、实际没出声（见 engine::note_sink_progress）。
    pub(super) sink_position: Option<Duration>,
    pub(super) sink_position_changed_at: Option<Instant>,
    /// 同一次停滞只报一条日志，位置重新推进后再武装
    pub(super) silence_reported: bool,
    pub(super) selected_device_id: Option<String>,
}

impl AudioThreadInner {
    pub(super) fn new() -> Self {
        Self {
            stream: None,
            handle: None,
            sink: None,
            track: None,
            volume: 0.7,
            speed: 1.0,
            speed_control: Arc::new(SpeedControl::default()),
            audio_level: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            audio_spectrum: Arc::new(std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits()))),
            dsp: Arc::new(DspState::new()),
            crossfade_duration: Duration::ZERO,
            fading_sinks: Vec::new(),
            fade_in: None,
            request_id: 0,
            handoff_deadline: None,
            sink_position: None,
            sink_position_changed_at: None,
            silence_reported: false,
            selected_device_id: Option::None,
        }
    }
}

pub(super) fn track_position(track: &TrackState, speed: f32) -> Duration {
    let mut position = track.base_position;
    if track.is_playing {
        if let Some(started_at) = track.started_at {
            position = position.saturating_add(started_at.elapsed().mul_f32(speed));
        }
    }

    if let Some(duration) = track.duration {
        position.min(duration)
    } else {
        position
    }
}
