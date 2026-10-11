//! 快捷方式模块
//! 第六部分：用 IShellLinkW + IPersistFile::Save 创建桌面 / 开始菜单 .lnk 快捷方式。
//! · 目标：%LOCALAPPDATA%\AAXSysPulse\AAXSysPulse.exe；工作目录为安装目录；图标取主程序 exe 自带
//! · 已存在时直接覆盖（IPersistFile::Save 天然覆盖写）

use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ShortcutError {
    #[error("Failed to get desktop folder")]
    NoDesktop,
    #[error("Failed to create COM object: {0}")]
    ComError(String),
    #[error("Failed to save shortcut: {0}")]
    SaveError(String),
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 创建一个指向主程序的 .lnk
#[cfg(windows)]
fn make_lnk(link_path: &Path) -> Result<(), ShortcutError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile,
        COINIT_APARTMENTTHREADED, CLSCTX_INPROC_SERVER,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    let exe = crate::install::get_main_program_path()
        .map_err(|e| ShortcutError::ComError(e.to_string()))?;
    let workdir = exe
        .parent()
        .ok_or_else(|| ShortcutError::ComError("no install dir".into()))?;

    // 本线程已被他人以其他模式初始化时 CoInitializeEx 返回错误码——同样可继续使用；
    // 仅在自己成功初始化时才配对 CoUninitialize。
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let initialized = hr.is_ok();

    let result = (|| -> Result<(), ShortcutError> {
        let link: IShellLinkW = unsafe {
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| ShortcutError::ComError(e.to_string()))?
        };

        // PCWSTR 只借不存，各 Vec<u16> 必须在对应调用返回前保持存活
        let exe_w: Vec<u16> = exe.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let dir_w: Vec<u16> = workdir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let lnk_w = wide(&link_path.to_string_lossy());
        let desc_w = wide(&format!("{} 桌面监控", aaxsyspulse_core::contract::APP_TECH_NAME));

        unsafe {
            link.SetPath(PCWSTR::from_raw(exe_w.as_ptr()))
                .and_then(|_| link.SetWorkingDirectory(PCWSTR::from_raw(dir_w.as_ptr())))
                .and_then(|_| link.SetIconLocation(PCWSTR::from_raw(exe_w.as_ptr()), 0))
                .and_then(|_| link.SetDescription(PCWSTR::from_raw(desc_w.as_ptr())))
                .map_err(|e| ShortcutError::ComError(e.to_string()))?;

            let persist: IPersistFile = link
                .cast()
                .map_err(|e| ShortcutError::ComError(e.to_string()))?;
            persist
                .Save(PCWSTR::from_raw(lnk_w.as_ptr()), true)
                .map_err(|e| ShortcutError::SaveError(e.to_string()))?;
        }
        Ok(())
    })();

    if initialized {
        unsafe { CoUninitialize() };
    }
    result
}

/// 在桌面创建 AAXSysPulse.lnk 快捷方式
#[cfg(windows)]
pub fn create_desktop_shortcut() -> Result<PathBuf, ShortcutError> {
    let desktop = dirs::desktop_dir().ok_or(ShortcutError::NoDesktop)?;
    let shortcut_path = desktop.join(aaxsyspulse_core::contract::SHORTCUT_FILE_NAME);
    make_lnk(&shortcut_path)?;
    tracing::info!("Created desktop shortcut at {}", shortcut_path.display());
    Ok(shortcut_path)
}

/// 在开始菜单创建 AAXSysPulse 文件夹与快捷方式
/// 路径：%APPDATA%\Microsoft\Windows\Start Menu\Programs\AAXSysPulse\
#[cfg(windows)]
pub fn create_start_menu_shortcut() -> Result<PathBuf, ShortcutError> {
    let program_dir = dirs::config_dir()
        .ok_or(ShortcutError::NoDesktop)?
        .join(aaxsyspulse_core::contract::START_MENU_FOLDER);
    std::fs::create_dir_all(&program_dir).map_err(|e| ShortcutError::SaveError(e.to_string()))?;

    let shortcut_path = program_dir.join(aaxsyspulse_core::contract::SHORTCUT_FILE_NAME);
    make_lnk(&shortcut_path)?;
    tracing::info!("Created start menu shortcut at {}", shortcut_path.display());
    Ok(shortcut_path)
}

#[cfg(not(windows))]
pub fn create_desktop_shortcut() -> Result<PathBuf, ShortcutError> {
    Err(ShortcutError::ComError("Not supported on this platform".to_string()))
}

#[cfg(not(windows))]
pub fn create_start_menu_shortcut() -> Result<PathBuf, ShortcutError> {
    Err(ShortcutError::ComError("Not supported on this platform".to_string()))
}
