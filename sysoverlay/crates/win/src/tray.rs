//! 系统托盘图标（Shell_NotifyIcon）。
//!
//! 图标取自 [`crate::app_icon`]：编译期内嵌的 16×16 ICO，运行时
//! `CreateIconFromResourceEx` 解码为 HICON —— 无外部资源文件，exe 自包含。

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFY_ICON_INFOTIP_FLAGS, NOTIFYICONDATAW,
};

/// 托盘回调消息（与 `window.rs` 同一契约常量，审查项 7.1.1 / P1-22）。
pub const WM_TRAY: u32 = aaxsyspulse_core::contract::WM_TRAY;

/// 托盘提示文字（悬停显示）。
const TRAY_TIP: &str = aaxsyspulse_core::contract::APP_USER_NAME;

/// 注册托盘图标。
pub fn add(hwnd: HWND) -> bool {
    unsafe {
        let icon = crate::app_icon::tray_icon();
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: icon,
            ..Default::default()
        };
        // 审查 162：复用 [`write_w`] —— 手写循环在超长截断后可能无 NUL 结尾。
        write_w(&mut nid.szTip, TRAY_TIP);
        Shell_NotifyIconW(NIM_ADD, &nid).as_bool()
    }
}

/// 弹出**系统原生气泡通知**（屏幕右下角，系统统一外观）。
///
/// 与旧的 `MessageBoxW` 模态框不同：不阻塞、不抢焦点、不需要用户点确定。
/// 前提是本进程已对本 `hwnd`/uID 调用过 `add()`（这里做的是 NIM_MODIFY）。
pub fn balloon(hwnd: HWND, title: &str, body: &str) -> bool {
    unsafe {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_INFO,
            ..Default::default()
        };
        write_w(&mut nid.szInfoTitle, title);
        write_w(&mut nid.szInfo, body);
        // 信息图标；联合体字段（uTimeout/uVersion 共用）需 unsafe 写入。
        nid.dwInfoFlags = NOTIFY_ICON_INFOTIP_FLAGS(NIIF_INFO.0);
        nid.Anonymous.uTimeout = BALLOON_TIMEOUT_MS;
        Shell_NotifyIconW(NIM_MODIFY, &nid).as_bool()
    }
}

/// 气泡显示时长（毫秒）。系统有 10–30s 的钳制范围，取下限即可。
const BALLOON_TIMEOUT_MS: u32 = 10_000;

/// 把 UTF-16 写入定长数组：自动补结尾 NUL，超长截断（绝不越界 panic）。
/// 审查 163：空 `dst` 直接返回（原先 `dst[0] = 0` 会 panic）。
fn write_w(dst: &mut [u16], s: &str) {
    if dst.is_empty() {
        return;
    }
    let wide: Vec<u16> = s.encode_utf16().collect();
    let n = wide.len().min(dst.len().saturating_sub(1));
    dst[..n].copy_from_slice(&wide[..n]);
    dst[n] = 0;
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
        // 审查 609：注销失败留痕 —— 残留托盘图标"幽灵图标"无日志最难排查。
        if !Shell_NotifyIconW(NIM_DELETE, &nid).as_bool() {
            crate::probe::log("tray: NIM_DELETE failed（图标残留）");
        }
    }
}
