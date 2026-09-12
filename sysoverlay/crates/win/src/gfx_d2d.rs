//! Direct2D + DirectWrite 渲染底座（P-D2D）。
//!
//! # 两条链路
//!
//! - **设置页**（本阶段 S1-S3）：`ID2D1HwndRenderTarget` 直画普通窗口客户区；
//! - **悬浮窗高画质模式**（S4）：`ID2D1DCRenderTarget` + `BindDC` 绑到现有
//!   offscreen DIB 的 DC，绘制结果交给既有 `UpdateLayeredWindowIndirect` 上屏。
//!
//! # 资源规则（本模块的三条铁律）
//!
//! 1. `Gfx` 是 D2D/DWrite 资源的**唯一持有者**——窗口销毁 = `Gfx` Drop = COM
//!    引用计数归零，不存在第二种释放路径；
//! 2. [`Gfx::rebuild`] 是**唯一重建入口**——设备丢失（`D2DERR_RECREATE_TARGET`）
//!    与未来 DPI 变化都走它，不存在第二套重建逻辑；
//! 3. 画刷按颜色缓存、文字格式按 (字号,字重,对齐) 缓存，**创建一次复用到底**
//!    （`IDWriteTextFormat` 创建极贵，是 D2D 的头号性能陷阱）。
//!
//! # 坐标约定
//!
//! Render target DPI 固定 96/96（恒等变换）：**全部布局按物理像素进行**，
//! 与现有 `s(dpi, v)` 缩放体系无缝对接，文字直接按像素高度给 DWrite。

use std::collections::HashMap;

use windows::core::{w, PCWSTR};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{BOOL, D2DERR_RECREATE_TARGET, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_END_CLOSED, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, ID2D1GeometrySink, ID2D1HwndRenderTarget, ID2D1Layer,
    ID2D1PathGeometry, ID2D1SolidColorBrush,
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_LAYER_OPTIONS_NONE,
    D2D1_LAYER_PARAMETERS, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
    D2D1_HWND_RENDER_TARGET_PROPERTIES,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat, IDWriteTextLayout,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

// ── 基础值类型 ─────────────────────────────────────────────

/// D2D 颜色（RGBA 0..1）。`key()` 用作画刷缓存键（alpha 量化到 u8）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorF(D2D1_COLOR_F);

impl ColorF {
    pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self(D2D1_COLOR_F {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a,
        })
    }

    /// 从 COLORREF（0x00BBGGRR，theme.rs 体系）转换，alpha 独立给。
    pub const fn from_colorref(c: u32, a: f32) -> Self {
        Self::rgba((c & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, ((c >> 16) & 0xFF) as u8, a)
    }

    /// 画刷缓存键：AABBGGRR。
    pub(crate) fn key(self) -> u32 {
        let d = self.0;
        let a = (d.a.clamp(0.0, 1.0) * 255.0).round() as u32;
        let r = (d.r.clamp(0.0, 1.0) * 255.0).round() as u32;
        let g = (d.g.clamp(0.0, 1.0) * 255.0).round() as u32;
        let b = (d.b.clamp(0.0, 1.0) * 255.0).round() as u32;
        (a << 24) | (b << 16) | (g << 8) | r
    }

    pub fn d2d(self) -> D2D1_COLOR_F {
        self.0
    }

    /// 以新 alpha 重着色（UI 选中态/悬停态半透明用）。
    pub fn with_alpha(mut self, a: f32) -> Self {
        self.0.a = a;
        self
    }
}

/// 物理像素矩形（f32，D2D 用）。
#[derive(Clone, Copy, Debug, Default)]
pub struct RectF {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

impl RectF {
    pub fn new(l: f32, t: f32, r: f32, b: f32) -> Self {
        Self { l, t, r, b }
    }

    pub fn from_i32(l: i32, t: i32, r: i32, b: i32) -> Self {
        Self { l: l as f32, t: t as f32, r: r as f32, b: b as f32 }
    }

    pub fn from_win32(r: RECT) -> Self {
        Self::from_i32(r.left, r.top, r.right, r.bottom)
    }

    pub fn w(self) -> f32 {
        (self.r - self.l).max(0.0)
    }

    pub fn h(self) -> f32 {
        (self.b - self.t).max(0.0)
    }

    /// 平移（翻页动画用）。
    pub fn offset(self, dx: f32, dy: f32) -> Self {
        Self { l: self.l + dx, t: self.t + dy, r: self.r + dx, b: self.b + dy }
    }

    pub fn d2d(self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.l, top: self.t, right: self.r, bottom: self.b }
    }

    pub fn rounded(self, radius: f32) -> D2D1_ROUNDED_RECT {
        D2D1_ROUNDED_RECT { rect: self.d2d(), radiusX: radius, radiusY: radius }
    }

    pub fn point(self) -> D2D_POINT_2F {
        D2D_POINT_2F { x: self.l, y: self.t }
    }
}

/// 文字水平对齐（进 TextFormat 缓存键）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HAlign {
    Leading,
    Center,
    Trailing,
}

