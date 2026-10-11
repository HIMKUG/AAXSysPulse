//! 驱动模块
//! WinRing0x64.sys 的安装步骤被拆成与安装页进度表一一对应的细粒度函数
//! （停止 / 删除 / 释放 / 注册 / 启动），由 install::run_install 按序调度。

use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use thiserror::Error;

#[cfg(windows)]
use windows::{
    core::{HRESULT, PCWSTR},
    Win32::Foundation::WIN32_ERROR,
    Win32::System::Services::{
        CloseServiceHandle, ControlService, CreateServiceW, DeleteService, OpenSCManagerW,
        OpenServiceW, QueryServiceConfigW, QueryServiceStatus, StartServiceW, SC_HANDLE,
        SC_MANAGER_ALL_ACCESS, SERVICE_ALL_ACCESS, SERVICE_CONTROL_STOP, SERVICE_DEMAND_START,
        SERVICE_ERROR_NORMAL, SERVICE_KERNEL_DRIVER, SERVICE_RUNNING, SERVICE_STOPPED,
        SERVICE_STOP_PENDING,
    },
};

#[derive(Error, Debug)]
pub enum DriverError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Not administrator")]
    NotAdmin,
    #[error("SCM error: {0}")]
    Scm(String),
    #[error("服务删除超时，请重启电脑后再试")]
    ServiceDeleteTimeout,
    #[error("驱动文件被其他进程占用，请关闭 HWMonitor 等软件后重试")]
    DriverFileInUse,
    /// 同名服务不是本软件装的（P1-13 / 2.4.3）。
    ///
    /// `AAXSysPulseWinRing0` 这个名字在硬件监控类软件里很容易撞车 —— WinRing0 是
    /// 公开源码的通用驱动，多个产品都在用同名服务/设备。旧实现按名打开即
    /// `DeleteService`/`StartService`，等于**替别人决定它的驱动服务该不该存在**。
    #[error("检测到同名服务但不属于本软件，已停止对其做任何操作")]
    ForeignService,
}

// 全部来自跨进程契约（审查项 7.1.2 / 3.1.14 / P1-22）：主程序 `msr.rs` 自愈重建
// 服务、`pre_launch_checks.rs` 查询服务状态，用的必须是同一份名字与路径。
const DRIVER_SERVICE_NAME: &str = aaxsyspulse_core::contract::DRIVER_SERVICE_NAME;
const DRIVER_DISPLAY_NAME: &str = aaxsyspulse_core::contract::DRIVER_DISPLAY_NAME;
const DRIVER_TARGET_DIR: &str = aaxsyspulse_core::contract::DRIVER_DIR;

/// 获取驱动目标路径（系统目录）
fn driver_target_path() -> PathBuf {
    PathBuf::from(DRIVER_TARGET_DIR).join(aaxsyspulse_core::contract::DRIVER_FILE_NAME)
}

/// 检查是否是管理员权限
#[cfg(windows)]
pub fn is_admin() -> bool {
    std::fs::metadata("C:\\Windows\\System32\\cfgmgr32.dll").is_ok()
}

#[cfg(not(windows))]
pub fn is_admin() -> bool {
    false
}

/// 检测驱动文件是否被其他进程占用
fn is_driver_file_in_use(path: &PathBuf) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
        .is_err()
}

/// 停止服务并等待其完全停止
#[cfg(windows)]
fn stop_service(sc_manager: SC_HANDLE, service_name_wide: &[u16]) -> Result<(), DriverError> {
    unsafe {
        let service = OpenServiceW(sc_manager, PCWSTR::from_raw(service_name_wide.as_ptr()), SERVICE_ALL_ACCESS)
            .map_err(|e| DriverError::Scm(e.to_string()))?;

        // 发送停止指令
        let mut status = std::mem::zeroed();
        let _ = ControlService(service, SERVICE_CONTROL_STOP, &mut status);

        // 关闭服务句柄
        let _ = CloseServiceHandle(service);

        // 轮询等待服务停止（最多 5 秒）
        let timeout = Duration::from_secs(5);
        let start = Instant::now();

        loop {
            if start.elapsed() > timeout {
                return Err(DriverError::Scm("服务停止超时".to_string()));
            }

            let service = OpenServiceW(sc_manager, PCWSTR::from_raw(service_name_wide.as_ptr()), SERVICE_ALL_ACCESS);
            // 打不开服务 = 服务已不存在，说明已停止（原实现 is_err 判完又 unwrap，
            // 只是"当时不会炸"，不是"不可能炸"）。
            let Ok(svc) = service else {
                return Ok(());
            };
            let mut status = std::mem::zeroed();
            if QueryServiceStatus(svc, &mut status).is_ok() {
                let state = status.dwCurrentState;
                if state == SERVICE_STOPPED {
                    let _ = CloseServiceHandle(svc);
                    return Ok(());
                }
                if state != SERVICE_STOP_PENDING {
                    let _ = CloseServiceHandle(svc);
                    break;
                }
            }
            let _ = CloseServiceHandle(svc);
            std::thread::sleep(Duration::from_millis(200));
        }

        Ok(())
    }
}

