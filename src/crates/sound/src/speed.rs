//! Wisp 原版同款变速不变调：vendored SoundTouch（WSOLA）。
//!
//! 1× 时完全旁路，非 1× 时把解码样本喂给 SoundTouch 的 tempo 引擎，
//! pitch/rate 保持 1.0，只改 tempo，因此音高不变（与 Wisp 的 TempoSource 一致）。

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use rodio::{Source, source::SeekError};
use soundtouch_rs::{InterpolationAlgorithm, SoundTouch, SoundTouchPreset};

const FEED_FRAMES: usize = 4096;
const MIN_SPEED: f32 = 0.1;
const MAX_SPEED: f32 = 5.0;

/// 跨线程速度控制：命令线程写目标速度，音频线程在下一次迭代中就地应用，
/// 全程不重建管线、不重开曲目（Wisp 的 SpeedControl 口径）。
#[derive(Debug, Default)]
pub struct SpeedControl {
    // f32 的位模式；0 表示没有待应用的新速度
    pending: AtomicU32,
    /// The tempo the source is actually playing, which the engine scales the heard position
    /// by: one second of output covers this many seconds of the track.
    applied: AtomicU32,
}

impl SpeedControl {
    pub fn request(&self, speed: f32) {
        self.pending.store(speed.to_bits(), Ordering::Release);
    }

    /// The tempo the last sample was produced at, one until a source reports otherwise.
    pub fn applied(&self) -> f32 {
        match self.applied.load(Ordering::Acquire) {
            0 => 1.0,
            bits => f32::from_bits(bits),
        }
    }

    fn mark_applied(&self, speed: f32) {
        self.applied.store(speed.to_bits(), Ordering::Release);
    }

    fn take(&self) -> Option<f32> {
        let bits = self.pending.swap(0, Ordering::AcqRel);
        (bits != 0).then(|| f32::from_bits(bits))
    }
}

/// 音频管线的速度源；1× 时内部直接透传，非 1× 交给 SoundTouch。
pub type SpeedSource<I> = TempoSource<I>;

/// 把解码器的 f32 样本按帧喂给 SoundTouch，并把输出交错回 f32。
pub struct TempoSource<S> {
    inner: S,
    engine: SoundTouch,
    control: Arc<SpeedControl>,
    speed: f32,
    channels: usize,
    input_planar: Vec<Vec<f32>>,
    output_planar: Vec<Vec<f32>>,
    pending_out: VecDeque<f32>,
    drained: bool,
}

impl<S> TempoSource<S>
where
    S: Source<Item = f32>,
{
    pub fn new(inner: S, speed: f32, control: Arc<SpeedControl>) -> Self {
        let channels = inner.channels().get().max(1) as usize;
        let sample_rate = inner.sample_rate().get().max(1) as usize;
        let speed = speed.clamp(MIN_SPEED, MAX_SPEED);
        control.mark_applied(speed);
        let engine = SoundTouch::builder(channels, sample_rate)
            .tempo(speed as f64)
            .pitch(1.0)
            .rate(1.0)
            .preset(SoundTouchPreset::Music)
            .interpolation_algo(InterpolationAlgorithm::Shannon)
            .build()
            .unwrap_or_else(|_| {
                SoundTouch::builder(channels, sample_rate)
                    .tempo(1.0)
                    .build()
                    .expect("default soundtouch engine")
            });
        Self {
            inner,
            engine,
            control,
            speed,
            channels,
            input_planar: vec![Vec::with_capacity(FEED_FRAMES); channels],
            // receive_samples 按切片长度写入，缓冲必须保持固定长度
            output_planar: vec![vec![0.0; FEED_FRAMES]; channels],
            pending_out: VecDeque::new(),
            drained: false,
        }
    }

    fn current_span_len(&self) -> Option<usize> {
        Some(self.pending_out.len())
    }

    /// 就地改变 tempo，丢弃引擎旧速缓存；不打断当前播放。
    fn apply_speed(&mut self, speed: f32) {
        let speed = speed.clamp(MIN_SPEED, MAX_SPEED);
        if (self.speed - speed).abs() < f32::EPSILON {
            return;
        }
        self.speed = speed;
        self.control.mark_applied(speed);
        self.drained = false;
        let guard = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.engine.set_tempo(speed as f64);
            let _ = self.engine.clear();
        }));
        if guard.is_err() {
            log::warn!("speed: soundtouch rejected {speed}x, keeping the old tempo");
        }
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn receive_into_pending(&mut self) -> bool {
        let mut any = false;
        while let Ok(frames) = self.engine.receive_samples(&mut self.output_planar) {
            // 缓冲保持固定长度，只读取返回帧数之前的部分
            if frames == 0 {
                break;
            }
            any = true;
            for index in 0..frames {
                for channel in 0..self.channels {
                    let sample = self.output_planar[channel][index].clamp(-1., 1.);
                    self.pending_out.push_back(sample);
                }
            }
        }
        any
    }

    fn process_chunk(&mut self) {
        // 原速直通：不进引擎，零延迟零损耗；切到非 1× 时由 next() 的 apply_speed 唤醒
        if (self.speed - 1.0).abs() < f32::EPSILON {
            let mut frames = 0usize;
            while frames < FEED_FRAMES {
                match self.inner.next() {
                    Some(sample) => self.pending_out.push_back(sample),
                    None => break,
                }
                frames += 1;
            }
            if frames == 0 {
                self.drained = true;
            }
            return;
        }
        for channel in self.input_planar.iter_mut() {
            channel.clear();
        }
        let mut frames = 0usize;
        let mut frame = Vec::with_capacity(self.channels);
        while frames < FEED_FRAMES {
            frame.clear();
            for _ in 0..self.channels {
                match self.inner.next() {
                    Some(sample) => frame.push(sample),
                    None => break,
                }
            }
            if frame.len() < self.channels {
                break;
            }
            for (channel, sample) in frame.iter().take(self.channels).enumerate() {
                self.input_planar[channel].push(*sample);
            }
            frames += 1;
        }
        if frames == 0 {
            let _ = self.engine.flush();
            self.receive_into_pending();
            self.drained = true;
            return;
        }
        if self.engine.put_samples(&self.input_planar).is_err() {
            self.drained = true;
            return;
        }
        let produced = self.receive_into_pending();
        if !produced && frames < FEED_FRAMES {
            let _ = self.engine.flush();
            self.receive_into_pending();
            self.drained = true;
        }
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let result = self.inner.try_seek(pos);
        let _ = self.engine.clear();
        self.pending_out.clear();
        self.drained = false;
        result
    }
}

