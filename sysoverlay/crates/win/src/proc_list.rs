//! 进程 Top 榜窗口：Top 8，可按 CPU / 内存切换排序（自绘）。
//!
//! 数据源 [`crate::process::ProcCollector`]（Nt API）。采样在独立后台线程执行，
//! WM_TIMER 只负责触发重绘，消除全表扫描阻塞 UI 消息泵的问题。

use std::sync::atomic::{AtomicIsize, Ordering};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
    SetBkMode, SetTextColor, DrawTextW, TRANSPARENT, HDC, PAINTSTRUCT, HBRUSH, DRAW_TEXT_FORMAT,
    DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, IsWindow,
    KillTimer, LoadCursorW, PostMessageW, RegisterClassExW, SetTimer, ShowWindow,
    IsWindowVisible, IDC_ARROW, SW_SHOW,
    WINDOW_EX_STYLE, WNDCLASSEXW, WS_CAPTION, WS_SYSMENU,
};

#[link(name = "user32")]
extern "system" {
    fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
}

use crate::dpi;
use crate::process::{ProcCollector, ProcStats};
use crate::theme::Theme;

const W_L: i32 = 460;
const H_L: i32 = 280;
const TOP_N: usize = 8;

const WM_TIMER: u32 = 0x0113;
const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_CLOSE: u32 = 0x0010;
const WM_DESTROY: u32 = 0x0002;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_NEW_SAMPLE: u32 = 0x8000;
const GWLP_UD: i32 = -21;

use std::sync::Mutex;

static PROCS_HWND: AtomicIsize = AtomicIsize::new(-1);
static PROCS_SAMPLE_THREAD: AtomicIsize = AtomicIsize::new(-1);
static PROCS_HISTORY: Mutex<Option<ProcStats>> = Mutex::new(None);

pub fn procs_push(stats: ProcStats) {
    if let Ok(mut guard) = PROCS_HISTORY.lock() {
        *guard = Some(stats);
    }
}

fn procs_take_latest() -> Option<ProcStats> {
    PROCS_HISTORY.lock().ok()?.take()
}

fn s(dpi: u32, v: i32) -> i32 {
    v * dpi as i32 / 96
}

#[derive(Clone, Copy, PartialEq)]
enum SortBy {
    Cpu,
    Mem,
    Watt,
}

struct Ctx {
    latest: ProcStats,
    sort: SortBy,
}

pub fn open() {
    crate::probe::log("[procs] open enter");
    let existing = PROCS_HWND.load(Ordering::SeqCst);
    if existing != -1 {
        let hwnd = HWND(existing as *mut _);
        if unsafe { IsWindow(hwnd) }.as_bool() {
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
            }
            return;
        }
        PROCS_HWND.store(-1, Ordering::SeqCst);
    }
    create();
}

