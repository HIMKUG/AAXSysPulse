//! CPU 实测功耗：WinRing0x64.sys 驱动 + RAPL MSR（用户拍板方案）。
//!
//! # 链路
//! 1. `WinRing0x64.sys`（固定路径 `C:\Windows\AAXSysPulse\`，来源 OpenHardwareMonitor 官方仓库）
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
//!
//! # 驱动文件查找策略（多位置候选 + 服务自愈）
//!
//! SCM 启动内核驱动只看服务注册表里的 BinaryPath，与 exe 当前目录无关。
//! 因此本模块按以下顺序保证「sys 与 exe 不同目录也能用」：
//! 1. 服务已存在且其 BinaryPath 指向的文件还在 → 原样复用（零改动）；
//! 2. 服务存在但 BinaryPath 失效（exe 被移动 / `cargo clean` 删掉
//!    `target/release` 等）→ 扫描候选位置，找到文件后用
//!    `ChangeServiceConfigW` 把服务重定向过去（自愈）；
//! 3. 服务不存在（首次部署）→ 用第一个存在的候选位置创建服务。
//! 候选位置：`C:\Windows\AAXSysPulse\`（稳定目录，优先）→ exe 同目录（兜底）。
//!
//! # Safety（本文件 unsafe 汇总）
//!
//! - `cpuid_info`：`__cpuid` 在 x86_64 目标上恒可用；`transmute` 仅做
//!   `[u32;3] → [u8;12]` 的同尺寸重解释（对齐要求收窄，合法）；
//! - `ring0`：SCM / `CreateFileW` / `DeviceIoControl` FFI，失败路径全部显式
//!   处理；设备句柄仅在 `teardown` 中 `CloseHandle` 一次，此后 `RING0_TORN`
//!   使一切访问短路，不存在双重释放或 use-after-close；
//! - `Ring0` 的 `Send/Sync`：见 impl 处 SAFETY 注释（唯一访问者是绑核采样线程）。

use crate::probe;
use std::sync::OnceLock;
use aaxsyspulse_core::power::watts_from_energy_delta;
// 辅助函数（query_service_binary_path / driver_file_exists）的签名与实现所需。
// ring0() 内部的同名局部 use 会遮蔽此处导入，属正常显式声明。
use windows::Win32::System::Services::{QueryServiceConfigW, QUERY_SERVICE_CONFIGW, SC_HANDLE};

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

// SAFETY: Ring0 仅持有一个内核设备句柄（HANDLE 本质是不透明整数，可跨线程传递）；
// 全部访问只发生在采样线程（app 启动时已绑定 0 号逻辑核），RING0 是 OnceLock
// 单例，不存在同一句柄被并发读写的路径。若未来新增其他访问线程，必须重新审视此处。
unsafe impl Send for Ring0 {}
unsafe impl Sync for Ring0 {}

