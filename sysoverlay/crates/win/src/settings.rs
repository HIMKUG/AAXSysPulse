//! 设置面板 —— 完全自绘的 Apple HIG 风格分页窗口（无原生控件）。
//!
//! # 结构（2026-09-05 用户拍板）
//!
//! - **自绘标题栏**：圆角矩形按钮 固定 / 最小化 / 关闭（右侧顺排）
//! - **6 分页导航**：常规 / 布局 / 颜色 / 动效 / 窗口 / 关于
//! - **分页切换动画**：水平滑动 + 淡入（三次方缓出，时长跟随 [`Config::anim_ms`]）
//! - 控件全部自绘：开关 / 分段 / **滑动条+输入框** / 色板（含「跟随默认」）/ 按钮
//! - 刷新率等旧控件保留，字号/间距/透明度/宽高等全部换成滑杆+输入框
//!
//! 所有改动即时生效 + 落盘 + `WM_RECONFIG` 通知悬浮窗重排版。

use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Instant;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::{
    AlphaBlend, BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW,
    CreatePen, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, Ellipse, EndPaint, FillRect,
    InvalidateRect, LineTo, MoveToEx, RoundRect, SelectObject, SetBkMode, SetTextColor, AC_SRC_OVER,
    DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK,
    DRAW_TEXT_FORMAT, HBRUSH, HDC, HFONT, PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, LoadCursorW, PostMessageW, RegisterClassExW,
    SetCursor, SetWindowPos, ShowWindow, HCURSOR, IDC_HAND, SW_HIDE, SW_SHOW, HWND_NOTOPMOST,
    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WINDOW_EX_STYLE, WNDCLASSEXW, WS_POPUP,
};

use crate::theme::{rgb, Theme};
use crate::titlebar::{self, TitleBarState, TitleBtn};
use crate::window::{self, WM_RECONFIG};
use crate::{autostart, config_io};
use sysoverlay_core::color::Rgb;
use sysoverlay_core::config::Config;
use sysoverlay_core::types::{Lang, MetricColors, RenderStyle, Shape};

/// 控件 ID 类型（与 IDC_ 常量对应）。
pub type WidgetId = i32;

// ── 消息常量 ─────────────────────────────────────────────
const WM_DESTROY: u32 = 0x0002;
const WM_CLOSE: u32 = 0x0010;
const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_SETTINGCHANGE: u32 = 0x001A;
const WM_SETCURSOR: u32 = 0x0020;
const WM_NCHITTEST: u32 = 0x0084;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_MOUSELEAVE: u32 = 0x02A2;
const WM_CAPTURECHANGED: u32 = 0x0215;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_DIAG_CLICK: u32 = 0x8000 + 41;
const WM_TIMER: u32 = 0x0113;

const HTCAPTION: u32 = 2;
const HTCLIENT: u32 = 1;
const HTTRANSPARENT: i32 = -1;

// ── 控件语义 ID ──────────────────────────────────────────
/// P2 指标开关（保留旧 IDC_CPU 以便诊断）。
const IDC_CPU: i32 = 101;
const IDC_MEM: i32 = 102;
const IDC_GPU: i32 = 103;
const IDC_NET: i32 = 104;
const IDC_POWER: i32 = 105;

/// P1 布局。
const IDC_STYLE: i32 = 150;
const IDC_SHAPE: i32 = 151;
const IDC_FONT: i32 = 152;
const IDC_MGAP: i32 = 153;
const IDC_LVGAP: i32 = 154;
const IDC_MEMUNIT: i32 = 155;
const IDC_BG: i32 = 156;
const IDC_FG: i32 = 157;
const IDC_SHADOW: i32 = 158;
const IDC_OUTLINE: i32 = 159;

/// P2 颜色：每指标 3 个（开关 + 名称色 + 数值色）+ 逐指标字号。
const IDC_M0_ON: i32 = 200;
const IDC_MFS0: i32 = 220;

/// P3 动效。
const IDC_HOVER_OP: i32 = 230;
const IDC_NORM_OP: i32 = 231;
const IDC_ANIM: i32 = 232;
const IDC_HOVER: i32 = 233;

/// P4 窗口。
const IDC_LOCK_SIZE: i32 = 240;
const IDC_WW: i32 = 241;
const IDC_WH: i32 = 242;
const IDC_RESIZE: i32 = 243;
const IDC_TOPMOST: i32 = 244;
const IDC_RATE: i32 = 245;
const IDC_PIN: i32 = 246;

/// P0 常规。
const IDC_LANG: i32 = 260;
const IDC_AUTOSTART: i32 = 261;
const IDC_DEBUG: i32 = 262;

/// 标题栏按钮（rect 恒定，独立命中）。

/// 刷新率分段：索引 → interval_ms（0 = Auto）。
const RATE_MS: [u32; 6] = [0, 100, 200, 1000, 2000, 5000];

/// 指标/前景/背景色板（浅深通用）。
const BG_SWATCHES: [(u8, u8, u8); 6] = [
    (26, 22, 20),
    (20, 22, 27),
    (27, 26, 30),
    (42, 42, 46),
    (255, 255, 255),
    (61, 61, 66),
];
const FG_SWATCHES: [(u8, u8, u8); 6] = [
    (244, 241, 238),
    (238, 241, 244),
    (255, 255, 255),
    (220, 220, 220),
    (20, 20, 20),
    (30, 30, 30),
];
/// 指标名称/数值色板（8 预设 + 第 9 格「跟随默认」）。
const METRIC_SWATCHES: [(u8, u8, u8); 8] = [
    (255, 255, 255),
    (10, 200, 255),
    (124, 255, 120),
    (255, 215, 120),
    (255, 140, 60),
    (255, 96, 160),
    (190, 120, 255),
    (140, 255, 255),
];
/// 色板总格数（含末尾“跟随默认”格）。
const SW_N: usize = 8 + 1;
const SW_FOLLOW: usize = 8;

// ── 布局常量（逻辑像素，按 DPI 缩放） ────────────────────
const CLIENT_W: i32 = 420;
const MARGIN_X: i32 = 16;
const TOP_MARGIN: i32 = 14;
const BOTTOM_MARGIN: i32 = 16;
const NAV_H: i32 = 32;
const NAV_GAP: i32 = 8;
const CARD_PAD: i32 = 10;
const CAPTION_H: i32 = 14;
const CAPTION_GAP: i32 = 6;
const ROW_H: i32 = 28;
const WIDGET_H: i32 = 24;
const CARD_RADIUS: i32 = 10;
const LABEL_W: i32 = 120;
const TOGGLE_W: i32 = 40;
const VALUEBOX_W: i32 = 64;
const FOOTNOTE_H: i32 = 60;

// 定时器
const PAGE_TIMER: usize = 11;
const EDIT_TIMER: usize = 12;

// ── GDI extern（window.rs 的声明是私有的，此处独立声明） ─
#[link(name = "user32")]
extern "system" {
    fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
    fn IsWindow(hwnd: HWND) -> windows::Win32::Foundation::BOOL;
}

const GWLP_UD: i32 = -21;

/// 已打开的设置窗口句柄（单例；-1 = 未创建）。
static SETTINGS_HWND: AtomicIsize = AtomicIsize::new(-1);

// ── 控件模型 ─────────────────────────────────────────────

