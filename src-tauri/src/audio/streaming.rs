use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};
use rodio::{decoder::DecoderError, Decoder, Source};
use std::{
    fs::File,
    io::{self, ErrorKind, Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::watch;

use super::mp4_decoder::Mp4Decoder;

pub(super) type BoxedAudioSource = Box<dyn Source<Item = i16> + Send>;

/// SeekFrom::End 等待总长度（下载完成/拿到 Content-Length）的硬上限。
/// 正常流式播放早已拿到 Content-Length 而立即返回；只有在缺长度的畸形流
/// 叠上源站僵死时才会耗尽这段时间——宁可报错停当前源，也不能把音频回调线程
/// 永久占住（表现为整应用卡死）。
const STREAMING_END_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// data 预分配上限。Content-Length 已知时按它一次性预留，避免 Vec 逐次倍增
/// 扩容在最后一次重分配时同时持有「旧 + 新」两块（≈2× 当前长度）的瞬时峰值。
/// 取 24MB 是个折中：常见 mp3/m4a 单曲都在此之内 → 全程零重分配；超大无损
/// 只预留这么多，避免用户秒切歌时白占一整份文件的容量。
const DATA_RESERVE_MAX_BYTES: u64 = 24 * 1024 * 1024;

/// Range 快进 tail 缓冲上限。tail 里的字节主顺序下载稍后也会写到 data，
/// 不设限时两者可同时持有近整份文件（峰值≈2× 文件长度）。超限即中止本次
/// Range 流（tail_finished），reader 读到 tail 末尾后会对新位置重新发起
/// Range，而 reset_tail 会先回收旧缓冲，因此不会一直占着这 32MB。
const TAIL_MAX_BYTES: usize = 32 * 1024 * 1024;

struct StreamingState {
    data: Vec<u8>,
    complete: bool,
    error: Option<String>,
    /// 响应头给出的总长度（Content-Length）。ISO MP4 解码需要它。
    total_len: Option<u64>,
    /// Range 快进流覆盖的字节区间 [tail_start, tail_start + tail_data.len())。
    /// 主顺序下载追平后合并清空，保证 data 始终是权威的连续前缀。
    tail_start: u64,
    tail_data: Vec<u8>,
    tail_done: bool,
    /// 下载完成并落盘后打开的只读句柄。一旦就位，读取改从磁盘进行、
    /// data 立即释放（长无损单曲可省下数十 MB 常驻）。
    /// 句柄保持打开还有一层作用：即使文件随后被缓存淘汰逻辑删除，
    /// 已打开的句柄仍可继续读到全部数据，当前播放不会中断。
    file: Option<File>,
}

pub(super) struct StreamingBuffer {
    state: Mutex<StreamingState>,
    available: Condvar,
    /// reader 读到未下载边界之外时发出的快进目标字节位置；None 表示无需快进。
    fast_forward_tx: watch::Sender<Option<u64>>,
    /// 顺序下载最终落盘的路径。None 表示该缓冲没有对应磁盘文件，永不回退读盘。
    final_path: Option<PathBuf>,
}

impl StreamingBuffer {
    pub(super) fn new(total_len: Option<u64>, final_path: Option<PathBuf>) -> Arc<Self> {
        let (fast_forward_tx, _) = watch::channel(None);
        // 已知总长度就一次性预留，避免逐次倍增扩容带来的瞬时双份占用
        let reserve_bytes = total_len
            .filter(|len| *len > 0)
            .map(|len| len.min(DATA_RESERVE_MAX_BYTES) as usize)
            .unwrap_or(0);
        Arc::new(Self {
            state: Mutex::new(StreamingState {
                data: Vec::with_capacity(reserve_bytes),
                complete: false,
                error: None,
                total_len,
                tail_start: 0,
                tail_data: Vec::new(),
                tail_done: true,
                file: None,
            }),
            available: Condvar::new(),
            fast_forward_tx,
            final_path,
        })
    }

    pub(super) fn append(&self, chunk: &[u8]) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "streaming audio buffer lock poisoned".to_string())?;
        // 已切换到磁盘读取：内存副本不再维护（此时顺序下载也已结束）
        if state.file.is_some() {
            return Ok(());
        }
        state.data.extend_from_slice(chunk);
        // 主下载追平 tail 区域后合并，避免重复占内存
        if !state.tail_data.is_empty() {
            let tail_end = state.tail_start + state.tail_data.len() as u64;
            if state.data.len() as u64 >= tail_end {
                // 同上：换掉整个 Vec 归还容量，而不是 clear() 留着
                state.tail_data = Vec::new();
                state.tail_done = true;
                // tail 已无用，取消后台 Range 流
                self.fast_forward_tx
                    .send_if_modified(|value| value.take().is_some());
            }
        }
        self.available.notify_all();
        Ok(())
    }

    pub(super) fn finish(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.complete = true;
            // 顺序下载已把同一份字节按序写进落盘文件（写盘与 append 用的是同
            // 一批 chunk），因此文件内容与 data 完全一致：打开句柄接管读取，
            // 释放整份内存副本。打开失败或长度对不上时保留 data 继续用内存，
            // 行为退回改造前。
            if let Some(path) = self.final_path.as_ref() {
                if let Ok(file) = File::open(path) {
                    let file_len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
                    if file_len >= state.data.len() as u64 {
                        state.total_len = Some(file_len);
                        state.data = Vec::new();
                        state.tail_data = Vec::new();
                        state.tail_done = true;
                        state.file = Some(file);
                    }
                }
            }
            self.available.notify_all();
        }
    }

    pub(super) fn fail(&self, message: String) {
        if let Ok(mut state) = self.state.lock() {
            state.error = Some(message);
            state.complete = true;
            // data 内容保留（reader 可能还在播已缓冲的部分），但把按
            // Content-Length 预留的容量归还——下载既然失败/被取消，未下载到的
            // 那部分预留（最多 24MB）就是纯占位。shrink_to_fit 不动 len，
            // 只换一块实际大小的分配，reader 读到的字节不变。
            state.data.shrink_to_fit();
            self.available.notify_all();
        }
    }

    pub(super) fn fast_forward_receiver(&self) -> watch::Receiver<Option<u64>> {
        self.fast_forward_tx.subscribe()
    }

    fn request_fast_forward(&self, target: u64) {
        self.fast_forward_tx.send_if_modified(|value| {
            if *value == Some(target) {
                false
            } else {
                *value = Some(target);
                true
            }
        });
    }

    /// reader 不再需要快进（已从主缓冲命中）时取消 Range 流。
    fn cancel_fast_forward(&self) {
        self.fast_forward_tx
            .send_if_modified(|value| value.take().is_some());
    }

    pub(super) fn reset_tail(&self, start: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.tail_start = start;
            // 用 take 换掉整个 Vec 而不是 clear()：clear 会保留上一次快进的
            // 容量（tail 上限 32MB），而这轮新目标通常远用不到那么多；
            // 换成空 Vec 把上次的容量真正还给分配器
            state.tail_data = Vec::new();
            state.tail_done = false;
            self.available.notify_all();
        }
    }

    pub(super) fn append_tail(&self, chunk: &[u8]) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "streaming audio buffer lock poisoned".to_string())?;
        // 已切换到磁盘读取：reader 不再需要快进，丢弃即可
        if state.file.is_some() {
            return Ok(());
        }
        // 超限即中止本次 Range 流：调用方会置 tail_done，reader 读到已缓存的
        // tail 末尾后重新发起 Range（reset_tail 先回收旧缓冲），内存有界
        if state.tail_data.len() >= TAIL_MAX_BYTES {
            return Err(format!(
                "streaming tail buffer reached {} bytes limit",
                TAIL_MAX_BYTES
            ));
        }
        state.tail_data.extend_from_slice(chunk);
        self.available.notify_all();
        Ok(())
    }

    pub(super) fn tail_finished(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.tail_done = true;
            self.available.notify_all();
        }
    }

    /// 当前内存副本占用（测试断言「落盘后已释放」用）
    #[cfg(test)]
    pub(super) fn debug_memory_len(&self) -> usize {
        self.state.lock().map(|state| state.data.len()).unwrap_or(0)
    }

    /// 是否已切换到磁盘读取（测试断言用）
    #[cfg(test)]
    pub(super) fn debug_is_file_backed(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.file.is_some())
            .unwrap_or(false)
    }

    /// 内存容量快照 (data 容量, tail 容量)，测试断言「容量已归还」用
    #[cfg(test)]
    pub(super) fn debug_capacities(&self) -> (usize, usize) {
        self.state
            .lock()
            .map(|state| (state.data.capacity(), state.tail_data.capacity()))
            .unwrap_or((0, 0))
    }
}

