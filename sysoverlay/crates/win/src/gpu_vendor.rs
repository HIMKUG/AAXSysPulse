//! 显卡厂商兼容模块（用户拍板：功耗+利用率全走厂商，D3DKMT 兜底）。
//!
//! # 三家后端
//! - **NVIDIA**：NVML（`nvidia-ml.dll`，随驱动安装，纯用户态）——利用率
//!   `nvmlDeviceGetUtilizationRates` + 功耗 `nvmlDeviceGetPowerUsage`(mW)。
//! - **AMD**：legacy ADL（`atiadlxx.dll`，随驱动安装）——多候选功耗接口
//!   （OD6 CurrentPowerData / ODN PerformanceStatus）按序尝试，**原始结构体
//!   整体打进探针日志**，用真实硬件现场收敛字段偏移（RDNA 各代布局不同）。
//! - **Intel**：IGCL（`igcl64.dll`）——先做加载/枚举/探针骨架，属性解析待
//!   真机验证后补齐（结构体版本号盲写风险高，宁缺毋滥）。
//!
//! # 通用规则
//! - 后端按独显检测结果懒初始化（`OnceLock`），加载失败永久降级 → 返回
//!   `(None, None)` → 利用率走 D3DKMT、功耗记 Unavailable；
//! - 每次失败都记探针（`SYSOVERLAY_PROBE=1`）。

use crate::gpu_detect;
use crate::probe;
use std::ffi::c_void;
use std::sync::OnceLock;

/// 一轮厂商读数：(利用率%, 功耗W)。
pub type VendorReading = (Option<f32>, Option<f32>);

enum Backend {
    Nvml(Nvml),
    Adl(Box<Adl>),
    Igcl(Igcl),
    None,
}

static BACKEND: OnceLock<Backend> = OnceLock::new();
static GPU_WATTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn backend() -> &'static Backend {
    BACKEND.get_or_init(|| {
        let Some(gpu) = gpu_detect::pick_dgpu() else {
            probe::log("vendor no dgpu -> backend None");
            return Backend::None;
        };
        match gpu.vendor_id {
            gpu_detect::VENDOR_NVIDIA => match Nvml::load(&gpu) {
                Some(b) => Backend::Nvml(b),
                None => Backend::None,
            },
            gpu_detect::VENDOR_AMD => match Adl::load(&gpu) {
                Some(b) => Backend::Adl(Box::new(b)),
                None => Backend::None,
            },
            gpu_detect::VENDOR_INTEL => match Igcl::load(&gpu) {
                Some(b) => Backend::Igcl(b),
                None => Backend::None,
            },
            _ => Backend::None,
        }
    })
}

pub fn sample() -> VendorReading {
    let r = match backend() {
        Backend::Nvml(b) => b.sample(),
        Backend::Adl(b) => b.sample(),
        Backend::Igcl(b) => b.sample(),
        Backend::None => (None, None),
    };
    if let Some(w) = r.1 {
        GPU_WATTS.store((w * 100.0).clamp(0.0, u32::MAX as f32) as u32,
            std::sync::atomic::Ordering::Relaxed);
    }
    r
}

pub fn cached_watts() -> Option<f32> {
    let v = GPU_WATTS.load(std::sync::atomic::Ordering::Relaxed);
    if v == 0 { None } else { Some(v as f32 / 100.0) }
}

fn load_library(name: &str) -> Option<windows::Win32::Foundation::HMODULE> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let local = exe_dir.join(name);
    let wide: Vec<u16> = local.to_string_lossy().encode_utf16().chain(Some(0)).collect();
    unsafe {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
        if let Ok(h) = LoadLibraryW(windows::core::PCWSTR(wide.as_ptr())) {
            return Some(h);
        }
        let w: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        if let Ok(h) = LoadLibraryW(windows::core::PCWSTR(w.as_ptr())) {
            return Some(h);
        }
        GetModuleHandleW(windows::core::PCWSTR(w.as_ptr())).ok()
    }
}

pub fn get_proc_pub(h: windows::Win32::Foundation::HMODULE, name: &str) -> *mut c_void {
    unsafe {
        let mut n = String::with_capacity(name.len() + 1);
        n.push_str(name);
        n.push('\0');
        windows::Win32::System::LibraryLoader::GetProcAddress(
            h, windows::core::PCSTR(n.as_ptr() as *const u8))
            .map(|f| f as *mut c_void)
            .unwrap_or(std::ptr::null_mut())
    }
}

