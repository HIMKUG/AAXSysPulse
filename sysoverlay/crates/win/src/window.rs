//! Win32 Overlay Window implementation.
//!
//! State is stored in GWLP_USERDATA as a raw pointer to a Box<ThreadState>,
//! eliminating the module-level WINDOW_STATE global mutex.

use crate::Config;
use crate::Snapshot;
use arc_swap::ArcSwap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, RECT};
use windows::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, InvalidateRect, ReleaseDC, HORZRES, VERTRES};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, DestroyWindow,
    IsWindowVisible, PostQuitMessage, RegisterClassExW, SetWindowPos, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, IDC_ARROW,
    LoadCursorW, MSG, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOW,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOPMOST,
    WS_EX_TOOLWINDOW, WS_POPUP, WS_THICKFRAME, WS_VISIBLE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};

/// Per-window mutable state, stored in GWLP_USERDATA.
struct ThreadState {
    cfg: Config,
    dragging: bool,
    drag_start: POINT,
    win_start: POINT,
    /// True when the window has been moved by the user and pos needs saving on exit.
    pos_dirty: bool,
    /// True when the window has been resized by the user and size needs saving on exit.
    size_dirty: bool,
    /// 设置面板「调整大小」模式：开启时窗口带 WS_THICKFRAME，可拖边。
    resize_mode: bool,
    /// 鼠标是否在窗口内（hover 模式点亮用）。
    hovered: bool,
}

impl ThreadState {
    fn new(cfg: Config) -> Self {
        Self {
            cfg,
            dragging: false,
            drag_start: POINT::default(),
            win_start: POINT::default(),
            pos_dirty: false,
            size_dirty: false,
            resize_mode: false,
            hovered: false,
        }
    }
}

// ─── Shared snapshot (sampling thread → UI thread) ───────────────────

/// Shared snapshot written by the app's sampling thread, read by WM_PAINT.
static SHARED_SNAP: OnceLock<ArcSwap<Snapshot>> = OnceLock::new();

/// Sequence number of the latest snapshot written by the sampling thread.
pub(crate) static SHARED_SEQ: AtomicU64 = AtomicU64::new(0);

/// Returns the current snapshot, or a placeholder if not yet initialized.
fn snap_get() -> Snapshot {
    SHARED_SNAP
        .get_or_init(|| ArcSwap::from_pointee(Snapshot::placeholder(0)))
        .load_full()
        .as_ref()
        .clone()
}

/// Stores a new snapshot into the shared atomic (called from app sampling thread).
pub fn snap_store(snap: Snapshot) {
    SHARED_SNAP
        .get_or_init(|| ArcSwap::from_pointee(Snapshot::placeholder(0)))
        .store(Arc::new(snap));
}

/// Returns a mutable handle to the shared sequence number (for app use).
pub fn get_shared_seq() -> &'static AtomicU64 {
    &SHARED_SEQ
}

// ─── 调度配置镜像（设置线程 → 采样线程，无锁） ───────────────────
//
// 设置页改 interval/refresh_auto/监控项开关后，采样线程需要立刻感知。
// 用三个原子量做无锁镜像，避免给采样循环加共享锁。

static SCHED_AUTO: AtomicBool = AtomicBool::new(true);
static SCHED_INTERVAL_MS: AtomicU32 = AtomicU32::new(1000);
/// 位 0..4 = cpu/mem/gpu/net 启用。
static SCHED_ENABLED: AtomicU32 = AtomicU32::new(0b1111);
/// Power 开关（独立于 SamplePlan，采样器自己按 power_interval_ms 分频）。
static SCHED_POWER: AtomicBool = AtomicBool::new(false);

/// 把当前配置发布给采样线程（启动与每次设置改动后调用）。
pub fn publish_schedule_cfg(cfg: &Config) {
    SCHED_AUTO.store(cfg.refresh_auto, Ordering::Relaxed);
    SCHED_INTERVAL_MS.store(cfg.interval_ms, Ordering::Relaxed);
    let mask = (cfg.show_cpu as u32)
        | ((cfg.show_mem as u32) << 1)
        | ((cfg.show_gpu as u32) << 2)
        | ((cfg.show_net as u32) << 3);
    SCHED_ENABLED.store(mask, Ordering::Relaxed);
    SCHED_POWER.store(cfg.power_enabled, Ordering::Relaxed);
}