impl HAlign {
    fn dwrite(self) -> DWRITE_TEXT_ALIGNMENT {
        match self {
            HAlign::Leading => DWRITE_TEXT_ALIGNMENT_LEADING,
            HAlign::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            HAlign::Trailing => DWRITE_TEXT_ALIGNMENT_TRAILING,
        }
    }

    fn key(self) -> u64 {
        match self {
            HAlign::Leading => 0,
            HAlign::Center => 1,
            HAlign::Trailing => 2,
        }
    }
}

/// 文字格式缓存键：(字号px, 字重, 水平对齐, 是否换行)。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FormatKey(u32, u32, u64, bool);

/// 文字布局缓存键：(文本, 格式键, 布局宽度px)。
///
/// `CreateTextLayout` 是 D2D/DWrite 的头号开销（逐次做 Unicode 断行 + shaping）。
/// 设置页绝大多数是静态标签（行名/导航/标题/脚注），逐帧重建纯属浪费——
/// 这里按「文本 + 格式 + 宽度」缓存 `IDWriteTextLayout` 与其度量。
#[derive(Clone, PartialEq, Eq, Hash)]
struct LayoutKey(String, FormatKey, u32);

/// 缓存的布局 + 实测度量（避免每帧再 `GetMetrics`）。
struct LayoutEntry {
    layout: IDWriteTextLayout,
    width: f32,
    height: f32,
}

/// 布局缓存上限：超出即整表清空（设置页文本量远小于此，正常永不触发）。
const LAYOUT_CACHE_MAX: usize = 512;

// ── SVG 路径（图标用） ─────────────────────────────────────

/// 归一化路径段（坐标为 viewBox 单位，绘制时线性映射到目标矩形）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathSeg {
    Move(f32, f32),
    Line(f32, f32),
    /// 三次贝塞尔：控制点 1、控制点 2、终点。
    Cubic(f32, f32, f32, f32, f32, f32),
    /// 闭合当前子路径。
    Close,
}

// ── Gfx：D2D 资源唯一持有者 ────────────────────────────────

pub struct Gfx {
    d2d: ID2D1Factory,
    dwrite: IDWriteFactory,
    hwnd: HWND,
    /// 设置页直画目标；None = 待懒创建 / 已因设备丢失回收。
    rt: Option<ID2D1HwndRenderTarget>,
    /// 画刷缓存：与 RT 绑定，RT 重建即失效。
    brushes: HashMap<u32, ID2D1SolidColorBrush>,
    /// 文字格式缓存：DWrite 对象，不随 RT 失效。
    formats: HashMap<FormatKey, IDWriteTextFormat>,
    /// 文字布局缓存：DWrite 对象，不随 RT 失效（只依赖 dwrite + format）。
    layouts: HashMap<LayoutKey, LayoutEntry>,
    /// 淡入层（翻页动画用）：与 RT 绑定，RT 重建即失效。
    layer: Option<ID2D1Layer>,
    /// C2：已探测的 UI 字体族（null 结尾 UTF-16），首次绘制时惰性填充，之后复用。
    ui_family: Option<Vec<u16>>,
    /// 图标路径几何缓存：键 = (图标号, 目标宽, 目标高)。
    /// 几何由工厂创建、与 RT 无关，故 RT 重建后仍可复用（无需清理）。
    paths: HashMap<(usize, i32, i32), ID2D1PathGeometry>,
}

