//! CPU sampler using GetSystemTimes API.
//!
//! Collects CPU utilization by measuring idle, kernel, and user time deltas.

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::GetSystemTimes;
use sysoverlay_core::metrics::cpu_usage;
use sysoverlay_core::sampler::{SampleOutcome, Sampler};
use sysoverlay_core::snapshot::{Metric, Snapshot};

/// CPU sampler using GetSystemTimes API.
pub struct CpuSampler {
    /// Previous times for delta calculation: (idle, kernel, user) in 100ns units
    prev: Option<(u64, u64, u64)>,
    /// Current CPU usage to return
    cpu: Metric<f32>,
}

impl CpuSampler {
    /// Create a new CPU sampler.
    pub fn new() -> Self {
        Self {
            prev: None,
            cpu: Metric::Unavailable,
        }
    }

    /// Returns the current CPU usage metric.
    pub fn get_cpu(&self) -> Metric<f32> {
        self.cpu
    }
}

impl Default for CpuSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler for CpuSampler {
    fn name(&self) -> &'static str {
        "CPU"
    }

    fn sample(&mut self, _prev: Option<&Snapshot>, _now_ms: u64) -> SampleOutcome {
        unsafe {
            let mut idle = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();

            if !GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_ok() {
                return SampleOutcome::Transient("GetSystemTimes failed");
            }

            let idle_t = (idle.dwLowDateTime as u64) | ((idle.dwHighDateTime as u64) << 32);
            let kernel_t = (kernel.dwLowDateTime as u64) | ((kernel.dwHighDateTime as u64) << 32);
            let user_t = (user.dwLowDateTime as u64) | ((user.dwHighDateTime as u64) << 32);

            if let Some((prev_idle, prev_kernel, prev_user)) = self.prev {
                let idle_delta = idle_t.saturating_sub(prev_idle);
                let kernel_delta = kernel_t.saturating_sub(prev_kernel);
                let user_delta = user_t.saturating_sub(prev_user);

                self.cpu = cpu_usage(idle_delta, kernel_delta, user_delta);
            }

            self.prev = Some((idle_t, kernel_t, user_t));
            SampleOutcome::Ok
        }
    }
}
