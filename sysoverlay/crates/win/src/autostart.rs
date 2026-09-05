//! 开机自启：HKCU\Software\Microsoft\Windows\CurrentVersion\Run。
//!
//! 值名 "SysOverlay"，数据为带引号的 exe 完整路径。
//! `current()` 查询是否已写入；`apply()` 幂等同步注册表状态。

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE,
    REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "SysOverlay";

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// exe 完整路径（带引号，防路径含空格）。
fn command_line() -> Option<String> {
    let mut buf = [0u16; 512];
    let len = unsafe { windows::Win32::System::LibraryLoader::GetModuleFileNameW(None, &mut buf) };
    if len == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    Some(format!("\"{}\"", path))
}

/// 当前是否已设置自启。
pub fn current() -> bool {
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

/// 同步自启状态到注册表（幂等）。
pub fn apply(enabled: bool) -> bool {
    if !enabled {
        return delete();
    }
    let Some(cmd) = command_line() else { return false };

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
        let data = to_wide(&cmd);
        // REG_SZ 数据 = UTF-16 字节（含结尾 NUL）
        let bytes: &[u8] = std::slice::from_raw_parts(
            data.as_ptr() as *const u8,
            data.len() * 2,
        );
        let ret = RegSetValueExW(
            hkey,
            PCWSTR(value_name.as_ptr()),
            0,
            REG_SZ,
            Some(bytes),
        );
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
