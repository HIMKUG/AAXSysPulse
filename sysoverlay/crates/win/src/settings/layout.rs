//! 布局：常量、DPI 缩放与**纯函数**页面构建（v5 设置页规范重写）。
//!
//! # 结构（报告 §11 目标形态）
//! 每页 = 页标题 + 副标题 + 若干**分组卡**；行高可变：
//! 基础行 28、滑杆行 40（容纳 min/max 端点标注）、行内说明 +15（多行按行数累加）。
//!
//! # 折叠卡展开动画
//! `expand[i]` 是 0..1 的**动画展开度**（由 `mod.rs` 的弹簧每帧推进）：
//! 监控项 i 的行数 = 1 + 4·expand[i]（小数行），因此卡片高度连续变化；
//! 卡内内容行始终按展开后的固定位置排布，由 `paint_page` 按卡片矩形**裁剪**露出。

use sysoverlay_core::config::Config;
use sysoverlay_core::types::Lang;

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

/// 页标题 / 副标题（报告 §11 每页 h5 + p）。
pub const TITLE_H: i32 = 26;
pub const TITLE_SUB_GAP: i32 = 4;
pub const SUB_H: i32 = 16;
pub const SUB_CARD_GAP: i32 = 10;

/// 下列沿用：
pub const BOTTOM_MARGIN: i32 = 16;
pub const CARD_PAD: i32 = 10;
pub const CAPTION_H: i32 = 14;
pub const CAPTION_GAP: i32 = 6;
/// 基础行高。
pub const ROW_H: i32 = 28;
/// 滑杆行高（额外容纳 min/max 端点标注，报告 B-05）。
pub const ROW_SLIDER_H: i32 = 40;
/// 行内说明（hint）每行附加高度。
pub const HINT_EXTRA: i32 = 20;
pub const WIDGET_H: i32 = 24;
pub const CARD_RADIUS: i32 = 10;
pub const LABEL_W: i32 = 120;
pub const TOGGLE_W: i32 = 40;
pub const VALUEBOX_W: i32 = 64;
pub const FOOTNOTE_H: i32 = 60;
pub const SCROLLBAR_W: i32 = 3;
pub const SCROLLBAR_INSET: i32 = 4;

/// **统一控件列**：分段 / 滑杆 / 色板等「大控件」右对齐到同一条右边缘（操作边缘），
/// 且左侧只在必要时向左扩（窗口变宽时控件不跟着无限拉长）。
pub const CTRL_W: i32 = 300;
/// 行尾圆形图标按钮边长。
pub const ICON_BTN: i32 = 24;
/// 步进器「−」/「＋」按钮宽（总宽 = 28 + 40 + 28 = 96）。
pub const STEPPER_BTN_W: i32 = 28;
pub const STEPPER_W: i32 = 96;

/// 滑杆范围（报告 §11：字号 12..72；间距沿用配置钳位 0..64）。
pub const FONT_MIN: f32 = 12.0;
pub const FONT_MAX: f32 = 72.0;
pub const GAP_MIN: f32 = 0.0;
pub const GAP_MAX: f32 = 64.0;

/// DPI 缩放（逻辑像素 → 物理像素）。
pub fn s(dpi: u32, v: i32) -> i32 {
    v * dpi as i32 / 96
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
    items: Vec<ItemDef>,
}

impl RowDef {
    /// 行高（逻辑像素）：滑杆行 40；行内 hint 每行 +15。
    fn height(&self) -> f32 {
        let has_slider = self.items.iter().any(|it| it.tall);
        let mut h = if has_slider { ROW_SLIDER_H as f32 } else { ROW_H as f32 };
        let lines = self.hint_text.map(|t| !t.is_empty() as u32).unwrap_or(0)
            + self.hint2.map(|t| !t.is_empty() as u32).unwrap_or(0);
        h += HINT_EXTRA as f32 * lines as f32;
        h
    }
}

fn row_def(label: &'static str, hint: Option<&'static str>, items: Vec<ItemDef>) -> RowDef {
    RowDef { label, hint_text: hint, hint2: None, divider: false, hint_only: false, items }
}

