//! Software-based offscreen rendering surface.
//!
//! This module provides a pure Rust software rasterizer for rendering
//! without a window. It produces PNG output for pixel-level comparison
//! with the C reference implementation.
//!
//! Pixel format: BGRA with pre-multiplied alpha (matching Windows DIB format).

use crate::render::{FontSpec, Rect, RenderTarget, Rgba};
use crate::font::{measure, render_char, FONT_HEIGHT, FONT_SPACING, FONT_WIDTH};

/// Software surface for offscreen rendering.
///
/// Maintains a BGRA pixel buffer with pre-multiplied alpha,
/// matching the Windows DIB format used by the C version.
pub struct SoftwareSurface {
    /// BGRA pre-multiplied pixel buffer.
    pub pixels: Vec<u8>,
    /// Surface width in pixels.
    pub width: i32,
    /// Surface height in pixels.
    pub height: i32,
}

impl SoftwareSurface {
    /// Creates a new empty surface with the given dimensions.
    ///
    /// All pixels are initialized to transparent black (0,0,0,0).
    pub fn new(width: i32, height: i32) -> Self {
        // 审查 107/108：与 resize 同口径 —— 负尺寸钳 0，i64 乘法防溢出。
        let need = (width.max(0) as i64) * (height.max(0) as i64) * 4;
        let need = need.clamp(0, i32::MAX as i64) as usize;
        Self {
            pixels: vec![0u8; need],
            width,
            height,
        }
    }

    /// **复用缓冲**：尺寸/容量不变时只清零，不重新分配。
    ///
    /// 悬浮窗每帧都要一块同尺寸的画布；逐帧 `vec![0u8; 187_200]` 的
    /// 分配 + 清零是纯浪费（P2）。尺寸真的变了才重新分配。
    pub fn resize(&mut self, width: i32, height: i32) {
        // 审查 108：i32 乘法在大 DPI/大窗口下会溢出，改 i64 并钳制。
        let need = (width.max(0) as i64) * (height.max(0) as i64) * 4;
        let need = need.clamp(0, i32::MAX as i64) as usize;
        if self.width != width || self.height != height || self.pixels.len() != need {
            self.pixels = vec![0u8; need];
            self.width = width;
            self.height = height;
        }
        // 注：渲染器 draw() 入口统一 out.clear()（StripRenderer/VerticalRenderer），
        // 此处不必再清零，避免每帧双 memset（P1-8）。
    }

    /// 在 (x, y) 处做**预乘 alpha 的 over 合成**：`out = src + dst * (1 - src_a)`。
    ///
    /// 越界坐标静默忽略 —— 裁剪是渲染器的责任，不该让调用方去做边界算术，
    /// 更不该为此引入 `unsafe`（`ui` crate 必须零 `unsafe`）。
    fn blend_pixel(&mut self, x: i32, y: i32, color: Rgba) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let idx = ((y * self.width + x) * 4) as usize;
        if idx + 3 >= self.pixels.len() {
            return;
        }

        let factor = 1.0 - color.alpha_float();
        let dst_b = self.pixels[idx] as f32;
        let dst_g = self.pixels[idx + 1] as f32;
        let dst_r = self.pixels[idx + 2] as f32;
        let dst_a = self.pixels[idx + 3] as f32;

        // 审查 499：out_r/g/b 必须与 out_a 一样钳到 255 —— 预乘色叠加
        // 理论上有界，但防御性钳位防 `as u8` 回绕（与 alpha 侧对称）。
        let out_r = (color.r as f32 + dst_r * factor).min(255.0);
        let out_g = (color.g as f32 + dst_g * factor).min(255.0);
        let out_b = (color.b as f32 + dst_b * factor).min(255.0);
        let out_a = (color.a as f32 + dst_a * factor).min(255.0);

