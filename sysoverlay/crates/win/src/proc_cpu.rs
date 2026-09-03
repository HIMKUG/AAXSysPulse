//! CPU sampler placeholder.
//!
//! This is a placeholder implementation that returns fixed test values.
//! The real CPU sampler using PDH will be implemented in a later phase.

use sysoverlay_core::metrics::cpu_usage;
use sysoverlay_core::sampler::{SampleOutcome, Sampler};
use sysoverlay_core::snapshot::{Metric, Snapshot};

/// A placeholder CPU sampler that returns fixed values for testing.
pub struct CpuSampler {
    /// Simulated CPU usage (0.0..=100.0)
    cpu: f32,
}

impl CpuSampler {
    /// Create a new CPU sampler with default test values.
    pub fn new() -> Self {
        Self { cpu: 50.0 }
    }
}

impl Default for CpuSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler for CpuSampler {
    fn name(&self) -> &'static str {
        "cpu"
    }

    fn sample(&mut self, _prev: Option<&Snapshot>, _now_ms: u64) -> SampleOutcome {
        // Placeholder: return fixed 50% CPU usage
        // Real implementation will use GetSystemTimes + PDH
        self.cpu = if self.cpu > 80.0 {
            30.0
        } else if self.cpu < 30.0 {
            70.0
        } else {
            self.cpu + 5.0
        };

        SampleOutcome::Ok
    }
}

/// Collect CPU metrics directly (for simple use cases).
///
/// Returns a dummy CPU metric for testing purposes.
pub fn collect_cpu() -> Metric<f32> {
    Metric::Value(50.0)
}
