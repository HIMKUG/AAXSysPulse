//! 设置面板外壳：窗口生命周期 + 单向数据流分派 + 全量 D2D 绘制。
//!
//! # 架构（S2/S3 重构落点）
//!
//! ```text
//! 用户事件 ──► Action ──► dispatch（业务规则 + 副作用）
//!                           │ 唯一写入口
//!                           ▼
//!                     overlay 的 Config
//!                           │
//!             ┌─────────────┼─────────────┐
//!             ▼             ▼             ▼
//!      WM_RECONFIG   DebouncedSave   invalidate（重绘）
//! ```
//!
//! - 控件经 [`CfgRef`](self::cfg_ref::CfgRef) 读写 Config，无散落 match；
//! - [`paint_d2d`] 用 D2D/DWrite 绘制全部 UI（ClearType，与旧 GDI 观感对齐）；
//! - 离开本窗口 = `SettingsCtx` Drop = 全部 D2D/DWrite 资源释放，零残留。

mod cfg_ref;
mod icons;
mod layout;
mod motion;
mod state;
mod tasks;
mod widget;

pub use tasks::SAVE_DEBOUNCE_MS;

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::Instant;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect,
    LoadCursorW, PostMessageW, RegisterClassExW, SetCursor, SetWindowPos, ShowWindow, HCURSOR,
    IDC_HAND, MINMAXINFO, SW_HIDE, SW_SHOW, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, WINDOW_EX_STYLE, WNDCLASSEXW, WS_MINIMIZEBOX, WS_POPUP,
};

use crate::gfx_d2d::{ColorF, Gfx, HAlign, RectF};
use crate::theme::Theme;
use crate::titlebar::{self, TitleBarState, TitleBtn};
use crate::window::{self, WM_RECONFIG};
use crate::config_io;
use state::{Action, EditState, Interaction, Page, WidgetState};
use tasks::DebouncedSave;

use cfg_ref::{CfgRef, Value};
use layout::{
    build_pages, content_origin, content_viewport, page_caption, s, CARD_PAD, CARD_RADIUS,
    CAPTION_GAP, CAPTION_H, FOOTNOTE_H,
    MAX_H, MAX_W, MIN_H, MIN_W, ROW_H, SB_ICON, SB_ICON_TEXT_GAP, SB_ITEM_GAP, SB_ITEM_H,
    SIDEBAR_PAD, SIDEBAR_W, SCROLLBAR_INSET, SCROLLBAR_W,
};
use state::pt_in;
use widget::{metric_font_status_restore_rect, sw_custom, sw_follow, sw_n, value_box_rect};

// ── 消息常量 ─────────────────────────────────────────────

/// 模态对话框（系统取色）打开标志：paint_d2d 检查到即跳过本帧，
/// 防止 ChooseColorW 的模态消息循环重入本窗口 proc 时与绘制争用 `Gfx`。
static MODAL_DIALOG: AtomicBool = AtomicBool::new(false);

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
const WM_GETMINMAXINFO: u32 = 0x0024;
const WM_SIZE: u32 = 0x0005;
const WM_EXITSIZEMOVE: u32 = 0x0232;
const WM_MOUSEWHEEL: u32 = 0x020A;

const HTCAPTION: u32 = 2;
const HTCLIENT: u32 = 1;
const HTTRANSPARENT: i32 = -1;
// 边框热区（WM_NCHITTEST 返回值，与 WinUser.h 数值一致）。
const HTLEFT: i32 = 10;
const HTRIGHT: i32 = 11;
const HTTOP: i32 = 12;
const HTTOPLEFT: i32 = 13;
const HTTOPRIGHT: i32 = 14;
const HTBOTTOM: i32 = 15;
const HTBOTTOMLEFT: i32 = 16;
const HTBOTTOMRIGHT: i32 = 17;

/// 诊断点击目标：颜色页 CPU 开关（build_pages 中第一个指标 Toggle 的 id）。
const IDC_DIAG_CPU: i32 = 200;

// 定时器
/// 动画主时钟：所有连续动画（翻页/滚动/悬停/布局）统一由它驱动，
/// WM_TIMER 回调里**直接 paint_d2d**，不经过 WM_PAINT 队列（输入饥饿免疫）。
const ANIM_TIMER: usize = 11;
const EDIT_TIMER: usize = 12;
/// 状态提示（关于页按钮反馈等）自动消失计时。
const STATUS_TIMER: usize = 13;
/// 步进器长按连发：350ms 首延迟，之后每 60ms 一步。
const STEPPER_TIMER: usize = 14;
const STEPPER_REPEAT_MS: u32 = 60;

// ── GDI extern（window.rs 的声明是私有的，此处独立声明） ─
#[link(name = "user32")]
extern "system" {
    fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
    fn IsWindow(hwnd: HWND) -> windows::Win32::Foundation::BOOL;
}

const GWLP_UD: i32 = -21;
const GWL_STYLE: i32 = -16;

/// 已打开的设置窗口句柄（单例；-1 = 未创建）。
static SETTINGS_HWND: AtomicIsize = AtomicIsize::new(-1);

// ── 坐标 newtype：编译期锁死双坐标系 ─────────────────────

/// 客户区坐标（WM_MOUSEMOVE / WM_NCHITTEST 的原始 pt）。
#[derive(Clone, Copy, Debug)]
pub struct ClientPt(pub POINT);

/// 内容坐标（相对内容区顶部；`Widget.rect` / 空间网格用这套）。
#[derive(Clone, Copy, Debug)]
pub struct ContentPt(pub POINT);

impl ClientPt {
    /// 客户区 → 内容区（正文命中测试唯一入口）。
    /// 减去侧边栏 x 偏移与滚动 y 偏移，得到内容坐标。
    pub fn to_content(self, dpi: u32, scroll_y: i32) -> ContentPt {
        let (ox, oy) = content_origin(dpi, scroll_y);
        ContentPt(POINT { x: self.0.x - ox, y: self.0.y - oy })
    }
}

// ── 生命周期根 ───────────────────────────────────────────

/// 设置窗口：一切状态与资源的 owner。
struct SettingsCtx {
    overlay: HWND,
    theme: Theme,
    dpi: u32,
    hand: HCURSOR,
    pages: [Page; 6],
    /// 当前分页。
    page: usize,
    /// 翻页动画（None = 无动画进行中）。
    page_anim: Option<PageAnim>,
    /// 统一交互状态机。
    interaction: Interaction,
    /// 正在编辑的输入框（buf/caret）。
    edit: Option<EditState>,
    /// 各指标配置卡的展开态（UI 状态，不落盘）。
    metric_expanded: [bool; 5],
    /// 滚轮惯性的滚动速度（px/s），橡胶边界用。
    scroll_vel: f32,
    /// 当前客户区尺寸（WM_SIZE 更新，rebuild_pages 与命中测试用）。
    client_size: (i32, i32),
    /// 侧边栏导航项悬停态（鼠标在侧边栏内时记录命中项下标）。
    nav_hover: Option<usize>,
    tracking: bool,
    /// 自定义标题栏状态。
    tb: TitleBarState,
    /// 待重绘的控件下标集合（None = 全量重绘）。
    dirty: Option<Vec<usize>>,
    /// 空间网格索引：每页一个。
    spatial_grids: Vec<state::SpatialGrid>,
    /// 逐控件过渡弹簧：键 = (页下标, 控件 id)。用 id 而非下标，
    /// 折叠展开引起重排时动画状态不会串到别的控件。
    transitions: std::collections::HashMap<(usize, i32), Transition>,
    /// 侧边栏选中胶囊的滑动脉簧（值 = 页面下标）。
    nav_sel: motion::Spring,
    /// 各指标折叠卡的动画展开度（0..1）。
    metric_expand_t: [motion::Spring; 5],
    /// 上一帧是否仍在推进布局类弹簧（收敛后再补一次重排）。
    layout_anim_active: bool,
    /// 翻页弹簧：水平位移（px，从 24 滑入到 0）。
    page_slide: motion::Spring,
    /// 翻页弹簧：淡入进度（0→1）。
    page_fade: motion::Spring,
    /// 滚动惯性弹簧（scroll_y 趋近目标）。
    scroll_spring: motion::Spring,
    /// 上一帧绘制时刻（帧间 dt）。
    last_frame: Instant,
    /// 高精度定时器已提升（timeBeginPeriod 引用计数须配对）。
    hires: bool,
    /// D2D 渲染底座（唯一资源持有者）。
    gfx: Gfx,
    /// 防抖落盘状态机。
    save: DebouncedSave,
    /// 进程内单调时钟起点（DebouncedSave 的 now 来源）。
    clock0: Instant,
    /// 键盘焦点（当前页控件下标；None = 无焦点）。
    focus: Option<usize>,
    /// 最近一次输入是否来自键盘（焦点环仅此时绘制，报告 §7.1）。
    keyboard_mode: bool,
    /// 瞬态状态提示（文案 + 出现时刻），2.5s 后自动清除。
    status: Option<(String, Instant)>,
    /// 步进器长按连发状态：(控件下标, 方向)。松开/失捕获即清。
    stepper_hold: Option<(usize, f64)>,
}

/// 翻页动画进行中标记（进度由弹簧推进，见 `anim_params`）。
#[derive(Clone, Copy)]
struct PageAnim;

/// 单个控件的过渡弹簧组（H2）。
#[derive(Clone, Copy, Default)]
pub(crate) struct Transition {
    hover: motion::Spring,
    press: motion::Spring,
    /// 值 / 选中项弹簧（Toggle 旋钮、Segment 指示条、Swatch 选中环、Slider 滑块）。
    val: motion::Spring,
    /// `val` 是否已按目标初始化（避免首帧从 0 滑入）。
    val_init: bool,
}

impl SettingsCtx {
    fn now_ms(&self) -> u64 {
        self.clock0.elapsed().as_millis() as u64
    }

    /// 当前正在交互的控件下标（若有）。
    fn active_widget(&self) -> Option<usize> {
        match &self.interaction {
            Interaction::Idle => None,
            Interaction::Hover { widget }
            | Interaction::Pressing { widget }
            | Interaction::Dragging { widget, .. }
            | Interaction::Editing { widget } => Some(*widget),
        }
    }

    fn is_editing(&self) -> bool {
        matches!(&self.interaction, Interaction::Editing { .. })
    }

    /// 标记单个控件需要重绘。
    fn invalidate_widget(&mut self, idx: usize) {
        if let Some(ref mut v) = self.dirty {
            if !v.contains(&idx) {
                v.push(idx);
            }
        } else {
            self.dirty = Some(vec![idx]);
        }
    }

    fn cfg(&self) -> Option<sysoverlay_core::config::Config> {
        window::get_config_of(self.overlay)
    }

    /// 依据当前 cfg 重建页面 + 网格（打开 / 配置结构变化后调用）。
    fn rebuild_pages(&mut self) {
        let t0 = std::time::Instant::now();
        if let Some(cfg) = self.cfg() {
            let lang = cfg.lang;
            let dpi = self.dpi;
            let (client_w, client_h) = self.client_size;
            let expand = self.metric_expand_t.map(|sp| sp.x.clamp(0.0, 1.0));
            build_pages(&mut self.pages, &cfg, lang, dpi, client_w, &expand);
            let mut grids = Vec::with_capacity(6);
            for page in &self.pages {
                let mut grid =
                    state::SpatialGrid::new(client_w, page.h.max(s(dpi, 200)), s(dpi, 30));
                for (idx, w) in page.widgets.iter().enumerate() {
                    grid.insert(idx, w.rect);
                }
                grids.push(grid);
            }
            self.spatial_grids = grids;
            // 尺寸变化后重新钳制滚动位置，防止悬空
            for p in self.pages.iter_mut() {
                let max = (p.h - client_h).max(0);
                p.scroll_y = p.scroll_y.clamp(0, max);
            }
            // 过渡弹簧按 (页, 控件 id) 持久化，无需随重排重建。
            crate::probe::log_lazy(|| {
                let n: usize = self.pages.iter().map(|p| p.widgets.len()).sum();
                format!(
                    "settings rebuild_pages widgets={} grids={} us={}",
                    n,
                    self.spatial_grids.len(),
                    t0.elapsed().as_micros()
                )
            });
        } else {
            crate::probe::log("settings rebuild_pages: cfg unavailable!");
        }
    }
}