        self.pixels[idx] = out_b as u8; // B
        self.pixels[idx + 1] = out_g as u8; // G
        self.pixels[idx + 2] = out_r as u8; // R
        self.pixels[idx + 3] = out_a as u8; // A
    }

    /// 把一个像素**直接置 0**（全透明，审查 109）。
    ///
    /// 不能用 `blend_pixel(…, transparent)` 实现清像素：over 合成下
    /// `src_a = 0 ⇒ out = dst`，是恒等操作，圆角根本清不掉。
    fn clear_pixel(&mut self, x: i32, y: i32) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let idx = ((y * self.width + x) * 4) as usize;
        if idx + 3 < self.pixels.len() {
            self.pixels[idx] = 0;
            self.pixels[idx + 1] = 0;
            self.pixels[idx + 2] = 0;
            self.pixels[idx + 3] = 0;
        }
    }

    /// Clears the surface to "transparent".
    ///
    /// ⚠️ 底色 alpha 必须**恰好为 0**，不能是 1/255 之类的「近零」值：
    /// 分层窗口（`WS_EX_LAYERED` + `UpdateLayeredWindow`）的命中测试由系统
    /// 按像素 alpha 判定 —— 只有 **alpha == 0** 的像素才会让鼠标事件穿透到
    /// 下方窗口（MSDN Layered Windows: "areas … whose alpha value is zero
    /// will let the mouse messages through"）。alpha=1 的像素仍会被本窗口吞掉
    /// 点击，且 `WM_NCHITTEST` 返回 `HTTRANSPARENT` **无法**跨线程/跨进程
    /// 补救（只对同线程窗口有效，跨进程会把点击直接吃掉）。
    ///
    /// 副作用（有意为之）：窗口只能按在文字/描边等实际有像素的地方拖动。
    pub fn clear(&mut self) {
        self.pixels.fill(0);
    }

    /// 只清空一个子矩形（按行填充，A3：省掉整画布 memset）。
    ///
    /// 负坐标/越界自动裁剪到画布范围，不 panic。
    /// 填充值 = [0,0,0,0]（BGRA 全透明）：与 [`Self::clear`] 同底色，
    /// 保证被清空的区域同样点击穿透（见 [`Self::clear`] 的说明）。
    pub fn clear_rect(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let x = x.max(0);
        let y = y.max(0);
        let w = w.max(0);
        let h = h.max(0);
        let x1 = (x + w).min(self.width);
        let y1 = (y + h).min(self.height);
        for yy in y..y1 {
            let start = ((yy * self.width + x) * 4) as usize;
            let end = ((yy * self.width + x1) * 4) as usize;
            if let Some(row) = self.pixels.get_mut(start..end) {
                for px in row.as_chunks_mut::<4>().0 {
                    px[0] = 0;
                    px[1] = 0;
                    px[2] = 0;
                    px[3] = 0;
                }
            }
        }
    }

    /// Converts the surface to a PNG-encoded byte vector.
    ///
    /// Uses the `png` crate. 仅测试用（PNG 对拍基线导出）：`png` 是
    /// dev-dependency，生产构建不编译它；勿在非测试路径调用本函数。
    #[cfg(test)]
    pub fn to_png(&self) -> Vec<u8> {
        use png::{Encoder, ColorType};

        let mut output = Vec::new();
        {
            let mut encoder = Encoder::new(&mut output, self.width as u32, self.height as u32);
            encoder.set_color(ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            // Convert BGRA to RGBA for PNG
            let rgba_pixels: Vec<u8> = self
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|c| [c[2], c[1], c[0], c[3]])
                .collect();
            writer.write_image_data(&rgba_pixels).unwrap();
        }
        output
    }
}

