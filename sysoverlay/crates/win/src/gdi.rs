//! GDI 渲染管线 —— 双渲染器调度 + DIB 上屏。
//!
//! 管线：
//!   1. 按 [`Config::render_style`] 选渲染器（`ui::Renderer`：Strip / Vertical）
//!   2. 计算**全局 alpha** = 悬停/非悬停两档透明度 + 三次方缓出过渡动画
//!   3. 内容指纹脏检查：指纹未变 → 跳过整帧（不建画布、不 ULW）
//!   4. 渲染器画进复用的 [`SoftwareSurface`] → DIB 缓存 → `UpdateLayeredWindow`
//!
//! 渲染细节（文字/排布）全部在 `ui`，本文件只负责 Win32 的搬运。
//!
//! # 透明度过渡动画
//!
//! 悬停↔非悬停切换时按 [`Config::anim_ms`] 动画到目标透明度，曲线为
//! 三次方缓出（`1-(1-t)^3`，非线性：起步快、收尾慢）。动画期间用
//! `SetTimer(16ms)` 驱动逐帧重绘，到达目标后 `KillTimer` 归零开销。
//!
//! # 适用边界（对抗性审查 §3 结论 + A/B 实测钉死，勿推广到别处）
//!
//! 「内容指纹跳过 + 脏区上传」架构只在以下前提下成立；每条都有 2026-09-12
//! 实测背书（见 docs/悬浮窗性能恢复计划.md 与 PERF_ANALYSIS.md 顶部勘误）：
//!
//! 1. **空间稀疏**：脏区必须物理上小于整窗。0.3 实验（pct=100%，n=113 无一例外）
//!    的成因是当时 `show_time` 下**每秒时钟变化强制 `Dirty::Full`**（历史
//!    workaround，见 85fe335「force full ULW on second change」），不是脏区模型
//!    算大了。该强制逻辑已随 023c91e（pptSrc 根因修复）删除，删除后单格变化的
//!    时钟帧实测 pct=5（见 docs/悬浮窗性能恢复计划.md）。
//!    2026-09-14 的 `Dirty::Rects`（连通分量多矩形）针对的是另一种退化：
//!    **同帧多个互不相邻的格变化**时，单并集矩形会把两格之间的空隙一起上传。
//!    注意它不改善单格变化帧（那种帧旧模型已经只上传该格）。
//! 2. **时间稀疏**：内容在两次采样间稳定。跳帧率对内容节奏敏感（实测 9% 与
//!    0% 两现）——高频变更内容下跳过率归零，收益消失。
//! 3. **ULWIndirect 语义**：整窗 DIB 的 `pptSrc` 必须 = (0,0)（窗口左上角在源
//!    DC 中的对齐点），写脏区左上角会与 `prcDirty` 叠加成 2× 偏移。这是本模块
//!    全部脏矩形上传的根基——`ulw_indirect_rect`（2026-09-14 从 present 与
//!    upload_dib 里抽出的）是本模块**唯一** ULWIndirect 出口，任何新增上传
//!    路径都照抄它，别凭直觉改。
//! 4. **收益已到顶**：整条管线空闲 1.3% 单核；P0 分解 seg_draw=1274µs 是 D2D
//!    引擎固有成本、无单点肥肉，P2 已依红线关闭。不要为再挤 1% 动架构。
//! 5. **引擎正确性**：D2D↔GDI 引擎切换必须强制整窗（`ENGINE_HASH_XOR` 让指纹
//!    不共享）；设置面板重绘强制整窗（`force_full_refresh`）；panic 后下一帧
//!    强制整窗（防陈旧帧）。这些是正确性要求，不是性能选择。

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicU32, Ordering as AtomicOrdering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Instant;

use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, DIB_RGB_COLORS,
    GetDC, ReleaseDC, SelectObject, BI_RGB, BITMAPINFO, HBITMAP, HDC,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::WindowsAndMessaging::{
    KillTimer, PostMessageW, SetTimer, SetWindowPos, UpdateLayeredWindow, UpdateLayeredWindowIndirect,
    UPDATELAYEREDWINDOWINFO, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
};

use aaxsyspulse_ui::offscreen::SoftwareSurface;
use aaxsyspulse_ui::renderer::CellSig;
use aaxsyspulse_ui::{RenderTarget as _, Renderer, StripRenderer, VerticalRenderer};

use aaxsyspulse_core::config::RenderEngine;

use crate::Config;
use crate::perf;
use crate::render_d2d::{paint_cells, D2DSurface, ENGINE_HASH_XOR};
use crate::Snapshot;

/// 悬浮窗透明度动画定时器 ID（仅本窗口使用；graph/procs 的定时器在各自窗口互不干扰）。
pub const ANIM_TIMER_ID: usize = 7;

/// 时钟行定时器 ID：**秒对齐**触发 render（唤醒时刻 = 下一个秒边界 +8ms），
/// 秒变化经帧指纹驱动脏区重绘。window.rs 的 WM_TIMER 不区分 ID（一律 render），
/// 但 wParam==CLOCK_TIMER_ID 时会调用 [`rearm_clock_timer`] 重对齐下一次到期。
pub const CLOCK_TIMER_ID: usize = 8;

/// CLOCK 定时器启停守卫（值 = 目标节奏）：`SetTimer` 同 ID 会**重置到期时间**，
/// 采样推送（最快 100ms 一帧）若每帧都 SetTimer，秒对齐时钟表永远到不了期
/// （饿死）。因此只在节奏/启停状态切换的那一刻 SetTimer/KillTimer 一次。
///
/// 0 = 关（不显示时间行）；1 = 每秒（显示秒）；2 = 每分（隐藏秒）。
/// 隐藏秒时按分钟对齐：秒不再进画面，每秒唤醒纯属浪费（用户实测反馈）。
static CLOCK_TIMER_MODE: AtomicU8 = AtomicU8::new(0);

/// 最近一次成功整窗上传时的时钟秒值。局部脏区上传（UpdateLayeredWindowIndirect
/// + prcDirty）在部分合成器（本机实测）上不会触发该子区重混 → 时间单元格每秒
/// 都上传了但屏幕不更新，只有等下一次整窗重绘机会才"攒着一起显现"——视觉上
/// 就是时间跟着调度器几秒一跳。秒变化时强制 Full 上传（每秒 1 次），其余帧
/// 仍走脏区优化。
static LAST_PRESENTED_SEC: AtomicU32 = AtomicU32::new(u32::MAX);

/// 距下一个**墙钟**秒边界的毫秒数（+8ms 余量保证醒来时秒已翻转）。
///
/// ⚠️ 必须对齐 `GetLocalTime` 的毫秒位，而不是 GetTickCount64 的秒边界：
/// tick 秒边界与墙钟秒边界只在开机瞬间对齐，此后相差一个固定相位
/// （本机实测 ~0.308s）。按 tick 对齐会让每次触发落在墙钟翻秒前 ~300ms，
/// 渲染读到旧秒 → 内容指纹未变 → 整帧被跳过 → 新秒要等下一拍才显示，
/// 表现为秒显示忽快忽慢（2026-09-12 相位直方图实测：绘制主峰 700-799ms）。
fn ms_to_next_second() -> u32 {
    let ms = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() }.wMilliseconds;
    (1000 - u32::from(ms)).max(1) + 8
}

/// 距下一个**墙钟**分边界（秒=00）的毫秒数（+8ms 余量）。
///
/// 隐藏秒后时间文本每分钟才变一次（HH:MM），按秒唤醒纯属空转：
/// 快速跳过路径比较的是**真实秒**（`clock::get().2`）而非格式化文本，
/// 秒一变就不相等 → 每秒仍要跑完 layout + FNV 全帧指纹才发现"画面没变"。
/// 改为按分钟对齐，隐藏秒时唤醒频率从 1Hz 降到 1/60Hz。
fn ms_to_next_minute() -> u32 {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let ms = u32::from(t.wMilliseconds);
    let sec = u32::from(t.wSecond);
    ((59 - sec) * 1000 + (1000 - ms)).max(1) + 8
}

/// 目标节奏对应的对齐间隔（毫秒）。
fn clock_interval(show_seconds: bool) -> u32 {
    if show_seconds {
        ms_to_next_second()
    } else {
        ms_to_next_minute()
    }
}

/// 目标节奏：0 = 关，1 = 每秒，2 = 每分。
fn clock_mode(want: bool, show_seconds: bool) -> u8 {
    if !want {
        0
    } else if show_seconds {
        1
    } else {
        2
    }
}

/// 按 `show_time` / `time_show_seconds` 同步 CLOCK 定时器（render 每帧调用；
/// swap 保证只在节奏变化的瞬间重设一次）。
fn sync_clock_timer(hwnd: HWND, want: bool, show_seconds: bool) {
    let mode = clock_mode(want, show_seconds);
    if mode != CLOCK_TIMER_MODE.swap(mode, AtomicOrdering::Relaxed) {
        unsafe {
            if mode == 0 {
                let _ = KillTimer(hwnd, CLOCK_TIMER_ID);
            } else {
                let _ = SetTimer(hwnd, CLOCK_TIMER_ID, clock_interval(show_seconds), None);
            }
        }
    }
}

/// CLOCK 定时器到点后的重对齐（UI 线程 WM_TIMER wParam==CLOCK_TIMER_ID 时调用）。
/// 按当前节奏（显示秒 = 1Hz，隐藏秒 = 1/分）重设下一次到期。
/// 此处重设不会饿死：此刻正是它自己的到期处理中，采样推送不参与 SetTimer。
pub fn rearm_clock_timer(hwnd: HWND, show_seconds: bool) {
    unsafe {
        let _ = SetTimer(hwnd, CLOCK_TIMER_ID, clock_interval(show_seconds), None);
    }
}

/// 按配置选择渲染器实例（'static，零分配）。旧卡片式（Classic）折入横版条。
fn renderer_for(style: aaxsyspulse_core::types::RenderStyle) -> &'static dyn Renderer {
    match style {
        aaxsyspulse_core::types::RenderStyle::Classic | aaxsyspulse_core::types::RenderStyle::Strip => {
            &StripRenderer
        }
        aaxsyspulse_core::types::RenderStyle::Vertical => &VerticalRenderer,
    }
}

// ── 透明度过渡动画状态（UI 线程独占） ──────────────────────

/// 一次过渡动画：从 `from_pct` 经过 `anim_ms` 到 `to_pct`（百分比 0..=100）。
#[derive(Clone, Copy, Debug)]
struct AnimState {
    active: bool,
    t0: Instant,
    from_pct: f32,
    to_pct: f32,
}

impl Default for AnimState {
    fn default() -> Self {
        Self {
            active: false,
            t0: Instant::now(),
            from_pct: 35.0,
            to_pct: 35.0,
        }
    }
}

static ANIM_STATE: LazyLock<Mutex<AnimState>> = LazyLock::new(|| Mutex::new(AnimState::default()));

/// 上一帧实际送屏的 alpha（百分比 × 100，0..=10000，避免浮点原子）。
static RENDERED_PCT_X100: AtomicU32 = AtomicU32::new(3500);

