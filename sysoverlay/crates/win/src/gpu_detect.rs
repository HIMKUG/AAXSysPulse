//! GPU 适配器检测（DXGI）：枚举适配器 → 按厂商 VendorId 分类 → 选出独显。
//!
//! # 判定规则（用户拍板）
//! - 只认三家：AMD 0x1002 / NVIDIA 0x10DE / Intel 0x8086；
//!   虚拟屏（如 OrayIddDriver、Microsoft 基本渲染）VendorId 不在三家 → 无条件排除；
//! - 核显忽略：在三家卡里取 `DedicatedVideoMemory` 最大者；要求 ≥4 GiB 才算独显
//!   （AMD 核显通常 512MB~2GB，RX 9070 GRE 16GB）；
//! - 没有满足条件的卡 → 返回 `None`，利用率回退 D3DKMT 通用路径。

use crate::probe;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1,
};

pub const VENDOR_AMD: u32 = 0x1002;
pub const VENDOR_NVIDIA: u32 = 0x10DE;
pub const VENDOR_INTEL: u32 = 0x8086;

/// 独显判定的专用显存下限（4 GiB）。
const DEDICATED_MEM_DGPU: usize = 4 * 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct GpuInfo {
    pub vendor_id: u32,
    pub device_id: u32,
    /// 专用显存（字节）。
    pub dedicated_mem: usize,
    /// 适配器名（UTF-8 截断，诊断用）。
    pub name: String,
    pub luid_low: u32,
    pub luid_high: i32,
}

impl GpuInfo {
    pub fn vendor_name(&self) -> &'static str {
        match self.vendor_id {
            VENDOR_AMD => "AMD",
            VENDOR_NVIDIA => "NVIDIA",
            VENDOR_INTEL => "Intel",
            _ => "unknown",
        }
    }
}

/// 枚举全部 DXGI 适配器（诊断 + 选卡）。
pub fn enum_adapters() -> Vec<GpuInfo> {
    let mut out = Vec::new();
    let probe_on = probe::enabled();
    unsafe {
        let factory: IDXGIFactory1 = match CreateDXGIFactory1() {
            Ok(f) => f,
            Err(e) => {
                probe::log(&format!("gpudetect CreateDXGIFactory1 failed hr=0x{:x}", e.code().0 as u32));
                return out;
            }
        };
        let mut i: u32 = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let d = match adapter.GetDesc1() {
                Ok(d) => d,
                Err(_) => continue,
            };
            let name = {
                let end = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
                String::from_utf16_lossy(&d.Description[..end])
            };
            let info = GpuInfo {
                vendor_id: d.VendorId,
                device_id: d.DeviceId,
                dedicated_mem: d.DedicatedVideoMemory,
                name: name.chars().take(48).collect(),
                luid_low: d.AdapterLuid.LowPart,
                luid_high: d.AdapterLuid.HighPart,
            };
            if probe_on {
                probe::log(&format!(
                    "gpudetect adapter vendor=0x{:04x} device=0x{:04x} dedicated={}MiB name={:?}",
                    info.vendor_id, info.device_id, info.dedicated_mem / (1024 * 1024), info.name
                ));
            }
            out.push(info);
        }
    }
    out
}

/// 列出全部独显：三家厂商卡里专用显存 ≥4 GiB 的那几块（多卡都算）。
pub fn list_dgpu() -> Vec<GpuInfo> {
    enum_adapters()
        .into_iter()
        .filter(|a| matches!(a.vendor_id, VENDOR_AMD | VENDOR_NVIDIA | VENDOR_INTEL))
        .filter(|a| a.dedicated_mem >= DEDICATED_MEM_DGPU)
        .collect()
}

/// 选出独显：三家厂商卡里专用显存最大且 ≥4 GiB 的那块。
pub fn pick_dgpu() -> Option<GpuInfo> {
    let best = list_dgpu().into_iter().max_by_key(|a| a.dedicated_mem)?;
    if probe::enabled() {
        probe::log(&format!(
            "gpudetect dgpu picked vendor={} device=0x{:04x} name={:?}",
            best.vendor_name(), best.device_id, best.name
        ));
    }
    Some(best)
}
