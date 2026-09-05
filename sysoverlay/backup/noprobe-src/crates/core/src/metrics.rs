//! Pure functions for metric computation (no Win32 dependencies).
//!
//! All functions here are deterministic and unit-testable.

use crate::snapshot::Metric;

/// CPU usage calculation from idle/kernel/user time deltas.
///
/// `idle_delta`, `kernel_delta`, and `user_delta` are time differences
/// in 100-nanosecond units (same unit as `FILETIME`).
///
/// # `GetSystemTimes` 语义（关键）
///
/// Windows `GetSystemTimes` 的三个输出**不是**互相独立的三项：
///
/// - `lpKernelTime`（内核态）**已经包含** `lpIdleTime`（空闲）时间；
/// - `lpUserTime`（用户态）与 kernel 并列，不含 idle；
/// - 三者都是**所有核心的累加值**。
///
/// 因此：
///
/// ```text
/// total = kernel + user     // 全部核心的 CPU 时间总量
/// busy  = total - idle      // 真正干活的时间
/// usage = busy / total
/// ```
///
/// > 历史 bug：旧实现写成 `total = idle + kernel + user`，把已经包含在 kernel
/// > 里的 idle **又加了一遍**。旧式 `(k+u)/(i+k+u)` 恒大于正确的 `(k+u-i)/(k+u)`，
/// > 净效果是 **CPU% 系统性偏高** —— 实测 4 核 50% 负载会被算成 66.7%。
///
/// Returns CPU usage as a percentage (0.0..=100.0) clamped to sane bounds,
/// or `Metric::Unavailable` if the total delta is zero (avoid division by zero).
pub fn cpu_usage(idle_delta: u64, kernel_delta: u64, user_delta: u64) -> Metric<f32> {
    // kernel 已含 idle，所以总时间就是 kernel + user，不再额外加 idle。
    let total = kernel_delta.saturating_add(user_delta);
    if total == 0 {
        return Metric::Unavailable;
    }

    // 理论上有 idle <= total；计数异常（如系统时间回拨）时 saturating 归零。
    let busy = total.saturating_sub(idle_delta);

    let usage = (busy as f64) / (total as f64) * 100.0;
    let usage = usage.clamp(0.0, 100.0) as f32;
    Metric::Value(usage)
}

/// Calculate a per-second rate from a byte delta and elapsed time.
///
/// `byte_delta` is the difference in byte count since the last sample
/// (can be negative due to counter wrap-around on 32-bit interfaces).
///
/// `elapsed_ms` is the time elapsed since the last sample in milliseconds.
///
/// Returns `None` if:
/// - `elapsed_ms` is outside the expected sampling interval (200ms..5000ms)
/// - `byte_delta` is negative (indicates counter wrap-around that we can't correct)
pub fn rate_per_sec(byte_delta: i64, elapsed_ms: u64) -> Option<u64> {
    // Guard against unrealistic sampling intervals.
    if !(200..=5000).contains(&elapsed_ms) {
        return None;
    }
    // Negative byte_delta means counter wrapped; we cannot compute a rate.
    if byte_delta < 0 {
        return None;
    }

    // Convert bytes/ms to bytes/sec: multiply by 1000, divide by elapsed_ms.
    // Use f64 to avoid overflow for large byte counters.
    let rate = (byte_delta as f64) * 1000.0 / (elapsed_ms as f64);
    Some(rate as u64)
}

