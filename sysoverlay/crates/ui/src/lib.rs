//! SysOverlay UI crate —— 离屏软件光栅化与卡片绘制。
//!
//! # 硬约束
//!
//! 本 crate **零 Win32 调用、零 `unsafe`**。
//! 需要平台能力的部分（GDI 文字、DIB 上屏）一律通过 trait 注入：
//! - [`raster::TextRasterizer`] —— 文字光栅化（`win` 可提供 GDI 实现）
//! - [`render::RenderTarget`] —— 像素落点（`win` 可提供 DIB 直写实现）
//!
//! # 模块
//!
//! - `render` — 渲染抽象（`Rect`/`Rgba`/`RenderTarget`/`FontSpec`）
//! - `offscreen` — `SoftwareSurface`：BGRA **预乘**缓冲，与 Windows DIB 布局一致
//! - `font` — 8×16 ASCII 点阵字体（Eco 路径）
//! - `palette` — `Style`：把 `core::color` 的纯逻辑调色板转成预乘色
//! - `raster` — 文字光栅化注入点
//! - `decor` — 背景/进度条装饰注入点（Eco / Quality）
//! - `card` — 卡片绘制**唯一骨架**

pub mod render;
pub mod offscreen;
pub mod font;
pub mod palette;
pub mod raster;
pub mod decor;
pub mod card;
pub mod renderer;

// 常用类型重导出
pub use card::{demo_snapshot, draw_card, DrawCtx};
pub use decor::{decoration_for, Decoration, EcoDecoration, QualityDecoration};
pub use font::render_char;
pub use offscreen::SoftwareSurface;
pub use palette::{Style, UiPalette};
pub use raster::{BitmapTextRasterizer, TextRasterizer};
pub use render::{FontSpec, Rect, RenderTarget, Rgba};
pub use renderer::{ClassicRenderer, Renderer, StripRenderer};
