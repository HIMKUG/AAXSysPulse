//! CPU 实测功耗：WinRing0x64.sys 驱动 + RAPL MSR（用户拍板方案）。
//!
//! # 链路
//! 1. `WinRing0x64.sys`（固定路径 `C:\Windows\SysOverlay\`，来源 OpenHardwareMonitor 官方仓库）
//!    → SCM 创建/启动内核服务（**需要管理员权限**）；
//! 2. 打开设备 `\\.\WinRing0_1_2_0`（驱动源码 OpenLibSys.h 固定名）；
//! 3. `DeviceIoControl(IOCTL_OLS_READ_MSR=0x9C402084, &msr_index)` 读寄存器。
//!
//! # RAPL 寄存器
//! - AMD Zen：`MSR_RAPL_POWER_UNIT=0xC0010299`（[12:8]=能量单位 1/2^n 焦），
//!   **`MSR_PKG_ENERGY_STATUS=0xC001029B`**（Package 累计能量，64 位）。
//!   ⚠️ 勿用 `0xC001029A` —— 那是 **per-core** 能量寄存器（每个核心各自计），
//!   读它会得到"当前线程所在核心"的水平（AMD 实测只有 ~2-3W），
//!   早期版本误用其当 PKG 导致 AMD 上功耗严重偏低（scaphandre 常量同样区分为
//!   `MSR_AMD_CORE_ENERGY_STATUS=0xC001029A` / `MSR_AMD_PKG_ENERGY_STATUS=0xC001029B`）。
//! - Intel：`0x606` / `0x611`（32 位，含回绕处理）。
//!
//! 功率 = Δ能量(J) / Δ时间(s)；32 位计数回绕时丢弃本轮重置基线。
//!
//! # 权限语义（用户拍板）
//! 任何一步失败（非管理员 / 无驱动文件 / 服务启动失败）→ 返回 `None`
//! → 调用方把 Power 行整体关闭（不显示、不再计算）。

use crate::probe;
use std::sync::OnceLock;
use sysoverlay_core::power::watts_from_energy_delta;

const MSR_AMD_UNIT: u32 = 0xC0010299;
const MSR_AMD_PKG_ENERGY: u32 = 0xC001029B;
const MSR_INTEL_UNIT: u32 = 0x606;
const MSR_INTEL_PKG_ENERGY: u32 = 0x611;
/// Intel DRAM 域能量（32 位）。PKG 域不含内存控制器，补上才完整。
const MSR_INTEL_DRAM_ENERGY: u32 = 0x619;

/// IOCTL_OLS_READ_MSR = CTL_CODE(40000, 0x821, METHOD_BUFFERED, FILE_ANY_ACCESS)
const IOCTL_OLS_READ_MSR: u32 = (40000u32 << 16) | (0x821u32 << 2);
const DEVICE_PATH: &str = "\\\\.\\WinRing0_1_2_0";
/// 能量计数回退判定阈值（超过说明是异常而非回绕）。
const WRAP_LIMIT_RAW: u64 = 1 << 31;

struct Ring0 {
    dev: windows::Win32::Foundation::HANDLE,
}

unsafe impl Send for Ring0 {}
unsafe impl Sync for Ring0 {}

static RING0: OnceLock<Option<Ring0>> = OnceLock::new();

/// CPU 厂商/家族（CPUID），探针与 MSR 选择用。
struct CpuId {
    amd: bool,
    family: u8,
    model: u8,
}

static CPUID: OnceLock<CpuId> = OnceLock::new();

fn cpuid_info() -> &'static CpuId {
    CPUID.get_or_init(|| unsafe {
        let r0 = std::arch::x86_64::__cpuid(0);
        let vendor = [r0.ebx, r0.edx, r0.ecx];
        let vb: [u8; 12] = std::mem::transmute(vendor);
        let amd = &vb == b"AuthenticAMD";
        let r1 = std::arch::x86_64::__cpuid(1);
        CpuId {
            amd,
            family: (((r1.eax >> 8) & 0xF) | ((r1.eax >> 20) & 0xFF)) as u8,
            model: (((r1.eax >> 4) & 0xF) | (((r1.eax >> 16) & 0xF) << 4)) as u8,
        }
    })
}