/// 翻页动画进度（H3）：弹簧驱动，返回 (水平偏移px, 不透明度0..1)。
/// 可中断：切页时不重置弹簧，仅由 `dispatch(SetPage)` 在「非动画中」时置初值。
fn anim_params(ctx: &mut SettingsCtx, dt: f32) -> (f32, f32) {
    if ctx.page_anim.is_none() {
        return (0.0, 1.0);
    }
    let (k, damp) = if motion::animations_enabled() {
        (motion::K_SHEET, motion::C_SHEET)
    } else {
        (motion::K_REDUCED, motion::C_REDUCED)
    };
    let settled_s = ctx.page_slide.step(0.0, dt, k, damp);
    let settled_f = ctx.page_fade.step(1.0, dt, k, damp);
    if settled_s && settled_f {
        ctx.page_anim = None;
    }
    (ctx.page_slide.x, ctx.page_fade.x.clamp(0.0, 1.0))
}

/// 推进「布局类」弹簧：折叠卡展开度 + 侧边栏选中胶囊。
/// 返回 true = 仍在动（调用方需每帧重排，并在帧末请求下一帧）。
fn step_layout_anims(ctx: &mut SettingsCtx, dt: f32) -> bool {
    let (k, damp) = if motion::animations_enabled() {
        (motion::K_SHEET, motion::C_SHEET)
    } else {
        (motion::K_REDUCED, motion::C_REDUCED)
    };
    let mut moving = false;
    for (i, sp) in ctx.metric_expand_t.iter_mut().enumerate() {
        let target = if ctx.metric_expanded[i] { 1.0 } else { 0.0 };
        if !sp.step(target, dt, k, damp) {
            moving = true;
        }
    }
    if !ctx.nav_sel.step(ctx.page as f32, dt, motion::K_UI, motion::C_UI) {
        moving = true;
    }
    moving
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

/// 窗口隐藏/销毁前的强制落盘（pending 改动不丢）。
pub fn flush_config(overlay: HWND) {
    if let Some(cfg) = window::get_config_of(overlay) {
        let _ = config_io::save_config(&cfg);
    }
}

// ── 窗口创建 ─────────────────────────────────────────────

fn create_dialog(overlay: HWND) {
    unsafe {
        let Ok(hinst) = GetModuleHandleW(None) else { return };

        static REGISTERED: std::sync::Once = std::sync::Once::new();
        REGISTERED.call_once(|| {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(settings_wndproc),
                hInstance: hinst.into(),
                lpszClassName: windows::core::w!("SysOverlay_Settings"),
                hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH::default(),
                ..Default::default()
            };
            let _ = RegisterClassExW(&wc);
        });

        // 主题按配置外观解析：跟随系统 / 强制浅色 / 强制深色。
        let theme = window::get_config_of(overlay)
            .map(|c| Theme::resolve(c.appearance))
            .unwrap_or_else(Theme::system);
        let dpi = crate::dpi::get_system_dpi();
        let lang = window::get_config_of(overlay)
            .map(|c| c.lang)
            .unwrap_or_default();
        let title_wide: Vec<u16> = crate::i18n::t(lang, "WinTitle")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let (cfg_w, cfg_h) = window::get_config_of(overlay)
            .map(|c| c.settings_size)
            .unwrap_or((layout::DEFAULT_W, layout::DEFAULT_H));
        let w = s(dpi, cfg_w.clamp(layout::MIN_W, layout::MAX_W));
        let h = s(dpi, cfg_h.clamp(layout::MIN_H, layout::MAX_H));

        // 屏幕居中
        let sw = crate::dpi::screen_w();
        let sh = crate::dpi::screen_h();
        let x = (sw - w).max(0) / 2;
        let y = (sh - h).max(0) / 2;

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::w!("SysOverlay_Settings"),
            windows::core::PCWSTR(title_wide.as_ptr()),
            WS_POPUP | WS_MINIMIZEBOX,
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
        // 双保险：初始样式去掉了 WS_THICKFRAME，此处运行时再剥一次，防止任何路径绕过。
        // WS_POPUP=0x80000000, WS_MINIMIZEBOX=0x00020000, WS_THICKFRAME=0x00040000。
        let _ = SetWindowLongPtrW(hwnd, GWL_STYLE, 0x80020000_isize);
        let Some(gfx) = Gfx::new(hwnd) else {
            crate::probe::log("settings Gfx::new failed (no D2D?)");
            return;
        };

        let mut ctx = SettingsCtx {
            overlay,
            theme,
            dpi,
            hand: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
            pages: Default::default(),
            page: 0,
            page_anim: None,
            interaction: Interaction::Idle,
            edit: None,
            metric_expanded: [false; 5],
            scroll_vel: 0.0,
            client_size: (w, h),
            nav_hover: None,
            tracking: false,
            tb: TitleBarState::default(),
            dirty: None,
            spatial_grids: Vec::new(),
            transitions: std::collections::HashMap::new(),
            nav_sel: motion::Spring::new(0.0),
            metric_expand_t: [motion::Spring::new(0.0); 5],
            layout_anim_active: false,
            page_slide: motion::Spring::new(0.0),
            page_fade: motion::Spring::new(1.0),
            scroll_spring: motion::Spring::new(0.0),
            last_frame: Instant::now(),
            hires: false,
            gfx,
            save: DebouncedSave::default(),
            clock0: Instant::now(),
            focus: None,
            keyboard_mode: false,
            status: None,
            stepper_hold: None,
        };
        ctx.rebuild_pages();
        crate::probe::log(&format!(
            "settings layout w={w} h={h} content_top={} page_h={:?} widgets={:?}",
            0i32,
            ctx.pages.iter().map(|p| p.h).collect::<Vec<_>>(),
            ctx.pages.iter().map(|p| p.widgets.len()).collect::<Vec<_>>()
        ));
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

fn ctx_of(hwnd: HWND) -> Option<&'static mut SettingsCtx> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_UD) as *mut SettingsCtx };
    if p.is_null() {
        None
    } else {
        Some(unsafe { &mut *p })
    }
}

fn find_hwnd(_ctx: &SettingsCtx) -> HWND {
    let v = SETTINGS_HWND.load(Ordering::SeqCst);
    HWND(if v == -1 { std::ptr::null_mut() } else { v as *mut _ })
}

// ── 命中测试 ─────────────────────────────────────────────

/// 控件命中（当前页）：空间网格 O(1)。`pt` 必须是**内容坐标**。
fn widget_at(ctx: &SettingsCtx, pt: ContentPt) -> Option<usize> {
    let grid = ctx.spatial_grids.get(ctx.page)?;
    let candidates = grid.query(pt.0);
    let hit = candidates.iter().copied().find(|&i| {
        let w = &ctx.pages[ctx.page].widgets[i];
        pt_in(&w.rect, pt.0)
    });
    if let Some(idx) = hit {
        let w = &ctx.pages[ctx.page].widgets[idx];
        crate::probe::log(&format!(
            "widget_at hit pt=({},{}) -> idx={} id={}",
            pt.0.x, pt.0.y, idx, w.id
        ));
    }
    hit
}

/// 侧边栏导航项命中（客户区坐标 → 项下标）。
fn sidebar_at(dpi: u32, pt: ClientPt) -> Option<usize> {
    let x0 = s(dpi, SIDEBAR_PAD) as i32;
    let x1 = s(dpi, SIDEBAR_W) - s(dpi, SIDEBAR_PAD);
    if pt.0.x < x0 || pt.0.x >= x1 {
        return None;
    }
    let top = s(dpi, 8 + 30 + 12);
    let step = s(dpi, SB_ITEM_H + SB_ITEM_GAP);
    let rel = pt.0.y - top;
    if rel < 0 {
        return None;
    }
    let i = rel / step;
    if i >= 6 {
        return None;
    }
    // 落于项间距（gap）内视为未命中
    if rel % step >= s(dpi, SB_ITEM_H) {
        return None;
    }
    Some(i as usize)
}

/// 鼠标是否在内容区（用于滚轮与滚动条拖拽的归属判定）。
fn in_content(dpi: u32, client_w: i32, pt: ClientPt) -> bool {
    pt.0.x >= s(dpi, SIDEBAR_W) && pt.0.x < client_w
}

// ── 单向数据流：Action 分派（唯一写入口） ────────────────

/// 拒绝「关闭全部指标」：至少保留一项可见。
fn is_metric_toggle(r: CfgRef) -> bool {
    matches!(r, CfgRef::ShowCpu | CfgRef::ShowMem | CfgRef::ShowGpu | CfgRef::ShowNet | CfgRef::PowerEnabled)
}

fn metrics_all_off_after(cfg: &sysoverlay_core::config::Config, turning_off: CfgRef) -> bool {
    let v = |r: CfgRef| matches!(r.get(cfg), Value::Bool(true));
    let would_off = |r: CfgRef| r == turning_off;
    !(v(CfgRef::ShowCpu) && !would_off(CfgRef::ShowCpu))
        && !(v(CfgRef::ShowMem) && !would_off(CfgRef::ShowMem))
        && !(v(CfgRef::ShowGpu) && !would_off(CfgRef::ShowGpu))
        && !(v(CfgRef::ShowNet) && !would_off(CfgRef::ShowNet))
        && !(v(CfgRef::PowerEnabled) && !would_off(CfgRef::PowerEnabled))
}

/// Action 分派：业务规则 + 副作用 + 唯一 Config 写入口。
fn dispatch(ctx: &mut SettingsCtx, action: Action) {
    let overlay = ctx.overlay;
    crate::probe::log_lazy(|| format!("dispatch {:?}", action));
    match action {
        Action::SetPage(p) => {
            if p != ctx.page {
                ctx.page = p;
                // H3：可中断翻页——仅在非动画中重置弹簧初值；动画中从当前位置续接。
                if ctx.page_anim.is_none() {
                    ctx.page_slide.set(24.0);
                    ctx.page_fade.set(0.0);
                }
                ctx.page_anim = Some(PageAnim);
                // 过渡弹簧数量对齐新页控件数。
                let n = ctx.pages[p].widgets.len();
                ctx.transitions.clear();
                for wi in 0..n {
                    ctx.transitions.insert((p, wi as i32), Transition::default());
                }
                // 翻页动画由 paint_d2d 帧末启动的 ANIM_TIMER(16ms) 驱动。
                ctx.interaction = Interaction::Idle;
            }
        }
        Action::Toggle(r) => {
            // 业务规则：至少一项指标可见
            if is_metric_toggle(r) {
                let reject = ctx.cfg().map(|c| metrics_all_off_after(&c, r)).unwrap_or(false);
                if reject {
                    crate::probe::log("dispatch: reject turning off last metric");
                    return;
                }
            }
            // 先读旧值，写后处理副作用
            let applied = apply_config(ctx, |cfg| {
                let Value::Bool(old) = r.get(cfg) else { return false };
                let new = !old;
                r.set(cfg, Value::Bool(new))
            });
            if applied {
                // 副作用
                let cfg_now = ctx.cfg();
                if let Some(cfg) = cfg_now {
                    match r {
                        CfgRef::Topmost => {
                            window::apply_topmost(overlay, cfg.topmost);
                        }
                        CfgRef::ClickThrough => {
                            window::apply_click_through(overlay, cfg.click_through);
                        }
                        _ => {}
                    }
                }
                // 语言切换影响全部静态文案 → 重建页面
                if r == CfgRef::Lang {
                    ctx.rebuild_pages();
                }
            }
        }
        Action::Select(r, i) => {
            let applied = apply_config(ctx, |cfg| {
                match r.get(cfg) {
                    Value::Select(old) => {
                        if old == i {
                            return false;
                        }
                        r.set(cfg, Value::Select(i))
                    }
                    Value::SelectOpt(old) => {
                        if old == Some(i) {
                            return false;
                        }
                        r.set(cfg, Value::SelectOpt(Some(i)))
                    }
                    _ => false,
                }
            });
            if r == CfgRef::Lang {
                if let Some(cfg) = ctx.cfg() {
                    crate::i18n::set_global_lang(cfg.lang);
                }
                ctx.rebuild_pages();
            }
            // 外观切换（跟随系统 / 浅色 / 深色）：重新解析主题 + 刷新窗口边框并重绘。
            if applied && r == CfgRef::Appearance {
                if let Some(cfg) = ctx.cfg() {
                    ctx.theme = Theme::resolve(cfg.appearance);
                }
                unsafe { apply_window_chrome(find_hwnd(ctx), ctx.theme.dark) };
                invalidate_settings();
            }
            // 显示位置（贴角）：立即把悬浮窗贴到选定角（报告 §5 Corner）。
            if applied && r == CfgRef::SnapCorner {
                if let Some(cfg) = ctx.cfg() {
                    if cfg.snap_corner <= 3 {
                        window::snap_overlay_to_corner(overlay, cfg.snap_corner);
                    }
                }
            }
        }
        Action::LiveValue(r, v) => {
            apply_config_quiet(ctx, |cfg| {
                r.set(cfg, Value::Num(v))
            });
            notify_overlay_reconfig(ctx);
        }
        Action::CommitValue(r, v) => {
            apply_config(ctx, |cfg| {
                r.set(cfg, Value::Num(v))
            });
        }
        Action::ToggleMetricExpand(i) => {
            if i < ctx.metric_expanded.len() {
                ctx.metric_expanded[i] = !ctx.metric_expanded[i];
                // 行结构变化 → 必须重排（重建卡片 / 网格 / 滚动范围）。
                ctx.rebuild_pages();
                invalidate_settings();
            }
        }
        // 滚轮滚动由 WM_MOUSEWHEEL 直接写 scroll_y（H4：橡胶边界 + 惯性吸附），
        // 因此不再经 Action::Scroll 中转。
    }
}

/// 统一配置修改入口（通知悬浮窗 + 防抖落盘 + 设置页重绘）。
/// 返回 true 表示 cfg 确实发生了变化。
fn apply_config(ctx: &mut SettingsCtx, f: impl FnOnce(&mut sysoverlay_core::config::Config) -> bool) -> bool {
    let t0 = std::time::Instant::now();
    let Some(mut cfg) = window::get_config_of(ctx.overlay) else {
        crate::probe::log("apply_config: get_config_of failed");
        return false;
    };
    if !f(&mut cfg) {
        return false;
    }
    window::modify_config(ctx.overlay, |c| *c = cfg.clone());
    // 通知悬浮窗重排
    unsafe {
        let _ = PostMessageW(ctx.overlay, WM_RECONFIG, WPARAM(0), LPARAM(0));
    }
    // 防抖落盘
    let now = ctx.now_ms();
    let saved = ctx.save.on_change(now) && config_io::save_config(&cfg);
    if saved {
        ctx.save.on_saved(now);
    }
    // 设置页重绘（重新从 overlay config 读取最新状态）
    invalidate_settings();
    crate::probe::log_lazy(|| {
        format!(
            "apply_config ok us={} debounce_saved={}",
            t0.elapsed().as_micros(),
            saved
        )
    });
    true
}

/// 拖动中版本：改内存预览，不落盘不通知。
fn apply_config_quiet(ctx: &mut SettingsCtx, f: impl FnOnce(&mut sysoverlay_core::config::Config) -> bool) -> bool {
    let Some(mut cfg) = window::get_config_of(ctx.overlay) else { return false };
    if !f(&mut cfg) {
        return false;
    }
    window::modify_config(ctx.overlay, |c| *c = cfg.clone());
    true
}

/// 通知悬浮窗立即按当前内存配置重排版/重绘（不落盘）。
///
/// 此前 LiveValue（拖动滑杆）只改 state.cfg 不通知 → 悬浮窗要等下一个
/// 采样快照（最长 2s+）才反映，用户观感即「拖滑杆不实时刷新」。
fn notify_overlay_reconfig(ctx: &SettingsCtx) {
    unsafe {
        let _ = PostMessageW(ctx.overlay, WM_RECONFIG, WPARAM(0), LPARAM(0));
    }
}

/// 刷新 settings 窗口。
fn invalidate_settings() {
    let hwnd = SETTINGS_HWND.load(Ordering::SeqCst);
    if hwnd != -1 {
        unsafe { let _ = InvalidateRect(HWND(hwnd as *mut _), None, false); }
    }
}

// ── 状态提示 / 动作辅助（报告 §6 action） ────────────────

fn lang_of(ctx: &SettingsCtx) -> sysoverlay_core::types::Lang {
    ctx.cfg().map(|c| c.lang).unwrap_or_else(global_lang)
}

/// 设置瞬态状态提示（2.5s 自动消失；WM_TIMER/STATUS_TIMER 清除）。
fn set_status(ctx: &mut SettingsCtx, text: &str) {
    ctx.status = Some((text.to_string(), Instant::now()));
    unsafe { start_timer(find_hwnd(ctx), STATUS_TIMER, 2500) };
    invalidate_settings();
}

/// 打开日志文件所在目录（报告 C-01：路径降级为动作）。
/// probe.log 位于 `%LOCALAPPDATA%\SysOverlayC\`（见 probe.rs）。
fn open_log_folder(ctx: &mut SettingsCtx) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let Some(dir) = std::env::var_os("LOCALAPPDATA") else {
        crate::probe::log("open_log_folder: no LOCALAPPDATA");
        return;
    };
    use std::os::windows::ffi::OsStrExt;
    let dir = std::path::PathBuf::from(dir).join("SysOverlayC");
    let _ = std::fs::create_dir_all(&dir);
    let dir_w: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    unsafe {
        let _ = ShellExecuteW(
            find_hwnd(ctx),
            PCWSTR(verb.as_ptr()),
            PCWSTR(dir_w.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
    set_status(ctx, crate::i18n::t(lang_of(ctx), "StatusFolderOpened"));
}

/// 复制版本信息到剪贴板（报告 §6 COPY_VERSION）。
fn copy_version_info(ctx: &mut SettingsCtx) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    let text = format!(
        "SysOverlay {}\n硬件监控悬浮条",
        env!("CARGO_PKG_VERSION")
    );
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let bytes = wide.len() * 2;

    unsafe {
        if OpenClipboard(find_hwnd(ctx)).is_err() {
            crate::probe::log("copy_version: OpenClipboard failed");
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
            let dst = GlobalLock(hmem);
            if !dst.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), dst as *mut u16, wide.len());
                let _ = GlobalUnlock(hmem);
                // 成功交出所有权；失败则系统忽略
                let _ = SetClipboardData(CF_UNICODETEXT.0 as u32, HANDLE(hmem.0));
            }
        }
        let _ = CloseClipboard();
    }
    set_status(ctx, crate::i18n::t(lang_of(ctx), "StatusCopied"));
}

