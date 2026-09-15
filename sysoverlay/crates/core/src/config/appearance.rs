//! `appearance.cfg` —— 外观/文字渲染相关配置（颜色、字号、间距、阴影描边）。
//!
//! 颜色统一用 `#RRGGBB`（`ini::fmt_color` / `ini::parse_color`），
//! 取代旧 C 版 COLORREF(BBGGRR)。

use crate::config::ini::{fmt_color, parse_bool, parse_color, parse_u32, Ini, Writer};
use crate::config::{Config, FONT_PX_MAX, FONT_PX_MIN};

pub const APPEARANCE_FILE: &str = "appearance.cfg";

pub fn serialize(cfg: &Config) -> String {
    let mut w = Writer::new();
    w.section("meta");
    w.kv("version", crate::config::CONFIG_VERSION);
    w.section("appearance");
    w.kv("bg", fmt_color(cfg.bg));
    w.kv("fg", fmt_color(cfg.fg));
    w.kv("font_size", format!("{:.1}", cfg.font_size));
    w.kv("font_weight", cfg.font_weight);
    w.kv("metric_gap", cfg.metric_gap);
    w.kv("label_value_gap", cfg.label_value_gap);
    w.kv("shadow", cfg.text_shadow as u8);
    w.kv("outline", cfg.text_outline as u8);
    w.into_string()
}

pub fn parse(cfg: &mut Config, text: &str) {
    let ini = Ini::parse(text);
    let s = "appearance";

    // [meta] version（用于跨文件一致性校验）
    if let Some(v) = ini.get("meta", "version") {
        cfg.version = parse_u32(v, cfg.version);
    }

    if let Some(v) = ini.get(s, "bg") {
        if let Some(c) = parse_color(v) {
            cfg.bg = c;
        }
    }
    if let Some(v) = ini.get(s, "fg") {
        if let Some(c) = parse_color(v) {
            cfg.fg = c;
        }
    }
    if let Some(v) = ini.get(s, "font_size") {
        if let Ok(n) = v.trim().parse::<f32>() {
            // 0.5 步进钳位（与设置页滑杆共用区间）
            cfg.font_size = (n.clamp(FONT_PX_MIN, FONT_PX_MAX) * 2.0).round() / 2.0;
        }
    }
    if let Some(v) = ini.get(s, "font_weight") {
        cfg.font_weight = (parse_u32(v, cfg.font_weight as u32).clamp(0, 2)) as u8;
    }
    if let Some(v) = ini.get(s, "metric_gap") {
        cfg.metric_gap = parse_u32(v, cfg.metric_gap).clamp(0, 64);
    }
    if let Some(v) = ini.get(s, "label_value_gap") {
        cfg.label_value_gap = parse_u32(v, cfg.label_value_gap).clamp(0, 64);
    }
    if let Some(v) = ini.get(s, "shadow") {
        cfg.text_shadow = parse_bool(v, cfg.text_shadow);
    }
    if let Some(v) = ini.get(s, "outline") {
        cfg.text_outline = parse_bool(v, cfg.text_outline);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb;

    #[test]
    fn appearance_roundtrip() {
        let cfg = Config::default();
        let text = serialize(&cfg);
        assert!(text.contains("bg = #"));
        let mut c2 = Config::default();
        // 先改默认值，确保确实是解析覆盖而非默认巧合
        c2.bg = Rgb::new(1, 2, 3);
        c2.font_size = 40.0;
        parse(&mut c2, &text);
        assert_eq!(c2.bg, cfg.bg);
        assert_eq!(c2.fg, cfg.fg);
        assert_eq!(c2.font_size, cfg.font_size);
        assert_eq!(c2.metric_gap, cfg.metric_gap);
        assert_eq!(c2.text_shadow, cfg.text_shadow);
    }

    #[test]
    fn empty_color_is_ignored_gracefully() {
        let mut cfg = Config::default();
        parse(&mut cfg, "[appearance]\nbg = \nfg = #AABBCC\n");
        assert_eq!(cfg.bg, Config::default().bg);
        assert_eq!(cfg.fg, Rgb::new(0xAA, 0xBB, 0xCC));
    }
}
