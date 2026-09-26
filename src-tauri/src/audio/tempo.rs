use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};

use rodio::{source::SeekError, Source};
use soundtouch_rs::{InterpolationAlgorithm, SoundTouch, SoundTouchPreset};

pub(super) const MIN_SPEED: f32 = 0.1;
pub(super) const MAX_SPEED: f32 = 5.0;

const FEED_FRAMES: usize = 4096;

/// 跨线程速度控制：命令线程写入目标速度，sink 填充线程在下一次迭代中应用，
/// 避免运行中重建音频管线。
#[derive(Default)]
pub(super) struct SpeedControl {
    // f32 的位模式；0 表示没有待应用的新速度
    pending: AtomicU32,
}

impl SpeedControl {
    pub(super) fn request(&self, speed: f32) {
        self.pending.store(speed.to_bits(), Ordering::Release);
    }

    fn take(&self) -> Option<f32> {
        let bits = self.pending.swap(0, Ordering::AcqRel);
        if bits == 0 {
            None
        } else {
            Some(f32::from_bits(bits))
        }
    }
}

/// 基于 SoundTouch（WSOLA）的变速不变调 Source。链路顺序必须是
/// `decoder.skip_duration(..) -> TempoSource -> LevelMeter`：skip 发生在
/// 音频时间域，过 tempo 之后再 skip 会因输出时长被缩放而错位。
pub(super) struct TempoSource<S> {
    inner: S,
    engine: SoundTouch,
    channels: usize,
    speed: f32,
    control: Arc<SpeedControl>,
    input_planar: Vec<Vec<f32>>,
    output_planar: Vec<Vec<f32>>,
    pending_out: VecDeque<i16>,
    /// inner 已到结尾且引擎残样已吐尽
    drained: bool,
}

impl<S> TempoSource<S>
where
    S: Source<Item = i16>,
{
    pub(super) fn new(inner: S, control: Arc<SpeedControl>, speed: f32) -> Self {
        let channels = inner.channels().max(1) as usize;
        let sample_rate = inner.sample_rate().max(1) as usize;
        let engine = SoundTouch::builder(channels, sample_rate)
            .tempo(speed.clamp(MIN_SPEED, MAX_SPEED) as f64)
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
            channels,
            speed: speed.clamp(MIN_SPEED, MAX_SPEED),
            control,
            input_planar: vec![Vec::with_capacity(FEED_FRAMES); channels],
            // receive_samples 按切片「长度」写入，缓冲必须保持固定长度，
            // 不能 clear 成空切片（否则永远收到 0 帧 → 无声）
            output_planar: vec![vec![0.0; FEED_FRAMES]; channels],
            pending_out: VecDeque::new(),
            drained: false,
        }
    }

    fn apply_speed(&mut self, speed: f32) {
        let speed = speed.clamp(MIN_SPEED, MAX_SPEED);
        if (self.speed - speed).abs() < f32::EPSILON {
            return;
        }
        self.speed = speed;
        self.drained = false;
        // 引擎内部（soundtouch WSOLA）有硬 assert，畸形缓冲状态下可能 panic；
        // release 虽已改 unwind，但这里仍就地兜住：切速失败只丢引擎缓存，
        // 让后续按当前 speed 直通产出，绝不因切速打挂整个音频线程。
        let guard = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.engine.set_tempo(speed as f64);
            // 丢弃引擎内缓存的旧速样本，避免切换后先播一段错误时长的内容；
            // pending_out 里已产出的样本保留继续播，切换处不做无谓的断音
            let _ = self.engine.clear();
        }));
        if guard.is_err() {
            eprintln!("[audio] soundtouch engine panicked during apply_speed({speed})");
        }
    }

    /// 从 inner 拉一段交错 i16，转成 planar f32 喂给引擎，并把引擎输出
    /// 攒到 pending_out。返回是否产出样本。
    ///
    /// 引擎的 put/receive/flush 在 vendored soundtouch 里有 assert! 与减法回绕，
    /// 偶现（变速 + seek/切速后缓冲稀疏的帧边界）会 panic；这里就地捕获，
    /// 转成普通 Err，由 `Iterator::next` 优雅停止当前源而非闪退。
    fn process_chunk(&mut self) -> Result<bool, String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.process_chunk_inner()))
            .unwrap_or_else(|payload| Err(engine_panic_message(payload)))
    }

    fn process_chunk_inner(&mut self) -> Result<bool, String> {
        // 原速直通：不经过引擎，零延迟零损耗
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
                return Ok(false);
            }
            return Ok(true);
        }

        let mut frames = 0usize;
        for channel in self.input_planar.iter_mut() {
            channel.clear();
        }
        // 按「帧」读取：每帧 channels 个样本。先读入临时缓冲，凑不满整帧
        // （inner 尾部样本数不是声道数整数倍）时丢弃残帧，保证各声道
        // planar 长度一致，否则 put_samples 会因长度不匹配报错中止播放
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
            for channel in 0..self.channels {
                self.input_planar[channel].push(frame[channel] as f32 / i16::MAX as f32);
            }
            frames += 1;
        }

        if frames == 0 {
            // inner 到头：flush 引擎残样后结束
            self.engine.flush().map_err(|e| e.to_string())?;
            let produced = self.receive_into_pending()?;
            self.drained = true;
            return Ok(produced);
        }

        self.engine
            .put_samples(&self.input_planar)
            .map_err(|e| e.to_string())?;
        let produced = self.receive_into_pending()?;
        if !produced && frames < FEED_FRAMES {
            // inner 已耗尽但引擎还没吐出任何样本（尾段缓冲中），flush 兜底
            self.engine.flush().map_err(|e| e.to_string())?;
            self.receive_into_pending()?;
            self.drained = true;
        }
        Ok(true)
    }

    fn receive_into_pending(&mut self) -> Result<bool, String> {
        let mut any = false;
        loop {
            // output_planar 保持固定长度（FEED_FRAMES），receive_samples
            // 按切片长度写入并返回实际帧数，只读 [0..frames]
            let frames = self
                .engine
                .receive_samples(&mut self.output_planar)
                .map_err(|e| e.to_string())?;
            if frames == 0 {
                return Ok(any);
            }
            any = true;
            for index in 0..frames {
                for channel in 0..self.channels {
                    let sample = self.output_planar[channel][index];
                    let clamped =
                        (sample * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32);
                    self.pending_out.push_back(clamped as i16);
                }
            }
        }
    }
}

