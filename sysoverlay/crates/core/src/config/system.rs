//! `system.cfg` —— 全局/系统级配置（与具体布局无关的字段）。
//!
//! 关键字段：
//! - `layout`：当前激活布局锚点（`horizontal`/`vertical`），**替代**对 `render_style`
//!   的序列化。加载时据此决定读 `layout_h.cfg` 还是 `layout_v.cfg`；
//!   切换布局时由 `config_io::switch_layout_in_place` 改写此锚点。
//! - 其余字段均为单例语义，与横/竖无关。

use crate::config::ini::{parse_bool, parse_i32, parse_u32, Ini, Writer};
use crate::config::Config;
use crate::types::{Appearance, Lang, NetMode, NetUnit, RenderEngine, RenderMode, RenderStyle, Zoom};

pub const SYSTEM_FILE: &str = "system.cfg";

fn snap_interval(ms: u32) -> u32 {
    const ALLOWED: [u32; 5] = [5000, 2000, 1000, 200, 100];
    // `min_by_key` 在空迭代器上返回 None。这里迭代的是定长常量数组，实际不可能空
    // （审查报告"上游传入空集合即 panic"的前提不成立），但仍写成不可失败的形式：
    // 兜底值 = 中间档 1000 ms，日后有人把 ALLOWED 改成切片/迭代器时行为依旧确定。
    *ALLOWED
        .iter()
        .min_by_key(|&&a| (a as i64 - ms as i64).abs())
        .unwrap_or(&1000)
}

pub fn serialize(cfg: &Config) -> String {
    let mut w = Writer::new();
    w.section("system");
    w.kv("version", 3);
    match cfg.window_pos {
        Some((x, y)) => {
            w.kv("x", x);
            w.kv("y", y);
        }
        None => {
            w.kv("x", -1);
            w.kv("y", -1);
        }
    }
    match cfg.window_size {
        Some((ww, hh)) => {
            w.kv("w", ww);
            w.kv("h", hh);
        }
        None => {
            w.kv("w", 0);
            w.kv("h", 0);
        }
    }
    w.kv("locked", cfg.size_locked as u8);
    // layout 锚点（决定加载 layout_h.cfg 还是 layout_v.cfg）
    w.kv(
        "layout",
        match cfg.render_style {
            RenderStyle::Vertical => "vertical",
            _ => "horizontal",
        },
    );
    w.kv("zoom", cfg.zoom.to_config());
    w.kv("net_unit", cfg.net_unit.to_config());
    w.kv(
        "net_mode",
        match cfg.net_mode {
            NetMode::Both => "both",
            NetMode::Down => "down",
            NetMode::Up => "up",
        },
    );
    w.kv("interval_ms", cfg.interval_ms);
    w.kv("refresh_auto", cfg.refresh_auto as u8);
    w.kv("topmost", cfg.topmost as u8);
    w.kv("normal_opacity", cfg.normal_opacity);
    w.kv("anim_ms", cfg.anim_ms);
    w.kv("autostart", cfg.autostart as u8);
    w.kv("debug_log", cfg.debug_log as u8);
    w.kv("render_mode", cfg.render_mode.to_config());
    w.kv("render_engine", cfg.render_engine.to_config());
    w.kv("appearance", cfg.appearance.to_config());
    w.kv("settings_w", cfg.settings_size.0);
    w.kv("settings_h", cfg.settings_size.1);
    w.kv("click_through", cfg.click_through as u8);
    w.kv("snap_corner", cfg.snap_corner);
    w.kv("fullscreen_auto_hide", cfg.fullscreen_auto_hide as u8);
    w.kv("gpu_primary_only", cfg.gpu_primary_only as u8);
    w.kv("gpu_3d_only", cfg.gpu_3d_only as u8);
    w.kv("net_low_freq", cfg.net_low_freq as u8);
    w.kv("lang", cfg.lang.to_config());
    // 启动前检测计数（每周一次，累计 3 次后永久跳过）
    w.kv("admin_check_continue_count", cfg.admin_check_continue_count);
    w.kv("driver_check_continue_count", cfg.driver_check_continue_count);
    w.kv("admin_check_last_date", cfg.admin_check_last_date);
    w.kv("driver_check_last_date", cfg.driver_check_last_date);
    w.into_string()
}

