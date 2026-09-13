"""
Prefix unused params with '_' in on_* functions to silence warnings.
Uses masked body (strings/comments stripped) for accurate usage detection.
"""
import re

PATH = r'sysoverlay\crates\win\src\settings\mod.rs'
with open(PATH, 'r', encoding='utf-8') as f:
    src = f.read()

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

fn_pat = re.compile(r'unsafe fn (on_\w+)\(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM\) -> LRESULT \{')

changes = []
for m in fn_pat.finditer(src):
    fn_name = m.group(1)
    brace = m.end() - 1
    depth = 0
    j = brace
    while j < len(src):
        if src[j] == '{': depth += 1
        elif src[j] == '}':
            depth -= 1
            if depth == 0: break
        j += 1
    body = src[m.end():j]  # between braces
    masked_body = mask(body)
    used = {}
    for name in ('hwnd', 'msg', 'wparam', 'lparam'):
        used[name] = re.search(r'\b' + name + r'\b', masked_body) is not None
    changes.append((fn_name, m.start(), used))

print(f"{'fn':28s} hwnd  msg  wparam lparam")
for fn, _, u in changes:
    print(f"{fn:28s} {str(u['hwnd']):5s} {str(u['msg']):5s} {str(u['wparam']):6s} {str(u['lparam'])}")

# Apply: rename unused params in signatures
new_src = src
for fn_name, pos, used in changes:
    old_sig = f'{fn_name}(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM)'
    parts = []
    for name, ty in (('hwnd','HWND'), ('msg','u32'), ('wparam','WPARAM'), ('lparam','LPARAM')):
        nm = name if used[name] else '_' + name
        parts.append(f'{nm}: {ty}')
    new_sig = f'{fn_name}(' + ', '.join(parts) + ')'
    assert new_src.count(old_sig) == 1, f"{fn_name}: sig count {new_src.count(old_sig)}"
    new_src = new_src.replace(old_sig, new_sig, 1)

with open(PATH, 'w', encoding='utf-8') as f:
    f.write(new_src)
print(f"\nWritten. Delta: {len(new_src) - len(src)}")
