//! Driver installer launcher
//!
//! This module manages the driver-installer pre-launcher.
//! The driver-installer is a separate binary that:
//! 1. Checks for admin privileges
//! 2. Checks if the WinRing0x64.sys driver is loaded
//! 3. If neither, prompts the user to install the driver
//! 4. Then launches the main application
//!
//! The driver-installer binary should be placed alongside the main executable.
//! It is NOT embedded to avoid build complexity.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};

/// Name of the driver-installer binary
const DRIVER_INSTALLER_NAME: &str = "driver-installer.exe";

/// Get the expected path of driver-installer (alongside the main exe)
fn external_installer_path() -> Option<PathBuf> {
    let exe_path = env::current_exe().ok()?;
    let exe_dir = exe_path.parent()?;
    Some(exe_dir.join(DRIVER_INSTALLER_NAME))
}

/// Check if driver-installer exists externally
pub fn is_available() -> bool {
    external_installer_path()
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Run the driver-installer to check admin/driver status and possibly install.
///
/// The driver-installer will either:
/// - Launch this application if checks pass or user confirms
/// - Return without launching if user cancels
///
/// Returns `Ok(true)` if the installer ran and made a decision (either launched
/// the app or user cancelled gracefully). Returns `Ok(false)` if the installer
/// could not be found or run.
///
/// Note: This function does NOT return if the driver-installer decides to
/// launch the main application - it will exec/replace the current process.
pub fn run_driver_installer_and_launch() -> io::Result<bool> {
    let installer_path = match external_installer_path() {
        Some(p) if p.exists() => p,
        Some(p) => {
            eprintln!("driver-installer not found at: {}", p.display());
            return Ok(false);
        }
        None => {
            eprintln!("could not determine executable path");
            return Ok(false);
        }
    };

    eprintln!("driver-installer: running from {}", installer_path.display());

    // Execute the driver-installer with CREATE_NO_WINDOW
    // The installer will:
    // 1. Show dialogs for admin/driver status
    // 2. Install driver if needed
    // 3. Launch the main app (replacing this process)
    // 4. Or return if user cancelled
    let exit_status = Command::new(&installer_path)
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .status()?;

    // If we get here, the installer returned without launching the app
    // Check its exit code:
    // 0 = driver loaded/installed, app launched (shouldn't reach here)
    // 1 = admin rights needed, user chose to launch anyway
    // 2 = driver not available, user chose to launch anyway  
    // 3 = user cancelled
    match exit_status.code() {
        Some(0) | Some(1) | Some(2) => {
            // Installer launched the app (exit code from before it exec'd)
            // We shouldn't reach here but treat as success
            Ok(true)
        }
        Some(3) => {
            // User cancelled
            Ok(false)
        }
        code => {
            eprintln!("driver-installer exited with: {:?}", code);
            Ok(false)
        }
    }
}

/// Get the expected external installer path for display purposes
pub fn expected_path_display() -> String {
    external_installer_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| DRIVER_INSTALLER_NAME.to_string())
}
