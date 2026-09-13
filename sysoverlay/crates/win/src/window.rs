//! Win32 Overlay Window implementation.
//!
//! State is stored in GWLP_USERDATA as a raw pointer to a Box<ThreadState>,
//! eliminating the module-level WINDOW_STATE global mutex.
//!
//! # Safety（本文件 unsafe 汇总）
//!
//! - `GWLP_USERDATA` 指针生命周期：`Box<ThreadState>` 在窗口创建时 `into_raw`
//!   存入，`WM_DESTROY` 中 `Box::from_raw` 取回——裸指针仅在窗口存续期有效，
//!   `GetWindowLongPtrW` 的消费方全部位于本文件的 wndproc 分支内；
//! - `GetWindowLongPtrW/SetWindowLongPtrW` 为手工声明的 32/64 位自适应 FFI
//!   （windows 0.58 未提供按窗口位宽分派的通用绑定）；
//! - 其余 unsafe 为常规 Win32 调用：句柄均来自 API 返回值，不跨线程共享。

use crate::Config;
use crate::Snapshot;
use arc_swap::ArcSwap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetDC, GetDeviceCaps, InvalidateRect,
    ReleaseDC, ValidateRect, HORZRES, PAINTSTRUCT, VERTRES,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, DestroyWindow,
    IsWindowVisible, PostQuitMessage, RegisterClassExW,
    SetWindowPos, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, IDC_ARROW,
    LoadCursorW, MSG, SWP_NOACTIVATE, SWP_NOZORDER, SWP_NOSIZE,
     SW_HIDE, SW_SHOW, WM_MOUSEACTIVATE,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOPMOST,
    WS_EX_TOOLWINDOW, WS_POPUP, WS_THICKFRAME, WS_VISIBLE,
    GetCursorPos,
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
    /// 缓存的窗口像素尺寸（P1-10）：尺寸变化（WM_SIZE/WM_DISPLAYCHANGE）时更新，
    /// 渲染每帧读缓存而非跨进程 `GetWindowRect`。0 表示尚未缓存。
    win_w: i32,
    win_h: i32,
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
            win_w: 0,
            win_h: 0,
        }
    }
}

// ─── Shared snapshot (sampling thread → UI thread) ───────────────────

/// Shared snapshot written by the app's sampling thread, read by WM_PAINT.
static SHARED_SNAP: OnceLock<ArcSwap<Snapshot>> = OnceLock::new();

/// Sequence number of the latest snapshot written by the sampling thread.
pub(crate) static SHARED_SEQ: AtomicU64 = AtomicU64::new(0);

