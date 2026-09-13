//! 全界面双语（En / Zh）文案表。
//!
//! 覆盖三处界面文本：悬浮窗主界面标签（CPU/内存/…）、托盘菜单、设置面板。
//! 配置键 `lang` 切换；文本源都在这里，便于统一维护。
//!
//! 数据布局：每行是一个 `(key, en, zh)` 三元组，按 key 字典序升序排列，
//! 供 `lookup` 做 const fn 二分查找。新增一种语言只需：
//!   1) 给每个三元组再加一列 `&str`；2) 在 `t()` 里按需选择该列。
//! 编译期 `CHECK_TABLE` 会断言「无重复 key、且按 key 有序」，从根上消除
//! 原先 en/zh 分 arm 编写时两语言漏配对的隐患。

use aaxsyspulse_core::types::Lang;
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

/// 翻译表：(key, en, zh)。按 key 字典序升序，供二分查找。
/// 新增语言：给每个条目加一列 `&str`，并在 `t()` 中选取。
const TABLE: &[(&str, &str, &str)] = &[
    ("© 2026 AAX SysPulse", "© 2026 AAX SysPulse", "AboutCopyright"),
    ("Shows live CPU, memory, GPU, network and power usage on your desktop.", "在桌面实时显示 CPU、内存、显卡、网络与整机功耗。", "AboutDesc"),
    ("Developer", "开发者", "AboutDeveloper"),
    ("HIMKUG", "HIMKUG", "AboutDeveloperName"),
    ("AAX SysPulse", "AAX SysPulse", "AboutName"),
    (concat!("Version ", env!("CARGO_PKG_VERSION")), concat!("版本 ", env!("CARGO_PKG_VERSION")), "AboutVer"),
    ("Bilibili →", "哔哩哔哩主页 →", "ActBilibili"),
    ("Open log folder →", "打开日志文件位置 →", "ActOpenLogFolder"),
    ("Follow global", "恢复跟随全局", "ActRestoreGlobal"),
    ("Could not update the startup entry (blocked by policy or permission). The switch was reverted.", "无法写入开机启动项（被组策略或权限阻止），开关已自动还原。", "AutoStartFailed"),
    ("Autostart enabled via the registry Run key. Because the app is not running as administrator, it will not be elevated at login (the Power row will show --). Enable it again while running as administrator to use a scheduled task with highest privileges.", "已开启自启（注册表 Run 方式）。当前未以管理员身份运行，登录时不会提权，功耗行将显示 --。以管理员身份运行后再开关一次，可改用「计划任务 + 最高权限」方式。", "AutoStartRunFallback"),
    ("Collapse", "收起", "BtnCollapse"),
    ("Done", "完成", "BtnDone"),
    ("Expand", "展开", "BtnExpand"),
    ("Reset", "重置", "BtnReset"),
    ("Resize", "调整", "BtnResize"),
    ("ADVANCED", "高级", "CardAdvanced"),
    ("APPEARANCE", "外观", "CardAppearance"),
    ("LANGUAGE", "语言", "CardLanguage"),
    ("MONITORING", "监控项", "CardMonitoring"),
    ("REFRESH RATE", "刷新率", "CardRefresh"),
    ("WINDOW", "窗口", "CardWindow"),
    ("AAX SysPulse", "AAX SysPulse", "ElevateCaption"),
    ("Power monitoring needs administrator rights (to read the CPU power sensor).\n\n\
             Restart AAX SysPulse as administrator?\n\
             (Choosing \"No\" turns the Power row off — every other metric is unaffected.)", "整机功耗显示需要管理员权限（读取 CPU 功耗传感器）。\n\n\
             是否以管理员身份重新启动 AAX SysPulse？\n\
             （选择「否」将关闭功耗行，不影响其他显示）", "ElevateText"),
    ("Writes runtime state for troubleshooting. It keeps writing to disk — enable only when diagnosing.", "记录运行状态用于排查问题，会持续写入磁盘，建议仅在需要时开启。", "FootnoteDebug"),
    ("Auto adapts the refresh rate to how fast system counters actually update, and eases off automatically when sampling gets expensive.", "自动根据系统计数器的实际更新速度调整刷新率，并在采样开销增大时自动降频。", "FootnoteRefresh"),
    ("Keeps the overlay above every other window. It does not steal mouse input.", "让悬浮窗始终显示在其他窗口之上；默认不抢占鼠标点击。", "FootnoteTopmost"),
    ("Arrangement", "排列", "GrpArrange"),
    ("Data Format", "数据格式", "GrpDataFormat"),
    ("Diagnostics", "诊断", "GrpDiagnostics"),
    ("Display", "显示", "GrpDisplay"),
    ("Layer & Input", "层级与交互", "GrpInteraction"),
    ("Rendering", "画面", "GrpPicture"),
    ("Appearance", "外观", "GrpPosLook"),
    ("Refresh Rate", "刷新频率", "GrpRefresh"),
    ("Sampling Scope", "统计范围", "GrpScope"),
    ("Startup", "启动", "GrpStartup"),
    ("Text Effects", "文字效果", "GrpTextEffects"),
    ("Keeps the overlay above every other window.", "让悬浮条始终显示在其他窗口之上。", "HintAlwaysTop"),
    ("It never steals mouse clicks; windows below stay interactive.", "默认不抢占鼠标点击，可正常操作下方的窗口。", "HintAlwaysTop2"),
    ("Follows the system theme automatically.", "跟随系统时将随系统主题自动切换", "HintAppearance"),
    ("Launch the overlay at login.", "登录系统后自动运行悬浮条", "HintAutostart"),
    ("On: the whole overlay ignores the mouse, even the text. Off (default): transparent areas always let clicks through — only the text/shadow pixels are hit, so drag the bar by its text.", "开启：整窗（含文字）都不接收鼠标；关闭（默认）：透明区域始终穿透点击，只有文字/描边处可命中，拖动窗口请按在文字上", "HintClickThrough"),
    ("Which screen corner to snap to.", "贴靠屏幕的哪个角", "HintCorner"),
    ("Writes runtime state for troubleshooting. It keeps writing to disk — enable only when diagnosing.", "记录运行状态用于排查问题，会持续写入磁盘，建议仅在需要时开启。", "HintDebugLog"),
    ("Drag the icon/name area of a card to reorder the items.", "按住卡片左侧的图标与名称区域上下拖动，即可调整监控项顺序。", "HintDragReorder"),
    ("Affects the overlay text size, range 8–256 px.", "影响悬浮窗中的文字大小，范围 8–256 px。", "HintFontSize"),
    ("Global default; each metric can override it on the Monitors page.", "全局默认值，单个监控项可在「监控项」页单独覆盖", "HintFontSizeGlobal"),
    ("Affects the text size of this metric in the overlay.", "影响该监控项在悬浮窗中的文字大小。", "HintFontSizeMetric"),
    ("Usage reflects 3D/game loads only; lower numbers are expected.", "占用率只反映游戏与 3D 应用的负载，开启后数值会偏低，属正常现象。", "HintGpu3D"),
    ("Reads only the primary GPU and skips other adapters to save overhead.", "只读取主要显卡的数据，忽略其他显示适配器，可减少开销。", "HintGpuPrimary"),
    ("Space between metric items.", "各监控项之间的空隙", "HintItemGap"),
    ("Interface language. Takes effect after restart.", "界面显示语言，重启后生效", "HintLanguage"),
    ("Percent shows usage; GB shows the actual amount.", "百分比显示占用率，GB 显示实际用量", "HintMemUnit"),
    ("Color of the metric's name.", "监控项名称的显示颜色。", "HintNameColor"),
    ("Show upload and download rates together.", "双向同时显示上下行速率", "HintNetDisplay"),
    ("Saves resources; the network rate updates with a short delay.", "减少资源占用，网速显示会有短暂延迟。", "HintNetLowFreq"),
    ("Distance between a label and its value.", "每项内部标签与数值之间的距离", "HintNvGap"),
    ("Fine-tune where the name/value sit in the overlay (pixels; negative moves left/up).", "微调名称与数值在悬浮窗中的位置（像素；负值向左/上偏移）。", "HintOffName"),
    ("Lower it to keep the overlay translucent for long-term display.", "降低后悬浮条会半透明，适合长期放置", "HintOpacity"),
    ("Turn off to skip kernel-level power sampling (saves CPU); the Power row then shows --.", "关闭后跳过内核级功耗采样以节省性能，功耗行将显示 --。", "HintPowerQuery"),
    ("Smooth favors lower cost; Fine favors animation quality.", "流畅优先降低绘制开销，精细优先保证动画细腻", "HintRenderMode"),
    ("Shorten labels: CPU→C, Mem→M, GPU→G, Net→N, Power→P, Time→T, Util→U.", "简化标签：CPU→C、Mem→M、GPU→G、Net→N、Power→P、Time→T、Util→U。", "HintShortNames"),
    ("Show or hide the metric name label (e.g. CPU, Mem).", "显示或隐藏监控项的名称标签（如 CPU、Mem）。", "HintShowLabel"),
    ("Horizontal strip, or stacked vertical lines.", "横版一行排布，竖版逐行堆叠", "HintStyle"),
    ("Clearer on light wallpapers; keep it off on dark backgrounds.", "在浅色壁纸上更清晰，深色背景下建议关闭", "HintTextShadow"),
    ("Adds an outline; easier to read on busy backgrounds.", "为文字添加轮廓，复杂背景下更易读", "HintTextStroke"),
    ("Show the clock as HH:MM:SS. Turn off to display HH:MM only.", "时间以 时:分:秒 显示；关闭后仅显示 时:分。", "HintTimeSeconds"),
    ("Show one decimal place (e.g. 24.7%). At idle an integer % can sit unchanged for a minute — decimals make updates visible.", "显示一位小数（如 24.7%）。空闲时整数百分比可能长时间不变，小数位让每次刷新都可见。", "HintUtilDecimal"),
    ("Color of the metric's value.", "监控项数值的显示颜色。", "HintValueColor"),
    ("Transition", "过渡时长", "LblAnimMs"),
    ("Appearance", "外观", "LblAppearance"),
    ("Launch at Login", "开机自启", "LblAutostart"),
    ("Background", "背景色", "LblBg"),
    ("Click-through", "鼠标可穿透", "LblClickThrough"),
    ("Position", "显示位置", "LblCorner"),
    ("CPU", "CPU", "LblCpu"),
    ("Debug Log", "调试日志", "LblDebug"),
    ("Render Mode", "渲染模式", "LblEngine"),
    ("Foreground", "前景色", "LblFg"),
    ("Default", "默认", "LblFollow"),
    ("Text Size", "字号", "LblFont"),
    ("Font Size", "字号", "LblFontSize"),
    ("Font Weight", "字体粗细", "LblFontWeight"),
    ("Label–Value Gap", "名称与数值的间距", "LblGapLabel"),
    ("Item Gap", "条目间距", "LblGapMetric"),
    ("GPU", "GPU", "LblGpu"),
    ("3D Load Only", "仅统计 3D 负载", "LblGpu3D"),
    ("Primary GPU Only", "仅统计主显卡", "LblGpuPrimary"),
    ("Height", "高度", "LblHeight"),
    ("Language", "语言", "LblLang"),
    ("Layout", "布局", "LblLayout"),
    ("Log File", "日志文件", "LblLogFolder"),
    ("Memory", "内存", "LblMem"),
    ("Memory Unit", "内存单位", "LblMemUnit"),
    ("Name Color", "名称色", "LblNameColor"),
    ("Network", "网络", "LblNet"),
    ("Lower Net Refresh Rate", "降低网络刷新频率", "LblNetLowFreq"),
    ("Network Display", "网络显示", "LblNetMode"),
    ("Name ↔", "名称左右", "LblOffNameX"),
    ("Name ↕", "名称上下", "LblOffNameY"),
    ("Value ↔", "数值左右", "LblOffValueX"),
    ("Value ↕", "数值上下", "LblOffValueY"),
    ("Opacity", "不透明度", "LblOpacity"),
    ("Text Outline", "文字描边", "LblOutline"),
    ("Overlay Size", "悬浮窗尺寸", "LblOverlaySize"),
    ("Pin Window", "固定窗口", "LblPin"),
    ("Power", "功耗", "LblPower"),
    ("Power Query", "功耗查询", "LblPowerQuery"),
    ("Refresh Rate", "刷新率", "LblRate"),
    ("Reset All", "全部重置", "LblResetAll"),
    ("Reset to Default", "恢复默认", "LblResetOffset"),
    ("Text Shadow", "文字阴影", "LblShadow"),
    ("Short Names", "简化名称", "LblShortNames"),
    ("Show", "显示", "LblShow"),
    ("Show Name", "显示名称", "LblShowLabel"),
    ("Style", "样式", "LblStyle"),
    ("Time", "时间", "LblTime"),
    ("Show Seconds", "显示秒", "LblTimeSeconds"),
    ("Always on Top", "窗口置顶", "LblTopmost"),
    ("Util", "智能整体占用", "LblUtil"),
    ("Show Decimals", "显示小数", "LblUtilDecimal"),
    ("Value Color", "数值色", "LblValueColor"),
    ("Width", "宽度", "LblWidth"),
    ("Quit", "退出", "MenuQuit"),
    ("Quick Restart", "快速重启", "MenuRestart"),
    ("Settings", "设置", "MenuSettings"),
    ("Changes apply instantly — see the desktop overlay.", "修改即时生效，可在桌面悬浮窗中查看。", "MetricsFoot"),
    ("Custom", "已自定义", "MtrCustomized"),
    ("Follows global", "跟随全局", "MtrFollow"),
    ("Off", "已关闭", "MtrOff"),
    ("On", "已启用", "MtrOn"),
    ("About", "关于", "PgAbout"),
    ("Preferences", "偏好", "PgGeneral"),
    ("Layout", "布局", "PgLayout"),
    ("Arrangement and sizing — applies to every metric. Data formats live on the Monitors page.", "悬浮条的排列方式与尺寸，对所有监控项生效；数据格式见「监控项」页", "PgLayoutSub"),
    ("Monitors", "监控项", "PgMetrics"),
    ("Configure each metric's display, format and colors; font size follows the global default.", "逐个配置悬浮条中每项数据的显示、格式与配色，字号默认跟随全局", "PgMetricsSub"),
    ("Motion", "动效", "PgMotion"),
    ("Trade-offs between resource usage and accuracy — all off is most accurate.", "在资源占用与数据精度之间取舍，全部关闭时数据最准", "PgPerfSub"),
    ("Performance", "性能", "PgPerformance"),
    ("Language, appearance and startup behavior.", "界面语言、外观主题与启动行为", "PgPrefsSub"),
    ("Window", "窗口", "PgWindow"),
    ("Overlay z-order, position and mouse behavior.", "悬浮条的层级、位置与鼠标行为", "PgWindowSub"),
    ("CPU", "CPU", "RowCpu"),
    ("GPU", "GPU", "RowGpu"),
    ("Memory", "内存", "RowMem"),
    ("Net", "网络", "RowNet"),
    ("Power", "功耗", "RowPower"),
    ("Settings", "设置", "SbTitle"),
    ("Dark", "深色", "SegAppearanceDark"),
    ("Light", "浅色", "SegAppearanceLight"),
    ("System", "跟随系统", "SegAppearanceSystem"),
    ("Auto", "自动", "SegAuto"),
    ("Bold", "粗", "SegBold"),
    ("Both", "双向", "SegBoth"),
    ("Cards", "卡片", "SegCards"),
    ("Col", "列", "SegCol"),
    ("Bottom L", "左下", "SegCornerLB"),
    ("Top L", "左上", "SegCornerLT"),
    ("Bottom R", "右下", "SegCornerRB"),
    ("Top R", "右上", "SegCornerRT"),
    ("Download", "下载", "SegDown"),
    ("EN", "英文", "SegEn"),
    ("Fine", "精细", "SegFine"),
    ("GB", "GB", "SegGb"),
    ("Horizontal", "横版", "SegHorizontal"),
    ("Medium", "中", "SegMedium"),
    ("None", "无", "SegNone"),
    ("Percent", "百分比", "SegPercent"),
    ("Performance", "性能", "SegPerf"),
    ("High Quality", "高画质", "SegQuality"),
    ("Round", "圆角", "SegRound"),
    ("Row", "行", "SegRow"),
    ("Smooth", "流畅", "SegSmooth"),
    ("Split", "分栏", "SegSplit"),
    ("Square", "方形", "SegSquare"),
    ("Strip", "条形", "SegStrip"),
    ("Thin", "细", "SegThin"),
    ("Upload", "上传", "SegUp"),
    ("Vertical", "竖版", "SegVertical"),
    ("ZH", "中文", "SegZh"),
    ("Log folder opened.", "已打开日志文件位置。", "StatusFolderOpened"),
    ("Link opened in browser.", "已在浏览器中打开链接。", "StatusLinkOpened"),
    ("Failed to open the link.", "打开链接失败。", "StatusOpenUrlFailed"),
    ("Processor usage", "中央处理器占用率", "SubCpu"),
    ("Graphics usage", "图形处理器占用率", "SubGpu"),
    ("Memory usage", "内存占用率", "SubMem"),
    ("Network throughput", "网络吞吐量", "SubNet"),
    ("System power draw", "整机功耗", "SubPower"),
    ("Unlock", "解锁", "TbUnlock"),
    ("GPU row stopped: vendor driver libraries (NVML/ADL) have been unloaded.\n\
             Utilization falls back to D3DKMT; vendor power readings require restarting AAX SysPulse.", "GPU 行已停止：厂商驱动库（NVML/ADL）已卸载。\n\
             利用率将回落到 D3DKMT 读取；厂商功耗读数需重启应用恢复。", "TeardownGpu"),
    ("Power row stopped: the MSR kernel driver has been unloaded.\n\
             Re-enabling it requires restarting AAX SysPulse to restore the display.", "功耗行已停止：MSR 内核驱动已卸载。\n\
             重新开启需要重启应用才能恢复显示。", "TeardownPower"),
    ("AAX SysPulse Settings", "AAX SysPulse 设置", "WinTitle"),
];

