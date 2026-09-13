"""
Restore + complete the in-flight feature that my `git checkout HEAD -- widget.rs`
accidentally reverted:

  widget.rs : (a) 删 CheckUpdate/CopyVersion  (b) 恢复关于页应用图标  (c) OpenUrl 文档
  mod.rs    : (a) 删 CheckUpdate/CopyVersion 两个 arm  (b) ResetMetricOffset/OpenUrl 直连
              (c) 删 copy_version_info（已无引用）  (d) 删我加的 Action::ResetMetricOffset/OpenUrl arm
  state.rs  : 删我加的 Action::ResetMetricOffset/OpenUrl

对齐依据：journal 2026-09-12/13 + 工作区 layout.rs（已不再构造那两个按钮、已构造新按钮）。
"""
import re

BASE = r'sysoverlay\crates\win\src\settings'
W = BASE + r'\widget.rs'
M = BASE + r'\mod.rs'
S = BASE + r'\state.rs'

def rd(p):
    with open(p, 'r', encoding='utf-8') as f:
        return f.read()
def wr(p, s):
    with open(p, 'w', encoding='utf-8') as f:
        f.write(s)
def rep1(s, old, new, tag):
    n = s.count(old)
    assert n == 1, f"[{tag}] count={n} (expected 1)"
    return s.replace(old, new, 1)

# ══════════════════════════ widget.rs ══════════════════════════
w = rd(W)

# (a) 删 CheckUpdate / CopyVersion；同时补回 OpenUrl 的「关于页」说明
w = rep1(w, '''    /// 用系统默认浏览器打开外部链接。
    /// 存 `&'static str` 以保持 `Copy`（URL 均为编译期常量）。
    OpenUrl(&'static str),
    /// 检查更新（无更新服务，反馈「已是最新」状态提示）。
    CheckUpdate,
    /// 复制版本信息到剪贴板。
    CopyVersion,
}''', '''    /// 用系统默认浏览器打开外部链接（关于页「哔哩哔哩主页」）。
    /// 存 `&'static str` 以保持 `Copy`（URL 均为编译期常量）。
    OpenUrl(&'static str),
}''', 'widget-enum')

# (b) 关于页应用图标
w = rep1(w, '''    let side = super::layout::s(dpi, 22) as f32;
    let cy = (r.t + r.b) / 2.0;
    let logo = RectF::new(r.l, cy - side / 2.0, r.l + side, cy + side / 2.0);
    gfx.fill_round(rt, logo, super::layout::s(dpi, 6) as f32, c(t, t.label));
    gfx.text(
        rt,
        crate::i18n::t(lang, "AboutLogo"),
        super::layout::s(dpi, 9) as f32,
        700,
        HAlign::Center,
        logo,
        c(t, t.card),
        true,
    );''', '''    let side = super::layout::s(dpi, 32) as f32;
    let cy = (r.t + r.b) / 2.0;
    let logo = RectF::new(r.l, cy - side / 2.0, r.l + side, cy + side / 2.0);
    // 应用图标：内嵌 ICO 的 64×64 帧，经 WIC 解码成 D2D 位图后按 logo 区域缩放绘制。
    // 解码失败（WIC 不可用等）时退回圆角占位块，保证关于页始终有内容。
    match crate::app_icon::d2d_bitmap(rt) {
        Some(bmp) => {
            use windows::Win32::Graphics::Direct2D::D2D1_BITMAP_INTERPOLATION_MODE_LINEAR;
            let dst = logo.d2d();
            unsafe {
                rt.DrawBitmap(&bmp, Some(&dst), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            }
        }
        // 兜底只画一圈淡化描边 —— 绝不再填纯色块：
        // 深色主题下 label 为白，填色块会被看成「纯白图片」。
        None => {
            gfx.stroke_round(rt, logo, super::layout::s(dpi, 6) as f32, c(t, t.secondary), 1.0);
        }
    }''', 'widget-about-icon')

wr(W, w)
print("widget.rs: enum + about-icon restored")

