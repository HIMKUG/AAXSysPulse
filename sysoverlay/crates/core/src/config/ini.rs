//! 通用 INI 风格（[section] + `key = value`）读写辅助。
//!
//! 设计目标：让 5 个 `.cfg` 文件**人类可读、可手工编辑、可整段复制分享**。
//! - 解析：宽容——未知 section / 未知 key / 空行 / `#` `;` 注释一律忽略，绝不 panic；
//!   单个坏行不影响其它行（满足「解析失败回退默认不崩溃」）。
//! - 写入：无分配压力的低频操作，直接拼 `String`。
//!
//! 颜色统一用 `#RRGGBB`（`Rgb::to_hex` / `Rgb::from_hex`），
//! 与旧 C 版 COLORREF(BBGGRR) 彻底解耦，肉眼即可校对。

use std::collections::HashMap;

use crate::color::Rgb;

/// 把 `Rgb` 写成 `#RRGGBB`（大写）。
pub fn fmt_color(c: Rgb) -> String {
    let h = c.to_hex();
    format!("#{}{}{}", 
        std::str::from_utf8(&h[0..2]).unwrap_or("00"),
        std::str::from_utf8(&h[2..4]).unwrap_or("00"),
        std::str::from_utf8(&h[4..6]).unwrap_or("00"))
}

/// 解析 `#RRGGBB` / `RRGGBB`（空白或缺失返回 `None`）。
pub fn parse_color(s: &str) -> Option<Rgb> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    Rgb::from_hex(s)
}

/// 宽容布尔解析：接受 `1/0/true/false/yes/no/on/off`，失败时回退 `default`。
pub fn parse_bool(s: &str, default: bool) -> bool {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => default,
    }
}

/// 解析 32 位无符号整数，失败回退 `default`。
pub fn parse_u32(s: &str, default: u32) -> u32 {
    s.trim().parse::<u32>().unwrap_or(default)
}

/// 解析有符号 32 位整数，失败回退 `default`。
pub fn parse_i32(s: &str, default: i32) -> i32 {
    s.trim().parse::<i32>().unwrap_or(default)
}

/// 已解析的 INI 文档：`section(小写) -> (key(小写) -> value)`。
pub struct Ini {
    sections: HashMap<String, HashMap<String, String>>,
}

impl Ini {
    /// 解析 INI 文本。所有 key / section 名在存储时转小写，便于大小写不敏感查找。
    pub fn parse(text: &str) -> Ini {
        let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut cur: String = String::new(); // "" = 段前（无 section 头）
        let mut cur_map: HashMap<String, String> = HashMap::new();

        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            // [section]
            if let Some(rest) = line.strip_prefix('[') {
                if let Some(sec) = rest.strip_suffix(']') {
                    // 逐 key 合并，而非整段替换：同名 section 第二次出现时（手改配置
                    // 复制粘贴很容易留下两个 [monitor]），旧实现用 mem::replace 把
                    // 前一段**整段丢弃** → 静默丢键。后写覆盖先写才是 INI 惯例。
                    sections
                        .entry(std::mem::take(&mut cur))
                        .or_default()
                        .extend(cur_map.drain());
                    cur = sec.trim().to_ascii_lowercase();
                    continue;
                }
            }
            // key = value（也容忍 "key value" 无等号，增强手工编辑容错）
            let (k, v) = if let Some((k, v)) = line.split_once('=') {
                (k.trim().to_ascii_lowercase(), v.trim().to_string())
            } else if let Some((k, v)) = line.split_once(char::is_whitespace) {
                (k.trim().to_ascii_lowercase(), v.trim().to_string())
            } else {
                continue;
            };
            if !k.is_empty() {
                cur_map.insert(k, v);
            }
        }
        sections.insert(cur, cur_map);
        Ini { sections }
    }

    /// 取某 section（小写名）。不存在返回 `None`。
    pub fn section(&self, name: &str) -> Option<&HashMap<String, String>> {
        self.sections.get(&name.to_ascii_lowercase())
    }

    /// 取 `section.key`（均大小写不敏感）。
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.section(section)
            .and_then(|m| m.get(&key.to_ascii_lowercase()))
            .map(|s| s.as_str())
    }
}

/// 序列化辅助：累积 `key = value` 行，自动加段头与注释。
pub struct Writer {
    buf: String,
}

impl Writer {
    pub fn new() -> Self {
        Writer {
            buf: String::from("# AAXSysPulse configuration — human-readable, safe to edit by hand.\n"),
        }
    }

    /// 开始一个新 section（自动前置空行）。
    pub fn section(&mut self, name: &str) {
        self.buf.push('\n');
        self.buf.push('[');
        self.buf.push_str(name);
        self.buf.push_str("]\n");
    }

    /// 写一行注释。
    pub fn comment(&mut self, c: &str) {
        self.buf.push_str("# ");
        self.buf.push_str(c);
        self.buf.push('\n');
    }

    /// 写一个 `key = value`（value 需实现 `Display`）。
    pub fn kv(&mut self, k: &str, v: impl std::fmt::Display) {
        self.buf.push_str(k);
        self.buf.push_str(" = ");
        self.buf.push_str(&v.to_string());
        self.buf.push('\n');
    }

    /// 写一行原始文本（已含换行由调用方负责）。
    pub fn raw(&mut self, line: &str) {
        self.buf.push_str(line);
        if !line.ends_with('\n') {
            self.buf.push('\n');
        }
    }

    pub fn into_string(self) -> String {
        self.buf
    }
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_hex_roundtrip() {
        let c = Rgb::new(0x1b, 0x16, 0x14);
        assert_eq!(fmt_color(c), "#1B1614");
        assert_eq!(parse_color("#1B1614"), Some(c));
        assert_eq!(parse_color("1B1614"), Some(c));
        assert_eq!(parse_color(""), None);
    }

    #[test]
    fn ini_roundtrip_basic() {
        let text = "[system]\nlayout = vertical\nzoom = 100\n\n[appearance]\n# comment\nbg = #000000\n";
        let ini = Ini::parse(text);
        assert_eq!(ini.get("system", "layout"), Some("vertical"));
        assert_eq!(ini.get("SYSTEM", "ZOOM"), Some("100")); // 大小写不敏感
        assert_eq!(ini.get("appearance", "bg"), Some("#000000"));
        assert_eq!(ini.get("nope", "x"), None);
    }

    #[test]
    fn ini_ignores_garbage() {
        let ini = Ini::parse("\n# hi\nnot-a-key\n[sec]\nbad line no equals\nkey = val\n");
        assert_eq!(ini.get("sec", "key"), Some("val"));
    }
}
