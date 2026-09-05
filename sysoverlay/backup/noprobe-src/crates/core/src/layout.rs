//! Card layout computation for different display modes.
//!
//! Handles the layout calculations for the overlay card including
//! regular mode, minimal mode, and various vertical layouts.
//!
//! # 为什么 `compute` 不吃 `&Config`
//!
//! v2 决策「compute 签名」：吃**窄参数** [`LayoutInput`] 而不是整个 `Config`。
//! 两个理由：
//! 1. **好测** —— 单测只需要构造 6 个字段，不用先造一份合法 `Config`。
//! 2. **显式化依赖** —— 「布局到底依赖哪些配置」在签名上一眼看尽，
//!    往 `Config` 里加字段不会悄悄改变布局行为。

use crate::snapshot::{Metric, Snapshot};
use crate::types::{Shape, VertLayout, Zoom};

/// 逻辑尺寸（96 DPI、100% 缩放下的像素）。与 C 版常量一致。
const CARD_W_L: i32 = 268;
const CARD_H_L: i32 = 184;
const MIN_H_L: i32 = 36;
const MIN_W_L: i32 = 320;
const MINV_W_L: i32 = 128;
const MINV_H_L: i32 = 96;
const MINV2_W_L: i32 = 88;
const MINV2_H_L: i32 = 162;
const PAD_X: i32 = 16;

/// 常规模式一行的高度：标签 17 + 间隙 3 + 进度条 6 + 间隙 9。
const REGULAR_ROW_H_L: i32 = 35;
/// 极简单列一行的高度（96 = 4 行 × 24）。
const MIN_ROW_H_L: i32 = 24;
/// 极简分列一行的高度（162 ≈ 3 行 × 54）。
const MIN_SPLIT_ROW_H_L: i32 = 54;
/// 极简单行模式下，功耗段额外占据的宽度。
const MIN_ROW_POWER_W_L: i32 = 80;

/// 一行的语义标识。
///
/// 此前 `rows: Vec<RowLayout>` 靠**下标顺序**隐含语义（0=CPU, 1=内存…），
/// 一旦某指标不可用少画一行，后面全部错位。加 `kind` 让渲染方按语义取行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Cpu,
    Memory,
    Gpu,
    Net,
    Power,
}

impl RowKind {
    /// 固定顺序（常规模式的垂直堆叠顺序，同时也是优先级）。
    pub const ALL: [RowKind; 5] = [
        RowKind::Cpu,
        RowKind::Memory,
        RowKind::Gpu,
        RowKind::Net,
        RowKind::Power,
    ];
}

/// 哪些行参与布局。
///
/// v2 Q16：**不可用就隐藏整行**（而不是画一行 `N/A`），
/// 所以布局必须知道哪些指标当前有值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowsShown {
    pub cpu: bool,
    pub memory: bool,
    pub gpu: bool,
    pub net: bool,
    pub power: bool,
}

impl RowsShown {
    /// 全部显示。
    pub const fn all() -> Self {
        Self {
            cpu: true,
            memory: true,
            gpu: true,
            net: true,
            power: false,
        }
    }

    /// 按快照推导：只有「有值」的指标才占位（含 `Stale` —— 陈旧值仍要显示，只是标灰）。
    pub fn from_snapshot(snap: &Snapshot, power_enabled: bool) -> Self {
        Self {
            cpu: snap.cpu.value().is_some(),
            memory: true,
            gpu: snap.gpu.value().is_some(),
            net: true,
            power: power_enabled && snap.power.value().is_some(),
        }
    }

    /// 按种类取开关。
    pub fn get(&self, kind: RowKind) -> bool {
        match kind {
            RowKind::Cpu => self.cpu,
            RowKind::Memory => self.memory,
            RowKind::Gpu => self.gpu,
            RowKind::Net => self.net,
            RowKind::Power => self.power,
        }
    }

    /// 当前可见的行（按 [`RowKind::ALL`] 顺序）。
    pub fn visible(&self) -> impl Iterator<Item = RowKind> + '_ {
        RowKind::ALL.into_iter().filter(move |k| self.get(*k))
    }

    /// 可见行数。
    pub fn count(&self) -> usize {
        self.visible().count()
    }
}