/// 三次方缓出：t ∈ [0,1] → e ∈ [0,1]，起步快、收尾慢（非线性过渡）。
fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// 计算本帧 alpha 百分比；动画进行中返回 true（调用方应确保后续帧继续到来）。
fn compute_alpha_pct(_hwnd: HWND, cfg: &Config) -> (f32, bool) {
    let desired = cfg.normal_opacity as f32;
    let anim_ms = cfg.anim_ms as f32;
    let rendered = RENDERED_PCT_X100.load(AtomicOrdering::Relaxed) as f32 / 100.0;

    let Ok(mut anim) = ANIM_STATE.lock() else {
        return (desired, false);
    };

    if anim.active {
        let t = (anim.t0.elapsed().as_secs_f32() * 1000.0 / anim_ms).clamp(0.0, 1.0);
        if t >= 1.0 {
            anim.active = false;
            anim.from_pct = anim.to_pct;
            return (anim.to_pct, false);
        }
        let e = ease_out_cubic(t);
        let pct = anim.from_pct + (anim.to_pct - anim.from_pct) * e;
        return (pct, true);
    }

    // 未在过渡：仅在目标变化时启动新的过渡（含 anim_ms=0 的直接跳变）
    if (rendered - desired).abs() < 0.05 {
        return (desired, false);
    }
    if anim_ms <= 0.5 {
        return (desired, false);
    }
    anim.active = true;
    anim.t0 = Instant::now();
    anim.from_pct = rendered;
    anim.to_pct = desired;
    (rendered, true)
}

/// 上一帧的指纹（`u64::MAX` 表示还没有画过）。
static LAST_FRAME_HASH: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(u64::MAX);

/// 脏矩形上屏开关：`AAXSYSPULSE_NODIRTY=1` 时退回经典整窗上传（A1 回退）。
static DIRTY_DISABLED: LazyLock<bool> =
    LazyLock::new(|| std::env::var("AAXSYSPULSE_NODIRTY").map(|v| v == "1").unwrap_or(false));

/// 上屏失败后的整窗恢复闸门：任一 ULW/ULWIndirect 失败即置位，下一帧
/// `compute_dirty_in` 消费之并强制整窗。与"帧指纹回滚"配合使用：
/// 指纹回滚保证重试帧不被跳过，本闸门保证重试帧整窗覆盖（缺一会冻结）。
static NEEDS_FULL_REUPLOAD: AtomicBool = AtomicBool::new(false);

/// 脏矩形追踪：上一帧各文字 cell 的几何 + 内容指纹（A1 比对源）。
static PREV_CELLS: OnceLock<Mutex<Vec<CellSig>>> = OnceLock::new();

/// 高画质（D2D）路径的上一帧 cell 快照（与性能模式独立）。
static PREV_CELLS_D2D: OnceLock<Mutex<Vec<CellSig>>> = OnceLock::new();

/// 脏矩形追踪：上一帧送屏的全局 alpha（alpha 变化必须整窗重画，否则残留）。
static PREV_ALPHA: AtomicU8 = AtomicU8::new(0);

/// 高画质（D2D）路径的上一帧 alpha（与性能模式独立）。
static PREV_ALPHA_D2D: AtomicU8 = AtomicU8::new(0);

/// 设置变更后强制下一帧**整窗刷新**（dirty=Full，整窗 ULW）。
///
/// 背景（2026-09-09 用户实测定位）：颜色/字体粗细等「像素类」设置变更后，帧指纹
/// 变了 → render 正常执行 → 但脏区是**局部 Rect**（仅变化的 cell）→ 局部
/// UpdateLayeredWindowIndirect 在部分系统/驱动合成器上**不生效** → 画面看起来
/// 「没刷新」；而字号/间距等「布局类」变更触发窗口 resize → dirty=Full → 整窗
/// ULW → 立即生效；「等下一次数值刷新」也是因数值文本变宽触发布局变化才 Full。
/// 结论：凡是设置面板触发的重绘，一律整窗上传（设置操作低频，Full 开销可忽略）。
static FORCE_FULL_REFRESH: AtomicBool = AtomicBool::new(false);

/// 设置面板调用：下一次 render 强制整窗刷新。
pub fn force_full_refresh() {
    FORCE_FULL_REFRESH.store(true, AtomicOrdering::Relaxed);
}

/// 消费强制整窗标志（render 每帧调用；true 则本帧整窗重绘）。
fn take_force_full() -> bool {
    FORCE_FULL_REFRESH.swap(false, AtomicOrdering::Relaxed)
}

/// 渲染请求纪元：每次 [`request_render`] 自增；render 真正处理到帧时记录
/// 已消费值（[`HANDLED_EPOCH`]）。render 顶部的快速跳过路径据此区分
/// 「确有新请求」与「秒对齐时钟的例行唤醒」——后者若无事可做直接返回。
static RENDER_EPOCH: AtomicU64 = AtomicU64::new(0);
static HANDLED_EPOCH: AtomicU64 = AtomicU64::new(0);
/// 渲染看门狗：请求纪元首次领先受理纪元的时刻（0 = 无积压）。
static WATCHDOG_PENDING_MS: AtomicU64 = AtomicU64::new(0);
/// 停摆 >10s 的日志只打一次（直到恢复）。
static WATCHDOG_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 帧在途标记：当前是否有渲染线程正在画一帧，以及该帧开始时刻。
///
/// 语义：**0 = 无在途帧**（空闲/健康）；非 0 = 有帧在途。
/// 只由当前渲染线程写入；`respawn` 后旧线程（仍卡在原生调用里）若回写，
/// 会被 `render_frame_end` 用世代号拦下（见下）。
static FRAME_START_MS: AtomicU64 = AtomicU64::new(0);
/// 当前「活跃渲染线程」的世代号，由 [`crate::render_thread::start`] 自增。
/// 旧线程的 gen 落后于它 → 其心跳被视为过期、丢弃。
static RENDER_GEN: AtomicU64 = AtomicU64::new(0);
/// 已为「本次停摆」触发过 respawn（幂等闸门）：避免一次卡死被每个采样 tick
/// 重复 respawn 出多个线程。帧恢复完成后清零。
static WEDGE_RESPAWNED: AtomicBool = AtomicBool::new(false);
/// 软停摆日志只打一次（直到帧恢复清零）。
static WATCHDOG_WEDGED: AtomicBool = AtomicBool::new(false);

/// 开启新一代渲染线程：由 [`crate::render_thread::start`] 调用。
/// 让旧线程的迟到心跳失效（世代 +1），并把「在途帧」清空——新线程从
/// **无在途帧**的干净状态起步（否则旧线程遗留的 FRAME_START_MS 会让看门狗
/// 误以为新线程一直卡着，从而误禁 D2D）。
///
/// 幂等闸门保留：一次停摆只 respawn 一次；新线程若又卡死，其**自己**会在
/// 5s 后写入新的 FRAME_START_MS，看门狗据此走 D2D 回退分支。
pub fn begin_render_generation() -> u64 {
    let g = RENDER_GEN.fetch_add(1, AtomicOrdering::Relaxed) + 1;
    // 新世代从无在途帧开始：清掉旧线程的残留时间戳。
    FRAME_START_MS.store(0, AtomicOrdering::Relaxed);
    g
}

/// 当前世代号（`render_thread` 启动时捕获，随每次帧心跳回写）。
pub fn render_generation() -> u64 {
    RENDER_GEN.load(AtomicOrdering::Relaxed)
}

/// 标记一帧开始（render 线程进入 [`render`] 前调用，携带本线程捕获的世代号）。
/// 旧线程若在此处写入，`render_frame_end` 的世代检查会使其自相矛盾——
/// 所以这里只在 gen == 当前世代时才写，杜绝旧线程刷新在途时间。
pub fn render_frame_begin(gen: u64) {
    // 只有当前活跃世代才允许开启新在途帧；旧线程的调用直接丢弃。
    if gen != RENDER_GEN.load(AtomicOrdering::Relaxed) {
        return;
    }
    // ⚠️ 必须与看门狗调用方传入的 now_ms 同一时钟（GetTickCount64 = 系统运行毫秒）。
    // 曾用 SystemTime::now()（UNIX 纪元毫秒，~1.789e12）：与 tick 毫秒（~1e7）相减
    // saturating 归零 → 5s 软停摆从未触发，每次卡死都掉回 30s 纪元路径。
    // 2026-09-12 深夜两次带探针复现（probe_1350/1400）抓出，编译与单测均无法发现。
    FRAME_START_MS.store(
        unsafe { windows::Win32::System::SystemInformation::GetTickCount64() },
        AtomicOrdering::Relaxed,
    );
}

/// 标记一帧结束（render 返回后调用）。**只有当前世代的线程才能清标记**——
/// 旧线程（已被 respawn 淘汰）的迟到写入必须被忽略，否则会误清新线程的在途帧。
pub fn render_frame_end(gen: u64) {
    if gen != RENDER_GEN.load(AtomicOrdering::Relaxed) {
        return; // 旧世代：丢弃
    }
    FRAME_START_MS.store(0, AtomicOrdering::Relaxed);
}

/// 帧恢复（当前世代已在途帧归零）时清零软停摆闸门，允许未来重新检测。
fn clear_wedge_gate() {
    WEDGE_RESPAWNED.store(false, AtomicOrdering::Relaxed);
    WATCHDOG_WEDGED.store(false, AtomicOrdering::Relaxed);
}

/// 看门狗动作：告诉调用方（采样线程）该做什么。调用方持有 overlay hwnd，
/// 才能做「禁用 D2D + 持久化配置」这类需要 hwnd 的收尾工作。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WatchdogAction {
    /// 健康，什么都不做。
    None,
    /// 渲染线程停摆 → 重建渲染线程（respawn）。
    Respawn,
    /// 新线程也卡死 → 禁用 D2D 并持久化回退 GDI（调用方需 hwnd）。
    DisableD2d,
}