impl SoftwareSurface {
    /// Clears (sets to transparent) the four corner regions of a rounded rectangle.
    ///
    /// 填充矩形后再调用本函数，清除四个角的四分之一圆像素，实现圆角效果。
    /// 审查 109：逐像素**直接置 0**（clear_pixel），不能用 over 合成"清"
    /// —— src_a=0 时合成是恒等操作，圆角永远清不掉。
    fn clear_corners(&mut self, r: Rect, radius: i32, _color: Rgba) {
        if radius <= 0 {
            return;
        }
        let radius = radius.min(r.w / 2).min(r.h / 2);
        let r_right = r.x.saturating_add(r.w);
        let r_bottom = r.y.saturating_add(r.h);

        // 左上角：圆心 (r.x, r.y)，清除圆内像素
        for dy in 0..radius {
            let dx_max = ((radius * radius - dy * dy) as f64).sqrt() as i32;
            for dx in 0..=dx_max {
                self.clear_pixel(r.x + dx, r.y + dy);
            }
        }
        // 右上角：圆心 (r.x + r.w, r.y)
        for dy in 0..radius {
            let dx_max = ((radius * radius - dy * dy) as f64).sqrt() as i32;
            for dx in 0..=dx_max {
                self.clear_pixel(r_right - dx, r.y + dy);
            }
        }
        // 左下角：圆心 (r.x, r.y + r.h)
        for dy in 0..radius {
            let dx_max = ((radius * radius - dy * dy) as f64).sqrt() as i32;
            for dx in 0..=dx_max {
                self.clear_pixel(r.x + dx, r_bottom - dy);
            }
        }
        // 右下角：圆心 (r.x + r.w, r.y + r.h)
        for dy in 0..radius {
            let dx_max = ((radius * radius - dy * dy) as f64).sqrt() as i32;
            for dx in 0..=dx_max {
                self.clear_pixel(r_right - dx, r_bottom - dy);
            }
        }
    }
}

/// Checks if a point is inside a circle centered at (cx, cy) with given radius.
fn point_in_circle(x: i32, y: i32, cx: i32, cy: i32, radius: i32) -> bool {
    let dx = (x - cx) as i64;
    let dy = (y - cy) as i64;
    dx * dx + dy * dy <= (radius as i64) * (radius as i64)
}

/// Checks if a point is inside a rounded rectangle.
fn point_in_round_rect(x: i32, y: i32, r: &Rect, radius: i32) -> bool {
    if x < r.x || x >= r.right() || y < r.y || y >= r.bottom() {
        return false;
    }
    if radius <= 0 {
        return true;
    }

    let inner_x = r.x + radius;
    let inner_y = r.y + radius;
    let inner_w = r.w - radius * 2;
    let inner_h = r.h - radius * 2;

    // If corners meet/overlap, the inner strip collapses to a line or empty space.
    // In that case every pixel in the bounding box is valid (the whole rect looks pill-shaped).
    if inner_h <= 0 || inner_w <= 0 {
        return true;
    }

    // Clearly inside the center strip.
    if x >= inner_x && x < inner_x + inner_w && y >= inner_y && y < inner_y + inner_h {
        return true;
    }

    // Check the relevant corner only.
    if x < r.x + radius {
        // Left side
        if y < r.y + radius {
            return point_in_circle(x, y, r.x + radius, r.y + radius, radius); // top-left
        } else if y >= r.bottom() - radius {
            return point_in_circle(x, y, r.x + radius, r.bottom() - radius, radius); // bottom-left
        }
    } else if x >= r.right() - radius {
        // Right side
        if y < r.y + radius {
            return point_in_circle(x, y, r.right() - radius, r.y + radius, radius); // top-right
        } else if y >= r.bottom() - radius {
            return point_in_circle(x, y, r.right() - radius, r.bottom() - radius, radius); // bottom-right
        }
    }

    false
}

impl RenderTarget for SoftwareSurface {
    fn clear(&mut self) {
        self.pixels.fill(0);
    }

