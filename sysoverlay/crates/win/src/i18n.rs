//! 全界面双语（En / Zh）文案表。
//!
//! 覆盖三处界面文本：悬浮窗主界面标签（CPU/内存/…）、托盘菜单、设置面板。
//! 配置键 `lang` 切换；文本源都在这里，便于统一维护。

use sysoverlay_core::types::Lang;
use std::sync::atomic::{AtomicU8, Ordering};

/// 全局当前语言（标题栏「解锁」等窗口外文案用；设置页切换语言时同步写入）。
static GLOBAL_LANG: AtomicU8 = AtomicU8::new(0); // 0 = En, 1 = Zh

pub fn set_global_lang(l: Lang) {
    GLOBAL_LANG.store(if l == Lang::Zh { 1 } else { 0 }, Ordering::Relaxed);
}

pub fn global_lang() -> Lang {
    if GLOBAL_LANG.load(Ordering::Relaxed) == 1 {
        Lang::Zh
    } else {
        Lang::En
    }
}

/// 翻译：按当前语言取文案。英文为默认（与 C 版一致）。
pub fn t<'a>(lang: Lang, key: &'a str) -> &'a str {
    let zh = lang == Lang::Zh;
    match (key, zh) {
        // ── 悬浮窗主界面标签 ────────────────────────────
        ("RowCpu", false) => "CPU",
        ("RowCpu", true) => "CPU",
        ("RowMem", false) => "Memory",
        ("RowMem", true) => "内存",
        ("RowGpu", false) => "GPU",
        ("RowGpu", true) => "GPU",
        ("RowNet", false) => "Net",
        ("RowNet", true) => "网络",
        ("RowPower", false) => "Power",
        ("RowPower", true) => "功耗",

        // ── 托盘菜单 ────────────────────────────────────
        ("MenuSettings", false) => "Settings",
        ("MenuSettings", true) => "设置",
        ("MenuCharts", false) => "Charts",
        ("MenuCharts", true) => "图表",
        ("MenuProcesses", false) => "Processes",
        ("MenuProcesses", true) => "进程",
        ("MenuQuit", false) => "Quit",
        ("MenuQuit", true) => "退出",

        // ── 设置面板：卡片标题 ──────────────────────────
        ("CardMonitoring", false) => "MONITORING",
        ("CardMonitoring", true) => "监控项",
        ("CardAppearance", false) => "APPEARANCE",
        ("CardAppearance", true) => "外观",
        ("CardRefresh", false) => "REFRESH RATE",
        ("CardRefresh", true) => "刷新率",
        ("CardWindow", false) => "WINDOW",
        ("CardWindow", true) => "窗口",
        ("CardAdvanced", false) => "ADVANCED",
        ("CardAdvanced", true) => "高级",
        ("CardLanguage", false) => "LANGUAGE",
        ("CardLanguage", true) => "语言",

        // ── 设置面板：行标签 ────────────────────────────
        ("LblCpu", false) => "CPU",
        ("LblCpu", true) => "CPU",
        ("LblMem", false) => "Memory",
        ("LblMem", true) => "内存",
        ("LblGpu", false) => "GPU",
        ("LblGpu", true) => "GPU",
        ("LblNet", false) => "Network",
        ("LblNet", true) => "网络",
        ("LblPower", false) => "Power",
        ("LblPower", true) => "功耗",
        ("LblMemUnit", false) => "Memory Unit",
        ("LblMemUnit", true) => "内存单位",
        ("LblShadow", false) => "Text Shadow",
        ("LblShadow", true) => "文字阴影",
        ("LblOutline", false) => "Text Outline",
        ("LblOutline", true) => "文字描边",
        ("LblTopmost", false) => "Always on Top",
        ("LblTopmost", true) => "窗口置顶",
        ("LblFont", false) => "Text Size",
        ("LblFont", true) => "字号",
        ("LblStyle", false) => "Style",
        ("LblStyle", true) => "样式",
        ("LblShape", false) => "Shape",
        ("LblShape", true) => "形状",
        ("LblOpacity", false) => "Opacity",
        ("LblOpacity", true) => "不透明度",
        ("LblHover", false) => "Hover Reveal",
        ("LblHover", true) => "悬停显示",
        ("LblLayout", false) => "Layout",
        ("LblLayout", true) => "布局",
        ("LblBg", false) => "Background",
        ("LblBg", true) => "背景色",
        ("LblFg", false) => "Foreground",
        ("LblFg", true) => "前景色",
        ("LblOverlaySize", false) => "Overlay Size",
        ("LblOverlaySize", true) => "悬浮窗尺寸",
        ("LblAutostart", false) => "Launch at Login",
        ("LblAutostart", true) => "开机自启",
        ("LblDebug", false) => "Debug Log",
        ("LblDebug", true) => "调试日志",
        ("LblFullscreenHide", false) => "Hide in Fullscreen",
        ("LblFullscreenHide", true) => "全屏时隐藏",

        // ── 设置面板：分段控件 / 按钮文案 ────────────────
        ("SegPercent", false) => "Percent",
        ("SegPercent", true) => "百分比",
        ("SegGb", false) => "GB",
        ("SegGb", true) => "GB",
        ("SegCards", false) => "Cards",
        ("SegCards", true) => "卡片",
        ("SegStrip", false) => "Strip",
        ("SegStrip", true) => "条形",
        ("SegRound", false) => "Round",
        ("SegRound", true) => "圆角",
        ("SegSquare", false) => "Square",
        ("SegSquare", true) => "方形",
        ("SegNone", false) => "None",
        ("SegNone", true) => "无",
        ("SegRow", false) => "Row",
        ("SegRow", true) => "行",
        ("SegCol", false) => "Col",
        ("SegCol", true) => "列",
        ("SegSplit", false) => "Split",
        ("SegSplit", true) => "分栏",
        ("SegEn", false) => "EN",
        ("SegEn", true) => "EN",
        ("SegZh", false) => "中文",
        ("SegZh", true) => "中文",
        ("SegAuto", false) => "Auto",
        ("SegAuto", true) => "自动",
        ("BtnDone", false) => "Done",
        ("BtnDone", true) => "完成",
        ("BtnResize", false) => "Resize",
        ("BtnResize", true) => "调整",

        // ── 设置面板：脚注 ──────────────────────────────
        (
            "FootnoteRefresh",
            false,
        ) => "Auto adapts the refresh rate to how fast system counters actually update, and eases off automatically when sampling gets expensive.",
        (
            "FootnoteRefresh",
            true,
        ) => "自动根据系统计数器的实际更新速度调整刷新率，并在采样开销增大时自动降频。",
        (
            "FootnoteDebug",
            false,
        ) => "Writes raw sampler output to %LOCALAPPDATA%\\SysOverlayC\\probe.log. Turn off when not diagnosing.",
        (
            "FootnoteDebug",
            true,
        ) => "将采样器的原始输出写入 %LOCALAPPDATA%\\SysOverlayC\\probe.log，非诊断时请关闭。",
        (
            "FootnoteFullscreenHide",
            false,
        ) => "Hides the overlay when a fullscreen app (e.g. game) is detected, restoring AFMF frame generation that overlays otherwise break.",
        (
            "FootnoteFullscreenHide",
            true,
        ) => "检测到全屏应用（如游戏）时自动隐藏悬浮窗，恢复被叠加层破坏的 AFMF 帧生成。",

        // ── 设置面板：窗口标题 ──────────────────────────
        ("WinTitle", false) => "SysOverlay Settings",
        ("WinTitle", true) => "SysOverlay 设置",

        // ── 标题栏按钮 ─────────────────────────────────
        ("TbUnlock", false) => "Unlock",
        ("TbUnlock", true) => "解锁",

        // ── 设置页分页导航（6 类） ─────────────────────
        ("PgGeneral", false) => "General",
        ("PgGeneral", true) => "常规",
        ("PgLayout", false) => "Layout",
        ("PgLayout", true) => "布局",
        ("PgColors", false) => "Colors",
        ("PgColors", true) => "颜色",
        ("PgMotion", false) => "Motion",
        ("PgMotion", true) => "动效",
        ("PgWindow", false) => "Window",
        ("PgWindow", true) => "窗口",
        ("PgAbout", false) => "About",
        ("PgAbout", true) => "关于",

        // ── 新控件标签 ─────────────────────────────────
        ("LblFontSize", false) => "Font Size",
        ("LblFontSize", true) => "字号",
        ("LblGapMetric", false) => "Metric Gap",
        ("LblGapMetric", true) => "指标间距",
        ("LblGapLabel", false) => "Label Gap",
        ("LblGapLabel", true) => "名值间距",
        ("LblWidth", false) => "Width",
        ("LblWidth", true) => "宽度",
        ("LblHeight", false) => "Height",
        ("LblHeight", true) => "高度",
        ("LblHoverOpacity", false) => "Hover Opacity",
        ("LblHoverOpacity", true) => "悬停不透明度",
        ("LblNormalOpacity", false) => "Idle Opacity",
        ("LblNormalOpacity", true) => "常态不透明度",
        ("LblAnimMs", false) => "Transition S",
        ("LblAnimMs", true) => "过渡时长",
        ("LblNameColor", false) => "Name Color",
        ("LblNameColor", true) => "名称色",
        ("LblValueColor", false) => "Value Color",
        ("LblValueColor", true) => "数值色",
        ("LblFollow", false) => "Default",
        ("LblFollow", true) => "默认",
        ("SegHorizontal", false) => "Horizontal",
        ("SegHorizontal", true) => "横版",
        ("SegVertical", false) => "Vertical",
        ("SegVertical", true) => "竖版",
        ("LblPin", false) => "Pin Window",
        ("LblPin", true) => "固定窗口",
        ("LblRate", false) => "Refresh Rate",
        ("LblRate", true) => "刷新率",

        // ── 关于页 ─────────────────────────────────────
        ("AboutName", false) => "SysOverlay",
        ("AboutName", true) => "SysOverlay",
        ("AboutDesc", false) => "Hardware monitoring overlay with custom layout, per-metric colors and animated transitions.",
        ("AboutDesc", true) => "硬件监控悬浮条：自定义布局、逐指标配色与非线性的过渡动画。",
        ("AboutVer", false) => {
            const S: &str = concat!("Version ", env!("CARGO_PKG_VERSION"));
            S
        }
        ("AboutVer", true) => {
            const S: &str = concat!("版本 ", env!("CARGO_PKG_VERSION"));
            S
        }

        _ => return key,
    }
}