# ══════════════════════════ mod.rs ══════════════════════════
m = rd(M)

# (a+b) 点击处理器：删 CheckUpdate/CopyVersion，改成直连（与 ResetAllColors/OpenLogFolder 同风格）
m = rep1(m, '''            widget::ButtonAction::CheckUpdate => {
                // 无更新服务：反馈「已是最新」（行为收敛于状态提示，不弹窗）。
                set_status(ctx, crate::i18n::t(lang_of(ctx), "StatusUpToDate"));
            }
            widget::ButtonAction::CopyVersion => {
                copy_version_info(ctx);
            }
            widget::ButtonAction::ResetMetricOffset(i) => {
                dispatch(ctx, Action::ResetMetricOffset(i));
            }
            widget::ButtonAction::OpenUrl(url) => {
                dispatch(ctx, Action::OpenUrl(url));
            }''', '''            widget::ButtonAction::ResetMetricOffset(i) => {
                apply_config(ctx, |cfg| {
                    if i < cfg.metric_offsets.len() && cfg.metric_offsets[i] != [0; 4] {
                        cfg.metric_offsets[i] = [0; 4];
                        true
                    } else {
                        false
                    }
                });
            }
            widget::ButtonAction::OpenUrl(url) => {
                open_url(ctx, url);
            }''', 'mod-click-arms')

# (c) 删 copy_version_info
m = rep1(m, '''/// 复制版本信息到剪贴板（报告 §6 COPY_VERSION）。
fn copy_version_info(ctx: &mut SettingsCtx) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    let text = format!(
        "SysOverlay {}\\n硬件监控悬浮条",
        env!("CARGO_PKG_VERSION")
    );
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let bytes = wide.len() * 2;

    unsafe {
        if OpenClipboard(find_hwnd(ctx)).is_err() {
            crate::probe::log("copy_version: OpenClipboard failed");
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
            let dst = GlobalLock(hmem);
            if !dst.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), dst as *mut u16, wide.len());
                let _ = GlobalUnlock(hmem);
                // 成功交出所有权；失败则系统忽略
                let _ = SetClipboardData(CF_UNICODETEXT.0 as u32, HANDLE(hmem.0));
            }
        }
        let _ = CloseClipboard();
    }
    set_status(ctx, crate::i18n::t(lang_of(ctx), "StatusCopied"));
}

''', '', 'mod-copy-version')

# (d) 删 dispatch 里我加的 Action::ResetMetricOffset/OpenUrl arm
m = rep1(m, '''        Action::ResetMetricOffset(i) => {
            apply_config(ctx, |cfg| {
                if i < cfg.metric_offsets.len() && cfg.metric_offsets[i] != [0; 4] {
                    cfg.metric_offsets[i] = [0; 4];
                    true
                } else {
                    false
                }
            });
        }
        Action::OpenUrl(url) => {
            open_url(ctx, url);
        }
        // 滚轮滚动由 WM_MOUSEWHEEL 直接写 scroll_y（H4：橡胶边界 + 惯性吸附），''',
       '''        // 滚轮滚动由 WM_MOUSEWHEEL 直接写 scroll_y（H4：橡胶边界 + 惯性吸附），''',
       'mod-dispatch-arms')

wr(M, m)
print("mod.rs: click arms fixed, copy_version_info removed, dispatch arms removed")

# ══════════════════════════ state.rs ══════════════════════════
s = rd(S)
s = rep1(s, '''    /// 展开/收起某指标的配置卡。
    ToggleMetricExpand(usize),
    /// 将指定指标的四项文字偏移复位为 0。
    ResetMetricOffset(usize),
    /// 用系统默认浏览器打开外部链接。
    OpenUrl(&'static str),
}''', '''    /// 展开/收起某指标的配置卡。
    ToggleMetricExpand(usize),
}''', 'state-action')
wr(S, s)
print("state.rs: reverted Action additions")

print("\n所有替换均唯一命中，完成。")