#[derive(Clone)]
pub(super) struct StreamingReader {
    shared: Arc<StreamingBuffer>,
    latest_request_id: Arc<AtomicU64>,
    request_id: u64,
    position: u64,
}

impl StreamingReader {
    pub(super) fn new(
        shared: Arc<StreamingBuffer>,
        latest_request_id: Arc<AtomicU64>,
        request_id: u64,
    ) -> Self {
        Self {
            shared,
            latest_request_id,
            request_id,
            position: 0,
        }
    }

    fn is_cancelled(&self) -> bool {
        self.latest_request_id.load(Ordering::SeqCst) != self.request_id
    }

    /// 返回流总长度：下载完成时用实际长度（落盘后即文件长度），否则用 Content-Length。
    pub(super) fn byte_len(&self) -> Option<u64> {
        let state = self.shared.state.lock().ok()?;
        if state.complete {
            Some(state.total_len.unwrap_or(state.data.len() as u64))
        } else {
            state.total_len
        }
    }

    fn cancelled_error() -> io::Error {
        io::Error::new(ErrorKind::Interrupted, "streaming audio load cancelled")
    }
}

impl Read for StreamingReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }

        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| io::Error::new(ErrorKind::Other, "streaming audio buffer poisoned"))?;

        // 落盘后从磁盘读：每次都按绝对位置 seek，因此多个 decoder 共用
        // 同一句柄（如交叉淡化时新旧两套管线）不会互相串位
        if let Some(file) = state.file.as_mut() {
            if self.is_cancelled() {
                return Err(Self::cancelled_error());
            }
            file.seek(SeekFrom::Start(self.position))?;
            let read_len = file.read(output)?;
            self.position += read_len as u64;
            return Ok(read_len);
        }

        loop {
            if self.is_cancelled() {
                return Err(Self::cancelled_error());
            }

            let data_len = state.data.len() as u64;
            let tail_start = state.tail_start;
            let tail_len = state.tail_data.len() as u64;

            // 主缓冲命中（权威数据，优先）
            if self.position < data_len {
                // reader 已回到主缓冲范围，未完成的快进请求已过期，取消 Range 流
                self.shared.cancel_fast_forward();
                let start = self.position as usize;
                let end = (start + output.len()).min(state.data.len());
                let read_len = end - start;
                output[..read_len].copy_from_slice(&state.data[start..end]);
                self.position += read_len as u64;
                return Ok(read_len);
            }

            // Range 快进流命中
            if tail_len > 0 && self.position >= tail_start && self.position < tail_start + tail_len
            {
                let start = (self.position - tail_start) as usize;
                let end = (start + output.len()).min(state.tail_data.len());
                let read_len = end - start;
                output[..read_len].copy_from_slice(&state.tail_data[start..end]);
                self.position += read_len as u64;
                return Ok(read_len);
            }

            if let Some(error) = state.error.clone() {
                return Err(io::Error::new(ErrorKind::Other, error));
            }

            // 主下载完结、tail 也无力覆盖目标位置 → EOF
            if state.complete && (state.tail_done || tail_len == 0) {
                return Ok(0);
            }

            // 目标越过已下载边界：请求 Range 快进，等待主缓冲或 tail 覆盖
            if self.position > data_len {
                self.shared.request_fast_forward(self.position);
            }

            let (next_state, _) = self
                .shared
                .available
                .wait_timeout(state, Duration::from_millis(250))
                .map_err(|_| io::Error::new(ErrorKind::Other, "streaming audio buffer poisoned"))?;
            state = next_state;
        }
    }
}

