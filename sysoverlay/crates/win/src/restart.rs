//! 托盘「快速重启」：杀掉当前进程并以同路径拉起新实例。
//!
//! 新实例走正常启动路径（main.rs），会重新加载 MSR 驱动服务
//! （`SysOverlayWinRing0`，`msr.rs` 幂等 `OpenService`+`StartService`）与采样线程，
//! 因此「杀掉并拉起所有驱动及其服务」由一次干净的重启天然保证。

use std::os::windows::ffi::OsStrExt;

/// 拉起新实例后退出当前进程。
///
/// 顺序保证：先 `ShellExecuteExW` 启动带 `--restart` 的新进程（让其重试抢单实例互斥体），
/// 成功后才 `std::process::exit` 让出互斥体 → 新实例立即接管。拉起失败时**保留**当前实例，
/// 不破坏用户正在运行的环境。
pub fn quick_restart() {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => {
            crate::probe::log("quick_restart: cannot resolve own exe path -> abort");
            return;
        }
    };
    let exe_w: Vec<u16> = exe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let params: Vec<u16> = "--restart\0".encode_utf16().collect();

    unsafe {
        use windows::Win32::UI::Shell::{
            ShellExecuteExW, SEE_MASK_DEFAULT, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
        };
        let mut sei = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            // NOASYNC：同步等待 Shell 真正派生新进程后再返回，
            // 保证 exit(0) 时新实例已存在并进入互斥体重试。
            fMask: SEE_MASK_DEFAULT | SEE_MASK_NOASYNC,
            lpFile: windows::core::PCWSTR(exe_w.as_ptr()),
            lpParameters: windows::core::PCWSTR(params.as_ptr()),
            nShow: 1,
            ..Default::default()
        };
        if ShellExecuteExW(&mut sei).is_ok() {
            crate::probe::log("quick_restart: relaunch requested, exiting current process");
            std::process::exit(0);
        } else {
            crate::probe::log("quick_restart: ShellExecuteExW failed -> keep running");
        }
    }
}
