//! Pre-launch checks for admin privileges and driver loading.
//!
//! Shows native Windows dialogs to prompt users about:
//! 1. Admin privileges (required for MSR driver installation)
//! 2. WinRing0x64.sys driver loading status
//!
//! Each check has a "continue count" - after 3 continue clicks, the check
//! is permanently skipped. Each check is also only shown once per week.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Services::{OpenSCManagerW, OpenServiceW, SC_MANAGER_ENUMERATE_SERVICE, SERVICE_QUERY_STATUS};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONWARNING, MB_OKCANCEL, MB_SETFOREGROUND, IDOK,
};

/// Service name for the MSR driver (must match msr.rs)
const SERVICE_NAME: &str = "SysOverlayWinRing0";

/// Number of continue clicks before permanently skipping a check
const CONTINUE_LIMIT: u8 = 3;

/// Milliseconds per week (7 days * 24 hours * 60 minutes * 60 seconds * 1000)
const WEEK_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// Get current date as YYYYMMDD format
fn current_date() -> u32 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();

    // Calculate date components
    let days_since_1970 = secs / 86400;
    let mut year = 1970u32;
    let mut remaining_days = days_since_1970;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let is_leap = is_leap_year(year);
    let days_in_months = if is_leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1u32;
    for days_in_month in days_in_months.iter() {
        if remaining_days < *days_in_month as u64 {
            break;
        }
        remaining_days -= *days_in_month as u64;
        month += 1;
    }

    let day = remaining_days as u32 + 1;

    year * 10000 + month * 100 + day
}

fn is_leap_year(year: u32) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

/// Get elapsed time since last check in milliseconds
fn time_since(timestamp_ms: u64) -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    now.saturating_sub(timestamp_ms)
}

/// Check if running with administrator privileges
pub fn is_admin() -> bool {
    unsafe {
        use windows::Win32::UI::Shell::IsUserAnAdmin;
        IsUserAnAdmin().as_bool()
    }
}

/// Check if the MSR driver service is loaded in SCM
pub fn is_driver_loaded() -> bool {
    unsafe {
        let scm = match OpenSCManagerW(None, None, SC_MANAGER_ENUMERATE_SERVICE) {
            Ok(h) => h,
            Err(_) => return false,
        };

        let svc_name: Vec<u16> = SERVICE_NAME.encode_utf16().chain(Some(0)).collect();
        let svc = OpenServiceW(scm, windows::core::PCWSTR(svc_name.as_ptr()), SERVICE_QUERY_STATUS);
        if svc.is_ok() {
            let _ = windows::Win32::Foundation::CloseHandle(HANDLE(scm.0 as _));
            return true;
        }

        let _ = windows::Win32::Foundation::CloseHandle(HANDLE(scm.0 as _));
        false
    }
}

/// Show a native Windows message box
fn show_message_box(title: &str, message: &str, flags: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE) -> windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_RESULT {
    let title_wide: Vec<u16> = OsStr::new(title)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let message_wide: Vec<u16> = OsStr::new(message)
        .encode_wide()
        .chain(Some(0))
        .collect();

    unsafe {
        MessageBoxW(
            None,
            windows::core::PCWSTR(message_wide.as_ptr()),
            windows::core::PCWSTR(title_wide.as_ptr()),
            MB_ICONWARNING | flags | MB_SETFOREGROUND,
        )
    }
}

/// Run the pre-launch check sequence
///
/// Returns `true` if the application should continue launching,
/// `false` if the user chose to exit.
pub fn run_pre_launch_checks(cfg: &mut sysoverlay_core::config::Config) -> bool {
    // Convert last check dates to milliseconds for comparison
    let admin_last_ms = date_to_ms(cfg.admin_check_last_date);
    let driver_last_ms = date_to_ms(cfg.driver_check_last_date);

    let today = current_date();
    let week_passed_admin = time_since(admin_last_ms) >= WEEK_MS;
    let week_passed_driver = time_since(driver_last_ms) >= WEEK_MS;

    // Check 1: Admin privileges
    if !is_admin() {
        // Check if we should show the dialog
        let should_check = cfg.admin_check_continue_count < CONTINUE_LIMIT
            && (cfg.admin_check_last_date == 0 || week_passed_admin);

        if should_check {
            let title = "SysOverlay - Administrator Privileges Required";
            let message = "[Admin Privileges Required]\n\n\
                           Administrator privileges are required for power monitoring functionality.\n\
                           Without admin rights, power monitoring will be disabled.\n\n\
                           Click Continue to launch anyway (power monitoring will be unavailable).\n\
                           Click Cancel to exit.\n\n\
                           Note: This warning will appear once per week.\n\
                           After 3 continue clicks, this warning will no longer appear.";

            let ret = show_message_box(title, message, MB_OKCANCEL);

            if ret.0 == IDOK.0 {
                // User clicked Continue
                cfg.admin_check_continue_count += 1;
                cfg.admin_check_last_date = today;
                // Proceed to check 2
            } else {
                // User clicked Cancel or closed dialog
                return false;
            }
        }
    }

    // Check 2: Driver loaded
    // 功耗查询关闭（power_enabled/power_query 任一关）时完全不读 MSR，
    // 无需驱动 → 跳过每周一次的驱动检查弹窗，不打扰用户。
    if cfg.power_enabled && cfg.power_query && !is_driver_loaded() {
        // Check if we should show the dialog
        let should_check = cfg.driver_check_continue_count < CONTINUE_LIMIT
            && (cfg.driver_check_last_date == 0 || week_passed_driver);

        if should_check {
            let title = "SysOverlay - Driver Not Loaded";
            let message = "[MSR Driver Not Loaded]\n\n\
                           SysOverlay WinRing0 MSR driver is not installed or not running.\n\
                           Power monitoring will be unavailable until the driver is installed.\n\n\
                           Click Continue to launch anyway (power monitoring will be disabled).\n\
                           Click Cancel to exit.\n\n\
                           Note: This warning will appear once per week.\n\
                           After 3 continue clicks, this warning will no longer appear.";

            let ret = show_message_box(title, message, MB_OKCANCEL);

            if ret.0 == IDOK.0 {
                // User clicked Continue
                cfg.driver_check_continue_count += 1;
                cfg.driver_check_last_date = today;
                // Proceed to main app
            } else {
                // User clicked Cancel or closed dialog
                return false;
            }
        }
    }

    true
}

/// Convert YYYYMMDD date to milliseconds since UNIX_EPOCH (approximate)
fn date_to_ms(date: u32) -> u64 {
    if date == 0 {
        return u64::MAX; // Never checked
    }

    let year = (date / 10000) as u64;
    let month = ((date / 100) % 100) as u64;
    let day = (date % 100) as u64;

    // Simplified: calculate days since 1970-01-01
    let mut days = 0u64;

    for y in 1970..year {
        days += if is_leap_year(y as u32) { 366 } else { 365 };
    }

    let days_in_months = if is_leap_year(year as u32) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    for m in 1..month {
        days += days_in_months[(m - 1) as usize] as u64;
    }

    days += day - 1;

    days * 24 * 60 * 60 * 1000
}
