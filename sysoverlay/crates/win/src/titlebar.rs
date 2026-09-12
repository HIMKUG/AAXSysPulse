//! 自绘标题栏助手 —— 图表 / 进程窗口共用（v4 新增）。
//!
//! # 交互语义（用户拍板 2026-09-05）
//!
//! - 三个**圆角矩形**按钮，自右向左排布：固定（Pin）/ 最小化（Min）/ 关闭（Close）
//! - **固定**：窗口置顶且不可拖动；锁定时标题栏只保留一个「解锁」按钮
//!   （鼠标悬停标题栏时才显示），点击解锁恢复拖动
//! - **最小化**：`SW_HIDE` 隐藏（保留窗口），进程采样线程探测到不可见后
//!   停止采样，CPU 归零；再次打开时恢复
//! - **关闭**：销毁窗口，彻底结束内部计算
//! - 窗口拖动走系统模态循环：`WM_NCHITTEST` 对标题栏返回 `HTCAPTION`
//!   （零自绘拖拽代码）；按钮区域返回 `HTCLIENT` 以便收到鼠标消息

use windows::Win32::Foundation::{COLORREF, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, Ellipse, FillRect, LineTo,
    MoveToEx, PS_SOLID, RoundRect, SelectObject, SetBkMode, SetTextColor, DT_CENTER, DT_LEFT,
    DT_SINGLELINE, DT_VCENTER, DRAW_TEXT_FORMAT, HDC, HFONT, TRANSPARENT,
};

use crate::theme::Theme;
use crate::i18n;

/// 标题栏高度（96 DPI 逻辑像素）。
pub const TB_H: i32 = 30;
const BTN_W: i32 = 26;
const BTN_W_UNLOCK: i32 = 60;
const BTN_H: i32 = 20;
const BTN_GAP: i32 = 4;
const BTN_MARGIN_R: i32 = 6;
pub const CAPTION_PAD_L: i32 = 12;

fn s(dpi: u32, v: i32) -> i32 {
    v * dpi as i32 / 96
}

/// 创建标题栏 / 正文通用字体句柄（96 DPI 基准 12pt，跟随 DPI）。窗口销毁时用
/// `DeleteObject` 释放。
pub fn title_font(dpi_value: u32) -> HFONT {
    unsafe {
        CreateFontW(
            -(12 * dpi_value as i32) / 72,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            0,
            windows::core::w!("Segoe UI"),
        )
    }
}

/// 标题栏按钮。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TitleBtn {
    Pin,
    Min,
    Close,
    /// 固定态的解锁按钮。
    Unlock,
}

/// 每个窗口自持的标题栏状态。
#[derive(Debug, Default)]
pub struct TitleBarState {
    pub pinned: bool,
    /// 当前悬停的按钮（未固定态）。
    pub hover: Option<TitleBtn>,
    /// 鼠标是否在标题栏区域内（固定态决定是否显示解锁按钮）。
    pub over: bool,
}

/// 按钮矩形（自右向左：…Pin / Min / Close；固定态：…Unlock）。
pub fn btn_rects(dpi_value: u32, win_w: i32, pinned: bool) -> Vec<(TitleBtn, RECT)> {
    let h = s(dpi_value, TB_H);
    let bw = s(dpi_value, BTN_W);
    let bh = s(dpi_value, BTN_H);
    let gap = s(dpi_value, BTN_GAP);
    let mr = s(dpi_value, BTN_MARGIN_R);
    let top = (h - bh) / 2;

    let mut out: Vec<(TitleBtn, RECT)> = Vec::new();
    if pinned {
        let w = s(dpi_value, BTN_W_UNLOCK);
        out.push((
            TitleBtn::Unlock,
            RECT { left: win_w - mr - w, top, right: win_w - mr, bottom: top + bh },
        ));
        return out;
    }
    // Pin / Min / Close 从右往左排；先算 Close 的右边
    let right_close = win_w - mr;
    let right_min = right_close - bw - gap;
    let right_pin = right_min - bw - gap;
    for (b, right) in [
        (TitleBtn::Pin, right_pin),
        (TitleBtn::Min, right_min),
        (TitleBtn::Close, right_close),
    ] {
        out.push((b, RECT { left: right - bw, top, right, bottom: top + bh }));
    }
    out
}