fn create() {
    unsafe {
        let hinst = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(_) => return,
        };
        crate::probe::log("[procs] create: module ok");
        static REGISTERED: std::sync::Once = std::sync::Once::new();
        REGISTERED.call_once(|| {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(procs_wndproc),
                hInstance: hinst.into(),
                lpszClassName: windows::core::w!("SysOverlay_Procs"),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH::default(),
                ..Default::default()
            };
            let rc = RegisterClassExW(&wc);
            crate::probe::log(&format!("[procs] create: RegisterClassExW rc={rc:x} err=0x{:x}", windows::Win32::Foundation::GetLastError().0));
        });
        crate::probe::log("[procs] create: class registered");

        let d = dpi::get_system_dpi();
        crate::probe::log("[procs] create: before CreateWindowExW");
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::w!("SysOverlay_Procs"),
            windows::core::w!("SysOverlay - Processes"),
            WS_CAPTION | WS_SYSMENU,
            300,
            260,
            s(d, W_L),
            s(d, H_L),
            None,
            None,
            hinst,
            None,
        );
        crate::probe::log("[procs] create: CreateWindowExW returned");
        let Ok(hwnd) = hwnd else { return };
        let ctx = Box::new(Ctx {
            latest: ProcStats { entries: Vec::new(), at_ms: 0 },
            sort: SortBy::Cpu,
        });
        let _ = SetWindowLongPtrW(hwnd, GWLP_UD, Box::into_raw(ctx) as isize);
        crate::probe::log("[procs] create: ctx attached");
        PROCS_HWND.store(hwnd.0 as isize, Ordering::SeqCst);

        let hwnd_addr = hwnd.0 as isize;
        let _sampler_thread = thread::spawn(move || {
            let hwnd = HWND(hwnd_addr as *mut _);
            let mut collector = ProcCollector::new();
            loop {
                if !IsWindow(hwnd).as_bool() {
                    break;
                }
                let snap = collector.sample();
                if IsWindowVisible(hwnd).as_bool() {
                    let _ = PostMessageW(
                        hwnd,
                        WM_NEW_SAMPLE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
                procs_push(snap);
                thread::sleep(Duration::from_secs(1));
            }
            PROCS_SAMPLE_THREAD.store(-1, Ordering::SeqCst);
        });

        let _ = SetTimer(hwnd, 1, 1000, None);
        crate::probe::log("[procs] create: timer set");
        let _ = ShowWindow(hwnd, SW_SHOW);
        crate::probe::log("[procs] create: shown");
    }
}

fn ctx_of(hwnd: HWND) -> Option<&'static mut Ctx> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_UD) as *mut Ctx };
    if p.is_null() {
        None
    } else {
        Some(unsafe { &mut *p })
    }
}

