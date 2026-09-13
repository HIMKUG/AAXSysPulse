//! 设置页滚动条：几何、坐标换算、命中测试的唯一真值来源。
//!
//! # 为什么单独成模块
//!
//! 旧实现把「几何公式」抄了三份（`paint_scrollbar` / `scrollbar_drag_scroll` /
//! `WM_LBUTTONDOWN` 命中），三处各自对 `scroll_y` 的正负号做了**互相矛盾**的假设，
//! 于是同时长出「方向反」「拖拽卡死」「滚轮无效」三个 bug。
//! 根治办法不是把三处都改对，而是**只留一处**：
//! 绘制、命中、拖拽映射全部走本模块，符号约定写死在类型里，不可能再分叉。
//!
//! # 坐标与符号约定（记牢这三条，全模块只在此处声明）
//!
//! - `scroll_y`：内容滚动偏移，**恒为 0..=max 的「已滚过距离」**，向下滚变大。
//!   内容原点 `content_origin().1 = -scroll_y`，即 scroll_y 越大内容越往上走。
//! - `offset`：拇指顶缘相对轨道顶缘的像素位移，**正值向下**。
//! - `ratio`：滚动进度 0..1（0 = 顶部，1 = 底部）。
//!
//! 三者互为单调递增的线性映射，因此「向下拖 → scroll_y 变大 → 内容上移」
//! 天然成立，不再需要任何取反。

use windows::Win32::Foundation::POINT;

use super::layout::{
    s, SCROLLBAR_HIT_W, SCROLLBAR_INSET, SCROLLBAR_MIN_THUMB, SCROLLBAR_W, SCROLLBAR_W_HOT,
};

/// 滚动条几何（物理像素）。绘制与命中共用同一实例，杜绝漂移。
#[derive(Clone, Copy, Debug)]
pub struct SbGeom {
    /// 轨道顶缘 y（客户区坐标）。
    pub track_top: f32,
    /// 轨道高度（= 可视高度减去上下内缩）。
    pub track_h: f32,
    /// **可见**轨道宽度。
    pub w: f32,
    /// **命中**宽度（含向外扩张的隐形抓取带）。
    pub hit_w: f32,
    /// 悬停/拖拽时的扩张宽度（`SCROLLBAR_W_HOT`，已按 DPI 缩放）。
    pub hot_w: f32,
    /// 拇指当前高度。
    pub thumb_h: f32,
    /// 拇指最大可位移量（= track_h - thumb_h），恒 >= 0。
    pub travel: f32,
    /// 滚动范围（内容高 - 视口高），恒 > 0（否则不应显示滚动条）。
    pub max_scroll: f32,
    /// 距窗口边缘的内缩量（物理像素）。
    pub inset: f32,
}

impl SbGeom {
    /// 拇指顶缘 y（相对客户区）。
    pub fn thumb_top(&self, offset: f32) -> f32 {
        self.track_top + offset
    }

    /// 由滚动进度（0..1）求拇指位移。
    pub fn offset_from_ratio(&self, ratio: f32) -> f32 {
        ratio.clamp(0.0, 1.0) * self.travel
    }

    /// 由拇指位移求滚动进度。
    pub fn ratio_from_offset(&self, offset: f32) -> f32 {
        if self.travel <= 0.0 {
            return 0.0;
        }
        (offset / self.travel).clamp(0.0, 1.0)
    }

    /// 进度 → `scroll_y`（0..=max_scroll，正值向下滚）。
    pub fn scroll_from_ratio(&self, ratio: f32) -> f32 {
        ratio.clamp(0.0, 1.0) * self.max_scroll
    }

    /// `scroll_y` → 进度。
    pub fn ratio_from_scroll(&self, scroll_y: f32) -> f32 {
        if self.max_scroll <= 0.0 {
            return 0.0;
        }
        (scroll_y / self.max_scroll).clamp(0.0, 1.0)
    }

    /// 拇指在客户区坐标下的矩形（x 左、y 上、x 右、y 下）。
    ///
    /// `win_w` 用于把滚动条贴到右缘；`hot` = 悬停或拖拽中（拇指加宽并右对齐，
    /// 加宽只往**内容侧**长，贴边侧不动，避免视觉上「跳出窗口」）。
    pub fn thumb_rect(&self, win_w: i32, offset: f32, hot: bool) -> (f32, f32, f32, f32) {
        let w = if hot { self.hot_w } else { self.w };
        let right = win_w as f32 - s_right_inset(self);
        let left = right - w;
        let top = self.thumb_top(offset);
        (left, top, right, top + self.thumb_h)
    }

    /// 轨道矩形（x 左、y 上、x 右、y 下）。
    pub fn track_rect(&self, win_w: i32) -> (f32, f32, f32, f32) {
        let right = win_w as f32 - s_right_inset(self);
        let left = right - self.w;
        (left, self.track_top, right, self.track_top + self.track_h)
    }
}

