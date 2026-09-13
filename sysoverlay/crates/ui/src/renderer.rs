//! 渲染器抽象与横/竖两种条状实现（v4）。
//!
//! # 职责
//!
//! 把一份 [`Snapshot`] + [`Config`] 画进离屏 [`SoftwareSurface`]。
//! 平台层（`win`）只面向 [`Renderer`] trait：选渲染器 → 画帧 → 上屏，
//! 不接触任何具体绘制细节。渲染逻辑全部落在 `ui`（零 Win32、可单测）。
//!
//! # 两种实现（v4，卡片式已删除）
//!
//! - [`StripRenderer`]：横版文字条（各指标左到右一字排开）。默认。
//! - [`VerticalRenderer`]：竖版文字条（同内容纵向排列）。
//!
//! 两者共享同一套"指标单元"构建逻辑（[`MetricRow`]）：
//! - 每个指标 = 名称 + [`Config::label_value_gap`]px 间距 + 数值（右对齐到固定槽位）
//! - 指标间以 [`Config::metric_gap`]px 分隔（横版=水平、竖版=上下）
//! - 字号：全局 [`Config::font_size`]，可被 [`Config::metric_font_sizes`] 逐指标覆盖
//! - 颜色：名称/数值分别允许 [`Config::metric_colors`] 逐槽覆盖（索引 = core::Metric）
//!
//! # 脏检查接口
//!
//! [`Renderer::frame_hash`] 返回"像素不变的门面指纹"。平台层逐帧比较指纹，
//! 相同则跳过整帧（不建画布、不 `UpdateLayeredWindow`），静止场景开销归零。
//! 指纹必须覆盖本渲染器**全部**会改变像素的输入（含全局 alpha ——
//! 透明度/悬停变化必须触发重绘）。

use crate::font::{measure_scaled_f, render_str_scaled_f};
use crate::offscreen::SoftwareSurface;
use crate::palette::Style;
use crate::Rgba;
use aaxsyspulse_core::color::Rgb;
use aaxsyspulse_core::config::Config;
use aaxsyspulse_core::snapshot::{Metric as SnapMetric, Snapshot};
use aaxsyspulse_core::types::Metric as MetricEnum;
use aaxsyspulse_core::types::NetMode;
use aaxsyspulse_core::units::{fmt_gb, fmt_pct, fmt_power, fmt_rate, StrBuf};

/// 渲染器接口：画一帧离屏内容（零 Win32、零 unsafe）。
pub trait Renderer {
    /// 稳定名称（诊断/测试用）。
    fn name(&self) -> &'static str;

    /// 画一帧到 `out`（默认实现：先 `layout` 再 `paint(full_clear=true)`）。
    fn draw(&self, out: &mut SoftwareSurface, snap: &Snapshot, cfg: &Config) -> (i32, i32) {
        let (nat_w, nat_h, cells) = self.layout(snap, cfg, out.width, out.height);
        self.paint(out, snap, cfg, &cells, true, None);
        (nat_w, nat_h)
    }

    /// 像素不变的门面指纹（含全局 alpha）。相同 → 平台层跳过整帧。
    /// 默认实现：layout 一次后再算，避免与 `draw` 重复 `build_rows`（C1）。
    fn frame_hash(&self, snap: &Snapshot, cfg: &Config, w: i32, h: i32, alpha: u8) -> u64 {
        let (_, _, cells) = self.layout(snap, cfg, w, h);
        self.hash_from_cells(&cells, cfg, w, h, alpha)
    }

    /// 不绘制的自然尺寸（窗口自适应 / 启动定位用）。
    fn natural_size(&self, cfg: &Config) -> (i32, i32);

    /// 布局：构建指标单元，返回自然尺寸与每个文字 cell 的几何 + 内容指纹。
    ///
    /// `build_rows` 在此只跑一次（C1），其产物既供 `hash_from_cells` 算指纹，
    /// 又供 `paint` 绘制——彻底消除"一帧 build_rows 两次"的重复计算。
    /// `w`/`h` 为画布（窗口）像素尺寸，用于垂直居中等定位。
    fn layout(&self, snap: &Snapshot, cfg: &Config, w: i32, h: i32) -> (i32, i32, Vec<CellSig>);

    /// 由 cell 指纹直接算门面指纹（与 `frame_hash` 等价，但复用 `layout` 结果）。
    fn hash_from_cells(&self, cells: &[CellSig], cfg: &Config, w: i32, h: i32, alpha: u8) -> u64;

    /// 绘制：把 `cells` 落到 `out`。
    /// `full_clear=true` 时整画布清零（首帧 / alpha 变化 / 布局变化）；
    /// 否则只清每个 cell 的包围盒（A3：省掉 187KB 整画布 memset）。
    /// `changed`（v3 性能档）：非整帧重画时按位标记哪些 cell 内容变了——
    /// 未变化的 cell 其像素已在复用画布上，**直接跳过重画**（标签零开销）。
    /// None = 全部重画。调用方保证：跳过的 cell 上帧像素完好（画布跨帧复用）。
    fn paint(
        &self,
        out: &mut SoftwareSurface,
        _snap: &Snapshot,
        _cfg: &Config,
        cells: &[CellSig],
        full_clear: bool,
        changed: Option<&[bool]>,
    ) -> (i32, i32) {
        if full_clear {
            out.clear();
        }
        for (i, c) in cells.iter().enumerate() {
            if let Some(ch) = changed {
                if !ch.get(i).copied().unwrap_or(true) {
                    continue; // 内容未变：像素仍在画布上，跳过
                }
            }
            if !full_clear {
                let pad = if c.outline { 2 } else if c.shadow { 1 } else { 0 };
                out.clear_rect(c.bbox_x, c.y - pad, c.w + 2 * pad, c.h + 2 * pad);
            }
            draw_row_text(out, c.text.as_str(), c.x, c.y, c.scale, c.color, c.shade, c.shadow, c.outline);
        }
        (0, 0)
    }
}

// ── FNV-1a 64 位折叠（两个渲染器共用，u64 分块，C5） ─────────

const FNV_OFFSET: u64 = 0xcbf29ce4_84222325;
const FNV_PRIME: u64 = 0x1000_0000_01b3;

