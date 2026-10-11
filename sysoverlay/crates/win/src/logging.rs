//! 统一日志出口：probe（诊断探针）、perf（性能统计）与 diag（降级事件）三个流
//! 共用的单一落盘模块。
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
//! - **开关语义不在本模块**：`AAXSYSPULSE_PROBE` / `AAXSYSPULSE_PERF` 的门控
//!   仍由 probe.rs / perf.rs 自行判断，本模块不做开关检查，只管落盘
//!   （panic hook 的「无条件写」也因此天然成立）。
//!
//! 时间戳取本地时间（`GetLocalTime`），对齐任务栏时钟，便于人工比对现场。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// 日志流：probe = 诊断探针（AAXSYSPULSE_PROBE / 设置面板「调试日志」），
/// perf = 性能统计（AAXSYSPULSE_PERF），
/// diag = **会话级降级事件**（不受任何开关门控，见 [`crate::probe::degrade`]）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Probe,
    Perf,
    Diag,
}

impl Stream {
    /// 行格式里的流名标签（grep 用）。
    fn tag(self) -> &'static str {
        match self {
            Stream::Probe => "probe",
            Stream::Perf => "perf",
            Stream::Diag => "diag",
        }
    }
}

/// 常驻文件句柄：打开一次复用，进程退出由 OS 兜底回收。
static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);
/// 会话文件名缓存：本会话内首次写入时确定，之后不再变。
static SESSION_NAME: OnceLock<String> = OnceLock::new();

/// 会话日志体积保险丝：单会话落盘总量封顶，超限后写入一次性标记行，
/// 之后本会话所有写入静默丢弃（重启/新会话自动恢复）。
/// 教训（2026-09-13）：渲染逐帧探针在开关失灵期间实测 ~1.2KB/s × 数小时；
/// 开关门控之外再上体积硬上限，任何未来的失控写入都被硬性截断。
const SESSION_CAP_BYTES: u64 = 8 * 1024 * 1024;
/// 本会话累计写入字节（纯内存计数，零系统调用）。
static SESSION_BYTES: AtomicU64 = AtomicU64::new(0);
/// 超限标记：置位后本会话所有写入直接丢弃。
static CAP_HIT: AtomicBool = AtomicBool::new(false);

/// 追加一段日志文本（单行或多行）到指定流。
///
/// 多行文本的每一行都会加 `[hh:mm:ss.mmm] [流名] ` 前缀，但整段合并为
/// 一次 `write_all`（perf 的 5s 聚合批量落盘设计因此得以保留）。
/// 打开文件失败时静默放弃——日志是尽力而为，绝不影响主流程。
pub fn write(stream: Stream, text: &str) {
    if CAP_HIT.load(Ordering::Relaxed) {
        return;
    }
    // +32：行前缀/时间戳余量。并发窗口内的少量超写可接受——保险丝只需
    // 保证「有界」，不做精确字节会计。
    let prev = SESSION_BYTES.fetch_add(text.len() as u64 + 32, Ordering::Relaxed);
    if prev >= SESSION_CAP_BYTES {
        // 首个越界者写一次性标记行（直通 write_inner 绕过自身检查），随后封笔。
        if !CAP_HIT.swap(true, Ordering::Relaxed) {
            write_inner(
                stream,
                &format!(
                    "── session log cap {}MB reached; further lines dropped until restart ──",
                    SESSION_CAP_BYTES / (1024 * 1024)
                ),
            );
        }
        return;
    }
    write_inner(stream, text);
}