/// 把服务名转成 null 结尾宽字符缓冲
#[cfg(windows)]
fn service_name_wide() -> Vec<u16> {
    DRIVER_SERVICE_NAME.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 服务镜像路径（`QUERY_SERVICE_CONFIGW::lpBinaryPathName`）。
///
/// 两次调用的标准写法：第一次只取 `BytesNeeded`，第二次按需用 `Vec<u8>`。
/// （主程序侧 `msr.rs` 的同类函数用 `vec![0u64; 512]` + 裸指针重解释，注释自承
/// "缓冲区不足即截断" —— 这里不再复制那个写法。）
#[cfg(windows)]
unsafe fn service_binary_path(svc: SC_HANDLE) -> Option<String> {
    let mut needed = 0u32;
    // 故意忽略返回值：缓冲区为 0 时必然返回 ERROR_MORE_DATA，needed 已被填好。
    let _ = QueryServiceConfigW(svc, None, 0, &mut needed);
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u8; needed as usize];
    let cfg = buf.as_mut_ptr().cast::<windows::Win32::System::Services::QUERY_SERVICE_CONFIGW>();
    if QueryServiceConfigW(svc, Some(cfg), needed, &mut needed).is_err() {
        return None;
    }
    let lp = (*cfg).lpBinaryPathName;
    if lp.0.is_null() {
        return None;
    }
    PCWSTR(lp.0 as *const u16).to_string().ok()
}

/// 该服务的镜像路径是否就是本软件释放的那一个。
///
/// 判据 = 目录前缀 `C:\Windows\AAXSysPulse\` **且** 文件名 `WinRing0x64.sys`。
/// **读不出路径按"不是我们的"处理**：宁可放弃功耗/温度监控（`driver_ok=false`，
/// 前端走第八部分「功耗监控不可用」分支），也不能停掉/删掉/改指别人的服务。
#[cfg(windows)]
fn is_our_driver_binary(path: Option<&str>) -> bool {
    let Some(raw) = path else { return false };
    let p = raw.replace('/', "\\").to_ascii_lowercase();
    let name = p.rsplit('\\').next().unwrap_or("");
    p.starts_with(&DRIVER_TARGET_DIR.to_ascii_lowercase()) && name == "winring0x64.sys"
}

/// 归属闸门：只有确认是本软件装的服务才继续；否则返回 `Err(ForeignService)`。
#[cfg(windows)]
unsafe fn ensure_ours(svc: SC_HANDLE) -> Result<(), DriverError> {
    let path = service_binary_path(svc);
    if is_our_driver_binary(path.as_deref()) {
        return Ok(());
    }
    tracing::warn!(
        "服务 {DRIVER_SERVICE_NAME} 的镜像路径为 {:?}，不是本软件释放的驱动，拒绝操作",
        path
    );
    Err(DriverError::ForeignService)
}

/// 35%：停止旧服务（不存在则返回 false，调用方按「跳过」处理）
#[cfg(windows)]
pub fn stop_old_service() -> Result<bool, DriverError> {
    if !is_admin() {
        return Err(DriverError::NotAdmin);
    }
    unsafe {
        let sc_manager: SC_HANDLE = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)
            .map_err(|e| DriverError::Scm(e.to_string()))?;
        let name = service_name_wide();
        let service = OpenServiceW(sc_manager, PCWSTR::from_raw(name.as_ptr()), SERVICE_ALL_ACCESS);
        let Ok(svc) = service else {
            let _ = CloseServiceHandle(sc_manager);
            return Ok(false);
        };
        // P1-13：先确认这是我们自己装的那个服务，再谈停止。
        let owned = ensure_ours(svc);
        let _ = CloseServiceHandle(svc);
        if let Err(e) = owned {
            let _ = CloseServiceHandle(sc_manager);
            return Err(e);
        }
        let r = stop_service(sc_manager, &name);
        let _ = CloseServiceHandle(sc_manager);
        r.map(|_| true)
    }
}