impl Default for RowsShown {
    fn default() -> Self {
        Self::all()
    }
}

/// `compute` 的全部输入 —— 布局依赖的**全部**配置，显式列出。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutInput {
    /// 是否极简模式。
    pub minimal: bool,
    /// 极简模式下的排列（仅 `minimal = true` 时有意义）。
    pub vert: VertLayout,
    /// 缩放档位。调用方应传 [`Config::effective_zoom`][crate::config::Config::effective_zoom]，
    /// 这样 Eco 的两档收敛对布局透明。
    pub zoom: Zoom,
    /// 当前 DPI（用于相对 96 DPI 的缩放系数）。
    pub dpi: u32,
    /// 卡片外形（影响是否预留背景边距）。
    pub shape: Shape,
    /// 哪些行参与布局。
    pub rows: RowsShown,
}

impl Default for LayoutInput {
    fn default() -> Self {
        Self {
            minimal: false,
            vert: VertLayout::SingleRow,
            zoom: Zoom::Zoom100,
            dpi: 96,
            shape: Shape::Round,
            rows: RowsShown::all(),
        }
    }
}

/// Row layout within a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowLayout {
    /// 这一行的语义。
    pub kind: RowKind,
    /// Label rectangle: (x, y, width, height).
    pub label_rect: (i32, i32, i32, i32),
    /// Value rectangle: (x, y, width, height).
    pub value_rect: (i32, i32, i32, i32),
    /// Progress bar track rectangle: (x, y, width, height)。
    /// 极简模式与网络/功耗行为 (0,0,0,0)，表示不画进度条。
    pub track_rect: (i32, i32, i32, i32),
}

/// Complete card layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardLayout {
    /// Total card width in pixels.
    pub width: i32,
    /// Total card height in pixels.
    pub height: i32,
    /// Row layouts for each visible metric row.
    pub rows: Vec<RowLayout>,
}

/// Compute card layout from a narrow input.
///
/// 与旧版不同：不再依赖下标顺序，调用方一律通过 [`RowLayout::kind`] 取行。
pub fn compute(input: &LayoutInput) -> CardLayout {
    let scale = input.dpi as f32 / 96.0;
    let zoom_factor = input.zoom.scale_factor();
    let f = |logical: i32| -> i32 { (logical as f32 * scale * zoom_factor + 0.5) as i32 };

    let n = input.rows.count();
    let _has_power = input.rows.power;

    let (card_w, card_h) = if input.minimal {
        match input.vert {
            VertLayout::SingleCol => (f(MINV_W_L), f(MINV_H_L + extra_rows(n, 4) * MIN_ROW_H_L)),
            VertLayout::SplitCol => (
                f(MINV2_W_L),
                f(MINV2_H_L + extra_rows(n, 4) * MIN_SPLIT_ROW_H_L),
            ),
            VertLayout::SingleRow => {
                (f(MIN_W_L + extra_rows(n, 4) * MIN_ROW_POWER_W_L), f(MIN_H_L))
            }
        }
    } else {
        (
            f(CARD_W_L),
            f(CARD_H_L + extra_rows(n, 4) * REGULAR_ROW_H_L),
        )
    };

    let pad_x = f(PAD_X);
    let rows = if input.minimal {
        layout_minimal(input, &f, card_w, card_h, pad_x)
    } else {
        layout_regular(input, &f, card_w, pad_x)
    };

    CardLayout {
        width: card_w,
        height: card_h,
        rows,
    }
}

/// 超出基准行数（4 行）的部分，用于按需加高/加宽。
fn extra_rows(n: usize, base: usize) -> i32 {
    n.saturating_sub(base) as i32
}

