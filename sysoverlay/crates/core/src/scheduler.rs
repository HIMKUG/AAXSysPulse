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
///
/// v3：Power 纳入统一分频（此前在 `SystemSampler` 里手写 `last_power_ms`）。
/// Power 的间隔与刷新率解耦（用户配置的 `power_interval_ms`），
/// 由 [`Scheduler::source_interval_ms`] 特判返回。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Source {
    Cpu = 0,
    Mem = 1,
    Gpu = 2,
    Net = 3,
    Power = 4,
}

pub const SRC_COUNT: usize = 5;

impl Source {
    pub const ALL: [Source; SRC_COUNT] = [
        Source::Cpu,
        Source::Mem,
        Source::Gpu,
        Source::Net,
        Source::Power,
    ];

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
            // Power 实际间隔由 power_interval_ms 决定，这里只是 base 参与木桶的下限。
            Source::Power => 500,
        }
    }
}

pub const MIN_EFFECTIVE_MS: [u32; SRC_COUNT] = [
    TICK_MS as u32,
    TICK_MS as u32,
    2_000,
    crate::cadence::REFRESH_FLOOR_MS as u32,
    500,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SamplePlan {
    pub cpu: bool,
    pub mem: bool,
    pub gpu: bool,
    pub net: bool,
    pub power: bool,
}

impl SamplePlan {
    pub fn any(&self) -> bool {
        self.cpu || self.mem || self.gpu || self.net || self.power
    }

    pub fn get(&self, src: Source) -> bool {
        match src {
            Source::Cpu => self.cpu,
            Source::Mem => self.mem,
            Source::Gpu => self.gpu,
            Source::Net => self.net,
            Source::Power => self.power,
        }
    }

    fn set(&mut self, src: Source, on: bool) {
        match src {
            Source::Cpu => self.cpu = on,
            Source::Mem => self.mem = on,
            Source::Gpu => self.gpu = on,
            Source::Net => self.net = on,
            Source::Power => self.power = on,
        }
    }
}

pub type EnabledMask = [bool; SRC_COUNT];

const IDLE_INTERVAL_MS: u32 = 1_000;

#[derive(Debug)]
pub struct Scheduler {
    pub user_interval_ms: u32,
    pub auto: bool,
    /// Power 源独立间隔（与刷新率解耦，用户配置 power_interval_ms，500..=60000）。
    pub power_interval_ms: u32,
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
            power_interval_ms: 3_000,
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

    /// 设置 Power 独立采样间隔（设置页改动即时生效）。
    pub fn set_power_interval(&mut self, ms: u32) {
        self.power_interval_ms = ms.clamp(500, 60_000);
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
        // Power 与刷新率解耦：固定用用户配置的独立间隔，不吃 base 木桶。
        if src == Source::Power {
            return self.power_interval_ms;
        }
        self.effective_interval_ms(enabled).max(MIN_EFFECTIVE_MS[src as usize])
    }

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

            if time_due {
                self.last_sample_ms[idx] = now_ms;
                self.has_sampled[idx] = true;
                plan.set(src, true);
            }
        }
        plan
    }

    fn set_ema_committed(&mut self, src: Source, value: f32) {
        match src {
            Source::Cpu => self.ema_cpu_committed = value,
            Source::Mem => self.ema_mem_committed = value,
            Source::Gpu => self.ema_gpu_committed = value,
            Source::Net => self.ema_net_committed = value,
            // Power 不参与 EMA（纯分频采样，无变化检测）。
            Source::Power => {}
        }
    }

    pub fn update_ema(&mut self, src: Source, raw: f32) {
        let ema = match src {
            Source::Cpu => &mut self.ema_cpu,
            Source::Mem => &mut self.ema_mem,
            Source::Gpu => &mut self.ema_gpu,
            Source::Net => &mut self.ema_net,
            // Power 不参与 EMA。
            Source::Power => return,
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
            // Power 不参与 EMA。
            Source::Power => 0.0,
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

    #[test]
    fn test_power_interval_decoupled_from_base() {
        // 木桶 base 由其他启用源决定，但 Power 间隔独立。
        let mut s = Scheduler::new(1000, true);
        s.set_power_interval(5000);
        let enabled = [true, false, false, false, true]; // cpu + power
        assert_eq!(s.source_interval_ms(Source::Cpu, &enabled), TICK_MS as u32);
        assert_eq!(s.source_interval_ms(Source::Power, &enabled), 5000);
    }

    #[test]
    fn test_power_due_only_after_interval() {
        let mut s = Scheduler::new(1000, true);
        s.set_power_interval(3000);
        let enabled = [false, false, false, false, true];
        let p1 = s.plan(0, &enabled);
        assert!(p1.power, "首次必采");
        let p2 = s.plan(1_000, &enabled);
        assert!(!p2.power, "未到期不采");
        let p3 = s.plan(3_000, &enabled);
        assert!(p3.power, "3s 到期");
    }

    #[test]
    fn test_all_off_yields_empty_plan_and_idle_wake() {
        let mut s = Scheduler::new(1000, true);
        let enabled = [false; SRC_COUNT];
        let p = s.plan(10_000, &enabled);
        assert!(!p.any());
        // next_wake 退化为 IDLE 心跳（正常守卫下不会全关，此处只验证不 panic/不归零）。
        let wake = s.next_wake_ms(10_000, &enabled);
        assert!(wake > 10_000);
    }

    #[test]
    fn test_subset_barrel_base_adapts_to_item_count() {
        // 木桶：base = 启用源中最大的最小间隔（最慢源定节奏）。
        let mut s = Scheduler::new(1000, true);
        let cpu_only = [true, false, false, false, false];
        assert_eq!(s.base_interval_ms(&cpu_only), TICK_MS as u32);
        let gpu_only = [false, false, true, false, false];
        assert_eq!(s.base_interval_ms(&gpu_only), 2_000);
        let gpu_cpu = [true, false, true, false, false];
        assert_eq!(s.base_interval_ms(&gpu_cpu), 2_000, "GPU 慢源抬高整桶");
        // 关掉 GPU 后桶自动回落（数量自适应）。
        assert_eq!(s.base_interval_ms(&cpu_only), TICK_MS as u32);
    }
}
