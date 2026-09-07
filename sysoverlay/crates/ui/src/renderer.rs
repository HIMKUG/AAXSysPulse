//! 渲染器抽象与横/竖两种条状实现（v4）。
//!
//! # 职责
//!
//! 把一份 [`Snapshot`] + [`Config`] 画进离屏 [`SoftwareSurface`]。
//! 平台层（`win`）只面向 [`Renderer`] trait：选渲染器 → 画帧 → 上屏，
//! 不接触任何具体绘制细节。渲染逻辑全部落在 `ui`（零 Win32、可单测）。
//!
//! # 两种实现（v4，卡片式已删除）
//!
//! - [`StripRenderer`]：横版文字条（各指标左到右一字排开）。默认。
//! - [`VerticalRenderer`]：竖版文字条（同内容纵向排列）。
//!
//! 两者共享同一套"指标单元"构建逻辑（[`MetricRow`]）：
//! - 每个指标 = 名称 + [`Config::label_value_gap`]px 间距 + 数值（右对齐到固定槽位）
//! - 指标间以 [`Config::metric_gap`]px 分隔（横版=水平、竖版=上下）
//! - 字号：全局 [`Config::font_size`]，可被 [`Config::metric_font_sizes`] 逐指标覆盖
//! - 颜色：名称/数值分别允许 [`Config::metric_colors`] 逐槽覆盖（索引 = core::Metric）
//!
//! # 脏检查接口
//!
//! [`Renderer::frame_hash`] 返回"像素不变的门面指纹"。平台层逐帧比较指纹，
//! 相同则跳过整帧（不建画布、不 `UpdateLayeredWindow`），静止场景开销归零。
//! 指纹必须覆盖本渲染器**全部**会改变像素的输入（含全局 alpha ——
//! 透明度/悬停变化必须触发重绘）。

use crate::font::{measure_scaled_f, render_str_scaled_f};
use crate::offscreen::SoftwareSurface;
use crate::palette::Style;
use crate::Rgba;
use sysoverlay_core::color::Rgb;
use sysoverlay_core::config::Config;
use sysoverlay_core::snapshot::{Metric as SnapMetric, Snapshot};
use sysoverlay_core::types::Metric as MetricEnum;
use sysoverlay_core::units::{fmt_gb, fmt_pct, fmt_power, fmt_rate, StrBuf};

/// 渲染器接口：画一帧离屏内容（零 Win32、零 unsafe）。
pub trait Renderer {
    /// 稳定名称（诊断/测试用）。
    fn name(&self) -> &'static str;

    /// 画一帧到 `out`。`out` 的尺寸由平台层决定（窗口当前大小），
    /// 渲染器负责在画布内排布内容；返回**自然尺寸**（供窗口自适应）。
    fn draw(&self, out: &mut SoftwareSurface, snap: &Snapshot, cfg: &Config) -> (i32, i32);

    /// 像素不变的门面指纹（含全局 alpha）。相同 → 平台层跳过整帧。
    fn frame_hash(&self, snap: &Snapshot, cfg: &Config, w: i32, h: i32, alpha: u8) -> u64;

    /// 不绘制的自然尺寸（窗口自适应 / 启动定位用）。
    fn natural_size(&self, cfg: &Config) -> (i32, i32);
}

// ── FNV-1a 64 位折叠（两个渲染器共用） ─────────────────────

const FNV_OFFSET: u64 = 0xcbf29ce4_84222325;
const FNV_PRIME: u64 = 0x1000_0000_01b3;

fn fnv_mix(acc: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *acc ^= b as u64;
        *acc = acc.wrapping_mul(FNV_PRIME);
    }
}

fn fnv_u32(acc: &mut u64, v: u32) {
    fnv_mix(acc, &v.to_le_bytes());
}

fn fnv_u8(acc: &mut u64, v: u8) {
    fnv_mix(acc, &[v]);
}

fn fnv_f32(acc: &mut u64, v: f32) {
    fnv_u32(acc, v.to_bits());
}

// ── 共享：指标单元 ─────────────────────────────────────────

