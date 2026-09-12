//! 显卡厂商兼容模块（用户拍板：功耗+利用率全走厂商，D3DKMT 兜底）。
//!
//! # 三家后端
//! - **NVIDIA**：NVML（`nvidia-ml.dll`，随驱动安装，纯用户态）——利用率
//!   `nvmlDeviceGetUtilizationRates` + 功耗 `nvmlDeviceGetPowerUsage`(mW)。
//! - **AMD**：legacy ADL（`atiadlxx.dll`，随驱动安装）——RDNA 走 **PMLog 一次
//!   查询** `ADL2_New_QueryPMLogData_Get`（传感器 ID 直接索引，单位瓦：
//!   BOARD_POWER=73 优先、回退 ASIC_POWER=23 / GFX+SOC），老卡回退
//!   `ADL2_Overdrive6_CurrentPower_Get`（8.8 定点，>>8 得瓦）。适配器匹配用
//!   **UDID 锚点扫描**（新版驱动 AdapterInfo.iSize 填 0，不能当真实 sizeof 用，
//!   步长错 4 字节会让每条记录漂移、dGPU 永远匹配不上——本机实测教训）。
//! - **Intel**：IGCL（`igcl64.dll`）——先做加载/枚举/探针骨架，属性解析待
//!   真机验证后补齐（结构体版本号盲写风险高，宁缺毋滥）。
//!
//! # 通用规则
//! - 后端按独显检测结果懒初始化（`OnceLock`），加载失败永久降级 → 返回
//!   `(None, None)` → 利用率走 D3DKMT、功耗记 Unavailable；
//! - **多显卡共同工作**：每张独显各读一次功耗并累加（整机显存总功耗），
//!   利用率以主卡（第一张）为准；Intel 后端为骨架暂不读功耗；
//! - 每次失败都记探针（`SYSOVERLAY_PROBE=1`）。

use crate::gpu_detect;
use crate::perf;
use crate::probe;
use std::ffi::c_void;
use std::sync::OnceLock;

/// 一轮厂商读数：(利用率%, 功耗W)。
pub type VendorReading = (Option<f32>, Option<f32>);

/// 多后端集合：按厂商懒初始化，可同时持有多个厂商后端。
/// 混合机器（如 NVIDIA 独显 + AMD 核显）两套后端并行采样，各卡功耗都能纳入。
struct BackendSet {
    nvml: Option<Nvml>,
    adl: Option<Box<Adl>>,
    igcl: Option<Igcl>,
}

static BACKEND: OnceLock<BackendSet> = OnceLock::new();
static GPU_WATTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// GPU 利用率缓存（0xFFFF_FFFF = 无缓存）；用 u32 bitpattern 而非 f32 避免 0.0 歧义。
const GPU_UTIL_NONE: u32 = 0xFFFF_FFFF;
static GPU_UTIL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(GPU_UTIL_NONE);

fn backend() -> &'static BackendSet {
    BACKEND.get_or_init(|| {
        // 只要机器上存在某厂商的显卡（含核显）就加载该厂商后端：
        // 多卡机器（同厂商多卡 / 混合厂商 / 独显+核显）每张卡的功耗都能统计。
        let gpus = gpu_detect::enum_adapters();
        let has = |v: u32| gpus.iter().any(|g| g.vendor_id == v);
        BackendSet {
            nvml: if has(gpu_detect::VENDOR_NVIDIA) { Nvml::load() } else { None },
            adl: if has(gpu_detect::VENDOR_AMD) { Adl::load().map(Box::new) } else { None },
            igcl: if has(gpu_detect::VENDOR_INTEL) { Igcl::load() } else { None },
        }
    })
}

/// 合并一轮读数：利用率取较大的那个，功耗相加。
fn merge_reading(dst: &mut VendorReading, (u, w): VendorReading) {
    if let Some(ui) = u {
        dst.0 = Some(dst.0.map_or(ui, |cur| cur.max(ui)));
    }
    if let Some(wi) = w {
        dst.1 = Some(dst.1.map_or(wi, |cur| cur + wi));
    }
}