fn as_fn<T>(ptr: *mut c_void) -> Option<T> {
    if ptr.is_null() { None } else { Some(unsafe { std::mem::transmute_copy(&ptr) }) }
}

unsafe impl Send for Nvml {}
unsafe impl Sync for Nvml {}
unsafe impl Send for Adl {}
unsafe impl Sync for Adl {}
unsafe impl Send for Igcl {}
unsafe impl Sync for Igcl {}

// ─────────────────────────── NVIDIA: NVML ───────────────────────────

type FnInit = Option<unsafe extern "system" fn() -> i32>;
type FnGetCount = Option<unsafe extern "system" fn(*mut u32) -> i32>;
type FnGetByIndex = Option<unsafe extern "system" fn(u32, *mut *mut c_void) -> i32>;
type FnGetUtil = Option<unsafe extern "system" fn(*mut c_void, *mut NvmlUtil) -> i32>;
type FnGetPower = Option<unsafe extern "system" fn(*mut c_void, *mut u32) -> i32>;
type FnGetPci = Option<unsafe extern "system" fn(*mut c_void, *mut NvmlPci) -> i32>;
type FnShutdown = Option<unsafe extern "system" fn() -> i32>;

#[repr(C)] struct NvmlUtil { gpu: u32, memory: u32 }
#[repr(C)] struct NvmlPci {
    bus_id: [u8; 32], device_id: u32, sub_system_id: u32,
    bus: u32, device: u32, domain: u32,
}

pub struct Nvml { dev: *mut c_void, get_util: FnGetUtil, get_power: FnGetPower, shutdown: FnShutdown }

impl Nvml {
    fn load(gpu: &gpu_detect::GpuInfo) -> Option<Self> {
        unsafe {
            let h = load_library("nvidia-ml.dll")?;
            let init: FnInit = as_fn(get_proc_pub(h, "nvmlInit_v2"));
            let get_count: FnGetCount = as_fn(get_proc_pub(h, "nvmlDeviceGetCount_v2"));
            let get_by_index: FnGetByIndex = as_fn(get_proc_pub(h, "nvmlDeviceGetHandleByIndex_v2"));
            let get_pci: FnGetPci = as_fn(get_proc_pub(h, "nvmlDeviceGetPciInfo_v3"));
            let get_util: FnGetUtil = as_fn(get_proc_pub(h, "nvmlDeviceGetUtilizationRates"));
            let get_power: FnGetPower = as_fn(get_proc_pub(h, "nvmlDeviceGetPowerUsage"));
            let shutdown: FnShutdown = as_fn(get_proc_pub(h, "nvmlShutdown"));
            let (init, get_count, get_by_index, get_util, get_power) =
                match (init, get_count, get_by_index, get_util, get_power) {
                    (Some(a), Some(b), Some(c), Some(d), Some(e)) => (a, b, c, d, e),
                    _ => { probe::log("nvml missing exports"); return None; }
                };
            if init() != 0 { probe::log("nvml init failed"); return None; }
            let mut count = 0u32;
            if get_count(&mut count) != 0 || count == 0 { probe::log("nvml no devices"); return None; }
            let mut dev: *mut c_void = std::ptr::null_mut();
            for i in 0..count {
                let mut d: *mut c_void = std::ptr::null_mut();
                if get_by_index(i, &mut d) != 0 { continue; }
                if let Some(gp) = get_pci {
                    let mut pci = NvmlPci { bus_id: [0; 32], device_id: 0, sub_system_id: 0, bus: 0, device: 0, domain: 0 };
                    if gp(d, &mut pci) == 0 && pci.device_id & 0xFFFF == gpu.device_id { dev = d; break; }
                }
                if dev.is_null() { dev = d; }
            }
            if dev.is_null() { probe::log("nvml no usable device"); return None; }
            probe::log("nvml backend ready");
            Some(Self { dev, get_util: Some(get_util), get_power: Some(get_power), shutdown })
        }
    }

    fn sample(&self) -> VendorReading {
        unsafe {
            let mut util = NvmlUtil { gpu: 0, memory: 0 };
            let u = self.get_util.and_then(|f|
                if f(self.dev, &mut util) == 0 { Some(util.gpu as f32) } else { None });
            let mut mw = 0u32;
            let w = self.get_power.and_then(|f|
                if f(self.dev, &mut mw) == 0 { Some(mw as f32 / 1000.0) } else { None });
            (u, w)
        }
    }
}

impl Drop for Nvml {
    fn drop(&mut self) { if let Some(f) = self.shutdown { unsafe { f() }; } }
}