/// 槽位宽度（字符数）——按各类数值的**最大**模板串宽取值：
/// 百分比 `100%`=4；GB 占用 `999.9GB`=7；网速 `888.8M/s`=8；功耗 `~8888W`=6。
pub const SLOT_PCT: u32 = 4;
pub const SLOT_GB: u32 = 7;
pub const SLOT_RATE: u32 = 8;
pub const SLOT_POWER: u32 = 6;
/// 左右内边距（像素）。
pub const PAD_X: i32 = 8;
/// 上下内边距（像素）。
pub const PAD_Y: i32 = 6;

/// 一个指标单元的**最终显示数据**（构建后即不可变，供绘制/测宽/哈希共用）。
pub struct MetricRow {
    /// 指标种类（配色槽位索引 = `metric.to_core()`）。
    pub metric: MetricEnum,
    /// 名称文本（短名，如 "CPU"）。
    pub label: StrBuf,
    /// 该项实际生效的字号缩放比。
    pub scale: f32,
    /// 该项实际生效的字号（像素，诊断/哈希用）。
    pub font_size: f32,
    /// 数值槽位宽度（字符数，横版右对齐用）。
    pub slot: u32,
    /// 名称颜色。
    pub label_color: Rgba,
    /// 数值颜色。
    pub value_color: Rgba,
    /// 数值文本（钉在固定槽位内右对齐）。
    pub value: StrBuf,
    /// 该项「名称 ↔ 数值」间距（像素）。
    pub label_value_gap: i32,
}

/// 把 snapshot 指标归一化成 (数值, 是否陈旧, 是否可用)。
fn metric_parts(m: &SnapMetric<f32>) -> (f32, bool, bool) {
    match m {
        SnapMetric::Value(v) => (*v, false, true),
        SnapMetric::Stale { value, .. } => (*value, true, true),
        SnapMetric::Unavailable => (0.0, false, false),
    }
}

/// 阴影/描边颜色：字体亮 → 黑影，字体暗 → 白影（与字体相反）。
fn shade_color(fg: Rgb) -> Rgba {
    let lum = 0.299 * fg.r as f32 + 0.587 * fg.g as f32 + 0.114 * fg.b as f32;
    if lum >= 128.0 {
        Rgba::opaque(0, 0, 0)
    } else {
        Rgba::opaque(255, 255, 255)
    }
}

/// core::Metric 在 [`Config::metric_colors`] / [`Config::metric_font_sizes`] 里的下标。
fn metric_index(m: MetricEnum) -> usize {
    match m {
        MetricEnum::Cpu => 0,
        MetricEnum::Mem => 1,
        MetricEnum::Gpu => 2,
        MetricEnum::Net => 3,
        MetricEnum::Power => 4,
    }
}

/// 名称/数值配色：指标专属色优先，否则回退到 fallback。
fn slot_color(cfg: &Config, m: MetricEnum, label: bool, fallback: Rgba) -> Rgba {
    let over = if label {
        cfg.metric_colors[metric_index(m)].label
    } else {
        cfg.metric_colors[metric_index(m)].value
    };
    match over {
        Some(c) => Rgba::opaque(c.r, c.g, c.b),
        None => fallback,
    }
}

/// 指标专属字号（像素）：逐指标覆盖优先，否则全局。
fn font_size_for(cfg: &Config, m: MetricEnum) -> f32 {
    cfg.metric_font_sizes[metric_index(m)].unwrap_or(cfg.font_size)
}

/// 字号 → 渲染缩放比。
fn font_scale_of(size: f32) -> f32 {
    crate::font::scale_of_font(size)
}