fn pt_in(r: &RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

/// 点是否落在标题栏区域内（不含按钮：按钮走单独命中测试）。
pub fn in_title_bar(dpi_value: u32, pt: POINT) -> bool {
    pt.y >= 0 && pt.y < s(dpi_value, TB_H)
}

/// 命中测试：返回命中的按钮；`None` = 标题栏空白（可拖动）或区域外。
/// 调用方先用 [`in_title_bar`] 判断是否在标题栏内。
pub fn hit_button(dpi_value: u32, win_w: i32, pt: POINT, pinned: bool) -> Option<TitleBtn> {
    btn_rects(dpi_value, win_w, pinned)
        .into_iter()
        .find(|(_, r)| pt_in(r, pt))
        .map(|(b, _)| b)
}

// ── 绘制 ──────────────────────────────────────────────────

/// 绘制标题栏（底色 + 标题 + 按钮）。`title` 为 None 时不画标题文字。
///
/// # Safety
///
/// `hdc` must be a valid HDC, and `font` must be a valid HFONT.
pub unsafe fn draw(
    hdc: HDC,
    theme: &Theme,
    dpi_value: u32,
    win_w: i32,
    title: Option<&str>,
    state: &TitleBarState,
    font: HFONT,
) {
    let h = s(dpi_value, TB_H);
    let rect = RECT { left: 0, top: 0, right: win_w, bottom: h };

    // 底层：与卡片底同色 + 底部一条分隔线
    let bg = CreateSolidBrush(COLORREF(theme.card));
    let _ = FillRect(hdc, &rect, bg);
    let _ = DeleteObject(bg);
    let pen = CreatePen(PS_SOLID, 1, COLORREF(theme.card_stroke));
    let old_pen = SelectObject(hdc, pen);
    let _ = MoveToEx(hdc, 0, h - 1, None);
    let _ = LineTo(hdc, win_w, h - 1);
    let _ = SelectObject(hdc, old_pen);
    let _ = DeleteObject(pen);

    // 标题（左侧）
    if let Some(t) = title {
        if !t.is_empty() {
            let tx = RECT {
                left: s(dpi_value, CAPTION_PAD_L),
                top: 0,
                right: win_w - s(dpi_value, 200),
                bottom: h,
            };
            draw_text(hdc, t, tx, theme.label, font, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
        }
    }

    // 按钮
    if state.pinned {
        // 固定态：悬停标题栏才显示解锁按钮
        if state.over {
            if let Some((b, r)) = btn_rects(dpi_value, win_w, true).into_iter().next() {
                draw_unlock_btn(hdc, theme, dpi_value, r, b, state.hover == Some(b), font);
            }
        }
    } else {
        for (b, r) in btn_rects(dpi_value, win_w, false) {
            draw_icon_btn(hdc, theme, dpi_value, r, b, state.hover == Some(b));
        }
    }
}

/// 解锁按钮：圆角矩形 + 文字。
unsafe fn draw_unlock_btn(
    hdc: HDC,
    theme: &Theme,
    dpi_value: u32,
    r: RECT,
    _b: TitleBtn,
    hovered: bool,
    font: HFONT,
) {
    let fill = if hovered { theme.accent_hover } else { theme.accent };
    round_rect(hdc, r, s(dpi_value, 6), fill, fill);
    let label = i18n::t(i18n::global_lang(), "TbUnlock");
    draw_text(hdc, label, r, 0xFFFFFF, font, DT_CENTER | DT_SINGLELINE | DT_VCENTER);
}

/// 文本：选定字体/颜色绘制（空串跳过，防止 DrawTextW 空指针崩溃）。
unsafe fn draw_text(
    hdc: HDC,
    text: &str,
    rect: RECT,
    color: u32,
    font: HFONT,
    fmt: DRAW_TEXT_FORMAT,
) {
    if text.is_empty() {
        return;
    }
    let old = SelectObject(hdc, font);
    let _ = SetTextColor(hdc, COLORREF(color));
    let _ = SetBkMode(hdc, TRANSPARENT);
    let mut buf: Vec<u16> = text.encode_utf16().collect();
    let mut r = rect;
    let _ = DrawTextW(hdc, &mut buf, &mut r, fmt);
    let _ = SelectObject(hdc, old);
}

/// 图标按钮（含悬停高亮）。
unsafe fn draw_icon_btn(
    hdc: HDC,
    theme: &Theme,
    dpi_value: u32,
    r: RECT,
    b: TitleBtn,
    hovered: bool,
) {
    let fill = if hovered { theme.button_bg_hover } else { theme.button_bg };
    round_rect(hdc, r, s(dpi_value, 6), fill, fill);
    let stroke = CreatePen(PS_SOLID, 1, windows::Win32::Foundation::COLORREF(theme.label));
    let old_pen = SelectObject(hdc, stroke);
    let cx = (r.left + r.right) / 2;
    let cy = (r.top + r.bottom) / 2;
    let m = s(dpi_value, 3);
    match b {
        TitleBtn::Pin => {
            // 图钉：顶部圆点 + 垂直针杆 + 底部短横
            let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(theme.label));
            let old_brush = SelectObject(hdc, brush);
            let d = s(dpi_value, 4);
            let c = RECT { left: cx - d / 2, top: r.top + m - d / 2 + s(dpi_value, 1), right: cx + d / 2, bottom: r.top + m - d / 2 + s(dpi_value, 1) + d };
            let _ = Ellipse(hdc, c.left, c.top, c.right, c.bottom);
            let _ = SelectObject(hdc, old_brush);
            let _ = DeleteObject(brush);
            let _ = MoveToEx(hdc, cx, r.top + m + s(dpi_value, 2), None);
            let _ = LineTo(hdc, cx, r.bottom - m);
            let _ = MoveToEx(hdc, cx - s(dpi_value, 4), r.bottom - m, None);
            let _ = LineTo(hdc, cx + s(dpi_value, 4), r.bottom - m);
        }
        TitleBtn::Min => {
            // 最小化：短横条
            let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(theme.label));
            let old_brush = SelectObject(hdc, brush);
            let bw = s(dpi_value, 10);
            let bh = s(dpi_value, 2);
            let x0 = cx - bw / 2;
            let y0 = cy - bh / 2;
            let _ = RoundRect(hdc, x0, y0, x0 + bw, y0 + bh, bh, bh);
            let _ = SelectObject(hdc, old_brush);
            let _ = DeleteObject(brush);
        }
        TitleBtn::Close => {
            // 关闭：×
            let _ = MoveToEx(hdc, cx - s(dpi_value, 4), cy - s(dpi_value, 4), None);
            let _ = LineTo(hdc, cx + s(dpi_value, 4), cy + s(dpi_value, 4));
            let _ = MoveToEx(hdc, cx + s(dpi_value, 4), cy - s(dpi_value, 4), None);
            let _ = LineTo(hdc, cx - s(dpi_value, 4), cy + s(dpi_value, 4));
        }
        TitleBtn::Unlock => {}
    }
    let _ = SelectObject(hdc, old_pen);
    let _ = DeleteObject(stroke);
}

/// 圆角矩形（描边 + 填充）。
unsafe fn round_rect(hdc: HDC, r: RECT, radius: i32, fill: u32, stroke: u32) {
    let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(fill));
    let pen = CreatePen(PS_SOLID, 1, windows::Win32::Foundation::COLORREF(stroke));
    let old_brush = SelectObject(hdc, brush);
    let old_pen = SelectObject(hdc, pen);
    let _ = RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius * 2, radius * 2);
    let _ = SelectObject(hdc, old_brush);
    let _ = SelectObject(hdc, old_pen);
    let _ = DeleteObject(brush);
    let _ = DeleteObject(pen);
}