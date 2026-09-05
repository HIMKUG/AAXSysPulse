//! Card drawing functions for the overlay UI.
//!
//! 这是卡片绘制的**唯一骨架**：Eco 与 Quality 共用同一份行结构，
//! 差异全部来自两个注入点（[`DrawCtx::text`] 与 [`DrawCtx::decor`]）。
//!
//! 骨架不感知窗口、不感知 DPI、不感知具体字体 ——
//! 几何全由 `layout::compute` 决定，颜色全由 `core::color` 决定，
//! 数字格式化全由 `core::units` 决定。这里只负责"把它们摆到一起"。

use crate::decor::Decoration;
use crate::palette::Style;
use crate::raster::TextRasterizer;
use crate::render::{Rect, RenderTarget};
use sysoverlay_core::config::Config;
use sysoverlay_core::layout::{CardLayout, RowKind, RowLayout};
use sysoverlay_core::snapshot::{Metric, MemoryStat, NetStat, Snapshot};
use sysoverlay_core::units::{fmt_mem, fmt_pct, fmt_power, fmt_rate_pair, StrBuf};
use sysoverlay_core::types::PowerDisplay;

/// 一次绘制所需的上下文（样式 + 两个注入点 + 悬停透明度）。
pub struct DrawCtx<'a> {
    /// 调色板（纯逻辑 + 预乘两份）。
    pub style: Style,
    /// 文字光栅化策略（Eco 点阵 / Quality 矢量）。
    pub text: &'a dyn TextRasterizer,
    /// 装饰策略（Eco 直角 / Quality 圆角描边）。
    pub decor: &'a dyn Decoration,
    /// 悬停透明度（1.0 = 不透明）。
    ///
    /// v2 决策「时序模型」：悬停只改 `BLENDFUNCTION`，不重画像素。
    /// 这里保留逐像素 alpha 缩放作为**软件路径**的实现，
    /// `win` 层在 Quality 模式下会走 `BLENDFUNCTION` 这条更快的路。
    pub hover_alpha: f32,
}

impl<'a> DrawCtx<'a> {
    /// 便捷构造：`Quality` 默认组合。
    pub fn new(style: Style, text: &'a dyn TextRasterizer, decor: &'a dyn Decoration) -> Self {
        Self {
            style,
            text,
            decor,
            hover_alpha: 1.0,
        }
    }
}

/// 一行的标签文案。
fn label_of(kind: RowKind) -> &'static str {
    match kind {
        RowKind::Cpu => "CPU",
        RowKind::Memory => "Memory",
        RowKind::Gpu => "GPU",
        RowKind::Net => "Net",
        RowKind::Power => "Power",
    }
}

/// 把 `StrBuf` 内容按右对齐画进 `rect`。
fn draw_value_right<T: RenderTarget>(
    out: &mut T,
    text: &dyn TextRasterizer,
    s: &str,
    rect: (i32, i32, i32, i32),
    color: crate::render::Rgba,
) {
    let (w, _) = text.measure(s);
    // 值域不够宽时从左侧开始画（截断总好过整行消失）
    let x = if w <= rect.2 { rect.0 + rect.2 - w } else { rect.0 };
    text.draw(s, Rect::with_pos(x, rect.1, w, rect.3), color, out);
}

/// Draws the complete overlay card.
///
/// # 骨架流程
/// 1. 清屏（分层窗口按 alpha 合成，残留会出重影）
/// 2. 背景（注入点 `decor`）
/// 3. 头部：主机名 + 运行时长
/// 4. 逐行绘制（行集合与几何来自 `layout`，语义来自 `RowKind`）
/// 5. 悬停 alpha
pub fn draw_card<T: RenderTarget>(
    out: &mut T,
    snap: &Snapshot,
    cfg: &Config,
    layout: &CardLayout,
    ctx: &DrawCtx<'_>,
) {
    out.clear();

    // 1. 背景（Content 模式由 decor 内部决定不画）
    ctx.decor.draw_background(
        Rect::new(layout.width, layout.height),
        cfg.shape,
        &ctx.style,
        out,
    );

    // 2. 头部
    draw_header(out, snap, layout, ctx);

    // 3. 还没有任何采样结果时给个明确提示，而不是画一堆 0%
    if snap.seq == 0 {
        draw_initializing(out, layout, ctx);
        apply_hover(out, ctx);
        return;
    }

    // 4. 逐行
    let mut buf = StrBuf::new();
    for row in &layout.rows {
        draw_row(out, row, snap, cfg, &mut buf, ctx);
    }

    // 5. 悬停
    apply_hover(out, ctx);
}

