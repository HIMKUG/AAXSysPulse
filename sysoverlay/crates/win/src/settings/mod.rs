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
mod layout;
mod state;
mod tasks;
mod widget;

pub use tasks::SAVE_DEBOUNCE_MS;

use std::sync::atomic::{AtomicIsize, Ordering};
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
    CreateWindowExW, DefWindowProcW, GetClientRect, LoadCursorW, PostMessageW, RegisterClassExW,
    SetCursor, SetWindowPos, ShowWindow, HCURSOR, IDC_HAND, SW_HIDE, SW_SHOW, HWND_NOTOPMOST,
    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WINDOW_EX_STYLE, WNDCLASSEXW, WS_POPUP,
};

use crate::gfx_d2d::{ColorF, Gfx, HAlign, RectF};
use crate::theme::Theme;
use crate::titlebar::{self, TitleBarState, TitleBtn};
use crate::window::{self, WM_RECONFIG};
use crate::{autostart, config_io};
use state::{Action, EditState, Interaction, Page, SliderFx, WidgetState};
use tasks::DebouncedSave;

use cfg_ref::{CfgRef, Value};
use layout::{
    build_pages, content_top_y, page_caption, s, CARD_PAD, CARD_RADIUS, CAPTION_GAP, CAPTION_H,
    CLIENT_W, FOOTNOTE_H, MARGIN_X, NAV_GAP, NAV_H, ROW_H,
};
use state::pt_in;
use widget::{track_rect, value_box_rect, SW_FOLLOW, SW_N};

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

/// 诊断点击目标：颜色页 CPU 开关（build_pages 中第一个指标 Toggle 的 id）。
const IDC_DIAG_CPU: i32 = 200;

// 定时器
const PAGE_TIMER: usize = 11;
const EDIT_TIMER: usize = 12;
const DRAG_TIMER: usize = 13;

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

// ── 坐标 newtype：编译期锁死双坐标系 ─────────────────────

/// 客户区坐标（WM_MOUSEMOVE / WM_NCHITTEST 的原始 pt）。
#[derive(Clone, Copy, Debug)]
pub struct ClientPt(pub POINT);

/// 内容坐标（相对内容区顶部；`Widget.rect` / 空间网格用这套）。
#[derive(Clone, Copy, Debug)]
pub struct ContentPt(pub POINT);

