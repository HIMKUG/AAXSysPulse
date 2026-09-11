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
    /// Net 上次采样时间（低频档分频用，v3 性能档）。
    last_net_ms: u64,
}

/// Net 低频档采样间隔（ms）：速率按真实 Δt 计算，拉长间隔只影响粒度不失真。
const NET_LOW_FREQ_MS: u64 = 2000;

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
            last_net_ms: 0,
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
            let t = crate::perf::begin();
            let cpu_out = self.cpu.sample(prev, now_ms, &mut snap);
            crate::perf::note_section("sample_cpu", t);
            self.push_diag(SamplerId::Cpu, cpu_out, 0);
        } else {
            self.push_diag(SamplerId::Cpu, SampleOutcome::Skipped, 0);
        }

        // Memory
        if plan.mem {
            let t = crate::perf::begin();
            let mem_out = self.mem.sample(prev, now_ms, &mut snap);
            crate::perf::note_section("sample_mem", t);
            self.push_diag(SamplerId::Memory, mem_out, 0);
        } else {
            self.push_diag(SamplerId::Memory, SampleOutcome::Skipped, 0);
        }

        // GPU：厂商模块优先（NVML/ADL/IGCL，用户拍板），成功时直接用，跳过 D3DKMT（省约 74ms）。
        // vendor 失败则 D3DKMT 补充（首次或恢复时用一次，后续由 vendor 缓存覆盖）。
        if plan.gpu {
            let t = crate::perf::begin();
            let (v_util, v_watts) = gpu_vendor::sample();
            self.syspower.set_gpu_watts(v_watts);
            // vendor 有值：写入快照并缓存，短路 D3DKMT 调用
            if let Some(u) = v_util {
                gpu_vendor::set_cached_util(u);
                snap.gpu = Metric::Value(u.clamp(0.0, 100.0));
                crate::perf::note_section("sample_gpu", t);
                self.push_diag(SamplerId::Gpu, SampleOutcome::Ok, 0);
            } else {
                // vendor 不可用：从缓存恢复；若缓存也无则用 D3DKMT
                let gpu_out = self.gpu.sample(prev, now_ms, &mut snap);
                crate::perf::note_section("sample_gpu", t);
                if let Some(u) = gpu_vendor::cached_util() {
                    snap.gpu = Metric::Value(u.clamp(0.0, 100.0));
                }
                self.push_diag(SamplerId::Gpu, gpu_out, 0);
            }
        } else {
            self.push_diag(SamplerId::Gpu, SampleOutcome::Skipped, 0);
        }

        // Network：低频档（v3 性能档，默认开）独立 2s 分频；
        // 速率 = Δbytes/Δms（真实间隔），拉长间隔只降粒度不失真。
        // 内部已改为「拓扑缓存 + GetIfEntry2 增量查询」，每周期仅查缓存的活跃 NIC
        // （本机实测 5 个，含 Hyper-V 虚拟交换机，<0.5ms），仅首次/全部掉线/10s 安全间隔
        // 才跑一次 GetIfTable2（~28ms）。
        let net_due = !crate::window::schedule_net_low()
            || now_ms.saturating_sub(self.last_net_ms) >= NET_LOW_FREQ_MS;
        if plan.net && net_due {
            self.last_net_ms = now_ms;
            let t = crate::perf::begin();
            let net_out = self.net.sample(prev, now_ms, &mut snap);
            crate::perf::note_section("sample_net", t);
            self.push_diag(SamplerId::Net, net_out, 0);
        } else {
            self.push_diag(SamplerId::Net, SampleOutcome::Skipped, 0);
        }

        // Power：整机功耗（用户拍板公式），独立 interval；
        // CPU 实测不可用（无管理员/无驱动）→ 整行关闭（Unavailable = 不显示不计算）
        if power_enabled && now_ms.saturating_sub(self.last_power_ms) >= self.power_interval_ms {
            self.last_power_ms = now_ms;
            let t = crate::perf::begin();
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
            crate::perf::note_section("sample_power", t);
            // 发布 CPU 实测功率（供进程榜做功耗分摊，跨线程原子共享）
            crate::power_system::store_cpu_watts(self.syspower.cpu_watts());
        } else if !power_enabled {
            // 查询被禁用（性能页 power_query 关 / 行开关关）：显式置 Unavailable，
            // 覆盖 prev 带来的旧值 → 功耗行显示 `--` 而非冻结的旧瓦数。
            snap.power = Metric::Unavailable;
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
