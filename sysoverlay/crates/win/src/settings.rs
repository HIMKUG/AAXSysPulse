//! 设置面板 —— **完全自绘**的 Apple HIG 风格窗口（无原生控件）。
//!
//! # 设计（2026-09-04 用户拍板：完全自绘 + 跟随系统明暗）
//!
//! 视觉语言取自 macOS System Settings / iOS 分组列表：
//! - 浅灰窗口底 + 白色**圆角分组卡片**（半径 10；内部控件半径 5-7，
//!   与外圆角保持同心 —— HIG 的 concentric corners 公式）
//! - 每行「标签居左、控件居右」；控件全部自绘：
//!   - **开关**（toggle）：42×24 胶囊 + 白色滑块，systemGreen 打开色
//!   - **分段控件**（segmented）：底槽 + 白色选中片，用于内存单位/刷新率
//!   - **步进器**（stepper）：[−][+] 双钮 + 数值，用于字号
//!   - **主按钮**：accent 蓝底白字（Resize / Done）
//! - 悬停反馈：控件轻微变色 + 手型光标（HIG focus feedback 精神）
//! - 明暗两套色板（[`crate::theme`]），注册表跟随系统；深色下标题栏同变
//! - Win11 上请求 DWM 圆角窗口（`DWMWA_WINDOW_CORNER_PREFERENCE`），
//!   旧系统静默忽略
//!
//! # 交互语义
//!
//! 与旧原生控件版完全一致（含「拒绝关闭最后一个监控项」的弹回），
//! 全部改动即时生效 + 落盘 + `WM_RECONFIG` 通知悬浮窗重排版。
//!
//! # 渲染
//!
//! 双缓冲（内存 DC → BitBlt）防闪烁；Segoe UI 按 DPI 缩放、ClearType。
//! 窗口单例、关闭即隐藏（保留状态秒开）。

use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, Ellipse, EndPaint, FillRect, InvalidateRect,
    RoundRect, SelectObject, SetBkMode, SetTextColor, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT,
    DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, DRAW_TEXT_FORMAT, HBRUSH, HDC, HFONT,
    PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, GetClientRect, LoadCursorW, PostMessageW,
    RegisterClassExW, SetCursor, ShowWindow, HCURSOR, IDC_HAND, SW_HIDE, SW_SHOW, WINDOW_EX_STYLE,
    WNDCLASSEXW, WS_CAPTION, WS_SYSMENU,
};

use crate::theme::{rgb, Theme};
use crate::window::{self, WM_RECONFIG};
use crate::{autostart, config_io};
use sysoverlay_core::color::Rgb;
use sysoverlay_core::types::{RenderStyle, Shape, VertLayout};

// ── 消息常量 ─────────────────────────────────────────────
const WM_DESTROY: u32 = 0x0002;
const WM_CLOSE: u32 = 0x0010;
const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_SETTINGCHANGE: u32 = 0x001A;
const WM_SETCURSOR: u32 = 0x0020;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_MOUSELEAVE: u32 = 0x02A2;
/// 诊断用（SYSOVERLAY_DIAG=settings,click）：模拟点击 CPU 开关。
/// 在 UI 线程消息流内处理，与真实鼠标点击走完全相同的
/// on_click → InvalidateRect → WM_PAINT 链路。
const WM_DIAG_CLICK: u32 = 0x8000 + 41;

// ── 控件语义 ID（沿用旧常量，便于对照） ──────────────────
const IDC_CPU: i32 = 101;
const IDC_MEM: i32 = 102;
const IDC_GPU: i32 = 103;
const IDC_NET: i32 = 104;
const IDC_POWER: i32 = 105;
const IDC_SHADOW: i32 = 111;
const IDC_OUTLINE: i32 = 112;
const IDC_TOPMOST: i32 = 113;
const IDC_RESIZE: i32 = 119;
const IDC_AUTOSTART: i32 = 120;
const IDC_MEMUNIT: i32 = 130;
const IDC_FONT: i32 = 131;
const IDC_RATE: i32 = 132;
const IDC_DEBUG: i32 = 140;

// ── v3 渲染器切换控件 ─────────────────────────────────────
const IDC_STYLE: i32 = 150;
const IDC_SHAPE: i32 = 151;
const IDC_OPACITY: i32 = 152;
const IDC_HOVER: i32 = 153;
const IDC_VERT: i32 = 154;
const IDC_BG: i32 = 155;
const IDC_FG: i32 = 156;

/// 背景/前景色：6 个预设色板（自绘窗口不引入 ChooseColor 面板复杂度）。
const BG_SWATCHES: [(u8, u8, u8); 6] = [
    (26, 22, 20),   // 深棕（C 版默认）
    (20, 22, 27),   // 深蓝
    (27, 26, 30),   // 深灰
    (42, 42, 46),   // 中深灰
    (255, 255, 255),// 白
    (61, 61, 66),   // 浅灰
];
const FG_SWATCHES: [(u8, u8, u8); 6] = [
    (244, 241, 238), // 暖白（C 版默认）
    (238, 241, 244), // 冷白
    (255, 255, 255), // 纯白
    (220, 220, 220), // 浅灰
    (20, 20, 20),    // 深黑
    (30, 30, 30),    // 深灰
];