/// Returns the current snapshot, or a placeholder if not yet initialized.
///
/// D3/E3：直接返回 `Arc<Snapshot>`（无锁 `load_full`），调用方经 `&*snap` 取
/// `&Snapshot` 即可，**不再每帧深拷贝整份快照**（共享 Arc 仅在最后一次写后
/// 才被采样线程 `store` 替换）。Snap 本身是 POD，这主要省掉每帧一次 Arc 强引用
/// 计数抖动与整结构体 memcpy。
pub fn snap_get() -> Arc<Snapshot> {
    SHARED_SNAP
        .get_or_init(|| ArcSwap::from_pointee(Snapshot::placeholder(0)))
        .load_full()
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
/// 位 0..4 = cpu/mem/gpu/net/power 启用（位 4 = 功耗采样门 = power_enabled ∧ power_query）。
/// cpu/mem/gpu 位含 `show_util` 的 OR 语义：Util 行开着时，即使 CPU/内存/GPU
/// 行显示关闭，采样也必须继续 —— 否则综合占用无从计算（用户拍板）。
static SCHED_ENABLED: AtomicU32 = AtomicU32::new(0b1_1111);
/// Power 独立采样间隔（毫秒，与刷新率解耦）。
static SCHED_POWER_INTERVAL_MS: AtomicU32 = AtomicU32::new(3000);
/// Net 低频采样开关（独立分频 2s，速率按真实 Δt 计算不失真）。
static SCHED_NET_LOW: AtomicBool = AtomicBool::new(true);
/// Util 计算开关（综合占用行）。true 时采样线程每周期用最新快照算闭式解。
static SCHED_UTIL: AtomicBool = AtomicBool::new(true);

// 配置代际：设置改动 → 代际 +1 并唤醒采样线程（全关停车后靠它唤醒）。
static CFG_GEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
static CFG_CV: std::sync::Condvar = std::sync::Condvar::new();

/// 采样线程睡眠等待：直到配置代际变化或 `timeout_ms` 到期。
/// 返回最新代际。`timeout_ms = 0` 表示纯停车（无限等待，仅被配置变更唤醒）。
pub fn wait_cfg_change(last_gen: u64, timeout_ms: u64) -> u64 {
    let mut gen = CFG_GEN.lock().unwrap_or_else(|e| e.into_inner());
    if *gen != last_gen {
        return *gen;
    }
    if timeout_ms == 0 {
        loop {
            gen = CFG_CV.wait(gen).unwrap_or_else(|e| e.into_inner());
            if *gen != last_gen {
                break;
            }
        }
    } else {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        while *gen == last_gen {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            let (g, _) = CFG_CV
                .wait_timeout(gen, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            gen = g;
        }
    }
    *gen
}

/// 把当前配置发布给采样线程（启动与每次设置改动后调用）。
pub fn publish_schedule_cfg(cfg: &Config) {
    SCHED_AUTO.store(cfg.refresh_auto, Ordering::Relaxed);
    SCHED_INTERVAL_MS.store(cfg.interval_ms, Ordering::Relaxed);
    // 功耗采样门 = 行开关 ∧ 查询开关（性能页）：query 关 → 跳过 MSR 内核查询，
    // 采样器把 Snapshot.power 显式置 Unavailable（功耗行显示 --）。
    let power_gate = cfg.power_enabled && cfg.power_query;
    // Util 数据源保障（用户拍板）：show_util 开启 ⇒ cpu/mem/gpu 采样位强制 ON，
    // 与各自显示行开关取 OR。「行显示关」只关显示，不关 Util 的数据源。
    let util_gate = cfg.show_util;
    let mask = ((cfg.show_cpu || util_gate) as u32)
        | (((cfg.show_mem || util_gate) as u32) << 1)
        | (((cfg.show_gpu || util_gate) as u32) << 2)
        | ((cfg.show_net as u32) << 3)
        | ((power_gate as u32) << 4);
    SCHED_ENABLED.store(mask, Ordering::Relaxed);
    SCHED_POWER_INTERVAL_MS.store(cfg.power_interval_ms, Ordering::Relaxed);
    SCHED_NET_LOW.store(cfg.net_low_freq, Ordering::Relaxed);
    SCHED_UTIL.store(cfg.show_util, Ordering::Relaxed);
    // GPU 查询范围（主适配器/3D 引擎 only）随配置即时生效
    crate::proc_gpu::set_gpu_scope(cfg.gpu_primary_only, cfg.gpu_3d_only);
    // 唤醒采样线程（含全关停车状态）
    let mut gen = CFG_GEN.lock().unwrap_or_else(|e| e.into_inner());
    *gen += 1;
    CFG_CV.notify_all();
    drop(gen);
}

/// 采样线程读取 Util 计算开关（设置页改动即时生效）。
pub fn schedule_util() -> bool {
    SCHED_UTIL.load(Ordering::Relaxed)
}

/// 采样线程读取当前调度参数：(interval_ms, auto, [cpu, mem, gpu, net, power], power_interval_ms)。
pub fn schedule_cfg() -> (u32, bool, [bool; 5], u32) {
    let mask = SCHED_ENABLED.load(Ordering::Relaxed);
    (
        SCHED_INTERVAL_MS.load(Ordering::Relaxed),
        SCHED_AUTO.load(Ordering::Relaxed),
        [
            mask & 1 != 0,
            mask & 2 != 0,
            mask & 4 != 0,
            mask & 8 != 0,
            mask & 16 != 0,
        ],
        SCHED_POWER_INTERVAL_MS.load(Ordering::Relaxed),
    )
}

/// 采样线程读取 Net 低频开关（设置页改动即时生效）。
pub fn schedule_net_low() -> bool {
    SCHED_NET_LOW.load(Ordering::Relaxed)
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

/// Retrieves ThreadState pointer from GWLP_USERDATA.
///
/// 返回 null 表示窗口尚未完成创建（WM_CREATE/WM_SIZE 等消息在
/// `CreateWindowExW` 返回之前同步到达，此时状态指针还未写入）。
/// 调用方用 `if let Some(state) = ...` 跳过即可——**绝不**在此处
/// PostQuitMessage，否则窗口创建阶段就会把整个应用静默杀死。
unsafe fn get_state_ptr(hwnd: HWND) -> *mut ThreadState {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA.0) as *mut ThreadState
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
pub fn is_window_hovered(hwnd: HWND) -> bool {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA.0) };
    if p == 0 {
        false
    } else {
        unsafe { (*(p as *mut ThreadState)).hovered }
    }
}

/// Notifies the app's sampling thread of a system resume event via a oneshot.
static RESUME_NOTIFY: OnceLock<std::sync::mpsc::Sender<()>> = OnceLock::new();

/// 会话锁定状态（BUG-1）：锁定时采样线程挂起，解锁立即补采。
static SESSION_LOCKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 采样线程查询：会话是否处于锁定状态（锁屏安全桌面不显示悬浮窗，采样纯烧 CPU）。
pub fn session_locked() -> bool {
    SESSION_LOCKED.load(std::sync::atomic::Ordering::Relaxed)
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    _wparam: windows::Win32::Foundation::WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // 明确拒绝激活：不抢前台应用焦点。
        WM_MOUSEACTIVATE => LRESULT(3), // MA_NOACTIVATE = 3

        // 命中测试：交给 DefWindowProc（客户区 → HTCLIENT）。
        //
        // 透明区域的穿透**不在这里做**：分层窗口（`WS_EX_LAYERED` +
        // `UpdateLayeredWindow`）的命中测试由系统按像素 alpha 完成，alpha == 0
        // 的像素根本不会命中本窗口，鼠标事件天然落到下方窗口。
        // 所以渲染端必须把背景清成 alpha 恰为 0（见 SoftwareSurface::clear /
        // D2DSurface::clear），不能是 1/255 之类的「近零」值。
        //
        // ⚠️ 不要再回到「WM_NCHITTEST 返回 HTTRANSPARENT」的做法：HTTRANSPARENT
        // 只在**同一线程**的窗口之间有效（MSDN 明确说明；跨进程时窗口管理器找不到
        // 下一个接收者，点击会被直接吃掉 —— 正是「透明区遮挡点击」的现象），
        // 且会让本窗口无法再接收任何鼠标消息。
        //
        // 也**不要**改成 HTCAPTION：那会把客户区当标题栏，鼠标消息变成
        // WM_NCMOUSEMOVE / WM_NCLBUTTONDOWN，自身的 hover / 拖拽逻辑收不到。

        WM_LBUTTONDOWN => {
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                // 记录**屏幕坐标**基准（客户区坐标会随窗口移动而漂移，
                // 拖动用客户区坐标会形成反馈环导致窗口振荡/拖不远）。
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                state.drag_start = pt;
                let mut rect = RECT::default();
                let _ = GetWindowRect(hwnd, &mut rect);
                state.win_start = POINT { x: rect.left, y: rect.top };
                state.dragging = true;
                let _ = SetCapture(hwnd);
            }
            LRESULT(0)
        }

        // WM_MOUSEMOVE 到达即说明鼠标在窗口客户区内（Windows 只向鼠标所在窗口投递）。
        // 标记 hover 并通过 render 重算 src_alpha（悬停 → 恢复不透明）。
        WM_MOUSEMOVE => {
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                if !state.hovered {
                    state.hovered = true;
                    track_mouse_leave(hwnd);
                    crate::render_thread::request(hwnd, &state.cfg);
                }

                if state.dragging {
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let new_x = state.win_start.x + (pt.x - state.drag_start.x);
                    let new_y = state.win_start.y + (pt.y - state.drag_start.y);
                    // 拖动中不 clamp——让窗口自由跟随光标，避免每帧 clamp
                    // 产生的"光标与窗口相对位移失同步"导致的卡顿/弹跳。
                    // 释放时会统一做一次安全钳制（见 WM_LBUTTONUP）。
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        new_x,
                        new_y,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
                    );
                    state.pos_dirty = true;
                }
            }
            LRESULT(0)
        }

        // WM_MOUSELEAVE 只在鼠标真正离开（非 SetCapture 期间）才触发。
        // 离开后标记非 hover，并通过 render 重算 src_alpha（恢复近乎透明）。
        WM_MOUSELEAVE => {
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                if !state.dragging {
                    state.hovered = false;
                    crate::render_thread::request(hwnd, &state.cfg);
                }
            }
            LRESULT(0)
        }

        WM_LBUTTONUP => {
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                state.dragging = false;
                if state.pos_dirty {
                    let mut rect = RECT::default();
                    if GetWindowRect(hwnd, &mut rect).as_bool() {
                        state.cfg.window_pos = Some((rect.left, rect.top));
                        // 用户自由拖动 → 解除贴角：让保存的位置成为唯一定位来源，
                        // 否则残留的 snap_corner<=3 会在下次启动把窗口强行拉回旧角，
                        // 表现为"不记忆窗口位置"。与 WM_EXITSIZEMOVE 的清理逻辑一致。
                        state.cfg.snap_corner = aaxsyspulse_core::config::SNAP_CORNER_FREE;
                    }
                    state.pos_dirty = false;
                }
                if state.size_dirty {
                    let mut rect = RECT::default();
                    if GetWindowRect(hwnd, &mut rect).as_bool() {
                        state.cfg.window_size = Some((rect.right - rect.left, rect.bottom - rect.top));
                    }
                    state.size_dirty = false;
                }
            }
            let _ = ReleaseCapture();
            // SetCapture 期间 WM_MOUSELEAVE 不触发；释放后重新登记，
            // 让系统立即评估鼠标是否还在窗口上（若不在会立刻补发 WM_MOUSELEAVE）。
            track_mouse_leave(hwnd);
            // 拖动模式已结束，立即提交一帧权威画面（而非沿用拖动前的旧帧/悬停帧），
            // 此时 hovered 状态与"光标是否还在窗口上"一致，光标脱窗则自动回到半透明。
            if let Some(state) = get_state_ptr(hwnd).as_ref() {
                crate::render_thread::request(hwnd, &state.cfg);
            }
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
                // 用户自由拖动 → 解除贴角（显示位置回到「自由定位」，
                // 避免下次启动强行拉回旧角，尊重用户刚拖到的位置）。
                if state.dragging {
                    state.cfg.snap_corner = aaxsyspulse_core::config::SNAP_CORNER_FREE;
                }
                state.pos_dirty = false;
                state.size_dirty = false;
                let was_dragging = state.dragging;
                state.dragging = false;
                if was_dragging {
                    track_mouse_leave(hwnd);
                    crate::render_thread::request(hwnd, &state.cfg);
                }
            }
            // 拖拽/缩放刚结束：交互触碰的大量页面即将转冷，主动修剪一次
            // （60s 节流；渲染热页下一帧软故障即回，无正确性影响）。
            crate::mem_tune::trim("overlay drag-end");
            LRESULT(0)
        }

        // Handle window resize: mark dirty so position/size are saved on release.
        WM_SIZE => {
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                // wParam == SIZE_RESTORED(0), SIZE_MAXIMIZED(1), SIZE_MINIMIZED(2)
                if _wparam.0 != 2 {
                    state.size_dirty = true;
                }
                // 缓存窗口尺寸，省去每帧 GetWindowRect（P1-10）
                let mut r = RECT::default();
                if GetWindowRect(hwnd, &mut r).as_bool() {
                    state.win_w = (r.right - r.left).max(1);
                    state.win_h = (r.bottom - r.top).max(1);
                }
                let _ = InvalidateRect(hwnd, None, true);
            }
            LRESULT(0)
        }

        WM_TIMER => {
            // CLOCK 定时器到点：先重对齐下一次到期（显示秒 → 下一个秒边界 1Hz；
            // 隐藏秒 → 下一个分边界 1/分），再走统一渲染入口。例行唤醒不自增
            // 渲染纪元——无新请求时 render 顶部的快速路径会直接返回。
            if _wparam.0 == crate::gdi::CLOCK_TIMER_ID {
                // 节奏跟随当前配置：显示秒 = 1Hz，隐藏秒 = 1/分。
                let secs = get_state_ptr(hwnd)
                    .as_ref()
                    .map(|s| s.cfg.time_show_seconds)
                    .unwrap_or(true);
                crate::gdi::rearm_clock_timer(hwnd, secs);
            }
            // 拖动期间不渲染：随时可能已被 SetWindowPos 刷新过几何，渲染只会
            // 提交过期/悬停帧；等 WM_LBUTTONUP/WM_EXITSIZEMOVE 统一补一帧。
            let dragging = get_state_ptr(hwnd)
                .as_ref()
                .map(|s| s.dragging)
                .unwrap_or(false);
            if !dragging && IsWindowVisible(hwnd).as_bool() {
                if let Some(state) = get_state_ptr(hwnd).as_mut() {
                    crate::render_thread::request(hwnd, &state.cfg);
                }
            }
            LRESULT(0)
        }

        WM_PAINT => {
            // 必须先验证更新区域再干活：本窗口是 WS_EX_LAYERED，像素走
            // UpdateLayeredWindow，但 WM_PAINT 若不清空更新区域，
            // GetMessageW 会以每秒数十万次的速度无限重新合成 WM_PAINT
            // （实测 ~17 万次/秒，CPU 被吃走约 5~10%，同时洪泛消息队列
            // 饿死同一线程上设置窗口的 WM_PAINT——两个症状同源）。
            //
            // 拖动期间（P3-1）：系统模态循环中 SetWindowPos 会触发 WM_PAINT，
            // 此时只清更新区域，等到 WM_EXITSIZEMOVE 才合并渲染一帧。
            let mut ps = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            if hdc.is_invalid() {
                let _ = unsafe { ValidateRect(hwnd, None) };
                return LRESULT(0);
            }
            let is_dragging = get_state_ptr(hwnd)
                .as_ref()
                .map(|s| s.dragging)
                .unwrap_or(false);
            if !is_dragging {
                if let Some(state) = get_state_ptr(hwnd).as_mut() {
                    crate::render_thread::request(hwnd, &state.cfg);
                }
            }
            let _ = unsafe { EndPaint(hwnd, &ps) };
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
                        // E1：唤醒采样线程（事件驱动，不再依赖其隐藏轮询）
                        if let Some(tx) = RESUME_NOTIFY.get() {
                            let _ = tx.send(());
                        }
                        if let Some(state) = get_state_ptr(hwnd).as_ref() {
                            crate::render_thread::request(hwnd, &state.cfg);
                        }
                    }
                }
                WM_RBUTTONUP => {
                    if let Some(cmd) = crate::menu::show_tray_menu(hwnd) {
                        match cmd {
                            crate::menu::IDM_SETTINGS => crate::settings::open_settings(hwnd),
                            crate::menu::IDM_RESTART => crate::restart::quick_restart(),
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

        // ── 设置面板改动：重排版 + 强制整窗刷新 + 发布调度配置 ──
        WM_RECONFIG => {
            let r0 = crate::perf::begin();
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                // ⚠️ 强制整窗刷新（用户实测：像素类设置——颜色/字体粗细——变更后，
                // 局部脏区 ULW 在其系统合成器上不生效 → 画面「看起来没刷新」；
                // 布局类变更因 resize 触发 Full 才生效。设置触发的重绘一律 Full。）
                crate::gdi::invalidate_frame_hash();
                crate::gdi::force_full_refresh();
                crate::probe::log("WM_RECONFIG processed (invalidated + force-full)");
                publish_schedule_cfg(&state.cfg);
                crate::render_thread::request(hwnd, &state.cfg);
            } else {
                crate::probe::log("WM_RECONFIG: no state");
            }
            crate::perf::note_section("wnd_reconfig", r0);
            LRESULT(0)
        }

        // ── 采样线程新快照：可见才渲染（推送式，空闲零开销） ──
        WM_NEW_SNAPSHOT => {
            let t0 = crate::perf::begin();
            // 拖动期间同样不渲染（见 WM_TIMER/WM_PAINT 注释）：采样线程每帧
            // 都会推一帧，若在拖动中渲染会把消失的悬停态/过期帧提交上去。
            // 合并冗余调用：一次取状态指针同时判断拖动与可见性（P2-20）。
            if let Some(state) = get_state_ptr(hwnd).as_ref() {
                if !state.dragging && IsWindowVisible(hwnd).as_bool() {
                    crate::render_thread::request(hwnd, &state.cfg);
                }
            }
            crate::perf::note_section("msg_newsnap", t0);
            LRESULT(0)
        }

        // 自愈：显示变化 → 重建 DIB 缓存（屏幕 DC 可能改变）+ 直接重画一帧
        WM_DISPLAYCHANGE => {
            crate::proc_gpu::invalidate_topology();
            crate::gdi::invalidate_dib_cache();
            // 显示器变化可能改变窗口尺寸/位置 → 使尺寸缓存失效，下次渲染重新取（P1-10）
            if let Some(state) = get_state_ptr(hwnd).as_mut() {
                state.win_w = 0;
                state.win_h = 0;
            }
            if IsWindowVisible(hwnd).as_bool() {
                if let Some(state) = get_state_ptr(hwnd).as_ref() {
                    crate::render_thread::request(hwnd, &state.cfg);
                }
            }
            LRESULT(1)
        }

        // Self-heal: system resume → notify sampling thread to reset backoff
        WM_POWERBROADCAST => {
            // BUG-2：0x0007 = PBT_APMRESUMESUSPEND（用户输入唤醒，前面还有一发
            // 0x0012）；0x0012 = PBT_APMRESUMEAUTOMATIC（定时/计划任务唤醒**只发
            // 此码**）。只听 0x0007 会让定时唤醒的机器漏掉采样退避重置。
            const PBT_APMRESUMESUSPEND: u32 = 0x0007;
            const PBT_APMRESUMEAUTOMATIC: u32 = 0x0012;
            if matches!(lparam.0 as u32, PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC) {
                if let Some(tx) = RESUME_NOTIFY.get() {
                    let _ = tx.send(());
                }
            }
            LRESULT(1)
        }

        // Self-heal: session lock → 暂停采样（BUG-1）；unlock → 清标志并唤醒补采
        WM_WTSSESSION_CHANGE => {
            const WTS_SESSION_LOCK: u32 = 0x7;
            const WTS_SESSION_UNLOCK: u32 = 0x8;
            match lparam.0 as u32 {
                WTS_SESSION_LOCK => {
                    SESSION_LOCKED.store(true, std::sync::atomic::Ordering::Relaxed);
                    crate::probe::log("session locked -> sampler paused");
                }
                WTS_SESSION_UNLOCK => {
                    SESSION_LOCKED.store(false, std::sync::atomic::Ordering::Relaxed);
                    // 复用 resume 通道：唤醒挂起中的采样线程 + 顺带重置退避，
                    // 解锁后第一个采样周期立即补采，消除数据空白窗口。
                    if let Some(tx) = RESUME_NOTIFY.get() {
                        let _ = tx.send(());
                    }
                    crate::probe::log("session unlocked -> sampler resumed");
                }
                _ => {}
            }
            LRESULT(0)
        }

        WM_ERASEBKGND => LRESULT(1),

        WM_DESTROY => {
            unsafe {
                use windows::Win32::System::RemoteDesktop::WTSUnRegisterSessionNotification;
                let _ = WTSUnRegisterSessionNotification(hwnd);
            }
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
        Some(unsafe { (*p).cfg })
    }
}

/// 就地修改悬浮窗配置。
pub fn modify_config(hwnd: HWND, f: impl FnOnce(&mut Config)) {
    let p = unsafe { get_state_ptr(hwnd) };
    if !p.is_null() {
        unsafe { f(&mut (*p).cfg) };
    }
}

/// 读取悬浮窗当前尺寸（缓存；未缓存或窗口未就绪时回退到 `GetWindowRect` 并写回缓存）。
///
/// 渲染每帧都需窗口尺寸；缓存避免每帧一次跨进程 `GetWindowRect` 系统调用（P1-10）。
pub(crate) fn get_window_size(hwnd: HWND) -> (i32, i32) {
    let p = unsafe { get_state_ptr(hwnd) };
    if !p.is_null() {
        let s = unsafe { &*p };
        if s.win_w > 0 && s.win_h > 0 {
            return (s.win_w, s.win_h);
        }
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.as_bool() {
        let w = (rect.right - rect.left).unsigned_abs() as i32;
        let h = (rect.bottom - rect.top).unsigned_abs() as i32;
        if w > 0 && h > 0 {
            if !p.is_null() {
                unsafe {
                    (*p).win_w = w;
                    (*p).win_h = h;
                }
            }
            return (w, h);
        }
    }
    (0, 0)
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

const GWL_EXSTYLE: i32 = -20;

/// 应用「鼠标可穿透」（设置面板开关，报告 §5 窗口页）。
/// 悬浮窗本就是 WS_EX_LAYERED，切换 WS_EX_TRANSPARENT 即可让鼠标事件穿透。
pub fn apply_click_through(hwnd: HWND, on: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{WS_EX_TRANSPARENT, SWP_NOMOVE, SWP_NOSIZE, SWP_FRAMECHANGED};
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let new_ex = if on {
            ex | WS_EX_TRANSPARENT.0
        } else {
            ex & !WS_EX_TRANSPARENT.0
        };
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex as isize);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

/// 把悬浮窗贴到指定屏幕角（报告 §5 Corner：0=右上 1=右下 2=左上 3=左下）。
pub fn snap_overlay_to_corner(hwnd: HWND, corner: u8) {
    use windows::Win32::UI::WindowsAndMessaging::{SWP_NOZORDER, SWP_NOACTIVATE};
    const MARGIN: i32 = 0;
    unsafe {
        let mut r = RECT::default();
        if !GetWindowRect(hwnd, &mut r).as_bool() {
            return;
        }
        let (sw, sh) = get_screen_size();
        let w = r.right - r.left;
        let h = r.bottom - r.top;
        let (x, y) = match corner {
            0 => (sw - w - MARGIN, MARGIN),                 // 右上
            1 => (sw - w - MARGIN, sh - h - MARGIN),        // 右下
            2 => (MARGIN, MARGIN),                          // 左上
            _ => (MARGIN, sh - h - MARGIN),                 // 左下
        };
        let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
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
        let mut state = Box::new(ThreadState::new(cfg));
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
            hIcon: crate::app_icon::large_icon(),
            hCursor: cursor,
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH::default(),
            lpszMenuName: windows::core::PCWSTR::null(),
            lpszClassName: windows::core::w!("AAXSysPulse_WndClass"),
            hIconSm: crate::app_icon::small_icon(),
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
        // 首次启动默认在左上方（x=screen_w/4），避开 AMD overlay 的右上角默认位。
        // 后续位置由用户拖动决定，存入 window_pos 持久化。
        let (pos_x, pos_y) = state.cfg.window_pos.unwrap_or((screen_w / 4, 24));
        // 兜底：历史脏数据里可能存了 0x0（早期拖动缺 SWP_NOSIZE 被缩成 0）。
        // 遇到非正尺寸用内容自然尺寸，避免创建 0x0 的隐形窗口。
        let (saved_w, saved_h) = state
            .cfg
            .window_size
            .unwrap_or((strip_w, strip_h));
        let (win_w, win_h) = if saved_w > 0 && saved_h > 0 {
            (saved_w, saved_h)
        } else {
            (strip_w, strip_h)
        };

        let mut ex_style = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        if state.cfg.topmost {
            ex_style |= WS_EX_TOPMOST;
        }

        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                windows::core::w!("AAXSysPulse_WndClass"),
                windows::core::w!("AAX SysPulse"),
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

        // 不用 unwrap：创建失败路径统一走 Err，避免任何残留的 panic 面。
        let hwnd = match hwnd {
            Ok(h) => h,
            Err(_) => return Err(WinError::new("Failed to create window")),
        };

        unsafe {
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA.0, state_ptr);
        }

        // 会话锁/解锁通知（BUG-1）：没有这个注册，WM_WTSSESSION_CHANGE 永远不会到达。
        unsafe {
            use windows::Win32::System::RemoteDesktop::{
                WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
            };
            let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
        }

        // 启动定位优先级（修复"每次启动都回到右下角 / 不记忆窗口位置"）：
        //   1) 已保存自由定位 window_pos 存在 → 直接采用（CreateWindowExW 已用此值创建窗口），
        //      并把遗留的贴角标记 snap_corner 归位为 FREE，使二者不再冲突。
        //      （贴角 UI 已在设置页删除，任何残留的 snap_corner<=3 都是历史遗留，
        //        不应再盖过用户拖到的位置。）
        //   2) 否则若仍显式选了贴角（snap_corner<=3 且 window_pos 为空）→ 贴到该角。
        //   3) 都没有 → 保留 CreateWindowExW 给出的默认位（screen_w/4, 24）。
        if state.cfg.window_pos.is_some() {
            if state.cfg.snap_corner != aaxsyspulse_core::config::SNAP_CORNER_FREE {
                state.cfg.snap_corner = aaxsyspulse_core::config::SNAP_CORNER_FREE;
            }
        } else if state.cfg.snap_corner <= 3 {
            snap_overlay_to_corner(hwnd, state.cfg.snap_corner);
        }
        if state.cfg.click_through {
            apply_click_through(hwnd, true);
        }

        // 立刻渲染首帧（启动即用，不等第一次采样）。
        //
        // 原因：本窗口是 WS_EX_LAYERED，**在首次 UpdateLayeredWindow 之前完全
        // 不显示**。而 CreateWindowExW 期间同步到达的那个 WM_PAINT，此时
        // GWLP_USERDATA 还没写入（见上），get_state_ptr 为 null → 渲染请求被
        // 丢弃；之后再没有 WM_PAINT 触发，于是窗口要等到采样线程的第一帧
        // （约 2s）才现身。这里补一次请求，用占位快照先把窗口画出来。
        crate::render_thread::request(hwnd, &state.cfg);
        // 兜底：万一渲染线程尚未就绪（通道未建立），再投递一个 WM_PAINT，
        // 由 WM_PAINT 分支在 state 就绪后重试。
        let _ = unsafe { InvalidateRect(hwnd, None, false) };

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
            // 每消息耗时探针（AAXSYSPULSE_PERF=1 时记录慢消息归属）
            let diag = crate::perf::msg_begin();
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            crate::perf::msg_end(diag, msg.hwnd.0 as usize, msg.message);
        }
    }

    /// Returns the current config (cloned).
    pub fn get_config(&self) -> Config {
        self.state.cfg
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
            let _ = SetWindowLongPtrW(self.hwnd, GWLP_USERDATA.0, 0);
        }
        // state Box drops here automatically
    }
}
