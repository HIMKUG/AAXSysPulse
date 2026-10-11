//! 配置文件读写模块 —— 与主程序 `AAXSysPulseC` v3 多文件格式严格对齐。
//!
//! 文件布局（与 `sysoverlay/crates/core/config/*` 的 serialize 完全一致）：
//! - `system.cfg` 全局/系统级：`version` 写在 **[system]** 段（不是 [meta]！主程序的
//!   `config_io::read_file_version` 从 [system] 读版本号，否则判为 v1）。
//! - `appearance.cfg`  颜色/字号/间距/阴影描边。`[meta] version = 3` + `[appearance]`。
//! - `monitor.cfg`     监控项开关/配色/功耗。`[meta] version = 3` + `[monitor]` + `[item.1]..[item.9]` + `[power]`。
//! - `layout_h.cfg`    横版布局专属（窗口几何随 system，这里只放偏移与顺序）。
//! - `layout_v.cfg`    竖版布局专属。
//!
//! 监控项数字 ID（与 `types::Metric` 顺序一致，1-based）：
//!   1=cpu 2=mem 3=gpu 4=net 5=power 6=time 7=util 8=disk_read 9=disk_write

use std::fs::{self, File};
use std::io::Write;
use std::io as io;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    // 原先还有一个 `Parse(String)` 变体：本模块所有解析都是"读不到就用默认值"
    // 的宽松策略（见 `parse_*` 系列），从不构造它 ⇒ 编译器报 dead_code（P2-6）。
}

/// 监控项数量（柔性框架）：与主程序 `Metric::COUNT` 对齐。
/// 主程序新增监控项时只需上调此值并补 `metric_id_for_key` 映射。
pub const METRIC_COUNT: usize = 9;
// 编译期钉死：主程序 `Metric::COUNT` 一变，这里立即编译失败，
// 而不是写出 9 长数组却被主程序按新数量截断/补默认（P1-22 同族漂移）。
const _: () = assert!(METRIC_COUNT == aaxsyspulse_core::types::Metric::COUNT);

/// 配置文件格式版本 —— 跨进程契约（审查项 7.1.3 / P1-22）。
/// 主程序 `core::config::CONFIG_VERSION` 与本常量是**同一个定义**（它自己也从这里 re-export），
/// 两端各写一个字面量 3 时，升版必然只改一边。
pub const CONFIG_VERSION: u32 = aaxsyspulse_core::contract::CONFIG_VERSION;

pub fn config_dir() -> PathBuf {
    // 目录名 = 跨进程契约（审查项 7.1.2 / P1-22）：主程序 `config_io::appdata_dir`
    // 读的是同一个目录，两边各写一份字面量时曾经一头 `AAXSysPulse` 一头 `AAXSysPulseC`。
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(aaxsyspulse_core::contract::APP_DATA_DIR_NAME)
        .join("config")
}

pub fn ensure_config_dir() -> io::Result<PathBuf> {
    let dir = config_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }
    Ok(dir)
}

// ============ INI 解析核心 ============

fn parse_ini(content: &str) -> std::collections::HashMap<String, std::collections::HashMap<String, String>> {
    let mut sections: std::collections::HashMap<String, std::collections::HashMap<String, String>> = std::collections::HashMap::new();
    let mut current_section = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            current_section = line[1..line.len() - 1].to_lowercase();
            sections.entry(current_section.clone()).or_default();
        } else if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_lowercase();
            let value = value.trim().to_string();
            sections.entry(current_section.clone()).or_default().insert(key, value);
        }
    }
    sections
}

fn parse_bool(s: &str) -> bool {
    matches!(s.trim(), "1" | "true" | "yes" | "on")
}

fn parse_u32(s: &str) -> u32 {
    s.trim().parse().unwrap_or(0)
}

fn parse_i32(s: &str) -> i32 {
    s.trim().parse().unwrap_or(0)
}

fn parse_f32(s: &str) -> f32 {
    s.trim().parse().unwrap_or(0.0)
}

// ============ 监控项 ID ↔ 前端 key 映射（唯一真源）============
//
// 必须与主程序 `sysoverlay/crates/core/src/types.rs` 的 `Metric` 枚举序完全一致：
//   index 0 → id 1 = cpu
//   index 1 → id 2 = mem
//   index 2 → id 3 = gpu
//   index 3 → id 4 = net
//   index 4 → id 5 = power
//   index 5 → id 6 = time
//   index 6 → id 7 = util
//   index 7 → id 8 = disk_read
//   index 8 → id 9 = disk_write

pub fn metric_key_for_id(id: u32) -> Option<&'static str> {
    match id {
        1 => Some("cpu"),
        2 => Some("mem"),
        3 => Some("gpu"),
        4 => Some("net"),
        5 => Some("power"),
        6 => Some("time"),
        7 => Some("util"),
        8 => Some("disk_read"),
        9 => Some("disk_write"),
        _ => None,
    }
}

pub fn metric_id_for_key(key: &str) -> Option<u32> {
    match key {
        "cpu" => Some(1),
        "mem" => Some(2),
        "gpu" => Some(3),
        "net" => Some(4),
        "power" => Some(5),
        "time" => Some(6),
        "util" => Some(7),
        "disk_read" => Some(8),
        "disk_write" => Some(9),
        _ => None,
    }
}

// ============ system.cfg ============
//
// ⚠️ version 必须写在 [system] 段。主程序 `config_io::read_file_version` 对 system.cfg
// 从 ("system", "version") 读取；若写在 [meta] 会被当成 v1，与其余 v3 文件组成 span=2
// → 触发 VersionSpanTooLarge 破坏性弹窗。

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemConfig {
    // [system]
    pub version: u32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub locked: bool,
    pub layout: String,
    pub zoom: u32,
    pub net_unit: String,
    pub net_mode: String,
    pub interval_ms: u32,
    pub refresh_auto: bool,
    pub topmost: bool,
    pub normal_opacity: u32,
    pub anim_ms: u32,
    pub autostart: bool,
    pub debug_log: bool,
    pub render_mode: String,
    pub render_engine: String,
    pub appearance: u32,
    pub settings_w: i32,
    pub settings_h: i32,
    pub click_through: bool,
    pub snap_corner: u32,
    pub fullscreen_auto_hide: bool,
    pub gpu_primary_only: bool,
    pub gpu_3d_only: bool,
    pub net_low_freq: bool,
    pub lang: String,
    pub admin_check_continue_count: u32,
    pub driver_check_continue_count: u32,
    pub admin_check_last_date: u32,
    pub driver_check_last_date: u32,
}