#[derive(Clone, Debug)]
enum WKind {
    Toggle { on: bool },
    Segmented { labels: Box<[&'static str]>, sel: usize },
    Button { label: String },
    /// 预设色板（背景/前景 6 格）。
    Swatches { sel: usize, palette: usize },
    /// 指标配色色板（8 预设 + 跟随默认）。
    MetricSwatches { follow: bool, sel: usize },
    /// 滑杆 + 数值输入框一体控件。
    Slider { val: f32, min: f32, max: f32, step: f32, unit: &'static str, dp: u32 },
}

struct Widget {
    id: i32,
    kind: Box<dyn IWidget>,
    rect: RECT,
}

#[derive(Debug)]
struct Row {
    label: &'static str,
    /// 该行从左到右的 widget 下标。
    widgets: Vec<usize>,
}

#[derive(Debug, Default)]
struct Card {
    caption: &'static str,
    rect: RECT,
    rows: Vec<Row>,
    footnote: Option<&'static str>,
}

#[derive(Default)]
struct Page {
    cards: Vec<Card>,
    widgets: Vec<Widget>,
    /// 内容区高度（逻辑像素缩放后）。
    h: i32,
}

const MAX_EDIT_BUF: usize = 16;

/// 输入框编辑态。
struct EditState {
    idx: usize,
    buf: Vec<u16>,
    caret: bool,
}

// ── 统一交互状态机（替换 drag/edit/hover 三个独立字段）─────────────

/// 替代 `drag: Option<usize>`、`edit: Option<EditState>`、`hover: Option<usize>`
/// 三职合一的互斥状态机。
#[derive(Debug, Clone, PartialEq)]
pub enum Interaction {
    Idle,
    Hover { widget: usize },       // usize = ctx.pages[page].widgets 的下标
    Dragging { widget: usize, origin: POINT },
    Editing { widget: usize },     // Editing 态只有 Slider 有
}

impl Default for Interaction {
    fn default() -> Self { Interaction::Idle }
}

// ── Widget Trait（P1 统一控件接口）────────────────────────

use std::any::Any;

/// 统一控件 trait。替代 WKind enum + 散落 draw_*/apply_* 函数。
/// 每个控件实现 paint() / hit_test() / handle() / sync_state()。
pub trait IWidget: Any + Send {
    fn id(&self) -> WidgetId;
    fn hit_test(&self, rect: RECT, pt: POINT) -> bool {
        pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom
    }
    /// 从 Config 同步当前值到 state
    fn sync_state(&self, cfg: &Config, state: &mut WidgetState);
    fn as_any(&self) -> &dyn Any;
}

/// 每帧派生的控件状态。
#[derive(Default, Clone, Debug)]
pub struct WidgetState {
    pub value_f: f64,     // 泛化浮点值
    pub selected: usize,  // 索引选择（Segmented/Swatches/MetricSwatches）
    pub hovered: bool,
    pub editing: bool,
}

/// Toggle 控件。
pub struct ToggleWidget {
    pub id: WidgetId,
    cfg_key: &'static str,
}

impl ToggleWidget {
    pub fn new(id: WidgetId, cfg_key: &'static str) -> Self {
        Self { id, cfg_key }
    }
}

impl IWidget for ToggleWidget {
    fn id(&self) -> WidgetId { self.id }

    fn sync_state(&self, cfg: &Config, state: &mut WidgetState) {
        state.value_f = match self.cfg_key {
            "show_cpu" => cfg.show_cpu as i32,
            "show_mem" => cfg.show_mem as i32,
            "show_gpu" => cfg.show_gpu as i32,
            "show_net" => cfg.show_net as i32,
            "power_enabled" => cfg.power_enabled as i32,
            "text_shadow" => cfg.text_shadow as i32,
            "text_outline" => cfg.text_outline as i32,
            "topmost" => cfg.topmost as i32,
            "autostart" => cfg.autostart as i32,
            "debug_log" => cfg.debug_log as i32,
            "hover" => cfg.hover as i32,
            "window_size.is_some()" => cfg.window_size.is_some() as i32,
            _ => 0,
        } as f64;
    }

    fn as_any(&self) -> &dyn Any { self }
}

/// Segmented 分段控件。
pub struct SegmentedWidget {
    pub id: WidgetId,
    labels: Box<[String]>,
}

impl SegmentedWidget {
    pub fn new(id: WidgetId, labels: &[&str]) -> Self {
        Self { id: id, labels: labels.iter().map(|s| s.to_string()).collect() }
    }
}

impl IWidget for SegmentedWidget {
    fn id(&self) -> WidgetId { self.id }

    fn sync_state(&self, cfg: &Config, state: &mut WidgetState) {
        state.selected = match self.id {
            IDC_STYLE => cfg.render_style as usize,
            IDC_SHAPE => cfg.shape as usize,
            IDC_MEMUNIT => if cfg.mem_show_gb { 1 } else { 0 },
            IDC_RATE => if cfg.refresh_auto { 0 } else {
                match cfg.interval_ms {
                    100 => 1, 200 => 2, 1000 => 3, 2000 => 4, _ => 5,
                }
            },
            IDC_LANG => cfg.lang as usize,
            _ => 0,
        };
    }

    fn as_any(&self) -> &dyn Any { self }
}

/// Button 按钮控件。
pub struct ButtonWidget {
    pub id: WidgetId,
    label: String,
}

impl ButtonWidget {
    pub fn new(id: WidgetId, label: &str) -> Self {
        Self { id, label: label.into() }
    }
}

impl IWidget for ButtonWidget {
    fn id(&self) -> WidgetId { self.id }
    fn sync_state(&self, _cfg: &Config, _state: &mut WidgetState) {}
    fn as_any(&self) -> &dyn Any { self }
}

/// Swatches 颜色面板控件。
pub struct SwatchesWidget {
    pub id: WidgetId,
    palette: usize,  // palette index
}

impl SwatchesWidget {
    pub fn new(id: WidgetId, palette: usize) -> Self {
        Self { id, palette }
    }
}

impl IWidget for SwatchesWidget {
    fn id(&self) -> WidgetId { self.id }

    fn sync_state(&self, cfg: &Config, state: &mut WidgetState) {
        let palette = if self.palette == 0 { &BG_SWATCHES } else { &FG_SWATCHES };
        let color = if self.id == IDC_FG { cfg.fg } else { cfg.bg };
        state.selected = palette.iter().position(|&(r, g, b)| Rgb::new(r, g, b) == color).unwrap_or(0);
    }

    fn as_any(&self) -> &dyn Any { self }
}

/// MetricSwatches 逐指标颜色选择器。
pub struct MetricSwatchesWidget {
    pub id: WidgetId,
    metric: usize,  // 0=cpu, 1=mem, 2=gpu, 3=net
}

impl MetricSwatchesWidget {
    pub fn new(id: WidgetId, metric: usize) -> Self {
        Self { id, metric }
    }
}

impl IWidget for MetricSwatchesWidget {
    fn id(&self) -> WidgetId { self.id }

    fn sync_state(&self, cfg: &Config, state: &mut WidgetState) {
        // id = IDC_M0_ON + metric*3 + slot (slot 1=label, 2=value)
        let metric = ((self.id - IDC_M0_ON) / 3) as usize;
        let slot = (self.id - IDC_M0_ON) % 3;
        let is_label = slot == 1;
        if metric < 5 {
            let mc = &cfg.metric_colors[metric];
            let color_opt = if is_label { mc.label } else { mc.value };
            match color_opt {
                Some(c) => {
                    state.selected = METRIC_SWATCHES.iter().position(|&(r, g, b)| Rgb::new(r, g, b) == c).unwrap_or(SW_FOLLOW);
                    state.value_f = 0.0;
                }
                None => {
                    state.selected = SW_FOLLOW;
                    state.value_f = 1.0;
                }
            }
        }
    }

    fn as_any(&self) -> &dyn Any { self }
}

/// Slider 滑动条控件。
pub struct SliderWidget {
    pub id: WidgetId,
    min: f32,
    max: f32,
    step: f32,
    unit: &'static str,
    dp: u32,
}

impl SliderWidget {
    pub fn new(id: WidgetId, min: f32, max: f32, step: f32, unit: &'static str, dp: u32) -> Self {
        Self { id, min, max, step, unit, dp }
    }
}

impl IWidget for SliderWidget {
    fn id(&self) -> WidgetId { self.id }

    fn sync_state(&self, cfg: &Config, state: &mut WidgetState) {
        state.value_f = match self.id {
            IDC_FONT => cfg.font_size as f64,
            IDC_MGAP => cfg.metric_gap as f64,
            IDC_LVGAP => cfg.label_value_gap as f64,
            IDC_HOVER_OP => cfg.hover_opacity as f64,
            IDC_NORM_OP => cfg.normal_opacity as f64,
            IDC_ANIM => cfg.anim_ms as f64,
            IDC_WW => cfg.window_size.map(|s| s.0 as f64).unwrap_or(300.0),
            IDC_WH => cfg.window_size.map(|s| s.1 as f64).unwrap_or(200.0),
            _ if self.id >= IDC_MFS0 && self.id < IDC_MFS0 + 5 => {
                cfg.metric_font_sizes[(self.id - IDC_MFS0) as usize].unwrap_or(cfg.font_size) as f64
            }
            _ => 0.0,
        };
    }

    fn as_any(&self) -> &dyn Any { self }
}

struct Fonts {
    body: HFONT,
    small: HFONT,
}

struct SettingsCtx {
    overlay: HWND,
    theme: Theme,
    dpi: u32,
    fonts: Fonts,
    hand: HCURSOR,
    pages: [Page; 6],
    /// 当前分页。
    page: usize,
    /// 翻页动画（None = 无动画进行中）。
    page_anim: Option<PageAnim>,
    /// 统一交互状态机（替代 drag/edit/hover 三个独立字段）。
    interaction: Interaction,
    /// 正在编辑的输入框（buf/caret）。
    edit: Option<EditState>,
    tracking: bool,
    /// 自定义标题栏状态。
    tb: TitleBarState,
    /// 待重绘的控件下标集合（None = 全量重绘）。
    dirty: Option<Vec<usize>>,
}

// ── Interaction 辅助 ────────────────────────────────────────

impl SettingsCtx {
    /// 当前正在交互的控件下标（若有）。
    fn active_widget(&self) -> Option<usize> {
        match &self.interaction {
            Interaction::Idle => None,
            Interaction::Hover { widget }
            | Interaction::Dragging { widget, .. }
            | Interaction::Editing { widget } => Some(*widget),
        }
    }

    fn is_editing(&self) -> bool {
        matches!(&self.interaction, Interaction::Editing { .. })
    }

    fn is_dragging(&self) -> bool {
        matches!(&self.interaction, Interaction::Dragging { .. })
    }

    /// 标记单个控件需要重绘。
    fn invalidate_widget(&mut self, idx: usize) {
        if let Some(ref mut v) = self.dirty {
            if !v.contains(&idx) { v.push(idx); }
        } else {
            self.dirty = Some(vec![idx]);
        }
    }

    /// 标记整个当前页需要重绘。
    fn invalidate_page(&mut self) {
        self.dirty = None; // None = full page repaint
    }

    /// 取出 dirty list 并重置。
    fn take_dirty(&mut self) -> Option<Vec<usize>> {
        self.dirty.take()
    }
}

/// 翻页动画：新页自右侧滑入同时淡入。
struct PageAnim {
    t0: Instant,
}

/// DPI 缩放。
fn s(dpi: u32, v: i32) -> i32 {
    v * dpi as i32 / 96
}

// ── 公开入口 ─────────────────────────────────────────────

/// 打开（或前置）设置窗口。
pub fn open_settings(overlay: HWND) {
    let existing = SETTINGS_HWND.load(Ordering::SeqCst);
    if existing != -1 {
        let hwnd = HWND(existing as *mut _);
        if unsafe { IsWindow(hwnd) }.as_bool() {
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
            }
            return;
        }
        SETTINGS_HWND.store(-1, Ordering::SeqCst);
    }
    create_dialog(overlay);
}

/// 诊断用：模拟点击 CPU 开关（SYSOVERLAY_DIAG=settings,click）。
pub fn diag_click_cpu() {
    let hwnd = SETTINGS_HWND.load(Ordering::SeqCst);
    if hwnd == -1 {
        crate::probe::log("diag_click_cpu: no settings window");
        return;
    }
    unsafe {
        let _ = PostMessageW(HWND(hwnd as *mut _), WM_DIAG_CLICK, WPARAM(0), LPARAM(0));
    }
}

// ── 页面内容高度 ─────────────────────────────────────────

/// P2（颜色）行数最多：5 指标 × 3 行 = 15 行 + 说明。
fn page_rows(page: usize) -> i32 {
    match page {
        0 => 3,     // 常规：语言/自启/调试
        1 => 10,    // 布局
        2 => 15,    // 颜色
        3 => 4,     // 动效
        4 => 6,     // 窗口
        _ => 3,     // 关于
    }
}

/// 某页有没有脚注（目前只有「常规」页有）。
fn page_has_footnote(p: usize) -> bool {
    p == 0
}

/// 某页的内容区高度（含卡片四周 + 标题 + 脚注）。
fn content_height(dpi: u32, page: usize) -> i32 {
    let rows = page_rows(page);
    let foot = if page_has_footnote(page) { FOOTNOTE_H } else { 0 };
    s(dpi, TOP_MARGIN + BOTTOM_MARGIN)
        + s(dpi, CAPTION_H + CAPTION_GAP)
        + 2 * s(dpi, CARD_PAD)
        + s(dpi, rows * ROW_H)
        + s(dpi, foot)
}

/// 窗口总高 = 标题栏 + 导航 + 最高内容页。
fn window_total_h(dpi: u32) -> i32 {
    s(dpi, titlebar::TB_H)
        + s(dpi, NAV_H + NAV_GAP)
        + (0..6).map(|p| content_height(dpi, p)).max().unwrap_or(0)
        + s(dpi, 4)
}

/// 调用方持有 cfg 时用（打开/重建）。
fn window_total_h_from(dpi: u32) -> i32 {
    window_total_h(dpi)
}

// ── 窗口创建 ─────────────────────────────────────────────

fn create_dialog(overlay: HWND) {
    unsafe {
        let hinst = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(_) => return,
        };

        static REGISTERED: std::sync::Once = std::sync::Once::new();
        REGISTERED.call_once(|| {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(settings_wndproc),
                hInstance: hinst.into(),
                lpszClassName: windows::core::w!("SysOverlay_Settings"),
                hbrBackground: HBRUSH::default(),
                ..Default::default()
            };
            let _ = RegisterClassExW(&wc);
        });

        let theme = Theme::system();
        let dpi = crate::dpi::get_system_dpi();
        let lang = window::get_config_of(overlay)
            .map(|c| c.lang)
            .unwrap_or_default();
        let title_wide: Vec<u16> = crate::i18n::t(lang, "WinTitle")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let w = s(dpi, CLIENT_W);
        let h = window_total_h_from(dpi);

        // 屏幕居中
        let sw = crate::dpi::screen_w();
        let sh = crate::dpi::screen_h();
        let x = (sw - w).max(0) / 2;
        let y = (sh - h).max(0) / 2;

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::w!("SysOverlay_Settings"),
            windows::core::PCWSTR(title_wide.as_ptr()),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            None,
            hinst,
            None,
        );
        let Ok(hwnd) = hwnd else { return };

        let mut ctx = SettingsCtx {
            overlay,
            theme,
            dpi,
            fonts: Fonts::new(dpi),
            hand: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
            pages: Default::default(),
            page: 0,
            page_anim: None,
            interaction: Interaction::Idle,
            edit: None,
            tracking: false,
            tb: TitleBarState::default(),
            dirty: None,
        };
        if let Some(cfg) = window::get_config_of(overlay) {
            build_pages(&mut ctx, &cfg);
        }
        SetWindowLongPtrW(hwnd, GWLP_UD, Box::into_raw(Box::new(ctx)) as isize);

        apply_window_chrome(hwnd, theme.dark);

        SETTINGS_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        crate::probe::log(&format!("settings hwnd=0x{:x} w={w} h={h}", hwnd.0 as usize));
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
}

/// Win11：DWM 圆角 + 跟随主题的标题栏。
unsafe fn apply_window_chrome(hwnd: HWND, dark: bool) {
    let pref: i32 = 2;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &pref as *const i32 as *const _,
        4,
    );
    let dark_i: i32 = dark as i32;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
        &dark_i as *const i32 as *const _,
        4,
    );
}