/// C2：探测系统可用 UI 字体族，按 Apple 观感优先级降级。
///
/// `GetSystemFontCollection` 是重操作，结果由 [`Gfx::ui_font_family`] 缓存，
/// 绝不可逐帧调用。字族名以 null 结尾的 UTF-16 返回，可直接喂给 `CreateTextFormat`。
fn probe_font_family(dwrite: &IDWriteFactory) -> Vec<u16> {
    const CANDIDATES: [&str; 3] = [
        "Segoe UI Variable Text",
        "Segoe UI Variable",
        "Segoe UI",
    ];
    let fallback: Vec<u16> = "Segoe UI".encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut coll: Option<IDWriteFontCollection> = None;
        if dwrite.GetSystemFontCollection(&mut coll, BOOL(0)).is_ok() {
            if let Some(coll) = coll {
                for cand in CANDIDATES {
                    let w: Vec<u16> = cand.encode_utf16().chain(std::iter::once(0)).collect();
                    let mut idx = 0u32;
                    let mut exists = BOOL(0);
                    if coll.FindFamilyName(PCWSTR(w.as_ptr()), &mut idx, &mut exists).is_ok()
                        && exists.as_bool()
                    {
                        return w;
                    }
                }
            }
        }
    }
    fallback
}

/// 把归一化路径段按 `rect` 线性缩放，构建 D2D 路径几何。
///
/// `view` = SVG viewBox 边长（本项目统一 1024）。坐标为 viewBox 单位。
fn build_path(
    factory: &ID2D1Factory,
    segs: &[PathSeg],
    view: f32,
    rect: RectF,
) -> Option<ID2D1PathGeometry> {
    let sx = rect.w() / view.max(1.0);
    let sy = rect.h() / view.max(1.0);
    let pt = |x: f32, y: f32| D2D_POINT_2F { x: rect.l + x * sx, y: rect.t + y * sy };
    unsafe {
        let geom = factory.CreatePathGeometry().ok()?;
        let sink: ID2D1GeometrySink = geom.Open().ok()?;
        let mut open = false;
        for seg in segs {
            match *seg {
                PathSeg::Move(x, y) => {
                    if open {
                        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    }
                    sink.BeginFigure(pt(x, y), D2D1_FIGURE_BEGIN_FILLED);
                    open = true;
                }
                PathSeg::Line(x, y) => {
                    if open {
                        sink.AddLine(pt(x, y));
                    }
                }
                PathSeg::Cubic(x1, y1, x2, y2, x3, y3) => {
                    if open {
                        let bez = D2D1_BEZIER_SEGMENT {
                            point1: pt(x1, y1),
                            point2: pt(x2, y2),
                            point3: pt(x3, y3),
                        };
                        sink.AddBezier(&bez);
                    }
                }
                PathSeg::Close => {
                    if open {
                        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                        open = false;
                    }
                }
            }
        }
        if open {
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        }
        let _ = sink.Close();
        Some(geom)
    }
}

impl Gfx {
    /// 创建工厂（进程级；窗口句柄仅记录用于目标尺寸查询）。
    pub fn new(hwnd: HWND) -> Option<Self> {
        unsafe {
            let d2d: ID2D1Factory =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
            let dwrite: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            Some(Self {
                d2d,
                dwrite,
                hwnd,
                rt: None,
                brushes: HashMap::new(),
                formats: HashMap::new(),
                layouts: HashMap::new(),
                layer: None,
                ui_family: None,
                paths: HashMap::new(),
            })
        }
    }

