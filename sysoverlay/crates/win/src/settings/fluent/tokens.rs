//! Fluent Design System 2 token 库（路线 A：拆壳移植）。
//!
//! S1 交接物：按 `FluentToken映射表.md` §3 新增 Token 转录。
//! 所有 token 均以 D2D `ColorF`（rgba 0-1 浮点）存储，供绘制管线按需引用。
//!
//! 使用方式（示例）：
//! ```ignore
//! let tokens = fluent_tokens::FLUENT.light;
//! let bg = tokens.color_neutral_background1;
//! ```

use crate::gfx_d2d::ColorF;

// ── FluentTokens 结构体 ─────────────────────────────────────

/// Fluent token 集：浅色一套 + 深色一套。
///
/// 按需引用原则：现有 Theme 字段继续使用；当 switch/slider/button 等
/// Fluent 组件需要额外 token 时，从本模块取值。
#[derive(Clone, Copy, Debug)]
pub struct FluentTokens {
    pub light: FluentPalette,
    pub dark: FluentPalette,
}

/// 浅色 / 深色各自的一组 token。
#[derive(Clone, Copy, Debug)]
pub struct FluentPalette {
    // ── 颜色 ──────────────────────────────────────────────
    pub color_neutral_background1: ColorF,         // 窗口/卡片底（白/深灰）
    pub color_neutral_background3: ColorF,         // 控件底（灰/深灰）
    pub color_neutral_background6: ColorF,          // 深色选中片底
    pub color_brand_background: ColorF,             // 品牌色（蓝）
    pub color_brand_background_hover: ColorF,       // 品牌悬停
    pub color_brand_background_pressed: ColorF,     // 品牌按下
    pub color_compound_brand_background: ColorF,    // switch 填充色
    pub color_neutral_foreground_inverted: ColorF,   // 白色（旋钮）
    pub color_neutral_stroke_accessible: ColorF,    // switch 关闭描边
    pub color_neutral_stroke1: ColorF,              // 描边 1
    pub color_neutral_stroke2: ColorF,              // 描边 2（分隔线）
    pub color_subtle_background_hover: ColorF,      // 按钮悬停背景

    // ── 尺寸 ──────────────────────────────────────────────
    pub stroke_width_thin: f32,          // 边框宽
    pub border_radius_medium: f32,       // 卡片圆角（注意：现有 10 → S5 统一改 4）
    pub border_radius_small: f32,        // 控件圆角

    // ── 字体 ──────────────────────────────────────────────
    pub font_family_base: &'static str,
    pub font_weight_regular: u16,
    pub font_weight_semibold: u16,
    pub font_size_base200: f32,   // 12.0 ← 现有 caption_px(96dpi) ≈ 11，统一改 12
    pub font_size_base300: f32,  // 14.0 ← 现有 body_px(96dpi) ≈ 13，统一改 14
    pub font_size_base400: f32,  // 16.0 分组标题
    pub line_height_base200: f32,
    pub line_height_base300: f32,

    // ── 间距（Quel'Thalas web_light/web_dark 原值；vertical_m=12 恰与 layout ROW_GAP=12 一致）──
    pub spacing_horizontal_s: f32,    // 8.0
    pub spacing_horizontal_m: f32,    // 12.0
    pub spacing_horizontal_l: f32,    // 16.0（= 现有 CARD_PAD）
    pub spacing_vertical_s: f32,      // 8.0（= 现有 CAPTION_GAP）
    pub spacing_vertical_m: f32,      // 12.0（= 现有 ROW_GAP）
    pub spacing_vertical_l: f32,      // 16.0

    // ── 动效 ──────────────────────────────────────────────
    pub duration_faster: f32,         // 50ms（WAM 等效）
    pub duration_normal: f32,          // 167ms（WAM 等效）
    pub curve_easy_ease: [f32; 4],    // Fluent 标准曲线
    pub curve_decelerate_mid: [f32; 4],
}

// ── 颜色辅助 ─────────────────────────────────────────────────

/// Fluent 颜色解析：r/g/b 0-255 → ColorF rgba。
const fn c(r: u8, g: u8, b: u8) -> ColorF {
    ColorF::rgba(r, g, b, 1.0)
}

/// Fluent 颜色解析（带透明度）：r/g/b 0-255，a 0.0-1.0。
const fn ca(r: u8, g: u8, b: u8, a: f32) -> ColorF {
    ColorF::rgba(r, g, b, a)
}

// ── FluentPalette 浅色版 ─────────────────────────────────────

