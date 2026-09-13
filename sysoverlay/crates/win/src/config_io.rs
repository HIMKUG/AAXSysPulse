//! Configuration persistence using atomic file writes.
//!
//! File location: `%APPDATA%\AAXSysPulseC\config.txt`
//!
//! Atomic write strategy: write to a temp file in the same directory,
//! then `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` to atomically
//! replace the old file. Temp file and target live on the same volume,
//! satisfying the requirement for `MoveFileExW`.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::mpsc;
use aaxsyspulse_core::config::Config;

/// Returns the config directory path: `%APPDATA%\AAXSysPulseC\`
fn appdata_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|s| std::path::PathBuf::from(s).join("AAXSysPulseC"))
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
            
            Config::parse(&text)
        }
        Err(_) => Config::default(),
    }
}

/// Save configuration to persistent storage atomically.
/// Returns `true` on success.
pub fn save_config(cfg: &Config) -> bool {
    write_config_text(&cfg.serialize())
}

/// 异步落盘（P2）：UI 线程只做序列化 + 入队，磁盘 I/O 在专职写线程执行，
/// 拖动设置项时不再被磁盘写毛刺卡顿。防抖已把频率限制在 ~2s 一次，
/// 单线程串行写足够。返回 `true` = 已成功入队。
/// ⚠️ 进程退出路径（关窗/隐藏 flush）请用 [`save_config`] 同步落盘保数据。
pub fn save_config_async(cfg: &Config) -> bool {
    static WRITER: std::sync::OnceLock<mpsc::Sender<String>> = std::sync::OnceLock::new();
    let tx = WRITER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<String>();
        let _ = std::thread::Builder::new()
            .name("config-writer".into())
            .spawn(move || {
                for text in rx.iter() {
                    write_config_text(&text);
                }
            });
        tx
    });
    tx.send(cfg.serialize()).is_ok()
}

/// 把序列化文本原子落盘（临时文件 + MoveFileExW 替换）。返回 `true` 成功。
fn write_config_text(serialized: &str) -> bool {
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

    // Write to temp file，并在 rename 前 fsync：断电时 MoveFileExW 可能已
    // 替换成功而数据未落盘 → 空配置。宽容解析器可自愈，但丢全部设置仍不友好。
    let write_ok = std::fs::File::create(&temp).and_then(|mut f| {
        use std::io::Write;
        f.write_all(serialized.as_bytes())?;
        f.sync_all()
    });
    if let Err(e) = write_ok {
        eprintln!("[config_io] failed to write temp file: {}", e);
        let _ = std::fs::remove_file(&temp);
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