    /// 目标就绪（懒创建）。设备丢失/`rebuild` 后再次调用即自动重建。
    pub fn rt(&mut self) -> Option<&ID2D1HwndRenderTarget> {
        if self.rt.is_none() {
            let rt = self.create_rt()?;
            // 画刷与旧 RT 绑定，全部失效；TextFormat 是 DWrite 对象不受影响。
            self.brushes.clear();
            self.layer = None;
            self.rt = Some(rt);
        }
        self.rt.as_ref()
    }

    /// **唯一重建入口**：设备丢失、DPI 变化、窗口主题切换后调用。
    /// 下一次 [`Gfx::rt`] 按新参数重建。
    pub fn rebuild(&mut self) {
        self.rt = None;
        self.brushes.clear();
    }

    /// 尺寸变化（窗口固定尺寸的设置页基本用不到，防御性保留）。
    pub fn resize(&mut self, w: i32, h: i32) -> bool {
        let Some(rt) = self.rt.as_ref() else { return true };
        unsafe {
            let size = D2D_SIZE_U { width: w.max(1) as u32, height: h.max(1) as u32 };
            rt.Resize(&size).is_ok()
        }
    }

    /// 文字抗锯齿模式（RT 状态；绘制批量前设置一次）。
    pub fn set_text_aa(&self, rt: &ID2D1HwndRenderTarget, cleartype: bool) {
        unsafe {
            rt.SetTextAntialiasMode(if cleartype {
                D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE
            } else {
                D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE
            });
        }
    }

    /// 几何抗锯齿（RT 状态）。
    pub fn set_aa(&self, rt: &ID2D1HwndRenderTarget, on: bool) {
        unsafe {
            rt.SetAntialiasMode(if on {
                D2D1_ANTIALIAS_MODE_PER_PRIMITIVE
            } else {
                D2D1_ANTIALIAS_MODE_ALIASED
            });
        }
    }

    /// 取（或创建并缓存）纯色画刷。
    pub fn brush(&mut self, rt: &ID2D1HwndRenderTarget, c: ColorF) -> Option<ID2D1SolidColorBrush> {
        let key = c.key();
        if let Some(b) = self.brushes.get(&key) {
            return Some(b.clone());
        }
        // None = 默认属性（opacity 1.0 / 恒等变换），避免引 Numerics。
        let brush = unsafe { rt.CreateSolidColorBrush(&c.d2d(), None) }.ok()?;
        self.brushes.insert(key, brush.clone());
        Some(brush)
    }

    /// 文字格式缓存键（与 [`Self::text_format`] 同一套规则；布局缓存用它做键）。
    fn format_key(px: f32, weight: u32, align: HAlign, wrap: bool) -> FormatKey {
        FormatKey((px * 4.0).round() as u32, weight, align.key(), wrap)
    }

    /// 取（或创建并缓存）文字格式。`px` 为像素字号（RT 恒等 DPI 下 = DIP）。
    ///
    /// `wrap=false`：整行不换行（标签/值框）；`wrap=true`：按宽度换行（脚注）。
    pub fn text_format(
        &mut self,
        px: f32,
        weight: u32,
        align: HAlign,
        wrap: bool,
    ) -> Option<IDWriteTextFormat> {
        let key = Self::format_key(px, weight, align, wrap);
        if let Some(f) = self.formats.get(&key) {
            return Some(f.clone());
        }
        let fam = self.ui_font_family();
        let fam_w = PCWSTR(fam.as_ptr());
        let fmt = unsafe {
            self.dwrite
                .CreateTextFormat(
                    fam_w,
                    None,
                    DWRITE_FONT_WEIGHT(weight as i32),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    px.max(1.0),
                    w!(""),
                )
                .ok()?
        };
        unsafe {
            let _ = fmt.SetTextAlignment(align.dwrite());
            let _ = fmt.SetWordWrapping(if wrap {
                DWRITE_WORD_WRAPPING_WRAP
            } else {
                DWRITE_WORD_WRAPPING_NO_WRAP
            });
        }
        self.formats.insert(key, fmt.clone());
        Some(fmt)
    }

