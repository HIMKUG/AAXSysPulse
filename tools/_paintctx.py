"""
PaintCtx refactor for the four paint_* helpers + paint_d2d call sites.
ONE pass, no Edit cascade.

Design:
  struct PaintCtx<'a> { rt, theme, dpi, win_w, win_h, lang }
  - bundles the per-frame, read-only render-target/window state
  - `gfx` stays a separate &mut Gfx (the closure also uses it directly)
  - each painter gains `let X = pc.X;` bindings at the top so its body is
    byte-identical (zero behavior change)
"""
import re

PATH = r'sysoverlay\crates\win\src\settings\mod.rs'
with open(PATH, 'r', encoding='utf-8') as f:
    src = f.read()

orig_len = len(src)

def mask(s):
    out = list(s); n = len(s); i = 0
    while i < n:
        c = s[i]
        if c == '/' and i+1 < n and s[i+1] == '/':
            while i < n and s[i] != '\n': out[i] = ' '; i += 1
            continue
        if c == '/' and i+1 < n and s[i+1] == '*':
            i += 2
            while i < n-1:
                out[i] = ' '
                if s[i] == '*' and s[i+1] == '/': out[i]=' '; out[i+1]=' '; i += 2; break
                i += 1
            continue
        if c == '"':
            i += 1
            while i < n:
                if s[i] == '\\': out[i]=' '; i += 1; out[i]=' '; i += 1; continue
                if s[i] == '"': i += 1; break
                out[i] = ' '; i += 1
            continue
        if c == "'":
            i += 1
            while i < n:
                if s[i] == '\\': out[i]=' '; i += 1; out[i]=' '; i += 1; continue
                if s[i] == "'": i += 1; break
                out[i] = ' '; i += 1
            continue
        i += 1
    return ''.join(out)

def fn_body(src, fn_name):
    """Return (body_text, body_start, body_end) for 'fn <name>(' ... '{' body '}'."""
    m = re.search(r'\nfn ' + re.escape(fn_name) + r'\(', src)
    assert m, f"{fn_name} not found"
    brace = src.find('{', m.end())
    depth = 0; j = brace
    while j < len(src):
        if src[j] == '{': depth += 1
        elif src[j] == '}':
            depth -= 1
            if depth == 0: break
        j += 1
    return src[brace+1:j], brace+1, j

# ─── 1. Insert PaintCtx struct before paint_d2d ───────────────────────────────
STRUCT = '''/// 一帧内不变的渲染上下文：渲染目标 / 主题 / DPI / 客户区尺寸 / 语言。
/// 由 `paint_d2d` 在 `gfx.frame` 闭包内构造一次，四个 paint_* 共用，
/// 避免每个绘制函数都重复透传同样的 6 个参数。
struct PaintCtx<'a> {
    rt: &'a windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    theme: &'a Theme,
    dpi: u32,
    win_w: i32,
    win_h: i32,
    lang: sysoverlay_core::types::Lang,
}

'''
anchor_d2d = 'fn paint_d2d(hwnd: HWND) -> bool {'
assert src.count(anchor_d2d) == 1
src = src.replace(anchor_d2d, STRUCT + anchor_d2d, 1)

