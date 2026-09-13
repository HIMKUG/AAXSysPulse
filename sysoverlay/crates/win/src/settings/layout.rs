//! 布局：常量、DPI 缩放与**纯函数**页面构建（v5 设置页规范重写）。
//!
//! # 结构（报告 §11 目标形态）
//! 每页 = 页标题 + 副标题 + 若干**分组卡**。
//!
//! 行高 = **控件带** + 说明带：控件带 34（折叠头 52 / 滑杆 48），
//! 说明带 = 22 × hint 行数。行与行之间另有统一的 `ROW_GAP`（12），
//! 分隔线画在它的正中。
//!
//! # 间距刻度
//! 全页只用一组 4px 基准的刻度（见下方常量注释）：4 / 8 / 12 / 16 / 24。
//! 相邻量级至少差 1.5 倍，视觉上才能读出「谁和谁是一组」——
//! 改任何间距都必须从这里取值，不要就地写数字。
//!
//! 有标题的分组卡内部画行间分隔线（`want_div`）；无标题分组 =
//! 监控项独立卡，不画内部分隔。
//!
//! # 折叠卡展开动画
//! `expand[i]` 是 0..1 的**动画展开度**（由 `mod.rs` 的弹簧每帧推进）：
//! 展开行在 `expand[i] > 0.001` 时即参与布局（卡片一次长到位），
//! 展开态标记（箭头朝向）按 `expand[i] > 0.5` 判定；
//! 卡内内容行按展开后的固定位置排布，由 `paint_page` 按卡片矩形**裁剪**露出。

use aaxsyspulse_core::config::Config;
use aaxsyspulse_core::types::Lang;

use super::cfg_ref::CfgRef;
use super::state::{Card, Page, Row, Widget};
use super::widget::{
    AboutHeaderWidget, ButtonAction, ButtonKind, ButtonWidget, DisclosureWidget,
    MetricFontStatusWidget, MetricHeaderWidget, MetricSwatchesWidget, SegmentedWidget, SliderWidget,
    StepperWidget, ToggleWidget,
};

// ── 布局常量（逻辑像素，按 DPI 缩放） ────────────────────

/// 客户区默认 / 最小 / 最大尺寸（报告 §2：WINDOW_W = 900）。
pub const DEFAULT_W: i32 = 900;
pub const DEFAULT_H: i32 = 620;
pub const MIN_W: i32 = 560;
pub const MIN_H: i32 = 460;
pub const MAX_W: i32 = 1400;
pub const MAX_H: i32 = 1000;

/// 侧边栏（固定宽，不随窗口拉伸；报告 §2：SIDEBAR_W = 190）。
pub const SIDEBAR_W: i32 = 190;
pub const SIDEBAR_PAD: i32 = 8;
pub const SB_ITEM_H: i32 = 34;
pub const SB_ITEM_GAP: i32 = 2;
pub const SB_ICON: i32 = 16;
pub const SB_ICON_TEXT_GAP: i32 = 10;

/// 内容区。
pub const CONTENT_PAD_X: i32 = 20;
pub const CONTENT_PAD_TOP: i32 = 20;
/// 内容区最大宽度（报告 B-04：内容区 620 DIP，超出不拉伸）。
pub const CONTENT_MAX_W: i32 = 620;
/// 卡片间距（用户反馈：加大 CPU/内存/GPU 各栏之间的间距）。
pub const CARD_GAP: i32 = 24;
/// 监控项独立卡之间的间距（每项一张卡，紧凑排列）。
pub const METRIC_CARD_GAP: i32 = 14;

/// 页标题 / 副标题（报告 §11 每页 h5 + p）。
pub const TITLE_H: i32 = 30;
pub const TITLE_SUB_GAP: i32 = 4;
pub const SUB_H: i32 = 16;
/// 副标题 → 第一张卡（或其分组标题）。
pub const SUB_CARD_GAP: i32 = 14;

// ── 间距刻度（4px 基准，全页统一取用） ───────────────────
//
// 设计意图（用户反馈「间距要大且一致」）：
//   4   标签 ↔ 它自己的说明（同属一行，必须紧）
//   8   分组标题 ↔ 卡片顶缘
//   12  行 ↔ 行（卡片内部；分隔线居中于此）
//   16  卡片四周内边距
//   24  卡片 ↔ 卡片
// 相邻量级至少差 1.5 倍，视觉上才能读出「谁和谁是一组」。

/// 卡片四周内边距（上下左右同值）。
pub const CARD_PAD: i32 = 16;
/// 分组标题带高度（≥ 标题字号 caption_px(14)，否则字会被下一条挤住）。
pub const CAPTION_H: i32 = 18;
/// 分组标题 → 卡片顶缘（小间距，标题属于下面的卡片）。
pub const CAPTION_GAP: i32 = 8;

/// 基础行高 = **控件带**高度（标签 + 控件垂直居中于其中）。
pub const ROW_H: i32 = 34;
/// 行内说明（hint）每行附加高度。
/// 20 > 说明字号 14（Fluent base300），行盒上下各留 ~3px。
/// S5 字阶 Fluent 化时由 22 调来：body(16)+caption(14)=30 后，
/// 需 ROW_H+HINT_EXTRA = 24+30 = 54 才能维持纵向节奏不变式
/// （label_pad + hint_pad = ROW_GAP，见 vertical_rhythm 测试）。
pub const HINT_EXTRA: i32 = 20;
/// 行与行之间的空隙（卡片内部）。分隔线画在空隙正中。
pub const ROW_GAP: i32 = 12;
/// 滑杆行控件带高度：控件线（居中于首 ROW_H）+ 下方 min/max 端点标注。
/// 48 = 17(控件线) + 11(数值框下半) + 2 + 17(端点标注)。
pub const ROW_SLIDER_H: i32 = 48;
/// 滑杆数值框高度（控件线上下各一半）。
pub const VALUEBOX_H: i32 = 22;
/// 折叠头行控件带高度（监控项卡首行）：比基础行高更松，给抓手/标题留呼吸。
pub const ROW_HEADER_H: i32 = 52;

/// 下列沿用：
pub const BOTTOM_MARGIN: i32 = 20;
pub const WIDGET_H: i32 = 24;
/// 设置页**全局圆角**（逻辑 px）。
///
/// 卡片 / 按钮 / 步进器 / 分段控件 / 滑杆数值框 / 色板 / 标题栏按钮统一取它；
/// 实际绘制时按各自几何钳位（半径不超过短边一半，见 `gfx_d2d::RectF::rounded`），
/// 因此胶囊、圆形控件（开关轨道、滚动条拇指、侧边栏胶囊）不受影响。
/// 生效点在 `gfx_d2d::Gfx::set_global_radius`（绘制前逐帧按 DPI 写入）。
pub const RADIUS: i32 = 30;
/// 卡片圆角。与全局圆角同源（卡片是最主要的圆角面）。
pub const CARD_RADIUS: i32 = RADIUS;
pub const LABEL_W: i32 = 120;
pub const TOGGLE_W: i32 = 40;
pub const VALUEBOX_W: i32 = 64;
pub const FOOTNOTE_H: i32 = 60;
/// 滚动条**可见**宽度（逻辑 px）。3px 时代用户根本抓不住，
/// 这里给到 12px —— 既符合 Windows「细滚动条」观感，也够手指/指针稳定命中。
pub const SCROLLBAR_W: i32 = 12;
/// 滚动条**命中**宽度（逻辑 px）。可见宽度之外再向内容侧外扩，
/// 形成一条隐形但可点的「抓取带」（Apple HIG 建议最小 20pt 命中目标）。
pub const SCROLLBAR_HIT_W: i32 = 20;
/// 悬停/拖拽时拇指扩张到的宽度（逻辑 px）：状态反馈 + 进一步降低抓取难度。
pub const SCROLLBAR_W_HOT: i32 = 16;
/// 滚动条距窗口右缘/上下缘的内缩（逻辑 px）。
pub const SCROLLBAR_INSET: i32 = 4;
/// 拇指最小高度（逻辑 px）：内容极长时拇指仍要有可抓的实体。
pub const SCROLLBAR_MIN_THUMB: i32 = 40;

