//! 独立曲线窗口：CPU / 内存 / 网络三条 120 帧趋势（自绘）。
//!
//! - 普通 GDI 窗口（不透明）：跟随系统明暗主题
//! - 1s 定时器刷新；关闭即销毁（单例不留驻）
//! - 数据来自采样线程维护的 [`crate::history`]（ArcSwap 无锁）

use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreatePen, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
    MoveToEx, Polyline, SelectObject, SetBkMode, SetTextColor, DrawTextW, PS_SOLID, TRANSPARENT,
    HDC, PAINTSTRUCT, HBRUSH, DT_LEFT, DT_SINGLELINE, DT_VCENTER, ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, IsWindow, KillTimer,
    LoadCursorW, RegisterClassExW, SetTimer, ShowWindow, IDC_ARROW, SW_SHOW,
    WINDOW_EX_STYLE, WNDCLASSEXW, WS_CAPTION, WS_SYSMENU,
};

use crate::history;
use crate::theme::{rgb, Theme};
use crate::dpi;

/// 逻辑尺寸（96 DPI）。
const W_L: i32 = 420;
const H_L: i32 = 200;
/// 三条曲线的水平分带。
const BAND_COUNT: i32 = 3;
const BAND_GAP: i32 = 6;
const PAD: i32 = 10;
const LABEL_H: i32 = 16;

static GRAPH_HWND: AtomicIsize = AtomicIsize::new(-1);

fn s(dpi: u32, v: i32) -> i32 {
    v * dpi as i32 / 96
}

/// 打开（或前置）曲线窗口。
pub fn open() {
    let existing = GRAPH_HWND.load(Ordering::SeqCst);
    if existing != -1 {
        let hwnd = HWND(existing as *mut _);
        if unsafe { IsWindow(hwnd) }.as_bool() {
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
            }
            return;
        }
        GRAPH_HWND.store(-1, Ordering::SeqCst);
    }
    create();
}

fn create() {
    unsafe {
        let hinst = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(_) => return,
        };
        static REGISTERED: std::sync::Once = std::sync::Once::new();
        REGISTERED.call_once(|| {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(graph_wndproc),
                hInstance: hinst.into(),
                lpszClassName: windows::core::w!("SysOverlay_Graph"),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH::default(),
                ..Default::default()
            };
            let _ = RegisterClassExW(&wc);
        });

        let d = dpi::get_system_dpi();
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::w!("SysOverlay_Graph"),
            windows::core::w!("SysOverlay - Trends"),
            WS_CAPTION | WS_SYSMENU,
            280,
            240,
            s(d, W_L),
            s(d, H_L),
            None,
            None,
            hinst,
            None,
        );
        let Ok(hwnd) = hwnd else { return };
        GRAPH_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        let _ = SetTimer(hwnd, 1, 1000, None);
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
}

unsafe extern "system" fn graph_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        0x0113 /* WM_TIMER */ => {
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        0x000F /* WM_PAINT */ => {
            paint(hwnd);
            LRESULT(0)
        }
        0x0014 /* WM_ERASEBKGND */ => LRESULT(1),
        0x0010 /* WM_CLOSE */ => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        0x0002 /* WM_DESTROY */ => {
            let _ = KillTimer(hwnd, 1);
            GRAPH_HWND.store(-1, Ordering::SeqCst);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn paint(hwnd: HWND) {
    let theme = Theme::system();
    let d = dpi::get_system_dpi();
    let mut ps = PAINTSTRUCT::default();
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

    let tr = history::get();
    let (cpu, mem, rx, tx) = tr.series();
    // rx/tx 以 KB/s 存储（history 里 >>10）
    let net: Vec<f32> = (0..cpu.len())
        .map(|i| rx.get(i).copied().unwrap_or(0.0) + tx.get(i).copied().unwrap_or(0.0))
        .collect();
    let net_max = rx.iter().chain(tx.iter()).copied().fold(0.0f32, f32::max).max(1.0);

    let plot_top = s(d, PAD + LABEL_H);
    let band_h = (h - plot_top - s(d, PAD)) / BAND_COUNT;

    SetBkMode(hdc, TRANSPARENT);
    let lanes = [
        ("CPU %", cpu.as_slice(), 100.0, theme.accent),
        ("Memory %", mem.as_slice(), 100.0, rgb(255, 149, 0)),
        ("Net KB/s", net.as_slice(), net_max, rgb(52, 199, 89)),
    ];

    for (i, (label, data, scale, color)) in lanes.iter().enumerate() {
        let y0 = plot_top + i as i32 * band_h;
        // 标签（含当前值）
        let label_rect = RECT {
            left: s(d, PAD),
            top: y0,
            right: w - s(d, PAD),
            bottom: y0 + s(d, LABEL_H),
        };
        let last = data.iter().copied().last().unwrap_or(0.0);
        let cur = if *scale > 0.0 && last > 0.0 {
            format!("{}  {}", label, last as u32)
        } else {
            label.to_string()
        };
        // 空串 + DT_SINGLELINE|DT_VCENTER 会触发 user32!DrawTextW 空指针崩溃
        if !cur.is_empty() {
            let mut buf: Vec<u16> = cur.encode_utf16().collect();
            let _ = SetTextColor(hdc, COLORREF(theme.label));
            let mut lr = label_rect;
            let _ = DrawTextW(hdc, &mut buf, &mut lr, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
        }

        // 曲线车身
        let cy = y0 + s(d, LABEL_H);
        let ch = band_h - s(d, LABEL_H) - s(d, BAND_GAP);
        draw_curve(hdc, s(d, PAD), cy, w - 2 * s(d, PAD), ch, data, *scale, *color);
    }

    let _ = EndPaint(hwnd, &ps);
}

/// 折线绘制：x 均分，值线性映射到 [0, plot_h]。
fn draw_curve(hdc: HDC, x0: i32, y0: i32, pw: i32, ph: i32, data: &[f32], scale: f32, color: u32) {
    if data.is_empty() || pw <= 2 || ph <= 2 || scale <= 0.0 {
        return;
    }
    unsafe {
        let pen = CreatePen(PS_SOLID, 1, COLORREF(color));
        let old_pen = SelectObject(hdc, pen);
        let n = data.len() as i32;
        let mut pts: Vec<POINT> = Vec::with_capacity(data.len());
        for (i, &v) in data.iter().enumerate() {
            let x = x0 + if n > 1 { i as i32 * pw / (n - 1) } else { 0 };
            let t = (v / scale).clamp(0.0, 1.0);
            let y = y0 + (ph as f32 * (1.0 - t)) as i32;
            pts.push(POINT { x, y });
        }
        if let Some(p0) = pts.first() {
            let _ = MoveToEx(hdc, p0.x, p0.y, None);
            let _ = Polyline(hdc, &pts);
        }
        let _ = SelectObject(hdc, old_pen);
        let _ = DeleteObject(pen);
    }
}