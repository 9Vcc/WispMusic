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

/// `read` 等待下一段数据的停滞上限。
///
/// 它同时是 `SeekFrom::End` 等总长度时的上限（见 `Seek for StreamingReader`）：
/// 这两种等待都会把调用方（**音频线程**——解码器在 `rebuild_sink` 里同步读它）
/// 按在原地。没有上限时，只要出现「缓冲既没有新数据、也没被标成失败、又没下载完」
/// 的状态，这条循环就永远转下去，音频线程被这一条命令占死：
/// 命令队列整体停摆，此后每条 `player_*` 都要等满回包超时（20s）才报错，切歌再也不
/// 出声——实测正是「快速切歌后卡住」的最终形态。
///
/// 取 25s 的依据：生产侧自己的空闲超时是 20s（`audio/source.rs` 的 CHUNK_IDLE_TIMEOUT），
/// 真死的流会先由它报错、走上层的正常失败路径；只有**生产任务本身被饿死**（例如
/// runtime worker 被并发切歌占满，没人再轮询它）时，才轮到这条上限兜底。
const STREAMING_READ_STALL_TIMEOUT: Duration = Duration::from_secs(25);

/// 音频命令线程在建解码器/定位时用的收紧预算。
///
/// 同一个 `read` 有两个截然不同的调用方：mixer 线程（它卡住只是当前这首歌暂停发声），
/// 和**音频命令线程**（`rebuild_sink` 里同步建解码器、定位、`skip_duration`）。后者一被
/// 占住，整条命令队列跟着停摆——现场日志里那串「命令占住音频线程 23.1s / 20.1s /
/// 19.8s / 15.5s: SetOutputDevice」正是这个形态：切歌没声、连输出设备都切不动，
/// 每条 `player_*` 都等满 20s 回包超时。
///
/// 取 3s：CDN 首包实测在 1~2s 量级，3s 容得下正常慢首包，又把最坏情况从「超过回包
/// 超时」压到「远低于它」。
const AUDIO_THREAD_READ_STALL_TIMEOUT: Duration = Duration::from_secs(3);

/// 同一次读等待该给多久。纯函数，便于把「音频线程侧必须短于 20s 回包超时」锁进单测。
fn read_stall_budget(caller_is_audio_thread: bool) -> Duration {
    if caller_is_audio_thread {
        AUDIO_THREAD_READ_STALL_TIMEOUT
    } else {
        STREAMING_READ_STALL_TIMEOUT
    }
}

/// `read` 停滞预算的持有者：进入时收紧，离开作用域（含各处 `?` 早退）自动还原。
///
/// 用 RAII 而不是前后两句 store，是因为建解码器这条路径上有好几个早退点；漏还原会把
/// mixer 线程本该用的 25s 永久变成 3s，正常缓冲就会被误判成死流。
#[must_use = "预算靠这个 guard 离开作用域时还原"]
pub(super) struct TightReadBudget<'a> {
    buffer: &'a StreamingBuffer,
    previous_ms: u64,
}

impl Drop for TightReadBudget<'_> {
    fn drop(&mut self) {
        self.buffer
            .read_stall_ms
            .store(self.previous_ms, Ordering::Relaxed);
    }
}

/// `with_hard_deadline` 的持有者：离开作用域（含各处 `?` 早退）时清除总时限。
///
/// 一定要走 guard 而不是前后两次赋值：回退重建这条路径上有好几个早退点，漏清除会
/// 让属主线程此后每次读都立刻超时——那条线程再干别的流式工作（切设备/再点歌）就
/// 直接失败了。
#[must_use = "总时限靠这个 guard 离开作用域时清除"]
pub(super) struct HardDeadlineGuard<'a> {
    buffer: &'a StreamingBuffer,
    previous: Option<HardDeadline>,
}

impl Drop for HardDeadlineGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.buffer.state.lock() {
            state.hard_deadline = self.previous;
        }
        self.buffer.available.notify_all();
    }
}