# ─── 2. Rewrite the four signatures ───────────────────────────────────────────
# (old_signature_text, new_signature_text, list_of_(ident, expr) bindings)
SIGS = [
    (
        '''fn paint_window_controls(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    t: &Theme,
    dpi: u32,
    win_w: i32,
    tb: &TitleBarState,
) {''',
        'fn paint_window_controls(pc: &PaintCtx, gfx: &mut Gfx, tb: &TitleBarState) {',
        [('rt', 'pc.rt'), ('t', 'pc.theme'), ('dpi', 'pc.dpi'), ('win_w', 'pc.win_w')],
    ),
    (
        '''fn paint_sidebar(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    t: &Theme,
    dpi: u32,
    win_h: i32,
    lang: sysoverlay_core::types::Lang,
    _page_idx: usize,
    nav_hover: Option<usize>,
    sel_t: f32,
) {''',
        'fn paint_sidebar(\n    pc: &PaintCtx,\n    gfx: &mut Gfx,\n    _page_idx: usize,\n    nav_hover: Option<usize>,\n    sel_t: f32,\n) {',
        [('rt', 'pc.rt'), ('t', 'pc.theme'), ('dpi', 'pc.dpi'), ('win_h', 'pc.win_h'), ('lang', 'pc.lang')],
    ),
    (
        '''fn paint_scrollbar(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    t: &Theme,
    dpi: u32,
    win_w: i32,
    win_h: i32,
    sb_offset: f32,
    sb_hot: bool,
    content_h: i32,
) {''',
        'fn paint_scrollbar(\n    pc: &PaintCtx,\n    gfx: &mut Gfx,\n    sb_offset: f32,\n    sb_hot: bool,\n    content_h: i32,\n) {',
        [('rt', 'pc.rt'), ('t', 'pc.theme'), ('dpi', 'pc.dpi'), ('win_w', 'pc.win_w'), ('win_h', 'pc.win_h')],
    ),
    (
        '''fn paint_page(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    pages: &[Page; 6],
    page_idx: usize,
    interaction: &Interaction,
    edit: Option<&EditState>,
    cfg: Option<&sysoverlay_core::config::Config>,
    t: &Theme,
    dpi: u32,
    ox: f32,
    oy: f32,
    body: f32,
    caption: f32,
    _metric_expanded: &[bool; 6],
    drag_pos: Option<usize>,
    transitions: &mut std::collections::HashMap<(usize, i32), Transition>,
    card_slide: &[motion::Spring; 6],
    drag_release: Option<&ReleaseInfo>,
    metric_release: &[motion::Spring; 6],
    dt: f32,
    focus: Option<usize>,
    keyboard_mode: bool,
) -> bool {''',
        '''fn paint_page(
    pc: &PaintCtx,
    gfx: &mut Gfx,
    pages: &[Page; 6],
    page_idx: usize,
    interaction: &Interaction,
    edit: Option<&EditState>,
    cfg: Option<&sysoverlay_core::config::Config>,
    ox: f32,
    oy: f32,
    body: f32,
    caption: f32,
    _metric_expanded: &[bool; 6],
    drag_pos: Option<usize>,
    transitions: &mut std::collections::HashMap<(usize, i32), Transition>,
    card_slide: &[motion::Spring; 6],
    drag_release: Option<&ReleaseInfo>,
    metric_release: &[motion::Spring; 6],
    dt: f32,
    focus: Option<usize>,
    keyboard_mode: bool,
) -> bool {''',
        [('rt', 'pc.rt'), ('t', 'pc.theme'), ('dpi', 'pc.dpi')],
    ),
]

for old_sig, new_sig, bindings in SIGS:
    assert src.count(old_sig) == 1, f"sig not unique/absent: {old_sig[:50]}"
    src = src.replace(old_sig, new_sig, 1)

# ─── 3. Insert `let X = pc.X;` bindings at the top of each body ───────────────
for old_sig, new_sig, bindings in SIGS:
    # locate the new signature, then its body (brace-walk on MASKED source)
    idx = src.find(new_sig)
    assert idx >= 0
    masked_now = mask(src)
    brace = src.find('{', idx)
    body_start = brace + 1
    depth = 0; j = brace
    while j < len(src):
        if masked_now[j] == '{': depth += 1
        elif masked_now[j] == '}':
            depth -= 1
            if depth == 0: break
        j += 1
    body = src[body_start:j]
    masked_body = mask(body)
    # Emit only bindings whose ident is actually used
    used_lines = []
    for ident, expr in bindings:
        if re.search(r'\b' + re.escape(ident) + r'\b', masked_body):
            used_lines.append(f'    let {ident} = {expr};')
            # NOTE: bindings must come BEFORE first use; inserting at body top is fine
    if used_lines:
        insert = '\n' + '\n'.join(used_lines) + '\n'
        src = src[:body_start] + insert + src[body_start:]

