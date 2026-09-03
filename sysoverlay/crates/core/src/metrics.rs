//! Pure functions for metric computation (no Win32 dependencies).
//!
//! All functions here are deterministic and unit-testable.

use crate::snapshot::Metric;

/// CPU usage calculation from idle/kernel/user time deltas.
///
/// `idle_delta`, `kernel_delta`, and `user_delta` are time differences
/// in 100-nanosecond units (same unit as `FILETIME`).
///
/// Returns CPU usage as a percentage (0.0..=100.0) clamped to sane bounds,
/// or `Metric::Unavailable` if the deltas are all zero (avoid division by zero).
pub fn cpu_usage(idle_delta: u64, kernel_delta: u64, user_delta: u64) -> Metric<f32> {
    let total_time = idle_delta.saturating_add(kernel_delta).saturating_add(user_delta);
    if total_time == 0 {
        return Metric::Unavailable;
    }

    let busy_time = kernel_delta.saturating_add(user_delta);
    // CPU usage = fraction of total time that was busy (not idle)
    let usage = (busy_time as f64) / (total_time as f64) * 100.0;
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

/// GPU engine node key extraction.
///
/// PDH instance names for GPU engines have the form:
/// `luid_0x00000000_0x0001D3A5_phys_0_eng_0_engtype_3D_pid_1234`
///
/// This function extracts the node key portion (from `luid_` to just before `_pid_`)
/// which groups all per-process engine usage by physical node.
///
/// Returns `None` if the string does not contain `luid_` or `pid_`.
pub fn node_key(instance: &str) -> Option<&str> {
    let luid_start = instance.find("luid_")?;
    let pid_start = instance.find("_pid_")?;

    // node key spans from luid_ start to just before _pid_
    let end = pid_start;
    if luid_start >= end {
        return None;
    }
    Some(&instance[luid_start..end])
}

/// Aggregate GPU engine nodes: sum utilization per node and return the maximum.
///
/// `instances` is an iterator of `(instance_name, utilization)` pairs.
///
/// Returns the utilization of the busiest node, or `None` if there are no instances.
pub fn aggregate_gpu_nodes<'a>(instances: impl Iterator<Item = (&'a str, f32)>) -> Option<f32> {
    // Group by node key and sum.
    let mut node_totals: Vec<(&'a str, f32)> = Vec::new();

    for (name, util) in instances {
        let key = node_key(name)?;
        // Clamp negative values to zero (some PDH counters can return spurious negatives).
        let util = util.max(0.0);

        if let Some(entry) = node_totals.iter_mut().find(|e| e.0 == key) {
            entry.1 += util;
        } else {
            node_totals.push((key, util));
        }
    }

    node_totals.into_iter().map(|(_, total)| total).fold(None, |max, v| {
        Some(max.map_or(v, |m| v.max(m)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_usage_normal() {
        // 50% usage: 50 units idle out of 100 total (kernel+user=50)
        let m = cpu_usage(50, 25, 25);
        assert_eq!(m, Metric::Value(50.0));

        // 100% usage: no idle time (idle=0, kernel+user=100)
        let m = cpu_usage(0, 50, 50);
        assert_eq!(m, Metric::Value(100.0));

        // 0% usage: all idle (idle=100, kernel=0, user=0)
        let m = cpu_usage(100, 0, 0);
        assert_eq!(m, Metric::Value(0.0));
    }

    #[test]
    fn test_cpu_usage_clamp() {
        // Edge case: very small busy time
        let m = cpu_usage(0, 1, 0);
        assert_eq!(m, Metric::Value(100.0));
    }

    #[test]
    fn test_cpu_usage_unavailable() {
        // All zeros -> unavailable (no time elapsed).
        let m = cpu_usage(0, 0, 0);
        assert_eq!(m, Metric::Unavailable);
    }

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

    #[test]
    fn test_ema() {
        assert_eq!(ema(0.0, 10.0, 0.5), 5.0);
        assert_eq!(ema(5.0, 15.0, 0.5), 10.0);
        // Alpha=1.0 -> just return the new value.
        assert_eq!(ema(123.0, 456.0, 1.0), 456.0);
    }

    #[test]
    fn test_node_key() {
        let name = "luid_0x00000000_0x0001D3A5_phys_0_eng_0_engtype_3D_pid_1234";
        assert_eq!(node_key(name), Some("luid_0x00000000_0x0001D3A5_phys_0_eng_0_engtype_3D"));
    }

    #[test]
    fn test_node_key_no_luid() {
        let name = "invalid_instance_name";
        assert_eq!(node_key(name), None);
    }

    #[test]
    fn test_node_key_no_pid() {
        let name = "luid_0x1234_phys_0_eng_0";
        assert_eq!(node_key(name), None);
    }

    #[test]
    fn test_aggregate_gpu_nodes_single() {
        let instances = [("luid_0x1_phys_0_eng_0_engtype_3D_pid_100", 50.0)];
        assert_eq!(aggregate_gpu_nodes(instances.into_iter()), Some(50.0));
    }

    #[test]
    fn test_aggregate_gpu_nodes_multi_same_node() {
        // Two processes on the same node.
        let instances = [
            ("luid_0x1_phys_0_eng_0_engtype_3D_pid_100", 30.0),
            ("luid_0x1_phys_0_eng_0_engtype_3D_pid_200", 20.0),
        ];
        assert_eq!(aggregate_gpu_nodes(instances.into_iter()), Some(50.0));
    }

    #[test]
    fn test_aggregate_gpu_nodes_different_nodes() {
        let instances = [
            ("luid_0x1_phys_0_eng_0_engtype_3D_pid_100", 30.0),
            ("luid_0x2_phys_0_eng_0_engtype_3D_pid_100", 50.0),
        ];
        // Should return the max node total (50.0), not the sum.
        assert_eq!(aggregate_gpu_nodes(instances.into_iter()), Some(50.0));
    }

    #[test]
    fn test_aggregate_gpu_nodes_negative_clamped() {
        let instances = [("luid_0x1_phys_0_eng_0_engtype_3D_pid_100", -10.0)];
        // Negative should be clamped to 0.
        assert_eq!(aggregate_gpu_nodes(instances.into_iter()), Some(0.0));
    }

    #[test]
    fn test_aggregate_gpu_nodes_empty() {
        let instances: [(&str, f32); 0] = [];
        assert_eq!(aggregate_gpu_nodes(instances.into_iter()), None);
    }
}
