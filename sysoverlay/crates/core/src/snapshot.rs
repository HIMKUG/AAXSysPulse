//! Snapshot: immutable result of a single sampling cycle.
//!
//! All fields are scalar types, so `Clone` is cheap (shallow copy).

/// A complete sampling result, immutable once produced.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// Monotonically increasing sequence number.
    pub seq: u64,
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: u64,
    /// CPU utilization percentage (0.0..=100.0).
    pub cpu: Metric<f32>,
    /// Memory statistics.
    pub memory: MemoryStat,
    /// GPU utilization percentage (0.0..=100.0).
    pub gpu: Metric<f32>,
    /// Network statistics (bytes per second).
    pub net: NetStat,
}

/// A metric value with optional staleness information.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Metric<T> {
    /// Fresh value.
    Value(T),
    /// Value is stale: we are still showing `value` but it hasn't been updated
    /// for `stale_for` sampling cycles.
    Stale { value: T, stale_for: u32 },
    /// No data available (sensor unavailable, etc.).
    Unavailable,
}

impl<T: Copy> Metric<T> {
    /// Returns the value if present, otherwise `None`.
    pub fn value(&self) -> Option<T> {
        match self {
            Metric::Value(v) => Some(*v),
            Metric::Stale { value, .. } => Some(*value),
            Metric::Unavailable => None,
        }
    }

    /// Maps the inner value through `f`, preserving staleness.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Metric<U> {
        match self {
            Metric::Value(v) => Metric::Value(f(v)),
            Metric::Stale { value, stale_for } => Metric::Stale {
                value: f(value),
                stale_for,
            },
            Metric::Unavailable => Metric::Unavailable,
        }
    }
}

/// Memory usage statistics.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct MemoryStat {
    /// Memory load percentage (0.0..=100.0).
    pub load_pct: f32,
    /// Used memory in GiB.
    pub used_gb: f32,
    /// Total physical memory in GiB.
    pub total_gb: f32,
}

/// Network traffic statistics (bytes per second).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct NetStat {
    /// Received bytes per second.
    pub rx_bps: u64,
    /// Transmitted bytes per second.
    pub tx_bps: u64,
}
