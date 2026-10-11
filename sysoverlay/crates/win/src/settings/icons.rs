//! 侧边栏 / 窗口按钮图标：内嵌 SVG 路径 → D2D 填充几何。
//!
//! # 图标来源
//! `icons_svg/` 下 8 个 1024² 单色 SVG（用户提供），编译期用 `include_str!` 内嵌。
//! 这些文件已**预归一化**为绝对 `M/L/C/Z`（圆弧按 SVG 规范算法转为三次贝塞尔），
//! 因此运行期解析器无需处理相对坐标与圆弧——相关分支仅为兼容保留。
//! 运行期只在**首帧**解析一次（`OnceLock` 缓存）；此后逐帧复用 `Gfx` 内的路径几何缓存。
//! 换图标只需替换 `icons_svg/` 下的文件（保持绝对 `M/L/C/Z` 即可），本文件无需改动。
//!
//! # 尺寸契约
//! 路径按 viewBox [`VIEW`] 归一；绘制时线性映射到调用方给的 `rect`，
//! 因此任意 DPI / 尺寸下都清晰。

use std::sync::OnceLock;

use windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget;

use crate::gfx_d2d::{ColorF, Gfx, PathSeg, RectF};

/// viewBox 边长（全部图标统一 1024）。
const VIEW: f32 = 1024.0;

/// 图标标识。前 7 个对应侧边栏导航项，顺序即侧边栏顺序。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconKind {
    General,
    Layout,
    Personalize,
    Window,
    Performance,
    /// 诊断页（采样器状态）。复用 SOURCES 里空闲的 reset.svg（环形刷新 ≈ 重新检测）。
    Diag,
    About,
    /// 窗口控制按钮「固定」。
    Pin,
    // ── Fluent 图标（S6 接入，坐标已归一化到 1024 空间）──
    // 尚无 UI 消费方（供后续控件使用），消费后移除 allow。
    #[allow(dead_code)]
    ChevronUp,
    #[allow(dead_code)]
    ChevronDown,
    #[allow(dead_code)]
    ChevronRight,
    #[allow(dead_code)]
    Copy,
    #[allow(dead_code)]
    Dismiss,
    #[allow(dead_code)]
    Checkmark,
}

impl IconKind {
    /// 侧边栏 7 项（顺序 = 页面下标：General/Layout/Metrics/Window/Performance/Diag/About）。
    pub const ALL: [IconKind; 7] = [
        IconKind::General,
        IconKind::Layout,
        IconKind::Personalize,
        IconKind::Window,
        IconKind::Performance,
        IconKind::Diag,
        IconKind::About,
    ];

    /// 在 [`SOURCES`] / 路径缓存中的下标。
    fn index(self) -> usize {
        match self {
            IconKind::General => 0,
            IconKind::Layout => 1,
            IconKind::Personalize => 2,
            IconKind::Window => 3,
            IconKind::Performance => 4,
            IconKind::Diag => 6,
            IconKind::About => 5,
            IconKind::Pin => 7,
            IconKind::ChevronUp => 8,
            IconKind::ChevronDown => 9,
            IconKind::ChevronRight => 10,
            IconKind::Copy => 11,
            IconKind::Dismiss => 12,
            IconKind::Checkmark => 13,
        }
    }
}

/// 内嵌 SVG 源（顺序与 [`IconKind::index`] 一致）。
const SOURCES: [&str; 14] = [
    include_str!("icons_svg/settings.svg"),
    include_str!("icons_svg/layout.svg"),
    include_str!("icons_svg/palette.svg"),
    include_str!("icons_svg/window.svg"),
    include_str!("icons_svg/performance.svg"),
    include_str!("icons_svg/info.svg"),
    // 下标 6：reset.svg（曾用于行尾重置按钮；色板「跟随默认」格替代后保留占位，
    // 使 Pin 的下标稳定不变）。
    include_str!("icons_svg/reset.svg"),
    include_str!("icons_svg/pin.svg"),
    include_str!("icons_svg/chevron_up.svg"),
    include_str!("icons_svg/chevron_down.svg"),
    include_str!("icons_svg/chevron_right.svg"),
    include_str!("icons_svg/copy.svg"),
    include_str!("icons_svg/dismiss.svg"),
    include_str!("icons_svg/checkmark.svg"),
];

