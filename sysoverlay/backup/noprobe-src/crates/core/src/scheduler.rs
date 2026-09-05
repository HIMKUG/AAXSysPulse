//! 自适应刷新调度器（v3 性能核心）。
//!
//! # 问题
//!
//! 旧实现所有采样源（CPU/Mem/GPU/Net）都跟同一个 `interval_ms` 走：
//! - GPU/Net 的系统计数器本身约 **1 秒**才更新一次，10fps 设置下每 100ms
//!   采一次是纯浪费（PDH/GetIfTable2 都不便宜）——这就是用户观察到
//!   「设成 1fps 仍占 6% CPU」的根源之一；
//! - 反过来，用户设了低刷新率时，CPU/Mem 这种廉价且高频更新的源也被拖慢。
//!
//! # 设计（三件事）
//!
//! 1. **数据可用频率上限**：每个源有一个最小有效间隔
//!    [`MIN_EFFECTIVE_MS`]（低于它采到的还是旧计数器值，没有意义）。
//!    `Auto` 档的刷新上限 = 启用源中的最大值——这正是
//!    「实时侦测当前最高能获取到的系统状态的频率，设置一个自身的刷新上限」。
//! 2. **分频调度**：每个源独立记 `last_sample`，到期才采；
//!    未到期源在 [`SamplePlan`] 里为 false，采样器直接沿用上一次的值。
//!    手动档只约束 CPU/Mem 的节奏，GPU/Net 永远不快于 1s。
//! 3. **成本反馈**：每轮记录采样耗时占比的 EMA；持续超过预算
//!    ([`COST_BUDGET`]) 就把基础间隔翻倍降档（最多 [`MAX_PENALTY`] 级），
//!    空闲持续 [`CALM_CYCLES`] 轮再逐级回升。自愈，不依赖用户手调。
//!
//! # 分层约束
//!
//! 纯逻辑、零平台依赖、零 `unsafe`——调度决策可单测，平台层只管执行。

/// 调度器覆盖的采样源（Power 走独立的 `power_interval_ms`，不在其列）。
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

    /// 每个源计数器的**最小有效更新间隔**（毫秒）。
    ///
    /// - Cpu：`GetSystemTimes` 随系统时钟节拍更新（~15.6ms），显示层面
    ///   100ms 已是可感知上限，再快只是烧 CPU；
    /// - Mem：`GlobalMemoryStatusEx` 是即时快照，开销极低，同为 100ms；
    /// - Gpu：PDH/DXGI 计数器约 1s 才刷新，更快采到的还是旧值；
    /// - Net：`GetIfTable2` 字节计数器按 1s 粒度更新。
    pub const fn min_effective_ms(self) -> u32 {
        match self {
            Source::Cpu | Source::Mem => 100,
            Source::Gpu | Source::Net => 1_000,
        }
    }
}

/// 便捷常量数组：`MIN_EFFECTIVE_MS[src as usize]`。
pub const MIN_EFFECTIVE_MS: [u32; SRC_COUNT] = [100, 100, 1_000, 1_000];

/// 一轮采样计划：哪些源这次要真的采样。
///
/// `false` 的源由采样器沿用上一次快照的值（诊断记 `Skipped`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SamplePlan {
    pub cpu: bool,
    pub mem: bool,
    pub gpu: bool,
    pub net: bool,
}

