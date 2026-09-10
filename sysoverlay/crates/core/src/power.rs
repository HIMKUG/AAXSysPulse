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

/// 单帧最大间隔（毫秒）。超过说明调度抖动，该帧功率不可信。
pub const MAX_DT_MS: u64 = 5_000;

/// 最小有效间隔（毫秒）：太短会被计数器粒度放大成尖峰。
pub const MIN_DT_MS: u64 = 100;

/// 由能量增量推导平均功率（瓦）。
///
/// - `delta_joules`：本窗口累计能量增量（焦耳）；
/// - `dt_ms`：窗口墙钟时长（毫秒）。
///
/// 返回 `None` 表示该帧功率不可信（窗口过短/过长），调用方应沿用上一帧。
pub fn watts_from_energy_delta(delta_joules: f64, dt_ms: u64) -> Option<f32> {
    if !(MIN_DT_MS..=MAX_DT_MS).contains(&dt_ms) {
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
        assert_eq!(watts_from_energy_delta(1.0, 1000), Some(1.0));
        // 30 焦 / 1000ms = 30W
        assert_eq!(watts_from_energy_delta(30.0, 1000), Some(30.0));
    }

    #[test]
    fn watts_rejects_bad_windows() {
        assert_eq!(watts_from_energy_delta(1.0, 99), None, "过短窗口");
        assert_eq!(watts_from_energy_delta(1.0, 5_001), None, "过长窗口");
    }

    #[test]
    fn watts_clamps_physical_max() {
        // 能量暴增（读数为假）→ None（由调用方沿用上一帧），而不是输出 400W
        assert_eq!(watts_from_energy_delta(1_000_000.0, 1000), None);
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