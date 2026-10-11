//! 开机自启：HKCU\Software\Microsoft\Windows\CurrentVersion\Run。
//!
//! 值名 "AAX SysPulse"，数据为 `"<exe 完整路径>" --autostart`。
//!
//! # 双向保障
//!
//! 自启状态在两个地方各存一份，**两处必须一致**，否则用户看到的状态是假的：
//! 1. `Run` 键里的启动项（决定"会不会启动"）；
//! 2. `Explorer\StartupApproved\Run` 里的 12 字节标记（决定"是否被用户/系统禁用"）。
//!
//! 任务管理器「启动」页的启用/禁用只改第 2 处。因此：
//! - 开启 = 写第 1 处 **且** 清除第 2 处的禁用标记；
//! - 关闭 = 删第 1 处 **且** 删第 2 处的残留标记（否则下次开启仍被系统挡着）。
//!
//! `state()` 用于把系统**真实**状态读回来核对（仅在配置里记一个 bool 是不够的）。

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE,
    REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ,
};

const RUN_KEY: &str = aaxsyspulse_core::contract::RUN_KEY;
/// 任务管理器「启动」页的启用/禁用状态（12 字节二进制块）存放处。
const APPROVED_KEY: &str = aaxsyspulse_core::contract::APPROVED_RUN_KEY;
/// Run 键值名 —— 契约常量：卸载清理（引导 `install.rs`）引用同一份（7.1.2 / P1-22）。
const VALUE_NAME: &str = aaxsyspulse_core::contract::RUN_VALUE_MAIN;

/// 自启启动参数：登录时启动走这条路径，**不弹任何对话框、不弹 UAC**。
pub const AUTOSTART_FLAG: &str = "--autostart";

/// 系统侧的自启真实状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// 启动项存在且未被禁用 → 登录时真的会启动。
    Enabled,
    /// 启动项存在，但被用户在「任务管理器 → 启动」里禁用了 → **不会**启动。
    DisabledByUser,
    /// 启动项不存在。
    Absent,
}

/// 本次进程是否由自启拉起（`--autostart`）。
pub fn launched_at_logon() -> bool {
    std::env::args().any(|a| a == AUTOSTART_FLAG)
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 当前进程的 exe 完整路径。
fn exe_path() -> Option<String> {
    let mut buf = [0u16; 512];
    let len = unsafe { windows::Win32::System::LibraryLoader::GetModuleFileNameW(None, &mut buf) };
    if len == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// `Run` 项的数据：exe 完整路径（带引号，防路径含空格）+ 自启标记参数。
fn command_line() -> Option<String> {
    exe_path().map(|p| format!("\"{}\" {}", p, AUTOSTART_FLAG))
}

/// `Run` 键里的启动项是否存在。
fn value_exists() -> bool {
    unsafe {
        let key_name = to_wide(RUN_KEY);
        let value_name = to_wide(VALUE_NAME);
        let mut hkey = HKEY::default();
        let open = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_name.as_ptr()),
            0,
            KEY_QUERY_VALUE,
            &mut hkey,
        );
        if open.is_err() {
            return false;
        }
        let mut data = [0u16; 512];
        let mut size = (data.len() * 2) as u32;
        let ret = RegGetValueW(
            hkey,
            PCWSTR::null(),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(data.as_mut_ptr() as *mut _),
            Some(&mut size),
        );
        let _ = RegCloseKey(hkey);
        ret == ERROR_SUCCESS
    }
}

/// StartupApproved 二进制块的判定：首字节最低位 = 1 → 已禁用。
///
/// 系统写入的是 12 字节块，`[0]` 常见取值 0x02（启用）/ 0x03（禁用），
/// 也有 0x06 / 0x07 的变体（带"已被系统禁用"的高位），故只取最低位判断。
fn flag_is_disabled(first_byte: u8) -> bool {
    first_byte & 0x01 != 0
}

