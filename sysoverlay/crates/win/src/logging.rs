//! 统一日志出口：probe（诊断探针）与 perf（性能统计）两个流共用的单一落盘模块。
//!
//! # 设计意图
//!
//! 此前 [`crate::probe`] 与 [`crate::perf`] 各自做「每行 open/write/close」的
//! 文件 I/O，且写到两个不同的文件（`probe_*.log` / `perf.log`）。本模块把
//! 文件 I/O 收敛到一处，调用方（probe/perf）只负责各自的开关门控与行内容：
//!
//! - **单文件双流**：两个流都写进
//!   `%LOCALAPPDATA%\AAXSysPulseC\aaxsyspulse_YYYYMMDD_HHMM.log`
//!   （会话内**首次写入时**确定文件名，每会话一个文件；旧的 probe_*.log /
//!   perf.log 历史文件原样保留不动）。
//! - **统一行格式**：`[hh:mm:ss.mmm] [流名] 消息`，流名 = `probe` / `perf`，
//!   人类可读、可 grep。多行文本（如 perf 的 msgs 聚合行 + trail 续行）
//!   逐行加前缀，仍合并为**一次 write_all** 落盘。
//! - **常驻句柄**：`Mutex<Option<File>>` 打开一次复用，每行写入后 flush——
//!   消除旧实现的每行 open/close 开销。
//! - **开关语义不在本模块**：`SYSOVERLAY_PROBE` / `SYSOVERLAY_PERF` 的门控
//!   仍由 probe.rs / perf.rs 自行判断，本模块不做开关检查，只管落盘
//!   （panic hook 的「无条件写」也因此天然成立）。
//!
//! 时间戳取本地时间（`GetLocalTime`），对齐任务栏时钟，便于人工比对现场。

use std::sync::{Mutex, OnceLock};

/// 日志流：probe = 诊断探针（SYSOVERLAY_PROBE / 设置面板「调试日志」），
/// perf = 性能统计（SYSOVERLAY_PERF）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Probe,
    Perf,
}

impl Stream {
    /// 行格式里的流名标签（grep 用）。
    fn tag(self) -> &'static str {
        match self {
            Stream::Probe => "probe",
            Stream::Perf => "perf",
        }
    }
}

/// 常驻文件句柄：打开一次复用，进程退出由 OS 兜底回收。
static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);
/// 会话文件名缓存：本会话内首次写入时确定，之后不再变。
static SESSION_NAME: OnceLock<String> = OnceLock::new();

/// 追加一段日志文本（单行或多行）到指定流。
///
/// 多行文本的每一行都会加 `[hh:mm:ss.mmm] [流名] ` 前缀，但整段合并为
/// 一次 `write_all`（perf 的 5s 聚合批量落盘设计因此得以保留）。
/// 打开文件失败时静默放弃——日志是尽力而为，绝不影响主流程。
pub fn write(stream: Stream, text: &str) {
    // 前缀与内容在无锁阶段拼好，持锁时间只剩 write_all + flush。
    let prefix = format!("[{}] [{}] ", local_ts(), stream.tag());
    let body = prefixed(&prefix, text);

    let mut guard = FILE.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_none() {
        let Some(dir) = std::env::var_os("LOCALAPPDATA") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir).join("AAXSysPulseC");
        let _ = std::fs::create_dir_all(&dir);
        let name = session_name();
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(&name))
        {
            Ok(f) => *guard = Some(f),
            // 打不开（目录被占/权限问题）就静默放弃，后续也不断重试同一个坏路径。
            Err(_) => return,
        }
    }
    use std::io::Write;
    if let Some(f) = guard.as_mut() {
        if f.write_all(body.as_bytes()).is_ok() {
            let _ = f.flush();
        }
    }
}

/// 会话日志文件名：`aaxsyspulse_YYYYMMDD_HHMM.log`（本地时间，首次写入时刻）。
fn session_name() -> String {
    SESSION_NAME
        .get_or_init(|| {
            let st = local_systemtime();
            format!(
                "aaxsyspulse_{:04}{:02}{:02}_{:02}{:02}.log",
                st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute
            )
        })
        .clone()
}

/// 本地时间戳 `[hh:mm:ss.mmm]`（对齐任务栏时钟，便于人工比对现场）。
fn local_ts() -> String {
    let st = local_systemtime();
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
    )
}

fn local_systemtime() -> windows::Win32::Foundation::SYSTEMTIME {
    // windows 0.58：GetLocalTime() 无参、返回 SYSTEMTIME（Foundation 模块，同 gdi.rs）。
    unsafe { windows::Win32::System::SystemInformation::GetLocalTime() }
}

/// 给多行文本的每一行加前缀（纯函数，单测覆盖）。
///
/// 末尾统一保证一个换行；调用方传 `"a\nb\n"` 或 `"a\nb"` 结果一致。
fn prefixed(prefix: &str, text: &str) -> String {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut out = String::with_capacity(text.len() + prefix.len() * 4);
    for line in body.split('\n') {
        out.push_str(prefix);
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::prefixed;

    #[test]
    fn prefix_applies_to_every_line_of_multiline_text() {
        // perf 聚合批量落盘（聚合行 + section 段行）的场景：
        // 每行都要能被 `^\[..[..perf\] section` 单独 grep 到。
        let out = prefixed("[12:34:56.789] [perf] ", "frames=1 skipped=0\nsection seg us=2\n");
        assert_eq!(
            out,
            "[12:34:56.789] [perf] frames=1 skipped=0\n[12:34:56.789] [perf] section seg us=2\n"
        );
    }

    #[test]
    fn trailing_newline_is_normalized() {
        // 带/不带结尾换行的输入结果一致，且不会多出空前缀行。
        let a = prefixed("[p] ", "x\n");
        let b = prefixed("[p] ", "x");
        assert_eq!(a, "[p] x\n");
        assert_eq!(a, b);
    }
}
