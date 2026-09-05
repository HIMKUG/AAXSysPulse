//! 渲染器抽象与两种实现（v3 双渲染器）。
//!
//! # 职责
//!
//! 把一份 [`Snapshot`] + [`Config`] 画进离屏 [`SoftwareSurface`]。
//! 平台层（`win`）只面向 [`Renderer`] trait：选渲染器 → 画帧 → 上屏，
//! 不接触任何具体绘制细节。渲染逻辑全部落在 `ui`（零 Win32、可单测）。
//!
//! # 两种实现
//!
//! - [`StripRenderer`]：v3 横向文字条（无背景、固定槽位右对齐、阴影/描边）。默认。
//! - [`ClassicRenderer`]：旧 C 版卡片（圆角背景 + 主机名 + 行布局 + 进度条），
//!   复用 [`crate::card::draw_card`]，恢复透明度 / 形状 / 配色 / 悬停 / vert 布局。
//!
//! # 脏检查接口
//!
//! [`Renderer::frame_hash`] 返回"像素不变的门面指纹"。平台层逐帧比较指纹，
//! 相同则跳过整帧（不建画布、不 `UpdateLayeredWindow`），静止场景开销归零。
//! 指纹必须覆盖本渲染器**全部**会改变像素的输入（含全局 alpha ——
//! 透明度/悬停变化必须触发重绘）。

use crate::card::{draw_card, DrawCtx};
use crate::decor::decoration_for;
use crate::font::{measure_scaled, render_str_scaled};
use crate::offscreen::SoftwareSurface;
use crate::palette::Style;
use crate::raster::BitmapTextRasterizer;
use crate::Rgba;
use sysoverlay_core::color::Rgb;
use sysoverlay_core::config::Config;
use sysoverlay_core::layout::{compute, LayoutInput, RowsShown};
use sysoverlay_core::snapshot::{Metric, Snapshot};
use sysoverlay_core::units::{fmt_gb, fmt_pct, fmt_power, fmt_rate, StrBuf};

/// 渲染器接口：画一帧离屏内容（零 Win32、零 unsafe）。
pub trait Renderer {
    /// 稳定名称（诊断/测试用）。
    fn name(&self) -> &'static str;

    /// 画一帧到 `out`。`out` 的尺寸由平台层决定（窗口当前大小），
    /// 渲染器负责在画布内排布内容；返回**自然尺寸**（供窗口自适应）。
    fn draw(&self, out: &mut SoftwareSurface, snap: &Snapshot, cfg: &Config) -> (i32, i32);

    /// 像素不变的门面指纹（含全局 alpha）。相同 → 平台层跳过整帧。
    fn frame_hash(&self, snap: &Snapshot, cfg: &Config, w: i32, h: i32, alpha: u8) -> u64;

    /// 不绘制的自然尺寸（窗口自适应 / 启动定位用）。
    fn natural_size(&self, cfg: &Config) -> (i32, i32);
}

// ── FNV-1a 64 位折叠（两套渲染器共用） ─────────────────────

const FNV_OFFSET: u64 = 0xcbf29ce4_84222325;
const FNV_PRIME: u64 = 0x1000_0000_01b3;

/// 返回一个把字节混入 `acc` 的闭包风格的 helper，这里直接用函数实现。
fn fnv_mix(acc: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *acc ^= b as u64;
        *acc = acc.wrapping_mul(FNV_PRIME);
    }
}

fn fnv_u32(acc: &mut u64, v: u32) {
    fnv_mix(acc, &v.to_le_bytes());
}

fn fnv_u8(acc: &mut u64, v: u8) {
    fnv_mix(acc, &[v]);
}

// ── v3 横向条（StripRenderer） ─────────────────────────────

/// 槽位宽度（字符数）——按各类数值的**最大**模板串宽取值：
/// 百分比 `100%`=4；GB 占用 `999.9GB`=7；网速 `888.8M/s`=8；功耗 `~8888W`=6。
const SLOT_PCT: u32 = 4;
const SLOT_GB: u32 = 7;
const SLOT_RATE: u32 = 8;
const SLOT_POWER: u32 = 6;
/// 标签与数值间距 = 2 个空格；项与项间距 = 4 个空格。
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