fn fnv_mix(acc: &mut u64, bytes: &[u8]) {
    let mut chunks = bytes.chunks_exact(8);
    for ch in chunks.by_ref() {
        let v = u64::from_le_bytes([ch[0], ch[1], ch[2], ch[3], ch[4], ch[5], ch[6], ch[7]]);
        *acc ^= v;
        *acc = acc.wrapping_mul(FNV_PRIME);
    }
    for &b in chunks.remainder() {
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

fn fnv_f32(acc: &mut u64, v: f32) {
    fnv_u32(acc, v.to_bits());
}

// ── 共享：指标单元 ─────────────────────────────────────────

/// 槽位宽度（字符数）——按各类数值的**最大**模板串宽取值：
/// 百分比 `100%`=4；GB 占用 `999.9GB`=7；网速 `888.8M/s`=8；功耗 `8888W`=5。
pub const SLOT_PCT: u32 = 4;
pub const SLOT_GB: u32 = 7;
pub const SLOT_RATE: u32 = 8;
pub const SLOT_POWER: u32 = 5;
/// 左右内边距（像素）。
pub const PAD_X: i32 = 8;
/// 上下内边距（像素）。
pub const PAD_Y: i32 = 6;

/// 一个指标单元的**最终显示数据**（构建后即不可变，供绘制/测宽/哈希共用）。
pub struct MetricRow {
    /// 指标种类（配色槽位索引 = `metric.to_core()`）。
    pub metric: MetricEnum,
    /// 名称文本（短名，如 "CPU"）。
    pub label: StrBuf,
    /// 该项实际生效的字号缩放比。
    pub scale: f32,
    /// 该项实际生效的字号（像素，诊断/哈希用）。
    pub font_size: f32,
    /// 数值槽位宽度（字符数，横版右对齐用）。
    pub slot: u32,
    /// 名称颜色。
    pub label_color: Rgba,
    /// 数值颜色。
    pub value_color: Rgba,
    /// 数值文本（钉在固定槽位内右对齐）。
    pub value: StrBuf,
    /// 该项「名称 ↔ 数值」间距（像素）。
    pub label_value_gap: i32,
    /// 名称实测像素宽（C3：构建时算一次，draw 不再重测）。
    pub label_w: i32,
    /// 数值实测像素宽（C3）。
    pub value_w: i32,
    /// 槽位实测像素宽 = 单字符宽 × slot（C3）。
    pub slot_w: i32,
    /// 名称文字偏移（px）：+X 向右、+Y 向下。
    pub label_dx: i32,
    pub label_dy: i32,
    /// 数值文字偏移（px）。
    pub value_dx: i32,
    pub value_dy: i32,
}

impl MetricRow {
    fn empty() -> Self {
        Self {
            metric: MetricEnum::Cpu,
            label: StrBuf::new(),
            scale: 1.0,
            font_size: 16.0,
            slot: SLOT_PCT,
            label_color: Rgba::opaque(0, 0, 0),
            value_color: Rgba::opaque(0, 0, 0),
            value: StrBuf::new(),
            label_value_gap: 0,
            label_w: 0,
            value_w: 0,
            slot_w: 0,
            label_dx: 0,
            label_dy: 0,
            value_dx: 0,
            value_dy: 0,
        }
    }
}

/// 固定容量（最多 7 项：CPU/Mem/GPU/Net/Power/Time/Util）的栈上指标列表（C2：零堆分配）。
const MAX_METRICS: usize = 7;
pub struct RowList {
    items: [MetricRow; MAX_METRICS],
    count: usize,
}

impl RowList {
    fn new() -> Self {
        Self {
            items: [
                MetricRow::empty(),
                MetricRow::empty(),
                MetricRow::empty(),
                MetricRow::empty(),
                MetricRow::empty(),
                MetricRow::empty(),
                MetricRow::empty(),
            ],
            count: 0,
        }
    }
    fn push(&mut self, r: MetricRow) {
        if self.count < MAX_METRICS {
            self.items[self.count] = r;
            self.count += 1;
        }
    }
    fn as_slice(&self) -> &[MetricRow] {
        &self.items[..self.count]
    }
    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// 一个文字 cell 的**完整绘制指令 + 内容指纹**（供脏矩形上屏复用）。
///
/// `layout` 产出、`paint` 消费、`gdi` 比对上一帧算脏区。
/// `x/y/w/h` 是绘制/裁剪包围盒（含阴影/描边外扩），`hash` 是内容指纹。
#[derive(Clone)]
pub struct CellSig {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// 脏区/清除用包围盒左缘。
    ///
    /// 右侧数值 cell 右对齐时，ink 会向左溢出 `x`（D2D 真字形宽度 ≠
    /// 位图等宽度量），故包围盒必须比 `x` 更靠左覆盖整个预留槽位，
    /// 否则数值变化时左侧残影无法被清除/上传（幽灵像素）。
    pub bbox_x: i32,
    /// 该 cell 是否为右对齐数值（槽位 > 0）。
    ///
    /// D2D 引擎据此用 DWrite 实测字形宽把绘制原点重新对齐到布局锚点，
    /// 消除比例字体导致的逐行右缘漂移（位图度量 vs DIN 真字形）。
    pub right_align: bool,
    pub hash: u64,
    pub text: StrBuf,
    pub color: Rgba,
    pub shade: Rgba,
    pub shadow: bool,
    pub outline: bool,
    pub scale: f32,
}

/// 构造一个文字 cell 的绘制指令 + 内容指纹。
///
/// 包围盒按阴影/描边外扩（outline=±1、shadow=+1），保证 `draw_row_text`
/// 的所有偏移像素都落在 `w/h` 内（脏区上屏不漏像素）。
///
/// `h_rows` 为墨迹行数（引擎相关，见 [`visible_rows`]）。
/// `slot_w`：数值 cell 固定槽位宽度（像素），用于扩大脏区确保右对齐覆盖完整。
#[allow(clippy::too_many_arguments)]
fn cell_sig(
    text: &StrBuf,
    x: i32,
    y: i32,
    h_rows: f32,
    scale: f32,
    color: Rgba,
    shade: Rgba,
    shadow: bool,
    outline: bool,
    slot_w: i32,
) -> CellSig {
    let text_w = measure_scaled_f(text.as_str(), scale);
    let pad = if outline { 2 } else if shadow { 1 } else { 0 };
    // 槽位 > 0 = 右对齐数值 cell（标签 cell 传 0）。
    // 右对齐锚点 = x + text_w（布局按位图度量把右缘对齐到该点）；
    // 包围盒向左覆盖整个预留槽位（max(text_w, slot_w)），以容纳 D2D 真字形
    // 向右对齐修正后向左溢出的那部分 ink。
    let reserved = text_w.max(slot_w);
    let right_align = slot_w > 0;
    let bbox_x = if right_align {
        x + text_w - reserved - pad
    } else {
        x - pad
    };
    let w = reserved + 2 * pad;
    let h = (h_rows * scale).ceil() as i32;
    let mut acc = FNV_OFFSET;
    for b in text.as_str().as_bytes() {
        fnv_mix(&mut acc, &[*b]);
    }
    fnv_u32(&mut acc, color.to_colorref());
    fnv_u8(&mut acc, color.a);
    fnv_u32(&mut acc, shade.to_colorref());
    fnv_u8(&mut acc, shade.a);
    fnv_u8(&mut acc, shadow as u8);
    fnv_u8(&mut acc, outline as u8);
    fnv_f32(&mut acc, scale);
    CellSig {
        x,
        y,
        w,
        h: h + 2 * pad,
        bbox_x,
        right_align,
        hash: acc,
        text: *text,
        color,
        shade,
        shadow,
        outline,
        scale,
    }
}

/// 把 snapshot 指标归一化成 (数值, 是否陈旧, 是否可用)。
fn metric_parts(m: &SnapMetric<f32>) -> (f32, bool, bool) {
    match m {
        SnapMetric::Value(v) => (*v, false, true),
        SnapMetric::Stale { value, .. } => (*value, true, true),
        SnapMetric::Unavailable => (0.0, false, false),
    }
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

/// core::Metric 在 [`Config::metric_colors`] / [`Config::metric_font_sizes`] 里的下标。
fn metric_index(m: MetricEnum) -> usize {
    match m {
        MetricEnum::Cpu => 0,
        MetricEnum::Mem => 1,
        MetricEnum::Gpu => 2,
        MetricEnum::Net => 3,
        MetricEnum::Power => 4,
        MetricEnum::Time => 5,
        MetricEnum::Util => 6,
    }
}

/// 名称/数值配色：指标专属色优先，否则回退到 fallback。
/// 数值槽开启「自动色」时忽略自定义色，直接采用 fallback——
/// CPU/GPU/内存百分比的 fallback 即占用率强调色
/// （`style.accent(pct)`：<60% 绿 / 60~85% 橙 / ≥85% 红）。
fn slot_color(cfg: &Config, m: MetricEnum, label: bool, fallback: Rgba) -> Rgba {
    let mc = &cfg.metric_colors[metric_index(m)];
    if !label && mc.value_auto {
        return fallback;
    }
    let over = if label { mc.label } else { mc.value };
    match over {
        Some(c) => Rgba::opaque(c.r, c.g, c.b),
        None => fallback,
    }
}

/// 指标专属字号（像素）：逐指标覆盖优先，否则全局。
fn font_size_for(cfg: &Config, m: MetricEnum) -> f32 {
    cfg.metric_font_sizes[metric_index(m)].unwrap_or(cfg.font_size)
}

// ── 字重（细/中/粗） ─────────────────────────────────────

/// 字重档位 → 笔画覆盖率系数（0=细 0.45 / 1=中 0.70 / 2=粗 1.0）。
///
/// 位图字体只有一套字形，三档以 **alpha 调制**近似：覆盖率低的字在
/// 透明底上观感更细。1.0 = 原样（粗，历史默认观感）。阴影/描边同步
/// 弱化，保证相对对比不变。
pub const FONT_WEIGHT_FACTORS: [f32; 3] = [0.45, 0.70, 1.0];

pub fn font_weight_factor(w: u8) -> f32 {
    FONT_WEIGHT_FACTORS[(w as usize).min(FONT_WEIGHT_FACTORS.len() - 1)]
}

/// 按字重系数弱化颜色。输入/输出均为预乘 [`Rgba`]。
fn weight_faded(c: Rgba, factor: f32) -> Rgba {
    if factor >= 0.999 || c.a == 0 {
        return c;
    }
    // 预乘 → 直线 RGB → 以缩放后的 alpha 重新预乘（Rgba::new 负责预乘）。
    let inv = 255.0 / c.a as f32;
    let straight = |v: u8| ((v as f32 * inv).round() as i32).clamp(0, 255) as u8;
    let a = ((c.a as f32 * factor).round() as i32).clamp(1, 255) as u8;
    Rgba::new(straight(c.r), straight(c.g), straight(c.b), a)
}

/// 纵向墨迹行数（按渲染引擎）。
///
/// 性能模式位图字体把 7~8 行字形塞进 16 行单元（下半部全空），行高按
/// 可见行数预留，间距=0 时墨迹紧贴；高画质 DWrite 字体（DIN Black）
/// 上伸/下伸部超出位图单元，按完整 16 行 em 预留，否则竖版行间会互相
/// 压字、脏区也盖不住墨迹边缘。
fn visible_rows(cfg: &Config) -> f32 {
    if cfg.render_engine == aaxsyspulse_core::types::RenderEngine::D2d {
        16.0
    } else {
        crate::font::GLYPH_VISIBLE_ROWS_F
    }
}

/// 字号 → 渲染缩放比。
fn font_scale_of(size: f32) -> f32 {
    crate::font::scale_of_font(size)
}

/// 汇总按 cfg 的所有指标单元。**显示顺序 = `cfg.metric_order`**（设置页可拖动排序）；
/// 关闭的项（含时钟行）不产出行。单次构建（C1/C2）。
fn build_rows(snap: &Snapshot, cfg: &Config) -> RowList {
    let style = Style::derive(Rgb::default(), cfg.fg);
    let mut rows = RowList::new();
    for &m_idx in cfg.metric_order.iter() {
        let Some(m) = MetricEnum::ALL.get(m_idx as usize) else { continue };
        if let Some(row) = build_row(*m, snap, cfg, style) {
            rows.push(row);
        }
    }
    rows
}

/// 时间行数值文本：`HH:MM:SS`（show_seconds）或 `HH:MM`。
/// 时间来自 [`crate::clock`]（平台层每次渲染前写入本地时间），
/// 帧指纹随秒变化 → 平台层脏检查只重绘时间 cell（脏渲染）。
pub const TIME_SLOT_WITH_SEC: u32 = 8; // "HH:MM:SS"
pub const TIME_SLOT_NO_SEC: u32 = 5; // "HH:MM"

fn time_value(cfg: &Config) -> (StrBuf, u32) {
    let (h, m, s) = crate::clock::get();
    let mut b = StrBuf::new();
    push_two_digits(&mut b, h);
    b.push_str(":");
    push_two_digits(&mut b, m);
    let slot = if cfg.time_show_seconds {
        b.push_str(":");
        push_two_digits(&mut b, s);
        TIME_SLOT_WITH_SEC
    } else {
        TIME_SLOT_NO_SEC
    };
    (b, slot)
}

/// 写两位十进制数（补零；值 > 99 截断高位，时钟值域内不会发生）。
fn push_two_digits(b: &mut StrBuf, v: u32) {
    let v = v % 100;
    let tens = b'0' + (v / 10) as u8;
    let ones = b'0' + (v % 10) as u8;
    b.push_str(core::str::from_utf8(&[tens, ones]).unwrap_or("00"));
}

/// 构建单个指标的行；该项关闭 / 数据缺失时返回 None。
fn build_row(m: MetricEnum, snap: &Snapshot, cfg: &Config, style: Style) -> Option<MetricRow> {
    match m {
        MetricEnum::Cpu => {
            if !cfg.show_cpu {
                return None;
            }
            let (v, stale, ok) = metric_parts(&snap.cpu);
            let mut b = StrBuf::new();
            if ok {
                fmt_pct(&mut b, v);
            } else {
                b.push_str("--");
            }
            let color = if stale {
                style.ui.value_stale
            } else {
                style.accent(v)
            };
            let label = metric_label(cfg, 0, "CPU", "C");
            Some(new_row(
                MetricEnum::Cpu,
                label,
                b,
                SLOT_PCT,
                cfg,
                style.ui.name,
                color,
            ))
        }
        MetricEnum::Mem => {
            if !cfg.show_mem {
                return None;
            }
            let mut b = StrBuf::new();
            let slot = if cfg.mem_show_gb {
                // 实际占用量，不显示总量：如 `8.5GB`
                fmt_gb(&mut b, snap.memory.used_gb);
                SLOT_GB
            } else {
                fmt_pct(&mut b, snap.memory.load_pct);
                SLOT_PCT
            };
            let color = if cfg.mem_show_gb {
                style.ui.value
            } else {
                style.accent(snap.memory.load_pct)
            };
            let label = metric_label(cfg, 1, "Mem", "M");
            Some(new_row(
                MetricEnum::Mem,
                label,
                b,
                slot,
                cfg,
                style.ui.name,
                color,
            ))
        }
        MetricEnum::Gpu => {
            if !cfg.show_gpu {
                return None;
            }
            let (v, stale, ok) = metric_parts(&snap.gpu);
            let mut b = StrBuf::new();
            if ok {
                fmt_pct(&mut b, v);
            } else {
                b.push_str("--");
            }
            let color = if stale {
                style.ui.value_stale
            } else {
                style.accent(v)
            };
            let label = metric_label(cfg, 2, "GPU", "G");
            Some(new_row(
                MetricEnum::Gpu,
                label,
                b,
                SLOT_PCT,
                cfg,
                style.ui.name,
                color,
            ))
        }
        MetricEnum::Net => {
            if !cfg.show_net {
                return None;
            }
            // 方向语义：rx = 接收 = 下载 = ↓；tx = 发送 = 上传 = ↑。
            // 标题按模式区分方向：双向 Net / 上传 NetU / 下载 NetD（单行），
            // 双向单行双值（↓下载 ↑上传）；单向只显对应速率，不再靠箭头猜方向。
            let mut b = StrBuf::new();
            let slot;
            let label_long;
            match cfg.net_mode {
                NetMode::Down => {
                    label_long = "NetD";
                    fmt_rate(&mut b, snap.net.rx_bps, cfg.net_unit);
                    b.push_str("/s");
                    slot = SLOT_RATE;
                }
                NetMode::Up => {
                    label_long = "NetU";
                    fmt_rate(&mut b, snap.net.tx_bps, cfg.net_unit);
                    b.push_str("/s");
                    slot = SLOT_RATE;
                }
                NetMode::Both => {
                    label_long = "Net";
                    // ⚠️ `fmt_rate` 内部会 `buf.clear()`。若直接在 b 上连续调用两次，
                    // 第二次会把刚拼好的「↓下载值/s ↑」整段清空，最终只剩上传值/s
                    // ——直接表现为"双向只显示一个值，且与 NetU 相同"。
                    // 必须先把 rx/tx 各自格式化到独立缓冲，再拼进 b。
                    let mut rx = StrBuf::new();
                    let mut tx = StrBuf::new();
                    fmt_rate(&mut rx, snap.net.rx_bps, cfg.net_unit);
                    fmt_rate(&mut tx, snap.net.tx_bps, cfg.net_unit);
                    b.push_str("\u{2193}");
                    b.push_str(rx.as_str());
                    b.push_str("/s \u{2191}");
                    b.push_str(tx.as_str());
                    b.push_str("/s");
                    slot = SLOT_RATE * 2 + 3;
                }
            };
            let label = metric_label(cfg, 3, label_long, "N");
            Some(new_row(
                MetricEnum::Net,
                label,
                b,
                slot,
                cfg,
                style.ui.name,
                style.ui.value,
            ))
        }
        MetricEnum::Power => {
            if !cfg.power_enabled {
                return None;
            }
            let (v, stale, ok) = metric_parts(&snap.power);
            let mut b = StrBuf::new();
            if ok {
                // 整机功耗 = 实测(CPU/GPU) + 估算(内存/主板/硬盘)
                fmt_power(&mut b, v, false);
            } else {
                b.push_str("--");
            }
            let color = if stale {
                style.ui.value_stale
            } else {
                style.ui.value
            };
            let label = metric_label(cfg, 4, "Power", "P");
            Some(new_row(
                MetricEnum::Power,
                label,
                b,
                SLOT_POWER,
                cfg,
                style.ui.name,
                color,
            ))
        }
        MetricEnum::Time => {
            if !cfg.show_time {
                return None;
            }
            // 时钟标签用 "Time"：DIN 位图字体仅覆盖 ASCII（含 D2D 引擎的
            // 字体回退行为差异），中文标签交给设置页 i18n（DWrite）。
            let (b, slot) = time_value(cfg);
            let label = metric_label(cfg, 5, "Time", "T");
            Some(new_row(
                MetricEnum::Time,
                label,
                b,
                slot,
                cfg,
                style.ui.name,
                style.ui.value,
            ))
        }
        MetricEnum::Util => {
            if !cfg.show_util {
                return None;
            }
            // 综合占用：CPU/内存/GPU 的排队论闭式解（见 core::composite），
            // 由采样线程算好填进 snap.util。三种数值态：
            // - 正常：占用率强调色（与其他指标一致的三档变色）
            // - 降级（≥1 源缺失，重归一化续算）：陈旧色提示"数值可信度下降"
            // - 不可用（全源缺失）：红色 "--" 引导注意
            let (v, stale, ok) = metric_parts(&snap.util);
            let mut b = StrBuf::new();
            if ok {
                fmt_pct(&mut b, v);
            } else {
                b.push_str("--");
            }
            let color = if !ok {
                style.accent(100.0)
            } else if stale || snap.util_partial {
                style.stale_accent(v)
            } else {
                style.accent(v)
            };
            let label = metric_label(cfg, 6, "Util", "U");
            Some(new_row(
                MetricEnum::Util,
                label,
                b,
                SLOT_PCT,
                cfg,
                style.ui.name,
                color,
            ))
        }
    }
}

///  Resolve the display label for metric at index `mi`:
///  - if `metric_show_labels[mi]` is false → empty string (label hidden)
///  - else if `short_names` is true → use short form (e.g. "C", "M")
///  - else → use full form (e.g. "CPU", "Mem")
fn metric_label(cfg: &Config, mi: usize, full: &'static str, short: &'static str) -> &'static str {
    if !cfg.metric_show_labels.get(mi).copied().unwrap_or(true) {
        ""
    } else if cfg.short_names {
        short
    } else {
        full
    }
}

/// 组装一行：字号/配色槽位 + 实测宽度在此统一解析（C3：宽度只算一次）。
fn new_row(
    metric: MetricEnum,
    label: &str,
    value: StrBuf,
    slot: u32,
    cfg: &Config,
    name_fallback: Rgba,
    value_fallback: Rgba,
) -> MetricRow {
    let size = font_size_for(cfg, metric);
    let scale = font_scale_of(size);
    let label_w = measure_scaled_f(label, scale);
    let value_w = measure_scaled_f(value.as_str(), scale);
    let slot_w = measure_scaled_f("0", scale) * slot as i32;
    // 字重（细/中/粗）：对名称/数值色做 alpha 弱化（预乘），粗 = 原样。
    let wf = font_weight_factor(cfg.font_weight);
    // 名称隐藏时：标签宽度归零、间距归零（数值紧贴左侧起始位置）。
    let label_value_gap = if label.is_empty() { 0 } else { cfg.label_value_gap as i32 };
    // 逐指标文字偏移（名称X/名称Y/数值X/数值Y）。
    let off = cfg.metric_offsets[metric_index(metric)];
    MetricRow {
        metric,
        label: label_buf(label),
        scale,
        font_size: size,
        slot,
        label_color: weight_faded(slot_color(cfg, metric, true, name_fallback), wf),
        value_color: weight_faded(slot_color(cfg, metric, false, value_fallback), wf),
        value,
        label_value_gap,
        label_w,
        value_w,
        slot_w,
        label_dx: off[0],
        label_dy: off[1],
        value_dx: off[2],
        value_dy: off[3],
    }
}

fn label_buf(s: &str) -> StrBuf {
    let mut b = StrBuf::new();
    b.push_str(s);
    b
}

// ── 绘制 ──────────────────────────────────────────────────

/// 绘制一行文字（横版/竖版共用，含阴影/描边），`scale` 为浮点缩放比。
#[allow(clippy::too_many_arguments)]
fn draw_row_text(
    out: &mut SoftwareSurface,
    text: &str,
    x: i32,
    y: i32,
    scale: f32,
    color: Rgba,
    shade: Rgba,
    shadow: bool,
    outline: bool,
) {
    if text.is_empty() {
        return;
    }
    if outline {
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            render_str_scaled_f(
                &mut out.pixels,
                out.width,
                out.height,
                text,
                x + dx,
                y + dy,
                scale,
                shade,
            );
        }
    }
    if shadow {
        render_str_scaled_f(
            &mut out.pixels,
            out.width,
            out.height,
            text,
            x + 1,
            y + 1,
            scale,
            shade,
        );
    }
    render_str_scaled_f(&mut out.pixels, out.width, out.height, text, x, y, scale, color);
}

/// 由 cell 指纹算门面指纹（横/竖版共用，逻辑完全一致）。
fn hash_cells(cells: &[CellSig], cfg: &Config, w: i32, h: i32, alpha: u8) -> u64 {
    let mut acc = FNV_OFFSET;
    fnv_f32(&mut acc, cfg.font_size);
    fnv_u8(&mut acc, cfg.font_weight); // 字重变化必须触发重绘（字形源不同 → 像素不同）
    fnv_u8(&mut acc, cfg.text_shadow as u8);
    fnv_u8(&mut acc, cfg.text_outline as u8);
    fnv_u8(&mut acc, alpha);
    fnv_u32(&mut acc, w as u32);
    fnv_u32(&mut acc, h as u32);
    fnv_u32(&mut acc, cfg.metric_gap);
    fnv_u32(&mut acc, cfg.label_value_gap);
    // 逐指标文字偏移：位置类输入，改一个像素也必须触发重绘。
    for off in cfg.metric_offsets.iter().flatten() {
        fnv_u32(&mut acc, *off as u32);
    }
    for c in cells {
        for b in c.text.as_str().as_bytes() {
            fnv_mix(&mut acc, &[*b]);
        }
        fnv_mix(&mut acc, &[0xFE]);
        fnv_u32(&mut acc, c.color.to_colorref());
        fnv_u8(&mut acc, c.color.a);
        fnv_u32(&mut acc, c.shade.to_colorref());
        fnv_u8(&mut acc, c.shade.a);
        fnv_u8(&mut acc, c.shadow as u8);
        fnv_u8(&mut acc, c.outline as u8);
        fnv_f32(&mut acc, c.scale);
        fnv_u32(&mut acc, c.x as u32);
        fnv_u32(&mut acc, c.y as u32);
        fnv_u32(&mut acc, c.w as u32);
        fnv_u32(&mut acc, c.h as u32);
    }
    acc
}

// ── 横版（StripRenderer） ─────────────────────────────────

/// v4 横版文字条渲染器（无背景，默认风格）。
#[derive(Clone, Copy, Debug, Default)]
pub struct StripRenderer;

/// 一排指标的文字偏移在水平方向的**外扩量** `(左, 右)`。
///
/// 左 = 负偏移的最大绝对值（内容向左溢出量），右 = 正偏移的最大值。
/// 自然宽度须把二者计入，否则负偏移被窗口左缘裁掉、正偏移在右缘截断。
fn off_h_span(rows: &[MetricRow]) -> (i32, i32) {
    let mut left = 0;
    let mut right = 0;
    for r in rows {
        left = left.max(-r.label_dx).max(-r.value_dx);
        right = right.max(r.label_dx).max(r.value_dx);
    }
    (left, right)
}

/// 垂直方向的外扩量 `(上, 下)`（`off_h_span` 的纵轴对应）。
fn off_v_span(rows: &[MetricRow]) -> (i32, i32) {
    let mut top = 0;
    let mut bottom = 0;
    for r in rows {
        top = top.max(-r.label_dy).max(-r.value_dy);
        bottom = bottom.max(r.label_dy).max(r.value_dy);
    }
    (top, bottom)
}

/// 横版自然宽度：PAD + Σ(label + gap + slot)（项间加 metric_gap）+ PAD。
fn strip_natural_w_rows(rows: &[MetricRow], cfg: &Config) -> i32 {
    let (left, right) = off_h_span(rows);
    let mut natural = PAD_X + left;
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            natural += cfg.metric_gap as i32;
        }
        // 实测宽度可能超过预留槽位（如 Net 双向速率字符串远长于 17 字符槽），
        // 自然宽度必须按 max(value_w, slot_w) 估，否则窗口偏窄、末尾值被右缘
        // 裁掉——拖「名称↔数值间距」时网速整段被裁，看起来就是"间距不生效"。
        natural += row.label_w + row.label_value_gap + row.value_w.max(row.slot_w);
    }
    natural + PAD_X + right
}

