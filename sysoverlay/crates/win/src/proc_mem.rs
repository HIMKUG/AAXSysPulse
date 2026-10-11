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

            let total_gb =
                mem_status.ullTotalPhys as f64 / (1024.0 * 1024.0 * 1024.0);
            let avail_gb =
                mem_status.ullAvailPhys as f64 / (1024.0 * 1024.0 * 1024.0);
            let used_gb = total_gb - avail_gb;
            let load_pct = if total_gb > 0.0 {
                ((used_gb / total_gb) * 100.0) as f32
            } else {
                0.0
            };

            out.memory = MemoryStat {
                load_pct: load_pct.min(100.0),
                used_gb: used_gb as f32,
                total_gb: total_gb as f32,
            };
            out.timestamp_ms = now_ms;
            SampleOutcome::Ok
        }
    }
}