impl Seek for StreamingReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let target = match position {
            SeekFrom::Start(position) => position as i128,
            SeekFrom::Current(offset) => self.position as i128 + offset as i128,
            SeekFrom::End(offset) => {
                let mut state = self.shared.state.lock().map_err(|_| {
                    io::Error::new(ErrorKind::Other, "streaming audio buffer poisoned")
                })?;
                // 等待总长度确定（下载完成或拿到 Content-Length）期间，若源站
                // 僵死、complete 永不到来，这个循环会把调用方（音频回调线程）
                // 永久占住 → 表现为卡死。给一个硬上限，超时后按错误退出；
                // cancelled / error 两条早退路径本就能提前结束正常等待。
                let deadline = Instant::now() + STREAMING_END_WAIT_TIMEOUT;
                while !state.complete {
                    if self.is_cancelled() {
                        return Err(Self::cancelled_error());
                    }
                    if let Some(error) = state.error.clone() {
                        return Err(io::Error::new(ErrorKind::Other, error));
                    }
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            ErrorKind::TimedOut,
                            "streaming audio length wait timed out",
                        ));
                    }
                    let (next_state, wait) = self
                        .shared
                        .available
                        .wait_timeout(state, Duration::from_millis(250))
                        .map_err(|_| {
                            io::Error::new(ErrorKind::Other, "streaming audio buffer poisoned")
                        })?;
                    state = next_state;
                    if wait.timed_out() && Instant::now() >= deadline {
                        return Err(io::Error::new(
                            ErrorKind::TimedOut,
                            "streaming audio length wait timed out",
                        ));
                    }
                }
                // 落地后 total_len 已被 finish 校正为文件实际长度，
                // 未落地时它就是 Content-Length
                let end = state.total_len.unwrap_or(state.data.len() as u64);
                end as i128 + offset as i128
            }
        };

        if target < 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "cannot seek before start of audio stream",
            ));
        }

        self.position = target as u64;
        Ok(self.position)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StreamFormat {
    Mp3,
    Flac,
    OggVorbis,
    Wav,
    Aac,
    Mp4,
    M4a,
}

