"""
Verify the four paint_* bodies are unchanged vs HEAD
(modulo the newly inserted `let X = pc.X;` binding lines).
"""
import subprocess, re

HEAD = subprocess.check_output(
    ['git', 'show', 'HEAD:sysoverlay/crates/win/src/settings/mod.rs'],
    cwd=r'D:\code\sys-overlay-c-master'
).decode('utf-8')
with open(r'D:\code\sys-overlay-c-master\sysoverlay\crates\win\src\settings\mod.rs', 'r', encoding='utf-8') as f:
    NEW = f.read()

def body_of(src, name):
    m = re.search(r'\nfn ' + name + r'\(', src)
    assert m, name
    brace = src.find('{', m.end())
    depth = 0; j = brace
    while j < len(src):
        if src[j] == '{': depth += 1
        elif src[j] == '}':
            depth -= 1
            if depth == 0: break
        j += 1
    return src[brace+1:j]

def strip_pc_bindings(body):
    out = []
    for ln in body.split('\n'):
        if re.match(r'\s*let (rt|t|dpi|win_w|win_h|lang) = pc\.\w+;\s*$', ln):
            continue
        out.append(ln)
    return '\n'.join(out)

def norm(s):
    return re.sub(r'\s+', ' ', s).strip()

for name in ('paint_window_controls', 'paint_sidebar', 'paint_scrollbar', 'paint_page'):
    h = norm(body_of(HEAD, name))
    n = norm(strip_pc_bindings(body_of(NEW, name)))
    status = "IDENTICAL ✓" if h == n else "DIFF ✗"
    print(f"{name:26s} {status}   (HEAD {len(h)} chars, NEW {len(n)} chars)")
    if h != n:
        # show first divergence
        for k in range(min(len(h), len(n))):
            if h[k] != n[k]:
                print(f"   diverge at {k}:")
                print(f"     HEAD: ...{h[max(0,k-60):k+60]}")
                print(f"     NEW : ...{n[max(0,k-60):k+60]}")
                break