/// 刷新率分段：索引 → interval_ms（0 = Auto）。
const RATE_MS: [u32; 6] = [0, 100, 200, 1000, 2000, 5000];
const RATE_LABELS: [&str; 6] = ["Auto", "10", "5", "1", "0.5", "0.2"];

// ── 布局常量（逻辑像素，按 DPI 缩放） ────────────────────
const CLIENT_W: i32 = 360;
const MARGIN_X: i32 = 16;
const TOP_MARGIN: i32 = 14;
const BOTTOM_MARGIN: i32 = 16;
const CAPTION_H: i32 = 16;
const CAPTION_GAP: i32 = 4;
const CARD_PAD: i32 = 8;
const ROW_H: i32 = 30;
const CARD_GAP: i32 = 14;
const WIDGET_H: i32 = 24;
const FOOTNOTE_H: i32 = 34;
const CARD_RADIUS: i32 = 10;
/// 标签列宽（可容纳 "Always on Top" / "Launch at Login"）。
const LABEL_W: i32 = 116;
const TOGGLE_W: i32 = 42;

// ── GDI extern（window.rs 的声明是私有的，此处独立声明） ─
#[link(name = "user32")]
extern "system" {
    fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
    fn IsWindow(hwnd: HWND) -> windows::Win32::Foundation::BOOL;
}

const GWLP_UD: i32 = -21; // GWLP_USERDATA

/// 已打开的设置窗口句柄（单例；-1 = 未创建）。
static SETTINGS_HWND: AtomicIsize = AtomicIsize::new(-1);

// ── 控件模型（自描述：绘制所需状态全部内嵌） ─────────────

#[derive(Debug)]
enum WKind {
    Toggle { on: bool },
    Segmented { labels: &'static [&'static str], sel: usize },
    Stepper { value: u32 },
    Button { label: String },
    /// 颜色预设色板（每行一组方色块）。具体用哪套色板由 `Widget.id`（BG/FG）决定。
    Swatches { sel: usize },
}

#[derive(Debug)]
struct Widget {
    id: i32,
    kind: WKind,
    rect: RECT,
}

#[derive(Debug)]
struct Row {
    label: &'static str,
    /// `ctx.widgets` 的下标。
    widget: Option<usize>,
}

#[derive(Debug)]
struct Card {
    caption: &'static str,
    rect: RECT,
    rows: Vec<Row>,
    footnote: Option<&'static str>,
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
    cards: Vec<Card>,
    widgets: Vec<Widget>,
    /// 悬停中的控件下标。
    hover: Option<usize>,
    /// TrackMouseEvent 是否已登记（WM_MOUSELEAVE 只来一次）。
    tracking: bool,
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

/// 诊断用：向设置窗口投递一次「模拟点击 CPU 开关」消息
/// （SYSOVERLAY_DIAG=settings,click 时由主线程定时调用）。
/// 窗口/布局不存在则静默跳过。
pub fn diag_click_cpu() {
    let hwnd = SETTINGS_HWND.load(Ordering::SeqCst);
    if hwnd == -1 {
        crate::probe::log("diag_click_cpu: no settings window");
        return;
    }
    unsafe {
        let _ = PostMessageW(
            HWND(hwnd as *mut _),
            WM_DIAG_CLICK,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

// ── 窗口创建 ─────────────────────────────────────────────

/// 客户端总高（每张卡片的行数固定，只依赖 DPI）。
///
/// 卡片：A 显示项(5) / B 外观(12) / C 刷新率(1+footnote) /
/// D 窗口(2) / E 高级(1+footnote)。
/// 行数变化必须同步 [`build_layout`] 里的实际行数。
fn content_height(dpi: u32) -> i32 {
    const CARD_ROWS: [i32; 5] = [5, 12, 1, 2, 1];
    const CARD_FOOTNOTE: [bool; 5] = [false, false, true, false, true];
    let mut h = s(dpi, TOP_MARGIN + BOTTOM_MARGIN);
    for i in 0..5 {
        h += s(dpi, CAPTION_H + CAPTION_GAP)
            + 2 * s(dpi, CARD_PAD)
            + s(dpi, CARD_ROWS[i] * ROW_H);
        if CARD_FOOTNOTE[i] {
            h += s(dpi, FOOTNOTE_H);
        }
    }
    h + (CARD_ROWS.len() as i32 + 1) * s(dpi, CARD_GAP)
}

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
                hbrBackground: HBRUSH::default(), // 自绘全程覆盖，不做背景擦除
                ..Default::default()
            };
            let _ = RegisterClassExW(&wc);
        });