/// 创建/启动驱动服务并打开设备（一次性）。
fn ring0() -> Option<&'static Ring0> {
    RING0.get_or_init(|| unsafe {
        use windows::Win32::Foundation::{
            CloseHandle, GetLastError, GENERIC_READ, GENERIC_WRITE, ERROR_SERVICE_ALREADY_RUNNING, HANDLE,
        };
        use windows::Win32::Storage::FileSystem::CreateFileW;
        use windows::Win32::System::Services::{
            CreateServiceW, OpenSCManagerW, OpenServiceW, StartServiceW, SC_HANDLE,
            SC_MANAGER_ALL_ACCESS, SERVICE_ALL_ACCESS, SERVICE_DEMAND_START, SERVICE_ERROR_NORMAL,
            SERVICE_KERNEL_DRIVER,
        };

        // 1. 驱动文件**可选**：服务已安装（此前任一版本创建过）时直接复用——
        //    SCM 用服务注册表里的 BinaryPath 启动驱动，与 exe 当前目录无关。
        //    本地文件只在"服务不存在（首次部署）"时才是必需品。
        //    （教训：exe 换目录会导致检查文件失败 → 整个 MSR 关闭 → Power 整行 --。）
        //
        //    驱动固定重定向到 C:\Windows\SysOverlay\（需手动放置或首次部署时拷贝）。
        //    这样即使用户把 exe 移到其他目录，首次部署时仍能正确找到驱动。
        let driver_dir = std::path::PathBuf::from(r"C:\Windows\SysOverlay");
        let sys_path = driver_dir.join("WinRing0x64.sys");
        let sys_exists = sys_path.exists();
        if !sys_exists {
            probe::log("msr driver not beside exe (will reuse installed service if present)");
        }
        let sys_wide: Vec<u16> = sys_path.to_string_lossy().encode_utf16().chain(Some(0)).collect();

        // 2. SCM：创建（幂等）+ 启动服务
        let scm = match OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS) {
            Ok(h) => h,
            Err(e) => {
                probe::log(&format!(
                    "msr OpenSCManager failed hr=0x{:x}（需要管理员权限）",
                    e.code().0 as u32
                ));
                return None;
            }
        };
        let svc_name = windows::core::w!("SysOverlayWinRing0");
        let svc_display = windows::core::w!("SysOverlay WinRing0 MSR Driver");
        let svc: SC_HANDLE = match OpenServiceW(scm, svc_name, SERVICE_ALL_ACCESS) {
            Ok(h) => h,
            Err(_) => {
                // 服务不存在：首次部署，必须有本地驱动文件
                if !sys_exists {
                    probe::log(&format!(
                        "msr driver missing: {:?}（服务未安装，无法首次部署）",
                        sys_path
                    ));
                    return None;
                }
                match CreateServiceW(
                    scm,
                    svc_name,
                    svc_display,
                    SERVICE_ALL_ACCESS,
                    SERVICE_KERNEL_DRIVER,
                    SERVICE_DEMAND_START,
                    SERVICE_ERROR_NORMAL,
                    windows::core::PCWSTR(sys_wide.as_ptr()),
                    None,
                    None,
                    None,
                    None,
                    None,
                ) {
                    Ok(h) => h,
                    Err(e) => {
                        probe::log(&format!("msr CreateService failed hr=0x{:x}", e.code().0 as u32));
                        return None;
                    }
                }
            }
        };
        if let Err(e) = StartServiceW(svc, None) {
            if GetLastError() != ERROR_SERVICE_ALREADY_RUNNING {
                probe::log(&format!(
                    "msr StartService failed err={}（拒绝/驱动签名被拦）",
                    e.code().0
                ));
                return None;
            }
        }

        // 3. 打开设备
        let dev_wide: Vec<u16> = DEVICE_PATH.encode_utf16().chain(Some(0)).collect();
        let dev = match CreateFileW(
            windows::core::PCWSTR(dev_wide.as_ptr()),
            (GENERIC_READ | GENERIC_WRITE).0,
            windows::Win32::Storage::FileSystem::FILE_SHARE_READ
                | windows::Win32::Storage::FileSystem::FILE_SHARE_WRITE,
            None,
            windows::Win32::Storage::FileSystem::OPEN_EXISTING,
            windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL,
            None,
        ) {
            Ok(_h) if !_h.is_invalid() => _h,
            Ok(_h) => {
                probe::log("msr open device returned invalid handle");
                return None;
            }
            Err(e) => {
                probe::log(&format!("msr open device failed hr=0x{:x}", e.code().0 as u32));
                return None;
            }
        };
        let _ = CloseHandle(HANDLE(scm.0));
        let c = cpuid_info();
        probe::log(&format!(
            "msr ring0 ready vendor={} family=0x{:x} model=0x{:x}",
            if c.amd { "AMD" } else { "Intel" },
            c.family,
            c.model
        ));
        Some(Ring0 { dev })
    })
    .as_ref()
}

