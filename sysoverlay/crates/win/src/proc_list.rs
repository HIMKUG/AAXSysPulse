//! 进程 Top 榜窗口：Top 8，可按 CPU / 内存切换排序（自绘）。
//!
//! 数据源 [`crate::process::ProcCollector`]（Nt API）。采样在独立后台线程执行，
//! WM_TIMER 只负责触发重绘，消除全表扫描阻塞 UI 消息泵的问题。
//!
//! 自绘标题栏（固定 / 最小化 / 关闭三个圆角矩形按钮）：
//! - 固定：置顶且不可拖动，悬停标题栏才显示解锁按钮
//! - 最小化：隐藏窗口；采样线程探测到不可见后停采样（CPU 归零）
//! - 关闭：销毁窗口，采样线程随 IsWindow 失效自动退出

use std::sync::atomic::{AtomicIsize, Ordering};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
    ScreenToClient, SelectObject, SetBkMode, SetTextColor, DrawTextW, TRANSPARENT, HDC, HFONT,
    PAINTSTRUCT, HBRUSH, DRAW_TEXT_FORMAT, DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER,
    ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, IsWindow, IsWindowVisible,
    KillTimer, LoadCursorW, PostMessageW, RegisterClassExW, SetTimer, SetWindowPos, ShowWindow,
    IDC_ARROW, SW_HIDE, SW_SHOW, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, WINDOW_EX_STYLE, WNDCLASSEXW, WS_POPUP,
};

#[link(name = "user32")]
extern "system" {
    fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
}

use crate::dpi;
use crate::process::{ProcCollector, ProcStats};
use crate::theme::Theme;
use crate::titlebar;

const W_L: i32 = 460;
const H_L: i32 = 280;
const TOP_N: usize = 8;

const WM_TIMER: u32 = 0x0113;
const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_CLOSE: u32 = 0x0010;
const WM_DESTROY: u32 = 0x0002;
const WM_NCHITTEST: u32 = 0x0084;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_MOUSELEAVE: u32 = 0x02A2;
const WM_NEW_SAMPLE: u32 = 0x8000;
const HTCAPTION: isize = 2;
const HTCLIENT: isize = 1;
const HTTRANSPARENT: isize = -1;
const GWLP_UD: i32 = -21;
const TIMER_ID: usize = 1;

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
    tb: titlebar::TitleBarState,
    font: HFONT,
    tracking: bool,
}