/// 本轮读等待是否已经超预算。拆成纯函数是为了能把「到点必须退出」这条不变量
/// 单测锁死——让测试真的等 25s 既不现实也不可取。
fn read_wait_timed_out(deadline: Instant, now: Instant) -> bool {
    now >= deadline
}

/// 拖动后的目标位置离已下载边界超过这个字节数、且快进已确认不可用时，直接放弃等待。
/// 按最慢的正常播放码率折算，256KB 已经是好几秒的音频；等顺序下载爬过去毫无意义。
const UNREACHABLE_GAP_BYTES: u64 = 256 * 1024;

/// 上面那种情况用的短上限（远小于 25s 的通用停滞上限）。
const UNREACHABLE_GAP_WAIT_TIMEOUT: Duration = Duration::from_secs(2);

/// 该把这次等待按「够不着的位置」处理吗。
///
/// 源站不认 Range 时（`stream_range_into_tail` 只接受 206，否则把 tail 标记结束），
/// 拖到未下载的位置就只能等顺序下载自己爬过去——几十秒很常见，用户看到的是「拖了没
/// 反应」。紧贴下载边界的小缺口是正常播放的常态（缓冲总领先播放一点点），不能误伤。
fn should_tighten_wait_for_unreachable_gap(gap_bytes: u64, fast_forward_finished: bool) -> bool {
    fast_forward_finished && gap_bytes > UNREACHABLE_GAP_BYTES
}

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

/// 某条线程给自己的**多段**流式工作设的绝对截止时刻（见 `with_hard_deadline`）。
///
/// 单段等待各自都有上限（`read` 3s / `SeekFrom::End` 同预算），但一段工作里可以串起
/// 好几段（建解码器 → 定位 → 解码跳过），加起来照样能把音频命令线程占满回包超时。
/// 到点后属主线程的每次读/等立刻按超时失败，跟取消一样是**不可重试**的错误类别，
/// 会原样穿出去让解码器结束。
#[derive(Clone, Copy)]
struct HardDeadline {
    at: Instant,
    owner: std::thread::ThreadId,
}

/// 「总时限」是否已越过——**只对装上它的那条线程生效**。
///
/// 只认属主线程是刻意的：装的是一条命令自己的总预算（seek 回退路径等），到点就放弃；
/// mixer 等其它读者不能跟着被误伤，否则一次放弃会把正在播的歌也掐掉。
fn hard_deadline_expired(state: &StreamingState) -> bool {
    state.hard_deadline.is_some_and(|deadline| {
        deadline.owner == std::thread::current().id() && Instant::now() >= deadline.at
    })
}

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
    /// 见 `with_hard_deadline`：某条线程给自己的多段流式工作设的绝对截止时刻。
    hard_deadline: Option<HardDeadline>,
}

