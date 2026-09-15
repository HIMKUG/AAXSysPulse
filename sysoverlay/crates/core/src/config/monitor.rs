//! `monitor.cfg` —— 监控项相关配置。
//!
//! 按用户拍板的「数字 ID 顺延」约定，监控项以 **1-based 数字 ID** 在配置里寻址：
//! ```text
//! 1=cpu 2=mem 3=gpu 4=net 5=power 6=time 7=util  （新增从 8 起）
//! ```
//! 内部 [`Config`] 仍用 0-based 下标（`Metric::ALL` 序），读写时按 `id = index + 1` 转换。
//!
//! - `[monitor]`：显示顺序 `order`（ID 列表）、整屏级开关、时间项开关、简化名等。
//! - `[item.{id}]`：逐指标的「整项显隐 `show` / 名称显隐 `name` / 字号 `font` /
//!   名称色 `color_l` / 数值色 `color_v` / 数值自动色 `auto`」。
//! - `[power]`：功耗行的查询/显示语义/间隔/虚拟网卡排除。
//!
//! 整项显隐（`show`）对应旧 `show_*`；名称显隐（`name`）对应旧 `metric_show_labels`。

use crate::config::ini::{fmt_color, parse_bool, parse_color, parse_u32, Ini, Writer};
use crate::config::{Config, FONT_PX_MAX, FONT_PX_MIN};
use crate::types::PowerDisplay;

pub const MONITOR_FILE: &str = "monitor.cfg";

fn id_to_idx(id: u32) -> Option<usize> {
    if (1..=7).contains(&id) {
        Some((id - 1) as usize)
    } else {
        None
    }
}

/// 7 个监控项「整项显隐」与数字 ID 的对齐（下标 = id-1）。
fn visibility(cfg: &Config) -> [bool; 7] {
    [
        cfg.show_cpu,
        cfg.show_mem,
        cfg.show_gpu,
        cfg.show_net,
        cfg.power_enabled,
        cfg.show_time,
        cfg.show_util,
    ]
}

pub fn serialize(cfg: &Config) -> String {
    let mut w = Writer::new();

    w.section("meta");
    w.kv("version", crate::config::CONFIG_VERSION);
    w.section("monitor");
    w.kv("util_decimal", cfg.util_decimal as u8);
    w.kv("mem_gb", cfg.mem_show_gb as u8);
    w.kv("short_names", cfg.short_names as u8);
    w.kv("show_time", cfg.show_time as u8);
    w.kv("time_seconds", cfg.time_show_seconds as u8);

    // 逐监控项定义（按 ID 1..7）
    let vis = visibility(cfg);
    for id in 1..=7u32 {
        let idx = (id - 1) as usize;
        w.section(&format!("item.{}", id));
        w.kv("show", vis[idx] as u8);
        w.kv("name", cfg.metric_show_labels[idx] as u8);
        match cfg.metric_font_sizes[idx] {
            Some(f) => w.kv("font", format!("{:.1}", f)),
            None => w.kv("font", 0),
        }
        match cfg.metric_colors[idx].label {
            Some(c) => w.kv("color_l", fmt_color(c)),
            None => w.kv("color_l", ""),
        }
        match cfg.metric_colors[idx].value {
            Some(c) => w.kv("color_v", fmt_color(c)),
            None => w.kv("color_v", ""),
        }
        w.kv("auto", cfg.metric_colors[idx].value_auto as u8);
    }

    // 功耗行
    w.section("power");
    w.kv("enabled", cfg.power_enabled as u8);
    w.kv("query", cfg.power_query as u8);
    w.kv("display", cfg.power_display.to_config());
    w.kv("interval_ms", cfg.power_interval_ms);
    w.kv("exclude_virtual_nic", cfg.exclude_virtual_nic as u8);

    w.into_string()
}

