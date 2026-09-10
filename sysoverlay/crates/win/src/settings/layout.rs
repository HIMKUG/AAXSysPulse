//! 布局：常量、DPI 缩放与**纯函数**页面构建。
//!
//! `build_pages(cfg, lang, dpi, pages)` 无任何外部状态——同样的输入必然得到
//! 同样的控件树，可单测、可缓存。旧实现里的 `PageLayout` 半遗弃抽象在此收编
//! （其 `rows_top/widget_l/widget_r` 语义并入本函数局部变量）。

use sysoverlay_core::config::Config;
use sysoverlay_core::types::Lang;

use super::cfg_ref::CfgRef;
use super::state::{Card, Page, Row, Widget};
use super::widget::{
    ButtonAction, ButtonWidget, MetricSwatchesWidget, SegmentedWidget, SliderWidget, ToggleWidget,
};

// ── 布局常量（逻辑像素，按 DPI 缩放） ────────────────────

pub const CLIENT_W: i32 = 420;
pub const MARGIN_X: i32 = 16;
pub const TOP_MARGIN: i32 = 14;
pub const BOTTOM_MARGIN: i32 = 16;
pub const NAV_H: i32 = 32;
pub const NAV_GAP: i32 = 8;
pub const CARD_PAD: i32 = 10;
pub const CAPTION_H: i32 = 14;
pub const CAPTION_GAP: i32 = 6;
pub const ROW_H: i32 = 28;
pub const WIDGET_H: i32 = 24;
pub const CARD_RADIUS: i32 = 10;
pub const LABEL_W: i32 = 120;
pub const TOGGLE_W: i32 = 40;
pub const VALUEBOX_W: i32 = 64;
pub const FOOTNOTE_H: i32 = 60;

/// 摇杆"字面无上限"：参与增量调节的数值项以该值作为 max。
/// f32::MAX/2 避开求值精度损失的极值即可，UI 上推到尽头仍能继续加。
/// 真正的系统级边界是 `SetWindowPos` 的 i32 尺寸上限（32767），与应用无关。
pub const OPEN_MAX: f32 = f32::MAX / 2.0;

/// DPI 缩放（逻辑像素 → 物理像素）。
pub fn s(dpi: u32, v: i32) -> i32 {
    v * dpi as i32 / 96
}

// ── 页面元信息 ───────────────────────────────────────────

/// 各页行数。**窗口页 = 9**：旧实现此处写 6，而「窗口」页实际布局 9 行
/// （性能档轮追加的 GPU 主适配器/GPU 3D/网络低频三个 Toggle 溢出卡片、
/// 且窗口高度按 6 行计算裁掉底部）——本轮修正。
pub fn page_rows(page: usize) -> i32 {
    match page {
        0 => 3,  // 常规：语言/自启/调试
        1 => 9,  // 布局：风格/字号/项距/名称值距/内存单位/字重/网络模式/阴影/描边
        2 => 16, // 颜色：5 指标 × 3 行 + 一键重置
        3 => 4,  // 动效
        4 => 9,  // 窗口：尺寸/宽/高/置顶/刷新率/GPU主/GPU3D/网络低频/引擎
        _ => 3,  // 关于
    }
}

/// 某页有没有脚注（目前只有「常规」页有）。
pub fn page_has_footnote(p: usize) -> bool {
    p == 0
}

pub fn page_caption(p: usize, lang: Lang) -> &'static str {
    let tr = |k: &'static str| crate::i18n::t(lang, k);
    match p {
        0 => tr("PgGeneral"),
        1 => tr("PgLayout"),
        2 => tr("PgMetrics"),
        3 => tr("PgMotion"),
        4 => tr("PgWindow"),
        _ => tr("PgAbout"),
    }
}

pub fn page_footnote(p: usize, lang: Lang) -> Option<&'static str> {
    if p == 0 {
        return Some(crate::i18n::t(lang, "FootnoteDebug"));
    }
    None
}

