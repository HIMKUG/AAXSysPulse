//! 自适应刷新调度器（v2 EMA 核心）。
//!
//! # 核心变化
//!
//! v2 用 EMA 平滑吸收微振荡，死区迟滞防止抖动，Idle/Active 模式自适应提交节奏。
//! 不再依赖 v1 的成本反馈降档/升档机制（该机制在 v2 下与 EMA 冲突）。
//!
//! # 设计
//!
//! 1. **分频调度**：每个源独立记 `last_sample`，到期才采；
//!    未到期源在 [`SamplePlan`] 里为 false，采样器直接沿用上一次的值。
//! 2. **EMA 平滑**：`update_ema()` 用 `EMA_ALPHA = 0.3` 对原始值做指数移动平均，
//!    吸收微振荡后再参与变化判定。
//! 3. **Auto 档刷新上限**：取启用源中的最大最小有效间隔。

use crate::cadence::{EMA_ALPHA, TICK_MS};

/// 调度器覆盖的采样源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Source {
    Cpu = 0,
    Mem = 1,
    Gpu = 2,
    Net = 3,
}

pub const SRC_COUNT: usize = 4;

impl Source {
    pub const ALL: [Source; SRC_COUNT] = [Source::Cpu, Source::Mem, Source::Gpu, Source::Net];

    pub const fn bit(self) -> u32 {
        1 << (self as u32)
    }

    pub const fn idx(self) -> usize {
        self as usize
    }

    pub const fn min_effective_ms(self) -> u32 {
        match self {
            Source::Cpu | Source::Mem => TICK_MS as u32,
            Source::Gpu => 2_000,
            Source::Net => crate::cadence::REFRESH_FLOOR_MS as u32,
        }
    }
}