static RING0: OnceLock<Option<Ring0>> = OnceLock::new();
/// teardown 后置位：本会话内驱动已卸载，所有 MSR 访问短路返回 None（不热恢复）。
static RING0_TORN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
/// 已 teardown（行关闭卸载）后永久返回 None——本会话不热恢复，重开行显示 `--`。
fn ring0() -> Option<&'static Ring0> {
    if RING0_TORN.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    RING0.get_or_init(|| unsafe {
        use windows::Win32::Foundation::{
            CloseHandle, GetLastError, GENERIC_READ, GENERIC_WRITE, ERROR_SERVICE_ALREADY_RUNNING, HANDLE,
        };
        use windows::Win32::Storage::FileSystem::CreateFileW;
        use windows::Win32::System::Services::{
            // QueryServiceConfigW/QUERY_SERVICE_CONFIGW 在模块级导入（helper 用），
            // 此处不再重复引入以免遮蔽触发 unused 警告。
            ChangeServiceConfigW, CreateServiceW, OpenSCManagerW, OpenServiceW,
            StartServiceW, ENUM_SERVICE_TYPE, SC_HANDLE,
            SC_MANAGER_ALL_ACCESS, SERVICE_ALL_ACCESS, SERVICE_DEMAND_START, SERVICE_ERROR,
            SERVICE_ERROR_NORMAL, SERVICE_KERNEL_DRIVER, SERVICE_NO_CHANGE, SERVICE_START_TYPE,
        };

        // 1. 驱动文件候选位置（见模块头「驱动文件查找策略」）：
        //    稳定目录优先——不随 exe 移动/清理而失效；exe 同目录只作兜底。
        let sys_candidates: [std::path::PathBuf; 2] = [
            std::path::PathBuf::from(r"C:\Windows\AAXSysPulse\WinRing0x64.sys"),
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|d| d.join("WinRing0x64.sys")))
                .unwrap_or_default(),
        ];
        let found_sys = sys_candidates.iter().find(|p| p.exists());
        if found_sys.is_none() {
            probe::log("msr: no driver file in any candidate location (will fail unless service path is still valid)");
        }

        // 2. SCM：创建（幂等，含失效 BinaryPath 自愈）+ 启动服务
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
        let svc_name = windows::core::w!("AAXSysPulseWinRing0");
        let svc_display = windows::core::w!("AAXSysPulse WinRing0 MSR Driver");
        let svc: SC_HANDLE = match OpenServiceW(scm, svc_name, SERVICE_ALL_ACCESS) {
            Ok(h) => {
                // 服务已存在：BinaryPath 仍有效则原样复用；失效（文件被删/移动）
                // 则重定向到候选位置——修复旧版"服务指向已删除文件"的死局。
                match query_service_binary_path(h) {
                    Some(bp) if driver_file_exists(&bp) => {
                        probe::log(&format!("msr: reusing service, binary={bp}"));
                    }
                    other => {
                        probe::log(&format!(
                            "msr: service binary stale ({:?}) -> repairing",
                            other
                        ));
                        match found_sys {
                            Some(p) => {
                                let wide: Vec<u16> = p
                                    .to_string_lossy()
                                    .encode_utf16()
                                    .chain(Some(0))
                                    .collect();
                                if let Err(e) = ChangeServiceConfigW(
                                    h,
                                    ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                                    SERVICE_START_TYPE(SERVICE_NO_CHANGE),
                                    SERVICE_ERROR(SERVICE_NO_CHANGE),
                                    windows::core::PCWSTR(wide.as_ptr()),
                                    windows::core::PCWSTR::null(),
                                    None,
                                    windows::core::PCWSTR::null(),
                                    windows::core::PCWSTR::null(),
                                    windows::core::PCWSTR::null(),
                                    windows::core::PCWSTR::null(),
                                ) {
                                    probe::log(&format!(
                                        "msr ChangeServiceConfig failed hr=0x{:x}",
                                        e.code().0 as u32
                                    ));
                                    return None;
                                }
                                probe::log(&format!("msr: service repointed to {}", p.display()));
                            }
                            None => {
                                probe::log("msr: stale service binary and no driver file anywhere -> giving up");
                                return None;
                            }
                        }
                    }
                }
                h
            }
            Err(_) => {
                // 服务不存在：首次部署，任一候选位置有 .sys 即可
                match found_sys {
                    Some(p) => {
                        let wide: Vec<u16> =
                            p.to_string_lossy().encode_utf16().chain(Some(0)).collect();
                        match CreateServiceW(
                            scm,
                            svc_name,
                            svc_display,
                            SERVICE_ALL_ACCESS,
                            SERVICE_KERNEL_DRIVER,
                            SERVICE_DEMAND_START,
                            SERVICE_ERROR_NORMAL,
                            windows::core::PCWSTR(wide.as_ptr()),
                            None,
                            None,
                            None,
                            None,
                            None,
                        ) {
                            Ok(h) => {
                                probe::log(&format!("msr: service created at {}", p.display()));
                                h
                            }
                            Err(e) => {
                                probe::log(&format!(
                                    "msr CreateService failed hr=0x{:x}",
                                    e.code().0 as u32
                                ));
                                return None;
                            }
                        }
                    }
                    None => {
                        probe::log(&format!(
                            "msr driver missing everywhere（服务未安装，无法首次部署）：{}",
                            sys_candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
                        ));
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

/// 读取服务的当前 BinaryPath（原样返回，含 `\??\` 内核前缀）。
/// 查询失败 / 缓冲区不足 / 路径为空一律返回 None（调用方按"未知"处理并走修复）。
fn query_service_binary_path(svc: SC_HANDLE) -> Option<String> {
    unsafe {
        // QUERY_SERVICE_CONFIGW 含指针成员，需 8 字节对齐缓冲：用 u64 数组而非 Vec<u8>。
        // 4KB 足够常规路径；不足时宁可返回 None 走修复路径，也不引入二次调用复杂度。
        let mut buf = vec![0u64; 512];
        let mut needed: u32 = 0;
        QueryServiceConfigW(
            svc,
            Some(buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW),
            (buf.len() * 8) as u32,
            &mut needed,
        )
        .ok()?;
        let cfg = &*(buf.as_ptr() as *const QUERY_SERVICE_CONFIGW);
        let pw = cfg.lpBinaryPathName;
        if pw.0.is_null() {
            return None;
        }
        let mut len = 0usize;
        while *pw.0.add(len) != 0 {
            len += 1;
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(pw.0, len)))
    }
}

/// 服务的 BinaryPath（可能带 `\??\` 内核前缀）指向的文件是否真实存在。
fn driver_file_exists(binary_path: &str) -> bool {
    let p = binary_path.strip_prefix(r"\??\").unwrap_or(binary_path);
    std::path::Path::new(p).exists()
}

/// 卸载 MSR 后端（v3「行关即完全停止」）：
/// 1. 置位 torn 标志 → 后续所有采样短路返回 None（行显示 `--`）；
/// 2. 关闭设备句柄；
/// 3. `ControlService(STOP)` 停止并从内核卸载 WinRing0 驱动（服务注册保留，
///    下次应用启动会按需重新 Start；本会话内不热恢复，重开行时由设置页提示重启）。
///
/// 幂等：重复调用无副作用。由采样线程在 Power 行 falling-edge 时调用。
pub fn teardown() {
    use std::sync::atomic::Ordering;
    if RING0_TORN.swap(true, Ordering::SeqCst) {
        return; // 已卸载
    }
    // 关设备句柄（若已初始化）
    if let Some(Some(r)) = RING0.get() {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(r.dev);
        }
    }
    // 停止内核驱动服务
    unsafe {
        use windows::Win32::System::Services::{
            CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, SC_MANAGER_ALL_ACCESS,
            SERVICE_ALL_ACCESS, SERVICE_CONTROL_STOP,
        };
        let Ok(scm) = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS) else {
            probe::log("msr teardown: OpenSCManager failed（非管理员？驱动服务保留未停止）");
            return;
        };
        let svc_name = windows::core::w!("AAXSysPulseWinRing0");
        match OpenServiceW(scm, svc_name, SERVICE_ALL_ACCESS) {
            Ok(svc) => {
                let mut status = Default::default();
                if ControlService(svc, SERVICE_CONTROL_STOP, &mut status).is_err() {
                    probe::log("msr teardown: ControlService STOP failed（可能未运行）");
                } else {
                    probe::log("msr teardown: driver service stopped");
                }
                let _ = CloseServiceHandle(svc);
            }
            Err(_) => probe::log("msr teardown: service not found（本就未安装）"),
        }
        let _ = CloseServiceHandle(scm);
    }
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