    /// 画一行文字，返回其实测 (宽, 高) 供调用方做垂直居中/布局微调。
    ///
    /// `vcenter=true` 时在 `rect` 内垂直居中；否则从 `rect.t` 顶对齐。
    ///
    /// `IDWriteTextLayout` 按 (文本, 格式, 宽度) 缓存复用——静态标签不再逐帧
    /// 重新 shaping（设置页每帧 ~30 处文字，这是此前"卡顿"的主要来源）。
    pub(crate) fn draw_text(
        &mut self,
        rt: &ID2D1HwndRenderTarget,
        text: &str,
        fmt: &IDWriteTextFormat,
        fkey: FormatKey,
        rect: RectF,
        brush: &ID2D1SolidColorBrush,
        vcenter: bool,
    ) -> Option<(f32, f32)> {
        // 宽度取整进键：0.5px 内的抖动不值得重建一次布局。
        let key = LayoutKey(text.to_string(), fkey, (rect.w() * 2.0).round() as u32);
        let entry = match self.layouts.get(&key) {
            Some(e) => (e.layout.clone(), e.width, e.height),
            None => {
                let wide: Vec<u16> = text.encode_utf16().collect();
                let layout: IDWriteTextLayout = unsafe {
                    self.dwrite.CreateTextLayout(&wide, fmt, rect.w().max(1.0), rect.h().max(1.0)).ok()?
                };
                let mut m = DWRITE_TEXT_METRICS::default();
                unsafe { layout.GetMetrics(&mut m).ok()? };
                if self.layouts.len() >= LAYOUT_CACHE_MAX {
                    self.layouts.clear();
                }
                self.layouts.insert(
                    key,
                    LayoutEntry { layout: layout.clone(), width: m.width, height: m.height },
                );
                (layout, m.width, m.height)
            }
        };
        let (layout, mw, mh) = entry;
        unsafe {
            let y = if vcenter { rect.t + (rect.h() - mh) * 0.5 } else { rect.t };
            rt.DrawTextLayout(
                D2D_POINT_2F { x: rect.l, y },
                &layout,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
        Some((mw, mh))
    }

    /// 只测量不绘制（toast 胶囊等需要先量宽再画底的场景）。
    /// 复用布局缓存：宽度键取哨兵值 u32::MAX，与任何绘制调用不冲突。
    pub fn measure_text(&mut self, text: &str, px: f32, weight: u32) -> Option<(f32, f32)> {
        let fmt = self.text_format(px, weight, HAlign::Leading, false)?;
        let fkey = Self::format_key(px, weight, HAlign::Leading, false);
        let key = LayoutKey(text.to_string(), fkey, u32::MAX);
        if let Some(e) = self.layouts.get(&key) {
            return Some((e.width, e.height));
        }
        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout: IDWriteTextLayout = unsafe {
            self.dwrite.CreateTextLayout(&wide, &fmt, 1.0e4, 1.0e4).ok()?
        };
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut m).ok()? };
        if self.layouts.len() >= LAYOUT_CACHE_MAX {
            self.layouts.clear();
        }
        let out = (m.width, m.height);
        self.layouts.insert(key, LayoutEntry { layout, width: m.width, height: m.height });
        Some(out)
    }

