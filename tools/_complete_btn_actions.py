"""
Complete the incomplete ResetMetricOffset/OpenUrl feature in mod.rs.
Three insertions, all in one pass (no Edit cascade):
  1. dispatch: Action::ResetMetricOffset / Action::OpenUrl arms
  2. on_click_plain: ButtonAction::ResetMetricOffset / ButtonAction::OpenUrl arms
  3. open_url helper fn after copy_version_info
"""
PATH = r'sysoverlay\crates\win\src\settings\mod.rs'
with open(PATH, 'r', encoding='utf-8') as f:
    src = f.read()

orig_len = len(src)

# ─── Insertion 1: dispatch arms ────────────────────────────────────────────────
anchor1 = """        // 滚轮滚动由 WM_MOUSEWHEEL 直接写 scroll_y（H4：橡胶边界 + 惯性吸附），
        // 因此不再经 Action::Scroll 中转。
    }
}"""
assert src.count(anchor1) == 1, f"anchor1 count = {src.count(anchor1)}"

new_arms1 = """        Action::ResetMetricOffset(i) => {
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
        // 滚轮滚动由 WM_MOUSEWHEEL 直接写 scroll_y（H4：橡胶边界 + 惯性吸附），
        // 因此不再经 Action::Scroll 中转。
    }
}"""

src = src.replace(anchor1, new_arms1, 1)

# ─── Insertion 2: click handler arms ──────────────────────────────────────────
anchor2 = """            widget::ButtonAction::CopyVersion => {
                copy_version_info(ctx);
            }
        }"""
assert src.count(anchor2) == 1, f"anchor2 count = {src.count(anchor2)}"

new_arms2 = """            widget::ButtonAction::CopyVersion => {
                copy_version_info(ctx);
            }
            widget::ButtonAction::ResetMetricOffset(i) => {
                dispatch(ctx, Action::ResetMetricOffset(i));
            }
            widget::ButtonAction::OpenUrl(url) => {
                dispatch(ctx, Action::OpenUrl(url));
            }
        }"""

src = src.replace(anchor2, new_arms2, 1)

# ─── Insertion 3: open_url helper ─────────────────────────────────────────────
anchor3 = """    set_status(ctx, crate::i18n::t(lang_of(ctx), "StatusCopied"));
}"""
assert src.count(anchor3) == 1, f"anchor3 count = {src.count(anchor3)}"

new_helper = """    set_status(ctx, crate::i18n::t(lang_of(ctx), "StatusCopied"));
}

/// 用系统默认浏览器打开外部链接（ShellExecuteW 的 "open" 动词）。
/// 传入 URL 均为编译期常量，失败静默（不弹窗）。
fn open_url(ctx: &mut SettingsCtx, url: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let url_w: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    let verb: Vec<u16> = "open\\0".encode_utf16().collect();
    unsafe {
        let _ = ShellExecuteW(
            find_hwnd(ctx),
            PCWSTR(verb.as_ptr()),
            PCWSTR(url_w.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}"""

src = src.replace(anchor3, new_helper, 1)

print(f"Length: {orig_len} -> {len(src)} (+{len(src) - orig_len})")

# ─── Validate brace balance ───────────────────────────────────────────────────
depth = 0
i = 0
while i < len(src):
    c = src[i]
    if c == '/' and i + 1 < len(src) and src[i + 1] == '/':
        while i < len(src) and src[i] != '\n': i += 1
        i += 1; continue
    if c == '/' and i + 1 < len(src) and src[i + 1] == '*':
        i += 2
        while i < len(src) - 1:
            if src[i] == '*' and src[i + 1] == '/': i += 2; break
            i += 1
        continue
    if c == '"':
        i += 1
        while i < len(src):
            if src[i] == '\\': i += 2; continue
            if src[i] == '"': i += 1; break
            i += 1
        continue
    if c == "'":
        i += 1
        while i < len(src):
            if src[i] == '\\': i += 2; continue
            if src[i] == "'": i += 1; break
            i += 1
        continue
    if c == '{': depth += 1
    elif c == '}':
        depth -= 1
        if depth < 0:
            line = src[:i].count('\n') + 1
            print(f"NEGATIVE at line {line}")
            break
    i += 1
print(f"Brace balance: {depth} (expected 0)")

with open(PATH, 'w', encoding='utf-8') as f:
    f.write(src)
print("Written")
