//! 跨模块共享的领域枚举 —— **唯一真源**。
//!
//! # 为什么要有这个模块
//!
//! 此前 `Zoom`/`Shape`/`VertLayout` 同时存在于 `config.rs` 和 `layout.rs`，
//! 两份定义各自演化（连 `Zoom` 的缩放因子一个是 `f32` 一个是 `f64`）。
//! 按 v2 决策「枚举归属」，统一收在本模块：
//!
//! ```text
//! core::types  ←── config   （解析/序列化时引用）
//!              ←── layout   （计算时引用）
//!              ←── ui       （渲染时引用）
//! ```
//!
//! 这样 `layout` 不需要为了拿一个枚举去依赖 `config`（避免层内依赖），
//! 也让 `layout::compute` 可以只吃窄参数而不吃整个 `Config`。
//!
//! # C 版兼容性
//!
//! `shape`/`vert`/`netunit` 的数值序与 `overlay.c` 完全一致，
//! `zoom` 的档位集合就是 C 版 `ZOOM_PCT[5] = {75, 100, 125, 150, 200}`。

/// 卡片缩放档位。与 C 版 `ZOOM_PCT[5] = {75, 100, 125, 150, 200}` 一一对应。
///
/// # Eco / Quality 档位差异（v2 Q9）
/// - **Quality**：保留完整 5 档，与 C 版一致（配置往返不丢档）。
/// - **Eco**：只提供 **100% / 200%** 两档，其余档位静默 clamp 到 100%，不报错。
///   见 [`Zoom::clamp_for`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Zoom {
    Zoom75,
    #[default]
    Zoom100,
    Zoom125,
    Zoom150,
    Zoom200,
}

impl Zoom {
    /// 全部档位，顺序与 C 版 `ZOOM_PCT` 一致（菜单枚举顺序依赖此顺序）。
    pub const ALL: [Zoom; 5] = [
        Zoom::Zoom75,
        Zoom::Zoom100,
        Zoom::Zoom125,
        Zoom::Zoom150,
        Zoom::Zoom200,
    ];

    /// 缩放因子（1.0 = 100%）。
    ///
    /// 统一为 `f32`：这是 UI/布局精度足够且体积更小的选择
    /// （此前 `layout::Zoom` 用 `f64`，在 x64 上无收益）。
    pub fn scale_factor(self) -> f32 {
        match self {
            Zoom::Zoom75 => 0.75,
            Zoom::Zoom100 => 1.0,
            Zoom::Zoom125 => 1.25,
            Zoom::Zoom150 => 1.5,
            Zoom::Zoom200 => 2.0,
        }
    }

    /// 序列化为配置里的百分数（C 版 `zoom %d`）。
    pub fn to_config(self) -> u32 {
        match self {
            Zoom::Zoom75 => 75,
            Zoom::Zoom100 => 100,
            Zoom::Zoom125 => 125,
            Zoom::Zoom150 => 150,
            Zoom::Zoom200 => 200,
        }
    }

    /// 宽容解析：任何非法值都回落到 100%，不报错。
    ///
    /// 只吸附到最近的合法档位，避免手工编辑出 `zoom 137` 就整份配置失效。
    pub fn from_config(v: u32) -> Self {
        let mut best = Zoom::Zoom100;
        let mut best_dist = u32::MAX;
        for z in Zoom::ALL {
            let d = if v > z.to_config() {
                v - z.to_config()
            } else {
                z.to_config() - v
            };
            if d < best_dist {
                best_dist = d;
                best = z;
            }
        }
        best
    }

    /// 该档位是否为 Eco 模式允许的两档之一（100% / 200%）。
    pub fn is_eco_allowed(self) -> bool {
        matches!(self, Zoom::Zoom100 | Zoom::Zoom200)
    }

    /// 按渲染模式收敛档位：Eco 下非允许档位一律 clamp 到 100%。
    ///
    /// 「向下 clamp」而非报错 —— Eco 是省电/低负载模式，
    /// 把 125/150 放大到 200 会让卡片突然变大、画更多像素，与 Eco 的意图相反。
    pub fn clamp_for(self, mode: RenderMode) -> Self {
        match mode {
            RenderMode::Quality => self,
            RenderMode::Eco if self.is_eco_allowed() => self,
            RenderMode::Eco => Zoom::Zoom100,
        }
    }
}

/// 渲染模式：低负载（点阵）或高质量（矢量 + 装饰）。
///
/// 默认 **Quality**（v2 Q16：首次印象优先）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RenderMode {
    Eco,
    #[default]
    Quality,
}

impl RenderMode {
    /// 宽容解析：接受数值（0=eco 1=quality）与字符串。
    pub fn from_config(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "0" | "eco" => Some(RenderMode::Eco),
            "1" | "quality" => Some(RenderMode::Quality),
            _ => None,
        }
    }

    /// 序列化。C 版没有此键，用字符串写入（C 版会静默忽略）。
    pub fn to_config(self) -> &'static str {
        match self {
            RenderMode::Eco => "eco",
            RenderMode::Quality => "quality",
        }
    }
}

