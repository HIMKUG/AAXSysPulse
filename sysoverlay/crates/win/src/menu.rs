//! 托盘右键菜单：设置 / 退出。

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos,
    TrackPopupMenu, HMENU, MF_SEPARATOR, MF_STRING, TPM_LEFTALIGN, TPM_RETURNCMD, PA_NOACTIVATE,
};

pub const IDM_SETTINGS: u16 = 1;
pub const IDM_QUIT: u16 = 2;
pub const IDM_RESTART: u16 = 3;

unsafe fn append(menu: HMENU, id: u16, label: &str) -> bool {
    let wide: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
    // 审查 164：判返回值 —— 静默缺项的菜单最难排查。
    let ok = AppendMenuW(menu, MF_STRING, id as usize, windows::core::PCWSTR(wide.as_ptr())).is_ok();
    if !ok {
        crate::probe::log(&format!("menu: AppendMenuW failed id={id}"));
    }
    ok
}

/// 在光标处弹出托盘菜单，返回选中的命令 ID。
pub fn show_tray_menu(hwnd: HWND) -> Option<u16> {
    unsafe {
        let menu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return None,
        };
        // 每次右键重建菜单 → 语言即时跟随当前配置（无需重启）
        let lang = crate::window::get_config_of(hwnd)
            .map(|c| c.lang)
            .unwrap_or_default();
        let t = |k: &'static str| crate::i18n::t(lang, k);
        append(menu, IDM_SETTINGS, t("MenuSettings"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
        append(menu, IDM_RESTART, t("MenuRestart"));
        append(menu, IDM_QUIT, t("MenuQuit"));

        // 不调用 SetForegroundWindow：用 TPM_NOACTIVATE 让菜单跟随托盘图标，
        // 不抢前台应用焦点。
        // 审查 610：GetCursorPos 失败回退到屏幕中心 —— 原先菜单弹到 (0,0)。
        let mut pt = windows::Win32::Foundation::POINT::default();
        if GetCursorPos(&mut pt).is_err() {
            crate::probe::log("menu: GetCursorPos failed -> fall back to screen center");
            pt.x = crate::dpi::screen_w() / 2;
            pt.y = crate::dpi::screen_h() / 2;
        }
        let cmd = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_RETURNCMD
                | windows::Win32::UI::WindowsAndMessaging::TRACK_POPUP_MENU_FLAGS(PA_NOACTIVATE),
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
