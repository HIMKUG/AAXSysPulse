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

/// MSR 驱动服务名：契约常量，引导 `driver.rs` 建的同一个服务（7.1.2 / P1-22）。
const SERVICE_NAME: &str = aaxsyspulse_core::contract::DRIVER_SERVICE_NAME;

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

/// 预检结果。
///
/// 旧实现只用 `bool` 表示"是否继续"，调用方却把它当成"配置是否有改动"用，
/// 导致「点了取消也不退出」（弹窗承诺的 "Click Cancel to exit" 落空）。
/// 两个语义拆开，各归各。
pub struct Outcome {
    /// 是否继续启动（`false` = 用户选了取消，应退出）。
    pub proceed: bool,
    /// 预检是否改动了配置（继续计数 / 上次检查日期）→ 调用方需落盘。
    pub changed: bool,
}

/// Run the pre-launch check sequence.
pub fn run_pre_launch_checks(cfg: &mut aaxsyspulse_core::config::Config) -> Outcome {
    let mut changed = false;
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
            let title = "AAXSysPulse - Administrator Privileges Required";
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
                changed = true;
                // Proceed to check 2
            } else {
                // User clicked Cancel or closed dialog
                return Outcome { proceed: false, changed: true };
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
            let title = "AAXSysPulse - Driver Not Loaded";
            let message = "[MSR Driver Not Loaded]\n\n\
                           AAXSysPulse WinRing0 MSR driver is not installed or not running.\n\
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
                changed = true;
                // Proceed to main app
            } else {
                // User clicked Cancel or closed dialog
                return Outcome { proceed: false, changed: true };
            }
        }
    }

    Outcome { proceed: true, changed }
}

