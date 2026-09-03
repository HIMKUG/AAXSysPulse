//! GDI rendering pipeline - placeholder implementation.

use windows::Win32::Foundation::{BOOL, COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject,
    GetDC, ReleaseDC, SelectObject, BI_RGB, BITMAPINFO, HBRUSH, HPEN, PS_SOLID,
};
use windows::Win32::UI::WindowsAndMessaging::UpdateLayeredWindow;

#[link(name = "user32")]
extern "system" {
    fn GetWindowRect(hwnd: HWND, lpRect: *mut RECT) -> BOOL;
}

pub fn render_placeholder(hwnd: HWND) {
    unsafe {
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect);
        let width = (rect.right - rect.left) as u32;
        let height = (rect.bottom - rect.top) as u32;

        if width == 0 || height == 0 {
            return;
        }

        let screen_dc = GetDC(hwnd);
        let mem_dc = CreateCompatibleDC(screen_dc);

        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = width as i32;
        bmi.bmiHeader.biHeight = -(height as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bitmap = match CreateDIBSection(mem_dc, &bmi, windows::Win32::Graphics::Gdi::DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bmp) => bmp,
            Err(_) => {
                DeleteDC(mem_dc);
                ReleaseDC(hwnd, screen_dc);
                return;
            }
        };

        let old_bmp = SelectObject(mem_dc, bitmap);

        // Fill with gradient background
        if !bits.is_null() {
            let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (width * height) as usize);

            for y in 0..height as i32 {
                for x in 0..width as i32 {
                    let idx = (y * width as i32 + x) as usize;
                    if idx >= pixels.len() { break; }

                    let cx = width as f32 / 2.0;
                    let cy = height as f32 / 2.0;
                    let dx = (x as f32 - cx) / cx;
                    let dy = (y as f32 - cy) / cy;
                    let dist = (dx * dx + dy * dy).sqrt().min(1.0);

                    let t = dist;
                    let r = (20.0 + 10.0 * (1.0 - t)) as u8;
                    let g = (22.0 + 12.0 * (1.0 - t)) as u8;
                    let b = (27.0 + 20.0 * (1.0 - t)) as u8;

                    pixels[idx] = 0xFF000000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);
                }
            }

            // Draw placeholder bars
            for i in 0..3 {
                let bar_y = (height as i32 / 4) + (i as i32 * 25);
                let bar_w = (width as i32 - 40) / 2;
                let bar_h = 6i32;

                draw_rect(mem_dc, 20, bar_y, bar_w, bar_h, RGB(63, 69, 80));
                draw_rect(mem_dc, 20, bar_y, bar_w / 2, bar_h, RGB(74, 222, 128));
            }
        }

        // Update layered window
        let mut dst_pt = POINT::default();
        let mut sz = SIZE { cx: width as i32, cy: height as i32 };
        let mut src_pt = POINT::default();

        let bf = BLENDFUNCTION {
            BlendOp: 0,              // AC_SRC_OVER
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 1,          // AC_SRC_ALPHA
        };

        let _ = UpdateLayeredWindow(
            hwnd,
            screen_dc,
            Some(&mut dst_pt),
            Some(&mut sz),
            mem_dc,
            Some(&mut src_pt),
            COLORREF(0),
            Some(&bf as *const BLENDFUNCTION),
            windows::Win32::UI::WindowsAndMessaging::ULW_ALPHA,
        );

        let _ = SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(bitmap);
        DeleteDC(mem_dc);
        ReleaseDC(hwnd, screen_dc);
    }
}

#[allow(non_snake_case)]
fn RGB(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

unsafe fn draw_rect(hdc: windows::Win32::Graphics::Gdi::HDC, x: i32, y: i32, w: i32, h: i32, color: COLORREF) {
    use windows::Win32::Graphics::Gdi::{CreateSolidBrush, CreatePen, SelectObject, FillRect};

    if w <= 0 || h <= 0 { return; }

    let brush: HBRUSH = CreateSolidBrush(color);
    let pen: HPEN = CreatePen(PS_SOLID, 1, color);

    let old_brush = SelectObject(hdc, brush);
    let old_pen = SelectObject(hdc, pen);

    let mut r = RECT { left: x, top: y, right: x + w, bottom: y + h };
    let _ = FillRect(hdc, &mut r, brush);

    let _ = SelectObject(hdc, old_brush);
    let _ = SelectObject(hdc, old_pen);
    let _ = DeleteObject(brush);
    let _ = DeleteObject(pen);
}

pub fn render(hwnd: HWND, _snapshot: &crate::Snapshot, _cfg: &crate::Config) {
    render_placeholder(hwnd);
}
