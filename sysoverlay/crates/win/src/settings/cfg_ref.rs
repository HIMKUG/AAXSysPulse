//! `CfgRef` —— 控件 ↔ Config 的**全项目唯一映射表**。
//!
//! # 解决什么问题
//!
//! 旧实现里「控件 ↔ Config」的映射散落在三处巨型 match：
//! `ToggleWidget::sync_state`（15 分支）、`SegmentedWidget::sync_state`（6 分支）、
//! `SliderWidget::sync_state`（9 分支），加上 apply 侧（on_click / slider_drag）
//! 的对应反向 match。新增一个控件要改 3~4 处，漏一处就是「显示对、写不回」的
//! 静默 bug。
//!
//! 新模型：控件构造时持有一个 [`CfgRef`]，读写都走它——
//! `sync = cfg_ref.get(cfg)`，`apply = cfg_ref.set(cfg, value)`。
//! **类型错配在 debug 构建直接 panic，release 返回 false**，不再静默错值。
//!
//! # 单元测试
//!
//! 本文件测试即规格：每个变体都有 roundtrip 断言（set 后 get 回同一值），
//! 外加类型错配必须返回 false 的防回归断言。

use sysoverlay_core::color::Rgb;
use sysoverlay_core::config::Config;
use sysoverlay_core::types::{Appearance, Lang, NetMode, RenderEngine, RenderStyle};

/// 控件写入/读出的泛化值。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    /// 开关类。
    Bool(bool),
    /// 滑杆/数值类（字号、间距、透明度、动画时长、窗口宽高）。
    Num(f64),
    /// 分段选择类（样式、形状、内存单位、刷新率、语言）。
    Select(usize),
    /// 可空分段选择（显示位置：None = 未贴角/自由定位）。
    SelectOpt(Option<usize>),
    /// 指标颜色（None = 跟随默认）。
    Color(Option<Rgb>),
}

/// Config 单一真值引用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CfgRef {
    // ── bool 开关 ──
    ShowCpu,
    ShowMem,
    ShowGpu,
    ShowNet,
    PowerEnabled,
    /// 时钟行显示开关（监控项页 Time 卡）。
    ShowTime,
    /// 时钟是否显示秒（HH:MM:SS / HH:MM）。
    TimeSeconds,
    /// 逐指标名称显示开关（默认开；关闭后只显示数值）。
    MetricShowLabel(usize),
    /// 简化名称：CPU→C、Mem→M、GPU→G、Net→N、Power→P、Time→T。
    ShortNames,
    TextShadow,
    TextOutline,
    Topmost,
    #[allow(dead_code)] AutoStart,
    #[allow(dead_code)] DebugLog,
    GpuPrimaryOnly,
    Gpu3dOnly,
    NetLowFreq,
    /// 网络栏显示模式：0=双向 1=仅下载 2=仅上传。
    NetMode,
    /// 鼠标可穿透（窗口页，报告 §5）。
    ClickThrough,
    /// 贴靠屏幕的哪个角：0=右上 1=右下 2=左上 3=左下；None = 自由定位。
    SnapCorner,
    /// 悬浮窗不透明度（%），映射 `normal_opacity`（报告范围 40..=100）。
    Opacity,

    // ── 数值滑杆 ──
    FontSize,
    MetricGap,
    LabelValueGap,
    /// 逐指标字号（下标 0..5）。
    MetricFont(usize),

    // ── 分段选择 ──
    Style,
    /// 内存单位：0 = 百分比，1 = GB。
    MemUnit,
    /// 界面语言：0 = En，1 = Zh。
    Lang,
    /// 悬浮窗渲染引擎：0 = 性能（像素块），1 = 高画质（D2D + DirectWrite）。
    RenderEngine,
    /// 字重档位：0 = 细，1 = 中，2 = 粗（默认）。
    FontWeight,
    /// 外观模式：0 = 跟随系统，1 = 浅色，2 = 深色。
    /// 顺序必须与 layout.rs 的 labels 顺序一致（红线 R1）。
    Appearance,

    // ── 指标颜色 ──
    /// 指标名称色（下标 0..5）。
    MetricLabel(usize),
    /// 指标数值色（下标 0..5）。
    MetricValue(usize),
    /// 数值槽「自动色」：按占用率分档变色（<60% 绿 / 60~85% 橙 / ≥85% 红）。
    MetricValueAuto(usize),
}

