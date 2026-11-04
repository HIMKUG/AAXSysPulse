//! Memory sampler using GlobalMemoryStatusEx API.

use aaxsyspulse_core::sampler::{SampleOutcome, Sampler, SamplerId};
use aaxsyspulse_core::snapshot::{MemoryStat, Snapshot};

/// Memory sampler using GlobalMemoryStatusEx.
pub struct MemSampler;

impl MemSampler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MemSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler for MemSampler {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn id(&self) -> SamplerId {
        SamplerId::Memory
    }

    fn sample(
        &mut self,
        _prev: Option<&Snapshot>,
        now_ms: u64,
        out: &mut Snapshot,
    ) -> SampleOutcome {
        unsafe {
            let mut mem_status = windows::Win32::System::SystemInformation::MEMORYSTATUSEX {
                dwLength: std::mem::size_of::<
                    windows::Win32::System::SystemInformation::MEMORYSTATUSEX,
                >() as u32,
                ..Default::default()
            };

            if windows::Win32::System::SystemInformation::GlobalMemoryStatusEx(&mut mem_status).is_err() {
                return SampleOutcome::Transient;
            }

            // 审查 059/564：total == 0（API 异常/受限 VM）时不写 0.0/0.0G 假值，
            // 保持 Unavailable；API 失败路径由上层诊断环记 Transient。
            if mem_status.ullTotalPhys == 0 {
                out.memory = MemoryStat::default();
                out.timestamp_ms = now_ms;
                return SampleOutcome::Transient;
            }

            let total_gb =
                mem_status.ullTotalPhys as f64 / (1024.0 * 1024.0 * 1024.0);
            let avail_gb =
                mem_status.ullAvailPhys as f64 / (1024.0 * 1024.0 * 1024.0);
            // 审查 059：avail > total（异常/VM）时 used 为负 —— 双向钳制，
            // 不再只 min(100) 留下负百分比/负 GB。
            let used_gb = (total_gb - avail_gb).max(0.0);
            let load_pct = if total_gb > 0.0 {
                ((used_gb / total_gb) * 100.0).clamp(0.0, 100.0) as f32
            } else {
                0.0
            };

            out.memory = MemoryStat {
                load_pct,
                used_gb: used_gb as f32,
                total_gb: total_gb as f32,
            };
            out.timestamp_ms = now_ms;
            SampleOutcome::Ok
        }
    }
}
