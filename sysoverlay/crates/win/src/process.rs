//! 进程级采样：`NtQuerySystemInformation(SystemProcessInformation)`（免提权）。
//!
//! # 一次拿全量 vs PDH 逐计数器
//!
//! PDH `\Process(*)` 需要每个进程建一个查询，进程频繁启停时维护复杂且慢。
//! Nt 接口一次返回整张进程表（CPU 时间 + 私有内存），一条 FFI 搞定 ——
//! 与 bottom 的 `sysinfo` 同思路（sysinfo 的 Windows 后端走同一 API）。
//!
//! # CPU% 语义
//!
//! Δ(内核+用户时间, 100ns) / Δ墙钟 / 核心数 × 100 —— 与任务管理器一致（全核归一）。

use std::collections::HashMap;
use std::ptr;

/// 单个进程一条采样行。
#[derive(Clone, Debug)]
pub struct ProcEntry {
    pub pid: u32,
    pub name: String,
    /// 全核归一后的占用率（0..=100×cores，见字段注释）。
    pub cpu_pct: f32,
    /// 私有内存（字节）。
    pub mem_bytes: u64,
    /// 功耗分摊（瓦）：整机 CPU 实测功率 × 该进程 CPU 占比（scaphandre 同思路）。
    pub power_w: f32,
}

/// 一帧进程快照（按 CPU 降序预排好）。
pub struct ProcStats {
    pub entries: Vec<ProcEntry>,
    /// 采样墙钟（GetTickCount64）。
    pub at_ms: u64,
}

// NtQuerySystemInformation（ntdll，仅 x64 布局偏移适用）。
#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(
        info_class: u32,
        buffer: *mut core::ffi::c_void,
        len: u32,
        ret: *mut u32,
    ) -> i32;
}

const SYSTEM_PROCESS_INFORMATION: u32 = 5;
/// STATUS_INFO_LENGTH_MISMATCH。
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;

// ── SYSTEM_PROCESS_INFORMATION 关键字段偏移（文档版布局，Win11 x64 实测） ──
//
// 实测（探测到 System pid=4、名称 "System"）确认此系统用 wdm 文档布局：
//   ImageName(UNICODE_STRING: len@56, buf@64) / pid@80 / WorkingSetSize@136
//   线程数组 @192，步长 0x50（SYSTEM_THREAD_INFORMATION），
//   进程级 kernel/user 时间 = Σ(线程的 KernelTime/UserTime)。
const OFF_NAME_LEN: usize = 56;
const OFF_NAME_BUF: usize = 64;
const OFF_PID: usize = 80;
const OFF_WS: usize = 136; // 工作集（字节）
const OFF_THREAD_COUNT: usize = 184;
const OFF_THREADS: usize = 192;
const THREAD_STRIDE: usize = 0x50;
const THREAD_KERNEL: usize = 0;
const THREAD_USER: usize = 8;

/// 采样器：维护上一帧的 (pid → 累计 CPU 100ns) 映射算差分。
pub struct ProcCollector {
    prev: HashMap<u32, u64>,
    prev_ms: u64,
    cores: u32,
    ok: bool,
}

impl Default for ProcCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcCollector {
    pub fn new() -> Self {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1)
            .max(1);
        Self {
            prev: HashMap::new(),
            prev_ms: 0,
            cores,
            ok: true,
        }
    }

    fn now_ms() -> u64 {
        unsafe {
            use windows::Win32::System::SystemInformation::GetTickCount64;
            GetTickCount64() as u64
        }
    }

    /// 采一帧（全进程）。首帧只有基线（cpu_pct=0）。失败返回空。
    pub fn sample(&mut self) -> ProcStats {
        let at = Self::now_ms();
        let mut entries = Vec::new();
        if self.ok {
            if let Some(rows) = query_all() {
                let mut cur: HashMap<u32, u64> = HashMap::with_capacity(rows.len());
                let dt_ms = at.saturating_sub(self.prev_ms).max(1);
                let dt_hundreds = dt_ms as f64 * 1e4; // 1ms = 10^4 个 100ns
                for (pid, name, cpu_100ns, mem_bytes) in rows {
                    let pct = if let Some(&p) = self.prev.get(&pid) {
                        let d = cpu_100ns.saturating_sub(p) as f64;
                        // 全核总时间归一：核数 × 100% 上限
                        (d / dt_hundreds * 100.0 / self.cores as f64) as f32
                    } else {
                        0.0
                    };
                    cur.insert(pid, cpu_100ns);
                    if pid > 0 && !name.is_empty() {
                        let pct_clamped = pct.clamp(0.0, 100.0 * self.cores as f32);
                        // 功耗分摊：整机 CPU 实测功率（跨线程原子共享）× CPU 占比
                        let power_w = match crate::power_system::cpu_watts_global() {
                            Some(total_w) => {
                                sysoverlay_core::power::share_cpu_power(total_w, pct_clamped, self.cores)
                            }
                            None => 0.0,
                        };
                        entries.push(ProcEntry {
                            pid,
                            name,
                            cpu_pct: pct_clamped,
                            mem_bytes,
                            power_w,
                        });
                    }
                }
                self.prev = cur;
                self.prev_ms = at;
            } else {
                self.ok = false; // 连续失败则停用，不再每帧打空转
            }
        }
        // 按 CPU 降序（窗口直接取前 N）
        entries.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
        ProcStats { entries, at_ms: at }
    }
}

