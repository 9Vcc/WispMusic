use rodio::{decoder::DecoderError, source::SeekError, Source};
use std::{
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    time::Duration,
};
use symphonia::{
    core::{
        audio::{AudioBufferRef, SampleBuffer, SignalSpec},
        codecs::{Decoder as SymphoniaCodec, DecoderOptions},
        errors::Error,
        formats::{FormatOptions, FormatReader, SeekedTo},
        io::{MediaSource, MediaSourceStream},
        meta::MetadataOptions,
        probe::Hint,
        units,
    },
    default::get_codecs,
    default::get_probe,
};

// Decoder errors are not considered fatal.
// The correct action is to just get a new packet and try again.
// But a decode error in more than 3 consecutive packets is fatal.
const MAX_DECODE_ERRORS: usize = 3;

/// Symphonia 的 ISO MP4 读取器在探测阶段必须拿到流总长度（symphonia-format-isomp4
/// demuxer.rs 中 `mss.byte_len().ok_or(Error::SeekError(...))`），而 rodio 0.17 的
/// `ReadSeekSource::byte_len()` 恒返回 `None`，导致初始化期间抛出 SeekError 并触发
/// rodio 的 `unreachable!()` panic。此包装器携带真实长度修复该问题。
struct LenAwareSource<R> {
    inner: R,
    byte_len: u64,
}

impl<R> LenAwareSource<R> {
    fn new(inner: R, byte_len: u64) -> Self {
        Self { inner, byte_len }
    }
}

impl<R: Read + Seek + Send + Sync> MediaSource for LenAwareSource<R> {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.byte_len)
    }
}

impl<R: Read + Seek + Send + Sync> Read for LenAwareSource<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Read + Seek + Send + Sync> Seek for LenAwareSource<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

pub(super) struct Mp4Decoder {
    decoder: Box<dyn SymphoniaCodec>,
    current_frame_offset: usize,
    format: Box<dyn FormatReader>,
    buffer: SampleBuffer<i16>,
    spec: SignalSpec,
    total_duration: Option<Duration>,
}

impl Mp4Decoder {
    pub(super) fn new<R>(reader: R, byte_len: u64, extension: &str) -> Result<Self, DecoderError>
    where
        R: Read + Seek + Send + Sync + 'static,
    {
        let mss = MediaSourceStream::new(
            Box::new(LenAwareSource::new(reader, byte_len)) as Box<dyn MediaSource>,
            Default::default(),
        );
        Self::init(mss, extension)
    }

    fn init(mss: MediaSourceStream, extension: &str) -> Result<Self, DecoderError> {
        let mut hint = Hint::new();
        hint.with_extension(extension);
        let format_opts = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };
        let metadata_opts = MetadataOptions::default();
        let mut probed = get_probe()
            .format(&hint, mss, &format_opts, &metadata_opts)
            .map_err(decode_error)?;

        let track = probed
            .format
            .default_track()
            .ok_or(DecoderError::NoStreams)?;
        let total_duration = track_duration(track);
        let mut decoder = get_codecs()
            .make(
                &track.codec_params,
                &DecoderOptions {
                    verify: true,
                    ..Default::default()
                },
            )
            .map_err(decode_error)?;

        let mut decode_errors: usize = 0;
        let decoded = loop {
            let current_frame = probed.format.next_packet().map_err(decode_error)?;
            match decoder.decode(&current_frame) {
                Ok(decoded) => break decoded,
                Err(e) => match e {
                    Error::DecodeError(_) => {
                        decode_errors += 1;
                        if decode_errors > MAX_DECODE_ERRORS {
                            return Err(decode_error(e));
                        } else {
                            continue;
                        }
                    }
                    _ => return Err(decode_error(e)),
                },
            }
        };
        let spec = decoded.spec().to_owned();
        let buffer = Self::get_buffer(decoded, &spec);