/// 读取 StartupApproved 标记。返回 `None` = 无标记（视为启用）。
fn approved_flag() -> Option<u8> {
    unsafe {
        let key_name = to_wide(APPROVED_KEY);
        let value_name = to_wide(VALUE_NAME);
        let mut hkey = HKEY::default();
        let open = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_name.as_ptr()),
            0,
            KEY_QUERY_VALUE,
            &mut hkey,
        );
        if open.is_err() {
            return None;
        }
        let mut data = [0u8; 16];
        let mut size = data.len() as u32;
        let ret = RegGetValueW(
            hkey,
            PCWSTR::null(),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_BINARY,
            None,
            Some(data.as_mut_ptr() as *mut _),
            Some(&mut size),
        );
        let _ = RegCloseKey(hkey);
        if ret == ERROR_SUCCESS && size >= 1 {
            Some(data[0])
        } else {
            None
        }
    }
}

/// 删除 StartupApproved 标记（不存在 = 成功）。
///
/// 关闭时必须清掉：否则用户"关掉再开启"后，第 1 处写回去了、第 2 处仍是禁用标记，
/// 登录时**依然不会启动** —— 表现为"开关显示已开启但就是不生效"。
fn clear_approved_flag() -> bool {
    unsafe {
        let key_name = to_wide(APPROVED_KEY);
        let value_name = to_wide(VALUE_NAME);
        let mut hkey = HKEY::default();
        let open = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_name.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if open.is_err() {
            return true; // 键不存在 = 没有残留标记
        }
        let ret = RegDeleteValueW(hkey, PCWSTR(value_name.as_ptr()));
        let _ = RegCloseKey(hkey);
        ret == ERROR_SUCCESS || ret == ERROR_FILE_NOT_FOUND
    }
}

/// 系统侧真实状态（把注册表读回来核对）。
pub fn state() -> State {
    match mode() {
        // 计划任务由系统托管，不存在「被用户在启动页禁用」这一档。
        Mode::Task => State::Enabled,
        Mode::None => State::Absent,
        Mode::Run => match approved_flag() {
            Some(b) if flag_is_disabled(b) => State::DisabledByUser,
            _ => State::Enabled,
        },
    }
}

/// 当前生效的自启机制。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 未注册任何自启。
    None,
    /// 计划任务（**推荐**：登录触发 + 最高权限 + 延迟启动）。
    Task,
    /// 注册表 `Run` 项（退化方案：无管理员权限，登录时不会提权）。
    Run,
}

/// 当前自启用的是哪种机制。
pub fn mode() -> Mode {
    if task_exists() {
        Mode::Task
    } else if value_exists() {
        Mode::Run
    } else {
        Mode::None
    }
}

/// 系统里是否已注册自启（任一机制）。
pub fn current() -> bool {
    !matches!(mode(), Mode::None)
}

/// 把系统状态折算成「开关应该显示成什么」（**纯函数**，便于单测）。
///
/// 关键：写失败回滚时**不能简单取反**。用户在任务管理器里可能动过开关，
/// 取反会把真实状态写反，下一轮幂等同步又改回来 ⇒ 开关反复横跳。
/// 以系统真实状态为准才是自愈。
pub fn state_as_switch(s: State) -> bool {
    // DisabledByUser 也算"已注册"：启动项在，只是被禁用 —— 开关应显示为开
    // （用户的显式禁用是另一件事，不该被一次写失败悄悄改掉）。
    matches!(s, State::Enabled | State::DisabledByUser)
}

// ── 计划任务（schtasks） ──────────────────────────────────
//
// 为什么用计划任务：`Run` 项启动**拿不到管理员权限**，而功耗监控需要读 MSR，
// 于是每次登录都要弹一次 UAC。计划任务用 `/RL HIGHEST` 直接以最高权限运行
// （登录时不弹 UAC），还能 `/DELAY` 错开登录高峰、失败可重试。
//
// 代价：创建/删除任务需要管理员。因此：
// - 已提权时 → 用计划任务，并清掉 `Run` 项（避免双启动）；
// - 未提权时 → 退回 `Run` 项（不做"静默弹 UAC"，避免开关一按就跳授权框）；
// - 关闭时若任务已存在而未提权 → 起一个提权的自身进程去删（会弹一次 UAC）。

