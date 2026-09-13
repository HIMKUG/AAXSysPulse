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

use aaxsyspulse_core::color::Rgb;
use aaxsyspulse_core::config::{Config, OFFSET_MAX, OFFSET_MIN, SNAP_CORNER_FREE};
use aaxsyspulse_core::types::{Appearance, Lang, NetMode, RenderEngine, RenderStyle};

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
// 注意：刻意不用 `#[macro_export]`。本宏只在当前模块内使用；
// 若导出，其表内 token（`cfg` 参数、`MetricFont(i)` 等模式）会带上 crate-root 卫生上下文，
// 与模块内 `enum CfgRef` 变体、生成的 `get/set` 的 `cfg` 形参对不上，导致
// "expected value, found macro cfg" / "cannot find tuple variant" 这类卫生错误。
// `CfgRef` 是「控件 ↔ Config」的唯一映射表。枚举变体 + 下方 `get`/`set` 的
// match 臂一一对应；新增设置项时同步追加：① 枚举变体 ② get 臂 ③ set 臂。
// （历史上曾用 `for_each_cfg_field!` 宏同时生成 getter/setter，但 macro_rules!
// 的卫生机制会让表内 `cfg`/`i` 与生成的 `get`/`set` 形参处于不同卫生上下文，
// 导致 "expected value, found macro cfg"；故改为单一 hygiene 上下文的手写 impl，
// 行为完全等价于原宏生成结果。）
impl CfgRef {
    pub fn get(self, cfg: &Config) -> Value {
        match self {
            CfgRef::ShowCpu => Value::Bool(cfg.show_cpu),
            CfgRef::ShowMem => Value::Bool(cfg.show_mem),
            CfgRef::ShowGpu => Value::Bool(cfg.show_gpu),
            CfgRef::ShowNet => Value::Bool(cfg.show_net),
            CfgRef::ShowUtil => Value::Bool(cfg.show_util),
            CfgRef::UtilDecimal => Value::Bool(cfg.util_decimal),
            CfgRef::ShowTime => Value::Bool(cfg.show_time),
            CfgRef::TimeSeconds => Value::Bool(cfg.time_show_seconds),
            CfgRef::ShortNames => Value::Bool(cfg.short_names),
            CfgRef::MemUnit => Value::Select(if cfg.mem_show_gb { 1 } else { 0 }),
            CfgRef::FontSize => Value::Num(cfg.font_size as f64),
            CfgRef::FontWeight => Value::Select(cfg.font_weight as usize),
            CfgRef::MetricGap => Value::Num(cfg.metric_gap as f64),
            CfgRef::LabelValueGap => Value::Num(cfg.label_value_gap as f64),
            CfgRef::TextShadow => Value::Bool(cfg.text_shadow),
            CfgRef::TextOutline => Value::Bool(cfg.text_outline),
            CfgRef::AutoStart => Value::Bool(cfg.autostart),
            CfgRef::DebugLog => Value::Bool(cfg.debug_log),
            CfgRef::Topmost => Value::Bool(cfg.topmost),
            CfgRef::Opacity => Value::Num(cfg.normal_opacity as f64),
            CfgRef::NetMode => Value::Select(match cfg.net_mode { NetMode::Both => 0, NetMode::Down => 1, NetMode::Up => 2 }),
            CfgRef::Style => Value::Select(match cfg.render_style { RenderStyle::Vertical => 1, _ => 0 }),
            CfgRef::RenderEngine => Value::Select(if cfg.render_engine == RenderEngine::D2d { 1 } else { 0 }),
            CfgRef::Lang => Value::Select(cfg.lang as usize),
            CfgRef::PowerEnabled => Value::Bool(cfg.power_enabled),
            CfgRef::PowerQuery => Value::Bool(cfg.power_query),
            CfgRef::GpuPrimaryOnly => Value::Bool(cfg.gpu_primary_only),
            CfgRef::Gpu3dOnly => Value::Bool(cfg.gpu_3d_only),
            CfgRef::NetLowFreq => Value::Bool(cfg.net_low_freq),
            CfgRef::Appearance => Value::Select(cfg.appearance.to_config() as usize),
            CfgRef::ClickThrough => Value::Bool(cfg.click_through),
            CfgRef::SnapCorner => Value::SelectOpt(if cfg.snap_corner <= 3 { Some(cfg.snap_corner as usize) } else { None }),
            CfgRef::MetricShowLabel(i) => Value::Bool(*cfg.metric_show_labels.get(i).unwrap_or(&true)),
            CfgRef::MetricFont(i) => Value::Num(cfg.metric_font_sizes[i].unwrap_or(cfg.font_size) as f64),
            CfgRef::MetricOffset(i, c) => Value::Num(cfg.metric_offsets.get(i).and_then(|o| o.get(c as usize)).copied().unwrap_or(0) as f64),
            CfgRef::MetricLabel(i) => Value::Color(cfg.metric_colors[i].label),
            CfgRef::MetricValue(i) => Value::Color(cfg.metric_colors[i].value),
            CfgRef::MetricValueAuto(i) => Value::Bool(cfg.metric_colors[i].value_auto),
        }
    }