fn hint_row_def(text: &'static str) -> RowDef {
    RowDef { label: text, hint_text: None, hint2: None, divider: false, hint_only: true, items: Vec::new() }
}

// ── 页面构建（纯函数） ───────────────────────────────────

/// 依据 (cfg, lang, dpi, client_w, expand) 重建全部 6 页布局。
pub fn build_pages(
    pages: &mut [Page; 6],
    cfg: &Config,
    lang: Lang,
    dpi: u32,
    client_w: i32,
    expand: &[f32; 5],
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
    // 标签列 / 控件列。
    let widget_r = rows_r - 8.0; // 操作边缘
    let widget_l = rows_l + LABEL_W as f32;
    let ctrl_l = widget_l.max(widget_r - CTRL_W as f32);

    // 控件构造辅助（闭包仅捕获几何常量，无可变状态）。
    let seg = |labels: &[&'static str], cfg: CfgRef| SegmentedWidget {
        labels: labels.iter().map(|k| tr(k).to_string()).collect::<Vec<_>>().into(),
        cfg,
    };
    let item = |id: i32, kind: Box<dyn super::state::IWidget>, l: f32, r: f32, tall: bool| ItemDef { id, kind, l, r, tall };
    let ctrl_item = |id: i32, kind: Box<dyn super::state::IWidget>| {
        let tall = kind.as_any().is::<SliderWidget>();
        item(id, kind, ctrl_l, widget_r, tall)
    };
    let toggle_item = |id: i32, cfg: CfgRef| {
        item(id, Box::new(ToggleWidget { cfg }), widget_r - TOGGLE_W as f32, widget_r, false)
    };
    let stepper_item = |id: i32, cfg: CfgRef| {
        item(
            id,
            Box::new(StepperWidget { min: GAP_MIN as f64, max: GAP_MAX as f64, step: 1.0, cfg }),
            widget_r - STEPPER_W as f32,
            widget_r,
            false,
        )
    };
    let font_slider_item = |id: i32, cfg: CfgRef, l: f32, r: f32, metric: bool| ItemDef {
        id,
        kind: Box::new(SliderWidget { min: FONT_MIN, max: FONT_MAX, step: 1.0, dp: 0, unit: "px", cfg, metric_style: metric }),
        l,
        r,
        tall: true,
    };

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

        // 每页 = [(分组标题, 行列表, 脚注)]。
        let mut groups: Vec<(&'static str, Vec<RowDef>, Option<&'static str>)> = Vec::new();

        match p {
            0 => {
                // ── 偏好：显示 / 启动 / 诊断（报告 §11 w1） ──
                groups.push((
                    tr("GrpDisplay"),
                    vec![
                        row_def(
                            tr("LblLang"),
                            Some(tr("HintLanguage")),
                            vec![ctrl_item(230, Box::new(seg(&["SegEn", "SegZh"], CfgRef::Lang)))],
                        ),
                        row_def(
                            tr("LblAppearance"),
                            Some(tr("HintAppearance")),
                            vec![ctrl_item(
                                234,
                                Box::new(seg(
                                    &["SegAppearanceSystem", "SegAppearanceLight", "SegAppearanceDark"],
                                    CfgRef::Appearance,
                                )),
                            )],
                        ),
                    ],
                    None,
                ));
                groups.push((
                    tr("GrpStartup"),
                    vec![row_def(
                        tr("LblAutostart"),
                        Some(tr("HintAutostart")),
                        vec![toggle_item(231, CfgRef::AutoStart)],
                    )],
                    None,
                ));
                groups.push((
                    tr("GrpDiagnostics"),
                    vec![
                        row_def(
                            tr("LblDebug"),
                            Some(tr("HintDebugLog")),
                            vec![toggle_item(233, CfgRef::DebugLog)],
                        ),
                        // 日志文件：路径降级为按钮动作（报告 C-01/C-03）。
                        row_def(
                            tr("LblLogFolder"),
                            None,
                            vec![ctrl_item(
                                235,
                                Box::new(ButtonWidget {
                                    label: tr("ActOpenLogFolder").to_string(),
                                    action: ButtonAction::OpenLogFolder,
                                    kind: ButtonKind::Secondary,
                                }),
                            )],
                        ),
                    ],
                    None,
                ));
            }
            1 => {
                // ── 布局：排列 / 数据格式 / 文字效果（报告 §11 w2） ──
                groups.push((
                    tr("GrpArrange"),
                    vec![
                        row_def(
                            tr("LblStyle"),
                            Some(tr("HintStyle")),
                            vec![ctrl_item(150, Box::new(seg(&["SegHorizontal", "SegVertical"], CfgRef::Style)))],
                        ),
                        row_def(
                            tr("LblFontSize"),
                            Some(tr("HintFontSizeGlobal")),
                            vec![ctrl_item(152, {
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
                            tr("LblGapMetric"),
                            Some(tr("HintItemGap")),
                            vec![stepper_item(153, CfgRef::MetricGap)],
                        ),
                        row_def(
                            tr("LblGapLabel"),
                            Some(tr("HintNvGap")),
                            vec![stepper_item(154, CfgRef::LabelValueGap)],
                        ),
                    ],
                    None,
                ));
                groups.push((
                    tr("GrpDataFormat"),
                    vec![
                        row_def(
                            tr("LblMemUnit"),
                            Some(tr("HintMemUnit")),
                            vec![ctrl_item(155, Box::new(seg(&["SegPercent", "SegGb"], CfgRef::MemUnit)))],
                        ),
                        row_def(
                            tr("LblNetMode"),
                            Some(tr("HintNetDisplay")),
                            vec![ctrl_item(167, Box::new(seg(&["SegBoth", "SegDown", "SegUp"], CfgRef::NetMode)))],
                        ),
                    ],
                    None,
                ));
                groups.push((
                    tr("GrpTextEffects"),
                    vec![
                        row_def(
                            tr("LblFontWeight"),
                            None,
                            vec![ctrl_item(168, Box::new(seg(&["SegThin", "SegMedium", "SegBold"], CfgRef::FontWeight)))],
                        ),
                        row_def(
                            tr("LblShadow"),
                            Some(tr("HintTextShadow")),
                            vec![toggle_item(158, CfgRef::TextShadow)],
                        ),
                        row_def(
                            tr("LblOutline"),
                            Some(tr("HintTextStroke")),
                            vec![toggle_item(159, CfgRef::TextOutline)],
                        ),
                    ],
                    None,
                ));
            }
            2 => {
                // ── 监控项：折叠列表（两级字号，报告 §5/§11 w3） ──
                let metric_cfg: [CfgRef; 5] = [
                    CfgRef::ShowCpu,
                    CfgRef::ShowMem,
                    CfgRef::ShowGpu,
                    CfgRef::ShowNet,
                    CfgRef::PowerEnabled,
                ];
                let mut rows: Vec<RowDef> = Vec::new();
                // 开关紧邻名称（用户反馈：如「CPU」右侧空三格即开关），
                // 展开/收起圆形按钮贴最右缘。
                let tog_l = rows_l + s(dpi, 100) as f32; // 名称列(88) + 间距(12)
                let tog_r = tog_l + TOGGLE_W as f32;
                for i in 0..5 {
                    let e = expand[i].clamp(0.0, 1.0);
                    let expanded = e > 0.5;
                    // 折叠头：徽标 + 标题（折叠态无状态摘要）+ 开关（紧邻名称）
                    // + 圆形展开按钮（贴右缘）。
                    let head: Vec<ItemDef> = vec![
                        item(
                            208 + i as i32 * 10,
                            Box::new(MetricHeaderWidget { index: i, expanded, lang }),
                            rows_l,
                            tog_l - 8.0,
                            false,
                        ),
                        item(
                            200 + i as i32 * 10,
                            Box::new(ToggleWidget { cfg: metric_cfg[i] }),
                            tog_l,
                            tog_r,
                            false,
                        ),
                        item(
                            207 + i as i32 * 10,
                            Box::new(DisclosureWidget {
                                expanded,
                                action: ButtonAction::ToggleMetricExpand(i),
                            }),
                            widget_r - ICON_BTN as f32,
                            widget_r,
                            false,
                        ),
                    ];
                    rows.push(RowDef { label: "", hint_text: None, hint2: None, divider: i > 0, hint_only: false, items: head });

                    if e > 0.001 {
                        let _overridden = cfg.metric_font_sizes.get(i).copied().flatten().is_some();
                        // 字号滑杆：理想布局中独占整行，数值纯文本、绿填充、白圈滑块。
                        rows.push(RowDef {
                            label: tr("LblFontSize"),
                            hint_text: None,
                            hint2: None,
                            divider: false,
                            hint_only: false,
                            items: vec![font_slider_item(
                                201 + i as i32 * 10,
                                CfgRef::MetricFont(i),
                                ctrl_l,
                                widget_r,
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
                            items: vec![item(
                                206 + i as i32 * 10,
                                Box::new(MetricFontStatusWidget { index: i, lang }),
                                ctrl_l,
                                widget_r,
                                false,
                            )],
                        });
                        // 名称色 / 数值色（色板含「跟随默认」格承担重置语义）。
                        rows.push(row_def(
                            tr("LblNameColor"),
                            Some(tr("HintNameColor")),
                            vec![ctrl_item(203 + i as i32 * 10, Box::new(MetricSwatchesWidget { cfg: CfgRef::MetricLabel(i), auto_cfg: None }))],
                        ));
                        rows.push(row_def(
                            tr("LblValueColor"),
                            Some(tr("HintValueColor")),
                            vec![ctrl_item(205 + i as i32 * 10, Box::new(MetricSwatchesWidget { cfg: CfgRef::MetricValue(i), auto_cfg: Some(CfgRef::MetricValueAuto(i)) }))],
                        ));
                    }
                }
                // 页脚：全部重置 + 即时生效提示。
                rows.push(RowDef {
                    label: "",
                    hint_text: None,
                    hint2: None,
                    divider: false,
                    hint_only: false,
                    items: vec![item(
                        209,
                        Box::new(ButtonWidget {
                            label: tr("LblResetAll").to_string(),
                            action: ButtonAction::ResetAllColors,
                            kind: ButtonKind::Secondary,
                        }),
                        widget_r - 96.0,
                        widget_r,
                        false,
                    )],
                });
                rows.push(hint_row_def(tr("MetricsFoot")));
                groups.push(("", rows, None));
            }
            3 => {
                // ── 窗口：层级与交互 / 位置与外观（报告 §11 w4） ──
                groups.push((
                    tr("GrpInteraction"),
                    vec![
                        // 主句 + 弱化补充句（报告：说明拆为两行 hint）。
                        RowDef {
                            label: tr("LblTopmost"),
                            hint_text: Some(tr("HintAlwaysTop")),
                            hint2: Some(tr("HintAlwaysTop2")),
                            divider: false,
                            hint_only: false,
                            items: vec![toggle_item(244, CfgRef::Topmost)],
                        },
                        row_def(
                            tr("LblClickThrough"),
                            Some(tr("HintClickThrough")),
                            vec![toggle_item(245, CfgRef::ClickThrough)],
                        ),
                    ],
                    None,
                ));
                groups.push((
                    tr("GrpPosLook"),
                    vec![
                        row_def(
                            tr("LblCorner"),
                            Some(tr("HintCorner")),
                            vec![ctrl_item(
                                246,
                                Box::new(seg(
                                    &["SegCornerRT", "SegCornerRB", "SegCornerLT", "SegCornerLB"],
                                    CfgRef::SnapCorner,
                                )),
                            )],
                        ),
                        row_def(
                            tr("LblOpacity"),
                            Some(tr("HintOpacity")),
                            vec![ItemDef {
                                id: 247,
                                kind: Box::new(SliderWidget {
                                    min: 40.0,
                                    max: 100.0,
                                    step: 1.0,
                                    dp: 0,
                                    unit: "%",
                                    cfg: CfgRef::Opacity,
                                    metric_style: false,
                                }),
                                l: ctrl_l,
                                r: widget_r,
                                tall: true,
                            }],
                        ),
                    ],
                    None,
                ));
            }
            4 => {
                // ── 性能：统计范围 / 刷新频率 / 画面（报告 §11 w5） ──
                groups.push((
                    tr("GrpScope"),
                    vec![
                        row_def(
                            tr("LblGpuPrimary"),
                            Some(tr("HintGpuPrimary")),
                            vec![toggle_item(263, CfgRef::GpuPrimaryOnly)],
                        ),
                        row_def(
                            tr("LblGpu3D"),
                            Some(tr("HintGpu3D")),
                            vec![toggle_item(264, CfgRef::Gpu3dOnly)],
                        ),
                    ],
                    None,
                ));
                groups.push((
                    tr("GrpRefresh"),
                    vec![row_def(
                        tr("LblNetLowFreq"),
                        Some(tr("HintNetLowFreq")),
                        vec![toggle_item(265, CfgRef::NetLowFreq)],
                    )],
                    None,
                ));
                groups.push((
                    tr("GrpPicture"),
                    vec![row_def(
                        tr("LblEngine"),
                        Some(tr("HintRenderMode")),
                        vec![ctrl_item(266, Box::new(seg(&["SegSmooth", "SegFine"], CfgRef::RenderEngine)))],
                    )],
                    None,
                ));
            }
            _ => {
                // ── 关于（报告 §11 w6）：头部 / 动作 / 链接，版权脚注 ──
                groups.push((
                    "",
                    vec![
                        RowDef {
                            label: "",
                            hint_text: None,
                            hint2: None,
                            divider: false,
                            hint_only: false,
                            items: vec![item(270, Box::new(AboutHeaderWidget { lang }), rows_l, widget_r, false)],
                        },
                        RowDef {
                            label: "",
                            hint_text: None,
                            hint2: None,
                            divider: false,
                            hint_only: false,
                            items: vec![
                                item(
                                    271,
                                    Box::new(ButtonWidget {
                                        label: tr("ActCheckUpdate").to_string(),
                                        action: ButtonAction::CheckUpdate,
                                        kind: ButtonKind::Secondary,
                                    }),
                                    rows_l,
                                    rows_l + 110.0,
                                    false,
                                ),
                                item(
                                    272,
                                    Box::new(ButtonWidget {
                                        label: tr("ActCopyVersion").to_string(),
                                        action: ButtonAction::CopyVersion,
                                        kind: ButtonKind::Secondary,
                                    }),
                                    rows_l + 118.0,
                                    rows_l + 228.0,
                                    false,
                                ),
                            ],
                        },
                        hint_row_def(tr("AboutLinks")),
                    ],
                    page_footnote(5, lang),
                ));
            }
        }

        // ── 组装：页标题块 + 分组卡（浮点逻辑 y 游标） ──
        let mut y: f32 =
            CONTENT_PAD_TOP as f32 + (TITLE_H + TITLE_SUB_GAP + SUB_H + SUB_CARD_GAP) as f32;
        for (caption, rows, foot) in groups {
            // 分组标题带（画在卡顶上方，见 paint_page）。
            y += (CAPTION_H + CAPTION_GAP) as f32;
            let card_top = y;
            let rows_top = card_top + CARD_PAD as f32;
            let mut ry: f32 = 0.0;
            let mut out_rows: Vec<Row> = Vec::with_capacity(rows.len());
            for rd in rows {
                let rh = rd.height();
                let band_top = rows_top + ry;
                let mut idxs = Vec::with_capacity(rd.items.len());
                for it in rd.items {
                    let (wtop, wbot) = if it.tall {
                        (sf(dpi, band_top), sf(dpi, band_top + rh))
                    } else {
                        let t = sf(dpi, band_top + (ROW_H - WIDGET_H) as f32 / 2.0);
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
                    divider: rd.divider,
                    hint: rd.hint_only,
                    hint_text: rd.hint_text,
                    hint2: rd.hint2,
                    y_off: sf(dpi, ry),
                });
                ry += rh;
            }
            let rows_h = sf(dpi, ry);
            let foot_h = if foot.is_some() { s(dpi, FOOTNOTE_H) } else { 0 };
            let card_bottom = rows_top + rows_h as f32 + 2.0 * CARD_PAD as f32 + foot_h as f32;
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
            y = card_bottom + CARD_GAP as f32;
        }
        page.h = sf(dpi, y - CARD_GAP as f32 + BOTTOM_MARGIN as f32);
    }
}
