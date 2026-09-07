//! Configuration module for SysOverlay.
//!
//! Handles parsing, serializing, and validation of user configuration.
//!
//! # 格式策略：宽容读 + 保守写
//!
//! ## 写（serialize）—— 严格 C 版格式
//! 保证 Rust 版写出的配置能被 C 版原样读回（降级回 C 版不丢设置）：
//! - 空格分隔（`topmost 1`，**不是** `key=value`）
//! - `shape`/`vert`/`netunit` 输出**数值**（C 版原生）
//! - `bg`/`fg` 输出 `%06X` 的 **COLORREF（BBGGRR）**
//! - `interval` 输出 **100ms 单位**（`interval 10` = 1000ms）
//!
//! ## 读（parse）—— 两种格式都接受
//! - 同时接受空格分隔与 `key=value`（防御性：手工编辑过的配置不会丢）
//! - 枚举同时接受数值（C 版）与字符串（新版可读）
//! - 颜色按 COLORREF 解析（`from_colorref`，修掉 R/B 互换）
//! - `interval` 按 100ms 单位读入后换算为 ms
//! - **未知键静默忽略**：C 版 parser 亦如此，已实测确认不会崩
//!
//! # 零 panic
//! 每个字段解析失败都保留原值，最后统一 clamp。随机串喂入不会 panic。
//!
//! # 类型来源
//! 领域枚举（[`Zoom`]/[`Shape`]/[`VertLayout`]/[`NetUnit`]/[`RenderMode`]/[`PowerDisplay`]）
//! 与颜色 [`Rgb`] 都不在此处定义 —— 见 [`crate::types`] 与 [`crate::color`]。
//! 这里只做重导出，保证 `config::Shape` 这类旧路径继续可用。

// 枚举与颜色的唯一真源在 `core::types` / `core::color`，此处重导出保持 API 稳定。
pub use crate::color::Rgb;
pub use crate::types::{Lang, Metric, MetricColors, NetUnit, PowerDisplay, RenderMode, RenderStyle, Shape, VertLayout, Zoom};

/// 配置文件格式版本。用于将来格式变更时的迁移锚点。
pub const CONFIG_VERSION: u32 = 2;

/// 允许的刷新间隔（毫秒），从慢到快排列。
/// 对应：0.2fps / 0.5fps / 1fps / 5fps / 10fps。
pub const INTERVAL_MS_ALLOWED: [u32; 5] = [5000, 2000, 1000, 200, 100];

/// 用户配置。
///
/// 字段语义与 C 版 `overlay.c` 的全局变量一一对应；v2 新增字段集中在末尾。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// 配置格式版本，用于将来迁移。
    pub version: u32,
    pub minimal: bool,
    pub zoom: Zoom,
    pub hover: bool,
    pub net_unit: NetUnit,
    pub shape: Shape,
    pub vert: VertLayout,
    pub bg: Rgb,
    pub fg: Rgb,
    pub opacity: u8,
    /// 刷新间隔（**毫秒**）。写入配置时按 C 版语义除以 100。
    /// `refresh_auto` 为 true 时此值仅作为兜底，实际节奏由调度器决定。
    pub interval_ms: u32,
    /// Auto 刷新档（v3 调度器）：true 时刷新上限 = 启用源的最大有效间隔，
    /// 并叠加采样成本自适应降档。默认开启。
    pub refresh_auto: bool,
    pub topmost: bool,
    pub window_pos: Option<(i32, i32)>,
    /// Saved window size (w, h). None means use default CARD_W × CARD_H.
    pub window_size: Option<(i32, i32)>,

    // ── v3 横向条 ────────────────────────────────────────────
    /// 监控项开关（设置里可独立控制，至少保留一项由 UI 层保证）。
    pub show_cpu: bool,
    pub show_mem: bool,
    pub show_gpu: bool,
    pub show_net: bool,
    /// 内存显示模式：false = 百分比，true = 实际占用量（GB，不显示总量）。
    pub mem_show_gb: bool,
    /// 正文字号（像素，8.0..=40.0，0.5 步进）。旧键 `font_scale` 迁移为 ×16。
    pub font_size: f32,
    /// 逐指标字号覆盖（None = 跟随全局 `font_size`）。
    /// 索引与 [`Metric`] 顺序一致：CPU/MEM/GPU/Net/Power。
    pub metric_font_sizes: [Option<f32>; 5],
    /// 指标与指标之间的间距（px，0..=64）。横版 = 水平空隙，竖版 = 上下空隙。
    pub metric_gap: u32,
    /// 每个指标「名称 ↔ 数值」之间的间距（px，0..=64）。
    pub label_value_gap: u32,
    /// 悬停时的透明度（%，0..=100）。别名旧键 `opacity`。
    pub hover_opacity: u8,
    /// 非悬停时的透明度（%，0..=100，默认 35，旧版 hover 关 = 恒悬停档）。
    pub normal_opacity: u8,
    /// 悬停↔非悬停与设置页翻页的过渡动画时长（ms，100..=2000，默认 300）。
    pub anim_ms: u32,
    /// 逐项配色：每个指标的名称/数值色槽，None = 跟随 `fg`。
    pub metric_colors: [MetricColors; 5],
    /// 文字阴影（颜色自动取字体反色：白字黑影 / 黑字白影）。
    pub text_shadow: bool,
    /// 文字描边（1px 四向，颜色同阴影规则）。
    pub text_outline: bool,
    /// 开机自启（HKCU Run 注册表，设置切换时同步写入）。
    pub autostart: bool,

    // ── v2 新增 ──────────────────────────────────────────────
    pub render_mode: RenderMode,
    /// 功耗监控开关。关闭时 PowerSampler 根本不构造（零开销）。
    pub power_enabled: bool,
    pub power_display: PowerDisplay,
    /// 功耗采样间隔（毫秒，绝对值）。实际生效值 = max(本值, interval_ms)。
    pub power_interval_ms: u32,
    /// 是否排除虚拟网卡（WSL2 vEthernet / DockerNAT / Hyper-V switch）。
    pub exclude_virtual_nic: bool,
    /// 调试日志开关。开启后各采样器把原始 API 数据写入
    /// `%LOCALAPPDATA%\SysOverlayC\probe.log`（设置面板可实时切换）。
    pub debug_log: bool,

    // ── v3 新增 ──────────────────────────────────────────────
    /// 渲染风格：Classic（旧 C 版卡片）/ Strip（v3 横向条）。
    pub render_style: RenderStyle,
    /// 界面语言：En / Zh（全界面双语切换）。
    pub lang: Lang,
    /// 全屏游戏自动隐藏悬浮窗（根治 AFMF 合成层破坏）。
    /// 检测到全屏应用时隐藏，退出全屏自动恢复。默认开。
    pub afmf_auto_hide: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            minimal: false,
            zoom: Zoom::Zoom100,
            hover: false,
            net_unit: NetUnit::MB,
            shape: Shape::Round,
            vert: VertLayout::SingleRow,
            bg: Rgb::default(),
            fg: Rgb::new(238, 241, 244),
            opacity: 90,
            interval_ms: 1000,
            refresh_auto: true,
            topmost: true,
            window_pos: None,
            window_size: None,
            show_cpu: true,
            show_mem: true,
            show_gpu: true,
            show_net: true,
            mem_show_gb: false,
            // 32px ≈ 旧默认 font_scale 2（16px 基准 ×2）
            font_size: 32.0,
            metric_font_sizes: [None; 5],
            metric_gap: 8,
            label_value_gap: 6,
            hover_opacity: 90,
            normal_opacity: 35,
            anim_ms: 300,
            metric_colors: [MetricColors::default(); 5],
            text_shadow: true,
            text_outline: false,
            autostart: false,
            render_mode: RenderMode::Quality,
            power_enabled: false,
            power_display: PowerDisplay::CpuPackage,
            power_interval_ms: 3000,
            exclude_virtual_nic: true,
            debug_log: false,
            render_style: RenderStyle::Strip,
            lang: Lang::En,
            afmf_auto_hide: true,
        }
    }
}

