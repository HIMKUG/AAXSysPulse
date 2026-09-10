//! 六种自绘控件：数据（CfgRef 持有）+ D2D 绘制。
//!
//! # 迁移保真说明
//!
//! 视觉规格逐像素对齐旧 GDI 版（圆角半径/内缩/色值全部沿用），仅两处**有意**的修正：
//! 1. MetricSwatches 选中态旧实现会先用卡片色**填充**外框再描边（把选中格的
//!    颜色盖掉）；新版改为只描边不填充，选中格颜色保持可见；
//! 2. 文字省略：GDI `DT_END_ELLIPSIS` → D2D 端用轴对齐裁剪（PushAxisAlignedClip），
//!    视觉为直接截断（标签列宽充裕时无差异）。

use windows::Win32::Foundation::RECT;

use sysoverlay_core::config::Config;

use crate::gfx_d2d::{ColorF, Gfx, HAlign, RectF};
use crate::theme::Theme;

use super::cfg_ref::{CfgRef, Value};
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
/// 指标名称/数值色板（8 预设 + 第 9 格「跟随默认」）。
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
/// 色板总格数（含末尾「跟随默认」格）。
pub const SW_N: usize = 8 + 1;
pub const SW_FOLLOW: usize = 8;

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
    /// 把指定指标颜色重置为「跟随默认」（None）。
    ResetColor(CfgRef),
    /// 重置全部指标颜色（名称色 + 数值色全部返回跟随默认）。
    ResetAllColors,
    /// 把指定指标字号重置为「跟随全局字号」（metric_font_sizes[i] = None）。
    ResetFont(usize),
}

/// 按钮（绑定一个动作）。
pub struct ButtonWidget {
    pub label: String,
    pub action: ButtonAction,
}

/// 背景/前景色板。
pub struct SwatchesWidget {
    /// 0 = 背景色板，1 = 前景色板。
    pub palette: usize,
    pub cfg: CfgRef,
}

/// 逐指标颜色色板。
pub struct MetricSwatchesWidget {
    pub cfg: CfgRef,
}

/// 滑杆 + 数值框。
pub struct SliderWidget {
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub dp: u32,
    pub cfg: CfgRef,
    /// true = 摇杆（弹簧增量式、杆居中）；false = 简单滑杆（绝对值、杆位即值）。
    /// 间距类用简单滑杆：拖动时数值按偏移比例以 ~1..5 单位/秒逼近目标（用户规格）。
    pub joystick: bool,
}