fn read_msr(r: &Ring0, index: u32) -> Option<u64> {
    unsafe {
        use windows::Win32::System::IO::DeviceIoControl;
        let mut out: u64 = 0;
        let mut ret = 0u32;
        DeviceIoControl(
            r.dev,
            IOCTL_OLS_READ_MSR,
            Some(&index as *const u32 as *const core::ffi::c_void),
            4,
            Some(&mut out as *mut u64 as *mut core::ffi::c_void),
            8,
            Some(&mut ret),
            None,
        )
        .ok()?;
        Some(out)
    }
}

/// 采样线程启动时调用一次：把当前线程（即采样线程）绑到 0 号逻辑核，
/// 使后续所有 MSR 读数都在同一核上进行（RAPL 域语义一致，避免功耗毛刺）。
///
/// 相比 `read_msr` 在**每次读**都 `SetThreadAffinityMask`+还原
/// （每帧 4 次系统调用 + 强制线程迁移，污染 wall-clock `dt` 并把负载堆到 CPU0），
/// 一次性绑定后读数零迁移、零系统调用——`read_msr` 直接读即可。
///
/// `SetThreadAffinityMask` 失败（极少）不影响后续采样，仅退化回"不绑核"。
pub fn pin_sampling_thread() {
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadAffinityMask};
        let cur = GetCurrentThread();
        let _ = SetThreadAffinityMask(cur, 1); // 位 0 = 0 号逻辑核
    }
}

// ── 功率派生 ─────────────────────────────────────────────────────
//
// 数据语义（对齐 scaphandre 的"能量为主"）：本模块只负责读出**累计能量**
// 原始值与差异的硬件正确性（回绕/倒退/DS 域），功率 W 由
// [`core::power::watts_from_energy_delta`] 单点计算（窗口平均语义 + 物理钳制）。

pub struct MsrPower {
    /// (上次墙钟 ms, 上次 PKG 原始值, 上次 DRAM 原始值)。
    prev: Option<(u64, u64, u64)>,
    /// 能量单位（焦/原始计数）。
    unit_j: f32,
    ready: bool,
    /// Intel DRAM 域是否可用（0x619 探测成功；AMD 无此域）。
    dram_ok: bool,
    /// 上一次**有效**读数（异常时回退到它，避免显示跳变或掉成 `--`）。
    last_watts: Option<f32>,
    /// 连续异常计数。
    invalid: u8,
}

impl Default for MsrPower {
    fn default() -> Self {
        Self::new()
    }
}

impl MsrPower {
    pub fn new() -> Self {
        Self { prev: None, unit_j: 0.0, ready: false, dram_ok: false, last_watts: None, invalid: 0 }
    }

    /// DRAM 域是否可用（Intel 且探测成功）。估算侧据此与"内存条 ×5W"去重。
    pub fn dram_available(&self) -> bool {
        self.dram_ok
    }