/// 整条内容高度（像素）：最大字号 + 上下内边距（含偏移外扩）。
fn strip_h_rows(rows: &[MetricRow]) -> i32 {
    let max_scale = rows.iter().map(|r| r.scale).fold(0.0, f32::max);
    let (top, bottom) = off_v_span(rows);
    PAD_Y * 2 + (16.0 * max_scale).ceil() as i32 + top + bottom
}

impl Renderer for StripRenderer {
    fn name(&self) -> &'static str {
        "strip"
    }

    fn layout(&self, snap: &Snapshot, cfg: &Config, _w: i32, h: i32) -> (i32, i32, Vec<CellSig>) {
        // 阴影/描边与文字同步按字重弱化，保证相对对比不变。
        let shade = weight_faded(shade_color(cfg.fg), font_weight_factor(cfg.font_weight));
        let rows = build_rows(snap, cfg);
        let rows = rows.as_slice();
        if rows.is_empty() {
            return (PAD_X * 2, PAD_Y * 2, Vec::new());
        }

        // 所有指标共享一条基线（垂直居中），各自字号独立缩放
        let max_scale = rows.iter().map(|r| r.scale).fold(0.0, f32::max);
        let glyph_h = (16.0 * max_scale).ceil() as i32;
        let ink_rows = visible_rows(cfg);
        // 偏移外扩：内容左缘 / 上缘各让出外扩量，保证负偏移不被窗口边缘裁掉。
        let (off_l, _off_r) = off_h_span(rows);
        let (off_t, off_b) = off_v_span(rows);
        let y = ((h - glyph_h - off_t - off_b) / 2).max(PAD_Y) + off_t;

        let mut cells: Vec<CellSig> = Vec::with_capacity(rows.len() * 2);
        let mut x = PAD_X + off_l;
        for (i, row) in rows.iter().enumerate() {
            if i > 0 {
                x += cfg.metric_gap as i32;
            }
            let row_h = (16.0 * row.scale).ceil() as i32;
            let dy = (glyph_h - row_h) / 2;
            let ly = y + dy;
            // 名称隐藏时跳过标签 cell（value 直接顶左对齐位置）。
            if !row.label.is_empty() {
                cells.push(cell_sig(&row.label, x + row.label_dx, ly + row.label_dy, ink_rows, row.scale, row.label_color, shade, cfg.text_shadow, cfg.text_outline, 0));
            }
            let vx = if row.label.is_empty() {
                x
            } else {
                x + row.label_w + row.label_value_gap + if row.value_w < row.slot_w { row.slot_w - row.value_w } else { 0 }
            };
            cells.push(cell_sig(&row.value, vx + row.value_dx, ly + row.value_dy, ink_rows, row.scale, row.value_color, shade, cfg.text_shadow, cfg.text_outline, row.slot_w));
            // 超宽值（Net 双向速率）按实测宽度推进，避免与下一项重叠
            x += row.label_w + row.label_value_gap + row.value_w.max(row.slot_w);
        }

        (strip_natural_w_rows(rows, cfg), h.max(strip_h_rows(rows)), cells)
    }

    fn hash_from_cells(&self, cells: &[CellSig], cfg: &Config, w: i32, h: i32, alpha: u8) -> u64 {
        hash_cells(cells, cfg, w, h, alpha)
    }

    fn natural_size(&self, cfg: &Config) -> (i32, i32) {
        let snap = Snapshot::placeholder(0);
        let rows = build_rows(&snap, cfg);
        (strip_natural_w_rows(rows.as_slice(), cfg), strip_h_rows(rows.as_slice()))
    }
}

