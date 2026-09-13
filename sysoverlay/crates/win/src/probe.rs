//! 诊断探针（P-DIAG）：`AAXSYSPULSE_PROBE=1`（或设置面板「调试日志」开启）时
//! 把各采样器的**原始 API 数据**逐行写入统一日志文件的 `probe` 流。
//!
//! 落盘出口统一收敛到 [`crate::logging`]（单文件 `aaxsyspulse_YYYYMMDD_HHMM.log`、
//! 常驻句柄、`[hh:mm:ss.mmm] [probe] 消息` 行格式），本模块只负责：
//!
//! - 开关门控（设置面板开关 **或** 环境变量，二者取或）；
//! - 行序号（诊断用，确认没有丢行）。
//!
//! # 用途
//!
//! Net/GPU/Power 三个源曾经是纯桩（永远 0 / `--`），接上真实 API 后
//! 如果数值仍然不对，靠这份日志可以直接看到底层返回了什么：
//!
//! ```text
//! [10:24:31.412] [probe] #000123 net row 3 type=71 oper=1 alias="WLAN" in=12345678 out=8765432 keep=1
//! ```
//!
//! # 零成本原则
//!
//! 开关关闭时 [`log`] 在第一次字符串拼接前就返回，不做任何格式化、
//! 不碰文件。采样频率本身 ≤1Hz（Net/GPU 1s、Power 3s），
//! 落盘开销可以忽略。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// 全局开关（由设置面板的「调试日志」项在运行时切换）。
static DEBUG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 设置面板调用：开/关调试日志。立即生效，无需重启。
pub fn set_debug(on: bool) {
    DEBUG.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// 当前是否开启调试日志（供设置面板回显）。
pub fn debug() -> bool {
    DEBUG.load(std::sync::atomic::Ordering::Relaxed)
}

/// 是否启用：设置面板开关 **或** 环境变量 `AAXSYSPULSE_PROBE=1`（二者取或）。
///
/// 环境变量作为兜底通道，方便无法进设置面板的场景（如沙箱/脚本）开启。
pub fn enabled() -> bool {
    DEBUG.load(std::sync::atomic::Ordering::Relaxed) || env_probe_on()
}

/// 环境变量开关（进程内只读一次，避免每次 log 都读环境）。
fn env_probe_on() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| {
        std::env::var("AAXSYSPULSE_PROBE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

/// 行序号（诊断用，确认没有丢行）。
static SEQ: AtomicU64 = AtomicU64::new(0);

/// 追加一行探针日志（行号作为消息前缀，时间戳由统一日志模块补齐）。
/// 未启用时立即返回。
pub fn log(line: &str) {
    if !enabled() {
        return;
    }
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    crate::logging::write(crate::logging::Stream::Probe, &format!("#{n:06} {line}"));
}

/// 延迟格式化：仅在 [`enabled()`] 为真时才拼接字符串，关闭开关时零成本。
///
/// 用来替换 `log(&format!(...))`——后者在参数求值阶段就会白白构造字符串，
/// 即使探针关闭也照付一次 `format!` + 堆分配开销（热路径每周期一次）。
pub fn log_lazy<F: FnOnce() -> String>(f: F) {
    if !enabled() {
        return;
    }
    log(&f());
}

/// 启动头：版本 + 环境摘要，便于在日志里定位一次运行。
pub fn log_startup() {
    if !enabled() {
        return;
    }
    let pid = std::process::id();
    log(&format!(
        "startup pid={} perf={} probe=1",
        pid,
        perf_flag()
    ));
}

/// 安装全局 panic hook：任何线程 panic 都把位置 + 消息写入统一日志的 probe 流。
///
/// 为什么关键：wndproc 是 `extern "system"`，panic 一旦跨过该边界就是
/// UB/abort——表现正是「点一下按钮整个界面冻结」。没有 hook，
/// panic 消息只走 stderr（windows_subsystem 下不可见）。
/// hook 内的日志写失败也无所谓（尽力而为），绝不二次 panic。
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".into());
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string payload>".into()
        };
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>").to_string();
        // 无条件写：panic 时 probe 开关多半没开，而 panic 正是最需要抓的现场；
        // panic 极罕见，绕过 gate 的写盘成本可忽略。
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        crate::logging::write(
            crate::logging::Stream::Probe,
            &format!("#{n:06} PANIC thread={name} loc={loc} msg={msg}"),
        );
        default(info);
    }));
}

fn perf_flag() -> bool {
    std::env::var("AAXSYSPULSE_PERF")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}