impl ClientPt {
    /// 客户区 → 内容区（正文命中测试唯一入口）。
    pub fn to_content(self, dpi: u32) -> ContentPt {
        ContentPt(POINT { x: self.0.x, y: self.0.y - content_top_y(dpi) })
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
    /// 摇杆弹簧物理状态（拖动/回中期间 Some，其余 None = 零空闲开销）。
    fx: Option<SliderFx>,
    tracking: bool,
    /// 自定义标题栏状态。
    tb: TitleBarState,
    /// 待重绘的控件下标集合（None = 全量重绘）。
    dirty: Option<Vec<usize>>,
    /// 空间网格索引：每页一个。
    spatial_grids: Vec<state::SpatialGrid>,
    /// D2D 渲染底座（唯一资源持有者）。
    gfx: Gfx,
    /// 防抖落盘状态机。
    save: DebouncedSave,
    /// 进程内单调时钟起点（DebouncedSave 的 now 来源）。
    clock0: Instant,
    /// 上次通知悬浮窗重排的毫秒时间戳（LiveValue 拖动节流用）。
    last_reconfig_ms: u64,
}

#[derive(Clone, Copy)]
struct PageAnim {
    t0: Instant,
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
            build_pages(&mut self.pages, &cfg, lang, dpi);
            let page_h = self.pages.iter().map(|p| p.h).max().unwrap_or(0).max(s(dpi, 200));
            let mut grids = Vec::with_capacity(6);
            for page in &self.pages {
                let mut grid = state::SpatialGrid::new(s(dpi, layout::CLIENT_W), page_h, s(dpi, 30));
                for (idx, w) in page.widgets.iter().enumerate() {
                    grid.insert(idx, w.rect);
                }
                grids.push(grid);
            }
            self.spatial_grids = grids;
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

/// 翻页动画进度：返回 (水平偏移px, 不透明度0..1)。
fn anim_params(ctx: &SettingsCtx) -> (f32, f32) {
    match &ctx.page_anim {
        Some(anim) => {
            let dur_ms = ctx.cfg().map(|c| c.anim_ms).unwrap_or(300).max(1) as f32;
            let t = (anim.t0.elapsed().as_secs_f32() * 1000.0 / dur_ms).clamp(0.0, 1.0);
            let e = 1.0 - (1.0 - t).powi(3); // 三次方缓出
            ((1.0 - e) * 24.0, e)
        }
        None => (0.0, 1.0),
    }
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
        let h = layout::window_total_h(dpi);

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
            fx: None,
            tracking: false,
            tb: TitleBarState::default(),
            dirty: None,
            spatial_grids: Vec::new(),
            gfx,
            save: DebouncedSave::default(),
            clock0: Instant::now(),
            last_reconfig_ms: 0,
        };
        ctx.rebuild_pages();
        crate::probe::log(&format!(
            "settings layout w={w} h={h} content_top={} page_h={:?} widgets={:?}",
            content_top_y(dpi),
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

/// 导航条命中（客户区坐标；导航自己带 TB_H+NAV_GAP 偏移）。
fn nav_at(ctx: &SettingsCtx, pt: ClientPt) -> Option<usize> {
    let d = ctx.dpi;
    let top = s(d, crate::titlebar::TB_H) + s(d, NAV_GAP);
    if pt.0.y < top || pt.0.y >= top + s(d, NAV_H) {
        return None;
    }
    let n = 6;
    let seg = s(d, CLIENT_W) / n;
    let i = (pt.0.x / seg.max(1)) as usize;
    if (i as i32) < n { Some(i) } else { None }
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
                slider_cancel(ctx, find_hwnd(ctx));
                ctx.page = p;
                ctx.page_anim = Some(PageAnim { t0: Instant::now() });
                // 33ms ≈ 30fps：翻页过渡对流畅度足够，且比 16ms 少一半重绘负载
                unsafe { start_timer(find_hwnd(ctx), PAGE_TIMER, 33) };
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
                if r.set(cfg, Value::Bool(new)) {
                    // LockSize 开启时用自然尺寸而非默认 300x200；
                    // 关闭时保留 window_size 仅作几何恢复（fit 不再认它）。
                    if r == CfgRef::LockSize && new {
                        cfg.window_size = Some(crate::gdi::natural_size(cfg));
                    }
                    true
                } else {
                    false
                }
            });
            if applied {
                // 副作用
                let cfg_now = ctx.cfg();
                if let Some(cfg) = cfg_now {
                    match r {
                        CfgRef::Topmost => {
                            window::apply_topmost(overlay, cfg.topmost);
                        }
                        CfgRef::AutoStart => {
                            autostart::apply(cfg.autostart);
                        }
                        CfgRef::DebugLog => {
                            crate::probe::set_debug(cfg.debug_log);
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
            apply_config(ctx, |cfg| {
                let Value::Select(old) = r.get(cfg) else { return false };
                if old == i {
                    return false;
                }
                r.set(cfg, Value::Select(i))
            });
            if r == CfgRef::Lang {
                if let Some(cfg) = ctx.cfg() {
                    crate::i18n::set_global_lang(cfg.lang);
                }
                ctx.rebuild_pages();
            }
        }
        Action::LiveValue(r, v) => {
            // 拖动中：改内存预览 + 实时通知悬浮窗（不落盘，松手才提交）。
            // 宽/高滑杆本身即「自定义尺寸」意图 → 顺带锁定，否则 fit 不认。
            apply_config_quiet(ctx, |cfg| {
                if matches!(r, CfgRef::WinW | CfgRef::WinH) {
                    cfg.size_locked = true;
                    if cfg.window_size.is_none() {
                        cfg.window_size = Some(crate::gdi::natural_size(cfg));
                    }
                }
                r.set(cfg, Value::Num(v))
            });
            // 拖动中节流（80ms），避免 16ms 一跳把悬浮窗拖成 60fps 全量 layout。
            notify_overlay_reconfig_throttled(ctx, 80);
        }
        Action::CommitValue(r, v) => {
            apply_config(ctx, |cfg| {
                if matches!(r, CfgRef::WinW | CfgRef::WinH) {
                    cfg.size_locked = true;
                    if cfg.window_size.is_none() {
                        cfg.window_size = Some(crate::gdi::natural_size(cfg));
                    }
                }
                r.set(cfg, Value::Num(v))
            });
        }
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

/// 拖动中的节流通知：LiveValue 每 16ms 发出一跳，若每跳都 PostMessage WM_RECONFIG，
/// 悬浮窗线程会被迫以 60fps 全量 layout（build_rows + 全指纹哈希）——"摇杆卡顿"的直接
/// 成因之一。这里把拖动中的通知压到最小间隔（80ms ≈ 12 次/秒），仍足够实时预览，
/// 提交/落盘路径不受影响。
fn notify_overlay_reconfig_throttled(ctx: &mut SettingsCtx, min_gap_ms: u64) {
    let now = ctx.now_ms();
    if now.saturating_sub(ctx.last_reconfig_ms) < min_gap_ms {
        return;
    }
    ctx.last_reconfig_ms = now;
    notify_overlay_reconfig(ctx);
}

/// 刷新 settings 窗口。
fn invalidate_settings() {
    let hwnd = SETTINGS_HWND.load(Ordering::SeqCst);
    if hwnd != -1 {
        unsafe { let _ = InvalidateRect(HWND(hwnd as *mut _), None, false); }
    }
}

// ── 摇杆（弹簧杆）交互 ───────────────────────────────────
//
// 所有数值滑杆改造成"摇杆"：杆永远居中（不含绝对值），真实数值只在右侧值框
// 显示。拖动 = 在当前值基础上做增量，增量大小随推杆偏移（"力"）随时间累加，
// 并带弹簧-阻尼物理：松手后杆以阻尼弹簧回到中心，改动值保留。
//
// 弹簧积分用固定步长（16ms 定时器驱动），数值累加只在拖动阶段生效，回中阶段
// 只动视觉不改值。空闲态 `ctx.fx = None`、定时器停用 → 零额外 CPU/GPU 开销。

/// 弹簧-阻尼常数（临界阻尼附近，无可见振荡）。
const FX_K: f32 = 150.0;
const FX_C: f32 = 12.5;
/// 固定物理步长（秒）。
const FX_DT: f32 = 0.016;
/// 摇杆数值推进 = 加速度模型（用户规格"类似加速度公式，上限 +10/s"）：
/// - `FX_ACCEL`（单位/s²）：满偏 deflect=1 时的加速度。0→10/s 约 0.28s，
///   小偏移加速度按比例缩小 → "轻推慢加速、快推迅速到位"的物理手感。
/// - `FX_MAX_RATE`（单位/s）：推进速率上限（终端速度），钳制 ±10 单位/秒。
const FX_ACCEL: f32 = 36.0;
const FX_MAX_RATE: f32 = 10.0;
/// 简单滑杆速度上限（单位/秒）：speed = deflect × SIMPLE_MAX_RATE。
const SIMPLE_MAX_RATE: f32 = 5.0;

/// 鼠标 x → 目标位移 [-1,1]（以轨道中心为 0）。
fn slider_target_of(ctx: &SettingsCtx, idx: usize, pt: ContentPt) -> f32 {
    let page = &ctx.pages[ctx.page];
    let Some(w) = page.widgets.get(idx) else { return 0.0 };
    let tr = track_rect(&w.rect, ctx.dpi);
    let half = ((tr.right - tr.left).max(1) as f32) / 2.0;
    let center = (tr.left + tr.right) as f32 / 2.0;
    ((pt.0.x as f32 - center) / half).clamp(-1.0, 1.0)
}

/// 抓杆：摇杆从中心开始，简单滑杆从当前位置开始。
fn slider_grab(ctx: &mut SettingsCtx, idx: usize, pt: ContentPt) {
    let page = &ctx.pages[ctx.page];
    let Some(w) = page.widgets.get(idx) else { return };
    let Some(slider) = w.kind.as_any().downcast_ref::<widget::SliderWidget>() else { return };
    let joystick = slider.joystick;

    let deflect = if joystick {
        slider_target_of(ctx, idx, pt)
    } else {
        let tr = track_rect(&w.rect, ctx.dpi);
        let span = ((tr.right - tr.left).max(1) as f32).abs();
        let frac = if span > 0.0 {
            ((pt.0.x as f32 - tr.left as f32) / span).clamp(0.0, 1.0)
        } else {
            0.5
        };
        frac * 2.0 - 1.0
    };

    ctx.fx = Some(SliderFx {
        widget: idx,
        deflect,
        vel: 0.0,
        target: deflect,
        releasing: false,
        pos: 0.0,
        speed: 0.0,
        joystick,
    });
    unsafe { start_timer(find_hwnd(ctx), DRAG_TIMER, 16) };
}

/// 拖动中：更新目标位移（物理积分在定时器里跑）。
fn slider_move(ctx: &mut SettingsCtx, idx: usize, pt: ContentPt) {
    if ctx.fx.map_or(false, |f| f.widget == idx) {
        let t = slider_target_of(ctx, idx, pt);
        if let Some(fx) = ctx.fx.as_mut() {
            fx.target = t;
        }
    }
}

/// 取消摇杆（翻页/进入编辑/捕获丢失）：提交当前值并清态、停表。
fn slider_cancel(ctx: &mut SettingsCtx, hwnd: HWND) {
    let keep = ctx.fx.take();
    if keep.is_none() {
        return;
    }
    unsafe { kill_timer(hwnd, DRAG_TIMER) };
    if let Some(fx) = keep {
        if let Some(w) = ctx.pages[ctx.page].widgets.get(fx.widget) {
            if let Some(slider) = w.kind.as_any().downcast_ref::<widget::SliderWidget>() {
                let v = match slider.cfg.get(&ctx.cfg().unwrap_or_default()) {
                    Value::Num(x) => x,
                    _ => slider.min as f64,
                };
                dispatch(ctx, Action::CommitValue(slider.cfg, v));
            }
        }
    }
}

/// 松手：摇杆进入回中阶段（视觉弹回，值保留）；简单滑杆立即提交值。
fn slider_release(ctx: &mut SettingsCtx) {
    if let Some(fx) = ctx.fx.as_mut() {
        if fx.joystick {
            fx.releasing = true;
            fx.target = 0.0;
        } else {
            fx.releasing = true;
            fx.target = fx.deflect;
        }
    }
}

/// 物理步进 + 增量累加。返回是否仍需保持定时器（false = 收敛/简单滑杆松手停表）。
fn slider_tick(ctx: &mut SettingsCtx) -> bool {
    let Some(fx) = ctx.fx else { return false };
    let (slider_cfg, smin, smax, sstep, joystick) = {
        let page = &ctx.pages[ctx.page];
        let Some(w) = page.widgets.get(fx.widget) else { ctx.fx = None; return false };
        let Some(slider) = w.kind.as_any().downcast_ref::<widget::SliderWidget>() else {
            ctx.fx = None;
            return false;
        };
        (slider.cfg, slider.min, slider.max, slider.step, slider.joystick)
    };
    let cur = ctx.cfg();

    let mut fx = fx;

    if joystick {
        let a = FX_K * (fx.target - fx.deflect) - FX_C * fx.vel;
        fx.vel += a * FX_DT;
        fx.deflect += fx.vel * FX_DT;
        fx.deflect = fx.deflect.clamp(-1.0, 1.0);
        if fx.releasing && fx.deflect.abs() < 0.002 && fx.vel.abs() < 0.03 {
            let base = cur
                .and_then(|c| match slider_cfg.get(&c) { Value::Num(x) => Some(x), _ => None })
                .unwrap_or(smin as f64);
            ctx.fx = None;
            dispatch(ctx, Action::CommitValue(slider_cfg, base));
            return false;
        }
        if !fx.releasing {
            fx.speed += fx.deflect * FX_ACCEL * FX_DT;
            fx.speed = fx.speed.clamp(-FX_MAX_RATE, FX_MAX_RATE);
            fx.pos += fx.speed * FX_DT;
            if let Some(cfg) = &cur {
                if let Value::Num(base) = slider_cfg.get(cfg) {
                    let v = base as f32 + fx.pos;
                    let stepped = ((v / sstep).round() * sstep).clamp(smin, smax);
                    if (stepped - base as f32).abs() > 1e-6 {
                        dispatch(ctx, Action::LiveValue(slider_cfg, stepped as f64));
                    }
                }
            }
        }
    } else {
        fx.speed = fx.deflect * SIMPLE_MAX_RATE;
        if fx.releasing {
            ctx.fx = None;
            if let Some(cfg) = &cur {
                if let Value::Num(v) = slider_cfg.get(cfg) {
                    dispatch(ctx, Action::CommitValue(slider_cfg, v));
                }
            }
            return false;
        }
        if let Some(cfg) = &cur {
            if let Value::Num(base) = slider_cfg.get(cfg) {
                let v = base as f32 + fx.speed * FX_DT;
                let stepped = ((v / sstep).round() * sstep).clamp(smin, smax);
                if (stepped - base as f32).abs() > 1e-6 {
                    dispatch(ctx, Action::LiveValue(slider_cfg, stepped as f64));
                }
            }
        }
    }

    ctx.fx = Some(fx);
    ctx.invalidate_widget(fx.widget);
    unsafe { let _ = InvalidateRect(find_hwnd(ctx), None, false); }
    true
}

/// 进入输入框编辑态。
fn begin_edit(ctx: &mut SettingsCtx, idx: usize) {
    slider_cancel(ctx, find_hwnd(ctx));
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
        if i != st.selected {
            dispatch(ctx, Action::Select(seg.cfg, i));
        }
        return;
    }
    // Button：按动作派发（颜色/字号重置）
    if let Some(btn) = w.kind.as_any().downcast_ref::<widget::ButtonWidget>() {
        match btn.action {
            widget::ButtonAction::ResetColor(r) => {
                apply_config(ctx, |cfg| r.set(cfg, Value::Color(None)));
            }
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
            widget::ButtonAction::ResetFont(i) => {
                apply_config(ctx, |cfg| {
                    if i < cfg.metric_font_sizes.len() {
                        let changed = cfg.metric_font_sizes[i].is_some();
                        cfg.metric_font_sizes[i] = None;
                        changed
                    } else {
                        false
                    }
                });
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
    // MetricSwatches
    if let Some(ms) = w.kind.as_any().downcast_ref::<widget::MetricSwatchesWidget>() {
        let wd = (w.rect.right - w.rect.left) / SW_N as i32;
        let i = (((pt.0.x - w.rect.left) / wd.max(1)) as usize).min(SW_N - 1);
        let mut st = WidgetState::default();
        w.kind.sync(&cfg, &mut st);
        let follow = st.value_f > 0.5;
        crate::probe::log(&format!(
            "swatch click wrect=({},{},{},{}) i={} SW_N={} selected={} follow={}",
            w.rect.left, w.rect.top, w.rect.right, w.rect.bottom, i, SW_N, st.selected, follow
        ));
        // ⚠️ 修复（2026-09-09）：旧逻辑 `if i == SW_FOLLOW || follow { None }` 在
        // 当前为"跟随默认"(follow=true) 时，点**任意**色块都写成 None → 颜色永远
        // 切不过去（探针实证：apply_config ok 但渲染侧 metric_colors 恒为 None）。
        // 正确语义：只有点"跟随默认"格(≈)才写 None；点普通色块永远写所选颜色。
        if i != st.selected || !(i == SW_FOLLOW && follow) {
            let color = if i == SW_FOLLOW {
                None
            } else {
                let (r, g, b) = widget::METRIC_SWATCHES[i];
                Some(sysoverlay_core::color::Rgb::new(r, g, b))
            };
            let r = ms.cfg;
            apply_config(ctx, |cfg| r.set(cfg, Value::Color(color)));
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
        WM_NCHITTEST => {
            // lparam 是**屏幕**坐标，必须转成客户区坐标再与标题栏矩形比较
            // （参考 graph.rs / proc_list.rs：二者都先 ScreenToClient 再测命中）。
            // 之前直接用屏幕坐标当客户坐标，只有窗口贴屏幕顶边(y≈0)时才命中，
            // 其余位置判定失败 → 标题栏无法拖动。
            let mut sc = POINT {
                x: (lparam.0 & 0xFFFF) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
            };
            let _ = ScreenToClient(hwnd, &mut sc);
            let pt = ClientPt(sc);
            let d = crate::dpi::get_system_dpi();
            if titlebar::in_title_bar(d, pt.0) {
                let mut r = RECT::default();
                let _ = GetClientRect(hwnd, &mut r);
                if ctx_of(hwnd).map(|c| c.tb.pinned).unwrap_or(false) {
                    if titlebar::hit_button(d, r.right - r.left, pt.0, true).is_some() {
                        return LRESULT(HTCLIENT as isize);
                    }
                    return LRESULT(HTTRANSPARENT as isize);
                }
                if titlebar::hit_button(d, r.right - r.left, pt.0, false).is_some() {
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
                let cpt = pt.to_content(ctx.dpi);
                // 拖动中：更新滑杆值
                if let Interaction::Dragging { widget: idx, .. } = &ctx.interaction {
                    slider_move(ctx, *idx, cpt);
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
            let cpt = pt.to_content(ctx.dpi);

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
            if let Some(page) = nav_at(ctx, pt) {
                dispatch(ctx, Action::SetPage(page));
                let _ = InvalidateRect(hwnd, None, false);
                return LRESULT(0);
            }
            // 3) 控件（内容坐标）
            if let Some(idx) = widget_at(ctx, cpt) {
                if ctx.cfg().is_some() {
                    // 滑杆：值框内 → 编辑；否则拖动
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
                        // 摇杆：任意命中轨道即从中心抓杆（增量式弹簧）。
                        slider_cancel(ctx, hwnd);
                        ctx.interaction = Interaction::Dragging { widget: idx, origin: cpt.0 };
                        let _ = SetCapture(hwnd);
                        slider_grab(ctx, idx, cpt);
                        ctx.invalidate_widget(idx);
                        let _ = InvalidateRect(hwnd, None, false);
                        return LRESULT(0);
                    }
                    on_click_plain(ctx, idx, cpt);
                    ctx.invalidate_widget(idx);
                    let _ = InvalidateRect(hwnd, None, false);
                } else {
                    let _ = InvalidateRect(hwnd, None, false);
                }
            } else {
                // 未命中任何控件：记录坐标，用于验证「绘制坐标 vs 命中坐标」是否同一系。
                crate::probe::log(&format!(
                    "click MISS client=({},{}) content=({},{}) content_top={}",
                    pt.0.x, pt.0.y, cpt.0.x, cpt.0.y, content_top_y(ctx.dpi)
                ));
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Some(ctx) = ctx_of(hwnd) {
                if let Interaction::Dragging { widget, .. } = &ctx.interaction {
                    let idx = *widget;
                    ctx.invalidate_widget(idx);
                    ctx.interaction = Interaction::Idle;
                    // 松手 → 弹簧回中（DRAG_TIMER 继续驱动），收敛后自动提交。
                    slider_release(ctx);
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
                    slider_cancel(ctx, hwnd);
                }
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 as usize == PAGE_TIMER {
                if let Some(ctx) = ctx_of(hwnd) {
                    if let Some(anim) = &ctx.page_anim {
                        let dur_ms = ctx.cfg().map(|c| c.anim_ms).unwrap_or(300).max(1) as f32;
                        if anim.t0.elapsed().as_secs_f32() * 1000.0 / dur_ms >= 1.0 {
                            ctx.page_anim = None;
                            kill_timer(hwnd, PAGE_TIMER);
                        }
                    }
                }
                // ⚠️ 每帧必须主动失效重绘：此前只更新状态不 InvalidateRect，
                // 动画只画第一帧中间态就永久停格（半透明+偏移帧）——
                // 用户视角即「点一下导航整个界面卡死」（2026-09-09 实证）。
                let _ = InvalidateRect(hwnd, None, false);
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
            } else if wparam.0 as usize == DRAG_TIMER {
                if let Some(ctx) = ctx_of(hwnd) {
                    if !slider_tick(ctx) {
                        kill_timer(hwnd, DRAG_TIMER);
                    }
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if let Some(ctx) = ctx_of(hwnd) {
                if ctx.is_editing() && wparam.0 as u32 == 0x1B {
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
                ctx.theme = Theme::system();
                apply_window_chrome(hwnd, ctx.theme.dark);
                // 主题色在绘制时从 ctx.theme 读取，无需重建页面；重绘即可。
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            // 隐藏前清理摇杆物理（停表）+ flush pending 改动（防抖未到期的最后一次变更不丢）
            if let Some(ctx) = ctx_of(hwnd) {
                slider_cancel(ctx, hwnd);
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
    let title = crate::i18n::t(lang, "WinTitle");
    let theme = ctx.theme;
    let dpi = ctx.dpi;
    let content_top = content_top_y(dpi);
    let bg = c(&theme, theme.window_bg);
    let (dx, alpha) = anim_params(ctx);

    // 字段级不相交借用：闭包只捕获 pages/交互快照，与 &mut ctx.gfx 无重叠。
    let pages = &ctx.pages;
    let page_idx = ctx.page;
    let interaction = ctx.interaction.clone();
    let edit = ctx.edit.clone();
    let cfg = ctx.cfg();
    let tb = &ctx.tb;
    // 摇杆推杆位移：仅当前拖动/回中的滑杆非零，其余显示居中。
    let slider_thumb = ctx.fx.map(|f| (f.widget, f.deflect, f.joystick));

    let presented = ctx.gfx.frame(|rt, gfx| {
        unsafe { rt.Clear(Some(&bg.d2d())) };
        // 卡片均为不透明填充 → 全程 ClearType（与旧 GDI quality=5 一致）
        gfx.set_text_aa(rt, true);

        paint_titlebar(rt, gfx, &theme, dpi, win_w, tb, &title);
        paint_nav(rt, gfx, &theme, dpi, lang, win_w, page_idx);

        // 内容区（翻页动画：水平滑入 + 淡入）
        let bounds = RectF::new(0.0, content_top as f32, win_w as f32, win_h as f32);
        let faded = if alpha < 1.0 { gfx.push_fade(rt, bounds, alpha) } else { false };
        gfx.set_translate(rt, dx, 0.0);
        paint_page(
            rt, gfx, pages, page_idx, &interaction, edit.as_ref(), cfg.as_ref(), &theme, dpi, dx,
            body_px(dpi), small_px(dpi), slider_thumb,
        );
        gfx.reset_transform(rt);
        if faded {
            gfx.pop_fade(rt);
        }
    });
    // 帧探针：每 64 帧采样一次耗时（避免日志风暴）；失败帧每次都打（低频且关键）。
    static PAINT_FRAME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = PAINT_FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if presented {
        if n % 64 == 0 {
            crate::probe::log_lazy(|| {
                format!(
                    "settings paint frame#{} {}x{} page={} dx={:.1} alpha={:.2} us={}",
                    n, win_w, win_h, page_idx, dx, alpha,
                    t0.elapsed().as_micros()
                )
            });
        }
        unsafe { let _ = ValidateRect(hwnd, None); }
    } else {
        crate::probe::log_lazy(|| {
            format!(
                "settings paint FAILED frame#{} page={} dx={:.1} alpha={:.2}",
                n, page_idx, dx, alpha
            )
        });
    }
    presented
}

fn global_lang() -> sysoverlay_core::types::Lang {
    use sysoverlay_core::types::Lang;
    if crate::i18n::global_lang() == Lang::Zh { Lang::Zh } else { Lang::En }
}

fn paint_titlebar(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    t: &Theme,
    dpi: u32,
    win_w: i32,
    tb: &TitleBarState,
    title: &str,
) {
    let h = s(dpi, crate::titlebar::TB_H) as f32;
    // 底层：与卡片同色 + 底部分隔线
    gfx.fill_rect(rt, RectF::new(0.0, 0.0, win_w as f32, h), c(t, t.card));
    gfx.line(rt, 0.0, h - 1.0, win_w as f32, h - 1.0, c(t, t.card_stroke), 1.0);
    // 标题（左侧）
    if !title.is_empty() {
        let tx = RectF::new(
            s(dpi, titlebar::CAPTION_PAD_L) as f32,
            0.0,
            (win_w - s(dpi, 200)) as f32,
            h,
        );
        gfx.text(rt, title, small_px(dpi), 400, HAlign::Leading, tx, c(t, t.label), true);
    }
    // 按钮
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
                    let d = s(dpi, 4) as f32;
                    let dot_top = r.top as f32 + m - d / 2.0 + s(dpi, 1) as f32;
                    gfx.fill_ellipse(
                        rt,
                        RectF::new(cx - d / 2.0, dot_top, cx + d / 2.0, dot_top + d),
                        c(t, t.label),
                    );
                    gfx.line(rt, cx, r.top as f32 + m + s(dpi, 2) as f32, cx, r.bottom as f32 - m, c(t, t.label), 1.0);
                    gfx.line(
                        rt,
                        cx - s(dpi, 4) as f32,
                        r.bottom as f32 - m,
                        cx + s(dpi, 4) as f32,
                        r.bottom as f32 - m,
                        c(t, t.label),
                        1.0,
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

fn paint_nav(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    t: &Theme,
    dpi: u32,
    lang: sysoverlay_core::types::Lang,
    win_w: i32,
    page_idx: usize,
) {
    let top = (s(dpi, crate::titlebar::TB_H) + s(dpi, NAV_GAP)) as f32;
    let h = s(dpi, NAV_H) as f32;
    let slot = RectF::new(
        s(dpi, MARGIN_X) as f32,
        top,
        (win_w - s(dpi, MARGIN_X)) as f32,
        top + h,
    );
    gfx.fill_round(rt, slot, s(dpi, NAV_H) as f32 / 2.0, c(t, t.segmented_bg));
    let n = 6;
    let seg_w = slot.w() / n as f32;
    for i in 0..n {
        let r = RectF::new(slot.l + i as f32 * seg_w, top, slot.l + (i + 1) as f32 * seg_w, top + h);
        if i == page_idx {
            let m = s(dpi, 2) as f32;
            let ir = RectF::new(r.l + m, r.t + m, r.r - m, r.b - m);
            gfx.fill_round(rt, ir, s(dpi, NAV_H) as f32 / 2.0 - 2.0, c(t, t.seg_selected));
            gfx.stroke_round(rt, ir, s(dpi, NAV_H) as f32 / 2.0 - 2.0, c(t, t.seg_selected_stroke), 1.0);
        }
        let label = page_caption(i, lang);
        let color = if i == page_idx { c(t, t.label) } else { c(t, t.secondary) };
        gfx.text(rt, label, small_px(dpi), 400, HAlign::Center, r, color, true);
    }
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
    dx: f32,
    body: f32,
    small: f32,
    slider_thumb: Option<(usize, f32, bool)>,
) {
    let page = &pages[page_idx];
    // `card.rect` / `widget.rect` 都是**内容坐标**（相对内容区顶部），
    // 绘制必须统一加客户区偏移 content_top——漏加会让整页内容盖住导航栏
    // （2026-09-09 截图实证：分节标题与标题栏重叠、导航条不可见）。
    let ct = content_top_y(dpi) as f32;
    for card in &page.cards {
        // 分节标题
        let cap = RectF::new(
            (card.rect.left + s(dpi, 2)) as f32 + dx,
            (card.rect.top - s(dpi, CAPTION_H + CAPTION_GAP)) as f32 + ct,
            card.rect.right as f32 + dx,
            (card.rect.top - s(dpi, CAPTION_GAP)) as f32 + ct,
        );
        gfx.text(rt, card.caption, small, 400, HAlign::Leading, cap, c(t, t.secondary), true);

        // 圆角卡片
        let card_rect = RectF::from_i32(card.rect.left, card.rect.top, card.rect.right, card.rect.bottom)
            .offset(dx, ct);
        gfx.fill_round(rt, card_rect, s(dpi, CARD_RADIUS) as f32, c(t, t.card));
        gfx.stroke_round(rt, card_rect, s(dpi, CARD_RADIUS) as f32, c(t, t.card_stroke), 1.0);

        // 行
        for (row_i, row) in card.rows.iter().enumerate() {
            let row_top = (card.rect.top + s(dpi, CARD_PAD) + s(dpi, row_i as i32 * ROW_H)) as f32 + ct;
            if !row.label.is_empty() {
                let first_widget_left = row
                    .widgets
                    .first()
                    .map(|&i| (page.widgets[i].rect.left - s(dpi, 8)) as f32)
                    .unwrap_or((card.rect.right - s(dpi, 12)) as f32);
                let lr = RectF::new(
                    (card.rect.left + s(dpi, 12)) as f32 + dx,
                    row_top,
                    first_widget_left + dx,
                    row_top + s(dpi, ROW_H) as f32,
                );
                // 轴对齐裁剪替代旧 DT_END_ELLIPSIS（标签列宽充裕时无差异）
                unsafe { let _ = rt.PushAxisAlignedClip(&lr.d2d(), windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE); }
                gfx.text(rt, row.label, body, 400, HAlign::Leading, lr, c(t, t.label), true);
                unsafe { rt.PopAxisAlignedClip() };
            }
            for &wi in &row.widgets {
                let mut ws = WidgetState::default();
                if let Some(cfg) = cfg {
                    page.widgets[wi].kind.sync(cfg, &mut ws);
                }
                ws.hovered = matches!(interaction, Interaction::Hover { widget } if *widget == wi);
                ws.editing = matches!(interaction, Interaction::Editing { widget } if *widget == wi);
                let (dfl, joystick) = slider_thumb
                    .filter(|(i, _, _)| *i == wi)
                    .map(|(_, d, j)| (d, j))
                    .unwrap_or((0.0, false));
                widget::paint_widget(rt, gfx, &page.widgets[wi], &ws, dpi, t, dx, ct, body, small, edit, dfl, joystick);
            }
        }

        // 脚注（多行换行）
        if let Some(foot) = card.footnote {
            let rows_bottom = (card.rect.top + s(dpi, CARD_PAD) + s(dpi, card.rows.len() as i32 * ROW_H)) as f32 + ct;
            let fr = RectF::new(
                (card.rect.left + s(dpi, 12)) as f32 + dx,
                rows_bottom,
                (card.rect.right - s(dpi, 12)) as f32 + dx,
                rows_bottom + s(dpi, FOOTNOTE_H) as f32,
            );
            gfx.text_wrap(rt, foot, small, 400, HAlign::Leading, fr, c(t, t.secondary));
        }
    }
}
