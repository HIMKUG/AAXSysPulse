//! 功耗口径纯函数（scaphandre 式"能量→功率"派生的单一事实源）。
//!
//! # 语义
//!
//! 功率不是传感器直接给的，而是**测量窗口内的平均**：
//! `功率(W) = Δ能量(J) / Δ时间(s)`。能量计数器（RAPL PKG）是单调累积量，
//! 差分窗口由采样节奏决定。把公式收在本模块：
//! - 单测可覆盖（零平台依赖）；
//! - 展示端/进程分摊端共用同一公式，口径不会漂移；
//! - 物理合理性（钳制、窗口合法性）集中在同一处。

/// CPU 封装功耗的物理上限（瓦）。
///
/// 超出此值必为读数异常，而非真实功耗：AMD RAPL 的 32 位能量计数器
/// 在多 CCD 上读数会震荡/倒退。桌面/工作站封装上限约 280W，留余量取 400W。
pub const MAX_CPU_WATTS: f32 = 400.0;

/// 单帧最大间隔（毫秒）的**下限**：即便用户把功耗间隔调到很小，窗口超过这个值
/// 也说明调度抖动，该帧功率不可信。
pub const MAX_DT_MS: u64 = 5_000;

/// 最小有效间隔（毫秒）：太短会被计数器粒度放大成尖峰。
pub const MIN_DT_MS: u64 = 100;

/// 合法的**最长测量窗口** = `max(MAX_DT_MS, 2 × 功耗采样间隔)`。
///
/// ⚠️ 上界必须跟着用户配置的 `power_interval_ms` 走（审查项 2.1.4 的阻断级发现）：
/// 它原本写死 5 000 ms，而 `power_interval_ms` 允许 500..=60 000（`config.rs:323`
/// 默认 3 000，`scheduler.rs:154` clamp）。用户把功耗间隔调到 10 s 之后，
/// 每一帧的 `dt ≈ 10 000 > 5 000` 都被判为非法 ⇒ **功耗行恒 `--`**。
/// 取 2× 是给调度抖动留余量，同时仍能挡住"基线停住后 dt 累积到 3 个周期以上"
/// 那类假窗口（`win/src/msr.rs` 里"基线无条件推进"的注释解释了为什么要挡）。
pub fn max_dt_ms(interval_ms: u64) -> u64 {
    MAX_DT_MS.max(interval_ms.saturating_mul(2))
}

/// 由能量增量推导平均功率（瓦）。
///
/// - `delta_joules`：本窗口累计能量增量（焦耳）；
/// - `dt_ms`：窗口墙钟时长（毫秒）；
/// - `interval_ms`：**本路功耗采样的标称间隔**，用于推导窗口上界（见
///   [`max_dt_ms`]）。传调用方当前生效的 `power_interval_ms`，不要传常量。
///
/// 返回 `None` 表示该帧功率不可信（窗口过短/过长），调用方应沿用上一帧。
pub fn watts_from_energy_delta(delta_joules: f64, dt_ms: u64, interval_ms: u64) -> Option<f32> {
    if !(MIN_DT_MS..=max_dt_ms(interval_ms)).contains(&dt_ms) {
        return None;
    }
    let w = delta_joules * 1000.0 / dt_ms as f64;
    (w >= 0.0 && w <= MAX_CPU_WATTS as f64).then_some(w as f32)
}

/// 进程级功耗分摊：整机 CPU 实测功率按「该进程占总算力比例」分摊（瓦）。
///
/// `cpu_pct` 是**占总算力的百分比**，取值范围 `[0, 100]` —— 与 Windows
/// 任务管理器口径一致（单线程满载于 16 核机器 ≈ 6.25，全部核满载 = 100）。
/// 份额即 `cpu_pct / 100`，分摊功率 = `total_w × 份额`。
///
/// # 历史坑
///
/// 旧版要求传入「单核归一」的 `[0, cores*100]`，但 `win::process` 实际产出
/// 的是 `[0, 100]`（已除过 `cores`）。两者混用使分摊结果整体偏小 `cores`
/// 倍 —— 16 核机器上整整差 16 倍。统一到「占总算力」口径后不再需要
/// `cores` 参数，调用方也不会再传错。
pub fn share_cpu_power(total_w: f32, cpu_pct: f32) -> f32 {
    let share = (cpu_pct / 100.0).clamp(0.0, 1.0);
    (total_w * share).clamp(0.0, MAX_CPU_WATTS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watts_from_energy_delta_basic() {
        // 1 焦 / 1000ms = 1W
        assert_eq!(watts_from_energy_delta(1.0, 1000, 3000), Some(1.0));
        // 30 焦 / 1000ms = 30W
        assert_eq!(watts_from_energy_delta(30.0, 1000, 3000), Some(30.0));
    }

    #[test]
    fn watts_rejects_bad_windows() {
        assert_eq!(watts_from_energy_delta(1.0, 99, 3000), None, "过短窗口");
        assert_eq!(watts_from_energy_delta(1.0, 5_001, 1000), None, "间隔 1s 时窗口过长");
    }

    /// 2.1.4 的回归：功耗间隔被用户放大时，长窗口必须算**合法**。
    /// 旧实现上界恒为 5 000 ms ⇒ 间隔 10 s/60 s 时每帧都被拒 ⇒ 功耗行恒 `--`。
    #[test]
    fn long_window_is_legal_when_interval_is_long() {
        assert_eq!(max_dt_ms(1000), 5_000, "小间隔仍用 5s 下限");
        assert_eq!(max_dt_ms(10_000), 20_000);
        assert_eq!(max_dt_ms(60_000), 120_000);
        assert_eq!(max_dt_ms(u64::MAX), u64::MAX, "乘法溢出必须 saturate 而不是 panic/回绕");
        // 10 秒窗口、10 焦 → 1W，必须给得出值而不是 None
        assert_eq!(watts_from_energy_delta(10.0, 10_000, 10_000), Some(1.0));
        assert_eq!(watts_from_energy_delta(60.0, 60_000, 60_000), Some(1.0));
        // 但仍要挡住"窗口远超间隔"的假帧（2× 之外）
        assert_eq!(watts_from_energy_delta(10.0, 30_000, 10_000), None);
    }

    #[test]
    fn watts_clamps_physical_max() {
        // 能量暴增（读数为假）→ None（由调用方沿用上一帧），而不是输出 400W
        assert_eq!(watts_from_energy_delta(1_000_000.0, 1000, 3000), None);
    }

    #[test]
    fn share_cpu_power_division() {
        // 整机 100W、进程占总算力 12.5%（16 核机上约等于占满 2 核）→ 12.5W
        let w = share_cpu_power(100.0, 12.5);
        assert!((w - 12.5).abs() < 0.01);
        // 占总算力一半 → 50W
        let w = share_cpu_power(100.0, 50.0);
        assert!((w - 50.0).abs() < 0.01);
        // 全部核满载 → 吃掉整机全部功率
        let w = share_cpu_power(100.0, 100.0);
        assert!((w - 100.0).abs() < 0.01);
        // 越界值钳制
        assert_eq!(share_cpu_power(100.0, 9999.0), 100.0);
        assert_eq!(share_cpu_power(100.0, -5.0), 0.0);
    }
}