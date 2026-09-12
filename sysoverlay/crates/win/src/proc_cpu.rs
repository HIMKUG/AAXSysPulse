//! CPU sampler using GetSystemTimes API.

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::GetSystemTimes;
use sysoverlay_core::metrics::cpu_usage;
use sysoverlay_core::sampler::{SampleOutcome, Sampler, SamplerId};
use sysoverlay_core::snapshot::Snapshot;

/// CPU sampler using GetSystemTimes API.
pub struct CpuSampler {
    /// Previous times for delta calculation: (idle, kernel, user) in 100ns units.
    prev: Option<(u64, u64, u64)>,
}

impl CpuSampler {
    pub fn new() -> Self {
        Self { prev: None }
    }

    /// 重置 GetSystemTimes 前值（CPU 行重新启用时调用）：
    /// 跨大间隔的陈旧 delta 会让首帧占用率飙到 100%，重置后首帧只建基线。
    pub fn reset_baseline(&mut self) {
        self.prev = None;
    }
}

impl Default for CpuSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler for CpuSampler {
    fn name(&self) -> &'static str {
        "cpu"
    }

    fn id(&self) -> SamplerId {
        SamplerId::Cpu
    }

    fn sample(
        &mut self,
        _prev: Option<&Snapshot>,
        now_ms: u64,
        out: &mut Snapshot,
    ) -> SampleOutcome {
        unsafe {
            let mut idle = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();

            if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_err() {
                return SampleOutcome::Transient;
            }

            let idle_t =
                (idle.dwLowDateTime as u64) | ((idle.dwHighDateTime as u64) << 32);
            let kernel_t =
                (kernel.dwLowDateTime as u64) | ((kernel.dwHighDateTime as u64) << 32);
            let user_t =
                (user.dwLowDateTime as u64) | ((user.dwHighDateTime as u64) << 32);

            if let Some((prev_idle, prev_kernel, prev_user)) = self.prev {
                let idle_delta = idle_t.saturating_sub(prev_idle);
                let kernel_delta = kernel_t.saturating_sub(prev_kernel);
                let user_delta = user_t.saturating_sub(prev_user);

                out.cpu = cpu_usage(idle_delta, kernel_delta, user_delta);
                out.timestamp_ms = now_ms;
            }

            self.prev = Some((idle_t, kernel_t, user_t));
            SampleOutcome::Ok
        }
    }
}
