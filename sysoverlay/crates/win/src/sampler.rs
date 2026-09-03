//! Mock Sampler implementation for testing.
//!
//! Provides a composite sampler that combines CPU, Memory, GPU, and Network sampling
/// using the placeholder implementations. This allows the skeleton to
/// compile and run with test data before real samplers are implemented.

use crate::proc_cpu::CpuSampler;
use crate::proc_gpu::GpuSampler;
use crate::proc_mem::MemSampler;
use crate::proc_net::NetSampler;
use sysoverlay_core::sampler::{SampleOutcome, Sampler};
use sysoverlay_core::snapshot::{Metric, NetStat, Snapshot};

/// A composite sampler that combines CPU, Memory, GPU, and Network sampling.
///
/// This is a placeholder implementation that gathers data from all
/// individual placeholder samplers. In the real implementation, these
/// would be running in a background thread with proper synchronization.
pub struct MockSampler {
    cpu: CpuSampler,
    mem: MemSampler,
    gpu: GpuSampler,
    net: NetSampler,
    seq: u64,
}

impl MockSampler {
    /// Create a new mock sampler with all placeholder subsamplers.
    pub fn new() -> Self {
        Self {
            cpu: CpuSampler::new(),
            mem: MemSampler::new(),
            gpu: GpuSampler::new(),
            net: NetSampler::new(),
            seq: 0,
        }
    }

    /// Collect a complete snapshot from all subsamplers.
    ///
    /// Returns a new Snapshot with data from all sources.
    pub fn collect(&mut self, prev: Option<&Snapshot>, now_ms: u64) -> Snapshot {
        // Sample all sources
        let _ = self.cpu.sample(prev, now_ms);
        let _ = self.mem.sample(prev, now_ms);
        let _ = self.gpu.sample(prev, now_ms);
        let _ = self.net.sample(prev, now_ms);

        self.seq += 1;

        Snapshot {
            seq: self.seq,
            timestamp_ms: now_ms,
            cpu: Metric::Value(50.0), // Placeholder
            memory: self.mem.get_memory_stat(),
            gpu: Metric::Unavailable, // GPU not implemented yet
            net: NetStat {
                rx_bps: 1024 * 1024,
                tx_bps: 512 * 1024,
            },
        }
    }
}

impl Default for MockSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler for MockSampler {
    fn name(&self) -> &'static str {
        "composite"
    }

    fn sample(&mut self, prev: Option<&Snapshot>, now_ms: u64) -> SampleOutcome {
        // Sample all subsamplers
        let cpu_out = self.cpu.sample(prev, now_ms);
        let mem_out = self.mem.sample(prev, now_ms);
        let gpu_out = self.gpu.sample(prev, now_ms);
        let net_out = self.net.sample(prev, now_ms);

        // Return worst-case outcome
        if let SampleOutcome::Fatal(msg) = cpu_out {
            return SampleOutcome::Fatal(msg);
        }
        if let SampleOutcome::Fatal(msg) = mem_out {
            return SampleOutcome::Fatal(msg);
        }
        if let SampleOutcome::Fatal(msg) = gpu_out {
            return SampleOutcome::Fatal(msg);
        }
        if let SampleOutcome::Fatal(msg) = net_out {
            return SampleOutcome::Fatal(msg);
        }

        if let SampleOutcome::Transient(msg) = gpu_out {
            return SampleOutcome::Transient(msg);
        }

        SampleOutcome::Ok
    }
}
