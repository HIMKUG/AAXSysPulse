//! DPI awareness handling.
//!
//! Fallback strategy (compatible with all Windows versions):
//!   - `GetDeviceCaps(hdc, LOGPIXELSX)` → system DPI (used for layout)
//!   - `GetDeviceCaps(hdc, LOGPIXELSY)` → system DPI
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
        let _ = SetProcessDPIAware();
    }
    true
}

/// Get the system DPI from the primary monitor DC.
pub fn get_system_dpi() -> u32 {
    unsafe {
        let hdc = GetDC(HWND::default());
        let dpi = GetDeviceCaps(hdc, LOGPIXELSX);
        ReleaseDC(HWND::default(), hdc);
        if dpi == 0 { 96 } else { dpi as u32 }
    }
}

/// Get the DPI for the primary monitor using the system DC.
pub fn get_primary_monitor_dpi() -> u32 {
    get_system_dpi()
}

/// Scale a logical coordinate value based on DPI relative to 96 DPI.
pub fn scale_logical(value: i32, dpi: u32) -> i32 {
    if dpi == 96 {
        return value;
    }
    (value as f32 * dpi as f32 / 96.0) as i32
}
