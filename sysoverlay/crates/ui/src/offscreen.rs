//! Software-based offscreen rendering surface.
//!
//! This module provides a pure Rust software rasterizer for rendering
//! without a window. It produces PNG output for pixel-level comparison
//! with the C reference implementation.
//!
//! Pixel format: BGRA with pre-multiplied alpha (matching Windows DIB format).

use crate::render::{FontSpec, Rect, RenderTarget, Rgba};
use crate::font::{FONT_WIDTH, FONT_HEIGHT, FONT_SPACING, render_char};

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
        let size = (width * height * 4) as usize;
        Self {
            pixels: vec![0u8; size],
            width,
            height,
        }
    }

    /// Returns the byte offset for pixel (x, y) in BGRA format.
    fn pixel_offset(&self, x: i32, y: i32) -> i32 {
        (y * self.width + x) * 4
    }

    /// Returns a mutable pointer to the pixel at (x, y).
    ///
    /// # Safety
    ///
    /// The caller must ensure x and y are within surface bounds.
    fn pixel_ptr_mut(&mut self, x: i32, y: i32) -> *mut u8 {
        let offset = self.pixel_offset(x, y);
        // SAFETY: offset is computed from valid coordinates and stays within bounds
        unsafe { self.pixels.as_mut_ptr().add(offset as usize) }
    }

    /// Clears the surface to transparent.
    pub fn clear(&mut self) {
        self.pixels.fill(0);
    }

    /// Converts the surface to a PNG-encoded byte vector.
    ///
    /// Uses the `png` crate (dev-dependency only).
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
            let rgba_pixels: Vec<u8> = self.pixels
                .chunks_exact(4)
                .flat_map(|chunk| [chunk[2], chunk[1], chunk[0], chunk[3]])
                .collect();
            writer.write_image_data(&rgba_pixels).unwrap();
        }
        output
    }
}

/// Checks if a point is inside a rounded rectangle.
///
/// Uses simple circle approximation at corners.
fn point_in_round_rect(x: i32, y: i32, r: &Rect, radius: i32) -> bool {
    // Outside bounding box
    if x < r.x || x >= r.right() || y < r.y || y >= r.bottom() {
        return false;
    }

    if radius <= 0 {
        return true; // Sharp corners
    }

    // Check corner regions
    let inner = Rect::with_pos(r.x + radius, r.y + radius, r.w - radius * 2, r.h - radius * 2);
    if x >= inner.x && x < inner.right() && y >= inner.y && y < inner.bottom() {
        return true; // Center region
    }

    // Corner checks
    let corners = [
        (r.x + radius, r.y + radius),                           // top-left
        (r.right() - radius, r.y + radius),                     // top-right
        (r.x + radius, r.bottom() - radius),                    // bottom-left
        (r.right() - radius, r.bottom() - radius),              // bottom-right
    ];

    for (cx, cy) in corners {
        let dx = x - cx;
        let dy = y - cy;
        let dist_sq = (dx * dx + dy * dy) as i64;
        let radius_sq = (radius as i64) * (radius as i64);
        if dist_sq <= radius_sq {
            return true;
        }
    }

    false
}

impl RenderTarget for SoftwareSurface {
    /// Fills a rectangle with rounded corners using pre-multiplied alpha compositing.
    fn fill_round_rect(&mut self, r: Rect, radius: i32, color: Rgba) {
        if r.w <= 0 || r.h <= 0 || color.a == 0 {
            return;
        }

        // For small radius, just use fill_rect
        if radius <= 0 {
            self.fill_rect(r, color);
            return;
        }

        // Clamp radius to prevent artifacts
        let radius = radius.min(r.w / 2).min(r.h / 2);

        // Alpha factor for compositing
        let src_a = color.alpha_float();

        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                if point_in_round_rect(x, y, &r, radius) {
                    let ptr = self.pixel_ptr_mut(x, y);
                    unsafe {
                        // BGRA order
                        let dst_b = *ptr as f32;
                        let dst_g = *ptr.add(1) as f32;
                        let dst_r = *ptr.add(2) as f32;
                        let dst_a = *ptr.add(3) as f32;

                        // Pre-multiplied alpha compositing: out = src + dst * (1 - src_a)
                        let factor = 1.0 - src_a;
                        let out_r = color.r as f32 + dst_r * factor;
                        let out_g = color.g as f32 + dst_g * factor;
                        let out_b = color.b as f32 + dst_b * factor;
                        let out_a = (src_a * 255.0 + dst_a * factor).min(255.0);

                        *ptr = out_b as u8;
                        *ptr.add(1) = out_g as u8;
                        *ptr.add(2) = out_r as u8;
                        *ptr.add(3) = out_a as u8;
                    }
                }
            }
        }
    }

    /// Fills a rectangle with sharp corners.
    fn fill_rect(&mut self, r: Rect, color: Rgba) {
        if r.w <= 0 || r.h <= 0 || color.a == 0 {
            return;
        }

        // Clip to surface bounds
        let r = r.clip_to_surface(self.width, self.height);
        if r.w <= 0 || r.h <= 0 {
            return;
        }

        let src_a = color.alpha_float();

        for y in r.y..r.bottom() {
            let row_offset = y * self.width * 4;
            for x in r.x..r.right() {
                let offset = row_offset + x * 4;
                let idx = offset as usize;
                // BGRA
                let dst_b = self.pixels[idx] as f32;
                let dst_g = self.pixels[idx + 1] as f32;
                let dst_r = self.pixels[idx + 2] as f32;
                let dst_a = self.pixels[idx + 3] as f32;

                let factor = 1.0 - src_a;
                let out_r = color.r as f32 + dst_r * factor;
                let out_g = color.g as f32 + dst_g * factor;
                let out_b = color.b as f32 + dst_b * factor;
                let out_a = (src_a * 255.0 + dst_a * factor).min(255.0);

                self.pixels[idx] = out_b as u8;
                self.pixels[idx + 1] = out_g as u8;
                self.pixels[idx + 2] = out_r as u8;
                self.pixels[idx + 3] = out_a as u8;
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

        for c in s.chars() {
            // Simple character-by-character rendering with clipping
            if x + FONT_WIDTH <= at.x + at.w {
                unsafe {
                    render_char(
                        self.pixels.as_mut_ptr(),
                        self.width * 4,
                        c,
                        x,
                        y,
                        color,
                    );
                }
            }
            x += FONT_WIDTH + FONT_SPACING;
        }
    }

    /// Measures text dimensions when rendered with the bitmap font.
    fn measure_text(&self, s: &str, _font: FontSpec) -> (i32, i32) {
        let width = (s.len() as i32) * (FONT_WIDTH + FONT_SPACING) - FONT_SPACING;
        (width.max(0), FONT_HEIGHT)
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
}