// ── 键盘焦点链（报告 §7.1：自绘控件没有系统 Tab 链，必须自己实现） ──

/// Tab 推进焦点（backwards = Shift+Tab）。
fn focus_advance(ctx: &mut SettingsCtx, backwards: bool) {
    let n = ctx.pages[ctx.page].widgets.len();
    if n == 0 {
        return;
    }
    ctx.focus = Some(match (ctx.focus, backwards) {
        (None, false) => 0,
        (None, true) => n - 1,
        (Some(i), false) => (i + 1) % n,
        (Some(i), true) => (i + n - 1) % n,
    });
}

/// Space 触发焦点控件：复用点击路径（控件中心点）。
fn activate_focused(ctx: &mut SettingsCtx) {
    let Some(idx) = ctx.focus else { return };
    if idx >= ctx.pages[ctx.page].widgets.len() {
        ctx.focus = None;
        return;
    }
    let w = &ctx.pages[ctx.page].widgets[idx];
    let pt = ContentPt(POINT { x: (w.rect.left + w.rect.right) / 2, y: (w.rect.top + w.rect.bottom) / 2 });
    on_click_plain(ctx, idx, pt);
}

/// ← / →：滑杆 ±步进、步进器 ±1、分段左右切换（报告 §7.1 键盘可达性）。
fn on_hkey(ctx: &mut SettingsCtx, forward: bool) {
    let Some(idx) = ctx.focus else { return };
    let Some(cfg) = ctx.cfg() else { return };
    let page = &ctx.pages[ctx.page];
    let Some(w) = page.widgets.get(idx) else { return };
    let dir = if forward { 1.0 } else { -1.0 };
    if let Some(sl) = w.kind.as_any().downcast_ref::<widget::SliderWidget>() {
        let cur = match sl.cfg.get(&cfg) {
            Value::Num(x) => x,
            _ => return,
        };
        let nv = (cur + dir * sl.step as f64).clamp(sl.min as f64, sl.max as f64);
        dispatch(ctx, Action::CommitValue(sl.cfg, nv));
    } else if let Some(st) = w.kind.as_any().downcast_ref::<widget::StepperWidget>() {
        let cur = match st.cfg.get(&cfg) {
            Value::Num(x) => x,
            _ => return,
        };
        let nv = (cur + dir * st.step).clamp(st.min, st.max);
        dispatch(ctx, Action::CommitValue(st.cfg, nv));
    } else if let Some(seg) = w.kind.as_any().downcast_ref::<widget::SegmentedWidget>() {
        let n = seg.labels.len();
        if n == 0 {
            return;
        }
        let cur = match seg.cfg.get(&cfg) {
            Value::Select(i) => i,
            Value::SelectOpt(o) => o.unwrap_or(0),
            _ => return,
        };
        let ni = if forward { (cur + 1).min(n - 1) } else { cur.saturating_sub(1) };
        if ni != cur {
            dispatch(ctx, Action::Select(seg.cfg, ni));
        }
    }
}

// ── 步进器长按连发（−/＋ 按住持续步进） ──────────────────

/// 点位落在步进器的哪个按钮上：− = -1，＋ = +1，其余 0。
fn stepper_dir(rect: &RECT, dpi: u32, pt: ContentPt) -> f64 {
    let minus = widget::stepper_minus_rect(rect, dpi);
    let plus = widget::stepper_plus_rect(rect, dpi);
    if pt_in(&minus, pt.0) {
        -1.0
    } else if pt_in(&plus, pt.0) {
        1.0
    } else {
        0.0
    }
}

/// 步进一次（经 dispatch 唯一写入口）。
fn stepper_step(ctx: &mut SettingsCtx, idx: usize, dir: f64) {
    let Some(cfg) = ctx.cfg() else { return };
    let Some(w) = ctx.pages[ctx.page].widgets.get(idx) else { return };
    let Some(st) = w.kind.as_any().downcast_ref::<widget::StepperWidget>() else { return };
    let cur = match st.cfg.get(&cfg) {
        Value::Num(x) => x,
        _ => return,
    };
    let nv = (cur + dir * st.step).clamp(st.min, st.max);
    dispatch(ctx, Action::CommitValue(st.cfg, nv));
}

/// 进入输入框编辑态。
fn begin_edit(ctx: &mut SettingsCtx, idx: usize) {
    let (val, orig) = if let Some(slider) = ctx.pages[ctx.page].widgets[idx]
        .kind
        .as_any()
        .downcast_ref::<widget::SliderWidget>()
    {
        let v = match slider.cfg.get(&ctx.cfg().unwrap_or_default()) {
            Value::Num(x) => x as f32,
            _ => 0.0,
        };
        let f = if slider.dp == 0 { v.round() as i64 } else { (v * 10.0).round() as i64 / 10 };
        (format!("{f}"), v as f64)
    } else {
        (String::new(), 0.0)
    };
    ctx.interaction = Interaction::Editing { widget: idx };
    ctx.edit = Some(EditState {
        idx,
        buf: val.encode_utf16().collect(),
        caret: true,
        orig,
        select_all: true,
    });
}