/// Fluent 2.0 浅色 token（Fluent Design System 标准值）。
///
/// 参考来源：
/// - <https://learn.microsoft.com/en-us/windows/apps/design/wui-language/color#neutral-colors>
const fn fluent_light() -> FluentPalette {
    FluentPalette {
        // 颜色：逐值转录自 Quel'Thalas theme.rs web_light()（唯一真值，勿凭记忆改）。
        color_neutral_background1: c(255, 255, 255),          // #ffffff
        color_neutral_background3: c(245, 245, 245),           // #f5f5f5
        color_neutral_background6: c(230, 230, 230),           // #e6e6e6
        color_brand_background: c(15, 108, 189),               // #0f6cbd
        color_brand_background_hover: c(17, 94, 163),          // #115ea3
        color_brand_background_pressed: c(12, 59, 94),         // #0c3b5e
        color_compound_brand_background: c(15, 108, 189),      // #0f6cbd switch 填充
        color_neutral_foreground_inverted: c(255, 255, 255),   // #ffffff 旋钮
        color_neutral_stroke_accessible: c(97, 97, 97),        // #616161 switch 关闭描边
        color_neutral_stroke1: c(209, 209, 209),               // #d1d1d1（浅灰，不是黑！）
        color_neutral_stroke2: c(224, 224, 224),               // #e0e0e0 分隔线
        color_subtle_background_hover: c(245, 245, 245),       // #f5f5f5

        // 尺寸
        stroke_width_thin: 1.0,
        border_radius_medium: 4.0,   // ← 注意：现有 CARD_RADIUS=10 → S5 统一改 4
        border_radius_small: 2.0,

        // 字体（行高转录自 web_light：base200=16, base300=20）
        font_family_base: "Segoe UI",
        font_weight_regular: 400,
        font_weight_semibold: 600,
        font_size_base200: 12.0,
        font_size_base300: 14.0,
        font_size_base400: 16.0,
        line_height_base200: 16.0,
        line_height_base300: 20.0,

        // 间距（转录自 web_light：s=8, m=12, l=16；vertical_m=12 恰与 layout ROW_GAP=12 一致）
        spacing_horizontal_s: 8.0,
        spacing_horizontal_m: 12.0,
        spacing_horizontal_l: 16.0,
        spacing_vertical_s: 8.0,
        spacing_vertical_m: 12.0,
        spacing_vertical_l: 16.0,

        // 动效（转录自 web_light：durations 以秒计；曲线为 WAM cubic-bezier 控制点）
        duration_faster: 0.1,
        duration_normal: 0.2,
        curve_easy_ease: [0.33, 0.0, 0.67, 1.0],
        curve_decelerate_mid: [0.0, 0.0, 0.0, 1.0],
    }
}

// ── FluentPalette 深色版 ─────────────────────────────────────

/// Fluent 2.0 深色 token（Fluent Design System 标准值）。
const fn fluent_dark() -> FluentPalette {
    FluentPalette {
        // 颜色：逐值转录自 Quel'Thalas theme.rs web_dark()（唯一真值，勿凭记忆改）。
        // 注意：dark 控件底 background3 比 background1 更暗；旋钮 fg_inverted 是深灰。
        color_neutral_background1: c(41, 41, 41),            // #292929 深灰
        color_neutral_background3: c(20, 20, 20),            // #141414 控件底（比背景更暗）
        color_neutral_background6: c(51, 51, 51),            // #333333 选中片底
        color_brand_background: c(17, 94, 163),               // #115ea3（深蓝，非 Material 浅蓝）
        color_brand_background_hover: c(15, 108, 189),        // #0f6cbd
        color_brand_background_pressed: c(12, 59, 94),        // #0c3b5e
        color_compound_brand_background: c(71, 158, 245),     // #479ef5 switch 填充
        color_neutral_foreground_inverted: c(36, 36, 36),     // #242424 旋钮=深灰（非白！）
        color_neutral_stroke_accessible: c(173, 173, 173),    // #adadad switch 关闭描边
        color_neutral_stroke1: c(102, 102, 102),              // #666666（非白！）
        color_neutral_stroke2: c(82, 82, 82),                 // #525252 分隔线
        color_subtle_background_hover: c(56, 56, 56),         // #383838

        // 尺寸
        stroke_width_thin: 1.0,
        border_radius_medium: 4.0,
        border_radius_small: 2.0,

        // 字体 / 间距 / 动效：与浅色同值（web_dark 与 web_light 仅颜色不同）
        font_family_base: "Segoe UI",
        font_weight_regular: 400,
        font_weight_semibold: 600,
        font_size_base200: 12.0,
        font_size_base300: 14.0,
        font_size_base400: 16.0,
        line_height_base200: 16.0,
        line_height_base300: 20.0,

        spacing_horizontal_s: 8.0,
        spacing_horizontal_m: 12.0,
        spacing_horizontal_l: 16.0,
        spacing_vertical_s: 8.0,
        spacing_vertical_m: 12.0,
        spacing_vertical_l: 16.0,

        duration_faster: 0.1,
        duration_normal: 0.2,
        curve_easy_ease: [0.33, 0.0, 0.67, 1.0],
        curve_decelerate_mid: [0.0, 0.0, 0.0, 1.0],
    }
}

