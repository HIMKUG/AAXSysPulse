//! Memory sampler placeholder.

use sysoverlay_core::sampler::{SampleOutcome, Sampler};
use sysoverlay_core::snapshot::{MemoryStat, Snapshot};

/// Memory sampler.
pub struct MemSampler {
    mem: MemoryStat,
}

impl MemSampler {
    /// Create a new memory sampler.
    pub fn new() -> Self {
        Self {
            mem: MemoryStat {
                load_pct: 0.0,
                used_gb: 0.0,
                total_gb: 0.0,
            },
        }
    }

    /// Get current memory statistics.
    pub fn get_memory_stat(&self) -> MemoryStat {
        self.mem
    }
}

impl Default for MemSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler for MemSampler {
    fn name(&self) -> &'static str {
        "Memory"
    }

    fn sample(&mut self, _prev: Option<&Snapshot>, _now_ms: u64) -> SampleOutcome {
        // TODO: Implement with GlobalMemoryStatusEx
        self.mem = MemoryStat {
            load_pct: 50.0,
            used_gb: 8.0,
            total_gb: 16.0,
        };
        SampleOutcome::Ok
    }
}