/// 悬浮窗渲染风格：双渲染器切换（v3 新增）。
///
/// - [`RenderStyle::Classic`]：旧 C 版卡片（圆角背景 + 主机名 + 行布局 + 进度条），
///   复用 [`crate::layout::compute`] 与 `ui::draw_card`，恢复透明度 / 形状 / 配色 / 悬停 /
///   vert 布局等 C 版能力。
/// - [`RenderStyle::Strip`]：v3 横向文字条（无背景、槽位右对齐、阴影/描边），默认。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RenderStyle {
    Classic,
    #[default]
    Strip,
}

impl RenderStyle {
    /// 宽容解析：接受数值与字符串（新旧配置都读得动）。
    pub fn from_config(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "0" | "classic" | "card" | "cards" => Some(RenderStyle::Classic),
            "1" | "strip" | "compact" => Some(RenderStyle::Strip),
            _ => None,
        }
    }

    /// 序列化（C 版没有此键，写字符串，C 版会静默忽略）。
    pub fn to_config(self) -> &'static str {
        match self {
            RenderStyle::Classic => "classic",
            RenderStyle::Strip => "strip",
        }
    }
}

/// 网速单位。C 版数值序：**0=KB/s 1=MB/s 2=GB/s**。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NetUnit {
    KB,
    #[default]
    MB,
    GB,
}

impl NetUnit {
    /// 宽容解析：接受数值（C 版）与字符串（新版）。
    pub fn from_config(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "0" | "kb" => Some(NetUnit::KB),
            "1" | "mb" => Some(NetUnit::MB),
            "2" | "gb" => Some(NetUnit::GB),
            _ => None,
        }
    }

    /// 序列化为 C 版数值。
    pub fn to_config(self) -> u32 {
        match self {
            NetUnit::KB => 0,
            NetUnit::MB => 1,
            NetUnit::GB => 2,
        }
    }

    /// 每秒字节数按本单位换算后的数值。
    ///
    /// 与 C 版一致地采用 **1000 进制**（1 MB/s = 1_000_000 B/s），
    /// 因为网卡速率本身就是十进制标称。
    pub fn convert(self, bytes_per_sec: u64) -> f64 {
        match self {
            NetUnit::KB => bytes_per_sec as f64 / 1_000.0,
            NetUnit::MB => bytes_per_sec as f64 / 1_000_000.0,
            NetUnit::GB => bytes_per_sec as f64 / 1_000_000_000.0,
        }
    }

    /// 单位后缀字符（用于紧凑显示，避免 `format!`）。
    pub fn suffix(self) -> char {
        match self {
            NetUnit::KB => 'K',
            NetUnit::MB => 'M',
            NetUnit::GB => 'G',
        }
    }
}

/// 卡片外形。C 版数值序：**0=圆角 1=直角 2=纯内容**。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Shape {
    #[default]
    Round,
    Square,
    Content,
}

impl Shape {
    pub fn from_config(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "0" | "round" => Some(Shape::Round),
            "1" | "square" => Some(Shape::Square),
            "2" | "content" => Some(Shape::Content),
            _ => None,
        }
    }

    pub fn to_config(self) -> u32 {
        match self {
            Shape::Round => 0,
            Shape::Square => 1,
            Shape::Content => 2,
        }
    }

    /// 是否绘制背景板（`Content` 模式只画内容，不画底）。
    pub fn has_background(self) -> bool {
        !matches!(self, Shape::Content)
    }
}

/// 极简模式排列。C 版数值序：**0=单行 1=单列 2=分列**。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VertLayout {
    #[default]
    SingleRow,
    SingleCol,
    SplitCol,
}

impl VertLayout {
    pub fn from_config(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "0" | "singlerow" | "single_row" => Some(VertLayout::SingleRow),
            "1" | "singlecol" | "single_col" => Some(VertLayout::SingleCol),
            "2" | "splitcol" | "split_col" => Some(VertLayout::SplitCol),
            _ => None,
        }
    }

    pub fn to_config(self) -> u32 {
        match self {
            VertLayout::SingleRow => 0,
            VertLayout::SingleCol => 1,
            VertLayout::SplitCol => 2,
        }
    }
}

/// 功耗显示语义。详见 v2 方案 §2.1（A/B/C/D 四类功耗）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PowerDisplay {
    /// A. CPU 封装功耗（RAPL），跨设备语义明确，默认。
    #[default]
    CpuPackage,
    /// C. 电池放电率（仅笔记本，整机口径）。
    Battery,
    /// D. 估算模型兜底 —— UI **必须**以 `~` 前缀诚实标注，不能冒充实测值。
    Estimated,
}