/// 某页内容区高度（含卡片四周 + 标题 + 脚注）。
pub fn content_height(dpi: u32, page: usize) -> i32 {
    let rows = page_rows(page);
    let foot = if page_has_footnote(page) { FOOTNOTE_H } else { 0 };
    s(dpi, TOP_MARGIN + BOTTOM_MARGIN)
        + s(dpi, CAPTION_H + CAPTION_GAP)
        + 2 * s(dpi, CARD_PAD)
        + s(dpi, rows * ROW_H)
        + s(dpi, foot)
}

/// 窗口总高 = 标题栏 + 导航 + 最高内容页 + 底部余量。
pub fn window_total_h(dpi: u32) -> i32 {
    s(dpi, crate::titlebar::TB_H)
        + s(dpi, NAV_H + NAV_GAP)
        + (0..6).map(|p| content_height(dpi, p)).max().unwrap_or(0)
        + s(dpi, 4)
}

/// 内容区顶部相对客户区顶部的偏移——**单一事实源**。
///
/// `Widget.rect` 存内容坐标（相对内容区顶部）；命中测试与正文绘制统一走它。
pub fn content_top_y(d: u32) -> i32 {
    s(d, crate::titlebar::TB_H) + s(d, NAV_H + NAV_GAP)
}

// ── 页面构建（纯函数） ───────────────────────────────────

