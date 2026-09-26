//! Rust 堆占用计数（诊断用）。
//!
//! 进程内存报告此前只有工作集/私有提交量——那是 Rust 堆 + WebView2 运行时 +
//! 各种映射混在一起的总数，区分不了「流缓冲、任务持有的大分配」这类原生滞留。
//! 这里在全局分配器上做最简单的记账（分配加、释放减、realloc 换算），
//! 用两个 Relaxed 原子给出「当前存活」与「历史峰值」，让内存报告能单独看到
//! Rust 侧的数字。
//!
//! 口径说明：只覆盖 Rust 侧的堆分配，不含 WebView2 渲染进程（那边的 V8 堆由
//! 前端 `performance.memory` 报告）；系统 DLL、内存映射文件也不在此列。
//! 开销是每次分配一两次 Relaxed 原子操作，常态常开（诊断要覆盖 release）。

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

pub struct CountingAllocator;

fn record_alloc(size: usize) {
    let live = LIVE_BYTES.fetch_add(size, Ordering::Relaxed) + size;
    PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        // 失败时原块仍有效，计数不变
        if !new_ptr.is_null() {
            record_alloc(new_size);
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        new_ptr
    }
}

/// (当前存活字节数, 历史峰值字节数)
pub fn snapshot() -> (u64, u64) {
    let live = LIVE_BYTES.load(Ordering::Relaxed);
    // 两次读取之间存在并发分配/释放的窗口，单独读 peak 可能落后于 live；
    // 取两者的较大者，保证返回的这一对永远满足 peak >= live
    let peak = PEAK_BYTES.load(Ordering::Relaxed).max(live);
    (live as u64, peak as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 计数器是进程全局的，而同进程内其他测试并行分配/释放（流缓冲测试会
    /// 瞬时放掉几十 MB），单次读数会被噪声淹没。这里只钉两件在噪声下仍成立
    /// 的事：大块分配确实被计入（取多次采样的最大增量），释放确实让计数回落
    /// （取多次采样的最小读数）；容差远小于被观测的 64MB 块。
    #[test]
    fn counts_live_bytes_and_tracks_peak() {
        const BLOCK: usize = 64 * 1024 * 1024;
        const MIN_DELTA: i64 = 16 * 1024 * 1024;
        const SAMPLES: usize = 32;

        let baseline = snapshot().0 as i64;
        let big = vec![0u8; BLOCK];

        let mut best_delta = i64::MIN;
        for _ in 0..SAMPLES {
            best_delta = best_delta.max(snapshot().0 as i64 - baseline);
            if best_delta >= MIN_DELTA {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let (live, peak) = snapshot();
        assert!(
            best_delta >= MIN_DELTA,
            "持有 64MB 时存活计数的最大增量应达到 16MB：增量 {best_delta}，当前 {live}"
        );
        assert!(peak >= live, "峰值不应小于当前存活：{peak} < {live}");

        drop(big);

        let mut lowest = i64::MAX;
        for _ in 0..SAMPLES {
            lowest = lowest.min(snapshot().0 as i64);
            if lowest <= live as i64 - MIN_DELTA {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            lowest <= live as i64 - MIN_DELTA,
            "释放 64MB 后存活计数应至少回落 16MB：持有时 {live}，最低观测 {lowest}"
        );
    }
}