/// 采样线程读取当前调度参数：(interval_ms, auto, [cpu, mem, gpu, net])。
pub fn schedule_cfg() -> (u32, bool, [bool; 4]) {
    let mask = SCHED_ENABLED.load(Ordering::Relaxed);
    (
        SCHED_INTERVAL_MS.load(Ordering::Relaxed),
        SCHED_AUTO.load(Ordering::Relaxed),
        [mask & 1 != 0, mask & 2 != 0, mask & 4 != 0, mask & 8 != 0],
    )
}

/// 采样线程读取 Power 开关（设置页改动即时生效）。
pub fn schedule_power() -> bool {
    SCHED_POWER.load(Ordering::Relaxed)
}

// ─── Error type ────────────────────────────────────────────────────────

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

// ─── Window procedure ─────────────────────────────────────────────────

#[link(name = "user32")]
extern "system" {
    fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
    fn SetCapture(hwnd: HWND) -> HWND;
    fn ReleaseCapture() -> BOOL;
    fn GetWindowRect(hwnd: HWND, lpRect: *mut RECT) -> BOOL;
}

const WM_NCHITTEST: u32 = 0x0084;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_PAINT: u32 = 0x000F;
const WM_SIZE: u32 = 0x0005;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_DESTROY: u32 = 0x0002;
const WM_TIMER: u32 = 0x0113;
const WM_POWERBROADCAST: u32 = 0x0218;
const WM_DISPLAYCHANGE: u32 = 0x007E;
const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
/// Sent after the system move/size modal loop ends (caption drag & border resize).
const WM_EXITSIZEMOVE: u32 = 0x0232;

/// 托盘回调消息（tray.rs 同值，此处独立定义避免跨模块 const 依赖）。
const WM_TRAY: u32 = 0x8000 + 1;
/// 设置面板改动通知：重排版 + 自适应窗口尺寸。
pub const WM_RECONFIG: u32 = 0x8000 + 2;
/// 采样线程新快照通知：**推送式渲染**——UI 线程不再轮询，
/// 阻塞在 GetMessageW 上（空闲 0% CPU），有新数据才画。
pub const WM_NEW_SNAPSHOT: u32 = 0x8000 + 3;

const HTCAPTION_VAL: isize = 2;
const HTCLIENT_VAL: isize = 1;

/// Retrieves ThreadState pointer from GWLP_USERDATA.
///
/// 返回 null 表示窗口尚未完成创建（WM_CREATE/WM_SIZE 等消息在
/// `CreateWindowExW` 返回之前同步到达，此时状态指针还未写入）。
/// 调用方用 `if let Some(state) = ...` 跳过即可——**绝不**在此处
/// PostQuitMessage，否则窗口创建阶段就会把整个应用静默杀死。
unsafe fn get_state_ptr(hwnd: HWND) -> *mut ThreadState {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA.0 as i32) as *mut ThreadState
}

/// 登记 WM_MOUSELEAVE（TME_LEAVE 只来一次，进窗后需重新登记）。
fn track_mouse_leave(hwnd: HWND) {
    let mut tme = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    let _ = unsafe { TrackMouseEvent(&mut tme) };
}

/// 鼠标是否在窗口内（hover 渲染由 gdi::render 读取）。
pub fn is_hovered(hwnd: HWND) -> bool {
    let p = unsafe { get_state_ptr(hwnd) };
    if p.is_null() {
        false
    } else {
        unsafe { (*p).hovered }
    }
}

