//! 诊断探针（P-DIAG）：`SYSOVERLAY_PROBE=1` 时把各采样器的**原始 API 数据**
//! 逐行追加到 `%LOCALAPPDATA%\SysOverlayC\probe.log`。
//!
//! # 用途
//!
//! Net/GPU/Power 三个源曾经是纯桩（永远 0 / `--`），接上真实 API 后
//! 如果数值仍然不对，靠这份日志可以直接看到底层返回了什么：
//!
//! ```text
//! [0001756647012345] #000123 net row 3 type=71 oper=1 alias="WLAN" in=12345678 out=8765432 keep=1
//! ```
//!
//! # 零成本原则
//!
//! 开关关闭时 [`log`] 在第一次字符串拼接前就返回，不做任何格式化、
//! 不打开文件。采样频率本身 ≤1Hz（Net/GPU 1s、Power 3s），
//! 打开时的落盘开销可以忽略。

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

/// 是否启用：设置面板开关 **或** 环境变量 `SYSOVERLAY_PROBE=1`（二者取或）。
///
/// 环境变量作为兜底通道，方便无法进设置面板的场景（如沙箱/脚本）开启。
pub fn enabled() -> bool {
    DEBUG.load(std::sync::atomic::Ordering::Relaxed) || env_probe_on()
}

/// 环境变量开关（进程内只读一次，避免每次 log 都读环境）。
fn env_probe_on() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| {
        std::env::var("SYSOVERLAY_PROBE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

/// 行序号（诊断用，确认没有丢行）。
static SEQ: AtomicU64 = AtomicU64::new(0);

/// 追加一行探针日志（带毫秒时间戳 + 行号）。未启用时立即返回。
pub fn log(line: &str) {
    if !enabled() {
        return;
    }
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let full = format!("[{:013}] #{:06} {}\n", now_ms, n, line);
    write_raw(&full);
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

fn perf_flag() -> bool {
    std::env::var("SYSOVERLAY_PERF")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn write_raw(s: &str) {
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        let dir = std::path::PathBuf::from(dir).join("SysOverlayC");
        let _ = std::fs::create_dir_all(&dir);
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("probe.log"))
        {
            let _ = f.write_all(s.as_bytes());
        }
    }
}