        let theme = Theme::system();
        let dpi = crate::dpi::get_system_dpi();
        let style = WS_CAPTION | WS_SYSMENU;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: s(dpi, CLIENT_W),
            bottom: content_height(dpi),
        };
        let _ = AdjustWindowRect(&mut rect, style, false);
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;

        // 先创建（不可见）→ 装上下文 → 再显示，避免首帧读到空指针
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::w!("SysOverlay_Settings"),
            windows::core::w!("SysOverlay Settings"),
            style,
            200,
            200,
            w,
            h,
            None,
            None,
            hinst,
            None,
        );
        let Ok(hwnd) = hwnd else { return };

        let ctx = Box::new(SettingsCtx {
            overlay,
            theme,
            dpi,
            fonts: Fonts::new(dpi),
            hand: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
            cards: Vec::new(),
            widgets: Vec::new(),
            hover: None,
            tracking: false,
        });
        SetWindowLongPtrW(hwnd, GWLP_UD, Box::into_raw(ctx) as isize);

        // Win11 圆角窗口 + 跟随主题的标题栏（旧系统静默忽略）
        apply_window_chrome(hwnd, theme.dark);

        if let Some(cfg) = window::get_config_of(overlay) {
            sync_layout(hwnd, &cfg);
        }

        SETTINGS_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        crate::probe::log(&format!("settings hwnd=0x{:x}", hwnd.0 as usize));
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
}

/// Win11：DWM 圆角 + 跟随主题的标题栏。
unsafe fn apply_window_chrome(hwnd: HWND, dark: bool) {
    // DWMWCP_ROUND = 2
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
        0, // italic
        0, // underline
        0, // strikeout
        1, // DEFAULT_CHARSET
        0, // OUT_DEFAULT_PRECIS
        0, // CLIP_DEFAULT_PRECIS
        5, // CLEARTYPE_QUALITY
        0, // DEFAULT_PITCH | FF_DONTCARE
        windows::core::w!("Segoe UI"),
    )
}

// ── 布局构建 ─────────────────────────────────────────────

/// 依据 cfg 重建全部卡片/控件（打开、配置变化、主题变化后调用）。
fn sync_layout(hwnd: HWND, cfg: &sysoverlay_core::config::Config) {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_UD) as *mut SettingsCtx;
        let Some(ctx) = ptr.as_mut() else { return };
        build_layout(ctx, cfg);
        let _ = InvalidateRect(hwnd, None, false);
    }
}

