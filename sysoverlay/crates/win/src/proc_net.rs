//! Network sampler placeholder.
//!
//! Network sampling using Windows API.

use sysoverlay_core::sampler::{SampleOutcome, Sampler};
use sysoverlay_core::snapshot::{NetStat, Snapshot};

/// Network sampler.
pub struct NetSampler {
    net: NetStat,
}

impl NetSampler {
    /// Create a new network sampler.
    pub fn new() -> Self {
        Self {
            net: NetStat::default(),
        }
    }

    /// Get current network statistics.
    pub fn get_net_stat(&self) -> NetStat {
        self.net
    }

    /// Reset all baselines.
    pub fn reset_baselines(&mut self) {
        // No-op for placeholder
    }
}

impl Default for NetSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler for NetSampler {
    fn name(&self) -> &'static str {
        "Network"
    }

    fn sample(&mut self, _prev: Option<&Snapshot>, _now_ms: u64) -> SampleOutcome {
        // Placeholder - real implementation would use GetIfTable2 or PDH
        self.net = NetStat {
            rx_bps: 0,
            tx_bps: 0,
        };
        SampleOutcome::Ok
    }
}