impl<S> Iterator for TempoSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        loop {
            if let Some(sample) = self.pending_out.pop_front() {
                return Some(sample);
            }
            if self.drained {
                return None;
            }
            if let Some(new_speed) = self.control.take() {
                self.apply_speed(new_speed);
                continue;
            }
            self.process_chunk();
        }
    }
}

impl<S> Source for TempoSource<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        Some(self.pending_out.len())
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        TempoSource::try_seek(self, pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sine {
        at: usize,
        rate: u32,
        total: usize,
    }

    impl Iterator for Sine {
        type Item = f32;
        fn next(&mut self) -> Option<f32> {
            if self.at >= self.total {
                return None;
            }
            let value =
                (2. * std::f64::consts::PI * 440. * self.at as f64 / self.rate as f64).sin() as f32;
            self.at += 1;
            Some(value)
        }
    }

    impl Source for Sine {
        fn current_span_len(&self) -> Option<usize> {
            Some(self.total - self.at)
        }
        fn channels(&self) -> rodio::ChannelCount {
            std::num::NonZero::new(1).unwrap()
        }
        fn sample_rate(&self) -> rodio::SampleRate {
            std::num::NonZero::new(self.rate).unwrap()
        }
        fn total_duration(&self) -> Option<Duration> {
            None
        }
    }

    #[test]
    fn speed_one_is_a_true_bypass() {
        let source = Sine {
            at: 0,
            rate: 44_100,
            total: 44_100,
        };
        let played: Vec<f32> =
            SpeedSource::new(source, 1.0, Arc::new(SpeedControl::default())).collect();
        assert_eq!(played.len(), 44_100);
        let crossings = played
            .windows(2)
            .filter(|pair| pair[0] < 0. && pair[1] >= 0.)
            .count();
        assert!((crossings as i64 - 440).abs() < 30, "crossings {crossings}");
    }

    #[test]
    fn speed_two_keeps_the_tone() {
        let source = Sine {
            at: 0,
            rate: 44_100,
            total: 44_100 * 4,
        };
        let played: Vec<f32> =
            SpeedSource::new(source, 2.0, Arc::new(SpeedControl::default())).collect();
        assert!(played.len() > 44_100 && played.len() < 44_100 * 5);
        let crossings = played
            .windows(2)
            .filter(|pair| pair[0] < 0. && pair[1] >= 0.)
            .count();
        // 时长约减半，但 440Hz 的音高保持（过零率与 1× 相同）
        let per_second = crossings as f64 / (played.len() as f64 / 44_100.);
        assert!((per_second - 440.).abs() < 60., "hz estimate {per_second}");
    }
}