/// 汇总按 cfg 的所有指标单元（顺序即悬浮窗的显示顺序）。
fn build_rows(snap: &Snapshot, cfg: &Config) -> Vec<MetricRow> {
    let style = Style::derive(Rgb::default(), cfg.fg);
    let mut rows: Vec<MetricRow> = Vec::new();

    if cfg.show_cpu {
        let (v, stale, ok) = metric_parts(&snap.cpu);
        let mut b = StrBuf::new();
        if ok {
            fmt_pct(&mut b, v);
        } else {
            b.push_str("--");
        }
        let color = if stale {
            style.ui.value_stale
        } else {
            style.accent(v)
        };
        rows.push(new_row(
            MetricEnum::Cpu,
            "CPU",
            b.clone(),
            SLOT_PCT,
            cfg,
            style.ui.name,
            color,
        ));
    }

    if cfg.show_mem {
        let mut b = StrBuf::new();
        let slot = if cfg.mem_show_gb {
            // 实际占用量，不显示总量：如 `8.5GB`
            fmt_gb(&mut b, snap.memory.used_gb);
            SLOT_GB
        } else {
            fmt_pct(&mut b, snap.memory.load_pct);
            SLOT_PCT
        };
        let color = if cfg.mem_show_gb {
            style.ui.value
        } else {
            style.accent(snap.memory.load_pct)
        };
        rows.push(new_row(
            MetricEnum::Mem,
            "Mem",
            b,
            slot,
            cfg,
            style.ui.name,
            color,
        ));
    }

    if cfg.show_gpu {
        let (v, stale, ok) = metric_parts(&snap.gpu);
        let mut b = StrBuf::new();
        if ok {
            fmt_pct(&mut b, v);
        } else {
            b.push_str("--");
        }
        let color = if stale {
            style.ui.value_stale
        } else {
            style.accent(v)
        };
        rows.push(new_row(
            MetricEnum::Gpu,
            "GPU",
            b,
            SLOT_PCT,
            cfg,
            style.ui.name,
            color,
        ));
    }

    if cfg.show_net {
        let mut b = StrBuf::new();
        b.push_str("\u{2191}");
        fmt_rate(&mut b, snap.net.rx_bps, cfg.net_unit);
        b.push_str("/s \u{2193}");
        fmt_rate(&mut b, snap.net.tx_bps, cfg.net_unit);
        b.push_str("/s");
        rows.push(new_row(
            MetricEnum::Net,
            "Net",
            b,
            SLOT_RATE * 2 + 1,
            cfg,
            style.ui.name,
            style.ui.value,
        ));
    }

    if cfg.power_enabled {
        let (v, stale, ok) = metric_parts(&snap.power);
        let mut b = StrBuf::new();
        if ok {
            // 整机功耗 = 实测(CPU/GPU) + 估算(内存/主板/硬盘) → 恒带 ~ 前缀
            fmt_power(&mut b, v, true);
        } else {
            b.push_str("--");
        }
        let color = if stale {
            style.ui.value_stale
        } else {
            style.ui.value
        };
        rows.push(new_row(
            MetricEnum::Power,
            "Power",
            b,
            SLOT_POWER,
            cfg,
            style.ui.name,
            color,
        ));
    }

    rows
}

/// 组装一行：字号/配色槽位在此统一解析。
fn new_row(
    metric: MetricEnum,
    label: &str,
    value: StrBuf,
    slot: u32,
    cfg: &Config,
    name_fallback: Rgba,
    value_fallback: Rgba,
) -> MetricRow {
    let size = font_size_for(cfg, metric);
    MetricRow {
        metric,
        label: label_buf(label),
        scale: font_scale_of(size),
        font_size: size,
        slot,
        label_color: slot_color(cfg, metric, true, name_fallback),
        value_color: slot_color(cfg, metric, false, value_fallback),
        value,
        label_value_gap: cfg.label_value_gap as i32,
    }
}

fn label_buf(s: &str) -> StrBuf {
    let mut b = StrBuf::new();
    b.push_str(s);
    b
}

// ── 绘制 ──────────────────────────────────────────────────

/// 绘制一行文字（横版/竖版共用，含阴影/描边），`scale` 为浮点缩放比。
#[allow(clippy::too_many_arguments)]
fn draw_row_text(
    out: &mut SoftwareSurface,
    text: &str,
    x: i32,
    y: i32,
    scale: f32,
    color: Rgba,
    shade: Rgba,
    shadow: bool,
    outline: bool,
) {
    if text.is_empty() {
        return;
    }
    if outline {
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            render_str_scaled_f(
                &mut out.pixels,
                out.width,
                out.height,
                text,
                x + dx,
                y + dy,
                scale,
                shade,
            );
        }
    }
    if shadow {
        render_str_scaled_f(
            &mut out.pixels,
            out.width,
            out.height,
            text,
            x + 1,
            y + 1,
            scale,
            shade,
        );
    }
    render_str_scaled_f(&mut out.pixels, out.width, out.height, text, x, y, scale, color);
}

// ── 横版（StripRenderer） ─────────────────────────────────