impl Default for SystemConfig {
    /// 与 [`SystemConfig::load`] 的「文件不存在」分支**同一套默认值**。
    ///
    /// ⚠️ 早期 `#[derive(Default)]` 让数值全为 0：`normal_opacity = 0` 会让悬浮窗
    /// 完全透明，`settings_w/h = 0` 会被主程序钳到下限，`interval_ms = 0` 会被吸附
    /// 到 100ms（最快档）。文件缺失时写出的就是这份垃圾配置。
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            x: -1,
            y: -1,
            w: 0,
            h: 0,
            locked: false,
            layout: "horizontal".to_string(),
            zoom: 100,
            net_unit: "mb".to_string(),
            net_mode: "both".to_string(),
            interval_ms: 1000,
            refresh_auto: true,
            topmost: true,
            normal_opacity: 100,
            anim_ms: 300,
            autostart: false,
            debug_log: false,
            render_mode: "quality".to_string(),
            render_engine: "0".to_string(),
            appearance: 0,
            settings_w: 800,
            settings_h: 640,
            click_through: false,
            snap_corner: 0,
            fullscreen_auto_hide: true,
            gpu_primary_only: false,
            gpu_3d_only: false,
            net_low_freq: false,
            lang: "en".to_string(),
            admin_check_continue_count: 0,
            driver_check_continue_count: 0,
            admin_check_last_date: 0,
            driver_check_last_date: 0,
        }
    }
}

impl SystemConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let path = config_dir().join("system.cfg");
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path)?;
        let sections = parse_ini(&content);

        // 版本：优先 [system]（主程序读取口径），兼容旧 [meta] 写法
        let version = sections
            .get("system")
            .and_then(|s| s.get("version"))
            .or_else(|| sections.get("meta").and_then(|m| m.get("version")))
            .and_then(|v| v.parse().ok())
            .unwrap_or(CONFIG_VERSION);

        let sys = sections.get("system").cloned().unwrap_or_default();

        let get = |k: &str, default: &str| -> String {
            sys.get(k).cloned().unwrap_or_else(|| default.to_string())
        };

        Ok(Self {
            version,
            x: parse_i32(&get("x", "-1")),
            y: parse_i32(&get("y", "-1")),
            w: parse_i32(&get("w", "0")),
            h: parse_i32(&get("h", "0")),
            locked: parse_bool(&get("locked", "false")),
            layout: get("layout", "horizontal"),
            zoom: parse_u32(&get("zoom", "100")).clamp(75, 200),
            net_unit: get("net_unit", "mb"),
            net_mode: get("net_mode", "both"),
            interval_ms: parse_u32(&get("interval_ms", "1000")),
            refresh_auto: parse_bool(&get("refresh_auto", "true")),
            topmost: parse_bool(&get("topmost", "true")),
            normal_opacity: parse_u32(&get("normal_opacity", "100")).clamp(0, 100),
            anim_ms: parse_u32(&get("anim_ms", "300")).min(2000),
            autostart: parse_bool(&get("autostart", "false")),
            debug_log: parse_bool(&get("debug_log", "false")),
            render_mode: get("render_mode", "quality"),
            render_engine: get("render_engine", "0"),
            appearance: parse_u32(&get("appearance", "0")),
            settings_w: parse_i32(&get("settings_w", "800")).clamp(560, 1400),
            settings_h: parse_i32(&get("settings_h", "640")).clamp(460, 1000),
            click_through: parse_bool(&get("click_through", "false")),
            snap_corner: parse_u32(&get("snap_corner", "0")),
            fullscreen_auto_hide: parse_bool(&get("fullscreen_auto_hide", "true")),
            gpu_primary_only: parse_bool(&get("gpu_primary_only", "false")),
            gpu_3d_only: parse_bool(&get("gpu_3d_only", "false")),
            net_low_freq: parse_bool(&get("net_low_freq", "false")),
            lang: get("lang", "en"),
            admin_check_continue_count: parse_u32(&get("admin_check_continue_count", "0")),
            driver_check_continue_count: parse_u32(&get("driver_check_continue_count", "0")),
            admin_check_last_date: parse_u32(&get("admin_check_last_date", "0")),
            driver_check_last_date: parse_u32(&get("driver_check_last_date", "0")),
        })
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        let path = config_dir().join("system.cfg");
        ensure_config_dir()?;
        atomic_write(&path, self.ini_text().as_bytes())
    }

    /// `system.cfg` 的全文（审查项 7.4.2 / P1-23：与落盘解耦，才能被跨进程
    /// 兼容性测试直接喂给主程序 `core::config::system::parse`）。
    /// ⚠️ version 写在 [system] 段内，与主程序 read_file_version 口径一致。
    pub fn ini_text(&self) -> String {
        format!(
            r#"[system]
version = {ver}
x = {x}
y = {y}
w = {w}
h = {h}
locked = {locked}
layout = {layout}
zoom = {zoom}
net_unit = {net_unit}
net_mode = {net_mode}
interval_ms = {interval_ms}
refresh_auto = {refresh_auto}
topmost = {topmost}
normal_opacity = {normal_opacity}
anim_ms = {anim_ms}
autostart = {autostart}
debug_log = {debug_log}
render_mode = {render_mode}
render_engine = {render_engine}
appearance = {appearance}
settings_w = {settings_w}
settings_h = {settings_h}
click_through = {click_through}
snap_corner = {snap_corner}
fullscreen_auto_hide = {fullscreen_auto_hide}
gpu_primary_only = {gpu_primary_only}
gpu_3d_only = {gpu_3d_only}
net_low_freq = {net_low_freq}
lang = {lang}
admin_check_continue_count = {admin_check_continue_count}
driver_check_continue_count = {driver_check_continue_count}
admin_check_last_date = {admin_check_last_date}
driver_check_last_date = {driver_check_last_date}
"#,
            x = self.x,
            y = self.y,
            w = self.w,
            h = self.h,
            locked = self.locked,
            layout = self.layout,
            zoom = self.zoom,
            net_unit = self.net_unit,
            net_mode = self.net_mode,
            interval_ms = self.interval_ms,
            refresh_auto = self.refresh_auto,
            topmost = self.topmost,
            normal_opacity = self.normal_opacity,
            anim_ms = self.anim_ms,
            autostart = self.autostart,
            debug_log = self.debug_log,
            render_mode = self.render_mode,
            render_engine = self.render_engine,
            appearance = self.appearance,
            settings_w = self.settings_w,
            settings_h = self.settings_h,
            click_through = self.click_through,
            snap_corner = self.snap_corner,
            fullscreen_auto_hide = self.fullscreen_auto_hide,
            gpu_primary_only = self.gpu_primary_only,
            gpu_3d_only = self.gpu_3d_only,
            net_low_freq = self.net_low_freq,
            lang = self.lang,
            admin_check_continue_count = self.admin_check_continue_count,
            driver_check_continue_count = self.driver_check_continue_count,
            admin_check_last_date = self.admin_check_last_date,
            driver_check_last_date = self.driver_check_last_date,
            ver = self.version,
        )
    }

    /// 从前端 JSON patch 更新字段，然后保存
    pub fn patch_from_json(&mut self, patch: &serde_json::Value) {
        // ── 架构决策（2026-09-16）：引导程序**不写** `appearance` ──
        // 主程序的 `appearance` 是「**设置窗口**主题」（0=跟随系统 / 1=浅色 / 2=深色），
        // 与引导的「**悬浮窗**样式预设 A–F」是两个不同概念，不能混用。
        // 早期把 `style` A–F 映射成 appearance 0–5 造成两个问题：
        //   1) D/E/F → 3/4/5 超出枚举，主程序 `Appearance::from_config` 返回 None ⇒ 静默忽略；
        //   2) 主程序下次保存时把它归一化回 0 ⇒ 用户的选择被吞掉。
        // 现在该字段**只由 `load()` 原样读回、`save()` 原样写回**，
        // 用户在主程序设置页选的主题不会被引导覆盖。

        // gdi_mode → render_engine。
        // 主程序口径（types.rs::RenderEngine）：`"0"` = Gdi（默认，省显卡）、
        // `"1"` = D2d（DirectWrite 高质量）。
        // ⚠️ 早期实现写反了：`gdi=true` 写成 `"1"` ⇒ 选「GDI 兼容」反而启用了 D2D，
        // 与「降低约 8 倍显卡占用」的承诺完全相反。
        if let Some(gdi) = patch.get("gdi_mode").and_then(|v| v.as_bool()) {
            self.render_engine = if gdi { "0".to_string() } else { "1".to_string() };
        }
        // orientation → layout（horizontal / vertical）
        if let Some(o) = patch.get("orientation").and_then(|v| v.as_str()) {
            self.layout = o.to_string();
        }
        // opacity → normal_opacity（主程序是 **0–100 整数**，不是 0–1 小数）
        if let Some(op) = patch.get("opacity").and_then(|v| v.as_u64()) {
            self.normal_opacity = op as u32;
        }
        // position → x, y（窗口几何唯一归属 system.cfg）
        if let Some(pos) = patch.get("position").and_then(|v| v.as_object()) {
            if let Some(x) = pos.get("x").and_then(|v| v.as_i64()) {
                self.x = x as i32;
            }
            if let Some(y) = pos.get("y").and_then(|v| v.as_i64()) {
                self.y = y as i32;
            }
        }
    }
}

