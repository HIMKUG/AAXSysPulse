//! GDI 渲染管线 —— 横向文字条（v3）。
//!
//! 管线：
//!   1. 由 Config 决定显示项（CPU/Mem/GPU/Net/Power，各自开关）
//!   2. 数值按**固定槽位**右对齐排布（槽位宽 = 最大模板串宽 × 缩放），
//!      保证数值位数变化时后续元素零位移
//!   3. 阴影/描边先画（取字体反色），再画正文
//!   4. 写入 SoftwareSurface → DIB → UpdateLayeredWindow
//!
//! 无背景色：表面初始全透明，只画文字。

use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, DIB_RGB_COLORS,
    GetDC, ReleaseDC, SelectObject, BI_RGB, BITMAPINFO, HBITMAP, HDC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowPos, UpdateLayeredWindow, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
};

use crate::perf;

use crate::Config;
use crate::Snapshot;
use sysoverlay_core::snapshot::Metric;
use sysoverlay_core::units::{fmt_gb, fmt_pct, fmt_power, fmt_rate, StrBuf};
use sysoverlay_core::types::NetUnit;
use sysoverlay_ui::font::{measure_scaled, render_str_scaled};
use sysoverlay_ui::offscreen::SoftwareSurface;
use sysoverlay_ui::palette::Style;
use sysoverlay_ui::Rgba;

#[link(name = "user32")]
extern "system" {
    fn GetWindowRect(hwnd: HWND, lpRect: *mut RECT) -> BOOL;
}

use windows::Win32::Foundation::BOOL;

/// 槽位宽度（字符数）——按各类数值的**最大**模板串宽取值：
/// 百分比 `100%`=4；GB 占用 `999.9GB`=7；网速 `888.8M/s`=8；功耗 `~8888W`=6。
const SLOT_PCT: u32 = 4;
const SLOT_GB: u32 = 7;
const SLOT_RATE: u32 = 8;
const SLOT_POWER: u32 = 6;
/// 标签与数值间距 = 2 个空格；项与项间距 = 4 个空格（用户规格）。
const GAP_LABEL_VALUE: u32 = 2;
const GAP_ITEM: u32 = 4;
/// 左右内边距（字符数）。
const PAD_X: u32 = 2;
/// 上下内边距（相对字形高度 16 的比例基准）。
const PAD_Y: u32 = 2;

/// 单元：一段文本 + 槽位（0 = 自然宽度，>0 = 右对齐到固定槽位）。
struct Cell {
    text: StrBuf,
    slot: u32,
    color: Rgba,
}

/// 把 Metric 归一化成 (数值, 是否陈旧, 是否可用)。
fn metric_parts(m: &Metric<f32>) -> (f32, bool, bool) {
    match m {
        Metric::Value(v) => (*v, false, true),
        Metric::Stale { value, .. } => (*value, true, true),
        Metric::Unavailable => (0.0, false, false),
    }
}