/// 可见条右缘距窗口右缘的距离。
///
/// 约定：**可见条**右缘 = 窗口右缘 - `inset`（与旧实现的观感一致，条不贴死边框）；
/// **命中带**从窗口右缘一直覆盖到可见条左侧，因此拖动条外侧的隐形区域也有效，
/// 这正是「加宽可交互区域」的落地方式 —— 命中带可以比可见条宽，但可见条不因此变宽。
fn s_right_inset(g: &SbGeom) -> f32 {
    g.inset.max(0.0)
}

/// 命中带的左边界（客户区坐标）。
fn hit_left(g: &SbGeom, win_w: i32) -> f32 {
    let right = win_w as f32 - s_right_inset(g);
    // 命中带至少覆盖可见条，并额外向内容侧外扩到 hit_w 宽。
    (right - g.hit_w).min(right - g.w)
}

/// 构造几何。`content_h <= view_h` 时返回 `None`（内容不溢出 → 不显示滚动条）。
pub fn geom(dpi: u32, _win_w: i32, win_h: i32, content_h: i32) -> Option<SbGeom> {
    if content_h <= win_h {
        return None;
    }
    let inset = s(dpi, SCROLLBAR_INSET) as f32;
    let w = s(dpi, SCROLLBAR_W) as f32;
    let hit_w = s(dpi, SCROLLBAR_HIT_W) as f32;
    // 扩张宽度必须落在 [可见宽, 命中宽] 内：宽于命中带会让「看起来能点的地方」
    // 实际点不中，窄于可见宽则无反馈意义。
    let hot_w = (s(dpi, SCROLLBAR_W_HOT) as f32).clamp(w, hit_w.max(w));
    let track_top = inset;
    let track_h = (win_h as f32 - 2.0 * inset).max(1.0);
    let max_scroll = (content_h - win_h) as f32;

    // 拇指高 = 轨道高 × 视口占比，下限 MIN_THUMB，上限不超过轨道本身。
    let raw = track_h * win_h as f32 / content_h as f32;
    let thumb_h = raw
        .max(s(dpi, SCROLLBAR_MIN_THUMB) as f32)
        .min(track_h);

    Some(SbGeom {
        track_top,
        track_h,
        w,
        hit_w: hit_w.max(w),
        hot_w,
        thumb_h,
        travel: (track_h - thumb_h).max(0.0),
        max_scroll,
        inset,
    })
}

/// 命中结果：命中滚动条的哪个部位。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SbHit {
    /// 命中拇指：`grab` = 按下点相对拇指顶缘的偏移（拖拽保持贴手）。
    Thumb { grab: f32 },
    /// 命中轨道空白：点击应让拇指中心跳到该处。
    Track,
}

/// 命中测试（客户区坐标）。命中带在 **x 方向用 `hit_w` 外扩、y 方向覆盖整条轨道**，
/// 这样拇指之外的轨道也能点击跳转，且细条也不会「点不中」。
pub fn hit_test(g: &SbGeom, win_w: i32, pt: POINT, offset: f32) -> Option<SbHit> {
    // x：从窗口右缘向左覆盖整条命中带（含可见条外侧的隐形扩展区）。
    let left = hit_left(g, win_w);
    if (pt.x as f32) < left || (pt.x as f32) > win_w as f32 {
        return None;
    }
    // y：整条轨道（含内缩之外的一点点容错，避免贴边时点不中）。
    let y = pt.y as f32;
    if y < g.track_top - 2.0 || y > g.track_top + g.track_h + 2.0 {
        return None;
    }
    let top = g.thumb_top(offset);
    let bottom = top + g.thumb_h;
    if y >= top && y <= bottom {
        Some(SbHit::Thumb { grab: y - top })
    } else {
        Some(SbHit::Track)
    }
}

/// 拖拽中：由鼠标 y 与抓取偏移求新的拇指位移（贴手）。
///
/// **不夹持**——越界交给调用方走橡胶带，拖拽中夹持会让鼠标冲出轨道后
/// 出现「拇指不动、内容不动」的断联感。
pub fn offset_from_drag(g: &SbGeom, mouse_y: i32, grab: f32) -> f32 {
    mouse_y as f32 - g.track_top - grab
}