impl Config {
    /// 解析布尔值。接受 `1/0/true/false/yes/no/on/off`，失败时返回 `default`。
    fn parse_bool(s: &str, default: bool) -> bool {
        match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        }
    }

    /// 解析 COLORREF 十六进制（配置文件的 `bg`/`fg` 编码）。
    fn parse_colorref(s: &str) -> Option<Rgb> {
        let s = s.trim();
        let s = s.trim_start_matches("0x").trim_start_matches("0X");
        let s = s.trim_start_matches('#');
        if s.is_empty() {
            return None;
        }
        let cr = u32::from_str_radix(s, 16).ok()?;
        Some(Rgb::from_colorref(cr))
    }

    /// 解析配置文本。未知键静默忽略；任何字段解析失败都保留默认值；永不 panic。
    pub fn parse(text: &str) -> Self {
        let mut cfg = Config::default();

        for line in text.lines() {
            let Some((key, value)) = split_kv(line) else {
                continue;
            };
            let key = key.to_ascii_lowercase();

            match key.as_str() {
                "version" => {
                    if let Ok(v) = value.parse::<u32>() {
                        cfg.version = v;
                    }
                }
                "pos" => {
                    let mut it = value.split_whitespace();
                    if let (Some(x), Some(y)) = (it.next(), it.next()) {
                        if let (Ok(x), Ok(y)) = (x.parse::<i32>(), y.parse::<i32>()) {
                            cfg.window_pos = Some((x, y));
                        }
                    }
                }
                "size" => {
                    let mut it = value.split_whitespace();
                    if let (Some(w), Some(h)) = (it.next(), it.next()) {
                        if let (Ok(w), Ok(h)) = (w.parse::<i32>(), h.parse::<i32>()) {
                            if w >= 100 && h >= 60 {
                                cfg.window_size = Some((w, h));
                            }
                        }
                    }
                }
                "show_cpu" => cfg.show_cpu = Self::parse_bool(value, cfg.show_cpu),
                "show_mem" => cfg.show_mem = Self::parse_bool(value, cfg.show_mem),
                "show_gpu" => cfg.show_gpu = Self::parse_bool(value, cfg.show_gpu),
                "show_net" => cfg.show_net = Self::parse_bool(value, cfg.show_net),
                "mem_gb" => cfg.mem_show_gb = Self::parse_bool(value, cfg.mem_show_gb),
                // 新字号（px，0.5 步进）
                "font_size" | "fontsize" => {
                    if let Ok(v) = value.parse::<f32>() {
                        cfg.font_size = (v.clamp(8.0, 40.0) * 2.0).round() / 2.0;
                    }
                }
                // 旧字号（bitmap 缩放倍数）迁移：×16 像素，再钳位到合法范围
                "font_scale" => {
                    if let Ok(v) = value.parse::<u32>() {
                        cfg.font_size = ((v.clamp(1, 4) as f32) * 16.0).clamp(8.0, 40.0);
                    }
                }
                "metric_gap" | "mgap" => {
                    if let Ok(v) = value.parse::<u32>() {
                        cfg.metric_gap = v.clamp(0, 64);
                    }
                }
                "label_value_gap" | "label_gap" | "lvgap" => {
                    if let Ok(v) = value.parse::<u32>() {
                        cfg.label_value_gap = v.clamp(0, 64);
                    }
                }
                "hover_opacity" | "opacity" => {
                    if let Ok(v) = value.parse::<u32>() {
                        cfg.hover_opacity = v.clamp(0, 100) as u8;
                    }
                }
                "normal_opacity" => {
                    if let Ok(v) = value.parse::<u32>() {
                        cfg.normal_opacity = v.clamp(0, 100) as u8;
                    }
                }
                "anim_ms" | "anim" => {
                    if let Ok(v) = value.parse::<u32>() {
                        cfg.anim_ms = v.clamp(0, 500);
                    }
                }
                // 逐项配色：mc_<metric>_lbl|val  <COLORREF|0>
                //  "0"/"off"/"none"/"follow"/"default" → None（跟随全局文字色）
                _ if key.starts_with("mc_") => {
                    if let Some((slot, color)) = parse_metric_color(&key, value) {
                        let idx = match slot.0 {
                            Metric::Cpu => 0,
                            Metric::Mem => 1,
                            Metric::Gpu => 2,
                            Metric::Net => 3,
                            Metric::Power => 4,
                        };
                        let mc = &mut cfg.metric_colors[idx];
                        if slot.1 {
                            mc.label = color;
                        } else {
                            mc.value = color;
                        }
                    }
                }
                // 逐指标字号覆盖：mfs_<metric>  <size|0>（0 = 跟随全局字号）
                _ if key.starts_with("mfs_") => {
                    if let Some(idx) = parse_metric_font_size(&key, value) {
                        cfg.metric_font_sizes[idx.0] = idx.1;
                    }
                }
                "shadow" => cfg.text_shadow = Self::parse_bool(value, cfg.text_shadow),
                "outline" => cfg.text_outline = Self::parse_bool(value, cfg.text_outline),
                "autostart" => cfg.autostart = Self::parse_bool(value, cfg.autostart),
                "topmost" => cfg.topmost = Self::parse_bool(value, cfg.topmost),
                // C 版 interval 是 100ms 单位 → 换算为 ms 并吸附到允许档位。
                "interval" => {
                    if let Ok(v) = value.parse::<u32>() {
                        cfg.interval_ms = snap_interval_ms(v.saturating_mul(100));
                    }
                }
                "refresh_auto" | "auto" => {
                    cfg.refresh_auto = Self::parse_bool(value, cfg.refresh_auto);
                }
                "minimal" => cfg.minimal = Self::parse_bool(value, cfg.minimal),
                // zoom 保持用户原意（Quality 下 5 档全保留），
                // Eco 的两档收敛发生在渲染期（effective_zoom），不在解析期，
                // 否则「parse ∘ serialize 幂等」这条不变式会被破坏。
                "zoom" => {
                    if let Ok(v) = value.parse::<u32>() {
                        cfg.zoom = Zoom::from_config(v);
                    }
                }
                "hover" => cfg.hover = Self::parse_bool(value, cfg.hover),
                "netunit" => {
                    if let Some(u) = NetUnit::from_config(value) {
                        cfg.net_unit = u;
                    }
                }
                "shape" => {
                    if let Some(s) = Shape::from_config(value) {
                        cfg.shape = s;
                    }
                }
                "vert" => {
                    if let Some(v) = VertLayout::from_config(value) {
                        cfg.vert = v;
                    }
                }
                "bg" => {
                    if let Some(c) = Self::parse_colorref(value) {
                        cfg.bg = c;
                    }
                }
                "fg" => {
                    if let Some(c) = Self::parse_colorref(value) {
                        cfg.fg = c;
                    }
                }
                "render_mode" | "rendermode" => {
                    if let Some(m) = RenderMode::from_config(value) {
                        cfg.render_mode = m;
                    }
                }
                "power" | "power_enabled" => {
                    cfg.power_enabled = Self::parse_bool(value, cfg.power_enabled);
                }
                "power_display" => {
                    if let Some(d) = PowerDisplay::from_config(value) {
                        cfg.power_display = d;
                    }
                }
                "power_interval_ms" => {
                    if let Ok(v) = value.parse::<u32>() {
                        cfg.power_interval_ms = v.clamp(500, 60_000);
                    }
                }
                "exclude_virtual_nic" => {
                    cfg.exclude_virtual_nic = Self::parse_bool(value, cfg.exclude_virtual_nic);
                }
                "debug_log" | "debug" => {
                    cfg.debug_log = Self::parse_bool(value, cfg.debug_log);
                }
                "render_style" | "style" => {
                    if let Some(s) = RenderStyle::from_config(value) {
                        cfg.render_style = s;
                    }
                }
                "lang" | "language" => {
                    if let Some(l) = Lang::from_config(value) {
                        cfg.lang = l;
                    }
                }
                "afmf_auto_hide" | "fullscreen_hide" => {
                    cfg.afmf_auto_hide = Self::parse_bool(value, cfg.afmf_auto_hide);
                }
                // 未知键静默忽略：C 版或更新版本的配置文件都能安全加载。
                _ => {}
            }
        }

        cfg
    }

    /// 序列化为 **C 版兼容格式**（空格分隔、数值枚举、COLORREF 颜色、100ms 间隔）。
    ///
    /// v2 新增字段同样以空格分隔输出，已实测 C 版 `loadConfig` 会静默忽略它们。
    ///
    /// 返回写入的字节数与内容；用 [`StrBuf`][crate::units::StrBuf] 之外的
    /// `String` 是因为落盘是低频操作（一次启动一次），不值得为它牺牲可读性。
    pub fn serialize(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("version {}\n", self.version));
        if let Some((x, y)) = self.window_pos {
            s.push_str(&format!("pos {} {}\n", x, y));
        }
        if let Some((w, h)) = self.window_size {
            s.push_str(&format!("size {} {}\n", w, h));
        }
        // v3 横向条
        s.push_str(&format!("show_cpu {}\n", self.show_cpu as u32));
        s.push_str(&format!("show_mem {}\n", self.show_mem as u32));
        s.push_str(&format!("show_gpu {}\n", self.show_gpu as u32));
        s.push_str(&format!("show_net {}\n", self.show_net as u32));
        s.push_str(&format!("mem_gb {}\n", self.mem_show_gb as u32));
        s.push_str(&format!("font_size {:.1}\n", self.font_size));
        s.push_str(&format!("metric_gap {}\n", self.metric_gap));
        s.push_str(&format!("label_value_gap {}\n", self.label_value_gap));
        s.push_str(&format!("shadow {}\n", self.text_shadow as u32));
        s.push_str(&format!("outline {}\n", self.text_outline as u32));
        s.push_str(&format!("autostart {}\n", self.autostart as u32));
        s.push_str(&format!("topmost {}\n", self.topmost as u32));
        // 旧键 opacity 继续输出（语义 = 悬停透明度），C 版读得动
        s.push_str(&format!("opacity {}\n", self.hover_opacity));
        s.push_str(&format!("normal_opacity {}\n", self.normal_opacity));
        s.push_str(&format!("anim_ms {}\n", self.anim_ms));
        // C 版单位是 100ms
        s.push_str(&format!("interval {}\n", self.interval_ms / 100));
        s.push_str(&format!("refresh_auto {}\n", self.refresh_auto as u32));
        s.push_str(&format!("minimal {}\n", self.minimal as u32));
        s.push_str(&format!("zoom {}\n", self.zoom.to_config()));
        s.push_str(&format!("hover {}\n", self.hover as u32));
        s.push_str(&format!("netunit {}\n", self.net_unit.to_config()));
        s.push_str(&format!("shape {}\n", self.shape.to_config()));
        s.push_str(&format!("vert {}\n", self.vert.to_config()));
        s.push_str(&format!("bg {:06X}\n", self.bg.to_colorref()));
        s.push_str(&format!("fg {:06X}\n", self.fg.to_colorref()));
        // v2 新增字段（C 版忽略，不影响降级回 C 版）
        s.push_str(&format!("render_mode {}\n", self.render_mode.to_config()));
        s.push_str(&format!("power {}\n", self.power_enabled as u32));
        s.push_str(&format!("power_display {}\n", self.power_display.to_config()));
        s.push_str(&format!("power_interval_ms {}\n", self.power_interval_ms));
        s.push_str(&format!(
            "exclude_virtual_nic {}\n",
            self.exclude_virtual_nic as u32
        ));
        s.push_str(&format!("debug_log {}\n", self.debug_log as u32));
        s.push_str(&format!("render_style {}\n", self.render_style.to_config()));
        s.push_str(&format!("lang {}\n", self.lang.to_config()));
        s.push_str(&format!("afmf_auto_hide {}\n", self.afmf_auto_hide as u32));
        // 逐项配色：只有非跟随全局的色槽才落盘
        for (i, metric) in Metric::ALL.iter().enumerate() {
            let mc = &self.metric_colors[i];
            if let Some(c) = mc.label {
                s.push_str(&format!("mc_{}_lbl {:06X}\n", metric.key_name(), c.to_colorref()));
            }
            if let Some(c) = mc.value {
                s.push_str(&format!("mc_{}_val {:06X}\n", metric.key_name(), c.to_colorref()));
            }
        }
        // 逐指标字号覆盖：只有非跟随全局的槽才落盘
        for (i, metric) in Metric::ALL.iter().enumerate() {
            if let Some(size) = self.metric_font_sizes[i] {
                s.push_str(&format!("mfs_{} {:.1}\n", metric.key_name(), size));
            }
        }
        s
    }

    /// 实际生效的缩放档位。
    ///
    /// 配置里存的是**用户意图**（Quality 下 5 档全可存），
    /// Eco 模式在渲染时才收敛到 {100%, 200%}（v2 Q9）。
    pub fn effective_zoom(&self) -> Zoom {
        self.zoom.clamp_for(self.render_mode)
    }

    /// 功耗采样实际生效间隔（毫秒）。
    ///
    /// v2 Q8：`power_interval_ms` 是**绝对值**，但不能比主间隔更密
    /// （否则功耗采样会反过来拖累主循环）。生效值 = `max(power_interval_ms, interval_ms)`。
    pub fn effective_power_interval_ms(&self) -> u32 {
        self.power_interval_ms.max(self.interval_ms)
    }

    /// 是否显示功耗行。
    pub fn show_power(&self) -> bool {
        self.power_enabled
    }
}