    /// 绘制一帧：`BeginDraw → painter → EndDraw`。
    ///
    /// 设备丢失（`D2DERR_RECREATE_TARGET`）时自动 [`Gfx::rebuild`] 并返回 false，
    /// 调用方下一帧重试即恢复。返回 true 表示本帧已呈现。
    pub fn frame(&mut self, painter: impl FnOnce(&ID2D1HwndRenderTarget, &mut Self)) -> bool {
        if self.rt().is_none() {
            return false;
        }
        let Some(rt) = self.rt.clone() else { return false };
        unsafe {
            rt.BeginDraw();
            painter(&rt, self);
            if let Err(e) = rt.EndDraw(None, None) {
                if e.code() == D2DERR_RECREATE_TARGET {
                    crate::probe::log("d2d device lost -> rebuild");
                    self.rebuild();
                } else {
                    crate::probe::log(&format!("d2d EndDraw err=0x{:08x}", e.code().0));
                }
                return false;
            }
        }
        true
    }

    // ── 绘制基元（画刷经缓存；调用方持 rt 克隆，与 &mut self 无别名） ──

    pub fn fill_round(&mut self, rt: &ID2D1HwndRenderTarget, r: RectF, radius: f32, c: ColorF) {
        if let Some(b) = self.brush(rt, c) {
            unsafe { rt.FillRoundedRectangle(&r.rounded(radius), &b) };
        }
    }

    pub fn stroke_round(
        &mut self,
        rt: &ID2D1HwndRenderTarget,
        r: RectF,
        radius: f32,
        c: ColorF,
        width: f32,
    ) {
        if let Some(b) = self.brush(rt, c) {
            unsafe { rt.DrawRoundedRectangle(&r.rounded(radius), &b, width, None) };
        }
    }

    pub fn fill_rect(&mut self, rt: &ID2D1HwndRenderTarget, r: RectF, c: ColorF) {
        if let Some(b) = self.brush(rt, c) {
            unsafe { rt.FillRectangle(&r.d2d(), &b) };
        }
    }

