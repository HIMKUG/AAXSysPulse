//! Configuration persistence using atomic file writes.
//!
//! File location: `%APPDATA%\SysOverlayC\config.txt`
//!
//! Atomic write strategy: write to a temp file in the same directory,
//! then `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` to atomically
//! replace the old file. Temp file and target live on the same volume,
//! satisfying the requirement for `MoveFileExW`.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use sysoverlay_core::config::Config;

/// Returns the config directory path: `%APPDATA%\SysOverlayC\`
fn appdata_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|s| std::path::PathBuf::from(s).join("SysOverlayC"))
}

fn to_wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Load configuration from persistent storage.
/// Returns the saved config, or `Config::default()` if not found or unreadable.
pub fn load_config() -> Config {
    let path = match appdata_dir() {
        Some(d) => d.join("config.txt"),
        None => return Config::default(),
    };

    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let cfg = Config::parse(&text);
            cfg
        }
        Err(_) => Config::default(),
    }
}

/// Save configuration to persistent storage atomically.
/// Returns `true` on success.
pub fn save_config(cfg: &Config) -> bool {
    let dir = match appdata_dir() {
        Some(d) => d,
        None => return false,
    };

    // Create directory if missing
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("[config_io] failed to create directory: {}", e);
        return false;
    }

    let target = dir.join("config.txt");
    let temp = dir.join("config.txt.tmp");

    let serialized = cfg.serialize();

    // Write to temp file
    if let Err(e) = std::fs::write(&temp, serialized.as_bytes()) {
        eprintln!("[config_io] failed to write temp file: {}", e);
        return false;
    }

    // Atomic replace via MoveFileExW
    unsafe {
        use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING};

        let wide_target: Vec<u16> = to_wide(target.as_os_str());
        let wide_temp: Vec<u16> = to_wide(temp.as_os_str());

        let result = MoveFileExW(
            windows::core::PCWSTR(wide_temp.as_ptr()),
            windows::core::PCWSTR(wide_target.as_ptr()),
            MOVEFILE_REPLACE_EXISTING,
        );

        if result.is_err() {
            let err = windows::Win32::Foundation::GetLastError();
            eprintln!("[config_io] MoveFileExW failed: error {}", err.0);
            // Clean up temp file on failure
            let _ = std::fs::remove_file(&temp);
            return false;
        }
    }

    true
}

/// Returns the config file path, or `None` if the directory is not available.
pub fn get_config_path() -> Option<String> {
    appdata_dir().map(|d| d.join("config.txt").to_string_lossy().into_owned())
}