/// **统一控件列**：分段 / 滑杆 / 色板等「大控件」右对齐到同一条右边缘（操作边缘），
/// 且左侧只在必要时向左扩（窗口变宽时控件不跟着无限拉长）。
pub const CTRL_W: i32 = 300;
/// 行尾圆形图标按钮边长（监控项折叠头的展开按钮）。
pub const ICON_BTN: i32 = 28;
/// 步进器「−」/「＋」按钮宽（总宽 = 28 + 40 + 28 = 96）。
pub const STEPPER_BTN_W: i32 = 28;
pub const STEPPER_W: i32 = 96;

/// 滑杆范围（用户要求拓宽：字号 4..128；间距沿用配置钳位 0..64）。
/// 字号区间直接引用 core 的落盘钳位常量 —— 两处必须同源，否则「滑杆能拉、
/// 重启被截断」。
pub const FONT_MIN: f32 = aaxsyspulse_core::config::FONT_PX_MIN;
pub const FONT_MAX: f32 = aaxsyspulse_core::config::FONT_PX_MAX;
pub const GAP_MIN: f32 = 0.0;
pub const GAP_MAX: f32 = 64.0;

/// 逐指标文字偏移范围（像素，可为负）。与 core 落盘钳位同源，
/// 否则「步进器能拨到、重启被截断」。
pub const OFF_MIN: f32 = aaxsyspulse_core::config::OFFSET_MIN as f32;
pub const OFF_MAX: f32 = aaxsyspulse_core::config::OFFSET_MAX as f32;

/// DPI 缩放（逻辑像素 → 物理像素）。
pub fn s(dpi: u32, v: i32) -> i32 {
    v * dpi as i32 / 96
}

/// 标签所在的**文字带**高度（物理像素）。
///
/// 正文标签始终按 `ROW_H` 居中 —— 滑杆行的控件线就取 `ROW_H/2`，标签必须与它同线；
/// 小字说明行（`base_h` = `HINT_EXTRA`）则按自身行高居中即可。
pub fn label_band(base_h: i32, dpi: u32) -> i32 {
    base_h.min(s(dpi, ROW_H))
}

// ── 字号（DIP；全设置页唯一来源，mod/widget 一律从这里取） ──

/// 正文字号 = Fluent `font_size_base400`（16 DIP）。
/// S5 决策记录：映射表原定 base300(14)，但纵向节奏不变式要求
/// body+caption = ROW_H+HINT_EXTRA−24（原 17.33+14.67=32），14+12 会破坏节奏
/// 且迫使 ROW_H 连动重排。故取最接近现有尺寸的 base400，"字号偏小"反馈史不背刺。
pub fn body_px(dpi: u32) -> f32 {
    super::fluent::tokens::FLUENT.light.font_size_base400 * dpi as f32 / 96.0
}

/// 说明 / 次级字号 = Fluent `font_size_base300`（14 DIP）。
pub fn caption_px(dpi: u32) -> f32 {
    super::fluent::tokens::FLUENT.light.font_size_base300 * dpi as f32 / 96.0
}

/// 分隔线相对下一行顶缘的**上移量**（物理像素）。
///
/// 分隔线要和文字一样参与纵向节奏，所以按**墨迹等距**定位，而不是按行盒等距：
/// 线上方是上一行的最后一行字、下方是下一行的标签，两侧视觉距离必须相等。
///
/// 行盒都是 vcenter 的，墨迹上下各留 `(盒高 − 字号) / 2`：
///   - 上方有说明 = 说明行盒 `HINT_EXTRA`(20) 与说明字号(14) → 3
///   - 上方无说明 = 控件带 `ROW_H`(34) 与标签字号(16) → 9
///   - 下方恒为 控件带 `ROW_H` 与标签字号 → 9
///
/// 在 ROW_GAP 中点的基础上再补两侧留白差值的一半：
/// `rise = ROW_GAP/2 − (pad_below − pad_above)/2`。
///
/// 当前字号下（96 dpi）说明在上方时 rise = 3，于是
/// 「说明墨迹底 → 线」=「线 → 标签墨迹顶」=「标签墨迹底 → 说明墨迹顶」= **12**，
/// 全页每一个纵向间隔都相等 —— 这才叫「间距相同」。
pub fn divider_rise(dpi: u32, above_has_hint: bool) -> f32 {
    let pad_below = (s(dpi, ROW_H) as f32 - body_px(dpi)) / 2.0;
    let pad_above = if above_has_hint {
        (s(dpi, HINT_EXTRA) as f32 - caption_px(dpi)) / 2.0
    } else {
        pad_below
    };
    s(dpi, ROW_GAP) as f32 / 2.0 - (pad_below - pad_above) / 2.0
}

/// 浮点逻辑坐标缩放（行高可变，布局在浮点逻辑坐标上累积）。
fn sf(dpi: u32, v: f32) -> i32 {
    (v * dpi as f32 / 96.0).round() as i32
}

// ── 页面元信息 ───────────────────────────────────────────

pub fn page_caption(p: usize, lang: Lang) -> &'static str {
    let tr = |k: &'static str| crate::i18n::t(lang, k);
    match p {
        0 => tr("PgGeneral"),
        1 => tr("PgLayout"),
        2 => tr("PgMetrics"),
        3 => tr("PgWindow"),
        4 => tr("PgPerformance"),
        _ => tr("PgAbout"),
    }
}

/// 某页脚注（关于页版权）。
pub fn page_footnote(p: usize, lang: Lang) -> Option<&'static str> {
    let tr = |k: &'static str| crate::i18n::t(lang, k);
    match p {
        5 => Some(tr("AboutCopyright")),
        _ => None,
    }
}

/// 内容区原点（客户区坐标）：返回 (x, y)。
///
/// `x` = 侧边栏右边界；`y` = 内容滚动偏移的负值。
/// **所有** `Widget.rect` / `Card.rect` 存内容坐标，
/// 绘制与命中一律 `+ origin`，禁止裸加常量（红线 R7）。
pub fn content_origin(d: u32, scroll_y: i32) -> (i32, i32) {
    (s(d, SIDEBAR_W), -scroll_y)
}

/// 内容区可视尺寸（客户区坐标下的宽高）。
pub fn content_viewport(d: u32, client_w: i32, client_h: i32) -> (i32, i32) {
    ((client_w - s(d, SIDEBAR_W)).max(1), client_h)
}

