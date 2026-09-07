//! SysOverlay UI crate —— 离屏软件光栅化与指标条绘制。
//!
//! # 硬约束
//!
//! 本 crate **零 Win32 调用、零 `unsafe`**。
//! 需要平台能力的部分（DIB 上屏）一律通过 `win` 平台的
//! [`SoftwareSurface`] 搬运实现。
//!
//! # 模块
//!
//! - `render` — 渲染抽象（`Rect`/`Rgba`/`RenderTarget`/`FontSpec`）
//! - `offscreen` — `SoftwareSurface`：BGRA **预乘**缓冲，与 Windows DIB 布局一致
//! - `font` — 8×16 ASCII 点阵字体（支持浮点放大，0.5 步进）
//! - `palette` — `Style`：把 `core::color` 的纯逻辑调色板转成预乘色
//! - `renderer` — 渲染器接口 + 横版/竖版两种条状实现

pub mod render;
pub mod offscreen;
pub mod font;
pub mod palette;
pub mod renderer;

// 常用类型重导出
pub use font::{render_char, render_str_scaled, render_str_scaled_f, scale_of_font};
pub use offscreen::SoftwareSurface;
pub use palette::Style;
pub use render::{FontSpec, Rect, RenderTarget, Rgba};
pub use renderer::{Renderer, StripRenderer, VerticalRenderer};