unsafe extern "system" fn procs_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    crate::probe::log(&format!(
        "[procs] wndproc msg=0x{msg:04X} w=0x{:X} l=0x{:X}",
        wparam.0 as u32, lparam.0 as u32
    ));
    match msg {
        WM_NEW_SAMPLE => {
            if let Some(ctx) = ctx_of(hwnd) {
                if let Some(h) = procs_take_latest() {
                    ctx.latest = h;
                    sort_entries(ctx);
                }
            }
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_TIMER => {
            LRESULT(0)
        }
        WM_PAINT => {
            crate::probe::log("[procs] WM_PAINT in");
            paint(hwnd);
            crate::probe::log("[procs] WM_PAINT out");
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_LBUTTONDOWN => {
            if let Some(ctx) = ctx_of(hwnd) {
                let d = dpi::get_system_dpi();
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                if y < s(d, 20) {
                    let wl = s(d, W_L);
                    ctx.sort = if x >= wl - s(d, 150) {
                        if ctx.sort == SortBy::Mem { SortBy::Cpu } else { SortBy::Mem }
                    } else if x >= wl - s(d, 240) {
                        if ctx.sort == SortBy::Watt { SortBy::Cpu } else { SortBy::Watt }
                    } else {
                        if ctx.sort == SortBy::Cpu { SortBy::Mem } else { SortBy::Cpu }
                    };
                    sort_entries(ctx);
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, 1);
            let p = GetWindowLongPtrW(hwnd, GWLP_UD) as *mut Ctx;
            if !p.is_null() {
                drop(Box::from_raw(p));
                let _ = SetWindowLongPtrW(hwnd, GWLP_UD, 0);
            }
            PROCS_HWND.store(-1, Ordering::SeqCst);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn sort_entries(ctx: &mut Ctx) {
    let by = ctx.sort;
    ctx.latest.entries.sort_by(|a, b| {
        let ord = match by {
            SortBy::Cpu => b.cpu_pct.total_cmp(&a.cpu_pct),
            SortBy::Mem => b.mem_bytes.cmp(&a.mem_bytes),
            SortBy::Watt => b.power_w.total_cmp(&a.power_w),
        };
        ord.then_with(|| b.mem_bytes.cmp(&a.mem_bytes))
    });
}

fn fmt_mem(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1}G", bytes as f64 / 1_048_576.0).trim_end_matches(".0").to_string()
    } else {
        format!("{}M", (bytes / 1024).max(1))
    }
}

unsafe fn paint(hwnd: HWND) {
    crate::probe::log("[procs] paint step0 theme");
    let theme = Theme::system();
    let d = dpi::get_system_dpi();
    let mut ps = PAINTSTRUCT::default();
    crate::probe::log("[procs] paint step1 BeginPaint");
    let hdc = BeginPaint(hwnd, &mut ps);
    if hdc.is_invalid() {
        let _ = unsafe { ValidateRect(hwnd, None) };
        return;
    }
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;
    if w <= 0 || h <= 0 {
        let _ = EndPaint(hwnd, &ps);
        return;
    }

    let bg = CreateSolidBrush(COLORREF(theme.window_bg));
    let _ = FillRect(hdc, &rc, bg);
    let _ = DeleteObject(bg);
    crate::probe::log("[procs] paint step2 bg done");

    SetBkMode(hdc, TRANSPARENT);

    let header_h = s(d, 20);
    let cpu_col = RECT { left: w / 2 + s(d, 12), top: 0, right: w - s(d, 240), bottom: header_h };
    let watt_col = RECT { left: w - s(d, 240), top: 0, right: w - s(d, 150), bottom: header_h };
    let mem_col = RECT { left: w - s(d, 150), top: 0, right: w - s(d, 12), bottom: header_h };
    set_color(hdc, theme.secondary);
    let name_r = RECT { left: s(d, 12), top: 0, right: w / 2, bottom: header_h };
    txt(hdc, "Process", name_r, DT_LEFT);
    txt(hdc, "CPU %", cpu_col, DT_RIGHT);
    txt(hdc, "Watts", watt_col, DT_RIGHT);
    txt(hdc, "Memory", mem_col, DT_RIGHT);
    set_color(hdc, theme.label);

    crate::probe::log("[procs] paint step3 entries");
    let entries_owned;
    let entries = if let Some(ctx) = ctx_of(hwnd) {
        entries_owned = ctx.latest.entries.clone();
        entries_owned.as_slice()
    } else {
        &[]
    };

    let row_h = ((h - header_h) / TOP_N as i32).max(18);
    crate::probe::log("[procs] paint step4 rows");
    for (i, e) in entries.iter().take(TOP_N).enumerate() {
        let y = header_h + i as i32 * row_h;
        let name_r = RECT { left: s(d, 12), top: y, right: w / 2 - s(d, 8), bottom: y + row_h };
        txt(hdc, &e.name, name_r, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
        let cpu_r = RECT { left: w / 2, top: y, right: w - s(d, 240), bottom: y + row_h };
        txt(hdc, &format!("{:.0}%", e.cpu_pct), cpu_r, DT_RIGHT | DT_SINGLELINE | DT_VCENTER);
        let watt_r = RECT { left: w - s(d, 240), top: y, right: w - s(d, 150), bottom: y + row_h };
        let watt_label = if e.power_w > 0.05 {
            format!("{:.1}W", e.power_w)
        } else {
            String::new()
        };
        txt(hdc, &watt_label, watt_r, DT_RIGHT | DT_SINGLELINE | DT_VCENTER);
        let mem_r = RECT { left: w - s(d, 150), top: y, right: w - s(d, 12), bottom: y + row_h };
        txt(hdc, &fmt_mem(e.mem_bytes), mem_r, DT_RIGHT | DT_SINGLELINE | DT_VCENTER);
    }

    let _ = EndPaint(hwnd, &ps);
}

unsafe fn set_color(hdc: HDC, color: u32) {
    let _ = SetTextColor(hdc, COLORREF(color));
}

unsafe fn txt(hdc: HDC, text: &str, rect: RECT, fmt: DRAW_TEXT_FORMAT) {
    if text.is_empty() {
        return;
    }
    let mut buf: Vec<u16> = text.encode_utf16().collect();
    let mut r = rect;
    let _ = DrawTextW(hdc, &mut buf, &mut r, fmt);
}