/// v4 横版文字条渲染器（无背景，默认风格）。
#[derive(Clone, Copy, Debug, Default)]
pub struct StripRenderer;

/// 横版自然宽度：PAD + Σ(label + gap + slot)（项间加 metric_gap）+ PAD。
fn strip_natural_w(rows: &[MetricRow], cfg: &Config) -> i32 {
    let mut natural = PAD_X;
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            natural += cfg.metric_gap as i32;
        }
        let label_w = measure_scaled_f(row.label.as_str(), row.scale);
        let slot_w = measure_scaled_f("0", row.scale) * row.slot as i32;
        natural += label_w + row.label_value_gap + slot_w;
    }
    natural + PAD_X
}

/// 整条内容高度（像素）：最大字号 + 上下内边距。
fn strip_h(rows: &[MetricRow]) -> i32 {
    let max_scale = rows.iter().map(|r| r.scale).fold(0.0, f32::max);
    PAD_Y * 2 + (16.0 * max_scale).ceil() as i32
}

impl Renderer for StripRenderer {
    fn name(&self) -> &'static str {
        "strip"
    }

    fn draw(&self, out: &mut SoftwareSurface, snap: &Snapshot, cfg: &Config) -> (i32, i32) {
        out.clear();
        let shade = shade_color(cfg.fg);
        let rows = build_rows(snap, cfg);
        if rows.is_empty() {
            return (PAD_X * 2, PAD_Y * 2);
        }

        // 所有指标共享一条基线（垂直居中），各自字号独立缩放
        let max_scale = rows.iter().map(|r| r.scale).fold(0.0, f32::max);
        let glyph_h = (16.0 * max_scale).ceil() as i32;
        let y = ((out.height - glyph_h) / 2).max(PAD_Y);

        let mut x = PAD_X;
        for (i, row) in rows.iter().enumerate() {
            if i > 0 {
                x += cfg.metric_gap as i32;
            }
            let row_h = (16.0 * row.scale).ceil() as i32;
            let dy = (glyph_h - row_h) / 2;
            draw_row_text(
                out,
                row.label.as_str(),
                x,
                y + dy,
                row.scale,
                row.label_color,
                shade,
                cfg.text_shadow,
                cfg.text_outline,
            );
            x += measure_scaled_f(row.label.as_str(), row.scale) + row.label_value_gap;
            // 数值右对齐到固定槽位（数值位数变化不位移后续元素）
            let slot_w = measure_scaled_f("0", row.scale) * row.slot as i32;
            let vw = measure_scaled_f(row.value.as_str(), row.scale);
            let dx = if vw < slot_w { slot_w - vw } else { 0 };
            draw_row_text(
                out,
                row.value.as_str(),
                x + dx,
                y + dy,
                row.scale,
                row.value_color,
                shade,
                cfg.text_shadow,
                cfg.text_outline,
            );
            x += slot_w;
        }

        (strip_natural_w(&rows, cfg), out.height.max(strip_h(&rows)))
    }

    fn frame_hash(&self, snap: &Snapshot, cfg: &Config, w: i32, h: i32, alpha: u8) -> u64 {
        let rows = build_rows(snap, cfg);
        let mut acc = FNV_OFFSET;
        fnv_f32(&mut acc, cfg.font_size);
        fnv_u8(&mut acc, cfg.text_shadow as u8);
        fnv_u8(&mut acc, cfg.text_outline as u8);
        fnv_u8(&mut acc, alpha);
        fnv_u32(&mut acc, w as u32);
        fnv_u32(&mut acc, h as u32);
        fnv_u32(&mut acc, cfg.metric_gap);
        fnv_u32(&mut acc, cfg.label_value_gap);
        for r in &rows {
            for b in r.label.as_str().as_bytes() {
                fnv_mix(&mut acc, &[*b]);
            }
            fnv_mix(&mut acc, &[0xFE]);
            for b in r.value.as_str().as_bytes() {
                fnv_mix(&mut acc, &[*b]);
            }
            fnv_mix(&mut acc, &[0xFF]); // 文本分隔符
            fnv_u32(&mut acc, r.slot);
            fnv_u32(&mut acc, r.label_color.to_colorref());
            fnv_u32(&mut acc, r.value_color.to_colorref());
            fnv_f32(&mut acc, r.font_size);
        }
        acc
    }

    fn natural_size(&self, cfg: &Config) -> (i32, i32) {
        let snap = Snapshot::placeholder(0);
        let rows = build_rows(&snap, cfg);
        (strip_natural_w(&rows, cfg), strip_h(&rows))
    }
}