/// 采样线程每轮调用：检测渲染线程停摆（挂起/死亡/卡在原生调用）。
/// 判据一 = 请求纪元领先受理纪元持续不消（30s → 线程没消费请求）。
/// 判据二（更快）= 当前世代有一帧在途超过 5s（线程活着但卡在原生调用里）。
pub fn render_watchdog_tick(now_ms: u64) -> WatchdogAction {
    const STALL_LOG_MS: u64 = 10_000;
    const STALL_RESPAWN_MS: u64 = 30_000;
    // ── 软停摆快检（优先于纪元判据）──
    // 一帧超过阈值没画完 = 渲染线程卡在某个原生调用里（不是死、是阻塞，旧线程
    // 无法被取消）。阈值远小于 30s 纪元阈值，把「悬浮窗冻结」大幅缩短。
    // ⚠️ 幂等：同一次停摆只 respawn 一次（WEDGE_RESPAWNED 闸门），否则每个采样
    // tick 都会新建线程，造成线程泄漏。
    // ⚠️ 世代隔离：只认当前世代的帧心跳。旧线程（卡在原生调用里）迟到的
    // render_frame_end 带旧 gen，被 render_frame_end 忽略，不会误清新线程的标记。
    const HARD_STALL_MS: u64 = 5_000;
    let f_start = FRAME_START_MS.load(AtomicOrdering::Relaxed);
    if f_start != 0 {
        let fage = now_ms.saturating_sub(f_start);
        if fage > HARD_STALL_MS {
            if !WATCHDOG_WEDGED.swap(true, AtomicOrdering::Relaxed) {
                crate::probe::log(&format!(
                    "render watchdog: frame in-progress for {fage}ms (render thread wedged in native call?)"
                ));
            }
            // 幂等闸门：本次停摆只触发一次 respawn。旧线程即使永不返回，
            // 也不会被反复重开（不会无限泄漏线程）。
            if !WEDGE_RESPAWNED.swap(true, AtomicOrdering::Relaxed) {
                return WatchdogAction::Respawn; // 触发一次 respawn
            }
            // 已 respawn 过但当前世代仍在途超过阈值 → 新线程也卡住了。
            // 优先按「D2D 原生调用卡死」处置：禁用 D2D 并持久化回退 GDI。
            // 若本来就是 GDI 引擎（D2D 已禁），不误报，退化为等纪元判据。
            if !D2D_BROKEN.load(AtomicOrdering::Relaxed) {
                crate::probe::log(
                    "render watchdog: fresh thread also wedged -> disable D2D (persist GDI)",
                );
                return WatchdogAction::DisableD2d;
            }
            return WatchdogAction::None; // 不再重复 respawn
        }
    } else {
        // 当前世代无在途帧 → 渲染线程空闲且健康，清零闸门，允许未来重新检测。
        clear_wedge_gate();
    }
    let pending = RENDER_EPOCH.load(AtomicOrdering::Relaxed)
        != HANDLED_EPOCH.load(AtomicOrdering::Relaxed);
    if !pending {
        WATCHDOG_PENDING_MS.store(0, AtomicOrdering::Relaxed);
        WATCHDOG_LOGGED.store(false, AtomicOrdering::Relaxed);
        return WatchdogAction::None;
    }
    let first = WATCHDOG_PENDING_MS.load(AtomicOrdering::Relaxed);
    if first == 0 {
        WATCHDOG_PENDING_MS.store(now_ms, AtomicOrdering::Relaxed);
        return WatchdogAction::None;
    }
    let stalled_for = now_ms.saturating_sub(first);
    if stalled_for > STALL_LOG_MS && !WATCHDOG_LOGGED.swap(true, AtomicOrdering::Relaxed) {
        crate::probe::log(&format!(
            "render watchdog: epoch unhandled for {stalled_for}ms (render thread stalled?)"
        ));
    }
    if stalled_for > STALL_RESPAWN_MS {
        WatchdogAction::Respawn
    } else {
        WatchdogAction::None
    }
}

/// 请求悬浮窗重绘：纪元自增 + 消息唤醒。
/// 所有非时钟驱动的重绘请求（快照推送 / 设置变更 / 自愈扩缩窗）都必须走
/// 这里；秒对齐 CLOCK 定时器的例行唤醒**不得**自增纪元（否则快速路径失效）。
pub fn request_render(hwnd: HWND) {
    RENDER_EPOCH.fetch_add(1, AtomicOrdering::Relaxed);
    unsafe {
        let _ = PostMessageW(
            hwnd,
            crate::window::WM_NEW_SNAPSHOT,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(0),
        );
    }
}

/// 上一帧**实际送屏**的窗口尺寸（高 32 位 = w，低 32 位 = h；0 = 从未送屏）。
///
/// ⚠️ 脏区 Rect 上传的前提是「DIB 里非脏区域像素 == 窗口当前显示像素」。
/// 窗口尺寸一变，`SoftwareSurface::resize` 与 `DibCache::get` 都会**清零重建**，
/// 该前提即被破坏——此时若仍按 Rect 只上传脏子矩形，窗口其余部分会变全透明
/// （实测症状：上半部分和左侧消失，悬停改变 alpha 触发 Full 后才恢复）。
/// 因此：尺寸与上次送屏不同（或 DIB 缓存被废弃）→ 本帧强制整窗。
static LAST_PRESENT_SIZE: AtomicU64 = AtomicU64::new(0);

/// 使「上次送屏尺寸」记录失效（DIB 缓存被废弃 / 显示器变化时调用），
/// 下一帧强制整窗上传。
fn invalidate_present_size() {
    LAST_PRESENT_SIZE.store(0, AtomicOrdering::Relaxed);
}

/// D2D 高画质路径本会话是否仍可用。一旦 D2D 渲染 panic / 失败即置 false
/// 并回退 Gdi——否则配置持久为 `render_engine=D2d` 时重启也会进 D2D 卡死，
/// 用户无法从设置里改回来（「设置也打不开」）。
static D2D_BROKEN: AtomicBool = AtomicBool::new(false);

/// 看门狗 DisableD2d 动作的公开入口（供采样线程在持有 hwnd 时调用）。
pub fn disable_d2d_and_persist(hwnd: HWND) {
    mark_d2d_broken_and_persist_gdi(hwnd);
}

/// D2D 渲染失败后的自愈合：本会话禁用 D2D，并把持久配置回退为 Gdi。
fn mark_d2d_broken_and_persist_gdi(hwnd: HWND) {
    D2D_BROKEN.store(true, AtomicOrdering::Relaxed);
    if let Some(mut cfg) = crate::window::get_config_of(hwnd) {
        if cfg.render_engine == RenderEngine::D2d {
            cfg.render_engine = RenderEngine::Gdi;
            crate::window::modify_config(hwnd, |c| *c = cfg);
            let _ = crate::config_io::save_config(&cfg);
            crate::probe::log("d2d disabled (session) + persisted render_engine=Gdi");
        }
    }
}

/// 本帧是否必须整窗重绘+整窗上传（尺寸相对上次送屏发生了变化）。
fn present_size_changed(width: i32, height: i32) -> bool {
    let cur = ((width as u64) << 32) | (height as u32 as u64);
    let prev = LAST_PRESENT_SIZE.swap(cur, AtomicOrdering::Relaxed);
    prev != cur
}

/// 脏区：整窗，或若干连通分量矩形（可能边缘相碰，语义上互不重叠的"变化区域簇"）。
///
/// 多矩形语义：每个矩形独立走一次 `UpdateLayeredWindowIndirect(prcDirty=矩形)`，
/// `pptSrc` 恒为 (0,0)（见文件头适用边界第 3 条）。空向量 = 本帧没有任何像素变化
/// → 跳过上屏（DIB 与窗口显示本就一致；该分支接替了旧"无变化 → Full"承担的
/// 失败自愈职责，改由 [`NEEDS_FULL_REUPLOAD`] 闸门承担）。
/// 分量数超过 [`MAX_DIRTY_RECTS`] 时折叠为 `Full`，防病态布局下的系统调用风暴。
#[derive(Debug)]
enum Dirty {
    Full,
    Rects(Vec<RECT>),
}

impl Dirty {
    fn is_full(&self) -> bool {
        matches!(self, Dirty::Full)
    }
}

/// 脏区分量上限。横条布局 ≤ 10 格（MetricRow 上限），8 绰绰有余；
/// 触顶说明布局病态或调用方漏判 Full 条件，保守整窗。
const MAX_DIRTY_RECTS: usize = 8;

