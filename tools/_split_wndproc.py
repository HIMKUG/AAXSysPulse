#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Split `settings_wndproc`'s `match msg { ... }` into per-message free functions
(`unsafe fn on_<msg>(hwnd, msg, wparam, lparam) -> LRESULT`).

Pure code motion: each generated function body is byte-identical to the original
match arm. The dispatcher simply forwards the four wndproc params.

Robustness: we first build a "masked" copy of the source where string literals,
char/byte literals, and comments are blanked to spaces (length-preserving), so that
brace/paren/quote scanning never desyncs on content such as `'({v:?})'` or `'\r'`.
All indices are derived from the masked copy but slices are taken from the real src.
"""
import sys

PATH = r"D:\code\sys-overlay-c-master\sysoverlay\crates\win\src\settings\mod.rs"

def mask(src: str) -> str:
    """Blank out comments, strings, char/byte literals (length-preserving)."""
    n = len(src)
    out = list(src)
    i = 0
    while i < n:
        c = src[i]
        if c == '/' and i + 1 < n and src[i + 1] == '/':           # line comment
            j = i
            while j < n and src[j] != '\n':
                out[j] = ' '
                j += 1
            i = j
            continue
        if c == '/' and i + 1 < n and src[i + 1] == '*':           # block comment
            j = i + 2
            while j < n and not (src[j] == '*' and j + 1 < n and src[j + 1] == '/'):
                out[j] = ' '
                j += 1
            if j < n:
                out[j] = ' '
            if j + 1 < n:
                out[j + 1] = ' '
            i = j + 2
            continue
        if c == '"':                                               # string literal
            j = i + 1
            while j < n and src[j] != '"':
                if src[j] == '\\':
                    if j < n:
                        out[j] = ' '
                    if j + 1 < n:
                        out[j + 1] = ' '
                    j += 2
                    continue
                out[j] = ' '
                j += 1
            if j < n:
                out[j] = ' '
            i = j + 1
            continue
        if c == "'":                                               # char / byte / lifetime
            nxt = src[i + 1] if i + 1 < n else ''
            is_char = False
            if nxt == '\\':
                is_char = True
            elif (nxt.isalpha() or nxt == '_'):
                if i + 2 < n and src[i + 2] == "'":
                    is_char = True
            else:
                is_char = True
            if is_char:
                j = i + 1
                while j < n and src[j] != "'":
                    if src[j] == '\\':
                        if j < n:
                            out[j] = ' '
                        if j + 1 < n:
                            out[j + 1] = ' '
                        j += 2
                        continue
                    out[j] = ' '
                    j += 1
                if j < n:
                    out[j] = ' '
                i = j + 1
                continue
            else:
                i += 1   # lifetime, leave alone
                continue
        i += 1
    return ''.join(out)

def find_match_close(masked: str, start: int) -> int:
    assert masked[start] == '{', "expected '{'"
    depth = 0
    i = start
    n = len(masked)
    while i < n:
        c = masked[i]
        if c == '{':
            depth += 1
        elif c == '}':
            depth -= 1
            if depth == 0:
                return i
        i += 1
    raise RuntimeError("unbalanced brace in match")

def fn_name(pat: str) -> str:
    if pat == '_':
        return 'on_default'
    if pat.startswith('WM_'):
        return 'on_' + pat.lower()          # WM_PAINT -> on_wm_paint
    return 'on_' + pat.lower()

def main():
    with open(PATH, 'r', encoding='utf-8') as f:
        src = f.read()
    masked = mask(src)
    n = len(src)

    fn_idx = src.find('fn settings_wndproc')
    if fn_idx < 0:
        print("ERROR: settings_wndproc not found"); sys.exit(1)
    mm_idx = masked.find('match msg {', fn_idx)
    if mm_idx < 0:
        print("ERROR: match msg not found in settings_wndproc"); sys.exit(1)
    brace = masked.find('{', mm_idx)
    body_start = brace + 1
    body_end = find_match_close(masked, brace)

    arms = []
    i = body_start
    while i < body_end:
        while i < body_end and masked[i] in ' \t\r\n':
            i += 1
        if i >= body_end:
            break
        pat_start = i
        depth = 0
        while i < body_end:
            c = masked[i]
            if c == '{':
                depth += 1
            elif c == '}':
                depth -= 1
            elif c in '([<':
                depth += 1
            elif c in ')]>':
                depth -= 1
            elif masked[i:i + 2] == '=>' and depth == 0:
                break
            i += 1
        pat = src[pat_start:i].strip()
        i += 2  # skip '=>'
        while i < body_end and masked[i] in ' \t\r\n':
            i += 1
        if masked[i] == '{':
            close = find_match_close(masked, i)
            body = src[i:close + 1]
            i = close + 1
        else:
            bstart = i
            depth = 0
            while i < body_end:
                c = masked[i]
                if c == '{':
                    depth += 1
                elif c == '}':
                    depth -= 1
                elif c in '([':
                    depth += 1
                elif c in ')]':
                    depth -= 1
                elif c == ',' and depth == 0:
                    break
                i += 1
            body = src[bstart:i]
        while i < body_end and masked[i] in ' \t\r\n,':
            i += 1
        arms.append((pat, body))

    print(f"Extracted {len(arms)} arms:")
    for pat, body in arms:
        print(f"  {pat:18s} -> {fn_name(pat)}  (body {len(body)} chars)")

    # Build generated functions
    gen = []
    gen.append("// ── settings_wndproc 路由拆分 ───────────────────────────────────\n")
    gen.append("// 以下 on_* 由 `match msg` 各分支机械抽出，函数体与原 match arm 逐字一致，\n")
    gen.append("// 仅把 (hwnd, msg, wparam, lparam) 作为入参透传。行为不变。\n")
    for pat, body in arms:
        name = fn_name(pat)
        if body.lstrip().startswith('{'):
            fbody = body
        else:
            fbody = '{ ' + body.strip() + ' }'
        gen.append(
            f"unsafe fn {name}(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {fbody}\n"
        )
    gen_block = ''.join(gen)

    # Build dispatcher
    disp = ["    match msg {\n"]
    for pat, _ in arms:
        disp.append(f"        {pat} => {fn_name(pat)}(hwnd, msg, wparam, lparam),\n")
    disp.append("    }")
    new_match = ''.join(disp)

    # Assemble new source: insert gen_block before the function, replace the match body.
    before_fn = src[:fn_idx]
    after_match = src[body_end + 1:]
    new_src = before_fn + gen_block + "\n" + src[fn_idx:mm_idx] + new_match + after_match

    # Sanity: brace balance of whole file (masked)
    m2 = mask(new_src)
    depth = 0
    ok = True
    for ch in m2:
        if ch == '{':
            depth += 1
        elif ch == '}':
            depth -= 1
            if depth < 0:
                ok = False
                break
    if depth != 0 or not ok:
        print(f"ERROR: brace imbalance after transform (depth={depth})")
        sys.exit(2)

    with open(PATH, 'w', encoding='utf-8') as f:
        f.write(new_src)
    print("WROTE", PATH, "ok brace balance depth=", depth)

if __name__ == '__main__':
    main()