    /// 读取一轮 CPU 封装功耗（瓦 = PKG+DRAM 能量差窗口平均）。驱动缺失返回 None。
    pub fn sample(&mut self, now_ms: u64) -> Option<f32> {
        if !self.ready {
            self.init()?;
            self.ready = true;
        }
        let r = ring0()?;
        let amd = cpuid_info().amd;
        let pkg_idx = if amd { MSR_AMD_PKG_ENERGY } else { MSR_INTEL_PKG_ENERGY };
        let pkg = read_msr(r, pkg_idx)?;
        let dram = if self.dram_ok {
            read_msr(r, MSR_INTEL_DRAM_ENERGY).unwrap_or(0)
        } else {
            0
        };

        // 首轮：只建立基线，本帧无功率可言。
        let Some((pms, ppkg, pdram)) = self.prev else {
            self.prev = Some((now_ms, pkg, dram));
            return None;
        };

        let dt_ms = now_ms.saturating_sub(pms);
        // PKG 增量按位宽分流：
        // - Intel：PKG 32 位计数器，真回绕时 `wrapping` 才能还原真实 Δ；
        // - AMD（Zen）：64 位计数器，**倒退 = 读数异常**（实测虚拟化环境
        //   伪值会倒退），拒绝并沿用上帧，而不是当成回绕算出假功耗。
        let pkg_delta = if amd {
            match pkg.checked_sub(ppkg) {
                Some(d) => d,
                None => {
                    self.invalid = self.invalid.saturating_add(1);
                    probe::log(&format!(
                        "msr amd regression prev={ppkg} cur={pkg} hold={:?}",
                        self.last_watts
                    ));
                    return self.last_watts;
                }
            }
        } else {
            (pkg as u32).wrapping_sub(ppkg as u32) as u64
        };
        let dram_delta = if self.dram_ok {
            (dram as u32).wrapping_sub(pdram as u32) as u64
        } else {
            0
        };

        // 基线**无条件推进**：保证 dt 恒为单周期。
        // 若只在读数有效时推进，计数器震荡期间基线会停住，dt 累积到
        // 6s/9s，delta 跨越多个震荡周期，算出的"平均功率"看似合理
        // （实测 139W）实则仍是假值。推进基线后，震荡帧的 delta 必然
        // 超过单周期能量上限，被下面的钳制直接拒绝。
        self.prev = Some((now_ms, pkg, dram));

        // 几何筛（单周期窗口 + 回绕上限）→ core::power 物理钳制。
        if pkg_delta < WRAP_LIMIT_RAW && dram_delta < WRAP_LIMIT_RAW {
            let delta_j = (pkg_delta + dram_delta) as f64 * self.unit_j as f64;
            if let Some(w) = watts_from_energy_delta(delta_j, dt_ms) {
                self.invalid = 0;
                self.last_watts = Some(w);
                if probe::enabled() {
                    probe::log(&format!(
                        "msr pkg={pkg} dram={dram} delta_j={delta_j:.4} dt={dt_ms} unit_j={:.8} watts={w}",
                        self.unit_j
                    ));
                }
                return Some(w);
            }
        }

        // 无效帧：沿用上次有效值（基线已推进，下一帧重新计算）。
        self.invalid = self.invalid.saturating_add(1);
        probe::log(&format!(
            "msr reject dt={dt_ms} pkg_delta={pkg_delta} dram_delta={dram_delta} invalid={} hold={:?}",
            self.invalid, self.last_watts
        ));
        self.last_watts
    }

    fn init(&mut self) -> Option<()> {
        let c = cpuid_info();
        let unit_idx = if c.amd { MSR_AMD_UNIT } else { MSR_INTEL_UNIT };
        let r = ring0()?;
        let u = read_msr(r, unit_idx)?;
        let n = (u >> 8) & 0x1F;
        self.unit_j = 1.0 / (1u64 << n) as f32;
        // DRAM 域：仅 Intel 探测（AMD 没有可公开读取的 DRAM 能量域）。
        if !c.amd {
            self.dram_ok = read_msr(r, MSR_INTEL_DRAM_ENERGY).is_some();
        }
        probe::log(&format!(
            "msr unit msr=0x{unit_idx:x} raw=0x{u:x} n={n} unit_j={:.8} dram_ok={}",
            self.unit_j, self.dram_ok as u8
        ));
        Some(())
    }
}
