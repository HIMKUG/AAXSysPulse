//! AAXSysPulse UI crate —— 离屏软件光栅化与指标条绘制。
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

// 批次 6·①分类基线：与 core 同一口径 —— fixture 用「default() 后逐字段赋值」，
// 只在测试编译时放过该 lint（生产代码 0 处命中）。
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]
// 批次 6·②：非测试代码禁 `unwrap()`（生产代码实测 0 处命中，只挡未来回归）。
#![cfg_attr(not(test), warn(clippy::unwrap_used))]

pub mod render;
pub mod offscreen;
pub mod font;
pub mod palette;
pub mod renderer;
pub mod clock;

// 常用类型重导出
pub use font::{render_char, render_str_scaled, render_str_scaled_f, scale_of_font};
pub use offscreen::SoftwareSurface;
pub use palette::Style;
pub use render::{FontSpec, Rect, RenderTarget, Rgba};
pub use renderer::{Renderer, StripRenderer, VerticalRenderer};