impl StreamFormat {
    pub(super) fn label(self) -> &'static str {
        match self {
            StreamFormat::Mp3 => "mp3",
            StreamFormat::Flac => "flac",
            StreamFormat::OggVorbis => "ogg/vorbis",
            StreamFormat::Wav => "wav",
            StreamFormat::Aac => "aac",
            StreamFormat::Mp4 => "mp4",
            StreamFormat::M4a => "m4a",
        }
    }
}

/// rodio 的 SymphoniaDecoder::new 对初始化期间的 SeekError 直接 unreachable!()
/// panic（如 URL 扩展名与实际内容不符、探测回落到 isomp4 读取器时）。
/// 捕获解码器构造期间的 panic 并转为普通解码错误，让调用方按播放失败处理，
/// 避免整个进程 abort。
pub(super) fn decode_with_panic_guard<F>(
    build: F,
) -> Result<BoxedAudioSource, DecoderError>
where
    F: FnOnce() -> Result<BoxedAudioSource, DecoderError>,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(build)) {
        Ok(result) => result,
        Err(payload) => {
            let message = if let Some(text) = payload.downcast_ref::<&str>() {
                (*text).to_string()
            } else if let Some(text) = payload.downcast_ref::<String>() {
                text.clone()
            } else {
                "rodio decoder panicked during initialization".to_string()
            };
            Err(DecoderError::IoError(message))
        }
    }
}

pub(super) fn decode_streaming_reader(
    reader: StreamingReader,
    format: StreamFormat,
) -> Result<BoxedAudioSource, DecoderError> {
    match format {
        // rodio 0.17~0.20 的 ReadSeekSource 不提供 byte_len，symphonia 的
        // ISO MP4 读取器会因此在初始化时抛 SeekError 并触发 rodio 的
        // unreachable!() panic，这里改走自带 byte_len 的 Mp4Decoder。
        StreamFormat::Mp4 | StreamFormat::M4a => {
            let byte_len = reader.byte_len().ok_or_else(|| {
                DecoderError::IoError(
                    "mp4/m4a stream length unknown (missing Content-Length)".to_string(),
                )
            })?;
            Ok(Box::new(Mp4Decoder::new(reader, byte_len, format.label())?))
        }
        // 其余格式仍走 rodio，但探测时提示格式失败后可能回落到 isomp4
        // 读取器触发上述 panic，统一用 panic guard 兜底。
        StreamFormat::Mp3 => {
            decode_with_panic_guard(|| Ok(Box::new(Decoder::new_mp3(reader)?) as BoxedAudioSource))
        }
        StreamFormat::Flac => decode_with_panic_guard(|| {
            Ok(Box::new(Decoder::new_flac(reader)?) as BoxedAudioSource)
        }),
        StreamFormat::OggVorbis => decode_with_panic_guard(|| {
            Ok(Box::new(Decoder::new_vorbis(reader)?) as BoxedAudioSource)
        }),
        StreamFormat::Wav => decode_with_panic_guard(|| {
            Ok(Box::new(Decoder::new_wav(reader)?) as BoxedAudioSource)
        }),
        StreamFormat::Aac => decode_with_panic_guard(|| {
            Ok(Box::new(Decoder::new_aac(reader)?) as BoxedAudioSource)
        }),
    }
}