// ── 竖版（VerticalRenderer） ──────────────────────────────

/// v4 竖版文字条渲染器：同一批指标自上而下排列，
/// 行距 = [`Config::metric_gap`]，每行内名称 ↔ 数值间距 = [`Config::label_value_gap`]。
#[derive(Clone, Copy, Debug, Default)]
pub struct VerticalRenderer;

/// 竖版自然尺寸：宽 = 最宽行，高 = Σ(行高) + Σ(行距) + 内边距。
fn vertical_natural(rows: &[MetricRow], cfg: &Config) -> (i32, i32) {
    let mut max_w = PAD_X * 2;
    let mut total_h = PAD_Y * 2;
    for (i, row) in rows.iter().enumerate() {
        let label_w = measure_scaled_f(row.label.as_str(), row.scale);
        let slot_w = measure_scaled_f("0", row.scale) * row.slot as i32;
        max_w = max_w.max(PAD_X + label_w + row.label_value_gap + slot_w + PAD_X);
        total_h += (16.0 * row.scale).ceil() as i32;
        if i < rows.len().saturating_sub(1) {
            total_h += cfg.metric_gap as i32;
        }
    }
    (max_w, total_h)
}

impl Renderer for VerticalRenderer {
    fn name(&self) -> &'static str {
        "vertical"
    }

    fn draw(&self, out: &mut SoftwareSurface, snap: &Snapshot, cfg: &Config) -> (i32, i32) {
        out.clear();
        let shade = shade_color(cfg.fg);
        let rows = build_rows(snap, cfg);
        if rows.is_empty() {
            return (PAD_X * 2, PAD_Y * 2);
        }

        // 数值统一右对齐到最宽行（竖版可读性：各行数值列对齐）
        let (nat_w, _) = vertical_natural(&rows, cfg);
        let value_right = (nat_w - PAD_X).max(out.width - PAD_X);

        let mut y = PAD_Y;
        for row in &rows {
            let row_h = (16.0 * row.scale).ceil() as i32;
            draw_row_text(
                out,
                row.label.as_str(),
                PAD_X,
                y,
                row.scale,
                row.label_color,
                shade,
                cfg.text_shadow,
                cfg.text_outline,
            );
            // 数值靠右（不越过 value_right，也不与名称重叠）
            let vw = measure_scaled_f(row.value.as_str(), row.scale);
            let min_x =
                PAD_X + measure_scaled_f(row.label.as_str(), row.scale) + row.label_value_gap;
            let dx = (value_right - vw).max(min_x);
            draw_row_text(
                out,
                row.value.as_str(),
                dx,
                y,
                row.scale,
                row.value_color,
                shade,
                cfg.text_shadow,
                cfg.text_outline,
            );
            y += row_h + cfg.metric_gap as i32;
        }

        (nat_w, out.height.max(vertical_natural(&rows, cfg).1))
    }

    fn frame_hash(&self, snap: &Snapshot, cfg: &Config, w: i32, h: i32, alpha: u8) -> u64 {
        // 竖版与横版共享全部内容输入，指纹段一致即可（数据相同 → 跳过重绘）
        StripRenderer.frame_hash(snap, cfg, w, h, alpha)
    }

    fn natural_size(&self, cfg: &Config) -> (i32, i32) {
        let snap = Snapshot::placeholder(0);
        let rows = build_rows(&snap, cfg);
        vertical_natural(&rows, cfg)
    }
}

