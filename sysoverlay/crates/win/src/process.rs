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
    /// **占总算力的百分比**（`0..=100`，与 Windows 任务管理器口径一致）。
    ///
    /// 单线程满载于 16 核机器 ≈ 6.25；全部核满载 = 100。
    /// 注意不是「单核归一」的 `0..=100×cores`。
    pub cpu_pct: f32,
    /// 私有内存（字节）。
    pub mem_bytes: u64,
    /// 功耗分摊（瓦）：整机 CPU 实测功率 × 该进程占总算力比例。
    ///
    /// `None` = **整机 CPU 实测不可用**（无管理员权限 / WinRing0 未加载 /
    /// 首帧尚未建立 MSR 基线），与「实测为 0W」是两回事，UI 必须分别呈现。
    pub power_w: Option<f32>,
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

// ── SYSTEM_PROCESS_INFORMATION 关键字段偏移（MSDN 文档布局，x64） ──
//
// ⚠️ 2026-09-09 修正：此前 `OFF_THREAD_COUNT=184` / `OFF_THREADS=192` 是**错的**，
// 实际读到的是 `PagefileUsage` / `PeakPagefileUsage`（见下方字段表 26/27 项），
// 后果是「线程数」取到页文件用量的低 32 位（多半被 min(1024) 钳到 1024），
// 线程数组基址指到页文件用量区，Σ(Kernel+User) 读的是垃圾值 →
// CPU 差分恒为天文数字 → cpu_pct 全部钳到 100% →
// 进程榜功耗列全部等于整机 CPU 功率（实测 31.0W 全行相同）。
//
// x64 字段布局（逐项累加，含对齐填充）：
//   0   NextEntryOffset(4)      4   NumberOfThreads(4)
//   8   WorkingSetPrivateSize   16  HardFaultCount(4)   20  ThreadsHighWatermark(4)
//   24  CycleTime               32  CreateTime          40  UserTime   48 KernelTime
//   56  ImageName.Length(u16)   58  MaximumLength(u16)  64  Buffer(ptr)
//   72  BasePriority(4)+pad     80  UniqueProcessId     88  InheritedFromUniqueProcessId
//   96  HandleCount(4)          100 SessionId(4)        104 UniqueProcessKey
//   112 PeakVirtualSize         120 VirtualSize         128 PageFaultCount(4)
//   136 PeakWorkingSetSize      144 WorkingSetSize  ← 当前工作集（此前误读 136 = 峰值）
//   152 QuotaPeakPagedPoolUsage 160 QuotaPagedPoolUsage
//   168 QuotaPeakNonPaged       176 QuotaNonPaged
//   184 PagefileUsage           192 PeakPagefileUsage   200 PrivatePageCount
//   208 ReadOperationCount      216 WriteOperationCount 224 OtherOperationCount
//   232 ReadTransferCount       240 WriteTransferCount  248 OtherTransferCount
//   256 Threads[1] ← SYSTEM_THREAD_INFORMATION 数组起点（0x100，结构体大小固定）
const OFF_NAME_LEN: usize = 56;
const OFF_NAME_BUF: usize = 64;
const OFF_PID: usize = 80;
const OFF_WS: usize = 144; // 当前工作集（字节）
const OFF_THREAD_COUNT: usize = 4;
const OFF_THREADS: usize = 256;
const THREAD_STRIDE: usize = 0x50;
const THREAD_KERNEL: usize = 0;
const THREAD_USER: usize = 8;
/// 单进程线程数上限（防越界；正常进程远小于此）。
const MAX_THREADS: usize = 4096;