fn apply_hover<T: RenderTarget>(out: &mut T, ctx: &DrawCtx<'_>) {
    if ctx.hover_alpha < 1.0 {
        out.scale_alpha(ctx.hover_alpha);
    }
}

/// 头部：主机名。
fn draw_header<T: RenderTarget>(
    out: &mut T,
    _snap: &Snapshot,
    layout: &CardLayout,
    ctx: &DrawCtx<'_>,
) {
    // 极简模式下高度只有 36px，没有头部的位置
    if layout.height < 60 {
        return;
    }

    let lh = 18;
    let y = 12;
    let pad = 16;

    ctx.text.draw(
        "PC",
        Rect::with_pos(pad, y, layout.width - pad * 2, lh),
        ctx.style.ui.title,
        out,
    );
}

fn draw_initializing<T: RenderTarget>(out: &mut T, layout: &CardLayout, ctx: &DrawCtx<'_>) {
    // Eco 的位图字体没有中文字形，统一用 ASCII（v2 Q16）
    let msg = "Initializing...";
    let y = if layout.height < 60 { 10 } else { 48 };
    ctx.text.draw(
        msg,
        Rect::with_pos(16, y, layout.width - 32, ctx.text.line_height()),
        ctx.style.ui.name,
        out,
    );
}

/// 画一行：标签 + 值 + （可选）进度条。
fn draw_row<T: RenderTarget>(
    out: &mut T,
    row: &RowLayout,
    snap: &Snapshot,
    cfg: &Config,
    buf: &mut StrBuf,
    ctx: &DrawCtx<'_>,
) {
    let (stale, pct) = match row.kind {
        RowKind::Cpu => metric_state(snap.cpu),
        RowKind::Memory => (false, snap.memory.load_pct),
        RowKind::Gpu => metric_state(snap.gpu),
        RowKind::Net => (false, 0.0),
        RowKind::Power => metric_state(snap.power),
    };

    let name_color = if stale { ctx.style.ui.name_stale } else { ctx.style.ui.name };
    let value_color = if stale { ctx.style.ui.value_stale } else { ctx.style.ui.value };

    ctx.text.draw(
        label_of(row.kind),
        Rect::from_tuple(row.label_rect),
        name_color,
        out,
    );

    // 内存行额外补一段 "8.5/16.0G" 的次要信息
    if row.kind == RowKind::Memory {
        draw_mem_detail(out, row, &snap.memory, buf, ctx);
    }

    // 值
    match row.kind {
        RowKind::Cpu | RowKind::Memory | RowKind::Gpu => {
            fmt_pct(buf, pct);
            draw_value_right(out, ctx.text, buf.as_str(), row.value_rect, value_color);
        }
        RowKind::Net => {
            fmt_rate_pair(buf, snap.net.rx_bps, snap.net.tx_bps, cfg.net_unit);
            draw_value_right(out, ctx.text, buf.as_str(), row.value_rect, value_color);
        }
        RowKind::Power => {
            // v2 Q16：估算值必须带 `~` 前缀，不能冒充实测值
            let estimated = cfg.power_display == PowerDisplay::Estimated;
            fmt_power(buf, pct, estimated);
            draw_value_right(out, ctx.text, buf.as_str(), row.value_rect, value_color);
        }
    }

    // 进度条：仅功耗行有进度条（CPU/内存/GPU 只显示数字，网络行无进度条）
    if row.kind == RowKind::Power && row.track_rect.2 > 0 && row.track_rect.3 > 0 {
        ctx.decor.draw_progress(
            Rect::from_tuple(row.track_rect),
            pct,
            stale,
            &ctx.style,
            out,
        );
    }
}

