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
//!
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
use std::sync::{Mutex, MutexGuard, OnceLock};
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
// 全部访问只发生在采样线程（app 启动时已绑定 0 号逻辑核），RING0 由 Mutex 保护，
// 不存在同一句柄被并发读写的路径。若未来新增其他访问线程，必须重新审视此处。
unsafe impl Send for Ring0 {}
unsafe impl Sync for Ring0 {}

/// 句柄所有权 = 生命周期：`ring0()` 在唤醒后会用新 `Ring0` 替换旧的，
/// 没有 Drop 就只丢弃结构体、不关句柄 → 每次睡眠唤醒泄漏一个 HANDLE。
/// 有了 Drop，`*g = new` 与 `teardown` 的 `take()` 都会自动关闭旧句柄。
impl Drop for Ring0 {
    fn drop(&mut self) {
        // Drop 中无 fail-safe 路径可走；关闭失败（句柄已失效）忽略即可。
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.dev);
        }
    }
}

static RING0: Mutex<Option<Ring0>> = Mutex::new(None);
/// teardown 后置位：本会话内驱动已卸载，所有 MSR 访问短路返回 None（不热恢复）。
static RING0_TORN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 系统唤醒后置位：下一轮 `ring0()` 会重新打开设备 + 重启内核驱动服务，
/// 避免睡眠唤醒后句柄失效导致功耗永久 `--`（与 GPU `invalidate_topology` 对称）。
static RING0_STALE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// CPU 厂商/家族（CPUID），探针与 MSR 选择用。
struct CpuId {
    amd: bool,
    family: u8,
    model: u8,
}

static CPUID: OnceLock<CpuId> = OnceLock::new();

fn cpuid_info() -> &'static CpuId {
    // 注：Rust 1.96 起 `__cpuid(常量 leaf)` 已是安全函数；旧写法 `|| unsafe { .. }`
    // 在本版会触发 unused_unsafe 警告（2026-09-15 审计顺带清理）。
    CPUID.get_or_init(|| {
        let r0 = std::arch::x86_64::__cpuid(0);
        // 用 to_le_bytes 拼接，而不是 transmute([u32;3] → [u8;12])：语义等价
        // （x86 小端）但消除了不必要的 UB 面，也不再强假设目标字节序。
        let mut vb = [0u8; 12];
        vb[0..4].copy_from_slice(&r0.ebx.to_le_bytes());
        vb[4..8].copy_from_slice(&r0.edx.to_le_bytes());
        vb[8..12].copy_from_slice(&r0.ecx.to_le_bytes());
        let amd = &vb == b"AuthenticAMD";
        let r1 = std::arch::x86_64::__cpuid(1);
        CpuId {
            amd,
            family: (((r1.eax >> 8) & 0xF) | ((r1.eax >> 20) & 0xFF)) as u8,
            model: (((r1.eax >> 4) & 0xF) | (((r1.eax >> 16) & 0xF) << 4)) as u8,
        }
    })
}