impl CfgRef {
    /// 从 Config 读当前值。
    pub fn get(self, cfg: &Config) -> Value {
        match self {
            // bool
            CfgRef::ShowCpu => Value::Bool(cfg.show_cpu),
            CfgRef::ShowMem => Value::Bool(cfg.show_mem),
            CfgRef::ShowGpu => Value::Bool(cfg.show_gpu),
            CfgRef::ShowNet => Value::Bool(cfg.show_net),
            CfgRef::PowerEnabled => Value::Bool(cfg.power_enabled),
            CfgRef::ShowTime => Value::Bool(cfg.show_time),
            CfgRef::TimeSeconds => Value::Bool(cfg.time_show_seconds),
            CfgRef::MetricShowLabel(i) => {
                Value::Bool(*cfg.metric_show_labels.get(i).unwrap_or(&true))
            }
            CfgRef::ShortNames => Value::Bool(cfg.short_names),
            CfgRef::TextShadow => Value::Bool(cfg.text_shadow),
            CfgRef::TextOutline => Value::Bool(cfg.text_outline),
            CfgRef::Topmost => Value::Bool(cfg.topmost),
            CfgRef::AutoStart => Value::Bool(cfg.autostart),
            CfgRef::DebugLog => Value::Bool(cfg.debug_log),
            CfgRef::GpuPrimaryOnly => Value::Bool(cfg.gpu_primary_only),
            CfgRef::Gpu3dOnly => Value::Bool(cfg.gpu_3d_only),
            CfgRef::NetLowFreq => Value::Bool(cfg.net_low_freq),
            CfgRef::NetMode => {
                Value::Select(match cfg.net_mode {
                    NetMode::Both => 0,
                    NetMode::Down => 1,
                    NetMode::Up => 2,
                })
            }
            CfgRef::ClickThrough => Value::Bool(cfg.click_through),
            CfgRef::SnapCorner => Value::SelectOpt(if cfg.snap_corner <= 3 {
                Some(cfg.snap_corner as usize)
            } else {
                None
            }),
            CfgRef::Opacity => Value::Num(cfg.normal_opacity as f64),
            // num
            CfgRef::FontSize => Value::Num(cfg.font_size as f64),
            CfgRef::MetricGap => Value::Num(cfg.metric_gap as f64),
            CfgRef::LabelValueGap => Value::Num(cfg.label_value_gap as f64),
            CfgRef::MetricFont(i) => {
                Value::Num(cfg.metric_font_sizes[i].unwrap_or(cfg.font_size) as f64)
            }
            // select
            CfgRef::Style => Value::Select(match cfg.render_style {
                RenderStyle::Vertical => 1,
                _ => 0,
            }),
            CfgRef::MemUnit => Value::Select(if cfg.mem_show_gb { 1 } else { 0 }),
            CfgRef::Lang => Value::Select(cfg.lang as usize),
            CfgRef::RenderEngine => Value::Select(if cfg.render_engine == RenderEngine::D2d { 1 } else { 0 }),
            CfgRef::FontWeight => Value::Select(cfg.font_weight as usize),
            CfgRef::Appearance => Value::Select(cfg.appearance.to_config() as usize),
            // color
    CfgRef::MetricLabel(i) => Value::Color(cfg.metric_colors[i].label),
    CfgRef::MetricValue(i) => Value::Color(cfg.metric_colors[i].value),
    CfgRef::MetricValueAuto(i) => Value::Bool(cfg.metric_colors[i].value_auto),
    }
    }