// ── 行构建器（逻辑坐标） ─────────────────────────────────

struct ItemDef {
    id: i32,
    kind: Box<dyn super::state::IWidget>,
    /// 绝对逻辑 x（内容坐标）。
    l: f32,
    r: f32,
    /// 滑杆行：控件矩形纵跨整行（容纳端点标注）。
    tall: bool,
}

struct RowDef {
    label: &'static str,
    hint_text: Option<&'static str>,
    /// 说明第二行（主句 + 弱化补充句，报告窗口页）。
    hint2: Option<&'static str>,
    divider: bool,
    /// 整行是小字说明（无控件、无标签列）。
    hint_only: bool,
    /// 折叠头行（监控项卡首行）：独占更高的行高，容纳放大后的图标与标题。
    header: bool,
    items: Vec<ItemDef>,
}

impl RowDef {
    /// 行内 hint 行数（主句 + 弱化补充句；空串不算）。
    fn hint_lines(&self) -> u32 {
        self.hint_text.map(|t| !t.is_empty() as u32).unwrap_or(0)
            + self.hint2.map(|t| !t.is_empty() as u32).unwrap_or(0)
    }

    /// **控件带**高度（逻辑像素）。不包含 hint 带 —— hint 从控件带之后起画。
    ///
    /// 小字说明行（`hint_only`）按说明行高给足即可：旧实现套用 `ROW_H`(34)，
    /// 12px 的字待在 34 的行盒里，上下各空 11px —— 就是用户圈出来的「有点空」。
    fn base_h(&self) -> f32 {
        if self.header {
            ROW_HEADER_H as f32
        } else if self.hint_only {
            HINT_EXTRA as f32
        } else if self.items.iter().any(|it| it.tall) {
            ROW_SLIDER_H as f32
        } else {
            ROW_H as f32
        }
    }


    /// 行高（逻辑像素）= 控件带 + 行内 hint 每行 +15。
    fn height(&self) -> f32 {
        self.base_h() + HINT_EXTRA as f32 * self.hint_lines() as f32
    }
}

fn row_def(label: &'static str, hint: Option<&'static str>, items: Vec<ItemDef>) -> RowDef {
    RowDef { label, hint_text: hint, hint2: None, divider: false, hint_only: false, header: false, items }
}

fn hint_row_def(text: &'static str) -> RowDef {
    RowDef {
        label: text,
        hint_text: None,
        hint2: None,
        divider: false,
        hint_only: true,
        header: false,
        items: Vec::new(),
    }
}

// ── 页面构建（纯函数） ───────────────────────────────────

/// 行构建辅助：几何 + 语言 + 控件构造，供各 `page_*` 共用。
/// （原为 `build_pages` 内的闭包；拆出后各页函数不再重复捕获。）
struct Ux {
    lang: Lang,
    rows_l: f32,
    widget_r: f32,
    ctrl_l: f32,
}

