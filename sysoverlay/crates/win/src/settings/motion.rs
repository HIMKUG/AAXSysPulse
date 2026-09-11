//! Apple 流体动效：弹簧积分、动量投射、橡胶边界、系统可访问性探测。
//!
//! 参数取自 apple-design `spring-param-ref.md`（Apple WWDC 官方值）：
//! 常规 UI ζ=1.0 / response=0.4s；面板与手势 ζ=0.8 / response=0.3s。
//! 映射：ω₀ = 2π / response，k = ω₀²，c = 2ζω₀（质量 = 1）。

/// 常规 UI 过渡（悬停 / 按压 / 选中 / 换肤）：ζ=1.0，response=0.4s。
pub const K_UI: f32 = 246.74;
/// 常规 UI 过渡阻尼（ζ=1.0）。
pub const C_UI: f32 = 31.416;

/// 面板与手势（折叠展开 / 翻页 / 滚轮惯性）：ζ=0.8，response=0.3s。
pub const K_SHEET: f32 = 438.65;
/// 面板与手势阻尼（ζ=0.8）。
pub const C_SHEET: f32 = 33.510;

/// 减弱动效时的响应（≈ 瞬时）。
pub const K_REDUCED: f32 = 4000.0;
/// 减弱动效时的阻尼。
pub const C_REDUCED: f32 = 200.0;

/// 定步长子步长：帧率无关且数值稳定。
const DT: f32 = 1.0 / 240.0;

/// 单状态弹簧（半隐式欧拉积分，先速度后位置，比显式欧拉稳定）。
#[derive(Clone, Copy, Debug, Default)]
pub struct Spring {
    /// 当前值。
    pub x: f32,
    /// 当前速度。
    pub v: f32,
}

impl Spring {
    pub fn new(x: f32) -> Self {
        Self { x, v: 0.0 }
    }

    /// 推进 `frame_dt` 秒；返回 true 表示已收敛（可停止重绘，避免空转烧 CPU）。
    pub fn step(&mut self, target: f32, frame_dt: f32, k: f32, c: f32) -> bool {
        let mut left = frame_dt.clamp(0.0, 0.1);
        while left > 0.0 {
            let dt = left.min(DT);
            let a = k * (target - self.x) - c * self.v;
            self.v += a * dt;
            self.x += self.v * dt;
            left -= dt;
        }
        (target - self.x).abs() < 0.002 && self.v.abs() < 0.02
    }

    /// 直接置位并清零速度（用于跳变、可中断动画的起点移交）。
    pub fn set(&mut self, x: f32) {
        self.x = x;
        self.v = 0.0;
    }
}

/// Apple 官方动量投射（decelerationRate = 0.998）。
/// 给定当前速度（px/s），返回吸附终点相对当前位置的位移量。
pub fn project(initial_velocity: f32) -> f32 {
    (initial_velocity / 1000.0) * 0.998 / (1.0 - 0.998)
}

/// 橡胶边界：越界 `overshoot` 时实际可移动的距离（对数压缩）。
/// 越界越多，位移增量越小，呈现「拉扯回弹」手感而非硬截断。
pub fn rubber_band(overshoot: f32, dimension: f32, coeff: f32) -> f32 {
    if overshoot <= 0.0 || dimension <= 0.0 {
        return 0.0;
    }
    (1.0 - 1.0 / (overshoot * coeff / dimension + 1.0)) * dimension
}

use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETHIGHCONTRAST, SPI_GETCLIENTAREAANIMATION,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW, HIGHCONTRASTW_FLAGS};

static ANIM_CACHED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
static HC_CACHED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// 系统是否启用客户端区动画（「减弱动效」关闭时为 false）。
/// 结果缓存——系统设置不会逐帧变化，无需重复查询。
pub fn animations_enabled() -> bool {
    let c = ANIM_CACHED.load(std::sync::atomic::Ordering::Relaxed);
    if c >= 0 {
        return c == 1;
    }
    let mut on = windows::Win32::Foundation::BOOL(1);
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut on as *mut _ as *mut std::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    // 调用失败或显式关闭 → 视为关闭；其余视为开启（安全兜底：开启动效）。
    let v = if ok.is_ok() && !on.as_bool() { 0 } else { 1 };
    ANIM_CACHED.store(v, std::sync::atomic::Ordering::Relaxed);
    v == 1
}

/// 高对比度模式（卡片描边全色、禁用半透明选中态）。
/// 结果缓存。
pub fn high_contrast() -> bool {
    let c = HC_CACHED.load(std::sync::atomic::Ordering::Relaxed);
    if c >= 0 {
        return c == 1;
    }
    let mut hc = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            std::mem::size_of::<HIGHCONTRASTW>() as u32,
            Some(&mut hc as *mut _ as *mut std::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    let v = if ok.is_ok() && (hc.dwFlags & HCF_HIGHCONTRASTON) != HIGHCONTRASTW_FLAGS(0) {
        1
    } else {
        0
    };
    HC_CACHED.store(v, std::sync::atomic::Ordering::Relaxed);
    v == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_converges_to_target() {
        let mut s = Spring::new(0.0);
        for _ in 0..240 {
            s.step(1.0, 1.0 / 120.0, K_UI, C_UI);
        }
        assert!((s.x - 1.0).abs() < 0.01, "1s 后应到目标，实际 {}", s.x);
    }

    #[test]
    fn critical_damping_never_overshoots() {
        // ζ=1.0 临界阻尼：不得越过目标。
        let mut s = Spring::new(0.0);
        let mut max = 0.0f32;
        for _ in 0..480 {
            s.step(1.0, 1.0 / 240.0, K_UI, C_UI);
            max = max.max(s.x);
        }
        assert!(max <= 1.001, "临界阻尼不得过冲，峰值 {max}");
    }

    #[test]
    fn panel_spring_slightly_overshoots() {
        // ζ=0.8：允许小幅过冲（Apple sheet 手感）。
        let mut s = Spring::new(0.0);
        let mut max = 0.0f32;
        for _ in 0..480 {
            s.step(1.0, 1.0 / 240.0, K_SHEET, C_SHEET);
            max = max.max(s.x);
        }
        assert!(max > 1.0 && max < 1.15, "ζ=0.8 应小幅过冲，峰值 {max}");
    }

    #[test]
    fn rubber_band_is_compressive() {
        // 越界越多，位移增量越小（二阶差分递减）。
        let d1 = rubber_band(10.0, 400.0, 0.55);
        let d2 = rubber_band(20.0, 400.0, 0.55);
        assert!(d2 > d1, "越界位移应单调增");
        assert!(d2 - d1 < d1, "增量必须递减（橡胶感）");
    }

    #[test]
    fn projected_endpoint_grows_with_velocity() {
        assert!(project(2000.0) > project(1000.0));
    }
}