/// 依据 (cfg, lang, dpi) 重建全部 6 页布局。
pub fn build_pages(pages: &mut [Page; 6], _cfg: &Config, lang: Lang, dpi: u32) {
    let one = |v: i32| s(dpi, v);
    let card_l = one(MARGIN_X);
    let card_r = one(CLIENT_W) - one(MARGIN_X);
    let widget_r = card_r - one(8);
    let widget_l = card_l + one(CARD_PAD) + one(LABEL_W);
    let tr = |k: &'static str| crate::i18n::t(lang, k);

    for (p, page) in pages.iter_mut().enumerate() {
        page.cards.clear();
        page.widgets.clear();
        let rows = page_rows(p);
        let fnote = if page_has_footnote(p) { FOOTNOTE_H } else { 0 };
        let rect = windows::Win32::Foundation::RECT {
            left: card_l,
            top: one(TOP_MARGIN) + one(CAPTION_H) + one(CAPTION_GAP),
            right: card_r,
            bottom: one(TOP_MARGIN) + one(CAPTION_H) + one(CAPTION_GAP)
                + 2 * one(CARD_PAD)
                + one(rows * ROW_H)
                + one(fnote),
        };
        page.h = one(TOP_MARGIN + BOTTOM_MARGIN + CAPTION_H + CAPTION_GAP)
            + 2 * one(CARD_PAD)
            + one(rows * ROW_H)
            + one(fnote);

        page.cards.push(Card {
            caption: page_caption(p, lang),
            rect,
            rows: Vec::new(),
            footnote: page_footnote(p, lang),
        });
        let rows_top = rect.top + one(CARD_PAD);

        // 行布局：一次调用 = 一行（label + 若干控件）。
        // 行距 = 逻辑像素 `row_i * ROW_H` 经 DPI 缩放（与旧版逐行对齐）。
        let add_row = |page: &mut Page, row_i: i32, label: &'static str,
                     items: Vec<(i32, Box<dyn super::state::IWidget>, (i32, i32))>| {
            let row_top = rows_top + one(row_i * ROW_H);
            let mut idxs = Vec::with_capacity(items.len());
            for (id, kind, (l, r)) in items {
                let wrect = windows::Win32::Foundation::RECT {
                    left: l,
                    top: row_top + (one(ROW_H) - one(WIDGET_H)) / 2,
                    right: r,
                    bottom: row_top + (one(ROW_H) - one(WIDGET_H)) / 2 + one(WIDGET_H),
                };
                page.widgets.push(Widget { id, kind, rect: wrect });
                idxs.push(page.widgets.len() - 1);
            }
            if let Some(card) = page.cards.last_mut() {
                card.rows.push(Row { label, widgets: idxs });
            }
        };

        let full = (widget_l, widget_r);
        let rate_labels: Vec<String> = vec![
            tr("SegAuto").to_string(),
            "10".into(),
            "5".into(),
            "1".into(),
            "0.5".into(),
            "0.2".into(),
        ];

        match p {
            0 => {
                add_row(page, 0, tr("CardLanguage"), vec![(
                    260,
                    Box::new(SegmentedWidget {
                        labels: [tr("SegEn").to_string(), tr("SegZh").to_string()].into(),
                        cfg: CfgRef::Lang,
                    }),
                    (widget_l, widget_l + one(120)),
                )]);
                add_row(page, 1, tr("LblAutostart"), vec![(
                    261, Box::new(ToggleWidget { cfg: CfgRef::AutoStart }),
                    (widget_r - one(TOGGLE_W), widget_r),
                )]);
                add_row(page, 2, tr("LblDebug"), vec![(
                    262, Box::new(ToggleWidget { cfg: CfgRef::DebugLog }),
                    (widget_r - one(TOGGLE_W), widget_r),
                )]);
            }
            1 => {
                add_row(page, 0, tr("LblStyle"), vec![(
                    150,
                    Box::new(SegmentedWidget {
                        labels: [tr("SegHorizontal").to_string(), tr("SegVertical").to_string()].into(),
                        cfg: CfgRef::Style,
                    }),
                    full,
                )]);
                add_row(page, 1, tr("LblFontSize"), vec![(
                    152, Box::new(SliderWidget { min: 8.0, max: OPEN_MAX, step: 0.5, dp: 1, cfg: CfgRef::FontSize, joystick: true }),
                    full,
                )]);
                add_row(page, 2, tr("LblGapMetric"), vec![(
                    153, Box::new(SliderWidget { min: 0.0, max: OPEN_MAX, step: 1.0, dp: 0, cfg: CfgRef::MetricGap, joystick: false }),
                    full,
                )]);
                add_row(page, 3, tr("LblGapLabel"), vec![(
                    154, Box::new(SliderWidget { min: 0.0, max: OPEN_MAX, step: 1.0, dp: 0, cfg: CfgRef::LabelValueGap, joystick: false }),
                    full,
                )]);
                add_row(page, 4, tr("LblMemUnit"), vec![(
                    155,
                    Box::new(SegmentedWidget {
                        labels: [tr("SegPercent").to_string(), tr("SegGb").to_string()].into(),
                        cfg: CfgRef::MemUnit,
                    }),
                    (widget_l, widget_l + one(150)),
                )]);
                add_row(page, 5, tr("LblFontWeight"), vec![(
                    168,
                    Box::new(SegmentedWidget {
                        // ⚠️ 顺序必须与 CfgRef::FontWeight 的索引映射一致：
                        // 0=细 / 1=中 / 2=粗（默认粗 = 位图字体原样观感）。
                        labels: [tr("SegThin").to_string(), tr("SegMedium").to_string(), tr("SegBold").to_string()].into(),
                        cfg: CfgRef::FontWeight,
                    }),
                    full,
                )]);
                add_row(page, 6, tr("LblNetMode"), vec![(
                    167,
                    Box::new(SegmentedWidget {
                        // ⚠️ 顺序必须与 CfgRef::NetMode 的索引映射一致：
                        // 0=双向(Both) / 1=下载(Down) / 2=上传(Up)。
                        // 此前写成 [下载, 上传, 双向]（下标 0/1/2），
                        // 与映射错位 → 点「下载」实际写成「双向」、高亮也对不上，
                        // 正是 Style 分段同类的静默错配 bug。
                        labels: [tr("SegBoth").to_string(), tr("SegDown").to_string(), tr("SegUp").to_string()].into(),
                        cfg: CfgRef::NetMode,
                    }),
                    full,
                )]);
                add_row(page, 7, tr("LblShadow"), vec![(
                    158, Box::new(ToggleWidget { cfg: CfgRef::TextShadow }),
                    (widget_r - one(TOGGLE_W), widget_r),
                )]);
                add_row(page, 8, tr("LblOutline"), vec![(
                    159, Box::new(ToggleWidget { cfg: CfgRef::TextOutline }),
                    (widget_r - one(TOGGLE_W), widget_r),
                )]);
            }
            2 => {
                // 颜色：5 指标 × (开关 + 字号 / 名称色 / 数值色)
                let metric_cfg: [CfgRef; 5] = [
                    CfgRef::ShowCpu,
                    CfgRef::ShowMem,
                    CfgRef::ShowGpu,
                    CfgRef::ShowNet,
                    CfgRef::PowerEnabled,
                ];
                let labels: [&str; 5] = ["LblCpu", "LblMem", "LblGpu", "LblNet", "LblPower"];
                for (i, lbl_key) in labels.iter().enumerate() {
                    let base = i as i32 * 3;
                    // ⚠️ 每指标独占 id 段（200 + i×10 … 206 + i×10），
                    // 不再用 200+base / 201+base+1 之类的混编——此前三处 id 编号
                    // 交错（CPU 数值色 swatch=203 与 内存 toggle=203 撞号；`280+i`
                    // 又越出 29x 段），虽不影响下标命中，但混淆诊断（探针日志按 id
                    // 甄别控件），且未来挂 DIAG 等按 id 操作时会错。
                    let id_toggle = 200 + i as i32 * 10;
                    let id_slider = 201 + i as i32 * 10;
                    let id_slider_reset = 202 + i as i32 * 10;
                    let id_name_swatch = 203 + i as i32 * 10;
                    let id_name_reset = 204 + i as i32 * 10;
                    let id_value_swatch = 205 + i as i32 * 10;
                    let id_value_reset = 206 + i as i32 * 10;
                    add_row(page, base, tr(lbl_key), vec![
                        (
                            id_toggle,
                            Box::new(ToggleWidget { cfg: metric_cfg[i] }),
                            (widget_l, widget_l + one(TOGGLE_W)),
                        ),
                        (
                            id_slider,
                            Box::new(SliderWidget {
                                min: 8.0, max: OPEN_MAX, step: 0.5, dp: 1,
                                cfg: CfgRef::MetricFont(i),
                                joystick: true,
                            }),
                            // 滑杆缩短给右侧留出"字号重置"按钮
                            (widget_l + one(TOGGLE_W + 8), widget_r - one(68)),
                        ),
                        (
                            id_slider_reset,
                            Box::new(ButtonWidget {
                                label: tr("BtnReset").to_string(),
                                action: ButtonAction::ResetFont(i),
                            }),
                            (widget_r - one(68), widget_r),
                        ),
                    ]);
                    add_row(page, base + 1, tr("LblNameColor"), vec![
                        (
                            id_name_swatch,
                            Box::new(MetricSwatchesWidget { cfg: CfgRef::MetricLabel(i) }),
                            (widget_l, widget_r - one(76)),
                        ),
                        (
                            id_name_reset,
                            Box::new(ButtonWidget {
                                label: tr("BtnReset").to_string(),
                                action: ButtonAction::ResetColor(CfgRef::MetricLabel(i)),
                            }),
                            (widget_r - one(68), widget_r),
                        ),
                    ]);
                    add_row(page, base + 2, tr("LblValueColor"), vec![
                        (
                            id_value_swatch,
                            Box::new(MetricSwatchesWidget { cfg: CfgRef::MetricValue(i) }),
                            (widget_l, widget_r - one(76)),
                        ),
                        (
                            id_value_reset,
                            Box::new(ButtonWidget {
                                label: tr("BtnReset").to_string(),
                                action: ButtonAction::ResetColor(CfgRef::MetricValue(i)),
                            }),
                            (widget_r - one(68), widget_r),
                        ),
                    ]);
                }
                add_row(page, 15, tr("LblResetAll"), vec![(
                    207, Box::new(ButtonWidget {
                        label: tr("LblResetAll").to_string(),
                        action: ButtonAction::ResetAllColors,
                    }),
                    full,
                )]);
            }
            3 => {
                add_row(page, 0, tr("LblHoverOpacity"), vec![(
                    230, Box::new(SliderWidget { min: 0.0, max: 100.0, step: 1.0, dp: 0, cfg: CfgRef::HoverOpacity, joystick: true }),
                    full,
                )]);
                add_row(page, 1, tr("LblNormalOpacity"), vec![(
                    231, Box::new(SliderWidget { min: 0.0, max: 100.0, step: 1.0, dp: 0, cfg: CfgRef::NormalOpacity, joystick: true }),
                    full,
                )]);
                add_row(page, 2, tr("LblAnimMs"), vec![(
                    232, Box::new(SliderWidget { min: 100.0, max: 2000.0, step: 50.0, dp: 0, cfg: CfgRef::AnimMs, joystick: true }),
                    full,
                )]);
                add_row(page, 3, tr("LblHover"), vec![(
                    233, Box::new(ToggleWidget { cfg: CfgRef::Hover }),
                    (widget_r - one(TOGGLE_W), widget_r),
                )]);
            }
            4 => {
                add_row(page, 0, tr("LblOverlaySize"), vec![(
                    240, Box::new(ToggleWidget { cfg: CfgRef::LockSize }),
                    (widget_r - one(TOGGLE_W), widget_r),
                )]);
                add_row(page, 1, tr("LblWidth"), vec![(
                    241, Box::new(SliderWidget { min: 60.0, max: OPEN_MAX, step: 10.0, dp: 0, cfg: CfgRef::WinW, joystick: true }),
                    full,
                )]);
                add_row(page, 2, tr("LblHeight"), vec![(
                    242, Box::new(SliderWidget { min: 40.0, max: OPEN_MAX, step: 10.0, dp: 0, cfg: CfgRef::WinH, joystick: true }),
                    full,
                )]);
                add_row(page, 3, tr("LblTopmost"), vec![(
                    244, Box::new(ToggleWidget { cfg: CfgRef::Topmost }),
                    (widget_r - one(TOGGLE_W), widget_r),
                )]);
                add_row(page, 4, tr("LblRate"), vec![(
                    245,
                    Box::new(SegmentedWidget { labels: rate_labels.into_boxed_slice(), cfg: CfgRef::Rate }),
                    full,
                )]);
                add_row(page, 5, tr("LblGpuPrimary"), vec![(
                    263, Box::new(ToggleWidget { cfg: CfgRef::GpuPrimaryOnly }),
                    (widget_r - one(TOGGLE_W), widget_r),
                )]);
                add_row(page, 6, tr("LblGpu3D"), vec![(
                    264, Box::new(ToggleWidget { cfg: CfgRef::Gpu3dOnly }),
                    (widget_r - one(TOGGLE_W), widget_r),
                )]);
                add_row(page, 7, tr("LblNetLowFreq"), vec![(
                    265, Box::new(ToggleWidget { cfg: CfgRef::NetLowFreq }),
                    (widget_r - one(TOGGLE_W), widget_r),
                )]);
                add_row(page, 8, tr("LblEngine"), vec![(
                    266,
                    Box::new(SegmentedWidget {
                        labels: vec![tr("SegPerf").to_string(), tr("SegQuality").to_string()].into(),
                        cfg: CfgRef::RenderEngine,
                    }),
                    full,
                )]);
            }
            _ => {
                // 关于：纯文本行（无控件）
                add_row(page, 0, tr("AboutName"), vec![]);
                add_row(page, 1, tr("AboutVer"), vec![]);
                add_row(page, 2, "", vec![]);
            }
        }
    }
}