static PATHS: OnceLock<[Vec<PathSeg>; 14]> = OnceLock::new();

fn paths() -> &'static [Vec<PathSeg>; 14] {
    PATHS.get_or_init(|| std::array::from_fn(|i| parse(svg_path_data(SOURCES[i]))))
}

/// 取出 SVG 中 `d="…"` 的属性值（本项目图标均为单路径）。
fn svg_path_data(src: &str) -> &str {
    let Some(pos) = src.find(" d=\"") else { return "" };
    let start = pos + 4;
    match src[start..].find('"') {
        Some(end) => &src[start..start + end],
        None => "",
    }
}

/// SVG 椭圆弧端点参数化 → 圆心参数化（W3C 规范 F.6.5），切分 ≤90° 段，
/// 每段升为三次贝塞尔（kappa = 4/3·tan(Δθ/4)）。PathSeg 无弧段类型，
/// 这是 Fluent 图标（大量圆弧）能被本模块消费的唯一通路。
fn arc_to_cubics(
    out: &mut Vec<PathSeg>,
    x1: f32,
    y1: f32,
    rx: f32,
    ry: f32,
    rot_deg: f32,
    large: bool,
    sweep: bool,
    x2: f32,
    y2: f32,
) {
    if rx <= 0.0 || ry <= 0.0 {
        out.push(PathSeg::Line(x2, y2));
        return;
    }
    let (rx, ry) = (rx.abs(), ry.abs());
    let phi = rot_deg.to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();
    // F.6.5.1：端点 → 椭圆局部坐标
    let (dx2, dy2) = ((x1 - x2) / 2.0, (y1 - y2) / 2.0);
    let x1p = cos_phi * dx2 + sin_phi * dy2;
    let y1p = -sin_phi * dx2 + cos_phi * dy2;
    // F.6.5.2：半径不足以跨越弦长时等比放大
    let lambda = x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry);
    let (rx, ry) = if lambda > 1.0 {
        let s = lambda.sqrt();
        (rx * s, ry * s)
    } else {
        (rx, ry)
    };
    // F.6.5.3：圆心
    let sign = if large != sweep { 1.0 } else { -1.0 };
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let co = if den.abs() < 1e-12 {
        0.0
    } else {
        sign * (num / den).max(0.0).sqrt()
    };
    let cxp = co * rx * y1p / ry;
    let cyp = -co * ry * x1p / rx;
    let cx0 = cos_phi * cxp - sin_phi * cyp + (x1 + x2) / 2.0;
    let cy0 = sin_phi * cxp + cos_phi * cyp + (y1 + y2) / 2.0;
    // F.6.5.4：起角与扫角
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| -> f32 {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let mut a = (if len.abs() < 1e-12 { 0.0 } else { dot / len })
            .clamp(-1.0, 1.0)
            .acos();
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    };
    let (ux1, uy1) = ((x1p - cxp) / rx, (y1p - cyp) / ry);
    let (ux2, uy2) = ((-x1p - cxp) / rx, (-y1p - cyp) / ry);
    let theta1 = angle(1.0, 0.0, ux1, uy1);
    let mut dtheta = angle(ux1, uy1, ux2, uy2);
    if !sweep && dtheta > 0.0 {
        dtheta -= 2.0 * std::f32::consts::PI;
    } else if sweep && dtheta < 0.0 {
        dtheta += 2.0 * std::f32::consts::PI;
    }
    // 切分 ≤90° 并升阶。注意 sin_cos() 返回 (sin, cos)，先解构再按正余弦使用。
    let n = ((dtheta.abs() / (std::f32::consts::FRAC_PI_2)).ceil() as usize).max(1);
    let delta = dtheta / n as f32;
    let t = 4.0 / 3.0 * (delta / 4.0).tan();
    let mut th = theta1;
    for _ in 0..n {
        let th2 = th + delta;
        let (s1, c1) = th.sin_cos();
        let (s2, c2) = th2.sin_cos();
        let px = |ex: f32, ey: f32| {
            (
                cx0 + rx * ex * cos_phi - ry * ey * sin_phi,
                cy0 + rx * ex * sin_phi + ry * ey * cos_phi,
            )
        };
        let (ex, ey) = px(c2, s2);
        let (c1x, c1y) = px(c1 - t * s1, s1 + t * c1);
        let (c2x, c2y) = px(c2 + t * s2, s2 - t * c2);
        out.push(PathSeg::Cubic(c1x, c1y, c2x, c2y, ex, ey));
        th = th2;
    }
}

