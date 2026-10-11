//! 进程管理模块
//! 负责检测主程序进程、启动主程序、监控引导程序退出

use std::process::Command;

#[cfg(windows)]
use std::sync::atomic::Ordering;

/// 单实例互斥体句柄（存指针数值，0 = 未持有）。
#[cfg(windows)]
static GUIDE_MUTEX_HANDLE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 引导单实例互斥体名。
/// 刻意用会话局部命名空间（不带 `Global\`）：`Global\` 需要
/// `SeCreateGlobalPrivilege`，标准用户下 `CreateMutexW` 直接 `ACCESS_DENIED`，
/// 会让"是否已有实例"的判定彻底失真。引导不需要跨会话单实例。
#[cfg(windows)]
const GUIDE_MUTEX_NAME: &str = "Local\\AAXSysPulseGuide_SingleInstance";

/// 主程序悬浮窗窗口类名：契约常量（审查项 7.1.2 / P1-22），与 `notify.rs` 同源。
#[cfg(windows)]
const MAIN_WINDOW_CLASS: &str = aaxsyspulse_core::contract::OVERLAY_WINDOW_CLASS;

/// 抢占引导单实例锁。
///
/// 返回 `true` 表示本实例持有（可继续运行）；`false` 表示已有实例。
///
/// ⚠️ 关键：成功时**故意不 `CloseHandle`**，把句柄泄漏到进程退出为止。
/// Windows 命名互斥体是引用计数的 —— 旧实现在 `GetLastError()` 之后立即
/// `CloseHandle`，最后一个引用消失即销毁内核对象，于是第二个实例永远看不到
/// `ERROR_ALREADY_EXISTS`，单实例保护形同虚设（两个引导会并发改服务/写同一 INI）。
#[cfg(windows)]
pub fn acquire_guide_single_instance() -> bool {
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::PCWSTR;

    if GUIDE_MUTEX_HANDLE.load(Ordering::Relaxed) != 0 {
        return true; // 本进程已持有，重复调用不应自我否决
    }

    let name_wide: Vec<u16> = GUIDE_MUTEX_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        match CreateMutexW(None, false, PCWSTR::from_raw(name_wide.as_ptr())) {
            Ok(handle) => {
                let already = GetLastError() == ERROR_ALREADY_EXISTS;
                if already {
                    // 不是我们的对象，关闭我们这一份引用即可。
                    let _ = windows::Win32::Foundation::CloseHandle(handle);
                    false
                } else {
                    // 长期持有：进程退出（或显式 release）前不 CloseHandle。
                    GUIDE_MUTEX_HANDLE.store(handle.0 as usize, Ordering::Relaxed);
                    true
                }
            }
            Err(e) => {
                // 创建失败（极端环境）不应阻塞安装流程，但要留下痕迹。
                tracing::warn!("CreateMutexW for guide single-instance failed: {}", e);
                true
            }
        }
    }
}

/// 主动释放单实例锁。
///
/// 唯一必要场景是「以管理员身份重新启动」：`ShellExecuteW(runas)` 拉起的新实例
/// 与本进程处于**同一会话**（`Local\` 命名空间共享），若不先释放，新实例会在
/// 还没等到本进程 `exit(0)` 之前看到锁被占用 → 直接退出 → 用户点了提权却什么都没发生。
#[cfg(windows)]
pub fn release_guide_single_instance() {
    let raw = GUIDE_MUTEX_HANDLE.swap(0, Ordering::AcqRel);
    if raw != 0 {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(
                windows::Win32::Foundation::HANDLE(raw as *mut core::ffi::c_void),
            );
        }
    }
}

#[cfg(not(windows))]
pub fn release_guide_single_instance() {}

#[cfg(not(windows))]
pub fn acquire_guide_single_instance() -> bool {
    true
}

/// 主程序进程 PID：用悬浮窗窗口类名反查，找不到窗口即视为未运行。
#[cfg(windows)]
pub fn main_process_pid() -> Option<u32> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId};

    let class_wide: Vec<u16> = MAIN_WINDOW_CLASS
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let hwnd = unsafe { FindWindowW(PCWSTR::from_raw(class_wide.as_ptr()), PCWSTR::null()) }.ok()?;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32)) };
    if pid == 0 {
        None
    } else {
        Some(pid)
    }
}

#[cfg(not(windows))]
pub fn main_process_pid() -> Option<u32> {
    None
}