/// 按 cfg 汇总要显示的单元序列（与 draw 的绘制走同一份布局逻辑）。
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
        fmt_rate(&mut rx, snap.net.rx_bps, cfg.net_unit);
        rx.push_str("/s");
        cells.push(Cell { text: rx, slot: SLOT_RATE, color: style.ui.value });
        cells.push(gap_cell(1));
        cells.push(text_cell("\u{2193}", 0, style.ui.name)); // ↓
        let mut tx = StrBuf::new();
        fmt_rate(&mut tx, snap.net.tx_bps, cfg.net_unit);
        tx.push_str("/s");
        cells.push(Cell { text: tx, slot: SLOT_RATE, color: style.ui.value });
        any = true;
    }

    // Power：power_enabled 同时是采样开关与显示开关
    if cfg.power_enabled {
        let (v, stale, ok) = metric_parts(&snap.power);
        let mut b = StrBuf::new();
        if ok {
            // 整机功耗 = 实测(CPU/GPU) + 估算(内存/主板/硬盘) → 恒带 ~ 前缀
            fmt_power(&mut b, v, true);
        } else {
            b.push_str("--");
        }
        let color = if stale { style.ui.value_stale } else { style.ui.value };
        push_item!("Power", SLOT_POWER, b, color);
    }

    let _ = any;
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