impl SamplePlan {
    /// 是否有任何一个源要采样。
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

/// 采样源启用掩码（来自配置的 show_cpu/show_mem/show_gpu/show_net）。
pub type EnabledMask = [bool; SRC_COUNT];

/// 成本占比 EMA 平滑系数（新值权重 1/4）。
const COST_EMA_ALPHA_NUM: u32 = 1;
const COST_EMA_ALPHA_DEN: u32 = 4;
/// 采样耗时占比预算：超过它说明采样本身太贵，要降档。
const COST_BUDGET_PCT: u32 = 8; // %
/// 低于此占比视为「空闲」，连续 CALM_CYCLES 轮后回升一档。
const COST_CALM_PCT: u32 = 2; // %
const CALM_CYCLES: u32 = 10;
/// 成本惩罚档位数：间隔 ×2^level。
const MAX_PENALTY: u32 = 3;
/// 降档后的间隔上限（毫秒）——再贵也不慢过 0.2fps。
const MAX_INTERVAL_MS: u32 = 5_000;
/// 无任何启用源时的兜底间隔。
const IDLE_INTERVAL_MS: u32 = 1_000;

/// 自适应刷新调度器。
///
/// 生命周期：随采样线程构造一次；设置页改动通过 [`Scheduler::set_user`]
/// 注入；每轮 `plan()` → 采样 → `record_cycle()` → `next_wake_ms()`。
#[derive(Debug)]
pub struct Scheduler {
    /// 用户请求的间隔（手动档）。Auto 档忽略此值。
    user_interval_ms: u32,
    /// Auto 档：刷新上限取启用源的最大有效间隔。
    auto: bool,
    /// 成本惩罚档位（0..=MAX_PENALTY），间隔 ×2^level。
    penalty: u32,
    /// 连续超预算轮数（≥2 才降档，吸收单次尖峰）。
    overload_streak: u32,
    /// 连续低成本轮数（回升惩罚用）。
    calm_cycles: u32,
    /// 成本占比 EMA（basis points：800 = 8%）。
    cost_ema_bp: u32,
    /// 每源上次采样时间（GetTickCount64 毫秒）。
    last_sample_ms: [u64; SRC_COUNT],
    /// 每源是否已采样过（首轮必须全采，保证第一时间有数据）。
    has_sampled: [bool; SRC_COUNT],
}

impl Scheduler {
    pub fn new(user_interval_ms: u32, auto: bool) -> Self {
        Self {
            user_interval_ms: user_interval_ms.clamp(100, MAX_INTERVAL_MS),
            auto,
            penalty: 0,
            overload_streak: 0,
            calm_cycles: 0,
            cost_ema_bp: 0,
            last_sample_ms: [0; SRC_COUNT],
            has_sampled: [false; SRC_COUNT],
        }
    }

    /// 设置页改动注入（每轮都可调用，代价为零）。
    pub fn set_user(&mut self, user_interval_ms: u32, auto: bool) {
        self.user_interval_ms = user_interval_ms.clamp(100, MAX_INTERVAL_MS);
        self.auto = auto;
    }

    /// 基础间隔（未含成本惩罚）。
    ///
    /// - Auto：启用源的最大最小有效间隔——系统状态多久变一次，
    ///   刷新上限就是多少，绝不做无意义的空转；
    /// - 手动：用户请求值直接生效（GPU/Net 的下限在 [`Self::source_interval_ms`]
    ///   里另行保证）。
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

    /// 含成本惩罚后的基础间隔。
    pub fn effective_interval_ms(&self, enabled: &EnabledMask) -> u32 {
        (self.base_interval_ms(enabled) << self.penalty).min(MAX_INTERVAL_MS)
    }

    /// 单个源的实际采样间隔：基础间隔与该源最小有效间隔取大。
    pub fn source_interval_ms(&self, src: Source, enabled: &EnabledMask) -> u32 {
        self.effective_interval_ms(enabled).max(MIN_EFFECTIVE_MS[src as usize])
    }

    /// 生成本轮采样计划，并对到期源记录 `last_sample`。
    ///
    /// **首轮所有启用源都到期**（`has_sampled` 标志），保证第一时间有数据。
    pub fn plan(&mut self, now_ms: u64, enabled: &EnabledMask) -> SamplePlan {
        let mut plan = SamplePlan::default();
        for &src in Source::ALL.iter() {
            let idx = src as usize;
            if !enabled[idx] {
                continue; // 禁用源永不采样
            }
            let interval = self.source_interval_ms(src, enabled) as u64;
            let due = !self.has_sampled[idx]
                || now_ms.saturating_sub(self.last_sample_ms[idx]) >= interval;
            if due {
                self.last_sample_ms[idx] = now_ms;
                self.has_sampled[idx] = true;
                plan.set(src, true);
            }
        }
        plan
    }

    /// 记录一轮采样成本（毫秒），驱动成本反馈降档/回升。
    ///
    /// 占比 = 耗时 / 生效间隔。EMA 平滑后：
    /// - `> COST_BUDGET_PCT` **连续 2 轮**：升一档惩罚（间隔翻倍）——
    ///   连续性要求让 EMA 吸收单次尖峰，不因一次抖动降档；
    /// - `< COST_CALM_PCT`：连续 [`CALM_CYCLES`] 轮后降一档（间隔减半）。
    pub fn record_cycle(&mut self, cost_ms: u32, enabled: &EnabledMask) {
        let interval = self.effective_interval_ms(enabled).max(1);
        let ratio_bp = (cost_ms.min(interval) as u64 * 10_000 / interval as u64) as u32;
        self.cost_ema_bp = (self.cost_ema_bp * (COST_EMA_ALPHA_DEN - COST_EMA_ALPHA_NUM)
            + ratio_bp * COST_EMA_ALPHA_NUM)
            / COST_EMA_ALPHA_DEN;

        if self.cost_ema_bp > COST_BUDGET_PCT * 100 {
            self.overload_streak += 1;
            if self.overload_streak >= 2 && self.penalty < MAX_PENALTY {
                self.penalty += 1;
            }
            self.calm_cycles = 0;
        } else {
            self.overload_streak = 0;
            if self.cost_ema_bp < COST_CALM_PCT * 100 {
                self.calm_cycles += 1;
                if self.calm_cycles >= CALM_CYCLES && self.penalty > 0 {
                    self.penalty -= 1;
                    self.calm_cycles = 0;
                }
            } else {
                self.calm_cycles = 0;
            }
        }
    }

