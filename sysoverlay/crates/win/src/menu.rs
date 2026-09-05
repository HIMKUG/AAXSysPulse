//! 托盘右键菜单：设置 / 退出。

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos,
    TrackPopupMenu, HMENU, MF_SEPARATOR, MF_STRING, TPM_LEFTALIGN, TPM_RETURNCMD, PA_NOACTIVATE,
};

pub const IDM_SETTINGS: u16 = 1;
pub const IDM_QUIT: u16 = 2;
pub const IDM_CHARTS: u16 = 3;
pub const IDM_PROCESSES: u16 = 4;

unsafe fn append(menu: HMENU, id: u16, label: &str) {
    let wide: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
    let _ = AppendMenuW(menu, MF_STRING, id as usize, windows::core::PCWSTR(wide.as_ptr()));
}

/// 在光标处弹出托盘菜单，返回选中的命令 ID。
pub fn show_tray_menu(hwnd: HWND) -> Option<u16> {
    unsafe {
        let menu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return None,
        };
        append(menu, IDM_SETTINGS, "Settings");
        append(menu, IDM_CHARTS, "Charts");
        append(menu, IDM_PROCESSES, "Processes");
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
        append(menu, IDM_QUIT, "Quit");

        // 不调用 SetForegroundWindow：用 TPM_NOACTIVATE 让菜单跟随托盘图标，
        // 不抢游戏前台（AFMF 破坏路径 1：抢焦点）。
        let mut pt = windows::Win32::Foundation::POINT::default();
        let _ = GetCursorPos(&mut pt);
        let cmd = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_RETURNCMD
                | windows::Win32::UI::WindowsAndMessaging::TRACK_POPUP_MENU_FLAGS(PA_NOACTIVATE as u32),
            pt.x,
            pt.y,
            0,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);

        if cmd.as_bool() {
            Some(cmd.0 as u16)
        } else {
            None
        }
    }
}
