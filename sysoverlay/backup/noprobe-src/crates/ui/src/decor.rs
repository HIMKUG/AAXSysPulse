//! 装饰注入点：卡片背景与进度条的 Eco / Quality 两套画法。
//!
//! 与 [`crate::raster::TextRasterizer`] 一起，构成 v2 决策里
//! 「`draw_card` 唯一骨架 + 两个注入点」的第二个注入点。
//!
//! 两套画法**只影响观感，不影响几何** —— 几何全部由 `layout::compute` 决定，
//! 因此 Tier 1 测试断言布局即可，无需为每套模式重复一遍。

use crate::palette::Style;
use crate::render::{Rect, RenderTarget, Rgba};
use sysoverlay_core::layout::fill_pct;
use sysoverlay_core::types::Shape;

/// 卡片默认圆角半径（逻辑像素，96 DPI / 100%）。
const CARD_RADIUS: i32 = 14;

/// 装饰策略。
pub trait Decoration {
    /// 稳定名称（诊断 `--diag` 输出用）。
    fn name(&self) -> &'static str;

    /// 画卡片背景。
    ///
    /// `Shape::Content` 表示不画底（调用方负责清屏）。
    fn draw_background(
        &self,
        r: Rect,
        shape: Shape,
        style: &Style,
        out: &mut dyn RenderTarget,
    );

    /// 画进度条（底槽 + 填充）。
    ///
    /// `stale = true` 时用降饱和的强调色（v2 Q16：陈旧 = 降饱和，不是换色）。
    fn draw_progress(
        &self,
        r: Rect,
        pct: f32,
        stale: bool,
        style: &Style,
        out: &mut dyn RenderTarget,
    );
}

/// Eco 装饰：直角、无描边、无圆角抗锯齿。
///
/// 每一项省下的都是**逐像素的圆角判定**（`point_in_round_rect` 对整块背景
/// 都要跑一遍），在 200% 缩放的大卡片上是实打实的开销。
#[derive(Clone, Copy, Debug, Default)]
pub struct EcoDecoration;

impl Decoration for EcoDecoration {
    fn name(&self) -> &'static str {
        "eco"
    }

    fn draw_background(&self, r: Rect, shape: Shape, style: &Style, out: &mut dyn RenderTarget) {
        if !shape.has_background() {
            return;
        }
        // 直角：一次 fill_rect，不做圆角判定
        out.fill_rect(r, style.ui.bg);
    }

    fn draw_progress(
        &self,
        r: Rect,
        pct: f32,
        stale: bool,
        style: &Style,
        out: &mut dyn RenderTarget,
    ) {
        if r.w <= 0 || r.h <= 0 {
            return;
        }
        out.fill_rect(r, style.ui.track);

        let fill_w = (r.w as f32 * (fill_pct(pct) as f32 / 100.0)).round() as i32;
        if fill_w <= 0 {
            return;
        }
        let color = if stale { style.stale_accent(pct) } else { style.accent(pct) };
        // 方角填充
        out.fill_rect(Rect::with_pos(r.x, r.y, fill_w, r.h), color);
    }
}

/// Quality 装饰：圆角 + 1px 描边 + 圆角进度条。
#[derive(Clone, Copy, Debug, Default)]
pub struct QualityDecoration;