#[cfg(not(windows))]
pub fn stop_old_service() -> Result<bool, DriverError> {
    Err(DriverError::NotAdmin)
}

/// 45%：删除旧服务并轮询等待其真正消失（不存在则返回 false）
#[cfg(windows)]
pub fn delete_old_service() -> Result<bool, DriverError> {
    if !is_admin() {
        return Err(DriverError::NotAdmin);
    }
    let driver_path = driver_target_path();

    unsafe {
        let sc_manager: SC_HANDLE = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)
            .map_err(|e| DriverError::Scm(e.to_string()))?;
        let name = service_name_wide();

        let service = OpenServiceW(sc_manager, PCWSTR::from_raw(name.as_ptr()), SERVICE_ALL_ACCESS);
        let Ok(svc) = service else {
            let _ = CloseServiceHandle(sc_manager);
            return Ok(false);
        };
        // P1-13：**这是全项目最危险的一行** —— 旧实现按名打开即 DeleteService，
        // 任何别的软件注册的同名 WinRing0 服务会被我们删掉（别人的硬件监控当场失效）。
        // 归属不匹配时既不删除、也不去动 `C:\Windows\AAXSysPulse\` 里的文件。
        let owned = ensure_ours(svc);
        if let Err(e) = owned {
            let _ = CloseServiceHandle(svc);
            let _ = CloseServiceHandle(sc_manager);
            return Err(e);
        }
        // 标记删除；关闭句柄是服务能被彻底删除的必要条件
        DeleteService(svc).map_err(|e| DriverError::Scm(e.to_string()))?;
        let _ = CloseServiceHandle(svc);

        // 等待服务真正从 SCM 中消失（轮询 5 秒）
        let timeout = Duration::from_secs(5);
        let start = Instant::now();
        loop {
            if start.elapsed() > timeout {
                let _ = CloseServiceHandle(sc_manager);
                return Err(DriverError::ServiceDeleteTimeout);
            }
            let probe = OpenServiceW(sc_manager, PCWSTR::from_raw(name.as_ptr()), SERVICE_ALL_ACCESS);
            match probe {
                Ok(s) => {
                    let _ = CloseServiceHandle(s);
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(_) => break, // ERROR_SERVICE_DOES_NOT_EXIST：已彻底删除
            }
        }
        let _ = CloseServiceHandle(sc_manager);
    }

    // 清掉系统目录里的旧驱动文件，给 55% 释放让路
    if driver_path.exists() {
        if is_driver_file_in_use(&driver_path) {
            return Err(DriverError::DriverFileInUse);
        }
        let _ = fs::remove_file(&driver_path);
    }

    Ok(true)
}

#[cfg(not(windows))]
pub fn delete_old_service() -> Result<bool, DriverError> {
    Err(DriverError::NotAdmin)
}

/// 55%：释放驱动到系统目录 C:\Windows\AAXSysPulse\
///
/// 写临时文件再改名就位（审查项 3.1.7 / P2-12）：直接 `File::create` 覆盖目标，
/// 中途写失败会留下一份**截断的 .sys** —— 大小与内置副本不同，于是下次安装的
/// "大小一致即跳过"判据放过它，服务注册成功但驱动永远起不来，且没有自愈路径。
#[cfg(windows)]
pub fn release_driver_sys(driver_data: &[u8]) -> Result<(), DriverError> {
    let target_dir = PathBuf::from(DRIVER_TARGET_DIR);
    if !target_dir.exists() {
        fs::create_dir_all(&target_dir)?;
    }
    let driver_path = driver_target_path();
    let temp_path = driver_path.with_extension("sys.tmp");
    let result = (|| -> std::io::Result<()> {
        {
            let mut file = File::create(&temp_path)?;
            file.write_all(driver_data)?;
            file.sync_all()?;
        }
        fs::rename(&temp_path, &driver_path)?;
        let len = fs::metadata(&driver_path)?.len();
        if len != driver_data.len() as u64 {
            return Err(std::io::Error::other(format!(
                "驱动文件落地后大小不符：期望 {}，实际 {len}",
                driver_data.len()
            )));
        }
        Ok(())
    })();
    if result.is_err() {
        // 不把半成品留在目录里等下一次"看起来已经装好"。
        let _ = fs::remove_file(&temp_path);
    }
    result.map_err(DriverError::Io)
}

#[cfg(not(windows))]
pub fn release_driver_sys(_driver_data: &[u8]) -> Result<(), DriverError> {
    Err(DriverError::NotAdmin)
}