/// 按 cfg 汇总要显示的单元序列（与 render 的绘制走同一份布局逻辑）。
fn build_cells(snap: &Snapshot, cfg: &Config, style: &Style) -> Vec<Cell> {
    let mut cells: Vec<Cell> = Vec::new();
    let mut any = false;

    // 追加一个监控项：标签 + 间距 + 数值槽位
    macro_rules! push_item {
        ($label:expr, $slot:expr, $val:expr, $color:expr) => {
            if any {
                cells.push(gap_cell(GAP_ITEM));
            }
            cells.push(text_cell($label, 0, style.ui.name));
            cells.push(gap_cell(GAP_LABEL_VALUE));
            cells.push(Cell { text: $val, slot: $slot, color: $color });
            any = true;
        };
    }

    if cfg.show_cpu {
        let (v, stale, ok) = metric_parts(&snap.cpu);
        let mut b = StrBuf::new();
        if ok {
            fmt_pct(&mut b, v);
        } else {
            b.push_str("--");
        }
        let color = if stale { style.ui.value_stale } else { style.accent(v) };
        push_item!("CPU", SLOT_PCT, b, color);
    }

    if cfg.show_mem {
        let mut b = StrBuf::new();
        if cfg.mem_show_gb {
            // 实际占用量，不显示总量：如 `8.5GB`
            fmt_gb(&mut b, snap.memory.used_gb);
        } else {
            fmt_pct(&mut b, snap.memory.load_pct);
        }
        let color = if cfg.mem_show_gb { style.ui.value } else { style.accent(snap.memory.load_pct) };
        push_item!("Mem", if cfg.mem_show_gb { SLOT_GB } else { SLOT_PCT }, b, color);
    }

    if cfg.show_gpu {
        let (v, stale, ok) = metric_parts(&snap.gpu);
        let mut b = StrBuf::new();
        if ok {
            fmt_pct(&mut b, v);
        } else {
            b.push_str("--");
        }
        let color = if stale { style.ui.value_stale } else { style.accent(v) };
        push_item!("GPU", SLOT_PCT, b, color);
    }

    if cfg.show_net {
        if any {
            cells.push(gap_cell(GAP_ITEM));
        }
        cells.push(text_cell("Net", 0, style.ui.name));
        cells.push(gap_cell(GAP_LABEL_VALUE));
        cells.push(text_cell("\u{2191}", 0, style.ui.name)); // ↑
        let mut rx = StrBuf::new();
        fmt_rate(&mut rx, snap.net.rx_bps, NetUnit::MB);
        rx.push_str("/s");
        cells.push(Cell { text: rx, slot: SLOT_RATE, color: style.ui.value });
        cells.push(gap_cell(1));
        cells.push(text_cell("\u{2193}", 0, style.ui.name)); // ↓
        let mut tx = StrBuf::new();
        fmt_rate(&mut tx, snap.net.tx_bps, NetUnit::MB);
        tx.push_str("/s");
        cells.push(Cell { text: tx, slot: SLOT_RATE, color: style.ui.value });
        any = true;
    }

    // Power：power_enabled 同时是采样开关与显示开关
    if cfg.power_enabled {
        let (v, stale, ok) = metric_parts(&snap.power);
        let mut b = StrBuf::new();
        if ok {
            fmt_power(&mut b, v, stale);
        } else {
            b.push_str("--");
        }
        let color = if stale { style.ui.value_stale } else { style.ui.value };
        push_item!("Power", SLOT_POWER, b, color);
        let _ = &mut any;
    }

    cells
}

fn text_cell(s: &str, slot: u32, color: Rgba) -> Cell {
    let mut b = StrBuf::new();
    b.push_str(s);
    Cell { text: b, slot, color }
}

fn gap_cell(chars: u32) -> Cell {
    Cell { text: StrBuf::new(), slot: chars, color: Rgba::opaque(0, 0, 0) }
}

/// 阴影/描边颜色：字体亮 → 黑影，字体暗 → 白影（用户规格：默认与字体相反）。
fn shade_color(fg: sysoverlay_core::color::Rgb) -> Rgba {
    let lum = 0.299 * fg.r as f32 + 0.587 * fg.g as f32 + 0.114 * fg.b as f32;
    if lum >= 128.0 {
        Rgba::opaque(0, 0, 0)
    } else {
        Rgba::opaque(255, 255, 255)
    }
}

