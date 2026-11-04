//! Disk I/O sampler using PDH (Performance Data Helper).
//!
//! PDH 计数器必须调用两次 `PdhCollectQueryData` 才能得到有效值：
//! 第一次建立基线，第二次之后才计算出速率。所以本采样器内部维护状态机：
//! - 首次调用：只收集基线，返回 Unavailable
//! - 第二次及之后：返回实际字节/秒速率
//!
//! 计数路径：`\\PhysicalDisk(_Total)\\Disk Read Bytes/sec` 和
//! `\\PhysicalDisk(_Total)\\Disk Write Bytes/sec`（英文名，走
//! `PdhAddEnglishCounterW`，与系统语言无关）。

use windows::core::PCWSTR;
use windows::Win32::System::Performance::*;
use aaxsyspulse_core::snapshot::Metric;

/// PDH 计数器路径（**单反斜杠**开头的本地机器形式）。
///
/// ⚠️ 2026-09-17 实测定位：PDH 路径语法为
/// `\Object(Instance)\Counter`（本地）/ `\\Machine\Object(Instance)\Counter`（远端）。
/// 旧实现写成 `\\PhysicalDisk(_Total)\\Disk Read Bytes/sec`（双反斜杠）→ PDH 把
/// `PhysicalDisk(_Total)` 当成**机器名**：
/// - `PdhAddEnglishCounterW` 返回 `PDH_CSTATUS_NO_MACHINE`(0x800007D0)；
/// - 回退的 locale 版 `PdhAddCounterW` 反而返回 0（**假成功**，句柄非 0）；
/// - `PdhCollectQueryData` 永久返回 `PDH_NO_DATA`(0x800007D5) → `snap.disk.ready`
///   恒为 false → 磁盘读写行**永远不显示**（悬浮窗里连占位格都没有）。
///
/// 修一处路径即可让三条 API 全部恢复正常（单测 `disk_paths_are_local_form` 守门）。
pub const DISK_READ_PATH: &str = "\\PhysicalDisk(_Total)\\Disk Read Bytes/sec";
pub const DISK_WRITE_PATH: &str = "\\PhysicalDisk(_Total)\\Disk Write Bytes/sec";

/// `&str` → NUL 结尾 UTF-16 缓冲（PDH 要 `PCWSTR`，缓冲须存活到调用结束）。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// PDH 查询的磁盘读写计数器状态。
/// 所有句柄在 windows 0.58 中均为原始 isize。
#[derive(Default)]
pub struct DiskState {
    /// PDH 查询句柄（isize）。
    query: isize,
    /// 读计数器句柄。
    read_counter: isize,
    /// 写计数器句柄。
    write_counter: isize,
    /// 是否可用（查询/计数器至少注册成功一个；全失败 → 直接返回 Unavailable）。
    valid: bool,
    /// 上次采样时间戳（毫秒；采样时钟与 GetTickCount64 同源，由调度器传入，
    /// 审查 063：原注释误写"GetTickCount64 单位"），用于 1Hz 节流。
    ///
    /// ⚠️ 收集失败时也要推进：否则失败会绕过节流、每周期都重打 PDH 并刷日志
    /// （2026-09-17 实测：故障时 1s 内可打 3~4 条 "PdhCollectQueryData failed"）。
    last_sample_ms: u64,
    /// 连续无效轮数（审查 061：locale 版"假成功"（句柄非 0 却永不出数）
    /// 无法在 new() 识别 —— 出数前维持待验证态，连续多轮无效判 invalid）。
    invalid_streak: u32,
    /// 上次读/写速率（字节/秒）；`None` = 该计数器无效或无数据。
    ///
    /// ⚠️ 用 `Option` 而不是 `f64` + `> 0.0` 判断，是为了区分「**有效的** 0 B/s
    /// （磁盘空闲）」与「读不到数」——旧实现把空闲磁盘显示成 `--`（2026-09-15 审计）。
    last_read: Option<f64>,
    last_write: Option<f64>,
}

/// `PDH_FMT_COUNTERVALUE::CStatus` 是否表示「拿得到有效数据」。
///
/// `PDH_CSTATUS_VALID_DATA`(0) = 计数器值有效；`PDH_CSTATUS_NEW_DATA`(1) = 本次是新值。
/// 其余（`PDH_CSTATUS_NO_INSTANCE` / `PDH_CSTATUS_INVALID_DATA` / …）视作无数据。
fn pdh_status_is_valid(status: u32) -> bool {
    status == PDH_CSTATUS_VALID_DATA || status == PDH_CSTATUS_NEW_DATA
}

