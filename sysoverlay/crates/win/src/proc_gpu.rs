//! GPU sampler placeholder.
//!
//! This is a placeholder implementation that returns Metric::Unavailable.
//! The real GPU sampler using PDH will be implemented in a later phase.

use sysoverlay_core::sampler::{SampleOutcome, Sampler};
use sysoverlay_core::snapshot::{Metric, Snapshot};

/// A placeholder GPU sampler that returns Unavailable.
///
/// GPU sampling requires PDH queries to the GPU Engine counters,
/// which requires more complex setup. This placeholder allows
/// the skeleton to compile without actual GPU support.
pub struct GpuSampler;

impl GpuSampler {
    /// Create a new GPU sampler.
    pub fn new() -> Self {
        Self
    }
}

impl Default for GpuSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler for GpuSampler {
    fn name(&self) -> &'static str {
        "gpu"
    }

    fn sample(&mut self, _prev: Option<&Snapshot>, _now_ms: u64) -> SampleOutcome {
        // GPU sampling not yet implemented
        // Real implementation will use PDH queries to \GPU Engine(*)\Utilization Percentage
        SampleOutcome::Transient("GPU sampling not yet implemented")
    }
}

/// Collect GPU metrics directly (for simple use cases).
///
/// Returns Metric::Unavailable since GPU sampling is not yet implemented.
pub fn collect_gpu() -> Metric<f32> {
    Metric::Unavailable
}