/// 行步长（滚轮一格）。与列表行高对齐 → 滚轮滚动有「逐行推进」的确定感。
/// 滚轮一个 notch 滚动的像素距离。
///
/// 原为"一整行高"（48px@96dpi），用户反馈一次滚太多、灵敏度过高；
/// 现降为行高的 1/4（≈12px@96dpi），与常见应用的细粒度滚动手感一致。
pub fn wheel_step(dpi: u32) -> f32 {
    s(dpi, super::layout::ROW_H) as f32 / 4.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::POINT;

    fn g() -> SbGeom {
        // 96dpi、视口 600、内容 1800 → 滚动范围 1200
        geom(96, 800, 600, 1800).expect("内容溢出应产生几何")
    }

    #[test]
    fn hidden_when_content_fits() {
        assert!(geom(96, 800, 600, 600).is_none());
        assert!(geom(96, 800, 600, 400).is_none());
    }

    #[test]
    fn direction_is_monotonic_downward() {
        let g = g();
        // 进度越大 → 位移越大（向下）→ scroll_y 越大（内容上移）。全程单调，
        // 这是「向下拖内容向上走」这一交互约定的形式化表达。
        let top = g.scroll_from_ratio(0.0);
        let mid = g.scroll_from_ratio(0.5);
        let bot = g.scroll_from_ratio(1.0);
        assert!(top < mid && mid < bot);
        assert_eq!(top, 0.0);
        assert!((bot - g.max_scroll).abs() < 0.001);

        // 位移同样单调
        assert!(g.offset_from_ratio(0.0) < g.offset_from_ratio(0.5));
        assert!(g.offset_from_ratio(0.5) < g.offset_from_ratio(1.0));
    }

    #[test]
    fn ratio_offset_roundtrip() {
        let g = g();
        for r in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let off = g.offset_from_ratio(r);
            assert!((g.ratio_from_offset(off) - r).abs() < 1e-4, "ratio={r}");
        }
    }

    #[test]
    fn thumb_stays_inside_track() {
        let g = g();
        let h = g.track_top + g.track_h;
        for r in [0.0f32, 0.5, 1.0] {
            let off = g.offset_from_ratio(r);
            assert!(g.thumb_top(off) >= g.track_top - 0.001);
            assert!(g.thumb_top(off) + g.thumb_h <= h + 0.001);
        }
    }

    #[test]
    fn hit_test_thumb_and_track() {
        let g = g();
        let win_w = 800;
        // 顶部时拇指在轨道顶端
        let off = g.offset_from_ratio(0.0);
        let (l, t, r, b) = g.thumb_rect(win_w, off, false);
        let in_thumb = POINT { x: ((l + r) / 2.0) as i32, y: ((t + b) / 2.0) as i32 };
        assert!(matches!(hit_test(&g, win_w, in_thumb, off), Some(SbHit::Thumb { .. })));

        // 轨道底部空白 → Track
        let low = POINT { x: ((l + r) / 2.0) as i32, y: (g.track_top + g.track_h - 2.0) as i32 };
        assert_eq!(hit_test(&g, win_w, low, off), Some(SbHit::Track));

        // 内容区中间 → 不命中
        let away = POINT { x: 100, y: 300 };
        assert!(hit_test(&g, win_w, away, off).is_none());
    }

    #[test]
    fn hit_band_is_wider_than_visible_bar() {
        let g = g();
        let win_w = 800;
        let off = 0.0;
        let (l, _, r, _) = g.thumb_rect(win_w, off, false);
        // 可见条右缘距窗口右缘 = inset，不被命中带影响
        assert!((win_w as f32 - r - g.inset).abs() < 0.001, "可见条应内缩 inset");
        // 命中带比可见条更宽：可见条左侧 1px 处仍能命中
        let band_left = hit_left(&g, win_w);
        assert!(band_left < l, "命中带左边界应比可见条更靠左");
        let probe = POINT { x: (l - 1.0) as i32, y: (g.track_top + 5.0) as i32 };
        assert!(hit_test(&g, win_w, probe, off).is_some());
    }

    #[test]
    fn thumb_never_shorter_than_minimum() {
        // 内容极长：raw 高度会很小，必须被 MIN_THUMB 兜住
        let g = geom(96, 800, 600, 600_000).unwrap();
        assert!(g.thumb_h >= super::super::layout::SCROLLBAR_MIN_THUMB as f32 - 0.001);
        assert!(g.travel >= 0.0);
    }

    #[test]
    fn drag_is_hand_locked() {
        let g = g();
        // 用整数拇指顶缘起点（真实鼠标坐标恒为整数），保证往返无截断误差。
        let top = g.thumb_top(g.offset_from_ratio(0.5)).round();
        let grab = 10.0;
        let off0 = top - g.track_top;
        // 鼠标落在拇指内部：按下点 = 顶缘 + grab → 解出的位移必须回到 off0
        let mouse_y = (top + grab) as i32;
        assert!((offset_from_drag(&g, mouse_y, grab) - off0).abs() < 1e-4);

        // 再向下移 30px → 位移恰好 +30（贴手，无滞后无跳变）
        assert!((offset_from_drag(&g, mouse_y + 30, grab) - off0 - 30.0).abs() < 1e-4);
        // 方向：向下拖 → 位移增大（进而 scroll_y 增大 → 内容上移）
        assert!(offset_from_drag(&g, mouse_y + 30, grab) > offset_from_drag(&g, mouse_y, grab));
    }
}
