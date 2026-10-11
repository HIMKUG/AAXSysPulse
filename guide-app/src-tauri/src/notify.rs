//! 通知主程序重新加载配置模块
//! 引导程序写完配置文件后，调用 notify_main_reload_config() 通知主程序刷新
//!
//! 通信通道：`FindWindowW(主窗口类名)` + `PostMessageW(自定义消息)`。
//! 主程序侧对应 `sysoverlay/crates/win/src/window.rs` 的
//! `WM_RELOAD_CONFIG`(0x8008) / `WM_QUICK_RESTART`(0x8009) 分支，
//! 并在创建窗口时把这两个消息加进了 UIPI 过滤白名单（跨完整性可达）。

#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Foundation::{LPARAM, WPARAM};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};

/// 主程序窗口类名：契约常量（审查项 7.1.2 / P1-22），运行期转宽字符，
/// 本 crate 不再留第二份字面量副本。
#[cfg(windows)]
const MAIN_WINDOW_CLASS: &str = aaxsyspulse_core::contract::OVERLAY_WINDOW_CLASS;

/// 主程序自定义消息：重新加载配置（0x8000 + 8）。与主程序 `window.rs` 同一常量。
const WM_RELOAD_CONFIG: u32 = aaxsyspulse_core::contract::WM_RELOAD_CONFIG;

/// 主程序自定义消息：快速重启（0x8000 + 9）。主程序收到后先同步落盘
/// 自己的内存配置，再拉起新实例并退出（主程序 restart.rs）。
const WM_QUICK_RESTART: u32 = aaxsyspulse_core::contract::WM_QUICK_RESTART;

/// 尾随防抖窗口（毫秒）：这段时间内的多次变更合并成一次通知。
///
/// 来源（审查项 7.1.4）：这是**第二级**防抖。第一级在前端 ——
/// `src/wizard/composables/useGuide.js` 的 `DEBOUNCE.backup = 100`，合并的是
/// 用户连续改动引发的 `guide_backup` IPC。本常量合并的是"一次 flush 写多个
/// cfg 文件"引发的多次投递。50 的取值依据：主程序 `WM_RELOAD_CONFIG` 的处理
/// 是整份 reload + 重排（毫秒级），50ms 静默足够覆盖"写完 A 又写 B"的间隙，
/// 再小就退化成逐文件通知，再大则用户能察觉悬浮窗跟随延迟。
/// 两级叠加的最坏延迟 = 100 + 50 = 150ms。改这里请同步上面那段。
const DEBOUNCE_MS: u64 = 50;

/// 是否还有未发送的通知
#[cfg(windows)]
static PENDING: AtomicBool = AtomicBool::new(false);
/// 是否已有负责收尾的线程在跑（避免每次调用都开线程）
#[cfg(windows)]
static FLUSHER_ARMED: AtomicBool = AtomicBool::new(false);

/// 通知主程序重新读取配置文件并刷新悬浮窗。
///
/// **尾随**防抖：调用即标记 pending，由一个短命线程在 50ms 静默期后真正投递。
/// 旧实现是**头部**防抖（50ms 内的后续调用直接丢弃），于是"最后一次改动"
/// 恰恰可能不被通知 —— 用户看到的就是改了但悬浮窗没跟着变。
/// 主程序没在运行时静默返回。
pub fn notify_main_reload_config() {
    #[cfg(not(windows))]
    return;

    #[cfg(windows)]
    {
        PENDING.store(true, Ordering::SeqCst);
        // 已有收尾线程在跑：它下一轮会看到 pending，无需再开线程。
        if FLUSHER_ARMED.swap(true, Ordering::SeqCst) {
            return;
        }
        std::thread::spawn(|| loop {
            std::thread::sleep(Duration::from_millis(DEBOUNCE_MS));
            if PENDING.swap(false, Ordering::SeqCst) {
                deliver(WM_RELOAD_CONFIG);
            }
            if !PENDING.load(Ordering::SeqCst) {
                FLUSHER_ARMED.store(false, Ordering::SeqCst);
                break;
            }
        });
    }
}

/// 通知主程序执行一次快速重启（引导「完成」时调用，让新配置全链路即时生效）。
/// 不受 reload 防抖影响；主程序未运行时静默返回。
pub fn notify_main_quick_restart() {
    #[cfg(not(windows))]
    return;

    #[cfg(windows)]
    {
        // 重启前把挂着的 reload 通知补上，避免防抖窗口内的最后一次改动丢失。
        #[cfg(windows)]
        if PENDING.swap(false, Ordering::SeqCst) {
            deliver(WM_RELOAD_CONFIG);
        }
        deliver(WM_QUICK_RESTART);
    }
}

/// 投递一条自定义消息给主程序窗口，失败时退避重试。
/// `PostMessageW` 的失败面很窄（窗口正在销毁 / 消息队列已满），
/// 而"引导刚改完配置、主程序正好在重启"正是最容易撞上的时刻。
#[cfg(windows)]
fn deliver(msg: u32) -> bool {
    // 契约名字在调用点转宽字符；缓冲必须活过 FindWindowW。
    let class_w = aaxsyspulse_core::contract::wide_of(MAIN_WINDOW_CLASS);
    for attempt in 1..=3u64 {
        let hwnd = match unsafe { FindWindowW(PCWSTR(class_w.as_ptr()), PCWSTR::null()) } {
            Ok(hwnd) => hwnd,
            // 主程序没在运行：不是错误，静默返回。
            Err(_) => return false,
        };
        match unsafe { PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0)) } {
            Ok(()) => return true,
            Err(e) => {
                tracing::warn!("PostMessageW({msg:#x}) failed (attempt {attempt}): {e}");
                std::thread::sleep(Duration::from_millis(20 * attempt));
            }
        }
    }
    false
}