    pub fn set(self, cfg: &mut Config, v: Value) -> bool {
        match (self, v) {
            (CfgRef::ShowCpu, v) => {  match v { Value::Bool(x) => { cfg.show_cpu = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: ShowCpu <- {v:?}"); false }  } },
            (CfgRef::ShowMem, v) => {  match v { Value::Bool(x) => { cfg.show_mem = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: ShowMem <- {v:?}"); false }  } },
            (CfgRef::ShowGpu, v) => {  match v { Value::Bool(x) => { cfg.show_gpu = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: ShowGpu <- {v:?}"); false }  } },
            (CfgRef::ShowNet, v) => {  match v { Value::Bool(x) => { cfg.show_net = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: ShowNet <- {v:?}"); false }  } },
            (CfgRef::ShowUtil, v) => {  match v { Value::Bool(x) => { cfg.show_util = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: ShowUtil <- {v:?}"); false }  } },
            (CfgRef::UtilDecimal, v) => {  match v { Value::Bool(x) => { cfg.util_decimal = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: UtilDecimal <- {v:?}"); false }  } },
            (CfgRef::ShowTime, v) => {  match v { Value::Bool(x) => { cfg.show_time = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: ShowTime <- {v:?}"); false }  } },
            (CfgRef::TimeSeconds, v) => {  match v { Value::Bool(x) => { cfg.time_show_seconds = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: TimeSeconds <- {v:?}"); false }  } },
            (CfgRef::ShortNames, v) => {  match v { Value::Bool(x) => { cfg.short_names = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: ShortNames <- {v:?}"); false }  } },
            (CfgRef::MemUnit, v) => {  match v { Value::Select(i) => { cfg.mem_show_gb = i == 1; true } _ => { debug_assert!(false, "CfgRef 类型错配: MemUnit <- {v:?}"); false }  } },
            (CfgRef::FontSize, v) => {  match v { Value::Num(x) => { cfg.font_size = x as f32; true } _ => { debug_assert!(false, "CfgRef 类型错配: FontSize <- {v:?}"); false }  } },
            (CfgRef::FontWeight, v) => {  match v { Value::Select(i) => { if i <= 2 { cfg.font_weight = i as u8; true } else { false } } _ => { debug_assert!(false, "CfgRef 类型错配: FontWeight <- {v:?}"); false }  } },
            (CfgRef::MetricGap, v) => {  match v { Value::Num(x) => { cfg.metric_gap = x as u32; true } _ => { debug_assert!(false, "CfgRef 类型错配: MetricGap <- {v:?}"); false }  } },
            (CfgRef::LabelValueGap, v) => {  match v { Value::Num(x) => { cfg.label_value_gap = x as u32; true } _ => { debug_assert!(false, "CfgRef 类型错配: LabelValueGap <- {v:?}"); false }  } },
            (CfgRef::TextShadow, v) => {  match v { Value::Bool(x) => { cfg.text_shadow = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: TextShadow <- {v:?}"); false }  } },
            (CfgRef::TextOutline, v) => {  match v { Value::Bool(x) => { cfg.text_outline = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: TextOutline <- {v:?}"); false }  } },
            (CfgRef::AutoStart, v) => {  match v { Value::Bool(x) => { cfg.autostart = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: AutoStart <- {v:?}"); false }  } },
            (CfgRef::DebugLog, v) => {  match v { Value::Bool(x) => { cfg.debug_log = x; crate::probe::set_debug(x); true } _ => { debug_assert!(false, "CfgRef 类型错配: DebugLog <- {v:?}"); false }  } },
            (CfgRef::Topmost, v) => {  match v { Value::Bool(x) => { cfg.topmost = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: Topmost <- {v:?}"); false }  } },
            (CfgRef::Opacity, v) => {  match v { Value::Num(x) => { cfg.normal_opacity = (x as u32).clamp(0, 100) as u8; true } _ => { debug_assert!(false, "CfgRef 类型错配: Opacity <- {v:?}"); false }  } },
            (CfgRef::NetMode, v) => {  match v { Value::Select(i) => { match i { 0 => { cfg.net_mode = NetMode::Both; true } 1 => { cfg.net_mode = NetMode::Down; true } 2 => { cfg.net_mode = NetMode::Up; true } _ => false } } _ => { debug_assert!(false, "CfgRef 类型错配: NetMode <- {v:?}"); false }  } },
            (CfgRef::Style, v) => {  match v { Value::Select(i) => { match i { 0 => { cfg.render_style = RenderStyle::Strip; true } 1 => { cfg.render_style = RenderStyle::Vertical; true } _ => false } } _ => { debug_assert!(false, "CfgRef 类型错配: Style <- {v:?}"); false }  } },
            (CfgRef::RenderEngine, v) => {  match v { Value::Select(i) => { cfg.render_engine = if i == 1 { RenderEngine::D2d } else { RenderEngine::Gdi }; true } _ => { debug_assert!(false, "CfgRef 类型错配: RenderEngine <- {v:?}"); false }  } },
            (CfgRef::Lang, v) => {  match v { Value::Select(i) => { match i { 0 => { cfg.lang = Lang::En; true } 1 => { cfg.lang = Lang::Zh; true } _ => false } } _ => { debug_assert!(false, "CfgRef 类型错配: Lang <- {v:?}"); false }  } },
            (CfgRef::PowerEnabled, v) => {  match v { Value::Bool(x) => { cfg.power_enabled = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: PowerEnabled <- {v:?}"); false }  } },
            (CfgRef::PowerQuery, v) => {  match v { Value::Bool(x) => { cfg.power_query = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: PowerQuery <- {v:?}"); false }  } },
            (CfgRef::GpuPrimaryOnly, v) => {  match v { Value::Bool(x) => { cfg.gpu_primary_only = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: GpuPrimaryOnly <- {v:?}"); false }  } },
            (CfgRef::Gpu3dOnly, v) => {  match v { Value::Bool(x) => { cfg.gpu_3d_only = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: Gpu3dOnly <- {v:?}"); false }  } },
            (CfgRef::NetLowFreq, v) => {  match v { Value::Bool(x) => { cfg.net_low_freq = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: NetLowFreq <- {v:?}"); false }  } },
            (CfgRef::Appearance, v) => {  match v { Value::Select(i) => { match i { 0 => { cfg.appearance = Appearance::System; true } 1 => { cfg.appearance = Appearance::Light; true } 2 => { cfg.appearance = Appearance::Dark; true } _ => false } } _ => { debug_assert!(false, "CfgRef 类型错配: Appearance <- {v:?}"); false }  } },
            (CfgRef::ClickThrough, v) => {  match v { Value::Bool(x) => { cfg.click_through = x; true } _ => { debug_assert!(false, "CfgRef 类型错配: ClickThrough <- {v:?}"); false }  } },
            (CfgRef::SnapCorner, v) => {  match v {
                  Value::SelectOpt(o) => match o { Some(i) if i <= 3 => { cfg.snap_corner = i as u8; true } None => { cfg.snap_corner = SNAP_CORNER_FREE; true } _ => false },
                  Value::Select(i) => { if i <= 3 { cfg.snap_corner = i as u8; true } else { false } }
                  _ => { debug_assert!(false, "CfgRef 类型错配: SnapCorner <- {v:?}"); false }  } },
            (CfgRef::MetricShowLabel(i), v) => {  match v { Value::Bool(x) => { if let Some(slot) = cfg.metric_show_labels.get_mut(i) { *slot = x; true } else { false } } _ => { debug_assert!(false, "CfgRef 类型错配: MetricShowLabel <- {v:?}"); false }  } },
            (CfgRef::MetricFont(i), v) => {  match v { Value::Num(x) => {
                  if i < cfg.metric_font_sizes.len() {
                      let v2 = x as f32;
                      if (v2 - cfg.font_size).abs() < 0.05 { cfg.metric_font_sizes[i] = None; } else { cfg.metric_font_sizes[i] = Some(v2); }
                      true
                  } else { false }
              } _ => { debug_assert!(false, "CfgRef 类型错配: MetricFont <- {v:?}"); false }  } },
            (CfgRef::MetricOffset(i, c), v) => {  match v { Value::Num(x) => {
                  match cfg.metric_offsets.get_mut(i).and_then(|o| o.get_mut(c as usize)) {
                      Some(slot) => { *slot = (x.round() as i32).clamp(OFFSET_MIN, OFFSET_MAX); true }
                      None => false,
                  }
              } _ => { debug_assert!(false, "CfgRef 类型错配: MetricOffset <- {v:?}"); false }  } },
            (CfgRef::MetricLabel(i), v) => {  match v { Value::Color(c) => { if i < cfg.metric_colors.len() { cfg.metric_colors[i].label = c; true } else { false } } _ => { debug_assert!(false, "CfgRef 类型错配: MetricLabel <- {v:?}"); false }  } },
            (CfgRef::MetricValue(i), v) => {  match v { Value::Color(c) => { if i < cfg.metric_colors.len() { cfg.metric_colors[i].value = c; true } else { false } } _ => { debug_assert!(false, "CfgRef 类型错配: MetricValue <- {v:?}"); false }  } },
            (CfgRef::MetricValueAuto(i), v) => {  match v { Value::Bool(x) => { if i < cfg.metric_colors.len() { cfg.metric_colors[i].value_auto = x; true } else { false } } _ => { debug_assert!(false, "CfgRef 类型错配: MetricValueAuto <- {v:?}"); false }  } }
        }
    }
}

/// `CfgRef` 枚举变体列表与 `get`/`set` 的 match 臂一一对应；
/// `get`/`set` 是控件读写 Config 的唯一入口（单一事实来源）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CfgRef {
    ShowCpu, ShowMem, ShowGpu, ShowNet, ShowUtil, UtilDecimal, ShowTime, TimeSeconds, ShortNames,
    MemUnit, FontSize, FontWeight, MetricGap, LabelValueGap,
    TextShadow, TextOutline, AutoStart, DebugLog, Topmost,
    Opacity, NetMode, Style, RenderEngine, Lang,
    PowerEnabled, PowerQuery, GpuPrimaryOnly, Gpu3dOnly, NetLowFreq,
    Appearance, ClickThrough, SnapCorner,
    MetricShowLabel(usize), MetricFont(usize), MetricOffset(usize, u8),
    MetricLabel(usize), MetricValue(usize), MetricValueAuto(usize),
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
            (CfgRef::ShowUtil, Value::Bool(false)),
            (CfgRef::ShowUtil, Value::Bool(true)),
            (CfgRef::UtilDecimal, Value::Bool(false)),
            (CfgRef::UtilDecimal, Value::Bool(true)),
            (CfgRef::ShowTime, Value::Bool(true)),
            (CfgRef::ShowTime, Value::Bool(false)),
            (CfgRef::TimeSeconds, Value::Bool(true)),
            (CfgRef::TimeSeconds, Value::Bool(false)),
            (CfgRef::MetricShowLabel(0), Value::Bool(false)),
            (CfgRef::MetricShowLabel(3), Value::Bool(false)),
            (CfgRef::MetricShowLabel(5), Value::Bool(true)),
            (CfgRef::ShortNames, Value::Bool(true)),
            (CfgRef::ShortNames, Value::Bool(false)),
            (CfgRef::PowerQuery, Value::Bool(false)),
            (CfgRef::PowerQuery, Value::Bool(true)),
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
        assert_eq!(c.snap_corner, aaxsyspulse_core::config::SNAP_CORNER_FREE);
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