/// 切分一行配置为 (key, value)。
///
/// 同时支持：
/// - 空格分隔（C 版原生）：`topmost 1`、`pos 1200 40`（value 含空格）
/// - 等号分隔（手工编辑/未来格式）：`topmost=1`
///
/// 返回 `None` 表示空行或注释行。
fn split_kv(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
        return None;
    }

    if let Some(eq) = line.find('=') {
        let (k, rest) = line.split_at(eq);
        return Some((k.trim(), rest[1..].trim()));
    }

    // 空格分隔：key 是第一个 token，其余整体作为 value（供 "pos x y" 使用）
    let mut it = line.splitn(2, char::is_whitespace);
    let k = it.next().unwrap_or("").trim();
    let v = it.next().unwrap_or("").trim();
    if k.is_empty() {
        return None;
    }
    Some((k, v))
}

/// 把任意毫秒值吸附到 [`INTERVAL_MS_ALLOWED`] 中最近的一档。
fn snap_interval_ms(ms: u32) -> u32 {
    let mut best = INTERVAL_MS_ALLOWED[0];
    let mut best_dist = u32::MAX;
    for &allowed in &INTERVAL_MS_ALLOWED {
        let dist = if ms > allowed { ms - allowed } else { allowed - ms };
        if dist < best_dist {
            best_dist = dist;
            best = allowed;
        }
    }
    best
}

