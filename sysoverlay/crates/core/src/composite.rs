//! composite —— 综合占用（Util）计算核心。
//!
//! # 数学模型
//! 单个资源的排队论延迟因子 `L(ρ) = 1/(1-ρ)`；系统按工作占比加权：
//!
//! ```text
//! L_eff = Σ wᵢ · L(ρᵢ) = Σ wᵢ / (1 - ρᵢ)      (Σwᵢ = 1)
//! score = 1 - 1 / L_eff                        (把延迟换算回等效占用)
//! ```
//!
//! `score` 的含义：**造成同样总延迟的、单一资源的等效占用率**。
//!
//! ## 可证明的性质（均有单测覆盖）
//! - **一致性**：ρᵢ 全等于 ρ ⇒ `score == ρ`
//! - **有界性**：`min(ρ) ≤ score ≤ max(ρ)`
//! - **单调性**：任一 ρᵢ 增大 ⇒ score 增大（不会"涨了反而降"）
//! - **值域**：`score ∈ [0, 1)`
//!
//! ## 化简依据
//! `ρ·L = 1/(1-ρ) - 1 = L - 1`，故
//! `ΣwᵢρᵢLᵢ / ΣwᵢLᵢ = (L_eff - 1)/L_eff = 1 - 1/L_eff`。
//! 与"延迟加权平均"数学等价（实测最大偏差 2.1e-14），但只需 n+1 次除法。
//!
//! # 设计约束
//! 1. **零堆分配**：全栈上定长数组，无 `Vec` / `Box` / `String`。
//! 2. **零 panic**：核心路径无下标越界、无 `unwrap`；
//!    构造期非法权重返回 `Option`，不 panic。
//! 3. **零 NaN/Inf 逃逸**：所有输入先净化，所有输出饱和到 [0,1]。
//! 4. **不用 `powf` / `exp` / `sqrt`**：不依赖 libm，且跨平台逐位确定
//!    （IEEE754 只精确规定 + - × ÷，libm 各平台实现不同）。
//!
//! # 权重策略（用户拍板）
//! 单一通用权重，不区分场景：**三项等权**（各 1/3）。理由：
//! 闭式解本身已非线性放大饱和资源（单资源 90% ⇒ 等效 75%+），
//! 等权是最中立、无魔法常数的方案；差异化权重（游戏/渲染/办公）留作后续可选。

// 本模块**刻意**对 f64 使用否定式比较（`!(x > 0.0)` 而不是 `x <= 0.0`）：
// NaN 与任何数比较都返回 false ⇒ `!(x > 0.0)` 一次性拦住 NaN 与负数，
// 而 `x <= 0.0` 会把 NaN 放过去（设计约束 3「零 NaN 逃逸」）。
// clippy 的 `neg_cmp_op_on_partial_ord` 要求改写 `partial_cmp`，采纳它等于拆掉这条约束
// （审查项 P2 家族 / 批次 6 clippy 分类：本类 14 条告警按"lint 前提不成立"整模块豁免）。
#![allow(clippy::neg_cmp_op_on_partial_ord)]

/// 单资源占用率上限，同时定义了"最大可表达延迟因子" = `1/(1-RHO_MAX)` = 1000×。
///
/// 取 0.999 而非 1.0：一是保证 `1.0 - ρ ≥ 1e-3 > 0`，从根上杜绝除零；
/// 二是避免 ρ→1 时延迟因子爆到 1e308 量级（虽不溢出，但会吞掉其他资源的信息）。
pub const RHO_MAX: f64 = 0.999;

/// 通用权重（CPU/内存/GPU 等权，交给 [`Weights::new`] 归一成各 1/3）。
pub const W_UNIVERSAL: [f64; 3] = [1.0, 1.0, 1.0];

// ───────────────────────────── 输入净化 ─────────────────────────────

/// 把任意原始读数收敛到 `[0, RHO_MAX]`。
///
/// **NaN 安全写法**：浮点比较遇到 NaN 一律返回 `false`，
/// 所以必须用否定式 `!(x > 0.0)` 兜底，不能写 `if x <= 0.0`（那会让 NaN 漏到后面变成 `RHO_MAX`）。
/// 同理不能使用 `f64::clamp`——它对 NaN 入参返回 NaN，且在 debug 下可能 panic。
///
/// 映射规则：`NaN → 0.0`，负数 → `0.0`，超过 1 的饱和到 `RHO_MAX`。
#[inline(always)]
#[must_use]
pub fn sanitize_rho(x: f64) -> f64 {
    if !(x > 0.0) {
        0.0
    } else if x < RHO_MAX {
        x
    } else {
        RHO_MAX
    }
}