/// 拉取原始进程表缓冲（含扩容重试）。
fn query_raw() -> Option<Vec<u8>> {
    unsafe {
        let mut size: u32 = 256 * 1024;
        let mut buf: Vec<u8> = vec![0u8; size as usize];
        loop {
            let mut ret: u32 = 0;
            let st = NtQuerySystemInformation(
                SYSTEM_PROCESS_INFORMATION,
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                size,
                &mut ret,
            );
            if st == 0 {
                return Some(buf);
            }
            if st == STATUS_INFO_LENGTH_MISMATCH {
                let need = (ret.max(1024) as usize).max(size as usize * 2);
                buf = vec![0u8; need];
                size = need as u32;
                continue;
            }
            return None;
        }
    }
}

/// 解析整张进程表。
fn query_all() -> Option<Vec<(u32, String, u64, u64)>> {
    Some(parse_processes(&query_raw()?))
}

unsafe fn rd_u16(p: *const u8, off: usize) -> u16 {
    ptr::read_unaligned(p.add(off) as *const u16)
}

unsafe fn rd_u32(p: *const u8, off: usize) -> u32 {
    ptr::read_unaligned(p.add(off) as *const u32)
}

unsafe fn rd_u64(p: *const u8, off: usize) -> u64 {
    ptr::read_unaligned(p.add(off) as *const u64)
}

unsafe fn rd_i64(p: *const u8, off: usize) -> i64 {
    ptr::read_unaligned(p.add(off) as *const i64)
}

/// 把缓冲解析成 (pid, 名称, 累计CPU时间100ns, 工作集字节)。
///
/// **名称安全性**：`ImageName.Buffer` 指向**响应缓冲内部**（内核一次性分配
/// 整份表格含字符串）。指针解引用前必须校验落在 `[buf起点, buf终点]` 内，
/// 否则按无名称处理（偏移漂移时不崩）。
fn parse_processes(buf: &[u8]) -> Vec<(u32, String, u64, u64)> {
    unsafe {
        let base = buf.as_ptr();
        let total = buf.len();
        let base_lo = base as usize;
        let base_hi = base_lo + total;
        let mut out = Vec::new();
        let mut off: usize = 0;
        loop {
            if off + OFF_THREADS > total {
                break;
            }
            let p = base.add(off);
            let next = rd_u32(p, 0) as usize;
            let pid = rd_u64(p, OFF_PID) as u32;
            let ws = rd_u64(p, OFF_WS);
            // 进程级 CPU 时间 = Σ 线程（KernelTime+UserTime）
            let thread_count = (rd_u32(p, OFF_THREAD_COUNT) as usize).min(1024);
            let mut cpu_100ns: u64 = 0;
            for t in 0..thread_count {
                let t_off = OFF_THREADS + t * THREAD_STRIDE;
                if off + t_off + THREAD_USER + 8 > total {
                    break;
                }
                let tp = base.add(off + t_off);
                let k = rd_i64(tp, THREAD_KERNEL).max(0) as u64;
                let u = rd_i64(tp, THREAD_USER).max(0) as u64;
                cpu_100ns = cpu_100ns.saturating_add(k).saturating_add(u);
            }
            let name_len = rd_u16(p, OFF_NAME_LEN) as usize;
            let name_ptr = rd_u64(p, OFF_NAME_BUF) as usize;
            let name = if name_len > 0
                && name_len <= 1024
                && name_ptr >= base_lo
                && name_ptr + name_len <= base_hi
            {
                let slice = std::slice::from_raw_parts(name_ptr as *const u16, name_len / 2);
                String::from_utf16_lossy(slice)
            } else {
                String::new()
            };
            out.push((pid, name, cpu_100ns, ws));
            if next == 0 {
                break;
            }
            off += next;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机冒烟：Nt 接口必须能取到进程表（含系统进程与名称）。
    #[test]
    fn query_all_returns_real_process_table() {
        let Some(rows) = query_all() else {
            return; // 仅系统接口异常时为空，不强制失败
        };
        assert!(!rows.is_empty(), "Windows 上进程表不应为空");
        assert!(
            rows.iter().any(|(_, name, _, _)| !name.is_empty()),
            "至少应有一个带名称的进程"
        );
        assert!(
            rows.iter().any(|(pid, name, _, _)| *pid == 4 || name == "System"),
            "应包含 System(pid=4)"
        );
    }

    /// 两次采样之间 CPU% 应为有限值：[0, 核数×100]。
    #[test]
    fn collector_cpu_pct_bounded() {
        let mut c = ProcCollector::new();
        let a = c.sample();
        let b = c.sample();
        // 首帧是基线：有条目但全部 CPU%=0
        assert!(!a.entries.is_empty(), "系统上进程表不应为空");
        assert!(a.entries.iter().all(|e| e.cpu_pct == 0.0), "首帧 CPU% 应为 0（基线）");
        assert!(!b.entries.is_empty());
        for e in &b.entries {
            assert!(e.cpu_pct.is_finite(), "CPU% 必须有限: {}", e.cpu_pct);
            assert!((0.0..=100.0 * c.cores as f32).contains(&e.cpu_pct));
        }
    }
}