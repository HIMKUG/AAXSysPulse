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
use aaxsyspulse_core::sampler::{Backoff, SampleOutcome, Sampler, SamplerDiag, SamplerId};
use aaxsyspulse_core::scheduler::SamplePlan;
use aaxsyspulse_core::snapshot::{Metric, Snapshot};

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

    /// Net 上次采样时间（低频档分频用，v3 性能档）。
    last_net_ms: u64,
}

/// Net 低频档采样间隔（ms）：速率按真实 Δt 计算，拉长间隔只影响粒度不失真。
const NET_LOW_FREQ_MS: u64 = 2000;

impl SystemSampler {
    /// `exclude_virtual` controls whether loopback/Hyper-V/WSL2 nics are counted.
    ///
    /// Power 的分频已上收至调度器（`Source::Power` + 用户配置的独立间隔），
    /// 采样器只按 `plan.power` 执行。
    pub fn new(exclude_virtual: bool) -> Self {
        Self {
            cpu: CpuSampler::new(),
            mem: MemSampler::new(),
            gpu: GpuSampler::new(),
            net: NetSampler::new(exclude_virtual),
            syspower: SystemPower::new(),
            diag: SamplerDiag::new(),
            backoff: [Backoff::default(); 5],
            seq: 0,
            last_net_ms: 0,
        }
    }

    /// 监控项启停转换钩子（v3「关闭即完全停止」的执行端）。
    ///
    /// 禁用 → 释放该源的后端资源（句柄/内核服务/厂商库/拓扑缓存）；
    /// 启用 → 重置采样基线，避免跨大间隔的陈旧 delta 造成数值毛刺。
    /// 由采样线程在检测到 `schedule_cfg()` 掩码变化时调用（单线程所有权，无锁）。
    pub fn on_schedule_change(&mut self, prev: &[bool; 5], cur: &[bool; 5]) {
        // Power 行关：关闭 MSR 设备句柄并停止内核驱动服务（真正的服务级停止）。
        // 重开本会话不热恢复（teardown 一次性）→ 采样返回 None → 行显示 `--`；
        // 设置页在关闭瞬间已弹原生提示框告知「重启应用恢复」。
        if prev[4] && !cur[4] {
            crate::msr::teardown();
            crate::power_system::store_cpu_watts(None);
            self.backoff[SamplerId::Power as usize].reset();
            self.diag.push(aaxsyspulse_core::sampler::DiagEntry {
                sampler: SamplerId::Power,
                outcome: SampleOutcome::Skipped,
                code: 0,
                seq: self.seq as u32,
            });
            // GPU 行也关着 → 厂商库已无使用者（它只为功耗行的瓦数而继续留存），一并卸载。
            if !cur[2] {
                crate::gpu_vendor::teardown();
                self.syspower.set_gpu_watts(None);
            }
        }
        // GPU 行关：失效 D3DKMT 拓扑缓存；厂商库（NVML/ADL）**只在功耗行也关时**卸载
        // —— 功耗行仍需要 GPU 实测瓦数（整机功耗 = CPU + GPU + …），
        //    "不显示占用率"不等于"不测显卡功耗"。
        // 重开：D3DKMT 查询无状态、拓扑缓存自动重建；厂商库被卸载后本会话不热恢复（走 D3DKMT 兜底）。
        if prev[2] && !cur[2] {
            if !cur[4] {
                crate::gpu_vendor::teardown();
                // 厂商瓦数随后端卸载而失效：清掉陈旧读数，避免整机功耗冻结在旧值上。
                self.syspower.set_gpu_watts(None);
            }
            crate::proc_gpu::invalidate_topology();
        }
        // Net 重开：重置字节基线（速率 = Δbytes/Δt，跨大间隔必毛刺）。
        if !prev[3] && cur[3] {
            self.net.reset_baselines();
        }
        // CPU 重开：重置 GetSystemTimes 前值。
        if !prev[0] && cur[0] {
            self.cpu.reset_baseline();
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
        //
        // ⚠️ 采样门 = GPU 行 ∨ 功耗行（用户拍板：功耗开着就要拿到完整数据）。
        // 整机功耗 = CPU实测 + GPU实测 + …，所以「GPU 行关」只代表**不显示占用率**，
        // 不能连显卡瓦数一起停掉——那样整机功耗会静默漏掉整个显卡项。
        // GPU 行关闭时：照常跑厂商采样取瓦数，但不回写 snap.gpu、不跑 D3DKMT 兜底。
        if plan.gpu || plan.power {
            let t = crate::perf::begin();
            let (v_util, v_watts) = gpu_vendor::sample();
            // 瓦数无条件发布：功耗行（plan.power）依赖它
            self.syspower.set_gpu_watts(v_watts);
            if plan.gpu {
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
                // 功耗行专供：只要瓦数，不要占用率（也不付 D3DKMT 那 74ms）
                crate::perf::note_section("sample_gpu", t);
                self.push_diag(SamplerId::Gpu, SampleOutcome::Skipped, 0);
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

        // Power：整机功耗（用户拍板公式），分频由调度器 `Source::Power` 驱动；
        // CPU 实测不可用（无管理员/无驱动）→ 整行关闭（Unavailable = 不显示不计算）
        if plan.power {
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
            // 查询被禁用（行开关关 / 性能页 power_query 关 / teardown 后重开）：
            // 显式置 Unavailable，覆盖 prev 带来的旧值 → 功耗行显示 `--` 而非冻结的旧瓦数。
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

        self.diag.push(aaxsyspulse_core::sampler::DiagEntry {
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
        Self::new(true)
    }
}