pub(super) fn content_type(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

pub(super) fn content_length(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

pub(super) fn detect_stream_format(
    url: &str,
    content_type: Option<&str>,
    first_chunk: &[u8],
) -> Option<StreamFormat> {
    // 内容魔数优先于 URL 扩展名：部分音源 URL 扩展名与实际内容不符
    // （如 .mp3 链接实际返回 m4a）。ISO BMFF 内容必须路由到 Mp4Decoder，
    // 一旦交给 rodio 的通用探测（ReadSeekSource 无 byte_len），isomp4
    // 读取器会在初始化时抛 SeekError 并触发 rodio 的 unreachable!() panic。
    if looks_like_iso_bmff(first_chunk) {
        return Some(StreamFormat::M4a);
    }

    match extract_remote_ext(url) {
        "mp3" => return Some(StreamFormat::Mp3),
        "flac" => return Some(StreamFormat::Flac),
        "wav" => return Some(StreamFormat::Wav),
        "aac" => return Some(StreamFormat::Aac),
        "mp4" => return Some(StreamFormat::Mp4),
        "m4a" => return Some(StreamFormat::M4a),
        _ => {}
    }

    if let Some(format) = content_type.and_then(stream_format_from_content_type) {
        return Some(format);
    }

    if looks_like_aac_adts(first_chunk) {
        return Some(StreamFormat::Aac);
    }
    if first_chunk.starts_with(b"fLaC") {
        return Some(StreamFormat::Flac);
    }
    if looks_like_ogg_vorbis(first_chunk) {
        return Some(StreamFormat::OggVorbis);
    }
    if first_chunk.len() >= 12 && first_chunk.starts_with(b"RIFF") && &first_chunk[8..12] == b"WAVE"
    {
        return Some(StreamFormat::Wav);
    }
    if looks_like_iso_bmff(first_chunk) {
        return Some(StreamFormat::M4a);
    }
    if looks_like_mp3(first_chunk) {
        return Some(StreamFormat::Mp3);
    }

    None
}

pub(super) fn remote_cache_file_name(url: &str) -> String {
    use std::{
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
    };

    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{hash:016x}.{}", extract_remote_ext(url))
}

fn extract_remote_ext(url: &str) -> &str {
    let path = url.split('?').next().unwrap_or(url);
    let ext = path.rsplit('.').next().unwrap_or("audio");
    match ext.to_ascii_lowercase().as_str() {
        "mp3" => "mp3",
        "flac" => "flac",
        "m4a" | "m4b" | "m4p" | "m4r" => "m4a",
        "mp4" => "mp4",
        "aac" => "aac",
        "wav" => "wav",
        "ogg" => "ogg",
        _ => "audio",
    }
}

fn stream_format_from_content_type(content_type: &str) -> Option<StreamFormat> {
    let content_type = content_type.to_ascii_lowercase();
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or(content_type.as_str())
        .trim();

    match mime {
        "audio/mpeg" | "audio/mp3" | "audio/x-mpeg" => Some(StreamFormat::Mp3),
        "audio/flac" | "audio/x-flac" => Some(StreamFormat::Flac),
        "audio/vorbis" => Some(StreamFormat::OggVorbis),
        "audio/wav" | "audio/wave" | "audio/x-wav" | "audio/vnd.wave" => Some(StreamFormat::Wav),
        "audio/aac" | "audio/aacp" | "audio/x-aac" => Some(StreamFormat::Aac),
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => Some(StreamFormat::M4a),
        "video/mp4" | "application/mp4" => Some(StreamFormat::Mp4),
        _ => None,
    }
}

fn looks_like_mp3(bytes: &[u8]) -> bool {
    bytes.starts_with(b"ID3")
        || bytes.windows(2).take(512).any(|window| {
            window[0] == 0xff
                && (window[1] & 0xe0) == 0xe0
                && (window[1] & 0x18) != 0x08
                && (window[1] & 0x06) != 0x00
        })
}

fn looks_like_aac_adts(bytes: &[u8]) -> bool {
    bytes
        .windows(2)
        .take(512)
        .any(|window| window[0] == 0xff && (window[1] & 0xf6) == 0xf0)
}

fn looks_like_ogg_vorbis(bytes: &[u8]) -> bool {
    bytes.starts_with(b"OggS") && bytes.windows(6).take(512).any(|window| window == b"vorbis")
}

fn looks_like_iso_bmff(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp"
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use std::io::Write;

    fn temp_stream_file(name: &str, bytes: &[u8]) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "wisp-streaming-test-{}-{name}.bin",
            std::process::id()
        ));
        let mut file = File::create(&path).expect("create temp stream file");
        file.write_all(bytes).expect("write temp stream file");
        file.flush().expect("flush temp stream file");
        path
    }

    #[test]
    fn streaming_reader_reads_and_seeks_buffered_data() {
        let shared = StreamingBuffer::new(None, None);
        shared.append(b"abcdef").expect("append should succeed");
        shared.finish();
        let latest_request_id = Arc::new(AtomicU64::new(1));

        let mut reader = StreamingReader::new(shared, latest_request_id, 1);
        let mut output = [0u8; 3];
        assert_eq!(reader.read(&mut output).expect("read should succeed"), 3);
        assert_eq!(&output, b"abc");

        assert_eq!(
            reader
                .seek(SeekFrom::Start(2))
                .expect("seek should succeed"),
            2
        );

        let mut output = [0u8; 4];
        assert_eq!(reader.read(&mut output).expect("read should succeed"), 4);
        assert_eq!(&output, b"cdef");
        assert_eq!(reader.read(&mut output).expect("eof should succeed"), 0);
    }

    #[test]
    fn streaming_reader_byte_len_prefers_actual_length_when_complete() {
        let shared = StreamingBuffer::new(Some(100), None);
        let latest_request_id = Arc::new(AtomicU64::new(1));
        let reader = StreamingReader::new(shared, latest_request_id, 1);
        assert_eq!(reader.byte_len(), Some(100));

        let shared = StreamingBuffer::new(None, None);
        shared.append(b"abcdef").expect("append should succeed");
        shared.finish();
        let latest_request_id = Arc::new(AtomicU64::new(1));
        let reader = StreamingReader::new(shared, latest_request_id, 1);
        assert_eq!(reader.byte_len(), Some(6));
    }

    #[test]
    fn streaming_reader_stops_waiting_when_request_is_stale() {
        let shared = StreamingBuffer::new(None, None);
        let latest_request_id = Arc::new(AtomicU64::new(2));
        let mut reader = StreamingReader::new(shared, latest_request_id, 1);
        let mut output = [0u8; 4];

        let error = reader
            .read(&mut output)
            .expect_err("stale request should cancel read");
        assert_eq!(error.kind(), ErrorKind::Interrupted);
    }

    #[test]
    fn streaming_reader_switches_to_disk_and_releases_memory_after_finish() {
        let payload: Vec<u8> = (0..64u8).cycle().take(4096).collect();
        let path = temp_stream_file("disk-readback", &payload);
        let shared = StreamingBuffer::new(Some(payload.len() as u64), Some(path.clone()));
        shared.append(&payload).expect("append should succeed");

        let latest_request_id = Arc::new(AtomicU64::new(7));
        let mut reader = StreamingReader::new(Arc::clone(&shared), latest_request_id, 7);

        // 落盘前：走内存副本
        let mut output = [0u8; 8];
        assert_eq!(reader.read(&mut output).expect("read should succeed"), 8);
        assert_eq!(&output, &payload[..8]);
        assert!(!shared.debug_is_file_backed());
        assert_eq!(shared.debug_memory_len(), payload.len());

        shared.finish();

        // 落盘后：内存副本释放，读取改走文件
        assert!(shared.debug_is_file_backed());
        assert_eq!(shared.debug_memory_len(), 0);
        assert_eq!(reader.byte_len(), Some(payload.len() as u64));

        assert_eq!(reader.read(&mut output).expect("read should succeed"), 8);
        assert_eq!(&output, &payload[8..16]);

        // 回跳重读：位置由 reader 自己维护，与句柄当前偏移无关
        assert_eq!(reader.seek(SeekFrom::Start(0)).expect("seek"), 0);
        assert_eq!(reader.read(&mut output).expect("read should succeed"), 8);
        assert_eq!(&output, &payload[..8]);

        // SeekFrom::End 用的是落盘校正后的实际长度
        let tail_start = reader
            .seek(SeekFrom::End(-4))
            .expect("seek from end should succeed");
        assert_eq!(tail_start, payload.len() as u64 - 4);
        let mut tail = [0u8; 8];
        assert_eq!(reader.read(&mut tail).expect("read should succeed"), 4);
        assert_eq!(&tail[..4], &payload[payload.len() - 4..]);
        assert_eq!(reader.read(&mut tail).expect("eof should succeed"), 0);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_backed_reads_are_position_independent_across_readers() {
        let payload: Vec<u8> = (0..32u8).cycle().take(1024).collect();
        let path = temp_stream_file("shared-handle", &payload);
        let shared = StreamingBuffer::new(Some(payload.len() as u64), Some(path.clone()));
        shared.append(&payload).expect("append should succeed");
        shared.finish();
        assert!(shared.debug_is_file_backed());

        let latest_request_id = Arc::new(AtomicU64::new(1));
        let mut first = StreamingReader::new(Arc::clone(&shared), Arc::clone(&latest_request_id), 1);
        let mut second = StreamingReader::new(Arc::clone(&shared), Arc::clone(&latest_request_id), 1);

        first.seek(SeekFrom::Start(8)).expect("seek first");
        second.seek(SeekFrom::Start(64)).expect("seek second");

        let mut a = [0u8; 4];
        let mut b = [0u8; 4];
        assert_eq!(first.read(&mut a).expect("read first"), 4);
        assert_eq!(second.read(&mut b).expect("read second"), 4);
        assert_eq!(&a, &payload[8..12]);
        assert_eq!(&b, &payload[64..68]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn streaming_buffer_keeps_memory_when_disk_file_is_missing() {
        let payload: Vec<u8> = (0..32u8).cycle().take(512).collect();
        let mut missing = std::env::temp_dir();
        missing.push(format!(
            "wisp-streaming-test-{}-missing.bin",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&missing);

        let shared = StreamingBuffer::new(Some(payload.len() as u64), Some(missing));
        shared.append(&payload).expect("append should succeed");
        shared.finish();

        // 文件打不开时保留内存副本，行为退回改造前
        assert!(!shared.debug_is_file_backed());
        assert_eq!(shared.debug_memory_len(), payload.len());

        let latest_request_id = Arc::new(AtomicU64::new(1));
        let mut reader = StreamingReader::new(shared, latest_request_id, 1);
        let mut output = [0u8; 512];
        assert_eq!(reader.read(&mut output).expect("read should succeed"), 512);
        assert_eq!(&output[..], &payload[..]);
    }

    #[test]
    fn streaming_buffer_keeps_memory_when_disk_file_is_shorter() {
        let payload: Vec<u8> = (0..64u8).cycle().take(1024).collect();
        let path = temp_stream_file("short-file", &payload[..256]);

        let shared = StreamingBuffer::new(Some(payload.len() as u64), Some(path.clone()));
        shared.append(&payload).expect("append should succeed");
        shared.finish();

        // 磁盘文件比内存副本短说明落盘不可信，继续用内存，避免读到截断内容
        assert!(!shared.debug_is_file_backed());
        assert_eq!(shared.debug_memory_len(), payload.len());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stream_format_detection_uses_url_content_type_and_bytes() {
        assert_eq!(
            detect_stream_format(
                "https://example.test/song.mp3?token=abc",
                None,
                b"not enough data"
            ),
            Some(StreamFormat::Mp3)
        );
        assert_eq!(
            detect_stream_format(
                "https://example.test/play?id=1",
                Some("audio/mpeg"),
                b"not enough data"
            ),
            Some(StreamFormat::Mp3)
        );
        assert_eq!(
            detect_stream_format("https://example.test/play?id=1", None, b"ID3\x04\x00\x00"),
            Some(StreamFormat::Mp3)
        );
        assert_eq!(
            detect_stream_format(
                "https://example.test/play?id=1",
                None,
                &[0x00, 0xff, 0xfb, 0x90, 0x64]
            ),
            Some(StreamFormat::Mp3)
        );
        assert_eq!(
            detect_stream_format(
                "https://example.test/play?id=4",
                Some("audio/aac"),
                &[0xff, 0xf1, 0x50, 0x80]
            ),
            Some(StreamFormat::Aac)
        );
        assert_eq!(
            detect_stream_format(
                "https://example.test/song.m4a?token=abc",
                None,
                b"\x00\x00\x00\x20ftypM4A "
            ),
            Some(StreamFormat::M4a)
        );
        assert_eq!(
            detect_stream_format(
                "https://example.test/play?id=5",
                Some("audio/mp4; codecs=mp4a.40.2"),
                b"\x00\x00\x00\x18ftypisom"
            ),
            Some(StreamFormat::M4a)
        );
        assert_eq!(
            detect_stream_format(
                "https://example.test/play?id=6",
                None,
                b"\x00\x00\x00\x18ftypisom"
            ),
            Some(StreamFormat::M4a)
        );
        assert_eq!(
            detect_stream_format(
                "https://example.test/song.flac",
                Some("audio/flac"),
                b"fLaC"
            ),
            Some(StreamFormat::Flac)
        );
        assert_eq!(
            detect_stream_format(
                "https://example.test/play?id=2",
                Some("audio/vorbis"),
                b"OggS\x00\x00\x00vorbis"
            ),
            Some(StreamFormat::OggVorbis)
        );
        assert_eq!(
            detect_stream_format(
                "https://example.test/play?id=2",
                Some("application/ogg"),
                b"OggS\x00\x00\x00OpusHead"
            ),
            None
        );
        assert_eq!(
            detect_stream_format(
                "https://example.test/play?id=3",
                Some("audio/wav"),
                b"RIFF\x00\x00\x00\x00WAVEfmt "
            ),
            Some(StreamFormat::Wav)
        );
        assert_eq!(
            detect_stream_format(
                "https://example.test/cover.jpg",
                Some("image/jpeg"),
                b"\xff\xd8\xff"
            ),
            None
        );
    }

    /// 回归：快进监视任务靠「缓冲被 drop → Sender 随之 drop → rx.changed()
    /// 返回 Err」退出。若这条链路不成立（例如任务又改成持强引用自环），
    /// 任务会永久 park 并把整份流缓冲钉在堆上。
    #[test]
    fn dropping_buffer_closes_fast_forward_channel() {
        let mut rx = {
            let shared = StreamingBuffer::new(Some(4096), None);
            let mut rx = shared.fast_forward_receiver();
            // 发送端仍在（缓冲存活）时 changed() 挂起
            assert!(
                rx.changed().now_or_never().is_none(),
                "缓冲存活时 changed() 不应就绪"
            );
            // 最后一个强引用离开（reader 与下载任务都退出）→ 缓冲 drop
            drop(shared);
            rx
        };
        assert!(
            matches!(rx.changed().now_or_never(), Some(Err(_))),
            "缓冲 drop 后 changed() 必须立即返回 Err（任务退出路径）"
        );
    }

    /// 回归：fail() 后按 Content-Length 预留的 data 容量必须归还
    /// （未下载到的部分不该继续占位）
    #[test]
    fn fail_releases_reserved_data_capacity() {
        let shared = StreamingBuffer::new(Some(DATA_RESERVE_MAX_BYTES), None);
        shared.append(&[0u8; 128]).expect("append should succeed");
        assert!(
            shared.debug_capacities().0 >= DATA_RESERVE_MAX_BYTES as usize,
            "构造时应已按 Content-Length 预留"
        );

        shared.fail("network died".to_string());

        let (data_capacity, _) = shared.debug_capacities();
        assert!(
            data_capacity < DATA_RESERVE_MAX_BYTES as usize / 2,
            "fail() 必须归还未使用的预留容量，实际 {data_capacity}"
        );
    }

    /// 回归：reset_tail 换新快进目标时要把上一轮 tail 的容量真正还掉
    #[test]
    fn reset_tail_returns_previous_tail_capacity() {
        let shared = StreamingBuffer::new(None, None);
        shared.reset_tail(1024);
        shared
            .append_tail(&vec![0u8; 512 * 1024])
            .expect("append_tail should succeed");
        assert!(
            shared.debug_capacities().1 >= 512 * 1024,
            "tail 应已增长到 512KB 量级"
        );

        shared.reset_tail(2048);

        let (_, tail_capacity) = shared.debug_capacities();
        assert_eq!(tail_capacity, 0, "reset_tail 必须归还上一轮的 tail 容量");
    }
}