// ─────────────────────────── AMD: legacy ADL ───────────────────────────

type AdlCtx = *mut c_void;
type AdlMalloc = unsafe extern "system" fn(i32) -> *mut c_void;
type FnMainCreate = Option<unsafe extern "system" fn(i32, *mut AdlCtx) -> i32>;
type FnMainCreate2 = Option<unsafe extern "system" fn(AdlMalloc, i32, *mut AdlCtx) -> i32>;
type FnMainDestroy = Option<unsafe extern "system" fn(AdlCtx) -> i32>;
type FnNumAdapters2 = Option<unsafe extern "system" fn(AdlCtx, *mut i32) -> i32>;
type FnNumAdapters = Option<unsafe extern "system" fn(*mut i32) -> i32>;
type FnAdapterInfo = Option<unsafe extern "system" fn(*mut c_void, i32) -> i32>;
type FnAdapterInfo2 = Option<unsafe extern "system" fn(AdlCtx, *mut c_void, i32) -> i32>;
type FnOd6Power = Option<unsafe extern "system" fn(AdlCtx, i32, i32, *mut c_void) -> i32>;
type FnOdnStatus = Option<unsafe extern "system" fn(AdlCtx, i32, *mut c_void) -> i32>;

/// ODN PerformanceStatus 里功耗字段的下标（i32 数组）。
/// 待真实硬件首轮探针日志收敛后固化（RX 9070 GRE = RDNA4）。
const ADL_ODN_POWER_IDX: usize = 26;

/// 前几轮打印原始数据供调试（收敛后不再打印）。
const ADL_DUMP_ROUNDS: u8 = 3;

/// `sizeof(AdapterInfo)` 候选值。
///
/// ADL 各版本结构体长度不同（字段随 SDK 演进追加），写死一个值会在
/// 新版驱动上拿到 `ADL_ERR_INVALID_PARAM`(-3)。按序试探，取首个被
/// 驱动接受的；每个候选都分配独立缓冲，避免污染。
const ADL_INFO_SIZE_CANDIDATES: &[usize] = &[1576, 1592, 1600, 2048, 1456, 728, 1024];

/// strUDID 在 AdapterInfo 内的偏移与长度（各版本一致，位于结构体头部）。
const ADL_UDID_OFF: usize = 8;
const ADL_UDID_LEN: usize = 256;
/// iAdapterIndex 偏移（结构体第二个字段）。
const ADL_INDEX_OFF: usize = 4;

pub struct Adl {
    ctx: AdlCtx,
    adapter_index: i32,
    od6_power: FnOd6Power,
    odn_status: FnOdnStatus,
    destroy: FnMainDestroy,
    dump_count: std::sync::atomic::AtomicU8,
}

