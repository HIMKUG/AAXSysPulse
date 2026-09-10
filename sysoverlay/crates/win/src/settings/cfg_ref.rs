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
use sysoverlay_core::types::{Lang, NetMode, RenderEngine, RenderStyle};

/// 刷新率分段下标 → interval_ms（0 = Auto 档）。
pub const RATE_MS: [u32; 6] = [0, 100, 200, 1000, 2000, 5000];

/// 控件写入/读出的泛化值。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    /// 开关类。
    Bool(bool),
    /// 滑杆/数值类（字号、间距、透明度、动画时长、窗口宽高）。
    Num(f64),
    /// 分段选择类（样式、形状、内存单位、刷新率、语言）。
    Select(usize),
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
    TextShadow,
    TextOutline,
    Topmost,
    AutoStart,
    DebugLog,
    Hover,
    /// 固定悬浮窗尺寸（window_size.is_some()）。
    LockSize,
    GpuPrimaryOnly,
    Gpu3dOnly,
    NetLowFreq,
    /// 网络栏显示模式：0=双向 1=仅下载 2=仅上传。
    NetMode,

    // ── 数值滑杆 ──
    FontSize,
    MetricGap,
    LabelValueGap,
    HoverOpacity,
    NormalOpacity,
    AnimMs,
    /// 悬浮窗宽（window_size.0；None 时读默认 300）。
    WinW,
    /// 悬浮窗高（window_size.1；None 时读默认 200）。
    WinH,
    /// 逐指标字号（下标 0..5）。
    MetricFont(usize),

    // ── 分段选择 ──
    Style,
    /// 内存单位：0 = 百分比，1 = GB。
    MemUnit,
    /// 刷新率分段（0 = Auto，其余见 [`RATE_MS`]）。
    Rate,
    /// 界面语言：0 = En，1 = Zh。
    Lang,
    /// 悬浮窗渲染引擎：0 = 性能（像素块），1 = 高画质（D2D + DirectWrite）。
    RenderEngine,
    /// 字重档位：0 = 细，1 = 中，2 = 粗（默认）。
    FontWeight,

    // ── 指标颜色 ──
    /// 指标名称色（下标 0..5）。
    MetricLabel(usize),
    /// 指标数值色（下标 0..5）。
    MetricValue(usize),
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
            CfgRef::TextShadow => Value::Bool(cfg.text_shadow),
            CfgRef::TextOutline => Value::Bool(cfg.text_outline),
            CfgRef::Topmost => Value::Bool(cfg.topmost),
            CfgRef::AutoStart => Value::Bool(cfg.autostart),
            CfgRef::DebugLog => Value::Bool(cfg.debug_log),
            CfgRef::Hover => Value::Bool(cfg.hover),
            CfgRef::LockSize => Value::Bool(cfg.size_locked),
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
            // num
            CfgRef::FontSize => Value::Num(cfg.font_size as f64),
            CfgRef::MetricGap => Value::Num(cfg.metric_gap as f64),
            CfgRef::LabelValueGap => Value::Num(cfg.label_value_gap as f64),
            CfgRef::HoverOpacity => Value::Num(cfg.hover_opacity as f64),
            CfgRef::NormalOpacity => Value::Num(cfg.normal_opacity as f64),
            CfgRef::AnimMs => Value::Num(cfg.anim_ms as f64),
            CfgRef::WinW => Value::Num(cfg.window_size.map_or(300.0, |s| s.0 as f64)),
            CfgRef::WinH => Value::Num(cfg.window_size.map_or(200.0, |s| s.1 as f64)),
            CfgRef::MetricFont(i) => {
                Value::Num(cfg.metric_font_sizes[i].unwrap_or(cfg.font_size) as f64)
            }
            // select
            // Style 分段控件只有 2 档（横版/竖版）；枚举里 Classic 是历史遗留
            // （parse 阶段已迁移为 Strip）。此前 get 直接用 `as usize`（Classic=0/
            // Strip=1/Vertical=2）而 set 也按 3 值映射 → 点「竖版」(下标 1) 实际
            // 写成 Strip，竖版永远不生效、高亮也错位。
            CfgRef::Style => Value::Select(match cfg.render_style {
                RenderStyle::Vertical => 1,
                _ => 0,
            }),
            CfgRef::MemUnit => Value::Select(if cfg.mem_show_gb { 1 } else { 0 }),
            CfgRef::Rate => Value::Select(if cfg.refresh_auto {
                0
            } else {
                match cfg.interval_ms {
                    100 => 1,
                    200 => 2,
                    1000 => 3,
                    2000 => 4,
                    _ => 5,
                }
            }),
            CfgRef::Lang => Value::Select(cfg.lang as usize),
            CfgRef::RenderEngine => Value::Select(if cfg.render_engine == RenderEngine::D2d { 1 } else { 0 }),
            CfgRef::FontWeight => Value::Select(cfg.font_weight as usize),
            // color
    CfgRef::MetricLabel(i) => Value::Color(cfg.metric_colors[i].label),
    CfgRef::MetricValue(i) => Value::Color(cfg.metric_colors[i].value),
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
            (CfgRef::TextShadow, Value::Bool(x)) => cfg.text_shadow = x,
            (CfgRef::TextOutline, Value::Bool(x)) => cfg.text_outline = x,
            (CfgRef::Topmost, Value::Bool(x)) => cfg.topmost = x,
            (CfgRef::AutoStart, Value::Bool(x)) => cfg.autostart = x,
            (CfgRef::DebugLog, Value::Bool(x)) => cfg.debug_log = x,
            (CfgRef::Hover, Value::Bool(x)) => cfg.hover = x,
            (CfgRef::LockSize, Value::Bool(x)) => {
                // 锁定语义独立于几何恢复：window_size 永远保留（重启后恢复
                // 上次尺寸用），fit_window 只认 size_locked。
                cfg.size_locked = x;
                if x {
                    cfg.window_size.get_or_insert((300, 200));
                }
            }
            (CfgRef::GpuPrimaryOnly, Value::Bool(x)) => cfg.gpu_primary_only = x,
            (CfgRef::Gpu3dOnly, Value::Bool(x)) => cfg.gpu_3d_only = x,
            (CfgRef::NetLowFreq, Value::Bool(x)) => cfg.net_low_freq = x,
            (CfgRef::NetMode, Value::Select(i)) => match i {
                0 => cfg.net_mode = NetMode::Both,
                1 => cfg.net_mode = NetMode::Down,
                2 => cfg.net_mode = NetMode::Up,
                _ => return false,
            },
            // num
            (CfgRef::FontSize, Value::Num(x)) => cfg.font_size = x as f32,
            (CfgRef::MetricGap, Value::Num(x)) => cfg.metric_gap = x as u32,
            (CfgRef::LabelValueGap, Value::Num(x)) => cfg.label_value_gap = x as u32,
            (CfgRef::HoverOpacity, Value::Num(x)) => cfg.hover_opacity = x as u8,
            (CfgRef::NormalOpacity, Value::Num(x)) => cfg.normal_opacity = x as u8,
            (CfgRef::AnimMs, Value::Num(x)) => cfg.anim_ms = x as u32,
            (CfgRef::WinW, Value::Num(x)) => {
                let h = cfg.window_size.map_or(200, |s| s.1);
                cfg.window_size = Some((x as i32, h));
            }
            (CfgRef::WinH, Value::Num(x)) => {
                let w = cfg.window_size.map_or(300, |s| s.0);
                cfg.window_size = Some((w, x as i32));
            }
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
            (CfgRef::Rate, Value::Select(i)) => {
                if i == 0 {
                    cfg.refresh_auto = true;
                } else if i < RATE_MS.len() {
                    cfg.refresh_auto = false;
                    cfg.interval_ms = RATE_MS[i];
                } else {
                    return false;
                }
            }
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
            (CfgRef::PowerEnabled, Value::Bool(true)),
            (CfgRef::TextShadow, Value::Bool(false)),
            (CfgRef::TextOutline, Value::Bool(false)),
            (CfgRef::Topmost, Value::Bool(false)),
            (CfgRef::AutoStart, Value::Bool(true)),
            (CfgRef::DebugLog, Value::Bool(false)),
            (CfgRef::Hover, Value::Bool(false)),
            (CfgRef::LockSize, Value::Bool(true)),
            (CfgRef::GpuPrimaryOnly, Value::Bool(false)),
            (CfgRef::Gpu3dOnly, Value::Bool(false)),
            (CfgRef::NetLowFreq, Value::Bool(false)),
            (CfgRef::NetMode, Value::Select(0)),
            (CfgRef::NetMode, Value::Select(1)),
            (CfgRef::NetMode, Value::Select(2)),
            (CfgRef::FontSize, Value::Num(24.0)),
            (CfgRef::MetricGap, Value::Num(40.0)),
            (CfgRef::LabelValueGap, Value::Num(30.0)),
            (CfgRef::HoverOpacity, Value::Num(90.0)),
            (CfgRef::NormalOpacity, Value::Num(35.0)),
            (CfgRef::AnimMs, Value::Num(450.0)),
            (CfgRef::WinW, Value::Num(420.0)),
            (CfgRef::WinH, Value::Num(160.0)),
            (CfgRef::MetricFont(2), Value::Num(18.0)),
            (CfgRef::Style, Value::Select(1)),
            (CfgRef::MemUnit, Value::Select(1)),
            (CfgRef::Rate, Value::Select(3)),
            (CfgRef::Lang, Value::Select(1)),
            (CfgRef::RenderEngine, Value::Select(1)),
            (CfgRef::FontWeight, Value::Select(0)),
            (CfgRef::FontWeight, Value::Select(1)),
            (CfgRef::FontWeight, Value::Select(2)),
            (CfgRef::MetricLabel(0), Value::Color(Some(Rgb::new(10, 200, 255)))),
            (CfgRef::MetricValue(4), Value::Color(None)),
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
    fn lock_size_on_then_off_only_flips_lock_flag() {
        // 新语义：window_size 是「几何恢复」值，开关只动 size_locked——
        // 关闭锁定不得清掉几何（否则重启后窗口尺寸丢失）。
        let mut c = cfg();
        assert!(CfgRef::LockSize.set(&mut c, Value::Bool(true)));
        assert!(c.size_locked);
        assert!(c.window_size.is_some());
        assert!(CfgRef::LockSize.set(&mut c, Value::Bool(false)));
        assert!(!c.size_locked);
        assert!(c.window_size.is_some(), "关闭锁定只解锁，不清几何恢复值");
    }

    #[test]
    fn lock_size_on_preserves_existing_size() {
        let mut c = cfg();
        c.window_size = Some((640, 480));
        assert!(CfgRef::LockSize.set(&mut c, Value::Bool(true)));
        assert_eq!(c.window_size, Some((640, 480)), "重复开启不得重置已设尺寸");
    }

    #[test]
    fn win_w_set_keeps_existing_height() {
        let mut c = cfg();
        c.window_size = Some((300, 200));
        assert!(CfgRef::WinW.set(&mut c, Value::Num(500.0)));
        assert_eq!(c.window_size, Some((500, 200)), "改宽不得动高");
        assert!(CfgRef::WinH.set(&mut c, Value::Num(240.0)));
        assert_eq!(c.window_size, Some((500, 240)), "改高不得动宽");
    }

    #[test]
    fn rate_auto_uses_index_zero_and_others_map_to_ms() {
        let mut c = cfg();
        assert!(CfgRef::Rate.set(&mut c, Value::Select(0)));
        assert!(c.refresh_auto);
        assert!(CfgRef::Rate.set(&mut c, Value::Select(1)));
        assert!(!c.refresh_auto);
        assert_eq!(c.interval_ms, 100);
        assert!(CfgRef::Rate.set(&mut c, Value::Select(5)));
        assert_eq!(c.interval_ms, 5000);
    }

    #[test]
    fn metric_font_none_falls_back_to_global_font_size() {
        let mut c = cfg();
        c.font_size = 22.0;
        assert_eq!(CfgRef::MetricFont(0).get(&c), Value::Num(22.0));
        assert!(CfgRef::MetricFont(0).set(&mut c, Value::Num(15.0)));
        assert_eq!(c.metric_font_sizes[0], Some(15.0));
    }

    #[test]
    fn out_of_range_index_is_rejected_not_panicking() {
        let mut c = cfg();
        assert!(!CfgRef::MetricFont(9).set(&mut c, Value::Num(1.0)));
        assert!(!CfgRef::MetricLabel(9).set(&mut c, Value::Color(None)));
        assert!(!CfgRef::Rate.set(&mut c, Value::Select(99)));
        assert!(!CfgRef::Lang.set(&mut c, Value::Select(7)));
    }

    #[test]
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
}
