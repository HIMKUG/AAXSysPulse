//! CPU sampler using GetSystemTimes API.

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::GetSystemTimes;
use aaxsyspulse_core::metrics::cpu_usage;
use aaxsyspulse_core::sampler::{SampleOutcome, Sampler, SamplerId};
use aaxsyspulse_core::snapshot::Snapshot;

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
                // 审查 562：计数器回拨（休眠恢复/系统时间调整）时任一分量倒退，
                // saturating 会把 delta 静默归零、显示假 0% —— 显式判回拨，
                // 置 Unavailable 并以当前值为新基线，本帧按瞬时失败处理。
                if idle_t < prev_idle || kernel_t < prev_kernel || user_t < prev_user {
                    self.prev = Some((idle_t, kernel_t, user_t));
                    out.cpu = aaxsyspulse_core::snapshot::Metric::Unavailable;
                    out.timestamp_ms = now_ms;
                    return SampleOutcome::Transient;
                }
                let idle_delta = idle_t.saturating_sub(prev_idle);
                let kernel_delta = kernel_t.saturating_sub(prev_kernel);
                let user_delta = user_t.saturating_sub(prev_user);

                out.cpu = cpu_usage(idle_delta, kernel_delta, user_delta);
                out.timestamp_ms = now_ms;
            } else {
                // 审查 058/563：首帧只建基线 —— 显式置 Unavailable 并返回
                // Transient，不再把"没有数据"的帧伪装成 Ok（诊断/退避据此分类）。
                out.cpu = aaxsyspulse_core::snapshot::Metric::Unavailable;
                out.timestamp_ms = now_ms;
                return SampleOutcome::Transient;
            }

            self.prev = Some((idle_t, kernel_t, user_t));
            SampleOutcome::Ok
        }
    }
}