pub const MIN_EFFECTIVE_MS: [u32; SRC_COUNT] = [
    TICK_MS as u32,
    TICK_MS as u32,
    2_000,
    crate::cadence::REFRESH_FLOOR_MS as u32,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SamplePlan {
    pub cpu: bool,
    pub mem: bool,
    pub gpu: bool,
    pub net: bool,
}

impl SamplePlan {
    pub fn any(&self) -> bool {
        self.cpu || self.mem || self.gpu || self.net
    }

    pub fn get(&self, src: Source) -> bool {
        match src {
            Source::Cpu => self.cpu,
            Source::Mem => self.mem,
            Source::Gpu => self.gpu,
            Source::Net => self.net,
        }
    }

    fn set(&mut self, src: Source, on: bool) {
        match src {
            Source::Cpu => self.cpu = on,
            Source::Mem => self.mem = on,
            Source::Gpu => self.gpu = on,
            Source::Net => self.net = on,
        }
    }
}

pub type EnabledMask = [bool; SRC_COUNT];

const IDLE_INTERVAL_MS: u32 = 1_000;

#[derive(Debug)]
pub struct Scheduler {
    pub user_interval_ms: u32,
    pub auto: bool,
    pub last_sample_ms: [u64; SRC_COUNT],
    pub has_sampled: [bool; SRC_COUNT],
    pub ema_cpu: f32,
    pub ema_mem: f32,
    pub ema_gpu: f32,
    pub ema_net: f32,
    /// EMA 值上次提交时的快照（用于采样决策：EMA 无显著变化则跳过采样）
    ema_cpu_committed: f32,
    ema_mem_committed: f32,
    ema_gpu_committed: f32,
    ema_net_committed: f32,
}

impl Scheduler {
    pub fn new(user_interval_ms: u32, auto: bool) -> Self {
        Self {
            user_interval_ms: user_interval_ms.clamp(100, 5_000),
            auto,
            last_sample_ms: [0; SRC_COUNT],
            has_sampled: [false; SRC_COUNT],
            ema_cpu: 0.0,
            ema_mem: 0.0,
            ema_gpu: 0.0,
            ema_net: 0.0,
            ema_cpu_committed: 0.0,
            ema_mem_committed: 0.0,
            ema_gpu_committed: 0.0,
            ema_net_committed: 0.0,
        }
    }

    pub fn set_user(&mut self, user_interval_ms: u32, auto: bool) {
        self.user_interval_ms = user_interval_ms.clamp(100, 5_000);
        self.auto = auto;
    }

    pub fn base_interval_ms(&self, enabled: &EnabledMask) -> u32 {
        if self.auto {
            let mut max_eff = 0u32;
            for &src in Source::ALL.iter() {
                if enabled[src as usize] {
                    max_eff = max_eff.max(MIN_EFFECTIVE_MS[src as usize]);
                }
            }
            if max_eff == 0 {
                IDLE_INTERVAL_MS
            } else {
                max_eff
            }
        } else {
            self.user_interval_ms
        }
    }

    pub fn effective_interval_ms(&self, enabled: &EnabledMask) -> u32 {
        self.base_interval_ms(enabled)
    }

    pub fn source_interval_ms(&self, src: Source, enabled: &EnabledMask) -> u32 {
        self.effective_interval_ms(enabled).max(MIN_EFFECTIVE_MS[src as usize])
    }

    /// 采样 EMA 变化阈值（百分比）：EMA 与上次采样值的差值小于此值时跳过采样
    const SAMPLE_DELTA_THRESHOLD: f32 = 1.0;

    pub fn plan(&mut self, now_ms: u64, enabled: &EnabledMask) -> SamplePlan {
        let mut plan = SamplePlan::default();
        for &src in Source::ALL.iter() {
            let idx = src as usize;
            if !enabled[idx] {
                continue;
            }
            let interval = self.source_interval_ms(src, enabled) as u64;
            let time_due = !self.has_sampled[idx]
                || now_ms.saturating_sub(self.last_sample_ms[idx]) >= interval;

            // EMA 变化检测：即使时间到期，如果 EMA 变化小于阈值也跳过采样
            // 这样可以避免在数值稳定时频繁采样（节省 OS 系统调用开销）
            // 但首次采样（has_sampled[idx]=false）必须执行，否则 EMA 永远无法初始化
            let ema = self.ema(src);
            let ema_committed = self.ema_committed(src);
            let ema_delta = (ema - ema_committed).abs();
            let is_first_sample = !self.has_sampled[idx];
            let ema_significant = is_first_sample || ema_delta >= Self::SAMPLE_DELTA_THRESHOLD;

            if time_due && ema_significant {
                self.last_sample_ms[idx] = now_ms;
                self.has_sampled[idx] = true;
                self.set_ema_committed(src, ema);
                plan.set(src, true);
            }
        }
        plan
    }

    fn ema_committed(&self, src: Source) -> f32 {
        match src {
            Source::Cpu => self.ema_cpu_committed,
            Source::Mem => self.ema_mem_committed,
            Source::Gpu => self.ema_gpu_committed,
            Source::Net => self.ema_net_committed,
        }
    }

    fn set_ema_committed(&mut self, src: Source, value: f32) {
        match src {
            Source::Cpu => self.ema_cpu_committed = value,
            Source::Mem => self.ema_mem_committed = value,
            Source::Gpu => self.ema_gpu_committed = value,
            Source::Net => self.ema_net_committed = value,
        }
    }

    pub fn update_ema(&mut self, src: Source, raw: f32) {
        let ema = match src {
            Source::Cpu => &mut self.ema_cpu,
            Source::Mem => &mut self.ema_mem,
            Source::Gpu => &mut self.ema_gpu,
            Source::Net => &mut self.ema_net,
        };
        *ema = *ema + EMA_ALPHA * (raw - *ema);
    }

    /// 提交 EMA 值到 committed 状态（在采样成功后调用）
    pub fn commit_ema(&mut self, src: Source, value: f32) {
        self.update_ema(src, value);
        self.set_ema_committed(src, self.ema(src));
    }

    pub fn ema(&self, src: Source) -> f32 {
        match src {
            Source::Cpu => self.ema_cpu,
            Source::Mem => self.ema_mem,
            Source::Gpu => self.ema_gpu,
            Source::Net => self.ema_net,
        }
    }

    pub fn next_wake_ms(&self, now_ms: u64, enabled: &EnabledMask) -> u64 {
        let mut min_due = u64::MAX;
        for &src in Source::ALL.iter() {
            let idx = src as usize;
            if !enabled[idx] {
                continue;
            }
            let interval = self.source_interval_ms(src, enabled) as u64;
            let due = self.last_sample_ms[idx].saturating_add(interval);
            min_due = min_due.min(due);
        }
        if min_due == u64::MAX {
            now_ms.saturating_add(IDLE_INTERVAL_MS as u64)
        } else {
            min_due.max(now_ms)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_ON: EnabledMask = [true, true, true, true];

    #[test]
    fn test_ema_smoothing() {
        let mut s = Scheduler::new(100, true);
        s.update_ema(Source::Cpu, 10.0);
        assert!((s.ema(Source::Cpu) - 3.0).abs() < 0.1);
        s.update_ema(Source::Cpu, 50.0);
        let e = s.ema(Source::Cpu);
        assert!(e > 3.0 && e < 50.0);
    }

    #[test]
    fn test_scheduler_is_sendable() {
        fn assert_send<T: Send>() {}
        assert_send::<Scheduler>();
    }
}