impl Decoration for QualityDecoration {
    fn name(&self) -> &'static str {
        "quality"
    }

    fn draw_background(&self, r: Rect, shape: Shape, style: &Style, out: &mut dyn RenderTarget) {
        match shape {
            Shape::Content => return, // 纯内容模式：由调用方清屏
            Shape::Square => {
                out.fill_rect(r, style.ui.border);
                out.fill_rect(inset(r, 1), style.ui.bg);
            }
            Shape::Round => {
                out.fill_round_rect(r, CARD_RADIUS, style.ui.border);
                out.fill_round_rect(inset(r, 1), CARD_RADIUS - 1, style.ui.bg);
            }
        }
    }

    fn draw_progress(
        &self,
        r: Rect,
        pct: f32,
        stale: bool,
        style: &Style,
        out: &mut dyn RenderTarget,
    ) {
        if r.w <= 0 || r.h <= 0 {
            return;
        }
        let radius = r.h / 2;
        out.fill_round_rect(r, radius, style.ui.track);

        let fill_w = (r.w as f32 * (fill_pct(pct) as f32 / 100.0)).round() as i32;
        if fill_w <= 0 {
            return;
        }
        let color = if stale { style.stale_accent(pct) } else { style.accent(pct) };
        out.fill_round_rect(Rect::with_pos(r.x, r.y, fill_w, r.h), radius, color);
    }
}

/// 按 `n` 像素内缩。
fn inset(r: Rect, n: i32) -> Rect {
    Rect::with_pos(r.x + n, r.y + n, (r.w - n * 2).max(0), (r.h - n * 2).max(0))
}

/// 由渲染模式挑选装饰实现。
///
/// 这是 `Config::render_mode` → 具体 `Decoration` 的**唯一映射点**，
/// 避免 `if mode == Eco` 散落在绘制代码各处。
pub fn decoration_for(mode: sysoverlay_core::types::RenderMode) -> &'static dyn Decoration {
    match mode {
        sysoverlay_core::types::RenderMode::Eco => &EcoDecoration,
        sysoverlay_core::types::RenderMode::Quality => &QualityDecoration,
    }
}

