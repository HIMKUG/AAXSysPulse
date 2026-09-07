//! GDI 渲染管线 —— 双渲染器调度 + DIB 上屏。
//!
//! 管线：
//!   1. 按 [`Config::render_style`] 选渲染器（`ui::Renderer`：Strip / Vertical）
//!   2. 计算**全局 alpha** = 悬停/非悬停两档透明度 + 三次方缓出过渡动画
//!   3. 内容指纹脏检查：指纹未变 → 跳过整帧（不建画布、不 ULW）
//!   4. 渲染器画进复用的 [`SoftwareSurface`] → DIB 缓存 → `UpdateLayeredWindow`
//!
//! 渲染细节（文字/排布）全部在 `ui`，本文件只负责 Win32 的搬运。
//!
//! # 透明度过渡动画
//!
//! 悬停↔非悬停切换时按 [`Config::anim_ms`] 动画到目标透明度，曲线为
//! 三次方缓出（`1-(1-t)^3`，非线性：起步快、收尾慢）。动画期间用
//! `SetTimer(16ms)` 驱动逐帧重绘，到达目标后 `KillTimer` 归零开销。

use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Instant;

use windows::Win32::Foundation::{BOOL, COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, DIB_RGB_COLORS,
    GetDC, ReleaseDC, SelectObject, BI_RGB, BITMAPINFO, HBITMAP, HDC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    KillTimer, SetTimer, SetWindowPos, UpdateLayeredWindow, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOZORDER,
};

#[link(name = "user32")]
extern "system" {
    fn GetWindowRect(hwnd: HWND, lpRect: *mut RECT) -> BOOL;
}

use sysoverlay_ui::offscreen::SoftwareSurface;
use sysoverlay_ui::{Renderer, StripRenderer, VerticalRenderer};

use crate::Config;
use crate::perf;
use crate::Snapshot;

/// 悬浮窗透明度动画定时器 ID（仅本窗口使用；graph/procs 的定时器在各自窗口互不干扰）。
pub const ANIM_TIMER_ID: usize = 7;

/// 按配置选择渲染器实例（'static，零分配）。旧卡片式（Classic）折入横版条。
fn renderer_for(style: sysoverlay_core::types::RenderStyle) -> &'static dyn Renderer {
    match style {
        sysoverlay_core::types::RenderStyle::Classic | sysoverlay_core::types::RenderStyle::Strip => {
            &StripRenderer
        }
        sysoverlay_core::types::RenderStyle::Vertical => &VerticalRenderer,
    }
}

// ── 透明度过渡动画状态（UI 线程独占） ──────────────────────

/// 一次过渡动画：从 `from_pct` 经过 `anim_ms` 到 `to_pct`（百分比 0..=100）。
#[derive(Clone, Copy, Debug)]
struct AnimState {
    active: bool,
    t0: Instant,
    from_pct: f32,
    to_pct: f32,
}

impl Default for AnimState {
    fn default() -> Self {
        Self {
            active: false,
            t0: Instant::now(),
            from_pct: 35.0,
            to_pct: 35.0,
        }
    }
}

static ANIM_STATE: LazyLock<Mutex<AnimState>> = LazyLock::new(|| Mutex::new(AnimState::default()));

/// 上一帧实际送屏的 alpha（百分比 × 100，0..=10000，避免浮点原子）。
static RENDERED_PCT_X100: AtomicU32 = AtomicU32::new(3500);

/// 三次方缓出：t ∈ [0,1] → e ∈ [0,1]，起步快、收尾慢（非线性过渡）。
fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// 计算本帧 alpha 百分比；动画进行中返回 true（调用方应确保后续帧继续到来）。
fn compute_alpha_pct(hwnd: HWND, cfg: &Config) -> (f32, bool) {
    let hovered = crate::window::is_window_hovered(hwnd);
    let desired = if hovered {
        cfg.hover_opacity as f32
    } else {
        cfg.normal_opacity as f32
    };
    let anim_ms = cfg.anim_ms as f32;
    let rendered = RENDERED_PCT_X100.load(AtomicOrdering::Relaxed) as f32 / 100.0;

    let Ok(mut anim) = ANIM_STATE.lock() else {
        return (desired, false);
    };

    if anim.active {
        let t = (anim.t0.elapsed().as_secs_f32() * 1000.0 / anim_ms).clamp(0.0, 1.0);
        if t >= 1.0 {
            anim.active = false;
            anim.from_pct = anim.to_pct;
            return (anim.to_pct, false);
        }
        let e = ease_out_cubic(t);
        let pct = anim.from_pct + (anim.to_pct - anim.from_pct) * e;
        return (pct, true);
    }

    // 未在过渡：仅在目标变化时启动新的过渡（含 anim_ms=0 的直接跳变）
    if (rendered - desired).abs() < 0.05 {
        return (desired, false);
    }
    if anim_ms <= 0.5 {
        return (desired, false);
    }
    anim.active = true;
    anim.t0 = Instant::now();
    anim.from_pct = rendered;
    anim.to_pct = desired;
    (rendered, true)
}