/// 横向条的自然尺寸（像素）：由 cfg 决定（项开关 + 字号），与数值无关。
///
/// 布局：PAD_X + Σ(label + 2 + slot[ + 项间 4]) + PAD_X（字符单位），
/// 1 字符 = (8+1) × scale 像素；高度 = (16 + 2×PAD_Y) × scale。
pub fn strip_natural_size(cfg: &Config) -> (i32, i32) {
    let scale = cfg.font_scale.clamp(1, 4);
    let mut chars: u32 = PAD_X;

    let item_chars = |label_len: u32, slot: u32, first: bool, out: &mut u32| {
        if !first {
            *out += GAP_ITEM;
        }
        *out += label_len + GAP_LABEL_VALUE + slot;
    };

    let mut first = true;
    if cfg.show_cpu {
        item_chars(3, SLOT_PCT, first, &mut chars);
        first = false;
    }
    if cfg.show_mem {
        let slot = if cfg.mem_show_gb { SLOT_GB } else { SLOT_PCT };
        item_chars(3, slot, first, &mut chars);
        first = false;
    }
    if cfg.show_gpu {
        item_chars(3, SLOT_PCT, first, &mut chars);
        first = false;
    }
    if cfg.show_net {
        if !first {
            chars += GAP_ITEM;
        }
        // Net + 2 + ↑slot + 1 + ↓slot
        chars += 3 + GAP_LABEL_VALUE + SLOT_RATE + 1 + SLOT_RATE;
        first = false;
    }
    if cfg.power_enabled {
        item_chars(5, SLOT_POWER, first, &mut chars);
    }
    chars += PAD_X;

    let w = (chars * 9 * scale) as i32;
    let h = ((16 + PAD_Y * 2) * scale) as i32;
    (w.max(40), h.max(16))
}

/// 绘制一串文字（含阴影/描边 pass）。
#[allow(clippy::too_many_arguments)]
fn draw_text(
    surface: &mut SoftwareSurface,
    text: &str,
    x: i32,
    y: i32,
    scale: u32,
    color: Rgba,
    shade: Rgba,
    shadow: bool,
    outline: bool,
) {
    let s = scale.max(1);
    if outline {
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            render_str_scaled(
                &mut surface.pixels,
                surface.width,
                surface.height,
                text,
                x + dx * s as i32,
                y + dy * s as i32,
                s,
                shade,
            );
        }
    }
    if shadow {
        render_str_scaled(
            &mut surface.pixels,
            surface.width,
            surface.height,
            text,
            x + s as i32,
            y + s as i32,
            s,
            shade,
        );
    }
    render_str_scaled(
        &mut surface.pixels,
        surface.width,
        surface.height,
        text,
        x,
        y,
        s,
        color,
    );
}

/// 本帧内容指纹（P1 脏检查用）。
///
/// 覆盖「会改变像素」的全部输入：文本、槽位、颜色、画布尺寸、字号、阴影/描边。
/// 用 FNV-1a 64 位折叠 —— 逐帧比较只需一次整数比较。
fn frame_hash(
    cells: &[Cell],
    w: i32,
    h: i32,
    scale: u32,
    shadow: bool,
    outline: bool,
) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce4_84222325;
    const FNV_PRIME: u64 = 0x1000_0000_01b3;
    let mut acc = FNV_OFFSET;
    let mut mix = |byte: u8| {
        acc ^= byte as u64;
        acc = acc.wrapping_mul(FNV_PRIME);
    };
    for b in w.to_le_bytes() {
        mix(b);
    }
    for b in h.to_le_bytes() {
        mix(b);
    }
    for b in scale.to_le_bytes() {
        mix(b);
    }
    mix(shadow as u8);
    mix(outline as u8);
    for c in cells {
        for b in c.text.as_str().as_bytes() {
            mix(*b);
        }
        mix(0xFF); // 文本分隔符：防止 "AB"+"C" 与 "A"+"BC" 撞车
        for b in c.slot.to_le_bytes() {
            mix(b);
        }
        mix(c.color.r);
        mix(c.color.g);
        mix(c.color.b);
        mix(c.color.a);
    }
    acc
}

/// 上一帧的指纹（`u64::MAX` 表示还没有画过）。
static LAST_FRAME_HASH: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(u64::MAX);