/// 采样器：维护上一帧的 (pid → 累计 CPU 100ns) 映射算差分。
pub struct ProcCollector {
    /// 上一帧 (pid → 累计 CPU 100ns)，用于差分。
    prev: HashMap<u32, u64>,
    /// 复用的临时映射：每帧清空后填充，再与 `prev` 交换，避免每帧新建 HashMap。
    scratch: HashMap<u32, u64>,
    /// 复用的 Nt 查询缓冲：首帧分配 256KiB，之后只在系统进程表增长时扩容，
    /// 平时零分配（原实现每帧 `vec![0u8; 256KiB]` 重新分配+清零）。
    buf: Vec<u8>,
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
            scratch: HashMap::new(),
            buf: vec![0u8; 256 * 1024],
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
        let mut probe_dt_ms: u64 = 0;
        if self.ok {
            // 复用缓冲拉取原始表（零分配热路径），解析出本帧进程行。
            let rows = match self.query_raw() {
                Some(buf) => parse_processes(buf),
                None => {
                    self.ok = false; // 接口失败则停用，不再每帧打空转
                    Vec::new()
                }
            };
            if self.ok {
                // 首帧自检：偏移量一旦错位，这里立刻暴露（线程数/CPU 时间量级异常）。
                static CHECKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if !CHECKED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    crate::probe::log(&format!(
                        "proc_offset_check (expect: pid=4 System, threads<512, cpu=O(1e9..1e13)): {}",
                        rows.iter()
                            .take(3)
                            .map(|(pid, n, c, ws)| format!(
                                "[pid={pid} name={n} cpu100ns={c} ws={ws}]"
                            ))
                            .collect::<String>()
                    ));
                }
                let dt_ms = at.saturating_sub(self.prev_ms).max(1);
                let dt_hundreds = dt_ms as f64 * 1e4; // 1ms = 10^4 个 100ns
                // 复用 scratch 映射：清空后填充，再与 prev 交换，避免每帧新建 HashMap。
                self.scratch.clear();
                for (pid, name, cpu_100ns, mem_bytes) in rows {
                    let pct = if let Some(&p) = self.prev.get(&pid) {
                        let d = cpu_100ns.saturating_sub(p) as f64;
                        // 全核总时间归一：核数 × 100% 上限
                        (d / dt_hundreds * 100.0 / self.cores as f64) as f32
                    } else {
                        0.0
                    };
                    self.scratch.insert(pid, cpu_100ns);
                    if pid > 0 && !name.is_empty() {
                        // pct 已是「占总算力」口径（0..100），上限就是 100。
                        let pct_clamped = pct.clamp(0.0, 100.0);
                        // 功耗分摊：整机 CPU 实测功率（跨线程原子共享）× 占总算力比例。
                        // 无实测读数时为 None（UI 显示 "--"）——绝不能退化成 0，
                        // 否则所有进程都显示同一个值，看起来像"分摊失效"。
                        let power_w = crate::power_system::cpu_watts_global()
                            .map(|tw| sysoverlay_core::power::share_cpu_power(tw, pct_clamped));
                        entries.push(ProcEntry {
                            pid,
                            name,
                            cpu_pct: pct_clamped,
                            mem_bytes,
                            power_w,
                        });
                    }
                }
                std::mem::swap(&mut self.prev, &mut self.scratch);
                self.prev_ms = at;
                probe_dt_ms = dt_ms;
            }
        }
        // 按 CPU 降序（窗口直接取前 N）
        entries.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
        // 功耗分摊探针（排序后取前 6，才有诊断意义）：
        // 整机 CPU 实测瓦数 + 采样 dt + 核数 + 前几名的占比与分摊瓦数，
        // 直接回答「为什么每个进程瓦数都一样」。
        crate::probe::log_lazy(|| {
            let total_w = crate::power_system::cpu_watts_global();
            let mut top = String::new();
            for e in entries.iter().take(6) {
                top.push_str(&format!(
                    " {}={:.2}%:{}W",
                    e.name.trim_end_matches(".exe"),
                    e.cpu_pct,
                    e.power_w.map(|w| format!("{w:.2}")).unwrap_or_else(|| "--".into())
                ));
            }
            format!(
                "proc_power total_cpu_w={:?} dt_ms={} cores={} n={} top:{}",
                total_w,
                probe_dt_ms,
                self.cores,
                entries.len(),
                top
            )
        });
        ProcStats { entries, at_ms: at }
    }

    /// 拉取原始进程表缓冲（复用 `self.buf`，含扩容重试）。
    ///
    /// 首帧分配 256KiB，之后只在系统进程表增长（STATUS_INFO_LENGTH_MISMATCH）
    /// 时才扩容，平时零分配——相比每帧 `vec![0u8; 256KiB]` 省掉一次大块分配与清零。
    /// 返回的切片借自 `self.buf`，调用方读完后才能再次改 `self`。
    fn query_raw(&mut self) -> Option<&[u8]> {
        unsafe {
            let mut size: u32 = self.buf.len() as u32;
            if size == 0 {
                self.buf.resize(256 * 1024, 0);
                size = self.buf.len() as u32;
            }
            loop {
                let mut ret: u32 = 0;
                let st = NtQuerySystemInformation(
                    SYSTEM_PROCESS_INFORMATION,
                    self.buf.as_mut_ptr() as *mut core::ffi::c_void,
                    size,
                    &mut ret,
                );
                if st == 0 {
                    return Some(&self.buf[..size as usize]);
                }
                if st == STATUS_INFO_LENGTH_MISMATCH {
                    let need = (ret.max(1024) as usize).max(size as usize * 2);
                    self.buf.resize(need, 0);
                    size = need as u32;
                    continue;
                }
                return None;
            }
        }
    }
}

/// 测试/外部入口：用一次性收集器拉全量进程表（复用上面的方法）。
#[cfg(test)]
fn query_all() -> Option<Vec<(u32, String, u64, u64)>> {
    let mut c = ProcCollector::new();
    let buf = c.query_raw()?;
    Some(parse_processes(buf))
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
            let thread_count = (rd_u32(p, OFF_THREAD_COUNT) as usize).min(MAX_THREADS);
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