/// Exponential moving average.
///
/// `prev` is the previous EMA value (or seed).
/// `next` is the new raw observation.
/// `alpha` is the smoothing factor (0.0 < alpha <= 1.0); higher = more weight to new value.
pub fn ema(prev: f32, next: f32, alpha: f32) -> f32 {
    debug_assert!((0.0..=1.0).contains(&alpha), "alpha must be in range (0.0, 1.0]");
    prev + alpha * (next - prev)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── cpu_usage：GetSystemTimes 语义（kernel 已含 idle） ────────────

    /// 4 核机器、1 秒窗口、50% 负载：
    /// total(kernel+user) = 100，其中 idle = 50 → busy = 50 → 50%。
    #[test]
    fn test_cpu_usage_half_busy() {
        let m = cpu_usage(50, 60, 40);
        assert_eq!(m, Metric::Value(50.0));
    }

    /// 完全跑满：idle = 0，busy = total → 100%。
    #[test]
    fn test_cpu_usage_fully_busy() {
        let m = cpu_usage(0, 60, 40);
        assert_eq!(m, Metric::Value(100.0));
    }

    /// 完全空闲：所有时间都算进 kernel（idle 包含在 kernel 内），busy = 0。
    #[test]
    fn test_cpu_usage_fully_idle() {
        let m = cpu_usage(100, 100, 0);
        assert_eq!(m, Metric::Value(0.0));
    }

    /// 全部为零（没有经过任何时间）→ 不可用，而不是除零 panic。
    #[test]
    fn test_cpu_usage_unavailable_on_zero_total() {
        let m = cpu_usage(0, 0, 0);
        assert_eq!(m, Metric::Unavailable);
    }

    /// **回归测试**：这是此前那条 bug 的直接断言。
    ///
    /// 旧公式 `total = idle + kernel + user` 会把同一份数据算成
    /// `100 / 150 = 66.67%`，而正确答案是 50%。
    #[test]
    fn test_cpu_usage_does_not_double_count_idle() {
        let m = cpu_usage(50, 60, 40);
        let wrong = Metric::Value(66.666_664); // (60+40)/(50+60+40)
        assert_ne!(m, wrong, "kernel 已含 idle，不得再单独加一次 idle");
        assert_eq!(m, Metric::Value(50.0));
    }

    /// 计数异常（idle > total，例如系统时间回拨）时饱和到 0，不出现负数或 NaN。
    #[test]
    fn test_cpu_usage_idle_exceeds_total_saturates() {
        let m = cpu_usage(150, 100, 0);
        assert_eq!(m, Metric::Value(0.0));
    }

    /// 单核场景：kernel+user = 100，idle = 75 → 25%。
    #[test]
    fn test_cpu_usage_single_core_quarter() {
        let m = cpu_usage(75, 70, 30);
        assert_eq!(m, Metric::Value(25.0));
    }

    /// 多核真实量级：8 核 1 秒窗口 = 8×10^7 个 100ns 单位，其中 2 核满载。
    /// idle = 6e7, kernel+user = 8e7 → busy = 2e7 → 25%。
    #[test]
    fn test_cpu_usage_realistic_magnitude() {
        let unit_per_sec = 10_000_000u64;
        let total = 8 * unit_per_sec;
        let idle = 6 * unit_per_sec;
        let m = cpu_usage(idle, total - unit_per_sec, unit_per_sec);
        assert_eq!(m, Metric::Value(25.0));
    }

    /// 极小工作量的边界：busy = 1，total = 10000 → 0.01%（可表示，非 0）。
    #[test]
    fn test_cpu_usage_tiny_busy() {
        let m = cpu_usage(9_999, 10_000, 0);
        assert_eq!(m, Metric::Value(0.01));
    }

    // ── rate_per_sec ────────────────────────────────────────────────

    #[test]
    fn test_rate_per_sec_normal() {
        // 1000 bytes in 1000ms = 1000 bytes/sec
        assert_eq!(rate_per_sec(1000, 1000), Some(1000));

        // 1 MiB in 1000ms = 1 MiB/s = 1048576 bytes/sec
        assert_eq!(rate_per_sec(1048576, 1000), Some(1048576));
    }

    #[test]
    fn test_rate_per_sec_interval_rejected() {
        // Too fast (< 200ms)
        assert_eq!(rate_per_sec(1000, 100), None);
        // Too slow (> 5000ms)
        assert_eq!(rate_per_sec(1000, 6000), None);
    }

    #[test]
    fn test_rate_per_sec_negative_delta() {
        // Counter wrapped; should be rejected.
        assert_eq!(rate_per_sec(-100, 1000), None);
    }

    #[test]
    fn test_rate_per_sec_zero() {
        // Zero bytes is a valid rate (idle link).
        assert_eq!(rate_per_sec(0, 1000), Some(0));
    }

    // ── ema ─────────────────────────────────────────────────────────

    #[test]
    fn test_ema() {
        assert_eq!(ema(0.0, 10.0, 0.5), 5.0);
        assert_eq!(ema(5.0, 15.0, 0.5), 10.0);
        // Alpha=1.0 -> just return the new value.
        assert_eq!(ema(123.0, 456.0, 1.0), 456.0);
    }

    #[test]
    fn test_ema_alpha_zero_is_hold() {
        // alpha=0 → 完全保持旧值
        assert_eq!(ema(42.0, 999.0, 0.0), 42.0);
    }
}