impl Adl {
    fn load(gpu: &gpu_detect::GpuInfo) -> Option<Self> {
        unsafe {
            let h = load_library("atiadlxx.dll")?;

            // ADL2_Main_Control_Create + LocalAlloc（推荐，ADL2 上下文）
            let create2: FnMainCreate2 = as_fn(get_proc_pub(h, "ADL2_Main_Control_Create"));
            // ADL_Main_Control_Create（传统，无 malloc 回调）
            let create: FnMainCreate = as_fn(get_proc_pub(h, "ADL_Main_Control_Create"));
            let destroy_opt: FnMainDestroy = as_fn(get_proc_pub(h, "ADL2_Main_Control_Destroy"));
            let num_plain: FnNumAdapters = as_fn(get_proc_pub(h, "ADL_Adapter_NumberOfAdapters_Get"));
            let num2: FnNumAdapters2 = as_fn(get_proc_pub(h, "ADL2_Adapter_NumberOfAdapters_Get"));
            let info_plain: FnAdapterInfo = as_fn(get_proc_pub(h, "ADL_Adapter_AdapterInfo_Get"));
            let info2: FnAdapterInfo2 = as_fn(get_proc_pub(h, "ADL2_Adapter_AdapterInfo_Get"));
            let od6: FnOd6Power = as_fn(get_proc_pub(h, "ADL2_Overdrive6_CurrentPowerData_Get"));
            let odn: FnOdnStatus = as_fn(get_proc_pub(h, "ADL2_OverdriveN_Performance_Status_Get"));

            if destroy_opt.is_none() || (num_plain.is_none() && num2.is_none()) {
                probe::log("adl missing exports"); return None;
            }

            let mut ctx: AdlCtx = std::ptr::null_mut();
            let mut use_adl2 = false;

            if let Some(c2) = create2 {
                unsafe extern "system" fn adl_alloc(size: i32) -> *mut c_void {
                    if size <= 0 { std::ptr::null_mut() }
                    else {
                        let layout = std::alloc::Layout::from_size_align(size as usize, 8).unwrap();
                        std::alloc::alloc(layout) as *mut c_void
                    }
                }
                if c2(adl_alloc, 1, &mut ctx) == 0 && !ctx.is_null() {
                    use_adl2 = true;
                }
            }

            if !use_adl2 {
                if let Some(c) = create {
                    if c(1, &mut ctx) == 0 && !ctx.is_null() {
                        // keep use_adl2 = false
                    } else {
                        ctx = std::ptr::null_mut();
                    }
                }
                if ctx.is_null() {
                    probe::log("adl main_control_create failed"); return None;
                }
            }

            let mut n = 0i32;
            let num_ok = if use_adl2 {
                if let Some(f) = num2 { f(ctx, &mut n) == 0 && n > 0 } else { false }
            } else {
                if let Some(f) = num_plain { f(&mut n) == 0 && n > 0 } else { false }
            };
            if !num_ok {
                probe::log("adl adapter count failed"); return None;
            }

            // 逐个候选 sizeof(AdapterInfo) 试探，取驱动接受的第一个。
            let mut filled: Option<(Vec<u8>, usize)> = None;
            for &stride in ADL_INFO_SIZE_CANDIDATES {
                let total = stride * n as usize;
                let mut buf = vec![0u8; total];
                let ret = if use_adl2 {
                    if let Some(f) = info2 { f(ctx, buf.as_mut_ptr() as *mut c_void, total as i32) } else { -1 }
                } else {
                    if let Some(f) = info_plain { f(buf.as_mut_ptr() as *mut c_void, total as i32) } else { -1 }
                };
                if probe::enabled() {
                    probe::log(&format!("adl try stride={stride} total={total} ret={ret}"));
                }
                if ret == 0 {
                    // 结构体首字段 iSize 就是驱动眼中的真实 sizeof。
                    // 候选值只用于让驱动**接受**这次调用，遍历必须按真实步长：
                    // 实测候选 1576 被接受、而真实 sizeof=1572，若按候选遍历，
                    // 每个条目会累积 +4 字节偏移，UDID 被逐条"削掉"前缀
                    // （PCI_VEN_1002 → VEN_1002 → 1002 → &DEV_...），
                    // 独显的 DEV_7550 因此永远匹配不上。
                    let isize = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                    let real = if (512..=8192).contains(&isize) { isize } else { stride };
                    if probe::enabled() {
                        probe::log(&format!(
                            "adl stride candidate={stride} iSize={isize} use={real}"
                        ));
                    }
                    filled = Some((buf, real));
                    break;
                }
            }
            let Some((buf, stride)) = filled else {
                probe::log("adl adapter_info_get failed for all candidate strides"); return None;
            };

            // 按 strUDID 匹配（"PCI_VEN_1002&DEV_7550&SUBSYS_..."），
            // 不依赖 iVendorID 等字段偏移——那些偏移随 ADL 版本漂移。
            let mut adapter_index: Option<i32> = None;
            for k in 0..n as usize {
                let base = stride * k;
                if base + ADL_UDID_OFF + ADL_UDID_LEN > buf.len() {
                    break;
                }
                let idx = unsafe {
                    std::ptr::read_unaligned(buf.as_ptr().add(base + ADL_INDEX_OFF) as *const i32)
                };
                let udid = &buf[base + ADL_UDID_OFF..base + ADL_UDID_OFF + ADL_UDID_LEN];
                let ids = parse_udid_ids(udid);
                if probe::enabled() {
                    probe::log(&format!(
                        "adl adapter {k} idx={idx} udid={:?} ids={ids:?} want=(0x{:04x},0x{:04x})",
                        String::from_utf8_lossy(&udid[..udid.iter().position(|&c| c == 0).unwrap_or(32)]),
                        gpu.vendor_id, gpu.device_id
                    ));
                }
                if let Some((v, d)) = ids {
                    if v == gpu.vendor_id && d == gpu.device_id && adapter_index.is_none() {
                        adapter_index = Some(idx);
                    }
                }
            }

            let Some(adapter_index) = adapter_index else {
                probe::log("adl dgpu adapter not found"); return None;
            };

            probe::log(&format!(
                "adl backend ready adapter_index={adapter_index} use_adl2={use_adl2} od6={} odn={}",
                od6.is_some(), odn.is_some()
            ));
            Some(Self { ctx, adapter_index, od6_power: od6, odn_status: odn,
                        destroy: destroy_opt, dump_count: std::sync::atomic::AtomicU8::new(0) })
        }
    }