pub(super) struct StreamingBuffer {
    state: Mutex<StreamingState>,
    available: Condvar,
    /// `read` 的停滞预算（毫秒）。平时是 25s（mixer 线程用），音频命令线程建解码器
    /// 期间由 `tighten_read_budget` 临时收到 3s（见 AUDIO_THREAD_READ_STALL_TIMEOUT）。
    read_stall_ms: AtomicU64,
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
                hard_deadline: None,
            }),
            available: Condvar::new(),
            read_stall_ms: AtomicU64::new(read_stall_budget(false).as_millis() as u64),
            fast_forward_tx,
            final_path,
        })
    }

    /// 临时把 `read` 的停滞预算改成 `budget`，guard 离开作用域时还原成改之前的值。
    ///
    /// 改完立刻唤醒等待者：正在等的那次读每轮都会重新读这个预算（见 read 的循环），
    /// 所以收紧能当场生效，而不是只对下一次读生效。
    pub(super) fn with_read_budget(&self, budget: Duration) -> TightReadBudget<'_> {
        let previous_ms = self.read_stall_ms.swap(
            budget.max(Duration::from_millis(1)).as_millis() as u64,
            Ordering::Relaxed,
        );
        self.available.notify_all();
        TightReadBudget {
            buffer: self,
            previous_ms,
        }
    }

    /// 给**当前线程**装一个绝对总时限（见 HardDeadline），guard 离开作用域时清除。
    ///
    /// 用途是给「回退重建」这类**多段**流式工作设总预算：单段等待各有上限，加起来
    /// 仍可能把音频命令线程占满到回包超时。只影响装上它的那条线程——mixer 和其它
    /// 读者照旧走各自的停滞预算。
    pub(super) fn with_hard_deadline(&self, at: Instant) -> HardDeadlineGuard<'_> {
        let previous = match self.state.lock() {
            Ok(mut state) => {
                let previous = state.hard_deadline;
                state.hard_deadline = Some(HardDeadline {
                    at,
                    owner: std::thread::current().id(),
                });
                previous
            }
            Err(_) => None,
        };
        // 正在等的那次读每轮都会重新判：装完立刻唤醒，别让它把剩余预算等满
        self.available.notify_all();
        HardDeadlineGuard {
            buffer: self,
            previous,
        }
    }

    /// 音频命令线程即将同步读这条流之前收紧预算（见 AUDIO_THREAD_READ_STALL_TIMEOUT）。
    pub(super) fn tighten_read_budget(&self) -> TightReadBudget<'_> {
        self.with_read_budget(read_stall_budget(true))
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

    /// 当前 `read` 停滞预算（毫秒），测试断言「收紧了 / 还原了」用
    #[cfg(test)]
    pub(super) fn debug_read_stall_ms(&self) -> u64 {
        self.read_stall_ms.load(Ordering::Relaxed)
    }

    /// 是否有线程装着总时限，测试断言 guard 的装/清用
    #[cfg(test)]
    pub(super) fn debug_hard_deadline_active(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.hard_deadline.is_some())
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

    /// 音频命令线程即将同步读这条流（建解码器、定位）之前收紧 `read` 的停滞预算，
    /// guard 离开作用域时还原。见 StreamingBuffer::tighten_read_budget。
    pub(super) fn tighten_read_budget(&self) -> TightReadBudget<'_> {
        self.shared.tighten_read_budget()
    }

    /// 给当前线程装一个绝对总时限（回退重建这类多段流式工作的总预算）。
    /// 见 StreamingBuffer::with_hard_deadline。
    pub(super) fn with_hard_deadline(&self, at: Instant) -> HardDeadlineGuard<'_> {
        self.shared.with_hard_deadline(at)
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

    /// 取消信号的 `ErrorKind` 绝不能是 `Interrupted`。
    ///
    /// `Interrupted` 在整个生态里都是「可重试」的意思：std 的 `read_exact` /
    /// `read_to_end` 会吞掉它再读，symphonia 的 `MediaSourceStream::read_buf_exact`
    /// （media_source_stream.rs:319-331）也会吞掉它再读。而陈旧 reader 的这个错误是
    /// **立刻**返回的（不等数据）——于是重试立刻再失败、再重试……**死循环，100% CPU，
    /// 永不返回**。mp3 解码的每一帧帧体都要走 `read_buf_exact`（symphonia-bundle-mp3
    /// demuxer.rs:481），所以「解码/seek 正做到一半，用户点了别的歌」就会把那条线程
    /// 永久占死：现场是音频命令线程被 Seek 占死（此后每条命令都等满 20s 回包超时、
    /// 界面转圈、只能重启），悬停预取则是泄漏一个转满 CPU 的线程 + 钉住整份流缓冲。
    ///
    /// `ConnectionAborted` 表示「这条流被主动中止」，没有任何重试语义，会原样穿出去
    /// 让解码器按普通 I/O 错误结束。
    fn cancelled_error() -> io::Error {
        io::Error::new(
            ErrorKind::ConnectionAborted,
            "streaming audio load cancelled",
        )
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

        // 本次读请求的等待起点（见 STREAMING_READ_STALL_TIMEOUT）：一次成功的读会
        // 立刻 return，所以这个时限管的正是「一直没有数据可取」那种情况。
        // 预算每轮重新取：这样音频线程在等待中途收紧预算，正在等的那次读也能早退出
        // （否则收紧只对下一次读生效，而拖住命令队列的恰恰是这一次）。
        let wait_started = Instant::now();
        let mut gap_deadline: Option<Instant> = None;

        loop {
            if self.is_cancelled() {
                return Err(Self::cancelled_error());
            }
            // 多段工作的总时限（见 with_hard_deadline）：到点后属主线程的每次读都
            // 直接失败，不再吃单段预算——否则「建解码器 3s + 定位 3s + 跳过 3s」
            // 串起来照样能把音频命令线程占满回包超时。
            if hard_deadline_expired(&state) {
                return Err(io::Error::new(
                    ErrorKind::TimedOut,
                    "streaming audio read budget exhausted",
                ));
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

            // 兜底：数据、错误、完结三种信号都没来，且当前请求仍是最新（没被取消）
            // 时，绝不能继续等下去——占用的是音频线程，整条命令队列都会停摆
            let now = Instant::now();
            // 预算每轮现取：音频线程在等待中途收紧预算时，正在等的那次读也能提前退出
            let stall_deadline =
                wait_started + Duration::from_millis(self.shared.read_stall_ms.load(Ordering::Relaxed));
            if gap_deadline.is_none()
                && should_tighten_wait_for_unreachable_gap(
                    self.position.saturating_sub(data_len),
                    state.tail_done,
                )
            {
                // 只收紧一次：每轮重新计时会让这个短上限永远到不了点
                gap_deadline = Some(now + UNREACHABLE_GAP_WAIT_TIMEOUT);
            }
            let wait_deadline = gap_deadline.unwrap_or(stall_deadline);
            if read_wait_timed_out(wait_deadline, now) {
                return Err(io::Error::new(
                    ErrorKind::TimedOut,
                    if gap_deadline.is_some() {
                        "streaming audio seek position is unreachable"
                    } else {
                        "streaming audio data wait timed out"
                    },
                ));
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
                // 只在**总长度未知**时才等（等下载完成或等来 Content-Length）。
                // 长度已经知道就直接回答：Content-Length 本身就是总长度（`byte_len()`
                // 在下载完成前也正是用它），而「等 complete」会把调用方白按在一条僵死
                // 流上——音频命令线程一被按满 20s 回包超时，之后每条 `player_*` 都跟着
                // 超时：切歌转圈、连切换输出设备都没反应。旧实现在这里写死等 30s
                // （`STREAMING_END_WAIT_TIMEOUT`），既超过回包超时，又完全不吃下面那条
                // 收紧预算。
                //
                // 等待上限用和 `read` 同一个停滞预算（音频命令线程进来时被收紧到 3s），
                // 且每轮现取：等待中途收紧也当场生效。
                let wait_started = Instant::now();
                while state.total_len.is_none() && !state.complete {
                    if self.is_cancelled() {
                        return Err(Self::cancelled_error());
                    }
                    // 总时限优先于单段预算：回退重建里这一段也不该跑过头
                    if hard_deadline_expired(&state) {
                        return Err(io::Error::new(
                            ErrorKind::TimedOut,
                            "streaming audio length wait timed out",
                        ));
                    }
                    if let Some(error) = state.error.clone() {
                        return Err(io::Error::new(ErrorKind::Other, error));
                    }
                    let budget_ms = self.shared.read_stall_ms.load(Ordering::Relaxed);
                    if Instant::now() >= wait_started + Duration::from_millis(budget_ms) {
                        return Err(io::Error::new(
                            ErrorKind::TimedOut,
                            "streaming audio length wait timed out",
                        ));
                    }
                    let (next_state, _) = self
                        .shared
                        .available
                        .wait_timeout(state, Duration::from_millis(250))
                        .map_err(|_| {
                            io::Error::new(ErrorKind::Other, "streaming audio buffer poisoned")
                        })?;
                    state = next_state;
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

pub(crate) fn remote_cache_file_name(url: &str) -> String {
    use std::{
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
    };

    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{hash:016x}.{}", remote_cache_ext(url))
}

/// 缓存文件名使用的扩展名（不含点），未知扩展名回落 `mp3`。
///
/// 落盘的写入方（原生流式下载）与查询方（`check_audio_cache`）必须用同一份
/// 映射：两边算出的文件名只要差一个字符，缓存就永远不命中，表现为「明明下过
/// 了却每次重新拉流」。`extract_remote_ext` 对未知扩展名返回 `audio`（供格式
/// 探测判断「认不出来」），这里再折成具体后缀。
pub(crate) fn remote_cache_ext(url: &str) -> &'static str {
    match extract_remote_ext(url) {
        "audio" => "mp3",
        ext => ext,
    }
}

fn extract_remote_ext(url: &str) -> &'static str {
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

    /// 拖到够不着的位置、又快进已确认不可用时，绝不能让音频线程等顺序下载爬过去。
    /// 同时不能误伤正常播放——播放位置本来就总比下载边界落后一点点，那点缺口是常态。
    #[test]
    fn unreachable_seek_position_fails_fast_instead_of_crawling() {
        assert!(should_tighten_wait_for_unreachable_gap(
            UNREACHABLE_GAP_BYTES + 1,
            true
        ));
        assert!(!should_tighten_wait_for_unreachable_gap(
            UNREACHABLE_GAP_BYTES + 1,
            false
        ));
        assert!(!should_tighten_wait_for_unreachable_gap(
            UNREACHABLE_GAP_BYTES - 1,
            true
        ));
        assert!(!should_tighten_wait_for_unreachable_gap(0, true));

        // 短上限必须明显短于通用停滞上限，否则这条收紧没有意义
        assert!(UNREACHABLE_GAP_WAIT_TIMEOUT < STREAMING_READ_STALL_TIMEOUT);
    }

    /// 快速切歌后「彻底没声」的实测根因，是音频线程被一条 `read` 占死、命令队列整体
    /// 停摆。这条等待必须有硬上限：这里锁死上限存在，以及它与生产侧空转超时的相对
    /// 关系（生产侧 20s 自己会先报错，25s 这条只兜「没人再负责这条流」的情况）。
    #[test]
    fn streaming_read_wait_has_a_stall_ceiling() {
        let started = Instant::now();
        let deadline = started + STREAMING_READ_STALL_TIMEOUT;

        assert!(!read_wait_timed_out(deadline, started));
        assert!(!read_wait_timed_out(
            deadline,
            started + STREAMING_READ_STALL_TIMEOUT - Duration::from_millis(1)
        ));
        assert!(read_wait_timed_out(
            deadline,
            started + STREAMING_READ_STALL_TIMEOUT
        ));
        assert!(read_wait_timed_out(
            deadline,
            started + STREAMING_READ_STALL_TIMEOUT * 2
        ));

        // 短于它就越过「生产侧先报错」的正常路径，把慢流误判成死流
        assert!(STREAMING_READ_STALL_TIMEOUT > Duration::from_secs(20));
    }

    #[test]
    fn audio_thread_read_budget_is_shorter_than_the_command_timeout() {
        // 音频线程侧的预算必须明显短于 20s 回包超时：超过它，这条命令的调用方就已经
        // 报错放弃了，而队列里排在后面的每条命令（含切换输出设备）跟着一起烂。
        assert!(AUDIO_THREAD_READ_STALL_TIMEOUT < Duration::from_secs(20));
        // mixer 侧继续用宽松预算，正常缓冲不能被当成死流
        assert!(read_stall_budget(false) > read_stall_budget(true));
        assert_eq!(read_stall_budget(false), STREAMING_READ_STALL_TIMEOUT);
        assert_eq!(read_stall_budget(true), AUDIO_THREAD_READ_STALL_TIMEOUT);
    }

    /// guard 必须**无论怎么走**都还原预算：建解码器那条路径上有好几个 `?` 早退点，
    /// 漏还原会把 mixer 线程本该用的 25s 永久变成 3s（正常慢缓冲被误判成死流）。
    #[test]
    fn read_budget_returns_to_the_lenient_value_even_on_the_error_path() {
        let shared = StreamingBuffer::new(Some(4096), None);
        let lenient = read_stall_budget(false).as_millis() as u64;
        let tight = read_stall_budget(true).as_millis() as u64;
        assert_eq!(shared.debug_read_stall_ms(), lenient, "初始应是宽松预算");

        // 模拟 `?` 早退：guard 随内层作用域析构，一次都没被显式使用
        let early: Result<(), &str> = {
            let _guard = shared.tighten_read_budget();
            assert_eq!(shared.debug_read_stall_ms(), tight, "收紧后应按短预算等待");
            Err("decoder init failed")
        };
        assert!(early.is_err());
        assert_eq!(
            shared.debug_read_stall_ms(),
            lenient,
            "早退也必须还原宽松预算"
        );
    }

    /// 收紧必须真的作用在等待上：空缓冲、没有任何生产方时，带着 guard 的这次读要在
    /// 短预算内报错退出。改回「无论谁都等 25s」的话，这条测试会红。
    #[test]
    fn tightened_budget_makes_a_stalled_read_give_up_early() {
        let shared = StreamingBuffer::new(Some(4096), None);
        let latest_request_id = Arc::new(AtomicU64::new(1));
        let reader = StreamingReader::new(Arc::clone(&shared), latest_request_id, 1);
        let mut output = [0u8; 4];

        let started = Instant::now();
        let guard = reader.tighten_read_budget();
        let error = reader
            .clone()
            .read(&mut output)
            .expect_err("没有数据可取时必须按时退出");
        let elapsed = started.elapsed();
        std::mem::drop(guard);

        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(
            elapsed < STREAMING_READ_STALL_TIMEOUT,
            "收紧后不该等满宽松预算，实际 {elapsed:?}"
        );
        assert_eq!(
            shared.debug_read_stall_ms(),
            read_stall_budget(false).as_millis() as u64,
            "读完了也要还原"
        );
    }

    /// 收紧必须**当场**生效：已经在等的那次读也要提前撒手。
    /// 拖住命令队列的恰恰是那一次等待，只对下一次读生效等于没改。
    #[test]
    fn tightening_the_budget_releases_a_read_that_is_already_waiting() {
        let shared = StreamingBuffer::new(Some(4096), None);
        let latest_request_id = Arc::new(AtomicU64::new(1));
        let reader = StreamingReader::new(Arc::clone(&shared), latest_request_id, 1);
        let waiter = std::thread::spawn(move || {
            let mut reader = reader;
            let mut output = [0u8; 4];
            reader.read(&mut output)
        });
        // 让它先进入宽松预算的等待（25s，自己不会醒）
        std::thread::sleep(Duration::from_millis(300));

        let started = Instant::now();
        let guard = shared.with_read_budget(Duration::from_millis(200));
        let result = waiter.join().expect("等待中的读线程不该 panic");
        let elapsed = started.elapsed();
        drop(guard);

        assert!(result.is_err(), "没有数据时这次读必须报错退出");
        assert!(
            elapsed < Duration::from_secs(5),
            "收紧要立刻作用在正在等的那次读上，实际又等了 {elapsed:?}"
        );
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
        // 必须是「不可重试」的类别：Interrupted 会被 std/symphonia 吞掉重试成死循环
        assert_eq!(error.kind(), ErrorKind::ConnectionAborted);
    }

    /// 现场复现（2026-10-04 03:58 安装版日志）：音频命令线程被一条 Seek 占死 28s+
    /// 直到重启，期间 Pause/Play/BeginLoad 全部等满 20s 回包超时、界面转圈。
    ///
    /// 病灶是**取消信号的 ErrorKind**：陈旧请求的 `read` 立刻返回 `Interrupted`，而
    /// `Interrupted` 在 std（`read_exact`/`read_to_end`）与 symphonia
    /// （`MediaSourceStream::read_buf_exact`，media_source_stream.rs:319-331）里都被
    /// 当作「可重试」吞掉再读 → 立刻又拿到同一个 Interrupted → 死循环，100% CPU，
    /// 永不返回。凡是在陈旧流上做解码的路径都会中毒：seek 回退路径（音频命令线程）、
    /// 悬停预取的时长探测（spawn_blocking 线程，泄漏一个转满 CPU 的线程 + 整份
    /// 流缓冲）——后者正是「用久了越来越卡」的形态。
    ///
    /// 这条测试直接打在 mp3 解码的帧体读取上：`next_packet` → `read_mpeg_frame` 用
    /// `reader.read_buf_exact(&mut packet[MPEG_HEADER_LEN..])`（symphonia-bundle-mp3
    /// demuxer.rs:481）读每一帧，而它正是那条吞 Interrupted 的循环。取消改用不可重试的
    /// ErrorKind 之前，本测试以「没返回」失败（线程永不结束，故用带超时的 channel 收
    /// 结果，测试进程不会挂死）。
    #[test]
    fn symphonia_body_read_on_a_superseded_stream_must_fail_fast() {
        struct Wrapped(StreamingReader);

        impl Read for Wrapped {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.0.read(buf)
            }
        }

        impl Seek for Wrapped {
            fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
                self.0.seek(pos)
            }
        }

        impl symphonia::core::io::MediaSource for Wrapped {
            fn is_seekable(&self) -> bool {
                true
            }

            fn byte_len(&self) -> Option<u64> {
                self.0.byte_len()
            }
        }

        let shared = StreamingBuffer::new(Some(1024), None);
        let latest_request_id = Arc::new(AtomicU64::new(9));
        // reader 的请求号落后于最新代次 = 这次载入已被取代
        let reader = StreamingReader::new(Arc::clone(&shared), latest_request_id, 8);
        let mut source = symphonia::core::io::MediaSourceStream::new(
            Box::new(Wrapped(reader)),
            Default::default(),
        );

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use symphonia::core::io::ReadBytes;
            let mut frame_body = [0u8; 64];
            let outcome = source
                .read_buf_exact(&mut frame_body)
                .err()
                .map(|err| err.kind());
            let _ = tx.send(outcome);
        });

        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Some(kind)) => {
                assert_eq!(
                    kind,
                    ErrorKind::ConnectionAborted,
                    "取消信号必须是不可重试的类别——Interrupted 会被 std/symphonia 吞掉重试"
                );
            }
            Ok(None) => panic!("陈旧流上读帧体不该成功"),
            Err(_) => panic!(
                "superseded 流上读帧体没有返回：取消错误被当成可重试，陷入死循环\
                 （音频命令线程/预取线程会被永久占死）"
            ),
        }
    }

    /// `SeekFrom::End` 在**总长度已知**时必须立刻回答，不能去等「整首下载完成」。
    ///
    /// 旧实现无条件等 `complete`：Content-Length 早就拿到了也照等，等于把调用方
    /// （音频命令线程/mixer）按在一条僵死流上等满 30s——音频命令线程一超 20s 回包
    /// 超时，之后每条 `player_*` 都跟着超时：切歌转圈、连切换输出设备都没反应。
    #[test]
    fn seek_from_end_answers_from_content_length_without_waiting_for_download() {
        let shared = StreamingBuffer::new(Some(4096), None);
        let latest_request_id = Arc::new(AtomicU64::new(1));
        let mut reader = StreamingReader::new(Arc::clone(&shared), latest_request_id, 1);
        shared.append(&[0u8; 16]).expect("append should succeed");

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send((reader.seek(SeekFrom::End(0)), reader.seek(SeekFrom::End(-16))));
        });

        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok((Ok(end), Ok(before_end))) => {
                assert_eq!(end, 4096, "End(0) 应给出 Content-Length");
                assert_eq!(before_end, 4080, "End(负偏移) 应相对总长度回退");
            }
            Ok((outcome, _)) => panic!("已知长度必须立刻给出答案，实际：{outcome:?}"),
            Err(_) => panic!("已知 Content-Length 却去等下载完成——僵死流上这就是 30s 占死"),
        }
    }

    /// 长度确实未知时该等，但上限要跟 `read` 走同一条停滞预算（音频命令线程 3s），
    /// 而不是写死的 30s（30s > 20s 回包超时，整条命令队列会被拖死）；且等待**中途**
    /// 收紧也要当场生效。
    #[test]
    fn seek_from_end_wait_follows_the_tightened_budget() {
        let shared = StreamingBuffer::new(None, None);
        let latest_request_id = Arc::new(AtomicU64::new(1));
        let mut reader = StreamingReader::new(Arc::clone(&shared), latest_request_id, 1);

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let started = Instant::now();
            let outcome = reader.seek(SeekFrom::End(0));
            let _ = tx.send((outcome.is_err(), started.elapsed()));
        });
        // 让它先按宽松预算等上（25s，自己不会醒），再模拟音频命令线程收紧
        std::thread::sleep(Duration::from_millis(200));
        let guard = shared.with_read_budget(Duration::from_millis(500));

        let (is_err, elapsed) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("长度未知又等不来时必须按时报错，而不是写死等 30s");
        drop(guard);

        assert!(is_err, "等不到总长度必须报错");
        assert!(
            elapsed < Duration::from_secs(5),
            "上限要跟着收紧预算走，实际等了 {elapsed:?}"
        );
    }

    /// 总时限是**属主线程**的：到点后装上它的线程每次读都失败，别的线程（mixer）
    /// 照读不误——否则一次 seek 回退放弃会把正在播的歌也掐掉。
    #[test]
    fn hard_deadline_only_fails_reads_on_the_thread_that_installed_it() {
        let shared = StreamingBuffer::new(None, None);
        shared.append(b"abcdef").expect("append should succeed");
        let latest_request_id = Arc::new(AtomicU64::new(1));

        let mut owner_reader =
            StreamingReader::new(Arc::clone(&shared), Arc::clone(&latest_request_id), 1);
        let other_reader = StreamingReader::new(Arc::clone(&shared), latest_request_id, 1);

        // 装一个已经越过的总时限（属主 = 当前测试线程）
        let guard = shared.with_hard_deadline(Instant::now() - Duration::from_secs(1));
        assert!(shared.debug_hard_deadline_active());

        // 非属主线程不受影响
        let other_read = std::thread::spawn(move || {
            let mut reader = other_reader;
            let mut output = [0u8; 3];
            reader.read(&mut output).map(|len| (len, output))
        })
        .join()
        .expect("join");
        let (len, bytes) = other_read.expect("非属主线程不该被总时限拦下");
        assert_eq!((len, bytes), (3, [b'a', b'b', b'c']));

        // 属主线程立刻按超时失败
        let error = owner_reader
            .read(&mut [0u8; 3])
            .expect_err("属主线程必须被总时限拦住");
        assert_eq!(error.kind(), ErrorKind::TimedOut);

        drop(guard);
        assert!(!shared.debug_hard_deadline_active(), "guard 离开作用域必须清掉总时限");
    }

    /// 总时限要能截断**正在等**的那次读：单段预算 25s 时，等的那次读会在到点后
    /// 250ms 内（每轮唤醒一次的粒度）失败退出，而不是把剩余预算等满。
    #[test]
    fn hard_deadline_releases_a_waiting_read_early() {
        let shared = StreamingBuffer::new(None, None);
        let latest_request_id = Arc::new(AtomicU64::new(1));
        let reader_shared = Arc::clone(&shared);

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // 属主必须就是等在里面的这条线程；guard 单独用一份 Arc（reader 要 &mut 读）
            let shared_for_deadline = Arc::clone(&reader_shared);
            let reader = StreamingReader::new(reader_shared, latest_request_id, 1);
            let _guard =
                shared_for_deadline.with_hard_deadline(Instant::now() + Duration::from_millis(300));
            let mut reader = reader;
            let started = Instant::now();
            let outcome = reader.read(&mut [0u8; 4]);
            let _ = tx.send((outcome.is_err(), started.elapsed()));
        });

        let (is_err, elapsed) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("总时限必须截断这次等待，而不是等满 25s 的单段预算");
        assert!(is_err, "越界后这次读必须报错");
        assert!(
            elapsed < Duration::from_secs(3),
            "300ms 的总时限不该等到 3s 以上，实际 {elapsed:?}"
        );
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
