//! 数值格式化（纯逻辑，零分配）。
//!
//! # 为什么不用 `format!`
//!
//! 实测（2026-09-04 `cargo bloat`）：exe 106 KiB 中 `std` 占了 `.text` 的 **58.7%**，
//! 而 `windows` crate 只有 50 B。也就是说体积的真正杠杆是 **减少 `std` 用量**，
//! 其中 `format!`/`String`/`Vec` 的格式化 machinery 是大头。
//!
//! 悬浮窗每秒要格式化十几个数字，全部走 `format!` 既堆分配又拖体积。
//! 本模块改为**栈上定长缓冲 + 手写定点数**，做到：
//! - 零堆分配（无 `String`）
//! - 零 `format!`
//! - 缓冲区溢出时截断（而不是 panic —— 短字符串截掉尾巴远好过崩溃）
//!
//! 容量 32 字节足够容纳本项目的任何显示串（最长约 `"1234.56M / 1234.56M"`）。

use crate::types::NetUnit;

/// 栈上字符串缓冲容量。定长而非泛型，避免单态化膨胀。
pub const STR_CAP: usize = 32;

/// 定长 UTF-8 缓冲。只写入 ASCII，因此内容恒为合法 UTF-8。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrBuf {
    data: [u8; STR_CAP],
    len: usize,
}

impl StrBuf {
    /// 创建空缓冲。
    pub const fn new() -> Self {
        Self {
            data: [0; STR_CAP],
            len: 0,
        }
    }

    /// 清空（保留容量）。
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// 当前内容。`StrBuf` 只写入 ASCII，故解析恒成功；
    /// 万一出现非法字节也只返回空串，绝不 panic。
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.data[..self.len]).unwrap_or("")
    }

    /// 当前字节数。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 写入单个字节。缓冲满则静默丢弃（截断优于 panic）。
    fn push_byte(&mut self, b: u8) {
        if self.len < STR_CAP {
            self.data[self.len] = b;
            self.len += 1;
        }
    }

    /// 写入 ASCII 字符串。
    pub fn push_str(&mut self, s: &str) {
        for b in s.as_bytes() {
            self.push_byte(*b);
        }
    }

    /// 写入无符号整数（十进制）。
    fn push_u64(&mut self, mut v: u64) {
        if v == 0 {
            self.push_byte(b'0');
            return;
        }
        // 先算出位数，再从高位写入，避免额外缓冲
        let mut digits = [0u8; 20];
        let mut n = 0;
        while v > 0 {
            digits[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            self.push_byte(digits[n]);
        }
    }

    /// 写入定点小数：`v` 保留 `decimals` 位（四舍五入）。
    ///
    /// 非有限值（NaN/Inf）写成 `--`，越界值饱和，均不 panic。
    fn push_fixed(&mut self, v: f64, decimals: u8) {
        if !v.is_finite() {
            self.push_str("--");
            return;
        }
        let mut v = v;
        if v < 0.0 {
            self.push_byte(b'-');
            v = -v;
        }
        let scale = pow10(decimals) as f64;
        // 饱和保护：避免 v*scale 溢出 u64
        let scaled = if v >= (u64::MAX as f64) / scale {
            u64::MAX
        } else {
            (v * scale + 0.5).floor() as u64
        };
        let int_part = scaled / pow10(decimals);
        let frac = scaled % pow10(decimals);

        self.push_u64(int_part);

        if decimals > 0 {
            self.push_byte(b'.');
            let mut div = pow10(decimals) / 10;
            while div >= 1 {
                self.push_byte(b'0' + ((frac / div) % 10) as u8);
                div /= 10;
            }
        }
    }
}

impl Default for StrBuf {
    fn default() -> Self {
        Self::new()
    }
}

/// 10 的幂（0..=9）。
fn pow10(n: u8) -> u64 {
    const P: [u64; 10] = [
        1,
        10,
        100,
        1_000,
        10_000,
        100_000,
        1_000_000,
        10_000_000,
        100_000_000,
        1_000_000_000,
    ];
    P[(n as usize).min(9)]
}

/// 格式化百分比，如 `"67%"`。
///
/// 与 C 版一致：整数百分比不带小数。
pub fn fmt_pct(buf: &mut StrBuf, pct: f32) {
    buf.clear();
    buf.push_fixed(pct as f64, 0);
    buf.push_byte(b'%');
}