/// 探针用脏区摘要："Full" / "None(skip)" / "Rects(n=2, (l,t,r,b) (l,t,r,b))"。
fn fmt_dirty(dirty: &Dirty) -> String {
    match dirty {
        Dirty::Full => "Full".into(),
        Dirty::Rects(rs) if rs.is_empty() => "None(skip)".into(),
        Dirty::Rects(rs) => format!(
            "Rects(n={}, {})",
            rs.len(),
            rs.iter()
                .map(|r| format!("({},{},{},{})", r.left, r.top, r.right, r.bottom))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    }
}

/// 悬浮窗合理尺寸上限（px，>4K 屏宽度无意义；防异常尺寸导致 DIB 分配爆炸卡死）。
const MAX_WIN: i32 = 2048;

/// 渲染一帧到窗口（WM_NEW_SNAPSHOT / 尺寸变化 / 设置变化 入口）。
///
/// **脏检查**：内容与上一帧完全一致时整帧跳过——不建画布、不建 DIB、
/// 不 `UpdateLayeredWindow`。静止的机器上这会把渲染开销打到 0。
pub fn render(hwnd: HWND, snap: &Snapshot, cfg: &Config) {
    let t0 = perf::begin();
    // 渲染入口探针（14:44:04 冻结定位）：rt wake 有而本行无 → 挂在
    // render_once 预处理（RENDER_CFG/snap_get）；本行有而 d2d render cfg
    // 无 → 挂在下方 gap（alpha/尺寸/引擎检测）。
    crate::probe::log_lazy(|| "render enter".to_string());
    unsafe {
        // 本地时钟 → ui::clock（时间行数据源）。每次渲染前同步；
        // 时间文本进 cell 指纹，秒变化经脏检查只重绘时间 cell（脏渲染）。
        // windows 0.58：GetLocalTime() 无参、返回 SYSTEMTIME（Foundation 模块）。
        let st = GetLocalTime();
        aaxsyspulse_ui::clock::set_local(st.wHour as u32, st.wMinute as u32, st.wSecond as u32);
        // 时钟定时器按 show_time / time_show_seconds 同步启停与节奏
        //（SetTimer 重置问题见 CLOCK_TIMER_MODE）。
        sync_clock_timer(hwnd, cfg.show_time, cfg.time_show_seconds);

        // ── P0 快速跳过：例行唤醒无事可做时 ~0.1µs 返回，连布局/指纹都不建。
        // 旧 250ms 时钟每秒 4 次唤醒，其中 3 次做完整 layout + FNV 全帧指纹
        // 后才发现"秒没变"。条件覆盖所有会改变画面的输入：请求纪元 / 时钟秒 /
        // 强制整窗 / alpha 过渡动画 / 渲染引擎切换 / alpha 与上次上屏值。
        let (alpha_pct_f, animating) = compute_alpha_pct(hwnd, cfg);
        let engine_id = match cfg.render_engine {
            RenderEngine::Gdi => 0u8,
            RenderEngine::D2d => 1u8,
        };
        let alpha_pct_x100 = (alpha_pct_f.clamp(1.0, 100.0) * 100.0) as u32;
        if !animating
            && HANDLED_EPOCH.load(AtomicOrdering::Relaxed)
                == RENDER_EPOCH.load(AtomicOrdering::Relaxed)
            && aaxsyspulse_ui::clock::get().2 == LAST_PRESENTED_SEC.load(AtomicOrdering::Relaxed)
            && !FORCE_FULL_REFRESH.load(AtomicOrdering::Relaxed)
            && engine_id == LAST_ENGINE.load(AtomicOrdering::Relaxed)
            && alpha_pct_x100 == RENDERED_PCT_X100.load(AtomicOrdering::Relaxed)
        {
            perf::note_skipped();
            // 不调 end_render：跳过帧不是渲染帧，混入会污染 p50/p99（0.4 修正）。
            return;
        }

        // 尺寸走缓存（P1-10）：避免每帧一次跨进程 GetWindowRect 系统调用。
        let (width, height) = crate::window::get_window_size(hwnd);
        if width == 0 || height == 0 {
            crate::perf::note_section("render_early_zero", t0);
            return;
        }
        if width > MAX_WIN || height > MAX_WIN {
            crate::probe::log(&format!(
                "render: 异常窗口尺寸 ({width}x{height}) > {MAX_WIN} -> 回到自然尺寸"
            ));
            force_fit_natural(hwnd, cfg);
            request_render(hwnd);
            crate::perf::note_section("render_early_big", t0);
            return;
        }

        let renderer = renderer_for(cfg.render_style);

        // 引擎切换检测：性能 ↔ 高画质 切换后强制整窗重绘。
        // 两引擎的 cell 内容指纹完全相同（文本/几何/颜色一致），只有像素外观
        // 不同 → 脏区比对结果为「无变化」→ Rects(空) 直接跳过上屏，
        // 画面停留在旧引擎像素，直到悬停改变 alpha 触发整窗重画
        // （实测症状：切引擎后必须鼠标悬停才生效）。这里在指纹层面强制失效。
        static LAST_ENGINE: AtomicU8 = AtomicU8::new(u8::MAX);
        if LAST_ENGINE.swap(engine_id, AtomicOrdering::Relaxed) != engine_id {
            LAST_FRAME_HASH.store(u64::MAX, AtomicOrdering::Relaxed);
            invalidate_present_size();
            crate::probe::log("render engine switched → force full repaint");
        }

        // 全局 alpha：悬停/非悬停两档透明度，切换时三次方缓出过渡。
        // 动画进行中 → SetTimer 逐帧推进；结束后 KillTimer 归零开销。
        // （已在快速路径处计算，这里直接复用。）
        let src_alpha = ((alpha_pct_f.clamp(1.0, 100.0) / 100.0) * 255.0).round() as u8;

        // 高画质模式：整帧 Direct2D + DirectWrite（性能模式路径原样走下方 GDI 分支）。
        // ⚠️ 自愈合：D2D 绑 DIB 内存 DC 在部分驱动/显卡上可能死锁或 panic（见
        // render_d2d 注释）。用 catch_unwind 兜住 panic；一旦失败本会话禁用 D2D
        // 并回退 Gdi（否则下次启动仍进 D2D → 卡死，用户无法自救）。
        // 诊断探针（冻结定位）：确认 render 前导段已走完（GetLocalTime/clock/
    // sync_clock/compute_alpha/get_window_size/layout），卡点在其后。
    crate::probe::log_lazy(|| "render preamble done".to_string());
    if cfg.render_engine == RenderEngine::D2d && !D2D_BROKEN.load(AtomicOrdering::Relaxed) {
            static ANNOUNCED: AtomicBool = AtomicBool::new(false);
            if !ANNOUNCED.swap(true, AtomicOrdering::Relaxed) {
                crate::probe::log("engine=D2d (high quality) — first frame");
            }
            static D2D_FAILS: AtomicU32 = AtomicU32::new(0);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                render_d2d(hwnd, snap, cfg, renderer, width, height, src_alpha, alpha_pct_f, animating, t0)
            }));
            match outcome {
                Ok(true) => {
                    D2D_FAILS.store(0, AtomicOrdering::Relaxed);
                    perf::note_section("render_d2d_ok", t0);
                    return;
                }
                Ok(false) => {
                    // 本帧 D2D 绘制失败（bind 失败/设备丢失等）。⚠️ 关键修复：
                    // 旧实现直接 return → 每帧都空过 → 悬浮窗整体消失（用户实测
                    // 「切渲染模式后悬浮窗不见了」的根因）。现在立即走 GDI 分支
                    // 兜底上屏；连续失败 5 次判定 D2D 不可用并持久回退。
                    let n = D2D_FAILS.fetch_add(1, AtomicOrdering::Relaxed) + 1;
                    crate::probe::log(&format!("d2d frame failed x{n} → gdi fallback this frame"));
                    if n >= 5 {
                        D2D_FAILS.store(0, AtomicOrdering::Relaxed);
                        mark_d2d_broken_and_persist_gdi(hwnd);
                    }
                }
                Err(_) => {
                    D2D_FAILS.store(0, AtomicOrdering::Relaxed);
                    crate::probe::log("engine=D2d PANICKED → fall back to Gdi this session");
                    mark_d2d_broken_and_persist_gdi(hwnd);
                }
            }
            // 继续往下走 GDI 分支
        }

        // 布局（build_rows 只跑一次，C1）→ cell 指纹 → 门面指纹（脏检查）
        // 先定字重：性能路径的字形源按字重切换（DIN 三版注入位图表），
        // 必须在 paint 前设好；帧指纹另经 hash_cells 纳入 cfg.font_weight。
        // ensure_loaded 幂等（内部 OnceLock），首帧前把三档 DIN 栅格化并原子发布，
        // 此后渲染期零锁纯数组读 → 不引入逐帧开销。
        crate::din_bitmap::ensure_loaded();
        aaxsyspulse_ui::font::set_active_weight(cfg.font_weight);
        // 诊断探针：悬浮窗渲染侧实际读到的指标颜色（对照设置页写入前后）。
        crate::probe::log_lazy(|| {
            format!(
                "render cfg: cpu_label={:?} cpu_value={:?} gpu_label={:?} fw={} engine={} sized={}",
                cfg.metric_colors[0].label,
                cfg.metric_colors[0].value,
                cfg.metric_colors[2].label,
                cfg.font_weight,
                if cfg.render_engine == RenderEngine::D2d { "d2d" } else { "gdi" },
                cfg.size_locked,
            )
        });
        let (nat_w, nat_h, cells) = renderer.layout(snap, cfg, width, height);

        // A-自愈：内容放不下窗口时按运行时自然尺寸扩宽 + 续帧（两引擎共用）。
        // ⚠️ 只扩不缩：窗口比内容大时保持（用户可能手动拉宽过；且与 D2D
        // self-heal 组合时双向收窄会造成"缩→扩→缩"死循环——实测 282↔298）。
        if ensure_content_fits(hwnd, cfg, nat_w, nat_h, width, height) {
            perf::note_section("render_d2d_fit", t0);
            return;
        }

        let sig = renderer.hash_from_cells(&cells, cfg, width, height, src_alpha);
        // 本帧已受理当前纪元（无论后续绘制还是指纹跳过，画面都已是最新）。
        HANDLED_EPOCH.store(RENDER_EPOCH.load(AtomicOrdering::Relaxed), AtomicOrdering::Relaxed);
        if LAST_FRAME_HASH.load(std::sync::atomic::Ordering::Relaxed) == sig {
            perf::note_skipped();
            // 动画未走完但帧被跳过（理论上只在末帧重复时发生）：继续驱动定时器
            if animating {
                let _ = SetTimer(hwnd, ANIM_TIMER_ID, 16, None);
            }
            return;
        }
        LAST_FRAME_HASH.store(sig, std::sync::atomic::Ordering::Relaxed);

        // 画布复用（P2）：尺寸不变时只清零，不再分配
        let surface = SURFACE.get_or_init(|| Mutex::new(SoftwareSurface::new(0, 0)));
        let Ok(mut surface) = surface.lock() else { return };
        surface.resize(width.max(1), height.max(1));

        // 脏区（A1）：alpha / cell 数 / cell 位置变化 → 整窗；否则内容变化的 cell
        // 按包围盒**连通分量**聚成若干脏矩形（空隙不再被捎带上传）。
        // changed 位图供 paint 跳过未变化 cell（标签不重画，v3 性能档）。
        // ⚠️ 尺寸相对上次送屏有变 → 画布与 DIB 都被清零重建，必须整窗重画+整窗上传，
        // 否则脏矩形上传会让非脏区域变全透明（实测：上半部分和左侧消失）。
        let (mut dirty, mut changed) = compute_dirty(&cells, src_alpha);
        // 设置面板触发的重绘：强制整窗（局部 ULW 在部分合成器上不生效 → 颜色/粗细
        // 「看起来没刷新」，实测证据见 force_full_refresh 注释）。
        if take_force_full() || present_size_changed(width, height) {
            dirty = Dirty::Full;
            changed = vec![true; cells.len()];
        }
        // 时钟秒变化 → 曾强制整窗（旧注释：Rect 局部上传 ULW 成功但不重混）。
        // ✅ 根因已修（2026-09-12）：那不是驱动/DWM 问题，是 upload_dib 里
        // pptSrc 坐标语义错误（2× 偏移），对照实验见 docs/悬浮窗性能恢复计划.md。
        // 脏矩形上传恢复信任，时钟秒走正常脏区路径（多矩形后只上传时间格本身）。
        let present_sec = aaxsyspulse_ui::clock::get().2;

        // 绘制（A3：仅首帧/alpha/布局变化才整画布清零，否则只清脏区 cell）
        renderer.paint(
            &mut surface,
            snap,
            cfg,
            &cells,
            dirty.is_full(),
            if dirty.is_full() { None } else { Some(&changed) },
        );

        // 跳过上屏的帧（Rects 空，无像素变化）返回 true：该秒的画面内容
        // 本就已在窗口上，快速跳过路径据此记账是正确的。
        if present(hwnd, &surface, width, height, src_alpha, &dirty) {
            LAST_PRESENTED_SEC.store(present_sec, AtomicOrdering::Relaxed);
        } else {
            // 上屏失败（GetDC/DIB 分配/ULW 失败）：回滚指纹，让下一帧重试，
            // 否则相同内容的帧会被指纹跳过 → 窗口永久冻结在旧帧。
            LAST_FRAME_HASH.store(u64::MAX, AtomicOrdering::Relaxed);
        }

        // 记录本帧实际送屏的 alpha 作为下一次过渡的起点
        RENDERED_PCT_X100.store((alpha_pct_f.clamp(1.0, 100.0) * 100.0) as u32, AtomicOrdering::Relaxed);

        // 动画未走完 → 定期器拉下一帧；走完 → 停表归零开销
        if animating {
            let _ = SetTimer(hwnd, ANIM_TIMER_ID, 16, None);
        } else {
            let _ = KillTimer(hwnd, ANIM_TIMER_ID);
        }

        // 窗口尺寸与内容自然尺寸漂移过大（项开关/风格切换后未 fit）时兜底校正
        if (nat_w - width).abs() > 8 || (nat_h - height).abs() > 8 {
            drop(surface);
            fit_window(hwnd, cfg);
        }
    }
    perf::end_render(t0);
}

/// 比对当前帧 cell 与上一帧，求出 `UpdateLayeredWindow` 的脏区（A1）+
/// 每 cell 内容变化位图（v3 性能档：paint 跳过未变化 cell）。
///
/// - alpha 变化：整窗（SourceConstantAlpha 重混全窗，子区上传会残留）。
/// - cell 数或某个 cell 位置漂移：整窗（旧位置像素需清掉，避免残影）。
/// - 否则：内容指纹变化的 cell 包围盒并集（只上传真正变了的子矩形）。
/// - 邻接传播：变化 cell 的包围盒（已含阴影/描边 pad）若与未变化 cell 相交，
///   后者也标记为变化——否则清 pad 时会擦掉邻居边缘像素且不重画（1-2px 残缺）。
fn compute_dirty(cells: &[CellSig], src_alpha: u8) -> (Dirty, Vec<bool>) {
    compute_dirty_in(&PREV_CELLS, &PREV_ALPHA, cells, src_alpha)
}