/// 计划任务名 —— 契约常量，卸载侧删的是同一个名字（3.1.14 / P1-22）。
const TASK_NAME: &str = aaxsyspulse_core::contract::TASK_NAME;
/// 登录后延迟启动，错开登录高峰（格式 `HHHH:MM`）。
const TASK_DELAY: &str = "0000:30";
/// 提权辅助进程参数：创建 / 删除计划任务后立即退出。
pub const ARG_TASK_CREATE: &str = "--auto-task-create";
pub const ARG_TASK_REMOVE: &str = "--auto-task-remove";

/// `schtasks.exe` 绝对路径（不依赖 PATH）。
fn schtasks_path() -> String {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    format!("{root}\\System32\\schtasks.exe")
}

/// 执行 `schtasks` 并返回是否成功（退出码 0）。无法启动进程 = false。
fn run_schtasks(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    match std::process::Command::new(schtasks_path())
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(st) => st.success(),
        Err(_) => false,
    }
}

/// 计划任务是否已注册。
pub fn task_exists() -> bool {
    run_schtasks(&["/Query", "/TN", TASK_NAME])
}

/// 创建（或覆盖）计划任务：登录触发 + 最高权限 + 延迟 30s。
///
/// `/TR` 的值是 `"<exe>" --autostart`，交给 `std::process::Command` 做引号转义
/// （路径含空格必须带引号，否则 schtasks 会把路径截断到第一个空格）。
pub fn create_task() -> bool {
    let Some(exe) = exe_path() else { return false };
    let tr = format!("\"{exe}\" {AUTOSTART_FLAG}");
    run_schtasks(&[
        "/Create", "/TN", TASK_NAME, "/TR", &tr, "/SC", "ONLOGON", "/RL", "HIGHEST", "/DELAY",
        TASK_DELAY, "/F",
    ])
}

/// 删除计划任务（不存在 = 成功）。
pub fn delete_task() -> bool {
    if !task_exists() {
        return true;
    }
    run_schtasks(&["/Delete", "/TN", TASK_NAME, "/F"])
}

/// 起一个**提权**的自身进程执行计划任务操作，等待其退出码。
///
/// 仅在「任务已存在但当前进程没有管理员权限」时使用（删除任务必须要权限）。
/// 用户取消 UAC → 返回 false（调用方据实回滚开关，不做假状态）。
pub fn task_op_elevated(arg: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };

    let Some(exe) = exe_path() else { return false };
    let exe_w: Vec<u16> = exe.encode_utf16().chain(std::iter::once(0)).collect();
    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let params: Vec<u16> = format!("{arg}\0").encode_utf16().collect();
    unsafe {
        let mut sei = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(exe_w.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: 0, // SW_HIDE：只跑逻辑，不弹窗口
            ..Default::default()
        };
        if ShellExecuteExW(&mut sei).is_err() {
            // 用户取消 UAC / 提权被策略拦截
            crate::probe::log("autostart: elevated task op refused (UAC cancelled?)");
            return false;
        }
        if sei.hProcess.is_invalid() {
            return false;
        }
        let waited = WaitForSingleObject(sei.hProcess, 60_000);
        let mut code = 1u32;
        let got = GetExitCodeProcess(sei.hProcess, &mut code).is_ok();
        let _ = CloseHandle(sei.hProcess);
        waited == WAIT_OBJECT_0 && got && code == 0
    }
}