/// Convert YYYYMMDD date to milliseconds since UNIX_EPOCH (approximate)
fn date_to_ms(date: u32) -> u64 {
    if date == 0 {
        return u64::MAX; // Never checked
    }

    let year = (date / 10000) as u64;
    let month = ((date / 100) % 100) as u64;
    let day = (date % 100) as u64;

    // 形状闸门（批次 7·⑪顺手修）：`date` 是从 `system.cfg` 直接 `parse_u32` 出来的
    // 自由值（`core/src/config/system.rs:218`），手改或半写坏的文件能给出
    // `4294967295` 这类东西。原实现在那里 panic 两次：`month > 12` 时
    // `days_in_months[m - 1]` 越界，`day == 0` 时 `day - 1` 在 debug 下下溢 panic。
    // 而这两处都在**窗口出现之前**的启动自检里 ⇒ 用户只看到"双击 exe 没反应"。
    // 不认识形状的值按"从未检查过"处理（与 `date == 0` 同一语义，宁多弹一次不崩）。
    if !(1970..=9999).contains(&year) || !(1..=12).contains(&month) || day == 0 {
        return u64::MAX;
    }

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

#[cfg(test)]
mod tests {
    //! 审查项 P2-13 收尾：`crates/app` 此前**零测试**。
    //! 这里只覆盖四个纯函数（`current_date` / `is_leap_year` / `time_since` /
    //! `date_to_ms`）—— 它们的输出只取决于入参与系统时钟，不需要弹窗、不需要
    //! SCM、不需要真驱动。四个检查闸门（每周一次 + 连点 3 次跳过）的正确性全部
    //! 建立在这几个函数的换算上，而换算错成什么样都是"该弹的不弹、不该弹的天天弹"，
    //! 属静默错误，所以值得钉住。

    use super::*;

    const DAY_MS: u64 = 24 * 60 * 60 * 1000;

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    #[test]
    fn leap_year_follows_gregorian_rule() {
        for y in [1972u32, 1996, 2000, 2024, 2028, 2400] {
            assert!(is_leap_year(y), "{y} 应为闰年");
        }
        for y in [1900u32, 2100, 2200, 2300, 1999, 2026] {
            assert!(!is_leap_year(y), "{y} 应为平年");
        }
    }

    /// 锚点：UTC 纪元日的毫秒数写死，防止两处日期换算一起漂移。
    #[test]
    fn date_to_ms_matches_epoch_anchors() {
        assert_eq!(date_to_ms(19700101), 0);
        assert_eq!(date_to_ms(20240101), 1_704_067_200_000);
        assert_eq!(date_to_ms(20260101), 1_767_225_600_000);
    }

    #[test]
    fn date_to_ms_steps_one_day_across_month_and_year_edges() {
        for (a, b) in [
            (20260101u32, 20260102),
            (20260131, 20260201),
            (20240228, 20240229), // 闰年才有 2/29
            (20240229, 20240301),
            (20230228, 20230301), // 平年直接 3/1
            (20251231, 20260101),
        ] {
            assert_eq!(
                date_to_ms(b).saturating_sub(date_to_ms(a)),
                DAY_MS,
                "{a} → {b} 应该正好差一天"
            );
        }
    }

    /// 形状闸门（本批顺手修的部分）：`*_check_last_date` 是配置文件里的自由 u32，
    /// 坏值**必须**降级成"从未检查过"，而不是在启动自检里 panic。
    #[test]
    fn malformed_dates_degrade_to_never_checked_instead_of_panicking() {
        for bad in [
            0u32,
            1,
            19691231,     // 早于纪元
            20260100,     // day = 0 → 旧实现 `day - 1` 下溢
            20261301,     // month = 13 → 旧实现数组越界
            20260001,     // month = 0
            99999999,
            u32::MAX, // 4294967295 → year 42 万、month 72
        ] {
            assert_eq!(
                date_to_ms(bad),
                u64::MAX,
                "{bad:#08} 不是合法 YYYYMMDD，应返回 u64::MAX（= 从未检查过）"
            );
        }
    }

    /// `u64::MAX` 的配套语义：`time_since` 走 saturating_sub ⇒ 恒为 0 ⇒
    /// "是否满一周" 恒为 false。首跑之所以仍会弹一次，靠的是调用点那条
    /// `last_date == 0 ||` 短路 —— 两条一起才成立，故同测钉住。
    #[test]
    fn never_checked_reads_as_zero_elapsed() {
        assert_eq!(time_since(u64::MAX), 0);
        assert!(time_since(u64::MAX) < WEEK_MS, "u64::MAX 不能被当成'很久以前'");
    }

    #[test]
    fn current_date_is_a_plausible_yyyymmdd() {
        let d = current_date();
        let (y, m, day) = (d / 10000, (d / 100) % 100, d % 100);
        assert!((2026..=2100).contains(&y), "年份 out of range: {d}");
        assert!((1..=12).contains(&m), "月份 out of range: {d}");
        assert!((1..=31).contains(&day), "日 out of range: {d}");
    }

    /// 两条独立实现的换算必须互相吻合：`current_date()` 推出的今天，
    /// 经 `date_to_ms` 折回毫秒后应落在 [今天 0 点, 现在]，且不超过一天。
    #[test]
    fn current_date_round_trips_into_today_utc_midnight() {
        let midnight = date_to_ms(current_date());
        let now = now_ms();
        assert!(midnight <= now, "今天 0 点不可能在未来：{midnight} > {now}");
        assert!(
            now - midnight < DAY_MS,
            "0 点与现在相差超过一天 ⇒ 某处换算错了一天：{}",
            now - midnight
        );
    }

    #[test]
    fn week_gate_ignores_clock_skew_into_the_past() {
        // 系统时钟被往回调过（或配置来自更新的机器）时 timestamp 会大于 now；
        // saturating_sub 必须给 0 而不是环绕成一个巨大的数（那会让闸门永远不开）。
        assert_eq!(time_since(now_ms() + 10 * WEEK_MS), 0);
        let week_ago = now_ms().saturating_sub(WEEK_MS);
        assert!(time_since(week_ago) >= WEEK_MS - 5_000, "刚好一周前应放行");
    }
}