/// 上一帧的指纹（`u64::MAX` 表示还没有画过）。
static LAST_FRAME_HASH: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(u64::MAX);

/// 渲染一帧到窗口（WM_NEW_SNAPSHOT / 尺寸变化 / 设置变化 入口）。
///
/// **脏检查**：内容与上一帧完全一致时整帧跳过——不建画布、不建 DIB、
/// 不 `UpdateLayeredWindow`。静止的机器上这会把渲染开销打到 0。
pub fn render(hwnd: HWND, snap: &Snapshot, cfg: &Config) {
    let t0 = perf::begin();
    unsafe {
        let mut rect = RECT::default();
        if !GetWindowRect(hwnd, &mut rect).as_bool() {
            return;
        }
        let width = (rect.right - rect.left).unsigned_abs() as i32;
        let height = (rect.bottom - rect.top).unsigned_abs() as i32;
        if width == 0 || height == 0 {
            return;
        }

        let renderer = renderer_for(cfg.render_style);

        // 全局 alpha：悬停/非悬停两档透明度，切换时三次方缓出过渡。
        // 动画进行中 → SetTimer 逐帧推进；结束后 KillTimer 归零开销。
        let (alpha_pct_f, animating) = compute_alpha_pct(hwnd, cfg);
        let src_alpha = ((alpha_pct_f.clamp(1.0, 100.0) / 100.0) * 255.0).round() as u8;

        // 脏检查：指纹未变 → 什么都不做
        let sig = renderer.frame_hash(snap, cfg, width, height, src_alpha);
        if LAST_FRAME_HASH.load(std::sync::atomic::Ordering::Relaxed) == sig {
            perf::note_skipped();
            // 动画未走完但帧被跳过（理论上只在末帧重复时发生）：继续驱动定时器
            if animating {
                let _ = SetTimer(hwnd, ANIM_TIMER_ID, 16, None);
            }
            return;
        }
        LAST_FRAME_HASH.store(sig, std::sync::atomic::Ordering::Relaxed);

        // 画布复用（P2）：尺寸不变时只清零，不再分配
        let surface = SURFACE.get_or_init(|| Mutex::new(SoftwareSurface::new(0, 0)));
        let Ok(mut surface) = surface.lock() else { return };
        surface.resize(width.max(1), height.max(1));

        // 渲染器负责具体排布；返回自然尺寸（当前窗口尺寸不符时以画布为准）
        let (nat_w, nat_h) = renderer.draw(&mut surface, snap, cfg);

        update_layered(hwnd, &surface, width, height, src_alpha);

        // 记录本帧实际送屏的 alpha 作为下一次过渡的起点
        RENDERED_PCT_X100.store((alpha_pct_f.clamp(1.0, 100.0) * 100.0) as u32, AtomicOrdering::Relaxed);

        // 动画未走完 → 定期器拉下一帧；走完 → 停表归零开销
        if animating {
            let _ = SetTimer(hwnd, ANIM_TIMER_ID, 16, None);
        } else {
            let _ = KillTimer(hwnd, ANIM_TIMER_ID);
        }

        // 窗口尺寸与内容自然尺寸漂移过大（项开关/风格切换后未 fit）时兜底校正
        if (nat_w - width).abs() > 8 || (nat_h - height).abs() > 8 {
            drop(surface);
            fit_window(hwnd, cfg);
        }
    }
    perf::end_render(t0);
}

/// 逐帧复用的离屏画布（P2）。
///
/// 只在 UI 线程访问；用 `Mutex` 只是为了满足 Rust 的 `Sync` 要求
/// （不可免的静态），实际无竞争。
static SURFACE: OnceLock<Mutex<SoftwareSurface>> = OnceLock::new();

/// DIB 缓存（P2）：位图/内存 DC 按尺寸复用。
///
/// **为什么重要**：`CreateDIBSection` 每次都要在内核里分配 187 KB 并清零，
/// 再 `DeleteObject` 销毁 —— 每帧一次，纯浪费。这里只在尺寸真的变化时重建。
#[derive(Default)]
struct DibCache {
    w: i32,
    h: i32,
    dc: isize,
    bmp: isize,
    old_bmp: isize,
    bits: isize,
}

static DIB: OnceLock<Mutex<DibCache>> = OnceLock::new();

/// 释放 DIB 缓存（窗口销毁 / DPI 变化时调用）。
pub fn invalidate_dib_cache() {
    if let Some(c) = DIB.get() {
        if let Ok(mut c) = c.lock() {
            unsafe { c.free() };
            *c = DibCache::default();
        }
    }
}