impl Ux {
    fn tr(&self, k: &'static str) -> &'static str {
        crate::i18n::t(self.lang, k)
    }
    fn seg(&self, labels: &[&'static str], cfg: CfgRef) -> SegmentedWidget {
        SegmentedWidget {
            labels: labels.iter().map(|k| self.tr(k).to_string()).collect::<Vec<_>>().into(),
            cfg,
        }
    }
    fn item(&self, id: i32, kind: Box<dyn super::state::IWidget>, l: f32, r: f32, tall: bool) -> ItemDef {
        ItemDef { id, kind, l, r, tall }
    }
    fn ctrl_item(&self, id: i32, kind: Box<dyn super::state::IWidget>) -> ItemDef {
        let tall = kind.as_any().is::<SliderWidget>();
        self.item(id, kind, self.ctrl_l, self.widget_r, tall)
    }
    fn toggle_item(&self, id: i32, cfg: CfgRef) -> ItemDef {
        self.item(id, Box::new(ToggleWidget{ cfg }), self.widget_r - TOGGLE_W as f32, self.widget_r, false)
    }
    fn stepper_item(&self, id: i32, cfg: CfgRef) -> ItemDef {
        self.item(
            id,
            Box::new(StepperWidget{ min: GAP_MIN as f64, max: GAP_MAX as f64, step: 1.0, cfg }),
            self.widget_r - STEPPER_W as f32,
            self.widget_r,
            false,
        )
    }
    fn off_stepper_item(&self, id: i32, cfg: CfgRef) -> ItemDef {
        self.item(
            id,
            Box::new(StepperWidget{ min: OFF_MIN as f64, max: OFF_MAX as f64, step: 1.0, cfg }),
            self.widget_r - STEPPER_W as f32,
            self.widget_r,
            false,
        )
    }
    fn font_slider_item(&self, id: i32, cfg: CfgRef, l: f32, r: f32, metric: bool) -> ItemDef {
        ItemDef {
            id,
            kind: Box::new(SliderWidget{ min: FONT_MIN, max: FONT_MAX, step: 1.0, dp: 0, unit: "px", cfg, metric_style: metric }),
            l,
            r,
            tall: true,
        }
    }
}

type Group = (&'static str, Vec<RowDef>, Option<&'static str>, i32);

fn page_prefs(
    ux: &Ux,
    _cfg: &Config,
    _expand: &[f32; 7],
    _order: &[u8; 7],
    groups: &mut Vec<Group>,
) {
        // ── 偏好：显示 / 启动 / 诊断（报告 §11 w1） ──
        groups.push((
            ux.tr("GrpDisplay"),
            vec![
                row_def(
                    ux.tr("LblLang"),
                    Some(ux.tr("HintLanguage")),
                    vec![ux.ctrl_item(230, Box::new(ux.seg(&["SegEn", "SegZh"], CfgRef::Lang)))],
                ),
                row_def(
                    ux.tr("LblAppearance"),
                    Some(ux.tr("HintAppearance")),
                    vec![ux.ctrl_item(
                        234,
                        Box::new(ux.seg(
                            &["SegAppearanceSystem", "SegAppearanceLight", "SegAppearanceDark"],
                            CfgRef::Appearance,
                        )),
                    )],
                ),
            ],
            None,
            CARD_GAP,
        ));
        groups.push((
            ux.tr("GrpStartup"),
            vec![row_def(
                ux.tr("LblAutostart"),
                Some(ux.tr("HintAutostart")),
                vec![ux.toggle_item(231, CfgRef::AutoStart)],
            )],
            None,
            CARD_GAP,
        ));
        groups.push((
            ux.tr("GrpDiagnostics"),
            vec![
                row_def(
                    ux.tr("LblDebug"),
                    Some(ux.tr("HintDebugLog")),
                    vec![ux.toggle_item(233, CfgRef::DebugLog)],
                ),
                // 日志文件：路径降级为按钮动作（报告 C-01/C-03）。
                row_def(
                    ux.tr("LblLogFolder"),
                    None,
                    vec![ux.ctrl_item(
                        235,
                        Box::new(ButtonWidget {
                            label: ux.tr("ActOpenLogFolder").to_string(),
                            action: ButtonAction::OpenLogFolder,
                            kind: ButtonKind::Secondary,
                        }),
                    )],
                ),
            ],
            None,
            CARD_GAP,
        ));
}

fn page_layout(
    ux: &Ux,
    _cfg: &Config,
    _expand: &[f32; 7],
    _order: &[u8; 7],
    groups: &mut Vec<Group>,
) {
        // ── 布局：排列 / 文字效果（报告 §11 w2） ──
        // 「数据格式」（内存单位 / 网络显示）已下放到「监控项」页对应卡片，
        // 就近配置（用户反馈：与内存/网络无关的页放这些选项很跳）。
        groups.push((
            ux.tr("GrpArrange"),
            vec![
                row_def(
                    ux.tr("LblStyle"),
                    Some(ux.tr("HintStyle")),
                    vec![ux.ctrl_item(150, Box::new(ux.seg(&["SegHorizontal", "SegVertical"], CfgRef::Style)))],
                ),
                row_def(
                    ux.tr("LblFontSize"),
                    Some(ux.tr("HintFontSizeGlobal")),
                    vec![ux.ctrl_item(152, {
                        let mut it = Box::new(SliderWidget {
                            min: FONT_MIN, max: FONT_MAX, step: 1.0, dp: 0, unit: "px",
                            cfg: CfgRef::FontSize,
                            metric_style: false,
                        }) as Box<dyn super::state::IWidget>;
                        let _ = &mut it;
                        it
                    })],
                ),
                row_def(
                    ux.tr("LblGapMetric"),
                    Some(ux.tr("HintItemGap")),
                    vec![ux.stepper_item(153, CfgRef::MetricGap)],
                ),
                row_def(
                    ux.tr("LblGapLabel"),
                    Some(ux.tr("HintNvGap")),
                    vec![ux.stepper_item(154, CfgRef::LabelValueGap)],
                ),
            ],
            None,
            CARD_GAP,
        ));
        groups.push((
            ux.tr("GrpTextEffects"),
            vec![
                row_def(
                    ux.tr("LblFontWeight"),
                    None,
                    vec![ux.ctrl_item(168, Box::new(ux.seg(&["SegThin", "SegMedium", "SegBold"], CfgRef::FontWeight)))],
                ),
                row_def(
                    ux.tr("LblShadow"),
                    Some(ux.tr("HintTextShadow")),
                    vec![ux.toggle_item(158, CfgRef::TextShadow)],
                ),
                row_def(
                    ux.tr("LblOutline"),
                    Some(ux.tr("HintTextStroke")),
                    vec![ux.toggle_item(159, CfgRef::TextOutline)],
                ),
            ],
            None,
            CARD_GAP,
        ));
}

/// 监控项控件 id：`base + 10i` 沿用历史分配（i ≤ 5）；
/// Util（i = 6）整体平移到空闲的 **500 段** —— 直接套公式会得到 260..268，
/// 与既有静态控件撞号（同页 261 = 简化名称开关；跨页 263/265/266/267 = 性能页）。
/// 过渡弹簧按 (页, id) 键控、命中测试按 id 查控件，撞号会串动画/串交互。
fn mid(base: i32, i: usize) -> i32 {
    if i < 6 {
        return base + i as i32 * 10;
    }
    match base {
        200..=208 => 500 + (base - 200),
        260 => 519,
        400..=405 => 510 + (base - 400),
        _ => base + i as i32 * 10,
    }
}

fn page_metrics(
    ux: &Ux,
    cfg: &Config,
    expand: &[f32; 7],
    order: &[u8; 7],
    groups: &mut Vec<Group>,
) {
        // ── 监控项：每项一张**独立卡**（按 metric_order 排列，可拖动排序） ──
        let metric_cfg: [CfgRef; 7] = [
            CfgRef::ShowCpu,
            CfgRef::ShowMem,
            CfgRef::ShowGpu,
            CfgRef::ShowNet,
            CfgRef::PowerEnabled,
            CfgRef::ShowTime,
            CfgRef::ShowUtil,
        ];
        // 折叠头只保留 抓手 + 名称 + 贴右缘的圆形展开按钮；
        // 总开关移入展开菜单首行（「显示」）。
        // 位置 k ← order[k] = 指标下标。控件 id 跟随**指标下标**（不随
        // 位置变化），过渡弹簧按 (页, id) 键控 → 重排不串动画。
        for &mi in order.iter() {
            let i = mi as usize;
            if i >= metric_cfg.len() {
                continue;
            }
            let e = expand.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            let expanded = e > 0.5;
            let mut rows: Vec<RowDef> = Vec::new();
            // 折叠头：抓手点阵 + 标题（画在 MetricHeaderWidget 内）
            // + 圆形展开按钮（贴右缘）。
            // 两项都置 tall：矩形纵跨整行 → 头部拖动热区 = 100% 行高，
            // 展开按钮 28×48 → 圆形按短边取半径、垂直居中于行。
            rows.push(RowDef {
                label: "",
                hint_text: None,
                hint2: None,
                divider: false,
                hint_only: false,
                header: true,
                items: vec![
                    ux.item(
                        mid(208, i),
                        Box::new(MetricHeaderWidget { index: i, on_cfg: metric_cfg[i], lang: ux.lang }),
                        ux.rows_l,
                        ux.widget_r - ICON_BTN as f32,
                        true,
                    ),
                    ux.item(
                        mid(207, i),
                        Box::new(DisclosureWidget {
                            expanded,
                            action: ButtonAction::ToggleMetricExpand(i),
                        }),
                        ux.widget_r - ICON_BTN as f32,
                        ux.widget_r,
                        true,
                    ),
                ],
            });

            if e > 0.001 {
                // 首行 = 总开关（用户反馈：开关从折叠头移入展开菜单，命名「显示」）。
                rows.push(row_def(
                    ux.tr("LblShow"),
                    None,
                    vec![ux.toggle_item(mid(200, i), metric_cfg[i])],
                ));
                // Time 卡：显示秒开关紧随总开关（该卡最常用的选项）。
                if i == 5 {
                    rows.push(row_def(
                        ux.tr("LblTimeSeconds"),
                        Some(ux.tr("HintTimeSeconds")),
                        vec![ux.toggle_item(252, CfgRef::TimeSeconds)],
                    ));
                }
                // 显示名称开关（隐藏后只显示数值，如"111W"）。
                rows.push(row_def(
                    ux.tr("LblShowLabel"),
                    Some(ux.tr("HintShowLabel")),
                    vec![ux.toggle_item(mid(260, i), CfgRef::MetricShowLabel(i))],
                ));
                // 数据格式：内存单位 / 网络显示（用户反馈：从「布局」页并入
                // 对应卡片，就近配置；与字号/配色一样只影响这一项）。
                if i == 1 {
                    rows.push(row_def(
                        ux.tr("LblMemUnit"),
                        Some(ux.tr("HintMemUnit")),
                        vec![ux.ctrl_item(155, Box::new(ux.seg(&["SegPercent", "SegGb"], CfgRef::MemUnit)))],
                    ));
                } else if i == 3 {
                    rows.push(row_def(
                        ux.tr("LblNetMode"),
                        Some(ux.tr("HintNetDisplay")),
                        vec![ux.ctrl_item(167, Box::new(ux.seg(&["SegBoth", "SegDown", "SegUp"], CfgRef::NetMode)))],
                    ));
                }
                let _overridden = cfg.metric_font_sizes.get(i).copied().flatten().is_some();
                // 字号滑杆：理想布局中独占整行，数值纯文本、绿填充、白圈滑块。
                rows.push(RowDef {
                    label: ux.tr("LblFontSize"),
                    hint_text: None,
                    hint2: None,
                    divider: false,
                    hint_only: false,
                    header: false,
                    items: vec![ux.font_slider_item(
                        mid(201, i),
                        CfgRef::MetricFont(i),
                        ux.ctrl_l,
                        ux.widget_r,
                        true,
                    )],
                });
                // 字号状态条：未覆盖时「跟随全局…」；已覆盖时「已自定义」标签 + 说明 + 右侧恢复链接。
                rows.push(RowDef {
                    label: "",
                    hint_text: None,
                    hint2: None,
                    divider: false,
                    hint_only: false,
                    header: false,
                    items: vec![ux.item(
                        mid(206, i),
                        Box::new(MetricFontStatusWidget { index: i, lang: ux.lang }),
                        ux.ctrl_l,
                        ux.widget_r,
                        false,
                    )],
                });
                // 名称色 / 数值色（色板含「跟随默认」格承担重置语义）。
                rows.push(row_def(
                    ux.tr("LblNameColor"),
                    Some(ux.tr("HintNameColor")),
                    vec![ux.ctrl_item(mid(203, i), Box::new(MetricSwatchesWidget { cfg: CfgRef::MetricLabel(i), auto_cfg: None }))],
                ));
                rows.push(row_def(
                    ux.tr("LblValueColor"),
                    Some(ux.tr("HintValueColor")),
                    vec![ux.ctrl_item(mid(205, i), Box::new(MetricSwatchesWidget { cfg: CfgRef::MetricValue(i), auto_cfg: Some(CfgRef::MetricValueAuto(i)) }))],
                ));
                // 文字微调：名称 / 数值各自可左右、上下偏移（像素，可为负）。
                // 4 个步进器 + 1 个「恢复默认」按钮（把四项一并归零）。
                rows.push(row_def(
                    ux.tr("LblOffNameX"),
                    Some(ux.tr("HintOffName")),
                    vec![ux.off_stepper_item(mid(400, i), CfgRef::MetricOffset(i, 0))],
                ));
                rows.push(row_def(
                    ux.tr("LblOffNameY"),
                    None,
                    vec![ux.off_stepper_item(mid(401, i), CfgRef::MetricOffset(i, 1))],
                ));
                rows.push(row_def(
                    ux.tr("LblOffValueX"),
                    None,
                    vec![ux.off_stepper_item(mid(402, i), CfgRef::MetricOffset(i, 2))],
                ));
                rows.push(row_def(
                    ux.tr("LblOffValueY"),
                    None,
                    vec![ux.off_stepper_item(mid(403, i), CfgRef::MetricOffset(i, 3))],
                ));
                rows.push(RowDef {
                    label: "",
                    hint_text: None,
                    hint2: None,
                    divider: false,
                    hint_only: false,
                    header: false,
                    items: vec![ux.item(
                        mid(405, i),
                        Box::new(ButtonWidget {
                            label: ux.tr("LblResetOffset").to_string(),
                            action: ButtonAction::ResetMetricOffset(i),
                            kind: ButtonKind::Secondary,
                        }),
                        ux.widget_r - 120.0,
                        ux.widget_r,
                        false,
                    )],
                });
                // 展开态同样要分隔线（用户反馈：展开页也要分割线与呼吸感）。
                // 位置 = 本行顶（画在 ROW_GAP 正中，见 paint_page）。
                // 例外：字号状态条是字号滑杆的注解，二者之间不画线，
                // 否则注解被从它解释的那一行上切下来。
                for (k, rd) in rows.iter_mut().enumerate() {
                    let is_annotation = rd
                        .items
                        .iter()
                        .any(|it| it.kind.as_any().is::<MetricFontStatusWidget>());
                    rd.divider = k > 0 && !rd.hint_only && !is_annotation;
                }
            }
            // 每个指标 = 一张独立卡，卡间小间距（拖动排序的视觉分区）。
            groups.push(("", rows, None, METRIC_CARD_GAP));
        }
        // 页脚卡：**设置在前、动作按钮置底**。
        // 原先「全部重置」独占首行，行内只有右侧 96px 按钮、左侧约 500px
        // 全空（实测 497×59 的空白矩形，占整卡 24.8%）—— 观感上就是
        // 「一大块空着的地方」。改为「内容 → 提示 → 动作」的常规顺序，
        // 空白行落到卡底，读作操作区而不是莫名留白。
        groups.push((
            "",
            vec![
                // 简化名称：CPU→C、Mem→M、GPU→G、Net→N、Power→P、Time→T。
                row_def(
                    ux.tr("LblShortNames"),
                    Some(ux.tr("HintShortNames")),
                    vec![ux.toggle_item(261, CfgRef::ShortNames)],
                ),
                hint_row_def(ux.tr("HintDragReorder")),
                hint_row_def(ux.tr("MetricsFoot")),
                // 全部重置（动作放最后一行，右对齐）
                RowDef {
                    label: "",
                    hint_text: None,
                    hint2: None,
                    divider: false,
                    hint_only: false,
                    header: false,
                    items: vec![ux.item(
                        209,
                        Box::new(ButtonWidget {
                            label: ux.tr("LblResetAll").to_string(),
                            action: ButtonAction::ResetAllColors,
                            kind: ButtonKind::Secondary,
                        }),
                        ux.widget_r - 96.0,
                        ux.widget_r,
                        false,
                    )],
                },
            ],
            None,
            CARD_GAP,
        ));
}

fn page_window(
    ux: &Ux,
    _cfg: &Config,
    _expand: &[f32; 7],
    _order: &[u8; 7],
    groups: &mut Vec<Group>,
) {
        // ── 窗口：层级与交互 / 位置与外观（报告 §11 w4） ──
        groups.push((
            ux.tr("GrpInteraction"),
            vec![
                // 主句 + 弱化补充句（报告：说明拆为两行 hint）。
                RowDef {
                    label: ux.tr("LblTopmost"),
                    hint_text: Some(ux.tr("HintAlwaysTop")),
                    hint2: Some(ux.tr("HintAlwaysTop2")),
                    divider: false,
                    hint_only: false,
                    header: false,
                    items: vec![ux.toggle_item(244, CfgRef::Topmost)],
                },
                row_def(
                    ux.tr("LblClickThrough"),
                    Some(ux.tr("HintClickThrough")),
                    vec![ux.toggle_item(245, CfgRef::ClickThrough)],
                ),
            ],
            None,
            CARD_GAP,
        ));
        // 「显示位置（贴角）」已删除（用户反馈：意义不大，窗口可以直接手拖）。
        // 配置字段 snap_corner 保留（首次启动的落位仍用它），只是不再暴露到 UI。
        groups.push((
            ux.tr("GrpPosLook"),
            vec![row_def(
                ux.tr("LblOpacity"),
                Some(ux.tr("HintOpacity")),
                vec![ItemDef {
                    id: 247,
                    kind: Box::new(SliderWidget {
                        min: aaxsyspulse_core::config::OPACITY_PCT_MIN as f32,
                        max: aaxsyspulse_core::config::OPACITY_PCT_MAX as f32,
                        step: 1.0,
                        dp: 0,
                        unit: "%",
                        cfg: CfgRef::Opacity,
                        metric_style: false,
                    }),
                    l: ux.ctrl_l,
                    r: ux.widget_r,
                    tall: true,
                }],
            )],
            None,
            CARD_GAP,
        ));
}

fn page_perf(
    ux: &Ux,
    _cfg: &Config,
    _expand: &[f32; 7],
    _order: &[u8; 7],
    groups: &mut Vec<Group>,
) {
        // ── 性能：统计范围 / 刷新频率 / 画面（报告 §11 w5） ──
        groups.push((
            ux.tr("GrpScope"),
            vec![
                row_def(
                    ux.tr("LblGpuPrimary"),
                    Some(ux.tr("HintGpuPrimary")),
                    vec![ux.toggle_item(263, CfgRef::GpuPrimaryOnly)],
                ),
                row_def(
                    ux.tr("LblGpu3D"),
                    Some(ux.tr("HintGpu3D")),
                    vec![ux.toggle_item(264, CfgRef::Gpu3dOnly)],
                ),
                // 功耗查询：关闭跳过 MSR 内核查询（省 CPU），功耗行显示 --。
                row_def(
                    ux.tr("LblPowerQuery"),
                    Some(ux.tr("HintPowerQuery")),
                    vec![ux.toggle_item(267, CfgRef::PowerQuery)],
                ),
            ],
            None,
            CARD_GAP,
        ));
        groups.push((
            ux.tr("GrpRefresh"),
            vec![row_def(
                ux.tr("LblNetLowFreq"),
                Some(ux.tr("HintNetLowFreq")),
                vec![ux.toggle_item(265, CfgRef::NetLowFreq)],
            )],
            None,
            CARD_GAP,
        ));
        groups.push((
            ux.tr("GrpPicture"),
            vec![row_def(
                ux.tr("LblEngine"),
                Some(ux.tr("HintRenderMode")),
                vec![ux.ctrl_item(266, Box::new(ux.seg(&["SegSmooth", "SegFine"], CfgRef::RenderEngine)))],
            )],
            None,
            CARD_GAP,
        ));
}

fn page_about(
    ux: &Ux,
    _cfg: &Config,
    _expand: &[f32; 7],
    _order: &[u8; 7],
    groups: &mut Vec<Group>,
) {
        // ── 关于：头部（图标 / 名称 / 版本）/ 开发者，版权脚注 ──
        groups.push((
            "",
            vec![
                RowDef {
                    label: "",
                    hint_text: None,
                    hint2: None,
                    divider: false,
                    hint_only: false,
                    header: false,
                    items: vec![ux.item(270, Box::new(AboutHeaderWidget { lang: ux.lang }), ux.rows_l, ux.widget_r, false)],
                },
                // 开发者信息：左侧标签 + 说明（名字），右侧「哔哩哔哩主页」外链按钮。
                row_def(
                    ux.tr("AboutDeveloper"),
                    Some(ux.tr("AboutDeveloperName")),
                    vec![ux.ctrl_item(
                        271,
                        Box::new(ButtonWidget {
                            label: ux.tr("ActBilibili").to_string(),
                            action: ButtonAction::OpenUrl(
                                "https://space.bilibili.com/401121508",
                            ),
                            kind: ButtonKind::Secondary,
                        }),
                    )],
                ),
            ],
            page_footnote(5, ux.lang),
            CARD_GAP,
        ));
}

/// 依据 (cfg, lang, dpi, client_w, expand, order) 重建全部 6 页布局。
///
/// `order`：监控项显示顺序（第 k 位 = 指标下标），监控项页按它排列卡片；
/// 拖动排序期间由调用方传入拖动中的临时顺序实现实时预览。
pub fn build_pages(
    pages: &mut [Page; 6],
    cfg: &Config,
    lang: Lang,
    dpi: u32,
    client_w: i32,
    expand: &[f32; 7],
    order: &[u8; 7],
) {
    let tr = |k: &'static str| crate::i18n::t(lang, k);

    // 内容区几何（**内容坐标**：原点 = 侧边栏右缘；client_w 是物理像素）。
    // 红线 R7：Card/Widget.rect 均为内容坐标，绘制时统一 +ox（ox 已含侧栏偏移），
    // 因此这里绝不能再把 SIDEBAR_W 加进 x —— 否则整块内容右移一个侧栏宽。
    let client_lw = client_w as f32 * 96.0 / dpi as f32;
    let card_l = CONTENT_PAD_X as f32;
    // 卡片右缘：不超过 620（B-04），且不超出内容视口（客户区 - 侧栏 - 右边距）。
    let vp_lw = client_lw - SIDEBAR_W as f32;
    let card_r = (card_l + CONTENT_MAX_W as f32)
        .min(vp_lw - CONTENT_PAD_X as f32)
        .max(card_l + 240.0);
    // 卡内行区左右。
    let rows_l = card_l + CARD_PAD as f32;
    let rows_r = card_r - CARD_PAD as f32;
    // 标签列 / 控件列。控件右缘 = 行区右缘 → 与左侧标签内缩同值（左右对称）。
    let widget_r = rows_r;
    let widget_l = rows_l + LABEL_W as f32;
    let ctrl_l = widget_l.max(widget_r - CTRL_W as f32);

    // 行构建辅助（几何 + 语言 + 控件构造）。
    let ux = Ux { lang, rows_l, widget_r, ctrl_l };

    for (p, page) in pages.iter_mut().enumerate() {
        page.cards.clear();
        page.widgets.clear();
        page.title = page_caption(p, lang);
        page.subtitle = match p {
            0 => tr("PgPrefsSub"),
            1 => tr("PgLayoutSub"),
            2 => tr("PgMetricsSub"),
            3 => tr("PgWindowSub"),
            4 => tr("PgPerfSub"),
            _ => tr("AboutDesc"),
        };

        // 每页 = [(分组标题, 行列表, 脚注, 卡后间距)]；空标题分组跳过标题带。
        let mut groups: Vec<Group> = Vec::new();

        match p {
            0 => page_prefs(&ux, cfg, expand, order, &mut groups),
            1 => page_layout(&ux, cfg, expand, order, &mut groups),
            2 => page_metrics(&ux, cfg, expand, order, &mut groups),
            3 => page_window(&ux, cfg, expand, order, &mut groups),
            4 => page_perf(&ux, cfg, expand, order, &mut groups),
            _ => page_about(&ux, cfg, expand, order, &mut groups),
        }

        // ── 组装：页标题块 + 分组卡（浮点逻辑 y 游标） ──
        let mut y: f32 =
            CONTENT_PAD_TOP as f32 + (TITLE_H + TITLE_SUB_GAP + SUB_H + SUB_CARD_GAP) as f32;
        // 上一张卡与本次累计的间距（page.h 计算时扣除，避免尾部多留一个间距）。
        let mut last_gap: i32 = 0;
        for (caption, rows, foot, gap) in groups {
            // 分组标题带（画在卡顶上方，见 paint_page）；
            // 无标题分组（监控项独立卡 / 关于页）跳过标题带，卡间只用组内 gap。
            if !caption.is_empty() {
                y += (CAPTION_H + CAPTION_GAP) as f32;
            }
            let card_top = y;
            let rows_top = card_top + CARD_PAD as f32;
            let mut ry: f32 = 0.0;
            let mut out_rows: Vec<Row> = Vec::with_capacity(rows.len());
            // 行间分隔线：**有标题的分组卡**内部，行与行之间画一条细线（用户反馈：
            // 各行的说明文字与下一行标签粘在一起，加线后工整）。
            // 无标题分组 = 监控项独立卡（每项一卡），不画内部分隔；
            // 小字说明行（hint_only）与其上一行是一体，两侧都不画线。
            let want_div: Vec<bool> = rows
                .iter()
                .enumerate()
                .map(|(ri, rd)| {
                    let prev_hint = ri == 0 || rows[ri - 1].hint_only;
                    !caption.is_empty() && ri > 0 && !rd.hint_only && !prev_hint
                })
                .collect();
            for (ri, rd) in rows.into_iter().enumerate() {
                // 行与行之间留出统一的 ROW_GAP（第一行不留）。
                if ri > 0 {
                    ry += ROW_GAP as f32;
                }
                let rh = rd.height();
                let base_h = rd.base_h();
                let band_top = rows_top + ry;
                // 控件垂直定位带：无 hint 的行用整行高（真居中，折叠头行 48 时
                // 展开按钮/图标才不会顶在上缘）；有 hint 的行控件与首行标签对齐。
                let band_h = if rd.hint_lines() > 0 { ROW_H as f32 } else { rh };
                let mut idxs = Vec::with_capacity(rd.items.len());
                for it in rd.items {
                    let (wtop, wbot) = if it.tall {
                        // 纵跨**控件带**（不含下方 hint 带）：滑杆的基准线由控件带
                        // 顶部推得，且点在下方的说明文字上不会误触发拖拽/编辑。
                        (sf(dpi, band_top), sf(dpi, band_top + base_h))
                    } else {
                        let t = sf(dpi, band_top + (band_h - WIDGET_H as f32) / 2.0);
                        (t, t + s(dpi, WIDGET_H))
                    };
                    let wrect = windows::Win32::Foundation::RECT {
                        left: sf(dpi, it.l),
                        top: wtop,
                        right: sf(dpi, it.r),
                        bottom: wbot,
                    };
                    page.widgets.push(Widget { id: it.id, kind: it.kind, rect: wrect });
                    idxs.push(page.widgets.len() - 1);
                }
                out_rows.push(Row {
                    label: rd.label,
                    widgets: idxs,
                    divider: rd.divider || want_div[ri],
                    hint: rd.hint_only,
                    hint_text: rd.hint_text,
                    hint2: rd.hint2,
                    y_off: sf(dpi, ry),
                    base_h: sf(dpi, base_h),
                });
                ry += rh;
            }
            let rows_h = sf(dpi, ry);
            let foot_h = if foot.is_some() { s(dpi, FOOTNOTE_H) } else { 0 };
            // 上下内边距**同值**（旧实现底部多留 1×CARD_PAD，卡片看着「下坠」）。
            let card_bottom = rows_top + rows_h as f32 + CARD_PAD as f32 + foot_h as f32;
            page.cards.push(Card {
                caption,
                rect: windows::Win32::Foundation::RECT {
                    left: sf(dpi, card_l),
                    top: sf(dpi, card_top),
                    right: sf(dpi, card_r),
                    bottom: sf(dpi, card_bottom),
                },
                rows: out_rows,
                rows_h,
                footnote: foot,
            });
            y = card_bottom + gap as f32;
            last_gap = gap;
        }
        page.h = sf(dpi, y - last_gap as f32 + BOTTOM_MARGIN as f32);
    }
}

#[cfg(test)]
mod spacing_tests {
    use super::*;

    /// 测试用 DPI（100%，`s()` 恒等，断言里的数字即逻辑像素）。
    const D: u32 = 96;

    fn build_all(expand: f32) -> [Page; 6] {
        let cfg = Config::default();
        let mut pages: [Page; 6] = Default::default();
        build_pages(&mut pages, &cfg, Lang::Zh, D, 900, &[expand; 7], &[0, 1, 2, 6, 3, 4, 5]);
        pages
    }

    /// 行高 = 控件带 + 说明带（与 `RowDef::height` 同源，这里按 Row 的数据复算）。
    fn row_h(r: &Row) -> i32 {
        let lines = r.hint_text.map(|t| !t.is_empty() as i32).unwrap_or(0)
            + r.hint2.map(|t| !t.is_empty() as i32).unwrap_or(0);
        r.base_h + s(D, HINT_EXTRA) * lines
    }

    /// 卡片内的行必须首尾相接：第 i 行 y = 第 i-1 行 y + 高 + ROW_GAP。
    /// 这条断言直接锁住「行与行间距统一」，任何把间距写死/漏加都会红。
    #[test]
    fn rows_are_spaced_by_exactly_row_gap() {
        for (pi, page) in build_all(1.0).iter().enumerate() {
            for card in &page.cards {
                let mut expect = 0i32;
                for (i, r) in card.rows.iter().enumerate() {
                    if i > 0 {
                        expect += s(D, ROW_GAP);
                    }
                    assert_eq!(
                        r.y_off, expect,
                        "第 {pi} 页、标题「{}」的卡片：第 {i} 行 y_off 与「上一行底 + ROW_GAP」不符",
                        card.caption
                    );
                    expect += row_h(r);
                }
                // rows_h 必须等于累计高度（含行间距），否则卡片高与内容对不上。
                assert_eq!(card.rows_h, expect, "第 {pi} 页卡片 rows_h 与行累计高度不符");
            }
        }
    }

    /// 卡片上下内边距必须同值（旧实现底部多留 1×CARD_PAD，卡片看着「下坠」）。
    #[test]
    fn card_padding_is_symmetric() {
        for (pi, page) in build_all(1.0).iter().enumerate() {
            for card in &page.cards {
                if card.footnote.is_some() {
                    continue; // 有脚注的卡片底部由 FOOTNOTE_H 另行占位
                }
                let bottom = card.rect.bottom - (card.rect.top + s(D, CARD_PAD) + card.rows_h);
                assert_eq!(bottom, s(D, CARD_PAD), "第 {pi} 页卡片上下内边距不等");
            }
        }
    }

    /// 控件带高度只能取刻度值，防止有人就地写魔法数字。
    #[test]
    fn control_band_uses_scale_only() {
        let allowed = [
            s(D, ROW_H),
            s(D, ROW_SLIDER_H),
            s(D, ROW_HEADER_H),
            s(D, HINT_EXTRA),
        ];
        for page in build_all(1.0).iter() {
            for card in &page.cards {
                for r in &card.rows {
                    assert!(
                        allowed.contains(&r.base_h),
                        "控件带高度 {} 不在刻度 {} 内",
                        r.base_h,
                        allowed.iter().map(|v| v.to_string()).collect::<Vec<_>>().join("/")
                    );
                }
            }
        }
    }

    /// 分隔线只能在「行与行之间」出现（不能出现在首行，也不能切在标签与它的说明之间）。
    #[test]
    fn dividers_only_between_rows() {
        for page in build_all(1.0).iter() {
            for card in &page.cards {
                for (i, r) in card.rows.iter().enumerate() {
                    if r.divider {
                        assert!(i > 0, "首行不应有分隔线");
                        assert!(!card.rows[i - 1].hint, "分隔线不能切在说明行与上一行之间");
                        assert!(!r.hint, "分隔线不能切在说明行与其上一行之间");
                    }
                }
            }
        }
    }

    /// 监控项独立卡（无标题分组）：不画内部分隔，且首行是折叠头行高。
    /// 判定方式 = 首行含 `MetricHeaderWidget`（页脚卡不含，需排除）。
    fn is_metric_card(page: &Page, card: &Card) -> bool {
        card.rows
            .first()
            .map(|r| {
                r.widgets
                    .iter()
                    .any(|&i| page.widgets[i].kind.as_any().is::<MetricHeaderWidget>())
            })
            .unwrap_or(false)
    }

    #[test]
    fn metric_cards_have_no_inner_dividers_and_tall_header() {
        let mut checked = 0;
        for expand in [0.0f32, 1.0] {
            let pages = build_all(expand);
            let page = &pages[2];
            for card in &page.cards {
                assert!(card.caption.is_empty(), "监控项卡应为无标题分组");
                if !is_metric_card(page, card) {
                    continue;
                }
                checked += 1;
                assert_eq!(
                    card.rows[0].base_h,
                    s(D, ROW_HEADER_H),
                    "监控项卡首行应为折叠头行高"
                );
                for (k, r) in card.rows.iter().enumerate() {
                    // 折叠态只有折叠头一行，不需要内部分隔线；
                    // 展开态除「字号注解行」外，每行都要有分隔线（用户反馈：展开页也要）。
                    let annotation = r
                        .widgets
                        .iter()
                        .any(|&wi| page.widgets[wi].kind.as_any().is::<MetricFontStatusWidget>());
                    let want = expand > 0.5 && k > 0 && !annotation;
                    assert_eq!(r.divider, want, "展开度 {expand}：第 {k} 行分隔线不符预期");
                }
            }
        }
        assert_eq!(checked, 14, "两种展开态 × 7 张监控项卡");
    }

    /// 折叠（未展开）时监控项卡只有一行（就是折叠头）。
    #[test]
    fn collapsed_metric_card_has_single_row() {
        let pages = build_all(0.0);
        let page = &pages[2];
        for card in &page.cards {
            if !is_metric_card(page, card) {
                continue;
            }
            assert_eq!(card.rows.len(), 1, "折叠态监控项卡应只有折叠头一行");
            assert_eq!(card.rows[0].base_h, s(D, ROW_HEADER_H));
        }
    }

    /// 纵向节奏：分隔线要当成一行字参与。
    /// 「标签墨迹底 → 说明墨迹顶」「说明墨迹底 → 分隔线」「分隔线 → 下一标签墨迹顶」
    /// 三者必须相等 —— 用户反馈「字到分隔线的距离也应该相同」。
    #[test]
    fn vertical_rhythm_includes_the_divider() {
        let label_pad = (s(D, ROW_H) as f32 - body_px(D)) / 2.0;
        let hint_pad = (s(D, HINT_EXTRA) as f32 - caption_px(D)) / 2.0;
        // 行 0 顶 = 0，控件带 ROW_H，带一条说明；行 1 顶 = 行高 + ROW_GAP。
        let label_ink_b = s(D, ROW_H) as f32 - label_pad;
        let hint_ink_t = s(D, ROW_H) as f32 + hint_pad;
        let hint_ink_b = s(D, ROW_H) as f32 + s(D, HINT_EXTRA) as f32 - hint_pad;
        let row1_top = s(D, ROW_H + HINT_EXTRA + ROW_GAP) as f32;
        let label1_ink_t = row1_top + label_pad;
        let div = row1_top - divider_rise(D, true);

        let g1 = hint_ink_t - label_ink_b;
        let g2 = div - hint_ink_b;
        let g3 = label1_ink_t - div;
        assert!((g1 - g2).abs() < 0.05, "说明↔分隔线 与 标签↔说明 不等距：{g1} vs {g2}");
        assert!((g2 - g3).abs() < 0.05, "分隔线两侧不等距：{g2} vs {g3}");

        // 上一行无说明时，两侧也必须等距（rise 退化为 ROW_GAP/2）。
        let row1_top2 = s(D, ROW_H + ROW_GAP) as f32;
        let div2 = row1_top2 - divider_rise(D, false);
        let above = div2 - (s(D, ROW_H) as f32 - label_pad);
        let below = (row1_top2 + label_pad) - div2;
        assert!((above - below).abs() < 0.05, "无说明行：分隔线两侧不等距：{above} vs {below}");
        assert!((divider_rise(D, false) - s(D, ROW_GAP) as f32 / 2.0).abs() < 0.05);
    }

    /// 相邻量级必须拉开：分组标题↔卡片(8) < 行↔行(12) < 卡片内边距(16)
    /// < 卡片↔卡片(24)。否则视觉上读不出分组关系。
    #[test]
    fn spacing_scale_is_monotonic_with_contrast() {
        assert!(CAPTION_GAP > 0 && CAPTION_GAP < ROW_GAP);
        assert!(ROW_GAP < CARD_PAD);
        assert!(CARD_PAD < CARD_GAP);
        // 行盒必须放得下自己的字号，否则字会被行盒挤住（「贴到一起」的根因之一）。
        // 直接引用字号函数（而非写字面量）——字阶调整时测试自动跟随。
        assert!((HINT_EXTRA as f32) > caption_px(D), "说明行高放不下说明字号");
        assert!((CAPTION_H as f32) > caption_px(D), "分组标题带放不下标题字号");
        assert!((ROW_H as f32) > body_px(D), "基础行高放不下正文标签");
    }

    /// `build_pages` 重建预算（后端体检 P1 的测量门禁）：
    /// 重建是事件驱动（配置变更/折叠/拖拽落位/切语言），不逐帧执行，
    /// 因此预算宽松——目的是**锁住回归**（有人往循环里塞分配/重活立刻可见），
    /// 并在 release 下打印真实耗时供决策参考。
    #[test]
    fn page_rebuild_is_within_budget() {
        // 预热（首跑含页标题查表等冷路径，不代表稳态）。
        let _ = build_all(0.0);
        let _ = build_all(1.0);

        // 两种展开态各测 50 次取最小值（排除调度抖动）。
        let mut best_collapsed = f64::MAX;
        let mut best_expanded = f64::MAX;
        for _ in 0..50 {
            let t = std::time::Instant::now();
            let _ = build_all(0.0);
            best_collapsed = best_collapsed.min(t.elapsed().as_secs_f64() * 1e6);

            let t = std::time::Instant::now();
            let _ = build_all(1.0);
            best_expanded = best_expanded.min(t.elapsed().as_secs_f64() * 1e6);
        }
        println!(
            "rebuild_pages collapsed={best_collapsed:.0}µs expanded={best_expanded:.0}µs (release 下供参考)"
        );
        // 全部 6 页 × 全部控件的整体重建 < 5ms（release 实测通常远低于此）。
        // 超预算说明有人在 build_pages 循环里加了重活，先测量再优化。
        assert!(best_expanded < 5_000.0, "页面重建超预算：{best_expanded:.0}µs");
    }
}
