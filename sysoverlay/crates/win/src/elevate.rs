//! 管理员权限（用户拍板）：
//! - 启动时检测；无管理员且 Power 开启 → 弹窗询问「以管理员身份重启」；
//! - 选是 → `runas` 自重启（弹 UAC），当前进程退出（单实例让位，新进程带
//!   `--elevated` 参数重试抢互斥体）；
//! - 拒绝 UAC / 选否 → **Power 行强制关闭**（禁止显示 + 停止计算）。

use crate::probe;
use windows::Win32::UI::Shell::IsUserAnAdmin;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONQUESTION, MB_YESNO};

pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

/// 弹窗询问 + runas 自重启。返回 `true` = 已发起重启（调用方应立即退出）。
pub fn prompt_and_relaunch() -> bool {
    unsafe {
        let text = windows::core::w!("整机功耗显示需要管理员权限（读取 CPU 功耗传感器）。\n\n是否以管理员身份重新启动 SysOverlay？\n（选择「否」将关闭功耗行，不影响其他显示）");
        let caption = windows::core::w!("SysOverlay");
        let choice = MessageBoxW(None, text, caption, MB_YESNO | MB_ICONQUESTION);
        if choice != IDYES {
            probe::log("elevate user declined -> power row disabled");
            return false;
        }
        // 自身路径
        let mut path = [0u16; 512];
        let len = windows::Win32::System::LibraryLoader::GetModuleFileNameW(None, &mut path);
        let exe = String::from_utf16_lossy(&path[..len as usize]);
        let exe_w: Vec<u16> = exe.encode_utf16().chain(Some(0)).collect();
        let verb: Vec<u16> = "runas\0".encode_utf16().collect();
        let params: Vec<u16> = "--elevated\0".encode_utf16().collect();

        use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_DEFAULT, SHELLEXECUTEINFOW};
        let mut sei = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_DEFAULT,
            lpVerb: windows::core::PCWSTR(verb.as_ptr()),
            lpFile: windows::core::PCWSTR(exe_w.as_ptr()),
            lpParameters: windows::core::PCWSTR(params.as_ptr()),
            nShow: 1,
            ..Default::default()
        };
        if ShellExecuteExW(&mut sei).is_ok() {
            probe::log("elevate relaunch requested");
            true
        } else {
            probe::log("elevate ShellExecuteExW failed -> power row disabled");
            false
        }
    }
}