    /// 按系数缩放 alpha。
    ///
    /// 注意：像素是**预乘**的，只改 alpha 会让残留的 RGB 在合成时溢出，
    /// 所以 RGB 必须同步按同一系数缩放（这正是 v2 §1.2 `blend_onto` 要表达的语义）。
    fn scale_alpha(&mut self, factor: f32) {
        // 审查 501：clamp 对 NaN 返回 NaN，`as u8` 得 0 会把整帧抹成透明；
        // 非有限因子视为无效，整帧保持不变（no-op 而非破坏）。
        let f = if factor.is_finite() {
            factor.clamp(0.0, 1.0)
        } else {
            return;
        };
        for p in self.pixels.as_chunks_mut::<4>().0 {
            p[0] = (p[0] as f32 * f) as u8;
            p[1] = (p[1] as f32 * f) as u8;
            p[2] = (p[2] as f32 * f) as u8;
            p[3] = (p[3] as f32 * f) as u8;
        }
    }

    /// Fills a rectangle with rounded corners using pre-multiplied alpha compositing.
    fn fill_round_rect(&mut self, r: Rect, radius: i32, color: Rgba) {
        if r.w <= 0 || r.h <= 0 || color.a == 0 {
            return;
        }

        // 半径过大时（常见于小高度进度条），先填满再做四角裁圆。
        if radius.saturating_mul(2) >= r.h {
            self.fill_rect(r, color);
            if radius <= 0 {
                return;
            }
            let clamped = radius.min(r.w / 2).min(r.h / 2);
            self.clear_corners(r, clamped, color);
            return;
        }

        // 标准圆角路径：逐像素判定。
        // 审查 502：先裁剪到画布 —— 负/超大 r 不再遍历画布外的巨大范围。
        let radius = radius.min(r.w / 2).min(r.h / 2);
        let clip = r.clip_to_surface(self.width, self.height);
        if clip.w <= 0 || clip.h <= 0 {
            return;
        }
        for y in clip.y..clip.bottom() {
            for x in clip.x..clip.right() {
                if point_in_round_rect(x, y, &r, radius) {
                    self.blend_pixel(x, y, color);
                }
            }
        }
    }

    fn fill_rect(&mut self, r: Rect, color: Rgba) {
        if r.w <= 0 || r.h <= 0 || color.a == 0 {
            return;
        }

        // Clip to surface bounds
        let r = r.clip_to_surface(self.width, self.height);
        if r.w <= 0 || r.h <= 0 {
            return;
        }

        // 逐行取切片，兼顾安全与速度：
        // 切片自身保证行内不越界，避免了每个像素重复做边界检查。
        let factor = 1.0 - color.alpha_float();
        let src_a = color.a as f32;

        for y in r.y..r.bottom() {
            let start = ((y * self.width + r.x) * 4) as usize;
            let end = start + (r.w * 4) as usize;
            let row = match self.pixels.get_mut(start..end) {
                Some(s) => s,
                None => continue,
            };
            for p in row.as_chunks_mut::<4>().0 {
                // BGRA
                p[0] = (color.b as f32 + p[0] as f32 * factor) as u8;
                p[1] = (color.g as f32 + p[1] as f32 * factor) as u8;
                p[2] = (color.r as f32 + p[2] as f32 * factor) as u8;
                p[3] = (src_a + p[3] as f32 * factor).min(255.0) as u8;
            }
        }
    }

    /// Draws text at the specified position.
    ///
    /// Uses bitmap font rendering with the given color.
    /// Non-ASCII characters are rendered as ■.
    fn draw_text(&mut self, s: &str, at: Rect, color: Rgba, _font: FontSpec) {
        if s.is_empty() || color.a == 0 {
            return;
        }

        let mut x = at.x;
        let y = at.y;

        // Approximate: clip to rect height (we use fixed 16px font height)
        let _clip_h = at.h.min(FONT_HEIGHT);

        let limit = at.x + at.w;
        for c in s.chars() {
            if x + FONT_WIDTH > limit {
                break; // 超出给定矩形就停，不换行
            }
            render_char(
                &mut self.pixels,
                self.width,
                self.height,
                c,
                x,
                y,
                color,
            );
            x += FONT_WIDTH + FONT_SPACING;
        }
    }