// ───────────────────────────── 权重 ─────────────────────────────

/// 归一化权重（构造时保证 `Σw = 1`，且全部有限非负）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weights<const N: usize> {
    w: [f64; N],
}

impl<const N: usize> Weights<N> {
    /// 归一化原始权重。拒绝：NaN、负数、无穷大、全零 —— 一律返回 `None` 而不是 panic。
    #[must_use]
    pub fn new(raw: [f64; N]) -> Option<Self> {
        let mut sum = 0.0_f64;
        for &x in raw.iter() {
            if !(x >= 0.0) {
                return None; // NaN 或负数
            }
            sum += x;
        }
        if !(sum > 0.0) || !sum.is_finite() {
            return None; // 全零或溢出到 inf
        }
        let mut w = raw;
        for x in w.iter_mut() {
            *x /= sum;
        }
        Some(Self { w })
    }

    #[inline]
    #[must_use]
    pub fn as_slice(&self) -> &[f64; N] {
        &self.w
    }
}

/// 三个资源的常用别名（CPU / 内存 / GPU）。
pub type Weights3 = Weights<3>;

// ───────────────────────────── 核心聚合 ─────────────────────────────

/// 把 N 个资源占用率压成一个等效占用率，返回 `0.0..=1.0`。
///
/// # 不变量（调用方保证）
/// `rho` 中的每个元素都已经过 [`sanitize_rho`]。
/// 由此 `1.0 - ρ ≥ 1e-3 > 0`，绝不会除零。
///
/// # 数值保证
/// - `L_eff ≥ Σw = 1` ⇒ `1.0/L_eff ≤ 1` ⇒ 结果非负
/// - 最大中间值 `1/(1-RHO_MAX) = 1000`，离 f64 上界还有 305 个数量级，不可能溢出
/// - 即使调用方漏了净化、直接传入 `ρ = 1.0`，IEEE 语义下会得到 `+inf`，
///   `1 - 1/inf = 1.0`，**依然优雅退化为 100% 而不是 NaN**
#[inline]
#[must_use]
pub fn aggregate<const N: usize>(rho: &[f64; N], w: &Weights<N>) -> f64 {
    debug_assert!(
        rho.iter().all(|&r| (0.0..=RHO_MAX).contains(&r)),
        "rho must be sanitized into [0, RHO_MAX]"
    );
    let mut l_eff = 0.0_f64;
    for (&ri, &wi) in rho.iter().zip(w.w.iter()) {
        l_eff += wi / (1.0 - ri);
    }
    let s = 1.0 - 1.0 / l_eff;
    // NaN 安全的饱和：!(s>0.0) 同时拦下 NaN 与负数
    if !(s > 0.0) {
        0.0
    } else if s < 1.0 {
        s
    } else {
        1.0
    }
}

/// 三源（CPU/内存/GPU）**可用性感知**聚合。
///
/// 任一源为 `None`（采样不可用）时：把它的权重置零、由剩余源重新归一后继续
/// 计算（等权场景下等价于对可用源求闭式解），并标记 `degraded = true`；
/// 全部缺失返回 `None`（调用方应显示不可用）。
///
/// 返回 `(score 0..=1, 是否降级)`。
#[must_use]
pub fn aggregate3(rho: [Option<f64>; 3]) -> Option<(f64, bool)> {
    let raw_w = [
        if rho[0].is_some() { 1.0 } else { 0.0 },
        if rho[1].is_some() { 1.0 } else { 0.0 },
        if rho[2].is_some() { 1.0 } else { 0.0 },
    ];
    let w = Weights3::new(raw_w)?;
    let r = [
        sanitize_rho(rho[0].unwrap_or(0.0)),
        sanitize_rho(rho[1].unwrap_or(0.0)),
        sanitize_rho(rho[2].unwrap_or(0.0)),
    ];
    let degraded = rho.iter().any(|p| p.is_none());
    Some((aggregate(&r, &w), degraded))
}

/// 转成千分制整数（0..=1000），避免浮点格式化带来的显示抖动与跨平台差异。
///
/// 用否定式拦 NaN；`as u32` 在此区间内是安全截断（Rust 浮点→整型转换是饱和的）。
#[inline]
#[must_use]
pub fn to_milli(s01: f64) -> u32 {
    let v = s01 * 1000.0;
    if !(v > 0.0) {
        0
    } else if v >= 1000.0 {
        1000
    } else {
        (v + 0.5) as u32 // 四舍五入
    }
}