impl PowerDisplay {
    pub fn from_config(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "0" | "cpupackage" | "cpu_package" | "cpu" => Some(PowerDisplay::CpuPackage),
            "1" | "battery" => Some(PowerDisplay::Battery),
            "2" | "estimated" | "estimate" => Some(PowerDisplay::Estimated),
            _ => None,
        }
    }

    pub fn to_config(self) -> &'static str {
        match self {
            PowerDisplay::CpuPackage => "cpupackage",
            PowerDisplay::Battery => "battery",
            PowerDisplay::Estimated => "estimated",
        }
    }

    /// 该语义是否为**估算值**（需要 `~` 前缀标注）。
    pub fn is_estimated(self) -> bool {
        matches!(self, PowerDisplay::Estimated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_matches_c_zoom_pct() {
        // C 版 ZOOM_PCT[5] = {75, 100, 125, 150, 200}
        assert_eq!(
            Zoom::ALL.map(|z| z.to_config()),
            [75, 100, 125, 150, 200],
            "档位集合与顺序必须与 C 版一致"
        );
    }

    #[test]
    fn zoom_scale_factors() {
        assert_eq!(Zoom::Zoom75.scale_factor(), 0.75);
        assert_eq!(Zoom::Zoom100.scale_factor(), 1.0);
        assert_eq!(Zoom::Zoom125.scale_factor(), 1.25);
        assert_eq!(Zoom::Zoom150.scale_factor(), 1.5);
        assert_eq!(Zoom::Zoom200.scale_factor(), 2.0);
    }

    #[test]
    fn zoom_from_config_snaps_to_nearest() {
        assert_eq!(Zoom::from_config(75), Zoom::Zoom75);
        assert_eq!(Zoom::from_config(100), Zoom::Zoom100);
        // 非法/中间值吸附到最近档位，不回落成默认值
        assert_eq!(Zoom::from_config(130), Zoom::Zoom125);
        assert_eq!(Zoom::from_config(0), Zoom::Zoom75);
        assert_eq!(Zoom::from_config(9999), Zoom::Zoom200);
    }

    #[test]
    fn eco_only_allows_100_and_200() {
        assert!(Zoom::Zoom100.is_eco_allowed());
        assert!(Zoom::Zoom200.is_eco_allowed());
        assert!(!Zoom::Zoom75.is_eco_allowed());
        assert!(!Zoom::Zoom125.is_eco_allowed());
        assert!(!Zoom::Zoom150.is_eco_allowed());
    }

    #[test]
    fn eco_clamps_down_to_100() {
        // Quality 保持原档
        assert_eq!(Zoom::Zoom75.clamp_for(RenderMode::Quality), Zoom::Zoom75);
        assert_eq!(Zoom::Zoom125.clamp_for(RenderMode::Quality), Zoom::Zoom125);

        // Eco 下 75/125/150 一律 clamp 到 100（向下，不放大）
        assert_eq!(Zoom::Zoom75.clamp_for(RenderMode::Eco), Zoom::Zoom100);
        assert_eq!(Zoom::Zoom125.clamp_for(RenderMode::Eco), Zoom::Zoom100);
        assert_eq!(Zoom::Zoom150.clamp_for(RenderMode::Eco), Zoom::Zoom100);
        assert_eq!(Zoom::Zoom200.clamp_for(RenderMode::Eco), Zoom::Zoom200);
    }

    #[test]
    fn c_enum_ordering_is_preserved() {
        assert_eq!(Shape::from_config("0"), Shape::from_config("round"));
        assert_eq!(Shape::from_config("1"), Shape::from_config("square"));
        assert_eq!(Shape::from_config("2"), Shape::from_config("content"));

        assert_eq!(
            VertLayout::from_config("1"),
            VertLayout::from_config("single_col")
        );
        assert_eq!(NetUnit::from_config("2"), NetUnit::from_config("GB"));

        // 未知值返回 None（交给调用方保留原值）
        assert_eq!(Shape::from_config("7"), None);
        assert_eq!(NetUnit::from_config("tb"), None);
    }

    #[test]
    fn netunit_convert_is_decimal() {
        // 1 MB/s = 1_000_000 B/s
        assert_eq!(NetUnit::MB.convert(1_000_000), 1.0);
        assert_eq!(NetUnit::KB.convert(1_000_000), 1_000.0);
        assert_eq!(NetUnit::GB.convert(1_000_000_000), 1.0);
    }

    #[test]
    fn power_estimated_needs_marker() {
        assert!(PowerDisplay::Estimated.is_estimated());
        assert!(!PowerDisplay::CpuPackage.is_estimated());
        assert!(!PowerDisplay::Battery.is_estimated());
    }

    #[test]
    fn shape_has_background() {
        assert!(Shape::Round.has_background());
        assert!(Shape::Square.has_background());
        assert!(!Shape::Content.has_background());
    }
}