    /// Measures text dimensions when rendered with the bitmap font.
    fn measure_text(&self, s: &str, _font: FontSpec) -> (i32, i32) {
        (measure(s), FONT_HEIGHT)
    }

    fn blit_coverage(
        &mut self,
        at: Rect,
        coverage: &[u8],
        cov_stride: usize,
        cov_w: i32,
        cov_h: i32,
        color: Rgba,
    ) {
        if at.w <= 0 || at.h <= 0 || color.a == 0 {
            return;
        }
        let w = at.w.min(cov_w);
        let h = at.h.min(cov_h);

        for y in 0..h {
            let start = (y as usize) * cov_stride;
            let row = match coverage.get(start..start + w as usize) {
                Some(r) => r,
                None => continue,
            };
            for x in 0..w {
                let cov = row[x as usize];
                if cov == 0 {
                    continue;
                }
                // 审查 110：color 是预乘 Rgba，不能走 Rgba::new 二次预乘（变暗）。
                // 正确语义 = 按覆盖率同步缩放预乘 RGB 与 alpha。
                let t = cov as f32 / 255.0;
                let a = ((color.a as u32 * cov as u32) / 255) as u8;
                let c = Rgba {
                    r: (color.r as f32 * t).round() as u8,
                    g: (color.g as f32 * t).round() as u8,
                    b: (color.b as f32 * t).round() as u8,
                    a,
                };
                self.blend_pixel(at.x + x, at.y + y, c);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_surface_creation() {
        let surface = SoftwareSurface::new(100, 50);
        assert_eq!(surface.width, 100);
        assert_eq!(surface.height, 50);
        assert_eq!(surface.pixels.len(), 100 * 50 * 4);
    }

    #[test]
    fn test_fill_rect() {
        let mut surface = SoftwareSurface::new(100, 100);
        surface.fill_rect(Rect::with_pos(10, 10, 20, 30), Rgba::opaque(255, 0, 0));

        // Check a pixel inside
        let offset = (10 * 100 + 10) * 4;
        assert_eq!(surface.pixels[offset], 0); // B
        assert_eq!(surface.pixels[offset + 1], 0); // G
        assert_eq!(surface.pixels[offset + 2], 255); // R
    }

    #[test]
    fn test_measure_text() {
        let surface = SoftwareSurface::new(100, 100);
        let (w, h) = surface.measure_text("Hello", FontSpec::ui());
        assert_eq!(w, 5 * (FONT_WIDTH + FONT_SPACING) - FONT_SPACING);
        assert_eq!(h, FONT_HEIGHT);
    }

    #[test]
    fn test_to_png() {
        let mut surface = SoftwareSurface::new(4, 4);
        surface.fill_rect(Rect::new(4, 4), Rgba::opaque(255, 0, 0));
        let png = surface.to_png();
        // PNG header check
        assert_eq!(&png[0..8], &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
    }

    /// Regression: fill_round_rect(100x6, radius=3) must actually write pixels.
    #[test]
    fn test_fill_round_rect_small() {
        let mut s = SoftwareSurface::new(100, 6);
        // Simulate QualityDecoration's draw_progress: track (100x6, radius=3)
        s.fill_round_rect(Rect::with_pos(0, 0, 100, 6), 3, Rgba::opaque(41, 43, 48));
        // Center pixel must be painted
        let center = ((3 * 100 + 50) * 4) as usize;
        assert_ne!(s.pixels[center + 3], 0, "center pixel alpha must be > 0");
        // 审查 503：四个角落必须仍是全透明（clear_corners 真正生效的回归锁）
        for (cx, cy) in [(0usize, 0usize), (99, 0), (0, 5), (99, 5)] {
            let o = (cy * 100 + cx) * 4;
            assert_eq!(s.pixels[o + 3], 0, "corner ({cx},{cy}) must stay transparent");
        }
    }
}