/// Notifies the app's sampling thread of a system resume event via a oneshot.
static RESUME_NOTIFY: OnceLock<std::sync::mpsc::Sender<()>> = OnceLock::new();

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    _wparam: windows::Win32::Foundation::WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // 边框命中交给 DefWindowProc（WS_THICKFRAME 的 HTLEFT/HTRIGHT 等），
        // 只有客户区内部才当成标题栏（可拖动）。
        WM_NCHITTEST => {
            let ht = DefWindowProcW(hwnd, msg, _wparam, lparam).0;
            if ht == HTCLIENT_VAL {
                LRESULT(HTCAPTION_VAL)
            } else {
                LRESULT(ht)
            }
        }

        WM_LBUTTONDOWN => {
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                state.drag_start = POINT {
                    x: (lparam.0 & 0xFFFF) as i16 as i32,
                    y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
                };
                let mut rect = RECT::default();
                let _ = GetWindowRect(hwnd, &mut rect);
                state.win_start = POINT { x: rect.left, y: rect.top };
                state.dragging = true;
                let _ = SetCapture(hwnd);
            }
            LRESULT(0)
        }

        WM_MOUSEMOVE => {
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                // ── hover 点亮（无论是否在拖）──
                if !state.hovered {
                    state.hovered = true;
                    track_mouse_leave(hwnd);
                    if state.cfg.hover {
                        let snap = snap_get();
                        crate::gdi::render(hwnd, &snap, &state.cfg);
                    }
                }
                // ── 手动拖动 ──
                if state.dragging {
                    let cur = POINT {
                        x: (lparam.0 & 0xFFFF) as i16 as i32,
                        y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
                    };
                    let new_x = state.win_start.x + (cur.x - state.drag_start.x);
                    let new_y = state.win_start.y + (cur.y - state.drag_start.y);
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        new_x,
                        new_y,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    );
                    state.pos_dirty = true;
                }
            }
            LRESULT(0)
        }

        // hover 模式：鼠标离开 → 变淡再画一帧（alpha 变化会改写指纹，正常重绘）
        WM_MOUSELEAVE => {
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                if state.hovered && state.cfg.hover {
                    let snap = snap_get();
                    crate::gdi::render(hwnd, &snap, &state.cfg);
                }
                state.hovered = false;
            }
            LRESULT(0)
        }

        WM_LBUTTONUP => {
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                state.dragging = false;
                // Persist position if the user moved the window
                if state.pos_dirty {
                    let mut rect = RECT::default();
                    if GetWindowRect(hwnd, &mut rect).as_bool() {
                        state.cfg.window_pos = Some((rect.left, rect.top));
                    }
                    state.pos_dirty = false;
                }
                // Persist size if the user resized the window
                if state.size_dirty {
                    let mut rect = RECT::default();
                    if GetWindowRect(hwnd, &mut rect).as_bool() {
                        state.cfg.window_size = Some((rect.right - rect.left, rect.bottom - rect.top));
                    }
                    state.size_dirty = false;
                }
            }
            let _ = ReleaseCapture();
            LRESULT(0)
        }

        // 标题栏拖动（HTCAPTION）与边框缩放都走系统模态循环，
        // WM_LBUTTONUP 在该循环中不会到达，必须在循环结束后落盘。
        WM_EXITSIZEMOVE => {
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                let mut rect = RECT::default();
                if GetWindowRect(hwnd, &mut rect).as_bool() {
                    state.cfg.window_pos = Some((rect.left, rect.top));
                    state.cfg.window_size =
                        Some((rect.right - rect.left, rect.bottom - rect.top));
                }
                state.pos_dirty = false;
                state.size_dirty = false;
            }
            LRESULT(0)
        }

        // Handle window resize: mark dirty so position/size are saved on release.
        WM_SIZE => {
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                // wParam == SIZE_RESTORED(0), SIZE_MAXIMIZED(1), SIZE_MINIMIZED(2)
                if _wparam.0 != 2 {
                    state.size_dirty = true;
                }
                let _ = InvalidateRect(hwnd, None, true);
            }
            LRESULT(0)
        }

        WM_TIMER => {
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                let snap = snap_get();
                crate::gdi::render(hwnd, &snap, &state.cfg);
            }
            LRESULT(0)
        }

        WM_PAINT => {
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                let snap = snap_get();
                crate::gdi::render(hwnd, &snap, &state.cfg);
            }
            LRESULT(0)
        }

        // ── 托盘交互：单击 = 显隐切换；右键 = 设置 / 退出 ──
        WM_TRAY => {
            let mouse = (lparam.0 & 0xFFFF) as u32;
            match mouse {
                WM_LBUTTONUP => {
                    let visible = IsWindowVisible(hwnd).as_bool();
                    let _ = ShowWindow(hwnd, if visible { SW_HIDE } else { SW_SHOW });
                    // 重新显示时立即画一帧（推送模式下不轮询，不画会空白）
                    if !visible {
                        if let Some(state) = get_state_ptr(hwnd).as_ref() {
                            let snap = snap_get();
                            crate::gdi::render(hwnd, &snap, &state.cfg);
                        }
                    }
                }
                WM_RBUTTONUP => {
                    if let Some(cmd) = crate::menu::show_tray_menu(hwnd) {
                        match cmd {
                            crate::menu::IDM_SETTINGS => crate::settings::open_settings(hwnd),
                            crate::menu::IDM_CHARTS => crate::graph::open(),
                            crate::menu::IDM_PROCESSES => crate::proc_list::open(),
                            crate::menu::IDM_QUIT => {
                                let _ = DestroyWindow(hwnd);
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }

        // ── 设置面板改动：重排版 + 自适应窗口尺寸 + 发布调度配置 ──
        WM_RECONFIG => {
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                publish_schedule_cfg(&state.cfg);
                crate::gdi::fit_window(hwnd, &state.cfg);
                let snap = snap_get();
                crate::gdi::render(hwnd, &snap, &state.cfg);
            }
            LRESULT(0)
        }

        // ── 采样线程新快照：可见才渲染（推送式，空闲零开销） ──
        WM_NEW_SNAPSHOT => {
            if IsWindowVisible(hwnd).as_bool() {
                if let Some(state) = get_state_ptr(hwnd).as_ref() {
                    let snap = snap_get();
                    crate::gdi::render(hwnd, &snap, &state.cfg);
                }
            }
            LRESULT(0)
        }

        // 自愈：显示变化 → 重建 DIB 缓存（屏幕 DC 可能改变）+ 直接重画一帧
        WM_DISPLAYCHANGE => {
            crate::gdi::invalidate_dib_cache();
            if IsWindowVisible(hwnd).as_bool() {
                if let Some(state) = get_state_ptr(hwnd).as_ref() {
                    let snap = snap_get();
                    crate::gdi::render(hwnd, &snap, &state.cfg);
                }
            }
            LRESULT(1)
        }

        // Self-heal: system resume → notify sampling thread to reset backoff
        WM_POWERBROADCAST => {
            const PBT_APMRESUMESUSPEND: u32 = 0x0007;
            if lparam.0 as u32 == PBT_APMRESUMESUSPEND {
                if let Some(tx) = RESUME_NOTIFY.get() {
                    let _ = tx.send(());
                }
            }
            LRESULT(1)
        }

        // Self-heal: session lock → pause repaints (stop WM_TIMER briefly)
        WM_WTSSESSION_CHANGE => {
            const WTS_SESSION_LOCK: u32 = 0x7;
            if lparam.0 as u32 == WTS_SESSION_LOCK {
                // Could pause sampling here if we had a pause flag.
                // For now, just stop the timer.
                // The message loop's PeekMessage will keep running without invalidating.
            }
            LRESULT(0)
        }

        WM_ERASEBKGND => LRESULT(1),

        WM_DESTROY => {
            crate::tray::remove(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, _wparam, lparam),
    }
}

// ─── 供设置面板调用的窗口辅助函数（同线程，直接裸指针访问） ──────────

/// 读取悬浮窗当前配置克隆。
pub fn get_config_of(hwnd: HWND) -> Option<Config> {
    let p = unsafe { get_state_ptr(hwnd) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { (*p).cfg.clone() })
    }
}

/// 就地修改悬浮窗配置。
pub fn modify_config(hwnd: HWND, f: impl FnOnce(&mut Config)) {
    let p = unsafe { get_state_ptr(hwnd) };
    if !p.is_null() {
        unsafe { f(&mut (*p).cfg) };
    }
}

/// 「调整大小」模式是否开启。
pub fn resize_mode_active(hwnd: HWND) -> Option<bool> {
    let p = unsafe { get_state_ptr(hwnd) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { (*p).resize_mode })
    }
}

const GWL_STYLE: i32 = -16;

/// 切换「调整大小」模式：加/去 WS_THICKFRAME（设置面板按钮调用）。
pub fn set_resize_mode(hwnd: HWND, on: bool) {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let new_style = if on {
            style | WS_THICKFRAME.0
        } else {
            style & !WS_THICKFRAME.0
        };
        SetWindowLongPtrW(hwnd, GWL_STYLE, new_style as isize);
        // 通知系统非客户区变化
        use windows::Win32::UI::WindowsAndMessaging::{SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE};
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        if let Some(state) = get_state_ptr(hwnd).as_mut() {
            state.resize_mode = on;
        }
    }
}