// ============ appearance.cfg ============
// 与主程序 `core/config/appearance.rs` 一致。

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppearanceConfig {
    pub version: u32,
    pub bg: String,
    pub fg: String,
    pub font_size: f32,
    pub font_weight: u32,
    pub metric_gap: u32,
    pub label_value_gap: u32,
    pub shadow: bool,
    pub outline: bool,
}

impl Default for AppearanceConfig {
    /// 与 [`AppearanceConfig::load`] 缺失分支同默认值（否则 `font_size = 0.0`
    /// 会被主程序钳到 4px、间距全为 0）。
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            bg: "#000000".to_string(),
            fg: "#FFFFFF".to_string(),
            font_size: 12.0,
            font_weight: 2,
            metric_gap: 16,
            label_value_gap: 4,
            shadow: true,
            outline: false,
        }
    }
}

impl AppearanceConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let path = config_dir().join("appearance.cfg");
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path)?;
        let sections = parse_ini(&content);
        let app = sections.get("appearance").cloned().unwrap_or_default();

        let get = |k: &str, default: &str| -> String {
            app.get(k).cloned().unwrap_or_else(|| default.to_string())
        };

        Ok(Self {
            version: sections.get("meta").and_then(|m| m.get("version")).and_then(|v| v.parse().ok()).unwrap_or(CONFIG_VERSION),
            bg: get("bg", "#000000"),
            fg: get("fg", "#FFFFFF"),
            font_size: parse_f32(&get("font_size", "12.0")).clamp(8.0, 32.0),
            font_weight: parse_u32(&get("font_weight", "2")),
            metric_gap: parse_u32(&get("metric_gap", "16")),
            label_value_gap: parse_u32(&get("label_value_gap", "4")),
            shadow: parse_bool(&get("shadow", "true")),
            outline: parse_bool(&get("outline", "false")),
        })
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        let path = config_dir().join("appearance.cfg");
        ensure_config_dir()?;
        atomic_write(&path, self.ini_text().as_bytes())
    }

    /// `appearance.cfg` 的全文（审查项 7.4.2 / P1-23，理由同 `SystemConfig::ini_text`）。
    pub fn ini_text(&self) -> String {
        format!(
            r#"[meta]
version = {ver}

[appearance]
bg = {bg}
fg = {fg}
font_size = {font_size}
font_weight = {font_weight}
metric_gap = {metric_gap}
label_value_gap = {label_value_gap}
shadow = {shadow}
outline = {outline}
"#,
            bg = self.bg,
            fg = self.fg,
            font_size = self.font_size,
            font_weight = self.font_weight,
            metric_gap = self.metric_gap,
            label_value_gap = self.label_value_gap,
            shadow = self.shadow,
            outline = self.outline,
            ver = self.version,
        )
    }

    /// 从前端 JSON patch 更新字段，然后保存
    pub fn patch_from_json(&mut self, patch: &serde_json::Value) {
        // colors.name → fg（全局文字 / 名称默认色）
        if let Some(c) = patch.get("colors").and_then(|v| v.get("name")).and_then(|v| v.as_str()) {
            self.fg = c.to_string();
        }
        // ⚠️ 不得把 colors.value 写进 bg！
        // 主程序里 `bg` 是**悬浮窗背景色**，而前端 `colors.value` 是数值**文字色**——
        // 早期实现误把后者当 bg 写入，会把整块背景刷成数值颜色。
        // 数值色改由 monitor.cfg 的逐项 `color_v` 承载（见 MonitorConfig::patch_from_json）。
        // font_name → font_size（全局字号）
        if let Some(f) = patch.get("font_name").and_then(|v| v.as_u64()) {
            self.font_size = f as f32;
        }
        // font_weight → font_weight（主程序 0=细 / 1=中 / 2=粗，钳位 0..2）
        if let Some(w) = patch.get("font_weight").and_then(|v| v.as_u64()) {
            self.font_weight = (w as u32).min(2);
        }
    }
}