// ── 测试 ─────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_cfg() -> Config {
        Config::default()
    }

    fn demo_snap() -> Snapshot {
        let mut s = Snapshot::placeholder(0);
        s.cpu = SnapMetric::Value(47.0);
        s.gpu = SnapMetric::Value(23.5);
        s.memory.load_pct = 58.0;
        s.memory.used_gb = 8.5;
        s.net.rx_bps = 1_200_000;
        s.net.tx_bps = 340_000;
        s
    }

    #[test]
    fn renderers_render_pixels() {
        let snap = demo_snap();
        for renderer in [
            &StripRenderer as &dyn Renderer,
            &VerticalRenderer as &dyn Renderer,
        ] {
            let (w, h) = renderer.natural_size(&demo_cfg());
            let mut s = SoftwareSurface::new(w.max(100), h.max(20));
            renderer.draw(&mut s, &snap, &demo_cfg());
            let lit = s.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
            assert!(lit > 0, "{} 渲染器必须画出像素（lit={}）", renderer.name(), lit);
        }
    }

    #[test]
    fn alpha_and_values_change_hash() {
        let snap = demo_snap();
        let cfg = demo_cfg();
        let h1 = StripRenderer.frame_hash(&snap, &cfg, 300, 36, 90);
        let h2 = StripRenderer.frame_hash(&snap, &cfg, 300, 36, 91);
        assert_ne!(h1, h2, "alpha 变化必须改变指纹");

        let mut cfg2 = cfg;
        cfg2.net_unit = sysoverlay_core::types::NetUnit::GB;
        let h3 = StripRenderer.frame_hash(&snap, &cfg2, 300, 36, 90);
        assert_ne!(h1, h3, "单位切换必须改变指纹");

        let mut cfg3 = cfg;
        cfg3.font_size = 20.0;
        let h4 = StripRenderer.frame_hash(&snap, &cfg3, 300, 36, 90);
        assert_ne!(h1, h4, "字号变化必须改变指纹");

        let mut cfg4 = cfg;
        cfg4.metric_colors[metric_index(MetricEnum::Cpu)].label =
            Some(Rgb::new(255, 0, 0));
        let h5 = StripRenderer.frame_hash(&snap, &cfg4, 300, 36, 90);
        assert_ne!(h1, h5, "逐项配色变化必须改变指纹");
    }

    #[test]
    fn renderer_names_are_stable() {
        assert_eq!(StripRenderer.name(), "strip");
        assert_eq!(VerticalRenderer.name(), "vertical");
    }

    #[test]
    fn vertical_is_taller_than_strip() {
        let cfg = demo_cfg();
        let (_, sh) = StripRenderer.natural_size(&cfg);
        let (_, vh) = VerticalRenderer.natural_size(&cfg);
        assert!(vh > sh, "竖版必须比横版高（{sh} → {vh}）");
    }

    #[test]
    fn gaps_change_hash() {
        let snap = demo_snap();
        let mut cfg = demo_cfg();
        cfg.metric_gap = 24;
        let h1 = StripRenderer.frame_hash(&snap, &cfg, 400, 36, 90);
        let mut cfg2 = cfg;
        cfg2.metric_gap = 8;
        let h2 = StripRenderer.frame_hash(&snap, &cfg2, 400, 36, 90);
        assert_ne!(h1, h2, "指标间距必须改变指纹");

        let mut cfg3 = cfg;
        cfg3.label_value_gap = 20;
        let h3 = StripRenderer.frame_hash(&snap, &cfg3, 400, 36, 90);
        assert_ne!(h1, h3, "名称值间距必须改变指纹");
    }

    #[test]
    fn float_font_measure_matches_render() {
        // 最近邻字形宽度与渲染步进一致（不 panic、正数）
        let s = "CPU";
        let scale = font_scale_of(24.5);
        assert!(measure_scaled_f(s, scale) > 0);
        let mut surf = SoftwareSurface::new(200, 40);
        render_str_scaled_f(
            &mut surf.pixels,
            surf.width,
            surf.height,
            s,
            0,
            0,
            scale,
            Rgba::opaque(255, 255, 255),
        );
        assert!(surf.pixels.chunks_exact(4).any(|p| p[3] > 0), "必须画出内容");
    }

    #[test]
    fn per_metric_font_size_affects_size() {
        let mut cfg = demo_cfg();
        let base = StripRenderer.natural_size(&cfg);
        cfg.metric_font_sizes[metric_index(MetricEnum::Cpu)] = Some(40.0);
        let grown = StripRenderer.natural_size(&cfg);
        assert!(
            grown.0 > base.0,
            "CPU 字号放大后横版必须变宽（{} → {}）",
            base.0,
            grown.0
        );
    }
}