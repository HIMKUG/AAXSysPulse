//! Win32 Overlay Window implementation - simplified skeleton.

use crate::Config;
use crate::Snapshot;
use std::sync::Mutex;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, ReleaseDC, HORZRES, VERTRES};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW,
    PostQuitMessage, RegisterClassW, SetWindowPos, TranslateMessage,
    CS_HREDRAW, CS_VREDRAW, GWL_EXSTYLE, GWLP_USERDATA, IDC_ARROW,
    IMAGE_ICON, LR_SHARED, LoadImageW, MSG, SWP_NOACTIVATE, SWP_NOSIZE,
    SWP_NOZORDER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

const CARD_W: i32 = 268;
const CARD_H: i32 = 184;

static WINDOW_STATE: Mutex<Option<WindowState>> = Mutex::new(None);

struct WindowState {
    cfg: Config,
    snapshot: Snapshot,
    dragging: bool,
    drag_start: POINT,
    win_start: POINT,
}

#[derive(Debug)]
pub struct WinError {
    pub msg: String,
}

impl WinError {
    fn new(msg: &str) -> Self {
        Self { msg: msg.to_string() }
    }
}

impl std::fmt::Display for WinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WinError: {}", self.msg)
    }
}

impl std::error::Error for WinError {}

pub struct OverlayWindow {
    hwnd: HWND,
}

impl OverlayWindow {
    pub fn new(cfg: Config) -> Result<Self, WinError> {
        {
            let mut state = WINDOW_STATE.lock().unwrap();
            *state = Some(WindowState {
                cfg,
                snapshot: create_placeholder_snapshot(),
                dragging: false,
                drag_start: POINT { x: 0, y: 0 },
                win_start: POINT { x: 0, y: 0 },
            });
        }

        let cursor = unsafe {
            LoadImageW(None, IDC_ARROW, IMAGE_ICON, 0, 0, LR_SHARED)
                .map(|i| windows::Win32::UI::WindowsAndMessaging::HCURSOR(i.0))
                .unwrap_or_default()
        };

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: windows::Win32::Foundation::HINSTANCE::default(),
            hIcon: windows::Win32::UI::WindowsAndMessaging::HICON::default(),
            hCursor: cursor,
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH::default(),
            lpszMenuName: windows::core::PCWSTR::null(),
            lpszClassName: windows::core::w!("SysOverlay_WndClass"),
            hIconSm: windows::Win32::UI::WindowsAndMessaging::HICON::default(),
        };

        unsafe {
            let class_ptr = &wc as *const WNDCLASSEXW as *const std::ffi::c_void;
            if RegisterClassW(std::mem::transmute(class_ptr)) == 0 {
                return Err(WinError::new("Failed to register window class"));
            }
        }

        let (screen_w, _) = get_screen_size();
        let zoom_factor = 1.0f32;
        let card_w = (CARD_W as f32 * zoom_factor) as i32;
        let card_h = (CARD_H as f32 * zoom_factor) as i32;
        let pos_x = screen_w - card_w - 24;
        let pos_y = 24;

        let mut ex_style = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;

        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                windows::core::w!("SysOverlay_WndClass"),
                windows::core::w!("SysOverlay"),
                WS_POPUP | WS_VISIBLE,
                pos_x,
                pos_y,
                card_w,
                card_h,
                None,
                None,
                None,
                None,
            )
        };

        if hwnd.is_err() {
            return Err(WinError::new("Failed to create window"));
        }

        let hwnd = hwnd.unwrap();

        let self_ptr = Box::into_raw(Box::new(())) as isize;
        unsafe {
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA.0 as i32, self_ptr);
        }

        Ok(Self { hwnd })
    }

    pub fn run_message_loop(&self) {
        let mut msg = MSG::default();
        loop {
            unsafe {
                if GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&msg);
                } else {
                    break;
                }
            }
            if msg.message == 0x0002 {
                break;
            }
        }
    }

    pub fn set_snapshot(&mut self, snap: Snapshot) {
        if let Ok(mut state) = WINDOW_STATE.lock() {
            if let Some(ref mut s) = *state {
                s.snapshot = snap;
            }
        }
    }

    pub fn get_snapshot(&self) -> Option<Snapshot> {
        WINDOW_STATE.lock().ok().and_then(|state| state.as_ref().map(|s| s.snapshot.clone()))
    }

    pub fn get_config(&self) -> Option<Config> {
        WINDOW_STATE.lock().ok().and_then(|state| state.as_ref().map(|s| s.cfg.clone()))
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = SetWindowLongPtrW(self.hwnd, GWLP_USERDATA.0 as i32, 0);
        }
    }
}

#[link(name = "user32")]
extern "system" {
    fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
    fn SetCapture(hwnd: HWND) -> HWND;
    fn ReleaseCapture() -> BOOL;
    fn GetWindowRect(hwnd: HWND, lpRect: *mut RECT) -> BOOL;
}

const WM_NCHITTEST: u32 = 0x0084;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_DESTROY: u32 = 0x0002;

const HTCAPTION_VAL: isize = 2;

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTCAPTION_VAL),

        WM_LBUTTONDOWN => {
            if let Ok(mut state) = WINDOW_STATE.lock() {
                if let Some(ref mut s) = *state {
                    s.dragging = true;
                    s.drag_start = POINT {
                        x: (lparam.0 & 0xFFFF) as i16 as i32,
                        y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
                    };
                    let mut rect = RECT::default();
                    GetWindowRect(hwnd, &mut rect);
                    s.win_start = POINT { x: rect.left, y: rect.top };
                    SetCapture(hwnd);
                }
            }
            LRESULT(0)
        }

        WM_MOUSEMOVE => {
            if let Ok(mut state) = WINDOW_STATE.lock() {
                if let Some(ref mut s) = *state {
                    if s.dragging {
                        let cur = POINT {
                            x: (lparam.0 & 0xFFFF) as i16 as i32,
                            y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
                        };
                        let new_x = s.win_start.x + (cur.x - s.drag_start.x);
                        let new_y = s.win_start.y + (cur.y - s.drag_start.y);
                        SetWindowPos(hwnd, None, new_x, new_y, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER);
                    }
                }
            }
            LRESULT(0)
        }

        WM_LBUTTONUP => {
            if let Ok(mut state) = WINDOW_STATE.lock() {
                if let Some(ref mut s) = *state {
                    s.dragging = false;
                }
            }
            ReleaseCapture();
            LRESULT(0)
        }

        WM_PAINT => {
            crate::gdi::render_placeholder(hwnd);
            LRESULT(0)
        }

        WM_ERASEBKGND => LRESULT(1),

        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn get_screen_size() -> (i32, i32) {
    unsafe {
        let hdc = GetDC(None);
        let w = GetDeviceCaps(hdc, HORZRES);
        let h = GetDeviceCaps(hdc, VERTRES);
        ReleaseDC(None, hdc);
        (w, h)
    }
}

fn create_placeholder_snapshot() -> Snapshot {
    use sysoverlay_core::snapshot::{Metric, MemoryStat, NetStat};
    Snapshot {
        seq: 0,
        timestamp_ms: 0,
        cpu: Metric::Value(50.0),
        memory: MemoryStat { load_pct: 65.0, used_gb: 8.0, total_gb: 16.0 },
        gpu: Metric::Value(30.0),
        net: NetStat { rx_bps: 1024 * 1024, tx_bps: 512 * 1024 },
    }
}
