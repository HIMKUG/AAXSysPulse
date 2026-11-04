//! DPI awareness handling.
//!
//! Fallback strategy (compatible with all Windows versions):
//!   - `GetDeviceCaps(hdc, LOGPIXELSX)` → system DPI（用于布局；
//!     审查 611：Y 方向 `LOGPIXELSY` 从未使用，原头注释据实删除）
//!   - `SetProcessDPIAware()` legacy API for backward compatibility

use windows::Win32::Graphics::Gdi::{GetDeviceCaps, GetDC, ReleaseDC, LOGPIXELSX};
use windows::Win32::Foundation::HWND;

/// Apply DPI awareness for the current process.
/// Uses the legacy `SetProcessDPIAware()` API which is available on all Windows versions.
/// For per-monitor DPI on Windows 10+, use SetThreadDpiAwarenessContext at startup.
pub fn apply_dpi_awareness() -> bool {
    unsafe {
        #[link(name = "user32")]
        extern "system" {
            fn SetProcessDPIAware() -> windows::Win32::Foundation::BOOL;
        }
        // 审查 159：返回实际结果 —— 失败时进程仍是 DPI-unaware，
        // 此前 `let _` + 恒 true 把失败掩盖了。
        let ok = SetProcessDPIAware().as_bool();
        if !ok {
            crate::probe::log("dpi: SetProcessDPIAware failed（可能已按 manifest 声明或系统拒绝）");
        }
        ok
    }
}

/// Get the system DPI from the primary monitor DC.
pub fn get_system_dpi() -> u32 {
    unsafe {
        let hdc = GetDC(HWND::default());
        if hdc.is_invalid() {
            // 审查 160：空 HDC 直接回退 96 并留痕。
            crate::probe::log("dpi: GetDC(NULL) failed -> fallback 96");
            return 96;
        }
        let dpi = GetDeviceCaps(hdc, LOGPIXELSX);
        ReleaseDC(HWND::default(), hdc);
        if dpi == 0 { 96 } else { dpi as u32 }
    }
}

/// Get the DPI for the primary monitor using the system DC.
/// 审查 161：与 [`get_system_dpi`] 完全相同 —— 本模块无 per-monitor 支持
/// （仅启动期 `SetProcessDPIAware`），保留别名供调用方表意。
pub fn get_primary_monitor_dpi() -> u32 {
    get_system_dpi()
}

/// Scale a logical coordinate value based on DPI relative to 96 DPI.
pub fn scale_logical(value: i32, dpi: u32) -> i32 {
    if dpi == 96 {
        return value;
    }
    // 审查 157：统一四舍五入 —— titlebar / settings/layout 三处缩放口径一致。
    (value as f32 * dpi as f32 / 96.0).round() as i32
}

/// 主显示器**全屏**宽度（像素；`SM_CXSCREEN` 含任务栏区域 —— 审查 612：
/// 原注释"工作区"与实现不符；如需工作区应改用 SPI_GETWORKAREA）。
pub fn screen_w() -> i32 {
    unsafe { GetSystemMetrics(SM_CXSCREEN) as i32 }
}

/// 主显示器**全屏**高度（像素；口径说明同 [`screen_w`]，审查 612）。
pub fn screen_h() -> i32 {
    unsafe { GetSystemMetrics(SM_CYSCREEN) as i32 }
}

use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
