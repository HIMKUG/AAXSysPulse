# -*- coding: utf-8 -*-
# 编译修复补丁：sysinfo / gpu_detect / msr / gpu_vendor
import io

def patch(path, pairs):
    s = io.open(path, encoding="utf-8").read()
    for old, new in pairs:
        if old not in s:
            print(f"[WARN] not found in {path}: {old[:60]!r}")
        s = s.replace(old, new)
    io.open(path, "w", encoding="utf-8").write(s)
    print(f"patched {path}")

patch("crates/win/src/sysinfo.rs", [
    ("GetSystemFirmwareTable, FIRMWARE_TABLE_PROVIDER, FIRMWARE_TABLE_ID,",
     "GetSystemFirmwareTable, FIRMWARE_TABLE_PROVIDER,"),
    ("GetSystemFirmwareTable(sig, FIRMWARE_TABLE_ID(0), None)",
     "GetSystemFirmwareTable(sig, 0, None)"),
    ("GetSystemFirmwareTable(sig, FIRMWARE_TABLE_ID(0), Some(&mut buf))",
     "GetSystemFirmwareTable(sig, 0, Some(&mut buf))"),
    ("""            if h.is_ok() && !h.unwrap().is_invalid() {
                n += 1;
                let _ = windows::Win32::Foundation::CloseHandle(h.unwrap());
            }""",
     """            if let Ok(hd) = h {
                if !hd.is_invalid() {
                    n += 1;
                }
                let _ = windows::Win32::Foundation::CloseHandle(hd);
            }"""),
])

patch("crates/win/src/gpu_detect.rs", [
    ("    CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_DESC1,",
     "    CreateDXGIFactory1, IDXGIFactory1,"),
])

patch("crates/win/src/msr.rs", [
    ("            GENERIC_READ | GENERIC_WRITE,",
     "            (GENERIC_READ | GENERIC_WRITE).0,"),
])

patch("crates/win/src/gpu_vendor.rs", [
    ("""#[derive(Clone, Copy, PartialEq, Debug)]
enum PowerSource {
    /// 尚未收敛（探针日志阶段）。
    Pending,
    /// ADLODNPerformanceStatus 已收敛。
    Odn,
}""",
     """/// 收敛状态：0=探针阶段；1=已固化 ADL_ODN_POWER_IDX 并启用提取。
const ADL_PENDING: u8 = 0;
const ADL_EXTRACT: u8 = 1;"""),
    ("""    od6_power: FnOd6Power,
    odn_status: FnOdnStatus,
    destroy: FnMainDestroy,
    power_source: PowerSource,
}""",
     """    od6_power: FnOd6Power,
    odn_status: FnOdnStatus,
    destroy: FnMainDestroy,
    /// 原始结构体转储计数（只在前 3 轮打日志，收敛后启用提取）。
    dump_count: std::sync::atomic::AtomicU8,
}"""),
    ("            Some(Self { ctx, adapter_index, od6_power: od6, odn_status: odn, destroy: Some(destroy), power_source: PowerSource::Pending })",
     """            Some(Self {
                ctx,
                adapter_index,
                od6_power: od6,
                odn_status: odn,
                destroy: Some(destroy),
                dump_count: std::sync::atomic::AtomicU8::new(0),
            })"""),
])
print("all done")