/// 极简 SVG path 解析：支持 `M/m L/l H/h V/v C/c S/s Q/q T/t A/a Z/z`。
/// 二次贝塞尔（Q/T）与椭圆弧（A，W3C F.6.5 升阶）统一转为三次（C），
/// 因此下游只需一种曲线段。未知命令的参数会被跳过，不影响其余子路径。
fn parse(d: &str) -> Vec<PathSeg> {
    let mut out: Vec<PathSeg> = Vec::new();
    let bytes = d.as_bytes();
    let mut i = 0usize;
    let mut cmd = 'M';
    let mut prev = 'M';
    let (mut cx, mut cy) = (0.0f32, 0.0f32); // 当前点
    let (mut sx, mut sy) = (0.0f32, 0.0f32); // 子路径起点
    let (mut qcx, mut qcy) = (0.0f32, 0.0f32); // 上一二次控制点（T 反射用）
    let (mut pcx, mut pcy) = (0.0f32, 0.0f32); // 上一三次第二控制点（S 反射用）

    // 读取下一个数值（跳过空白与逗号）；None = 无可读数值。
    let read_num = |i: &mut usize| -> Option<f32> {
        while *i < bytes.len() {
            let c = bytes[*i] as char;
            if c == ',' || c.is_ascii_whitespace() {
                *i += 1;
            } else {
                break;
            }
        }
        if *i >= bytes.len() {
            return None;
        }
        let s = *i;
        if bytes[*i] == b'-' || bytes[*i] == b'+' {
            *i += 1;
        }
        let mut dot = false;
        while *i < bytes.len() {
            let c = bytes[*i];
            if c.is_ascii_digit() {
                *i += 1;
            } else if c == b'.' && !dot {
                dot = true;
                *i += 1;
            } else {
                break;
            }
        }
        if *i < bytes.len() && (bytes[*i] == b'e' || bytes[*i] == b'E') {
            *i += 1;
            if *i < bytes.len() && (bytes[*i] == b'-' || bytes[*i] == b'+') {
                *i += 1;
            }
            while *i < bytes.len() && bytes[*i].is_ascii_digit() {
                *i += 1;
            }
        }
        if *i == s {
            return None;
        }
        d[s..*i].parse::<f32>().ok()
    };

    // 读取弧标记（恰一个 '0'/'1' 字符）。SVG 允许标记与后续数字紧邻（如 "01.5"），
    // 用 read_num 读会把 "01" 吞成一个数，必须按单字符读。
    let read_flag = |i: &mut usize| -> Option<bool> {
        while *i < bytes.len() {
            let c = bytes[*i] as char;
            if c == ',' || c.is_ascii_whitespace() {
                *i += 1;
            } else {
                break;
            }
        }
        match bytes.get(*i) {
            Some(b'0') => {
                *i += 1;
                Some(false)
            }
            Some(b'1') => {
                *i += 1;
                Some(true)
            }
            _ => None,
        }
    };

    // 二次 → 三次升阶。
    let quad = |out: &mut Vec<PathSeg>, x0: f32, y0: f32, qx: f32, qy: f32, x: f32, y: f32| {
        let c1 = (x0 + 2.0 / 3.0 * (qx - x0), y0 + 2.0 / 3.0 * (qy - y0));
        let c2 = (x + 2.0 / 3.0 * (qx - x), y + 2.0 / 3.0 * (qy - y));
        out.push(PathSeg::Cubic(c1.0, c1.1, c2.0, c2.1, x, y));
    };

    while i < bytes.len() {
        // 记录**本轮起点**：只有整轮都没推进时才强制前进（防死循环）。
        // 必须放在命令字母判断之前——否则会吞掉紧跟 `z` 的下一个命令字母。
        let before = i;
        // 先跳过命令间分隔符（空白 / 逗号），再判断是否为命令字母。
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c == ',' || c.is_ascii_whitespace() {
                i += 1;
            } else {
                break;
            }
        }
        if i >= bytes.len() {
            break;
        }
        let ch = bytes[i] as char;
        if ch.is_ascii_alphabetic() {
            cmd = ch;
            prev = cmd.to_ascii_lowercase();
            i += 1;
            // 注意：不 continue——无参命令（z/Z）需在本轮立即执行。
        }
        let lower = cmd.to_ascii_lowercase();
        let rel = cmd.is_ascii_lowercase();
        match lower {
            'm' => {
                let (Some(x), Some(y)) = (read_num(&mut i), read_num(&mut i)) else { break };
                if rel {
                    cx += x;
                    cy += y;
                } else {
                    cx = x;
                    cy = y;
                }
                sx = cx;
                sy = cy;
                out.push(PathSeg::Move(cx, cy));
                // 后续坐标对按 lineto 处理（SVG 语义）。
                cmd = if rel { 'l' } else { 'L' };
            }
            'l' => {
                let (Some(x), Some(y)) = (read_num(&mut i), read_num(&mut i)) else { break };
                if rel {
                    cx += x;
                    cy += y;
                } else {
                    cx = x;
                    cy = y;
                }
                out.push(PathSeg::Line(cx, cy));
            }
            'h' => {
                let Some(x) = read_num(&mut i) else { break };
                cx = if rel { cx + x } else { x };
                out.push(PathSeg::Line(cx, cy));
            }
            'v' => {
                let Some(y) = read_num(&mut i) else { break };
                cy = if rel { cy + y } else { y };
                out.push(PathSeg::Line(cx, cy));
            }
            'c' => {
                let (Some(x1), Some(y1), Some(x2), Some(y2), Some(x3), Some(y3)) = (
                    read_num(&mut i),
                    read_num(&mut i),
                    read_num(&mut i),
                    read_num(&mut i),
                    read_num(&mut i),
                    read_num(&mut i),
                ) else {
                    break;
                };
                let (ax, ay, bx, by, ex, ey) = if rel {
                    (cx + x1, cy + y1, cx + x2, cy + y2, cx + x3, cy + y3)
                } else {
                    (x1, y1, x2, y2, x3, y3)
                };
                out.push(PathSeg::Cubic(ax, ay, bx, by, ex, ey));
                pcx = bx;
                pcy = by;
                cx = ex;
                cy = ey;
            }
            's' => {
                let (Some(x2), Some(y2), Some(x3), Some(y3)) = (
                    read_num(&mut i),
                    read_num(&mut i),
                    read_num(&mut i),
                    read_num(&mut i),
                ) else {
                    break;
                };
                let (ax, ay) = if matches!(prev, 'c' | 's') {
                    (2.0 * cx - pcx, 2.0 * cy - pcy)
                } else {
                    (cx, cy)
                };
                let (bx, by, ex, ey) = if rel {
                    (cx + x2, cy + y2, cx + x3, cy + y3)
                } else {
                    (x2, y2, x3, y3)
                };
                out.push(PathSeg::Cubic(ax, ay, bx, by, ex, ey));
                pcx = bx;
                pcy = by;
                cx = ex;
                cy = ey;
            }
            'q' => {
                let (Some(x1), Some(y1), Some(x2), Some(y2)) = (
                    read_num(&mut i),
                    read_num(&mut i),
                    read_num(&mut i),
                    read_num(&mut i),
                ) else {
                    break;
                };
                let (qx, qy, ex, ey) = if rel {
                    (cx + x1, cy + y1, cx + x2, cy + y2)
                } else {
                    (x1, y1, x2, y2)
                };
                quad(&mut out, cx, cy, qx, qy, ex, ey);
                qcx = qx;
                qcy = qy;
                cx = ex;
                cy = ey;
            }
            't' => {
                let (Some(x2), Some(y2)) = (read_num(&mut i), read_num(&mut i)) else { break };
                let (qx, qy) = if matches!(prev, 'q' | 't') {
                    (2.0 * cx - qcx, 2.0 * cy - qcy)
                } else {
                    (cx, cy)
                };
                let (ex, ey) = if rel { (cx + x2, cy + y2) } else { (x2, y2) };
                quad(&mut out, cx, cy, qx, qy, ex, ey);
                qcx = qx;
                qcy = qy;
                cx = ex;
                cy = ey;
            }
            'z' => {
                out.push(PathSeg::Close);
                cx = sx;
                cy = sy;
            }
            'a' => {
                // SVG 椭圆弧（Fluent 图标大量使用）→ 升为 ≤90° 的三次贝塞尔段。
                // 参数组：rx ry rot large sweep x y，可连续重复（隐式重复同一命令）。
                while let (Some(rx), Some(ry), Some(rot)) =
                    (read_num(&mut i), read_num(&mut i), read_num(&mut i))
                {
                    let (Some(large), Some(sweep)) = (read_flag(&mut i), read_flag(&mut i)) else {
                        break;
                    };
                    let (Some(ax), Some(ay)) = (read_num(&mut i), read_num(&mut i)) else {
                        break;
                    };
                    let (ex, ey) = if rel { (cx + ax, cy + ay) } else { (ax, ay) };
                    arc_to_cubics(&mut out, cx, cy, rx, ry, rot, large, sweep, ex, ey);
                    cx = ex;
                    cy = ey;
                }
            }
            // 未支持命令：跳过其参数，保持解析继续。
            _ => {
                while i < bytes.len() && !(bytes[i] as char).is_ascii_alphabetic() {
                    i += 1;
                }
            }
        }
        if i == before {
            i += 1;
        }
    }
    out
}

