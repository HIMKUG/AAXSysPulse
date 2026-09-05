//! 性能测量开关（P5）：`SYSOVERLAY_PERF=1` 时统计渲染开销并落盘。
//!
//! # 为什么需要它
//!
//! 悬浮窗是 GUI 子系统进程（`windows_subsystem = "windows"`），**没有控制台**，
//! `eprintln!` 看不到。所以这里直接写文件：
//!
//! ```text
//! %LOCALAPPDATA%\SysOverlayC\perf.log
//! ```
//!
//! 每 5 秒追加一行：`frames`（实际渲染帧数）、`skipped`（脏检查跳过的帧）、
//! `avg_us` / `max_us`（单帧耗时，微秒）。
//!
//! # 零成本原则
//!
//! 开关关闭时，[`begin`] 返回 0、[`end_render`] 立即返回 —— 一次原子读 + 一次分支，
//! 不做任何计时、不分配、不写盘。
//!
//! # 怎么用数据判断优化是否生效
//!
//! - `frames` ≈ 你设定的刷新率（1fps → 每秒约 1 帧）：说明 P0 的 WM_PAINT 忙循环已修好；
//! - `skipped` 持续增长：说明 P1 脏检查在生效（内容没变就不渲染）；
//! - `avg_us` 下降：说明 P2/P3 的每帧成本优化生效。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

/// 是否启用（进程内只读一次环境变量）。
fn enabled() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| {
        std::env::var("SYSOVERLAY_PERF")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

const REPORT_INTERVAL_MS: u64 = 5_000;

static FRAMES: AtomicU64 = AtomicU64::new(0);
static SKIPPED: AtomicU64 = AtomicU64::new(0);
static TOTAL_US: AtomicU64 = AtomicU64::new(0);
static MAX_US: AtomicU64 = AtomicU64::new(0);
static WINDOW_START_MS: AtomicU64 = AtomicU64::new(0);
/// 报告窗口内是否已有累计（避免刚启动就打一行 0 帧）。
static HAS_DATA: AtomicBool = AtomicBool::new(false);

/// 高精度计时起点（开关关闭时返回 0）。
pub fn begin() -> u64 {
    if !enabled() {
        return 0;
    }
    qpc()
}

/// 结束一次**实际渲染**的计时并累计。
pub fn end_render(start: u64) {
    if !enabled() || start == 0 {
        return;
    }
    let us = elapsed_us(start);
    FRAMES.fetch_add(1, Ordering::Relaxed);
    TOTAL_US.fetch_add(us, Ordering::Relaxed);
    HAS_DATA.store(true, Ordering::Relaxed);
    // max：CAS 更新（无锁）
    let mut cur = MAX_US.load(Ordering::Relaxed);
    while us > cur {
        match MAX_US.compare_exchange_weak(cur, us, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(v) => cur = v,
        }
    }
    maybe_report();
}

/// 记录一次被脏检查跳过的帧（P1）。
pub fn note_skipped() {
    if !enabled() {
        return;
    }
    SKIPPED.fetch_add(1, Ordering::Relaxed);
    HAS_DATA.store(true, Ordering::Relaxed);
    maybe_report();
}

/// 每 [`REPORT_INTERVAL_MS`] 落盘一次统计（覆盖式：文件只保留最近 200 行由调用方裁剪，
/// 这里用追加，避免每次读整个文件）。
fn maybe_report() {
    let now = qpc_ms();
    let start = WINDOW_START_MS.load(Ordering::Relaxed);
    if start == 0 {
        WINDOW_START_MS.store(now, Ordering::Relaxed);
        return;
    }
    if now.saturating_sub(start) < REPORT_INTERVAL_MS {
        return;
    }
    if !HAS_DATA.load(Ordering::Relaxed) {
        WINDOW_START_MS.store(now, Ordering::Relaxed);
        return;
    }
    // 抢到窗口的线程负责输出
    if WINDOW_START_MS
        .compare_exchange(start, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    let frames = FRAMES.swap(0, Ordering::Relaxed);
    let skipped = SKIPPED.swap(0, Ordering::Relaxed);
    let total = TOTAL_US.swap(0, Ordering::Relaxed);
    let max = MAX_US.swap(0, Ordering::Relaxed);
    let avg = if frames > 0 { total / frames } else { 0 };

    let line = format!(
        "frames={:<6} skipped={:<6} avg_us={:<6} max_us={:<6}\n",
        frames, skipped, avg, max
    );
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        let dir = std::path::PathBuf::from(dir).join("SysOverlayC");
        let _ = std::fs::create_dir_all(&dir);
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("perf.log"))
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

// ── QPC 封装 ─────────────────────────────────────────────

fn qpc() -> u64 {
    unsafe {
        let mut c: i64 = 0;
        let _ = windows::Win32::System::Performance::QueryPerformanceCounter(&mut c);
        c as u64
    }
}

fn freq() -> u64 {
    static F: OnceLock<u64> = OnceLock::new();
    *F.get_or_init(|| unsafe {
        let mut f: i64 = 0;
        if windows::Win32::System::Performance::QueryPerformanceFrequency(&mut f).is_ok() && f > 0 {
            f as u64
        } else {
            1_000 // 兜底：假装 1 count = 1 ms
        }
    })
}

fn elapsed_us(start: u64) -> u64 {
    let d = qpc().saturating_sub(start);
    // 避免溢出：先算毫秒再转微秒
    (d as u128 * 1_000_000 / freq() as u128) as u64
}

fn qpc_ms() -> u64 {
    (qpc() as u128 * 1_000 / freq() as u128) as u64
}
