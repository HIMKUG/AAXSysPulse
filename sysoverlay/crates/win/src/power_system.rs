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
        // 惰性 DRAM 去重：首帧成功读 MSR 后才知道 DRAM 域是否可用。
        // 可用 → 内存功耗已被 PKG+DRAM 实测覆盖，撤掉条数估算，避免双计。
        if self.ram_est_w > 0.0 && self.msr.dram_available() {
            self.ram_est_w = 0.0;
            probe::log("syspower dram domain active -> ram_est_w zeroed (dedup)");
        }
        let gpu = self.gpu_watts.unwrap_or(0.0);
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