/// 取某个像素的 RGBA（BGRA 存储 → RGBA 返回），越界返回全透明。
///
/// 供测试做**像素探针**（v2 决策：Tier 1 用少量像素探针验证绘制结果，
/// 而不是暴露 draw list）。
pub fn probe(pixels: &[u8], w: i32, x: i32, y: i32) -> Rgba {
    let idx = ((y * w + x) * 4) as usize;
    if idx + 3 >= pixels.len() {
        return Rgba::transparent();
    }
    Rgba {
        r: pixels[idx + 2],
        g: pixels[idx + 1],
        b: pixels[idx],
        a: pixels[idx + 3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offscreen::SoftwareSurface;
    use crate::raster::{BitmapTextRasterizer, TextRasterizer};
    use sysoverlay_core::types::RenderMode;

    fn style() -> Style {
        Style::default()
    }

    #[test]
    fn decoration_for_maps_mode() {
        assert_eq!(decoration_for(RenderMode::Eco).name(), "eco");
        assert_eq!(decoration_for(RenderMode::Quality).name(), "quality");
    }

    #[test]
    fn content_shape_draws_no_background() {
        for d in [&EcoDecoration as &dyn Decoration, &QualityDecoration] {
            let mut s = SoftwareSurface::new(40, 40);
            d.draw_background(Rect::new(40, 40), Shape::Content, &style(), &mut s);
            assert!(
                s.pixels.chunks_exact(4).all(|p| p[3] == 0),
                "{} 在 Content 模式下不应画底",
                d.name()
            );
        }
    }

    #[test]
    fn eco_background_is_square_and_opaque() {
        let mut s = SoftwareSurface::new(40, 40);
        EcoDecoration.draw_background(Rect::new(40, 40), Shape::Round, &style(), &mut s);
        // Eco 忽略 Round，画满整块（含四角）
        for (x, y) in [(0, 0), (39, 0), (0, 39), (39, 39)] {
            assert_eq!(probe(&s.pixels, 40, x, y).a, 255, "Eco 角落({},{})必须有底色", x, y);
        }
    }

    #[test]
    fn quality_round_background_leaves_corners_transparent() {
        let mut s = SoftwareSurface::new(40, 40);
        QualityDecoration.draw_background(Rect::new(40, 40), Shape::Round, &style(), &mut s);
        assert_eq!(
            probe(&s.pixels, 40, 0, 0).a,
            0,
            "Quality 圆角：左上角应留空"
        );
        assert_eq!(probe(&s.pixels, 40, 20, 20).a, 255, "中心必须画满");
    }

    #[test]
    fn quality_square_background_fills_corners() {
        let mut s = SoftwareSurface::new(40, 40);
        QualityDecoration.draw_background(Rect::new(40, 40), Shape::Square, &style(), &mut s);
        assert_eq!(probe(&s.pixels, 40, 0, 0).a, 255);
    }

    #[test]
    fn progress_fill_is_proportional() {
        let r = Rect::with_pos(0, 0, 100, 6);
        let mut s = SoftwareSurface::new(100, 6);
        QualityDecoration.draw_progress(r, 50.0, false, &style(), &mut s);

        // 50% → 左半是强调色，右半仍是底槽色
        let left = probe(&s.pixels, 100, 10, 3);
        let right = probe(&s.pixels, 100, 90, 3);
        assert_ne!(left, right, "填充区与底槽区颜色必须不同");
        assert_eq!(left, style().accent(50.0), "50% 属于 OK 档（绿色）");
    }

    #[test]
    fn zero_pct_draws_track_only() {
        let r = Rect::with_pos(0, 0, 100, 6);
        let mut s = SoftwareSurface::new(100, 6);
        QualityDecoration.draw_progress(r, 0.0, false, &style(), &mut s);
        let px = probe(&s.pixels, 100, 50, 3);
        assert_eq!(px, style().ui.track, "0% 时整条都是底槽色");
    }

    /// v2 Q16：陈旧 = 降饱和，必须比新鲜值更灰/更暗，但不能变成别的色相。
    #[test]
    fn stale_progress_is_desaturated_not_recolor() {
        let r = Rect::with_pos(0, 0, 100, 6);
        let mut fresh = SoftwareSurface::new(100, 6);
        let mut stale = SoftwareSurface::new(100, 6);
        QualityDecoration.draw_progress(r, 90.0, false, &style(), &mut fresh);
        QualityDecoration.draw_progress(r, 90.0, true, &style(), &mut stale);

        let f = probe(&fresh.pixels, 100, 10, 3);
        let st = probe(&stale.pixels, 100, 10, 3);
        assert!(st.r < f.r, "陈旧的红必须更暗");
        assert!(st.r >= st.b, "陈旧红仍应偏红（R >= B），不能翻成蓝");
    }

    #[test]
    fn eco_and_quality_agree_on_geometry() {
        // 两套装饰的进度条填充宽度必须一致（几何由 layout 决定，与装饰无关）
        let r = Rect::with_pos(0, 0, 100, 6);
        let mut e = SoftwareSurface::new(100, 6);
        let mut q = SoftwareSurface::new(100, 6);
        EcoDecoration.draw_progress(r, 33.0, false, &style(), &mut e);
        QualityDecoration.draw_progress(r, 33.0, false, &style(), &mut q);

        let filled = |s: &SoftwareSurface| {
            (0..100)
                .filter(|&x| probe(&s.pixels, 100, x, 3) == style().accent(33.0))
                .count()
        };
        // 圆角会有几个像素的抗锯齿差异，允许 ±4
        let diff = (filled(&e) as i32 - filled(&q) as i32).abs();
        assert!(diff <= 4, "两套装饰的填充宽度应基本一致，实测差 {} px", diff);
    }

    #[test]
    fn bitmap_text_and_decor_compose() {
        // 骨架组合冒烟：背景 + 文字都要出现
        let mut s = SoftwareSurface::new(120, 40);
        QualityDecoration.draw_background(Rect::new(120, 40), Shape::Round, &style(), &mut s);
        BitmapTextRasterizer.draw("CPU", Rect::with_pos(16, 12, 60, 16), style().ui.name, &mut s);
        let lit = s.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(lit > 100, "背景+文字应写出足量像素，实测 {}", lit);
    }
}