/// 实际落盘（常驻句柄 + write_all + flush），无体积检查。
fn write_inner(stream: Stream, text: &str) {
    // 前缀与内容在无锁阶段拼好，持锁时间只剩 write_all + flush。
    let prefix = format!("[{}] [{}] ", local_ts(), stream.tag());
    let body = prefixed(&prefix, text);

    let mut guard = FILE.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_none() {
        let Some(dir) = std::env::var_os("LOCALAPPDATA") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir).join(aaxsyspulse_core::contract::APP_DATA_DIR_NAME);
        let _ = std::fs::create_dir_all(&dir);
        let name = session_name();
        // 首次打开时清理旧会话（7.3.2）。放在这里而不是启动路径上：
        // 只有真的会写日志才需要清理，且本进程内天然只跑一次。
        prune_dir(&dir, &name);
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

// ── 跨会话保留策略（审查项 7.3.2）────────────────────────────────
//
// 本模块原先**只有会话内保险丝**（`SESSION_CAP_BYTES`），没有跨会话清理：
// 文件名带 `YYYYMMDD_HHMM` ⇒ 每次启动新建一个、永不删除旧的。天天开关机
// 一年就是 365 × ≤8 MB（最坏 ≈ 2.9 GB）无上限增长，且没有任何入口能清掉。
//
// 策略（与引导侧同一口径，写进 docs/RUNTIME_ISSUES 的"日志"一节）：
// · 保留 14 天；
// · 且目录内本前缀文件总量 ≤ 64 MB（超了从最旧开始删）；
// · **本会话正在写的文件永不删**（否则会把当前日志截掉）；
// · 清理失败一律静默 —— 日志是尽力而为，绝不阻塞启动。

const LOG_PREFIX: &str = "aaxsyspulse_";
const KEEP_DAYS: u64 = 14;
const KEEP_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// 纯决策函数（可单测，不碰文件系统）：
/// 入参 = `(文件名, mtime 秒, 字节数)` 列表 + 当前会话文件名；
/// 返回应当删除的文件名。时间桶相同时按名字序删，保证结果确定。
fn prune_plan(entries: &[(String, u64, u64)], now_secs: u64, current: &str) -> Vec<String> {
    let cutoff = now_secs.saturating_sub(KEEP_DAYS * 86_400);
    let mut doomed: Vec<String> = Vec::new();
    let mut kept: Vec<&(String, u64, u64)> = Vec::new();
    for e in entries {
        if e.0 == current {
            kept.push(e);
        } else if e.1 < cutoff {
            doomed.push(e.0.clone());
        } else {
            kept.push(e);
        }
    }
    let mut total: u64 = kept.iter().map(|e| e.2).sum();
    if total > KEEP_TOTAL_BYTES {
        kept.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
        for e in kept {
            if total <= KEEP_TOTAL_BYTES {
                break;
            }
            if e.0 == current {
                continue; // 本会话文件豁免：宁可不达标也不截断正在写的日志
            }
            total = total.saturating_sub(e.2);
            doomed.push(e.0.clone());
        }
    }
    doomed
}

/// 按上面的策略清理日志目录。只在**首次落盘打开文件时**调用一次。
fn prune_dir(dir: &std::path::Path, current: &str) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut entries: Vec<(String, u64, u64)> = Vec::new();
    for entry in rd.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if !name.starts_with(LOG_PREFIX) || !name.ends_with(".log") {
            continue; // 历史 probe_*.log / perf.log 不动（见模块头）
        }
        let Ok(md) = entry.metadata() else { continue };
        if !md.is_file() {
            continue;
        }
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        entries.push((name, mtime, md.len()));
    }
    for name in prune_plan(&entries, now, current) {
        let _ = std::fs::remove_file(dir.join(&name));
    }
}

#[cfg(test)]
mod tests {
    use super::{prune_plan, prefixed};

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

    fn e(name: &str, age_days: u64, mb: u64) -> (String, u64, u64) {
        const DAY: u64 = 86_400;
        let now = 1_700_000_000;
        (
            name.to_string(),
            now - age_days * DAY,
            mb * 1024 * 1024,
        )
    }

    #[test]
    fn prune_drops_only_files_over_14_days() {
        let now = 1_700_000_000;
        let list = vec![e("aaxsyspulse_old.log", 15, 1), e("aaxsyspulse_new.log", 3, 1)];
        let doomed = prune_plan(&list, now, "aaxsyspulse_cur.log");
        assert_eq!(doomed, vec!["aaxsyspulse_old.log".to_string()]);
    }

    #[test]
    fn prune_cuts_oldest_first_until_under_total_cap() {
        // 5 个各 20 MB（共 100 MB > 64 MB），全部还在 14 天内；**i 越大越新**。
        let now = 1_700_000_000;
        let list: Vec<(String, u64, u64)> = (0..5)
            .map(|i| e(&format!("aaxsyspulse_{i}.log"), 5 - i as u64, 20))
            .collect();
        let current = "aaxsyspulse_4.log"; // 本会话文件 = 最新那个
        let doomed = prune_plan(&list, now, current);
        // 需删到 ≤64 MB ⇒ 从最旧的两个下手（i=0 满 5 天、i=1 满 4 天）。
        assert_eq!(
            doomed,
            vec!["aaxsyspulse_0.log".to_string(), "aaxsyspulse_1.log".to_string()]
        );
    }

    #[test]
    fn prune_never_deletes_the_session_file_being_written() {
        // 极端：唯一文件 = 当前会话文件且已超总量上限 ⇒ 豁免，一个都不删。
        let now = 1_700_000_000;
        let list = vec![e("aaxsyspulse_cur.log", 1, 100)];
        assert!(prune_plan(&list, now, "aaxsyspulse_cur.log").is_empty());
        // 超龄也不删：正在写的文件被截掉比留一天更糟。
        let old_cur = vec![(
            "aaxsyspulse_cur.log".to_string(),
            now - 400 * 86_400,
            1u64,
        )];
        assert!(prune_plan(&old_cur, now, "aaxsyspulse_cur.log").is_empty());
    }
}