// ============ monitor.cfg ============
// 与主程序 `core/config/monitor.rs` 一致：items 固定 9 个（id = index+1），
// 顺序与 types::Metric 枚举一致。order（显示顺序）由 layout.cfg 承载，这里不写。

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MonitorConfig {
    pub version: u32,
    pub util_decimal: bool,
    pub mem_gb: bool,
    pub short_names: bool,
    pub show_time: bool,
    pub time_seconds: bool,
    pub items: Vec<MonitorItemConfig>, // 长度 9，id = index + 1
    pub power_enabled: bool,
    pub power_query: bool,
    pub power_display: String,
    pub power_interval_ms: u32,
    pub exclude_virtual_nic: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MonitorItemConfig {
    pub id: u32,
    pub show: bool,
    pub name: bool,
    /// 逐项名称模式（主程序 monitor.cfg `[item.N] short`）：0=跟随全局 short_names，1=完整，2=简写。
    #[serde(default)]
    pub short: u32,
    pub font: f32,
    pub color_l: String,
    pub color_v: String,
    pub auto: bool,
}

impl Default for MonitorConfig {
    /// ⚠️ 必须自带 9 个 item（id = 1..9）：`save()` 会按 `items[(id-1)]` 写
    /// `[item.1]..[item.9]`，空 `items` 会**越界 panic**。`unwrap_or_default()`
    /// 是一条常见回退路径，不能在这里踩坑。
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            util_decimal: true,
            mem_gb: true,
            short_names: false,
            show_time: true,
            time_seconds: true,
            items: (1..=METRIC_COUNT as u32)
                .map(|id| MonitorItemConfig {
                    id,
                    show: true,
                    name: true,
                    ..Default::default()
                })
                .collect(),
            power_enabled: true,
            power_query: true,
            power_display: "cpupackage".to_string(),
            power_interval_ms: 3000,
            exclude_virtual_nic: true,
        }
    }
}

impl MonitorConfig {
    pub fn load() -> Result<Self, ConfigError> {
        // 始终保证 items 长度为 9（id = index+1），避免 save() 越界
        let mut cfg = MonitorConfig {
            items: (1..=METRIC_COUNT as u32)
                .map(|id| MonitorItemConfig { id, ..Default::default() })
                .collect(),
            ..Default::default()
        };

        let path = config_dir().join("monitor.cfg");
        if !path.exists() {
            return Ok(cfg);
        }

        let content = fs::read_to_string(&path)?;
        let sections = parse_ini(&content);

        let mon = sections.get("monitor").cloned().unwrap_or_default();
        let get = |k: &str, default: &str| -> String {
            mon.get(k).cloned().unwrap_or_else(|| default.to_string())
        };

        cfg.version = sections.get("meta").and_then(|m| m.get("version")).and_then(|v| v.parse().ok()).unwrap_or(CONFIG_VERSION);
        cfg.util_decimal = parse_bool(&get("util_decimal", "true"));
        cfg.mem_gb = parse_bool(&get("mem_gb", "true"));
        cfg.short_names = parse_bool(&get("short_names", "false"));
        cfg.show_time = parse_bool(&get("show_time", "true"));
        cfg.time_seconds = parse_bool(&get("time_seconds", "true"));

        let power = sections.get("power").cloned().unwrap_or_default();
        cfg.power_enabled = parse_bool(&power.get("enabled").cloned().unwrap_or_else(|| "true".into()));
        cfg.power_query = parse_bool(&power.get("query").cloned().unwrap_or_else(|| "true".into()));
        cfg.power_display = power.get("display").cloned().unwrap_or_else(|| "cpupackage".into());
        cfg.power_interval_ms = parse_u32(&power.get("interval_ms").cloned().unwrap_or_else(|| "3000".into()));
        cfg.exclude_virtual_nic = parse_bool(&power.get("exclude_virtual_nic").cloned().unwrap_or_else(|| "true".into()));

        for id in 1..=METRIC_COUNT as u32 {
            if let Some(item_map) = sections.get(&format!("item.{}", id)) {
                let idx = (id - 1) as usize;
                let g = |k: &str, d: &str| -> String {
                    item_map.get(k).cloned().unwrap_or_else(|| d.to_string())
                };
                cfg.items[idx].id = id;
                cfg.items[idx].show = parse_bool(&g("show", "true"));
                cfg.items[idx].name = parse_bool(&g("name", "true"));
                cfg.items[idx].short = parse_u32(&g("short", "0")).min(2);
                cfg.items[idx].font = parse_f32(&g("font", "0"));
                cfg.items[idx].color_l = g("color_l", "");
                cfg.items[idx].color_v = g("color_v", "");
                cfg.items[idx].auto = parse_bool(&g("auto", "false"));
            }
        }

        // 与主程序 `monitor::parse` 的**覆盖顺序**对齐，消除「两份表示不一致」：
        //   主程序读取顺序 = `[monitor]` → `[item.*]`（id 6 会覆盖 show_time）
        //   → `[power]`（覆盖 item 5 的 show）。
        // 故最终权威为：`show_time` ← `[item.6] show`；`power_enabled` ← `[power] enabled`。
        // 这里把引导侧的两份表示同步一致，避免 UI 显示与主程序实际生效值不符。
        cfg.show_time = cfg.items[5].show;
        cfg.items[4].show = cfg.power_enabled;

        Ok(cfg)
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        let path = config_dir().join("monitor.cfg");
        ensure_config_dir()?;
        atomic_write(&path, self.ini_text().as_bytes())
    }