pub fn sample() -> VendorReading {
    let t0 = perf::begin();
    let bs = backend();
    let mut r: VendorReading = (None, None);
    if let Some(b) = &bs.nvml {
        let t1 = perf::begin();
        let rv = b.sample();
        perf::note_section("gpunvml", t1);
        merge_reading(&mut r, rv);
    }
    if let Some(b) = &bs.adl {
        let t1 = perf::begin();
        let rv = b.sample();
        perf::note_section("gpuadl", t1);
        merge_reading(&mut r, rv);
    }
    if let Some(b) = &bs.igcl {
        let t1 = perf::begin();
        let rv = b.sample();
        perf::note_section("gpuigcl", t1);
        merge_reading(&mut r, rv);
    }
    perf::note_section("gpusample", t0);
    if let Some(w) = r.1 {
        GPU_WATTS.store((w * 100.0).clamp(0.0, u32::MAX as f32) as u32,
            std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(u) = r.0 {
        // 利用率也缓存（D3DKMT 约 74ms，vendor 成功时直接缓存复用）
        GPU_UTIL.store(u.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }
    r
}

/// 返回缓存的 GPU 利用率（None = 无缓存）。
/// 当 vendor 不可用时由 D3DKMT 采样结果填充。
pub fn cached_util() -> Option<f32> {
    let v = GPU_UTIL.load(std::sync::atomic::Ordering::Relaxed);
    if v == GPU_UTIL_NONE { None } else { Some(f32::from_bits(v)) }
}

/// 设置缓存的 GPU 利用率（由 D3DKMT 采样器调用）。
pub fn set_cached_util(u: f32) {
    GPU_UTIL.store(u.to_bits(), std::sync::atomic::Ordering::Relaxed);
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
type FnShutdown = Option<unsafe extern "system" fn() -> i32>;

#[repr(C)] struct NvmlUtil { gpu: u32, memory: u32 }

pub struct Nvml {
    /// 全部 NVIDIA 独显设备句柄（多卡时逐张累加功耗）。
    devs: Vec<*mut c_void>,
    get_util: FnGetUtil,
    get_power: FnGetPower,
    shutdown: FnShutdown,
}

impl Nvml {
    fn load() -> Option<Self> {
        unsafe {
            let h = load_library("nvidia-ml.dll")?;
            let init: FnInit = as_fn(get_proc_pub(h, "nvmlInit_v2"));
            let get_count: FnGetCount = as_fn(get_proc_pub(h, "nvmlDeviceGetCount_v2"));
            let get_by_index: FnGetByIndex = as_fn(get_proc_pub(h, "nvmlDeviceGetHandleByIndex_v2"));
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
            // 收集全部 NVIDIA 卡（多卡共同工作 ⇒ 功耗需逐张累加）。
            let mut devs: Vec<*mut c_void> = Vec::with_capacity(count as usize);
            for i in 0..count {
                let mut d: *mut c_void = std::ptr::null_mut();
                if get_by_index(i, &mut d) != 0 { continue; }
                devs.push(d);
            }
            if devs.is_empty() { probe::log("nvml no usable device"); return None; }
            probe::log(&format!("nvml backend ready gpus={}", devs.len()));
            Some(Self { devs, get_util: Some(get_util), get_power: Some(get_power), shutdown })
        }
    }

    fn sample(&self) -> VendorReading {
        unsafe {
            // 利用率取第一张卡（多卡以主卡为准，功耗累加）。
            let mut util = NvmlUtil { gpu: 0, memory: 0 };
            let u = if let Some(&dev) = self.devs.first() {
                self.get_util.and_then(|f|
                    if f(dev, &mut util) == 0 { Some(util.gpu as f32) } else { None })
            } else { None };
            // 功耗逐张累加（多卡共同工作 ⇒ 整机显存总功耗）。
            let mut total_w = 0.0;
            let mut any = false;
            for &dev in &self.devs {
                let mut mw = 0u32;
                if let Some(f) = self.get_power {
                    if f(dev, &mut mw) == 0 {
                        total_w += mw as f32 / 1000.0;
                        any = true;
                    }
                }
            }
            (u, if any { Some(total_w) } else { None })
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
/// ADL2_Overdrive_Caps(context, adapter, *supported, *enabled, *version)
type FnOdCaps = Option<unsafe extern "system" fn(AdlCtx, i32, *mut i32, *mut i32, *mut i32) -> i32>;
/// ADL2_New_QueryPMLogData_Get(context, adapter, *ADLPMLogDataOutput)
type FnQueryPMLog = Option<unsafe extern "system" fn(AdlCtx, i32, *mut c_void) -> i32>;
/// ADL2/ADL_Overdrive6_CurrentPower_Get(context, adapter, type, *value)
type FnOd6Power = Option<unsafe extern "system" fn(AdlCtx, i32, i32, *mut i32) -> i32>;

// ADL_PMLOG_SENSORS（ADL SDK adl_defines.h，LHM AtiAdlxx 同源）
const ADL_PMLOG_ASIC_POWER: i32 = 23;
const ADL_PMLOG_SOC_POWER: i32 = 17;
const ADL_PMLOG_GFX_POWER: i32 = 30;
const ADL_PMLOG_BOARD_POWER: i32 = 73;
const ADL_PMLOG_INFO_ACTIVITY_GFX: i32 = 19;
const ADL_PMLOG_MAX_SENSORS: usize = 256;
/// ADLPMLogDataOutput：{ int size; struct { int supported; int value; } sensors[256]; }
/// （LibreHardwareMonitor AtiAdlxx.cs 同布局；按传感器 ID 直接索引。）
const ADL_PMLOG_OUT_SIZE: usize = 4 + ADL_PMLOG_MAX_SENSORS * 8;

/// ADLODNCurrentPowerType（8.8 定点，>>8 得瓦）
const ADL_OD6_TOTAL: i32 = 0;
const ADL_OD6_PPT: i32 = 1;

/// 成功后打印前几轮 PMLog 原始传感器值（供与 AMD 面板对照校准），之后静默。
const ADL_PROBE_ROUNDS: u8 = 5;

/// `sizeof(AdapterInfo)` 候选值——只用于**让驱动接受这次调用**（定缓冲大小）。
/// 真实步长不再依赖 iSize / 候选值：本机实测新版驱动把 iSize 填 0，
/// 候选 1576 而真实 1572，逐条 +4 漂移导致 UDID"削前缀"、dGPU 永远匹配不上。
const ADL_INFO_SIZE_CANDIDATES: &[usize] = &[1576, 1592, 1600, 2048, 1456, 728, 1024];

pub struct Adl {
    ctx: AdlCtx,
    /// 全部 AMD 独显适配器索引（多卡时逐张累加功耗）。
    adapter_indices: Vec<i32>,
    query_pmlog: FnQueryPMLog,
    od6_power: FnOd6Power,
    destroy: FnMainDestroy,
    probe_rounds: std::sync::atomic::AtomicU8,
}

impl Adl {
    fn load() -> Option<Self> {
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
            let od_caps: FnOdCaps = as_fn(get_proc_pub(h, "ADL2_Overdrive_Caps"));
            let query_pmlog: FnQueryPMLog =
                as_fn(get_proc_pub(h, "ADL2_New_QueryPMLogData_Get"));
            // OD6 功耗：ADL2 导出名优先，回退传统导出名
            let od6_power: FnOd6Power =
                as_fn(get_proc_pub(h, "ADL2_Overdrive6_CurrentPower_Get"))
                    .or_else(|| as_fn(get_proc_pub(h, "ADL_Overdrive6_CurrentPower_Get")));

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

            // 拿到一份驱动接受的 AdapterInfo 缓冲（候选值只定缓冲大小）。
            let mut filled: Option<Vec<u8>> = None;
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
                    filled = Some(buf);
                    break;
                }
            }
            let Some(buf) = filled else {
                probe::log("adl adapter_info_get failed for all candidate strides"); return None;
            };

            // UDID 锚点扫描：每条 AdapterInfo 都含 "VEN_"（strUDID 字段，
            // 起点在 iSize+iAdapterIndex 之后 = 记录头 +8）。相邻锚点即真实步长，
            // 锚点前 4 字节就是 iAdapterIndex——完全不依赖 iSize/候选步长。
            let entries = scan_udid_entries(&buf);
            let stride = if entries.len() >= 2 {
                Some(entries[1].0 - entries[0].0)
            } else {
                None
            };
            if probe::enabled() {
                probe::log(&format!(
                    "adl udid anchors={} stride={:?}",
                    entries.len(),
                    stride
                ));
            }

            // 收集全部 AMD 显卡适配器（含核显，多卡共同工作 ⇒ 功耗逐张累加）。
            // ⚠️ 2026-09-09 加固（"gpu≈1200W 假功耗"事故根因）：
            // UDID 锚点扫描对记录布局不稳定（同一记录内多次 "VEN_"、前缀 PCI_ 有无不定），
            // 会把垃圾字节当 iAdapterIndex → `adapters=[0, 1548305232, 1, ...]`，
            // 每个"伪索引"PMLog 都返回读数 → 5 张"卡"功耗逐张累加成 ~1200W。
            // 防御四连：index sanity（真实适配器索引是小序号）→ 去重 → 数量封顶
            // （不超过**唯一 device_id** 的 AMD 独显卡数）→ 功耗守卫（H2）。
            // 唯一 device_id：本机 DXGI 会把同一块 9070 GRE 枚举成 3 份重复条目，
            // 若直接按条目数封顶仍会累加 3 张"伪卡"功耗（20+19+22≈60W 偏高）。
            let mut want: Vec<(u32, u32)> = Vec::new();
            for g in gpu_detect::enum_adapters() {
                if g.vendor_id == gpu_detect::VENDOR_AMD
                    && !want.contains(&(g.vendor_id, g.device_id))
                {
                    want.push((g.vendor_id, g.device_id));
                }
            }
            let uniq_dgpu: Vec<u32> = {
                let mut v: Vec<u32> = Vec::new();
                for g in gpu_detect::list_dgpu() {
                    if g.vendor_id == gpu_detect::VENDOR_AMD && !v.contains(&g.device_id) {
                        v.push(g.device_id);
                    }
                }
                v
            };
            let max_cards = uniq_dgpu.len().max(1);

            let mut adapter_indices: Vec<i32> = Vec::new();
            for (off, idx) in &entries {
                let end = (*off + 256).min(buf.len());
                let udid = &buf[*off..end];
                let ids = parse_udid_ids(udid);
                // sanity：iAdapterIndex 一定是小序号（真实卡 0~63）；垃圾字节读出
                // 的 (如 1548305232) 直接丢弃，这是清除"伪功耗累加"的第一道闸。
                let sane = *idx >= 0 && *idx < 64;
                let dup = adapter_indices.contains(idx);
                if probe::enabled() {
                    let printable = &udid[..udid.iter().position(|&c| c == 0).unwrap_or(32).min(udid.len())];
                    probe::log(&format!(
                        "adl entry off={off} idx={idx} sane={sane} dup={dup} udid={:?} ids={ids:?} want={:?}",
                        String::from_utf8_lossy(printable),
                        want
                    ));
                }
                if let Some((v, d)) = ids {
                    if want.contains(&(v, d)) && sane && !dup
                        && adapter_indices.len() < max_cards
                    {
                        adapter_indices.push(*idx);
                    }
                }
            }

            if adapter_indices.is_empty() {
                probe::log("adl dgpu adapter not found"); return None;
            }

            // Overdrive 版本（诊断信息：RDNA 应为 8）
            if let Some(caps) = od_caps {
                let (mut sup, mut en, mut ver) = (0i32, 0i32, 0i32);
                if let Some(&idx) = adapter_indices.first() {
                    if caps(ctx, idx, &mut sup, &mut en, &mut ver) == 0 {
                        probe::log(&format!(
                            "adl overdrive_caps supported={sup} enabled={en} version={ver}"
                        ));
                    }
                }
            }

            probe::log(&format!(
                "adl backend ready adapters={:?} use_adl2={use_adl2} pmlog={} od6={}",
                adapter_indices, query_pmlog.is_some(), od6_power.is_some()
            ));
            Some(Self { ctx, adapter_indices, query_pmlog, od6_power,
                        destroy: destroy_opt, probe_rounds: std::sync::atomic::AtomicU8::new(0) })
        }
    }

    /// 逐张卡各查一次功耗并累加；利用率取第一张卡（主卡）为准。
    fn sample(&self) -> VendorReading {
        unsafe {
            let mut sum_w = 0.0;
            let mut any_w = false;
            let mut util: Option<f32> = None;

            for (k, &adapter_index) in self.adapter_indices.iter().enumerate() {
                let mut card_w: Option<f32> = None;
                let mut card_util: Option<f32> = None;

                // 主路径：PMLog 一次查询（OD8+/RDNA，传感器 ID 直接索引，单位瓦）。
                // 选取顺序与 AMD 面板"总板功率"对齐：BOARD → ASIC → GFX+SOC。
                // ⚠️ 每次查询前必须清零输出缓冲：部分驱动不会覆写全部 2KB，
                // 残留上次调用的传感器值会让无效索引也"读到功耗"（伪功耗累加的另一来源）。
                if let Some(q) = self.query_pmlog {
                    let mut out = [0u8; ADL_PMLOG_OUT_SIZE];
                    out[0..4].copy_from_slice(&(ADL_PMLOG_OUT_SIZE as i32).to_le_bytes());
                    let mut ret = q(self.ctx, adapter_index, out.as_mut_ptr() as *mut c_void);
                    if ret != 0 {
                        // 某些驱动校验 size==0（LHM 即以 0 传入），回退一次。
                        out = [0u8; ADL_PMLOG_OUT_SIZE];
                        ret = q(self.ctx, adapter_index, out.as_mut_ptr() as *mut c_void);
                    }
                    if ret == 0 {
                        let probing = self.probe_rounds.load(std::sync::atomic::Ordering::Relaxed)
                            < ADL_PROBE_ROUNDS;
                        if probing {
                            let dump = [ADL_PMLOG_BOARD_POWER, ADL_PMLOG_ASIC_POWER,
                                        ADL_PMLOG_GFX_POWER, ADL_PMLOG_SOC_POWER,
                                        ADL_PMLOG_INFO_ACTIVITY_GFX]
                                .map(|id| format!("{id}:{:?}", pmlog_sensor(&out, id)))
                                .join(" ");
                            probe::log(&format!("adl pmlog sensors idx={adapter_index} {dump}"));
                        }
                        let board = pmlog_sensor(&out, ADL_PMLOG_BOARD_POWER);
                        let asic = pmlog_sensor(&out, ADL_PMLOG_ASIC_POWER);
                        let gfx = pmlog_sensor(&out, ADL_PMLOG_GFX_POWER);
                        let soc = pmlog_sensor(&out, ADL_PMLOG_SOC_POWER);
                        card_w = match () {
                            _ if board.is_some() => board,
                            _ if asic.is_some() => asic,
                            _ => match (gfx, soc) {
                                (Some(g), Some(s)) => Some(g + s),
                                (g, s) => g.or(s),
                            },
                        }
                        .map(|w| w.max(0) as f32);
                        // 功耗守卫：单卡读数超 800W 判定为传感器垃圾（真实消费级卡
                        // 峰值 < 600W），丢弃并告警，不上污染数。
                        if let Some(w) = card_w {
                            if w > 800.0 {
                                probe::log(&format!(
                                    "adl board_power idx={adapter_index} value={w}W > 800W — discarded (bad sensor)"
                                ));
                                card_w = None;
                            }
                        }
                        card_util = pmlog_sensor(&out, ADL_PMLOG_INFO_ACTIVITY_GFX)
                            .map(|a| (a.max(0).min(100)) as f32);
                        if probing {
                            self.probe_rounds
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    } else if probe::enabled() {
                        probe::log(&format!("adl pmlog_query ret={ret} idx={adapter_index}"));
                    }
                }

                // 回退：OD6 CurrentPower（GCN~RDNA2 兼容路径，8.8 定点）。
                if card_w.is_none() {
                    if let Some(od6) = self.od6_power {
                        for (t, name) in [(ADL_OD6_TOTAL, "total"), (ADL_OD6_PPT, "ppt")] {
                            let mut v = 0i32;
                            if od6(self.ctx, adapter_index, t, &mut v) == 0 && v > 0 {
                                card_w = Some((v >> 8).max(0) as f32);
                                probe::log(&format!(
                                    "adl od6_power idx={adapter_index} type={name} raw8_8={v} watts={:?}",
                                    card_w
                                ));
                                break;
                            }
                        }
                    }
                }

                if let Some(w) = card_w {
                    sum_w += w;
                    any_w = true;
                }
                // 利用率只取主卡（第一张）。
                if k == 0 {
                    util = card_util;
                }
            }

            // 总功耗守卫：张数已知（≤ cap），单卡已被 800W 截断；总和超过
            // 800×张数 即说明仍混入伪读数，整体作废返回 None（不显示假功耗）。
            let total = if any_w && sum_w <= 800.0 * self.adapter_indices.len() as f32 {
                Some(sum_w)
            } else {
                if any_w {
                    probe::log(&format!(
                        "adl total power {sum_w:.1}W > {}×cards — discarded",
                        800.0 * self.adapter_indices.len() as f32
                    ));
                }
                None
            };

            (util, total)
        }
    }
}

/// 在 AdapterInfo 缓冲里扫描全部 UDID 锚点。
///
/// 返回 `(UDID 起始偏移, iAdapterIndex)` 列表（顺序即记录顺序）。
/// iAdapterIndex 位于 strUDID 前 4 字节（iSize、iAdapterIndex 之后）。
fn scan_udid_entries(buf: &[u8]) -> Vec<(usize, i32)> {
    let mut out: Vec<(usize, i32)> = Vec::new();
    let mut i = 0usize;
    while let Some(rel) = find_sub(buf, i, b"VEN_") {
        // "PCI_VEN_..." 的锚点算 "PCI_" 处；裸 "VEN_..." 直接用。
        let udid_off = if rel >= 4 && &buf[rel - 4..rel] == b"PCI_" { rel - 4 } else { rel };
        // 同一条记录里不会出现第二个 "VEN_"（SUBSYS 串不含），但防重。
        if out.last().map(|(off, _)| *off) != Some(udid_off) {
            let idx = if udid_off >= 4 {
                i32::from_le_bytes([
                    buf[udid_off - 4], buf[udid_off - 3], buf[udid_off - 2], buf[udid_off - 1],
                ])
            } else {
                -1
            };
            out.push((udid_off, idx));
        }
        i = rel + 4;
    }
    out
}

fn find_sub(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() { return None; }
    (from..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// 读 PMLog 输出缓冲里第 `id` 号传感器：{supported, value} × 256，跳过 size 头。
fn pmlog_sensor(out: &[u8; ADL_PMLOG_OUT_SIZE], id: i32) -> Option<i32> {
    if id < 0 { return None; }
    let off = 4 + (id as usize) * 8;
    if off + 8 > out.len() { return None; }
    let supported = i32::from_le_bytes(out[off..off + 4].try_into().ok()?);
    let value = i32::from_le_bytes(out[off + 4..off + 8].try_into().ok()?);
    (supported != 0).then_some(value)
}

impl Drop for Adl {
    fn drop(&mut self) { if let Some(f) = self.destroy { unsafe { f(self.ctx) }; } }
}

// ─────────────────────────── Intel: IGCL ───────────────────────────

pub struct Igcl { _init_ok: bool }

impl Igcl {
    fn load() -> Option<Self> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一条 AdapterInfo 记录：{iSize, iAdapterIndex, strUDID[256], ...pad}
    fn make_entry(buf: &mut [u8], base: usize, stride: i32, idx: i32, dev: u32) {
        buf[base..base + 4].copy_from_slice(&stride.to_le_bytes()); // iSize
        buf[base + 4..base + 8].copy_from_slice(&idx.to_le_bytes()); // iAdapterIndex
        let udid = format!("PCI_VEN_1002&DEV_{dev:04X}&SUBSYS_12341002&REV_C1");
        let bytes = udid.as_bytes();
        buf[base + 8..base + 8 + bytes.len()].copy_from_slice(bytes);
    }

    #[test]
    fn scan_udid_entries_finds_all_with_real_stride() {
        // 真实步长 1572 vs 候选 1576 的漂移事故：锚点扫描必须无视步长假设。
        let stride = 300usize; // 测试用小步长（iSize+idx+UDID+pad）
        let n = 3;
        let mut buf = vec![0u8; stride * n];
        make_entry(&mut buf, 0, 300, 0, 0x13C0); // iGPU
        make_entry(&mut buf, stride, 300, 1, 0x7550); // dGPU
        make_entry(&mut buf, stride * 2, 300, 2, 0x13C0); // 再来一条 iGPU

        let entries = scan_udid_entries(&buf);
        assert_eq!(entries.len(), 3, "必须找到全部 3 条记录: {entries:?}");
        assert_eq!(entries[0], (8, 0));
        assert_eq!(entries[1], (8 + stride, 1));
        assert_eq!(entries[2], (8 + stride * 2, 2));

        // 多卡场景：两块 dGPU（0x7550、0x13C0）都要被收集，不能只取一块。
        let dgpus: Vec<_> = [0x7550u32, 0x13C0]
            .iter()
            .map(|dev| (0x1002u32, *dev))
            .collect();
        let mut matched: Vec<i32> = Vec::new();
        for (off, idx) in &entries {
            let end = (*off + 256).min(buf.len());
            if let Some((v, d)) = parse_udid_ids(&buf[*off..end]) {
                if dgpus.contains(&(v, d)) {
                    matched.push(*idx);
                }
            }
        }
        assert_eq!(matched, vec![0, 1, 2], "多块 AMD 独显必须全部匹配");

        // 锚点前 4 字节是 iAdapterIndex（驱动写 0 的 iSize 不影响）
        let ids: Vec<_> = entries
            .iter()
            .map(|(off, _)| parse_udid_ids(&buf[*off..(*off + 256).min(buf.len())]))
            .collect();
        assert_eq!(ids[0], Some((0x1002, 0x13C0)));
        assert_eq!(ids[1], Some((0x1002, 0x7550)), "dGPU 必须按 DEV 匹配");
        assert_eq!(ids[2], Some((0x1002, 0x13C0)));
    }

    #[test]
    fn pmlog_sensor_reads_supported_entries() {
        let mut out = [0u8; ADL_PMLOG_OUT_SIZE];
        out[0..4].copy_from_slice(&(ADL_PMLOG_OUT_SIZE as i32).to_le_bytes());
        let mut put = |id: i32, supported: i32, value: i32| {
            let off = 4 + id as usize * 8;
            out[off..off + 4].copy_from_slice(&supported.to_le_bytes());
            out[off + 4..off + 8].copy_from_slice(&value.to_le_bytes());
        };
        put(ADL_PMLOG_BOARD_POWER, 1, 240);
        put(ADL_PMLOG_ASIC_POWER, 1, 231);
        put(ADL_PMLOG_INFO_ACTIVITY_GFX, 0, 999); // supported=0 → 不可用

        assert_eq!(pmlog_sensor(&out, ADL_PMLOG_BOARD_POWER), Some(240));
        assert_eq!(pmlog_sensor(&out, ADL_PMLOG_ASIC_POWER), Some(231));
        assert_eq!(pmlog_sensor(&out, ADL_PMLOG_INFO_ACTIVITY_GFX), None);
        assert_eq!(pmlog_sensor(&out, -1), None, "负 ID 必须安全返回 None");
    }

    #[test]
    fn udid_parse_tolerates_prefix_variants() {
        assert_eq!(
            parse_udid_ids(b"PCI_VEN_1002&DEV_7550&SUBSYS_1EA21002&REV_C0"),
            Some((0x1002, 0x7550))
        );
        assert_eq!(parse_udid_ids(b"VEN_1002&DEV_13C0"), Some((0x1002, 0x13C0)));
        assert_eq!(parse_udid_ids(b"1002&DEV_13C0"), None, "缺 VEN_ 段");
        assert_eq!(parse_udid_ids(b""), None);
    }
}