pub fn parse(cfg: &mut Config, text: &str) {
    let ini = Ini::parse(text);
    let s = "monitor";

    // [meta] version（用于跨文件一致性校验）
    if let Some(v) = ini.get("meta", "version") {
        cfg.version = parse_u32(v, cfg.version);
    }

    if let Some(v) = ini.get(s, "util_decimal") {
        cfg.util_decimal = parse_bool(v, cfg.util_decimal);
    }
    if let Some(v) = ini.get(s, "mem_gb") {
        cfg.mem_show_gb = parse_bool(v, cfg.mem_show_gb);
    }
    if let Some(v) = ini.get(s, "short_names") {
        cfg.short_names = parse_bool(v, cfg.short_names);
    }
    if let Some(v) = ini.get(s, "show_time") {
        cfg.show_time = parse_bool(v, cfg.show_time);
    }
    if let Some(v) = ini.get(s, "time_seconds") {
        cfg.time_show_seconds = parse_bool(v, cfg.time_show_seconds);
    }

    // 逐监控项
    for id in 1..=7u32 {
        let idx = match id_to_idx(id) {
            Some(i) => i,
            None => continue,
        };
        if let Some(sec) = ini.section(&format!("item.{}", id)) {
            if let Some(v) = sec.get("show") {
                let b = parse_bool(v, cfg.metric_show_labels[idx]);
                match id {
                    1 => cfg.show_cpu = b,
                    2 => cfg.show_mem = b,
                    3 => cfg.show_gpu = b,
                    4 => cfg.show_net = b,
                    5 => cfg.power_enabled = b,
                    6 => cfg.show_time = b,
                    7 => cfg.show_util = b,
                    _ => {}
                }
            }
            if let Some(v) = sec.get("name") {
                cfg.metric_show_labels[idx] = parse_bool(v, cfg.metric_show_labels[idx]);
            }
            if let Some(v) = sec.get("font") {
                let t = v.trim();
                if t.is_empty() || t == "0" {
                    cfg.metric_font_sizes[idx] = None;
                } else if let Ok(n) = t.parse::<f32>() {
                    let n = n.clamp(FONT_PX_MIN, FONT_PX_MAX);
                    // 与全局字号相等 = 跟随全局（防止误钉死）
                    cfg.metric_font_sizes[idx] =
                        if (n - cfg.font_size).abs() < 0.05 { None } else { Some(n) };
                }
            }
            if let Some(v) = sec.get("color_l") {
                cfg.metric_colors[idx].label = if v.trim().is_empty() {
                    None
                } else {
                    parse_color(v)
                };
            }
            if let Some(v) = sec.get("color_v") {
                cfg.metric_colors[idx].value = if v.trim().is_empty() {
                    None
                } else {
                    parse_color(v)
                };
            }
            if let Some(v) = sec.get("auto") {
                cfg.metric_colors[idx].value_auto = parse_bool(v, cfg.metric_colors[idx].value_auto);
            }
        }
    }

    // 功耗行
    if let Some(v) = ini.get("power", "enabled") {
        cfg.power_enabled = parse_bool(v, cfg.power_enabled);
    }
    if let Some(v) = ini.get("power", "query") {
        cfg.power_query = parse_bool(v, cfg.power_query);
    }
    if let Some(v) = ini.get("power", "display") {
        if let Some(d) = PowerDisplay::from_config(v) {
            cfg.power_display = d;
        }
    }
    if let Some(v) = ini.get("power", "interval_ms") {
        cfg.power_interval_ms = parse_u32(v, cfg.power_interval_ms).clamp(500, 60_000);
    }
    if let Some(v) = ini.get("power", "exclude_virtual_nic") {
        cfg.exclude_virtual_nic = parse_bool(v, cfg.exclude_virtual_nic);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb;

    #[test]
    fn monitor_roundtrip_default() {
        let cfg = Config::default();
        let text = serialize(&cfg);
        assert!(text.contains("[item.1]"));
        assert!(text.contains("[item.7]"));
        let mut c2 = Config::default();
        // 打乱后再解析，确保真的是覆盖
        c2.show_net = false;
        c2.show_util = false;
        parse(&mut c2, &text);
        assert_eq!(c2.show_net, cfg.show_net);
        assert_eq!(c2.show_util, cfg.show_util);
        // order 已移至 layout 文件，monitor 文件不负责顺序
        assert_eq!(c2.metric_order, [0, 1, 2, 6, 3, 4, 5, 7, 8]); // 默认不变
    }

    #[test]
    fn per_item_roundtrip() {
        let mut cfg = Config::default();
        cfg.show_cpu = true;
        cfg.show_gpu = true;
        cfg.show_util = false;
        cfg.metric_colors[3].value = Some(Rgb::new(10, 20, 30));
        cfg.metric_colors[3].value_auto = true;
        cfg.metric_font_sizes[1] = Some(18.5);
        let text = serialize(&cfg);
        let mut c2 = Config::default();
        parse(&mut c2, &text);
        assert_eq!(c2.show_cpu, true);
        assert_eq!(c2.show_gpu, true);
        assert_eq!(c2.show_util, false);
        assert_eq!(c2.metric_colors[3].value, Some(Rgb::new(10, 20, 30)));
        assert!(c2.metric_colors[3].value_auto);
        assert_eq!(c2.metric_font_sizes[1], Some(18.5));
    }

    #[test]
    fn order_ids_map_to_indices() {
        // order 已移至 layout 文件（[layout] order），不再在 monitor 文件里。
        // 此处改为验证 monitor 文件不包含 order（order 字段已移除）。
        let cfg = Config::default();
        let text = serialize(&cfg);
        let ini = Ini::parse(&text);
        assert!(
            ini.get("monitor", "order").is_none(),
            "monitor 文件不含 order（order 在 layout 文件里）"
        );
        // 默认顺序在 layout parse 时从 layout 文件读取（由 system 的 layout 锚点决定）。
        // monitor 文件本身只含 per-metric 项的开关/配色/功耗，不含 order。
    }
}