impl IWidget for ToggleWidget {
    fn cfg(&self) -> Option<CfgRef> { Some(self.cfg) }
    fn sync(&self, cfg: &Config, st: &mut WidgetState) { sync_via_cfg(self.cfg, cfg, st) }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl IWidget for SegmentedWidget {
    fn cfg(&self) -> Option<CfgRef> { Some(self.cfg) }
    fn sync(&self, cfg: &Config, st: &mut WidgetState) { sync_via_cfg(self.cfg, cfg, st) }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl IWidget for ButtonWidget {
    fn cfg(&self) -> Option<CfgRef> { None }
    fn sync(&self, _cfg: &Config, _st: &mut WidgetState) {}
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
                        .position(|&(r, g, b)| sysoverlay_core::color::Rgb::new(r, g, b) == rgb)
                })
                .unwrap_or(0);
        }
    }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl IWidget for MetricSwatchesWidget {
    fn cfg(&self) -> Option<CfgRef> { Some(self.cfg) }
    fn sync(&self, cfg: &Config, st: &mut WidgetState) {
        if let Value::Color(c) = self.cfg.get(cfg) {
            match c {
                Some(rgb) => {
                    st.selected = METRIC_SWATCHES
                        .iter()
                        .position(|&(r, g, b)| sysoverlay_core::color::Rgb::new(r, g, b) == rgb)
                        .unwrap_or(SW_FOLLOW);
                    st.value_f = 0.0;
                }
                None => {
                    st.selected = SW_FOLLOW;
                    st.value_f = 1.0;
                }
            }
        }
    }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

impl IWidget for SliderWidget {
    fn cfg(&self) -> Option<CfgRef> { Some(self.cfg) }
    fn sync(&self, cfg: &Config, st: &mut WidgetState) { sync_via_cfg(self.cfg, cfg, st) }
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

// ── D2D 绘制 ─────────────────────────────────────────────

fn c(_t: &Theme, v: u32) -> ColorF {
    ColorF::from_colorref(v, 1.0)
}

/// 控件绘制总入口（含动画水平偏移 `dx`，物理像素）。
#[allow(clippy::too_many_arguments)]
pub fn paint_widget(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    w: &Widget,
    state: &WidgetState,
    dpi: u32,
    theme: &Theme,
    dx: f32,
    ct: f32,
    body_px: f32,
    small_px: f32,
    edit: Option<&EditState>,
    slider_deflect: f32,
    slider_joystick: bool,
) {
    // `w.rect` 是内容坐标（相对内容区顶部）；ct = content_top_y(dpi) 客户区偏移。
    let r = RectF::from_i32(w.rect.left, w.rect.top, w.rect.right, w.rect.bottom).offset(dx, ct);
    if let Some(_t) = w.kind.as_any().downcast_ref::<ToggleWidget>() {
        paint_toggle(rt, gfx, state, dpi, theme, r);
    } else if let Some(seg) = w.kind.as_any().downcast_ref::<SegmentedWidget>() {
        paint_segmented(rt, gfx, state, dpi, theme, r, &seg.labels, small_px);
    } else if let Some(btn) = w.kind.as_any().downcast_ref::<ButtonWidget>() {
        paint_button(rt, gfx, state, dpi, theme, r, &btn.label, body_px);
    } else if let Some(sw) = w.kind.as_any().downcast_ref::<SwatchesWidget>() {
        let palette: &[(u8, u8, u8)] = if sw.palette == 0 { &BG_SWATCHES } else { &FG_SWATCHES };
        paint_swatches(rt, gfx, state, dpi, theme, r, palette);
    } else if let Some(_ms) = w.kind.as_any().downcast_ref::<MetricSwatchesWidget>() {
        paint_metric_swatches(rt, gfx, state, dpi, theme, r, small_px);
    } else if let Some(slider) = w.kind.as_any().downcast_ref::<SliderWidget>() {
        paint_slider(
            rt, gfx, state, dpi, theme, r, slider.dp, slider.min, slider.max, small_px, edit, slider_deflect,
            slider_joystick,
        );
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
    let on = state.value_f > 0.5;
    let radius = r.h() / 2.0;
    let (fill, stroke) = if on {
        (c(t, t.toggle_on), c(t, t.toggle_on))
    } else {
        (c(t, t.toggle_off), c(t, t.toggle_off_stroke))
    };
    gfx.fill_round(rt, r, radius, fill);
    gfx.stroke_round(rt, r, radius, stroke, 1.0);
    let m = super::layout::s(dpi, 3) as f32;
    let d = r.h() - 2.0 * m;
    let kx = if on { r.r - m - d } else { r.l + m };
    gfx.fill_ellipse(
        rt,
        RectF::new(kx, r.t + m, kx + d, r.b - m),
        c(t, t.knob),
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
    let n = labels.len().max(1);
    let seg_w = r.w() / n as f32;
    gfx.fill_round(rt, r, super::layout::s(dpi, 7) as f32, c(t, t.segmented_bg));
    for (i, label) in labels.iter().enumerate() {
        let seg = RectF::new(
            r.l + i as f32 * seg_w,
            r.t,
            r.l + (i + 1) as f32 * seg_w,
            r.b,
        );
        if i == state.selected {
            let m = super::layout::s(dpi, 2) as f32;
            let ir = RectF::new(seg.l + m, seg.t + m, seg.r - m, seg.b - m);
            gfx.fill_round(rt, ir, super::layout::s(dpi, 5) as f32, c(t, t.seg_selected));
            gfx.stroke_round(rt, ir, super::layout::s(dpi, 5) as f32, c(t, t.seg_selected_stroke), 1.0);
        }
        let color = if i == state.selected { c(t, t.label) } else { c(t, t.secondary) };
        gfx.text(rt, label, px, 400, HAlign::Center, seg, color, true);
    }
}

fn paint_button(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    state: &WidgetState,
    dpi: u32,
    t: &Theme,
    r: RectF,
    label: &str,
    px: f32,
) {
    let fill = if state.hovered { c(t, t.accent_hover) } else { c(t, t.accent) };
    gfx.fill_round(rt, r, super::layout::s(dpi, 7) as f32, fill);
    gfx.text(rt, label, px, 400, HAlign::Center, r, ColorF::rgba(255, 255, 255, 1.0), true);
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
    let n = palette.len().max(1);
    let wd = r.w() / n as f32;
    let m = super::layout::s(dpi, 3) as f32;
    for (i, &(cr, cg, cb)) in palette.iter().enumerate() {
        let sw = RectF::new(r.l + i as f32 * wd, r.t, r.l + (i + 1) as f32 * wd, r.b);
        let inner = RectF::new(sw.l + m, sw.t + m, sw.r - m, sw.b - m);
        let stroke = if i == state.selected { c(t, t.accent) } else { c(t, t.card_stroke) };
        gfx.fill_round(rt, inner, super::layout::s(dpi, 5) as f32, ColorF::rgba(cr, cg, cb, 1.0));
        gfx.stroke_round(rt, inner, super::layout::s(dpi, 5) as f32, stroke, 1.0);
    }
}

fn paint_metric_swatches(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    state: &WidgetState,
    dpi: u32,
    t: &Theme,
    r: RectF,
    px: f32,
) {
    let follow_flag = state.value_f > 0.5;
    let n = SW_N;
    let wd = r.w() / n as f32;
    let m = super::layout::s(dpi, 3) as f32;
    for i in 0..n {
        let sw = RectF::new(r.l + i as f32 * wd, r.t, r.l + (i + 1) as f32 * wd, r.b);
        let inner = RectF::new(sw.l + m, sw.t + m, sw.r - m, sw.b - m);
        if i < SW_FOLLOW {
            let (cr, cg, cb) = METRIC_SWATCHES[i];
            gfx.fill_round(rt, inner, super::layout::s(dpi, 4) as f32, ColorF::rgba(cr, cg, cb, 1.0));
            gfx.stroke_round(rt, inner, super::layout::s(dpi, 4) as f32, c(t, t.card_stroke), 1.0);
        } else {
            gfx.fill_round(rt, inner, super::layout::s(dpi, 4) as f32, c(t, t.card));
            gfx.stroke_round(rt, inner, super::layout::s(dpi, 4) as f32, c(t, t.secondary), 1.0);
            gfx.text(rt, "≈", px, 400, HAlign::Center, inner, c(t, t.secondary), true);
        }
        let selected = (i == state.selected) || (i == SW_FOLLOW && follow_flag && state.selected == SW_FOLLOW);
        if selected {
            // 修正：旧版先填卡片色再描边（盖掉选中格颜色），新版只描边。
            gfx.stroke_round(rt, sw, super::layout::s(dpi, 5) as f32, c(t, t.accent), 1.0);
        }
    }
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
    edit: Option<&EditState>,
    deflect: f32,
    joystick: bool,
) {
    let val = state.value_f as f32;
    let editing = state.editing;
    let base = RectF::new(r.l, r.t, r.r, r.b);
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

    // 轨道
    let track_y = (tr.t + tr.b) / 2.0;
    gfx.line(rt, tr.l, track_y, tr.r, track_y, c(t, t.segmented_bg), super::layout::s(dpi, 2).max(1) as f32);

    // 滑块位置：摇杆 = 居中 + deflect×半宽；简单滑杆 = 按当前值映射轨道
    let thumb_d = super::layout::s(dpi, 13) as f32;
    let cx = if joystick {
        let center_x = (tr.l + tr.r) / 2.0;
        let half = tr.w() / 2.0;
        (center_x + deflect * half).clamp(tr.l, tr.r)
    } else {
        let span = (max - min).max(1.0);
        let frac = ((val - min) / span).clamp(0.0, 1.0);
        tr.l + frac * tr.w()
    };
    gfx.fill_ellipse(
        rt,
        RectF::new(cx - thumb_d / 2.0, track_y - thumb_d / 2.0, cx + thumb_d / 2.0, track_y + thumb_d / 2.0),
        c(t, t.accent),
    );

    // 值框
    let fill = if editing { c(t, t.button_bg_hover) } else { c(t, t.button_bg) };
    gfx.fill_round(rt, vb, super::layout::s(dpi, 6) as f32, fill);
    let content = RectF::new(vb.l, vb.t, vb.r - super::layout::s(dpi, 4) as f32, vb.b);

    // 显示文本：编辑态优先显示用户正在输入的内容
    let show: String = if editing {
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
    gfx.text(rt, &show, px, 400, HAlign::Trailing, content, c(t, t.label), true);
}