/// 在 `rect` 内以 `color` 填充绘制图标（`rect` 建议为正方形）。
pub fn draw(
    rt: &ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    kind: IconKind,
    rect: RectF,
    color: ColorF,
) {
    let idx = kind.index();
    let segs = &paths()[idx];
    if segs.is_empty() {
        return;
    }
    gfx.fill_path(rt, idx, segs, VIEW, rect, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只取锚点（Move/Line 终点、Cubic 终点）：贝塞尔控制点允许越出 viewBox。
    fn each_anchor(segs: &[PathSeg], mut f: impl FnMut(f32, f32)) {
        for s in segs {
            match *s {
                PathSeg::Move(x, y) | PathSeg::Line(x, y) => f(x, y),
                PathSeg::Cubic(_, _, _, _, x, y) => f(x, y),
                PathSeg::Close => {}
            }
        }
    }

    #[test]
    fn all_icons_parse_non_empty() {
        for (i, segs) in paths().iter().enumerate() {
            assert!(!segs.is_empty(), "icon #{i} 解析为空（检查 icons_svg 与解析器）");
        }
    }

    #[test]
    fn every_icon_stays_within_viewbox() {
        const EPS: f32 = 24.0; // 允许手绘 SVG 锚点轻微越界
        for (i, segs) in paths().iter().enumerate() {
            let (mut mnx, mut mny, mut mxx, mut mxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            each_anchor(segs, |x, y| {
                mnx = mnx.min(x);
                mny = mny.min(y);
                mxx = mxx.max(x);
                mxy = mxy.max(y);
            });
            let w = mxx - mnx;
            assert!(mnx >= -EPS && mny >= -EPS, "icon #{i} 越出 viewBox 左上 ({mnx},{mny})");
            assert!(
                mxx <= VIEW + EPS && mxy <= VIEW + EPS,
                "icon #{i} 越出 viewBox 右下 ({mxx},{mxy})"
            );
            assert!(w > VIEW * 0.3, "icon #{i} 宽度异常（解析可能截断）w={} VIEW*0.3={}", w, VIEW * 0.3);
        }
    }

    #[test]
    fn quad_is_elevated_to_cubic() {
        let segs = parse("M0 0Q10 0 10 10");
        assert_eq!(segs.len(), 2, "应为 Move + Cubic，实际 {segs:?}");
        assert!(matches!(segs[1], PathSeg::Cubic(..)));
    }

    #[test]
    fn relative_commands_accumulate() {
        let segs = parse("m10 10 l5 0 l0 5 z");
        assert_eq!(segs[0], PathSeg::Move(10.0, 10.0));
        assert_eq!(segs[1], PathSeg::Line(15.0, 10.0));
        assert_eq!(segs[2], PathSeg::Line(15.0, 15.0));
        assert_eq!(segs[3], PathSeg::Close);
    }

    #[test]
    fn semicircle_arc_becomes_two_cubics() {
        // 半圆（弦长 = 直径）切成两段 90° 贝塞尔，终点精确落位。
        let segs = parse("M0 0A1 1 0 0 1 2 0");
        assert_eq!(segs[0], PathSeg::Move(0.0, 0.0));
        let n_cubic = segs.iter().filter(|s| matches!(s, PathSeg::Cubic(..))).count();
        assert_eq!(n_cubic, 2, "半圆应切成两段 90° 贝塞尔，实际 {segs:?}");
        if let Some(PathSeg::Cubic(_, _, _, _, x, y)) = segs.last() {
            assert!((x - 2.0).abs() < 1e-4 && y.abs() < 1e-4, "弧终点漂移 ({x},{y})");
        }
        // sweep=1 在 y-down 坐标系里应向上（负 y）隆起。
        if let Some(PathSeg::Cubic(_, _, _, _, x, y)) = segs.get(1) {
            assert!((x - 1.0).abs() < 1e-4 && *y < -0.9, "隆起方向错误 ({x},{y})");
        }
    }

    #[test]
    fn zero_radius_arc_degrades_to_line() {
        let segs = parse("M0 0A0 0 0 0 1 5 5");
        assert_eq!(segs[1], PathSeg::Line(5.0, 5.0));
    }

    #[test]
    fn fluent_chevron_down_16_parses_with_arcs() {
        // Quel'Thalas chevron_down_16_regular 原串：相对命令 + 紧邻小数 + 弧标记。
        let d = "M3.15 5.65c.2-.2.5-.2.7 0L8 9.79l4.15-4.14a.5.5 0 0 1 .7.7l-4.5 4.5a.5.5 0 0 1-.7 0l-4.5-4.5a.5.5 0 0 1 0-.7Z";
        let segs = parse(d);
        assert_eq!(*segs.last().unwrap(), PathSeg::Close, "解析中断，{segs:?}");
        let n_cubic = segs.iter().filter(|s| matches!(s, PathSeg::Cubic(..))).count();
        // 弧1 弦长0.99≈直径→164°切2段；弧2/3 弦长0.7→89°各1段；加起始 1 段贝塞尔 = 5。
        assert_eq!(n_cubic, 5, "实际 {n_cubic}");
        // 锚点全部落在 0..16 viewBox 内（圆弧升阶无信息漂移）。
        for s in segs {
            let check = |x: f32, y: f32| {
                assert!((-0.1..=16.1).contains(&x) && (-0.1..=16.1).contains(&y), "越界 ({x},{y})");
            };
            match s {
                PathSeg::Move(x, y) | PathSeg::Line(x, y) => check(x, y),
                PathSeg::Cubic(_, _, _, _, x, y) => check(x, y),
                PathSeg::Close => {}
            }
        }
    }
}