/// 提交输入框内容。
fn commit_edit(ctx: &mut SettingsCtx) {
    let Some(e) = ctx.edit.take() else { return };
    ctx.interaction = Interaction::Idle;
    let text: String = e.buf.iter().map(|&c| char::from_u32(c as u32).unwrap_or('\u{FFFD}')).collect();
    let page = &ctx.pages[ctx.page];
    let Some(w) = page.widgets.get(e.idx) else { return };
    let Some(slider) = w.kind.as_any().downcast_ref::<widget::SliderWidget>() else { return };
    // 解析失败（空串/越界字符）回退编辑前值，不再回退到 min——旧行为会让
    // 用户输错一个字符就整项被改成最小值，非常反直觉。
    let parsed: f32 = match text.trim().parse() {
        Ok(x) => x,
        Err(_) => {
            dispatch(ctx, Action::CommitValue(slider.cfg, e.orig));
            return;
        }
    };
    let v = ((parsed / slider.step.max(0.001)).round() * slider.step)
        .max(slider.min)
        .min(slider.max);
    dispatch(ctx, Action::CommitValue(slider.cfg, v as f64));
}

// ── 点击类控件 ───────────────────────────────────────────

/// 系统颜色对话框（ChooseColorW）：色板「+」格的自定义取色。
/// 模态期间置 MODAL_DIALOG 暂停自绘，防止模态消息循环重入争用 Gfx。
/// 成功后写入所选颜色并同时关闭「自动色」。
fn open_color_picker(ctx: &mut SettingsCtx, color_cfg: CfgRef, auto_cfg: Option<CfgRef>) {
    use windows::Win32::Foundation::COLORREF;
    use windows::Win32::UI::Controls::Dialogs::{
        ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW,
    };

    static CUST: std::sync::Mutex<[COLORREF; 16]> = std::sync::Mutex::new([COLORREF(0); 16]);

    // 当前颜色作为对话框初值（跟随默认/自动时用白色）。
    let cur = ctx.cfg().and_then(|c| match color_cfg.get(&c) {
        Value::Color(Some(rgb)) => Some(rgb),
        _ => None,
    });

    let Ok(mut cust) = CUST.lock() else { return };
    let mut cc = CHOOSECOLORW::default();
    cc.lStructSize = std::mem::size_of::<CHOOSECOLORW>() as u32;
    cc.hwndOwner = find_hwnd(ctx);
    cc.rgbResult = COLORREF(cur.map(|rgb| rgb.to_colorref()).unwrap_or(0x00FF_FFFF));
    cc.lpCustColors = cust.as_mut_ptr();
    cc.Flags = CC_FULLOPEN | CC_RGBINIT;

    MODAL_DIALOG.store(true, Ordering::SeqCst);
    let ok = unsafe { ChooseColorW(&mut cc) }.as_bool();
    MODAL_DIALOG.store(false, Ordering::SeqCst);
    drop(cust);

    if ok {
        let rgb = sysoverlay_core::color::Rgb::from_colorref(cc.rgbResult.0);
        apply_config(ctx, |c| {
            let mut changed = false;
            if let Some(r) = auto_cfg {
                changed |= r.set(c, Value::Bool(false));
            }
            changed |= color_cfg.set(c, Value::Color(Some(rgb)));
            changed
        });
    }
}

fn on_click_plain(ctx: &mut SettingsCtx, idx: usize, pt: ContentPt) {
    let w = &ctx.pages[ctx.page].widgets[idx];
    let Some(cfg) = ctx.cfg() else {
        // cfg 拿不到也要给视觉反馈，不静默吞掉
        crate::probe::log("on_click_plain: cfg unavailable");
        invalidate_settings();
        return;
    };
    crate::probe::log(&format!(
        "click dispatch idx={idx} id={} page={} pt=({},{})",
        w.id, ctx.page, pt.0.x, pt.0.y
    ));

    // Toggle
    if let Some(t) = w.kind.as_any().downcast_ref::<widget::ToggleWidget>() {
        dispatch(ctx, Action::Toggle(t.cfg));
        return;
    }
    // 指标折叠头：点击整行 = 展开/收起（手风琴语义，报告 L03）。
    if let Some(mh) = w.kind.as_any().downcast_ref::<widget::MetricHeaderWidget>() {
        dispatch(ctx, Action::ToggleMetricExpand(mh.index));
        return;
    }
    // 字号状态条：仅点击右侧「恢复跟随全局」按钮才恢复该指标字号。
    if let Some(ms) = w.kind.as_any().downcast_ref::<widget::MetricFontStatusWidget>() {
        let index = ms.index;
        let lang = ms.lang;
        let overridden = cfg.metric_font_sizes.get(index).copied().flatten().is_some();
        if overridden {
            let rect = w.rect;
            let r = RectF::new(
                rect.left as f32, rect.top as f32,
                rect.right as f32, rect.bottom as f32,
            );
            if let Some(btn) = metric_font_status_restore_rect(
                r, ctx.dpi, &mut ctx.gfx, true, lang, body_px(ctx.dpi),
            ) {
                let p = pt.0;
                if p.x >= btn.l as i32 && p.x < btn.r as i32
                    && p.y >= btn.t as i32 && p.y < btn.b as i32
                {
                    apply_config(ctx, |cfg| {
                        if index < cfg.metric_font_sizes.len() {
                            let changed = cfg.metric_font_sizes[index].is_some();
                            cfg.metric_font_sizes[index] = None;
                            changed
                        } else {
                            false
                        }
                    });
                }
            }
        }
        return;
    }
    // Segmented
    if let Some(seg) = w.kind.as_any().downcast_ref::<widget::SegmentedWidget>() {
        let n = seg.labels.len();
        if n == 0 {
            return;
        }
        let seg_w = (w.rect.right - w.rect.left) / n as i32;
        let i = (((pt.0.x - w.rect.left) / seg_w.max(1)) as usize).min(n - 1);
        let mut st = WidgetState::default();
        w.kind.sync(&cfg, &mut st);
        // `has_sel=false`（如显示位置处于「自由定位」）时 selected 退化成 0，
        // 若仍用 `i != st.selected` 会令点击第 0 段被吞掉 → 选项无法选中。
        if !st.has_sel || i != st.selected {
            dispatch(ctx, Action::Select(seg.cfg, i));
        }
        return;
    }
    // Button：按动作派发（字号重置、日志目录、关于页动作）
    if let Some(btn) = w.kind.as_any().downcast_ref::<widget::ButtonWidget>() {
        match btn.action {
            widget::ButtonAction::ResetAllColors => {
                apply_config(ctx, |cfg| {
                    let mut changed = false;
                    for mc in cfg.metric_colors.iter_mut() {
                        if mc.label.is_some() {
                            mc.label = None;
                            changed = true;
                        }
                        if mc.value.is_some() {
                            mc.value = None;
                            changed = true;
                        }
                    }
                    changed
                });
            }
            widget::ButtonAction::ToggleMetricExpand(i) => {
                dispatch(ctx, Action::ToggleMetricExpand(i));
            }
            widget::ButtonAction::OpenLogFolder => {
                open_log_folder(ctx);
            }
            widget::ButtonAction::CheckUpdate => {
                // 无更新服务：反馈「已是最新」（行为收敛于状态提示，不弹窗）。
                set_status(ctx, crate::i18n::t(lang_of(ctx), "StatusUpToDate"));
            }
            widget::ButtonAction::CopyVersion => {
                copy_version_info(ctx);
            }
        }
        return;
    }
    // Swatches：选中态由颜色值派生（与旧实现一致），只写色值。
    if let Some(sw) = w.kind.as_any().downcast_ref::<widget::SwatchesWidget>() {
        let n = if sw.palette == 0 { widget::BG_SWATCHES.len() } else { widget::FG_SWATCHES.len() };
        let wd = (w.rect.right - w.rect.left) / n as i32;
        let i = (((pt.0.x - w.rect.left) / wd.max(1)) as usize).min(n - 1);
        let mut st = WidgetState::default();
        w.kind.sync(&cfg, &mut st);
        if i != st.selected {
            let palette: &[(u8, u8, u8)] =
                if sw.palette == 0 { &widget::BG_SWATCHES } else { &widget::FG_SWATCHES };
            let (r, g, b) = palette[i];
            let color = sysoverlay_core::color::Rgb::new(r, g, b);
            let r = sw.cfg;
            apply_config(ctx, |cfg| r.set(cfg, Value::Color(Some(color))));
        }
        return;
    }
    // MetricSwatches（8 预设色 [+ 自动色 A] + 跟随默认 ≈ + 自定义 +）
    if let Some(ms) = w.kind.as_any().downcast_ref::<widget::MetricSwatchesWidget>() {
        let auto = ms.auto_cfg.is_some();
        let color_cfg = ms.cfg;
        let auto_cfg = ms.auto_cfg;
        let n = sw_n(auto);
        let wd = (w.rect.right - w.rect.left) / n as i32;
        let i = (((pt.0.x - w.rect.left) / wd.max(1)) as usize).min(n - 1);
        let follow_i = sw_follow(auto);
        let custom_i = sw_custom(auto);

        // 自定义取色：打开系统颜色对话框（ChooseColorW）。
        if i == custom_i {
            open_color_picker(ctx, color_cfg, auto_cfg);
            return;
        }

        let auto_on = auto_cfg
            .map(|r| matches!(r.get(&cfg), Value::Bool(true)))
            .unwrap_or(false);

        // 自动色 A 格：开启占用率分档变色。
        if auto && i == widget::SW_AUTO {
            if let Some(r) = auto_cfg {
                apply_config(ctx, |c| r.set(c, Value::Bool(true)));
            }
            return;
        }

        let mut st = WidgetState::default();
        w.kind.sync(&cfg, &mut st);
        let follow = st.value_f > 0.5;
        // 正确语义：点「跟随默认」写 None；点普通色块写所选颜色；
        // 自动色开启时点颜色/跟随需同时关掉 auto。
        if i != st.selected || auto_on || !(i == follow_i && follow) {
            let color = if i == follow_i {
                None
            } else {
                let (r, g, b) = widget::METRIC_SWATCHES[i];
                Some(sysoverlay_core::color::Rgb::new(r, g, b))
            };
            apply_config(ctx, |c| {
                let mut changed = false;
                if auto_on {
                    if let Some(r) = auto_cfg {
                        changed |= r.set(c, Value::Bool(false));
                    }
                }
                changed |= color_cfg.set(c, Value::Color(color));
                changed
            });
        }
        return;
    }
    // Disclosure（折叠箭头，G3）
    if let Some(d) = w.kind.as_any().downcast_ref::<widget::DisclosureWidget>() {
        if let widget::ButtonAction::ToggleMetricExpand(i) = d.action {
            dispatch(ctx, Action::ToggleMetricExpand(i));
        }
        return;
    }
    // Stepper（− / 值 / ＋，报告 §4）：按左三分之一减、右三分之一加。
    if let Some(st) = w.kind.as_any().downcast_ref::<widget::StepperWidget>() {
        let minus = widget::stepper_minus_rect(&w.rect, ctx.dpi);
        let plus = widget::stepper_plus_rect(&w.rect, ctx.dpi);
        let dir: f64 = if pt_in(&minus, pt.0) {
            -1.0
        } else if pt_in(&plus, pt.0) {
            1.0
        } else {
            0.0
        };
        if dir != 0.0 {
            let cur = match st.cfg.get(&cfg) {
                Value::Num(x) => x,
                _ => 0.0,
            };
            let nv = (cur + dir * st.step).clamp(st.min, st.max);
            dispatch(ctx, Action::CommitValue(st.cfg, nv));
        }
        return;
    }
    // Slider：点击不处理（走拖动/编辑）
}

// ── 标题栏按钮 ───────────────────────────────────────────

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

// ── 定时器辅助 ───────────────────────────────────────────

unsafe fn start_timer(hwnd: HWND, id: usize, ms: u32) {
    let _ = windows::Win32::UI::WindowsAndMessaging::SetTimer(hwnd, id, ms, None);
}