// ── 竖版（VerticalRenderer） ──────────────────────────────

/// v4 竖版文字条渲染器：同一批指标自上而下排列，
/// 行距 = [`Config::metric_gap`]，每行内名称 ↔ 数值间距 = [`Config::label_value_gap`]。
#[derive(Clone, Copy, Debug, Default)]
pub struct VerticalRenderer;

/// 竖版内容基准宽（不含内边距与偏移）：最宽一行的 `label + gap + max(value, slot)`。
fn vertical_base_w(rows: &[MetricRow]) -> i32 {
    let mut w = 0;
    for row in rows {
        w = w.max(row.label_w + row.label_value_gap + row.value_w.max(row.slot_w));
    }
    w
}

/// 竖版自然尺寸：宽 = 最宽行，高 = Σ(行高) + Σ(行距) + 内边距。
fn vertical_natural_rows(rows: &[MetricRow], cfg: &Config) -> (i32, i32) {
    // 行高按引擎墨迹行数预留（位图字体 8 行 / DWrite 字体 16 行 em），
    // 间距为 0 时性能模式各行墨迹紧贴。
    let line_h = visible_rows(cfg);
    let (off_l, off_r) = off_h_span(rows);
    let (off_t, off_b) = off_v_span(rows);
    let mut max_w = PAD_X * 2;
    let mut total_h = PAD_Y * 2 + off_t + off_b;
    for (i, row) in rows.iter().enumerate() {
        // 同 strip：实测宽度可能超过槽位（Net 双向），取 max 防右缘裁切
        max_w = max_w.max(PAD_X + row.label_w + row.label_value_gap + row.value_w.max(row.slot_w) + PAD_X);
        total_h += (line_h * row.scale).ceil() as i32;
        if i < rows.len().saturating_sub(1) {
            total_h += cfg.metric_gap as i32;
        }
    }
    (max_w + off_l + off_r, total_h)
}

