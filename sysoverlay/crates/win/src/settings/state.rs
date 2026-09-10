//! 设置页纯数据层：控件树 / 空间网格 / 交互状态机 / Action。
//!
//! 本模块**零 Win32 资源**——只有数据与纯逻辑，可随时整体替换或快照。
//! 生命周期规则：`Widget.rect` 存**内容坐标**（相对内容区顶部），
//! 命中测试一律先经 `ClientPt → ContentPt` 换算（见 `mod.rs` 的 `to_content`）。

use windows::Win32::Foundation::{POINT, RECT};

use super::cfg_ref::CfgRef;

/// 控件 ID 类型（诊断/命中区分用；语义由 [`CfgRef`] 承担）。
pub type WidgetId = i32;

/// 统一交互状态机（替代旧 drag/edit/hover 三职独立字段）。
#[derive(Debug, Clone, PartialEq)]
pub enum Interaction {
    Idle,
    /// 悬停中的控件（widgets 下标）。
    Hover { widget: usize },
    /// 正在拖动的滑杆。
    Dragging { widget: usize, origin: POINT },
    /// 滑杆值框编辑态。
    Editing { widget: usize },
}

impl Default for Interaction {
    fn default() -> Self {
        Interaction::Idle
    }
}

/// 输入框编辑态。
#[derive(Clone)]
pub struct EditState {
    pub idx: usize,
    pub buf: Vec<u16>,
    pub caret: bool,
    /// 编辑前的值（提交/取消失败时回退用）。
    pub orig: f64,
    /// 首次键入即全选替换（人类直觉：点进输入框直接打字覆盖旧值）。
    pub select_all: bool,
}

/// 摇杆（弹簧杆）物理状态：杆永远居中，拖拽只产生增量。
///
/// - `deflect`：杆相对中心的位移（[-1,1]，1 = 触及轨道右端），**不代表绝对值**。
/// - `vel`：杆速度（弹簧-阻尼积分用）。
/// - `target`：目标位移（拖动中 = 手指位置；松手后 = 0 → 回中）。
/// - `releasing`：已松开、正弹簧回中阶段（此阶段只动视觉，不再累加数值）。
/// - `pos`：数值累计推量（单位）——加速度模型的"位移"。
/// - `speed`：当前推进速率（单位/秒），由加速度积累并钳制到 ±10/s。
///
/// 数值推进 = **加速度模型**（用户规格："类似于加速度的公式，上限为 +10/s"）：
/// 每 tick `speed += deflect·ACCEL·dt`（推得越远加速度越大）、`speed` 钳到
/// ±[`FX_MAX_RATE`]、`pos += speed·dt`；数值 = base + pos。轻推慢加速、快推
/// 逼近 10 单位/秒的上限，符合"物理"直觉；旧 integral 模型对所有偏移等速推进，
/// 偏快且不具"加速感"。
#[derive(Clone, Copy, Debug)]
pub struct SliderFx {
    pub widget: usize,
    pub deflect: f32,
    pub vel: f32,
    pub target: f32,
    pub releasing: bool,
    pub pos: f32,
    pub speed: f32,
    /// true = 摇杆（杆居中，增量式，速度上限 10/s）。
    /// false = 简单滑杆（杆位即值，速度上限 5/s）。
    pub joystick: bool,
}

/// 每帧派生的控件状态（从 Config 同步 + Interaction 覆盖）。
#[derive(Default, Clone, Debug)]
pub struct WidgetState {
    /// 泛化数值（Toggle 0/1、Slider 当前值、MetricSwatches 的 follow 标志）。
    pub value_f: f64,
    /// 选择下标（Segmented/Swatches/MetricSwatches）。
    pub selected: usize,
    pub hovered: bool,
    pub editing: bool,
}

/// 单向数据流的动作枚举：交互层只产出 Action，写 Config 只在 dispatch。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// 翻页。
    SetPage(usize),
    /// 切换开关。
    Toggle(CfgRef),
    /// 分段选择。
    Select(CfgRef, usize),
    /// 滑杆拖动中的实时值（不落盘，预览用）。
    LiveValue(CfgRef, f64),
    /// 滑杆提交（松手 / 输入框回车）。
    CommitValue(CfgRef, f64),
}

// ── 控件树 ───────────────────────────────────────────────

pub struct Widget {
    pub id: WidgetId,
    pub kind: Box<dyn IWidget>,
    pub rect: RECT,
}

#[derive(Debug)]
pub struct Row {
    pub label: &'static str,
    /// 该行从左到右的 widget 下标。
    pub widgets: Vec<usize>,
}

#[derive(Debug, Default)]
pub struct Card {
    pub caption: &'static str,
    pub rect: RECT,
    pub rows: Vec<Row>,
    pub footnote: Option<&'static str>,
}