        Ok(Mp4Decoder {
            decoder,
            current_frame_offset: 0,
            format: probed.format,
            buffer,
            spec,
            total_duration,
        })
    }

    #[inline]
    fn get_buffer(decoded: AudioBufferRef, spec: &SignalSpec) -> SampleBuffer<i16> {
        let duration = units::Duration::from(decoded.capacity() as u64);
        let mut buffer = SampleBuffer::<i16>::new(duration, spec.clone());
        buffer.copy_interleaved_ref(decoded);
        buffer
    }

    /// demuxer 级 seek：定位目标时间的包并解码，使下一次 `next()` 直接产出
    /// 目标位置的样本，替代「从头解码再丢弃」的 skip_duration 路径。
    fn try_seek_inner(&mut self, pos: Duration) -> Result<(), String> {
        use symphonia::core::formats::{SeekMode, SeekTo};

        let seek_beyond_end = self
            .total_duration
            .is_some_and(|dur| dur.saturating_sub(pos).as_millis() < 1);

        let time: units::Time = if seek_beyond_end {
            let time = self
                .total_duration
                .expect("is_some_and above guarantees Some")
                .as_secs_f64()
                .into();
            skip_back_a_tiny_bit(time)
        } else {
            pos.as_secs_f64().into()
        };

        // 对齐帧内通道偏移，seek 后从正确的通道继续
        let to_skip = self.current_frame_offset % self.channels() as usize;

        let seek_res = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: None,
                },
            )
            .map_err(|err| format!("mp4 format seek failed: {err}"))?;

        self.refine_position(seek_res)?;
        self.current_frame_offset += to_skip;
        Ok(())
    }

    /// 解码 seek 落点所在的包并跳过包内已过的样本（镜像 rodio SymphoniaDecoder
    /// 的 refine_position 语义）。
    fn refine_position(&mut self, seek_res: SeekedTo) -> Result<(), String> {
        let mut samples_to_pass = seek_res.required_ts.saturating_sub(seek_res.actual_ts);
        let decoded = loop {
            let packet = self
                .format
                .next_packet()
                .map_err(|err| format!("fetch packet after seek failed: {err}"))?;
            if packet.dur() <= samples_to_pass {
                samples_to_pass -= packet.dur();
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => break decoded,
                Err(Error::DecodeError(_)) => {
                    // 容错跳过坏包，继续取下一包
                    samples_to_pass = 0;
                    continue;
                }
                Err(err) => return Err(format!("decode packet after seek failed: {err}")),
            }
        };

        self.spec = decoded.spec().to_owned();
        self.buffer = Self::get_buffer(decoded, &self.spec);
        self.current_frame_offset = samples_to_pass as usize * self.channels() as usize;
        Ok(())
    }
}

fn skip_back_a_tiny_bit(
    units::Time {
        mut seconds,
        mut frac,
    }: units::Time,
) -> units::Time {
    frac -= 0.0001;
    if frac < 0.0 {
        if seconds == 0 {
            // 时长不足 0.0001s 的极端文件：直接钳回 0，避免借位后跳到 0.9999s
            return units::Time { seconds: 0, frac: 0.0 };
        }
        // 借位：frac 为负时从秒位借 1，frac 取其补数（如 -0.0001 → 0.9999）。
        // 注意是 1.0 + frac（负数取反），写成 1.0 - frac 会得到 >1.0 的非法值，
        // 触发 symphonia calc_timestamp 的 "Invalid range for Time fractional part" panic
        seconds -= 1;
        frac = 1.0 + frac;
    }
    units::Time { seconds, frac }
}

fn decode_error(error: Error) -> DecoderError {
    match error {
        Error::IoError(e) => DecoderError::IoError(e.to_string()),
        Error::DecodeError(e) => DecoderError::DecodeError(e),
        // rodio 对初始化期 SeekError 调用 unreachable!()，这里降级为普通错误返回
        Error::SeekError(e) => DecoderError::IoError(format!("seek error: {e:?}")),
        Error::Unsupported(_) => DecoderError::UnrecognizedFormat,
        Error::LimitError(e) => DecoderError::LimitError(e),
        Error::ResetRequired => DecoderError::ResetRequired,
    }
}

#[derive(Debug)]
struct SeekFailure(String);

impl std::fmt::Display for SeekFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SeekFailure {}

fn track_duration(track: &symphonia::core::formats::Track) -> Option<Duration> {
    let n_frames = track.codec_params.n_frames?;
    let time_base = track.codec_params.time_base?;
    Some(Duration::from(time_base.calc_time(n_frames)))
}

