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
/// 续篇 661：加 #[cfg(windows)] —— 非 Windows 构建下 unused 会撞 workspace lints。
#[cfg(windows)]
const WM_RELOAD_CONFIG: u32 = aaxsyspulse_core::contract::WM_RELOAD_CONFIG;

/// 主程序自定义消息：快速重启（0x8000 + 9）。主程序收到后先同步落盘
/// 自己的内存配置，再拉起新实例并退出（主程序 restart.rs）。
#[cfg(windows)]
const WM_QUICK_RESTART: u32 = aaxsyspulse_core::contract::WM_QUICK_RESTART;

/// 尾随防抖窗口（毫秒）：这段时间内的多次变更合并成一次通知。
///
/// 来源（审查项 7.1.4）：这是**第二级**防抖。第一级在前端 ——
/// `src/wizard/composables/useGuide.js` 的 `DEBOUNCE.backup = 100`，合并的是
/// 用户连续改动引发的 `guide_backup` IPC。本常量合并的是"一次 flush 写多个
/// cfg 文件"引发的多次投递。50 的取值依据：主程序 `WM_RELOAD_CONFIG` 的处理
/// 是整份 reload + 重排（毫秒级），50ms 静默足够覆盖"写完 A 又写 B"的间隙，
/// 再小就退化成逐文件通知，再大则用户能察觉悬浮窗跟随延迟。
/// 两级叠加的最坏延迟 = 100 + 50 = **200ms**（首轮 017：旧注释漏算一级防抖写成
/// 150ms，已更正）。改这里请同步上面那段与 useGuide.js 的同源注释。
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
/// 主程序没在运行时投递失败，返回 `false`（续篇 650：调用方据此映射错误码，
/// 不再恒 Ok 谎报成功；返回 true 仅表示"已排队/投递成功"）。
pub fn notify_main_reload_config() -> bool {
    #[cfg(not(windows))]
    return false;

    #[cfg(windows)]
    {
        PENDING.store(true, Ordering::SeqCst);
        // 已有收尾线程在跑：它下一轮会看到 pending，无需再开线程。
        if FLUSHER_ARMED.swap(true, Ordering::SeqCst) {
            return true;
        }
        let spawned = std::thread::Builder::new()
            .name("config-notify-flusher".into())
            .spawn(|| loop {
                std::thread::sleep(Duration::from_millis(DEBOUNCE_MS));
                if PENDING.swap(false, Ordering::SeqCst) {
                    deliver(WM_RELOAD_CONFIG);
                }
                if !PENDING.load(Ordering::SeqCst) {
                    // 首轮 015：「读 PENDING → 清 ARMED」之间存在竞态 —— 此刻新
                    // 调用可能已置 PENDING=true 并因 ARMED 仍为 true 而跳过开线程，
                    // 标志会悬挂到下次调用。用 compare_exchange 收尾，收尾后再复核
                    // 一次；仍有 pending 就重新接管一轮，绝不悬挂。
                    if FLUSHER_ARMED
                        .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        if PENDING.load(Ordering::SeqCst) {
                            // 重新接管：CAS 抢回 armed；抢不到说明已有新线程在跑。
                            if FLUSHER_ARMED
                                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                                .is_ok()
                            {
                                continue;
                            }
                        }
                        break;
                    }
                    // CAS 失败（ARMED 已被别人动过）：继续兜底一轮
                }
            });
        spawned.is_ok()
    }
}

/// 通知主程序执行一次快速重启（引导「完成」时调用，让新配置全链路即时生效）。
/// 不受 reload 防抖影响；主程序未运行时返回 `false`。
pub fn notify_main_quick_restart() -> bool {
    #[cfg(not(windows))]
    return false;

    #[cfg(windows)]
    {
        // 重启前把挂着的 reload 通知补上，避免防抖窗口内的最后一次改动丢失。
        if PENDING.swap(false, Ordering::SeqCst) {
            // 续篇 660：投递失败把挂起的 reload 标记还原 —— 否则最后一次改动
            // 永不补发。PENDING 会被下一次 notify_main_reload_config 的 flusher 兜走。
            if !deliver(WM_RELOAD_CONFIG) {
                PENDING.store(true, Ordering::SeqCst);
            }
        }
        let ok = deliver(WM_QUICK_RESTART);
        if !ok {
            // 首轮 016：deliver 的返回值不再被忽略 —— 全部投递失败零留痕曾让
            // "完成引导后主程序没重启"无从排查。写一条安装日志。
            crate::install::log_install(
                "通知主程序重启",
                "失败",
                Some("WM_QUICK_RESTART 投递失败（主程序可能未运行或窗口正销毁）"),
            );
        }
        ok
    }
}

/// 把主程序的设置窗口带到前台（首轮 218 / 首轮 240 引导侧）：
/// 第二个引导实例启动时单实例锁已被占用 —— 旧实现直接 `exit(1)`，用户点图标
/// "没反应"。现在向主程序窗口投递 `WM_OPEN_SETTINGS`（契约常量，主程序
/// UIPI 白名单已放行）再退出。
pub fn notify_main_open_settings() -> bool {
    #[cfg(not(windows))]
    return false;

    #[cfg(windows)]
    {
        let ok = deliver(aaxsyspulse_core::contract::WM_OPEN_SETTINGS);
        if !ok {
            tracing::warn!("单实例已被占用，且 WM_OPEN_SETTINGS 投递失败（主程序可能未运行）");
        }
        ok
    }
}

/// 投递一条自定义消息给主程序窗口，失败时退避重试。
/// `PostMessageW` 的失败面很窄（窗口正在销毁 / 消息队列已满），
/// 而"引导刚改完配置、主程序正好在重启"正是最容易撞上的时刻。
#[cfg(windows)]
fn deliver(msg: u32) -> bool {
    // 契约名字在调用点转宽字符；缓冲必须活过 FindWindowW。
    let class_w = aaxsyspulse_core::contract::wide_of(MAIN_WINDOW_CLASS);
    // 首轮 014：FindWindowW 失败也纳入重试 —— 主程序重启、窗口尚未重建的瞬间
    // 恰好丢消息（旧实现查找失败直接 return false）。
    for attempt in 1..=3u64 {
        let hwnd = match unsafe { FindWindowW(PCWSTR(class_w.as_ptr()), PCWSTR::null()) } {
            Ok(hwnd) => hwnd,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(20 * attempt));
                continue;
            }
        };
        // 首轮 012：只按类名取第一个 HWND，不校验归属 PID —— 同名类/残留窗口会
        // 收错消息。用进程归属校验（映像名 == 主程序 exe）过滤。
        if !crate::process::hwnd_belongs_to_main_process(hwnd) {
            tracing::warn!("deliver({msg:#x}): FindWindowW 命中的窗口不属于主程序进程，重试");
            std::thread::sleep(Duration::from_millis(20 * attempt));
            continue;
        }
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
