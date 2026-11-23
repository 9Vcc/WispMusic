//! Manual soak for the wasm provider slot.
//!
//! Run it with a real package:
//! `WISP_NETEASE_WISP=... cargo test -p music --test plugin_soak -- --ignored --nocapture`
//!
//! `WISP_SOAK_SECS` shortens the measured run for a quick check; the default is ten minutes.
//! `WISP_SOAK_WARMUP_SECS` controls the unmeasured warmup (default two minutes) that lets the
//! wasm heap, JIT pages and allocator reach their steady state. The test keeps one slot busy at
//! 20 pure ops per second, swaps three package copies in a row, and samples the process working
//! set. It is ignored so ordinary test runs never wait for it.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use music::plugins::runtime::{PluginHealth, WasmPlugin};
use wisp_plugin_api::CoverAtArgs;
use wisp_plugin_api::CoverSizeDto;
use wisp_plugin_api::ops::music_api::COVER_AT;

const CALL_INTERVAL: Duration = Duration::from_millis(50);
const LOG_INTERVAL: Duration = Duration::from_secs(30);
const SWAPS: usize = 3;
const RSS_LIMIT_PERCENT: u64 = 105;

#[test]
#[ignore = "manual ten-minute RSS soak; set WISP_NETEASE_WISP"]
fn a_ten_minute_soak_keeps_plugin_memory_flat() {
    let package = std::env::var_os("WISP_NETEASE_WISP")
        .expect("set WISP_NETEASE_WISP to a real .wisp package");
    let duration = secs("WISP_SOAK_SECS", 600);
    let warmup = secs("WISP_SOAK_WARMUP_SECS", 120);
    let args = serde_json::to_string(&CoverAtArgs {
        cover: "https://p1.music.126.net/a.jpg".to_owned(),
        size: CoverSizeDto::Full,
    })
    .expect("the cover args");

    let dir = tempfile::tempdir().expect("a temp dir");
    let plugin = WasmPlugin::new(Path::new(&package)).expect("the real plugin");
    assert_eq!(plugin.health(), PluginHealth::Active);
    assert!(!plugin.describe().expect("describe").is_empty());
    plugin.call(COVER_AT, &args).expect("the warmup op");

    // Three real hot swaps with the same package bytes. Different paths force the shadow path
    // even though the manifest version is unchanged.
    let mut copies: Vec<PathBuf> = Vec::with_capacity(SWAPS);
    for index in 0..SWAPS {
        let copy = dir.path().join(format!("soak-{index}.wisp"));
        fs::copy(&package, &copy).expect("the package copy");
        plugin.swap(&copy).expect("the hot swap");
        assert_eq!(plugin.health(), PluginHealth::Active);
        plugin.call(COVER_AT, &args).expect("the post-swap op");
        copies.push(copy);
    }
    assert_eq!(plugin.health(), PluginHealth::Active);

    let started = Instant::now();
    let mut warmup_calls = 0u64;
    let mut next_log = Duration::ZERO;
    println!(
        "soak: warmup {}s rss={}",
        warmup.as_secs(),
        rss::display(rss::working_set())
    );
    while started.elapsed() < warmup {
        plugin.call(COVER_AT, &args).expect("the warmup op");
        warmup_calls += 1;
        thread::sleep(CALL_INTERVAL);
        let elapsed = started.elapsed();
        if elapsed >= next_log {
            println!(
                "soak: warmup t={}s calls={warmup_calls} rss={}",
                elapsed.as_secs(),
                rss::display(rss::working_set())
            );
            next_log += LOG_INTERVAL;
        }
    }

    let baseline = rss::working_set();
    let soak_started = Instant::now();
    let mut calls = 0u64;
    let mut peak = baseline;
    let mut low = baseline;
    let mut next_log = Duration::ZERO;
    println!(
        "soak: measured {}s baseline={}",
        duration.as_secs(),
        rss::display(baseline)
    );

    while soak_started.elapsed() < duration {
        plugin.call(COVER_AT, &args).expect("the soak op");
        calls += 1;
        thread::sleep(CALL_INTERVAL);

        if let Some(rss) = rss::working_set() {
            peak = Some(peak.map_or(rss, |value| value.max(rss)));
            low = Some(low.map_or(rss, |value| value.min(rss)));
        }
        let elapsed = soak_started.elapsed();
        if elapsed >= next_log {
            println!(
                "soak: t={}s calls={calls} rss={}",
                elapsed.as_secs(),
                rss::display(rss::working_set())
            );
            next_log += LOG_INTERVAL;
        }
    }

    let elapsed = soak_started.elapsed();
    let final_rss = rss::working_set();
    println!(
        "soak: done t={}s calls={calls} swaps={SWAPS} rss={} low={} peak={} packages={}",
        elapsed.as_secs(),
        rss::display(final_rss),
        rss::display(low),
        rss::display(peak),
        copies.len()
    );
    assert!(
        calls >= (duration.as_secs() * 10).saturating_sub(10),
        "the soak ran too few calls: {calls}"
    );
    assert_eq!(plugin.health(), PluginHealth::Active);

    if let (Some(base), Some(peak)) = (baseline, peak) {
        let change = peak.saturating_mul(100) / base.max(1);
        assert!(
            change <= RSS_LIMIT_PERCENT,
            "the process working set grew from {} to {} ({change}%)",
            rss::display(Some(base)),
            rss::display(Some(peak))
        );
    }
}

fn secs(name: &str, default: u64) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(default))
}

#[cfg(windows)]
mod rss {
    use std::ffi::c_void;

    #[repr(C)]
    #[allow(non_snake_case)]
    struct ProcessMemoryCounters {
        cb: u32,
        PageFaultCount: u32,
        PeakWorkingSetSize: usize,
        WorkingSetSize: usize,
        QuotaPeakPagedPoolUsage: usize,
        QuotaPagedPoolUsage: usize,
        QuotaPeakNonPagedPoolUsage: usize,
        QuotaNonPagedPoolUsage: usize,
        PagefileUsage: usize,
        PeakPagefileUsage: usize,
    }

    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }

    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
    }

    pub fn working_set() -> Option<u64> {
        let mut counters: ProcessMemoryCounters = unsafe { std::mem::zeroed() };
        counters.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        let ok = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut counters,
                std::mem::size_of::<ProcessMemoryCounters>() as u32,
            )
        };
        (ok != 0).then_some(counters.WorkingSetSize as u64)
    }

    pub fn display(bytes: Option<u64>) -> String {
        bytes.map_or_else(
            || "unknown".to_owned(),
            |bytes| format!("{:.1} MiB", bytes as f64 / (1024. * 1024.)),
        )
    }
}

#[cfg(not(windows))]
mod rss {
    pub fn working_set() -> Option<u64> {
        None
    }

    pub fn display(_bytes: Option<u64>) -> String {
        "unknown".to_owned()
    }
}