/// 开启自启。
///
/// 优先用**计划任务**（登录触发 + `/RL HIGHEST` 最高权限 + 延迟 30s）——
/// 这样登录时不会弹 UAC，功耗监控直接可用。创建任务需要管理员：
/// - 当前已提权 → 建任务，并清掉 `Run` 项（避免两处都启动）；
/// - 未提权（或建任务失败）→ 退回 `Run` 项。
///
/// `clear_approved`：是否顺带解除「任务管理器 → 启动」里的禁用标记。
/// 只有用户**主动开启**时才传 true；启动时的幂等同步不覆盖用户的系统级选择。
fn enable(clear_approved: bool) -> bool {
    if crate::elevate::is_admin() && create_task() {
        let _ = delete();
        let _ = clear_approved_flag();
        crate::probe::log("autostart: enabled via Task Scheduler (highest privileges)");
        return true;
    }
    // 退回 Run 项：无管理员权限时登录不自启提权，但不弹任何授权框。
    let Some(cmd) = command_line() else { return false };
    if !write_value(&cmd) {
        return false;
    }
    if clear_approved {
        let _ = clear_approved_flag();
    }
    crate::probe::log("autostart: enabled via HKCU\\Run (no elevation at logon)");
    true
}

/// 关闭自启：**两处一起清**（计划任务 + `Run` 项 + 禁用标记）。
///
/// 任务已存在而当前未提权时，起一个提权辅助进程去删（会弹一次 UAC）——
/// 删不掉就如实返回 false，由调用方回滚开关，绝不留下"显示已关、其实还在自启"。
fn disable() -> bool {
    let task_gone = if task_exists() {
        if crate::elevate::is_admin() {
            delete_task()
        } else {
            crate::probe::log("autostart: task exists but not elevated -> request elevated removal");
            task_op_elevated(ARG_TASK_REMOVE)
        }
    } else {
        true
    };
    let run_gone = delete() && clear_approved_flag();
    task_gone && run_gone
}

/// 同步自启状态（**幂等，启动时调用**）。
///
/// - `enabled = true`：按当前 exe 路径重写启动项 —— 自动修复用户移动 /
///   重命名 / 升级后失效的旧路径。
/// - `enabled = false`：清掉计划任务 + `Run` 项 + 禁用标记。
///
/// **不覆盖**用户在「任务管理器 → 启动」里的禁用选择（那是用户的显式意图，
/// 每启动一次就强行改回来会变成流氓行为）。需要解除时用 [`apply_user`]。
pub fn apply(enabled: bool) -> bool {
    if enabled {
        enable(false)
    } else {
        disable()
    }
}

/// 用户在**应用内显式切换**开关时调用（在 [`apply`] 基础上解除系统禁用标记）。
pub fn apply_user(enabled: bool) -> bool {
    if enabled {
        enable(true)
    } else {
        disable()
    }
}

