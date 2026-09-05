//! GPU sampler using D3DKMTQueryStatistics (WDDM 2.0+).
//!
//! Falls back to `Metric::Unavailable` if the driver does not support the API.
//! Per Q12 decision: PDH is removed entirely; D3DKMT is the sole GPU source.

use sysoverlay_core::sampler::{SampleOutcome, Sampler, SamplerId};
use sysoverlay_core::snapshot::{Metric, Snapshot};

/// GPU sampler using D3DKMTQueryStatistics.
pub struct GpuSampler {
    available: bool,
}

impl GpuSampler {
    pub fn new() -> Self {
        Self { available: true }
    }

    /// Always returns a sampler; first sample probes driver and degrades honestly.
    pub fn available() -> Self {
        Self { available: true }
    }

    fn try_sample_d3dkmt(&mut self, out: &mut Snapshot) -> SampleOutcome {
        // TODO: D3DKMTQueryStatistics via windows-rs bindings.
        // Expected flow:
        //   1. D3DKMTOpenAdapterFromLuid(luid) → adapter handle
        //   2. D3DKMTQueryStatistics(handle, type=EngineUsage) → utilization
        //   3. D3DKMTCloseAdapter(handle)
        //   4. Map AggregateVideoMemoryUsagePercentage → f32
        //
        // On STATUS_INVALID_PARAMETER (legacy WDDM < 2.0): degrade honestly.
        if !self.available {
            out.gpu = Metric::Unavailable;
            return SampleOutcome::Skipped;
        }
        out.gpu = Metric::Unavailable;
        SampleOutcome::Skipped
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

    fn id(&self) -> SamplerId {
        SamplerId::Gpu
    }

    fn sample(&mut self, _prev: Option<&Snapshot>, _now_ms: u64, out: &mut Snapshot) -> SampleOutcome {
        self.try_sample_d3dkmt(out)
    }
}