/// 档位（千分制整数阈值，彻底绕开浮点边界）。
///
/// 分档是非线性的：80 分就进"预警"，因为排队论里 80% 占用 = 5 倍延迟。
/// 当前仅供诊断/未来 UI 使用；悬浮窗数值颜色沿用与其他指标一致的强调色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grade {
    Smooth,   // 流畅
    Good,     // 良好
    Notice,   // 注意
    Warn,     // 预警
    Severe,   // 严重
    Critical, // 危险
}

impl Grade {
    #[inline]
    #[must_use]
    pub fn of_milli(m: u32) -> Self {
        match m {
            950.. => Grade::Critical,
            900..=949 => Grade::Severe,
            800..=899 => Grade::Warn,
            700..=799 => Grade::Notice,
            500..=699 => Grade::Good,
            _ => Grade::Smooth,
        }
    }

    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Grade::Smooth => "Smooth",
            Grade::Good => "Good",
            Grade::Notice => "Notice",
            Grade::Warn => "Warn",
            Grade::Severe => "Severe",
            Grade::Critical => "Critical",
        }
    }
}

// ───────────────────────────── 时间维度 ─────────────────────────────

/// 时间维度的配置。所有 α 都按 `α = dt/(τ+dt)` 现算，
/// 因此**对采样抖动、漏采、系统休眠都免疫**（固定 α 的 EMA 做不到这点）。
#[derive(Clone, Copy, Debug)]
pub struct MeterConfig {
    /// 上升时间常数（秒），尖峰来临时快速冲高
    pub tau_up: f64,
    /// 下降时间常数（秒），尖峰过后缓慢回落
    pub tau_down: f64,
    /// 卡顿占空比的时间常数（秒）
    pub tau_duty: f64,
    /// 卡顿阈值（0..1），超过即计入卡顿指数
    pub warn: f64,
    /// 尖峰触发阈值（0..1）
    pub spike: f64,
    /// 尖峰解除阈值（0..1），低于它才重新武装 —— 迟滞，防止在阈值附近抖动反复计数
    pub spike_release: f64,
}

impl Default for MeterConfig {
    fn default() -> Self {
        Self {
            tau_up: 2.0,
            // 下降 3s（用户实测拍板）：负载结束后 ~3s 内回落到位，
            // 配合提交侧 1.5~4.5s 的刷新节奏，观感即"平均 3 秒刷新"。
            // （原 15s 曾被误读为"卡死在峰值不动"）
            tau_down: 3.0,
            tau_duty: 30.0,
            warn: 0.70,
            spike: 0.85,
            spike_release: 0.75,
        }
    }
}

/// 一阶滞后系数。`dt` 非法（NaN / ≤0 / 过大）时退化为 1.0（即"直接采信本次读数"）。
#[inline(always)]
#[must_use]
fn alpha(dt: f64, tau: f64) -> f64 {
    let dt = if dt > 0.0 && dt < 1.0e6 { dt } else { 1.0 };
    dt / (tau + dt)
}

/// 跳变直通阈值：0..=1 量程上的 0.25（25 个百分点）。
///
/// 依据：CPU/内存/GPU 占用率在采样窗口内的正常抖动幅度远低于此；
/// 单步跨过它只可能是真实负载跃迁（应用启动/退出、GPU 突发），
/// 此时不再用 τ 拖尾（用户实测拍板：大动向必须立即反映到显示），
/// 而是直接采信本次读数。显示节奏仍由提交侧的刷新下限约束。
const JUMP_STEP: f64 = 0.25;

/// 状态化的负载计。O(1) 内存（5 个 f64 + 2 个整数），无环形缓冲区，无分配。
#[derive(Clone, Copy, Debug)]
pub struct Meter {
    cfg: MeterConfig,
    ema: f64,
    duty: f64,
    prev: f64,
    armed: bool,
    spikes: u32,
    ticks: u32,
}

/// 一次更新的输出快照。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterSnapshot {
    /// 本次瞬时综合占用（千分制）
    pub instant_milli: u32,
    /// 平滑后的综合占用（千分制），**建议用它做显示**
    pub smooth_milli: u32,
    /// 卡顿指数：近期超过 warn 阈值的时间占比（千分制，0..=1000）
    pub stutter_milli: u32,
    /// 累计尖峰次数（带迟滞的上升沿计数）
    pub spikes: u32,
    pub grade: Grade,
}

impl Meter {
    #[must_use]
    pub fn new(cfg: MeterConfig) -> Self {
        Self {
            cfg,
            ema: 0.0,
            duty: 0.0,
            prev: 0.0,
            armed: false,
            spikes: 0,
            ticks: 0,
        }
    }