/// 设置面板「开机自启」开关的**异步**应用入口（**UI 线程安全**）。
///
/// # 为什么必须异步
///
/// [`apply_user`] 是阻塞的，且最坏会弹 UAC 并 [`WaitForSingleObject`] 60s
/// （删计划任务需要提权）。设置开关跑在 **UI 线程**上（`settings` 的
/// `Action::Toggle` → `apply_config` → [`CfgRef::set`]），同步调用会让整个
/// 程序（悬浮窗 + 设置窗）无响应最长一分钟 —— 那比"自启没写成功"严重得多。
///
/// # 契约
///
/// - 立即返回，绝不阻塞 UI 线程；
/// - 开关值（`cfg.autostart`）**照旧由调用方同步写入**（纯内存，见
///   [`crate::settings::cfg_ref`]），因此 UI 立刻回显、落盘不受影响；
/// - 系统侧结果通过 [`WM_AUTOSTART_DONE`] 异步回报 UI 线程：
///   `WPARAM` = 请求值（1/0），`LPARAM` = 是否成功；
/// - 失败时调用方**按系统真实状态回滚开关**并提示，绝不留下"显示已开、其实没开"。
///
/// 用后台线程 + [`PostMessageW`] 回报，而不是 `Send` 句柄：与本模块既有的
/// 提权辅助进程（[`ARG_TASK_CREATE`] / [`ARG_TASK_REMOVE`]）同一套模式。
pub fn apply_user_async(hwnd: windows::Win32::Foundation::HWND, enabled: bool) {
    // HWND 不是 Send（裸指针不保证线程安全）→ 跨线程只传地址，线程内重建。
    // 与 main.rs 采样线程传 hwnd 的做法一致。
    let hwnd_addr = hwnd.0 as usize;
    std::thread::spawn(move || {
        let ok = apply_user(enabled);
        crate::probe::log(&format!(
            "autostart: async apply_user({enabled}) ok={ok} mode={:?} os_state={:?}",
            mode(),
            state()
        ));
        use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        let hwnd = HWND(hwnd_addr as *mut _);
        // 投递失败（窗口已销毁）无副作用可做：进程正在退出，系统侧状态已写完。
        let _ = unsafe {
            PostMessageW(
                hwnd,
                aaxsyspulse_core::contract::WM_AUTOSTART_DONE,
                WPARAM(enabled as usize),
                LPARAM(ok as isize),
            )
        };
    });
}

/// 写入 `Run` 项。
fn write_value(cmd: &str) -> bool {
    unsafe {
        let key_name = to_wide(RUN_KEY);
        let value_name = to_wide(VALUE_NAME);
        let mut hkey = HKEY::default();
        let open = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_name.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        );
        if open.is_err() {
            return false;
        }
        let data = to_wide(cmd);
        // REG_SZ 数据 = UTF-16 字节（含结尾 NUL）
        let bytes: &[u8] =
            std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
        let ret = RegSetValueExW(hkey, PCWSTR(value_name.as_ptr()), 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(hkey);
        ret == ERROR_SUCCESS
    }
}

fn delete() -> bool {
    unsafe {
        let key_name = to_wide(RUN_KEY);
        let value_name = to_wide(VALUE_NAME);
        let mut hkey = HKEY::default();
        let open = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_name.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if open.is_err() {
            // 键不存在 = 已删除 = 成功
            return true;
        }
        let ret = RegDeleteValueW(hkey, PCWSTR(value_name.as_ptr()));
        let _ = RegCloseKey(hkey);
        ret == ERROR_SUCCESS || ret == ERROR_FILE_NOT_FOUND
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_approved_flag_low_bit_means_disabled() {
        // 0x02 / 0x06 = 启用；0x03 / 0x07 = 禁用（只取最低位）。
        assert!(!flag_is_disabled(0x02));
        assert!(!flag_is_disabled(0x06));
        assert!(flag_is_disabled(0x03));
        assert!(flag_is_disabled(0x07));
        assert!(!flag_is_disabled(0x00));
    }

    #[test]
    fn command_line_is_quoted_and_carries_autostart_flag() {
        // 只验证格式契约：路径必须带引号（防空格）+ 带 --autostart。
        let cmd = command_line().expect("GetModuleFileNameW 应能取到自身路径");
        assert!(cmd.starts_with('"'), "路径必须带引号：{cmd}");
        assert!(cmd.contains("\" --autostart"), "必须带自启标记：{cmd}");
    }

    /// 自愈决策：回滚时开关必须**跟随系统真实状态**，不是简单取反。
    #[test]
    fn state_as_switch_reflects_registration_not_enablement() {
        // 启动项不存在 → 显示关。
        assert!(!state_as_switch(State::Absent));
        // 存在且启用 → 显示开。
        assert!(state_as_switch(State::Enabled));
        // 存在但被用户在任务管理器禁用 → 仍显示"开"（禁用是另一件事，
        // 不该被一次写失败悄悄改成关，反之亦然 —— 否则开关反复横跳）。
        assert!(state_as_switch(State::DisabledByUser));
    }
}