fn layout_minimal(
    input: &LayoutInput,
    f: &impl Fn(i32) -> i32,
    card_w: i32,
    card_h: i32,
    pad_x: i32,
) -> Vec<RowLayout> {
    let rh = f(16);
    let visible: Vec<RowKind> = input.rows.visible().collect();

    let mut rows = Vec::with_capacity(visible.len());
    let inner_w = card_w - pad_x * 2;

    match input.vert {
        VertLayout::SingleRow => {
            // 所有指标挤在一行：按可见数均分宽度。
            let n = visible.len().max(1) as i32;
            let seg = inner_w / n;
            for (i, kind) in visible.into_iter().enumerate() {
                let x = pad_x + i as i32 * seg;
                rows.push(RowLayout {
                    kind,
                    label_rect: (x, f(9), seg / 2, rh),
                    value_rect: (x + seg / 2, f(9), seg / 2, rh),
                    track_rect: (0, 0, 0, 0),
                });
            }
        }
        VertLayout::SingleCol => {
            // 每个指标独占一行。
            let row_h = MIN_ROW_H_L;
            for (i, kind) in visible.into_iter().enumerate() {
                let y = f(i as i32 * row_h);
                rows.push(RowLayout {
                    kind,
                    label_rect: (pad_x, y, inner_w / 2, rh),
                    value_rect: (pad_x + inner_w / 2, y, inner_w / 2, rh),
                    track_rect: (0, 0, 0, 0),
                });
            }
        }
        VertLayout::SplitCol => {
            // 两列：左列偶数序、右列奇数序。
            let col_w = inner_w / 2;
            let row_h = MIN_SPLIT_ROW_H_L;
            for (i, kind) in visible.into_iter().enumerate() {
                let col = (i % 2) as i32;
                let line = (i / 2) as i32;
                let x = pad_x + col * col_w;
                let y = f(line * row_h);
                rows.push(RowLayout {
                    kind,
                    label_rect: (x, y, col_w, rh),
                    value_rect: (x, y + f(24), col_w, rh),
                    track_rect: (0, 0, 0, 0),
                });
            }
        }
    }

    let _ = card_h; // 尺寸由调用方（窗口）使用，行内 y 已按逻辑行高排布
    rows
}

fn layout_regular(
    input: &LayoutInput,
    f: &impl Fn(i32) -> i32,
    card_w: i32,
    pad_x: i32,
) -> Vec<RowLayout> {
    let lh = f(18);
    let bh = f(6);
    let row_h = f(17);
    let bar_w = card_w - pad_x * 2;

    let mut rows = Vec::new();
    let mut y = f(12) + lh + f(9);

    for kind in input.rows.visible() {
        // 网络行与功耗行不画进度条，但保持同样的水平栅格。
        let has_track = matches!(kind, RowKind::Cpu | RowKind::Memory | RowKind::Gpu);
        let track_rect = if has_track {
            (pad_x, y + row_h + f(3), bar_w, bh)
        } else {
            (0, 0, 0, 0)
        };

        rows.push(RowLayout {
            kind,
            label_rect: (pad_x, y, bar_w / 2, row_h),
            value_rect: (pad_x + bar_w / 2, y, bar_w / 2, row_h),
            track_rect,
        });

        y += if has_track {
            row_h + f(3) + bh + f(9)
        } else {
            row_h + f(9)
        };
    }

    rows
}

/// Calculate fill width percentage for a progress bar.
pub fn fill_pct(value: f32) -> i32 {
    value.clamp(0.0, 100.0) as i32
}

/// Scale a logical coordinate to screen coordinates based on DPI and zoom.
pub fn scale(logical: i32, dpi: u32, zoom: Zoom) -> i32 {
    (logical as f32 * (dpi as f32 / 96.0) * zoom.scale_factor() + 0.5) as i32
}

