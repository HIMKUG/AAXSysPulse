//! Rendering primitives and traits for UI drawing.
//!
//! This module defines the core rendering abstraction used by the UI crate.
//! Implementors must provide software rasterization primitives for:
//! - Filled rectangles (with optional rounded corners)
//! - Text rendering with font control
//! - Text measurement

/// Rectangle with pixel coordinates (top-left origin).
#[derive(Clone, Copy, Debug, Default)]
pub struct Rect {
    /// X coordinate in pixels.
    pub x: i32,
    /// Y coordinate in pixels.
    pub y: i32,
    /// Width in pixels.
    pub w: i32,
    /// Height in pixels.
    pub h: i32,
}

impl Rect {
    /// Creates a new rect at (0, 0) with the given dimensions.
    pub fn new(w: i32, h: i32) -> Self {
        Self { x: 0, y: 0, w, h }
    }

    /// Creates a new rect at (x, y) with the given dimensions.
    pub fn with_pos(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Returns the right edge (x + w).
    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    /// Returns the bottom edge (y + h).
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// Clips this rect to not exceed surface bounds.
    pub fn clip_to_surface(&self, surface_w: i32, surface_h: i32) -> Self {
        let x = self.x.max(0).min(surface_w);
        let y = self.y.max(0).min(surface_h);
        let w = (self.w - (x - self.x)).max(0).min(surface_w - x);
        let h = self.h - (y - self.y).max(0).min(surface_h - y);
        Self { x, y, w, h }
    }
}

/// RGBA color with pre-multiplied alpha support.
///
/// In pre-multiplied alpha, the RGB components are scaled by the alpha value.
/// This format is used internally by our BGRA software rasterizer.
#[derive(Clone, Copy, Debug, Default)]
pub struct Rgba {
    /// Red component (0-255, pre-multiplied by alpha).
    pub r: u8,
    /// Green component (0-255, pre-multiplied by alpha).
    pub g: u8,
    /// Blue component (0-255, pre-multiplied by alpha).
    pub b: u8,
    /// Alpha component (0-255). 0 = fully transparent, 255 = fully opaque.
    pub a: u8,
}

impl Rgba {
    /// Creates a new RGBA color from individual components (non-premultiplied).
    ///
    /// The RGB values will be pre-multiplied by alpha when stored.
    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        let a_f = a as f32 / 255.0;
        Self {
            r: (r as f32 * a_f) as u8,
            g: (g as f32 * a_f) as u8,
            b: (b as f32 * a_f) as u8,
            a,
        }
    }

    /// Creates an opaque RGBA color (alpha = 255).
    pub fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Creates a fully transparent color.
    pub fn transparent() -> Self {
        Self { r: 0, g: 0, b: 0, a: 0 }
    }

    /// Returns the alpha as a float in range [0.0, 1.0].
    pub fn alpha_float(&self) -> f32 {
        self.a as f32 / 255.0
    }

    /// Converts to a Windows COLORREF value (0x00BBGGRR format).
    pub fn to_colorref(&self) -> u32 {
        (self.r as u32) | ((self.g as u32) << 8) | ((self.b as u32) << 16)
    }
}

/// Font specification for text rendering.
#[derive(Clone, Copy, Debug, Default)]
pub struct FontSpec {
    /// Font size in points (72 points = 1 inch).
    pub size_pt: f32,
    /// Whether the font should be bold.
    pub bold: bool,
}

impl FontSpec {
    /// Creates a new font spec.
    pub fn new(size_pt: f32, bold: bool) -> Self {
        Self { size_pt, bold }
    }

    /// Default UI font: 12pt regular.
    pub fn ui() -> Self {
        Self::new(12.0, false)
    }

    /// Default monospace font: 12pt semi-bold.
    pub fn mono() -> Self {
        Self::new(12.0, true)
    }
}

/// Core rendering trait for software rasterization.
///
/// This trait is implemented by surfaces that can be drawn to.
/// All coordinates are in pixels with origin at top-left.
pub trait RenderTarget {
    /// Fills a rectangle with rounded corners using the given color.
    ///
    /// If `radius` is 0, behaves like a regular filled rectangle.
    fn fill_round_rect(&mut self, r: Rect, radius: i32, color: Rgba);

    /// Fills a rectangle with sharp corners using the given color.
    fn fill_rect(&mut self, r: Rect, color: Rgba);

    /// Draws text at the specified position with the given color and font.
    ///
    /// Text is clipped to the `at` rectangle bounds.
    fn draw_text(&mut self, s: &str, at: Rect, color: Rgba, font: FontSpec);

    /// Measures the pixel dimensions of text when rendered with the given font.
    ///
    /// Returns (width, height) in pixels.
    fn measure_text(&self, s: &str, font: FontSpec) -> (i32, i32);
}
