use rodio::{Sink, Source};
use std::{
    panic::{self, AssertUnwindSafe},
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use super::device::open_output;
use super::dsp::{DspState, LevelMeter, SPECTRUM_BANDS};
use super::source::{decode_file_source, AudioSource, ReplayGainInfo};
use super::state::{AudioThreadInner, FadeInState, FadingSink};
use super::streaming::{decode_streaming_reader, BoxedAudioSource};
use super::tempo::{SpeedControl, TempoSource};

pub(super) fn rebuild_sink(
    inner: &mut AudioThreadInner,
    position: Duration,
    is_playing: bool,
    crossfade_old_sink: bool,
) -> Result<Option<Duration>, String> {
    let source = inner
        .track
        .as_ref()
        .map(|track| track.source.clone())
        .ok_or_else(|| "no loaded audio source".to_string())?;

    ensure_output(inner)?;
    let handle = inner
        .handle
        .as_ref()
        .ok_or_else(|| "audio output handle is not initialized".to_string())?;
    let sink = Sink::try_new(handle).map_err(|err| format!("create audio sink failed: {err}"))?;
    let should_crossfade = crossfade_old_sink
        && is_playing
        && inner.sink.is_some()
        && !inner.crossfade_duration.is_zero();
    sink.set_volume(if should_crossfade { 0.0 } else { inner.volume });

    reset_audio_analysis(inner);
    let duration = append_source(
        &sink,
        &source,
        position,
        Arc::clone(&inner.audio_level),
        Arc::clone(&inner.audio_spectrum),
        Arc::clone(&inner.dsp),
        inner.speed,
        Arc::clone(&inner.speed_control),
    )?;
    if !is_playing {
        sink.pause();
    }

    // 新 sink 已经就绪（源也定位完成）之后才停/淡出旧 sink——旧实现在
    // set_output_device_inner 里提前把旧 sink 停掉，导致「建源 + 定位」的全部
    // 耗时都落在静音窗口内。
    if let Some(old_sink) = inner.sink.take() {
        if should_crossfade {
            inner.fading_sinks.push(FadingSink {
                sink: old_sink,
                started_at: Instant::now(),
                duration: inner.crossfade_duration,
                start_volume: inner.volume,
            });
            inner.fade_in = Some(FadeInState {
                started_at: Instant::now(),
                duration: inner.crossfade_duration,
                target_volume: inner.volume,
            });
        } else {
            old_sink.stop();
            stop_fading_sinks(inner);
        }
    }
    inner.sink = Some(sink);
    Ok(duration)
}

pub(super) fn update_fades(inner: &mut AudioThreadInner) {
    let now = Instant::now();

    if let Some(fade_in) = inner.fade_in.as_ref() {
        let progress = fade_progress(now, fade_in.started_at, fade_in.duration);
        if let Some(sink) = inner.sink.as_ref() {
            sink.set_volume(fade_in.target_volume * progress);
        }
        if progress >= 1.0 {
            if let Some(sink) = inner.sink.as_ref() {
                sink.set_volume(fade_in.target_volume);
            }
            inner.fade_in = None;
        }
    }

    inner.fading_sinks.retain_mut(|fade| {
        let progress = fade_progress(now, fade.started_at, fade.duration);
        let volume = fade.start_volume * (1.0 - progress);
        fade.sink.set_volume(volume.max(0.0));
        if progress >= 1.0 || fade.sink.empty() {
            fade.sink.stop();
            false
        } else {
            true
        }
    });
}

pub(super) fn stop_fading_sinks(inner: &mut AudioThreadInner) {
    inner.fade_in = None;
    for fade in inner.fading_sinks.drain(..) {
        fade.sink.stop();
    }
}

fn fade_progress(now: Instant, started_at: Instant, duration: Duration) -> f32 {
    if duration.is_zero() {
        return 1.0;
    }

    (now.saturating_duration_since(started_at).as_secs_f32() / duration.as_secs_f32())
        .clamp(0.0, 1.0)
}

fn ensure_output(inner: &mut AudioThreadInner) -> Result<(), String> {
    if inner.handle.is_some() {
        return Ok(());
    }

    let (stream, handle) = open_output(inner.selected_device_id.as_deref())?;
    inner.stream = Some(stream);
    inner.handle = Some(handle);
    Ok(())
}

/// 把源挂到 sink 上，并保证它**从 `position` 开始**产出样本。
///
/// 定位优先走 demuxer 级 seek（`Source::try_seek`，毫秒级）——注意是在挂载**之前**
/// 对源本身做的，所以源一开始产出就在目标位置，不会漏出文件开头的声音，也不需要
/// 事后在 sink 上补一次 seek。
///
/// 旧实现无条件用 `decoder.skip_duration(position)`：rodio 0.20 的 `skip_duration`
/// 在**构造时**就同步执行整个跳过（`skip_samples` 里逐样本 `input.next()` 后丢弃），
/// 成本随播放位置线性增长——长无损曲目中后段定位要解码数分钟音频，播放线程被占住
/// 就是秒级静音（切设备、seek 回退路径都会中招）。
fn append_source(
    sink: &Sink,
    source: &AudioSource,
    position: Duration,
    audio_level: Arc<AtomicU32>,
    audio_spectrum: Arc<[AtomicU32; SPECTRUM_BANDS]>,
    dsp: Arc<DspState>,
    speed: f32,
    speed_control: Arc<SpeedControl>,
) -> Result<Option<Duration>, String> {
    match source {
        AudioSource::File { path, .. } => append_positioned_source(
            sink,
            || decode_file_source(path),
            position,
            audio_level,
            audio_spectrum,
            source.replay_gain(),
            dsp,
            speed,
            speed_control,
        ),
        AudioSource::Stream {
            reader,
            format,
            label,
            ..
        } => append_positioned_source(
            sink,
            || {
                decode_streaming_reader(reader.clone(), *format).map_err(|err| {
                    format!(
                        "decode streaming {} audio {label} failed: {err}",
                        format.label()
                    )
                })
            },
            position,
            audio_level,
            audio_spectrum,
            source.replay_gain(),
            dsp,
            speed,
            speed_control,
        ),
    }
}

/// 把「在音频线程上碰解码器」这段工作包起来。
///
/// 为什么必须包：rodio 0.20.1 在 symphonia 初始化拿到 `Error::SeekError` 时直接
/// `unreachable!()`（`rodio-0.20.1/src/decoder/symphonia.rs:45`），而这段代码跑在
/// **音频线程**上。线程一死，播放能力整个会话都回不来——现象是「没声音、连输出
/// 设备都切不动」，而且因为命令根本没被卡住（是线程没了），连慢命令日志都不会留下。
/// 真机日志里就抓到过这条 panic。畸形/半残的第三方音源不该有这种权限：宁可这一次
/// 载入失败，让前端按普通报错处理。
fn guard_decoder_panic<T>(
    label: &str,
    work: impl FnOnce() -> T + panic::UnwindSafe,
) -> Result<T, String> {
    panic::catch_unwind(work).map_err(|_| format!("audio decoder panicked while {label}"))
}

/// 建解码器 → 定位 → 包变速层 → 挂到 sink。
///
/// `create_decoder` 是闭包而不是现成解码器：回退路径需要**第二个干净的解码器**
/// （失败的 seek 可能把原解码器留在不确定状态），且首次构造才能真正丢弃重来。
fn append_positioned_source<F>(
    sink: &Sink,
    create_decoder: F,
    position: Duration,
    audio_level: Arc<AtomicU32>,
    audio_spectrum: Arc<[AtomicU32; SPECTRUM_BANDS]>,
    replay_gain: Option<ReplayGainInfo>,
    dsp: Arc<DspState>,
    speed: f32,
    speed_control: Arc<SpeedControl>,
) -> Result<Option<Duration>, String>
where
    F: Fn() -> Result<BoxedAudioSource, String>,
{
    let decoder = guard_decoder_panic("initializing decoder", AssertUnwindSafe(&create_decoder))??;
    let duration = decoder.total_duration();

    let mut positioned = wrap_with_tempo(decoder, speed, Arc::clone(&speed_control));
    // 位置为 0 时压根不 seek（保持原有短路：新建的解码器本来就在开头）
    let seek_ok = position.is_zero()
        || guard_decoder_panic(
            "seeking decoder",
            AssertUnwindSafe(|| positioned.try_seek(position).is_ok()),
        )?;
    if !seek_ok {
        // 回退路径：正常不会走到——LevelMeter → TempoSource → decoder 三层都转发了
        // try_seek（dsp.rs / tempo.rs / mp4_decoder.rs），Symphonia 解码器也支持。
        // 走到这里说明该源确实不支持 demuxer 级 seek，只能退回解码跳过。
        let decoder = guard_decoder_panic(
            "reinitializing decoder for skip",
            AssertUnwindSafe(&create_decoder),
        )??;
        positioned = guard_decoder_panic(
            "skipping to position",
            AssertUnwindSafe(|| {
                wrap_with_tempo(decoder.skip_duration(position), speed, speed_control)
            }),
        )?;
    }

    sink.append(LevelMeter::new(
        positioned,
        audio_level,
        audio_spectrum,
        replay_gain,
        dsp,
    ));
    Ok(duration)
}

/// 变速倍率非 1.0 时包一层 SoundTouch 变速源（不变调）；
/// skip_duration 在音频时间域执行，必须先 skip 再 tempo。
/// 始终包一层变速源：速度 1.0 时 TempoSource 内部直通，非 1.0 时走
/// SoundTouch 变速。这样任意时刻改速度都通过 SpeedControl 实时生效，
/// 不需要为「1.0 ↔ 其他」的边界重建音频管线。skip_duration 必须在
/// 音频时间域先执行（先 skip 再 tempo）。
fn wrap_with_tempo<S>(
    source: S,
    speed: f32,
    speed_control: Arc<SpeedControl>,
) -> Box<dyn Source<Item = i16> + Send>
where
    S: Source<Item = i16> + Send + 'static,
{
    Box::new(TempoSource::new(source, speed_control, speed))
}

pub(super) fn reset_audio_analysis(inner: &AudioThreadInner) {
    inner.audio_level.store(0.0f32.to_bits(), Ordering::Relaxed);
    for band in inner.audio_spectrum.iter() {
        band.store(0.0f32.to_bits(), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// rodio 在解码器初始化拿到 SeekError 时会 `unreachable!()` panic（真机日志里
    /// 抓到过原文），而这段代码跑在音频线程上：panic 等于把整个会话的播放能力打死
    /// ——没声音、切不动输出设备，还不留任何慢命令日志。所以必须转成一次普通失败。
    /// （测试里那条 panic 消息会照常打到 stderr，这是默认 panic hook 的行为，不是漏网）
    #[test]
    fn decoder_panic_becomes_an_error_instead_of_a_dead_thread() {
        let outcome = guard_decoder_panic("initializing decoder", AssertUnwindSafe(|| -> Result<u32, String> {
            panic!("simulated rodio unreachable!()");
        }));

        let message = outcome.expect_err("panic 必须被转成 Err，而不是穿出音频线程");
        assert!(message.contains("panicked while initializing decoder"));

        // 正常路径的语义与错误透传都不该被这层包装改变
        assert_eq!(
            guard_decoder_panic(
                "initializing decoder",
                AssertUnwindSafe(|| Ok::<u32, String>(7u32)),
            )
            .unwrap()
            .unwrap(),
            7
        );
        assert_eq!(
            guard_decoder_panic(
                "initializing decoder",
                AssertUnwindSafe(|| Err::<u32, String>("bad format".to_string()))
            )
            .unwrap()
            .unwrap_err(),
            "bad format"
        );
    }
}