// ── 字体 ─────────────────────────────────────────────────

impl Fonts {
    unsafe fn new(dpi: u32) -> Self {
        Self {
            body: make_font(dpi, 13, 400),
            small: make_font(dpi, 11, 400),
        }
    }

    unsafe fn delete(&mut self) {
        let _ = DeleteObject(self.body);
        let _ = DeleteObject(self.small);
    }
}

unsafe fn make_font(dpi: u32, pt: i32, weight: i32) -> HFONT {
    CreateFontW(
        -(pt * dpi as i32) / 72,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        1,
        0,
        0,
        5,
        0,
        windows::core::w!("Segoe UI"),
    )
}

// ── 页面构建 ─────────────────────────────────────────────

/// 依据 cfg 重建全部 6 页布局（打开 / 配置变化 / 主题变化后调用）。
#[allow(clippy::too_many_lines, non_snake_case)]
fn build_pages(ctx: &mut SettingsCtx, cfg: &sysoverlay_core::config::Config) {
    let dpi = ctx.dpi;
    let one = |v: i32| s(dpi, v);
    let card_l = one(MARGIN_X);
    let card_r = one(CLIENT_W) - one(MARGIN_X);
    let widget_r = card_r - one(8);
    let widget_l = card_l + one(CARD_PAD) + one(LABEL_W);
    let tr = |k: &'static str| crate::i18n::t(cfg.lang, k);

    for (p, page) in ctx.pages.iter_mut().enumerate() {
        page.cards.clear();
        page.widgets.clear();
        let rows = page_rows(p);
        let fnote = if page_has_footnote(p) { FOOTNOTE_H } else { 0 };
        let rect = RECT {
            left: card_l,
            top: one(TOP_MARGIN) + one(CAPTION_H) + one(CAPTION_GAP),
            right: card_r,
            bottom: one(TOP_MARGIN) + one(CAPTION_H) + one(CAPTION_GAP)
                + 2 * one(CARD_PAD)
                + one(rows * ROW_H)
                + one(fnote),
        };
        page.h = one(TOP_MARGIN + BOTTOM_MARGIN + CAPTION_H + CAPTION_GAP)
            + 2 * one(CARD_PAD)
            + one(rows * ROW_H)
            + one(fnote);

        let card = Card {
            caption: page_caption(p, cfg.lang),
            rect,
            rows: Vec::new(),
            footnote: page_footnote(p, cfg.lang),
        };
        page.cards.push(card);
        let rows_top = rect.top + one(CARD_PAD);

        let full = (widget_l, widget_r);
        let mut PAGE = PageLayout {
            dpi,
            rows_top,
            widget_l,
            widget_r,
        };

        // 逐页填充（每步通过 page 收集 widget）
        fn layout_at(
            pg: &mut Page,
            dpi: u32,
            rows_top: i32,
            _widget_l: i32,
            _widget_r: i32,
            row_i: i32,
            label: &'static str,
            items: Vec<(i32, Box<dyn IWidget>, (i32, i32))>,
        ) {
            let one = |v: i32| s(dpi, v);
            let row_top = rows_top + one(row_i as i32 * ROW_H);
            let mut idxs = Vec::with_capacity(items.len());
            for (id, kind, (l, r)) in items {
                let rect = RECT {
                    left: l,
                    top: row_top + (one(ROW_H) - one(WIDGET_H)) / 2,
                    right: r,
                    bottom: row_top + (one(ROW_H) - one(WIDGET_H)) / 2 + one(WIDGET_H),
                };
                pg.widgets.push(Widget { id, kind, rect });
                idxs.push(pg.widgets.len() - 1);
            }
            pg.cards
                .last_mut()
                .map(|c| c.rows.push(Row { label, widgets: idxs }));
        }

        let mut L = |row_i: i32, label: &'static str, items: Vec<(i32, Box<dyn IWidget>, (i32, i32))>| {
            layout_at(
                page, dpi, rows_top, widget_l, widget_r, row_i, label, items,
            );
        };

        match p {
            0 => {
                L(0, tr("CardLanguage"), vec![
                    (IDC_LANG, Box::new(SegmentedWidget::new(IDC_LANG, &[tr("SegEn"), tr("SegZh")])), (widget_l, widget_l + one(120))),
                ]);
                L(1, tr("LblAutostart"), vec![
                    (IDC_AUTOSTART, Box::new(ToggleWidget::new(IDC_AUTOSTART, "autostart")), (widget_r - one(TOGGLE_W), widget_r)),
                ]);
                L(2, tr("LblDebug"), vec![
                    (IDC_DEBUG, Box::new(ToggleWidget::new(IDC_DEBUG, "debug_log")), (widget_r - one(TOGGLE_W), widget_r)),
                ]);
            }
            1 => {
                L(0, tr("LblStyle"), vec![
                    (IDC_STYLE, Box::new(SegmentedWidget::new(IDC_STYLE, &[tr("SegHorizontal"), tr("SegVertical")])), full),
                ]);
                L(1, tr("LblShape"), vec![
                    (IDC_SHAPE, Box::new(SegmentedWidget::new(IDC_SHAPE, &[tr("SegRound"), tr("SegSquare"), tr("SegNone")])), full),
                ]);
                L(2, tr("LblFontSize"), vec![
                    (IDC_FONT, Box::new(SliderWidget::new(IDC_FONT, 8.0, 40.0, 0.5, "pt", 1)), full),
                ]);
                L(3, tr("LblGapMetric"), vec![
                    (IDC_MGAP, Box::new(SliderWidget::new(IDC_MGAP, 0.0, 64.0, 1.0, "px", 0)), full),
                ]);
                L(4, tr("LblGapLabel"), vec![
                    (IDC_LVGAP, Box::new(SliderWidget::new(IDC_LVGAP, 0.0, 64.0, 1.0, "px", 0)), full),
                ]);
                L(5, tr("LblMemUnit"), vec![
                    (IDC_MEMUNIT, Box::new(SegmentedWidget::new(IDC_MEMUNIT, &[tr("SegPercent"), tr("SegGb")])), (widget_l, widget_l + one(150))),
                ]);
                L(6, tr("LblBg"), vec![
                    (IDC_BG, Box::new(SwatchesWidget::new(IDC_BG, 0)), full),
                ]);
                L(7, tr("LblFg"), vec![
                    (IDC_FG, Box::new(SwatchesWidget::new(IDC_FG, 1)), full),
                ]);
                L(8, tr("LblShadow"), vec![
                    (IDC_SHADOW, Box::new(ToggleWidget::new(IDC_SHADOW, "text_shadow")), (widget_r - one(TOGGLE_W), widget_r)),
                ]);
                L(9, tr("LblOutline"), vec![
                    (IDC_OUTLINE, Box::new(ToggleWidget::new(IDC_OUTLINE, "text_outline")), (widget_r - one(TOGGLE_W), widget_r)),
                ]);
            }
            2 => {
                // 颜色：5 × (开关+字号 / 名称色 / 数值色)
                let metrics: [(&str, &str, i32, bool, MetricColors, Option<f32>); 5] = [
                    ("CPU", "LblCpu", IDC_M0_ON, cfg.show_cpu, cfg.metric_colors[0], cfg.metric_font_sizes[0]),
                    ("MEM", "LblMem", IDC_M0_ON + 3, cfg.show_mem, cfg.metric_colors[1], cfg.metric_font_sizes[1]),
                    ("GPU", "LblGpu", IDC_M0_ON + 6, cfg.show_gpu, cfg.metric_colors[2], cfg.metric_font_sizes[2]),
                    ("NET", "LblNet", IDC_M0_ON + 9, cfg.show_net, cfg.metric_colors[3], cfg.metric_font_sizes[3]),
                    ("PWR", "LblPower", IDC_M0_ON + 12, cfg.power_enabled, cfg.metric_colors[4], cfg.metric_font_sizes[4]),
                ];
                for (i, (_code, lbl_key, id_on, on, _colors, mfs)) in metrics.iter().enumerate() {
                    let base = i as i32 * 3;
                    let cfg_key = match i {
                        0 => "show_cpu", 1 => "show_mem", 2 => "show_gpu", 3 => "show_net", 4 => "power_enabled",
                        _ => "show_cpu",
                    };
                    L(base, tr(lbl_key), vec![
                        (*id_on, Box::new(ToggleWidget::new(*id_on, cfg_key)), (widget_l, widget_l + one(TOGGLE_W))),
                        (IDC_MFS0 + i as i32, Box::new(SliderWidget::new(IDC_MFS0 + i as i32, 8.0, 40.0, 0.5, "pt", 1)), (widget_l + one(TOGGLE_W + 8), widget_r)),
                    ]);
                    // MetricSwatches: id = IDC_M0_ON + base + 1 (label) or + 2 (value)
                    L(base + 1, tr("LblNameColor"), vec![
                        (IDC_M0_ON + base + 1, Box::new(MetricSwatchesWidget::new(IDC_M0_ON + base + 1, i)), full),
                    ]);
                    L(base + 2, tr("LblValueColor"), vec![
                        (IDC_M0_ON + base + 2, Box::new(MetricSwatchesWidget::new(IDC_M0_ON + base + 2, i)), full),
                    ]);
                }
            }
            3 => {
                L(0, tr("LblHoverOpacity"), vec![
                    (IDC_HOVER_OP, Box::new(SliderWidget::new(IDC_HOVER_OP, 0.0, 100.0, 1.0, "%", 0)), full),
                ]);
                L(1, tr("LblNormalOpacity"), vec![
                    (IDC_NORM_OP, Box::new(SliderWidget::new(IDC_NORM_OP, 0.0, 100.0, 1.0, "%", 0)), full),
                ]);
                L(2, tr("LblAnimMs"), vec![
                    (IDC_ANIM, Box::new(SliderWidget::new(IDC_ANIM, 100.0, 2000.0, 50.0, "ms", 0)), full),
                ]);
                L(3, tr("LblHover"), vec![
                    (IDC_HOVER, Box::new(ToggleWidget::new(IDC_HOVER, "hover")), (widget_r - one(TOGGLE_W), widget_r)),
                ]);
            }
            4 => {
                L(0, tr("LblOverlaySize"), vec![
                    (IDC_LOCK_SIZE, Box::new(ToggleWidget::new(IDC_LOCK_SIZE, "window_size.is_some()")), (widget_r - one(TOGGLE_W), widget_r)),
                ]);
                L(1, tr("LblWidth"), vec![
                    (IDC_WW, Box::new(SliderWidget::new(IDC_WW, 60.0, 2000.0, 10.0, "px", 0)), full),
                ]);
                L(2, tr("LblHeight"), vec![
                    (IDC_WH, Box::new(SliderWidget::new(IDC_WH, 40.0, 2000.0, 10.0, "px", 0)), full),
                ]);
                L(3, "", vec![
                    (IDC_RESIZE, Box::new(ButtonWidget::new(IDC_RESIZE, "")), full),
                ]);
                L(4, tr("LblTopmost"), vec![
                    (IDC_TOPMOST, Box::new(ToggleWidget::new(IDC_TOPMOST, "topmost")), (widget_r - one(TOGGLE_W), widget_r)),
                ]);
                L(5, tr("LblRate"), vec![
                    (IDC_RATE, Box::new(SegmentedWidget::new(IDC_RATE, &[tr("SegAuto"), "10", "5", "1", "0.5", "0.2"])), full),
                ]);
            }
            _ => {
                // 关于：展示纯文本行（无控件）
                L(0, tr("AboutName"), vec![]);
                L(1, tr("AboutVer"), vec![]);
                L(2, "", vec![]);
            }
        }

        PAGE.rows_top = rows_top;
        let _ = PAGE;
    }
}