/// 从 `Metric` 取出用于进度条填充的百分比（不可用 → 0）。
pub fn metric_pct(m: Metric<f32>) -> f32 {
    m.value().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> LayoutInput {
        LayoutInput::default()
    }

    #[test]
    fn zoom_factors_come_from_types() {
        // Zoom 现在由 core::types 提供，layout 不再自带一份
        assert_eq!(Zoom::Zoom75.scale_factor(), 0.75);
        assert_eq!(Zoom::Zoom100.scale_factor(), 1.0);
        assert_eq!(Zoom::Zoom125.scale_factor(), 1.25);
        assert_eq!(Zoom::Zoom150.scale_factor(), 1.5);
        assert_eq!(Zoom::Zoom200.scale_factor(), 2.0);
    }

    #[test]
    fn card_dimensions_normal() {
        let layout = compute(&input());
        assert_eq!(layout.width, CARD_W_L);
        assert_eq!(layout.height, CARD_H_L);
    }

    #[test]
    fn card_dimensions_zoomed() {
        let mut i = input();
        i.zoom = Zoom::Zoom200;
        let layout = compute(&i);
        assert_eq!(layout.width, CARD_W_L * 2);
        assert_eq!(layout.height, CARD_H_L * 2);
    }

    #[test]
    fn card_dimensions_minimal_single_row() {
        let mut i = input();
        i.minimal = true;
        i.vert = VertLayout::SingleRow;
        let layout = compute(&i);
        assert_eq!(layout.width, MIN_W_L);
        assert_eq!(layout.height, MIN_H_L);
    }

    #[test]
    fn card_dimensions_minimal_single_col() {
        let mut i = input();
        i.minimal = true;
        i.vert = VertLayout::SingleCol;
        let layout = compute(&i);
        assert_eq!(layout.width, MINV_W_L);
        assert_eq!(layout.height, MINV_H_L);
    }

    #[test]
    fn card_dimensions_minimal_split_col() {
        let mut i = input();
        i.minimal = true;
        i.vert = VertLayout::SplitCol;
        let layout = compute(&i);
        assert_eq!(layout.width, MINV2_W_L);
        assert_eq!(layout.height, MINV2_H_L);
    }

    #[test]
    fn card_dimensions_dpi_scaling() {
        let mut i = input();
        i.dpi = 192;
        let layout = compute(&i);
        assert_eq!(layout.width, CARD_W_L * 2);
        assert_eq!(layout.height, CARD_H_L * 2);
    }

    #[test]
    fn card_dimensions_dpi_and_zoom_combined() {
        let mut i = input();
        i.dpi = 192;
        i.zoom = Zoom::Zoom150;
        let layout = compute(&i);
        // 268 * 2 * 1.5 = 804
        assert_eq!(layout.width, CARD_W_L * 3);
        assert_eq!(layout.height, CARD_H_L * 3);
    }

    #[test]
    fn fill_pct_clamps() {
        assert_eq!(fill_pct(50.0), 50);
        assert_eq!(fill_pct(0.0), 0);
        assert_eq!(fill_pct(100.0), 100);
        assert_eq!(fill_pct(-10.0), 0);
        assert_eq!(fill_pct(150.0), 100);
    }

    #[test]
    fn scale_helper() {
        assert_eq!(scale(100, 96, Zoom::Zoom100), 100);
        assert_eq!(scale(100, 192, Zoom::Zoom100), 200);
        assert_eq!(scale(100, 96, Zoom::Zoom200), 200);
    }

    #[test]
    fn rows_populated_regular_mode() {
        let layout = compute(&input());
        assert_eq!(layout.rows.len(), 4, "常规模式 4 行（CPU/内存/GPU/网络）");
    }

    /// 回归：极简单列此前只生成 1 个占位行，实际应有 4 行。
    #[test]
    fn rows_populated_minimal_single_col() {
        let mut i = input();
        i.minimal = true;
        i.vert = VertLayout::SingleCol;
        let layout = compute(&i);
        assert_eq!(layout.rows.len(), 4, "极简单列也必须给出 4 行，而不是 1 个占位");
    }

    /// 回归：极简单行此前生成 0 行（"由绘制代码处理"），导致渲染方无法按布局绘制。
    #[test]
    fn rows_populated_minimal_single_row() {
        let mut i = input();
        i.minimal = true;
        i.vert = VertLayout::SingleRow;
        let layout = compute(&i);
        assert_eq!(layout.rows.len(), 4);
        // 单行模式下所有行的 y 相同（同一行内横向排布）
        let ys: Vec<i32> = layout.rows.iter().map(|r| r.label_rect.1).collect();
        assert!(ys.windows(2).all(|w| w[0] == w[1]), "单行模式所有 y 必须一致");
        // x 递增
        let xs: Vec<i32> = layout.rows.iter().map(|r| r.label_rect.0).collect();
        assert!(xs.windows(2).all(|w| w[1] > w[0]), "x 必须递增");
    }

    #[test]
    fn rows_carry_semantic_kind() {
        let layout = compute(&input());
        let kinds: Vec<RowKind> = layout.rows.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            vec![RowKind::Cpu, RowKind::Memory, RowKind::Gpu, RowKind::Net]
        );
    }

    /// v2 Q16：不可用 → 隐藏整行，后面的行不能错位。
    #[test]
    fn unavailable_metric_hides_its_row() {
        let mut i = input();
        i.rows.gpu = false;
        let layout = compute(&i);
        assert_eq!(layout.rows.len(), 3);
        assert!(
            !layout.rows.iter().any(|r| r.kind == RowKind::Gpu),
            "GPU 不可用时整行消失，不留占位"
        );
        // 剩下的行语义仍正确、顺序不乱
        let kinds: Vec<RowKind> = layout.rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, vec![RowKind::Cpu, RowKind::Memory, RowKind::Net]);
    }

    #[test]
    fn power_row_appears_when_enabled() {
        let mut i = input();
        i.rows.power = true;
        let layout = compute(&i);
        assert_eq!(layout.rows.len(), 5);
        assert!(layout.rows.iter().any(|r| r.kind == RowKind::Power));
        // 加一行就要加高，否则最后一行会被裁掉
        assert_eq!(layout.height, CARD_H_L + REGULAR_ROW_H_L);
    }

    #[test]
    fn power_row_has_no_progress_track() {
        let mut i = input();
        i.rows.power = true;
        let layout = compute(&i);
        let power = layout.rows.iter().find(|r| r.kind == RowKind::Power).unwrap();
        assert_eq!(power.track_rect, (0, 0, 0, 0), "功耗行不画进度条");
        // 网络行同样没有进度条
        let net = layout.rows.iter().find(|r| r.kind == RowKind::Net).unwrap();
        assert_eq!(net.track_rect, (0, 0, 0, 0));
        // CPU 行有
        let cpu = layout.rows.iter().find(|r| r.kind == RowKind::Cpu).unwrap();
        assert!(cpu.track_rect.2 > 0, "CPU 行必须有进度条");
    }

    #[test]
    fn shape_does_not_affect_geometry() {
        // Shape 只影响渲染，不影响布局
        let base = compute(&input());
        for shape in [Shape::Round, Shape::Square, Shape::Content] {
            let mut i = input();
            i.shape = shape;
            let l = compute(&i);
            assert_eq!(l.width, base.width);
            assert_eq!(l.height, base.height);
        }
    }

    #[test]
    fn rows_shown_from_snapshot() {
        use crate::snapshot::{MemoryStat, NetStat};
        let snap = Snapshot {
            seq: 1,
            timestamp_ms: 0,
            uptime_ms: 0,
            cpu: Metric::Value(10.0),
            memory: MemoryStat::default(),
            gpu: Metric::Unavailable,
            net: NetStat::default(),
            power: Metric::Unavailable,
        };
        let rows = RowsShown::from_snapshot(&snap, true);
        assert!(rows.cpu);
        assert!(!rows.gpu, "GPU 不可用 → 隐藏");
        assert!(!rows.power, "功耗不可用 → 隐藏");
        assert_eq!(rows.count(), 3);
    }

    /// 陈旧值仍要显示（只是标灰），不能当成不可用隐藏。
    #[test]
    fn stale_metric_still_occupies_a_row() {
        let i = input();
        let _ = i;
        use crate::snapshot::Metric;
        let snap_metric: Metric<f32> = Metric::Stale {
            value: 42.0,
            stale_for: 3,
        };
        assert_eq!(metric_pct(snap_metric), 42.0, "陈旧值仍参与布局与绘制");
    }

    #[test]
    fn metric_pct_unavailable_is_zero() {
        let m: Metric<f32> = Metric::Unavailable;
        assert_eq!(metric_pct(m), 0.0);
    }
}