impl DibCache {
    unsafe fn free(&mut self) {
        if self.old_bmp != 0 && self.dc != 0 {
            let _ = SelectObject(HDC(self.dc as *mut _), windows::Win32::Graphics::Gdi::HGDIOBJ(self.old_bmp as *mut _));
        }
        if self.bmp != 0 {
            let _ = DeleteObject(HBITMAP(self.bmp as *mut _));
        }
        if self.dc != 0 {
            let _ = DeleteDC(HDC(self.dc as *mut _));
        }
        self.dc = 0;
        self.bmp = 0;
        self.old_bmp = 0;
        self.bits = 0;
        self.w = 0;
        self.h = 0;
    }

    /// 取（新建或复用）一个 `w × h` 的 32 位 DIB，返回 (内存 DC, 像素指针)。
    unsafe fn get(&mut self, screen_dc: HDC, w: i32, h: i32) -> Option<(HDC, *mut u8)> {
        if self.w == w && self.h == h && self.dc != 0 && self.bmp != 0 && self.bits != 0 {
            return Some((HDC(self.dc as *mut _), self.bits as *mut u8));
        }
        self.free();

        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.is_invalid() {
            return None;
        }
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize =
            std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h; // top-down DIB
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bitmap = match CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(_) => {
                let _ = DeleteDC(mem_dc);
                return None;
            }
        };
        if bits.is_null() {
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(mem_dc);
            return None;
        }

        let old = SelectObject(mem_dc, bitmap);
        self.w = w;
        self.h = h;
        self.dc = mem_dc.0 as isize;
        self.bmp = bitmap.0 as isize;
        self.old_bmp = old.0 as isize;
        self.bits = bits as isize;
        Some((mem_dc, bits as *mut u8))
    }
}

/// 把 SoftwareSurface 上屏（DIB + UpdateLayeredWindow）。
///
/// `src_alpha` 为全局恒定 alpha（透明度 × 悬停系数，0..=255）。
#[allow(clippy::too_many_arguments)]
fn update_layered(hwnd: HWND, surface: &SoftwareSurface, width: i32, height: i32, src_alpha: u8) {
    unsafe {
        let screen_dc = GetDC(hwnd);
        if screen_dc.is_invalid() {
            return;
        }

        let dib = DIB.get_or_init(|| Mutex::new(DibCache::default()));
        let Ok(mut dib) = dib.lock() else {
            let _ = ReleaseDC(hwnd, screen_dc);
            return;
        };
        let Some((mem_dc, bits)) = dib.get(screen_dc, width, height) else {
            drop(dib);
            let _ = ReleaseDC(hwnd, screen_dc);
            return;
        };

        if !bits.is_null() && !surface.pixels.is_empty() {
            // 画布由 renderer 保证与窗口同尺寸（render 中 resize 过）
            let dst_len = (width * height * 4) as usize;
            if surface.pixels.len() >= dst_len {
                let dst = std::slice::from_raw_parts_mut(bits, dst_len);
                dst.copy_from_slice(&surface.pixels[..dst_len]);
            }
        }

        // pptDst 传 None：保持窗口当前位置。
        // 若传 Some((0,0)) 之类的值，UpdateLayeredWindow 会把窗口挪到该坐标——
        // 这正是旧版"一拖动就弹回原位"的元凶（每帧渲染都重置位置）。
        let mut sz = SIZE { cx: width, cy: height };
        let mut src_pt = POINT::default();

        let bf = BLENDFUNCTION {
            BlendOp: windows::Win32::Graphics::Gdi::AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: src_alpha,
            AlphaFormat: windows::Win32::Graphics::Gdi::AC_SRC_ALPHA as u8,
        };

        let _ = UpdateLayeredWindow(
            hwnd,
            screen_dc,
            None,
            Some(&mut sz),
            mem_dc,
            Some(&mut src_pt),
            COLORREF(0),
            Some(&bf as *const BLENDFUNCTION),
            windows::Win32::UI::WindowsAndMessaging::ULW_ALPHA,
        );

        // P2：不删除 mem_dc / bitmap —— 它们留在 DIB 缓存里逐帧复用。
        let _ = ReleaseDC(hwnd, screen_dc);
    }
}

/// 当前渲染器的自然尺寸（窗口自适应 / 启动定位用）。
pub fn natural_size(cfg: &Config) -> (i32, i32) {
    renderer_for(cfg.render_style).natural_size(cfg)
}

/// 让窗口自适应尺寸：手动设置过宽高（`window_size = Some`）时沿用用户尺寸，
/// 否则按内容自然尺寸（保持左上角位置）。
/// 设置项变化（项开关 / 风格 / 字号）后由 WM_RECONFIG 调用。
pub fn fit_window(hwnd: HWND, cfg: &Config) {
    let (nx, ny) = natural_size(cfg);
    let (w, h) = cfg
        .window_size
        .filter(|&(w, h)| w >= 60 && h >= 40)
        .unwrap_or((nx, ny));
    let w = w.max(60);
    let h = h.max(40);
    unsafe {
        let _ = SetWindowPos(hwnd, None, 0, 0, w, h, SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOMOVE);
        // 不调 InvalidateRect：尺寸变化后由调用方（WM_RECONFIG）直接 render 一帧，
        // 避免触发 WM_PAINT 忙循环。
    }
}