/// 创建/启动驱动服务并打开设备（一次性，唤醒后可由 `invalidate_on_resume` 重做）。
/// 已 teardown（行关闭卸载）后永久返回 None——本会话不热恢复，重开行显示 `--`。
unsafe fn open_ring0() -> Option<Ring0> {
        use windows::Win32::Foundation::{
            GENERIC_READ, GENERIC_WRITE, ERROR_SERVICE_ALREADY_RUNNING,
        };
        use windows::Win32::Storage::FileSystem::CreateFileW;
        use windows::Win32::System::Services::{
            // QueryServiceConfigW/QUERY_SERVICE_CONFIGW 在模块级导入（helper 用），
            // 此处不再重复引入以免遮蔽触发 unused 警告。
            ChangeServiceConfigW, CloseServiceHandle, CreateServiceW, OpenSCManagerW, OpenServiceW,
            StartServiceW, ENUM_SERVICE_TYPE, SC_HANDLE,
            SC_MANAGER_ALL_ACCESS, SERVICE_ALL_ACCESS, SERVICE_DEMAND_START, SERVICE_ERROR,
            SERVICE_ERROR_NORMAL, SERVICE_KERNEL_DRIVER, SERVICE_NO_CHANGE, SERVICE_START_TYPE,
        };

        // 1. 驱动文件候选位置（见模块头「驱动文件查找策略」）：
        //    稳定目录优先——不随 exe 移动/清理而失效；exe 同目录只作兜底。
        let sys_candidates: [std::path::PathBuf; 2] = [
            std::path::Path::new(aaxsyspulse_core::contract::DRIVER_DIR)
                .join(aaxsyspulse_core::contract::DRIVER_FILE_NAME),
            std::env::current_exe()
                .ok()
                .and_then(|e| {
                    e.parent().map(|d| d.join(aaxsyspulse_core::contract::DRIVER_FILE_NAME))
                })
                .unwrap_or_default(),
        ];
        let found_sys = sys_candidates.iter().find(|p| p.exists());
        if found_sys.is_none() {
            probe::log("msr: no driver file in any candidate location (will fail unless service path is still valid)");
        }

        // RAII：失败路径统一 CloseServiceHandle。旧版手动管理漏关了 svc 句柄，
        // 泄漏的句柄令服务删除永远停在「标记删除」（ERROR_SERVICE_MARKED_FOR_DELETE），
        // 同名服务无法重建——排查「换装路径后服务死局」时实测踩中。
        struct SvcHandleGuard(SC_HANDLE);
        impl Drop for SvcHandleGuard {
            fn drop(&mut self) {
                // Drop 中无 fail-safe 路径可走；关闭失败（句柄已失效）忽略即可。
                unsafe {
                    let _ = CloseServiceHandle(self.0);
                }
            }
        }

        // 2. SCM：创建（幂等，含失效 BinaryPath 自愈）+ 启动服务
        let scm = match OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS) {
            Ok(h) => SvcHandleGuard(h),
            Err(e) => {
                probe::log(&format!(
                    "msr OpenSCManager failed hr=0x{:x}（需要管理员权限）",
                    e.code().0 as u32
                ));
                return None;
            }
        };
        // 服务名走契约常量 + 运行期宽字符（审查项 7.1.2 / P1-22）：`w!` 只吃字面量，
        // 用它就等于在本 crate 再存一份副本。显示名同样收敛 —— 之前这里是
        // "…WinRing0 MSR Driver"，而引导 `driver.rs` 建的是 "…WinRing0 Driver"，
        // 谁先建服务谁决定 services.msc 里看到的名字。
        let svc_name_w =
            aaxsyspulse_core::contract::wide_of(aaxsyspulse_core::contract::DRIVER_SERVICE_NAME);
        let svc_display_w =
            aaxsyspulse_core::contract::wide_of(aaxsyspulse_core::contract::DRIVER_DISPLAY_NAME);
        let svc_name = windows::core::PCWSTR(svc_name_w.as_ptr());
        let svc_display = windows::core::PCWSTR(svc_display_w.as_ptr());
        let svc: SvcHandleGuard = match OpenServiceW(scm.0, svc_name, SERVICE_ALL_ACCESS) {
            Ok(h) => {
                let h = SvcHandleGuard(h);
                // 服务已存在：BinaryPath 仍有效则原样复用；失效（文件被删/移动）
                // 则重定向到候选位置——修复旧版"服务指向已删除文件"的死局。
                match query_service_binary_path(h.0) {
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
                                    h.0,
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
                                    return None; // guard 自动关闭 svc + scm
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
                            scm.0,
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
                                SvcHandleGuard(h)
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
        // StartService 失败不再直接放弃：183（ERROR_ALREADY_EXISTS）通常表示设备名
        // 已被另一个 WinRing0 实例占用（其他监控程序 / 遗留安装的旧服务仍在运行）——
        // 此时设备本身可用。落到下方设备打开探测：能打开就照常读 MSR，
        // 与 driver-installer 的 RES_OK_DEVICE 语义对齐。
        if let Err(e) = StartServiceW(svc.0, None) {
            // 用 e.code() 而非 GetLastError()：后者是"最近一次 Win32 调用"的错误码，
            // 可能被 crate 内部后续调用污染，导致 183 判定失效。
            if e.code().0 as u32 != ERROR_SERVICE_ALREADY_RUNNING.0 {
                probe::log(&format!(
                    "msr StartService failed err={}（继续探测设备是否已由其他实例提供）",
                    e.code().0
                ));
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
                return None; // guard 自动关闭 svc + scm
            }
            Err(e) => {
                // 走到这里 = 服务没能由本会话启动、且设备也没有任何实例提供：
                // 真正的失败（权限不足 / 签名被 DSE 拦截 / 驱动文件损坏）。
                probe::log(&format!(
                    "msr open device failed hr=0x{:x}（服务未运行且设备不可用）",
                    e.code().0 as u32
                ));
                return None; // guard 自动关闭 svc + scm
            }
        };
        let c = cpuid_info();
        probe::log(&format!(
            "msr ring0 ready vendor={} family=0x{:x} model=0x{:x}",
            if c.amd { "AMD" } else { "Intel" },
            c.family,
            c.model
        ));
        Some(Ring0 { dev })
}

/// 取 `RING0` 的锁：**中毒不致命**（审查项 7.2.1 / P1-20）。
///
/// 这行在采样热路径上。旧写法 `lock().unwrap()` 意味着"任何一个持锁线程 panic 过，
/// 此后每轮采样都 panic" ⇒ 采样线程反复死亡、五个指标永久定格，而现场看起来只是
/// 数字不更新（比崩溃更难诊断）。取中毒锁的正确姿势是 `into_inner()` 继续用：
/// 本字段的不变量只有「句柄要么有效要么 None」，`Option<Ring0>` 自身没有可被
/// 撕开的多字段一致性，中毒后照用安全。
fn lock_ring0() -> MutexGuard<'static, Option<Ring0>> {
    RING0.lock().unwrap_or_else(|e| e.into_inner())
}

/// 取得 MSR 设备句柄：懒初始化；若 `invalidate_on_resume` 已标记失效则重建。
/// 返回 `MutexGuard` 持有锁，保证采样期间句柄不被 `teardown` 关闭。
fn ring0() -> Option<MutexGuard<'static, Option<Ring0>>> {
    if RING0_TORN.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let mut g = lock_ring0();
    if g.is_none() || RING0_STALE.load(std::sync::atomic::Ordering::Relaxed) {
        let new = unsafe { open_ring0() };
        *g = new;
        RING0_STALE.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    Some(g)
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
    // 关设备句柄（若已初始化）：take() 取得所有权，Ring0 的 Drop 自动 CloseHandle。
    // 用 try_lock 而不是 lock（审查项 2.2.3）：功耗线程可能正卡在 `DeviceIoControl`
    // 上并持有该锁，阻塞等待会把采样线程挂回同一个死点上。取不到锁时只跳过
    // 显式关闭 —— 那次阻塞调用返回后 `Ring0` 的 Drop 仍会 CloseHandle。
    match RING0.try_lock() {
        Ok(mut g) => {
            let _ = g.take();
        }
        Err(_) => probe::log("msr teardown: 功耗线程仍持有设备锁，句柄将在其调用返回后关闭"),
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
        let svc_name_w =
            aaxsyspulse_core::contract::wide_of(aaxsyspulse_core::contract::DRIVER_SERVICE_NAME);
        let svc_name = windows::core::PCWSTR(svc_name_w.as_ptr());
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

/// 系统唤醒后调用：标记 MSR 设备句柄待重建。
///
/// 睡眠/唤醒期间 WinRing0 内核驱动服务通常仍在运行，但设备句柄可能失效
/// （驱动被电源管理重置 / 显卡驱动重初始化连带影响）。下一轮采样 `ring0()`
/// 检测到 `RING0_STALE` 会重新打开设备并重启服务，避免功耗永久 `--`。
/// 与 GPU `invalidate_topology` 对称处理唤醒自愈。
pub fn invalidate_on_resume() {
    if RING0_TORN.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    RING0_STALE.store(true, std::sync::atomic::Ordering::Relaxed);
    probe::log("msr: marked stale on resume — will reopen device + restart service on next sample");
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
///
/// **为什么是 0 号核（审查项 2.1.4）**：掩码 `1` = 位 0。RAPL 的 PKG 能量计数是
/// **封装级**的，语义上要求同一读者连续读同一物理封装 —— 真正要防的是"批量读
/// 途中线程被迁到另一个封装"（双路机器上那是两个不同的能量域），跨域会让
/// delta 直接失真（不是精度问题而是数值错误）。绑到任意**一个**核都能达到这个
/// 目的，选 0 只是因为它在 `GetActiveProcessorCount` 非零的机器上必然存在，
/// 不需要先查拓扑。多处理器组（>64 逻辑核）机器上 `SetThreadAffinityMask` 只能
/// 覆盖 group 0，此处忽略失败即退化为不绑核 —— 与"读不到就 None"相比是可接受的
/// 降级（记录于 2.1.4，未做 group 感知）。
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
        let g = ring0()?;
        let r = g.as_ref()?;
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
        // 窗口上界必须按**当前生效的功耗间隔**推导（2.1.4）：`power_interval_ms`
        // 允许到 60 s，写死的 5 s 上界会把大间隔下的每一帧都判为非法。
        let interval_ms = crate::window::schedule_cfg().3 as u64;
        if pkg_delta < WRAP_LIMIT_RAW && dram_delta < WRAP_LIMIT_RAW {
            let delta_j = (pkg_delta + dram_delta) as f64 * self.unit_j as f64;
            if let Some(w) = watts_from_energy_delta(delta_j, dt_ms, interval_ms) {
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
        let g = ring0()?;
        let r = g.as_ref()?;
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

// ─── 执行隔离（审查项 2.2.3 / P1-15）───────────────────────────
//
// `MsrPower::sample` 底层是同步阻塞的 `DeviceIoControl`。它原本与 CPU/内存/GPU/
// 网络跑在同一条采样线程上：设备无响应（驱动卡死、电源状态切换瞬间设备栈抖动）
// 时整条采样线程停在一次读取上，**五个源一起定格**，而渲染看门狗只管渲染线程，
// 对卡住的采样线程无所作为。
//
// 这里把阻塞调用挪到专用线程，采样线程只 `recv_timeout` 等一个期限：
// · 超时 → 本帧功耗记 `--`，其余四源照常更新；
// · 工作线程仍卡在设备调用里时不重复派单（否则每轮超时泄漏一条线程）；
// · 连续 3 次真实超时 → 本会话放弃功耗源（丢弃通道，工作线程自行退出）。
//
// 附带效应：`teardown` 里对 `RING0` 的 `lock` 改成 `try_lock` —— 工作线程卡死时
// 它正持有该锁，阻塞等待会让"隔离"退化成把采样线程挂回同一个死点上。

/// 一轮 MSR 采样回传给调用方的结果。
#[derive(Clone, Copy)]
pub struct MsrFrame {
    /// 封装功率（瓦）；`None` = 本帧无有效读数（首轮建基线 / 驱动不可用）。
    pub watts: Option<f32>,
    /// Intel DRAM 能量域是否可用（`SystemPower` 据此与"内存条 ×5W"估算去重）。
    pub dram_available: bool,
}

/// 连续超时多少次后放弃本会话的功耗采样。
const MSR_TIMEOUT_GIVEUP: u32 = 3;

fn msr_worker(req: std::sync::mpsc::Receiver<u64>, reply: std::sync::mpsc::Sender<MsrFrame>) {
    let mut msr = MsrPower::new();
    while let Ok(now_ms) = req.recv() {
        let watts = msr.sample(now_ms);
        if reply
            .send(MsrFrame {
                watts,
                dram_available: msr.dram_available(),
            })
            .is_err()
        {
            break; // 调用方已丢弃回包通道（放弃或退出）
        }
    }
}

/// `MsrPower` 的超时隔离外壳，供采样线程持有。
pub struct MsrPowerAsync {
    req_tx: Option<std::sync::mpsc::Sender<u64>>,
    reply_rx: Option<std::sync::mpsc::Receiver<MsrFrame>>,
    /// 上一次请求还没回来（工作线程仍在设备调用里）。
    in_flight: bool,
    timeouts: u32,
    disabled: bool,
}

impl Default for MsrPowerAsync {
    fn default() -> Self {
        Self::new()
    }
}

impl MsrPowerAsync {
    pub fn new() -> Self {
        Self {
            req_tx: None,
            reply_rx: None,
            in_flight: false,
            timeouts: 0,
            disabled: false,
        }
    }

    /// 取一次功耗读数。返回 `None` = 本帧无可信读数（超时 / 已放弃 / 工作线程在路上），
    /// 调用方按既有语义把功耗行置 `Unavailable`。
    ///
    /// 本函数**不会阻塞超过 `timeout_ms`**。
    pub fn sample(&mut self, now_ms: u64, timeout_ms: u64) -> Option<MsrFrame> {
        use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
        if self.disabled {
            return None;
        }
        if self.in_flight {
            // 上一轮派出去的任务还没回来：不重复派单。若回包已在队列里，说明
            // 那次阻塞调用最终返回了 —— 收下迟到结果、复位状态，下一帧重新计时。
            match self.reply_rx.as_ref().map(|rx| rx.try_recv()) {
                Some(Ok(_)) => self.in_flight = false,
                Some(Err(TryRecvError::Disconnected)) => {
                    self.in_flight = false;
                    self.drop_channels();
                }
                Some(Err(TryRecvError::Empty)) => {}
                None => self.in_flight = false,
            }
            return None;
        }
        if self.req_tx.is_none() {
            self.spawn_worker();
        }
        // 借用只在块内有效：块结束后才能改 self（记超时 / 放弃）。
        let outcome = match (self.req_tx.as_ref(), self.reply_rx.as_ref()) {
            (Some(tx), Some(rx)) => {
                // 丢弃任何迟到的回包：只接受本次请求的结果，避免把上一轮的旧窗口当成新值。
                while rx.try_recv().is_ok() {}
                if tx.send(now_ms).is_err() {
                    Err(RecvTimeoutError::Disconnected)
                } else {
                    rx.recv_timeout(std::time::Duration::from_millis(timeout_ms.max(1)))
                }
            }
            _ => Err(RecvTimeoutError::Disconnected),
        };
        match outcome {
            Ok(frame) => {
                self.timeouts = 0;
                Some(frame)
            }
            Err(RecvTimeoutError::Timeout) => {
                self.in_flight = true;
                self.timeouts = self.timeouts.saturating_add(1);
                probe::log(&format!(
                    "msr timeout after {timeout_ms}ms (consecutive={}) — 功耗行本帧跳过，其余指标不受影响",
                    self.timeouts
                ));
                if self.timeouts >= MSR_TIMEOUT_GIVEUP {
                    self.disable("连续超时");
                }
                None
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.drop_channels();
                None
            }
        }
    }

    fn spawn_worker(&mut self) {
        let (req_tx, req_rx) = std::sync::mpsc::channel::<u64>();
        let (reply_tx, reply_rx) = std::sync::mpsc::channel::<MsrFrame>();
        let spawned = std::thread::Builder::new()
            .name("msr-power".into())
            .spawn(move || msr_worker(req_rx, reply_tx));
        match spawned {
            Ok(_) => {
                self.req_tx = Some(req_tx);
                self.reply_rx = Some(reply_rx);
            }
            Err(e) => {
                probe::log(&format!("msr: 无法启动功耗线程（{e}）→ 本会话关闭功耗行"));
                self.disable("线程创建失败");
            }
        }
    }

    fn drop_channels(&mut self) {
        self.req_tx = None;
        self.reply_rx = None;
    }

    fn disable(&mut self, why: &str) {
        if self.disabled {
            return; // 已经降级过：degrade 流要求"每个降级动作只记一次"
        }
        self.disabled = true;
        // 丢弃请求通道：工作线程要么立刻 recv 到断开，要么从卡住的设备调用返回后
        // 发现回包通道已死而退出；两条路都不会留下长期驻留的线程。
        self.drop_channels();
        self.in_flight = false;
        probe::log(&format!(
            "msr: {why} → 本会话停止功耗采样（行显示 --，重启应用恢复）"
        ));
        // 降级事件默认可见：调试日志关着也要留下"这一整行的 `--` 是从何时、为何起"。
        probe::degrade(&format!(
            "degrade: MSR 功耗源本会话放弃（{why}）—— 功耗行将持续显示 --，重启应用后重试"
        ));
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    /// 采样热路径的锁**中毒后仍不得 panic**（审查项 7.2.1 / P1-20）。
    ///
    /// 只测 `lock_ring0()`，**不测 `ring0()`**：后者会真的 `OpenSCManager` 并尝试
    /// 打开/启动内核驱动设备，单元测试不该带这种副作用（且在管理员机器上会留下服务状态）。
    #[test]
    fn poisoned_ring0_lock_still_usable() {
        let h = std::thread::spawn(|| {
            let _g = RING0.lock().unwrap_or_else(|e| e.into_inner());
            panic!("故意在持锁时 panic，制造中毒状态");
        });
        let _ = h.join();
        assert!(RING0.is_poisoned(), "前置条件失败：锁未被投毒");
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut g = lock_ring0();
            g.take(); // 顺带复位，不把句柄留给后续测试
        }));
        RING0.clear_poison(); // 中毒是进程级状态，别外溢给同 crate 的其它测试
        assert!(
            outcome.is_ok(),
            "lock_ring0() 在中毒时 panic —— 采样线程会从此每轮死亡、五个指标永久定格"
        );
    }
}