print(f"Signatures + bindings done. Delta so far: {len(src) - orig_len}")

# ─── 4. Update the four call sites in paint_d2d ───────────────────────────────
# 4a. Construct pc once, right before the sidebar call.
old_sidebar_call = '''        // 侧边栏（固定在左侧，不随内容滚动）
        paint_sidebar(rt, gfx, &theme, dpi, win_h, lang, page_idx, nav_hover, nav_t);'''
new_sidebar_call = '''        // 本帧绘制上下文（渲染目标/主题/DPI/尺寸/语言），四个 paint_* 共用。
        let pc = PaintCtx { rt, theme: &theme, dpi, win_w, win_h, lang };
        // 侧边栏（固定在左侧，不随内容滚动）
        paint_sidebar(&pc, gfx, page_idx, nav_hover, nav_t);'''
assert src.count(old_sidebar_call) == 1, "sidebar call not found"
src = src.replace(old_sidebar_call, new_sidebar_call, 1)

# 4b. paint_page call: drop rt, &theme, dpi lines; prefix &pc
old_page_call = '''        anim_active = paint_page(
            rt,
            gfx,
            pages,
            page_idx,
            &interaction,
            edit.as_ref(),
            cfg.as_ref(),
            &theme,
            dpi,
            ox as f32,
            oy as f32,
            body_px(dpi),
            caption_px(dpi),
            metric_expanded,
            drag_pos,
            transitions,
            card_slide,
            ctx.drag_release.as_ref(),
            &ctx.metric_release,
            dt,
            ctx.focus,
            ctx.keyboard_mode,
        );'''
new_page_call = '''        anim_active = paint_page(
            &pc,
            gfx,
            pages,
            page_idx,
            &interaction,
            edit.as_ref(),
            cfg.as_ref(),
            ox as f32,
            oy as f32,
            body_px(dpi),
            caption_px(dpi),
            metric_expanded,
            drag_pos,
            transitions,
            card_slide,
            ctx.drag_release.as_ref(),
            &ctx.metric_release,
            dt,
            ctx.focus,
            ctx.keyboard_mode,
        );'''
assert src.count(old_page_call) == 1, "paint_page call not found"
src = src.replace(old_page_call, new_page_call, 1)

# 4c. paint_scrollbar
old_sb_call = '''        paint_scrollbar(
            rt, gfx, &theme, dpi, win_w, win_h, sb_offset, sb_hot, pages[page_idx].h,
        );'''
new_sb_call = '''        paint_scrollbar(
            &pc, gfx, sb_offset, sb_hot, pages[page_idx].h,
        );'''
assert src.count(old_sb_call) == 1, "scrollbar call not found"
src = src.replace(old_sb_call, new_sb_call, 1)

# 4d. paint_window_controls
old_wc_call = '        paint_window_controls(rt, gfx, &theme, dpi, win_w, tb);'
new_wc_call = '        paint_window_controls(&pc, gfx, tb);'
assert src.count(old_wc_call) == 1, "window_controls call not found"
src = src.replace(old_wc_call, new_wc_call, 1)

print(f"Call sites updated. Final delta: {len(src) - orig_len}")

# ─── 5. Brace balance ─────────────────────────────────────────────────────────
depth = 0; i = 0
while i < len(src):
    c = src[i]
    if c == '/' and i+1 < len(src) and src[i+1] == '/':
        while i < len(src) and src[i] != '\n': i += 1
        i += 1; continue
    if c == '/' and i+1 < len(src) and src[i+1] == '*':
        i += 2
        while i < len(src)-1:
            if src[i] == '*' and src[i+1] == '/': i += 2; break
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
            print(f"NEGATIVE at line {src[:i].count(chr(10))+1}")
            break
    i += 1
print(f"Brace balance: {depth} (expected 0)")

with open(PATH, 'w', encoding='utf-8') as f:
    f.write(src)
print("Written")
