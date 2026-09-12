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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;

use windows::Win32::Foundation::HWND;

use sysoverlay_core::config::Config;

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
    let _ = std::thread::Builder::new()
        .name("overlay-render".into())
        .spawn(move || {
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
                    render_once(hwnd_addr);
                }));
                if outcome.is_err() {
                    crate::probe::log("rt: PANIC caught, thread survives");
                }
            }
        });
}

/// 单次渲染：读配置镜像 + 快照 → gdi::render。
/// RENDER_CFG 中毒（一次性 panic 污染）时 `into_inner` 恢复，
/// 不再 `continue` 静默跳过（那会让后续所有帧永久消失）。
fn render_once(hwnd_addr: isize) {
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
    crate::gdi::render(hwnd, &snap, &cfg);
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
pub fn request(hwnd: HWND, cfg: &Config) {
    if let Ok(mut g) = RENDER_CFG.lock() {
        *g = Some(*cfg);
    }
    if RENDER_QUEUED.load(Ordering::Relaxed) {
        return; // 已有请求排队：配置镜像已更新，在途那次渲染即为最新
    }
    let tx = RENDER_TX.lock().ok().and_then(|g| g.clone());
    if let Some(tx) = tx {
        // 生产者只有 UI 线程：load→send→store 无竞争。
        if tx.send(hwnd.0 as isize).is_ok() {
            RENDER_QUEUED.store(true, Ordering::Relaxed);
        }
    }
}