/// 格式化带一位小数的百分比，如 `"24.7%"`。
///
/// 专用于综合占用（Util）：空闲时整数值长期不变（25%±0.3 显示都是 "25%"），
/// 单元格哈希不翻转 → 屏幕上看不到刷新。一位小数让 1.5s 提交节奏下的
/// 平滑抖动可见，负载尖峰的跳变直通也更精确（99.9 优于整数 100）。
pub fn fmt_pct1(buf: &mut StrBuf, pct: f32) {
    buf.clear();
    buf.push_fixed(pct as f64, 1);
    buf.push_byte(b'%');
}

/// 格式化网速，如 `"1.23M"` / `"856K"` / `"0.01G"`。
///
/// 小数位数沿用 C 版口径：KB 取整、GB 两位、MB 大于等于 100 取整否则两位。
pub fn fmt_rate(buf: &mut StrBuf, bytes_per_sec: u64, unit: NetUnit) {
    buf.clear();
    let v = unit.convert(bytes_per_sec);
    let decimals = match unit {
        NetUnit::KB => 0,
        NetUnit::GB => 2,
        NetUnit::MB => {
            if v >= 100.0 {
                0
            } else {
                2
            }
        }
    };
    buf.push_fixed(v, decimals);
    buf.push_byte(unit.suffix() as u8);
}

/// 格式化上下行速率对，如 `"1.23M / 0.23M"`。
pub fn fmt_rate_pair(buf: &mut StrBuf, rx_bps: u64, tx_bps: u64, unit: NetUnit) {
    buf.clear();
    // rx 与 tx 各用一个临时缓冲（栈上，零分配），再拼进目标缓冲，
    // 避免"先写后截断"这种容易写错的复用技巧。
    let mut rx = StrBuf::new();
    let mut tx = StrBuf::new();
    fmt_rate(&mut rx, rx_bps, unit);
    fmt_rate(&mut tx, tx_bps, unit);

    buf.push_str(rx.as_str());
    buf.push_str(" / ");
    buf.push_str(tx.as_str());
}

/// 格式化内存占用，如 `"8.5/16.0G"`。
pub fn fmt_mem(buf: &mut StrBuf, used_gb: f32, total_gb: f32) {
    buf.clear();
    buf.push_fixed(used_gb as f64, 1);
    buf.push_byte(b'/');
    buf.push_fixed(total_gb as f64, 1);
    buf.push_byte(b'G');
}

/// 格式化内存实际占用量（不带总量），如 `"8.5GB"`。
pub fn fmt_gb(buf: &mut StrBuf, used_gb: f32) {
    buf.clear();
    buf.push_fixed(used_gb as f64, 1);
    buf.push_str("GB");
}

/// 格式化运行时长。
///
/// 从大到小只保留两个粒度，保证短串：`"45s"` / `"12m"` / `"5h12m"` / `"3d4h"`。
pub fn fmt_uptime(buf: &mut StrBuf, uptime_ms: u64) {
    buf.clear();
    let secs = uptime_ms / 1000;
    if secs < 60 {
        buf.push_u64(secs);
        buf.push_byte(b's');
    } else if secs < 3600 {
        buf.push_u64(secs / 60);
        buf.push_byte(b'm');
    } else if secs < 86_400 {
        buf.push_u64(secs / 3600);
        buf.push_byte(b'h');
        buf.push_u64((secs % 3600) / 60);
        buf.push_byte(b'm');
    } else {
        buf.push_u64(secs / 86_400);
        buf.push_byte(b'd');
        buf.push_u64((secs % 86_400) / 3600);
        buf.push_byte(b'h');
    }
}

/// 格式化功耗（瓦），如 `"45W"`。
///
/// `estimated = true` 时加 **`~` 前缀** —— v2 Q16 要求估算值必须诚实标注，
/// 不能冒充实测值。（功耗关闭时根本不调用本函数。）
pub fn fmt_power(buf: &mut StrBuf, watts: f32, estimated: bool) {
    buf.clear();
    if estimated {
        buf.push_byte(b'~');
    }
    buf.push_fixed(watts as f64, 0);
    buf.push_byte(b'W');
}