    /// 内切椭圆（开关滑钮 / 滑杆 thumb）。
    pub fn fill_ellipse(&mut self, rt: &ID2D1HwndRenderTarget, r: RectF, c: ColorF) {
        let e = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
            point: D2D_POINT_2F { x: (r.l + r.r) * 0.5, y: (r.t + r.b) * 0.5 },
            radiusX: r.w() * 0.5,
            radiusY: r.h() * 0.5,
        };
        if let Some(b) = self.brush(rt, c) {
            unsafe { rt.FillEllipse(&e, &b) };
        }
    }

    /// 填充一条 SVG 路径（viewBox `view` 边长）到 `rect`。
    /// `key` 为图标序号；几何按 (key, w, h) 缓存，避免逐帧重建。
    pub fn fill_path(
        &mut self,
        rt: &ID2D1HwndRenderTarget,
        key: usize,
        segs: &[PathSeg],
        view: f32,
        rect: RectF,
        color: ColorF,
    ) {
        let ck = (key, rect.w() as i32, rect.h() as i32);
        if !self.paths.contains_key(&ck) {
            let Some(g) = build_path(&self.d2d, segs, view, rect) else { return };
            self.paths.insert(ck, g);
        }
        // 克隆句柄以结束对 self.paths 的借用（COM 引用计数，代价极低）。
        let Some(geom) = self.paths.get(&ck).cloned() else { return };
        if let Some(b) = self.brush(rt, color) {
            unsafe { rt.FillGeometry(&geom, &b, None); }
        }
    }

    pub fn line(
        &mut self,
        rt: &ID2D1HwndRenderTarget,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        c: ColorF,
        width: f32,
    ) {
        if let Some(b) = self.brush(rt, c) {
            unsafe {
                rt.DrawLine(
                    D2D_POINT_2F { x: x1, y: y1 },
                    D2D_POINT_2F { x: x2, y: y2 },
                    &b,
                    width,
                    None,
                )
            };
        }
    }

    /// 文字一步到位：格式缓存 + 画刷缓存 + 实测 (宽,高) 返回。单行（不换行）。
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        rt: &ID2D1HwndRenderTarget,
        s: &str,
        px: f32,
        weight: u32,
        align: HAlign,
        rect: RectF,
        c: ColorF,
        vcenter: bool,
    ) -> Option<(f32, f32)> {
        self.text_ex(rt, s, px, weight, align, rect, c, vcenter, false)
    }

    /// 多行版本（`wrap=true`，按宽度换行；脚注用）。
    #[allow(clippy::too_many_arguments)]
    pub fn text_wrap(
        &mut self,
        rt: &ID2D1HwndRenderTarget,
        s: &str,
        px: f32,
        weight: u32,
        align: HAlign,
        rect: RectF,
        c: ColorF,
    ) -> Option<(f32, f32)> {
        self.text_ex(rt, s, px, weight, align, rect, c, false, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn text_ex(
        &mut self,
        rt: &ID2D1HwndRenderTarget,
        s: &str,
        px: f32,
        weight: u32,
        align: HAlign,
        rect: RectF,
        c: ColorF,
        vcenter: bool,
        wrap: bool,
    ) -> Option<(f32, f32)> {
        let fmt = self.text_format(px, weight, align, wrap)?;
        let b = self.brush(rt, c)?;
        let fkey = Self::format_key(px, weight, align, wrap);
        self.draw_text(rt, s, &fmt, fkey, rect, &b, vcenter)
    }

    /// 淡入层：`push_fade(..)` 与 `pop_fade(rt)` 成对；层不可用时返回 false，
    /// 调用方降级为无淡入直接画。
    pub fn push_fade(&mut self, rt: &ID2D1HwndRenderTarget, bounds: RectF, opacity: f32) -> bool {
        if self.layer.is_none() {
            self.layer = unsafe { rt.CreateLayer(None) }.ok();
        }
        let Some(layer) = self.layer.clone() else { return false };
        let p = D2D1_LAYER_PARAMETERS {
            contentBounds: bounds.d2d(),
            geometricMask: std::mem::ManuallyDrop::new(None),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: opacity.clamp(0.0, 1.0),
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        unsafe { rt.PushLayer(&p, &layer) };
        true
    }

    pub fn pop_fade(&self, rt: &ID2D1HwndRenderTarget) {
        unsafe { rt.PopLayer(); };
    }

    /// 整帧水平平移（翻页滑入）。`dx` 为物理像素。
    pub fn set_translate(&self, rt: &ID2D1HwndRenderTarget, dx: f32, dy: f32) {
        unsafe { rt.SetTransform(&Matrix3x2::translation(dx, dy)) };
    }

    pub fn reset_transform(&self, rt: &ID2D1HwndRenderTarget) {
        unsafe { rt.SetTransform(&Matrix3x2::identity()) };
    }

    /// C2：取（或探测并缓存）UI 字体族 wide 名。`ui_family` 命中即不再探测。
    fn ui_font_family(&mut self) -> Vec<u16> {
        if let Some(f) = &self.ui_family {
            return f.clone();
        }
        let chosen = probe_font_family(&self.dwrite);
        self.ui_family = Some(chosen.clone());
        chosen
    }

    fn create_rt(&self) -> Option<ID2D1HwndRenderTarget> {        unsafe {
            let mut rc = RECT::default();
            GetClientRect(self.hwnd, &mut rc).ok()?;
            let size = D2D_SIZE_U {
                width: (rc.right - rc.left).max(1) as u32,
                height: (rc.bottom - rc.top).max(1) as u32,
            };
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                // 恒等 DPI：布局全部按物理像素（与 s(dpi,v) 体系对接）。
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: self.hwnd,
                pixelSize: size,
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            self.d2d.CreateHwndRenderTarget(&props, &hwnd_props).ok()
        }
    }
}

#[cfg(test)]
mod font_tests {
    use super::*;

    #[test]
    fn probe_returns_null_terminated_wide() {
        unsafe {
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap();
            let fam = probe_font_family(&dwrite);
            assert!(!fam.is_empty(), "字族名不应为空");
            assert_eq!(*fam.last().unwrap(), 0, "须为空结尾 UTF-16 串");
        }
    }
}
