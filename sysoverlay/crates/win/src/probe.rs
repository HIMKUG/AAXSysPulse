//! 诊断探针（P-DIAG）：`SYSOVERLAY_PROBE=1` 时把各采样器的**原始 API 数据**
//! 逐行追加到 `%LOCALAPPDATA%\SysOverlayC\probe_<YYYYMMDD>_<HHMM>.log`。
//!
//! 日志按 **10 分钟时间窗滚动**：文件名带当前 10 分钟窗起点（UTC），窗口切换后
//! 自动新建文件，不再把全部运行日志堆进同一个大文件——单文件不会再无限膨胀。
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

/// 安装全局 panic hook：任何线程 panic 都把位置 + 消息写入当前 10 分钟窗的日志文件。
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
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        write_raw(&format!(
            "[{:013}] #{:06} PANIC thread={} loc={} msg={}\n",
            now_ms, n, name, loc, msg
        ));
        default(info);
    }));
}

fn perf_flag() -> bool {
    std::env::var("SYSOVERLAY_PERF")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// 滚动写盘：每 10 分钟落到独立文件，避免单文件无限膨胀。
///
/// 文件名带「当前 10 分钟时间窗」前缀（UTC，与日志行内毫秒时间戳同源），
/// 形如 `probe_20260912_1330.log`。调用方无需持有句柄——每次按时间窗算出文件名
/// 后追加即可，跨进程重启/跨午夜/跨整点都能正确新建文件。
fn write_raw(s: &str) {
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        let dir = std::path::PathBuf::from(dir).join("SysOverlayC");
        let _ = std::fs::create_dir_all(&dir);
        // 当前 10 分钟时间窗起点（ms）。
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let win = now_ms / (10 * 60_000);
        let win_start_ms = win * 10 * 60_000;
        let name = windowed_name(win_start_ms);
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(name))
        {
            let _ = f.write_all(s.as_bytes());
        }
    }
}

/// 把 Unix 毫秒转成 `probe_YYYYMMDD_HHMM.log` 文件名（UTC，无外部依赖）。
fn windowed_name(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs / 86400; // 1970-01-01 起的日数（epoch 之后恒为正）
    let rem = secs.rem_euclid(86400);
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    // days -> Y/M/D（Howard Hinnant 的 civil_from_days，整数截断向零）
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("probe_{:04}{:02}{:02}_{:02}{:02}.log", y, m, d, hh, mm)
}

#[cfg(test)]
mod tests {
    use super::windowed_name;

    #[test]
    fn epoch_maps_to_19700101_0000() {
        assert_eq!(windowed_name(0), "probe_19700101_0000.log");
    }

    #[test]
    fn ten_minute_floor_keeps_window_start() {
        // 模拟 write_raw 的 10 分钟取整：文件名取当前窗起点（分钟为 10 的倍数）。
        let floor10 = |ms: u64| {
            let w = ms / (10 * 60_000);
            w * 10 * 60_000
        };
        // 2026-09-12 13:27:35 UTC 落在 13:20 窗内 → 文件名取窗起点 1320。
        let secs = 1_789_219_655; // 2026-09-12 13:27:35 UTC
        assert_eq!(
            windowed_name(floor10(secs * 1000)),
            "probe_20260912_1320.log"
        );
        // 同一窗内再晚一点（仍 ≤13:29:59）仍落在 1320 窗。
        assert_eq!(
            windowed_name(floor10((secs + 2 * 60) * 1000)),
            "probe_20260912_1320.log"
        );
        // 越过 13:30 边界（13:27:35 + 3min = 13:30:35）→ 进入 1330 窗。
        assert_eq!(
            windowed_name(floor10((secs + 3 * 60) * 1000)),
            "probe_20260912_1330.log"
        );
    }
}
