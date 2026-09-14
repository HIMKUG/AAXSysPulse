//! 渲染线程（P1）：悬浮窗整条渲染管线挪出 UI 消息循环。
//!
//! # 解决什么问题
//!
//! 旧架构里 overlay 渲染（布局 → DWrite 绘制 → ULW 上屏）同步运行在 UI 线程的
//! 窗口过程里——与设置窗口共用一个消息循环。D2D/DWrite 的驱动级卡顿无法用
//! `catch_unwind` 兜住（那是 panic 专用），一旦发生，设置窗口等其余 UI 一起冻结。
//! 渲染搬进专职线程后，UI 线程只泵消息 + 投递请求，驱动毛刺最多延迟一帧上屏。
//!
//! # 线程模型
//!
//! - **UI 线程**：wndproc 把「hwnd + 最新 Config（`Copy`，顺手发布到镜像）」投进
//!   通道；`RENDER_QUEUED` 原子去重——突发多请求合并为一次渲染，队列恒长 ≤1。
//! - **渲染线程**：取请求 → 读配置镜像 + `Arc<Snapshot>`（arc_swap 共享，无锁读）
//!   → [`crate::gdi::render`]。`SetTimer` / `SetWindowPos` 跨线程调用均合法
//!   （定时器与几何归属窗口本身，WM_TIMER 仍投递回 UI 线程队列）。
//! - **D2D 工厂**：render_d2d 与设置页 gfx_d2d 各持独立 SINGLE_THREADED 工厂，
//!   无跨线程共享；DIB / D2D RT 自此只被渲染线程触碰，线程亲和性一致。
//!
//! # 适用边界（对抗性审查，详见 gdi.rs 模块头）
//!
//! 本线程的收益前提与 gdi.rs 的「指纹跳过 + 脏区上传」绑定（同一套架构）：
//! 内容空间/时间稀疏、`pptSrc=(0,0)` 语义、引擎切换强制整窗。
//! 渲染搬出 UI 线程只解决「驱动毛刺延迟一帧上屏」，**不**解决「渲染线程自身
//! 卡死在原生调用」——那是看门狗（`render_watchdog_tick`）的职责，且它只覆盖
//! 渲染线程；UI 线程消息循环被设置窗口饿死（冻结 1040 型）不在自愈范围。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;

use windows::Win32::Foundation::HWND;

use aaxsyspulse_core::config::Config;

static RENDER_TX: Mutex<Option<mpsc::Sender<isize>>> = Mutex::new(None);
static RENDER_QUEUED: AtomicBool = AtomicBool::new(false);
/// 最新配置镜像（Config 是 `Copy`）：UI 线程每次请求渲染时覆盖发布，
/// 渲染线程取走当帧快照——不存在部分字段撕裂。
static RENDER_CFG: Mutex<Option<Config>> = Mutex::new(None);

/// 启动渲染线程。**必须在窗口创建前调用**，否则首帧渲染请求无人消费。
/// 幂等：看门狗自愈（[`respawn`]）复用同一路径重建通道 + 线程。
pub fn start() {
    let (tx, rx) = mpsc::channel::<isize>();
    if let Ok(mut g) = RENDER_TX.lock() {
        *g = Some(tx);
    }
    RENDER_QUEUED.store(false, Ordering::Relaxed);
    // 开启新世代：让旧线程（可能仍卡在原生调用里）的迟到帧心跳失效，
    // 防止它清掉新线程的在途标记而使看门狗失明。
    crate::gdi::begin_render_generation();
    // 本线程捕获自己的世代号（此后不再变），随每次帧心跳回写。
    let my_gen = crate::gdi::render_generation();
    let _ = std::thread::Builder::new()
        .name("overlay-render".into())
        .spawn(move || {
            // 启动期预热（2026-09-14）：把 DWrite 首次字体解析的一次性成本
            // （实测首帧 seg_d2d_layout 317ms）提前到这里，与主线程随后的
            // 窗口创建/托盘/自启初始化**并行**，使窗口内容更早出现。
            // 配置镜像由 main 在 start() 前发布（publish_cfg）；未发布时
            // （watchdog respawn 路径）跳过——那时字体早已解析过，无需预热。
            crate::render_d2d::warmup_if_cfg();
            loop {
                let Ok(hwnd_addr) = rx.recv() else {
                    crate::probe::log("rt: channel closed -> exit");
                    return;
                };
                // 先清「已排队」标记再渲染：渲染期间到达的新请求会重新入队，
                // 不会被本次渲染吞掉。
                RENDER_QUEUED.store(false, Ordering::Relaxed);
                crate::probe::log_lazy(|| format!("rt wake hwnd={hwnd_addr}"));
                // panic 兜底：此前线程 panic 会静默死亡 → 悬浮窗永久冻结
                // （采样线程继续发请求但无人消费）。捕获后记日志继续跑。
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    render_once(hwnd_addr, my_gen);
                }));
                if outcome.is_err() {
                    crate::probe::log("rt: PANIC caught, thread survives");
                    // BUG-3：panic 可能把 prev-cells/帧指纹停在半新半旧——
                    // 下一帧哈希可能误判「内容未变」而跳过上传，画面从此
                    // 停在 panic 时刻。强制下一帧整窗重绘，把自愈做完整。
                    crate::gdi::force_full_refresh();
                }
            }
        });
}

