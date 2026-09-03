//! Memory sampler placeholder.
//!
//! This is a placeholder implementation that returns fixed test values.
//! The real memory sampler using GlobalMemoryStatusEx will be implemented later.

use sysoverlay_core::sampler::{SampleOutcome, Sampler};
use sysoverlay_core::snapshot::{MemoryStat, Snapshot};

/// A placeholder memory sampler that returns fixed values for testing.
pub struct MemSampler {
    /// Simulated memory usage
    mem: MemoryStat,
}

impl MemSampler {
    /// Create a new memory sampler with default test values.
    pub fn new() -> Self {
        Self {
            mem: MemoryStat {
                load_pct: 65.0,
                used_gb: 8.0,
                total_gb: 16.0,
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
        "memory"
    }

    fn sample(&mut self, _prev: Option<&Snapshot>, _now_ms: u64) -> SampleOutcome {
        // Placeholder: return fixed memory values
        // Real implementation will use GlobalMemoryStatusEx
        SampleOutcome::Ok
    }
}

/// Collect memory metrics directly (for simple use cases).
///
/// Returns dummy memory statistics for testing purposes.
pub fn collect_memory() -> MemoryStat {
    MemoryStat {
        load_pct: 65.0,
        used_gb: 8.0,
        total_gb: 16.0,
    }
}