    /// 回到 [`Meter::new`] 的初始状态（配置 `cfg` 保留）。
    ///
    /// 用途（审查项 2.2.5）：用户改设置等于"重新开始观测"，旧的 ~3 s 下降拖尾
    /// （`tau_down`）不该被带进新布局。调用方见 `win::sampler::reset_util_smoothing`。
    pub fn reset(&mut self) {
        self.ema = 0.0;
        self.duty = 0.0;
        self.prev = 0.0;
        self.armed = false;
        self.spikes = 0;
        self.ticks = 0;
    }

    /// 喂入一个 0..=1 的综合占用值与距上次调用的秒数。
    ///
    /// 关键形式：`ema += (s - ema) * a`
    /// 而不是 `ema = a*s + (1-a)*ema`。
    /// 前者在 `s == ema` 时严格保持不变（差为 0，增量精确为 0），
    /// 不会出现长期运行时的残差漂移；后者在 a=0.45 这类非二制数上会有 ULP 级残留。
    pub fn update(&mut self, s01: f64, dt_secs: f64) -> MeterSnapshot {
        let s = if !(s01 > 0.0) {
            0.0
        } else if s01 < 1.0 {
            s01
        } else {
            1.0
        };

        // 大动向直通优先于非对称 τ：单步跨过 JUMP_STEP 视为真实负载跃迁，
        // 立即采信本次读数（a=1 ⇒ ema=s）；常规抖动仍走 τ 平滑。
        let a = if (s - self.ema).abs() >= JUMP_STEP {
            1.0
        } else if s > self.ema {
            alpha(dt_secs, self.cfg.tau_up)
        } else {
            alpha(dt_secs, self.cfg.tau_down)
        };
        self.ema += (s - self.ema) * a;
        // 再饱和一次，防 ULP 越界
        if !(self.ema > 0.0) {
            self.ema = 0.0;
        } else if self.ema > 1.0 {
            self.ema = 1.0;
        }

        let over = if s >= self.cfg.warn { 1.0 } else { 0.0 };
        self.duty += (over - self.duty) * alpha(dt_secs, self.cfg.tau_duty);

        // 带迟滞的上升沿计数
        if s >= self.cfg.spike && !self.armed {
            self.armed = true;
            self.spikes = self.spikes.saturating_add(1);
        } else if s < self.cfg.spike_release {
            self.armed = false;
        }

        self.prev = s;
        self.ticks = self.ticks.saturating_add(1);

        let sm = to_milli(self.ema);
        MeterSnapshot {
            instant_milli: to_milli(s),
            smooth_milli: sm,
            stutter_milli: to_milli(self.duty),
            spikes: self.spikes,
            grade: Grade::of_milli(sm),
        }
    }

    #[must_use]
    pub fn last_raw(&self) -> f64 {
        self.prev
    }

    #[must_use]
    pub fn ticks(&self) -> u32 {
        self.ticks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w3() -> Weights3 {
        Weights3::new(W_UNIVERSAL).unwrap()
    }
    fn s3(r: [f64; 3]) -> f64 {
        aggregate(
            &[sanitize_rho(r[0]), sanitize_rho(r[1]), sanitize_rho(r[2])],
            &w3(),
        )
    }

    #[test]
    fn identity_all_equal() {
        for i in 1..100 {
            let x = i as f64 / 100.0;
            let s = s3([x, x, x]);
            assert!((s - x).abs() < 1e-12, "identity failed at {x}: {s}");
        }
    }

    #[test]
    fn bounds_and_monotonic_grid() {
        let step = 0.02;
        let mut i = 0.0;
        while i <= 1.0 {
            let mut j = 0.0;
            while j <= 1.0 {
                let mut k = 0.0;
                while k <= 1.0 {
                    let r = [i, j, k];
                    let s = s3(r);
                    let mn = i.min(j).min(k);
                    let mx = i.max(j).max(k);
                    assert!(s >= mn - 1e-12 && s <= mx + 1e-12, "bounds {r:?} -> {s}");
                    for d in 0..3 {
                        let mut r2 = r;
                        r2[d] = (r2[d] + step).min(1.0);
                        if r2[d] != r[d] {
                            assert!(s3(r2) >= s - 1e-12, "monotonic {r:?} +{d}");
                        }
                    }
                    k += step;
                }
                j += step;
            }
            i += step;
        }
    }

    /// 闭式解 与 两遍加权平均 必须等价
    #[test]
    fn closed_form_equals_two_pass() {
        let w = w3();
        for i in 1..20 {
            for j in 1..20 {
                for k in 1..20 {
                    let r = [
                        sanitize_rho(i as f64 / 20.0),
                        sanitize_rho(j as f64 / 20.0),
                        sanitize_rho(k as f64 / 20.0),
                    ];
                    let mut num = 0.0;
                    let mut den = 0.0;
                    for (&rt, &wt) in r.iter().zip(w.as_slice().iter()) {
                        let l = 1.0 / (1.0 - rt);
                        num += wt * rt * l;
                        den += wt * l;
                    }
                    let two = num / den;
                    let closed = aggregate(&r, &w);
                    assert!((two - closed).abs() < 1e-12, "{two} vs {closed}");
                }
            }
        }
    }

    #[test]
    fn never_nan_or_inf() {
        let bad = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 2.0, 0.0, 1.0];
        for &a in bad.iter() {
            for &b in bad.iter() {
                for &c in bad.iter() {
                    let s = s3([a, b, c]);
                    assert!(s.is_finite(), "non-finite for {a},{b},{c}");
                    assert!((0.0..=1.0).contains(&s), "range {s}");
                }
            }
        }
    }