    /// 线程下次唤醒时间（毫秒 tick）。永不早于 `now_ms`（忙时立即返回 now）。
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

    /// 当前成本 EMA（诊断/测试用，basis points）。
    pub fn cost_ema_bp(&self) -> u32 {
        self.cost_ema_bp
    }

    /// 当前惩罚档位（诊断/测试用）。
    pub fn penalty(&self) -> u32 {
        self.penalty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_ON: EnabledMask = [true, true, true, true];
    const CPU_ONLY: EnabledMask = [true, false, false, false];

    // ── Auto 档：刷新上限 = 启用源的最大有效间隔 ─────────────

    #[test]
    fn test_auto_caps_at_slowest_enabled_source() {
        let mut s = Scheduler::new(100, true);
        assert_eq!(
            s.base_interval_ms(&ALL_ON),
            1_000,
            "GPU/Net 在列时 Auto 上限是 1s"
        );
        assert_eq!(
            s.base_interval_ms(&CPU_ONLY),
            100,
            "只有 CPU/Mem 时 Auto 可以跑满 100ms"
        );
    }

    #[test]
    fn test_auto_with_no_enabled_sources_falls_back() {
        let mut s = Scheduler::new(100, true);
        assert_eq!(s.base_interval_ms(&[false; 4]), IDLE_INTERVAL_MS);
    }

    // ── 手动档：用户间隔生效，GPU/Net 仍受 1s 下限 ───────────

    #[test]
    fn test_manual_respects_user_interval_for_cpu() {
        let mut s = Scheduler::new(100, false); // 10fps
        assert_eq!(s.base_interval_ms(&ALL_ON), 100);
        assert_eq!(
            s.source_interval_ms(Source::Cpu, &ALL_ON),
            100,
            "手动 10fps 时 CPU 跟随用户节奏"
        );
        assert_eq!(
            s.source_interval_ms(Source::Gpu, &ALL_ON),
            1_000,
            "手动 10fps 时 GPU 仍不快于 1s（计数器本身 1s 才更新）"
        );
        assert_eq!(s.source_interval_ms(Source::Net, &ALL_ON), 1_000);
    }

    #[test]
    fn test_manual_slow_interval_propagates_to_all_sources() {
        let mut s = Scheduler::new(5_000, false); // 0.2fps
        assert_eq!(s.source_interval_ms(Source::Cpu, &ALL_ON), 5_000);
        assert_eq!(s.source_interval_ms(Source::Gpu, &ALL_ON), 5_000);
    }

    #[test]
    fn test_user_interval_is_clamped() {
        let mut s = Scheduler::new(1, false);
        assert_eq!(s.base_interval_ms(&ALL_ON), 100, "下限 100ms");
        let mut s2 = Scheduler::new(999_999, false);
        assert_eq!(s2.base_interval_ms(&ALL_ON), MAX_INTERVAL_MS, "封顶 5s");
    }

    // ── 分频调度：plan 只在到期时采样 ────────────────────────

    #[test]
    fn test_first_plan_samples_all_enabled() {
        let mut s = Scheduler::new(100, false);
        let plan = s.plan(50, &ALL_ON);
        assert_eq!(plan, SamplePlan { cpu: true, mem: true, gpu: true, net: true }, "首轮全部到期");
    }

    #[test]
    fn test_gpu_not_due_within_one_second() {
        let mut s = Scheduler::new(100, false);
        let _ = s.plan(0, &ALL_ON); // 首轮：全部采样
        let plan = s.plan(500, &ALL_ON);
        assert!(plan.cpu, "CPU 每 100ms 到期");
        assert!(plan.mem);
        assert!(!plan.gpu, "GPU 500ms 时未到期");
        assert!(!plan.net, "Net 500ms 时未到期");
    }

    #[test]
    fn test_gpu_due_after_one_second() {
        let mut s = Scheduler::new(100, false);
        let _ = s.plan(0, &ALL_ON);
        let plan = s.plan(1_000, &ALL_ON);
        assert!(plan.gpu, "1000ms 时 GPU 必须到期");
        assert!(plan.net);
    }

    #[test]
    fn test_disabled_source_never_planned() {
        let mut s = Scheduler::new(100, false);
        let _ = s.plan(0, &CPU_ONLY);
        let plan = s.plan(10_000, &CPU_ONLY);
        assert!(plan.cpu);
        assert!(!plan.mem && !plan.gpu && !plan.net, "禁用源永不采样");
    }

    // ── 成本反馈降档/回升 ────────────────────────────────────

    #[test]
    fn test_high_cost_derates_interval() {
        let mut s = Scheduler::new(1_000, false);
        let _ = s.plan(0, &ALL_ON);
        // 300ms 采样耗时 / 1000ms 间隔 = 30% ≫ 8% 预算；
        // EMA 从 0 起步，第 3 轮起连续超预算 → 降档
        s.record_cycle(300, &ALL_ON);
        s.record_cycle(300, &ALL_ON);
        s.record_cycle(300, &ALL_ON);
        assert!(s.penalty() > 0, "成本超标必须降档");
        assert_eq!(
            s.effective_interval_ms(&ALL_ON),
            2_000,
            "一档惩罚 = 间隔翻倍"
        );
    }

    #[test]
    fn test_penalty_caps_at_max() {
        let mut s = Scheduler::new(1_000, false);
        let _ = s.plan(0, &ALL_ON);
        for _ in 0..20 {
            s.record_cycle(900, &ALL_ON); // 90% 占比
        }
        assert_eq!(s.penalty(), MAX_PENALTY);
        assert_eq!(
            s.effective_interval_ms(&ALL_ON),
            MAX_INTERVAL_MS,
            "惩罚封顶 5s，不慢过 0.2fps"
        );
    }

    #[test]
    fn test_low_cost_recovers_gradually() {
        let mut s = Scheduler::new(1_000, false);
        let _ = s.plan(0, &ALL_ON);
        for _ in 0..10 {
            s.record_cycle(300, &ALL_ON);
        }
        assert!(s.penalty() > 0);
        // 回升节奏：每 CALM_CYCLES 轮空闲降一档（3 档 → 需要 30+ 轮）
        for _ in 0..40 {
            s.record_cycle(1, &ALL_ON);
        }
        assert_eq!(s.penalty(), 0, "持续空闲应逐级回升");
        assert_eq!(s.effective_interval_ms(&ALL_ON), 1_000);
    }

    #[test]
    fn test_record_cycle_uses_ema_not_single_spike() {
        let mut s = Scheduler::new(1_000, false);
        let _ = s.plan(0, &ALL_ON);
        s.record_cycle(1, &ALL_ON);
        s.record_cycle(1, &ALL_ON);
        // 单次尖峰不该立刻降档（EMA 平滑 + 连续性要求）
        s.record_cycle(999, &ALL_ON);
        assert_eq!(s.penalty(), 0, "EMA 应吸收单次尖峰");
        // 连续两轮超预算才降档
        s.record_cycle(999, &ALL_ON);
        assert_eq!(s.penalty(), 1, "持续超载必须降档");
    }

    // ── 唤醒时间 ─────────────────────────────────────────────

    #[test]
    fn test_next_wake_is_min_due_across_enabled_sources() {
        let mut s = Scheduler::new(100, false);
        let _ = s.plan(0, &ALL_ON);
        // last 全为 0 → CPU 下次到期 100ms，GPU 1000ms → min = 100
        assert_eq!(s.next_wake_ms(0, &ALL_ON), 100);
    }

    #[test]
    fn test_next_wake_never_earlier_than_now() {
        let mut s = Scheduler::new(100, false);
        let _ = s.plan(0, &ALL_ON);
        let wake = s.next_wake_ms(5_000, &ALL_ON);
        assert!(wake >= 5_000, "过期源应立即唤醒，不能回退到过去");
    }

    #[test]
    fn test_next_wake_with_no_enabled_sources() {
        let s = Scheduler::new(100, false);
        assert_eq!(
            s.next_wake_ms(1_000, &[false; 4]),
            2_000,
            "全禁用时按兜底间隔休眠"
        );
    }

    // ── 布局/结构约束 ────────────────────────────────────────

    #[test]
    fn test_min_effective_constants() {
        assert_eq!(MIN_EFFECTIVE_MS, [100, 100, 1_000, 1_000]);
        for &src in Source::ALL.iter() {
            assert_eq!(src.min_effective_ms(), MIN_EFFECTIVE_MS[src as usize]);
        }
    }

    #[test]
    fn test_scheduler_is_sendable() {
        // 采样线程独占持有，必须可跨线程构造
        fn assert_send<T: Send>() {}
        assert_send::<Scheduler>();
    }
}