/// 解析逐项配色的键与值。
///
/// 键形如 `mc_<metric>_lbl` / `mc_<metric>_val`（metric ∈ cpu/mem/gpu/net/power）。
/// 返回 `Some((slot, Option<Rgb>))`：`slot.1 == true` 表示名称槽（label），
/// 否则为数值槽（value）。值 `0`/`off`/`none`/`follow`/`default` → `None`（跟随全局）。
/// 未知指标名或非法值返回 `None`（静默忽略）。
fn parse_metric_color(key: &str, value: &str) -> Option<((Metric, bool), Option<Rgb>)> {
    // 去掉 "mc_" 前缀后剩余 <metric>_lbl|val
    let rest = key.strip_prefix("mc_")?;
    let (metric_part, slot_part) = rest.rsplit_once('_')?;
    let metric = match metric_part {
        "cpu" => Metric::Cpu,
        "mem" => Metric::Mem,
        "gpu" => Metric::Gpu,
        "net" => Metric::Net,
        "power" => Metric::Power,
        _ => return None,
    };
    let slot = match slot_part {
        "lbl" | "label" => true,
        "val" | "value" => false,
        _ => return None,
    };
    let v = value.trim();
    let color = match v.to_ascii_lowercase().as_str() {
        "0" | "off" | "none" | "follow" | "default" => None,
        _ => {
            let cr = u32::from_str_radix(v.trim_start_matches("0x").trim_start_matches('#'), 16).ok()?;
            Some(Rgb::from_colorref(cr))
        }
    };
    Some(((metric, slot), color))
}

