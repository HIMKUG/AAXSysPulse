//! SysOverlay UI Library
//!
//! This crate provides the offscreen software rasterization backend for
//! rendering overlay cards without a window. Used for pixel-level comparison
//! with the C reference implementation.
//!
//! # Modules
//!
//! - `render` - Core rendering traits and types (Rect, Rgba, RenderTarget)
//! - `offscreen` - SoftwareSurface implementation for BGRA rendering
//! - `card` - Card drawing functions matching C drawCard logic
//! - `font` - 8x16 bitmap font for ASCII text rendering (placeholder)

mod render;
mod offscreen;
mod card;
mod font;

// Re-export core types
pub use render::{FontSpec, Rect, RenderTarget, Rgba};
pub use offscreen::SoftwareSurface;

// Re-export for convenience
pub use card::{draw_card, demo_snapshot, Palette};