pub fn open() {
    crate::probe::log("[procs] open enter");
    let existing = PROCS_HWND.load(Ordering::SeqCst);
    if existing != -1 {
        let hwnd = HWND(existing as *mut _);
        if unsafe { IsWindow(hwnd) }.as_bool() {
            unsafe {
                // 最小化隐藏后重新打开：恢复定时器
                let _ = SetTimer(hwnd, TIMER_ID, 1000, None);
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
            WS_POPUP,
            300,
            260,
            s(d, W_L),
            s(d, H_L + titlebar::TB_H),
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
            tb: titlebar::TitleBarState::default(),
            font: titlebar::title_font(d),
            tracking: false,
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
                // 最小化隐藏期间不采样（省 CPU）；探测到可见再恢复
                if !IsWindowVisible(hwnd).as_bool() {
                    thread::sleep(Duration::from_secs(5));
                    continue;
                }
                let snap = collector.sample();
                let _ = PostMessageW(
                    hwnd,
                    WM_NEW_SAMPLE,
                    WPARAM(0),
                    LPARAM(0),
                );
                procs_push(snap);
                thread::sleep(Duration::from_secs(1));
            }
            PROCS_SAMPLE_THREAD.store(-1, Ordering::SeqCst);
        });

        let _ = SetTimer(hwnd, TIMER_ID, 1000, None);
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
        // 标题栏命中：按钮区 HTCLIENT 收鼠标；空白区 HTCAPTION 走系统拖动；
        // 固定态空白区 HTTRANSPARENT（不可拖动，仅悬停显示解锁按钮）。
        WM_NCHITTEST => {
            let d = dpi::get_system_dpi();
            let mut sc = POINT {
                x: (lparam.0 & 0xFFFF) as u16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as u16 as i32,
            };
            let _ = ScreenToClient(hwnd, &mut sc);
            if titlebar::in_title_bar(d, sc) {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let win_w = rc.right - rc.left;
                if let Some(ctx) = ctx_of(hwnd) {
                    if ctx.tb.pinned {
                        if titlebar::hit_button(d, win_w, sc, true).is_some() {
                            return LRESULT(HTCLIENT);
                        }
                        return LRESULT(HTTRANSPARENT);
                    }
                    if titlebar::hit_button(d, win_w, sc, false).is_some() {
                        return LRESULT(HTCLIENT);
                    }
                    return LRESULT(HTCAPTION);
                }
            }
            LRESULT(HTCLIENT)
        }
        WM_MOUSEMOVE => {
            if let Some(ctx) = ctx_of(hwnd) {
                let d = dpi::get_system_dpi();
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let win_w = rc.right - rc.left;
                let pt = POINT {
                    x: (lparam.0 & 0xFFFF) as u16 as i32,
                    y: ((lparam.0 >> 16) & 0xFFFF) as u16 as i32,
                };
                let over = titlebar::in_title_bar(d, pt);
                let hover = if over {
                    titlebar::hit_button(d, win_w, pt, ctx.tb.pinned)
                } else {
                    None
                };
                if ctx.tb.over != over || ctx.tb.hover != hover {
                    ctx.tb.over = over;
                    ctx.tb.hover = hover;
                    let _ = InvalidateRect(hwnd, None, false);
                }
                if !ctx.tracking {
                    ctx.tracking = true;
                    let mut te = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut te);
                }
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            if let Some(ctx) = ctx_of(hwnd) {
                ctx.tracking = false;
                ctx.tb.over = false;
                ctx.tb.hover = None;
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            if let Some(ctx) = ctx_of(hwnd) {
                let d = dpi::get_system_dpi();
                let x = (lparam.0 & 0xFFFF) as u16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i32;
                // 1) 标题栏按钮
                if titlebar::in_title_bar(d, POINT { x, y }) {
                    let mut rc = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rc);
                    let win_w = rc.right - rc.left;
                    if let Some(btn) = titlebar::hit_button(d, win_w, POINT { x, y }, ctx.tb.pinned) {
                        match btn {
                            titlebar::TitleBtn::Pin => {
                                ctx.tb.pinned = true;
                                ctx.tb.hover = None;
                                let _ = SetWindowPos(
                                    hwnd,
                                    HWND_TOPMOST,
                                    0, 0, 0, 0,
                                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                                );
                                crate::probe::log("[procs] pin on");
                            }
                            titlebar::TitleBtn::Unlock => {
                                ctx.tb.pinned = false;
                                let _ = SetWindowPos(
                                    hwnd,
                                    HWND_NOTOPMOST,
                                    0, 0, 0, 0,
                                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                                );
                                crate::probe::log("[procs] pin off");
                            }
                            titlebar::TitleBtn::Min => {
                                let _ = KillTimer(hwnd, TIMER_ID);
                                let _ = ShowWindow(hwnd, SW_HIDE);
                                crate::probe::log("[procs] minimized");
                            }
                            titlebar::TitleBtn::Close => {
                                let _ = DestroyWindow(hwnd);
                                crate::probe::log("[procs] close btn");
                            }
                        }
                        let _ = InvalidateRect(hwnd, None, false);
                        return LRESULT(0);
                    }
                }
                // 2) 表头排序区（标题栏下方）
                if y >= s(d, titlebar::TB_H) && y < s(d, titlebar::TB_H + 20) {
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
            let _ = KillTimer(hwnd, TIMER_ID);
            let p = GetWindowLongPtrW(hwnd, GWLP_UD) as *mut Ctx;
            if !p.is_null() {
                let _ = DeleteObject((*p).font);
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
    if bytes >= 1_073_741_824 {
        // >= 1 GiB → GB
        format!("{:.1}G", bytes as f64 / 1_073_741_824.0).trim_end_matches(".0").to_string()
    } else if bytes >= 1_048_576 {
        // >= 1 MiB → MB
        format!("{:.1}M", bytes as f64 / 1_048_576.0).trim_end_matches(".0").to_string()
    } else {
        // < 1 MiB → KB
        format!("{}K", (bytes / 1024).max(1))
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

    // 自绘标题栏
    let font = ctx_of(hwnd).map(|c| c.font).unwrap_or_default();
    let (tb_title, tb_state) = match ctx_of(hwnd) {
        Some(c) => (Some("SysOverlay - Processes"), &c.tb),
        None => (None, &titlebar::TitleBarState::default()),
    };
    titlebar::draw(hdc, &theme, d, w, tb_title, tb_state, font);
    let _ = SelectObject(hdc, font);

    SetBkMode(hdc, TRANSPARENT);

    let header_y = s(d, titlebar::TB_H);
    let header_h = s(d, 20);
    let cpu_col = RECT { left: w / 2 + s(d, 12), top: header_y, right: w - s(d, 240), bottom: header_y + header_h };
    let watt_col = RECT { left: w - s(d, 240), top: header_y, right: w - s(d, 150), bottom: header_y + header_h };
    let mem_col = RECT { left: w - s(d, 150), top: header_y, right: w - s(d, 12), bottom: header_y + header_h };
    set_color(hdc, theme.secondary);
    let name_r = RECT { left: s(d, 12), top: header_y, right: w / 2, bottom: header_y + header_h };
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

    let row_h = ((h - header_y - header_h) / TOP_N as i32).max(18);
    crate::probe::log("[procs] paint step4 rows");
    for (i, e) in entries.iter().take(TOP_N).enumerate() {
        let y = header_y + header_h + i as i32 * row_h;
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