/// 解析逐指标字号覆盖的键与值。
///
/// 键形如 `mfs_<metric>`（metric ∈ cpu/mem/gpu/net/power）。
/// 返回 `Some((idx, Option<f32>))`：`idx` 为 [`Metric`] 数组下标，
/// 值 `0`/`off`/`none`/`follow`/`default` → `None`（跟随全局字号）。
/// 未知指标名或非法值返回 `None`（静默忽略）。
fn parse_metric_font_size(key: &str, value: &str) -> Option<(usize, Option<f32>)> {
    let rest = key.strip_prefix("mfs_")?;
    let idx = match rest {
        "cpu" => 0,
        "mem" => 1,
        "gpu" => 2,
        "net" => 3,
        "power" => 4,
        _ => return None,
    };
    let v = value.trim();
    let size = match v.to_ascii_lowercase().as_str() {
        "0" | "off" | "none" | "follow" | "default" => None,
        _ => {
            let s = (v.parse::<f32>().ok()?.clamp(8.0, 40.0) * 2.0).round() / 2.0;
            Some(s)
        }
    };
    Some((idx, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── C 版格式（v1/v2 兼容的核心） ─────────────────────────

    /// 这是一份**真实 C 版**格式的配置（数值取自 overlay.c 默认值）。
    const C_REAL_CONFIG: &str = "\
pos 1200 40
topmost 1
opacity 90
interval 10
minimal 0
zoom 100
hover 0
netunit 1
shape 0
vert 1
bg 1B1614
fg F4F1EE
";

    #[test]
    fn test_parse_real_c_config() {
        let cfg = Config::parse(C_REAL_CONFIG);

        assert_eq!(cfg.window_pos, Some((1200, 40)), "pos 必须能被读回（此前写而不读）");
        assert!(cfg.topmost);
        assert_eq!(cfg.hover_opacity, 90, "旧键 opacity → 悬停透明度");
        assert_eq!(cfg.normal_opacity, 35, "没有 normal_opacity 行时用默认 35");
        assert_eq!(cfg.interval_ms, 1000, "C 版 interval 10 = 10×100ms");
        assert!(!cfg.minimal);
        assert_eq!(cfg.zoom, Zoom::Zoom100);
        assert!(!cfg.hover);
        assert_eq!(cfg.net_unit, NetUnit::MB, "C 版 netunit 1 = MB/s");
        assert_eq!(cfg.shape, Shape::Round, "C 版 shape 0 = 圆角");
        assert_eq!(cfg.vert, VertLayout::SingleCol, "C 版 vert 1 = 单列");
        // COLORREF(BBGGRR)：0x1B1614 → R=0x14=20, G=0x16=22, B=0x1B=27
        assert_eq!(cfg.bg, Rgb::new(20, 22, 27), "bg 必须按 COLORREF 解析，不能 R/B 互换");
        assert_eq!(cfg.fg, Rgb::new(238, 241, 244));
    }

    #[test]
    fn test_serialize_matches_c_format() {
        let cfg = Config::default();
        let s = cfg.serialize();

        // 空格分隔、数值枚举、COLORREF、100ms 间隔
        assert!(s.contains("topmost 1\n"), "必须是空格分隔，实际:\n{}", s);
        assert!(s.contains("interval 10\n"), "interval 必须是 100ms 单位");
        assert!(s.contains("shape 0\n"), "shape 必须是数值");
        assert!(s.contains("vert 0\n"), "vert 必须是数值");
        assert!(s.contains("netunit 1\n"), "netunit 必须是数值");
        assert!(s.contains("bg 1B1614\n"), "bg 必须是 COLORREF(BBGGRR)");
        assert!(s.contains("fg F4F1EE\n"));
        assert!(!s.contains('='), "默认写出的配置不应含 '='");
    }

    /// 关键回归：非默认配置往返必须一致。
    /// 旧版 `test_config_serialize_roundtrip` 只用默认配置，导致假阳性绿。
    #[test]
    fn test_roundtrip_non_default() {
        let mut cfg = Config::default();
        cfg.topmost = false;
        cfg.hover_opacity = 40;
        cfg.normal_opacity = 15;
        cfg.font_size = 20.5;
        cfg.metric_gap = 24;
        cfg.label_value_gap = 12;
        cfg.anim_ms = 300;
        cfg.interval_ms = 2000;
        cfg.minimal = true;
        cfg.zoom = Zoom::Zoom150;
        cfg.hover = true;
        cfg.net_unit = NetUnit::GB;
        cfg.shape = Shape::Content;
        cfg.vert = VertLayout::SplitCol;
        cfg.bg = Rgb::new(0x1A, 0x2B, 0x3C);
        cfg.fg = Rgb::new(0xF0, 0xF1, 0xF2);
        cfg.window_pos = Some((800, 600));
        cfg.render_mode = RenderMode::Eco;
        cfg.power_enabled = true;
        cfg.power_display = PowerDisplay::Battery;
        cfg.power_interval_ms = 5000;
        cfg.exclude_virtual_nic = false;
        cfg.render_style = RenderStyle::Vertical;
        cfg.metric_colors[0] = MetricColors {
            label: Some(Rgb::new(0x12, 0x34, 0x56)),
            value: None,
        };
        cfg.metric_colors[3] = MetricColors {
            label: None,
            value: Some(Rgb::new(0xAB, 0xCD, 0xEF)),
        };

        let text = cfg.serialize();
        let parsed = Config::parse(&text);
        assert_eq!(cfg, parsed, "非默认配置往返失败，实际输出:\n{}", text);
    }

    #[test]
    fn test_roundtrip_all_zoom_and_shapes() {
        for zoom in Zoom::ALL {
            for shape in [Shape::Round, Shape::Square, Shape::Content] {
                for vert in [VertLayout::SingleRow, VertLayout::SingleCol, VertLayout::SplitCol] {
                    for unit in [NetUnit::KB, NetUnit::MB, NetUnit::GB] {
                        let mut cfg = Config::default();
                        cfg.zoom = zoom;
                        cfg.shape = shape;
                        cfg.vert = vert;
                        cfg.net_unit = unit;
                        assert_eq!(cfg, Config::parse(&cfg.serialize()));
                    }
                }
            }
        }
    }

    /// C 版 5 档 zoom 必须全部能往返（不能被静默丢档）。
    #[test]
    fn test_all_c_zoom_levels_roundtrip() {
        for pct in [75, 100, 125, 150, 200] {
            let cfg = Config::parse(&format!("zoom {}", pct));
            assert_eq!(cfg.zoom.to_config(), pct, "zoom {} 被丢档", pct);
        }
    }

    // ── 单位换算 ───────────────────────────────────────────

    #[test]
    fn test_interval_is_100ms_units() {
        // C 版 INTERVAL_DDJ = {5,10,20} → {500,1000,2000} ms
        // v2 档位集 = {5000,2000,1000,200,100}（0.2/0.5/1/5/10 fps），
        // 不在集合中的旧值吸附到最近的档位。
        assert_eq!(Config::parse("interval 5").interval_ms, 200); // 500 → 200
        assert_eq!(Config::parse("interval 10").interval_ms, 1000);
        assert_eq!(Config::parse("interval 20").interval_ms, 2000);

        // 扩展档位
        assert_eq!(Config::parse("interval 2").interval_ms, 200);
        assert_eq!(Config::parse("interval 1").interval_ms, 100);
        assert_eq!(Config::parse("interval 50").interval_ms, 5000);
    }

    #[test]
    fn test_interval_snaps_to_allowed() {
        // 非档位值吸附到最近允许值
        assert_eq!(Config::parse("interval 7").interval_ms, 1000); // 700 → 1000
        assert_eq!(Config::parse("interval 25").interval_ms, 2000); // 2500 → 2000
        assert_eq!(Config::parse("interval 30").interval_ms, 2000); // 3000 → 2000
        // 越界值被吸附，不会溢出或产生荒谬间隔
        assert_eq!(Config::parse("interval 1").interval_ms, 100);
        assert_eq!(Config::parse("interval 99999").interval_ms, 5000);
    }

    #[test]
    fn test_interval_roundtrip_all_allowed() {
        for ms in INTERVAL_MS_ALLOWED {
            let mut cfg = Config::default();
            cfg.interval_ms = ms;
            assert_eq!(cfg, Config::parse(&cfg.serialize()), "interval {}ms 往返失败", ms);
        }
    }

    // ── 颜色 ───────────────────────────────────────────────

    #[test]
    fn test_color_is_colorref_not_rrggbb() {
        // 这是此前 R/B 互换 bug 的直接回归测试
        let cfg = Config::parse("bg 1B1614");
        assert_eq!(cfg.bg.r, 0x14, "COLORREF 低字节是 R");
        assert_eq!(cfg.bg.g, 0x16);
        assert_eq!(cfg.bg.b, 0x1B, "COLORREF 高字节是 B");
    }

    #[test]
    fn test_color_accepts_0x_prefix() {
        assert_eq!(Config::parse("bg 0x1B1614").bg, Rgb::new(20, 22, 27));
    }

    #[test]
    fn test_invalid_color_keeps_default() {
        let cfg = Config::parse("bg ZZZZZZ\nfg\n");
        // 注意：Rgb::default() 是背景色，fg 的默认值不同，必须分开断言
        assert_eq!(cfg.bg, Rgb::new(20, 22, 27));
        assert_eq!(cfg.fg, Rgb::new(238, 241, 244));
    }

    // ── 宽容读 ─────────────────────────────────────────────

    #[test]
    fn test_key_value_format_also_accepted() {
        let cfg = Config::parse("topmost=0\nopacity=70\nshape=square\nnetunit=gb\nvert=splitcol\n");
        assert!(!cfg.topmost);
        assert_eq!(cfg.hover_opacity, 70);
        assert_eq!(cfg.shape, Shape::Square);
        assert_eq!(cfg.net_unit, NetUnit::GB);
        assert_eq!(cfg.vert, VertLayout::SplitCol);
    }

    #[test]
    fn test_enums_accept_both_numeric_and_string() {
        assert_eq!(Shape::from_config("0"), Shape::from_config("round"));
        assert_eq!(Shape::from_config("2"), Shape::from_config("Content"));
        assert_eq!(VertLayout::from_config("2"), VertLayout::from_config("split_col"));
        assert_eq!(NetUnit::from_config("2"), NetUnit::from_config("GB"));
        assert_eq!(RenderMode::from_config("0"), RenderMode::from_config("eco"));
        assert_eq!(PowerDisplay::from_config("1"), PowerDisplay::from_config("battery"));
    }

    #[test]
    fn test_unknown_keys_ignored() {
        // C 版亦静默忽略；v2 新字段对旧版无损
        let cfg = Config::parse("this_key_does_not_exist 42\nanother=whatever\n");
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn test_comments_and_blank_lines() {
        let cfg = Config::parse("# comment\n\n; another\n\ntopmost 0\n");
        assert!(!cfg.topmost);
    }

    // ── 零 panic ───────────────────────────────────────────

    #[test]
    fn test_garbage_never_panics() {
        // 各种畸形输入都必须安静地返回合法配置
        let inputs = [
            "",
            "\n\n\n",
            "====",
            "topmost",
            "topmost ",
            "pos",
            "pos 1",
            "pos abc def",
            "interval -5",
            "interval 99999999999999999999",
            "opacity -1",
            "opacity 999999999999",
            "zoom -1",
            "bg",
            "bg 0xFFFFFFFFFF",
            "bg -1",
            "power_interval_ms -99999",
            "render_mode ???",
            "\u{0}\u{1}\u{2}",
            "topmost\u{0}1",
        ];
        for input in inputs {
            let cfg = Config::parse(input);
            // 只要不 panic 且值域合法即可
            assert!((0..=100).contains(&cfg.hover_opacity), "hover_opacity 越界: {}", input);
            assert!((0..=100).contains(&cfg.normal_opacity), "normal_opacity 越界: {}", input);
            assert_eq!(cfg.font_size, (cfg.font_size.clamp(8.0, 40.0) * 2.0).round() / 2.0, "字号 0.5 步进: {}", input);
            assert!(
                INTERVAL_MS_ALLOWED.contains(&cfg.interval_ms),
                "interval 未吸附: {} → {}",
                input,
                cfg.interval_ms
            );
            // 序列化后必须能再解析回来（不变式：parse ∘ serialize 收敛）
            let reparsed = Config::parse(&cfg.serialize());
            assert_eq!(cfg, reparsed, "序列化后往返不一致: {}", input);
        }
    }

    #[test]
    fn test_parse_then_serialize_is_stable() {
        // 幂等性：parse → serialize → parse 必须收敛（第二遍不再变化）
        let once = Config::parse(C_REAL_CONFIG).serialize();
        let twice = Config::parse(&once).serialize();
        assert_eq!(once, twice);
    }

    // ── v2 新增语义 ────────────────────────────────────────

    #[test]
    fn test_effective_zoom_clamps_only_in_eco() {
        let mut cfg = Config::default();
        cfg.zoom = Zoom::Zoom125;

        cfg.render_mode = RenderMode::Quality;
        assert_eq!(cfg.effective_zoom(), Zoom::Zoom125, "Quality 保留 5 档");

        cfg.render_mode = RenderMode::Eco;
        assert_eq!(cfg.effective_zoom(), Zoom::Zoom100, "Eco 只留 100/200");
    }

    #[test]
    fn test_effective_power_interval_is_max_of_both() {
        // v2 §2.7/Q8：功耗间隔是绝对值，但不小于主间隔
        let mut cfg = Config::default();
        cfg.interval_ms = 2000;
        cfg.power_interval_ms = 3000;
        assert_eq!(cfg.effective_power_interval_ms(), 3000);

        cfg.interval_ms = 5000;
        assert_eq!(cfg.effective_power_interval_ms(), 5000, "主间隔更长时以主间隔为准");

        cfg.interval_ms = 1000;
        assert_eq!(cfg.effective_power_interval_ms(), 3000);
    }

    #[test]
    fn test_show_power_follows_switch() {
        let mut cfg = Config::default();
        assert!(!cfg.show_power(), "默认关闭（零开销原则）");
        cfg.power_enabled = true;
        assert!(cfg.show_power());
    }

    // ── v3 调度器 ───────────────────────────────────────────

    #[test]
    fn test_refresh_auto_default_on() {
        assert!(Config::default().refresh_auto, "Auto 是默认档");
        // C 版配置没有 refresh_auto 行 → 落到默认值 true
        assert!(Config::parse(C_REAL_CONFIG).refresh_auto);
    }

    #[test]
    fn test_refresh_auto_roundtrip() {
        let mut cfg = Config::default();
        cfg.refresh_auto = false;
        cfg.interval_ms = 200;
        assert_eq!(Config::parse(&cfg.serialize()), cfg);
        assert!(Config::parse("refresh_auto 0").interval_ms == 1000
            && !Config::parse("refresh_auto 0").refresh_auto);
        // 兼容短键名 auto
        assert!(Config::parse("auto 1").refresh_auto);
        assert!(!Config::parse("auto 0").refresh_auto);
    }

    // ── 保留的原有测试 ─────────────────────────────────────

    #[test]
    fn test_rgb_default() {
        let c = Rgb::default();
        assert_eq!(c.r, 20);
        assert_eq!(c.g, 22);
        assert_eq!(c.b, 27);
    }

    #[test]
    fn test_config_default() {
        let cfg = Config::default();
        assert!(!cfg.minimal);
        assert_eq!(cfg.zoom, Zoom::Zoom100);
        assert_eq!(cfg.hover_opacity, 90);
        assert_eq!(cfg.normal_opacity, 35);
        assert_eq!(cfg.font_size, 32.0);
        assert_eq!(cfg.metric_gap, 8);
        assert_eq!(cfg.label_value_gap, 6);
        assert_eq!(cfg.anim_ms, 150);
        assert!(cfg.topmost);
        assert_eq!(cfg.version, CONFIG_VERSION);
        assert!(!cfg.power_enabled, "功耗默认关闭（零开销原则）");
        assert_eq!(cfg.render_mode, RenderMode::Quality, "默认 Quality（首次印象）");
    }

    #[test]
    fn test_zoom_parsing_falls_back_to_100() {
        assert_eq!(Config::parse("zoom 100").zoom, Zoom::Zoom100);
        assert_eq!(Config::parse("zoom 200").zoom, Zoom::Zoom200);
        // 非法值吸附到最近档位而非静默丢弃
        assert_eq!(Config::parse("zoom 130").zoom, Zoom::Zoom125);
    }

    // ── v3 渲染风格 ───────────────────────────────────────

    #[test]
    fn test_render_style_default_is_strip() {
        assert_eq!(Config::default().render_style, RenderStyle::Strip);
        // 旧配置文件没有 render_style 键 → 落到默认 strip，老用户迁移无损
        assert_eq!(Config::parse(C_REAL_CONFIG).render_style, RenderStyle::Strip);
    }

    #[test]
    fn test_render_style_roundtrip_both_ways() {
        let mut cfg = Config::default();
        cfg.render_style = RenderStyle::Strip;
        assert_eq!(Config::parse(&cfg.serialize()), cfg);

        cfg.render_style = RenderStyle::Vertical;
        assert_eq!(Config::parse(&cfg.serialize()), cfg);
    }

    #[test]
    fn test_render_style_accepts_numeric_and_string() {
        // 卡片式（旧 0/classic）删除后迁移为横版条状，不再保留
        assert_eq!(
            Config::parse("render_style 0").render_style,
            RenderStyle::Strip
        );
        assert_eq!(
            Config::parse("render_style classic").render_style,
            RenderStyle::Strip
        );
        assert_eq!(Config::parse("render_style 1").render_style, RenderStyle::Strip);
        assert_eq!(Config::parse("style strip").render_style, RenderStyle::Strip);
        assert_eq!(
            Config::parse("render_style vertical").render_style,
            RenderStyle::Vertical
        );
        assert_eq!(Config::parse("style 2").render_style, RenderStyle::Vertical);
        // 未知值保留默认，不 panic
        assert_eq!(Config::parse("render_style ???").render_style, RenderStyle::Strip);
    }

    // ── v4 逐指标字号覆盖 ────────────────────────────────────

    #[test]
    fn test_metric_font_size_default_follows_global() {
        let cfg = Config::default();
        assert_eq!(cfg.metric_font_sizes, [None; 5], "默认全部跟随全局字号");
    }

    #[test]
    fn test_metric_font_size_parse_and_serialize_roundtrip() {
        let mut cfg = Config::default();
        cfg.metric_font_sizes[0] = Some(24.0); // CPU
        cfg.metric_font_sizes[3] = Some(16.5); // Net
        let text = cfg.serialize();
        let parsed = Config::parse(&text);
        assert_eq!(parsed, cfg, "逐指标字号往返失败，实际输出:\n{}", text);
        assert!(text.contains("mfs_cpu 24.0\n"));
        assert!(text.contains("mfs_net 16.5\n"));
        // 跟随全局的槽不落盘
        assert!(!text.contains("mfs_mem"));
        assert!(!text.contains("mfs_gpu"));
        assert!(!text.contains("mfs_power"));
    }

    #[test]
    fn test_metric_font_size_accepts_follow_and_clamps() {
        let cfg = Config::parse("mfs_cpu follow\nmfs_gpu 999\nmfs_mem 12.25");
        assert_eq!(cfg.metric_font_sizes[0], None, "follow → None");
        assert_eq!(cfg.metric_font_sizes[2], Some(40.0), "越界 clamp 到 40");
        assert_eq!(cfg.metric_font_sizes[1], Some(12.5), "0.5 步进四舍五入 (12.25→12.5)");
        assert_eq!(cfg.metric_font_sizes[3], None, "未指定的槽保持跟随");
    }

    #[test]
    fn test_metric_font_size_unknown_ignored() {
        let cfg = Config::parse("mfs_xxx 20.0\nmfs");
        assert_eq!(cfg.metric_font_sizes, [None; 5]);
    }
}
