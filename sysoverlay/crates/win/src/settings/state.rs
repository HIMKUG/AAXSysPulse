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
#[derive(Default)]
pub enum Interaction {
    #[default]
    Idle,
    /// 悬停中的控件（widgets 下标）。
    Hover { widget: usize },
    /// 已按下但未抬起（按压反馈用；抬起时若命中同一控件则触发动作）。
    Pressing { widget: usize },
    /// 正在拖动的滑杆（绝对位置）。
    Dragging { widget: usize, origin: POINT },
    /// 滑杆值框编辑态。
    Editing { widget: usize },
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

/// 每帧派生的控件状态（从 Config 同步 + Interaction 覆盖）。
#[derive(Default, Clone, Debug)]
pub struct WidgetState {
    /// 泛化数值（Toggle 0/1、Slider 当前值、MetricSwatches 的 follow 标志）。
    pub value_f: f64,
    /// 选择下标（Segmented/Swatches/MetricSwatches）。
    pub selected: usize,
    /// 分段是否存在选中项（显示位置未贴角时为 false → 不画选中胶囊）。
    pub has_sel: bool,
    /// 键盘焦点（仅 keyboard_mode 下绘制焦点环）。
    pub focused: bool,
    pub hovered: bool,
    pub editing: bool,
    /// 悬停过渡进度 0..1（弹簧驱动）。
    pub hover_t: f32,
    /// 按压过渡进度 0..1（弹簧驱动）。
    pub press_t: f32,
    /// 过渡弹簧的当前值：Toggle = 0..1、Segment/Swatch = 选中下标、Slider = 数值。
    /// 由 `paint_page` 从逐控件弹簧写入；绘制侧据此做「滑动 / 渐变」而非硬切换。
    pub anim_v: f32,
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
    /// 展开/收起某指标的配置卡。
    ToggleMetricExpand(usize),
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
    /// 分组分隔线（画在本行顶缘；折叠卡分节用）。
    pub divider: bool,
    /// 辅助说明行（hint：比正文小一号、次级色）。
    pub hint: bool,
    /// 行内说明（画在标签下方的小字；有值时行高 = ROW_H_HINT）。
    pub hint_text: Option<&'static str>,
    /// 行内说明第二行（主句 + 弱化补充句，报告窗口页）。
    pub hint2: Option<&'static str>,
    /// 行顶相对「卡内行区顶部」的物理偏移（行高可变，由布局写入）。
    pub y_off: i32,
    /// 本行**控件带**高度（物理像素，由布局写入）= 行高 − hint 行数 × HINT_EXTRA。
    ///
    /// 行内 hint 必须从控件带**之后**起画，两端都不能偏：
    ///   - 按固定 `ROW_H`(28) 起画 → 滑杆行（控件带 46）的 12/72 端点标注被压住；
    ///   - 按整行高起画 → 溢出到下一行，压住下一行的标签（整页错位）。
    pub base_h: i32,
}

#[derive(Debug, Default)]
pub struct Card {
    pub caption: &'static str,
    pub rect: RECT,
    pub rows: Vec<Row>,
    /// 行区总高（物理像素；脚注定位用，行高可变后不再能按行数 × ROW_H 推算）。
    pub rows_h: i32,
    pub footnote: Option<&'static str>,
}

#[derive(Default)]
pub struct Page {
    /// 页标题（内容区顶部大标题；报告 §11 每页 h5）。
    pub title: &'static str,
    /// 页副标题（标题下方的次级说明行）。
    pub subtitle: &'static str,
    pub cards: Vec<Card>,
    pub widgets: Vec<Widget>,
    /// 内容区高度（DPI 缩放后，物理像素）。
    pub h: i32,
    /// 内容滚动偏移（DPI 缩放后，物理像素）——负值向上滚动。
    pub scroll_y: i32,
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
    /// 过渡弹簧的目标值：Toggle = 0/1；Segment/Swatch = 选中下标；
    /// Slider = 数值；纯按钮 / 箭头 = 0。用于「切换即滑动」的非线性过渡。
    fn anim_target(&self, state: &WidgetState) -> f32 {
        state.value_f as f32
    }
    fn as_any(&self) -> &dyn Any;
}

/// 默认实现：读 [`CfgRef`] 并按值种类填 `value_f` / `selected`。
pub fn sync_via_cfg(r: CfgRef, cfg: &Config, state: &mut WidgetState) {
    match r.get(cfg) {
        Value::Bool(b) => state.value_f = b as i32 as f64,
        Value::Num(x) => state.value_f = x,
        Value::Select(i) => {
            state.selected = i;
            state.has_sel = true;
        }
        Value::SelectOpt(o) => {
            state.selected = o.unwrap_or(0);
            state.has_sel = o.is_some();
        }
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
                let idx = r * self.cols as usize + c;
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
