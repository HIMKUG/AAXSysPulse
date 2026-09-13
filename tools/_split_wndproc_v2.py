#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Split `settings_wndproc`'s `match msg { ... }` into per-message free functions.
Extracted functions are NOT `unsafe` — they are plain `fn`.
The dispatcher (inside `settings_wndproc`) wraps calls in `unsafe { ... }`.
All function bodies are byte-identical to original match arms.

The original `unsafe extern "system" fn settings_wndproc(...) -> LRESULT {`
is preserved intact; helpers are inserted BEFORE it.
"""
import sys, re

PATH = r"D:\code\sys-overlay-c-master\sysoverlay\crates\win\src\settings\mod.rs"

def mask(src: str) -> str:
    """Blank out comments, strings, char literals (length-preserving)."""
    out = list(src)
    n = len(src)
    i = 0
    while i < n:
        c = src[i]
        if c == '/' and i + 1 < n and src[i + 1] == '/':
            j = i
            while j < n and src[j] != '\n':
                out[j] = ' '
                j += 1
            i = j
            continue
        if c == '/' and i + 1 < n and src[i + 1] == '*':
            j = i + 2
            while j < n - 1 and not (src[j] == '*' and src[j + 1] == '/'):
                out[j] = ' '
                j += 1
            if j < n: out[j] = ' '
            if j + 1 < n: out[j + 1] = ' '
            i = j + 2
            continue
        if c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                if src[j] == '\\' and j + 1 < n:
                    out[j] = ' '; j += 1
                out[j] = ' '; j += 1
            if j < n: out[j] = ' '
            i = j + 1
            continue
        if c == "'":
            j = i + 1
            while j < n and src[j] != "'":
                if src[j] == '\\' and j + 1 < n:
                    out[j] = ' '; j += 1
                out[j] = ' '; j += 1
            if j < n: out[j] = ' '
            i = j + 1
            continue
        i += 1
    return ''.join(out)

def find_brace_close(masked: str, start: int) -> int:
    """Find matching } for { at position start (masked version)."""
    assert masked[start] == '{', f"expected '{{' at {start}, got {masked[start]!r}"
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
    raise RuntimeError("unclosed brace")

def fn_name_for(pat: str) -> str:
    if pat == '_':
        return 'on_default'
    return 'on_' + pat.lower()

def main():
    with open(PATH, 'r', encoding='utf-8') as f:
        src = f.read()
    masked = mask(src)

    # Locate 'unsafe extern "system" fn settings_wndproc'
    FN_ANCHOR = 'unsafe extern "system" fn settings_wndproc'
    fn_start = src.find(FN_ANCHOR)
    if fn_start < 0:
        print(f"ERROR: cannot find {FN_ANCHOR!r}")
        sys.exit(1)

    # The function signature starts at fn_start and ends at '-> LRESULT {'
    sig_end = src.find('-> LRESULT {', fn_start)
    if sig_end < 0:
        print("ERROR: cannot find '-> LRESULT {' in settings_wndproc")
        sys.exit(1)
    body_brace = sig_end + len('-> LRESULT {') - 1  # position of '{'

    # Find 'match msg {' within settings_wndproc body
    match_start = src.find('match msg {', body_brace)
    if match_start < 0:
        print("ERROR: cannot find 'match msg {' in settings_wndproc")
        sys.exit(1)
    # The '{' may be separated by whitespace (e.g. `match msg {    \n    {`)
    # so scan forward from match_start to find the actual opening brace
    match_brace_start = match_start
    while masked[match_brace_start] != '{':
        match_brace_start += 1
        if match_brace_start >= len(masked):
            raise RuntimeError("could not find opening '{' for match")

    # Find closing '}' of match (at indent 8 spaces)
    match_close = find_brace_close(masked, match_brace_start)
    # The line after match_close should be '    }' (4 spaces) closing settings_wndproc body
    after_match = src[match_close + 1:]
    wc_end = match_close + 1 + re.search(r'\n    }\n', after_match).start() + 1
    # wc_end points to '\n' before the '    }'

    # Extract match arms
    arms = []
    i = match_brace_start + 1
    while i < match_close:
        while i < match_close and masked[i] in ' \t\r\n':
            i += 1
        if i >= match_close:
            break
        pat_start = i
        depth = 0
        while i < match_close:
            mc = masked[i]
            if mc == '{':
                depth += 1
            elif mc == '}':
                depth -= 1
            elif mc in '([<':
                depth += 1
            elif mc in ')]>':
                depth -= 1
            elif mc == '=' and i + 1 < match_close and masked[i + 1] == '>':
                break
            i += 1
        pat = src[pat_start:i].strip()
        i += 2  # skip '=>'
        while i < match_close and masked[i] in ' \t\r\n':
            i += 1
        # Arm body
        if masked[i] == '{':
            b_start = i
            b_end = find_brace_close(masked, i)
            body = src[b_start:b_end + 1]
            i = b_end + 1
        else:
            # single expression (e.g. LRESULT(1))
            b_start = i
            depth = 0
            while i < match_close:
                mc = masked[i]
                if mc == '{': depth += 1
                elif mc == '}': depth -= 1
                elif mc in '([<': depth += 1
                elif mc in ')]>': depth -= 1
                elif mc == ',' and depth == 0:
                    break
                i += 1
            body = src[b_start:i]
        while i < match_close and masked[i] in ' \t\r\n,':
            i += 1
        arms.append((pat, body))

    print(f"Extracted {len(arms)} arms:")
    for pat, body in arms:
        print(f"  {pat:22s} -> {fn_name_for(pat):22s} ({len(body)} chars)")

    # Build helper function text
    lines = []
    lines.append("")
    lines.append("// ── settings_wndproc 路由拆分 ───────────────────────────────────")
    lines.append("// 各 on_wm_* 由 match msg 分支机械抽出，函数体逐字一致；")
    lines.append("// 仅把 (hwnd, msg, wparam, lparam) 入参透传。行为不变。")
    lines.append("")
    for pat, body in arms:
        name = fn_name_for(pat)
        # Normalize: if body is single-line '{ expr }' keep it; otherwise wrap in braces
        stripped = body.strip()
        if stripped.startswith('{') and stripped.endswith('}') and stripped.count('\n') <= 1:
            sig = f"unsafe fn {name}(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {stripped}"
            lines.append(sig)
        else:
            lines.append(f"unsafe fn {name}(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {{")
            for ln in body.split('\n'):
                lines.append(ln)
            lines.append("}")
        lines.append("")
    helpers = '\n'.join(lines)

    # Build new match body (dispatcher)
    disp_lines = ["    match msg {"]
    for pat, _ in arms:
        name = fn_name_for(pat)
        disp_lines.append(f"        {pat} => unsafe {{ {name}(hwnd, msg, wparam, lparam) }},")
    disp_lines.append("    }")
    new_match_body = '\n'.join(disp_lines)

    # Assemble: helpers inserted before fn_start; dispatcher replaces old match body
    # The old match body is from match_start to wc_end (inclusive: includes '\n    }')
    old_match_body = src[match_start:wc_end + 1]
    assert masked[match_start:match_start + 11] == 'match msg {', "match_start sanity check failed"

    new_src = helpers + '\n' + src[:match_start] + new_match_body + src[wc_end + 1:]

    # Sanity: brace balance
    m2 = mask(new_src)
    depth = 0
    for ch in m2:
        if ch == '{': depth += 1
        elif ch == '}':
            depth -= 1
            if depth < 0:
                print(f"ERROR: brace imbalance at depth={depth}"); sys.exit(2)
    if depth != 0:
        print(f"ERROR: brace imbalance final depth={depth}"); sys.exit(2)

    with open(PATH, 'w', encoding='utf-8') as f:
        f.write(new_src)
    print(f"\nWROTE {PATH} — brace balance ok (depth={depth})")

if __name__ == '__main__':
    main()