/// 阴影/描边颜色：字体亮 → 黑影，字体暗 → 白影（与字体相反）。
fn shade_color(fg: Rgb) -> Rgba {
    let lum = 0.299 * fg.r as f32 + 0.587 * fg.g as f32 + 0.114 * fg.b as f32;
    if lum >= 128.0 {
        Rgba::opaque(0, 0, 0)
    } else {
        Rgba::opaque(255, 255, 255)
    }
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

/// v3 横向文字条渲染器（无背景，默认风格）。
#[derive(Clone, Copy, Debug, Default)]
pub struct StripRenderer;

impl StripRenderer {
    /// 横向条的自然尺寸（像素）：由 cfg 决定（项开关 + 字号），与数值无关。
    ///
    /// 布局：PAD_X + Σ(label + 2 + slot[ + 项间 4]) + PAD_X（字符单位），
    /// 1 字符 = (8+1) × scale 像素；高度 = (16 + 2×PAD_Y) × scale。
    fn natural_size(cfg: &Config) -> (i32, i32) {
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
}

impl Renderer for StripRenderer {
    fn name(&self) -> &'static str {
        "strip"
    }

    fn draw(&self, out: &mut SoftwareSurface, snap: &Snapshot, cfg: &Config) -> (i32, i32) {
        out.clear();
        let scale = cfg.font_scale.clamp(1, 4);
        let style = Style::derive(Rgb::default(), cfg.fg);
        let shade = shade_color(cfg.fg);
        let cells = build_cells(snap, cfg, &style);

        // 字形基线垂直居中
        let glyph_h = 16 * scale as i32;
        let pad_y = PAD_Y as i32 * scale as i32;
        let y = pad_y.max((out.height - glyph_h) / 2);
        let mut x = PAD_X as i32 * scale as i32;
        let char_step = 9 * scale as i32;

        for cell in &cells {
            if cell.slot == 0 {
                let w = measure_scaled(cell.text.as_str(), scale);
                if !cell.text.is_empty() {
                    draw_text(out, cell.text.as_str(), x, y, scale, cell.color, shade, cfg.text_shadow, cfg.text_outline);
                }
                x += w;
            } else {
                // 固定槽位：右对齐（数值位数变化不影响后续元素）
                let slot_w = cell.slot as i32 * char_step;
                let w = measure_scaled(cell.text.as_str(), scale);
                let dx = if w < slot_w { slot_w - w } else { 0 };
                if !cell.text.is_empty() {
                    draw_text(out, cell.text.as_str(), x + dx, y, scale, cell.color, shade, cfg.text_shadow, cfg.text_outline);
                }
                x += slot_w;
            }
        }

        Self::natural_size(cfg)
    }

    fn frame_hash(&self, snap: &Snapshot, cfg: &Config, w: i32, h: i32, alpha: u8) -> u64 {
        let scale = cfg.font_scale.clamp(1, 4);
        let style = Style::derive(Rgb::default(), cfg.fg);
        let cells = build_cells(snap, cfg, &style);

        let mut acc = FNV_OFFSET;
        fnv_u32(&mut acc, cfg.font_scale);
        fnv_u8(&mut acc, cfg.text_shadow as u8);
        fnv_u8(&mut acc, cfg.text_outline as u8);
        fnv_u8(&mut acc, alpha);
        fnv_u32(&mut acc, w as u32);
        fnv_u32(&mut acc, h as u32);
        fnv_u32(&mut acc, scale);
        for c in &cells {
            for b in c.text.as_str().as_bytes() {
                fnv_mix(&mut acc, &[*b]);
            }
            fnv_mix(&mut acc, &[0xFF]); // 文本分隔符：防止 "AB"+"C" 与 "A"+"BC" 撞车
            fnv_u32(&mut acc, c.slot);
            fnv_u8(&mut acc, c.color.r);
            fnv_u8(&mut acc, c.color.g);
            fnv_u8(&mut acc, c.color.b);
            fnv_u8(&mut acc, c.color.a);
        }
        acc
    }

    fn natural_size(&self, cfg: &Config) -> (i32, i32) {
        Self::natural_size(cfg)
    }
}

// ── 旧 C 版卡片（ClassicRenderer） ─────────────────────────

/// 旧 C 版卡片渲染器：圆角背景 + 主机名 + 行布局 + 进度条。
///
/// 复用 [`crate::card::draw_card`] 骨架：形状 / 配色 / 悬停 alpha /
/// vert 布局全部由 `cfg` 与 `core::layout` 决定。
#[derive(Clone, Copy, Debug, Default)]
pub struct ClassicRenderer;

impl ClassicRenderer {
    /// 布局（供自然尺寸 / 哈希共用）。
    fn layout_of(snap: &Snapshot, cfg: &Config) -> sysoverlay_core::layout::CardLayout {
        let input = LayoutInput {
            zoom: cfg.effective_zoom(),
            shape: cfg.shape,
            vert: cfg.vert,
            minimal: cfg.minimal,
            dpi: 96,
            rows: RowsShown::from_snapshot(snap, cfg.power_enabled),
        };
        compute(&input)
    }
}

impl Renderer for ClassicRenderer {
    fn name(&self) -> &'static str {
        "classic"
    }

    fn draw(&self, out: &mut SoftwareSurface, snap: &Snapshot, cfg: &Config) -> (i32, i32) {
        let layout = Self::layout_of(snap, cfg);
        let ctx = DrawCtx {
            style: Style::derive(cfg.bg, cfg.fg),
            text: &BitmapTextRasterizer,
            decor: decoration_for(cfg.render_mode),
            // 悬停/透明度由平台层统一走 BLENDFUNCTION（比逐像素缩放快），
            // 软件路径保持 1.0，避免双重衰减。
            hover_alpha: 1.0,
        };
        draw_card(out, snap, cfg, &layout, &ctx);
        (layout.width, layout.height)
    }

    fn frame_hash(&self, snap: &Snapshot, cfg: &Config, w: i32, h: i32, alpha: u8) -> u64 {
        let mut acc = FNV_OFFSET;
        // 布局相关配置
        fnv_u32(&mut acc, cfg.shape.to_config());
        fnv_u32(&mut acc, cfg.vert.to_config());
        fnv_u8(&mut acc, cfg.minimal as u8);
        fnv_u32(&mut acc, cfg.effective_zoom().to_config());
        // 配色
        fnv_u32(&mut acc, cfg.bg.to_colorref());
        fnv_u32(&mut acc, cfg.fg.to_colorref());
        fnv_u8(&mut acc, cfg.net_unit.to_config() as u8);
        fnv_u8(&mut acc, cfg.mem_show_gb as u8);
        fnv_u8(&mut acc, cfg.power_enabled as u8);
        fnv_u32(&mut acc, w as u32);
        fnv_u32(&mut acc, h as u32);
        fnv_u8(&mut acc, alpha);
        // 数据值（四舍五入到 0.1 防抖动）
        let q = |v: f32| (v.clamp(0.0, 16384.0) * 10.0) as i32;
        fnv_u32(&mut acc, q(snap.cpu.value().unwrap_or(-1.0)) as u32);
        fnv_u32(&mut acc, q(snap.memory.load_pct) as u32);
        fnv_u32(&mut acc, q(snap.memory.used_gb) as u32);
        fnv_u32(&mut acc, q(snap.gpu.value().unwrap_or(-1.0)) as u32);
        fnv_u32(&mut acc, (snap.net.rx_bps >> 4) as u32);
        fnv_u32(&mut acc, (snap.net.tx_bps >> 4) as u32);
        fnv_u32(&mut acc, q(snap.power.value().unwrap_or(-1.0)) as u32);
        acc
    }

    fn natural_size(&self, cfg: &Config) -> (i32, i32) {
        // 经典卡片尺寸不依赖快照（行数由 show_* 开关 + power 决定）。
        // 用 RowsShown::all() 近似会偏大；这里按开关构造一个虚拟空快照来确定行集。
        let mut snap = Snapshot::placeholder(0);
        if cfg.show_cpu {
            snap.cpu = Metric::Value(0.0);
        }
        if cfg.show_gpu {
            snap.gpu = Metric::Value(0.0);
        }
        if cfg.power_enabled {
            snap.power = Metric::Value(0.0);
        }
        let layout = Self::layout_of(&snap, cfg);
        (layout.width, layout.height)
    }
}

