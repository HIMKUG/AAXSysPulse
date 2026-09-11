//! 智能调度节拍常量。
//!
//! 所有采样周期取 1500ms 的约数，保证每个节拍点所有源必然都有新值。
//! 这是 v2 相比 v1 木桶的核心简化：**用周期整除代替状态机维护**。
//!
//! # 设计原则
//!
//! - **刷新硬下限**：`REFRESH_FLOOR_MS` 是两次提交之间的最小间隔，0.67fps 硬上限，
//!   无论 EMA/死区/活动模式怎么判定，间隔不足 1500ms 一律不提交。
//! - **周期对齐**：所有源周期都是 1500 的约数，木桶天然是满的，
//!   不需要 `mark_settled` / 超时兜底 / `is_full` 断言。
//! - **EMA 平滑治本**：微振荡（70↔90）在 EMA 层被吸收，不构成"变化"。
//! - **死区防抖**：进入 3% / 退出 1%，防止临界点反复触发。
//! - **活动模式**：持续高负载时稳定 1.5s 节拍，既不追振荡也不降频。

/// 刷新硬下限：两次提交至少隔 1500ms（0.67fps 硬上限）。
///
/// 无论前三层防御怎么判定，间隔不足 1500ms 一律不提交。
/// 这是唯一需要改一处就能翻转方向的常量。
pub const REFRESH_FLOOR_MS: u64 = 1500;

/// 基础采样节拍（所有源周期为其约数）。
///
/// - Cpu / Mem：500ms（100ms → 500ms 改版对齐 1500）
/// - Net：      1500ms（对齐刷新节拍）
/// - Gpu：      2000ms（DXGI/D3DKMT 约 1s 才刷新）
pub const TICK_MS: u64 = 500;

/// Idle 兜底：无显著变化也至少这么久刷一次（保时钟/运行时间新鲜）。
pub const IDLE_MAX_MS: u64 = 4500;

/// EMA 平滑系数（越小越平滑，对振荡抑制越强）。
/// 0.3 意味着新值占 30% 权重，EMA 在 ~3 轮内追上真实值。
pub const EMA_ALPHA: f32 = 0.3;

/// 显著变化进入阈值（百分比）：EMA 与上次提交的差值 ≥ 此值时认为"有变化"。
pub const DELTA_ENTER: f32 = 3.0;

/// 显著变化退出阈值（百分比）：EMA 与上次提交的差值 < 此值时认为"无变化"。
pub const DELTA_EXIT: f32 = 1.0;

/// 连续无显著变化的节拍数（满足才退出 Active → Idle）。
pub const EXIT_TICKS: u32 = 3;

/// Active 水位阈值：CPU ≥ 此值时认为是"持续高负载"。
pub const ACTIVE_CPU_PCT: f32 = 30.0;

/// Active 水位阈值：网络 ≥ 此值时认为是"持续高负载"（1 MB/s）。
pub const ACTIVE_NET_BPS: f32 = 1_048_576.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_floor_is_1500() {
        assert_eq!(REFRESH_FLOOR_MS, 1500);
    }

    #[test]
    fn tick_is_divisor_of_refresh_floor() {
        assert!(REFRESH_FLOOR_MS % TICK_MS == 0);
    }

    #[test]
    fn idle_max_exceeds_refresh_floor() {
        assert!(IDLE_MAX_MS > REFRESH_FLOOR_MS);
    }
}
