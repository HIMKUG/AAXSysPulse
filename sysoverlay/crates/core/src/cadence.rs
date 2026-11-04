//! 智能调度节拍常量。
//!
//! 多数采样周期取 1500ms 的约数（GPU 例外为 2000ms），保证节拍点各源都有新值。
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
/// 由 app/main.rs 的提交门使用；与 scheduler 的 IDLE_INTERVAL_MS（1s，全源
/// 停用时的唤醒心跳）是两个不同机制，勿合并（审查 094：已核实语义不同）。
pub const IDLE_MAX_MS: u64 = 4500;

/// EMA 平滑系数（越小越平滑，对振荡抑制越强）。
/// 0.3 意味着新值占 30% 权重，EMA 在 ~3 轮内追上真实值。
pub const EMA_ALPHA: f32 = 0.3;

// 审查 097：编译期关系断言放在模块顶层（普通构建也生效）；原先写在
// #[cfg(test)] 测试体内的 const 断言在 release 构建里根本不存在。
const _: () = assert!(REFRESH_FLOOR_MS.is_multiple_of(TICK_MS));
const _: () = assert!(IDLE_MAX_MS > REFRESH_FLOOR_MS);

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

    /// 审查 622：`assert_eq!(常量, 1500)` 这类恒真式已删除，常量间关系
    /// 由模块顶层的编译期断言把关；这里只断言派生量纲。
    #[test]
    fn constants_derive_sensibly() {
        assert!(EMA_ALPHA > 0.0 && EMA_ALPHA <= 1.0);
        assert!(TICK_MS > 0);
        assert!(REFRESH_FLOOR_MS % TICK_MS == 0);
    }
}