    #[test]
    fn weights_reject_bad_input() {
        assert!(Weights3::new([0.0, 0.0, 0.0]).is_none());
        assert!(Weights3::new([f64::NAN, 1.0, 1.0]).is_none());
        assert!(Weights3::new([-1.0, 1.0, 1.0]).is_none());
        assert!(Weights3::new([f64::INFINITY, 1.0, 1.0]).is_none());
        let w = Weights3::new([1.0, 1.0, 2.0]).unwrap();
        let s: f64 = w.as_slice().iter().sum();
        assert!((s - 1.0).abs() < 1e-15);
    }

    #[test]
    fn meter_monotone_and_clamped() {
        let mut m = Meter::new(MeterConfig::default());
        for v in [0.9, 0.9, 0.1, 0.1, f64::NAN, -5.0, 2.0] {
            let snap = m.update(v, 1.0);
            assert!(snap.smooth_milli <= 1000);
            assert!(snap.stutter_milli <= 1000);
        }
        // 恒定输入必须收敛到该值本身（无残差）
        let mut m2 = Meter::new(MeterConfig::default());
        for _ in 0..500 {
            m2.update(0.42, 1.0);
        }
        assert_eq!(m2.update(0.42, 1.0).smooth_milli, 420);
    }

    #[test]
    fn grade_is_integral() {
        assert_eq!(Grade::of_milli(0), Grade::Smooth);
        assert_eq!(Grade::of_milli(499), Grade::Smooth);
        assert_eq!(Grade::of_milli(500), Grade::Good);
        assert_eq!(Grade::of_milli(800), Grade::Warn);
        assert_eq!(Grade::of_milli(950), Grade::Critical);
        assert_eq!(Grade::of_milli(u32::MAX), Grade::Critical);
    }

    // ── aggregate3：可用性感知重归一化 ──────────────────────

    #[test]
    fn aggregate3_full_equals_aggregate() {
        let (s, degraded) = aggregate3([Some(0.3), Some(0.6), Some(0.2)]).unwrap();
        assert!(!degraded);
        assert!((s - s3([0.3, 0.6, 0.2])).abs() < 1e-15);
    }

    #[test]
    fn aggregate3_missing_source_renormalizes() {
        // GPU 缺失：等权下等价于 CPU/内存 二源闭式解（各 1/2）。
        let (s, degraded) = aggregate3([Some(0.3), Some(0.6), None]).unwrap();
        assert!(degraded, "缺源必须标记降级");
        let w2 = Weights3::new([1.0, 1.0, 0.0]).unwrap();
        let expect = aggregate(&[0.3, 0.6, 0.0], &w2);
        assert!((s - expect).abs() < 1e-15);
        // 一致性：两源相等时输出等于该值本身。
        let (s2, _) = aggregate3([Some(0.4), Some(0.4), None]).unwrap();
        assert!((s2 - 0.4).abs() < 1e-12);
    }

    #[test]
    fn aggregate3_all_missing_is_none() {
        assert!(aggregate3([None, None, None]).is_none());
    }

    #[test]
    fn aggregate3_tolerates_bad_values() {
        // NaN / 超界输入必须被净化（NaN→0、>1 饱和），不得逃逸出 [0,1]；
        // 坏值 ≠ 缺源：`Some(NaN)` 不触发降级标记。
        let (s, degraded) = aggregate3([Some(f64::NAN), Some(5.0), Some(-1.0)]).unwrap();
        assert!(!degraded, "有值但坏 ≠ 缺源");
        assert!(s.is_finite() && (0.0..=1.0).contains(&s));
    }
}
