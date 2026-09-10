//! 独立曲线窗口：CPU / 内存 / 网络三条 120 帧趋势（自绘）。
//!
//! - 普通 GDI 窗口（不透明）：跟随系统明暗主题
//! - 自绘标题栏（固定 / 最小化 / 关闭三个圆角矩形按钮），不使用系统标题栏
//! - 固定：置顶且不可拖动；最小化：隐藏并停用定时器；关闭：销毁（单例不留驻）
//! - 数据来自采样线程维护的 [`crate::history`]（ArcSwap 无锁）

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreatePen, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
    MoveToEx, Polyline, ScreenToClient, SelectObject, SetBkMode, SetTextColor, DrawTextW,
    PS_SOLID, TRANSPARENT, HDC, HFONT, PAINTSTRUCT, HBRUSH, DT_LEFT, DT_SINGLELINE, DT_VCENTER,
    ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
    GetWindowLongPtrW, GetMessageW, IsWindow, KillTimer, LoadCursorW, PostMessageW,
    PostQuitMessage, RegisterClassExW, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    TranslateMessage, IDC_ARROW, MSG, SW_HIDE, SW_SHOW, HWND_NOTOPMOST, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WINDOW_LONG_PTR_INDEX, WINDOW_EX_STYLE, WNDCLASSEXW,
    WM_APP, WS_POPUP,
};

use crate::dpi;
use crate::history;
use crate::theme::{rgb, Theme};
use crate::titlebar;

/// 逻辑尺寸（96 DPI）。
const W_L: i32 = 420;
const H_L: i32 = 200;
/// 三条曲线的水平分带。
const BAND_COUNT: i32 = 3;
const BAND_GAP: i32 = 6;
const PAD: i32 = 10;
const LABEL_H: i32 = 16;

// 消息
const WM_TIMER: u32 = 0x0113;
const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_CLOSE: u32 = 0x0010;
const WM_DESTROY: u32 = 0x0002;
const WM_NCHITTEST: u32 = 0x0084;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_MOUSELEAVE: u32 = 0x02A2;
const HTCAPTION: isize = 2;
const HTCLIENT: isize = 1;
const HTTRANSPARENT: isize = -1;
const GWLP_UD: i32 = -21;
const TIMER_ID: usize = 1;
/// 恢复显示 + 重挂定时器（最小化后再次 open）：交给窗口所属曲线线程处理，
/// 不在调用线程跨线程直接 SetTimer/ShowWindow。
const WM_APP_RESTORE: u32 = WM_APP + 1;

static GRAPH_HWND: AtomicIsize = AtomicIsize::new(-1);
/// 曲线窗口线程存活标志（含创建中）：防重复 spawn；线程退出时复位。
static GRAPH_THREAD_ALIVE: AtomicBool = AtomicBool::new(false);
/// 上次绘制的时序序号：定时器脏检查用（无变化不重绘）。
static LAST_PAINT_SEQ: AtomicU64 = AtomicU64::new(0);
/// 定时器连续跳过次数：每 5 拍强制重绘一次，保证主题/DPI 等非数据变化最终可见。
static PAINT_SKIP_TICKS: AtomicU32 = AtomicU32::new(0);

struct Ctx {
    tb: titlebar::TitleBarState,
    font: HFONT,
    tracking: bool,
}

fn s(dpi: u32, v: i32) -> i32 {
    v * dpi as i32 / 96
}

/// 打开（或前置）曲线窗口。最小化隐藏后再次打开会恢复定时器。
pub fn open() {
    let existing = GRAPH_HWND.load(Ordering::SeqCst);
    if existing != -1 {
        let hwnd = HWND(existing as *mut _);
        unsafe {
            if IsWindow(hwnd).as_bool() {
                // 窗口由曲线线程创建与轮询：恢复动作经消息交给所属线程，
                // 避免从 UI 线程跨线程直接操作。
                let _ = PostMessageW(hwnd, WM_APP_RESTORE, WPARAM(0), LPARAM(0));
                crate::history::set_graph_active(true);
            } else {
                GRAPH_HWND.store(-1, Ordering::SeqCst);
                spawn_thread();
            }
        }
        return;
    }
    spawn_thread();
}