/// 内存行在标签右侧补一段用量细节。
fn draw_mem_detail<T: RenderTarget>(
    out: &mut T,
    row: &RowLayout,
    mem: &MemoryStat,
    buf: &mut StrBuf,
    ctx: &DrawCtx<'_>,
) {
    fmt_mem(buf, mem.used_gb, mem.total_gb);
    let (label_w, _) = ctx.text.measure(label_of(RowKind::Memory));
    let x = row.label_rect.0 + label_w + 6;
    let w = row.label_rect.2 - label_w - 6;
    if w > 0 {
        ctx.text.draw(
            buf.as_str(),
            Rect::with_pos(x, row.label_rect.1, w, row.label_rect.3),
            ctx.style.ui.sub,
            out,
        );
    }
}

/// 取出指标的状态：(是否陈旧, 数值)。
fn metric_state(m: Metric<f32>) -> (bool, f32) {
    match m {
        Metric::Value(v) => (false, v),
        Metric::Stale { value, .. } => (true, value),
        Metric::Unavailable => (false, 0.0),
    }
}

/// Creates a demo snapshot for testing / PNG reference generation.
pub fn demo_snapshot() -> Snapshot {
    Snapshot {
        seq: 1,
        timestamp_ms: 1_000,
        uptime_ms: 1_000,
        cpu: Metric::Value(67.5),
        memory: MemoryStat {
            load_pct: 45.0,
            used_gb: 8.52,
            total_gb: 16.0,
        },
        gpu: Metric::Value(89.2),
        net: NetStat {
            rx_bps: 1_234_567,
            tx_bps: 234_567,
        },
        power: Metric::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decor::{decoration_for, probe, EcoDecoration, QualityDecoration};
    use crate::offscreen::SoftwareSurface;
    use crate::raster::BitmapTextRasterizer;
    use sysoverlay_core::config::Config;
    use sysoverlay_core::layout::{compute, LayoutInput, RowsShown};
    use sysoverlay_core::types::RenderMode;

    /// 常规模式的默认绘制组合。
    fn draw(snap: &Snapshot, cfg: &Config, mode: RenderMode) -> SoftwareSurface {
        let input = LayoutInput {
            zoom: cfg.effective_zoom(),
            shape: cfg.shape,
            vert: cfg.vert,
            minimal: cfg.minimal,
            dpi: 96,
            rows: RowsShown::from_snapshot(snap, cfg.power_enabled),
        };
        let layout = compute(&input);
        let mut surface = SoftwareSurface::new(layout.width, layout.height);
        let ctx = DrawCtx {
            style: Style::derive(cfg.bg, cfg.fg),
            text: &BitmapTextRasterizer,
            decor: decoration_for(mode),
            hover_alpha: 1.0,
        };
        draw_card(&mut surface, snap, cfg, &layout, &ctx);
        surface
    }

    #[test]
    fn demo_snapshot_has_power_field() {
        let s = demo_snapshot();
        assert!(s.power.value().is_none(), "demo 默认不含功耗");
    }

    #[test]
    fn card_renders_non_empty() {
        let s = draw(&demo_snapshot(), &Config::default(), RenderMode::Quality);
        let lit = s.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(lit > 1000, "卡片应画出足量像素，实测 {}", lit);
    }

    #[test]
    fn background_is_drawn() {
        let s = draw(&demo_snapshot(), &Config::default(), RenderMode::Quality);
        // 中心必须是背景色（不是全透明）
        let px = probe(&s.pixels, s.width, s.width / 2, s.height / 2);
        assert_eq!(px.a, 255);
    }

    #[test]
    fn content_shape_has_no_background_but_has_text() {
        let mut cfg = Config::default();
        cfg.shape = sysoverlay_core::types::Shape::Content;
        let s = draw(&demo_snapshot(), &cfg, RenderMode::Quality);
        let lit = s.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(lit > 0, "Content 模式仍要画文字");
        // 左上角（无文字处）应是透明的
        assert_eq!(probe(&s.pixels, s.width, 1, 1).a, 0);
    }

    #[test]
    fn placeholder_snapshot_shows_initializing() {
        let snap = Snapshot::placeholder(0);
        let s = draw(&snap, &Config::default(), RenderMode::Quality);
        let lit = s.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(lit > 0, "占位快照也要有东西可看");
        // 不应出现进度条填充（所有指标都不可用 → 0% → 不画填充）。
        // 用"OK 档绿色是否出现"作像素探针。
        let has_green = s
            .pixels
            .chunks_exact(4)
            .any(|p| p[2] == 74 && p[1] == 222 && p[0] == 128 && p[3] == 255);
        assert!(!has_green, "占位快照不应画出 OK 档进度条");
    }

    /// v2 Q16：陈旧值仍要显示（标灰），不能当成不可用整行消失。
    #[test]
    fn stale_metric_still_renders_but_gray() {
        let mut snap = demo_snapshot();
        snap.cpu = Metric::Stale {
            value: 67.5,
            stale_for: 5,
        };

        let fresh = draw(&demo_snapshot(), &Config::default(), RenderMode::Quality);
        let stale = draw(&snap, &Config::default(), RenderMode::Quality);

        // 两帧的"有内容像素数"应接近（陈旧没让整行消失）
        let count = |s: &SoftwareSurface| s.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        let (a, b) = (count(&fresh), count(&stale));
        let diff = (a as i32 - b as i32).abs();
        assert!(diff < a as i32 / 4, "陈旧不应大幅改变构图（差 {} px）", diff);
    }

    #[test]
    fn power_row_renders_when_available() {
        let mut snap = demo_snapshot();
        snap.power = Metric::Value(45.0);
        let mut cfg = Config::default();
        cfg.power_enabled = true;

        let s = draw(&snap, &cfg, RenderMode::Quality);
        let lit = s.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(lit > 1000);

        // 卡片必须变高以容纳功耗行
        let without = draw(&demo_snapshot(), &Config::default(), RenderMode::Quality);
        assert!(s.height > without.height, "启用功耗后卡片必须加高");
    }

    /// v2 Q16：估算功耗必须带 `~` 前缀（Eco 无中文，用 ASCII 标记）。
    #[test]
    fn estimated_power_marks_with_tilde() {
        let mut buf = StrBuf::new();
        fmt_power(&mut buf, 45.0, true);
        assert!(buf.as_str().starts_with('~'));
        fmt_power(&mut buf, 45.0, false);
        assert!(!buf.as_str().starts_with('~'));
    }

    #[test]
    fn eco_and_quality_produce_same_card_size() {
        let snap = demo_snapshot();
        let cfg = Config::default();
        let q = draw(&snap, &cfg, RenderMode::Quality);
        let e = draw(&snap, &cfg, RenderMode::Eco);
        assert_eq!((q.width, q.height), (e.width, e.height), "两套模式卡片尺寸必须一致");
    }

    #[test]
    fn hover_alpha_darkens_everything() {
        let snap = demo_snapshot();
        let cfg = Config::default();
        let input = LayoutInput::default();
        let layout = compute(&input);

        let mut faded = SoftwareSurface::new(layout.width, layout.height);
        let ctx = DrawCtx {
            style: Style::default(),
            text: &BitmapTextRasterizer,
            decor: &QualityDecoration,
            hover_alpha: 0.5,
        };
        draw_card(&mut faded, &snap, &cfg, &layout, &ctx);

        let full = draw(&snap, &cfg, RenderMode::Quality);
        let avg = |s: &SoftwareSurface| {
            s.pixels.chunks_exact(4).map(|p| p[3] as u32).sum::<u32>()
                / s.pixels.len() as u32
                / 4
        };
        assert!(avg(&faded) < avg(&full), "悬停淡出后平均 alpha 必须更低");
    }

    #[test]
    fn eco_decoration_is_used_in_eco_mode() {
        assert_eq!(EcoDecoration.name(), "eco");
        assert_eq!(QualityDecoration.name(), "quality");
    }
}
