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
use aaxsyspulse_core::composite::{Meter, MeterConfig};
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

    /// 综合占用（Util）的时间维度计：非对称 EMA + 卡顿指数。
    /// 状态归采样线程所有（单线程所有权，无锁）。
    util_meter: Meter,
    /// 上次 Util 更新的时间戳（毫秒）；None = 尚未更新过（首帧 dt 视为非法 → 全信读数）。
    last_util_ms: Option<u64>,

    /// Diagnostic ring buffer (64 entries, no heap, survives crashes).
    pub diag: SamplerDiag,

    /// Exponential back-off state per sampler（含 Power，共 5 个）.
    backoff: [Backoff; 5],

    /// Sequence number for the next snapshot.
    seq: u64,

    /// Net 上次采样时间（低频档分频用，v3 性能档）。
    last_net_ms: u64,

    /// Disk I/O 采样器（按需创建，用户开启磁盘监控时才实例化）。
    disk: Option<crate::proc_disk::DiskState>,
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
            util_meter: Meter::new(MeterConfig::default()),
            last_util_ms: None,
            diag: SamplerDiag::new(),
            backoff: [Backoff::default(); 5],
            seq: 0,
            last_net_ms: 0,
            disk: None,
        }
    }

    /// 复位 Util 的时间维度计（审查项 2.2.5 / P1-16）。
    ///
    /// 只在"影响数值的设置发生变化、采样线程被强制提交一次"时调用：
    /// `MeterConfig::tau_down = 3.0` 秒意味着切布局/切字号后，Util 仍会带着
    /// 切换前约 3 s 的拖尾（观感 = 新尺寸下显示旧平滑值）。`last_util_ms` 一并
    /// 置 `None` ⇒ 下一帧 `dt = 0` 被计为非法窗口 ⇒ 直接全信本次读数（与首帧同语义）。
    /// 尖峰计数清零是有意的一部分：改设置等于重新开始观测。
    pub fn reset_util_smoothing(&mut self) {
        self.util_meter.reset();
        self.last_util_ms = None;
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
    ///
    /// `util_enabled` = 综合占用行开关（`show_util`）。开启时每周期用快照里的
    /// CPU/内存/GPU 占用率算排队论闭式解（缺源重归一化）并经非对称 EMA 平滑。
    pub fn sample(
        &mut self,
        plan: &SamplePlan,
        prev: Option<&Snapshot>,
        now_ms: u64,
        power_enabled: bool,
        util_enabled: bool,
        disk_enabled: bool,
    ) -> Snapshot {
        // 从上一份快照出发：未到期源直接保留旧值，失败源也不再丢数据。
        let mut snap = prev.cloned().unwrap_or_else(|| Snapshot::placeholder(now_ms));
        snap.seq = self.seq;
        snap.timestamp_ms = now_ms;
        snap.uptime_ms = now_ms;

        // CPU（退避中则跳过本轮：指数退避此前只被记录、从未被消费）
        if plan.cpu && self.backoff[SamplerId::Cpu as usize].is_backing_off(now_ms) {
            self.push_diag(SamplerId::Cpu, SampleOutcome::Skipped, 0, now_ms);
        } else if plan.cpu {
            let t = crate::perf::begin();
            let cpu_out = self.cpu.sample(prev, now_ms, &mut snap);
            crate::perf::note_section("sample_cpu", t);
            self.push_diag(SamplerId::Cpu, cpu_out, 0, now_ms);
        } else {
            self.push_diag(SamplerId::Cpu, SampleOutcome::Skipped, 0, now_ms);
        }

        // Memory（退避门同上）
        if plan.mem && self.backoff[SamplerId::Memory as usize].is_backing_off(now_ms) {
            self.push_diag(SamplerId::Memory, SampleOutcome::Skipped, 0, now_ms);
        } else if plan.mem {
            let t = crate::perf::begin();
            let mem_out = self.mem.sample(prev, now_ms, &mut snap);
            crate::perf::note_section("sample_mem", t);
            self.push_diag(SamplerId::Memory, mem_out, 0, now_ms);
        } else {
            self.push_diag(SamplerId::Memory, SampleOutcome::Skipped, 0, now_ms);
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
                    // NaN 防护：f32::clamp 对 NaN 返回 NaN（厂商库异常时会漏进快照，
                    // 渲染成 "NaN%"）。
                    snap.gpu = if u.is_finite() {
                        Metric::Value(u.clamp(0.0, 100.0))
                    } else {
                        Metric::Unavailable
                    };
                    crate::perf::note_section("sample_gpu", t);
                    self.push_diag(SamplerId::Gpu, SampleOutcome::Ok, 0, now_ms);
                } else {
                    // vendor 不可用：从缓存恢复；若缓存也无则用 D3DKMT
                    let gpu_out = self.gpu.sample(prev, now_ms, &mut snap);
                    crate::perf::note_section("sample_gpu", t);
                    if let Some(u) = gpu_vendor::cached_util() {
                        snap.gpu = if u.is_finite() {
                            Metric::Value(u.clamp(0.0, 100.0))
                        } else {
                            Metric::Unavailable
                        };
                    }
                    self.push_diag(SamplerId::Gpu, gpu_out, 0, now_ms);
                }
            } else {
                // 功耗行专供：只要瓦数，不要占用率（也不付 D3DKMT 那 74ms）
                crate::perf::note_section("sample_gpu", t);
                self.push_diag(SamplerId::Gpu, SampleOutcome::Skipped, 0, now_ms);
            }
        } else {
            self.push_diag(SamplerId::Gpu, SampleOutcome::Skipped, 0, now_ms);
        }

        // Network：低频档（v3 性能档，默认开）独立 2s 分频；
        // 速率 = Δbytes/Δms（真实间隔），拉长间隔只降粒度不失真。
        // 内部已改为「拓扑缓存 + GetIfEntry2 增量查询」，每周期仅查缓存的活跃 NIC
        // （本机实测 5 个，含 Hyper-V 虚拟交换机，<0.5ms），仅首次/全部掉线/10s 安全间隔
        // 才跑一次 GetIfTable2（~28ms）。
        let net_due = !crate::window::schedule_net_low()
            || now_ms.saturating_sub(self.last_net_ms) >= NET_LOW_FREQ_MS;
        if plan.net && net_due && self.backoff[SamplerId::Net as usize].is_backing_off(now_ms) {
            self.push_diag(SamplerId::Net, SampleOutcome::Skipped, 0, now_ms);
        } else if plan.net && net_due {
            self.last_net_ms = now_ms;
            let t = crate::perf::begin();
            let net_out = self.net.sample(prev, now_ms, &mut snap);
            crate::perf::note_section("sample_net", t);
            self.push_diag(SamplerId::Net, net_out, 0, now_ms);
        } else {
            self.push_diag(SamplerId::Net, SampleOutcome::Skipped, 0, now_ms);
        }

        // Power：整机功耗（用户拍板公式），分频由调度器 `Source::Power` 驱动；
        // CPU 实测不可用（无管理员/无驱动）→ 整行关闭（Unavailable = 不显示不计算）
        if plan.power && self.backoff[SamplerId::Power as usize].is_backing_off(now_ms) {
            // 退避中不重复走 MSR 打开流程（SCM + 服务查询 + 设备打开，代价最高）
            self.push_diag(SamplerId::Power, SampleOutcome::Skipped, 0, now_ms);
        } else if plan.power {
            let t = crate::perf::begin();
            // 等待上限（审查项 2.2.3 / P1-15）：MSR 的实际读取在专用线程上，
            // 这里最多阻塞 min(功耗间隔, 3s)。取 3s 上限的理由：正常 RAPL 读取是
            // 微秒级，只有首轮惰性开服务（SCM + StartService + CreateFile）会慢，
            // 3s 足够；超过 3s 的读取本身已经没有意义（下一帧就要来了）。
            let power_interval_ms = crate::window::schedule_cfg().3.max(1000);
            let timeout_ms = power_interval_ms.min(3000) as u64;
            match self.syspower.sample(now_ms, timeout_ms) {
                Some(w) => {
                    snap.power = Metric::Value(w);
                    self.push_diag(SamplerId::Power, SampleOutcome::Ok, 0, now_ms);
                }
                None => {
                    snap.power = Metric::Unavailable;
                    self.push_diag(SamplerId::Power, SampleOutcome::Skipped, 0, now_ms);
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

        // Disk I/O（PDH）：读写速率，1Hz 采样。用户开启磁盘监控后（通过 metric_order 包含 DiskRead/DiskWrite）
        // 才创建 DiskState 并采样。关闭时销毁 PDH 查询（None）。
        if disk_enabled {
            if self.disk.is_none() {
                self.disk = Some(crate::proc_disk::DiskState::new());
            }
            if let Some(ref mut d) = self.disk {
                let (read, write) = d.sample(now_ms);
                // ready 只在**确实拿到有效读数**时为 true：PDH 首轮只建立基线，
                // 旧实现无条件置 true → 前 2s 显示 "0 B/s" 而不是未就绪。
                snap.disk.ready = read.value().is_some() || write.value().is_some();
                snap.disk.read_bps = read.value().unwrap_or(0.0);
                snap.disk.write_bps = write.value().unwrap_or(0.0);
            }
        } else {
            // 磁盘监控关闭：销毁 PDH 查询，下次开启时重建。
            self.disk = None;
            if let Some(prev) = prev {
                snap.disk = prev.disk;
            }
        }

        // Util：综合占用（用户拍板：闭式解 + 非对称 EMA，等权，缺源重归一化）。
        //
        // 数据源保障在 publish 侧完成（show_util OR 进 cpu/mem/gpu 采样位），
        // 这里只管算：显示行关 ≠ 数据源停 —— Util 行开着时 CPU/内存/GPU
        // 始终有新数据进快照，即使它们各自的行是隐藏的。
        if util_enabled {
            let dt_ms = self.last_util_ms.map(|t| now_ms.saturating_sub(t));
            self.last_util_ms = Some(now_ms);
            let cpu = snap.cpu.value().map(|v| f64::from(v) / 100.0);
            // 内存没有 Metric 包装（load_pct 恒有定义），视为始终可用；
            // 首帧未采样时为 prev 携带的 0，随后即被真实读数覆盖。
            let mem = Some(f64::from(snap.memory.load_pct) / 100.0);
            let gpu = snap.gpu.value().map(|v| f64::from(v) / 100.0);
            match aaxsyspulse_core::composite::aggregate3([cpu, mem, gpu]) {
                Some((score, degraded)) => {
                    let dt_secs = dt_ms.map(|d| d as f64 / 1000.0).unwrap_or(0.0);
                    let m = self.util_meter.update(score, dt_secs);
                    snap.util = Metric::Value(m.smooth_milli as f32 / 10.0);
                    snap.util_partial = degraded;
                    if crate::probe::enabled() {
                        crate::probe::log(&format!(
                            "util in cpu={cpu:?} mem={mem:?} gpu={gpu:?} score={score:.3} instant={} smooth={} dt={dt_ms:?}",
                            m.instant_milli, m.smooth_milli
                        ));
                    }
                }
                None => {
                    // 三源全缺（理论上仅硬件级故障）：行显示红色 --。
                    snap.util = Metric::Unavailable;
                    snap.util_partial = false;
                }
            }
        } else {
            // 行关：显式置 Unavailable，覆盖 prev 带来的旧值（与功耗行同策略）。
            snap.util = Metric::Unavailable;
            snap.util_partial = false;
        }

        self.seq += 1;
        snap
    }

    fn push_diag(&mut self, id: SamplerId, outcome: SampleOutcome, code: u16, now_ms: u64) {
        // Update back-off.
        // ⚠️ 只有**真正成功**才清零退避。旧实现 `else { reset() }` 会让 Skipped
        // （未到期 / 被退避门挡下）也清零 → 退避永远被自己重置，失败源每轮重试，
        // Backoff 形同虚设（2026-09-15 审计）。
        let idx = id as usize;
        if outcome.is_failure() {
            // 传 now_ms：退避门据此刻画「下次允许尝试的时刻」，到点即放行。
            self.backoff[idx].next_delay_ms(now_ms);
        } else if matches!(outcome, SampleOutcome::Ok) {
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

    /// Back-off state for UI to display（`now_ms` 须与采样时钟同源）。
    pub fn backing_off(&self, id: SamplerId, now_ms: u64) -> bool {
        self.backoff[id as usize].is_backing_off(now_ms)
    }

    /// Returns how long to wait before retrying (milliseconds)；0 = 已可重试。
    ///
    /// 旧实现恒返回硬编码 `1000`（自称 placeholder），调用方拿到的等待时长是假的；
    /// 现按 `Backoff` 的实际到期时刻计算（2026-09-15 审计）。
    pub fn retry_delay_ms(&self, id: SamplerId, now_ms: u64) -> u32 {
        self.backoff[id as usize].remaining_delay_ms(now_ms)
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

#[cfg(test)]
mod util_probe {
    use super::*;
    use aaxsyspulse_core::scheduler::SamplePlan;

    /// 临时诊断：用本机真实传感器连跑 6 个周期，打印 Util 计算管线各环节。
    /// 运行：cargo test -p aaxsyspulse-win --lib util_live_probe -- --nocapture --ignored
    #[test]
    #[ignore]
    fn util_live_probe() {
        let mut s = SystemSampler::new(true);
        let plan = SamplePlan { cpu: true, mem: true, gpu: true, net: false, power: false };
        let mut prev = Snapshot::placeholder(0);
        let mut t: u64 = 1_000;
        for i in 0..6 {
            let snap = s.sample(&plan, Some(&prev), t, false, true, false);
            println!(
                "i={i} cpu={:?} mem={} gpu={:?} -> util={:?} partial={}",
                snap.cpu, snap.memory.load_pct, snap.gpu, snap.util, snap.util_partial
            );
            prev = snap;
            t += 2_000;
        }
    }
}