/// 按字节序比较两个字符串（const 上下文可用的安全比较，避免依赖 str 的 Ord 是否 const 稳定）。
const fn bytes_lt(a: &[u8], b: &[u8]) -> bool {
    let n = if a.len() < b.len() { a.len() } else { b.len() };
    let mut i = 0;
    while i < n {
        if a[i] != b[i] {
            return a[i] < b[i];
        }
        i += 1;
    }
    a.len() < b.len()
}

/// const fn 二分查找：返回 key 在 TABLE 中的下标。
const fn lookup(key: &str) -> Option<usize> {
    let kb = key.as_bytes();
    let mut lo = 0usize;
    let mut hi = TABLE.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let mk = TABLE[mid].2.as_bytes();
        if bytes_lt(mk, kb) {
            lo = mid + 1;
        } else if bytes_lt(kb, mk) {
            hi = mid;
        } else {
            return Some(mid);
        }
    }
    None
}

/// 编译期自检：TABLE 必须按 key 严格升序且无重复 key。
/// 若有人漏加/错排条目，编译直接失败。
/// 用 `const _` 匿名常量块，保证无论是否被引用都会在编译期求值。
const _: () = {
    let mut i = 1;
    while i < TABLE.len() {
        if !bytes_lt(TABLE[i-1].2.as_bytes(), TABLE[i].2.as_bytes()) {
            panic!("i18n TABLE not strictly sorted/unique at index");
        }
        i += 1;
    }
};

/// 翻译：按当前语言取文案。英文为默认（与 C 版一致）。
/// 未知 key 原样透传（保持旧行为）。
pub fn t(lang: Lang, key: &str) -> &str {
    match lookup(key) {
        Some(i) => if lang == Lang::Zh { TABLE[i].1 } else { TABLE[i].0 },
        None => key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aaxsyspulse_core::types::Lang;
    #[test]
    fn unknown_key_passthrough() {
        assert_eq!(t(Lang::En, "__nope__"), "__nope__");
        assert_eq!(t(Lang::Zh, "__nope__"), "__nope__");
    }
    #[test]
    fn lookup_hits_every_entry() {
        for (en, zh, key) in TABLE {
            assert_eq!(t(Lang::En, key), *en);
            assert_eq!(t(Lang::Zh, key), *zh);
        }
    }
}