// ── 全局单例 ─────────────────────────────────────────────────

/// Fluent token 全局实例（light/dark 两套，按需切换）。
pub static FLUENT: FluentTokens = FluentTokens {
    light: fluent_light(),
    dark: fluent_dark(),
};

// ── 测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// ColorF → (r,g,b) 0-255，用于与 Quel'Thalas 源码 hex 逐值对账。
    fn rgb8(cf: ColorF) -> (u8, u8, u8) {
        let c = cf.d2d();
        (
            (c.r * 255.0).round() as u8,
            (c.g * 255.0).round() as u8,
            (c.b * 255.0).round() as u8,
        )
    }

    #[test]
    fn light_brand_is_quelthalas_0f6cbd_not_fluent1_0078d4() {
        // 真值 #0f6cbd=(15,108,189)。禁止混入 Fluent 1 的 #0078D4=(0,120,212)。
        assert_eq!(rgb8(FLUENT.light.color_brand_background), (15, 108, 189));
    }

    #[test]
    fn dark_brand_is_quelthalas_115ea3_not_material_4fc3f7() {
        // 真值 #115ea3=(17,94,163)。禁止混入 Material Design 的 #4FC3F7=(79,195,247)。
        assert_eq!(rgb8(FLUENT.dark.color_brand_background), (17, 94, 163));
        assert_eq!(rgb8(FLUENT.dark.color_compound_brand_background), (71, 158, 245));
    }

    #[test]
    fn light_stroke1_is_grey_not_black() {
        // light stroke1 = #d1d1d1。写成 #000000 会让控件画出黑框。
        assert_eq!(rgb8(FLUENT.light.color_neutral_stroke1), (209, 209, 209));
    }

    #[test]
    fn dark_knob_and_stroke_are_dark_not_white() {
        // dark 旋钮 = #242424、stroke1 = #666666。写成白色会在深色主题画出白框白旋钮。
        assert_eq!(rgb8(FLUENT.dark.color_neutral_foreground_inverted), (36, 36, 36));
        assert_eq!(rgb8(FLUENT.dark.color_neutral_stroke1), (102, 102, 102));
    }

    #[test]
    fn dark_bg3_darker_than_bg1() {
        // Fluent dark：控件底(#141414)必须比窗口底(#292929)更暗。
        let bg1 = FLUENT.dark.color_neutral_background1.d2d();
        let bg3 = FLUENT.dark.color_neutral_background3.d2d();
        assert!(bg3.r < bg1.r, "dark 控件底应比窗口底更暗");
    }

    #[test]
    fn motion_tokens_match_quelthalas() {
        // durations 以秒计：faster=0.1, normal=0.2（WAM 真值，勿写 0.05/0.167）。
        assert!((FLUENT.light.duration_faster - 0.1).abs() < 1e-6);
        assert!((FLUENT.light.duration_normal - 0.2).abs() < 1e-6);
        // curve_easy_ease = [0.33,0,0.67,1]（标准 ease-in-out）。
        assert_eq!(FLUENT.light.curve_easy_ease, [0.33, 0.0, 0.67, 1.0]);
        assert_eq!(FLUENT.light.curve_decelerate_mid, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn light_dark_colors_distinct() {
        // 同一 token 在浅色和深色下必须不同（否则映射无意义）。
        assert_ne!(FLUENT.light.color_neutral_background1,
                   FLUENT.dark.color_neutral_background1);
        assert_ne!(FLUENT.light.color_brand_background,
                   FLUENT.dark.color_brand_background);
    }

    #[test]
    fn test_brand_colors_are_blueish() {
        let b = FLUENT.light.color_brand_background.d2d();
        assert!(b.r < 0.1, "brand R should be near 0");
        assert!(b.g > 0.3 && b.g < 0.6, "brand G should be ~0.47");
        assert!(b.b > 0.7, "brand B should be near 1");
    }

    #[test]
    fn test_font_sizes_monotonic() {
        assert!(FLUENT.light.font_size_base200 < FLUENT.light.font_size_base300);
        assert!(FLUENT.light.font_size_base300 < FLUENT.light.font_size_base400);
    }

    #[test]
    fn test_curve_ease_is_valid() {
        for v in FLUENT.light.curve_easy_ease {
            assert!(v >= 0.0 && v <= 1.0, "curve_easy_ease value {v} out of range");
        }
    }

    #[test]
    fn test_duration_faster_less_than_normal() {
        assert!(FLUENT.light.duration_faster < FLUENT.light.duration_normal);
    }
}
