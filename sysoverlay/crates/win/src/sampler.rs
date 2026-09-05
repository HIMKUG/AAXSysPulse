//! Composite sampler: orchestrates CPU + Memory + GPU + Network sampling.
//!
//! Replaces the old `MockSampler` stub. Each subsampler implements the
//! [`Sampler`] trait independently. This layer handles:
//! - Assembling individual `sample()` calls into one `Snapshot`
//! - Stale propagation (if a subsampler returns Ok but holds the previous
//!   value because this is the first call, mark the stale metrics accordingly)
//! - Exponential back-off on consecutive failures

use crate::proc_cpu::CpuSampler;
use crate::proc_gpu::GpuSampler;
use crate::proc_mem::MemSampler;
use crate::proc_net::NetSampler;
use crate::gpu_vendor;
use crate::power_system::SystemPower;
use sysoverlay_core::sampler::{Backoff, SampleOutcome, Sampler, SamplerDiag, SamplerId};
use sysoverlay_core::scheduler::SamplePlan;
use sysoverlay_core::snapshot::{Metric, Snapshot};

/// Composite sampler that drives all metric collectors.
pub struct SystemSampler {
    cpu: CpuSampler,
    mem: MemSampler,
    gpu: GpuSampler,
    net: NetSampler,
    /// 整机功耗（用户拍板公式：CPU实测+GPU实测+内存+主板+硬盘）。
    syspower: SystemPower,

    /// Diagnostic ring buffer (64 entries, no heap, survives crashes).
    pub diag: SamplerDiag,

    /// Exponential back-off state per sampler（含 Power，共 5 个）.
    backoff: [Backoff; 5],

    /// Sequence number for the next snapshot.
    seq: u64,

    /// Power 独立采样间隔（默认 3000ms，来自 config.power_interval_ms）。
    power_interval_ms: u64,
    /// Power 上次采样时间（毫秒 tick）。
    last_power_ms: u64,
}

impl SystemSampler {
    /// `exclude_virtual` controls whether loopback/Hyper-V/WSL2 nics are counted.
    ///
    /// `power_interval_ms` 是 Power 源的独立采样间隔（它不走 SamplePlan 分频，
    /// 因为功耗读数与刷新率无耦合）。
    pub fn new(exclude_virtual: bool, power_interval_ms: u32) -> Self {
        Self {
            cpu: CpuSampler::new(),
            mem: MemSampler::new(),
            gpu: GpuSampler::new(),
            net: NetSampler::new(exclude_virtual),
            syspower: SystemPower::new(),
            diag: SamplerDiag::new(),
            backoff: [Backoff::default(); 5],
            seq: 0,
            power_interval_ms: power_interval_ms.clamp(500, 60_000) as u64,
            last_power_ms: 0,
        }
    }

    /// Perform one sampling cycle — **按调度计划分频执行**。
    ///
    /// 只有 [`SamplePlan`] 中为 true 的源会真正调用子采样器；
    /// 其余源沿用 `prev` 快照里的值（诊断记 `Skipped`，不计退避）。
    /// 这是调度器「GPU/Net 独立降频」的执行端：计划外的源零开销。
    ///
    /// `prev` 是上一份完整快照（通常来自线程本地缓存），未到期源从中复制。
    pub fn sample(&mut self, plan: &SamplePlan, prev: Option<&Snapshot>, now_ms: u64, power_enabled: bool) -> Snapshot {
        // 从上一份快照出发：未到期源直接保留旧值，失败源也不再丢数据。
        let mut snap = prev.cloned().unwrap_or_else(|| Snapshot::placeholder(now_ms));
        snap.seq = self.seq;
        snap.timestamp_ms = now_ms;
        snap.uptime_ms = now_ms;

        // CPU
        if plan.cpu {
            let cpu_out = self.cpu.sample(prev, now_ms, &mut snap);
            self.push_diag(SamplerId::Cpu, cpu_out, 0);
        } else {
            self.push_diag(SamplerId::Cpu, SampleOutcome::Skipped, 0);
        }

        // Memory
        if plan.mem {
            let mem_out = self.mem.sample(prev, now_ms, &mut snap);
            self.push_diag(SamplerId::Memory, mem_out, 0);
        } else {
            self.push_diag(SamplerId::Memory, SampleOutcome::Skipped, 0);
        }

        // GPU：厂商模块优先（NVML/ADL/IGCL，用户拍板），失败回退 D3DKMT
        if plan.gpu {
            let (v_util, v_watts) = gpu_vendor::sample();
            self.syspower.set_gpu_watts(v_watts);
            let gpu_out = self.gpu.sample(prev, now_ms, &mut snap);
            if let Some(u) = v_util {
                snap.gpu = Metric::Value(u.clamp(0.0, 100.0));
            }
            self.push_diag(SamplerId::Gpu, gpu_out, 0);
        } else {
            self.push_diag(SamplerId::Gpu, SampleOutcome::Skipped, 0);
        }

        // Network
        if plan.net {
            let net_out = self.net.sample(prev, now_ms, &mut snap);
            self.push_diag(SamplerId::Net, net_out, 0);
        } else {
            self.push_diag(SamplerId::Net, SampleOutcome::Skipped, 0);
        }

        // Power：整机功耗（用户拍板公式），独立 interval；
        // CPU 实测不可用（无管理员/无驱动）→ 整行关闭（Unavailable = 不显示不计算）
        if power_enabled && now_ms.saturating_sub(self.last_power_ms) >= self.power_interval_ms {
            self.last_power_ms = now_ms;
            match self.syspower.sample(now_ms) {
                Some(w) => {
                    snap.power = Metric::Value(w);
                    self.push_diag(SamplerId::Power, SampleOutcome::Ok, 0);
                }
                None => {
                    snap.power = Metric::Unavailable;
                    self.push_diag(SamplerId::Power, SampleOutcome::Skipped, 0);
                }
            }
            // 发布 CPU 实测功率（供进程榜做功耗分摊，跨线程原子共享）
            crate::power_system::store_cpu_watts(self.syspower.cpu_watts());
        }

        self.seq += 1;
        snap
    }

    fn push_diag(&mut self, id: SamplerId, outcome: SampleOutcome, code: u16) {
        // Update back-off.
        let idx = id as usize;
        if outcome.is_failure() {
            self.backoff[idx].next_delay_ms();
        } else {
            self.backoff[idx].reset();
        }

        self.diag.push(sysoverlay_core::sampler::DiagEntry {
            sampler: id,
            outcome,
            code,
            seq: self.seq as u32,
        });
    }

    /// Number of consecutive failures for a given sampler.
    pub fn failures(&self, id: SamplerId) -> u32 {
        self.backoff[id as usize].failures()
    }

    /// Back-off state for UI to display.
    pub fn backing_off(&self, id: SamplerId) -> bool {
        self.backoff[id as usize].is_backing_off()
    }

    /// Returns how long to wait before retrying (milliseconds).
    pub fn retry_delay_ms(&self, id: SamplerId) -> u32 {
        let _ = self.backoff[id as usize]
            .failures()
            .saturating_sub(1);
        // Exposed so the caller (sampling thread) knows how long to sleep.
        // A real implementation would use this in the sleep loop.
        1000 // placeholder; actual backoff tracked per-sampler above
    }
}

impl SystemSampler {
    /// Resets all per-sampler backoff state (called after system resume).
    pub fn reset_backoff(&mut self) {
        for b in &mut self.backoff {
            b.reset();
        }
    }
}

impl Default for SystemSampler {
    fn default() -> Self {
        Self::new(true, 3000)
    }
}