/// 渲染横向监控条到窗口（WM_NEW_SNAPSHOT / 尺寸变化 / 设置变化 入口）。
///
/// **P1 脏检查**：内容与上一帧完全一致时整帧跳过——不建画布、不建 DIB、
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

        let scale = cfg.font_scale.clamp(1, 4);
        let style = Style::derive(sysoverlay_core::color::Rgb::default(), cfg.fg);
        let shade = shade_color(cfg.fg);
        let cells = build_cells(snap, cfg, &style);

        // 脏检查：指纹未变 → 什么都不做
        let sig = frame_hash(&cells, width, height, scale, cfg.text_shadow, cfg.text_outline);
        if LAST_FRAME_HASH.load(std::sync::atomic::Ordering::Relaxed) == sig {
            perf::note_skipped();
            return;
        }
        LAST_FRAME_HASH.store(sig, std::sync::atomic::Ordering::Relaxed);

        // 画布：字形基线垂直居中
        let glyph_h = 16 * scale as i32;
        let pad_y = PAD_Y as i32 * scale as i32;
        let y = pad_y.max((height - glyph_h) / 2);
        let mut x = PAD_X as i32 * scale as i32;
        let char_step = 9 * scale as i32;

        // P2：画布复用 —— 尺寸不变时只清零，不再分配 187 KB
        let surface = SURFACE.get_or_init(|| Mutex::new(SoftwareSurface::new(0, 0)));
        let Ok(mut surface) = surface.lock() else { return };
        surface.resize(width.max(1), height.max(1));
        for cell in &cells {
            if cell.slot == 0 {
                let w = measure_scaled(cell.text.as_str(), scale);
                if !cell.text.is_empty() {
                    draw_text(&mut surface, cell.text.as_str(), x, y, scale, cell.color, shade, cfg.text_shadow, cfg.text_outline);
                }
                x += w;
            } else {
                // 固定槽位：右对齐（数值位数变化不影响后续元素）
                let slot_w = cell.slot as i32 * char_step;
                let w = measure_scaled(cell.text.as_str(), scale);
                let dx = if w < slot_w { slot_w - w } else { 0 };
                if !cell.text.is_empty() {
                    draw_text(&mut surface, cell.text.as_str(), x + dx, y, scale, cell.color, shade, cfg.text_shadow, cfg.text_outline);
                }
                x += slot_w;
            }
        }

        update_layered(hwnd, &surface, width, height);
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
fn update_layered(hwnd: HWND, surface: &SoftwareSurface, width: i32, height: i32) {
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
            let dst = std::slice::from_raw_parts_mut(bits, surface.pixels.len());
            dst.copy_from_slice(&surface.pixels);
        }

        // pptDst 传 None：保持窗口当前位置。
        // 若传 Some((0,0)) 之类的值，UpdateLayeredWindow 会把窗口挪到该坐标——
        // 这正是旧版"一拖动就弹回原位"的元凶（每帧渲染都重置位置）。
        let mut sz = SIZE { cx: width, cy: height };
        let mut src_pt = POINT::default();

        let bf = BLENDFUNCTION {
            BlendOp: windows::Win32::Graphics::Gdi::AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
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
        // bitmap 仍选在 mem_dc 中，下次 `DibCache::get` 直接复用，零分配。
        let _ = ReleaseDC(hwnd, screen_dc);
    }
}

/// 让窗口自适应内容尺寸（保持左上角位置）。
/// 设置项变化（项开关 / 字号）后由 WM_APP_RECONFIG 调用。
pub fn fit_window_to_strip(hwnd: HWND, cfg: &Config) {
    let (w, h) = strip_natural_size(cfg);
    unsafe {
        let _ = SetWindowPos(hwnd, None, 0, 0, w, h, SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOMOVE);
        // 不调 InvalidateRect：尺寸变化后由调用方（WM_RECONFIG）直接 render 一帧，
        // 避免触发 WM_PAINT 忙循环（P0）。
    }
}