/// 65%：注册内核服务（CreateServiceW，DEMAND_START）
#[cfg(windows)]
pub fn register_driver_service() -> Result<(), DriverError> {
    if !is_admin() {
        return Err(DriverError::NotAdmin);
    }
    let driver_path = driver_target_path();
    unsafe {
        let sc_manager: SC_HANDLE = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)
            .map_err(|e| DriverError::Scm(e.to_string()))?;

        let driver_path_str = driver_path.to_string_lossy();
        let driver_path_wide: Vec<u16> = driver_path_str.encode_utf16().chain(std::iter::once(0)).collect();
        let name = service_name_wide();
        let display_name_wide: Vec<u16> = DRIVER_DISPLAY_NAME.encode_utf16().chain(std::iter::once(0)).collect();

        let service = CreateServiceW(
            sc_manager,
            PCWSTR::from_raw(name.as_ptr()),
            PCWSTR::from_raw(display_name_wide.as_ptr()),
            SERVICE_ALL_ACCESS,
            SERVICE_KERNEL_DRIVER,
            SERVICE_DEMAND_START,
            SERVICE_ERROR_NORMAL,
            PCWSTR::from_raw(driver_path_wide.as_ptr()),
            None,
            None,
            None,
            None,
            None,
        )
        .map_err(|e| {
            // ERROR_SERVICE_EXISTS(1073)：名字已被占用。走到这里说明 45% 的
            // `delete_old_service` **主动拒绝**了删除（归属不匹配）⇒ 占名的是别人的服务。
            // 报成 ForeignService 而不是笼统的 SCM 错误，安装日志里才看得出为什么
            // 最终功耗监控不可用。
            if e.code() == HRESULT::from(WIN32_ERROR(1073)) {
                DriverError::ForeignService
            } else {
                DriverError::Scm(e.to_string())
            }
        })?;

        let _ = CloseServiceHandle(service);
        let _ = CloseServiceHandle(sc_manager);
    }
    tracing::info!("Driver service registered");
    Ok(())
}

#[cfg(not(windows))]
pub fn register_driver_service() -> Result<(), DriverError> {
    Err(DriverError::NotAdmin)
}

/// 75%：启动服务（已在运行按成功处理，ERROR_SERVICE_ALREADY_RUNNING = 1056）
#[cfg(windows)]
pub fn start_driver_service() -> Result<(), DriverError> {
    if !is_admin() {
        return Err(DriverError::NotAdmin);
    }
    unsafe {
        let sc_manager: SC_HANDLE = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)
            .map_err(|e| DriverError::Scm(e.to_string()))?;
        let name = service_name_wide();
        let service = OpenServiceW(sc_manager, PCWSTR::from_raw(name.as_ptr()), SERVICE_ALL_ACCESS)
            .map_err(|e| DriverError::Scm(e.to_string()))?;

        // P1-13 反向场景：`register_driver_service` 因名字已存在而失败时，旧实现会
        // 直接 StartService **别人那个同名服务**，于是主程序打开 `\\.\WinRing0_1_2_0`
        // 用的是第三方提供的驱动（不可控、可能更旧且易受攻击）。现在拒绝启动。
        let owned = ensure_ours(service);
        if let Err(e) = owned {
            let _ = CloseServiceHandle(service);
            let _ = CloseServiceHandle(sc_manager);
            return Err(e);
        }

        let r = match StartServiceW(service, None) {
            Ok(()) => Ok(()),
            // ERROR_SERVICE_ALREADY_RUNNING(1056)：已在运行按成功处理
            Err(e) if e.code() == HRESULT::from(WIN32_ERROR(1056)) => Ok(()),
            Err(e) => Err(DriverError::Scm(e.to_string())),
        };

        let _ = CloseServiceHandle(service);
        let _ = CloseServiceHandle(sc_manager);
        r?;
    }
    tracing::info!("Driver service started");
    Ok(())
}

#[cfg(not(windows))]
pub fn start_driver_service() -> Result<(), DriverError> {
    Err(DriverError::NotAdmin)
}

