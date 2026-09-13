//! 自绘控件：数据（CfgRef 持有）+ D2D 绘制 + 非线性过渡。
//!
//! # 过渡（H2 扩展）
//! 每个控件有一组弹簧（hover / press / val，见 `mod.rs::Transition`）。
//! `WidgetState::anim_v` 携带 `val` 弹簧的当前值：
//!   - Toggle → 0..1（旋钮滑动 + 轨道渐变）
//!   - Segmented / Swatches → 选中下标（指示条 / 选中环滑动）
//!   - Slider → 数值（滑块滑动）
//!   - Disclosure → 0..1（箭头展开角度）
//!   因此**所有可切换控件都是「滑过去」而非「跳过去」**。

use windows::Win32::Foundation::RECT;

use aaxsyspulse_core::config::Config;
use aaxsyspulse_core::types::Lang;

use crate::gfx_d2d::{ColorF, Gfx, HAlign, RectF};
use crate::theme::Theme;

use super::cfg_ref::{CfgRef, Value};
use super::layout::s;
use super::state::{sync_via_cfg, EditState, IWidget, Widget, WidgetState};

// ── 色板常量 ─────────────────────────────────────────────

/// 背景/前景 6 预设（COLORREF 三元组）。
pub const BG_SWATCHES: [(u8, u8, u8); 6] = [
    (26, 22, 20),
    (20, 22, 27),
    (27, 26, 30),
    (42, 42, 46),
    (255, 255, 255),
    (61, 61, 66),
];
pub const FG_SWATCHES: [(u8, u8, u8); 6] = [
    (244, 241, 238),
    (238, 241, 244),
    (255, 255, 255),
    (220, 220, 220),
    (20, 20, 20),
    (30, 30, 30),
];
/// 指标名称/数值色板（8 预设 + 「跟随默认」等扩展格）。
pub const METRIC_SWATCHES: [(u8, u8, u8); 8] = [
    (255, 255, 255),
    (10, 200, 255),
    (124, 255, 120),
    (255, 215, 120),
    (255, 140, 60),
    (255, 96, 160),
    (190, 120, 255),
    (140, 255, 255),
];

// ── 控件定义 ─────────────────────────────────────────────

/// 开关。
pub struct ToggleWidget {
    pub cfg: CfgRef,
}

/// 分段选择。
pub struct SegmentedWidget {
    pub labels: Box<[String]>,
    pub cfg: CfgRef,
}

/// 按钮动作（点击时由 settings 层派发）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonAction {
    /// 重置全部指标颜色（名称色 + 数值色全部返回跟随默认）。
    ResetAllColors,
    /// 展开/收起指定指标的配置卡。
    ToggleMetricExpand(usize),
    /// 将指定指标的四项文字偏移（名称X/Y、数值X/Y）复位为 0。
    ResetMetricOffset(usize),
    /// 打开日志文件所在目录（报告 C-01：路径降级为动作，不出现在文案）。
    OpenLogFolder,
    /// 用系统默认浏览器打开外部链接（关于页「哔哩哔哩主页」）。
    /// 存 `&'static str` 以保持 `Copy`（URL 均为编译期常量）。
    OpenUrl(&'static str),
}

/// 按钮视觉级别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// 次级：设置页降噪样式（常态极淡、悬停浮起）。
    Secondary,
    /// 链接：无底色，强调色文字（「恢复跟随全局」等，报告 §6 action）。
    Link,
}

/// 按钮（绑定一个动作）。
pub struct ButtonWidget {
    pub label: String,
    pub action: ButtonAction,
    pub kind: ButtonKind,
}

/// 折叠箭头（向右 = 收起，向下 = 展开；过渡时按角度插值）。
pub struct DisclosureWidget {
    pub expanded: bool,
    pub action: ButtonAction,
}

/// 指标折叠头（个性化页）：字母徽标 + 标题 + （关闭时）状态标记。
/// 总开关已移入展开菜单（首行「显示」），因此折叠态用一枚「已关闭」弱化标签
/// 承担状态指示（开启时不画，保持折叠行安静）。
/// 整行可点击（展开/收起）；箭头是行内独立控件、不与其重叠。
/// `on` 状态在绘制时从 `Config` 实时读取（而非构建时捕获），
/// 保证配置变更后"已关闭"标签实时更新。
pub struct MetricHeaderWidget {
    pub index: usize,
    /// 构建时语言（跟随 cfg.lang，不依赖全局态时序）。
    pub lang: Lang,
}

/// 指标字号状态条（个性化页展开后位于滑杆下方）。
/// 未覆盖：左侧「跟随全局」药丸 + 说明文字。
/// 已覆盖：左侧绿色「已自定义」药丸 + 说明文字 + 右侧「恢复跟随全局」按钮。
/// 点击**右侧恢复按钮**触发 ResetFont（不再整行可点）。
pub struct MetricFontStatusWidget {
    pub index: usize,
    pub lang: Lang,
}

/// 计算字号状态条右侧「恢复跟随全局」按钮的矩形。
/// 未覆盖（`overridden=false`）时返回 None。
pub fn metric_font_status_restore_rect(
    r: RectF,
    dpi: u32,
    gfx: &mut Gfx,
    overridden: bool,
    lang: Lang,
    body_px: f32,
) -> Option<RectF> {
    if !overridden {
        return None;
    }
    let text = crate::i18n::t(lang, "ActRestoreGlobal");
    let (tw, _th) = gfx.measure_text(text, body_px, 400)?;
    let pad_x = super::layout::s(dpi, 10) as f32;
    let h = super::layout::s(dpi, 20) as f32;
    let w = tw + 2.0 * pad_x;
    let by = (r.t + r.b) / 2.0;
    let btn = RectF::new((r.r - w).max(r.l), by - h / 2.0, r.r, by + h / 2.0);
    Some(btn)
}

/// 背景/前景色板。
pub struct SwatchesWidget {
    /// 0 = 背景色板，1 = 前景色板。
    pub palette: usize,
    pub cfg: CfgRef,
}

/// 逐指标颜色色板。
/// `auto_cfg` 有值（数值色板）时末尾多一格「自动色 A」：占用率分档变色。
pub struct MetricSwatchesWidget {
    pub cfg: CfgRef,
    pub auto_cfg: Option<CfgRef>,
}

// 色板格下标：8 预设色 + （自动色 A）+ 跟随默认 ≈ + 自定义 +
pub const SW_COLORS: usize = 8;
/// 「自动色」格（仅数值色板）。
pub const SW_AUTO: usize = 8;
/// 色板总格数。
pub fn sw_n(auto: bool) -> usize {
    if auto { 8 + 3 } else { 8 + 2 }
}
/// 「跟随默认」格下标。
pub fn sw_follow(auto: bool) -> usize {
    if auto { 9 } else { 8 }
}
/// 「自定义取色」格下标。
pub fn sw_custom(auto: bool) -> usize {
    if auto { 10 } else { 9 }
}