impl Renderer for VerticalRenderer {
    fn name(&self) -> &'static str {
        "vertical"
    }

    fn layout(&self, snap: &Snapshot, cfg: &Config, _w: i32, h: i32) -> (i32, i32, Vec<CellSig>) {
        // 阴影/描边与文字同步按字重弱化，保证相对对比不变。
        let shade = weight_faded(shade_color(cfg.fg), font_weight_factor(cfg.font_weight));
        let rows = build_rows(snap, cfg);
        let rows = rows.as_slice();
        if rows.is_empty() {
            return (PAD_X * 2, PAD_Y * 2, Vec::new());
        }

        // 数值统一右对齐到最宽行（竖版可读性：各行数值列对齐）。
        // ⚠️ 只对齐**内容自然宽度**（`nat_w`），不随窗口宽度漂移：
        // 若用 `w`，D2D 自愈扩窗会把窗口越扩越大 → 数值右缘又跟着右移 →
        // "扩宽→右移→再扩宽"指数级死循环（实测 text_right 301→14225 无限增大）。
        let (nat_w, nat_h) = vertical_natural_rows(rows, cfg);
        let (off_l, _off_r) = off_h_span(rows);
        let (off_t, _off_b) = off_v_span(rows);
        // 内容左缘（偏移外扩后）；数值右对齐到「内容基准宽」右缘，再各自加 value_dx。
        let origin_l = PAD_X + off_l;
        let value_right = origin_l + vertical_base_w(rows);
        let line_h = visible_rows(cfg);

        let mut cells: Vec<CellSig> = Vec::with_capacity(rows.len() * 2);
        let mut y = PAD_Y + off_t;
        for row in rows {
            // 行高用引擎墨迹行数（位图字体 8 行 / DWrite 16 行），
            // 间距为 0 时性能模式各行墨迹紧贴。
            let row_h = (line_h * row.scale).ceil() as i32;
            // 名称隐藏时跳过标签 cell（数值顶左对齐）。
            if !row.label.is_empty() {
                cells.push(cell_sig(&row.label, origin_l + row.label_dx, y + row.label_dy, line_h, row.scale, row.label_color, shade, cfg.text_shadow, cfg.text_outline, 0));
            }
            let min_x = if row.label.is_empty() { origin_l } else { origin_l + row.label_w + row.label_value_gap };
            let dx = (value_right - row.value_w).max(min_x);
            cells.push(cell_sig(&row.value, dx + row.value_dx, y + row.value_dy, line_h, row.scale, row.value_color, shade, cfg.text_shadow, cfg.text_outline, row.slot_w));
            y += row_h + cfg.metric_gap as i32;
        }

        (nat_w, h.max(nat_h), cells)
    }

    fn hash_from_cells(&self, cells: &[CellSig], cfg: &Config, w: i32, h: i32, alpha: u8) -> u64 {
        // 竖版与横版共享全部内容输入，指纹段一致即可（数据相同 → 跳过重绘）
        hash_cells(cells, cfg, w, h, alpha)
    }

    fn natural_size(&self, cfg: &Config) -> (i32, i32) {
        let snap = Snapshot::placeholder(0);
        let rows = build_rows(&snap, cfg);
        vertical_natural_rows(rows.as_slice(), cfg)
    }
}

