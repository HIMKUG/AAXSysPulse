//! `ui` 侧的调色板：把 `core::color::Palette`（`Rgb`）转成预乘 `Rgba`。
//!
//! 颜色**派生规则**留在 `core::color`（纯逻辑、可离线单测），
//! 这里只做表示层转换，不再复制一份阈值判断。

use crate::render::Rgba;
use sysoverlay_core::color::{Palette as CorePalette, Rgb};

/// 渲染期使用的调色板（预乘 alpha）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiPalette {
    /// 背景
    pub bg: Rgba,
    /// 边框
    pub border: Rgba,
    /// 进度条底槽
    pub track: Rgba,
    /// 标题
    pub title: Rgba,
    /// 指标名
    pub name: Rgba,
    /// 指标值
    pub value: Rgba,
    /// 次要信息
    pub sub: Rgba,
    /// 陈旧指标名（降饱和）
    pub name_stale: Rgba,
    /// 陈旧指标值（降饱和）
    pub value_stale: Rgba,
}

impl UiPalette {
    /// 由 `core` 的纯逻辑调色板构造。
    pub fn from_core(core: CorePalette) -> Self {
        Self {
            bg: opaque(core.bg),
            border: opaque(core.border),
            track: opaque(core.track),
            title: opaque(core.title),
            name: opaque(core.name),
            value: opaque(core.value),
            sub: opaque(core.sub),
            name_stale: opaque(core.name.desaturate(0.55)),
            value_stale: opaque(core.value.desaturate(0.55)),
        }
    }
}

fn opaque(c: Rgb) -> Rgba {
    Rgba::opaque(c.r, c.g, c.b)
}

/// 一次绘制所需的**全部样式**：纯逻辑调色板 + 预乘调色板。
///
/// 之所以两个都带着：`core::Palette` 提供阈值判断（纯逻辑、可测），
/// `UiPalette` 提供可直接上屏的预乘色（表示层）。
/// 打包在一起避免到处传两个参数。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// 纯逻辑调色板（阈值/派生规则）。
    pub core: CorePalette,
    /// 预乘调色板（直接给渲染用）。
    pub ui: UiPalette,
}

impl Style {
    /// 由 (bg, fg) 派生整套样式。
    pub fn derive(bg: Rgb, fg: Rgb) -> Self {
        let core = CorePalette::derive(bg, fg);
        Self {
            core,
            ui: UiPalette::from_core(core),
        }
    }

    /// 新鲜值的强调色。
    pub fn accent(&self, pct: f32) -> Rgba {
        opaque(self.core.accent_for(pct))
    }

    /// 陈旧值的强调色（降饱和，保留色相层次）。
    pub fn stale_accent(&self, pct: f32) -> Rgba {
        opaque(self.core.stale_accent_for(pct))
    }
}

impl Default for Style {
    fn default() -> Self {
        Self::derive(Rgb::default(), Rgb::new(238, 241, 244))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_keeps_raw_channels() {
        let s = Style::derive(Rgb::new(20, 22, 27), Rgb::new(238, 241, 244));
        assert_eq!(s.ui.bg, Rgba::opaque(20, 22, 27));
        assert_eq!(s.ui.value, Rgba::opaque(238, 241, 244));
    }

    #[test]
    fn stale_colors_are_more_gray_than_fresh() {
        let s = Style::default();
        let spread = |c: Rgba| (c.r as i32 - c.b as i32).abs();
        assert!(
            spread(s.ui.value_stale) <= spread(s.ui.value),
            "陈旧值的通道离散度必须更小（更灰）"
        );
    }

    #[test]
    fn accent_delegates_to_core() {
        let s = Style::default();
        assert_eq!(s.accent(90.0), Rgba::opaque(248, 113, 113));
        let stale = s.stale_accent(90.0);
        assert!(stale.r < 248, "陈旧强调色必须更暗");
    }

    #[test]
    fn default_matches_c_defaults() {
        let s = Style::default();
        assert_eq!(s.core.bg, Rgb::new(20, 22, 27));
        // `Palette::value` 就是 fg 本身（未被缩放）
        assert_eq!(s.core.value, Rgb::new(238, 241, 244));
    }
}