impl DiskState {
    /// 创建并打开 PDH 查询，注册磁盘读写计数器。
    pub fn new() -> Self {
        // 打开查询：szDataSource=None 表示实时性能计数器
        let mut query: isize = 0;
        unsafe {
            let ret = PdhOpenQueryW(None, 0, &mut query);
            if ret != 0 {
                crate::probe::log("disk: PdhOpenQueryW failed");
                return Self::default();
            }

            // 计数器路径见 [`DISK_READ_PATH`]：**必须单反斜杠**，否则 PDH 当机器名解析。
            // 优先英文 API（与系统语言无关）；失败再回退 locale API，但 locale 版可能
            // 「假成功」（返回 0 却永远不出数），所以下面用 valid 兜底判定。
            let read_buf = wide(DISK_READ_PATH);
            let write_buf = wide(DISK_WRITE_PATH);
            let read_p = PCWSTR::from_raw(read_buf.as_ptr());
            let write_p = PCWSTR::from_raw(write_buf.as_ptr());

            let mut rc: isize = 0;
            let en_r = PdhAddEnglishCounterW(query, read_p, 0, &mut rc);
            if en_r != 0 {
                // 审查 062：locale 回退已删除 —— 路径本身是英文字面量，
                // 传给期望本地化名的 PdhAddCounterW 在中文系统上必然失败，
                // 还会"假成功"占用句柄（见模块头 2026-09-17 实测）。
                crate::probe::log(&format!("disk: add read counter failed (0x{en_r:08X})"));
            } else {
                crate::probe::log("disk: read counter added");
            }

            let mut wc: isize = 0;
            let en_w = PdhAddEnglishCounterW(query, write_p, 0, &mut wc);
            if en_w != 0 {
                crate::probe::log(&format!("disk: add write counter failed (0x{en_w:08X})"));
            } else {
                crate::probe::log("disk: write counter added");
            }

            // 两个计数器都没注册成功 = 这个采样器没有意义：
            // 标 invalid 后 sample() 直接返回 Unavailable（渲染层显示 `--` 占位），
            // 不再每周期空转 PDH、也不再刷 "PdhCollectQueryData failed" 日志。
            let valid = rc != 0 || wc != 0;
            if !valid {
                crate::probe::log("disk: no usable counters -> sampler disabled");
            }

            // 立即建立基线（第一次收集）。审查 565：失败要留痕，
            // 不得无声吞掉 —— 待验证态由 sample() 的 invalid_streak 继续把关。
            let base = PdhCollectQueryData(query);
            if base != 0 {
                crate::probe::log(&format!("disk: baseline collect failed (0x{base:08X})"));
            }

            crate::probe::log("disk: PDH query initialized");
            Self {
                query,
                read_counter: rc,
                write_counter: wc,
                valid,
                last_sample_ms: 0,
                invalid_streak: 0,
                last_read: None,
                last_write: None,
            }
        }
    }

    /// 执行一次采样（按 1Hz 节流）。
    ///
    /// - 速率有效性由计数器自身的 `CStatus` 判定（`new()` 已收集过一次基线，
    ///   本函数再收一次即可出数；首帧若仍为 `PDH_CSTATUS_INVALID_DATA` 会返回
    ///   `Unavailable`，由渲染层显示 `--` 占位）。
    /// - 相邻两次调用若间隔 < 1s，返回上次值（节流，不重新查询 PDH）。
    ///
    /// 返回 `(read_bps, write_bps)`，单位为字节/秒（f64）。
    pub fn sample(&mut self, now_ms: u64) -> (Metric<f64>, Metric<f64>) {
        // 1Hz 节流：两次调用间隔 < 1000ms 时返回上次值
        const DISK_INTERVAL_MS: u64 = 1000;
        if self.last_sample_ms > 0 && now_ms.saturating_sub(self.last_sample_ms) < DISK_INTERVAL_MS {
            return (
                self.last_read.map_or(Metric::Unavailable, Metric::Value),
                self.last_write.map_or(Metric::Unavailable, Metric::Value),
            );
        }

        if !self.valid {
            return (Metric::Unavailable, Metric::Unavailable);
        }

        unsafe {
            // 收集失败也要推进节流时间戳（否则失败路径绕过节流 → 空转 + 刷日志）。
            self.last_sample_ms = now_ms;

            // 收集数据
            let ret = PdhCollectQueryData(self.query);
            if ret != 0 {
                // 带错误码：PDH_NO_DATA(0x800007D5)=计数器无实例/无数据，
                // PDH_CSTATUS_NO_MACHINE(0x800007D0)=路径被当成机器名（见 DISK_READ_PATH）。
                crate::probe::log(&format!("disk: PdhCollectQueryData failed (0x{ret:08X})"));
                // 审查 566：失败时清掉上次值 —— 否则下一帧命中 1Hz 节流会把
                // 上次成功值当新值返回（"读不到"被伪装成旧速率）。
                self.last_read = None;
                self.last_write = None;
                return (Metric::Unavailable, Metric::Unavailable);
            }

            // 读取格式化值（PDH_FMT_DOUBLE）；无效数据由 CStatus 过滤为 None。
            let read_val = self.counter_value(self.read_counter);
            let write_val = self.counter_value(self.write_counter);

            // 审查 061：双侧持续无数 → 判计数器假成功并禁用采样器。
            match (read_val, write_val) {
                (None, None) => {
                    self.invalid_streak += 1;
                    if self.invalid_streak >= 10 && self.valid {
                        self.valid = false;
                        crate::probe::log("disk: no valid data for 10 cycles -> sampler disabled");
                    }
                }
                _ => self.invalid_streak = 0,
            }
            self.last_read = read_val;
            self.last_write = write_val;

            (
                self.last_read.map_or(Metric::Unavailable, Metric::Value),
                self.last_write.map_or(Metric::Unavailable, Metric::Value),
            )
        }
    }

