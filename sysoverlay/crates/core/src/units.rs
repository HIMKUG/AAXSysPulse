//! Units formatting module for SysOverlay.
//!
//! Provides formatting functions for network speeds, memory, percentages, and uptime.

use crate::config::NetUnit;

/// Formats a network speed in bits per second to a human-readable string.
pub fn format_rate(bps: u64, unit: NetUnit) -> String {
    let (value, unit_str) = convert_rate(bps, unit);
    format!("{:.2} {}/s", value, unit_str)
}

/// Formats a network speed in a shorter single-unit format.
pub fn format_rate_short(bps: u64, unit: NetUnit) -> String {
    let (value, unit_str) = convert_rate(bps, unit);
    if value >= 100.0 {
        format!("{:.0}{}", value, &unit_str[0..1])
    } else if value >= 10.0 {
        format!("{:.1}{}", value, &unit_str[0..1])
    } else {
        format!("{:.2}{}", value, &unit_str[0..1])
    }
}

/// Formats bytes as gigabytes for memory display.
pub fn format_bytes_gb(gb: f32) -> String {
    if gb >= 100.0 {
        format!("{:.0} GB", gb)
    } else if gb >= 10.0 {
        format!("{:.1} GB", gb)
    } else {
        format!("{:.2} GB", gb)
    }
}

/// Formats a percentage value.
pub fn format_pct(pct: f32) -> String {
    format!("{:.0}%", pct)
}

/// Formats uptime in seconds to a human-readable string.
pub fn format_uptime(sec: f64) -> String {
    if sec < 0.0 {
        return String::from("0:00");
    }

    let total_minutes = (sec / 60.0) as u64;
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;

    if hours >= 24 {
        let days = hours / 24;
        let remaining_hours = hours % 24;
        format!("{}天 {:02}:{:02}", days, remaining_hours, minutes)
    } else {
        format!("{:02}:{:02}", hours, minutes)
    }
}

fn convert_rate(bps: u64, unit: NetUnit) -> (f64, &'static str) {
    let bps = bps as f64;
    let bps = bps / 8.0;

    match unit {
        NetUnit::KB => {
            let kb = bps / 1024.0;
            (kb, "KB")
        }
        NetUnit::MB => {
            let mb = bps / (1024.0 * 1024.0);
            (mb, "MB")
        }
        NetUnit::GB => {
            let gb = bps / (1024.0 * 1024.0 * 1024.0);
            (gb, "GB")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_rate() {
        assert_eq!(format_rate(8_000_000, NetUnit::MB), "1.00 MB/s");
        assert_eq!(format_rate(12_000_000, NetUnit::MB), "1.50 MB/s");
        assert_eq!(format_rate(102_400, NetUnit::KB), "12.50 KB/s");
    }

    #[test]
    fn test_format_rate_short() {
        assert_eq!(format_rate_short(12_000_000, NetUnit::MB), "1.50M");
        assert_eq!(format_rate_short(8_000_000, NetUnit::MB), "1.00M");
        assert_eq!(format_rate_short(102_400, NetUnit::KB), "12.5K");
        assert_eq!(format_rate_short(1_200_000_000, NetUnit::MB), "143M");
    }

    #[test]
    fn test_format_bytes_gb() {
        assert_eq!(format_bytes_gb(8.52), "8.52 GB");
        assert_eq!(format_bytes_gb(16.0), "16.00 GB");
        assert_eq!(format_bytes_gb(128.5), "128.5 GB");
        assert_eq!(format_bytes_gb(256.0), "256 GB");
    }

    #[test]
    fn test_format_pct() {
        assert_eq!(format_pct(87.5), "88%");
        assert_eq!(format_pct(100.0), "100%");
        assert_eq!(format_pct(0.0), "0%");
        assert_eq!(format_pct(50.6), "51%");
    }

    #[test]
    fn test_format_uptime() {
        assert_eq!(format_uptime(870.0), "00:14");
        assert_eq!(format_uptime(3600.0), "01:00");
        assert_eq!(format_uptime(14.0 * 3600.0 + 30.0 * 60.0), "14:30");
        assert_eq!(format_uptime(25.0 * 3600.0), "1天 01:00");
        assert_eq!(format_uptime(2.5 * 24.0 * 3600.0), "2天 12:00");
    }

    #[test]
    fn test_format_uptime_negative() {
        assert_eq!(format_uptime(-100.0), "0:00");
    }
}
