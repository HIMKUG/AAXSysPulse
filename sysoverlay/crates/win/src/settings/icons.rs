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

/// 图标标识。前 6 个对应侧边栏导航项，顺序即侧边栏顺序。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconKind {
    General,
    Layout,
    Personalize,
    Window,
    Performance,
    About,
    /// 窗口控制按钮「固定」。
    Pin,
}

impl IconKind {
    /// 侧边栏 6 项（顺序 = 页面下标）。
    pub const ALL: [IconKind; 6] = [
        IconKind::General,
        IconKind::Layout,
        IconKind::Personalize,
        IconKind::Window,
        IconKind::Performance,
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
            IconKind::About => 5,
            IconKind::Pin => 7,
        }
    }
}

/// 内嵌 SVG 源（顺序与 [`IconKind::index`] 一致）。
const SOURCES: [&str; 8] = [
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
];

static PATHS: OnceLock<[Vec<PathSeg>; 8]> = OnceLock::new();

fn paths() -> &'static [Vec<PathSeg>; 8] {
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

/// 极简 SVG path 解析：支持 `M/m L/l H/h V/v C/c S/s Q/q T/t Z/z`。
/// 二次贝塞尔（Q/T）统一升阶为三次（C），因此下游只需一种曲线段。
/// 未知命令（如圆弧 A）的参数会被跳过，不影响其余子路径。
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
            // 未支持命令（如 A 圆弧）：跳过其参数，保持解析继续。
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
            assert!(mnx >= -EPS && mny >= -EPS, "icon #{i} 越出 viewBox 左上 ({mnx},{mny})");
            assert!(
                mxx <= VIEW + EPS && mxy <= VIEW + EPS,
                "icon #{i} 越出 viewBox 右下 ({mxx},{mxy})"
            );
            assert!(mxx - mnx > VIEW * 0.3, "icon #{i} 宽度异常（解析可能截断）");
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
}