    /// 读取格式化后的计数器值；`None` = 句柄无效 / 调用失败 / `CStatus` 表明数据无效。
    ///
    /// 审查 567：读/写两个逐字重复的读取函数收敛为这一份。
    unsafe fn counter_value(&self, handle: isize) -> Option<f64> {
        if handle == 0 {
            return None;
        }
        let mut buf: PDH_FMT_COUNTERVALUE = std::mem::zeroed();
        let ret = PdhGetFormattedCounterValue(handle, PDH_FMT_DOUBLE, None, &mut buf);
        if ret != 0 || !pdh_status_is_valid(buf.CStatus) {
            return None;
        }
        // PDH_FMT_COUNTERVALUE::Anonymous 是 union，PDH_FMT_DOUBLE 格式下用 doubleValue
        Some(buf.Anonymous.doubleValue)
    }
}

impl Drop for DiskState {
    fn drop(&mut self) {
        if self.query != 0 {
            unsafe { let _ = PdhCloseQuery(self.query); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归守门：PDH 路径必须是**单反斜杠**的本地形式。
    ///
    /// 写成双反斜杠会被 PDH 当成机器名 → `PDH_CSTATUS_NO_MACHINE`，
    /// 而 locale 回退 API 会「假成功」，最终磁盘行永远无数据（2026-09-17 定位）。
    #[test]
    fn disk_paths_are_local_form() {
        for p in [DISK_READ_PATH, DISK_WRITE_PATH] {
            assert!(
                p.starts_with("\\PhysicalDisk(_Total)\\"),
                "PDH 路径必须是单反斜杠本地形式，实际为 {p:?}"
            );
            assert!(
                !p.starts_with("\\\\"),
                "双反斜杠会被 PDH 解析成机器名（PDH_CSTATUS_NO_MACHINE）：{p:?}"
            );
        }
        assert_eq!(DISK_READ_PATH, "\\PhysicalDisk(_Total)\\Disk Read Bytes/sec");
        assert_eq!(DISK_WRITE_PATH, "\\PhysicalDisk(_Total)\\Disk Write Bytes/sec");
    }

    /// `wide()` 产出的缓冲必须以 0 结尾（PCWSTR 约定），且内容不含额外字符。
    #[test]
    fn wide_is_nul_terminated() {
        let w = wide("ab");
        assert_eq!(w, vec![b'a' as u16, b'b' as u16, 0]);
    }

    /// 诊断用（默认忽略）：真实调用本机 PDH，打印全链路返回码。
    ///
    /// 运行：`cargo test -p aaxsyspulse-win --lib pdh_live_probe -- --ignored --nocapture`
    /// 期望（正常机器）：计数已注册、collect=0x00000000、格式化出非零速率。
    #[test]
    #[ignore]
    fn pdh_live_probe() {
        let mut d = DiskState::new();
        println!("valid={}", d.valid);
        for i in 0..4 {
            let (r, w) = d.sample(1_000 * (i as u64 + 1));
            println!("sample#{i} read={r:?} write={w:?}");
            std::thread::sleep(std::time::Duration::from_millis(1_100));
        }
        assert!(d.valid, "本机 PDH 磁盘计数器应可用");
    }
}
