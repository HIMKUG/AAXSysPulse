//! 诊断探针：验证托盘气泡通知是否**真的**弹出。
//!
//! 判据不是 `Shell_NotifyIconW` 的返回值（它只表示"参数被接受"），
//! 而是是否收到 `NIN_BALLOONSHOW` 回调 —— 那是系统在气泡真正显示时发的。
//!
//! 运行：`cargo run -p aaxsyspulse-win --example balloon_probe`

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFY_ICON_INFOTIP_FLAGS, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassExW,
    TranslateMessage, MSG, WINDOW_EX_STYLE, WS_OVERLAPPED,
};

const WM_TRAY: u32 = 0x8000 + 1;
/// 气泡相关回调（lParam 低 16 位）
const NIN_BALLOONSHOW: u32 = 0x0402;
const NIN_BALLOONHIDE: u32 = 0x0403;
const NIN_BALLOONTIMEOUT: u32 = 0x0404;
const NIN_BALLOONUSERCLICK: u32 = 0x0405;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_TRAY {
        let ev = (l.0 & 0xFFFF) as u32;
        match ev {
            NIN_BALLOONSHOW => {
                println!("RESULT: BALLOON SHOWED (NIN_BALLOONSHOW)");
                cleanup(hwnd);
                std::process::exit(0);
            }
            NIN_BALLOONHIDE => println!("balloon hidden"),
            NIN_BALLOONTIMEOUT => println!("balloon timed out"),
            NIN_BALLOONUSERCLICK => println!("balloon clicked"),
            _ => println!("tray event 0x{:x}", ev),
        }
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, w, l)
}

unsafe fn cleanup(hwnd: HWND) {
    let nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}

fn write_w(dst: &mut [u16], s: &str) {
    let wide: Vec<u16> = s.encode_utf16().collect();
    let n = wide.len().min(dst.len().saturating_sub(1));
    dst[..n].copy_from_slice(&wide[..n]);
    dst[n] = 0;
}

fn main() {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None)
            .map(|h| HINSTANCE(h.0))
            .unwrap_or_default();

        let wc = windows::Win32::UI::WindowsAndMessaging::WNDCLASSEXW {
            cbSize: std::mem::size_of::<windows::Win32::UI::WindowsAndMessaging::WNDCLASSEXW>()
                as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: windows::core::w!("BalloonProbeCls"),
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::w!("BalloonProbeCls"),
            windows::core::w!("probe"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            hinst,
            None,
        )
        .expect("create window");

        // ── 1. 添加托盘图标（用真实的应用图标） ──
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: aaxsyspulse_win::app_icon::tray_icon(),
            ..Default::default()
        };
        write_w(&mut nid.szTip, "Balloon Probe");
        let ok_add = Shell_NotifyIconW(NIM_ADD, &nid).as_bool();
        println!("NIM_ADD            = {}", ok_add);

        // ── 2. 弹气泡（与 tray::balloon 完全相同的写法） ──
        let mut nid2 = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_INFO,
            ..Default::default()
        };
        write_w(&mut nid2.szInfoTitle, "AAX SysPulse");
        write_w(&mut nid2.szInfo, "balloon probe body");
        nid2.dwInfoFlags = NOTIFY_ICON_INFOTIP_FLAGS(NIIF_INFO.0);
        nid2.Anonymous.uTimeout = 10_000;
        let ok_balloon = Shell_NotifyIconW(NIM_MODIFY, &nid2).as_bool();
        println!("NIM_MODIFY(balloon) = {}", ok_balloon);
        println!("waiting up to 5s for NIN_BALLOONSHOW ...");

        // ── 3. 超时兜底 ──
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            println!("RESULT: NO BALLOON (no NIN_BALLOONSHOW within 5s)");
            std::process::exit(1);
        });

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