/// 给数值加 `~` 前缀（陈旧/估算的通用标注）。
///
/// v2 Q16：Eco 模式无中文字形，统一用 ASCII 标记：
/// `~` = 估算值、降饱和 = 陈旧值、隐藏整行 = 不可用。
pub fn with_estimate_prefix(buf: &mut StrBuf, value: f32, decimals: u8) {
    buf.clear();
    buf.push_byte(b'~');
    buf.push_fixed(value as f64, decimals);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(b: &StrBuf) -> &str {
        b.as_str()
    }

    #[test]
    fn buf_push_and_clear() {
        let mut b = StrBuf::new();
        assert!(b.is_empty());
        b.push_str("hello");
        assert_eq!(s(&b), "hello");
        assert_eq!(b.len(), 5);
        b.clear();
        assert!(b.is_empty());
        assert_eq!(s(&b), "");
    }

    #[test]
    fn buf_truncates_instead_of_panicking() {
        let mut b = StrBuf::new();
        for _ in 0..100 {
            b.push_str("abcdefghij");
        }
        assert_eq!(b.len(), STR_CAP, "必须截断到容量，不能越界或 panic");
        assert!(b.as_str().len() == STR_CAP);
    }

    #[test]
    fn u64_digits() {
        let mut b = StrBuf::new();
        b.push_u64(0);
        assert_eq!(s(&b), "0");
        b.clear();
        b.push_u64(12345);
        assert_eq!(s(&b), "12345");
        b.clear();
        b.push_u64(u64::MAX);
        assert_eq!(s(&b), "18446744073709551615");
    }

    #[test]
    fn pct_formatting() {
        let mut b = StrBuf::new();
        fmt_pct(&mut b, 67.5);
        assert_eq!(s(&b), "68%", "四舍五入到整数百分比");
        fmt_pct(&mut b, 0.0);
        assert_eq!(s(&b), "0%");
        fmt_pct(&mut b, 100.0);
        assert_eq!(s(&b), "100%");
    }

    #[test]
    fn rate_formatting_per_unit() {
        let mut b = StrBuf::new();

        // MB：1_234_567 B/s → 1.23M
        fmt_rate(&mut b, 1_234_567, NetUnit::MB);
        assert_eq!(s(&b), "1.23M");

        // MB：大流量取整
        fmt_rate(&mut b, 234_000_000, NetUnit::MB);
        assert_eq!(s(&b), "234M");

        // KB
        fmt_rate(&mut b, 1_234_567, NetUnit::KB);
        assert_eq!(s(&b), "1235K");

        // GB
        fmt_rate(&mut b, 1_234_567_000, NetUnit::GB);
        assert_eq!(s(&b), "1.23G");

        // 零流量
        fmt_rate(&mut b, 0, NetUnit::MB);
        assert_eq!(s(&b), "0.00M");
    }

    #[test]
    fn rate_pair_formatting() {
        let mut b = StrBuf::new();
        fmt_rate_pair(&mut b, 1_234_567, 234_567, NetUnit::MB);
        assert_eq!(s(&b), "1.23M / 0.23M");
    }

    #[test]
    fn mem_formatting() {
        let mut b = StrBuf::new();
        fmt_mem(&mut b, 8.52, 16.0);
        assert_eq!(s(&b), "8.5/16.0G");
    }

    #[test]
    fn uptime_formatting() {
        let mut b = StrBuf::new();

        fmt_uptime(&mut b, 45_000);
        assert_eq!(s(&b), "45s");

        fmt_uptime(&mut b, 12 * 60_000);
        assert_eq!(s(&b), "12m");

        fmt_uptime(&mut b, (5 * 3600 + 12 * 60) * 1000);
        assert_eq!(s(&b), "5h12m");

        fmt_uptime(&mut b, (3 * 86_400 + 4 * 3600) * 1000);
        assert_eq!(s(&b), "3d4h");
    }

    #[test]
    fn power_marks_estimate_with_tilde() {
        let mut b = StrBuf::new();
        fmt_power(&mut b, 45.4, false);
        assert_eq!(s(&b), "45W", "实测值不带前缀");
        fmt_power(&mut b, 45.4, true);
        assert_eq!(s(&b), "~45W", "估算值必须带 ~ 前缀");
    }

    #[test]
    fn non_finite_never_panics() {
        let mut b = StrBuf::new();
        b.push_fixed(f64::NAN, 2);
        assert_eq!(s(&b), "--");
        b.clear();
        b.push_fixed(f64::INFINITY, 0);
        assert_eq!(s(&b), "--");
        b.clear();
        b.push_fixed(-1.5, 1);
        assert_eq!(s(&b), "-1.5");
    }

    #[test]
    fn huge_values_saturate() {
        let mut b = StrBuf::new();
        b.push_fixed(1e30, 2);
        assert!(!b.is_empty());
        assert!(b.as_str().chars().all(|c| c.is_ascii_digit() || c == '.'));
    }

    #[test]
    fn pow10_table() {
        assert_eq!(pow10(0), 1);
        assert_eq!(pow10(3), 1_000);
        assert_eq!(pow10(9), 1_000_000_000);
        assert_eq!(pow10(99), 1_000_000_000, "越界夹紧到最大档");
    }
}