    fn sample(&self) -> VendorReading {
        unsafe {
            let dominated = self.dump_count.load(std::sync::atomic::Ordering::Relaxed) < ADL_DUMP_ROUNDS;
            let mut watts: Option<f32> = None;

            // 候选 1：ODN PerformanceStatus（RDNA 主路径）
            if let Some(odn) = self.odn_status {
                let mut raw = [0i32; 64];
                raw[0] = (raw.len() * 4) as i32;
                let ret = odn(self.ctx, self.adapter_index, raw.as_mut_ptr() as *mut c_void);
                if ret == 0 {
                    if probe::enabled() && dominated {
                        let dump: Vec<String> = raw.iter().take(40).enumerate()
                            .map(|(i, v)| format!("{i}:{v}")).collect();
                        probe::log(&format!("adl odn_status raw {}", dump.join(" ")));
                    }
                    // 收敛后启用提取（idx=26, 0.25W/LSB，实测 RX 9070 GRE 在 10-60 范围）
                    if !dominated {
                        watts = Some(raw[ADL_ODN_POWER_IDX] as f32 * 0.25);
                    }
                } else if probe::enabled() {
                    probe::log(&format!("adl odn_status ret={ret}"));
                }
            }

            // 候选 2：OD6 CurrentPowerData（GCN~RDNA2 路径）
            if watts.is_none() {
                if let Some(od6) = self.od6_power {
                    if dominated {
                        for (ptype, isize_bytes) in [(0i32, 28i32), (1, 28), (0, 24), (0, 32)] {
                            let mut raw = [0i32; 16];
                            raw[0] = isize_bytes;
                            let ret = od6(self.ctx, self.adapter_index, ptype, raw.as_mut_ptr() as *mut c_void);
                            if ret == 0 {
                                let dump: Vec<String> = raw.iter().take(10).enumerate()
                                    .map(|(i, v)| format!("{i}:{v}")).collect();
                                probe::log(&format!(
                                    "adl od6_power ptype={ptype} isize={isize_bytes} ret=0 raw {}",
                                    dump.join(" ")));
                                break;
                            } else if probe::enabled() {
                                probe::log(&format!("adl od6_power ptype={ptype} ret={ret}"));
                            }
                        }
                    }
                }
            }

            if dominated {
                self.dump_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            (None, watts)
        }
    }
}

impl Drop for Adl {
    fn drop(&mut self) { if let Some(f) = self.destroy { unsafe { f(self.ctx) }; } }
}

// ─────────────────────────── Intel: IGCL ───────────────────────────

pub struct Igcl { _init_ok: bool }

impl Igcl {
    fn load(_gpu: &gpu_detect::GpuInfo) -> Option<Self> {
        let h = load_library("igcl64.dll")?;
        let ctl_init = get_proc_pub(h, "ctlInit");
        let ctl_enum = get_proc_pub(h, "ctlEnumerateDevices");
        probe::log(&format!(
            "igcl loaded exports init={} enum={}",
            !ctl_init.is_null(), !ctl_enum.is_null()
        ));
        Some(Self { _init_ok: !ctl_init.is_null() })
    }
    fn sample(&self) -> VendorReading { (None, None) }
}

// ─────────────────────────── 小工具 ───────────────────────────

/// 从 `strUDID` 解析 (VendorID, DeviceID)。
///
/// UDID 形如 `PCI_VEN_1002&DEV_7550&SUBSYS_1EA21002&REV_C0`，
/// 也有驱动写成小写或省略 `PCI_` 前缀。解析失败返回 None（该条目跳过）。
fn parse_udid_ids(udid: &[u8]) -> Option<(u32, u32)> {
    let end = udid.iter().position(|&c| c == 0).unwrap_or(udid.len());
    let s = std::str::from_utf8(&udid[..end]).ok()?;
    let up = s.to_ascii_uppercase();

    let after = |key: &str| -> Option<&str> {
        let p = up.find(key)?;
        Some(&up[p + key.len()..])
    };
    let hex4 = |s: &str| -> Option<u32> {
        let t: String = s.chars().take_while(|c| c.is_ascii_hexdigit()).take(4).collect();
        if t.len() == 4 { u32::from_str_radix(&t, 16).ok() } else { None }
    };

    let ven = hex4(after("VEN_")?)?;
    let dev = hex4(after("DEV_")?)?;
    Some((ven, dev))
}