pub fn parse(cfg: &mut Config, text: &str) {
    let ini = Ini::parse(text);
    let s = "system";

    // [meta] version（用于跨文件一致性校验）
    if let Some(v) = ini.get("meta", "version") {
        cfg.version = parse_u32(v, cfg.version);
    }

    // 窗口几何（全局，横竖共享）——从 [system] 段读取。
    // 负坐标合法（副屏 / 顶边外），只有序列化端的 -1/-1「未设置」哨兵对
    // 才视为无位置。旧判据 `x>=0 && y>=0` 会把 y<0 的窗口位置判丢，
    // 且下一次退出把真位置反写成 -1/-1，表现为"不记忆窗口位置"。
    let x = ini.get(s, "x").map(|v| parse_i32(v, -1)).unwrap_or(-1);
    let y = ini.get(s, "y").map(|v| parse_i32(v, -1)).unwrap_or(-1);
    cfg.window_pos = if x == -1 && y == -1 { None } else { Some((x, y)) };
    let ww = ini.get(s, "w").map(|v| parse_i32(v, 0)).unwrap_or(0);
    let hh = ini.get(s, "h").map(|v| parse_i32(v, 0)).unwrap_or(0);
    cfg.window_size = if ww > 0 && hh > 0 && ww <= 2048 && hh <= 2048 {
        Some((ww, hh))
    } else {
        None
    };
    if let Some(v) = ini.get(s, "locked") {
        cfg.size_locked = parse_bool(v, cfg.size_locked);
    }

    if let Some(v) = ini.get(s, "layout") {
        cfg.render_style = if v.trim().eq_ignore_ascii_case("vertical") {
            RenderStyle::Vertical
        } else {
            RenderStyle::Strip
        };
    }
    if let Some(v) = ini.get(s, "zoom") {
        cfg.zoom = Zoom::from_config(parse_u32(v, cfg.zoom.to_config()));
    }
    if let Some(v) = ini.get(s, "net_unit") {
        if let Some(u) = NetUnit::from_config(v) {
            cfg.net_unit = u;
        }
    }
    if let Some(v) = ini.get(s, "net_mode") {
        if let Some(m) = NetMode::from_config(v) {
            cfg.net_mode = m;
        }
    }
    if let Some(v) = ini.get(s, "interval_ms") {
        cfg.interval_ms = snap_interval(parse_u32(v, cfg.interval_ms));
    }
    if let Some(v) = ini.get(s, "refresh_auto") {
        cfg.refresh_auto = parse_bool(v, cfg.refresh_auto);
    }
    if let Some(v) = ini.get(s, "topmost") {
        cfg.topmost = parse_bool(v, cfg.topmost);
    }
    if let Some(v) = ini.get(s, "normal_opacity") {
        let n = parse_u32(v, cfg.normal_opacity as u32);
        cfg.normal_opacity = (n.clamp(0, 100)) as u8;
    }
    if let Some(v) = ini.get(s, "anim_ms") {
        cfg.anim_ms = parse_u32(v, cfg.anim_ms).clamp(0, 2000);
    }
    if let Some(v) = ini.get(s, "autostart") {
        cfg.autostart = parse_bool(v, cfg.autostart);
    }
    if let Some(v) = ini.get(s, "debug_log") {
        cfg.debug_log = parse_bool(v, cfg.debug_log);
    }
    if let Some(v) = ini.get(s, "render_mode") {
        if let Some(m) = RenderMode::from_config(v) {
            cfg.render_mode = m;
        }
    }
    if let Some(v) = ini.get(s, "render_engine") {
        if let Some(e) = RenderEngine::from_config(v) {
            cfg.render_engine = e;
        }
    }
    if let Some(v) = ini.get(s, "appearance") {
        if let Some(a) = Appearance::from_config(v) {
            cfg.appearance = a;
        }
    }
    if let Some(v) = ini.get(s, "settings_w") {
        cfg.settings_size.0 = parse_i32(v, cfg.settings_size.0).clamp(560, 1400);
    }
    if let Some(v) = ini.get(s, "settings_h") {
        cfg.settings_size.1 = parse_i32(v, cfg.settings_size.1).clamp(460, 1000);
    }
    if let Some(v) = ini.get(s, "click_through") {
        cfg.click_through = parse_bool(v, cfg.click_through);
    }
    if let Some(v) = ini.get(s, "snap_corner") {
        let n = parse_u32(v, cfg.snap_corner as u32);
        cfg.snap_corner = if n <= 3 { n as u8 } else { crate::config::SNAP_CORNER_FREE };
    }
    if let Some(v) = ini.get(s, "fullscreen_auto_hide") {
        cfg.fullscreen_auto_hide = parse_bool(v, cfg.fullscreen_auto_hide);
    }
    if let Some(v) = ini.get(s, "gpu_primary_only") {
        cfg.gpu_primary_only = parse_bool(v, cfg.gpu_primary_only);
    }
    if let Some(v) = ini.get(s, "gpu_3d_only") {
        cfg.gpu_3d_only = parse_bool(v, cfg.gpu_3d_only);
    }
    if let Some(v) = ini.get(s, "net_low_freq") {
        cfg.net_low_freq = parse_bool(v, cfg.net_low_freq);
    }
    if let Some(v) = ini.get(s, "lang") {
        if let Some(l) = Lang::from_config(v) {
            cfg.lang = l;
        }
    }
    if let Some(v) = ini.get(s, "admin_check_continue_count") {
        cfg.admin_check_continue_count = (parse_u32(v, cfg.admin_check_continue_count as u32).min(3)) as u8;
    }
    if let Some(v) = ini.get(s, "driver_check_continue_count") {
        cfg.driver_check_continue_count =
            (parse_u32(v, cfg.driver_check_continue_count as u32).min(3)) as u8;
    }
    if let Some(v) = ini.get(s, "admin_check_last_date") {
        cfg.admin_check_last_date = parse_u32(v, cfg.admin_check_last_date);
    }
    if let Some(v) = ini.get(s, "driver_check_last_date") {
        cfg.driver_check_last_date = parse_u32(v, cfg.driver_check_last_date);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_roundtrip_default() {
        let cfg = Config::default();
        let text = serialize(&cfg);
        let mut c2 = Config::default();
        parse(&mut c2, &text);
        assert_eq!(c2.render_style, cfg.render_style);
        assert_eq!(c2.zoom, cfg.zoom);
        assert_eq!(c2.net_unit, cfg.net_unit);
        assert_eq!(c2.net_mode, cfg.net_mode);
        assert_eq!(c2.interval_ms, cfg.interval_ms);
        assert_eq!(c2.normal_opacity, cfg.normal_opacity);
        assert_eq!(c2.render_engine, cfg.render_engine);
        assert_eq!(c2.appearance, cfg.appearance);
        assert_eq!(c2.lang, cfg.lang);
        assert_eq!(c2.admin_check_continue_count, cfg.admin_check_continue_count);
    }

    #[test]
    fn negative_positions_roundtrip_and_sentinel() {
        // 回归锁（2026-09-19）：负坐标（副屏/顶边外）必须原样往返；
        // 只有 -1/-1 哨兵对表示"未设置"。
        let mut cfg = Config::default();
        cfg.window_pos = Some((473, -58));
        let mut c2 = Config::default();
        c2.window_pos = None;
        parse(&mut c2, &serialize(&cfg));
        assert_eq!(c2.window_pos, Some((473, -58)), "负 y 位置不得被丢弃");

        let mut none_cfg = Config::default();
        none_cfg.window_pos = None;
        let t = serialize(&none_cfg);
        assert!(t.contains("x = -1") && t.contains("y = -1"));
        let mut c3 = Config::default();
        parse(&mut c3, &t);
        assert_eq!(c3.window_pos, None, "-1/-1 哨兵 = 未设置");
    }

    #[test]
    fn layout_anchor_drives_render_style() {
        let mut cfg = Config::default();
        // 默认 Strip -> horizontal
        let t = serialize(&cfg);
        assert!(t.contains("layout = horizontal"));
        cfg.render_style = RenderStyle::Vertical;
        assert!(serialize(&cfg).contains("layout = vertical"));
        // 解析回来
        let mut c2 = Config::default();
        parse(&mut c2, &serialize(&cfg));
        assert_eq!(c2.render_style, RenderStyle::Vertical);
    }

    #[test]
    fn interval_snaps_to_allowed() {
        let mut cfg = Config::default();
        cfg.interval_ms = 333;
        let t = serialize(&cfg);
        // 333 应吸附到 200（允许档 5000/2000/1000/200/100 中最近）
        let mut c2 = Config::default();
        parse(&mut c2, &t);
        assert_eq!(c2.interval_ms, 200);
    }
}