// ── 测试 ─────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_cfg() -> Config {
        Config::default()
    }

    #[test]
    fn strip_and_classic_both_render_pixels() {
        let snap = crate::card::demo_snapshot();
        for renderer in [&StripRenderer as &dyn Renderer, &ClassicRenderer as &dyn Renderer] {
            let (w, h) = renderer.natural_size(&demo_cfg());
            let mut s = SoftwareSurface::new(w, h);
            renderer.draw(&mut s, &snap, &demo_cfg());
            let lit = s.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
            assert!(lit > 0, "{} 渲染器必须画出像素（lit={}）", renderer.name(), lit);
        }
    }

    #[test]
    fn classic_respects_shape_content_transparency() {
        let mut cfg = demo_cfg();
        cfg.shape = sysoverlay_core::types::Shape::Content;
        let mut s = SoftwareSurface::new(300, 200);
        ClassicRenderer.draw(&mut s, &crate::card::demo_snapshot(), &cfg);
        // Content 模式：无背景 → 左上角（无文字处）透明
        let idx = ((1 * s.width + 1) * 4) as usize;
        assert_eq!(s.pixels[idx + 3], 0, "Content 模式背景必须透明");
    }

    #[test]
    fn classic_natural_size_grows_with_power() {
        let mut cfg = demo_cfg();
        let (_, h0) = ClassicRenderer.natural_size(&cfg);
        cfg.power_enabled = true;
        let (_, h1) = ClassicRenderer.natural_size(&cfg);
        assert!(h1 > h0, "启用功耗后经典卡片必须更高（{h0} → {h1}）");
    }

    #[test]
    fn strip_alpha_and_values_change_hash() {
        let snap = crate::card::demo_snapshot();
        let cfg = demo_cfg();
        let h1 = StripRenderer.frame_hash(&snap, &cfg, 300, 36, 90);
        let h2 = StripRenderer.frame_hash(&snap, &cfg, 300, 36, 91);
        assert_ne!(h1, h2, "alpha 变化必须改变指纹");

        let mut cfg2 = cfg;
        cfg2.net_unit = sysoverlay_core::types::NetUnit::GB;
        let h3 = StripRenderer.frame_hash(&snap, &cfg2, 300, 36, 90);
        assert_ne!(h1, h3, "单位切换必须改变指纹");
    }

    #[test]
    fn classic_hash_changes_with_shape() {
        let snap = crate::card::demo_snapshot();
        let cfg = demo_cfg();
        let h1 = ClassicRenderer.frame_hash(&snap, &cfg, 280, 190, 90);
        let mut cfg2 = cfg;
        cfg2.shape = sysoverlay_core::types::Shape::Square;
        let h2 = ClassicRenderer.frame_hash(&snap, &cfg2, 280, 190, 90);
        assert_ne!(h1, h2);
    }

    #[test]
    fn renderer_names_are_stable() {
        assert_eq!(StripRenderer.name(), "strip");
        assert_eq!(ClassicRenderer.name(), "classic");
    }
}