/// 只发布配置镜像、不投递渲染请求。
///
/// 用途：`main` 在 `start()` 之前调用，让渲染线程一起来就能按正确的
/// 字重/字号做启动期预热（[`crate::render_d2d::warmup_if_cfg`]）——
/// 否则预热只能等到第一帧，与"提前"的目的相悖。
pub fn publish_cfg(cfg: &Config) {
    let mut slot = match RENDER_CFG.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    *slot = Some(*cfg);
}

/// 读配置镜像快照（中毒时取内层值，与 [`render_once`] 同语义）。未发布 → None。
pub(crate) fn cfg_snapshot() -> Option<Config> {
    match RENDER_CFG.lock() {
        Ok(g) => *g,
        Err(p) => *p.into_inner(),
    }
}

/// 单次渲染：读配置镜像 + 快照 → gdi::render。
/// RENDER_CFG 中毒（一次性 panic 污染）时 `into_inner` 恢复，
/// 不再 `continue` 静默跳过（那会让后续所有帧永久消失）。
fn render_once(hwnd_addr: isize, gen: u64) {
    let cfg = match RENDER_CFG.lock() {
        Ok(g) => *g,
        Err(p) => {
            crate::probe::log("rt: RENDER_CFG poisoned -> recovering inner value");
            *p.into_inner()
        }
    };
    let Some(cfg) = cfg else { return };
    let snap = crate::window::snap_get();
    let hwnd = HWND(hwnd_addr as *mut _);
    // 帧在途标记：watchdog 据此区分「线程死了」与「线程活着但卡在原生调用里」。
    // 携带本线程的世代号——旧线程（被 respawn 淘汰）的迟到写入会被 frame_end 忽略。
    // 包在 catch_unwind 外层——panic 被兜住后本函数仍会返回，frame_end 照常调用。
    crate::gdi::render_frame_begin(gen);
    crate::gdi::render(hwnd, &snap, &cfg);
    crate::gdi::render_frame_end(gen);
}

/// 看门狗自愈入口：渲染线程停摆（挂起/死亡）>30s 后重建通道与线程。
/// 旧线程若仍挂起则遗留（不再触碰新通道）；DIB/D2D RT 由新线程
/// 首次渲染时的尺寸检查自愈。由采样线程在
/// [`crate::gdi::render_watchdog_tick`] 返回 true 时调用。
pub fn respawn() {
    crate::probe::log("rt respawn: rebuilding render thread after stall");
    start();
}

/// UI 线程调用：发布最新配置 + 投递渲染请求（同帧多次请求合并为一次）。
/// 启动期（窗口创建前）也可只发布配置不请求渲染：见 [`publish_cfg`]。
pub fn request(hwnd: HWND, cfg: &Config) {
    // 中毒恢复与 render_once 对称：锁内只是 Copy 赋值，panic 源实际不存在，
    // 但一旦中毒若不 into_inner，配置镜像会永久冻结在旧值（快照照常更新、
    // 配置永不生效）。
    let mut cfg_slot = match RENDER_CFG.lock() {
        Ok(g) => g,
        Err(p) => {
            crate::probe::log("rt: RENDER_CFG poisoned in request -> recovering inner value");
            p.into_inner()
        }
    };
    *cfg_slot = Some(*cfg);
    // ⚠️ 先原子占位、后发送（store 在 send 之前）。旧实现 send 之后再
    // store(true)，渲染线程的 store(false) 可插进两者之间——stale true
    // 从此永久吞掉后续所有请求（配置镜像还在更新、epoch 还在 +1，但
    // 通道里再无消息）→ HANDLED_EPOCH 永不追平 → 30s 纪元冻结，只有
    // respawn 复位。2026-09-12 实测 8 次停摆中 5 次的根因（签名：
    // upload-end 后静默、无 rt wake、必须等 respawn）。
    if RENDER_QUEUED.swap(true, Ordering::Relaxed) {
        return; // 已有请求排队：配置镜像已更新，在途那次渲染即为最新
    }
    let tx = RENDER_TX.lock().ok().and_then(|g| g.clone());
    let send_ok = match tx {
        Some(tx) => tx.send(hwnd.0 as isize).is_ok(),
        None => false,
    };
    if !send_ok {
        // 通道不可用（未启动/正在 respawn）：交还排队权，下一个请求重试。
        RENDER_QUEUED.store(false, Ordering::Relaxed);
    }
}