/// 应用置顶状态（设置面板开关）。
pub fn apply_topmost(hwnd: HWND, on: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{SWP_NOMOVE, SWP_NOSIZE, HWND_NOTOPMOST, HWND_TOPMOST};
    unsafe {
        let insert_after: HWND = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
        let _ = SetWindowPos(
            hwnd,
            insert_after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
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

// ─── OverlayWindow ─────────────────────────────────────────────────────

pub struct OverlayWindow {
    hwnd: HWND,
    /// Owned state; pointer stored in GWLP_USERDATA for wndproc access.
    state: Box<ThreadState>,
}

/// Sets up the resume-notification channel (sampling thread end).
pub fn setup_resume_channel() -> std::sync::mpsc::Receiver<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = RESUME_NOTIFY.set(tx);
    rx
}

impl OverlayWindow {
    pub fn new(cfg: Config) -> Result<Self, WinError> {
        let state = Box::new(ThreadState::new(cfg));
        // GWLP_USERDATA pointer — must stay valid for the lifetime of the window.
        let state_ptr = &*state as *const ThreadState as isize;

        let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();

        let hinst = unsafe { GetModuleHandleW(None) }
            .map(|h| windows::Win32::Foundation::HINSTANCE(h.0))
            .unwrap_or_default();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: windows::Win32::UI::WindowsAndMessaging::HICON::default(),
            hCursor: cursor,
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH::default(),
            lpszMenuName: windows::core::PCWSTR::null(),
            lpszClassName: windows::core::w!("SysOverlay_WndClass"),
            hIconSm: windows::Win32::UI::WindowsAndMessaging::HICON::default(),
        };

        unsafe {
            if RegisterClassExW(&wc) == 0 {
                let err = windows::Win32::Foundation::GetLastError();
                return Err(WinError::new(&format!(
                    "Failed to register window class (error {})", err.0
                )));
            }
        }

        let (screen_w, _) = get_screen_size();

        // 内容自然尺寸（由渲染器决定：横向条 / 经典卡片）；用户曾手动调整过则用保存值
        let (strip_w, strip_h) = crate::gdi::natural_size(&state.cfg);
        let (pos_x, pos_y) = state.cfg.window_pos.unwrap_or((screen_w - strip_w - 24, 24));
        let (win_w, win_h) = state.cfg.window_size.unwrap_or((strip_w, strip_h));

        let mut ex_style = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        if state.cfg.topmost {
            ex_style |= WS_EX_TOPMOST;
        }

        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                windows::core::w!("SysOverlay_WndClass"),
                windows::core::w!("SysOverlay"),
                WS_POPUP | WS_VISIBLE,
                pos_x,
                pos_y,
                win_w,
                win_h,
                None,
                None,
                hinst,
                None,
            )
        };

        if hwnd.is_err() {
            return Err(WinError::new("Failed to create window"));
        }

        let hwnd = hwnd.unwrap();

        unsafe {
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA.0 as i32, state_ptr);
        }

        Ok(Self { hwnd, state })
    }

    pub fn run_message_loop(&self) {
        use windows::Win32::UI::WindowsAndMessaging::GetMessageW;

        let mut msg = MSG::default();
        loop {
            // 阻塞等待消息：空闲时线程挂起（0% CPU）。
            // 渲染由采样线程的 WM_NEW_SNAPSHOT 推送驱动，不再轮询。
            let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if ret.0 <= 0 {
                // 0 = WM_QUIT；-1 = 错误——两者都退出
                break;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    /// Returns the current config (cloned).
    pub fn get_config(&self) -> Config {
        self.state.cfg.clone()
    }

    /// Updates the config in-place (used by menu commands).
    pub fn update_config(&mut self, f: impl FnOnce(&mut Config)) {
        f(&mut self.state.cfg);
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        // Clear GWLP_USERDATA so wndproc can't access freed memory after we drop state.
        unsafe {
            let _ = SetWindowLongPtrW(self.hwnd, GWLP_USERDATA.0 as i32, 0);
        }
        // state Box drops here automatically
    }
}
