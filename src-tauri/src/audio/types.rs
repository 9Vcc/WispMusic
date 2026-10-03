use serde::Serialize;

use super::dsp::{EQUALIZER_BANDS, SPECTRUM_BANDS};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSnapshot {
    pub(super) request_id: u64,
    pub(super) is_ready: bool,
    pub(super) is_playing: bool,
    pub(super) current_time: f64,
    pub(super) duration: f64,
    pub(super) volume: f32,
    pub(super) speed: f32,
    pub(super) source: Option<String>,
    pub(super) audio_level: f32,
    pub(super) audio_spectrum: Vec<f32>,
    pub(super) replay_gain_db: Option<f32>,
    pub(super) replay_gain_applied: bool,
    pub(super) replay_gain_preamp_db: f32,
    pub(super) equalizer_enabled: bool,
    pub(super) equalizer_gains_db: Vec<f32>,
    pub(super) crossfade_duration: f64,
}

impl AudioSnapshot {
    pub(super) fn idle(volume: f32, request_id: u64) -> Self {
        Self {
            request_id,
            is_ready: false,
            is_playing: false,
            current_time: 0.0,
            duration: 0.0,
            volume,
            speed: 1.0,
            source: None,
            audio_level: 0.0,
            audio_spectrum: vec![0.0; SPECTRUM_BANDS],
            replay_gain_db: None,
            replay_gain_applied: false,
            replay_gain_preamp_db: 0.0,
            equalizer_enabled: false,
            equalizer_gains_db: vec![0.0; EQUALIZER_BANDS],
            crossfade_duration: 0.0,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreloadResult {
    pub(super) request_id: u64,
    pub(super) is_ready: bool,
    pub(super) source: Option<String>,
    pub(super) duration: f64,
}

/// 音频线程体检结果。**不进命令队列**：线程被某条命令占住、队列整体停摆时，
/// 这是唯一还能问出「此刻在忙什么、忙了多久」的入口。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioThreadStatus {
    /// 正在处理的命令种类；None = 线程空闲在等下一条命令
    pub busy_with: Option<String>,
    /// 这条命令已经占了多久（秒）
    pub busy_secs: f64,
    pub last_command: Option<String>,
    pub last_command_secs: f64,
    /// 距离线程最后一次回到事件循环过去了多久（毫秒）。心跳长期停在这里
    /// 就说明线程根本没在转，而不是命令队列排队
    pub loop_age_ms: u64,
    pub loops: u64,
}

#[derive(Clone, Serialize)]
pub(super) struct AudioEvent {
    kind: String,
    snapshot: Option<AudioSnapshot>,
    message: Option<String>,
}

impl AudioEvent {
    pub(super) fn snapshot(kind: &str, snapshot: AudioSnapshot) -> Self {
        Self {
            kind: kind.to_string(),
            snapshot: Some(snapshot),
            message: None,
        }
    }
}