/// 滑杆 + 数值框（绝对位置，杆位即值）。
pub struct SliderWidget {
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub dp: u32,
    /// 数值单位后缀（如 "px"）；空串 = 不显示。
    pub unit: &'static str,
    pub cfg: CfgRef,
    /// 个性化页字号滑杆专用样式：绿色填充 + 白圈滑块 + 数值纯文本（无圆角底框）。
    pub metric_style: bool,
}

/// 步进器（− / 值 / ＋；报告 §4 组件与 §11 视觉稿：条目间距、名值间距）。
pub struct StepperWidget {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub cfg: CfgRef,
}

/// 关于页头部：Logo 方块 + 应用名 + 版本（报告 §11 关于页）。
pub struct AboutHeaderWidget {
    pub lang: Lang,
}

impl IWidget for ToggleWidget {
    fn cfg(&self) -> Option<CfgRef> { Some(self.cfg) }
    fn sync(&self, cfg: &Config, st: &mut WidgetState) { sync_via_cfg(self.cfg, cfg, st) }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl IWidget for SegmentedWidget {
    fn cfg(&self) -> Option<CfgRef> { Some(self.cfg) }
    fn sync(&self, cfg: &Config, st: &mut WidgetState) { sync_via_cfg(self.cfg, cfg, st) }
    /// 指示条在分段之间滑动。
    fn anim_target(&self, st: &WidgetState) -> f32 { st.selected as f32 }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl IWidget for ButtonWidget {
    fn cfg(&self) -> Option<CfgRef> { None }
    fn sync(&self, _cfg: &Config, _st: &mut WidgetState) {}
    fn anim_target(&self, _st: &WidgetState) -> f32 { 0.0 }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl IWidget for DisclosureWidget {
    fn cfg(&self) -> Option<CfgRef> { None }
    fn sync(&self, _cfg: &Config, _st: &mut WidgetState) {}
    /// 箭头展开角度。
    fn anim_target(&self, _st: &WidgetState) -> f32 { if self.expanded { 1.0 } else { 0.0 } }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl IWidget for MetricHeaderWidget {
    fn cfg(&self) -> Option<CfgRef> { None }
    fn sync(&self, _cfg: &Config, _st: &mut WidgetState) {}
    fn anim_target(&self, _st: &WidgetState) -> f32 { 0.0 }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl IWidget for MetricFontStatusWidget {
    fn cfg(&self) -> Option<CfgRef> { None }
    fn sync(&self, _cfg: &Config, _st: &mut WidgetState) {}
    fn anim_target(&self, _st: &WidgetState) -> f32 { 0.0 }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl IWidget for SwatchesWidget {
    fn cfg(&self) -> Option<CfgRef> { Some(self.cfg) }
    fn sync(&self, cfg: &Config, st: &mut WidgetState) {
        if let Value::Color(c) = self.cfg.get(cfg) {
            let palette: &[(u8, u8, u8)] =
                if self.palette == 0 { &BG_SWATCHES } else { &FG_SWATCHES };
            st.selected = c
                .and_then(|rgb| {
                    palette
                        .iter()
                        .position(|&(r, g, b)| aaxsyspulse_core::color::Rgb::new(r, g, b) == rgb)
                })
                .unwrap_or(0);
        }
    }
    fn anim_target(&self, st: &WidgetState) -> f32 { st.selected as f32 }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl IWidget for MetricSwatchesWidget {
    fn cfg(&self) -> Option<CfgRef> { Some(self.cfg) }
    fn sync(&self, cfg: &Config, st: &mut WidgetState) {
        let has_auto = self.auto_cfg.is_some();
        // 「自动色」开启：选中 A 格（仅数值色板），优先级高于自定义色。
        if let Some(r) = self.auto_cfg {
            if matches!(r.get(cfg), Value::Bool(true)) {
                st.selected = SW_AUTO;
                st.has_sel = true;
                return;
            }
        }
        if let Value::Color(c) = self.cfg.get(cfg) {
            match c {
                Some(rgb) => {
                    st.selected = METRIC_SWATCHES
                        .iter()
                        .position(|&(r, g, b)| aaxsyspulse_core::color::Rgb::new(r, g, b) == rgb)
                        .unwrap_or(sw_custom(has_auto));
                    st.value_f = 0.0;
                }
                None => {
                    st.selected = sw_follow(has_auto);
                    st.value_f = 1.0;
                }
            }
        }
    }
    fn anim_target(&self, st: &WidgetState) -> f32 { st.selected as f32 }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl IWidget for SliderWidget {
    fn cfg(&self) -> Option<CfgRef> { Some(self.cfg) }
    fn sync(&self, cfg: &Config, st: &mut WidgetState) { sync_via_cfg(self.cfg, cfg, st) }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl IWidget for StepperWidget {
    fn cfg(&self) -> Option<CfgRef> { Some(self.cfg) }
    fn sync(&self, cfg: &Config, st: &mut WidgetState) { sync_via_cfg(self.cfg, cfg, st) }
    fn anim_target(&self, _st: &WidgetState) -> f32 { 0.0 }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl IWidget for AboutHeaderWidget {
    fn cfg(&self) -> Option<CfgRef> { None }
    fn sync(&self, _cfg: &Config, _st: &mut WidgetState) {}
    fn anim_target(&self, _st: &WidgetState) -> f32 { 0.0 }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

// ── 滑杆子几何 ───────────────────────────────────────────

/// 滑杆值框子矩形（右侧固定宽）。
pub fn value_box_rect(slider_rect: &RECT, dpi: u32) -> RECT {
    let w = super::layout::s(dpi, super::layout::VALUEBOX_W);
    RECT {
        left: slider_rect.right - w,
        top: slider_rect.top,
        right: slider_rect.right,
        bottom: slider_rect.bottom,
    }
}

/// 滑杆轨道子矩形（值框左侧）。
pub fn track_rect(slider_rect: &RECT, dpi: u32) -> RECT {
    let vb = value_box_rect(slider_rect, dpi);
    RECT {
        left: slider_rect.left + super::layout::s(dpi, 2),
        top: slider_rect.top,
        right: vb.left - super::layout::s(dpi, 6),
        bottom: slider_rect.bottom,
    }
}

/// 步进器「−」按钮子矩形（左侧；命中测试与绘制共用）。
pub fn stepper_minus_rect(stepper_rect: &RECT, dpi: u32) -> RECT {
    let w = super::layout::s(dpi, super::layout::STEPPER_BTN_W);
    RECT { left: stepper_rect.left, top: stepper_rect.top, right: stepper_rect.left + w, bottom: stepper_rect.bottom }
}

/// 步进器「＋」按钮子矩形（右侧）。
pub fn stepper_plus_rect(stepper_rect: &RECT, dpi: u32) -> RECT {
    let w = super::layout::s(dpi, super::layout::STEPPER_BTN_W);
    RECT { left: stepper_rect.right - w, top: stepper_rect.top, right: stepper_rect.right, bottom: stepper_rect.bottom }
}

// ── 滑杆值 ↔ 位置映射（纯函数） ──────────────────────────

/// 值 → 归一化位置 0..1（钳制）。
pub fn slider_t_from_value(v: f64, min: f64, max: f64) -> f32 {
    if (max - min).abs() < f64::EPSILON {
        return 0.0;
    }
    (((v - min) / (max - min)).clamp(0.0, 1.0)) as f32
}

/// 轨道内 x 坐标 → 吸附步进后的值（钳制到 min..=max）。
pub fn slider_value_from_x(x: f32, track: RectF, min: f64, max: f64, step: f64) -> f64 {
    let t = ((x - track.l) / track.w().max(1.0)).clamp(0.0, 1.0) as f64;
    let raw = min + t * (max - min);
    if step <= 0.0 {
        return raw.clamp(min, max);
    }
    let snapped = ((raw - min) / step).round() * step + min;
    snapped.clamp(min, max)
}

// ── D2D 绘制 ─────────────────────────────────────────────

fn c(_t: &Theme, v: u32) -> ColorF {
    ColorF::from_colorref(v, 1.0)
}

/// 按进度在两色间线性插值。
pub fn lerp_color(a: ColorF, b: ColorF, t: f32) -> ColorF {
    let t = t.clamp(0.0, 1.0);
    let (a, b) = (a.d2d(), b.d2d());
    ColorF::rgba(
        ((a.r + (b.r - a.r) * t).clamp(0.0, 1.0) * 255.0).round() as u8,
        ((a.g + (b.g - a.g) * t).clamp(0.0, 1.0) * 255.0).round() as u8,
        ((a.b + (b.b - a.b) * t).clamp(0.0, 1.0) * 255.0).round() as u8,
        a.a + (b.a - a.a) * t,
    )
}

fn lerp2(a: (f32, f32), b: (f32, f32), t: f32) -> (f32, f32) {
    let t = t.clamp(0.0, 1.0);
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// 控件绘制总入口（内容坐标 + 内容区原点）。
#[allow(clippy::too_many_arguments)]
pub fn paint_widget(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    w: &Widget,
    state: &WidgetState,
    dpi: u32,
    theme: &Theme,
    ox: f32,
    oy: f32,
    body_px: f32,
    small_px: f32,
    edit: Option<&EditState>,
    cfg: Option<&Config>,
) {
    let r = RectF::from_i32(w.rect.left, w.rect.top, w.rect.right, w.rect.bottom).offset(ox, oy);
    if let Some(_t) = w.kind.as_any().downcast_ref::<ToggleWidget>() {
        paint_toggle(rt, gfx, state, dpi, theme, r);
    } else if let Some(seg) = w.kind.as_any().downcast_ref::<SegmentedWidget>() {
        paint_segmented(rt, gfx, state, dpi, theme, r, &seg.labels, small_px);
    } else if let Some(btn) = w.kind.as_any().downcast_ref::<ButtonWidget>() {
        paint_button(rt, gfx, state, dpi, theme, r, &btn.label, body_px, btn.kind);
    } else if let Some(sw) = w.kind.as_any().downcast_ref::<SwatchesWidget>() {
        let palette: &[(u8, u8, u8)] = if sw.palette == 0 { &BG_SWATCHES } else { &FG_SWATCHES };
        paint_swatches(rt, gfx, state, dpi, theme, r, palette);
    } else if let Some(ms) = w.kind.as_any().downcast_ref::<MetricSwatchesWidget>() {
        paint_metric_swatches(rt, gfx, state, dpi, theme, r, small_px, ms.auto_cfg.is_some());
    } else if let Some(slider) = w.kind.as_any().downcast_ref::<SliderWidget>() {
        paint_slider(
            rt, gfx, state, dpi, theme, r, slider.dp, slider.min, slider.max, small_px,
            slider.unit, small_px, edit, slider.metric_style,
        );
    } else if let Some(_d) = w.kind.as_any().downcast_ref::<DisclosureWidget>() {
        paint_disclosure(rt, gfx, state, dpi, theme, r);
    } else if let Some(st) = w.kind.as_any().downcast_ref::<StepperWidget>() {
        paint_stepper(rt, gfx, state, dpi, theme, r, st.min, st.max);
    } else if let Some(ab) = w.kind.as_any().downcast_ref::<AboutHeaderWidget>() {
        paint_about_header(rt, gfx, dpi, theme, r, ab.lang);
    } else if let Some(mh) = w.kind.as_any().downcast_ref::<MetricHeaderWidget>() {
        paint_metric_header(rt, gfx, dpi, theme, r, mh.index, cfg, mh.lang, body_px, small_px);
    } else if let Some(ms) = w.kind.as_any().downcast_ref::<MetricFontStatusWidget>() {
        paint_metric_font_status(
            rt, gfx, state, dpi, theme, r, cfg, ms.index, ms.lang, body_px, small_px,
        );
    }

    // 键盘焦点环（报告 §7.1：仅键盘导航时出现，鼠标用户看不到）。
    if state.focused {
        let g = super::layout::s(dpi, 2) as f32;
        let fr = RectF::new(r.l - g, r.t - g, r.r + g, r.b + g);
        gfx.stroke_round(rt, fr, super::layout::s(dpi, 7) as f32, c(theme, theme.accent), 2.0);
    }
}

fn paint_toggle(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    state: &WidgetState,
    dpi: u32,
    t: &Theme,
    r: RectF,
) {
    // Fluent 2 switch（Quel'Thalas switch.rs 拆壳移植，S2 试点）：
    // 轨道 40×20 胶囊（layout 预留 TOGGLE_W=40），旋钮 ⌀14 = 20 − 2×(描边1+间隙2)。
    // 关 = 无填充 + accessible 描边；开 = compound brand 填充淡入、描边淡出；
    // 旋钮色从描边色渐变到 inverted（dark 主题是深灰 #242424，不是白）。
    // 悬停/按压端点色按 Fluent 语义离散切换（hover_t/press_t 弹簧收敛到 0/1 后判定），
    // 位置与交叉渐变仍由 `anim_v` 弹簧连续驱动 —— 单一弹簧语言不变。
    let ft = if t.dark {
        &super::fluent::tokens::FLUENT.dark
    } else {
        &super::fluent::tokens::FLUENT.light
    };
    let tw = super::layout::s(dpi, 40) as f32;
    let th = super::layout::s(dpi, 20) as f32;
    let r = RectF::new(r.r - tw, (r.t + r.b - th) / 2.0, r.r, (r.t + r.b + th) / 2.0);
    let k = state.anim_v.clamp(0.0, 1.0);
    let hovered = state.hover_t > 0.5;
    let pressed = state.press_t > 0.5;
    let stroke_off = if pressed {
        ft.color_neutral_stroke_accessible_pressed
    } else if hovered {
        ft.color_neutral_stroke_accessible_hover
    } else {
        ft.color_neutral_stroke_accessible
    };
    let fill_on = if pressed {
        ft.color_compound_brand_background_pressed
    } else if hovered {
        ft.color_compound_brand_background_hover
    } else {
        ft.color_compound_brand_background
    };
    let radius = th / 2.0;
    // 轨道填充：开态淡入（关态无填充）。
    gfx.fill_round(rt, r, radius, fill_on.with_alpha(k));
    // 描边：关态可见，随开态淡出。
    gfx.stroke_round(rt, r, radius, stroke_off.with_alpha(1.0 - k), 1.0);
    // 旋钮：两端圆心之间滑动，颜色从描边色渐变到 inverted。
    let thumb_d = th - 2.0 * (super::layout::s(dpi, 1) + super::layout::s(dpi, 2)) as f32;
    let off_x = r.l + radius;
    let on_x = r.l + tw - radius;
    let thumb_x = off_x + (on_x - off_x) * k;
    let thumb_color = lerp_color(stroke_off, ft.color_neutral_foreground_inverted, k);
    let cy = (r.t + r.b) / 2.0;
    gfx.fill_ellipse(
        rt,
        RectF::new(thumb_x - thumb_d / 2.0, cy - thumb_d / 2.0, thumb_x + thumb_d / 2.0, cy + thumb_d / 2.0),
        thumb_color,
    );
}

fn paint_segmented(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    state: &WidgetState,
    dpi: u32,
    t: &Theme,
    r: RectF,
    labels: &[String],
    px: f32,
) {
    let ft = if t.dark {
        &super::fluent::tokens::FLUENT.dark
    } else {
        &super::fluent::tokens::FLUENT.light
    };
    let n = labels.len().max(1);
    let seg_w = r.w() / n as f32;
    let m = super::layout::s(dpi, 2) as f32;
    let hovered = state.hover_t > 0.5;
    // 底=color_neutral_background3，悬停加一层 subtle hover
    gfx.fill_round(rt, r, super::layout::s(dpi, 4) as f32, ft.color_neutral_background3);
    if hovered {
        gfx.fill_round(rt, r, super::layout::s(dpi, 4) as f32, ft.color_subtle_background_hover);
    }
    // 选中片沿「动画下标」滑动（而非逐格跳变）；无选中态（未贴角）不画胶囊。
    let sel = state.anim_v.clamp(0.0, (n - 1) as f32);
    let active = (sel.round() as usize).min(n - 1);
    if state.has_sel {
        let ir = RectF::new(
            r.l + sel * seg_w + m,
            r.t + m,
            r.l + (sel + 1.0) * seg_w - m,
            r.b - m,
        );
        let sel_fill = if t.dark { ft.color_neutral_background6 } else { ft.color_neutral_background1 };
        gfx.fill_round(rt, ir, super::layout::s(dpi, 3) as f32, sel_fill);
        gfx.stroke_round(rt, ir, super::layout::s(dpi, 3) as f32, ft.color_neutral_stroke1, 1.0);
    }
    for (i, label) in labels.iter().enumerate() {
        let seg = RectF::new(r.l + i as f32 * seg_w, r.t, r.l + (i + 1) as f32 * seg_w, r.b);
        let is_active = state.has_sel && i == active;
        let color = if is_active { ft.color_neutral_foreground1 } else { ft.color_neutral_foreground2 };
        // 轴对齐裁剪：英文标签比中文宽，溢出不再压入相邻分段。
        unsafe {
            rt.PushAxisAlignedClip(&seg.d2d(), windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
        gfx.text(rt, label, px, 400, HAlign::Center, seg, color, true);
        unsafe { rt.PopAxisAlignedClip() };
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_button(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    state: &WidgetState,
    dpi: u32,
    t: &Theme,
    r: RectF,
    label: &str,
    px: f32,
    kind: ButtonKind,
) {
    // 链接样式：无底色，强调色文字（报告 §6 action：恢复跟随全局等）。
    if kind == ButtonKind::Link {
        let color = if state.hovered { c(t, t.accent_hover) } else { c(t, t.accent) };
        gfx.text(rt, label, px, 400, HAlign::Center, r, color, true);
        return;
    }
    // 次级按钮：Fluent 离散端点色（hover/pressed 各一套 token）。
    let ft = if t.dark {
        &super::fluent::tokens::FLUENT.dark
    } else {
        &super::fluent::tokens::FLUENT.light
    };
    let hovered = state.hover_t > 0.5;
    let pressed = state.press_t > 0.5;
    let fill = if pressed { ft.color_neutral_background1_pressed }
        else if hovered { ft.color_neutral_background1_hover }
        else { ft.color_neutral_background1 };
    let stroke = if pressed { ft.color_neutral_stroke1_pressed }
        else if hovered { ft.color_neutral_stroke1_hover }
        else { ft.color_neutral_stroke1 };
    let text = ft.color_neutral_foreground1;
    let sc = 1.0 - 0.02 * state.press_t;
    let cx = (r.l + r.r) / 2.0;
    let cy = (r.t + r.b) / 2.0;
    let rr = RectF::new(
        cx - r.w() / 2.0 * sc,
        cy - r.h() / 2.0 * sc,
        cx + r.w() / 2.0 * sc,
        cy + r.h() / 2.0 * sc,
    );
    gfx.fill_round(rt, rr, super::layout::s(dpi, 4) as f32, fill);
    gfx.stroke_round(rt, rr, super::layout::s(dpi, 4) as f32, stroke, 1.0);
    // 轴对齐裁剪：英文按钮文字溢出不再画出按钮边界（EN 本地化保护）。
    unsafe {
        rt.PushAxisAlignedClip(&rr.d2d(), windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
    }
    gfx.text(rt, label, px, 400, HAlign::Center, rr, text, true);
    unsafe { rt.PopAxisAlignedClip() };
}

/// 步进器（报告 §11 视觉稿：`[−] 值 [＋]`；条目间距 / 名值间距用）。
/// 三段等高：−按钮 | 值 | ＋按钮，整体一个圆角容器。
fn paint_stepper(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    state: &WidgetState,
    dpi: u32,
    t: &Theme,
    r: RectF,
    min: f64,
    max: f64,
) {
    let ft = if t.dark {
        &super::fluent::tokens::FLUENT.dark
    } else {
        &super::fluent::tokens::FLUENT.light
    };
    let hovered = state.hover_t > 0.5;
    let pressed = state.press_t > 0.5;
    let vbh = super::layout::s(dpi, 24) as f32;
    let box_r = RectF::new(r.l, r.t + (r.h() - vbh) / 2.0, r.r, r.t + (r.h() + vbh) / 2.0);
    // 容器：填充=neutral_background1（悬停替换为 subtle hover），边框=stroke1，圆角 s(dpi,4)。
    // 层序：先填充后描边——悬停填充若画在描边之后会把不透明底压在边框上。
    gfx.fill_round(rt, box_r, super::layout::s(dpi, 4) as f32, ft.color_neutral_background1);
    if hovered {
        gfx.fill_round(rt, box_r, super::layout::s(dpi, 4) as f32, ft.color_subtle_background_hover);
    }
    gfx.stroke_round(rt, box_r, super::layout::s(dpi, 4) as f32, ft.color_neutral_stroke1, 1.0);

    let minus = RectF::from_win32(stepper_minus_rect(
        &windows::Win32::Foundation::RECT {
            left: box_r.l as i32, top: box_r.t as i32,
            right: box_r.r as i32, bottom: box_r.b as i32,
        },
        dpi,
    ));
    let plus = RectF::from_win32(stepper_plus_rect(
        &windows::Win32::Foundation::RECT {
            left: box_r.l as i32, top: box_r.t as i32,
            right: box_r.r as i32, bottom: box_r.b as i32,
        },
        dpi,
    ));
    // 按钮符号颜色：hover/pressed 各一套端点色，触底/触顶=disabled
    let val = state.value_f;
    let glyph_color = |at_limit: bool| -> ColorF {
        if at_limit { ft.color_neutral_foreground_disabled }
        else if pressed { ft.color_neutral_foreground3_pressed }
        else if hovered { ft.color_neutral_foreground3_hover }
        else { ft.color_neutral_foreground3 }
    };
    let cm = glyph_color(val <= min + 1e-6);
    let cp = glyph_color(val >= max - 1e-6);
    let k = super::layout::s(dpi, 5) as f32;
    let my = (minus.t + minus.b) / 2.0;
    let mx = (minus.l + minus.r) / 2.0;
    gfx.line(rt, mx - k, my, mx + k, my, cm, 1.5);
    let py_ = (plus.t + plus.b) / 2.0;
    let px_ = (plus.l + plus.r) / 2.0;
    gfx.line(rt, px_ - k, py_, px_ + k, py_, cp, 1.5);
    gfx.line(rt, px_, py_ - k, px_, py_ + k, cp, 1.5);

    // 值（中间，居中；整数直出，C-02：无 "40.0" 式浮点直出）。
    let value_r = RectF::new(minus.r, box_r.t, plus.l, box_r.b);
    let show = format!("{}", val.round() as i64);
    gfx.text(rt, &show, super::layout::s(dpi, 12) as f32, 500, HAlign::Center, value_r, ft.color_neutral_foreground1, true);
}

/// 色块选中态双层环（React Aria 规范，报告 C03）：内白边 + 外深环
/// （outline-offset:-4px）。白边保证在深色/彩色块上可见，深环保证在浅色块上可见，
/// 因此无论色块本身深浅都清晰可辨。
fn swatch_selected_ring(
    gfx: &mut Gfx,
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    dpi: u32,
    cell: RectF,
    radius: f32,
) {
    let white = ColorF::rgba(255, 255, 255, 1.0);
    let dark = ColorF::rgba(15, 23, 42, 1.0);
    let m1 = super::layout::s(dpi, 1) as f32;
    gfx.stroke_round(
        rt,
        RectF::new(cell.l + m1, cell.t + m1, cell.r - m1, cell.b - m1),
        radius,
        white,
        2.0,
    );
    let m2 = super::layout::s(dpi, 4) as f32;
    gfx.stroke_round(
        rt,
        RectF::new(cell.l + m2, cell.t + m2, cell.r - m2, cell.b - m2),
        radius,
        dark,
        2.0,
    );
}

fn paint_swatches(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    state: &WidgetState,
    dpi: u32,
    t: &Theme,
    r: RectF,
    palette: &[(u8, u8, u8)],
) {
    let ft = if t.dark {
        &super::fluent::tokens::FLUENT.dark
    } else {
        &super::fluent::tokens::FLUENT.light
    };
    let n = palette.len().max(1);
    let wd = r.w() / n as f32;
    let m = super::layout::s(dpi, 3) as f32;
    for (i, &(cr, cg, cb)) in palette.iter().enumerate() {
        let sw = RectF::new(r.l + i as f32 * wd, r.t, r.l + (i + 1) as f32 * wd, r.b);
        let inner = RectF::new(sw.l + m, sw.t + m, sw.r - m, sw.b - m);
        gfx.fill_round(rt, inner, super::layout::s(dpi, 4) as f32, ColorF::rgba(cr, cg, cb, 1.0));
        gfx.stroke_round(rt, inner, super::layout::s(dpi, 4) as f32, ft.color_neutral_stroke2, 1.0);
    }
    // 选中态：双层环（内白边 + 外深环，报告 C03），沿动画下标滑动。
    let sel = state.anim_v.clamp(0.0, (n - 1) as f32);
    let cell = RectF::new(r.l + sel * wd, r.t, r.l + (sel + 1.0) * wd, r.b);
    let inner_sel = RectF::new(cell.l + m, cell.t + m, cell.r - m, cell.b - m);
    swatch_selected_ring(gfx, rt, dpi, inner_sel, super::layout::s(dpi, 4) as f32);
}

fn paint_metric_swatches(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    state: &WidgetState,
    dpi: u32,
    t: &Theme,
    r: RectF,
    px: f32,
    auto: bool,
) {
    let ft = if t.dark {
        &super::fluent::tokens::FLUENT.dark
    } else {
        &super::fluent::tokens::FLUENT.light
    };
    let n = sw_n(auto);
    let follow_i = sw_follow(auto);
    let custom_i = sw_custom(auto);
    let wd = r.w() / n as f32;
    let m = super::layout::s(dpi, 3) as f32;
    for i in 0..n {
        let sw = RectF::new(r.l + i as f32 * wd, r.t, r.l + (i + 1) as f32 * wd, r.b);
        let inner = RectF::new(sw.l + m, sw.t + m, sw.r - m, sw.b - m);
        if i < SW_COLORS {
            let (cr, cg, cb) = METRIC_SWATCHES[i];
            gfx.fill_round(rt, inner, super::layout::s(dpi, 4) as f32, ColorF::rgba(cr, cg, cb, 1.0));
            gfx.stroke_round(rt, inner, super::layout::s(dpi, 4) as f32, ft.color_neutral_stroke2, 1.0);
        } else if auto && i == SW_AUTO {
            // 自动色：中性底框 + 「A」（占用率分档变色：<60 绿 / 60~85 橙 / ≥85 红）。
            gfx.fill_round(rt, inner, super::layout::s(dpi, 4) as f32, c(t, t.card));
            gfx.stroke_round(rt, inner, super::layout::s(dpi, 4) as f32, c(t, t.secondary), 1.0);
            gfx.text(rt, "A", px, 600, HAlign::Center, inner, c(t, t.secondary), true);
        } else if i == follow_i {
            gfx.fill_round(rt, inner, super::layout::s(dpi, 4) as f32, c(t, t.card));
            gfx.stroke_round(rt, inner, super::layout::s(dpi, 4) as f32, c(t, t.secondary), 1.0);
            gfx.text(rt, "≈", px, 400, HAlign::Center, inner, c(t, t.secondary), true);
        } else if i == custom_i {
            // 自定义入口：虚线框 + 「+」。
            gfx.fill_round(rt, inner, super::layout::s(dpi, 4) as f32, c(t, t.card).with_alpha(0.3));
            gfx.stroke_round(rt, inner, super::layout::s(dpi, 4) as f32, c(t, t.secondary), 1.0);
            gfx.text(rt, "+", px, 400, HAlign::Center, inner, c(t, t.secondary), true);
        }
    }
    // 选中态：双层环（报告 C03），含末尾「跟随默认」格。
    let sel = state.anim_v.clamp(0.0, (n - 1) as f32);
    let cell = RectF::new(r.l + sel * wd, r.t, r.l + (sel + 1.0) * wd, r.b);
    let m3 = super::layout::s(dpi, 3) as f32;
    let inner_sel = RectF::new(cell.l + m3, cell.t + m3, cell.r - m3, cell.b - m3);
    swatch_selected_ring(gfx, rt, dpi, inner_sel, super::layout::s(dpi, 4) as f32);
}

#[allow(clippy::too_many_arguments)]
fn paint_slider(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    state: &WidgetState,
    dpi: u32,
    t: &Theme,
    r: RectF,
    dp: u32,
    min: f32,
    max: f32,
    px: f32,
    unit: &'static str,
    small_px: f32,
    edit: Option<&EditState>,
    metric_style: bool,
) {
    // 文本显示真值；滑块/填充用动画值（跳变与重置时滑过去）。
    let val = state.value_f;
    let anim = if state.anim_v == 0.0 && val != 0.0 { val as f32 } else { state.anim_v };
    let editing = state.editing;
    let base = RectF::new(r.l, r.t, r.r, r.b);
    // 行矩形可能加高（容纳端点标注）：轨道固定贴行顶，值框高度与行高解耦。
    let vb = RectF::from_win32(value_box_rect(
        &windows::Win32::Foundation::RECT {
            left: base.l as i32,
            top: base.t as i32,
            right: base.r as i32,
            bottom: base.b as i32,
        },
        dpi,
    ));
    let tr = RectF::from_win32(track_rect(
        &windows::Win32::Foundation::RECT {
            left: base.l as i32,
            top: base.t as i32,
            right: base.r as i32,
            bottom: base.b as i32,
        },
        dpi,
    ));

    // 控件线 = 首 28 行（标签行）的垂直中心 —— 与左侧标签同一基线。
    // 用户反馈：数值框原来按整行矩形居中，滑杆行 46 时比标签低 16px，看着不齐。
    let ctrl_cy = r.t + super::layout::s(dpi, super::layout::ROW_H) as f32 / 2.0;
    let track_y = ctrl_cy;
    // Fluent slider 几何（slider.rs 拆壳移植，S3）：轨道 4px 圆角胶囊，
    // 旋钮 ⌀20 = 外圈 1px + 白盘 ⌀18 + 品牌内点 ⌀12；悬停/按压切 compound 端点色。
    let ft = if t.dark {
        &super::fluent::tokens::FLUENT.dark
    } else {
        &super::fluent::tokens::FLUENT.light
    };
    let hovered = state.hover_t > 0.5;
    let pressed = state.press_t > 0.5;
    let brand = if pressed {
        ft.color_compound_brand_background_pressed
    } else if hovered {
        ft.color_compound_brand_background_hover
    } else {
        ft.color_compound_brand_background
    };
    let rail_h = super::layout::s(dpi, 4).max(1) as f32;
    let rail_r = rail_h / 2.0;
    let rail_fill = if metric_style { c(t, t.segmented_bg) } else { ft.color_neutral_stroke_accessible };
    gfx.fill_round(rt, RectF::new(tr.l, track_y - rail_r, tr.r, track_y + rail_r), rail_r, rail_fill);
    let frac = slider_t_from_value(anim as f64, min as f64, max as f64);
    let cx = (tr.l + frac * tr.w()).clamp(tr.l, tr.r);
    let fill_color = if metric_style { c(t, t.toggle_on) } else { brand };
    if cx > tr.l + 0.5 {
        gfx.fill_round(rt, RectF::new(tr.l, track_y - rail_r, cx, track_y + rail_r), rail_r, fill_color);
    }
    // 旋钮三层圆盘（Gfx 无描边椭圆，外圈用大盘压出细环）。
    let thumb_d = super::layout::s(dpi, 20) as f32;
    let dot_d = super::layout::s(dpi, 12) as f32;
    let mid_d = thumb_d - 2.0 * super::layout::s(dpi, 1) as f32;
    let disc = |d: f32| RectF::new(cx - d / 2.0, track_y - d / 2.0, cx + d / 2.0, track_y + d / 2.0);
    if metric_style {
        // 个性化页「白圈滑块」（用户反馈样式）：白盘 + 绿内点，无灰环。
        gfx.fill_ellipse(rt, disc(thumb_d), c(t, t.knob));
        gfx.fill_ellipse(rt, disc(dot_d), c(t, t.toggle_on));
    } else {
        gfx.fill_ellipse(rt, disc(thumb_d), ft.color_neutral_stroke1);
        gfx.fill_ellipse(rt, disc(mid_d), ft.color_neutral_background1);
        gfx.fill_ellipse(rt, disc(dot_d), brand);
    }

    // 端点标注（报告 B-05：每个滑块必须有可见的 min / max 端点）。
    // 画在**数值框下方**（仍在滑杆行高内）；行内 hint 从行高之后起画，
    // 因此不再被下方说明文字压住（用户反馈：12 / 72 被下边的字遮挡）。
    let vbh = super::layout::s(dpi, super::layout::VALUEBOX_H) as f32;
    let ep_t = ctrl_cy + vbh / 2.0 + super::layout::s(dpi, 2) as f32;
    let ep_b = ep_t + small_px * 1.15;
    let min_s = format!("{}", min.round() as i64);
    let max_s = format!("{}", max.round() as i64);
    gfx.text(rt, &min_s, small_px, 400, HAlign::Leading,
             RectF::new(tr.l, ep_t, tr.l + super::layout::s(dpi, 40) as f32, ep_b), c(t, t.secondary), true);
    gfx.text(rt, &max_s, small_px, 400, HAlign::Trailing,
             RectF::new(tr.r - super::layout::s(dpi, 40) as f32, ep_t, tr.r, ep_b), c(t, t.secondary), true);

    // 数值显示：普通滑杆带圆角底框；个性化页字号滑杆用纯文本（理想布局）。
    // 纵向中心 = 控件线（ctrl_cy），与左侧标签同一基线。
    let vbr = RectF::new(vb.l, ctrl_cy - vbh / 2.0, vb.r, ctrl_cy + vbh / 2.0);
    let content = if metric_style {
        // 纯文本右对齐，右侧留少量内边距。
        RectF::new(vb.l, vbr.t, vb.r - super::layout::s(dpi, 4) as f32, vbr.b)
    } else {
        let fill = if editing { c(t, t.button_bg_hover) } else { c(t, t.button_bg) };
        gfx.fill_round(rt, vbr, super::layout::s(dpi, 6) as f32, fill);
        RectF::new(vbr.l, vbr.t, vbr.r - super::layout::s(dpi, 8) as f32, vbr.b)
    };

    let mut show: String = if editing {
        edit
            .map(|e| {
                let mut s: String = e.buf.iter().map(|&c| char::from_u32(c as u32).unwrap_or('\u{FFFD}')).collect();
                if e.caret {
                    s.push('_');
                }
                s
            })
            .unwrap_or_default()
    } else if dp == 0 {
        format!("{}", val.round() as i64)
    } else {
        format!("{val:.1}")
    };
    // 单位后缀（C01：40.0 → 40 px）。
    if !unit.is_empty() && !editing {
        show.push(' ');
        show.push_str(unit);
    }
    gfx.text(rt, &show, px, 400, HAlign::Trailing, content, c(t, t.label), true);
}

/// 折叠箭头：0 = 朝右，1 = 朝下；两端点按 `anim_v` 插值（箭头「翻开」）。
fn paint_disclosure(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    state: &WidgetState,
    dpi: u32,
    t: &Theme,
    r: RectF,
) {
    // 圆形按钮（用户反馈：展开/收起应为圆形按钮，符号居中）。
    // 矩形纵跨整行（tall）：半径按短边（= ICON_BTN）取，圆心即行中心。
    let cx = (r.l + r.r) / 2.0;
    let cy = (r.t + r.b) / 2.0;
    let rad = (r.w().min(r.h())) / 2.0 - super::layout::s(dpi, 3) as f32;
    let bg = lerp_color(c(t, t.button_bg), c(t, t.button_bg_hover), state.hover_t);
    let er = RectF::new(cx - rad, cy - rad, cx + rad, cy + rad);
    gfx.fill_ellipse(rt, er, bg);
    let color = if state.hovered { c(t, t.label) } else { c(t, t.secondary) };
    let k = super::layout::s(dpi, 5) as f32;
    let st = super::layout::s(dpi, 1).max(1) as f32 * 1.5;
    let a = state.anim_v.clamp(0.0, 1.0);
    // 朝右：(-0.5k,-k) (0.5k,0) (-0.5k,k)；朝下：(-k,-0.5k) (0,0.5k) (k,-0.5k)
    let p1 = lerp2((-k * 0.5, -k), (-k, -k * 0.5), a);
    let p2 = lerp2((k * 0.5, 0.0), (0.0, k * 0.5), a);
    let p3 = lerp2((-k * 0.5, k), (k, -k * 0.5), a);
    gfx.line(rt, cx + p1.0, cy + p1.1, cx + p2.0, cy + p2.1, color, st);
    gfx.line(rt, cx + p3.0, cy + p3.1, cx + p2.0, cy + p2.1, color, st);
}

/// 字号状态条（理想布局）：字号滑杆下方的一行。
///   - 未覆盖：「跟随全局」药丸（右侧「恢复跟随全局」按钮）。
///   - 已覆盖：「已自定义」标签（右侧「恢复跟随全局」按钮）。
///   整行可点（命中右侧链接区域即重置）。
#[allow(clippy::too_many_arguments)]
fn paint_metric_font_status(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    state: &WidgetState,
    dpi: u32,
    t: &Theme,
    r: RectF,
    cfg: Option<&Config>,
    index: usize,
    lang: Lang,
    body_px: f32,
    caption_px: f32,
) {
    let overridden = cfg
        .map(|c| c.metric_font_sizes.get(index).copied().flatten().is_some())
        .unwrap_or(false);
    let by = (r.t + r.b) / 2.0;
    let gap = super::layout::s(dpi, 10) as f32;

    if overridden {
        // 左侧「已自定义」药丸（按钮外观）。
        let tag_text = crate::i18n::t(lang, "MtrCustomized");
        let tag_w = gfx
            .measure_text(tag_text, caption_px, 600)
            .map(|(w, _)| w + 2.0 * gap)
            .unwrap_or_else(|| super::layout::s(dpi, 56) as f32);
        let tag_h = super::layout::s(dpi, 18) as f32;
        let tag_r = RectF::new(r.l, by - tag_h / 2.0, r.l + tag_w, by + tag_h / 2.0);
        let green = c(t, t.toggle_on);
        gfx.fill_round(rt, tag_r, tag_h / 2.0, green.with_alpha(0.16));
        gfx.text(rt, tag_text, caption_px, 600, HAlign::Center, tag_r, green, true);

        // 右侧「恢复跟随全局」按钮（实测宽 + 内边距，hover 高亮）。
        if let Some(btn) = metric_font_status_restore_rect(r, dpi, gfx, true, lang, body_px) {
            if state.hover_t > 0.01 {
                gfx.fill_round(rt, btn, btn.h() / 2.0, c(t, t.accent).with_alpha(0.10 * state.hover_t));
            }
            gfx.text(
                rt, crate::i18n::t(lang, "ActRestoreGlobal"), body_px, 400,
                HAlign::Center, btn, c(t, t.accent), true,
            );
        }
    } else {
        // 未覆盖：灰色「跟随全局」药丸 + 说明。
        let tag_text = crate::i18n::t(lang, "MtrFollow");
        let tag_w = gfx
            .measure_text(tag_text, caption_px, 600)
            .map(|(w, _)| w + 2.0 * gap)
            .unwrap_or_else(|| super::layout::s(dpi, 56) as f32);
        let tag_h = super::layout::s(dpi, 18) as f32;
        let tag_r = RectF::new(r.l, by - tag_h / 2.0, r.l + tag_w, by + tag_h / 2.0);
        gfx.fill_round(rt, tag_r, tag_h / 2.0, c(t, t.secondary).with_alpha(0.12));
        gfx.text(rt, tag_text, caption_px, 600, HAlign::Center, tag_r, c(t, t.secondary), true);
    }
}

/// 指标折叠头：抓手点阵 + 标题 + （关闭时）「已关闭」标记（报告 L03/L08）。
///   字母徽标已移除（用户反馈无信息量）；总开关在展开菜单首行（「显示」），
///   因此这里仅在关闭态补一枚弱化标签，避免折叠列表出现「看不出已关」的死角。
///   硬约束（B-07）：折叠头不含任何可调参数控件，仅抓手 + 名称 + 状态 + 箭头。
///   `on` 状态在绘制时从 `Config` 实时读取，保证配置变更后"已关闭"标签实时更新。
#[allow(clippy::too_many_arguments)]
fn paint_metric_header(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    dpi: u32,
    t: &Theme,
    r: RectF,
    index: usize,
    cfg: Option<&Config>,
    lang: Lang,
    body_px: f32,
    caption_px: f32,
) {
    // 从实时配置中读取指标开关状态（而非依赖构建时捕获的值）。
    // metric_cfg 顺序：0=CPU, 1=Mem, 2=GPU, 3=Net, 4=Power, 5=Time
    let on = cfg.map_or(false, |c| {
        let cfg_ref = match index {
            0 => super::cfg_ref::CfgRef::ShowCpu,
            1 => super::cfg_ref::CfgRef::ShowMem,
            2 => super::cfg_ref::CfgRef::ShowGpu,
            3 => super::cfg_ref::CfgRef::ShowNet,
            4 => super::cfg_ref::CfgRef::PowerEnabled,
            5 => super::cfg_ref::CfgRef::ShowTime,
            _ => return false,
        };
        matches!(cfg_ref.get(c), super::cfg_ref::Value::Bool(true))
    });
    const KEYS: [&str; 6] = ["LblCpu", "LblMem", "LblGpu", "LblNet", "LblPower", "LblTime"];
    let name = crate::i18n::t(lang, KEYS[index.min(5)]);
    // 折叠头标题比正文大一档（用户反馈：字号偏小）。
    let title_px = body_px * 1.2;

    // 拖动抓手点阵（2×3）：矩形纵跨整行（tall）→ gy 即行几何中心，上下居中；
    // 左侧内缩 + 与标题保持间距，左右都有呼吸感，不贴卡片边缘。
    let gy = (r.t + r.b) / 2.0;
    let dot = s(dpi, 2) as f32;
    let gap = s(dpi, 5) as f32;
    let gx0 = r.l + s(dpi, 6) as f32;
    let grip_col = c(t, t.secondary).with_alpha(0.6);
    for row in -1..=1i32 {
        for col in 0..2i32 {
            let cx = gx0 + col as f32 * gap;
            let cy = gy + row as f32 * gap;
            gfx.fill_ellipse(rt, RectF::new(cx - dot, cy - dot, cx + dot, cy + dot), grip_col);
        }
    }

    // 标题（600 字重）：紧接抓手之后，与抓手留 16 呼吸间距；关闭态降为次级色。
    let title_l = gx0 + gap + dot + s(dpi, 16) as f32;
    let tr = RectF::new(title_l, r.t, r.r, r.b);
    let title_col = if on { c(t, t.label) } else { c(t, t.secondary) };
    let mw = gfx.text(rt, name, title_px, 600, HAlign::Leading, tr, title_col, true);

    // 关闭态：名称右侧补一枚「已关闭」（开启时不画，保持折叠列表安静）。
    if !on {
        let tag_l = title_l + mw.map(|(w, _)| w).unwrap_or(0.0) + s(dpi, 12) as f32;
        if tag_l < r.r {
            let tag_r = RectF::new(tag_l, r.t, r.r, r.b);
            gfx.text(
                rt,
                crate::i18n::t(lang, "MtrOff"),
                caption_px,
                400,
                HAlign::Leading,
                tag_r,
                c(t, t.secondary),
                true,
            );
        }
    }
}

/// 关于页头部：应用图标 + 应用名 + 版本（报告 §11 关于页布局）。
fn paint_about_header(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    dpi: u32,
    t: &Theme,
    r: RectF,
    lang: Lang,
) {
    let side = super::layout::s(dpi, 32) as f32;
    let cy = (r.t + r.b) / 2.0;
    let logo = RectF::new(r.l, cy - side / 2.0, r.l + side, cy + side / 2.0);
    // 应用图标：内嵌 ICO 的 64×64 帧，经 WIC 解码成 D2D 位图后按 logo 区域缩放绘制。
    // 解码失败（WIC 不可用等）时退回圆角占位块，保证关于页始终有内容。
    match crate::app_icon::d2d_bitmap(rt) {
        Some(bmp) => {
            use windows::Win32::Graphics::Direct2D::D2D1_BITMAP_INTERPOLATION_MODE_LINEAR;
            let dst = logo.d2d();
            unsafe {
                rt.DrawBitmap(&bmp, Some(&dst), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            }
        }
        // 兜底只画一圈淡化描边 —— 绝不再填纯色块：
        // 深色主题下 label 为白，填色块会被看成「纯白图片」。
        None => {
            gfx.stroke_round(rt, logo, super::layout::s(dpi, 6) as f32, c(t, t.secondary), 1.0);
        }
    }
    // 应用名 + 版本（版本次级色，紧跟名称）。
    let name = crate::i18n::t(lang, "AboutName");
    let name_r = RectF::new(logo.r + super::layout::s(dpi, 10) as f32, r.t, r.r, r.b);
    let mw = gfx.text(rt, name, body_px(dpi), 600, HAlign::Leading, name_r, c(t, t.label), true);
    if let Some((w, _)) = mw {
        let ver_r = RectF::new(
            name_r.l + w + super::layout::s(dpi, 10) as f32,
            r.t,
            r.r,
            r.b,
        );
        gfx.text(rt, crate::i18n::t(lang, "AboutVer"), caption_px(dpi), 400, HAlign::Leading, ver_r, c(t, t.secondary), true);
    }
}

/// 字号取 `layout` 的单一来源（避免此处再抄一份 13/11）。
fn body_px(dpi: u32) -> f32 {
    super::layout::body_px(dpi)
}

fn caption_px(dpi: u32) -> f32 {
    super::layout::caption_px(dpi)
}

#[cfg(test)]
mod slider_tests {
    use super::*;

    #[test]
    fn t_maps_value_range_to_unit() {
        assert_eq!(slider_t_from_value(8.0, 8.0, 256.0), 0.0);
        assert_eq!(slider_t_from_value(256.0, 8.0, 256.0), 1.0);
        assert!((slider_t_from_value(132.0, 8.0, 256.0) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn t_clamps_out_of_range() {
        assert_eq!(slider_t_from_value(-100.0, 8.0, 256.0), 0.0);
        assert_eq!(slider_t_from_value(9999.0, 8.0, 256.0), 1.0);
    }

    #[test]
    fn value_from_x_snaps_to_step_and_clamps() {
        let track = RectF::new(100.0, 0.0, 300.0, 24.0);
        assert!((slider_value_from_x(200.0, track, 8.0, 256.0, 0.5) - 132.0).abs() < 1e-3);
        assert_eq!(slider_value_from_x(0.0, track, 8.0, 256.0, 0.5), 8.0);
        assert_eq!(slider_value_from_x(9999.0, track, 8.0, 256.0, 0.5), 256.0);
        let v = slider_value_from_x(201.0, track, 8.0, 256.0, 0.5);
        assert!((v * 2.0).fract().abs() < 1e-6, "必须吸附到 0.5 步进，实际 {v}");
    }

    #[test]
    fn lerp_color_midpoint_and_clamp() {
        let a = ColorF::rgba(0, 0, 0, 1.0);
        let b = ColorF::rgba(255, 255, 255, 1.0);
        let m = lerp_color(a, b, 0.5).d2d();
        assert!((m.r - 0.5).abs() < 0.01 && (m.g - 0.5).abs() < 0.01);
        assert_eq!(lerp_color(a, b, -1.0).d2d().r, 0.0);
        assert_eq!(lerp_color(a, b, 2.0).d2d().r, 1.0);
    }
}