// ── 测试 ─────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_cfg() -> Config {
        // 新默认值里 CPU/内存/GPU 显示是关的（用户拍板）；
        // 渲染测试关心的是"行的内容与几何"，与默认开关策略无关，
        // 这里显式打开以保持既有断言语义。
        let mut c = Config::default();
        c.show_cpu = true;
        c.show_mem = true;
        c.show_gpu = true;
        c
    }

    fn demo_snap() -> Snapshot {
        let mut s = Snapshot::placeholder(0);
        s.cpu = SnapMetric::Value(47.0);
        s.gpu = SnapMetric::Value(23.5);
        s.memory.load_pct = 58.0;
        s.memory.used_gb = 8.5;
        s.net.rx_bps = 1_200_000;
        s.net.tx_bps = 340_000;
        s
    }

    #[test]
    fn renderers_render_pixels() {
        let snap = demo_snap();
        for renderer in [
            &StripRenderer as &dyn Renderer,
            &VerticalRenderer as &dyn Renderer,
        ] {
            let (w, h) = renderer.natural_size(&demo_cfg());
            let mut s = SoftwareSurface::new(w.max(100), h.max(20));
            renderer.draw(&mut s, &snap, &demo_cfg());
            let lit = s.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
            assert!(lit > 0, "{} 渲染器必须画出像素（lit={}）", renderer.name(), lit);
        }
    }

    #[test]
    fn alpha_and_values_change_hash() {
        let snap = demo_snap();
        let cfg = demo_cfg();
        let h1 = StripRenderer.frame_hash(&snap, &cfg, 300, 36, 90);
        let h2 = StripRenderer.frame_hash(&snap, &cfg, 300, 36, 91);
        assert_ne!(h1, h2, "alpha 变化必须改变指纹");

        let mut cfg2 = cfg;
        cfg2.net_unit = aaxsyspulse_core::types::NetUnit::GB;
        let h3 = StripRenderer.frame_hash(&snap, &cfg2, 300, 36, 90);
        assert_ne!(h1, h3, "单位切换必须改变指纹");

        let mut cfg3 = cfg;
        cfg3.font_size = 20.0;
        let h4 = StripRenderer.frame_hash(&snap, &cfg3, 300, 36, 90);
        assert_ne!(h1, h4, "字号变化必须改变指纹");

        let mut cfg4 = cfg;
        cfg4.metric_colors[metric_index(MetricEnum::Cpu)].label =
            Some(Rgb::new(255, 0, 0));
        let h5 = StripRenderer.frame_hash(&snap, &cfg4, 300, 36, 90);
        assert_ne!(h1, h5, "逐项配色变化必须改变指纹");
    }

    #[test]
    fn metric_offsets_shift_cells_and_grow_natural_size() {
        let snap = demo_snap();
        let cfg = demo_cfg();
        // h=0 → 垂直居中被 PAD_Y 下限吞掉，基线对两版一致，便于逐值断言。
        let (nw, nh, cells) = StripRenderer.layout(&snap, &cfg, 0, 0);

        // CPU 的数值右移 5、下移 3；名称不动。
        let mut cfg2 = cfg;
        cfg2.metric_offsets[metric_index(MetricEnum::Cpu)] = [0, 0, 5, 3];
        let (nw2, nh2, cells2) = StripRenderer.layout(&snap, &cfg2, 0, 0);

        assert_eq!(cells2.len(), cells.len(), "cell 数量不随偏移变化");
        assert_eq!(
            (cells2[0].x, cells2[0].y),
            (cells[0].x, cells[0].y),
            "名称 cell 不受数值偏移影响"
        );
        assert_eq!(cells2[1].x, cells[1].x + 5, "数值 X 偏移 +5");
        assert_eq!(cells2[1].y, cells[1].y + 3, "数值 Y 偏移 +3");
        assert!(nw2 > nw, "正值 X 偏移必须扩宽自然宽度 {nw} → {nw2}");
        assert!(nh2 > nh, "正值 Y 偏移必须增高自然高度 {nh} → {nh2}");

        // 指纹随之改变（否则改偏移不触发重绘）。
        assert_ne!(
            StripRenderer.frame_hash(&snap, &cfg, 0, 0, 90),
            StripRenderer.frame_hash(&snap, &cfg2, 0, 0, 90),
            "偏移变化必须改变门面指纹"
        );

        // 竖版同样生效：数值右移 5。
        let (_, _, vcells) = VerticalRenderer.layout(&snap, &cfg, 0, 0);
        let (_, _, vcells2) = VerticalRenderer.layout(&snap, &cfg2, 0, 0);
        assert_eq!(vcells2[0].x, vcells[0].x, "竖版名称不动");
        assert_eq!(vcells2[1].x, vcells[1].x + 5, "竖版数值 X 偏移 +5");
        assert_eq!(vcells2[1].y, vcells[1].y + 3, "竖版数值 Y 偏移 +3");

        // 负偏移（向左上）也必须扩宽/增高，避免被窗口边缘裁掉。
        let mut cfg3 = cfg;
        cfg3.metric_offsets[metric_index(MetricEnum::Cpu)] = [-6, -4, 0, 0];
        let (nw3, nh3, _) = StripRenderer.layout(&snap, &cfg3, 0, 0);
        assert!(nw3 > nw, "负 X 偏移也要扩宽 {nw} → {nw3}");
        assert!(nh3 > nh, "负 Y 偏移也要增高 {nh} → {nh3}");
    }

    #[test]
    fn renderer_names_are_stable() {
        assert_eq!(StripRenderer.name(), "strip");
        assert_eq!(VerticalRenderer.name(), "vertical");
    }

    #[test]
    fn vertical_is_taller_than_strip() {
        let cfg = demo_cfg();
        let (_, sh) = StripRenderer.natural_size(&cfg);
        let (_, vh) = VerticalRenderer.natural_size(&cfg);
        assert!(vh > sh, "竖版必须比横版高（{sh} → {vh}）");
    }

    #[test]
    fn per_metric_font_size_applies_to_mem() {
        // 回归锁：mfs_mem（逐项字号覆盖，索引 1）必须真实影响 Mem 行缩放。
        // （用户实测"字号调整对内存项目不生效"——渲染管线本身要保证生效。）
        let snap = demo_snap();
        let mut cfg = demo_cfg();
        let (_, _, base) = VerticalRenderer.layout(&snap, &cfg, 400, 400);
        let base_mem = base.iter().find(|c| c.text.as_str() == "Mem").expect("Mem label cell").scale;

        cfg.metric_font_sizes[metric_index(MetricEnum::Mem)] = Some(40.0);
        let (_, _, sized) = VerticalRenderer.layout(&snap, &cfg, 400, 400);
        let mem = sized.iter().find(|c| c.text.as_str() == "Mem").expect("Mem label cell");
        assert!((mem.scale - 40.0 / 16.0).abs() < 1e-3, "Mem 缩放应 = 40/16，实际 {}", mem.scale);
        assert!(mem.scale > base_mem, "覆盖字号必须大于默认");
    }

    #[test]
    fn font_weight_fades_text_and_changes_hash() {
        let snap = demo_snap();
        let mut cfg = demo_cfg();
        cfg.font_weight = 2;
        let h_bold = StripRenderer.frame_hash(&snap, &cfg, 400, 36, 90);
        cfg.font_weight = 0;
        let h_thin = StripRenderer.frame_hash(&snap, &cfg, 400, 36, 90);
        assert_ne!(h_bold, h_thin, "字重变化必须改变帧指纹（否则切档不重绘）");

        let (_, _, cells) = StripRenderer.layout(&snap, &cfg, 400, 36);
        assert!(cells.iter().all(|c| c.color.a < 255), "细档文字必须半透明");
        assert!(cells.iter().all(|c| c.shade.a < 255), "阴影/描边须与文字同步弱化");
        cfg.font_weight = 2;
        let (_, _, cells) = StripRenderer.layout(&snap, &cfg, 400, 36);
        assert!(cells.iter().all(|c| c.color.a == 255), "粗档 = 原样不透明");
    }

    #[test]
    fn gaps_change_hash() {
        let snap = demo_snap();
        let mut cfg = demo_cfg();
        cfg.metric_gap = 24;
        let h1 = StripRenderer.frame_hash(&snap, &cfg, 400, 36, 90);
        let mut cfg2 = cfg;
        cfg2.metric_gap = 8;
        let h2 = StripRenderer.frame_hash(&snap, &cfg2, 400, 36, 90);
        assert_ne!(h1, h2, "指标间距必须改变指纹");

        let mut cfg3 = cfg;
        cfg3.label_value_gap = 20;
        let h3 = StripRenderer.frame_hash(&snap, &cfg3, 400, 36, 90);
        assert_ne!(h1, h3, "名称值间距必须改变指纹");
    }

    #[test]
    fn float_font_measure_matches_render() {
        // 最近邻字形宽度与渲染步进一致（不 panic、正数）
        let s = "CPU";
        let scale = font_scale_of(24.5);
        assert!(measure_scaled_f(s, scale) > 0);
        let mut surf = SoftwareSurface::new(200, 40);
        render_str_scaled_f(
            &mut surf.pixels,
            surf.width,
            surf.height,
            s,
            0,
            0,
            scale,
            Rgba::opaque(255, 255, 255),
        );
        assert!(surf.pixels.chunks_exact(4).any(|p| p[3] > 0), "必须画出内容");
    }

    #[test]
    fn per_metric_font_size_affects_size() {
        let mut cfg = demo_cfg();
        let base = StripRenderer.natural_size(&cfg);
        cfg.metric_font_sizes[metric_index(MetricEnum::Cpu)] = Some(40.0);
        let grown = StripRenderer.natural_size(&cfg);
        assert!(
            grown.0 > base.0,
            "CPU 字号放大后横版必须变宽（{} → {}）",
            base.0,
            grown.0
        );
    }

    #[test]
    fn net_both_row_keeps_download_and_upload_values() {
        // 回归锁：`fmt_rate` 内部会 `buf.clear()`。Both 模式若在同一缓冲上连续
        // 调用两次（中间只 push_str 分隔符），第二次会把刚拼好的
        // 「↓下载值/s ↑」整段清空，最终只剩上传值/s —— 用户实测「双向只显示
        // 一个值，且与 NetU 相同」。必须用独立缓冲分别格式化 rx/tx 再拼接。
        let mut cfg = demo_cfg();
        cfg.net_mode = NetMode::Both;
        cfg.net_unit = aaxsyspulse_core::types::NetUnit::MB;
        let mut snap = Snapshot::placeholder(0);
        snap.net.rx_bps = 11_850_000; // 11.85M（下载）
        snap.net.tx_bps = 240_000; // 0.24M（上传）

        let rows = build_rows(&snap, &cfg);
        let net = rows
            .as_slice()
            .iter()
            .find(|r| matches!(r.metric, MetricEnum::Net))
            .expect("Net 行必须存在");
        assert_eq!(net.label.as_str(), "Net", "双向模式标签为 Net");
        let s = net.value.as_str();
        assert!(s.contains('\u{2193}'), "双向必须含下载箭头 ↓：{s:?}");
        assert!(s.contains('\u{2191}'), "双向必须含上传箭头 ↑：{s:?}");
        assert!(s.contains("11.85M"), "双向必须含下载值 11.85M：{s:?}");
        assert!(s.contains("0.24M"), "双向必须含上传值 0.24M：{s:?}");
        assert!(
            s.find("11.85M").unwrap() < s.find("0.24M").unwrap(),
            "下载值须排在上传值之前：{s:?}"
        );
    }

    #[test]
    fn net_single_direction_rows_show_only_own_value() {
        // 单向模式只显示对应方向的速率（下载 NetD / 上传 NetU）。
        let mut snap = Snapshot::placeholder(0);
        snap.net.rx_bps = 11_850_000;
        snap.net.tx_bps = 240_000;

        let mut cfg = demo_cfg();
        cfg.net_unit = aaxsyspulse_core::types::NetUnit::MB;
        cfg.net_mode = NetMode::Down;
        let rows = build_rows(&snap, &cfg);
        let d = rows.as_slice().iter().find(|r| matches!(r.metric, MetricEnum::Net)).unwrap();
        assert_eq!(d.label.as_str(), "NetD");
        assert!(d.value.as_str().contains("11.85M"));
        assert!(!d.value.as_str().contains("0.24M"));

        cfg.net_mode = NetMode::Up;
        let rows = build_rows(&snap, &cfg);
        let u = rows.as_slice().iter().find(|r| matches!(r.metric, MetricEnum::Net)).unwrap();
        assert_eq!(u.label.as_str(), "NetU");
        assert!(u.value.as_str().contains("0.24M"));
        assert!(!u.value.as_str().contains("11.85M"));
    }

    #[test]
    fn value_cells_are_right_aligned_with_slot_covering_bbox() {
        // 右对齐数值 cell 必须标记 right_align，且包围盒向左覆盖整个预留槽位
        // ——D2D 真字形比位图度量宽时，右对齐修正会让 ink 向左溢出绘制原点 `x`，
        // 包围盒不覆盖就会留下幽灵像素。
        let snap = demo_snap();
        let cfg = demo_cfg();
        let (_, _, cells) = VerticalRenderer.layout(&snap, &cfg, 600, 600);

        let value = cells
            .iter()
            .find(|c| c.text.as_str().contains('%'))
            .expect("CPU 百分比数值 cell");
        assert!(value.right_align, "数值 cell 必须标记右对齐");
        assert!(
            value.bbox_x <= value.x,
            "右对齐数值包围盒左缘不得晚于绘制原点：bbox_x={} x={}",
            value.bbox_x,
            value.x
        );

        let label = cells
            .iter()
            .find(|c| c.text.as_str() == "CPU")
            .expect("CPU 标签 cell");
        assert!(!label.right_align, "标签 cell 不得右对齐");
    }

    #[test]
    fn vertical_value_cells_share_one_right_edge() {
        // 竖版右对齐锚点一致性：所有数值 cell 的「绘制原点 + 位图度量宽」必须相等。
        // D2D 引擎正是按该锚点用 DWrite 实测宽度重定位，才能让整列数值右缘对齐
        // （否则比例字形宽度差异会让各行右缘漂移，网速串最明显）。
        let snap = demo_snap();
        let mut cfg = demo_cfg();
        cfg.net_mode = NetMode::Both;
        cfg.label_value_gap = 64;
        cfg.power_enabled = true;
        let (_, _, cells) = VerticalRenderer.layout(&snap, &cfg, 900, 900);

        let mut anchors: Vec<i32> = cells
            .iter()
            .filter(|c| c.right_align)
            .map(|c| c.x + measure_scaled_f(c.text.as_str(), c.scale))
            .collect();
        assert!(anchors.len() >= 5, "应当有 5 个数值 cell，实际 {}", anchors.len());
        anchors.sort_unstable();
        anchors.dedup();
        assert_eq!(
            anchors.len(),
            1,
            "所有数值 cell 必须共享同一右对齐锚点，实际 {anchors:?}"
        );
    }

    #[test]
    fn time_row_renders_clock_text() {
        // 时钟行：默认显示，标签 "Time"，数值 = HH:MM:SS（槽位 8）。
        crate::clock::set_local(22, 12, 36);
        let cfg = demo_cfg();
        let rows = build_rows(&Snapshot::placeholder(0), &cfg);
        let t = rows
            .as_slice()
            .iter()
            .find(|r| matches!(r.metric, MetricEnum::Time))
            .expect("默认配置必须包含 Time 行");
        assert_eq!(t.label.as_str(), "Time");
        assert_eq!(t.value.as_str(), "22:12:36");
        assert_eq!(t.slot, TIME_SLOT_WITH_SEC);

        // 关闭秒 → HH:MM（槽位 5）
        let mut cfg2 = cfg;
        cfg2.time_show_seconds = false;
        let rows = build_rows(&Snapshot::placeholder(0), &cfg2);
        let t = rows
            .as_slice()
            .iter()
            .find(|r| matches!(r.metric, MetricEnum::Time))
            .expect("show_time=true 时必须有 Time 行");
        assert_eq!(t.value.as_str(), "22:12");
        assert_eq!(t.slot, TIME_SLOT_NO_SEC);

        // 完全关闭 → 无 Time 行
        let mut cfg3 = cfg;
        cfg3.show_time = false;
        let rows = build_rows(&Snapshot::placeholder(0), &cfg3);
        assert!(
            !rows.as_slice().iter().any(|r| matches!(r.metric, MetricEnum::Time)),
            "show_time=false 时不得出现 Time 行"
        );
    }

    #[test]
    fn metric_order_drives_display_order() {
        // 顺序必须真实生效：把 Time 排到最前，行序随之改变。
        crate::clock::set_local(1, 2, 3);
        let mut cfg = demo_cfg();
        cfg.metric_order = [5, 0, 1, 2, 3, 4, 6];
        let rows = build_rows(&Snapshot::placeholder(0), &cfg);
        let seq: Vec<MetricEnum> = rows.as_slice().iter().map(|r| r.metric).collect();
        assert_eq!(
            seq,
            vec![
                MetricEnum::Time,
                MetricEnum::Cpu,
                MetricEnum::Mem,
                MetricEnum::Gpu,
                MetricEnum::Net,
                MetricEnum::Util
            ],
            "metric_order 必须决定显示顺序（Power 默认关，Util 不可用仍显示 --）"
        );

        // 时间文本变化 → 帧指纹必须变化（脏渲染依赖此性质）
        let h1 = StripRenderer.frame_hash(&Snapshot::placeholder(0), &cfg, 400, 36, 90);
        crate::clock::set_local(1, 2, 4);
        let h2 = StripRenderer.frame_hash(&Snapshot::placeholder(0), &cfg, 400, 36, 90);
        assert_ne!(h1, h2, "秒数变化必须改变帧指纹");
        // 同一秒内 → 指纹稳定（跳帧省渲染的前提）
        let h3 = StripRenderer.frame_hash(&Snapshot::placeholder(0), &cfg, 400, 36, 90);
        assert_eq!(h2, h3, "同一秒内指纹必须稳定");
    }

    #[test]
    fn util_row_states_and_colors() {
        // Util 行三态：正常值 / 降级（部分源缺失）/ 不可用（全源缺失）。
        let cfg = demo_cfg();
        assert!(cfg.show_util, "Util 默认显示");

        // 不可用（启动首帧）：显示红色 "--"，行仍占位。
        let rows = build_rows(&Snapshot::placeholder(0), &cfg);
        let u = rows
            .as_slice()
            .iter()
            .find(|r| matches!(r.metric, MetricEnum::Util))
            .expect("show_util=true 时必须有 Util 行");
        assert_eq!(u.label.as_str(), "Util");
        assert_eq!(u.value.as_str(), "--");
        assert_eq!(u.value_color, Style::default().accent(100.0), "不可用必须红色");

        // 正常值：占用率强调色（47% → 绿档）。
        let mut snap = Snapshot::placeholder(0);
        snap.util = SnapMetric::Value(47.0);
        snap.util_partial = false;
        let rows = build_rows(&snap, &cfg);
        let u = rows.as_slice().iter().find(|r| matches!(r.metric, MetricEnum::Util)).unwrap();
        assert_eq!(u.value.as_str(), "47%");
        assert_eq!(u.value_color, Style::default().accent(47.0));

        // 降级（GPU 缺失重归一化）：陈旧色。
        let mut snap = Snapshot::placeholder(0);
        snap.util = SnapMetric::Value(61.0);
        snap.util_partial = true;
        let rows = build_rows(&snap, &cfg);
        let u = rows.as_slice().iter().find(|r| matches!(r.metric, MetricEnum::Util)).unwrap();
        assert_eq!(u.value_color, Style::default().stale_accent(61.0), "降级值必须用陈旧色");

        // 关闭 → 行消失。
        let mut cfg2 = cfg;
        cfg2.show_util = false;
        let rows = build_rows(&Snapshot::placeholder(0), &cfg2);
        assert!(
            !rows.as_slice().iter().any(|r| matches!(r.metric, MetricEnum::Util)),
            "show_util=false 时不得出现 Util 行"
        );
    }

    #[test]
    fn util_short_label_and_slot() {
        // 简称 U；数值槽与百分比一致（"100%" 4 字符）。
        let mut cfg = demo_cfg();
        cfg.short_names = true;
        let mut snap = Snapshot::placeholder(0);
        snap.util = SnapMetric::Value(100.0);
        let rows = build_rows(&snap, &cfg);
        let u = rows.as_slice().iter().find(|r| matches!(r.metric, MetricEnum::Util)).unwrap();
        assert_eq!(u.label.as_str(), "U");
        assert_eq!(u.value.as_str(), "100%");
        assert_eq!(u.slot, SLOT_PCT);
    }

    /// `layout` 预算门禁（恢复计划 P4.3，照 build_pages 门禁模式）：
    /// layout 是渲染线程**每帧**路径（P0 实测 seg_layout p50=6µs / max=16µs），
    /// 预算锁回归——有人往 build_rows / cell_sig 循环里塞分配/重活立刻可见，
    /// 并在 release 下打印真实耗时供决策参考。
    #[test]
    fn overlay_layout_is_within_budget() {
        let snap = demo_snap();
        let cfg = demo_cfg();
        // 预热（首跑含查表/分配冷路径，不代表稳态）。
        let _ = StripRenderer.layout(&snap, &cfg, 400, 36);
        let _ = VerticalRenderer.layout(&snap, &cfg, 400, 400);

        // 两个渲染器各测 50 次取最小值（排除调度抖动）。
        let mut best_strip = f64::MAX;
        let mut best_vertical = f64::MAX;
        for _ in 0..50 {
            let t = std::time::Instant::now();
            let _ = StripRenderer.layout(&snap, &cfg, 400, 36);
            best_strip = best_strip.min(t.elapsed().as_secs_f64() * 1e6);

            let t = std::time::Instant::now();
            let _ = VerticalRenderer.layout(&snap, &cfg, 400, 400);
            best_vertical = best_vertical.min(t.elapsed().as_secs_f64() * 1e6);
        }
        println!(
            "overlay layout strip={best_strip:.0}µs vertical={best_vertical:.0}µs (release 下供参考)"
        );
        // 预算先实测再定值：release 实测 ~6µs（P0 seg_layout）。500µs ≈ 80× 余量，
        // debug 测试构建也远够——锁「往每帧路径加重活」的回归即可。
        assert!(best_strip < 500.0, "横版布局超预算：{best_strip:.0}µs");
        assert!(best_vertical < 500.0, "竖版布局超预算：{best_vertical:.0}µs");
    }
}