#[derive(Default)]
pub struct Page {
    pub cards: Vec<Card>,
    pub widgets: Vec<Widget>,
    /// 内容区高度（DPI 缩放后，物理像素）。
    pub h: i32,
}

// ── 控件 trait ───────────────────────────────────────────

use std::any::Any;

use super::cfg_ref::Value;
use sysoverlay_core::config::Config;

/// 统一控件 trait：每个控件**持有自己的 [`CfgRef`]**，
/// sync 只有一行 `cfg.get(...)`，不再有散落的巨型 match。
pub trait IWidget: Any + Send {
    /// 控件绑定的 Config 引用（纯按钮类返回 None）。
    fn cfg(&self) -> Option<CfgRef>;
    /// 从 Config 同步当前值到 state。
    fn sync(&self, cfg: &Config, state: &mut WidgetState);
    fn as_any(&self) -> &dyn Any;
}

/// 默认实现：读 [`CfgRef`] 并按值种类填 `value_f` / `selected`。
pub fn sync_via_cfg(r: CfgRef, cfg: &Config, state: &mut WidgetState) {
    match r.get(cfg) {
        Value::Bool(b) => state.value_f = b as i32 as f64,
        Value::Num(x) => state.value_f = x,
        Value::Select(i) => state.selected = i,
        Value::Color(_) => {}
    }
}

// ── 空间网格（O(1) 平均命中） ─────────────────────────────

/// 把当前页分割成 cell×cell 网格，每格存控件下标。
pub struct SpatialGrid {
    pub cell_size: i32,
    pub cols: i32,
    pub rows: i32,
    cells: Vec<Vec<usize>>,
}

impl SpatialGrid {
    pub fn new(w: i32, h: i32, cell_size: i32) -> Self {
        let cols = (w / cell_size).max(1);
        let rows = (h / cell_size).max(1);
        let cells = vec![Vec::new(); (cols * rows) as usize];
        Self { cell_size, cols, rows, cells }
    }

    pub fn insert(&mut self, widget_idx: usize, rect: RECT) {
        let min_c = (rect.left / self.cell_size).max(0).min(self.cols - 1) as usize;
        let max_c = (rect.right / self.cell_size).max(0).min(self.cols - 1) as usize;
        let min_r = (rect.top / self.cell_size).max(0).min(self.rows - 1) as usize;
        let max_r = (rect.bottom / self.cell_size).max(0).min(self.rows - 1) as usize;
        for r in min_r..=max_r {
            for c in min_c..=max_c {
                let idx = (r * self.cols as usize + c) as usize;
                self.cells[idx].push(widget_idx);
            }
        }
    }

    /// 给定点的候选控件下标（可能含重复，调用方去重）。
    pub fn query(&self, pt: POINT) -> &[usize] {
        static EMPTY: Vec<usize> = Vec::new();
        if pt.x < 0 || pt.y < 0 {
            return &EMPTY;
        }
        let c = pt.x / self.cell_size;
        let r = pt.y / self.cell_size;
        if c < 0 || c >= self.cols || r < 0 || r >= self.rows {
            return &EMPTY;
        }
        &self.cells[(r * self.cols + c) as usize]
    }
}

/// 点是否在矩形内（半开区间）。
pub fn pt_in(r: &RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spatial_grid_hits_only_own_cell() {
        let mut g = SpatialGrid::new(300, 200, 30);
        g.insert(0, RECT { left: 0, top: 0, right: 50, bottom: 20 });
        g.insert(1, RECT { left: 200, top: 150, right: 280, bottom: 190 });
        assert_eq!(g.query(POINT { x: 10, y: 5 }), &[0]);
        assert_eq!(g.query(POINT { x: 250, y: 160 }), &[1]);
        assert!(g.query(POINT { x: 120, y: 80 }).is_empty());
        // 越界安全
        assert!(g.query(POINT { x: -5, y: 5 }).is_empty());
        assert!(g.query(POINT { x: 999, y: 999 }).is_empty());
    }

    #[test]
    fn spatial_grid_widget_spanning_cells_found_in_all() {
        let mut g = SpatialGrid::new(300, 300, 30);
        g.insert(7, RECT { left: 0, top: 0, right: 120, bottom: 40 });
        assert!(g.query(POINT { x: 5, y: 5 }).contains(&7));
        assert!(g.query(POINT { x: 100, y: 30 }).contains(&7));
    }

    #[test]
    fn pt_in_half_open() {
        let r = RECT { left: 10, top: 10, right: 20, bottom: 20 };
        assert!(pt_in(&r, POINT { x: 10, y: 10 }));
        assert!(!pt_in(&r, POINT { x: 20, y: 15 }), "右/下边界开区间");
        assert!(!pt_in(&r, POINT { x: 9, y: 15 }));
    }
}