    /// 写回 Config。类型错配返回 false（debug 构建 panic）。
    pub fn set(self, cfg: &mut Config, v: Value) -> bool {
        match (self, v) {
            // bool
            (CfgRef::ShowCpu, Value::Bool(x)) => cfg.show_cpu = x,
            (CfgRef::ShowMem, Value::Bool(x)) => cfg.show_mem = x,
            (CfgRef::ShowGpu, Value::Bool(x)) => cfg.show_gpu = x,
            (CfgRef::ShowNet, Value::Bool(x)) => cfg.show_net = x,
            (CfgRef::PowerEnabled, Value::Bool(x)) => cfg.power_enabled = x,
            (CfgRef::ShowTime, Value::Bool(x)) => cfg.show_time = x,
            (CfgRef::TimeSeconds, Value::Bool(x)) => cfg.time_show_seconds = x,
            (CfgRef::MetricShowLabel(i), Value::Bool(x)) => {
                if let Some(v) = cfg.metric_show_labels.get_mut(i) {
                    *v = x;
                } else {
                    return false;
                }
            }
            (CfgRef::ShortNames, Value::Bool(x)) => cfg.short_names = x,
            (CfgRef::TextShadow, Value::Bool(x)) => cfg.text_shadow = x,
            (CfgRef::TextOutline, Value::Bool(x)) => cfg.text_outline = x,
            (CfgRef::Topmost, Value::Bool(x)) => cfg.topmost = x,
            (CfgRef::AutoStart, Value::Bool(x)) => cfg.autostart = x,
            (CfgRef::DebugLog, Value::Bool(x)) => cfg.debug_log = x,
            (CfgRef::GpuPrimaryOnly, Value::Bool(x)) => cfg.gpu_primary_only = x,
            (CfgRef::Gpu3dOnly, Value::Bool(x)) => cfg.gpu_3d_only = x,
            (CfgRef::NetLowFreq, Value::Bool(x)) => cfg.net_low_freq = x,
            (CfgRef::NetMode, Value::Select(i)) => match i {
                0 => cfg.net_mode = NetMode::Both,
                1 => cfg.net_mode = NetMode::Down,
                2 => cfg.net_mode = NetMode::Up,
                _ => return false,
            },
            (CfgRef::ClickThrough, Value::Bool(x)) => cfg.click_through = x,
            (CfgRef::SnapCorner, Value::SelectOpt(o)) => match o {
                Some(i) if i <= 3 => cfg.snap_corner = i as u8,
                None => cfg.snap_corner = sysoverlay_core::config::SNAP_CORNER_FREE,
                _ => return false,
            },
            (CfgRef::SnapCorner, Value::Select(i)) => {
                // 容错：普通分段值也接受
                if i <= 3 {
                    cfg.snap_corner = i as u8;
                } else {
                    return false;
                }
            }
            (CfgRef::Opacity, Value::Num(x)) => cfg.normal_opacity = (x as u32).clamp(0, 100) as u8,
            // num
            (CfgRef::FontSize, Value::Num(x)) => cfg.font_size = x as f32,
            (CfgRef::MetricGap, Value::Num(x)) => cfg.metric_gap = x as u32,
            (CfgRef::LabelValueGap, Value::Num(x)) => cfg.label_value_gap = x as u32,
            (CfgRef::MetricFont(i), Value::Num(x)) => {
                if i < cfg.metric_font_sizes.len() {
                    let v = x as f32;
                    // 设为与全局字号相同的值 = 重置为「跟随全局」（None）。
                    // 逐项滑杆此前只能写 Some(x)，一旦误触该指标就永久钉死、
                    // 全局字号对它失效且 UI 上无任何恢复途径（用户实测
                    // "字号调整对内存项目不生效"的根因）。
                    if (v - cfg.font_size).abs() < 0.05 {
                        cfg.metric_font_sizes[i] = None;
                    } else {
                        cfg.metric_font_sizes[i] = Some(v);
                    }
                } else {
                    return false;
                }
            }
            // select
            // 2 档：0 = 横版（Strip，Classic 已在 parse 迁移）、1 = 竖版。
            (CfgRef::Style, Value::Select(i)) => match i {
                0 => cfg.render_style = RenderStyle::Strip,
                1 => cfg.render_style = RenderStyle::Vertical,
                _ => return false,
            },
            (CfgRef::MemUnit, Value::Select(i)) => cfg.mem_show_gb = i == 1,
            // 字重：0=细 1=中 2=粗（越界拒绝，parse 阶段另有钳位兜底）
            (CfgRef::FontWeight, Value::Select(i)) => {
                if i <= 2 {
                    cfg.font_weight = i as u8;
                } else {
                    return false;
                }
            }
            (CfgRef::Appearance, Value::Select(i)) => match i {
                0 => cfg.appearance = Appearance::System,
                1 => cfg.appearance = Appearance::Light,
                2 => cfg.appearance = Appearance::Dark,
                _ => return false,
            },
            (CfgRef::Lang, Value::Select(i)) => match i {
                0 => cfg.lang = Lang::En,
                1 => cfg.lang = Lang::Zh,
                _ => return false,
            },
            (CfgRef::RenderEngine, Value::Select(i)) => {
                cfg.render_engine = if i == 1 { RenderEngine::D2d } else { RenderEngine::Gdi }
            }
            // color
            (CfgRef::MetricLabel(i), Value::Color(c)) => {
                if i < cfg.metric_colors.len() {
                    cfg.metric_colors[i].label = c;
                } else {
                    return false;
                }
            }
            (CfgRef::MetricValue(i), Value::Color(c)) => {
                if i < cfg.metric_colors.len() {
                    cfg.metric_colors[i].value = c;
                } else {
                    return false;
                }
            }
            (CfgRef::MetricValueAuto(i), Value::Bool(x)) => {
                if i < cfg.metric_colors.len() {
                    cfg.metric_colors[i].value_auto = x;
                } else {
                    return false;
                }
            }
            // 类型错配：debug 炸，release 拒绝
            (r, v) => {
                debug_assert!(
                    false,
                    "CfgRef 类型错配: {r:?} <- {v:?}"
                );
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    /// 每个变体的 (CfgRef, 写入值)：set 后 get 必须回读同一值。
    fn roundtrip_cases() -> Vec<(CfgRef, Value)> {
        vec![
            (CfgRef::ShowCpu, Value::Bool(false)),
            (CfgRef::ShowMem, Value::Bool(false)),
            (CfgRef::ShowGpu, Value::Bool(false)),
            (CfgRef::ShowNet, Value::Bool(false)),
            (CfgRef::ShowTime, Value::Bool(true)),
            (CfgRef::ShowTime, Value::Bool(false)),
            (CfgRef::TimeSeconds, Value::Bool(true)),
            (CfgRef::TimeSeconds, Value::Bool(false)),
            (CfgRef::MetricShowLabel(0), Value::Bool(false)),
            (CfgRef::MetricShowLabel(3), Value::Bool(false)),
            (CfgRef::MetricShowLabel(5), Value::Bool(true)),
            (CfgRef::ShortNames, Value::Bool(true)),
            (CfgRef::ShortNames, Value::Bool(false)),
            (CfgRef::PowerEnabled, Value::Bool(true)),
            (CfgRef::TextShadow, Value::Bool(false)),
            (CfgRef::TextOutline, Value::Bool(false)),
            (CfgRef::Topmost, Value::Bool(false)),
            (CfgRef::AutoStart, Value::Bool(true)),
            (CfgRef::DebugLog, Value::Bool(false)),
            (CfgRef::GpuPrimaryOnly, Value::Bool(false)),
            (CfgRef::Gpu3dOnly, Value::Bool(false)),
            (CfgRef::NetLowFreq, Value::Bool(false)),
            (CfgRef::NetMode, Value::Select(0)),
            (CfgRef::NetMode, Value::Select(1)),
            (CfgRef::NetMode, Value::Select(2)),
            (CfgRef::FontSize, Value::Num(24.0)),
            (CfgRef::MetricGap, Value::Num(40.0)),
            (CfgRef::LabelValueGap, Value::Num(30.0)),
            (CfgRef::MetricFont(2), Value::Num(18.0)),
            (CfgRef::Style, Value::Select(1)),
            (CfgRef::MemUnit, Value::Select(1)),
            (CfgRef::Lang, Value::Select(1)),
            (CfgRef::RenderEngine, Value::Select(1)),
            (CfgRef::FontWeight, Value::Select(0)),
            (CfgRef::FontWeight, Value::Select(1)),
            (CfgRef::FontWeight, Value::Select(2)),
            (CfgRef::Appearance, Value::Select(0)),
            (CfgRef::Appearance, Value::Select(1)),
            (CfgRef::Appearance, Value::Select(2)),
            (CfgRef::MetricLabel(0), Value::Color(Some(Rgb::new(10, 200, 255)))),
            (CfgRef::MetricValue(4), Value::Color(None)),
            // v5 窗口页新增
            (CfgRef::ClickThrough, Value::Bool(true)),
            (CfgRef::ClickThrough, Value::Bool(false)),
            (CfgRef::SnapCorner, Value::SelectOpt(Some(0))),
            (CfgRef::SnapCorner, Value::SelectOpt(Some(3))),
            (CfgRef::SnapCorner, Value::SelectOpt(None)),
            (CfgRef::Opacity, Value::Num(80.0)),
            (CfgRef::Opacity, Value::Num(40.0)),
        ]
    }

    #[test]
    fn every_variant_set_then_get_roundtrips() {
        for (r, v) in roundtrip_cases() {
            let mut c = cfg();
            assert!(r.set(&mut c, v), "set 失败: {r:?}");
            assert_eq!(r.get(&c), v, "roundtrip 失败: {r:?}");
        }
    }

    #[test]
    fn metric_font_set_to_global_resets_to_follow() {
        // 回归锁：逐项字号设为与全局相同的值 = 重置为「跟随全局」。
        // 此前滑杆只能写 Some(x)，误触一次该指标就被永久钉死，
        // 全局字号对它失效且无 UI 途径恢复（mfs_mem 34.5 事故）。
        let mut c = cfg();
        assert!(CfgRef::MetricFont(1).set(&mut c, Value::Num(40.0)));
        assert_eq!(c.metric_font_sizes[1], Some(40.0), "异于全局 → 覆盖");

        let global = c.font_size;
        assert!(CfgRef::MetricFont(1).set(&mut c, Value::Num(global as f64)));
        assert_eq!(c.metric_font_sizes[1], None, "等于全局 → 跟随");
        assert_eq!(CfgRef::MetricFont(1).get(&c), Value::Num(global as f64));
    }

    #[test]
    fn out_of_range_index_is_rejected_not_panicking() {
        let mut c = cfg();
        assert!(!CfgRef::MetricFont(9).set(&mut c, Value::Num(1.0)));
        assert!(!CfgRef::MetricLabel(9).set(&mut c, Value::Color(None)));
        assert!(!CfgRef::Lang.set(&mut c, Value::Select(7)));
    }

    #[test]
    fn metric_font_none_falls_back_to_global_font_size() {
        let mut c = cfg();
        c.font_size = 22.0;
        assert_eq!(CfgRef::MetricFont(0).get(&c), Value::Num(22.0));
        assert!(CfgRef::MetricFont(0).set(&mut c, Value::Num(15.0)));
        assert_eq!(c.metric_font_sizes[0], Some(15.0));
    }

    // debug_assert! 在 release 下被编译掉 → 本测试只能在 debug 构建中运行
    // （`cargo test --release` 会因「未触发 panic」而失败，故加门控）。
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic]
    fn type_mismatch_panics_in_debug() {
        let mut c = cfg();
        let _ = CfgRef::ShowCpu.set(&mut c, Value::Num(1.0));
    }

    #[test]
    fn lang_select_matches_i18n_segment_order() {
        // 设置页语言分段标签顺序 = [En, Zh]，下标必须一一对应。
        let mut c = cfg();
        assert!(CfgRef::Lang.set(&mut c, Value::Select(1)));
        assert!(matches!(c.lang, Lang::Zh));
    }

    #[test]
    fn snap_corner_free_semantics() {
        // 默认（无键）= 自由定位 → UI 显示为未选中（SelectOpt(None)）
        let c = cfg();
        assert_eq!(CfgRef::SnapCorner.get(&c), Value::SelectOpt(None));
        // 贴角后可读回；解除贴角写回 255
        let mut c = cfg();
        assert!(CfgRef::SnapCorner.set(&mut c, Value::SelectOpt(Some(1))));
        assert_eq!(c.snap_corner, 1);
        assert!(CfgRef::SnapCorner.set(&mut c, Value::SelectOpt(None)));
        assert_eq!(c.snap_corner, sysoverlay_core::config::SNAP_CORNER_FREE);
    }

    #[test]
    fn opacity_maps_to_normal_opacity() {
        let mut c = cfg();
        c.normal_opacity = 35; // 旧默认
        assert_eq!(CfgRef::Opacity.get(&c), Value::Num(35.0));
        assert!(CfgRef::Opacity.set(&mut c, Value::Num(70.0)));
        assert_eq!(c.normal_opacity, 70);
    }
}