/// 在独立线程上创建曲线窗口并跑自己的消息泵（多核方案 #1）：
/// 1s 定时重绘 + GDI 绘制全部发生在曲线线程，悬浮条 UI 线程不再被周期唤醒。
/// 窗口销毁（WM_DESTROY→PostQuitMessage）后线程自然退出并复位 `GRAPH_THREAD_ALIVE`。
fn spawn_thread() {
    if GRAPH_THREAD_ALIVE.swap(true, Ordering::SeqCst) {
        return; // 已有曲线线程（运行中或正在退出），下次 open 再试
    }
    if std::thread::Builder::new()
        .name("sysoverlay-graph".into())
        .spawn(graph_thread_main)
        .is_err()
    {
        GRAPH_THREAD_ALIVE.store(false, Ordering::SeqCst);
    }
}

fn graph_thread_main() {
    if !create() {
        GRAPH_THREAD_ALIVE.store(false, Ordering::SeqCst);
        return;
    }
    unsafe {
        let mut msg = MSG::default();
        loop {
            // GetMessageW 返回 0 = WM_QUIT，-1 = 错误——两者都结束曲线线程
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 <= 0 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    GRAPH_THREAD_ALIVE.store(false, Ordering::SeqCst);
}

/// 在**当前线程**（曲线线程）上创建窗口；成功返回 true。
/// 修复原实现竞态：两个快速 open() 都看到 GRAPH_HWND==-1 会建出两个窗口，
/// 第二个覆盖句柄、第一个成孤儿（其 WM_DESTROY 还会错误清掉有效句柄）。
/// 现由 `spawn_thread` 的 `GRAPH_THREAD_ALIVE` CAS 保证进程内单窗口单线程。
fn create() -> bool {
    unsafe {
        let hinst = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(_) => return false,
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
            WS_POPUP,
            280,
            240,
            s(d, W_L),
            s(d, H_L + titlebar::TB_H),
            None,
            None,
            hinst,
            None,
        );
        let Ok(hwnd) = hwnd else { return false };
        let ctx = Box::new(Ctx {
            tb: titlebar::TitleBarState::default(),
            font: titlebar::title_font(d),
            tracking: false,
        });
        let _ = SetWindowLongPtrW(hwnd, WINDOW_LONG_PTR_INDEX(GWLP_UD), Box::into_raw(ctx) as isize);
        GRAPH_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        let _ = SetTimer(hwnd, TIMER_ID, 1000, None);
        let _ = ShowWindow(hwnd, SW_SHOW);
        crate::history::set_graph_active(true);
        true
    }
}

fn ctx_of(hwnd: HWND) -> Option<&'static mut Ctx> {
    let p = unsafe { GetWindowLongPtrW(hwnd, WINDOW_LONG_PTR_INDEX(GWLP_UD)) as *mut Ctx };
    if p.is_null() {
        None
    } else {
        Some(unsafe { &mut *p })
    }
}