impl<S> Iterator for TempoSource<S>
where
    S: Source<Item = i16>,
{
    type Item = i16;

    fn next(&mut self) -> Option<i16> {
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
            match self.process_chunk() {
                Ok(true) => {}
                Ok(false) => {}
                Err(err) => {
                    eprintln!("[audio] tempo processing failed, stopping source: {err}");
                    return None;
                }
            }
        }
    }
}

impl<S> Source for TempoSource<S>
where
    S: Source<Item = i16>,
{
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.pending_out.len())
    }

    fn channels(&self) -> u16 {
        self.channels as u16
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let result = self.inner.try_seek(pos);
        // 丢弃旧位置的缓冲样本，让 seek 后立即产出新位置音频
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = self.engine.clear();
        }));
        self.pending_out.clear();
        self.drained = false;
        result
    }
}

/// 把 catch_unwind 捕获的 panic payload 归一化成可读的错误字符串。
fn engine_panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    let detail = if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic".to_string()
    };
    format!("soundtouch engine panicked: {detail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固定长度的立体声测试源，样本值可预测
    struct FiniteSource {
        remaining: usize,
    }

    impl Iterator for FiniteSource {
        type Item = i16;
        fn next(&mut self) -> Option<i16> {
            if self.remaining == 0 {
                return None;
            }
            self.remaining -= 1;
            Some(((self.remaining % 5000) * 3) as i16)
        }
    }

    impl rodio::Source for FiniteSource {
        fn current_frame_len(&self) -> Option<usize> {
            None
        }
        fn channels(&self) -> u16 {
            2
        }
        fn sample_rate(&self) -> u32 {
            44100
        }
        fn total_duration(&self) -> Option<Duration> {
            None
        }
    }

    fn drain_all(source: &mut TempoSource<FiniteSource>) -> Vec<i16> {
        std::iter::from_fn(|| source.next()).collect()
    }

    #[test]
    fn passthrough_at_normal_speed() {
        let input_frames = 4096;
        let mut source = TempoSource::new(
            FiniteSource {
                remaining: input_frames * 2,
            },
            Arc::new(SpeedControl::default()),
            1.0,
        );
        let output = drain_all(&mut source);
        // 1.0x 直通：样本逐个原样透传
        assert_eq!(output.len(), input_frames * 2);
    }

    #[test]
    fn speed_change_takes_effect_live() {
        let input_frames = 44100; // 一秒立体声
        let control = Arc::new(SpeedControl::default());
        let mut source = TempoSource::new(
            FiniteSource {
                remaining: input_frames * 2,
            },
            Arc::clone(&control),
            1.0,
        );

        // 前半段原速直通
        let mut first_half = Vec::new();
        for _ in 0..(input_frames * 2 / 2) {
            if let Some(sample) = source.next() {
                first_half.push(sample);
            } else {
                break;
            }
        }
        assert_eq!(first_half.len(), input_frames);

        // 播放中切到 2.0x，不经重建管线
        control.request(2.0);
        let rest = drain_all(&mut source);
        let total = first_half.len() + rest.len();

        // 切速后必须继续产出样本（回归：receive 缓冲清零导致的无声）
        assert!(
            !rest.is_empty(),
            "no output after switching to 2.0x — speed change is broken"
        );
        // 2.0x 拉伸后总输出必须少于输入（含 flush 的静音填充余量）
        assert!(
            total < input_frames * 2,
            "expected compressed output at 2.0x, got {total} of {}",
            input_frames * 2
        );
    }

    #[test]
    fn odd_trailing_samples_do_not_abort_playback() {
        // 立体声流尾部多出 1 个样本（非声道数整数倍）：旧的按声道直写逻辑
        // 会造成 planar 长度不匹配（483 vs 482 类错误）并中止播放
        let input_frames = 500;
        let mut source = TempoSource::new(
            FiniteSource {
                remaining: input_frames * 2 + 1,
            },
            Arc::new(SpeedControl::default()),
            1.5,
        );
        let output = drain_all(&mut source);
        assert!(
            !output.is_empty(),
            "odd trailing sample should be dropped, not abort playback"
        );
    }
}
