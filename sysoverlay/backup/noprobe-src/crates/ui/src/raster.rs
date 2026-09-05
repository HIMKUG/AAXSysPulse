//! 文字光栅化注入点（Eco 点阵 / Quality 矢量）。
//!
//! # 为什么是 trait 而不是 `if mode == Eco`
//!
//! v2 决策「双渲染模式」：`draw_card` 保持**唯一骨架**，
//! Eco/Quality 的差异通过**两个注入点**注入（本模块的 `TextRasterizer`
//! 与 [`crate::decor::Decoration`]），而不是在绘制函数里铺满 `if/else`。
//! 这样：
//! - 骨架只有一份，两套模式不可能画出不同的行结构；
//! - `win` 可以提供基于 GDI 的 `TextRasterizer`，
//!   而 `ui` 自己仍然零 Win32 依赖。
//!
//! # 不为测试增加抽象
//!
//! v2 决策「测试分层」明确要求**不为测试暴露 draw list**。
//! 所以这里没有 `RecordingTextRasterizer` 之类的假实现：
//! Tier 1 断言纯函数 `layout::compute` 的输出，
//! 需要验证绘制结果时用**像素探针**读 `SoftwareSurface`（见 `card::tests`）。

use crate::font::{measure, FONT_HEIGHT};
use crate::render::{FontSpec, Rect, RenderTarget, Rgba};

/// 文字光栅化策略。
pub trait TextRasterizer {
    /// 稳定名称（诊断 `--diag` 输出用）。
    fn name(&self) -> &'static str;

    /// 在 `at` 区域内绘制文字，超出部分裁剪。
    fn draw(&self, s: &str, at: Rect, color: Rgba, out: &mut dyn RenderTarget);

    /// 文字尺寸（宽, 高）。
    fn measure(&self, s: &str) -> (i32, i32);

    /// 行高（像素）。
    fn line_height(&self) -> i32;
}

/// Eco 模式：8×16 位图点阵。
///
/// 纯 Rust、零 Win32、零额外内存 —— 常驻悬浮窗的省电路径。
/// 代价：只有 ASCII（非 ASCII 走 ■ 替代字形），且没有抗锯齿。
#[derive(Clone, Copy, Debug, Default)]
pub struct BitmapTextRasterizer;

impl TextRasterizer for BitmapTextRasterizer {
    fn name(&self) -> &'static str {
        "bitmap8x16"
    }

    fn draw(&self, s: &str, at: Rect, color: Rgba, out: &mut dyn RenderTarget) {
        out.draw_text(s, at, color, FontSpec::ui());
    }

    fn measure(&self, s: &str) -> (i32, i32) {
        (measure(s), FONT_HEIGHT)
    }

    fn line_height(&self) -> i32 {
        FONT_HEIGHT
    }
}

impl<'a> TextRasterizer for &'a dyn TextRasterizer {
    fn name(&self) -> &'static str {
        (**self).name()
    }

    fn draw(&self, s: &str, at: Rect, color: Rgba, out: &mut dyn RenderTarget) {
        (**self).draw(s, at, color, out)
    }

    fn measure(&self, s: &str) -> (i32, i32) {
        (**self).measure(s)
    }

    fn line_height(&self) -> i32 {
        (**self).line_height()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offscreen::SoftwareSurface;

    #[test]
    fn bitmap_rasterizer_name() {
        assert_eq!(BitmapTextRasterizer.name(), "bitmap8x16");
    }

    #[test]
    fn bitmap_rasterizer_measures() {
        let (w, h) = BitmapTextRasterizer.measure("CPU");
        assert_eq!(w, 3 * (8 + 1) - 1);
        assert_eq!(h, 16);
        assert_eq!(BitmapTextRasterizer.line_height(), 16);
    }

    #[test]
    fn bitmap_rasterizer_actually_draws_pixels() {
        let mut s = SoftwareSurface::new(64, 16);
        BitmapTextRasterizer.draw(
            "A",
            Rect::with_pos(0, 0, 64, 16),
            Rgba::opaque(255, 255, 255),
            &mut s,
        );
        let lit = s.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(lit > 0, "位图光栅器必须真的写出像素");
    }

    /// 空白字符串不应画出任何像素（避免"空文本画出杂点"）。
    #[test]
    fn empty_text_draws_nothing() {
        let mut s = SoftwareSurface::new(64, 16);
        BitmapTextRasterizer.draw(
            "",
            Rect::with_pos(0, 0, 64, 16),
            Rgba::opaque(255, 255, 255),
            &mut s,
        );
        assert!(s.pixels.chunks_exact(4).all(|p| p[3] == 0));
    }
}
