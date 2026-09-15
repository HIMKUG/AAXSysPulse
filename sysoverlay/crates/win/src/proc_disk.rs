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

use windows::Win32::System::Performance::*;
use aaxsyspulse_core::snapshot::Metric;

/// PDH 查询的磁盘读写计数器状态。
/// 所有句柄在 windows 0.58 中均为原始 isize。
pub struct DiskState {
    /// PDH 查询句柄（isize）。
    query: isize,
    /// 读计数器句柄。
    read_counter: isize,
    /// 写计数器句柄。
    write_counter: isize,
    /// 是否已建立基线（首次 PdhCollectQueryData 调用）。
    baselined: bool,
    /// 初始是否成功（查询打开失败时句柄为 0）。
    valid: bool,
    /// 上次采样时间戳（毫秒，GetTickCount64 单位），用于 1Hz 节流。
    last_sample_ms: u64,
    /// 上次读/写速率（字节/秒）；`None` = 该计数器无效或无数据。
    ///
    /// ⚠️ 用 `Option` 而不是 `f64` + `> 0.0` 判断，是为了区分「**有效的** 0 B/s
    /// （磁盘空闲）」与「读不到数」——旧实现把空闲磁盘显示成 `--`（2026-09-15 审计）。
    last_read: Option<f64>,
    last_write: Option<f64>,
}

impl Default for DiskState {
    fn default() -> Self {
        Self {
            query: 0,
            read_counter: 0,
            write_counter: 0,
            baselined: false,
            valid: false,
            last_sample_ms: 0,
            last_read: None,
            last_write: None,
        }
    }
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

            // ⚠️ 计数器路径是**英文**名，必须用与系统语言无关的 `PdhAddEnglishCounterW`。
            // 性能计数器对象名会被本地化（中文系统上是「逻辑磁盘」而非 PhysicalDisk），
            // 用 locale 相关的 `PdhAddCounterW` 查英文名必然失败 → 中文系统磁盘行永久 `--`
            // （2026-09-15 审计）。先试英文 API（Vista+ 的 pdh.dll 均提供），失败再回退
            // 本地 API，保证老系统/异常环境行为不退化。
            let mut rc: isize = 0;
            let read_path = windows::core::w!("\\\\PhysicalDisk(_Total)\\\\Disk Read Bytes/sec");
            let mut ret_r = PdhAddEnglishCounterW(query, read_path, 0, &mut rc);
            if ret_r != 0 {
                ret_r = PdhAddCounterW(query, read_path, 0, &mut rc);
            }
            if ret_r != 0 {
                crate::probe::log("disk: add read counter failed (english + locale fallback)");
            } else {
                crate::probe::log("disk: read counter added");
            }

            let mut wc: isize = 0;
            let write_path = windows::core::w!("\\\\PhysicalDisk(_Total)\\\\Disk Write Bytes/sec");
            let mut ret_w = PdhAddEnglishCounterW(query, write_path, 0, &mut wc);
            if ret_w != 0 {
                ret_w = PdhAddCounterW(query, write_path, 0, &mut wc);
            }
            if ret_w != 0 {
                crate::probe::log("disk: add write counter failed (english + locale fallback)");
            } else {
                crate::probe::log("disk: write counter added");
            }

            // 立即建立基线（第一次收集）
            let _ = PdhCollectQueryData(query);

            crate::probe::log("disk: PDH query initialized");
            return Self {
                query,
                read_counter: rc,
                write_counter: wc,
                baselined: false,
                valid: true,
                last_sample_ms: 0,
                last_read: None,
                last_write: None,
            };
        }
    }

    /// 执行一次采样（按 1Hz 节流）。
    ///
    /// - 首次调用只建立基线，返回 Unavailable；第二次及之后返回实际速率（字节/秒）。
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
            // 收集数据
            let ret = PdhCollectQueryData(self.query);
            if ret != 0 {
                crate::probe::log("disk: PdhCollectQueryData failed");
                return (Metric::Unavailable, Metric::Unavailable);
            }

            if !self.baselined {
                // 首次调用：基线已建立，下次才是第一帧有效数据
                self.baselined = true;
                return (Metric::Unavailable, Metric::Unavailable);
            }

            // 读取格式化值（PDH_FMT_DOUBLE）
            let read_val = self.read_counter_value();
            let write_val = self.write_counter_value();

            self.last_read = read_val;
            self.last_write = write_val;
            self.last_sample_ms = now_ms;

            (
                self.last_read.map_or(Metric::Unavailable, Metric::Value),
                self.last_write.map_or(Metric::Unavailable, Metric::Value),
            )
        }
    }

    /// 读取格式化后的读速率；`None` = 计数器未注册 / 调用失败 / `CStatus` 表明数据无效。
    unsafe fn read_counter_value(&self) -> Option<f64> {
        if self.read_counter == 0 {
            return None;
        }
        let mut buf: PDH_FMT_COUNTERVALUE = std::mem::zeroed();
        let ret = PdhGetFormattedCounterValue(
            self.read_counter,
            PDH_FMT_DOUBLE,
            None,
            &mut buf,
        );
        if ret != 0 || !pdh_status_is_valid(buf.CStatus) {
            return None;
        }
        // PDH_FMT_COUNTERVALUE::Anonymous 是 union，PDH_FMT_DOUBLE 格式下用 doubleValue
        Some(buf.Anonymous.doubleValue)
    }

    /// 读取格式化后的写速率；语义同 [`Self::read_counter_value`]。
    unsafe fn write_counter_value(&self) -> Option<f64> {
        if self.write_counter == 0 {
            return None;
        }
        let mut buf: PDH_FMT_COUNTERVALUE = std::mem::zeroed();
        let ret = PdhGetFormattedCounterValue(
            self.write_counter,
            PDH_FMT_DOUBLE,
            None,
            &mut buf,
        );
        if ret != 0 || !pdh_status_is_valid(buf.CStatus) {
            return None;
        }
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
