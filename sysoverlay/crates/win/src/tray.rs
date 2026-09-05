//! 系统托盘图标（Shell_NotifyIcon）。
//!
//! 图标**运行时代码生成**：把位图字体的 'S' 字形画进 16×16 ARGB 缓冲，
//! 经 `CreateIcon` 转成 HICON —— 无外部资源文件，exe 自包含。

use std::sync::OnceLock;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW};
use windows::Win32::UI::WindowsAndMessaging::CreateIcon;

/// 托盘回调消息（wndproc 里按 `msg == WM_TRAY` 处理，lparam 低 16 位为鼠标消息）。
pub const WM_TRAY: u32 = 0x8000 + 1; // WM_APP + 1

/// HICON 句柄值（isize 存 static，绕开 HICON 非 Send/Sync）。
static TRAY_ICON: OnceLock<isize> = OnceLock::new();

/// 生成 16×16 托盘图标：透明底 + 青色 'S' 字形（位图字体 8×16，水平居中）。
unsafe fn make_icon() -> windows::Win32::UI::WindowsAndMessaging::HICON {
    const W: usize = 16;
    const H: usize = 16;
    // XOR 掩码：32bpp BGRA（非预乘）
    let mut xor = vec![0u8; W * H * 4];
    let bitmap = sysoverlay_ui::font::get_char_bitmap('S');
    for row in 0..H {
        let bits = bitmap[row];
        for col in 0..8 {
            if bits & (1 << (7 - col)) != 0 {
                let idx = ((row * W) + col + 4) * 4; // x 偏移 4 居中
                xor[idx] = 255;     // B
                xor[idx + 1] = 196; // G
                xor[idx + 2] = 86;  // R
                xor[idx + 3] = 255; // A
            }
        }
    }
    // AND 掩码全 0：交给 XOR 的 alpha 通道
    let and = [0u8; W * H / 8];

    match CreateIcon(
        None,
        W as i32,
        H as i32,
        1,
        32,
        and.as_ptr(),
        xor.as_ptr(),
    ) {
        Ok(h) => h,
        Err(_) => windows::Win32::UI::WindowsAndMessaging::HICON::default(),
    }
}

/// 注册托盘图标。
pub fn add(hwnd: HWND) -> bool {
    unsafe {
        let icon_val = *TRAY_ICON.get_or_init(|| make_icon().0 as isize);
        let icon = windows::Win32::UI::WindowsAndMessaging::HICON(icon_val as *mut _);
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: icon,
            ..Default::default()
        };
        // szTip: "SysOverlay"
        let tip: Vec<u16> = "SysOverlay".encode_utf16().chain(std::iter::once(0)).collect();
        for (i, c) in tip.iter().take(nid.szTip.len()).enumerate() {
            nid.szTip[i] = *c;
        }
        Shell_NotifyIconW(NIM_ADD, &nid).as_bool()
    }
}

/// 注销托盘图标（窗口销毁前调用）。
pub fn remove(hwnd: HWND) {
    unsafe {
        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}