/// 获取驱动安装状态
#[cfg(windows)]
pub fn get_driver_status() -> Result<DriverStatus, DriverError> {
    if !is_admin() {
        return Ok(DriverStatus { installed: false, running: false, has_file: false });
    }

    let has_file = driver_target_path().exists();

    unsafe {
        let sc_manager: SC_HANDLE = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)
            .map_err(|e| DriverError::Scm(e.to_string()))?;

        let service_name_wide: Vec<u16> = DRIVER_SERVICE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
        let service = OpenServiceW(sc_manager, PCWSTR::from_raw(service_name_wide.as_ptr()), SERVICE_ALL_ACCESS);

        let (installed, running) = if let Ok(svc) = service {
            // P1-13：`installed` 的语义必须是"**本软件**装的服务在"，不是"同名服务在"。
            // 不判归属就会把别人的服务报成"已安装且正在运行"，于是 `run_install` 末尾
            // 据实时状态算出的 `driver_ok` 为真、界面宣称功耗监控可用 ——
            // 而实际用的是第三方驱动（这正是 2.4.3 的静默复用场景）。
            let ours = ensure_ours(svc).is_ok();
            let mut status = std::mem::zeroed();
            let running = ours
                && QueryServiceStatus(svc, &mut status).is_ok()
                && status.dwCurrentState == SERVICE_RUNNING;
            let _ = CloseServiceHandle(svc);
            (ours, running)
        } else {
            (false, false)
        };

        let _ = CloseServiceHandle(sc_manager);

        Ok(DriverStatus { installed, running, has_file })
    }
}

#[cfg(not(windows))]
pub fn get_driver_status() -> Result<DriverStatus, DriverError> {
    Ok(DriverStatus { installed: false, running: false, has_file: false })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverStatus {
    pub installed: bool,
    pub running: bool,
    pub has_file: bool,
}

/// 以管理员身份重新启动（传递 --elevated 参数，跳过权限检测页）
#[cfg(windows)]
pub fn restart_as_admin() -> Result<(), DriverError> {
    use std::env;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;

    let exe_path = env::current_exe()
        .map_err(DriverError::Io)?;

    // 传递 --elevated 参数，新进程据此跳过权限检测页
    let exe_str = exe_path.to_string_lossy();
    let exe_wide: Vec<u16> = OsStr::new(&*exe_str).encode_wide().chain(std::iter::once(0)).collect();
    let args_wide: Vec<u16> = OsStr::new(" --elevated").encode_wide().chain(std::iter::once(0)).collect();

    // 拉起新实例前先释放单实例锁：提权后的新进程与本进程处于**同一会话**
    // （`Local\` 命名空间共享），而 `ShellExecuteW(runas)` 会阻塞到 UAC 交互结束，
    // 新实例几乎必然在本进程 `exit(0)` 之前就已启动。不释放的话新实例会判定
    // "已有引导在跑"而直接退出，用户看到的现象是"点了提权，两个窗口都没了"。
    crate::process::release_guide_single_instance();

    unsafe {
        ShellExecuteW(
            None,
            PCWSTR::from_raw(windows::core::w!("runas").as_ptr()),
            PCWSTR::from_raw(exe_wide.as_ptr()),
            PCWSTR::from_raw(args_wide.as_ptr()),
            None,
            SW_SHOWNORMAL,
        );
    }

    // 退出当前进程
    std::process::exit(0);
}

#[cfg(not(windows))]
pub fn restart_as_admin() -> Result<(), DriverError> {
    Err(DriverError::NotAdmin)
}

/// 归属判据的纯函数测试（P1-13）：这条判据决定"敢不敢碰一个已存在的服务"，
/// 判错方向就是**删掉别人的驱动**，因此必须可测。
#[cfg(all(test, windows))]
mod ownership_tests {
    use super::is_our_driver_binary;

    #[test]
    fn our_dir_and_file_counts_as_ours() {
        assert!(is_our_driver_binary(Some("C:\\Windows\\AAXSysPulse\\WinRing0x64.sys")));
        // 大小写不敏感（SCM 路径由用户/别的安装器写入时不保证口径）
        assert!(is_our_driver_binary(Some("c:\\windows\\AAXSYSPULSE\\winring0x64.sys")));
        // 正斜杠写法同样认（注册表里手工写出来的常见形态）
        assert!(is_our_driver_binary(Some("C:/Windows/AAXSysPulse/WinRing0x64.sys")));
    }

    #[test]
    fn anything_else_is_not_ours() {
        // 报告 2.4.3 的验收场景：同名服务指向系统自带驱动
        assert!(!is_our_driver_binary(Some("C:\\Windows\\System32\\drivers\\kbdclass.sys")));
        // 别人目录里的同名驱动（第三方软件自带 WinRing0 的情况）
        assert!(!is_our_driver_binary(Some("C:\\Program Files\\Other\\WinRing0x64.sys")));
        // 我们目录里但不是那个驱动
        assert!(!is_our_driver_binary(Some("C:\\Windows\\AAXSysPulse\\other.sys")));
        // 读不出路径 ⇒ 一律按"不是我们的"
        assert!(!is_our_driver_binary(None));
        assert!(!is_our_driver_binary(Some("")));
    }
}