unsafe extern "system" fn graph_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER => {
            // 脏检查：时序序号未变就跳过重绘（曲线线程独享此判断，跳过近乎零成本）。
            // 每 5 拍强制一次，保证主题/DPI 等非数据变化最终可见。
            let seq = history::peek_seq();
            let last = LAST_PAINT_SEQ.load(Ordering::SeqCst);
            let skips = PAINT_SKIP_TICKS.load(Ordering::SeqCst);
            if seq != last || skips >= 4 {
                PAINT_SKIP_TICKS.store(0, Ordering::SeqCst);
                let _ = InvalidateRect(hwnd, None, false);
            } else {
                PAINT_SKIP_TICKS.store(skips + 1, Ordering::SeqCst);
            }
            LRESULT(0)
        }
        WM_APP_RESTORE => {
            let _ = SetTimer(hwnd, TIMER_ID, 1000, None);
            let _ = ShowWindow(hwnd, SW_SHOW);
            LRESULT(0)
        }
        WM_PAINT => {
            paint(hwnd);
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
            let d = dpi::get_system_dpi();
            let pt = POINT {
                x: (lparam.0 & 0xFFFF) as u16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as u16 as i32,
            };
            if titlebar::in_title_bar(d, pt) {
                if let Some(ctx) = ctx_of(hwnd) {
                    let mut rc = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rc);
                    let win_w = rc.right - rc.left;
                    if let Some(btn) = titlebar::hit_button(d, win_w, pt, ctx.tb.pinned) {
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
                                crate::probe::log("[graph] pin on");
                            }
                            titlebar::TitleBtn::Unlock => {
                                ctx.tb.pinned = false;
                                let _ = SetWindowPos(
                                    hwnd,
                                    HWND_NOTOPMOST,
                                    0, 0, 0, 0,
                                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                                );
                                crate::probe::log("[graph] pin off");
                            }
                            titlebar::TitleBtn::Min => {
                                let _ = KillTimer(hwnd, TIMER_ID);
                                let _ = ShowWindow(hwnd, SW_HIDE);
                                crate::probe::log("[graph] minimized");
                            }
                            titlebar::TitleBtn::Close => {
                                let _ = DestroyWindow(hwnd);
                            }
                        }
                        let _ = InvalidateRect(hwnd, None, false);
                        return LRESULT(0);
                    }
                }
            }
            if let Some(_ctx) = ctx_of(hwnd) {
                // 内容区点击：无操作（本轮仅标题栏交互）
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, TIMER_ID);
            if let Some(ctx) = ctx_of(hwnd) {
                let _ = DeleteObject(ctx.font);
            }
            let p = GetWindowLongPtrW(hwnd, WINDOW_LONG_PTR_INDEX(GWLP_UD)) as *mut Ctx;
            if !p.is_null() {
                drop(Box::from_raw(p));
                let _ = SetWindowLongPtrW(hwnd, WINDOW_LONG_PTR_INDEX(GWLP_UD), 0);
            }
            GRAPH_HWND.store(-1, Ordering::SeqCst);
            crate::history::set_graph_active(false);
            // 曲线线程自己的消息泵靠 WM_QUIT 收尾（窗口已在本线程销毁完毕）
            PostQuitMessage(0);
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

    // 自绘标题栏（单次借用 ctx：原实现两次 ctx_of 会同时产生两个 &mut 别名）
    let (font, tb_title, tb_state) = match ctx_of(hwnd) {
        Some(c) => (c.font, Some("SysOverlay - Trends") as Option<&'static str>, &c.tb),
        None => (HFONT::default(), None, &titlebar::TitleBarState::default()),
    };
    titlebar::draw(hdc, &theme, d, w, tb_title, tb_state, font);

    let plot_top = s(d, PAD + LABEL_H) + s(d, titlebar::TB_H);
    let band_h = (h - plot_top - s(d, PAD)) / BAND_COUNT;

    SetBkMode(hdc, TRANSPARENT);
    let tr = history::get();
    let (cpu, mem, rx, tx) = tr.series();
    // rx/tx 以 KB/s 存储（history 里 >>10）
    let net: Vec<f32> = (0..cpu.len())
        .map(|i| rx.get(i).copied().unwrap_or(0.0) + tx.get(i).copied().unwrap_or(0.0))
        .collect();
    // 缩放基准用**合成后**的 rx+tx 序列：旧实现取 max(rx,tx)，上下行同时大时
    // 合成值 > 基准被钳到 1.0，曲线顶格失真（bug 修复）
    let net_max = net.iter().copied().fold(0.0f32, f32::max).max(1.0);

    let lanes = [
        ("CPU %", cpu.as_slice(), 100.0, theme.accent),
        ("Memory %", mem.as_slice(), 100.0, rgb(255, 149, 0)),
        ("Net KB/s", net.as_slice(), net_max, rgb(52, 199, 89)),
    ];

    let _ = SelectObject(hdc, font);

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

    // 记录本次绘制覆盖到的时序序号，供 WM_TIMER 脏检查
    LAST_PAINT_SEQ.store(tr.seq, Ordering::SeqCst);
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