fn build_layout(ctx: &mut SettingsCtx, cfg: &sysoverlay_core::config::Config) {
    let dpi = ctx.dpi;
    let one = |v: i32| s(dpi, v);
    let card_l = one(MARGIN_X);
    let card_r = one(CLIENT_W) - one(MARGIN_X);
    let widget_r = card_r - one(12);
    let widget_l = card_l + one(CARD_PAD + 4) + one(LABEL_W);

    ctx.cards.clear();
    ctx.widgets.clear();
    let mut y = one(TOP_MARGIN);

    struct B<'a> {
        ctx: &'a mut SettingsCtx,
        y: i32,
        dpi: u32,
        card_l: i32,
        card_r: i32,
    }
    impl B<'_> {
        /// 开一张卡片，返回 (卡片下标, 首行 y)。
        fn begin(&mut self, caption: &'static str, rows: usize, extra_h: i32) -> (usize, i32) {
            let one = |v: i32| s(self.dpi, v);
            self.y += one(CAPTION_H) + one(CAPTION_GAP);
            let rect = RECT {
                left: self.card_l,
                top: self.y,
                right: self.card_r,
                bottom: self.y + 2 * one(CARD_PAD) + one(rows as i32 * ROW_H) + extra_h,
            };
            self.ctx.cards.push(Card { caption, rect, rows: Vec::new(), footnote: None });
            let rows_top = rect.top + one(CARD_PAD);
            self.y = rect.bottom;
            (self.ctx.cards.len() - 1, rows_top)
        }

        fn add_row(
            &mut self,
            card: usize,
            row_i: usize,
            label: &'static str,
            id: i32,
            kind: WKind,
            span: (i32, i32),
        ) {
            let one = |v: i32| s(self.dpi, v);
            let row_top = self.ctx.cards[card].rect.top + one(CARD_PAD) + one(row_i as i32 * ROW_H);
            let rect = RECT {
                left: span.0,
                top: row_top + (one(ROW_H) - one(WIDGET_H)) / 2,
                right: span.1,
                bottom: row_top + (one(ROW_H) - one(WIDGET_H)) / 2 + one(WIDGET_H),
            };
            self.ctx.widgets.push(Widget { id, kind, rect });
            let idx = self.ctx.widgets.len() - 1;
            self.ctx.cards[card].rows.push(Row { label, widget: Some(idx) });
        }
    }

    let mut b = B { ctx, y, dpi, card_l, card_r };
    let full = (card_l + one(12), card_r - one(12));

    // ── 卡片 A：监控项 ──
    let (c, top) = b.begin("MONITORING", 5, 0);
    let _ = top;
    for (row_i, (id, label, on)) in [
        (IDC_CPU, "CPU", cfg.show_cpu),
        (IDC_MEM, "Memory", cfg.show_mem),
        (IDC_GPU, "GPU", cfg.show_gpu),
        (IDC_NET, "Network", cfg.show_net),
        (IDC_POWER, "Power", cfg.power_enabled),
    ]
    .into_iter()
    .enumerate()
    {
        let rect_span = (widget_r - one(TOGGLE_W), widget_r);
        b.add_row(c, row_i, label, id, WKind::Toggle { on }, rect_span);
    }

    // ── 卡片 B：外观 ──
    let (c, _) = b.begin("APPEARANCE", 12, 0);
    for (row_i, (id, label, kind)) in [
        (
            IDC_MEMUNIT,
            "Memory Unit",
            WKind::Segmented { labels: &["Percent", "GB"], sel: if cfg.mem_show_gb { 1 } else { 0 } },
        ),
        (IDC_SHADOW, "Text Shadow", WKind::Toggle { on: cfg.text_shadow }),
        (IDC_OUTLINE, "Text Outline", WKind::Toggle { on: cfg.text_outline }),
        (IDC_TOPMOST, "Always on Top", WKind::Toggle { on: cfg.topmost }),
        (IDC_FONT, "Text Size", WKind::Stepper { value: cfg.font_scale }),
    ]
    .into_iter()
    .enumerate()
    {
        b.add_row(c, row_i, label, id, kind, (widget_l, widget_r));
    }

    // 渲染风格组（v3 双渲染器）：
    // Style / Shape / Opacity / Hover / Vert / Bg / Fg
    for (row_i, (id, label, kind)) in [
        (
            IDC_STYLE,
            "Style",
            WKind::Segmented {
                labels: &["Cards", "Strip"],
                sel: match cfg.render_style {
                    sysoverlay_core::types::RenderStyle::Classic => 0,
                    sysoverlay_core::types::RenderStyle::Strip => 1,
                },
            },
        ),
        (
            IDC_SHAPE,
            "Shape",
            WKind::Segmented {
                labels: &["Round", "Square", "None"],
                sel: cfg.shape.to_config() as usize,
            },
        ),
        (IDC_OPACITY, "Opacity", WKind::Stepper { value: cfg.opacity as u32 }),
        (IDC_HOVER, "Hover Reveal", WKind::Toggle { on: cfg.hover }),
        (
            IDC_VERT,
            "Layout",
            WKind::Segmented {
                labels: &["Row", "Col", "Split"],
                sel: cfg.vert.to_config() as usize,
            },
        ),
        (
            IDC_BG,
            "Background",
            WKind::Swatches { sel: swatch_sel(&BG_SWATCHES, cfg.bg) },
        ),
        (
            IDC_FG,
            "Foreground",
            WKind::Swatches { sel: swatch_sel(&FG_SWATCHES, cfg.fg) },
        ),
    ]
    .into_iter()
    .enumerate()
    {
        b.add_row(c, 5 + row_i, label, id, kind, (widget_l, widget_r));
    }

    // ── 卡片 C：刷新率 ──
    let (c, _) = b.begin("REFRESH RATE", 1, one(FOOTNOTE_H));
    let sel = if cfg.refresh_auto {
        0
    } else {
        match cfg.interval_ms {
            100 => 1,
            200 => 2,
            1000 => 3,
            2000 => 4,
            _ => 5,
        }
    };
    b.add_row(c, 0, "", IDC_RATE, WKind::Segmented { labels: &RATE_LABELS, sel }, full);
    b.ctx.cards[c].footnote = Some(
        "Auto adapts the refresh rate to how fast system counters actually update, and eases off automatically when sampling gets expensive.",
    );

    // ── 卡片 D：窗口 ──
    let (c, _) = b.begin("WINDOW", 2, 0);
    let resizing = window::resize_mode_active(b.ctx.overlay).unwrap_or(false);
    b.add_row(
        c,
        0,
        "Overlay Size",
        IDC_RESIZE,
        WKind::Button { label: if resizing { "Done".into() } else { "Resize".into() } },
        (widget_l, widget_r),
    );
    b.add_row(c, 1, "Launch at Login", IDC_AUTOSTART, WKind::Toggle { on: cfg.autostart }, (widget_l, widget_r));

    // ── 卡片 E：高级（调试） ──
    let (c, _) = b.begin("ADVANCED", 1, one(FOOTNOTE_H));
    b.add_row(c, 0, "Debug Log", IDC_DEBUG, WKind::Toggle { on: cfg.debug_log }, (widget_l, widget_r));
    b.ctx.cards[c].footnote = Some(
        "Writes raw sampler output to %LOCALAPPDATA%\\SysOverlayC\\probe.log. Turn off when not diagnosing.",
    );

    y = b.y;
    let _ = y;
}