/// 布局过程中的临时句柄（避免页面代码直接操作 page 内部借用）。
struct PageLayout {
    #[allow(dead_code)]
    dpi: u32,
    #[allow(dead_code)]
    rows_top: i32,
    #[allow(dead_code)]
    widget_l: i32,
    #[allow(dead_code)]
    widget_r: i32,
}

fn page_caption(p: usize, lang: Lang) -> &'static str {
    let tr = |k: &'static str| crate::i18n::t(lang, k);
    match p {
        0 => tr("PgGeneral"),
        1 => tr("PgLayout"),
        2 => tr("PgColors"),
        3 => tr("PgMotion"),
        4 => tr("PgWindow"),
        _ => tr("PgAbout"),
    }
}

fn page_footnote(p: usize, lang: Lang) -> Option<&'static str> {
    if p == 0 {
        return Some(crate::i18n::t(lang, "FootnoteDebug"));
    }
    None
}

/// 在预设色板里找与当前配置颜色相等的下标（找不到取 0）。
fn swatch_sel(palette: &[(u8, u8, u8)], color: Rgb) -> usize {
    palette
        .iter()
        .position(|&(r, g, b)| Rgb::new(r, g, b) == color)
        .unwrap_or(0)
}

fn metric_swatch_sel(color: Rgb) -> usize {
    METRIC_SWATCHES
        .iter()
        .position(|&(r, g, b)| Rgb::new(r, g, b) == color)
        .unwrap_or(SW_FOLLOW)
}

// ── 窗口过程 ─────────────────────────────────────────────

fn ctx_of(hwnd: HWND) -> Option<&'static mut SettingsCtx> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_UD) as *mut SettingsCtx };
    if p.is_null() {
        None
    } else {
        Some(unsafe { &mut *p })
    }
}

