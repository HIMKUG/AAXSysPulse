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
use std::sync::{Mutex, OnceLock};

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

// ── 消息级耗时探针（UI 线程谁在卡？） ─────────────────────
//
// SYSOVERLAY_PERF=1 时，消息循环每条消息从 GetMessageW 返回到
// DispatchMessageW 返回之间的耗时（wall-time）若超过 MSG_SLOW_US
// 阈值，立即追加一行到 perf.log：`msg slow hwnd=.. msg=0x.. us=..`。
// 用于定位「UI 线程被谁占住」（进程扫描 / 渲染 / 设置窗口重绘）。

/// 慢消息阈值（微秒）：超过即落盘。默认 20ms —— 超过人眼感知不跟手。
const MSG_SLOW_US: u64 = 20_000;
/// 消息循环累计统计（5s 窗口），与渲染统计共用 REPORT_INTERVAL_MS。
static MSG_COUNT: AtomicU64 = AtomicU64::new(0);
static MSG_SLOW_COUNT: AtomicU64 = AtomicU64::new(0);
static MSG_MAX_US: AtomicU64 = AtomicU64::new(0);
static MSG_MAX_INFO: Mutex<Option<String>> = Mutex::new(None);
/// 最近消息轨迹（环形，Mutex 保护；只写不删，探针用）。
static MSG_TRAIL: Mutex<Vec<(u64, usize, u32)>> = Mutex::new(Vec::new());
const TRAIL_MAX: usize = 16;

/// 消息循环在 DispatchMessageW 之前调用：仅当开启时取一个 QPC 起点。
pub fn msg_begin() -> u64 {
    if !enabled() {
        return 0;
    }
    qpc()
}

/// DispatchMessageW 返回后调用。
pub fn msg_end(start: u64, hwnd: usize, msg: u32) {
    if !enabled() || start == 0 {
        return;
    }
    // 消息洪流采样：每 5000 条记一笔 (hwnd,msg)，定位是谁在被疯狂唤醒
    if crate::probe::enabled() {
        let n = MSG_COUNT.load(Ordering::Relaxed);
        if n % 5_000 == 0 && n < 100_000 {
            crate::probe::log(&format!("msgbit n={n} hwnd={hwnd:#x} msg=0x{msg:04X} us={}", elapsed_us(start)));
        }
    }
    let us = elapsed_us(start);
    // 轨迹（最慢者优先保留，便于崩溃前定位）
    if let Ok(mut trail) = MSG_TRAIL.lock() {
        if trail.len() < TRAIL_MAX {
            trail.push((us, hwnd, msg));
            trail.sort_by_key(|&(u, _, _)| std::cmp::Reverse(u));
        } else if let Some(last) = trail.last_mut() {
            if us > last.0 {
                *last = (us, hwnd, msg);
            }
            trail.sort_by_key(|&(u, _, _)| std::cmp::Reverse(u));
        }
    }
    MSG_COUNT.fetch_add(1, Ordering::Relaxed);
    let mut cur = MSG_MAX_US.load(Ordering::Relaxed);
    while us > cur {
        match MSG_MAX_US.compare_exchange_weak(cur, us, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(v) => cur = v,
        }
    }
    if us > MSG_SLOW_US {
        MSG_SLOW_COUNT.fetch_add(1, Ordering::Relaxed);
        // 记录最慢一笔的归属（粗略即可：取第一个跨阈值——够定位方向）
        if let Ok(mut info) = MSG_MAX_INFO.lock() {
            if info.is_none() {
                *info = Some(format!("hwnd={:#x} msg=0x{:04X} us={}", hwnd, msg, us));
                // 立即追加，不等 5s 聚合窗口 —— 慢消息是稀缺事件，即时可见
                write_line(&format!("msg_slow hwnd={:#x} msg=0x{:04X} us={}\n", hwnd, msg, us));
            }
        }
    }
    if MSG_COUNT.fetch_add(0, Ordering::Relaxed) > 0 {
        // 每 5s 聚合一次（复用渲染的 maybe_report 窗口）
        maybe_report_msgs();
    }
}

fn maybe_report_msgs() {
    let now = qpc_ms();
    static MSGS_WINDOW: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    let start_ref = MSGS_WINDOW.get_or_init(|| std::sync::atomic::AtomicU64::new(0));
    let start = start_ref.load(Ordering::Relaxed);
    if start == 0 {
        start_ref.store(now, Ordering::Relaxed);
        return;
    }
    if now.saturating_sub(start) < REPORT_INTERVAL_MS {
        return;
    }
    if start_ref.compare_exchange(start, now, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        return;
    }
    let count = MSG_COUNT.swap(0, Ordering::Relaxed);
    let slow = MSG_SLOW_COUNT.swap(0, Ordering::Relaxed);
    let max = MSG_MAX_US.swap(0, Ordering::Relaxed);
    let max_info = MSG_MAX_INFO.lock().ok().and_then(|mut m| m.take());
    let trail = MSG_TRAIL
        .lock()
        .ok()
        .map(|mut t| {
            let s: Vec<String> = t
                .drain(..)
                .map(|(us, hwnd, msg)| format!("{:#x}:{:04X}:{us}us", hwnd, msg))
                .collect();
            s.join(" ")
        })
        .unwrap_or_default();
    let line = match max_info {
        Some(info) => format!("msgs={:<6} slow>{}ms={:<4} max_us={:<6} worst={}\n  trail: {}\n",
            count, MSG_SLOW_US / 1000, slow, max, info, trail),
        None => format!("msgs={:<6} slow>{}ms={:<4} max_us={:<6}\n  trail: {}\n",
            count, MSG_SLOW_US / 1000, slow, max, trail),
    };
    write_line(&line);
}

/// 追加一行到 perf.log（不 trim，简单可靠）。
fn write_line(line: &str) {
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

/// 通用耗时探针：`label` 超过阈值（20ms）时立即落盘一次。
/// 用于采样器/进程扫描等**非消息**路径的开销观测。
pub fn note_section(label: &str, start: u64) {
    if !enabled() || start == 0 {
        return;
    }
    let us = elapsed_us(start);
    if us > MSG_SLOW_US {
        write_line(&format!("section {} us={}\n", label, us));
    }
}

/// 采样循环周期探针：记录一轮采样耗时与休眠时长，判别忙等/采样超支。
/// 数值超过阈值时落盘（正常 1s 周期时几乎不写）。
pub fn note_cycle(cost_ms: u32, sleep_ms: u64) {
    if !enabled() {
        return;
    }
    // 忙等（≤2ms 睡眠）或采样超支（>100ms）都值得记录
    if sleep_ms <= 2 || cost_ms > 100 {
        write_line(&format!("cycle cost_ms={} sleep_ms={}\n", cost_ms, sleep_ms));
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