unsafe fn kill_timer(hwnd: HWND, id: usize) {
    let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(hwnd, id);
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
            // 不走 BeginPaint，必须显式 ValidateRect，否则 WM_PAINT 无限循环。
            if !paint_d2d(hwnd) {
                let _ = ValidateRect(hwnd, None);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_GETMINMAXINFO => {
            // resize 钳制：客户区最小/最大尺寸（经 DPI 缩放）。
            let mm = lparam.0 as *mut MINMAXINFO;
            if !mm.is_null() {
                let d = crate::dpi::get_system_dpi();
                (*mm).ptMinTrackSize.x = s(d, MIN_W);
                (*mm).ptMinTrackSize.y = s(d, MIN_H);
                (*mm).ptMaxTrackSize.x = s(d, MAX_W);
                (*mm).ptMaxTrackSize.y = s(d, MAX_H);
            }
            LRESULT(0)
        }
        WM_SIZE => {
            // 记录客户区尺寸：内容区宽度、滚动钳制、尺寸落盘都以它为准。
            if let Some(ctx) = ctx_of(hwnd) {
                ctx.client_size = ((lparam.0 & 0xFFFF) as i32, ((lparam.0 >> 16) & 0xFFFF) as i32);
                ctx.rebuild_pages();
            }
            LRESULT(0)
        }
        WM_EXITSIZEMOVE => {
            // 拖动结束：把最终客户区尺寸落盘（物理像素）。
            if let Some(ctx) = ctx_of(hwnd) {
                let (w, h) = ctx.client_size;
                window::modify_config(ctx.overlay, |c| c.settings_size = (w, h));
                if let Some(c) = ctx.cfg() {
                    let _ = config_io::save_config(&c);
                }
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            // 仅当光标位于内容区时滚动（侧边栏不接受滚轮）。
            // H4：越界用橡胶位移（可再挪一点后回弹），并记录速度供惯性吸附。
            if let Some(ctx) = ctx_of(hwnd) {
                let mut sc = POINT {
                    x: (lparam.0 & 0xFFFF) as i16 as i32,
                    y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
                };
                let _ = ScreenToClient(hwnd, &mut sc);
                let mut cr = RECT::default();
                let _ = GetClientRect(hwnd, &mut cr);
                if in_content(ctx.dpi, cr.right - cr.left, ClientPt(sc)) {
                    let raw = ((wparam.0 >> 16) & 0xFFFF) as i16 as i32;
                    let d = ctx.dpi;
                    let (_, vp_h) = content_viewport(d, ctx.client_size.0, ctx.client_size.1);
                    let max = (ctx.pages[ctx.page].h - vp_h).max(0);
                    let delta_px = raw / 120 * s(d, 48);
                    let nv = ctx.pages[ctx.page].scroll_y - delta_px;
                    ctx.pages[ctx.page].scroll_y = if nv < 0 {
                        -motion::rubber_band((-nv) as f32, vp_h as f32, 0.55) as i32
                    } else if nv > max {
                        max + motion::rubber_band((nv - max) as f32, vp_h as f32, 0.55) as i32
                    } else {
                        nv
                    };
                    // 惯性速度（px/s），供帧内动量投射。
                    ctx.scroll_vel = -(raw as f32) / 120.0 * s(d, 48) as f32 * 12.0;
                    ctx.scroll_spring.set(ctx.pages[ctx.page].scroll_y as f32);
                    invalidate_settings();
                }
            }
            LRESULT(0)
        }
        WM_NCHITTEST => {
            // lparam 是**屏幕**坐标，先转客户区坐标（参考 graph.rs / proc_list.rs）。
            let mut sc = POINT {
                x: (lparam.0 & 0xFFFF) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
            };
            let _ = ScreenToClient(hwnd, &mut sc);
            let pt = ClientPt(sc);
            let d = crate::dpi::get_system_dpi();
            let mut r = RECT::default();
            let _ = GetClientRect(hwnd, &mut r);
            let (cw, ch) = (r.right - r.left, r.bottom - r.top);
            // (a) 边框热区：WS_THICKFRAME 原生 resize（先判角，再判边）。
            let b = s(d, 6);
            let (on_l, on_r) = (pt.0.x < b, pt.0.x >= cw - b);
            let (on_t, on_b) = (pt.0.y < b, pt.0.y >= ch - b);
            if on_t && on_l { return LRESULT(HTTOPLEFT as isize); }
            if on_t && on_r { return LRESULT(HTTOPRIGHT as isize); }
            if on_b && on_l { return LRESULT(HTBOTTOMLEFT as isize); }
            if on_b && on_r { return LRESULT(HTBOTTOMRIGHT as isize); }
            if on_t { return LRESULT(HTTOP as isize); }
            if on_b { return LRESULT(HTBOTTOM as isize); }
            if on_l { return LRESULT(HTLEFT as isize); }
            if on_r { return LRESULT(HTRIGHT as isize); }
            // (b) 标题栏：按钮区 HTCLIENT（收鼠标消息），空白 HTCAPTION（拖动）。
            if titlebar::in_title_bar(d, pt.0) {
                if ctx_of(hwnd).map(|c| c.tb.pinned).unwrap_or(false) {
                    if titlebar::hit_button(d, cw, pt.0, true).is_some() {
                        return LRESULT(HTCLIENT as isize);
                    }
                    return LRESULT(HTTRANSPARENT as isize);
                }
                if titlebar::hit_button(d, cw, pt.0, false).is_some() {
                    return LRESULT(HTCLIENT as isize);
                }
                return LRESULT(HTCAPTION as isize);
            }
            LRESULT(HTCLIENT as isize)
        }
        WM_MOUSEMOVE => {
            let pt = ClientPt(POINT {
                x: (lparam.0 & 0xFFFF) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
            });
            if let Some(ctx) = ctx_of(hwnd) {
                let cpt = pt.to_content(ctx.dpi, ctx.pages[ctx.page].scroll_y);
                // 侧边栏悬停态（用于导航项高亮）
                ctx.nav_hover = if pt.0.x < s(ctx.dpi, SIDEBAR_W) as i32 {
                    sidebar_at(ctx.dpi, pt)
                } else {
                    None
                };
                // 拖动中：按当前 x 绝对落值（不再累积、不再看 origin）
                if let Interaction::Dragging { widget: idx, .. } = &ctx.interaction {
                    let idx = *idx;
                    if let Some(s) = ctx.pages[ctx.page].widgets[idx]
                        .kind
                        .as_any()
                        .downcast_ref::<widget::SliderWidget>()
                    {
                        let r = ctx.pages[ctx.page].widgets[idx].rect;
                        let tr = widget::track_rect(&r, ctx.dpi);
                        let new = widget::slider_value_from_x(
                            cpt.0.x as f32,
                            RectF::from_win32(tr),
                            s.min as f64,
                            s.max as f64,
                            s.step as f64,
                        );
                        dispatch(ctx, Action::LiveValue(s.cfg, new));
                    }
                    let _ = InvalidateRect(hwnd, None, false);
                    return LRESULT(0);
                }
                // 标题栏悬停态（翻页动画期间跳过 hover 探测，省一次控件命中+重绘）
                let d = ctx.dpi;
                let was_over = ctx.tb.over;
                ctx.tb.over = if ctx.page_anim.is_none() {
                    titlebar::in_title_bar(d, pt.0)
                } else {
                    false
                };
                let mut r = RECT::default();
                let _ = GetClientRect(hwnd, &mut r);
                let btn = if ctx.tb.over && ctx.page_anim.is_none() {
                    titlebar::hit_button(d, r.right - r.left, pt.0, ctx.tb.pinned)
                } else {
                    None
                };
                let changed = was_over != ctx.tb.over || ctx.tb.hover != btn;
                ctx.tb.hover = btn;
                if changed {
                    let _ = InvalidateRect(hwnd, None, false);
                }
                // 控件悬停（翻页动画期间跳过；只标脏新旧两控件）
                let new_hover = if ctx.page_anim.is_none() {
                    widget_at(ctx, cpt)
                } else {
                    None
                };
                let old_hover = ctx.active_widget();
                if new_hover != old_hover {
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
                ctx.nav_hover = None;
                // 光标离窗：步进器长按连发停止。
                ctx.stepper_hold = None;
                kill_timer(hwnd, STEPPER_TIMER);
                if let Interaction::Hover { widget } = &ctx.interaction {
                    ctx.invalidate_widget(*widget);
                    ctx.interaction = Interaction::Idle;
                }
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let pt = ClientPt(POINT {
                x: (lparam.0 & 0xFFFF) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
            });
            crate::probe::log(&format!("WM_LBUTTONDOWN pt=({},{})", pt.0.x, pt.0.y));
            let Some(ctx) = ctx_of(hwnd) else { return LRESULT(0) };
            let cpt = pt.to_content(ctx.dpi, ctx.pages[ctx.page].scroll_y);
            // 鼠标输入 → 退出键盘模式（焦点环仅键盘导航时出现，报告 §7.1）。
            ctx.keyboard_mode = false;
            ctx.focus = widget_at(ctx, cpt);

            // 正在编辑而点击的不是同一值框 → 提交并退出编辑
            if ctx.is_editing() {
                if let Interaction::Editing { widget: edit_idx } = &ctx.interaction {
                    let clicked_idx = widget_at(ctx, cpt);
                    let clicked_same = clicked_idx == Some(*edit_idx);
                    let clicked_val_box = clicked_idx.map(|i| {
                        pt_in(&value_box_rect(&ctx.pages[ctx.page].widgets[i].rect, ctx.dpi), cpt.0)
                    }).unwrap_or(false);
                    if !clicked_same || !clicked_val_box {
                        commit_edit(ctx);
                        kill_timer(hwnd, EDIT_TIMER);
                    }
                }
            }

            let d = ctx.dpi;
            let mut r = RECT::default();
            let _ = GetClientRect(hwnd, &mut r);
            // 1) 标题栏按钮
            if titlebar::in_title_bar(d, pt.0) {
                if let Some(btn) = titlebar::hit_button(d, r.right - r.left, pt.0, ctx.tb.pinned) {
                    on_title_btn(ctx, btn);
                    let _ = InvalidateRect(hwnd, None, false);
                    return LRESULT(0);
                }
            }
            // 2) 导航
            if let Some(page) = sidebar_at(ctx.dpi, pt) {
                dispatch(ctx, Action::SetPage(page));
                let _ = InvalidateRect(hwnd, None, false);
                return LRESULT(0);
            }
            // 3) 控件（内容坐标）
            if let Some(idx) = widget_at(ctx, cpt) {
                if ctx.cfg().is_some() {
                    // 滑杆：值框内 → 编辑；否则按下即跳值 + 进入拖动
                    let is_slider = ctx.pages[ctx.page].widgets[idx]
                        .kind
                        .as_any()
                        .downcast_ref::<widget::SliderWidget>()
                        .is_some();
                    if is_slider {
                        if pt_in(&value_box_rect(&ctx.pages[ctx.page].widgets[idx].rect, d), cpt.0) {
                            begin_edit(ctx, idx);
                            ctx.invalidate_widget(idx);
                            let _ = SetFocus(hwnd);
                            start_timer(hwnd, EDIT_TIMER, 500);
                            let _ = InvalidateRect(hwnd, None, false);
                            return LRESULT(0);
                        }
                        // 按下即跳值 + 进入拖动（Apple「按下即反馈」）
                        let s = ctx.pages[ctx.page].widgets[idx]
                            .kind
                            .as_any()
                            .downcast_ref::<widget::SliderWidget>()
                            .unwrap();
                        let r = ctx.pages[ctx.page].widgets[idx].rect;
                        let tr = widget::track_rect(&r, ctx.dpi);
                        let new = widget::slider_value_from_x(
                            cpt.0.x as f32,
                            RectF::from_win32(tr),
                            s.min as f64,
                            s.max as f64,
                            s.step as f64,
                        );
                        dispatch(ctx, Action::LiveValue(s.cfg, new));
                        ctx.interaction = Interaction::Dragging { widget: idx, origin: cpt.0 };
                        let _ = SetCapture(hwnd);
                        ctx.invalidate_widget(idx);
                        let _ = InvalidateRect(hwnd, None, false);
                        return LRESULT(0);
                    }
                    // 步进器：按下即 ±1 并进入长按连发（350ms 首延迟 → 60ms/步）；
                    // 抬起 / 失去捕获 / 离开窗口即停。UP 路径对持握中的步进器不再重复触发。
                    let step_dir = {
                        let w = &ctx.pages[ctx.page].widgets[idx];
                        if w.kind.as_any().is::<widget::StepperWidget>() {
                            stepper_dir(&w.rect, d, cpt)
                        } else {
                            0.0
                        }
                    };
                    if step_dir != 0.0 {
                        stepper_step(ctx, idx, step_dir);
                        ctx.stepper_hold = Some((idx, step_dir));
                        ctx.interaction = Interaction::Pressing { widget: idx };
                        let _ = SetCapture(hwnd);
                        start_timer(hwnd, STEPPER_TIMER, 350);
                        ctx.invalidate_widget(idx);
                        let _ = InvalidateRect(hwnd, None, false);
                        return LRESULT(0);
                    }
                    // H2：按下即进入 Pressing（视觉反馈），抬起且仍命中才触发动作。
                    ctx.interaction = Interaction::Pressing { widget: idx };
                    let _ = SetCapture(hwnd);
                    ctx.invalidate_widget(idx);
                    let _ = InvalidateRect(hwnd, None, false);
                } else {
                    let _ = InvalidateRect(hwnd, None, false);
                }
            } else {
                // 未命中任何控件：记录坐标，用于验证「绘制坐标 vs 命中坐标」是否同一系。
                crate::probe::log(&format!(
                    "click MISS client=({},{}) content=({},{}) content_top={}",
                    pt.0.x, pt.0.y, cpt.0.x, cpt.0.y, content_origin(ctx.dpi, ctx.pages[ctx.page].scroll_y).1
                ));
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Some(ctx) = ctx_of(hwnd) {
                if let Interaction::Dragging { widget, .. } = &ctx.interaction {
                    let idx = *widget;
                    // 松手 → 提交当前内存值（落盘）
                    if let Some(s) = ctx.pages[ctx.page].widgets[idx]
                        .kind
                        .as_any()
                        .downcast_ref::<widget::SliderWidget>()
                    {
                        if let Some(cfg) = ctx.cfg() {
                            if let Value::Num(n) = s.cfg.get(&cfg) {
                                dispatch(ctx, Action::CommitValue(s.cfg, n));
                            }
                        }
                    }
                    ctx.invalidate_widget(idx);
                    ctx.interaction = Interaction::Idle;
                    let _ = ReleaseCapture();
                    let _ = InvalidateRect(hwnd, None, false);
                } else if let Interaction::Pressing { widget } = &ctx.interaction {
                    // H2：抬起时若仍命中同一控件 → 执行动作；否则仅回弹。
                    // 步进器在按下时已触发（且长按连发），UP 不再重复。
                    let idx = *widget;
                    let held = ctx.stepper_hold.map(|(i, _)| i) == Some(idx);
                    ctx.stepper_hold = None;
                    kill_timer(hwnd, STEPPER_TIMER);
                    let pt = ClientPt(POINT {
                        x: (lparam.0 & 0xFFFF) as i16 as i32,
                        y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
                    });
                    let cpt = pt.to_content(ctx.dpi, ctx.pages[ctx.page].scroll_y);
                    let still = widget_at(ctx, cpt) == Some(idx);
                    ctx.interaction = Interaction::Idle;
                    let _ = ReleaseCapture();
                    if still && !held {
                        on_click_plain(ctx, idx, cpt);
                    }
                    ctx.invalidate_widget(idx);
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            if let Some(ctx) = ctx_of(hwnd) {
                // 捕获丢失（被系统打断）：长按连发一并停止。
                ctx.stepper_hold = None;
                kill_timer(hwnd, STEPPER_TIMER);
                if matches!(ctx.interaction, Interaction::Dragging { .. } | Interaction::Pressing { .. }) {
                    ctx.interaction = Interaction::Idle;
                }
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 as usize == ANIM_TIMER {
                // 动画主时钟：绕过 WM_PAINT 队列直接绘制（WM_PAINT 是最低优先级
                // 消息，鼠标移动会无限推迟它 → 动画帧饿死，这是卡顿根源）。
                // 本帧是否续表由 paint_d2d 帧末决定（收敛即 kill + 恢复时钟精度）。
                paint_d2d(hwnd);
            } else if wparam.0 as usize == EDIT_TIMER {
                if let Some(ctx) = ctx_of(hwnd) {
                    let need = ctx.edit.as_mut().map(|e| { e.caret = !e.caret; true }).unwrap_or(false);
                    if !need {
                        kill_timer(hwnd, EDIT_TIMER);
                    }
                    if ctx.is_editing() {
                        if let Some(idx) = ctx.active_widget() {
                            ctx.invalidate_widget(idx);
                        }
                    }
                }
                let _ = InvalidateRect(hwnd, None, false);
            } else if wparam.0 as usize == STATUS_TIMER {
                // 状态提示到期清除。
                if let Some(ctx) = ctx_of(hwnd) {
                    ctx.status = None;
                }
                kill_timer(hwnd, STATUS_TIMER);
                let _ = InvalidateRect(hwnd, None, false);
            } else if wparam.0 as usize == STEPPER_TIMER {
                // 步进器长按连发：仍在按住同一控件则步进并续期，否则停止。
                if let Some(ctx) = ctx_of(hwnd) {
                    let holding = match (&ctx.interaction, ctx.stepper_hold) {
                        (Interaction::Pressing { widget }, Some((hi, _))) => *widget == hi,
                        _ => false,
                    };
                    if holding {
                        let (idx, dir) = ctx.stepper_hold.unwrap();
                        stepper_step(ctx, idx, dir);
                        start_timer(hwnd, STEPPER_TIMER, STEPPER_REPEAT_MS);
                    } else {
                        ctx.stepper_hold = None;
                        kill_timer(hwnd, STEPPER_TIMER);
                    }
                }
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if let Some(ctx) = ctx_of(hwnd) {
                let vk = wparam.0 as u32;
                // 键盘输入 → 进入键盘模式（焦点环仅此时绘制，报告 §7.1）。
                if !ctx.is_editing() {
                    ctx.keyboard_mode = true;
                }
                match vk {
                    0x1B => {
                        // ESC：退出编辑 / 清除焦点。
                        if ctx.is_editing() {
                            if let Some(idx) = ctx.active_widget() {
                                ctx.invalidate_widget(idx);
                            }
                            ctx.interaction = Interaction::Idle;
                            kill_timer(hwnd, EDIT_TIMER);
                        }
                        ctx.focus = None;
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                    0x09 => {
                        // Tab：焦点链推进（Shift+Tab 反向）。
                        if !ctx.is_editing() {
                            let shift = unsafe {
                                windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x10) < 0
                            };
                            focus_advance(ctx, shift);
                            let _ = InvalidateRect(hwnd, None, false);
                        }
                    }
                    0x20 => {
                        // Space：触发焦点控件（开关切换 / 按钮点击）。
                        if !ctx.is_editing() {
                            activate_focused(ctx);
                            let _ = InvalidateRect(hwnd, None, false);
                        }
                    }
                    0x25 | 0x27 => {
                        // ← / →：滑杆 ±步进、步进器 ±1、分段左右切换。
                        if !ctx.is_editing() {
                            on_hkey(ctx, vk == 0x27);
                            let _ = InvalidateRect(hwnd, None, false);
                        }
                    }
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_CHAR => {
            if let Some(ctx) = ctx_of(hwnd) {
                if ctx.is_editing() {
                    if let Some(e) = ctx.edit.as_mut() {
                        let c = (wparam.0 & 0xFFFF) as u16;
                        // 首次键入直接替换整串旧值（select_all，人类直觉）
                        let is_input = (b'0' as u16..=b'9' as u16).contains(&c) || c == b'.' as u16;
                        if c == '\r' as u16 || c == '\n' as u16 {
                            commit_edit(ctx);
                            kill_timer(hwnd, EDIT_TIMER);
                        } else if c == 0x08 {
                            if e.select_all {
                                e.buf.clear();
                                e.select_all = false;
                            } else {
                                e.buf.pop();
                            }
                            e.caret = true;
                        } else if is_input {
                            if e.select_all {
                                e.buf.clear();
                                e.select_all = false;
                            }
                            if e.buf.len() < 16 {
                                e.buf.push(c);
                            }
                            e.caret = true;
                        }
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
                // 失焦 = 提交（旧实现直接丢弃编辑内容，输入白白消失）
                if ctx.is_editing() {
                    commit_edit(ctx);
                    kill_timer(hwnd, EDIT_TIMER);
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_DIAG_CLICK => {
            if let Some(ctx) = ctx_of(hwnd) {
                // 切到颜色页（index 2）并模拟点击 CPU 开关（id 200）
                if let Some((p, idx)) = ctx.pages.iter().enumerate()
                    .find_map(|(p, page)| page.widgets.iter().position(|w| w.id == IDC_DIAG_CPU).map(|idx| (p, idx)))
                {
                    ctx.page = p;
                    ctx.page_anim = None;
                    let cfg_ref = ctx.pages[p].widgets[idx].kind.cfg();
                    if let Some(r) = cfg_ref {
                        dispatch(ctx, Action::Toggle(r));
                    }
                    let _ = InvalidateRect(hwnd, None, false);
                } else {
                    crate::probe::log("diag_click: CPU widget not found");
                }
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            if let Some(ctx) = ctx_of(hwnd) {
                if matches!(&ctx.interaction, Interaction::Hover { .. })
                    || ctx.tb.hover.is_some()
                    || ctx.is_editing()
                {
                    SetCursor(ctx.hand);
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_SETTINGCHANGE => {
            if let Some(ctx) = ctx_of(hwnd) {
                // 跟随系统时跟随 OS 变化；强制浅/深色时保持用户选择。
                let app = ctx.cfg().map(|c| c.appearance).unwrap_or_default();
                ctx.theme = Theme::resolve(app);
                apply_window_chrome(hwnd, ctx.theme.dark);
                // 主题色在绘制时从 ctx.theme 读取，无需重建页面；重绘即可。
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            // 隐藏前 flush pending 改动（防抖未到期的最后一次变更不丢）
            if let Some(ctx) = ctx_of(hwnd) {
                if ctx.save.has_pending() {
                    flush_config(ctx.overlay);
                }
            }
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_DESTROY => {
            crate::probe::log("settings WM_DESTROY");
            if let Some(ctx) = ctx_of(hwnd) {
                if ctx.save.has_pending() {
                    crate::probe::log("settings destroy: flushing pending save");
                    flush_config(ctx.overlay);
                }
            }
            let ptr = GetWindowLongPtrW(hwnd, GWLP_UD) as *mut SettingsCtx;
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
                SetWindowLongPtrW(hwnd, GWLP_UD, 0);
            }
            SETTINGS_HWND.store(-1, Ordering::SeqCst);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ── D2D 绘制 ─────────────────────────────────────────────

fn body_px(dpi: u32) -> f32 {
    13.0 * dpi as f32 / 72.0
}

fn small_px(dpi: u32) -> f32 {
    11.0 * dpi as f32 / 72.0
}

fn c(_t: &Theme, v: u32) -> ColorF {
    ColorF::from_colorref(v, 1.0)
}

/// 全量 D2D 绘制一帧。返回 false = D2D 链路不可用（下一帧自动重建重试）。
fn paint_d2d(hwnd: HWND) -> bool {
    // 模态对话框（系统取色）打开中：跳过自绘，防止消息循环重入争用 Gfx。
    if MODAL_DIALOG.load(Ordering::SeqCst) {
        return true;
    }
    let t0 = std::time::Instant::now();
    let Some(ctx) = ctx_of(hwnd) else {
        crate::probe::log("paint_d2d: ctx_of failed");
        return false;
    };
    let mut r = RECT::default();
    unsafe { let _ = GetClientRect(hwnd, &mut r); }
    let win_w = r.right - r.left;
    let win_h = r.bottom - r.top;
    if win_w <= 0 || win_h <= 0 {
        return true; // 无效尺寸：当成功处理（ValidateRect 由调用方完成）
    }
    let lang = ctx.cfg().map(|c| c.lang).unwrap_or_else(global_lang);
    let theme = ctx.theme;
    let dpi = ctx.dpi;

    // 帧间 dt（上限 ~1/30s：帧被推迟时防弹簧大步跳格）。
    let now = Instant::now();
    let dt = (now - ctx.last_frame).as_secs_f32().clamp(0.0, 0.033);
    ctx.last_frame = now;

    // 布局类弹簧（折叠卡展开度 / 侧栏选中胶囊）：动则每帧重排；
    // 收敛的那一帧再补一次（收起时要把已展开的内容行真正移除）。
    let moving = step_layout_anims(ctx, dt);
    if moving || ctx.layout_anim_active {
        ctx.rebuild_pages();
    }
    ctx.layout_anim_active = moving;

    // H4：滚动惯性——动量投射 → 行吸附 → 弹簧趋近（收敛即停）。
    if ctx.scroll_vel.abs() > 1.0 {
        let (_, vp_h) = content_viewport(dpi, ctx.client_size.0, ctx.client_size.1);
        let page = &mut ctx.pages[ctx.page];
        let max = (page.h - vp_h).max(0);
        let projected = motion::project(ctx.scroll_vel);
        let row = s(dpi, ROW_H) as f32;
        let raw = (page.scroll_y as f32 + projected).clamp(0.0, max as f32);
        let snapped = if row > 0.0 { (raw / row).round() * row } else { raw };
        let target = snapped.clamp(0.0, max as f32);
        let settled = ctx.scroll_spring.step(target, dt, motion::K_SHEET, motion::C_SHEET);
        page.scroll_y = ctx.scroll_spring.x.round() as i32;
        if settled {
            ctx.scroll_vel = 0.0;
        }
    }

    let (anim_dx, alpha) = anim_params(ctx, dt);

    // 字段级不相交借用：闭包只捕获 pages/交互快照，与 &mut ctx.gfx 无重叠。
    let pages = &ctx.pages;
    let page_idx = ctx.page;
    let scroll_y = pages[page_idx].scroll_y;
    let interaction = ctx.interaction.clone();
    let edit = ctx.edit.clone();
    let cfg = ctx.cfg();
    let tb = &ctx.tb;
    let nav_hover = ctx.nav_hover;
    let metric_expanded = &ctx.metric_expanded;
    let transitions = &mut ctx.transitions;
    let nav_t = ctx.nav_sel.x;
    // 瞬态状态提示文案（窗口层绘制，不随内容滚动）。
    let status_msg = ctx.status.as_ref().map(|(s, _)| s.clone());
    // 置 true 表示本帧仍有未收敛的过渡弹簧 → 需再请求一帧。
    let mut anim_active = false;

    let (ox, oy) = content_origin(dpi, scroll_y);
    let (vp_w, _vp_h) = content_viewport(dpi, win_w, win_h);

    let presented = ctx.gfx.frame(|rt, gfx| {
        unsafe { rt.Clear(Some(&c(&theme, theme.window_bg).d2d())) };
        // 卡片均为不透明填充 → 全程 ClearType（与旧 GDI quality=5 一致）
        gfx.set_text_aa(rt, true);

        let bounds = RectF::new(0.0, 0.0, win_w as f32, win_h as f32);
        let faded = if alpha < 1.0 { gfx.push_fade(rt, bounds, alpha) } else { false };

        // 侧边栏（固定在左侧，不随内容滚动）
        paint_sidebar(rt, gfx, &theme, dpi, win_h, lang, page_idx, nav_hover, nav_t);

        // 内容区（翻页动画：水平滑入）。先裁到视口，再平移绘制。
        let clip = RectF::new(ox as f32, 0.0, (ox + vp_w) as f32, win_h as f32);
        unsafe {
            let _ = rt.PushAxisAlignedClip(
                &clip.d2d(),
                windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            );
        }
        gfx.fill_rect(rt, clip, c(&theme, theme.content_bg));
        gfx.set_translate(rt, anim_dx, 0.0);
        anim_active = paint_page(
            rt,
            gfx,
            pages,
            page_idx,
            &interaction,
            edit.as_ref(),
            cfg.as_ref(),
            &theme,
            dpi,
            ox as f32,
            oy as f32,
            body_px(dpi),
            caption_px(dpi),
            metric_expanded,
            transitions,
            dt,
            ctx.focus,
            ctx.keyboard_mode,
        );
        gfx.reset_transform(rt);
        unsafe { rt.PopAxisAlignedClip() };

        // 滚动条（内容溢出时淡入，浮于内容之上）
        paint_scrollbar(rt, gfx, &theme, dpi, win_w, win_h, scroll_y, pages[page_idx].h);
        // 窗口控制按钮（右上角，浮于内容区之上，不占布局高度）
        paint_window_controls(rt, gfx, &theme, dpi, win_w, tb);

        // 瞬态状态提示：右下角小胶囊（先量宽再画底；卡片底色 + 描边 + 正文色）。
        if let Some(msg) = &status_msg {
            let px = caption_px(dpi);
            if let Some((tw, _th)) = gfx.measure_text(msg, px, 400) {
                let pad = s(dpi, 12) as f32;
                let h = s(dpi, 26) as f32;
                let w = tw + 2.0 * pad;
                let margin = s(dpi, 14) as f32;
                let r = RectF::new(
                    win_w as f32 - margin - w,
                    win_h as f32 - margin - h,
                    win_w as f32 - margin,
                    win_h as f32 - margin,
                );
                gfx.fill_round(rt, r, h / 2.0, c(&theme, theme.card));
                gfx.stroke_round(rt, r, h / 2.0, c(&theme, theme.card_stroke), 1.0);
                gfx.text(rt, msg, px, 400, HAlign::Center, r, c(&theme, theme.label), true);
            }
        }

        if faded {
            gfx.pop_fade(rt);
        }
    });
    // 帧探针：每 64 帧采样一次耗时（避免日志风暴）；失败帧每次都打（低频且关键）。
    static PAINT_FRAME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = PAINT_FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // H2/H3/H4：仍有未收敛动画 → 维持 ANIM_TIMER(16ms) 主时钟直画下一帧。
    // 动画期间提升系统时钟精度到 1ms（SetTimer 粒度默认 15.6ms，不提升则
    // 16ms 表实际抖到 ~31ms）；收敛即停表并恢复，防引用计数泄漏与空转烧 CPU。
    let animating = presented
        && (anim_active || moving || ctx.page_anim.is_some() || ctx.scroll_vel.abs() > 1.0);
    if animating && !ctx.hires {
        unsafe {
            windows::Win32::Media::timeBeginPeriod(1);
            start_timer(hwnd, ANIM_TIMER, 16);
        }
        ctx.hires = true;
    } else if !animating && ctx.hires {
        unsafe {
            kill_timer(hwnd, ANIM_TIMER);
            windows::Win32::Media::timeEndPeriod(1);
        }
        ctx.hires = false;
    }
    if presented {
        if n % 64 == 0 {
            crate::probe::log_lazy(|| {
                format!(
                    "settings paint frame#{} {}x{} page={} dx={:.1} alpha={:.2} us={}",
                    n, win_w, win_h, page_idx, anim_dx, alpha,
                    t0.elapsed().as_micros()
                )
            });
        }
        unsafe { let _ = ValidateRect(hwnd, None); }
    } else {
        crate::probe::log_lazy(|| {
            format!(
                "settings paint FAILED frame#{} page={} dx={:.1} alpha={:.2}",
                n, page_idx, anim_dx, alpha
            )
        });
    }
    presented
}

fn global_lang() -> sysoverlay_core::types::Lang {
    use sysoverlay_core::types::Lang;
    if crate::i18n::global_lang() == Lang::Zh { Lang::Zh } else { Lang::En }
}

fn paint_window_controls(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    t: &Theme,
    dpi: u32,
    win_w: i32,
    tb: &TitleBarState,
) {
    // 按钮（仅右上角；标题已移入侧边栏，此处不画顶栏底色）
    if tb.pinned {
        if tb.over {
            if let Some((b, r)) = titlebar::btn_rects(dpi, win_w, true).into_iter().next() {
                let rf = RectF::from_i32(r.left, r.top, r.right, r.bottom);
                let fill = if tb.hover == Some(b) { c(t, t.accent_hover) } else { c(t, t.accent) };
                gfx.fill_round(rt, rf, s(dpi, 6) as f32, fill);
                let label = crate::i18n::t(global_lang(), "TbUnlock");
                gfx.text(rt, label, small_px(dpi), 400, HAlign::Center, rf, ColorF::rgba(255, 255, 255, 1.0), true);
            }
        }
    } else {
        for (b, r) in titlebar::btn_rects(dpi, win_w, false) {
            let rf = RectF::from_i32(r.left, r.top, r.right, r.bottom);
            let fill = if tb.hover == Some(b) { c(t, t.button_bg_hover) } else { c(t, t.button_bg) };
            gfx.fill_round(rt, rf, s(dpi, 6) as f32, fill);
            let cx = (r.left + r.right) as f32 / 2.0;
            let cy = (r.top + r.bottom) as f32 / 2.0;
            let m = s(dpi, 3) as f32;
            match b {
                TitleBtn::Pin => {
                    // 用户提供的「固定」图钉 SVG（icons.rs 内嵌路径）。
                    let sz = (r.bottom - r.top) as f32 - 2.0 * m;
                    icons::draw(
                        rt,
                        gfx,
                        icons::IconKind::Pin,
                        RectF::new(cx - sz / 2.0, cy - sz / 2.0, cx + sz / 2.0, cy + sz / 2.0),
                        c(t, t.label),
                    );
                }
                TitleBtn::Min => {
                    let bw = s(dpi, 10) as f32;
                    let bh = s(dpi, 2) as f32;
                    gfx.fill_round(
                        rt,
                        RectF::new(cx - bw / 2.0, cy - bh / 2.0, cx + bw / 2.0, cy + bh / 2.0),
                        bh / 2.0,
                        c(t, t.label),
                    );
                }
                TitleBtn::Close => {
                    let k = s(dpi, 4) as f32;
                    gfx.line(rt, cx - k, cy - k, cx + k, cy + k, c(t, t.label), 1.0);
                    gfx.line(rt, cx + k, cy - k, cx - k, cy + k, c(t, t.label), 1.0);
                }
                TitleBtn::Unlock => {}
            }
        }
    }
}

/// 分节标题字号（与 small_px 同量级；独立命名便于 I1 统一光学层级）。
fn caption_px(dpi: u32) -> f32 {
    11.0 * dpi as f32 / 72.0
}

/// 侧边栏标题字号（I1：20pt）。
fn display_px(dpi: u32) -> f32 {
    20.0 * dpi as f32 / 72.0
}

/// 侧边栏（D3）：底色 + 标题 + 6 个导航项（图标 + 文案 + 胶囊选中/悬停）。
/// 固定在左侧，不随内容滚动；几何必须与命中测试 `sidebar_at` 保持一致。
fn paint_sidebar(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    t: &Theme,
    dpi: u32,
    win_h: i32,
    lang: sysoverlay_core::types::Lang,
    _page_idx: usize,
    nav_hover: Option<usize>,
    sel_t: f32,
) {
    let x1 = s(dpi, SIDEBAR_W) as f32;
    let hc = motion::high_contrast();
    gfx.fill_rect(rt, RectF::new(0.0, 0.0, x1, win_h as f32), c(t, t.sidebar_bg));

    // 标题（顶带左半侧，与右上角窗口按钮同高）。I1：20pt / 600。
    let pad = s(dpi, SIDEBAR_PAD) as f32;
    gfx.text(
        rt,
        crate::i18n::t(lang, "SbTitle"),
        display_px(dpi),
        600,
        HAlign::Leading,
        RectF::new(pad, s(dpi, 8) as f32, x1 - pad, s(dpi, 8 + 30) as f32),
        c(t, t.label),
        true,
    );

    let isz = s(dpi, SB_ICON) as f32;
    let item_h = s(dpi, SB_ITEM_H) as f32;
    let step = s(dpi, SB_ITEM_H + SB_ITEM_GAP) as f32;
    let top = s(dpi, 8 + 30 + 12) as f32;
    let active = (sel_t.round() as usize).min(5);
    // 悬停底（画在选中胶囊之下）
    if let Some(h) = nav_hover {
        let hy = top + h as f32 * step;
        gfx.fill_round(
            rt,
            RectF::new(pad, hy, x1 - pad, hy + item_h),
            item_h / 2.0,
            c(t, t.sidebar_hover).with_alpha(0.07),
        );
    }
    // 选中胶囊：沿 `sel_t` 滑到目标项（而非瞬间跳格）。
    let py = top + sel_t.clamp(0.0, 5.0) * step;
    let pill = if hc {
        c(t, t.sidebar_selected)
    } else {
        c(t, t.sidebar_selected).with_alpha(0.12)
    };
    gfx.fill_round(rt, RectF::new(pad, py, x1 - pad, py + item_h), item_h / 2.0, pill);
    for i in 0..6 {
        let y = top + i as f32 * step;
        let selected = i == active;
        let ix = pad + s(dpi, 6) as f32;
        let iy = y + (item_h - isz) / 2.0;
        let icon_color = if selected { c(t, t.accent) } else { c(t, t.secondary) };
        icons::draw(
            rt,
            gfx,
            icons::IconKind::ALL[i],
            RectF::new(ix, iy, ix + isz, iy + isz),
            icon_color,
        );
        let tx = ix + isz + s(dpi, SB_ICON_TEXT_GAP) as f32;
        let label_color = if selected { c(t, t.accent) } else { c(t, t.label) };
        gfx.text(
            rt,
            page_caption(i, lang),
            caption_px(dpi),
            500,
            HAlign::Leading,
            RectF::new(tx, y, x1 - pad, y + item_h),
            label_color,
            true,
        );
    }
}

/// 浮动滚动条（D3f）：仅当内容高于视口时显示；
/// 拇指位置 = (-scroll_y) / max_scroll，拇指高 = 视口/内容比例。
#[allow(clippy::too_many_arguments)]
fn paint_scrollbar(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    t: &Theme,
    dpi: u32,
    win_w: i32,
    win_h: i32,
    scroll_y: i32,
    content_h: i32,
) {
    if content_h <= win_h {
        return;
    }
    let inset = s(dpi, SCROLLBAR_INSET) as f32;
    let w = s(dpi, SCROLLBAR_W) as f32;
    let track_h = win_h as f32 - 2.0 * inset;
    let max_scroll = (content_h - win_h) as f32;
    let thumb_h = (track_h * win_h as f32 / content_h as f32)
        .max(s(dpi, 24) as f32)
        .min(track_h);
    let ratio = ((-scroll_y) as f32 / max_scroll).clamp(0.0, 1.0);
    let top = inset + ratio * (track_h - thumb_h);
    let x = win_w as f32 - inset - w;
    gfx.fill_round(
        rt,
        RectF::new(x, top, x + w, top + thumb_h),
        w / 2.0,
        c(t, t.secondary).with_alpha(0.45),
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_page(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    pages: &[Page; 6],
    page_idx: usize,
    interaction: &Interaction,
    edit: Option<&EditState>,
    cfg: Option<&sysoverlay_core::config::Config>,
    t: &Theme,
    dpi: u32,
    ox: f32,
    oy: f32,
    body: f32,
    caption: f32,
    _metric_expanded: &[bool; 5],
    transitions: &mut std::collections::HashMap<(usize, i32), Transition>,
    dt: f32,
    focus: Option<usize>,
    keyboard_mode: bool,
) -> bool {
    let mut active = false;
    let page = &pages[page_idx];
    // `card.rect` / `widget.rect` 均为**内容坐标**；绘制统一加内容区原点 (ox, oy)。
    // 红线 R7：漏加会让内容与侧边栏/标题重叠。折叠卡（G1-G3）将消费 _metric_expanded。

    // 页标题 + 副标题（报告 §11 每页 h5 + p；随内容滚动）。
    // 内容坐标：ox 已含侧栏偏移，这里只加内容左边距。
    let title_l = s(dpi, layout::CONTENT_PAD_X) as f32 + ox;
    let title_r = if let Some(first) = page.cards.first() {
        first.rect.right as f32 + ox
    } else {
        title_l + s(dpi, 400) as f32
    };
    let ty = s(dpi, layout::CONTENT_PAD_TOP) as f32 + oy;
    gfx.text(
        rt,
        page.title,
        body * 1.3,
        600,
        HAlign::Leading,
        RectF::new(title_l, ty, title_r, ty + s(dpi, layout::TITLE_H) as f32),
        c(t, t.label),
        true,
    );
    let sy_ = ty + s(dpi, layout::TITLE_H + layout::TITLE_SUB_GAP) as f32;
    gfx.text(
        rt,
        page.subtitle,
        caption,
        400,
        HAlign::Leading,
        RectF::new(title_l, sy_, title_r, sy_ + s(dpi, layout::SUB_H) as f32),
        c(t, t.secondary),
        true,
    );

    for card in &page.cards {
        // 分节标题
        let cap = RectF::new(
            (card.rect.left + s(dpi, 2)) as f32 + ox,
            (card.rect.top - s(dpi, CAPTION_H + CAPTION_GAP)) as f32 + oy,
            card.rect.right as f32 + ox,
            (card.rect.top - s(dpi, CAPTION_GAP)) as f32 + oy,
        );
        gfx.text(rt, card.caption, caption, 400, HAlign::Leading, cap, c(t, t.secondary), true);

        // 圆角卡片
        let card_rect = RectF::from_i32(card.rect.left, card.rect.top, card.rect.right, card.rect.bottom)
            .offset(ox, oy);
        gfx.fill_round(rt, card_rect, s(dpi, CARD_RADIUS) as f32, c(t, t.card));
        // I1：高对比度下卡片描边用全色强化边界。
        let stroke = if motion::high_contrast() { c(t, t.label) } else { c(t, t.card_stroke) };
        gfx.stroke_round(rt, card_rect, s(dpi, CARD_RADIUS) as f32, stroke, 1.0);
        // 内容裁剪到卡片内：折叠动画 = 卡片长高 + 内容被揭开。
        unsafe {
            let _ = rt.PushAxisAlignedClip(
                &card_rect.d2d(),
                windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            );
        }

        // 行（行高可变：位置由布局写入 row.y_off / row.h）
        for row in &card.rows {
            let row_top = (card.rect.top + s(dpi, CARD_PAD)) as f32 + row.y_off as f32 + oy;
            // 分组分隔线（监控项折叠卡分节：贴行顶的细线）。
            if row.divider {
                let ly = row_top + s(dpi, 2) as f32;
                gfx.line(
                    rt,
                    (card.rect.left + s(dpi, 12)) as f32 + ox,
                    ly,
                    (card.rect.right - s(dpi, 12)) as f32 + ox,
                    ly,
                    c(t, t.card_stroke),
                    1.0,
                );
            }
            if !row.label.is_empty() {
                let first_widget_left = row
                    .widgets
                    .first()
                    .map(|&i| (page.widgets[i].rect.left - s(dpi, 8)) as f32)
                    .unwrap_or((card.rect.right - s(dpi, 12)) as f32);
                let lr = RectF::new(
                    (card.rect.left + s(dpi, 12)) as f32 + ox,
                    row_top,
                    first_widget_left + ox,
                    row_top + s(dpi, ROW_H) as f32,
                );
                // hint 行：小一号次级色（V03 辅助说明）；普通行：正文标签色。
                let (px, col) =
                    if row.hint { (caption, t.secondary) } else { (body, t.label) };
                // 轴对齐裁剪替代旧 DT_END_ELLIPSIS（标签列宽充裕时无差异）
                unsafe { let _ = rt.PushAxisAlignedClip(&lr.d2d(), windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE); }
                gfx.text(rt, row.label, px, if row.hint { 400 } else { 500 }, HAlign::Leading, lr, c(t, col), true);
                unsafe { rt.PopAxisAlignedClip() };
                // 行内说明（报告 §11：hint 紧贴标签下方的小字；可选第二行）。
                // 行距 = HINT_EXTRA（与布局预留的行高一致，避免与下一行重叠）。
                // 宽度横跨到卡片右缘：hint 位于行顶部 28 带之下，不会与控件重叠。
                if row.hint_text.is_some() || row.hint2.is_some() {
                    let mut hy = row_top + s(dpi, ROW_H) as f32;
                    let line_h = s(dpi, layout::HINT_EXTRA) as f32;
                    let hint_r = (card.rect.right - s(dpi, 12)) as f32 + ox;
                    for line in [row.hint_text, row.hint2].into_iter().flatten() {
                        let hr = RectF::new(
                            (card.rect.left + s(dpi, 12)) as f32 + ox,
                            hy,
                            hint_r,
                            hy + line_h,
                        );
                        unsafe { let _ = rt.PushAxisAlignedClip(&hr.d2d(), windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE); }
                        gfx.text(rt, line, caption, 400, HAlign::Leading, hr, c(t, t.secondary), true);
                        unsafe { rt.PopAxisAlignedClip() };
                        hy += line_h;
                    }
                }
            }
            for &wi in &row.widgets {
                let mut ws = WidgetState::default();
                if let Some(cfg) = cfg {
                    page.widgets[wi].kind.sync(cfg, &mut ws);
                }
                // 逐控件推进三根弹簧：hover / press / val（值·选中项）。
                let is_hover = matches!(interaction, Interaction::Hover { widget } if *widget == wi)
                    || matches!(interaction, Interaction::Pressing { widget } if *widget == wi);
                let is_press = matches!(interaction, Interaction::Pressing { widget } if *widget == wi);
                let is_drag = matches!(interaction, Interaction::Dragging { widget, origin: _ } if *widget == wi);
                let (k, damp) = if motion::animations_enabled() {
                    (motion::K_UI, motion::C_UI)
                } else {
                    (motion::K_REDUCED, motion::C_REDUCED)
                };
                let target_v = page.widgets[wi].kind.anim_target(&ws);
                let key = (page_idx, page.widgets[wi].id);
                let tr = transitions.entry(key).or_default();
                if !tr.val_init {
                    tr.val.set(target_v);
                    tr.val_init = true;
                }
                let sh = tr.hover.step(if is_hover { 1.0 } else { 0.0 }, dt, k, damp);
                let sp = tr.press.step(if is_press { 1.0 } else { 0.0 }, dt, k, damp);
                // 拖动的滑杆直接跟手（不插值）；其余切换一律「滑过去」。
                let sv = if is_drag {
                    tr.val.set(target_v);
                    true
                } else {
                    tr.val.step(target_v, dt, k, damp)
                };
                ws.hover_t = tr.hover.x.clamp(0.0, 1.0);
                ws.press_t = tr.press.x.clamp(0.0, 1.0);
                ws.anim_v = tr.val.x;
                if !(sh && sp && sv) {
                    active = true;
                }
                ws.hovered = ws.hover_t > 0.5;
                ws.editing = matches!(interaction, Interaction::Editing { widget } if *widget == wi);
                // 键盘焦点环仅键盘模式绘制（报告 §7.1 should_draw_focus_ring）。
                ws.focused = keyboard_mode && focus == Some(wi);
                widget::paint_widget(rt, gfx, &page.widgets[wi], &ws, dpi, t, ox, oy, body, caption, edit, cfg);
            }
        }

        // 脚注（多行换行；位置 = 行区底部，行高可变后由 rows_h 给出）
        if let Some(foot) = card.footnote {
            let rows_bottom = (card.rect.top + s(dpi, CARD_PAD)) as f32 + card.rows_h as f32 + oy;
            let fr = RectF::new(
                (card.rect.left + s(dpi, 12)) as f32 + ox,
                rows_bottom,
                (card.rect.right - s(dpi, 12)) as f32 + ox,
                rows_bottom + s(dpi, FOOTNOTE_H) as f32,
            );
            gfx.text_wrap(rt, foot, caption, 400, HAlign::Leading, fr, c(t, t.secondary));
        }
        unsafe { rt.PopAxisAlignedClip() };
    }
    active
}
