//! Network sampler placeholder.
//!
//! This is a placeholder implementation that returns fixed test values.
//! The real network sampler using PDH will be implemented later.

use sysoverlay_core::sampler::{SampleOutcome, Sampler};
use sysoverlay_core::snapshot::{NetStat, Snapshot};

/// A placeholder network sampler that returns fixed values for testing.
pub struct NetSampler {
    /// Simulated network stats (bytes per second)
    net: NetStat,
}

impl NetSampler {
    /// Create a new network sampler with default test values.
    pub fn new() -> Self {
        Self {
            net: NetStat {
                rx_bps: 1024 * 1024,     // 1 MB/s
                tx_bps: 512 * 1024,     // 512 KB/s
            },
        }
    }
}

impl Default for NetSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler for NetSampler {
    fn name(&self) -> &'static str {
        "network"
    }

    fn sample(&mut self, _prev: Option<&Snapshot>, _now_ms: u64) -> SampleOutcome {
        // Placeholder: return fixed network values
        // Real implementation will use PDH queries to \Network Interface(*)\*
        SampleOutcome::Ok
    }
}

/// Collect network metrics directly (for simple use cases).
///
/// Returns dummy network statistics for testing purposes.
pub fn collect_network() -> NetStat {
    NetStat {
        rx_bps: 1024 * 1024,
        tx_bps: 512 * 1024,
    }
}