    /// `monitor.cfg` 的全文（审查项 7.4.2 / P1-23，理由同 `SystemConfig::ini_text`）。
    pub fn ini_text(&self) -> String {
        let mut items_str = String::new();
        for id in 1..=METRIC_COUNT as u32 {
            let item = &self.items[(id - 1) as usize];
            items_str.push_str(&format!(
                "[item.{}]\nshow = {}\nname = {}\nshort = {}\nfont = {}\ncolor_l = {}\ncolor_v = {}\nauto = {}\n\n",
                id, item.show, item.name, item.short, item.font, item.color_l, item.color_v, item.auto,
            ));
        }

        format!(
            r#"[meta]
version = {ver}

[monitor]
util_decimal = {util_decimal}
mem_gb = {mem_gb}
short_names = {short_names}
show_time = {show_time}
time_seconds = {time_seconds}

{items_str}[power]
enabled = {power_enabled}
query = {power_query}
display = {power_display}
interval_ms = {power_interval_ms}
exclude_virtual_nic = {exclude_virtual_nic}
"#,
            util_decimal = self.util_decimal,
            mem_gb = self.mem_gb,
            short_names = self.short_names,
            show_time = self.show_time,
            time_seconds = self.time_seconds,
            items_str = items_str,
            power_enabled = self.power_enabled,
            power_query = self.power_query,
            power_display = self.power_display,
            power_interval_ms = self.power_interval_ms,
            exclude_virtual_nic = self.exclude_virtual_nic,
            ver = self.version,
        )
    }