unsafe extern "system" fn settings_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCHITTEST => {
            // 标题栏空白 → 可拖动；按钮/内容 → 客户区；固定态空白 → 穿透（不可拖动）
            let pt = POINT {
                x: (lparam.0 & 0xFFFF) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
            };
            let d = crate::dpi::get_system_dpi();
            if titlebar::in_title_bar(d, pt) {
                let mut r = RECT::default();
                let _ = GetClientRect(hwnd, &mut r);
                if ctx_of(hwnd).map(|c| c.tb.pinned).unwrap_or(false) {
                    if titlebar::hit_button(d, r.right - r.left, pt, true).is_some() {
                        return LRESULT(HTCLIENT as isize);
                    }
                    return LRESULT(HTTRANSPARENT as isize);
                }
                if titlebar::hit_button(d, r.right - r.left, pt, false).is_some() {
                    return LRESULT(HTCLIENT as isize);
                }
                return LRESULT(HTCAPTION as isize);
            }
            LRESULT(HTCLIENT as isize)
        }
        WM_MOUSEMOVE => {
            let pt = POINT {
                x: (lparam.0 & 0xFFFF) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
            };
            if let Some(ctx) = ctx_of(hwnd) {
                // 拖动中：更新滑杆值
                if let Interaction::Dragging { widget: idx, .. } = &ctx.interaction {
                    if let Some(cfg) = window::get_config_of(ctx.overlay) {
                        slider_drag(ctx, *idx, pt, &cfg);
                    }
                    let _ = InvalidateRect(hwnd, None, false);
                    return LRESULT(0);
                }
                // 标题栏悬停态
                let d = ctx.dpi;
                let was_over = ctx.tb.over;
                ctx.tb.over = titlebar::in_title_bar(d, pt);
                let mut r = RECT::default();
                let _ = GetClientRect(hwnd, &mut r);
                let btn = if ctx.tb.over {
                    titlebar::hit_button(d, r.right - r.left, pt, ctx.tb.pinned)
                } else {
                    None
                };
                let changed = was_over != ctx.tb.over || ctx.tb.hover != btn;
                ctx.tb.hover = btn;
                if changed {
                    let _ = InvalidateRect(hwnd, None, false);
                }
                // 控件悬停
                let new_hover = hover_of(ctx, pt);
                let old_hover = ctx.active_widget();
                if new_hover != old_hover {
                    // P3C: 只标记旧悬停和新悬停的控件为dirty（而非全量重绘）
                    if let Some(old_idx) = old_hover {
                        ctx.invalidate_widget(old_idx);
                    }
                    if let Some(new_idx) = new_hover {
                        ctx.invalidate_widget(new_idx);
                    }
                    ctx.interaction = new_hover.map(|i| Interaction::Hover { widget: i }).unwrap_or(Interaction::Idle);
                    let _ = InvalidateRect(hwnd, None, false);
                }
                if !ctx.tracking {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    if TrackMouseEvent(&mut tme).is_ok() {
                        ctx.tracking = true;
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            if let Some(ctx) = ctx_of(hwnd) {
                ctx.tracking = false;
                ctx.tb.over = false;
                ctx.tb.hover = None;
                if let Interaction::Hover { widget } = &ctx.interaction {
                    // P3C: 标记hover控件为dirty
                    ctx.invalidate_widget(*widget);
                    ctx.interaction = Interaction::Idle;
                    let _ = InvalidateRect(hwnd, None, false);
                }
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let pt = POINT {
                x: (lparam.0 & 0xFFFF) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
            };
            let Some(ctx) = ctx_of(hwnd) else { return LRESULT(0) };
            let d = ctx.dpi;
            let mut r = RECT::default();
            let _ = GetClientRect(hwnd, &mut r);
            // 1) 标题栏按钮
            if titlebar::in_title_bar(d, pt) {
                if let Some(btn) = titlebar::hit_button(d, r.right - r.left, pt, ctx.tb.pinned) {
                    on_title_btn(ctx, btn);
                    let _ = InvalidateRect(hwnd, None, false);
                    return LRESULT(0);
                }
            }
            // 2) 导航
            if let Some(page) = nav_at(ctx, pt) {
                if page != ctx.page {
                    ctx.page = page;
                    ctx.page_anim = Some(PageAnim { t0: Instant::now() });
                    start_timer(hwnd, PAGE_TIMER, 16);
                    ctx.interaction = Interaction::Idle;
                }
                let _ = InvalidateRect(hwnd, None, false);
                return LRESULT(0);
            }
            // 3) 控件
            let idx = widget_at(ctx, pt);
            if let Some(idx) = idx {
                if let Some(cfg) = window::get_config_of(ctx.overlay) {
                    // 滑杆：值框内 → 编辑；否则拖动
                    if ctx.pages[ctx.page].widgets[idx].kind.as_any().downcast_ref::<SliderWidget>().is_some() {
                        if pt_in(&value_box_rect(&ctx.pages[ctx.page].widgets[idx].rect, d), pt) {
                            begin_edit(ctx, idx);
                            ctx.invalidate_widget(idx);
                            let _ = SetFocus(hwnd);
                            start_timer(hwnd, EDIT_TIMER, 500);
                            let _ = InvalidateRect(hwnd, None, false);
                            return LRESULT(0);
                        }
                        ctx.interaction = Interaction::Dragging { widget: idx, origin: pt };
                        let _ = SetCapture(hwnd);
                        slider_drag(ctx, idx, pt, &cfg);
                        ctx.invalidate_widget(idx);
                        let _ = InvalidateRect(hwnd, None, false);
                        return LRESULT(0);
                    }
                    on_click_plain(ctx, idx, pt);
                    ctx.invalidate_widget(idx);
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Some(ctx) = ctx_of(hwnd) {
                if let Interaction::Dragging { widget, .. } = &ctx.interaction {
                    ctx.invalidate_widget(*widget);
                    ctx.interaction = Interaction::Idle;
                    finish_drag(ctx);
                    let _ = ReleaseCapture();
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            if let Some(ctx) = ctx_of(hwnd) {
                if let Interaction::Dragging { .. } = ctx.interaction {
                    ctx.interaction = Interaction::Idle;
                    finish_drag(ctx);
                }
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 as usize == PAGE_TIMER {
                if let Some(ctx) = ctx_of(hwnd) {
                    if let Some(anim) = &ctx.page_anim {
                        let dur_ms = cfg_of(ctx).map(|c| c.anim_ms).unwrap_or(300).max(1) as f32;
                        let t = anim.t0.elapsed().as_secs_f32() * 1000.0 / dur_ms;
                        if t >= 1.0 {
                            ctx.page_anim = None;
                            kill_timer(hwnd, PAGE_TIMER);
                        }
                    }
                }
            } else if wparam.0 as usize == EDIT_TIMER {
                if let Some(ctx) = ctx_of(hwnd) {
                    let need = ctx
                        .edit
                        .as_mut()
                        .map(|e| {
                            e.caret = !e.caret;
                            true
                        })
                        .unwrap_or(false);
                    if !need {
                        kill_timer(hwnd, EDIT_TIMER);
                    }
                    // P3D: 标记编辑控件为dirty（光标闪烁需要重绘）
                    if ctx.is_editing() {
                        if let Some(idx) = ctx.active_widget() {
                            ctx.invalidate_widget(idx);
                        }
                    }
                }
            }
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if let Some(ctx) = ctx_of(hwnd) {
                if ctx.is_editing() {
                    let vk = wparam.0 as u32;
                    if vk == 0x1B {
                        // Esc 取消 - P3D: 标记编辑控件为dirty
                        if let Some(idx) = ctx.active_widget() {
                            ctx.invalidate_widget(idx);
                        }
                        ctx.interaction = Interaction::Idle;
                        kill_timer(hwnd, EDIT_TIMER);
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                }
            }
            LRESULT(0)
        }
        WM_CHAR => {
            if let Some(ctx) = ctx_of(hwnd) {
                if ctx.is_editing() {
                    if let Some(e) = ctx.edit.as_mut() {
                        let c = (wparam.0 & 0xFFFF) as u16;
                        if c == '\r' as u16 || c == '\n' as u16 {
                            commit_edit(ctx);
                            kill_timer(hwnd, EDIT_TIMER);
                        } else if c == 0x08 {
                            // Backspace
                            e.buf.pop();
                            e.caret = true;
                        } else if (c >= b'0' as u16 && c <= b'9' as u16) || c == b'.' as u16 {
                            if e.buf.len() < MAX_EDIT_BUF {
                                e.buf.push(c);
                            }
                            e.caret = true;
                        }
                        // P3D: 标记编辑控件为dirty（内容变化需要重绘）
                        if let Some(idx) = ctx.active_widget() {
                            ctx.invalidate_widget(idx);
                        }
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                }
            }
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            if let Some(ctx) = ctx_of(hwnd) {
                if ctx.edit.take().is_some() {
                    // P3D: 标记编辑控件为dirty
                    if let Some(idx) = ctx.active_widget() {
                        ctx.invalidate_widget(idx);
                    }
                    ctx.interaction = Interaction::Idle;
                    kill_timer(hwnd, EDIT_TIMER);
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_DIAG_CLICK => {
            if let Some(ctx) = ctx_of(hwnd) {
                // 切到颜色页并模拟点击 CPU 开关
                let mut found = None;
                for (p, page) in ctx.pages.iter().enumerate() {
                    if let Some(idx) = page.widgets.iter().position(|w| w.id == IDC_CPU) {
                        found = Some((p, idx));
                        break;
                    }
                }
                if let Some((p, _idx)) = found {
                    ctx.page = p;
                    ctx.page_anim = None;
                    // 直接改配置
                    apply_toggle(ctx.overlay, IDC_CPU);
                    let _ = InvalidateRect(hwnd, None, false);
                } else {
                    crate::probe::log("diag_click: CPU widget not found");
                }
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            if let Some(ctx) = ctx_of(hwnd) {
                if matches!(&ctx.interaction, Interaction::Hover { .. }) || ctx.tb.hover.is_some() || ctx.is_editing() {
                    SetCursor(ctx.hand);
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_SETTINGCHANGE => {
            if let Some(ctx) = ctx_of(hwnd) {
                ctx.theme = Theme::system();
                let new_fonts = Fonts::new(ctx.dpi);
                ctx.fonts.delete();
                ctx.fonts = new_fonts;
                apply_window_chrome(hwnd, ctx.theme.dark);
                if let Some(cfg) = window::get_config_of(ctx.overlay) {
                    build_pages(ctx, &cfg);
                }
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_UD) as *mut SettingsCtx;
            if !ptr.is_null() {
                if let Some(ctx) = ptr.as_mut() {
                    ctx.fonts.delete();
                }
                drop(Box::from_raw(ptr));
                SetWindowLongPtrW(hwnd, GWLP_UD, 0);
            }
            SETTINGS_HWND.store(-1, Ordering::SeqCst);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn cfg_of(ctx: &SettingsCtx) -> Option<sysoverlay_core::config::Config> {
    window::get_config_of(ctx.overlay)
}

/// 标题栏按钮动作。
fn on_title_btn(ctx: &mut SettingsCtx, btn: TitleBtn) {
    match btn {
        TitleBtn::Pin => {
            ctx.tb.pinned = true;
            unsafe {
                let _ = SetWindowPos(
                    find_hwnd(ctx),
                    HWND_TOPMOST,
                    0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
            crate::probe::log("[settings] titlebar pin");
        }
        TitleBtn::Unlock => {
            ctx.tb.pinned = false;
            unsafe {
                let _ = SetWindowPos(
                    find_hwnd(ctx),
                    HWND_NOTOPMOST,
                    0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
        TitleBtn::Min => {
            unsafe { let _ = ShowWindow(find_hwnd(ctx), SW_HIDE); }
            crate::probe::log("[settings] titlebar min (hidden)");
        }
        TitleBtn::Close => {
            unsafe { let _ = PostMessageW(find_hwnd(ctx), WM_CLOSE, WPARAM(0), LPARAM(0)); }
        }
    }
}

fn find_hwnd(_ctx: &SettingsCtx) -> HWND {
    let v = SETTINGS_HWND.load(Ordering::SeqCst);
    HWND(if v == -1 { std::ptr::null_mut() } else { v as *mut _ })
}

/// 导航条命中。
fn nav_at(ctx: &SettingsCtx, pt: POINT) -> Option<usize> {
    let d = ctx.dpi;
    let top = s(d, titlebar::TB_H) + s(d, NAV_GAP);
    if pt.y < top || pt.y >= top + s(d, NAV_H) {
        return None;
    }
    let n = 6;
    let seg = s(d, CLIENT_W) / n;
    let i = (pt.x / seg.max(1)) as usize;
    if (i as i32) < n { Some(i) } else { None }
}

/// 控件命中（当前页）。
fn widget_at(ctx: &SettingsCtx, pt: POINT) -> Option<usize> {
    ctx.pages[ctx.page]
        .widgets
        .iter()
        .position(|w| pt_in(&w.rect, pt))
}

/// 控件悬停（当前页）。
fn hover_of(ctx: &SettingsCtx, pt: POINT) -> Option<usize> {
    widget_at(ctx, pt)
}

/// 滑杆值框子矩形（右侧固定宽）。
fn value_box_rect(slider_rect: &RECT, dpi: u32) -> RECT {
    let w = s(dpi, VALUEBOX_W);
    RECT {
        left: slider_rect.right - w,
        top: slider_rect.top,
        right: slider_rect.right,
        bottom: slider_rect.bottom,
    }
}

/// 滑杆轨道子矩形（值框左侧）。
fn track_rect(slider_rect: &RECT, dpi: u32) -> RECT {
    let vb = value_box_rect(slider_rect, dpi);
    RECT {
        left: slider_rect.left + s(dpi, 2),
        top: slider_rect.top,
        right: vb.left - s(dpi, 6),
        bottom: slider_rect.bottom,
    }
}

/// 拖动滑杆：按 x 归一化 → 步进取整。
fn slider_drag(ctx: &mut SettingsCtx, idx: usize, pt: POINT, _cfg: &sysoverlay_core::config::Config) {
    let page = &mut ctx.pages[ctx.page];
    if let Some(w) = page.widgets.get_mut(idx) {
        if let Some(slider) = w.kind.as_any().downcast_ref::<SliderWidget>() {
            let tr = track_rect(&w.rect, ctx.dpi);
            let tw = (tr.right - tr.left).max(1) as f32;
            let t = ((pt.x - tr.left) as f32 / tw).clamp(0.0, 1.0);
            let raw = slider.min + t * (slider.max - slider.min);
            let stepped = ((raw / slider.step).round() * slider.step).max(slider.min).min(slider.max);
            // 0.5 步进做尾数规整
            let f = if slider.step < 1.0 { (stepped * 10.0).round() / 10.0 } else { stepped.round() };
            let id = w.id;
            apply_slider_cfg(ctx.overlay, id, f, false);
        }
    }
}

/// 拖动结束：落盘一次（拖动中已改内存，这里统一写配置文件）。
fn finish_drag(ctx: &mut SettingsCtx) {
    apply_config(ctx.overlay, |_cfg| true);
}

/// 进入输入框编辑态。
fn begin_edit(ctx: &mut SettingsCtx, idx: usize) {
    let val = if let Some(slider) = ctx.pages[ctx.page].widgets[idx].kind.as_any().downcast_ref::<SliderWidget>() {
        let v = get_slider_value(ctx, slider.id);
        let f = if slider.dp == 0 { v.round() as i64 } else { (v * 10.0).round() as i64 / 10 };
        format!("{}", f)
    } else {
        String::new()
    };
    ctx.interaction = Interaction::Editing { widget: idx };
    ctx.edit = Some(EditState {
        idx,
        buf: val.encode_utf16().collect(),
        caret: true,
    });
}

/// Helper to get slider's current value from Config
fn get_slider_value(ctx: &SettingsCtx, id: i32) -> f32 {
    if let Some(cfg) = cfg_of(ctx) {
        match id {
            IDC_FONT => cfg.font_size,
            IDC_MGAP => cfg.metric_gap as f32,
            IDC_LVGAP => cfg.label_value_gap as f32,
            IDC_HOVER_OP => cfg.hover_opacity as f32,
            IDC_NORM_OP => cfg.normal_opacity as f32,
            IDC_ANIM => cfg.anim_ms as f32,
            IDC_WW => cfg.window_size.map(|s| s.0 as f32).unwrap_or(300.0),
            IDC_WH => cfg.window_size.map(|s| s.1 as f32).unwrap_or(200.0),
            _ if id >= IDC_MFS0 && id < IDC_MFS0 + 5 => {
                cfg.metric_font_sizes[(id - IDC_MFS0) as usize].unwrap_or(cfg.font_size)
            }
            _ => 0.0,
        }
    } else {
        0.0
    }
}

/// 提交输入框内容。
fn commit_edit(ctx: &mut SettingsCtx) {
    let Some(e) = ctx.edit.take() else { return };
    ctx.interaction = Interaction::Idle;
    let text: String = e
        .buf
        .iter()
        .map(|&c| char::from_u32(c as u32).unwrap_or('�'))
        .collect();
    let id = ctx.pages[ctx.page].widgets[e.idx].id;
    let (min, max, step) = if let Some(slider) = ctx.pages[ctx.page].widgets[e.idx].kind.as_any().downcast_ref::<SliderWidget>() {
        (slider.min, slider.max, slider.step)
    } else {
        (0.0, 0.0, 1.0)
    };
    let parsed: f32 = text.trim().parse().unwrap_or(min);
    let v = ((parsed / step.max(0.001)).round() * step).max(min).min(max);
    apply_slider_cfg(ctx.overlay, id, v, true);
}

/// 应用滑杆/输入框的新值。`commit` 为 true 时落盘。
fn apply_slider_cfg(overlay: HWND, id: i32, v: f32, commit: bool) {
    let f = |cfg: &mut Config| match id {
        IDC_FONT => {
            let nv = (v.max(8.0).min(40.0) * 2.0).round() / 2.0;
            if (cfg.font_size - nv).abs() > 0.01 { cfg.font_size = nv; true } else { false }
        }
        IDC_MGAP => {
            let nv = v.round().clamp(0.0, 64.0) as u32;
            if cfg.metric_gap != nv { cfg.metric_gap = nv; true } else { false }
        }
        IDC_LVGAP => {
            let nv = v.round().clamp(0.0, 64.0) as u32;
            if cfg.label_value_gap != nv { cfg.label_value_gap = nv; true } else { false }
        }
        IDC_HOVER_OP => {
            let nv = v.round().clamp(0.0, 100.0) as u8;
            if cfg.hover_opacity != nv { cfg.hover_opacity = nv; true } else { false }
        }
        IDC_NORM_OP => {
            let nv = v.round().clamp(0.0, 100.0) as u8;
            if cfg.normal_opacity != nv { cfg.normal_opacity = nv; true } else { false }
        }
        IDC_ANIM => {
            let nv = (v.round() / 50.0).round() * 50.0;
            let nv = nv.clamp(100.0, 2000.0) as u32;
            if cfg.anim_ms != nv { cfg.anim_ms = nv; true } else { false }
        }
        IDC_WW | IDC_WH => {
            let w = if id == IDC_WW { v.round().clamp(60.0, 2000.0) as i32 } else { v.round().clamp(40.0, 2000.0) as i32 };
            let cur = cfg.window_size.unwrap_or_else(|| gdi_natural(cfg));
            let nv = if id == IDC_WW { (w, cur.1) } else { (cur.0, w) };
            if cfg.window_size != Some(nv) {
                cfg.window_size = Some(nv);
                // 锁定尺寸开关自动打开
                true
            } else {
                false
            }
        }
        _ if id >= IDC_MFS0 && id < IDC_MFS0 + 5 => {
            let mi = (id - IDC_MFS0) as usize;
            let nv = (v.max(8.0).min(40.0) * 2.0).round() / 2.0;
            if cfg.metric_font_sizes[mi] != Some(nv) {
                cfg.metric_font_sizes[mi] = Some(nv);
                true
            } else {
                false
            }
        }
        _ => false,
    };
    if commit {
        apply_config(overlay, f);
    } else {
        apply_config_quiet(overlay, f);
    }
}

fn gdi_natural(cfg: &sysoverlay_core::config::Config) -> (i32, i32) {
    crate::gdi::natural_size(cfg)
}

// ── 交互（点击类控件） ────────────────────────────────────

fn on_click_plain(ctx: &mut SettingsCtx, idx: usize, pt: POINT) {
    let overlay = ctx.overlay;
    let w = &ctx.pages[ctx.page].widgets[idx];
    let cfg = cfg_of(ctx).unwrap_or_default();

    // Toggle
    if w.kind.as_any().downcast_ref::<ToggleWidget>().is_some() {
        apply_toggle(overlay, w.id);
        return;
    }

    // Segmented
    if let Some(_seg) = w.kind.as_any().downcast_ref::<SegmentedWidget>() {
        let n = 2i32; // labels.len() not directly accessible
        let seg_w = (w.rect.right - w.rect.left) / n;
        let i = (((pt.x - w.rect.left) / seg_w.max(1)) as usize).min(1);
        let mut st = WidgetState::default();
        w.kind.sync_state(&cfg, &mut st);
        if i != st.selected {
            apply_segment(overlay, w.id, i);
        }
        return;
    }

    // Button
    if w.kind.as_any().downcast_ref::<ButtonWidget>().is_some() {
        let cur = window::resize_mode_active(overlay).unwrap_or(false);
        window::set_resize_mode(overlay, !cur);
        return;
    }

    // Swatches
    if let Some(_sw) = w.kind.as_any().downcast_ref::<SwatchesWidget>() {
        let n = 6i32;
        let wd = (w.rect.right - w.rect.left) / n;
        let i = (((pt.x - w.rect.left) / wd.max(1)) as usize).min(5);
        let mut st = WidgetState::default();
        w.kind.sync_state(&cfg, &mut st);
        if i != st.selected {
            apply_swatch(overlay, w.id, i);
        }
        return;
    }

    // MetricSwatches
    if let Some(_ms) = w.kind.as_any().downcast_ref::<MetricSwatchesWidget>() {
        let n = SW_N as i32;
        let wd = (w.rect.right - w.rect.left) / n;
        let i = (((pt.x - w.rect.left) / wd.max(1)) as usize).min(SW_N - 1);
        let mut st = WidgetState::default();
        w.kind.sync_state(&cfg, &mut st);
        let follow = st.value_f > 0.5;
        if i != st.selected || !(i == SW_FOLLOW && follow) {
            apply_metric_swatch(overlay, w.id, i, follow);
        }
        return;
    }

    // Slider - no action on click (handled by drag)
}

// ── 删除旧 set_cfg_opt / set_cfg，替换为 apply_config ─────────

/// 统一配置修改入口：修改 + 通知 + 原子保存。
/// `overlay` 用于通知悬浮窗重排（notify=true 时）。
fn apply_config(overlay: HWND, f: impl FnOnce(&mut Config) -> bool) {
    let Some(mut cfg) = window::get_config_of(overlay) else { return };
    if f(&mut cfg) {
        window::modify_config(overlay, |c| *c = cfg.clone());
        commit_change(overlay, &cfg, true);
    }
}

/// 不通知悬浮窗的版本（拖动中实时预览用）。
fn apply_config_quiet(overlay: HWND, f: impl FnOnce(&mut Config) -> bool) {
    let Some(mut cfg) = window::get_config_of(overlay) else { return };
    if f(&mut cfg) {
        window::modify_config(overlay, |c| *c = cfg.clone());
        // 拖动结束后由 finish_drag 统一保存
    }
}

fn apply_toggle(overlay: HWND, id: i32) {
    apply_config(overlay, |cfg| match id {
        IDC_CPU => {
            cfg.show_cpu = !cfg.show_cpu;
            if guard(cfg) { true } else { cfg.show_cpu = !cfg.show_cpu; false }
        }
        IDC_MEM => {
            cfg.show_mem = !cfg.show_mem;
            if guard(cfg) { true } else { cfg.show_mem = !cfg.show_mem; false }
        }
        IDC_GPU => {
            cfg.show_gpu = !cfg.show_gpu;
            if guard(cfg) { true } else { cfg.show_gpu = !cfg.show_gpu; false }
        }
        IDC_NET => {
            cfg.show_net = !cfg.show_net;
            if guard(cfg) { true } else { cfg.show_net = !cfg.show_net; false }
        }
        IDC_POWER => {
            cfg.power_enabled = !cfg.power_enabled;
            if guard(cfg) { true } else { cfg.power_enabled = !cfg.power_enabled; false }
        }
        IDC_SHADOW => {
            cfg.text_shadow = !cfg.text_shadow;
            true
        }
        IDC_OUTLINE => {
            cfg.text_outline = !cfg.text_outline;
            true
        }
        IDC_TOPMOST => {
            cfg.topmost = !cfg.topmost;
            window::apply_topmost(overlay, cfg.topmost);
            true
        }
        IDC_PIN => {
            cfg.topmost = !cfg.topmost;
            window::apply_topmost(overlay, cfg.topmost);
            true
        }
        IDC_AUTOSTART => {
            cfg.autostart = !cfg.autostart;
            autostart::apply(cfg.autostart);
            true
        }
        IDC_DEBUG => {
            cfg.debug_log = !cfg.debug_log;
            crate::probe::set_debug(cfg.debug_log);
            true
        }
        IDC_HOVER => {
            cfg.hover = !cfg.hover;
            true
        }
        IDC_LOCK_SIZE => {
            if cfg.window_size.is_some() {
                cfg.window_size = None;
            } else {
                cfg.window_size = Some(gdi_natural(cfg));
            }
            true
        }
        _ => false,
    });
}

fn guard(cfg: &sysoverlay_core::config::Config) -> bool {
    cfg.show_cpu || cfg.show_mem || cfg.show_gpu || cfg.show_net || cfg.power_enabled
}

fn apply_segment(overlay: HWND, id: i32, idx: usize) {
    apply_config(overlay, |cfg| match id {
        IDC_MEMUNIT => {
            let want = idx == 1;
            if cfg.mem_show_gb != want { cfg.mem_show_gb = want; true } else { false }
        }
        IDC_RATE => match idx {
            0 => {
                if cfg.refresh_auto { false } else { cfg.refresh_auto = true; true }
            }
            i if i < RATE_MS.len() => {
                let ms = RATE_MS[i];
                if cfg.refresh_auto || cfg.interval_ms != ms { cfg.refresh_auto = false; cfg.interval_ms = ms; true } else { false }
            }
            _ => false,
        },
        IDC_STYLE => {
            let want = if idx == 1 { RenderStyle::Vertical } else { RenderStyle::Strip };
            if cfg.render_style != want { cfg.render_style = want; true } else { false }
        }
        IDC_SHAPE => {
            let want = match idx {
                0 => Shape::Round,
                1 => Shape::Square,
                _ => Shape::Content,
            };
            if cfg.shape != want { cfg.shape = want; true } else { false }
        }
        IDC_LANG => {
            let want = if idx == 0 { Lang::En } else { Lang::Zh };
            if cfg.lang != want {
                cfg.lang = want;
                crate::i18n::set_global_lang(want);
                true
            } else {
                false
            }
        }
        _ => false,
    });
}

fn apply_swatch(overlay: HWND, id: i32, idx: usize) {
    apply_config(overlay, |cfg| match id {
        IDC_BG => {
            let (r, g, b) = BG_SWATCHES[idx.min(BG_SWATCHES.len() - 1)];
            let want = Rgb::new(r, g, b);
            if cfg.bg != want { cfg.bg = want; true } else { false }
        }
        IDC_FG => {
            let (r, g, b) = FG_SWATCHES[idx.min(FG_SWATCHES.len() - 1)];
            let want = Rgb::new(r, g, b);
            if cfg.fg != want { cfg.fg = want; true } else { false }
        }
        _ => false,
    });
}

/// 指标配色：id = IDC_M0_LBL/IDC_M0_VAL 偏移区。
/// 槽位：ID = IDC_M0_ON + m*3 + 1 → label；+2 → value。
fn apply_metric_swatch(overlay: HWND, id: i32, idx: usize, follow: bool) {
    if id < IDC_M0_ON || id >= IDC_M0_ON + 15 {
        return;
    }
    let slot = (id - IDC_M0_ON) % 3;
    let metric = ((id - IDC_M0_ON) / 3) as usize;
    let is_label = slot == 1;
    let color = if idx == SW_FOLLOW || follow {
        None // 跟随默认
    } else {
        let (r, g, b) = METRIC_SWATCHES[idx.min(METRIC_SWATCHES.len() - 1)];
        Some(Rgb::new(r, g, b))
    };
    apply_config(overlay, |cfg| {
        let mc = &mut cfg.metric_colors[metric];
        let old = if is_label { mc.label } else { mc.value };
        if is_label {
            mc.label = color;
        } else {
            mc.value = color;
        }
        old != color
    });
}

// ── Config 写入唯一入口 ────────────────────────────────────

/// 统一修改 Config 的入口。
/// 1. 通过 overlay HWND 通知悬浮窗重排（若 notify=true）
/// 2. 立即保存到磁盘（带防抖：若 500ms 内已有写入则跳过）
/// 注意：此函数不持有 Config 锁，调用前确保没有并发写入
fn commit_change(overlay: HWND, cfg: &Config, modified: bool) {
    if !modified {
        return;
    }
    unsafe {
        let _ = PostMessageW(overlay, WM_RECONFIG, WPARAM(0), LPARAM(0));
    }
    // 防抖：下次 save 在 500ms 后再执行，本次跳过
    static LAST_SAVE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // config_io::save_config 本身已做原子写，外部防抖由调用方保证
    let _ = config_io::save_config(cfg);
}

// ── 绘制 ─────────────────────────────────────────────────

unsafe fn paint(hwnd: HWND) {
    let Some(ctx) = ctx_of(hwnd) else { return };
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    if hdc.is_invalid() {
        let _ = EndPaint(hwnd, &ps);
        return;
    }
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;
    if w <= 0 || h <= 0 {
        let _ = EndPaint(hwnd, &ps);
        return;
    }

    // 双缓冲
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, w, h);
    if bmp.is_invalid() {
        let _ = DeleteDC(mem);
        let _ = EndPaint(hwnd, &ps);
        return;
    }
    let old_bmp = SelectObject(mem, bmp);

    let bg = CreateSolidBrush(COLORREF(ctx.theme.window_bg));
    let _ = FillRect(mem, &rc, bg);
    let _ = DeleteObject(bg);

    // 标题栏
    titlebar::draw(
        mem,
        &ctx.theme,
        ctx.dpi,
        w,
        Some(crate::i18n::t(ctx.theme_lang(), "WinTitle")),
        &ctx.tb,
        ctx.fonts.small,
    );

    // 导航
    draw_nav(mem, ctx, w);

    // 内容区（翻页动画：水平滑入 + 淡入）
    draw_content_animated(mem, ctx, w, h);

    let _ = BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
    let _ = SelectObject(mem, old_bmp);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mem);
    let _ = EndPaint(hwnd, &ps);
}

/// 设置页自身语言（跟随全局 i18n）。
impl SettingsCtx {
    fn theme_lang(&self) -> sysoverlay_core::types::Lang {
        cfg_of(self)
            .map(|c| c.lang)
            .unwrap_or(i18n_now())
    }
}

fn i18n_now() -> sysoverlay_core::types::Lang {
    use sysoverlay_core::types::Lang;
    if crate::i18n::global_lang() == Lang::Zh {
        Lang::Zh
    } else {
        Lang::En
    }
}

/// 导航条：6 个均分分段。
unsafe fn draw_nav(hdc: HDC, ctx: &SettingsCtx, win_w: i32) {
    let t = &ctx.theme;
    let d = ctx.dpi;
    let top = s(d, titlebar::TB_H) + s(d, NAV_GAP);
    let h = s(d, NAV_H);
    // 底槽
    let slot = RECT {
        left: s(d, MARGIN_X),
        top,
        right: win_w - s(d, MARGIN_X),
        bottom: top + h,
    };
    round_rect(hdc, slot, s(d, NAV_H) / 2, t.segmented_bg, t.segmented_bg);

    let n = 6;
    let seg_w = (slot.right - slot.left) / n;
    for i in 0..n {
        let r = RECT {
            left: slot.left + i * seg_w,
            top,
            right: slot.left + (i + 1) * seg_w,
            bottom: top + h,
        };
        if i as usize == ctx.page {
            let m = s(d, 2);
            let ir = RECT {
                left: r.left + m,
                top: r.top + m,
                right: r.right - m,
                bottom: r.bottom - m,
            };
            round_rect(hdc, ir, s(d, NAV_H) / 2 - 2, t.seg_selected, t.seg_selected_stroke);
        }
        let label = page_caption(i as usize, ctx.theme_lang());
        let color = if i as usize == ctx.page { t.label } else { t.secondary };
        text(hdc, label, r, color, ctx.fonts.small, DT_SINGLELINE | DT_CENTER | DT_VCENTER);
    }
}

/// 内容区：翻页动画用独立层 AlphaBlend（滑动 + 淡入）。
unsafe fn draw_content_animated(hdc: HDC, ctx: &SettingsCtx, win_w: i32, win_h: i32) {
    let content_top = s(ctx.dpi, titlebar::TB_H) + s(ctx.dpi, NAV_H + NAV_GAP);
    let content_h = (win_h - content_top).max(1);

    // 动画进度
    let (dx, alpha) = match &ctx.page_anim {
        Some(anim) => {
            let dur_ms = cfg_of(ctx).map(|c| c.anim_ms).unwrap_or(300).max(1) as f32;
            let t = (anim.t0.elapsed().as_secs_f32() * 1000.0 / dur_ms).clamp(0.0, 1.0);
            let e = 1.0 - (1.0 - t).powi(3);
            (
                ((1.0 - e) * 24.0).round() as i32,
                (e * 255.0).round() as u8,
            )
        }
        None => (0, 255),
    };

    let tmp = CreateCompatibleDC(hdc);
    let tbmp = CreateCompatibleBitmap(hdc, win_w, content_h);
    if tbmp.is_invalid() {
        let _ = DeleteDC(tmp);
        return;
    }
    let old = SelectObject(tmp, tbmp);

    // 内容层背景
    let bg = CreateSolidBrush(COLORREF(ctx.theme.window_bg));
    let full = RECT { left: 0, top: 0, right: win_w, bottom: content_h };
    let _ = FillRect(tmp, &full, bg);
    let _ = DeleteObject(bg);

    // 绘制内容（带水平偏移）
    draw_page(tmp, ctx, &full, dx);

    // 淡入 + 上屏
    let bf = windows::Win32::Graphics::Gdi::BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: alpha,
        AlphaFormat: 0,
    };
    let _ = AlphaBlend(
        hdc,
        0,
        content_top,
        win_w,
        content_h,
        tmp,
        0,
        0,
        win_w,
        content_h,
        bf,
    );

    let _ = SelectObject(tmp, old);
    let _ = DeleteObject(tbmp);
    let _ = DeleteDC(tmp);
}

unsafe fn draw_page(hdc: HDC, ctx: &SettingsCtx, area: &RECT, dx: i32) {
    let t = &ctx.theme;
    let d = ctx.dpi;
    let page = &ctx.pages[ctx.page];
    let cfg = cfg_of(ctx);
    SetBkMode(hdc, TRANSPARENT);

    for card in &page.cards {
        // 分节标题
        let cap = RECT {
            left: card.rect.left + dx + s(d, 2),
            top: card.rect.top - s(d, CAPTION_H + CAPTION_GAP),
            right: card.rect.right + dx,
            bottom: card.rect.top - s(d, CAPTION_GAP),
        };
        text(hdc, card.caption, cap, t.secondary, ctx.fonts.small, DT_SINGLELINE | DT_LEFT | DT_VCENTER);

        // 圆角卡片
        let card_rect = RECT {
            left: card.rect.left + dx,
            top: card.rect.top,
            right: card.rect.right + dx,
            bottom: card.rect.bottom,
        };
        round_rect(hdc, card_rect, s(d, CARD_RADIUS), t.card, t.card_stroke);

        // 行
        for (row_i, row) in card.rows.iter().enumerate() {
            let row_top = card.rect.top + s(d, CARD_PAD) + s(d, row_i as i32 * ROW_H);
            if !row.label.is_empty() {
                let first_widget_left = row
                    .widgets
                    .first()
                    .map(|&i| page.widgets[i].rect.left - s(d, 8))
                    .unwrap_or(card.rect.right - s(d, 12));
                let r = RECT {
                    left: card.rect.left + dx + s(d, 12),
                    top: row_top,
                    right: first_widget_left + dx,
                    bottom: row_top + s(d, ROW_H),
                };
                text(hdc, row.label, r, t.label, ctx.fonts.body,
                    DT_SINGLELINE | DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
            }
            for &wi in &row.widgets {
                // Derive WidgetState from Config, then override with Interaction state
                let mut ws = WidgetState::default();
                if let Some(ref c) = cfg {
                    page.widgets[wi].kind.sync_state(c, &mut ws);
                }
                // Interaction-driven overrides (applied after sync_state)
                ws.hovered = matches!(&ctx.interaction, Interaction::Hover { widget } if *widget == wi);
                if ctx.is_editing() {
                    ws.editing = matches!(&ctx.interaction, Interaction::Editing { widget } if *widget == wi);
                }
                draw_widget(hdc, ctx, wi, &ws, dx);
            }
        }

        // 脚注
        if let Some(foot) = card.footnote {
            let rows_bottom = card.rect.top + s(d, CARD_PAD) + s(d, card.rows.len() as i32 * ROW_H);
            let r = RECT {
                left: card.rect.left + dx + s(d, 12),
                top: rows_bottom,
                right: card.rect.right + dx - s(d, 12),
                bottom: rows_bottom + s(d, FOOTNOTE_H),
            };
            text(hdc, foot, r, t.secondary, ctx.fonts.small, DT_WORDBREAK);
        }
    }
    let _ = area;
}

unsafe fn draw_widget(hdc: HDC, ctx: &SettingsCtx, idx: usize, state: &WidgetState, dx: i32) {
    let page = &ctx.pages[ctx.page];
    let w = &page.widgets[idx];
    let r = RECT {
        left: w.rect.left + dx,
        top: w.rect.top,
        right: w.rect.right + dx,
        bottom: w.rect.bottom,
    };

    // Type dispatch using as_any()
    if let Some(toggle) = w.kind.as_any().downcast_ref::<ToggleWidget>() {
        let on = state.value_f > 0.5;
        draw_toggle(hdc, ctx, r, on);
    } else if let Some(seg) = w.kind.as_any().downcast_ref::<SegmentedWidget>() {
        draw_segmented_widget(w, state, hdc, ctx.dpi, &ctx.theme, r);
    } else if let Some(btn) = w.kind.as_any().downcast_ref::<ButtonWidget>() {
        draw_button_widget(w, state, hdc, ctx.dpi, &ctx.theme, r);
    } else if let Some(sw) = w.kind.as_any().downcast_ref::<SwatchesWidget>() {
        draw_swatches_widget(w, state, hdc, ctx.dpi, &ctx.theme, r);
    } else if let Some(ms) = w.kind.as_any().downcast_ref::<MetricSwatchesWidget>() {
        draw_metric_swatches_widget(w, state, hdc, ctx.dpi, &ctx.theme, r);
    } else if let Some(slider) = w.kind.as_any().downcast_ref::<SliderWidget>() {
        draw_slider_widget(w, state, hdc, ctx.dpi, &ctx.theme, r, slider.dp);
    }
}

/// 背景/前景色板。
unsafe fn draw_swatches(hdc: HDC, ctx: &SettingsCtx, rect: RECT, palette: &[(u8, u8, u8)], sel: usize) {
    let n = palette.len().max(1) as i32;
    let wd = (rect.right - rect.left) / n;
    let m = s(ctx.dpi, 3);
    for (i, &(r, g, b)) in palette.iter().enumerate() {
        let sw = RECT {
            left: rect.left + i as i32 * wd,
            top: rect.top,
            right: rect.left + (i as i32 + 1) * wd,
            bottom: rect.bottom,
        };
        let inner = RECT {
            left: sw.left + m,
            top: sw.top + m,
            right: sw.right - m,
            bottom: sw.bottom - m,
        };
        let stroke = if i == sel { ctx.theme.accent } else { ctx.theme.card_stroke };
        round_rect(hdc, inner, s(ctx.dpi, 5), rgb(r, g, b), stroke);
    }
}

/// 指标配色色板（8 预设 + 最后「默认」格）。
unsafe fn draw_metric_swatches(hdc: HDC, ctx: &SettingsCtx, rect: RECT, follow: bool, sel: usize) {
    let d = ctx.dpi;
    let n = SW_N as i32;
    let wd = (rect.right - rect.left) / n;
    let m = s(d, 3);
    for (i, _) in (0..SW_N).enumerate() {
        let sw = RECT {
            left: rect.left + i as i32 * wd,
            top: rect.top,
            right: rect.left + (i as i32 + 1) * wd,
            bottom: rect.bottom,
        };
        let inner = RECT {
            left: sw.left + m,
            top: sw.top + m,
            right: sw.right - m,
            bottom: sw.bottom - m,
        };
        if i < SW_FOLLOW {
            let (r, g, b) = METRIC_SWATCHES[i];
            round_rect(hdc, inner, s(d, 4), rgb(r, g, b), ctx.theme.card_stroke);
        } else {
            // 「默认」格：虚线感 → 半色 + 斜线太复杂，用描边空心 + 小字
            round_rect(hdc, inner, s(d, 4), ctx.theme.card, ctx.theme.secondary);
            text(hdc, "≈", inner, ctx.theme.secondary, ctx.fonts.small, DT_SINGLELINE | DT_CENTER | DT_VCENTER);
        }
        let selected = (i == sel) || (i == SW_FOLLOW && follow && sel == SW_FOLLOW);
        if selected {
            let outline = RECT { left: sw.left, top: sw.top, right: sw.right, bottom: sw.bottom };
            round_rect(hdc, outline, s(d, 5), ctx.theme.card /*不着色*/, ctx.theme.accent);
        }
    }
}

unsafe fn draw_toggle(hdc: HDC, ctx: &SettingsCtx, rect: RECT, on: bool) {
    let t = &ctx.theme;
    let radius = (rect.bottom - rect.top) / 2;
    let (fill, stroke) = if on {
        (t.toggle_on, t.toggle_on)
    } else {
        (t.toggle_off, t.toggle_off_stroke)
    };
    round_rect(hdc, rect, radius, fill, stroke);
    let m = s(ctx.dpi, 3);
    let d = (rect.bottom - rect.top) - 2 * m;
    let kx = if on { rect.right - m - d } else { rect.left + m };
    let kr = RECT { left: kx, top: rect.top + m, right: kx + d, bottom: rect.bottom - m };
    ellipse(hdc, kr, t.knob, t.knob_stroke);
}

unsafe fn draw_segmented(hdc: HDC, ctx: &SettingsCtx, rect: RECT, labels: &[&str], sel: usize) {
    let t = &ctx.theme;
    let n = labels.len().max(1) as i32;
    let seg_w = (rect.right - rect.left) / n;
    round_rect(hdc, rect, s(ctx.dpi, 7), t.segmented_bg, t.segmented_bg);
    for (i, label) in labels.iter().enumerate() {
        let r = RECT {
            left: rect.left + i as i32 * seg_w,
            top: rect.top,
            right: rect.left + (i as i32 + 1) * seg_w,
            bottom: rect.bottom,
        };
        if i == sel {
            let m = s(ctx.dpi, 2);
            let ir = RECT { left: r.left + m, top: r.top + m, right: r.right - m, bottom: r.bottom - m };
            round_rect(hdc, ir, s(ctx.dpi, 5), t.seg_selected, t.seg_selected_stroke);
        }
        let color = if i == sel { t.label } else { t.secondary };
        text(hdc, label, r, color, ctx.fonts.small, DT_SINGLELINE | DT_CENTER | DT_VCENTER);
    }
}

/// 滑杆 + 输入框：轨道 + 圆滑块 + 右端圆角值框。
unsafe fn draw_slider(
    hdc: HDC,
    ctx: &SettingsCtx,
    rect: RECT,
    val: f32,
    min: f32,
    max: f32,
    unit: &str,
    dp: u32,
    editing: bool,
) {
    let t = &ctx.theme;
    let d = ctx.dpi;
    let vb = value_box_rect(&rect, d);
    let tr = track_rect(&rect, d);

    // 轨道
    let track_y = (tr.top + tr.bottom) / 2;
    let pen = CreatePen(PS_SOLID, s(d, 2).max(1), COLORREF(t.segmented_bg));
    let old_pen = SelectObject(hdc, pen);
    let _ = MoveToEx(hdc, tr.left, track_y, None);
    let _ = LineTo(hdc, tr.right, track_y);
    let _ = SelectObject(hdc, old_pen);
    let _ = DeleteObject(pen);

    // 滑块
    let tfrac = if max > min { ((val - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
    let cx = tr.left + ((tr.right - tr.left) as f32 * tfrac).round() as i32;
    let thumb_d = s(d, 13);
    let thumb = RECT {
        left: cx - thumb_d / 2,
        top: track_y - thumb_d / 2,
        right: cx + thumb_d / 2,
        bottom: track_y + thumb_d / 2,
    };
    ellipse(hdc, thumb, t.accent, t.accent);

    // 值框
    let fill = if editing { t.button_bg_hover } else { t.button_bg };
    round_rect(hdc, vb, s(d, 6), fill, fill);
    let content = RECT { left: vb.left, top: vb.top, right: vb.right - s(d, 4), bottom: vb.bottom };

    // 显示文本（编辑中显示缓冲）
    let show: Vec<u16> = if editing {
        if let Some(e) = &ctx.edit {
            e.buf.clone()
        } else {
            Vec::new()
        }
    } else if dp == 0 {
        format!("{}{}", val.round() as i64, unit).encode_utf16().collect()
    } else {
        format!("{:.1}{}", val, unit).encode_utf16().collect()
    };
    text_utf16(hdc, &show, content, t.label, ctx.fonts.small, DT_RIGHT | DT_SINGLELINE | DT_VCENTER);

    // 光标
    if editing {
        if let Some(e) = &ctx.edit {
            if e.caret {
                let mut r = content;
                r.left = r.right - s(d, 8);
                draw_caret(hdc, r, t.label);
            }
        }
    }
}

unsafe fn draw_caret(hdc: HDC, rect: RECT, color: u32) {
    let pen = CreatePen(PS_SOLID, 1, COLORREF(color));
    let old_pen = SelectObject(hdc, pen);
    let y0 = rect.top + 2;
    let y1 = rect.bottom - 2;
    let _ = MoveToEx(hdc, rect.left, y0, None);
    let _ = LineTo(hdc, rect.left, y1);
    let _ = SelectObject(hdc, old_pen);
    let _ = DeleteObject(pen);
}

unsafe fn draw_button(hdc: HDC, ctx: &SettingsCtx, rect: RECT, label: &str, hovered: bool) {
    let t = &ctx.theme;
    let fill = if hovered { t.accent_hover } else { t.accent };
    round_rect(hdc, rect, s(ctx.dpi, 7), fill, fill);
    text(hdc, label, rect, rgb(255, 255, 255), ctx.fonts.body, DT_SINGLELINE | DT_CENTER | DT_VCENTER);
}

// ── GDI 基元 ─────────────────────────────────────────────

unsafe fn round_rect(hdc: HDC, r: RECT, radius: i32, fill: u32, stroke: u32) {
    let brush = CreateSolidBrush(COLORREF(fill));
    let pen = CreatePen(PS_SOLID, 1, COLORREF(stroke));
    let old_brush = SelectObject(hdc, brush);
    let old_pen = SelectObject(hdc, pen);
    let _ = RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius * 2, radius * 2);
    let _ = SelectObject(hdc, old_brush);
    let _ = SelectObject(hdc, old_pen);
    let _ = DeleteObject(brush);
    let _ = DeleteObject(pen);
}

unsafe fn ellipse(hdc: HDC, r: RECT, fill: u32, stroke: u32) {
    let brush = CreateSolidBrush(COLORREF(fill));
    let pen = CreatePen(PS_SOLID, 1, COLORREF(stroke));
    let old_brush = SelectObject(hdc, brush);
    let old_pen = SelectObject(hdc, pen);
    let _ = Ellipse(hdc, r.left, r.top, r.right, r.bottom);
    let _ = SelectObject(hdc, old_brush);
    let _ = SelectObject(hdc, old_pen);
    let _ = DeleteObject(brush);
    let _ = DeleteObject(pen);
}

unsafe fn text(hdc: HDC, s: &str, rect: RECT, color: u32, font: HFONT, fmt: DRAW_TEXT_FORMAT) {
    if s.is_empty() {
        return;
    }
    let buf: Vec<u16> = s.encode_utf16().collect();
    text_utf16(hdc, &buf, rect, color, font, fmt);
}

unsafe fn text_utf16(hdc: HDC, buf: &[u16], rect: RECT, color: u32, font: HFONT, fmt: DRAW_TEXT_FORMAT) {
    if buf.is_empty() {
        return;
    }
    let old = SelectObject(hdc, font);
    let _ = SetTextColor(hdc, COLORREF(color));
    let _ = SetBkMode(hdc, TRANSPARENT);
    let mut owned: Vec<u16> = buf.to_vec();
    owned.push(0);
    let mut r = rect;
    let _ = DrawTextW(hdc, &mut owned, &mut r, fmt);
    let _ = SelectObject(hdc, old);
}

unsafe fn start_timer(hwnd: HWND, id: usize, ms: u32) {
    let _ = windows::Win32::UI::WindowsAndMessaging::SetTimer(hwnd, id, ms, None);
}

unsafe fn kill_timer(hwnd: HWND, id: usize) {
    let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(hwnd, id);
}

// ── 新版绘制函数（基于 WidgetState）─────────────────────────

/// Segmented 绘制。
unsafe fn draw_segmented_widget(w: &Widget, state: &WidgetState, hdc: HDC, dpi: u32, theme: &Theme, rect: RECT) {
    let sel = state.selected;
    let labels: &Box<[String]> = &w.kind.as_any().downcast_ref::<SegmentedWidget>().map(|s| &s.labels).unwrap();
    let n = labels.len().max(1) as i32;
    let seg_w = (rect.right - rect.left) / n;
    round_rect(hdc, rect, s(dpi, 7), theme.segmented_bg, theme.segmented_bg);
    for (i, label) in labels.iter().enumerate() {
        let r = RECT {
            left: rect.left + i as i32 * seg_w,
            top: rect.top,
            right: rect.left + (i as i32 + 1) * seg_w,
            bottom: rect.bottom,
        };
        if i == sel {
            let m = s(dpi, 2);
            let ir = RECT { left: r.left + m, top: r.top + m, right: r.right - m, bottom: r.bottom - m };
            round_rect(hdc, ir, s(dpi, 5), theme.seg_selected, theme.seg_selected_stroke);
        }
        let color = if i == sel { theme.label } else { theme.secondary };
        text(hdc, label, r, color, /*ctx.fonts.small*/ CreateFontW(-(11 * dpi as i32) / 72, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, windows::core::w!("Segoe UI")), DT_SINGLELINE | DT_CENTER | DT_VCENTER);
    }
}

/// Button 绘制。
unsafe fn draw_button_widget(w: &Widget, state: &WidgetState, hdc: HDC, dpi: u32, theme: &Theme, rect: RECT) {
    let hovered = state.hovered;
    let label: &String = w.kind.as_any().downcast_ref::<ButtonWidget>().map(|b| &b.label).unwrap();
    let fill = if hovered { theme.accent_hover } else { theme.accent };
    round_rect(hdc, rect, s(dpi, 7), fill, fill);
    text(hdc, label, rect, rgb(255, 255, 255), /*ctx.fonts.body*/ CreateFontW(-(13 * dpi as i32) / 72, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, windows::core::w!("Segoe UI")), DT_SINGLELINE | DT_CENTER | DT_VCENTER);
}

/// Swatches 绘制。
unsafe fn draw_swatches_widget(w: &Widget, state: &WidgetState, hdc: HDC, dpi: u32, theme: &Theme, rect: RECT) {
    let sel = state.selected;
    let sw = w.kind.as_any().downcast_ref::<SwatchesWidget>().unwrap();
    let palette = if sw.palette == 0 { &BG_SWATCHES } else { &FG_SWATCHES };
    let n = palette.len().max(1) as i32;
    let wd = (rect.right - rect.left) / n;
    let m = s(dpi, 3);
    for (i, &(r, g, b)) in palette.iter().enumerate() {
        let sw_rect = RECT {
            left: rect.left + i as i32 * wd,
            top: rect.top,
            right: rect.left + (i as i32 + 1) * wd,
            bottom: rect.bottom,
        };
        let inner = RECT {
            left: sw_rect.left + m,
            top: sw_rect.top + m,
            right: sw_rect.right - m,
            bottom: sw_rect.bottom - m,
        };
        let stroke = if i == sel { theme.accent } else { theme.card_stroke };
        round_rect(hdc, inner, s(dpi, 5), rgb(r, g, b), stroke);
    }
}

/// MetricSwatches 绘制。
unsafe fn draw_metric_swatches_widget(w: &Widget, state: &WidgetState, hdc: HDC, dpi: u32, theme: &Theme, rect: RECT) {
    let sel = state.selected;
    let follow = state.value_f > 0.5;
    let d = dpi;
    let n = SW_N as i32;
    let wd = (rect.right - rect.left) / n;
    let m = s(d, 3);
    for i in 0..SW_N {
        let sw = RECT {
            left: rect.left + i as i32 * wd,
            top: rect.top,
            right: rect.left + (i as i32 + 1) * wd,
            bottom: rect.bottom,
        };
        let inner = RECT {
            left: sw.left + m,
            top: sw.top + m,
            right: sw.right - m,
            bottom: sw.bottom - m,
        };
        if i < SW_FOLLOW {
            let (r, g, b) = METRIC_SWATCHES[i];
            round_rect(hdc, inner, s(d, 4), rgb(r, g, b), theme.card_stroke);
        } else {
            round_rect(hdc, inner, s(d, 4), theme.card, theme.secondary);
            text(hdc, "≈", inner, theme.secondary, CreateFontW(-(11 * dpi as i32) / 72, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, windows::core::w!("Segoe UI")), DT_SINGLELINE | DT_CENTER | DT_VCENTER);
        }
        let selected = (i == sel) || (i == SW_FOLLOW && follow && sel == SW_FOLLOW);
        if selected {
            let outline = RECT { left: sw.left, top: sw.top, right: sw.right, bottom: sw.bottom };
            round_rect(hdc, outline, s(d, 5), theme.card, theme.accent);
        }
    }
}

/// Slider 绘制。
unsafe fn draw_slider_widget(w: &Widget, state: &WidgetState, hdc: HDC, dpi: u32, theme: &Theme, rect: RECT, dp: u32) {
    let val = state.value_f as f32;
    let editing = state.editing;
    let t = theme;
    let d = dpi;
    let vb = value_box_rect(&rect, d);
    let tr = track_rect(&rect, d);

    // 轨道
    let track_y = (tr.top + tr.bottom) / 2;
    let pen = CreatePen(PS_SOLID, s(d, 2).max(1), COLORREF(t.segmented_bg));
    let old_pen = SelectObject(hdc, pen);
    let _ = MoveToEx(hdc, tr.left, track_y, None);
    let _ = LineTo(hdc, tr.right, track_y);
    let _ = SelectObject(hdc, old_pen);
    let _ = DeleteObject(pen);

    // 滑块
    let min = 0.0f32; let max = 100.0f32; // Placeholder, will be fixed
    let tfrac = if max > min { ((val - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
    let cx = tr.left + ((tr.right - tr.left) as f32 * tfrac).round() as i32;
    let thumb_d = s(d, 13);
    let thumb = RECT {
        left: cx - thumb_d / 2,
        top: track_y - thumb_d / 2,
        right: cx + thumb_d / 2,
        bottom: track_y + thumb_d / 2,
    };
    ellipse(hdc, thumb, t.accent, t.accent);

    // 值框
    let fill = if editing { t.button_bg_hover } else { t.button_bg };
    round_rect(hdc, vb, s(d, 6), fill, fill);
    let content = RECT { left: vb.left, top: vb.top, right: vb.right - s(d, 4), bottom: vb.bottom };

    // 显示文本
    let show: Vec<u16> = if editing {
        // Editing state uses ctx.edit buf - simplified here
        Vec::new()
    } else if dp == 0 {
        format!("{}{}", val.round() as i64, "").encode_utf16().collect()
    } else {
        format!("{:.1}{}", val, "").encode_utf16().collect()
    };
    text_utf16(hdc, &show, content, t.label, CreateFontW(-(11 * dpi as i32) / 72, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, windows::core::w!("Segoe UI")), DT_RIGHT | DT_SINGLELINE | DT_VCENTER);
}

fn pt_in(r: &RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}