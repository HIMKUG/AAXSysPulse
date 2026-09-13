//! 整机功耗（用户拍板公式）：
//!
//! ```text
//! Power = CPU实测(WinRing0+RAPL) + GPU实测(厂商模块) + 内存条数×5W + 主板44W + 硬盘数×6W
//! ```
//!
//! # 失败语义（用户拍板）
//! - **CPU 实测不可用（无管理员 / 无 WinRing0 / MSR 读取失败）→ 整行关闭**
//!   （`sample()` 返回 `None` → `snap.power = Unavailable` → 不显示、不再计算）；
//! - GPU 功耗拿不到 → 该项记 0 继续显示（利用率还有 D3DKMT 兜底）。

use crate::msr::MsrPower;
use crate::probe;
use crate::sysinfo;
use std::sync::atomic::{AtomicU32, Ordering};

const EST_RAM_W_PER_STICK: f32 = 5.0;
const EST_MAINBOARD_W: f32 = 44.0;
const EST_DISK_W: f32 = 6.0;

/// 最近一轮 **CPU 实测**功率（瓦 ×100 存原子，跨线程共享，供进程功耗分摊）。
static CPU_WATTS_CENTI: AtomicU32 = AtomicU32::new(0);

/// 采样线程每轮保存 CPU 实测功率（供进程榜分摊，0 = 无读数）。
pub fn store_cpu_watts(w: Option<f32>) {
    let v = w.map(|x| (x * 100.0).clamp(0.0, u16::MAX as f32) as u32).unwrap_or(0);
    CPU_WATTS_CENTI.store(v, Ordering::Relaxed);
}

/// 进程榜读取当前 CPU 实测功率（瓦）。返回 `None` 表示无读数（不分摊）。
pub fn cpu_watts_global() -> Option<f32> {
    let v = CPU_WATTS_CENTI.load(Ordering::Relaxed);
    if v == 0 { None } else { Some(v as f32 / 100.0) }
}

pub struct SystemPower {
    msr: MsrPower,
    ram_sticks: usize,
    /// 内存估算重量（瓦）：Intel DRAM 域可用时置 0（避免与实测 DRAM 双计）。
    ram_est_w: f32,
    disks: usize,
    /// 最近一轮 GPU 功耗（厂商模块读数）。
    gpu_watts: Option<f32>,
    /// 最近一轮 CPU 实测功率（供进程分摊）。
    last_cpu_w: Option<f32>,
    /// GPU 瓦数缺失状态（**仅在翻转时**记探针，避免每帧刷屏）。
    gpu_watts_missing: bool,
}

impl Default for SystemPower {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemPower {
    pub fn new() -> Self {
        let msr = MsrPower::new();
        let ram_sticks = sysinfo::ram_stick_count();
        let disks = sysinfo::physical_disk_count();
        // RAM 估算先按条数计（5W/条）；首帧成功读 MSR 后若 DRAM 域可用，
        // 在 sample() 里惰性置 0 去重（DRAM 域探测依赖驱动惰性加载完成）。
        let s = Self {
            msr,
            ram_sticks,
            ram_est_w: ram_sticks as f32 * EST_RAM_W_PER_STICK,
            disks,
            gpu_watts: None,
            last_cpu_w: None,
            gpu_watts_missing: false,
        };
        probe::log(&format!(
            "syspower init ram={} disks={} ram_est_w={} board_w={EST_MAINBOARD_W} disk_w={}",
            s.ram_sticks, s.disks, s.ram_est_w, s.disks as f32 * EST_DISK_W
        ));
        s
    }

    /// 更新最近一轮 GPU 功耗（厂商模块读数，None = 不可用记 0）。
    pub fn set_gpu_watts(&mut self, w: Option<f32>) {
        self.gpu_watts = w;
    }

    /// 最近一轮 CPU 实测功率（进程分摊用；首帧前为 None）。
    pub fn cpu_watts(&self) -> Option<f32> {
        self.last_cpu_w
    }

    /// 整机功耗（瓦）。`None` = CPU 实测不可用 → Power 行关闭。
    pub fn sample(&mut self, now_ms: u64) -> Option<f32> {
        let cpu = self.msr.sample(now_ms)?;
        self.last_cpu_w = Some(cpu);
        // 每帧重新评估内存估算（撤掉「一次性锁存」）：
        // DRAM 域可用 → 内存功耗已被 PKG+DRAM 实测覆盖，撤掉条数估算避免双计；
        // DRAM 域不可用（驱动未就绪 / MSR 读取失败 / 驱动重新加载）→ 回落条数估算，
        // 保证内存功耗在实测缺失时仍不为零，且驱动恢复后不会永久漏算。
        self.ram_est_w = if self.msr.dram_available() {
            0.0
        } else {
            self.ram_sticks as f32 * EST_RAM_W_PER_STICK
        };
        // GPU 功耗直通显示，不再做 EMA：原 0.6/0.4 平滑在 3s 采样节拍下把
        // 负载爬坡拖长约 12s（"功耗 15s 才见 300W"的主要软件侧成因）。
        // NVML/ADLX 读数本身已被驱动内部平均，叠加 EMA 属双重平滑；
        // 显示节奏由提交侧 1.5~4.5s 的刷新下限约束，无需在此再平滑。
        let gpu = self.gpu_watts.unwrap_or(0.0);
        // 可观测性（BUG-4 的教训）：GPU 瓦数缺失时整机功耗会**静默漏掉整个显卡项**
        // （曾经的成因是 GPU 行开关误当采样门）。这里在状态翻转时记一行，
        // 让"功耗是否完整"从日志即可判定，而不是靠用户拿硬件监控对账才发现。
        let missing = self.gpu_watts.is_none();
        if missing != self.gpu_watts_missing {
            self.gpu_watts_missing = missing;
            probe::log(if missing {
                "syspower gpu watts unavailable → 显卡项按 0 并入（整机功耗偏低，查 gpu_vendor 读数）"
            } else {
                "syspower gpu watts available → 显卡项已纳入整机功耗"
            });
        }
        let total = cpu
            + gpu
            + self.ram_est_w
            + EST_MAINBOARD_W
            + self.disks as f32 * EST_DISK_W;
        if probe::enabled() {
            probe::log(&format!(
                "syspower cpu={cpu:.1} gpu={gpu:.1} ram_est={} disks={} total={total:.1}W",
                self.ram_est_w, self.disks
            ));
        }
        Some(total)
    }
}