// ── 窗口过程 ─────────────────────────────────────────────

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
        WM_MOUSEMOVE => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_UD) as *mut SettingsCtx;
            if let Some(ctx) = ptr.as_mut() {
                let pt = POINT {
                    x: (lparam.0 & 0xFFFF) as i16 as i32,
                    y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
                };
                let new_hover = ctx.widgets.iter().position(|w| pt_in(&w.rect, pt));
                if new_hover != ctx.hover {
                    ctx.hover = new_hover;
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
            let ptr = GetWindowLongPtrW(hwnd, GWLP_UD) as *mut SettingsCtx;
            if let Some(ctx) = ptr.as_mut() {
                ctx.tracking = false;
                if ctx.hover.take().is_some() {
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_UD) as *mut SettingsCtx;
            if let Some(ctx) = ptr.as_mut() {
                let pt = POINT {
                    x: (lparam.0 & 0xFFFF) as i16 as i32,
                    y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
                };
                if let Some(idx) = ctx.widgets.iter().position(|w| pt_in(&w.rect, pt)) {
                    on_click(ctx, idx, pt);
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_DIAG_CLICK => {
            // 模拟点击 CPU 开关：UI 线程内取最新布局并走完整 on_click 链路
            let ptr = GetWindowLongPtrW(hwnd, GWLP_UD) as *mut SettingsCtx;
            if let Some(ctx) = ptr.as_mut() {
                if let Some(idx) = ctx.widgets.iter().position(|w| w.id == IDC_CPU) {
                    let w = &ctx.widgets[idx];
                    let pt = POINT {
                        x: (w.rect.left + w.rect.right) / 2,
                        y: (w.rect.top + w.rect.bottom) / 2,
                    };
                    crate::probe::log(&format!(
                        "diag_click cpu rect={},{},{},{}",
                        w.rect.left, w.rect.top, w.rect.right, w.rect.bottom
                    ));
                    on_click(ctx, idx, pt);
                    let _ = InvalidateRect(hwnd, None, false);
                } else {
                    crate::probe::log("diag_click: CPU widget not found");
                }
            } else {
                crate::probe::log("diag_click: ctx null");
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_UD) as *mut SettingsCtx;
            if let Some(ctx) = ptr.as_ref() {
                if ctx.hover.is_some() {
                    SetCursor(ctx.hand);
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_SETTINGCHANGE => {
            // 系统明暗切换：重建主题 + 字体 + 布局
            let ptr = GetWindowLongPtrW(hwnd, GWLP_UD) as *mut SettingsCtx;
            if let Some(ctx) = ptr.as_mut() {
                ctx.theme = Theme::system();
                let new_fonts = Fonts::new(ctx.dpi);
                ctx.fonts.delete();
                ctx.fonts = new_fonts;
                apply_window_chrome(hwnd, ctx.theme.dark);
                if let Some(cfg) = window::get_config_of(ctx.overlay) {
                    build_layout(ctx, &cfg);
                }
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            // 隐藏而非销毁：保留状态，下次秒开
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

// ── 交互 ─────────────────────────────────────────────────

fn pt_in(r: &RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

/// 在预设色板里找与当前配置颜色相等的下标（找不到取 0）。
fn swatch_sel(palette: &[(u8, u8, u8)], color: Rgb) -> usize {
    palette
        .iter()
        .position(|&(r, g, b)| Rgb::new(r, g, b) == color)
        .unwrap_or(0)
}

/// 点击分发：Toggle → 语义开关；Segmented → 段索引；Stepper → ±；Button → Resize。
fn on_click(ctx: &mut SettingsCtx, idx: usize, pt: POINT) {
    let t0 = crate::perf::begin();
    let overlay = ctx.overlay;
    let (id, action) = {
        let w = &ctx.widgets[idx];
        match &w.kind {
            WKind::Toggle { .. } => (w.id, Action::Toggle),
            WKind::Segmented { labels, sel } => {
                let n = labels.len().max(1) as i32;
                let seg_w = (w.rect.right - w.rect.left) / n;
                let i = (((pt.x - w.rect.left) / seg_w.max(1)) as usize).min(labels.len() - 1);
                if i == *sel {
                    return; // 点在已选中段：无操作
                }
                (w.id, Action::Segment(i))
            }
            WKind::Stepper { .. } => {
                let (dec, inc) = stepper_subrects(&w.rect);
                let mid = POINT { x: pt.x, y: (w.rect.top + w.rect.bottom) / 2 };
                if pt_in(&dec, mid) {
                    (w.id, Action::StepperDec)
                } else if pt_in(&inc, mid) {
                    (w.id, Action::StepperInc)
                } else {
                    return;
                }
            }
            WKind::Button { .. } => (w.id, Action::Button),
            WKind::Swatches { sel } => {
                let n = 6i32;
                let wd = (w.rect.right - w.rect.left) / n;
                let i = (((pt.x - w.rect.left) / wd.max(1)) as usize).min(5);
                if i == *sel {
                    return; // 点在同一色块：无操作
                }
                (w.id, Action::Swatch(i))
            }
        }
    };

    match action {
        Action::Toggle => apply_toggle(overlay, id),
        Action::Segment(i) => apply_segment(overlay, id, i),
        Action::Swatch(i) => apply_swatch(overlay, id, i),
        Action::StepperDec | Action::StepperInc => {
            let inc = matches!(action, Action::StepperInc);
            if id == IDC_OPACITY {
                let step = 5u32;
                set_cfg(overlay, |cfg| {
                    let old = cfg.opacity as u32;
                    let nv = if inc { old + step } else { old.saturating_sub(step) }.clamp(5, 100);
                    if nv != old {
                        cfg.opacity = nv as u8;
                        true
                    } else {
                        false
                    }
                });
            } else {
                set_cfg(overlay, |cfg| {
                    let old = cfg.font_scale;
                    cfg.font_scale = if inc { old + 1 } else { old.saturating_sub(1) }.clamp(1, 4);
                    cfg.font_scale != old
                });
            }
        }
        Action::Button => {
            // Resize ↔ Done：切换边框缩放模式（不改配置，只同步按钮文字）
            let cur = window::resize_mode_active(overlay).unwrap_or(false);
            window::set_resize_mode(overlay, !cur);
        }
    }
    // 统一弹回同步（含拒绝非法操作后的回弹 + 按钮文字刷新）
    if let Some(now) = window::get_config_of(overlay) {
        crate::probe::log(&format!(
            "on_click before build_layout cfg_cpu_on={}",
            now.show_cpu as u8
        ));
        build_layout(ctx, &now);
        let cpu_on = ctx
            .widgets
            .iter()
            .find(|w| w.id == IDC_CPU)
            .map(|w| matches!(w.kind, WKind::Toggle { on: true }))
            .unwrap_or(false);
        crate::probe::log(&format!(
            "on_click after  build_layout widget_cpu_on={}",
            cpu_on as u8
        ));
    } else {
        crate::probe::log("on_click get_config_of(overlay) -> None");
    }
    crate::perf::note_section("settings_onclick", t0);
}

enum Action {
    Toggle,
    Segment(usize),
    Swatch(usize),
    StepperDec,
    StepperInc,
    Button,
}

/// 修改配置的统一入口：改 → 通知悬浮窗 → 落盘。
fn set_cfg(overlay: HWND, f: impl FnOnce(&mut sysoverlay_core::config::Config) -> bool) {
    let Some(cur) = window::get_config_of(overlay) else { return };
    let mut cfg = cur.clone();
    if f(&mut cfg) {
        window::modify_config(overlay, |c| *c = cfg.clone());
        unsafe {
            let _ = PostMessageW(overlay, WM_RECONFIG, WPARAM(0), LPARAM(0));
        }
        let _ = config_io::save_config(&cfg);
    }
}

fn apply_toggle(overlay: HWND, id: i32) {
    set_cfg(overlay, |cfg| match id {
        // 先翻转再校验守卫：非法则回滚（至少保留一项）
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
        IDC_AUTOSTART => {
            cfg.autostart = !cfg.autostart;
            autostart::apply(cfg.autostart);
            true
        }
        IDC_DEBUG => {
            cfg.debug_log = !cfg.debug_log;
            // 立即生效：设置面板切换后下一次采样就按新值落盘
            crate::probe::set_debug(cfg.debug_log);
            if cfg.debug_log {
                crate::probe::log(&format!(
                    "debug log enabled by user (pid={})",
                    std::process::id()
                ));
            }
            true
        }
        IDC_HOVER => {
            cfg.hover = !cfg.hover;
            true
        }
        _ => false,
    });
}

/// 监控项守卫：至少保留一项。
fn guard(cfg: &sysoverlay_core::config::Config) -> bool {
    cfg.show_cpu || cfg.show_mem || cfg.show_gpu || cfg.show_net || cfg.power_enabled
}

fn apply_segment(overlay: HWND, id: i32, idx: usize) {
    set_cfg(overlay, |cfg| match id {
        IDC_MEMUNIT => {
            let want = idx == 1;
            if cfg.mem_show_gb != want {
                cfg.mem_show_gb = want;
                true
            } else {
                false
            }
        }
        IDC_RATE => match idx {
            0 => {
                if cfg.refresh_auto {
                    false
                } else {
                    cfg.refresh_auto = true;
                    true
                }
            }
            i if i < RATE_MS.len() => {
                let ms = RATE_MS[i];
                if cfg.refresh_auto || cfg.interval_ms != ms {
                    cfg.refresh_auto = false;
                    cfg.interval_ms = ms;
                    true
                } else {
                    false
                }
            }
            _ => false,
        },
        IDC_STYLE => {
            let want = if idx == 0 { RenderStyle::Classic } else { RenderStyle::Strip };
            if cfg.render_style != want {
                cfg.render_style = want;
                true
            } else {
                false
            }
        }
        IDC_SHAPE => {
            let want = match idx {
                0 => Shape::Round,
                1 => Shape::Square,
                _ => Shape::Content,
            };
            if cfg.shape != want {
                cfg.shape = want;
                true
            } else {
                false
            }
        }
        IDC_VERT => {
            let want = match idx {
                0 => VertLayout::SingleRow,
                1 => VertLayout::SingleCol,
                _ => VertLayout::SplitCol,
            };
            if cfg.vert != want {
                cfg.vert = want;
                true
            } else {
                false
            }
        }
        _ => false,
    });
}

/// 预设色板点击：改背景/前景色。
fn apply_swatch(overlay: HWND, id: i32, idx: usize) {
    set_cfg(overlay, |cfg| match id {
        IDC_BG => {
            let (r, g, b) = BG_SWATCHES[idx.min(BG_SWATCHES.len() - 1)];
            let want = Rgb::new(r, g, b);
            if cfg.bg != want {
                cfg.bg = want;
                true
            } else {
                false
            }
        }
        IDC_FG => {
            let (r, g, b) = FG_SWATCHES[idx.min(FG_SWATCHES.len() - 1)];
            let want = Rgb::new(r, g, b);
            if cfg.fg != want {
                cfg.fg = want;
                true
            } else {
                false
            }
        }
        _ => false,
    });
}

/// 步进器子矩形：(−钮, ＋钮)；剩余区域显示数值。
fn stepper_subrects(rect: &RECT) -> (RECT, RECT) {
    let h = rect.bottom - rect.top;
    let btn_w = 26 * h / 24;
    let inc = RECT { left: rect.right - btn_w, top: rect.top, right: rect.right, bottom: rect.bottom };
    let dec = RECT { left: inc.left - btn_w - 2, top: rect.top, right: inc.left - 2, bottom: rect.bottom };
    (dec, inc)
}

// ── 绘制 ─────────────────────────────────────────────────

unsafe fn paint(hwnd: HWND) {
    let t0 = crate::perf::begin();
    let ptr = GetWindowLongPtrW(hwnd, GWLP_UD) as *mut SettingsCtx;
    let Some(ctx) = ptr.as_ref() else { return };
    // 诊断：paint 实际渲染的 CPU toggle 状态（与 config 对照，验证 UI 刷新链路）
    {
        let cpu_on = ctx
            .widgets
            .iter()
            .find(|w| w.id == IDC_CPU)
            .map(|w| matches!(w.kind, WKind::Toggle { on: true }))
            .unwrap_or(false);
        static LAST_CPU_ON: std::sync::atomic::AtomicIsize =
            std::sync::atomic::AtomicIsize::new(-1);
        let l = LAST_CPU_ON.swap(cpu_on as isize, std::sync::atomic::Ordering::Relaxed);
        if l != cpu_on as isize {
            crate::probe::log(&format!(
                "settings paint cpu_toggle={} (changed)",
                cpu_on as u8
            ));
        }
    }
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

    draw_all(mem, ctx);

    let _ = BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
    let _ = SelectObject(mem, old_bmp);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mem);
    let _ = EndPaint(hwnd, &ps);
    crate::perf::note_section("settings_paint", t0);
}

unsafe fn draw_all(hdc: HDC, ctx: &SettingsCtx) {
    let t = &ctx.theme;
    let dpi = ctx.dpi;
    let one = |v: i32| s(dpi, v);
    SetBkMode(hdc, TRANSPARENT);

    for card in &ctx.cards {
        // 分节标题（卡片上方，次级色小字）
        let cap = RECT {
            left: card.rect.left + one(2),
            top: card.rect.top - one(CAPTION_H + CAPTION_GAP),
            right: card.rect.right,
            bottom: card.rect.top - one(CAPTION_GAP),
        };
        text(hdc, card.caption, cap, t.secondary, ctx.fonts.small, DT_SINGLELINE | DT_LEFT | DT_VCENTER);

        // 圆角卡片
        round_rect(hdc, card.rect, one(CARD_RADIUS), t.card, t.card_stroke);

        // 行
        for (row_i, row) in card.rows.iter().enumerate() {
            let row_top = card.rect.top + one(CARD_PAD) + one(row_i as i32 * ROW_H);
            if !row.label.is_empty() {
                let right = row
                    .widget
                    .map(|i| ctx.widgets[i].rect.left - one(8))
                    .unwrap_or(card.rect.right - one(12));
                let r = RECT {
                    left: card.rect.left + one(12),
                    top: row_top,
                    right,
                    bottom: row_top + one(ROW_H),
                };
                text(hdc, row.label, r, t.label, ctx.fonts.body,
                    DT_SINGLELINE | DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
            }
            if let Some(i) = row.widget {
                draw_widget(hdc, ctx, i, ctx.hover == Some(i));
            }
        }

        // 卡片脚注
        if let Some(foot) = card.footnote {
            let rows_bottom = card.rect.top + one(CARD_PAD) + one(card.rows.len() as i32 * ROW_H);
            let r = RECT {
                left: card.rect.left + one(12),
                top: rows_bottom,
                right: card.rect.right - one(12),
                bottom: rows_bottom + one(FOOTNOTE_H),
            };
            text(hdc, foot, r, t.secondary, ctx.fonts.small, DT_WORDBREAK);
        }
    }
}

unsafe fn draw_widget(hdc: HDC, ctx: &SettingsCtx, idx: usize, hovered: bool) {
    let w = &ctx.widgets[idx];
    match &w.kind {
        WKind::Toggle { on } => draw_toggle(hdc, ctx, w.rect, *on),
        WKind::Segmented { labels, sel } => draw_segmented(hdc, ctx, w.rect, labels, *sel),
        WKind::Stepper { value } => draw_stepper(hdc, ctx, w, *value, hovered),
        WKind::Button { label } => draw_button(hdc, ctx, w.rect, label, hovered),
        WKind::Swatches { sel } => {
            let palette = if w.id == IDC_BG { &BG_SWATCHES } else { &FG_SWATCHES };
            draw_swatches(hdc, ctx, w.rect, palette, *sel);
        }
    }
}

/// 颜色预设色板：一排色块，选中的块描 accent 边。
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
        let fill = rgb(r, g, b);
        let stroke = if i == sel { ctx.theme.accent } else { ctx.theme.card_stroke };
        round_rect(hdc, inner, s(ctx.dpi, 5), fill, stroke);
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

    // 滑块：白色圆形，3px 内边距
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
    // 底槽（描边同色 → 无边感）
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

unsafe fn draw_stepper(hdc: HDC, ctx: &SettingsCtx, w: &Widget, value: u32, hovered: bool) {
    let t = &ctx.theme;
    let rect = w.rect;
    let (dec, inc) = stepper_subrects(&rect);
    let bg = if hovered { t.button_bg_hover } else { t.button_bg };
    round_rect(hdc, dec, s(ctx.dpi, 6), bg, bg);
    round_rect(hdc, inc, s(ctx.dpi, 6), bg, bg);
    text(hdc, "−", dec, t.label, ctx.fonts.body, DT_SINGLELINE | DT_CENTER | DT_VCENTER);
    text(hdc, "+", inc, t.label, ctx.fonts.body, DT_SINGLELINE | DT_CENTER | DT_VCENTER);
    // 数值：剩余区域右对齐（透明度显示 %，字号显示 x）
    let value_r = RECT { left: rect.left, top: rect.top, right: dec.left - 6, bottom: rect.bottom };
    let label = if w.id == IDC_OPACITY {
        format!("{}%", value)
    } else {
        format!("{}x", value)
    };
    text(hdc, &label, value_r, t.label, ctx.fonts.body,
        DT_SINGLELINE | DT_RIGHT | DT_VCENTER);
}

unsafe fn draw_button(hdc: HDC, ctx: &SettingsCtx, rect: RECT, label: &str, hovered: bool) {
    let t = &ctx.theme;
    let fill = if hovered { t.accent_hover } else { t.accent };
    round_rect(hdc, rect, s(ctx.dpi, 7), fill, fill);
    text(hdc, label, rect, rgb(255, 255, 255), ctx.fonts.body,
        DT_SINGLELINE | DT_CENTER | DT_VCENTER);
}

// ── GDI 基元 ─────────────────────────────────────────────

/// 圆角矩形（描边 + 填充），对象即用即删。
unsafe fn round_rect(hdc: HDC, r: RECT, radius: i32, fill: u32, stroke: u32) {
    let brush = CreateSolidBrush(COLORREF(fill));
    let pen = CreatePen(PS_SOLID, 1, COLORREF(stroke));
    let old_brush = SelectObject(hdc, brush);
    let old_pen = SelectObject(hdc, pen);
    let _ = RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius * 2, radius * 2);
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    let _ = DeleteObject(brush);
    let _ = DeleteObject(pen);
}

unsafe fn ellipse(hdc: HDC, r: RECT, fill: u32, stroke: u32) {
    let brush = CreateSolidBrush(COLORREF(fill));
    let pen = CreatePen(PS_SOLID, 1, COLORREF(stroke));
    let old_brush = SelectObject(hdc, brush);
    let old_pen = SelectObject(hdc, pen);
    let _ = Ellipse(hdc, r.left, r.top, r.right, r.bottom);
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    let _ = DeleteObject(brush);
    let _ = DeleteObject(pen);
}

/// 文本（选定字体/颜色，绘制后还原字体）。
unsafe fn text(hdc: HDC, s: &str, rect: RECT, color: u32, font: HFONT, fmt: DRAW_TEXT_FORMAT) {
    // 空串直接跳过：DT_SINGLELINE|DT_VCENTER + 空文本会触发 user32!DrawTextW
    // 空指针崩溃（0xC0000005 @ DrawTextW+0xF62，实测复现，空串无绘制意义）
    if s.is_empty() {
        return;
    }
    let old = SelectObject(hdc, font);
    SetTextColor(hdc, COLORREF(color));
    SetBkMode(hdc, TRANSPARENT);
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    let mut r = rect;
    let _ = DrawTextW(hdc, &mut buf, &mut r, fmt);
    SelectObject(hdc, old);
}