    /// 从前端 JSON patch 更新字段，然后保存。
    /// 注意：显示顺序 `order` 由 `LayoutConfig` 负责（主程序把 order 存在 layout.cfg）。
    pub fn patch_from_json(&mut self, patch: &serde_json::Value) {
        // 0) 显式 `per_item_colors: null` ⇒ 清空逐项配色覆盖，回落全局色。
        //    前端「统一设置」会把 perColors 置 null；若不清空，旧配置里的逐项色会
        //    一直生效，把预设写下的全局色整个遮住。
        if matches!(patch.get("per_item_colors"), Some(serde_json::Value::Null)) {
            for item in self.items.iter_mut() {
                item.color_l.clear();
                item.color_v.clear();
            }
        }
        // 全局文字色 colors.{name,value} 落盘口径：
        // 主程序**没有**「全局数值色」字段，只有逐项 `metric_colors[idx].{label,value}`
        // （`None` = 跟随全局 `fg`）。因此把前端两路全局色映射到**全部 9 项**的
        // color_l / color_v，等效于前端的「统一设置」（前端此时会把 perColors 置 null）。
        // 逐项覆盖 `per_item_colors` 在下方随后应用，优先级更高。
        if let Some(colors) = patch.get("colors").and_then(|v| v.as_object()) {
            let name = colors.get("name").and_then(|v| v.as_str());
            let value = colors.get("value").and_then(|v| v.as_str());
            for item in self.items.iter_mut() {
                if let Some(n) = name {
                    item.color_l = n.to_string();
                }
                if let Some(v) = value {
                    item.color_v = v.to_string();
                }
            }
        }

        // items: { cpu: true, mem: false, ... } → items[id-1].show
        if let Some(items_obj) = patch.get("items").and_then(|v| v.as_object()) {
            for (key, val) in items_obj {
                if let Some(id) = metric_id_for_key(key) {
                    if let Some(show) = val.as_bool() {
                        let idx = (id - 1) as usize;
                        if idx < self.items.len() {
                            self.items[idx].show = show;
                            // power 同时映射 item.5 与 [power] enabled
                            if id == 5 {
                                self.power_enabled = show;
                            }
                        }
                    }
                }
            }
        }

        // per_item_colors: { cpu: { name, value }, ... } → items[id-1].color_l / color_v
        if let Some(per_colors) = patch.get("per_item_colors").and_then(|v| v.as_object()) {
            for (key, val) in per_colors {
                if let Some(id) = metric_id_for_key(key) {
                    if let Some(obj) = val.as_object() {
                        let idx = (id - 1) as usize;
                        if idx < self.items.len() {
                            if let Some(n) = obj.get("name").and_then(|v| v.as_str()) {
                                self.items[idx].color_l = n.to_string();
                            }
                            if let Some(v) = obj.get("value").and_then(|v| v.as_str()) {
                                self.items[idx].color_v = v.to_string();
                            }
                        }
                    }
                }
            }
        }

        // 显式 `per_item_fonts: null` ⇒ 清空逐项字号覆盖（写出 `font = 0`，
        // 主程序把它读成 `None` = 跟随全局字号）。前端无逐项字号时快照里就是 null；
        // 预设必须能把它复位，否则旧的逐项字号会盖过预设的 `font_size`。
        if matches!(patch.get("per_item_fonts"), Some(serde_json::Value::Null)) {
            for item in self.items.iter_mut() {
                item.font = 0.0;
            }
        }

        // per_item_fonts: { cpu: { value: 16 }, ... } → items[id-1].font
        if let Some(per_fonts) = patch.get("per_item_fonts").and_then(|v| v.as_object()) {
            for (key, val) in per_fonts {
                if let Some(id) = metric_id_for_key(key) {
                    if let Some(obj) = val.as_object() {
                        let idx = (id - 1) as usize;
                        if idx < self.items.len() {
                            if let Some(f) = obj.get("value").and_then(|v| v.as_u64()) {
                                self.items[idx].font = f as f32;
                                self.items[idx].auto = false;
                            }
                        }
                    }
                }
            }
        }

        // name_mode / per_item_name_mode：名称显示方式（第 8 页）
        //   none   → 不显示名称（item.name = false）
        //   simple → 简写（item.short = 2，或跟随全局 short_names = true）
        //   full   → 完整（item.short = 1，或跟随全局 short_names = false）
        // 全局 name_mode 落 short_names + 全部 item.name，并把逐项 short 复位为 0（跟随）；
        // per_item_name_mode 随后写逐项显值（无损：主程序 v0.7.2 起支持 [item.N] short）。
        if matches!(patch.get("per_item_name_mode"), Some(serde_json::Value::Null)) {
            for it in self.items.iter_mut() {
                it.short = 0;
            }
        }
        if let Some(mode) = patch.get("name_mode").and_then(|v| v.as_str()) {
            match mode {
                "none" => {
                    for it in self.items.iter_mut() {
                        it.name = false;
                    }
                }
                "simple" => {
                    for it in self.items.iter_mut() {
                        it.name = true;
                    }
                    self.short_names = true;
                }
                "full" => {
                    for it in self.items.iter_mut() {
                        it.name = true;
                    }
                    self.short_names = false;
                }
                _ => {}
            }
        }
        if let Some(per) = patch.get("per_item_name_mode").and_then(|v| v.as_object()) {
            for (key, val) in per {
                if let Some(id) = metric_id_for_key(key) {
                    if let Some(mode) = val.as_str() {
                        let idx = (id - 1) as usize;
                        if idx < self.items.len() {
                            match mode {
                                "none" => self.items[idx].name = false,
                                "simple" => {
                                    self.items[idx].name = true;
                                    self.items[idx].short = 2;
                                }
                                "full" => {
                                    self.items[idx].name = true;
                                    self.items[idx].short = 1;
                                }
                                "follow" => self.items[idx].short = 0,
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }
}

// ============ layout_h.cfg / layout_v.cfg ============
// 与主程序 `core/config/layout.rs` 一致：窗口几何唯一归属 system.cfg，
// 这里只放逐指标 4 分量偏移 `offset.{id}` 与显示顺序 `order`（1-based ID 列表）。

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LayoutConfig {
    pub version: u32,
    pub order: Vec<u32>,        // 9 个 id（1..9）
    pub offsets: Vec<[i32; 4]>, // 9 组 [lx, ly, vx, vy]
}

impl Default for LayoutConfig {
    /// 与 [`LayoutConfig::load`] 缺失分支同默认值：`order = 1..9`、9 组零偏移。
    /// 否则会写出空 `order`（主程序按默认序兜底，但语义不该依赖兜底）。
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            order: (1..=METRIC_COUNT as u32).collect(),
            offsets: vec![[0, 0, 0, 0]; METRIC_COUNT],
        }
    }
}

impl LayoutConfig {
    pub fn load(vertical: bool) -> Result<Self, ConfigError> {
        let filename = if vertical { "layout_v.cfg" } else { "layout_h.cfg" };
        let path = config_dir().join(filename);
        let mut cfg = LayoutConfig {
            version: CONFIG_VERSION,
            order: (1..=METRIC_COUNT as u32).collect(),
            offsets: vec![[0, 0, 0, 0]; METRIC_COUNT],
        };

        if path.exists() {
            let content = fs::read_to_string(&path)?;
            let sections = parse_ini(&content);
            cfg.version = sections.get("meta").and_then(|m| m.get("version")).and_then(|v| v.parse().ok()).unwrap_or(CONFIG_VERSION);

            // order：1-based ID 列表 → 拼成完整 9 长序列（缺失的按自然序补在末尾）
            if let Some(l) = sections.get("layout") {
                if let Some(o) = l.get("order") {
                    let ids: Vec<u32> = o.split_whitespace().filter_map(|s| s.parse().ok()).collect();
                    if !ids.is_empty() {
                        let mut full: Vec<u32> = Vec::with_capacity(METRIC_COUNT);
                        let mut seen = [false; METRIC_COUNT + 1];
                        for &id in &ids {
                            if (1..=METRIC_COUNT as u32).contains(&id) && !seen[id as usize] {
                                seen[id as usize] = true;
                                full.push(id);
                            }
                        }
                        for id in 1..=METRIC_COUNT as u32 {
                            if !seen[id as usize] {
                                full.push(id);
                            }
                        }
                        cfg.order = full;
                    }
                }
            }

            // offset.1 .. offset.9
            if let Some(m) = sections.get("metrics") {
                for id in 1..=METRIC_COUNT as u32 {
                    if let Some(v) = m.get(&format!("offset.{}", id)) {
                        let parts: Vec<i32> = v.split_whitespace().filter_map(|s| s.parse().ok()).collect();
                        if parts.len() >= 4 {
                            cfg.offsets[(id - 1) as usize] = [parts[0], parts[1], parts[2], parts[3]];
                        }
                    }
                }
            }
        }
        Ok(cfg)
    }

    pub fn save(&self, vertical: bool) -> Result<(), ConfigError> {
        let filename = if vertical { "layout_v.cfg" } else { "layout_h.cfg" };
        let path = config_dir().join(filename);
        ensure_config_dir()?;
        atomic_write(&path, self.ini_text().as_bytes())
    }

    /// `layout_*.cfg` 的全文（审查项 7.4.2 / P1-23，理由同 `SystemConfig::ini_text`；
    /// 纵/横两个文件共用同一份正文，只有文件名不同）。
    pub fn ini_text(&self) -> String {
        let mut offsets_str = String::new();
        for (i, off) in self.offsets.iter().enumerate() {
            offsets_str.push_str(&format!(
                "offset.{} = {} {} {} {}\n",
                i + 1, off[0], off[1], off[2], off[3]
            ));
        }

        let order_str = self.order.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(" ");

        format!(
            r#"[meta]
version = {ver}

[metrics]
{offsets_str}[layout]
order = {order}
"#,
            offsets_str = offsets_str,
            order = order_str,
            ver = self.version,
        )
    }

    /// 从前端 JSON patch 更新顺序，然后保存。
    /// `order` 是前端 key 列表（cpu/mem/gpu/...），映射为 id 并拼成完整 9 长序列。
    pub fn patch_from_json(&mut self, patch: &serde_json::Value) {
        if let Some(arr) = patch.get("order").and_then(|v| v.as_array()) {
            let mut full: Vec<u32> = Vec::with_capacity(METRIC_COUNT);
            let mut seen = [false; METRIC_COUNT + 1];
            for v in arr {
                if let Some(key) = v.as_str() {
                    if let Some(id) = metric_id_for_key(key) {
                        if (1..=METRIC_COUNT as u32).contains(&id) && !seen[id as usize] {
                            seen[id as usize] = true;
                            full.push(id);
                        }
                    }
                }
            }
            for id in 1..=METRIC_COUNT as u32 {
                if !seen[id as usize] {
                    full.push(id);
                }
            }
            if full.len() == METRIC_COUNT {
                self.order = full;
            }
        }
    }
}

// ============ 启动期补齐 ============

/// 保证 5 个 `.cfg` **全部存在**（缺失的用默认值补成 v3）。返回本次补写的文件名。
///
/// ⚠️ 为什么必须连「非激活布局」一起补齐：
/// 主程序 `config_io::check_version_consistency` 会遍历**全部 5 个文件**读版本号，
/// 文件缺失 ⇒ 版本读不到 ⇒ 被判为 **v1**；此时其余文件已是 v3 ⇒ span = 3-1 = 2 > 1
/// ⇒ 触发 `VersionSpanTooLarge` 破坏性弹窗。引导流程若只写「当前激活布局」，
/// 在**主程序从未运行过**的全新机器上 `layout_v.cfg` 会永久缺失
/// ⇒ 主程序每次启动都报「版本跨度过大」。
/// 附带好处：主程序的旧配置迁移判定是「`config.txt` 存在 **且任一 `.cfg` 缺失**」，
/// 补齐 5 个文件同时避免了引导写好的设置被旧 `config.txt` 迁移覆盖。
///
/// 只写**缺失**的文件，已存在的一律不动（不覆盖用户 / 主程序的数据）。
pub fn ensure_all_files() -> Result<Vec<String>, ConfigError> {
    ensure_config_dir()?;
    let dir = config_dir();
    let mut created = Vec::new();

    if !dir.join("system.cfg").exists() {
        SystemConfig::default().save()?;
        created.push("system.cfg".to_string());
    }
    if !dir.join("appearance.cfg").exists() {
        AppearanceConfig::default().save()?;
        created.push("appearance.cfg".to_string());
    }
    if !dir.join("monitor.cfg").exists() {
        MonitorConfig::default().save()?;
        created.push("monitor.cfg".to_string());
    }
    for vertical in [false, true] {
        let name = if vertical { "layout_v.cfg" } else { "layout_h.cfg" };
        if !dir.join(name).exists() {
            LayoutConfig::default().save(vertical)?;
            created.push(name.to_string());
        }
    }

    Ok(created)
}

// ============ 原子写 ============

/// 唯一临时文件名：`.system.cfg.<pid>.<seq>.tmp`
/// 固定名 `xxx.tmp` 会在两个写者并发时互相覆盖（A 的 tmp 被 B 改名走，A 的 rename 报错）。
fn temp_path_for(path: &std::path::Path) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

/// 原子写：写临时文件 → fsync → 原地替换。
///
/// `std::fs::rename` 在 Windows 上就是 `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)`，
/// 目标存在（甚至被别的进程以 share 方式打开）时可直接替换，**不需要先删**。
/// 旧实现 `remove_file(path)` + `rename`：删除与改名之间配置文件**不存在**，
/// 主程序恰好在这个窗口读盘会拿到"文件缺失"，被 `load_config` 判为配置损坏。
pub(crate) fn atomic_write<E: From<io::Error>>(
    path: &std::path::Path,
    content: &[u8],
) -> Result<(), E> {
    let temp_path = temp_path_for(path);
    {
        let mut file = File::create(&temp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
    }
    // 目标可能被主程序瞬时占用（它每次刷新都读一遍配置）：退避重试而不是直接失败。
    let mut last_err: Option<io::Error> = None;
    for attempt in 1..=3u64 {
        match fs::rename(&temp_path, path) {
            Ok(()) => {
                last_err = None;
                break;
            }
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(30 * attempt));
            }
        }
    }
    if let Some(e) = last_err {
        let _ = fs::remove_file(&temp_path);
        return Err(E::from(e));
    }
    Ok(())
}

// ============ 跨进程配置兼容性（审查项 7.4.2 / P1-23）============
//
// 引导写盘、主程序读盘，两端各有一套手写的 writer/parser，过去**各自测自己**：
// 引导的测试证明"引导能读回自己写的"，主程序的测试同理 —— 于是键名改名、
// 段落搬走、布尔写成 `"1"`/`"true"` 之类漂移只有真机安装时才暴露（而且往往
// 表现为主程序静默用默认值，看不出是谁的锅）。
//
// 下面的测试把两端接起来：**引导的 `ini_text()` 直接喂给主程序的解析器**，
// 断言语义值存活。纯内存、不落盘（`config_dir()` 是真用户目录，测试绝不能碰）。
#[cfg(test)]
mod cross_process_tests {
    use super::*;
    use aaxsyspulse_core::config as core_cfg;
    use aaxsyspulse_core::config::ini::Ini;
    use aaxsyspulse_core::types::{Metric, NameMode, RenderStyle};

    fn fresh_core_config() -> core_cfg::Config {
        core_cfg::Config::default()
    }

    #[test]
    fn system_cfg_survives_main_parser() {
        let mut g = SystemConfig::default();
        // 刻意取"边界值"：负坐标（副屏/顶边外）、非常见透明度、非默认吸附角。
        g.x = -1200;
        g.y = 48;
        g.w = 520;
        g.h = 300;
        g.locked = true;
        g.layout = "vertical".to_string();
        g.interval_ms = 200;
        g.refresh_auto = false;
        g.normal_opacity = 66;
        g.anim_ms = 700;
        g.autostart = true;
        g.debug_log = true;
        g.settings_w = 700;
        g.settings_h = 500;
        g.click_through = true;
        g.snap_corner = 2;
        g.net_unit = "kb".to_string();
        g.lang = "zh".to_string();
        g.admin_check_continue_count = 3;

        let text = g.ini_text();
        let mut c = fresh_core_config();
        core_cfg::system::parse(&mut c, &text);

        assert_eq!(c.window_pos, Some((-1200, 48)), "负坐标被主程序判丢");
        assert_eq!(c.window_size, Some((520, 300)));
        assert!(c.size_locked, "locked 未生效");
        assert_eq!(c.render_style, RenderStyle::Vertical, "layout 锚点漂移");
        assert_eq!(c.interval_ms, 200, "interval_ms 未落在合法档位集合内");
        assert!(!c.refresh_auto);
        assert_eq!(c.normal_opacity, 66);
        assert_eq!(c.anim_ms, 700);
        assert!(c.autostart && c.debug_log && c.click_through);
        assert_eq!(c.snap_corner, 2);
        assert_eq!(c.settings_size, (700, 500));
        assert_eq!(c.admin_check_continue_count, 3);
    }

    #[test]
    fn appearance_cfg_survives_main_parser() {
        let mut g = AppearanceConfig::default();
        g.bg = "#101418".to_string();
        g.fg = "#F2F5F7".to_string();
        g.font_size = 13.5;
        g.font_weight = 1;
        g.metric_gap = 12;
        g.label_value_gap = 4;
        g.shadow = true;
        g.outline = false;

        let mut c = fresh_core_config();
        core_cfg::appearance::parse(&mut c, &g.ini_text());

        assert_eq!((c.bg.r, c.bg.g, c.bg.b), (0x10, 0x14, 0x18), "bg 色值漂移");
        assert_eq!((c.fg.r, c.fg.g, c.fg.b), (0xF2, 0xF5, 0xF7), "fg 色值漂移");
        assert_eq!(c.font_size, 13.5);
        assert_eq!(c.font_weight, 1);
        assert_eq!(c.metric_gap, 12);
        assert_eq!(c.label_value_gap, 4);
        assert!(c.text_shadow, "shadow 布尔写法主程序不认");
        assert!(!c.text_outline);
    }

    #[test]
    fn monitor_cfg_survives_main_parser() {
        let mut g = MonitorConfig::default();
        g.util_decimal = false;
        g.mem_gb = true;
        g.short_names = true;
        g.show_time = true;
        g.time_seconds = false;
        g.items[0].show = true;
        g.items[0].name = false;
        g.items[0].short = 2; // 简写 → NameMode::Simple
        g.items[0].font = 11.5;
        g.items[0].color_l = "#FF8800".to_string();
        g.items[0].color_v = "#0088FF".to_string();
        g.items[1].show = false;
        g.power_enabled = true;
        g.power_query = false;
        g.power_interval_ms = 1500;
        g.exclude_virtual_nic = true;

        let mut c = fresh_core_config();
        core_cfg::monitor::parse(&mut c, &g.ini_text());

        assert!(!c.util_decimal);
        assert!(c.mem_show_gb);
        assert!(c.short_names);
        assert!(!c.time_show_seconds);
        assert_eq!(Metric::ALL.len(), g.items.len(), "监控项数量两端不一致");
        assert!(c.show_cpu, "item.1 show 未生效");
        assert!(!c.show_mem, "item.2 show=false 被读成 true");
        assert!(!c.metric_show_labels[0], "item.1 name 未生效");
        assert_eq!(c.metric_name_modes[0], NameMode::Simple, "short 枚举漂移");
        assert_eq!(c.metric_font_sizes[0], Some(11.5), "逐项字号被吞");
        // 逐项色是 `Option<Rgb>`：None = 跟随全局色，None≠读不到，语义必须区分。
        assert_eq!(
            c.metric_colors[0].label.map(|p| (p.r, p.g, p.b)),
            Some((0xFF, 0x88, 0x00)),
            "逐项名称色 color_l 漂移"
        );
        assert_eq!(
            c.metric_colors[0].value.map(|p| (p.r, p.g, p.b)),
            Some((0x00, 0x88, 0xFF)),
            "逐项数值色 color_v 漂移"
        );
        assert!(c.power_enabled);
        assert!(!c.power_query);
        assert_eq!(c.power_interval_ms, 1500);
        assert!(c.exclude_virtual_nic);
    }

    #[test]
    fn layout_cfg_survives_main_parser() {
        let mut g = LayoutConfig::default();
        g.offsets[0] = [3, -2, 5, 1];
        g.order = vec![4, 1, 2, 3, 5, 6, 7, 8, 9];

        let mut c = fresh_core_config();
        // style 参数在 layout::parse 里不使用（文件名才是布局权威），这里只为满足签名。
        core_cfg::layout::parse(RenderStyle::Strip, &mut c, &g.ini_text());

        assert_eq!(c.metric_offsets[0], [3, -2, 5, 1], "文字偏移分量漂移");
        assert_eq!(c.metric_order[0], 3, "order 的 1-based→0-based 换算漂移");
        assert_eq!(c.metric_order[1], 0);
    }

    /// `version` 的落位口径与主程序 `win::config_io::read_file_version` 一致：
    /// `system.cfg` 在 `[system]` 段，其余四个在 `[meta]` 段。写错段 = 主程序
    /// 视为"缺版本"⇒ 按 v1 走迁移，甚至因跨度过大而拒绝启动。
    #[test]
    fn version_header_matches_main_program_reader() {
        let files = [
            ("system.cfg", SystemConfig::default().ini_text()),
            ("appearance.cfg", AppearanceConfig::default().ini_text()),
            ("monitor.cfg", MonitorConfig::default().ini_text()),
            ("layout_h.cfg", LayoutConfig::default().ini_text()),
            ("layout_v.cfg", LayoutConfig::default().ini_text()),
        ];
        assert_eq!(files.len(), 5, "主程序读 5 个文件，引导也必须补齐");
        for (name, text) in files {
            let ini = Ini::parse(&text);
            let section = if name == "system.cfg" { "system" } else { "meta" };
            let raw = ini
                .get(section, "version")
                .unwrap_or_else(|| panic!("{name} 的 version 不在 [{section}] 段"));
            assert_eq!(
                raw.trim().parse::<u32>().ok(),
                Some(CONFIG_VERSION),
                "{name} 的版本与契约常量不同步"
            );
        }
    }
}