/// 主程序资源占用真值：`(生命周期 CPU 平均占用百分比, 工作集字节数)`。
///
/// 主程序未运行 / 句柄权限不足时返回 `None` —— 由调用方显示"未取到"，
/// 不再伪造 `0.03% / 1MB`（报告 P2-7）。
#[cfg(windows)]
pub fn main_process_usage() -> Option<(f64, u64)> {
    use windows::Win32::Foundation::{CloseHandle, FILETIME};
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// FILETIME 纪元（1601-01-01）与 UNIX 纪元的 100ns 差值
    const FILETIME_UNIX_EPOCH_DIFF: u64 = 116_444_736_000_000_000;

    let pid = main_process_pid()?;
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;

    let mut creation = FILETIME::default();
    let mut exit_t = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let times_ok =
        unsafe { windows::Win32::System::Threading::GetProcessTimes(
            handle,
            &mut creation,
            &mut exit_t,
            &mut kernel,
            &mut user,
        ) }
        .is_ok();

    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    let mem_ok = unsafe { GetProcessMemoryInfo(handle, &mut counters, counters.cb) }.is_ok();

    unsafe {
        let _ = CloseHandle(handle);
    }

    let mem_bytes = if mem_ok { counters.WorkingSetSize as u64 } else { 0 };

    if !times_ok {
        return if mem_bytes > 0 { Some((0.0, mem_bytes)) } else { None };
    }

    let to_u64 = |ft: &FILETIME| {
        ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
    };
    let busy = to_u64(&kernel) + to_u64(&user);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64 / 100 + FILETIME_UNIX_EPOCH_DIFF)
        .unwrap_or_else(|_| to_u64(&creation) + 1);
    let alive = now.saturating_sub(to_u64(&creation)).max(1);
    let cpu_avg_percent = (busy as f64 / alive as f64) * 100.0;

    Some((
        if cpu_avg_percent.is_finite() {
            (cpu_avg_percent * 10.0).round() / 10.0
        } else {
            0.0
        },
        mem_bytes,
    ))
}

#[cfg(not(windows))]
pub fn main_process_usage() -> Option<(f64, u64)> {
    None
}

/// 检查主程序是否在运行。
///
/// 旧实现枚举进程并用 `name.contains("AAXSysPulse")` 匹配模块名 ——
/// 引导自身（`AAXSysPulse-Guide`）同样包含该子串，于是**只要引导在跑就恒返回
/// true**，导致 `release_main_program` 的"主程序在运行则跳过"分支永远命中，
/// 主程序 exe 从未被释放到磁盘（表现为"安装完成但什么都没装上"）。
/// 现改为探测悬浮窗窗口类名（见 [`main_process_pid`]）：一次 API 调用，且不存在名字误匹配。
#[cfg(windows)]
pub fn is_main_process_running() -> bool {
    main_process_pid().is_some()
}

#[cfg(not(windows))]
pub fn is_main_process_running() -> bool {
    false
}

/// 礼貌请求主程序退出：`PostMessageW(hwnd, WM_CLOSE)`。
///
/// 主程序没有 WM_CLOSE 分支 → `DefWindowProcW` 销毁窗口 → `WM_DESTROY` 里
/// 回收定时器/DIB/托盘并 `PostQuitMessage`，退出前照常落盘。
/// 替代原先的 `taskkill /F /IM AAXSysPulse.exe`：按映像名强杀会误伤同名伪装进程，
/// 且被强杀的实例跳过退出保存（报告 8.41 / P0-9）。
#[cfg(windows)]
pub fn request_main_close() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};

    let class_wide: Vec<u16> = MAIN_WINDOW_CLASS
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let hwnd = match unsafe { FindWindowW(PCWSTR::from_raw(class_wide.as_ptr()), PCWSTR::null()) } {
        Ok(h) => h,
        Err(_) => return false,
    };
    unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok() }
}

#[cfg(not(windows))]
pub fn request_main_close() -> bool {
    false
}

/// 轮询等待主程序窗口消失；超时返回 false。
pub fn wait_main_exit(timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if !is_main_process_running() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    !is_main_process_running()
}

/// 启动主程序
///
/// `app`：仅在能拿到 Tauri 运行时的场合（如样式选择页）才做「启动前兜底释放」；
/// `--autostart` 这类进程早期路径传 None，直接启动安装目录里已有的 exe。
#[cfg(windows)]
pub fn start_main_process(app: Option<&tauri::AppHandle>) -> Result<(), String> {
    let exe_path = crate::install::get_main_program_path()
        .map_err(|e| format!("Failed to get main exe path: {}", e))?;

    // ⚠️ 兜底再释放一次引导程序**内置的当前版本**主程序（大小一致即跳过，增量友好）。
    // 旧部署（或手动拷贝的旧 exe）没有 `WM_RELOAD_CONFIG` 热重载处理，
    // 会导致「引导里改了设置，悬浮窗不实时响应」——正是通信问题的根因。
    // 主程序已在运行时其 exe 被锁定，无法覆盖，只能跳过并告警。
    if let Some(app) = app {
        if !is_main_process_running() {
            match crate::install::release_main_program(app) {
                Ok(p) => tracing::info!("released current main program to {}", p.display()),
                Err(e) => tracing::warn!("release_main_program failed (continue): {}", e),
            }
        } else {
            tracing::warn!("main process already running; skip releasing main program");
        }
    }

    if !exe_path.exists() {
        return Err(format!("主程序不存在: {}", exe_path.display()));
    }

    Command::new(&exe_path)
        .current_dir(exe_path.parent().unwrap_or(&exe_path))
        .spawn()
        .map_err(|e| format!("Failed to start main process: {}", e))?;

    tracing::info!("Main process started: {}", exe_path.display());
    Ok(())
}

#[cfg(not(windows))]
pub fn start_main_process(_app: Option<&tauri::AppHandle>) -> Result<(), String> {
    Err("Not supported on this platform".to_string())
}