/// 通过 `ftyp` 盒魔数判断 ISO BMFF（m4a/mp4/mov 等）。`BufReader::fill_buf`
/// 只窥探不消费，判断后读取位置仍在文件开头。
pub(super) fn is_iso_bmff<R: Read>(reader: &mut BufReader<R>) -> io::Result<bool> {
    let buf = reader.fill_buf()?;
    Ok(buf.len() >= 12 && &buf[4..8] == b"ftyp")
}

impl Source for Mp4Decoder {
    #[inline]
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.buffer.samples().len())
    }

    #[inline]
    fn channels(&self) -> u16 {
        self.spec.channels.count() as u16
    }

    #[inline]
    fn sample_rate(&self) -> u32 {
        self.spec.rate
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.try_seek_inner(pos)
            .map_err(|err| SeekError::Other(Box::new(SeekFailure(err))))
    }
}

impl Iterator for Mp4Decoder {
    type Item = i16;

    #[inline]
    fn next(&mut self) -> Option<i16> {
        if self.current_frame_offset == self.buffer.len() {
            let mut decode_errors: usize = 0;
            let decoded = loop {
                match self.format.next_packet() {
                    Ok(packet) => match self.decoder.decode(&packet) {
                        Ok(decoded) => break decoded,
                        Err(e) => match e {
                            Error::DecodeError(_) => {
                                decode_errors += 1;
                                if decode_errors > MAX_DECODE_ERRORS {
                                    return None;
                                } else {
                                    continue;
                                }
                            }
                            _ => return None,
                        },
                    },
                    Err(_) => return None,
                }
            };
            self.spec = decoded.spec().to_owned();
            self.buffer = Self::get_buffer(decoded, &self.spec);
            self.current_frame_offset = 0;
        }

        let sample = *self.buffer.samples().get(self.current_frame_offset)?;
        self.current_frame_offset += 1;

        Some(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn is_iso_bmff_detects_ftyp_magic_without_consuming() {
        let mp4 = b"\x00\x00\x00\x20ftypM4A \x00\x00\x00\x00";
        let mut reader = BufReader::new(Cursor::new(mp4));
        assert!(is_iso_bmff(&mut reader).expect("sniff should succeed"));
        // 窥探后位置仍在开头
        let mut head = [0u8; 4];
        reader.read_exact(&mut head).expect("read should succeed");
        assert_eq!(&head, b"\x00\x00\x00\x20");

        let mut reader = BufReader::new(Cursor::new(b"fLaC0000"));
        assert!(!is_iso_bmff(&mut reader).expect("sniff should succeed"));

        let mut reader = BufReader::new(Cursor::new(b"ftyp"));
        assert!(!is_iso_bmff(&mut reader).expect("sniff should succeed"));
    }

    #[test]
    fn mp4_decoder_returns_error_instead_of_panicking_on_malformed_data() {
        // 带 ftyp 魔数但结构损坏的数据：旧实现（rodio ReadSeekSource 的
        // byte_len=None）在初始化时触发 unreachable!() panic；新实现应返回 Err
        let mut data = Vec::new();
        data.extend_from_slice(&32u32.to_be_bytes());
        data.extend_from_slice(b"ftypM4A ");
        data.extend_from_slice(&[0u8; 24]);
        let byte_len = data.len() as u64;

        let result = Mp4Decoder::new(Cursor::new(data), byte_len, "m4a");
        assert!(result.is_err(), "malformed mp4 should error, not panic");
    }

    #[test]
    fn skip_back_borrows_correctly_when_frac_underflows() {
        // 整秒边界借位：180.0s 回退 0.0001 应得 179.9999s，frac 必须落在 [0, 1)
        let t = skip_back_a_tiny_bit(units::Time::new(180, 0.0));
        assert_eq!(t.seconds, 179);
        assert!(t.frac >= 0.0 && t.frac < 1.0, "frac must be in [0, 1), got {}", t.frac);

        // 正常小数回退
        let t = skip_back_a_tiny_bit(units::Time::new(10, 0.5));
        assert_eq!(t.seconds, 10);
        assert!((t.frac - 0.4999).abs() < 1e-9);

        // 极端：不足 0.0001s 的时长，钳回 0
        let t = skip_back_a_tiny_bit(units::Time::new(0, 0.00005));
        assert_eq!(t.seconds, 0);
        assert_eq!(t.frac, 0.0);
    }
}