/// [`compute_dirty`] 的存储参数化版本：GDI 与高画质 D2D 路径各持独立的
/// prev-cell 状态（两引擎画出的像素不同，跳过/脏区判定互不共享）。
fn compute_dirty_in(
    prev_cells: &OnceLock<Mutex<Vec<CellSig>>>,
    prev_alpha: &AtomicU8,
    cells: &[CellSig],
    src_alpha: u8,
) -> (Dirty, Vec<bool>) {
    let n = cells.len();
    let all = || vec![true; n];
    let prev_alpha_v = prev_alpha.load(AtomicOrdering::Relaxed);
    let prev = prev_cells.get_or_init(|| Mutex::new(Vec::new()));
    let Ok(mut prev) = prev.lock() else {
        return (Dirty::Full, all());
    };
    // 上屏失败恢复：上一帧 ULW 失败 → 本帧整窗覆盖（配套帧指纹回滚，见该静态注释）。
    if NEEDS_FULL_REUPLOAD.swap(false, AtomicOrdering::Relaxed) {
        *prev = cells.to_vec();
        prev_alpha.store(src_alpha, AtomicOrdering::Relaxed);
        return (Dirty::Full, all());
    }
    if prev_alpha_v != src_alpha || prev.len() != n {
        *prev = cells.to_vec();
        prev_alpha.store(src_alpha, AtomicOrdering::Relaxed);
        return (Dirty::Full, all());
    }

    // 第一遍：找出**内容变化**格（hash 不同）。上传矩形只由这些格构成。
    let mut content_changed = vec![false; n];
    let mut force_full = false;
    for (i, (a, b)) in cells.iter().zip(prev.iter()).enumerate() {
        if a.x != b.x || a.y != b.y {
            // 布局漂移：旧位置像素必须随整窗上传清掉
            force_full = true;
            break;
        }
        if a.hash != b.hash {
            content_changed[i] = true;
        }
    }
    *prev = cells.to_vec();
    prev_alpha.store(src_alpha, AtomicOrdering::Relaxed);
    if force_full {
        return (Dirty::Full, all());
    }

    // 第二遍：邻接传播（语义与旧版一致，服务 GDI paint 跳格重画）：
    // 变化格包围盒（含阴影/描边外扩）相交的邻居也要重画——变化格 clear_rect
    // 会擦掉邻居墨迹。传播只扩 `changed`，**不扩上传矩形**：邻居内容没变，
    // 重画出的像素与上一帧相同，无需上传（两引擎的字体渲染都是确定性的）。
    let mut changed = content_changed.clone();
    loop {
        let mut grew = false;
        for i in 0..n {
            if !changed[i] {
                continue;
            }
            for j in 0..n {
                if changed[j] || i == j {
                    continue;
                }
                if cells_intersect(&cells[i], &cells[j]) {
                    changed[j] = true;
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }

    // 连通分量：把**内容变化**格按包围盒相交聚成簇，每簇一个 bbox 并集矩形。
    // 与旧"单一并集"的本质区别：两簇之间的空隙不再被捎带上传。
    // （不同簇的矩形可能相交——重叠上传无害：拷的是同一份 DIB 内容，无需去重。）
    let rects = dirty_components(cells, &content_changed);
    if rects.len() > MAX_DIRTY_RECTS {
        return (Dirty::Full, all());
    }
    (Dirty::Rects(rects), changed)
}

/// cell 包围盒（已含阴影/描边外扩）相交测试。
fn cells_intersect(a: &CellSig, b: &CellSig) -> bool {
    a.bbox_x < b.bbox_x + b.w && b.bbox_x < a.bbox_x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/// 把内容变化格按包围盒相交关系聚成连通分量，返回每簇的 bbox 并集矩形。
/// 并查集实现；cell ≤ 10，O(n²) 可忽略。
fn dirty_components(cells: &[CellSig], changed: &[bool]) -> Vec<RECT> {
    let n = cells.len();
    if n == 0 {
        return Vec::new();
    }
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for i in 0..n {
        if !changed[i] {
            continue;
        }
        for j in (i + 1)..n {
            if changed[j] && cells_intersect(&cells[i], &cells[j]) {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }
    // 归组：root → bbox 并集。包围盒用 bbox_x（右对齐格墨迹左溢，见文件头第 4 条）。
    let mut acc: Vec<(usize, RECT)> = Vec::new();
    for i in 0..n {
        if !changed[i] {
            continue;
        }
        let r = RECT {
            left: cells[i].bbox_x,
            top: cells[i].y,
            right: cells[i].bbox_x + cells[i].w,
            bottom: cells[i].y + cells[i].h,
        };
        let root = find(&mut parent, i);
        match acc.iter_mut().find(|(root0, _)| *root0 == root) {
            Some((_, rect)) => {
                rect.left = rect.left.min(r.left);
                rect.top = rect.top.min(r.top);
                rect.right = rect.right.max(r.right);
                rect.bottom = rect.bottom.max(r.bottom);
            }
            None => acc.push((root, r)),
        }
    }
    acc.into_iter().map(|(_, rect)| rect).collect()
}

/// 逐帧复用的离屏画布（P2）。
///
/// 只在 UI 线程访问；用 `Mutex` 只是为了满足 Rust 的 `Sync` 要求
/// （不可免的静态），实际无竞争。
static SURFACE: OnceLock<Mutex<SoftwareSurface>> = OnceLock::new();

/// DIB 缓存（P2）：位图/内存 DC 按尺寸复用。
///
/// **为什么重要**：`CreateDIBSection` 每次都要在内核里分配 187 KB 并清零，
/// 再 `DeleteObject` 销毁 —— 每帧一次，纯浪费。这里只在尺寸真的变化时重建。
#[derive(Default)]
struct DibCache {
    w: i32,
    h: i32,
    dc: isize,
    bmp: isize,
    old_bmp: isize,
    bits: isize,
}

static DIB: OnceLock<Mutex<DibCache>> = OnceLock::new();

/// 释放 DIB 缓存（窗口销毁 / DPI 变化时调用）。
pub fn invalidate_dib_cache() {
    // DIB 已销毁：下次 present 面对的是全新清零 DIB，必须整窗上传
    invalidate_present_size();
    if let Some(c) = DIB.get() {
        if let Ok(mut c) = c.lock() {
            unsafe { c.free() };
            *c = DibCache::default();
        }
    }
}

// ── 命中测试 ────────────────────────────────────────────────────────────
//
// 曾经这里有 `sample_alpha()` 供 `WM_NCHITTEST` 逐像素返回 HTTRANSPARENT。
// 该做法已废弃并删除：
//   * `HTTRANSPARENT` 只对**同一线程**的窗口有效（MSDN 原文），悬浮窗下方的
//     窗口属于别的进程 → 窗口管理器找不到下一个接收者，点击被直接吃掉，
//     表现为「透明区仍然遮挡点击」。
//   * 正确机制是分层窗口自带的按像素 alpha 命中测试：DIB 里 alpha **恰好为 0**
//     的像素不参与命中，鼠标事件自动落到下方窗口。因此渲染端必须把背景清成
//     全 0（SoftwareSurface::clear / D2DSurface::clear），且 WM_NCHITTEST 一律
//     交给 DefWindowProc（客户区 → HTCLIENT）。
// 结论：这里不再需要任何采样代码，改动背景 alpha 时请一并看这两处 clear。

impl DibCache {
    unsafe fn free(&mut self) {
        if self.old_bmp != 0 && self.dc != 0 {
            let _ = SelectObject(HDC(self.dc as *mut _), windows::Win32::Graphics::Gdi::HGDIOBJ(self.old_bmp as *mut _));
        }
        if self.bmp != 0 {
            let _ = DeleteObject(HBITMAP(self.bmp as *mut _));
        }
        if self.dc != 0 {
            let _ = DeleteDC(HDC(self.dc as *mut _));
        }
        self.dc = 0;
        self.bmp = 0;
        self.old_bmp = 0;
        self.bits = 0;
        self.w = 0;
        self.h = 0;
    }

    /// 取（新建或复用）一个 `w × h` 的 32 位 DIB，返回 (内存 DC, 像素指针)。
    unsafe fn get(&mut self, screen_dc: HDC, w: i32, h: i32) -> Option<(HDC, *mut u8)> {
        if self.w == w && self.h == h && self.dc != 0 && self.bmp != 0 && self.bits != 0 {
            return Some((HDC(self.dc as *mut _), self.bits as *mut u8));
        }
        self.free();

        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.is_invalid() {
            return None;
        }
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize =
            std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h; // top-down DIB
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bitmap = match CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(_) => {
                let _ = DeleteDC(mem_dc);
                return None;
            }
        };
        if bits.is_null() {
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(mem_dc);
            return None;
        }

        let old = SelectObject(mem_dc, bitmap);
        self.w = w;
        self.h = h;
        self.dc = mem_dc.0 as isize;
        self.bmp = bitmap.0 as isize;
        self.old_bmp = old.0 as isize;
        self.bits = bits as isize;
        Some((mem_dc, bits as *mut u8))
    }
}

/// 把 SoftwareSurface 上屏（DIB + UpdateLayeredWindow / UpdateLayeredWindowIndirect）。
///
/// `src_alpha` 为全局恒定 alpha（透明度 × 悬停系数，0..=255）。
/// `dirty` 决定上屏方式（A1/A2）：
/// - `Dirty::Full` 或 `AAXSYSPULSE_NODIRTY=1` → 整窗拷贝 + 经典 `UpdateLayeredWindow`。
/// - `Dirty::Rects(rects)` → 逐矩形拷进 DIB + 逐矩形 ULWIndirect；空列表 = 无像素
///   变化，跳过上屏（DIB 与窗口内容本就一致）。
/// 返回 false = 上屏失败（调用方回滚帧指纹；NEEDS_FULL_REUPLOAD 保证下帧整窗恢复）。
#[allow(clippy::too_many_arguments)]
fn present(
    hwnd: HWND,
    surface: &SoftwareSurface,
    width: i32,
    height: i32,
    src_alpha: u8,
    dirty: &Dirty,
) -> bool {
    unsafe {
        let screen_dc = GetDC(hwnd);
        if screen_dc.is_invalid() {
            return false;
        }

        let dib = DIB.get_or_init(|| Mutex::new(DibCache::default()));
        let Ok(mut dib) = dib.lock() else {
            let _ = ReleaseDC(hwnd, screen_dc);
            return false;
        };
        let Some((mem_dc, bits)) = dib.get(screen_dc, width, height) else {
            drop(dib);
            let _ = ReleaseDC(hwnd, screen_dc);
            return false;
        };

        // pptDst 传 NULL：保持窗口当前位置。
        // 若传 Some((0,0))，UpdateLayeredWindow 会把窗口挪到该坐标——
        // 这正是旧版"一拖动就弹回原位"的元凶（每帧渲染都重置位置）。
        let sz = SIZE { cx: width, cy: height };
        let bf = BLENDFUNCTION {
            BlendOp: windows::Win32::Graphics::Gdi::AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: src_alpha,
            AlphaFormat: windows::Win32::Graphics::Gdi::AC_SRC_ALPHA as u8,
        };

        let ok = if *DIRTY_DISABLED || matches!(dirty, Dirty::Full) {
            // A2 整窗拷贝（历史：187KB 中间拷贝在 Full 帧不可避免）
            let rect = RECT { left: 0, top: 0, right: width, bottom: height };
            copy_rect_to_dib(bits, surface, &rect, width, height);
            let src_pt = POINT::default();
            UpdateLayeredWindow(
                hwnd,
                screen_dc,
                None,
                Some(&sz),
                mem_dc,
                Some(&src_pt),
                COLORREF(0),
                Some(&bf as *const BLENDFUNCTION),
                windows::Win32::UI::WindowsAndMessaging::ULW_ALPHA,
            )
            .is_ok()
        } else if let Dirty::Rects(rects) = dirty {
            if rects.is_empty() {
                true // 无像素变化：跳过上屏（DIB 与窗口显示一致）
            } else {
                // A2：先把每个脏矩形从画布拷进 DIB（DIB 其余区域保留旧帧，
                // 与窗口显示一致），再逐矩形 ULWIndirect。拷贝全部先行，
                // 与上传解耦：任一矩形上传失败不影响 DIB 的完整性。
                for r in rects {
                    copy_rect_to_dib(bits, surface, r, width, height);
                }
                let mut all_ok = true;
                for r in rects {
                    if !ulw_indirect_rect(hwnd, screen_dc, mem_dc, width, height, &bf, r) {
                        all_ok = false;
                        break; // 失败即止：闸门保证下一帧整窗恢复
                    }
                }
                all_ok
            }
        } else {
            false
        };
        if !ok {
            NEEDS_FULL_REUPLOAD.store(true, AtomicOrdering::Relaxed);
            crate::probe::log("present: UpdateLayeredWindow failed");
        }

        // P2：不删除 mem_dc / bitmap —— 它们留在 DIB 缓存里逐帧复用。
        let _ = ReleaseDC(hwnd, screen_dc);
        ok
    }
}

/// 对单个脏矩形执行 `UpdateLayeredWindowIndirect`。
///
/// ✅ 根因修复（2026-09-12 对照实验钉死，见 docs/悬浮窗性能恢复计划.md）：
/// `pptSrc` 语义 = 「窗口左上角在 hdcSrc 中的对齐点」。整窗 DIB 的正确值是 (0,0)。
/// 旧代码写 (r.left, r.top)，与窗口坐标的 prcDirty 叠加后源像素被 2× 偏移——
/// API 返回成功但贴错像素，即历史「Rect ok=true 而画面不更新」的根因。
/// **本函数是全项目唯一 ULWIndirect 出口**（文件头适用边界第 3 条：
/// 任何新增上传路径必须照抄本函数，禁止另写）。
#[allow(clippy::too_many_arguments)]
unsafe fn ulw_indirect_rect(
    hwnd: HWND,
    screen_dc: HDC,
    mem_dc: HDC,
    width: i32,
    height: i32,
    bf: &BLENDFUNCTION,
    r: &RECT,
) -> bool {
    let ppsrc = POINT { x: 0, y: 0 };
    // prcDirty 非空时 psize/pptDst 被忽略；psize 给整窗尺寸是规范姿势。
    let psize = SIZE { cx: width, cy: height };
    // 与 copy_rect_to_dib 同一套钳制：cell 包围盒（含右对齐槽位/描边外扩）
    // 可能略微越出窗口，越界的 prcDirty 交给 ULWIndirect 只会失败。
    // 钳成空 → 无需上传，返回 true（本矩形没有可见像素在上屏范围内）。
    let prc = RECT {
        left: r.left.max(0),
        top: r.top.max(0),
        right: r.right.min(width),
        bottom: r.bottom.min(height),
    };
    if prc.right <= prc.left || prc.bottom <= prc.top {
        return true;
    }
    let info = UPDATELAYEREDWINDOWINFO {
        cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
        hdcDst: screen_dc,
        pptDst: std::ptr::null(),
        psize: &psize as *const SIZE,
        hdcSrc: mem_dc,
        pptSrc: &ppsrc as *const POINT,
        crKey: COLORREF(0),
        pblend: bf as *const BLENDFUNCTION,
        dwFlags: windows::Win32::UI::WindowsAndMessaging::ULW_ALPHA,
        prcDirty: &prc as *const RECT,
    };
    UpdateLayeredWindowIndirect(hwnd, &info).as_bool()
}

/// 把已绘制好的 DIB（mem_dc）上传到分层窗口（高画质 D2D 路径用）。
///
/// D2D 直接写进 DIB 的预乘 BGRA 比特，所以无需 `copy_rect_to_dib`——
/// 只需把 DIB 上屏。`dirty` 决定上屏方式（与 GDI 路径同一套 A1/A2 语义）：
/// - `Dirty::Full` 或 `AAXSYSPULSE_NODIRTY=1` → 经典 `UpdateLayeredWindow`。
/// - `Dirty::Rects(rects)` → 逐矩形 ULWIndirect；空列表 = 跳过上屏。
/// 返回 false = 上屏失败（调用方应回滚帧指纹，否则相同内容帧会被跳过 → 冻结）。
unsafe fn upload_dib(hwnd: HWND, width: i32, height: i32, src_alpha: u8, dirty: &Dirty) -> bool {
    let screen_dc = GetDC(hwnd);
    if screen_dc.is_invalid() {
        return false;
    }
    let dib = DIB.get_or_init(|| Mutex::new(DibCache::default()));
    let Ok(mut dib) = dib.lock() else {
        let _ = ReleaseDC(hwnd, screen_dc);
        return false;
    };
    let Some((mem_dc, _bits)) = dib.get(screen_dc, width, height) else {
        drop(dib);
        let _ = ReleaseDC(hwnd, screen_dc);
        return false;
    };
    let bf = BLENDFUNCTION {
        BlendOp: windows::Win32::Graphics::Gdi::AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: src_alpha,
        AlphaFormat: windows::Win32::Graphics::Gdi::AC_SRC_ALPHA as u8,
    };
    // 卡死探针（2026-09-12 13:40 事故）：渲染线程最后日志停在 PAINT-DONE，
    // 唯一剩余阻塞点在 upload_dib 内部（GetDC / DIB.lock / ULW）。
    // ULW 前后各打一条，下次复现即可把挂起点钉死到具体 API。
    crate::probe::log_lazy(|| format!("upload-begin {width}x{height} dirty={}", fmt_dirty(dirty)));
    let ok = if *DIRTY_DISABLED || matches!(dirty, Dirty::Full) {
        let sz = SIZE { cx: width, cy: height };
        let src_pt = POINT::default();
        UpdateLayeredWindow(
            hwnd,
            screen_dc,
            None,
            Some(&sz),
            mem_dc,
            Some(&src_pt),
            COLORREF(0),
            Some(&bf as *const BLENDFUNCTION),
            windows::Win32::UI::WindowsAndMessaging::ULW_ALPHA,
        )
        .is_ok()
    } else if let Dirty::Rects(rects) = dirty {
        if rects.is_empty() {
            true // 无像素变化：跳过上屏（DIB 与窗口显示一致）
        } else {
            // 逐矩形 ULWIndirect：pptSrc=(0,0) / pptDst=NULL 语义由
            // ulw_indirect_rect 唯一出口保证（见该函数注释）。
            let mut all_ok = true;
            for r in rects {
                if !ulw_indirect_rect(hwnd, screen_dc, mem_dc, width, height, &bf, r) {
                    all_ok = false;
                    break; // 失败即止：闸门保证下一帧整窗恢复
                }
            }
            all_ok
        }
    } else {
        false
    };
    if !ok {
        NEEDS_FULL_REUPLOAD.store(true, AtomicOrdering::Relaxed);
        crate::probe::log("upload_dib: UpdateLayeredWindow failed");
    }
    crate::probe::log_lazy(|| format!("upload-end ok={ok}"));
    let _ = ReleaseDC(hwnd, screen_dc);
    ok
}

/// 高画质模式整帧渲染：复用引擎无关的 `layout` 取 cell，用 `D2DSurface`（DWrite 画字）
/// 绑到 DIB 内存 DC 并行绘制，再整窗上传。性能模式路径完全不进入本函数。
#[allow(clippy::too_many_arguments)]
fn render_d2d(
    hwnd: HWND,
    snap: &Snapshot,
    cfg: &Config,
    renderer: &'static dyn Renderer,
    width: i32,
    height: i32,
    src_alpha: u8,
    alpha_pct_f: f32,
    animating: bool,
    t0: u64,
) -> bool {
    unsafe {
        crate::probe::log_lazy(|| "d2d enter".to_string());
        // 布局（与性能模式同一套 cell 几何/文字）→ 门面指纹；
        // 异或 ENGINE_HASH_XOR 让引擎切换强制整窗重绘（同内容不同像素不能共享跳过）。
        // P0-0.2 四段分解：seg_layout / seg_hash / seg_draw / seg_upload。
        let t_layout = perf::begin();
        let (nat_w, nat_h, cells) = renderer.layout(snap, cfg, width, height);
        perf::note_section("seg_layout", t_layout);
        // 诊断探针（D2D 侧）：悬浮窗渲染实际读到的指标颜色。
        crate::probe::log_lazy(|| {
            format!(
                "d2d render cfg: cpu_label={:?} cpu_value={:?} gpu_label={:?} fw={}",
                cfg.metric_colors[0].label,
                cfg.metric_colors[0].value,
                cfg.metric_colors[2].label,
                cfg.font_weight,
            )
        });

        // A-自愈（D2D 前置版）：内容放不下窗口 → 扩宽 + 续帧 + 本帧不画。
        // ⚠️ 只扩不缩（与 GDI 分支共用 ensure_content_fits），避免与下方
        // self-heal 互相拉锯死循环（实测 282↔298 每帧抖动、418 被压回 282）。
        if ensure_content_fits(hwnd, cfg, nat_w, nat_h, width, height) {
            return true; // 扩窗续帧（非失败）
        }

        let t_hash = perf::begin();
        // 指纹直接复用上方 layout 产出的 cells（与 GDI 分支 hash_from_cells 同一姿势）。
        // ⚠️ 勿改回 `renderer.frame_hash(...)`：trait 默认实现内部会再跑一遍完整
        // layout —— 双倍 build_rows，且把第二次 layout 藏进 seg_hash 时段，
        // P0 四段分解（seg_layout / seg_hash）的读数会失真。
        let base = renderer.hash_from_cells(&cells, cfg, width, height, src_alpha);
        // 本帧已受理当前纪元（见 GDI 分支同名注释）。
        HANDLED_EPOCH.store(RENDER_EPOCH.load(AtomicOrdering::Relaxed), AtomicOrdering::Relaxed);
        let sig = base ^ ENGINE_HASH_XOR;
        if LAST_FRAME_HASH.load(std::sync::atomic::Ordering::Relaxed) == sig {
            perf::note_skipped();
            perf::note_section("render_d2d_skip", t0);
            crate::probe::log_lazy(|| format!("d2d HASH-SKIP sig={sig:x}"));
            if animating {
                let _ = SetTimer(hwnd, ANIM_TIMER_ID, 16, None);
            }
            return true; // 内容未变（非失败）
        }
        LAST_FRAME_HASH.store(sig, std::sync::atomic::Ordering::Relaxed);

        // 脏区（独立 prev 状态）：alpha 变化/布局漂移/尺寸变化 → 整窗；否则变化 cell 并集。
        // 尺寸检查放在跳过检查之后：跳过帧不得消费「尺寸变化」标记。
        let (mut dirty, _changed) = compute_dirty_in(&PREV_CELLS_D2D, &PREV_ALPHA_D2D, &cells, src_alpha);
        // 设置面板触发的重绘：强制整窗（局部 ULW 在部分合成器上不生效 → 颜色/粗细
        // 「看起来没刷新」，实测证据见 force_full_refresh 注释）。
        if take_force_full() || present_size_changed(width, height) {
            dirty = Dirty::Full;
        }
        // 时钟秒变化 → 曾强制整窗（根因同 GDI 分支注释：pptSrc 坐标语义错误
        // 已修复，Rect 局部上传恢复信任，见 docs/悬浮窗性能恢复计划.md）。
        let present_sec = aaxsyspulse_ui::clock::get().2;
        perf::note_section("seg_hash", t_hash);
        crate::probe::log_lazy(|| {
            format!(
                "d2d render {}x{} cells={} dirty={} alpha={}",
                width, height, cells.len(), fmt_dirty(&dirty), src_alpha
            )
        });

        let screen_dc = GetDC(hwnd);
        if screen_dc.is_invalid() {
            return false;
        }
        let dib = DIB.get_or_init(|| Mutex::new(DibCache::default()));
        let Ok(mut dib) = dib.lock() else {
            let _ = ReleaseDC(hwnd, screen_dc);
            return false;
        };
        let Some((mem_dc, _bits)) = dib.get(screen_dc, width, height) else {
            drop(dib);
            let _ = ReleaseDC(hwnd, screen_dc);
            return false;
        };

        // 绑 D2D 到 DIB 内存 DC，整帧重绘（上屏按脏区子矩形，DWrite 确定性渲染
        // 保证同输入同像素，未上传区域与窗口显示一致）。
        let mut surface = match D2DSurface::bind(mem_dc, width, height) {
            Some(s) => s,
            None => {
                crate::probe::log("d2d bind FAILED (no RT) — gdi fallback this frame");
                drop(dib);
                let _ = ReleaseDC(hwnd, screen_dc);
                LAST_FRAME_HASH.store(u64::MAX, AtomicOrdering::Relaxed);
                return false; // 失败：调用方走 GDI 分支兜底
            }
        };
        let t_draw = perf::begin();
        surface.begin_draw();
        surface.clear();
        paint_cells(&mut surface, &cells, cfg.font_weight);
        // 自愈扩窗（D2D 右缘裁切根治）：绘制完成后用 DWrite 实测最右文本右缘。
        // 布局按位图度量推算 cell，DIN 真字形更宽 → 右缘可能超出窗口被裁。
        // 若超出，立即按实测右缘扩宽窗口并续帧重画，直到收敛（无需估算余量）。
        let text_right = surface.max_text_right;
        if !surface.end_draw() {
            // 设备丢失/状态错误：本帧跳过上屏并回滚指纹，改走 GDI 分支兜底；
            // 下帧 bind 会重建 RT 重试 D2D。
            drop(dib);
            let _ = ReleaseDC(hwnd, screen_dc);
            LAST_FRAME_HASH.store(u64::MAX, AtomicOrdering::Relaxed);
            return false;
        }
        perf::note_section("seg_draw", t_draw);
        // seg_draw 内部的细分段（fmt/brush/layout/text）由 render_d2d 在 cell 循环里
        // 自累加，在此一次性落盘（避免上万条段样本撑爆缓冲）。
        crate::render_d2d::flush_seg_acc();
        let need_w = (text_right + 12.0).ceil() as i32;
        // 收缩目标取 max(DWrite 实测, 布局自然宽)：扩（guard 用 nat_w）与缩
        // （此处用 need_w）的目标若互相错开 8~32px 会形成扩缩循环，取 max
        // 保证收缩终点 ≥ guard 的扩窗终点 → 单向收敛。
        let fit_w = need_w.max(nat_w);
        // 仅当确实超出且未达上限才扩；超过上限（内容本身超宽）放弃扩避免死循环。
        if !cfg.size_locked && need_w > width && need_w <= MAX_WIN {
            crate::probe::log(&format!(
                "d2d self-heal width: text_right={text_right:.1} need={need_w} win={width} -> widen + retry"
            ));
            drop(dib);
            let _ = ReleaseDC(hwnd, screen_dc);
            LAST_FRAME_HASH.store(u64::MAX, AtomicOrdering::Relaxed);
            fit_window_to(hwnd, cfg, need_w, height);
            request_render(hwnd);
            perf::note_section("render_d2d_heal", t0);
            return true; // 扩窗续帧（非失败）
        }
        // 收缩回精确内容宽度（滞后带 32px 防抖动）。此前"只扩不缩"叠加
        // fit_window_to 内部的 ×1.4 复利，把窗口一路顶到 MAX_WIN(2048)——
        // 每秒一次的时钟整窗上传白白多拷近 10 倍内存。size_locked 尊重用户锁定。
        if !cfg.size_locked && width > fit_w + 32 {
            crate::probe::log(&format!(
                "d2d self-heal shrink: fit_w={fit_w} win={width} -> shrink + retry"
            ));
            drop(dib);
            let _ = ReleaseDC(hwnd, screen_dc);
            LAST_FRAME_HASH.store(u64::MAX, AtomicOrdering::Relaxed);
            fit_window_to(hwnd, cfg, fit_w, height);
            request_render(hwnd);
            perf::note_section("render_d2d_shrink", t0);
            return true; // 缩窗续帧（非失败）
        }
        crate::probe::log_lazy(|| format!("d2d PAINT-DONE -> upload {}x{} dirty={}", width, height, fmt_dirty(&dirty)));

        // 释放 DIB 锁再上屏：upload_dib 内部会重新加锁取同一份 mem_dc。
        // std Mutex 不可重入——上一轮补 begin_draw 后 end_draw 成功、执行流抵达
        // upload_dib，若仍持有锁即同线程死锁（悬浮窗线程卡死 → 重启仍卡 → 设置打不开）。
        drop(dib);
        let t_upload = perf::begin();
        let uploaded = upload_dib(hwnd, width, height, src_alpha, &dirty);
        // 恢复计划 0.3 实验：脏区面积占比。区分两个假设——
        // ① pct≈100：脏区物理上≈整窗，prcDirty 无罪，接受现状；
        // ② pct 很小但耗时未降：ULWIndirect 的 prcDirty 分支有问题 → 真优化机会。
        // 恢复计划 0.3 实验：脏区面积占比。多矩形 = 各分量面积之和 / 整窗面积。
        let (dirty_pct, rect_n) = match &dirty {
            Dirty::Full => (100u64, 1usize),
            Dirty::Rects(rs) => {
                let area: u64 = rs
                    .iter()
                    .map(|r| ((r.right - r.left).max(0) as u64) * ((r.bottom - r.top).max(0) as u64))
                    .sum();
                let pct = (area * 100)
                    .checked_div((width.max(0) as u64) * (height.max(0) as u64))
                    .unwrap_or(100)
                    .min(100);
                (pct, rs.len())
            }
        };
        perf::note_section(&format!("seg_upload pct={dirty_pct} n={rect_n}"), t_upload);
        if !uploaded {
            LAST_FRAME_HASH.store(u64::MAX, AtomicOrdering::Relaxed);
        } else {
            LAST_PRESENTED_SEC.store(present_sec, AtomicOrdering::Relaxed);
        }
        // P0-0.1：D2D 是生产引擎，必须喂给 end_render——此前 perf.log 聚合
        // 只统计 GDI 兜底帧，监控数字长期失真（PERF_ANALYSIS 勘误 §6）。
        perf::end_render(t0);

        RENDERED_PCT_X100.store((alpha_pct_f.clamp(1.0, 100.0) * 100.0) as u32, AtomicOrdering::Relaxed);
        if animating {
            let _ = SetTimer(hwnd, ANIM_TIMER_ID, 16, None);
        } else {
            let _ = KillTimer(hwnd, ANIM_TIMER_ID);
        }
        let _ = ReleaseDC(hwnd, screen_dc);
        true
    }
}

/// 把画布脏区子矩形直拷进 DIB 比特（A2：跳过整窗 187KB 拷贝）。
///
/// DIB 与画布同尺寸、同为 top-down BGRA，所以目标偏移 = 源偏移。
unsafe fn copy_rect_to_dib(bits: *mut u8, surface: &SoftwareSurface, r: &RECT, width: i32, height: i32) {
    let x0 = r.left.max(0);
    let y0 = r.top.max(0);
    let x1 = r.right.min(width);
    let y1 = r.bottom.min(height);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    for yy in y0..y1 {
        let s_start = ((yy * width + x0) * 4) as usize;
        let s_end = ((yy * width + x1) * 4) as usize;
        if s_end > surface.pixels.len() {
            continue;
        }
        let src = &surface.pixels[s_start..s_end];
        let dst = std::slice::from_raw_parts_mut(bits.add(s_start), s_end - s_start);
        dst.copy_from_slice(src);
    }
}

/// 当前渲染器的自然尺寸（窗口自适应 / 启动定位用）。
pub fn natural_size(cfg: &Config) -> (i32, i32) {
    renderer_for(cfg.render_style).natural_size(cfg)
}

/// 让窗口自适应尺寸：**仅在尺寸锁定**（`size_locked`，设置页 LockSize）时沿用
/// 自定义尺寸，否则按内容自然尺寸（保持左上角位置）。
/// 设置项变化（项开关 / 风格 / 字号）后由 WM_RECONFIG 调用。
///
/// ⚠️ 2026-09-09 修复：此前直接用 `window_size`（它其实是"上次几何"，C 版
/// `size` 键每次启动都会填）→ 任何重启后尺寸被永久钉死，竖版/字号/间距全部
/// 失效。现在锁定语义由 `size_locked` 单独承担。
pub fn fit_window(hwnd: HWND, cfg: &Config) {
    let (nx, ny) = natural_size(cfg);
    // D2D 的自然宽度按位图字体度量估算、偏窄（DIN 真字形更宽）→ 估算路径
    // 给 D2D 加安全余量；GDI 度量与渲染同源（精确），不加不失真。
    // 渲染路径（guard/self-heal/收缩）一律用精确实测值，见 fit_window_to 注释。
    let nx = if cfg.render_engine == RenderEngine::D2d {
        natural_w_oversized(nx, cfg)
    } else {
        nx
    };
    fit_window_to(hwnd, cfg, nx, ny);
}

/// 把窗口设置为指定尺寸（`nx/ny` 传入**运行时自然尺寸**），保持左上角与层级。
///
/// 与 [`fit_window`] 的区别：后者用 placeholder 快照估算自然宽度（"0%" 等短样例），
/// 在字号/数值较长时**低估**真实内容宽度 → 窗口恒偏窄 → 右侧字体被裁且
/// fit guard 反复触发死循环。guard 路径改用运行时 `Renderer::layout` 实测的
/// (nat_w, nat_h)，一次设到位、内容与窗口完全匹配。
fn fit_window_to(hwnd: HWND, cfg: &Config, nx: i32, ny: i32) {
    // ⚠️ 此处**不得**再套 natural_w_oversized：nx/ny 就是最终目标尺寸。
    // 余量只允许由调用方从内容自然宽度计算一次（fit_window），若在这里对
    // "当前窗口宽度"再乘 1.4，self-heal 每触发一次就复利放大一次
    // （591→827→1158→1621→2048 触顶 MAX_WIN，实测窗口被顶到上限）。
    let (w, h) = if cfg.size_locked {
        cfg.window_size
            .filter(|&(w, h)| w >= 60 && h >= 40 && w <= MAX_WIN && h <= MAX_WIN)
            .unwrap_or((nx.max(60), ny.max(40)))
    } else {
        (nx.max(60), ny.max(40))
    };
    let w = w.clamp(60, MAX_WIN);
    let h = h.clamp(40, MAX_WIN);
    unsafe {
        let _ = SetWindowPos(hwnd, None, 0, 0, w, h, SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOMOVE);
        // 不调 InvalidateRect：调用方（fit guard / WM_RECONFIG）随后会续一帧渲染。
    }
}

/// 强制回到自然尺寸（忽略 size_locked）：用于检测到异常窗口尺寸（坏配置/历史
/// 无上限值）时兜底，避免超大 DIB 分配导致渲染卡死。
fn force_fit_natural(hwnd: HWND, cfg: &Config) {
    let (nx, ny) = natural_size(cfg);
    let w = nx.clamp(60, MAX_WIN);
    let h = ny.clamp(40, MAX_WIN);
    unsafe {
        let _ = SetWindowPos(hwnd, None, 0, 0, w, h, SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOMOVE);
    }
}

/// D2D 高画质引擎宽度补偿。
///
/// layout 的自然宽度按**位图字体度量**（8×16 等宽 cell）累加，而 D2D 引擎用 DIN
/// 真字形的 DWrite 渲染（字宽通常 > 位图度量，尤其 DIN-Black 偏宽）→ 自然宽度
/// 低估真实内容宽度 → 窗口恒窄、右侧文本（如双向 Net 的 "↓xx/s ↑xx/s" 后半段
/// 上传值）落在窗口外被"裁掉"，观感即"双向只显示一个数值"。给 D2D 下自然宽度
/// 加 +15% 安全余量；GDI 性能档度量同源（渲染即位图旁路），不加余量不失真。
fn natural_w_oversized(nat_w: i32, _cfg: &Config) -> i32 {
    // 网络双向模式 "↓xxx/s ↑xxx/s" 的渲染宽度被低估，
    // 导致 GDI/D2D 两侧上传值都被裁切。
    // 加 +40% 安全余量（D2D 因 DIN 字形偏宽原本已有此补偿，
    // GDI 同理，故统一应用）。
    let v = ((nat_w as f32) * 1.4).ceil() as i32;
    // 上限钳制：guard 比较与 fit 共用此值，超限会造成"扩→clamp→再扩"死循环。
    v.clamp(60, MAX_WIN)
}

/// 尺寸自愈（**GDI/D2D 两引擎共用**，杜绝双分支各自维护 guard 导致逻辑漂移）：
/// 内容放不下窗口时按运行时自然尺寸扩宽并续帧。返回 true = 调用方本帧应跳过。
///
/// ⚠️ 只扩不缩：窗口比内容大时保持（用户可能手动拉宽过；且与 D2D self-heal
/// 组合时，双向收窄会造成"缩→扩→缩"死循环——实测 282↔298 每帧抖动）。
fn ensure_content_fits(hwnd: HWND, cfg: &Config, nat_w: i32, nat_h: i32, width: i32, height: i32) -> bool {
    // 渲染期一律用**原始自然宽度**比较：D2D 的精确定宽由 render_d2d 的
    // DWrite 实测（text_right）self-heal 承担，GDI 的位图度量与渲染同源、
    // 本身精确。余量（natural_w_oversized）若参与渲染期比较，会与「收缩回
    // 精确尺寸」互相拉扯 → 扩缩死循环；余量只保留在 fit_window 估算路径。
    if !cfg.size_locked && (nat_w > width + 8 || nat_h > height + 8) {
        crate::probe::log(&format!(
            "fit guard(expand): nat=({nat_w},{nat_h}) win=({width},{height}) -> expand"
        ));
        fit_window_to(hwnd, cfg, nat_w.max(width), nat_h.max(height));
        request_render(hwnd);
        return true;
    }
    false
}
///
/// 背景：`render` 的脏检查在「帧指纹相同」时跳过整帧。字体粗细/颜色/阴影等设置
/// 只改变 D2D 的**像素外观**（字形档/描边/alpha），不改变 cell 几何/文本/颜色值
/// → 指纹不变 → 重绘被跳过 → 悬浮窗要等下一次采样内容变化（最长 2s）才更新，
/// 用户观感即"设置改了不立刻刷新"。WM_RECONFIG 前调用本函数强制失效。
pub fn invalidate_frame_hash() {
    LAST_FRAME_HASH.store(u64::MAX, AtomicOrdering::Relaxed);
    // 快速跳过路径同步失效：设置变更（指纹失效）必须立即重绘，
    // 不能被"秒未变 ∧ 纪元未变"的快速路径吞掉。
    RENDER_EPOCH.fetch_add(1, AtomicOrdering::Relaxed);
}

#[cfg(test)]
mod dirty_tests {
    use super::*;
    use aaxsyspulse_core::units::StrBuf;
    use aaxsyspulse_ui::Rgba;

    /// 最小可比对 CellSig：脏判定只看 x/y/bbox_x/w/h/hash。
    fn cell(x: i32, hash: u64) -> CellSig {
        CellSig {
            x,
            y: 8,
            w: 40,
            h: 28,
            bbox_x: x,
            right_align: false,
            hash,
            text: StrBuf::new(),
            color: Rgba::default(),
            shade: Rgba::default(),
            shadow: false,
            outline: false,
            scale: 1.0,
        }
    }

    fn fresh_state() -> (OnceLock<Mutex<Vec<CellSig>>>, AtomicU8) {
        (OnceLock::new(), AtomicU8::new(0))
    }

    fn run(
        state: &(OnceLock<Mutex<Vec<CellSig>>>, AtomicU8),
        cells: &[CellSig],
    ) -> (Dirty, Vec<bool>) {
        compute_dirty_in(&state.0, &state.1, cells, 255)
    }

    #[test]
    fn first_frame_is_full() {
        let st = fresh_state();
        let (d, changed) = run(&st, &[cell(0, 1), cell(100, 2)]);
        assert!(d.is_full());
        assert!(changed.iter().all(|&c| c), "首帧必须全部重画");
    }

    #[test]
    fn two_far_changed_cells_yield_two_rects() {
        let st = fresh_state();
        run(&st, &[cell(0, 1), cell(100, 2), cell(300, 3)]); // 建立基线
        // 第 0、2 格变化（相距远，中间隔第 1 格）→ 2 个矩形，空隙不上传
        let cells = [cell(0, 99), cell(100, 2), cell(300, 98)];
        let (d, changed) = run(&st, &cells);
        let Dirty::Rects(rs) = d else { panic!("应为 Rects，实际 {}", fmt_dirty(&d)) };
        assert_eq!(rs.len(), 2, "相距远的变化格不得合并");
        assert!(rs[0].right <= rs[1].left, "矩形之间必须隔开空隙");
        assert!(changed[0] && changed[2] && !changed[1], "邻接传播不得标记不相交邻居");
    }

    #[test]
    fn adjacent_changed_cells_merge_into_one_rect() {
        let st = fresh_state();
        // ⚠️ cells_intersect 用半开区间严格判定（right/bottom 边界不算相交）：
        // 「相触」（0..40 与 40..80）**不算**相交，必须真正重叠才并成一簇。
        run(&st, &[cell(0, 1), cell(30, 2)]); // 包围盒 0..40 与 30..70 重叠
        let cells = [cell(0, 99), cell(30, 98)];
        let (d, _) = run(&st, &cells);
        let Dirty::Rects(rs) = d else { panic!("应为 Rects") };
        assert_eq!(rs.len(), 1, "重叠的变化格必须并成一簇");
        assert_eq!((rs[0].left, rs[0].right), (0, 70));
    }

    #[test]
    fn unchanged_neighbor_is_not_uploaded() {
        // 传播邻居（changed=true）不应扩大上传矩形：上传只由内容变化格构成
        let st = fresh_state();
        // 两格重叠：0..40 与 20..60（真重叠才会触发邻接传播）；基线后只改第 0 格
        run(&st, &[cell(0, 1), cell(20, 2)]);
        let cells = [cell(0, 99), cell(20, 2)];
        let (d, changed) = run(&st, &cells);
        let Dirty::Rects(rs) = d else { panic!("应为 Rects") };
        assert_eq!(rs.len(), 1);
        assert_eq!((rs[0].left, rs[0].right), (0, 40), "未变化的邻居格不得进入上传矩形");
        assert!(changed[1], "但 paint 位图要标记邻居（clear_rect 擦除防线）");
    }

    #[test]
    fn no_change_yields_empty_rects_not_full() {
        let st = fresh_state();
        let cells = [cell(0, 1), cell(100, 2)];
        run(&st, &cells);
        let (d, _) = run(&st, &cells); // 同内容再来一帧
        let Dirty::Rects(rs) = d else { panic!("应为 Rects") };
        assert!(rs.is_empty(), "无变化必须是空矩形列表（跳过上屏）");
    }

    #[test]
    fn alpha_change_is_full() {
        let st = fresh_state();
        run(&st, &[cell(0, 1)]);
        let (d, _) = compute_dirty_in(&st.0, &st.1, &[cell(0, 1)], 128); // alpha 变了
        assert!(d.is_full());
    }

    #[test]
    fn position_drift_is_full() {
        let st = fresh_state();
        run(&st, &[cell(0, 1)]);
        let (d, _) = run(&st, &[cell(10, 1)]); // x 漂移
        assert!(d.is_full());
    }

    #[test]
    fn too_many_components_folds_to_full() {
        let st = fresh_state();
        let base: Vec<CellSig> = (0..10).map(|i| cell(i * 100, i as u64 + 1)).collect();
        run(&st, &base);
        let changed: Vec<CellSig> = (0..10).map(|i| cell(i * 100, i as u64 + 100)).collect();
        let (d, _) = run(&st, &changed);
        assert!(d.is_full(), "超过 MAX_DIRTY_RECTS(8) 必须折叠整窗");
    }

    #[test]
    fn upload_failure_gate_forces_full_next_frame() {
        let st = fresh_state();
        let cells = [cell(0, 1), cell(100, 2)];
        run(&st, &cells);
        run(&st, &cells);
        NEEDS_FULL_REUPLOAD.store(true, AtomicOrdering::Relaxed);
        let (d, changed) = run(&st, &cells);
        assert!(d.is_full(), "闸门置位后下一帧必须整窗");
        assert!(changed.iter().all(|&c| c));
        // 闸门一次性：消费后回到正常路径
        assert!(!NEEDS_FULL_REUPLOAD.load(AtomicOrdering::Relaxed));
    }
}
