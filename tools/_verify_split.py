"""
Verify: the 22 on_* function bodies are byte-equivalent (modulo whitespace)
to the original match arm bodies from HEAD.
"""
import subprocess, re

HEAD_SRC = subprocess.check_output(
    ['git', 'show', 'HEAD:sysoverlay/crates/win/src/settings/mod.rs'],
    cwd=r'D:\code\sys-overlay-c-master'
).decode('utf-8')

with open(r'D:\code\sys-overlay-c-master\sysoverlay\crates\win\src\settings\mod.rs', 'r', encoding='utf-8') as f:
    NEW_SRC = f.read()

def find_match_body(src, label):
    mi = src.find('match msg {')
    assert mi >= 0, f"{label}: no match msg"
    brace = mi + len('match msg {') - 1
    depth = 0
    j = brace
    while j < len(src):
        if src[j] == '{': depth += 1
        elif src[j] == '}':
            depth -= 1
            if depth == 0: break
        j += 1
    return src[brace + 1:j]

def parse_arms(body):
    arms = []
    pos, n = 0, len(body)
    while pos < n:
        while pos < n and body[pos] in ' \t\n': pos += 1
        if pos >= n: break
        pat_end = body.find('=>', pos)
        pattern = body[pos:pat_end].strip()
        pos = pat_end + 2
        while pos < n and body[pos] in ' \t': pos += 1
        if body[pos] == '{':
            depth, k = 0, pos
            while k < n:
                if body[k] == '{': depth += 1
                elif body[k] == '}':
                    depth -= 1
                    if depth == 0: break
                k += 1
            arm_body = body[pos + 1:k]
            pos = k + 1
        else:
            depth, k = 0, pos
            while k < n:
                ch = body[k]
                if ch in '{[(': depth += 1
                elif ch in '}])': depth -= 1
                elif ch == ',' and depth == 0: break
                k += 1
            arm_body = body[pos:k]
            pos = k + 1
        arms.append((pattern, arm_body))
    return arms

def norm(s):
    """Whitespace-normalize: collapse all runs of whitespace, strip."""
    return re.sub(r'\s+', ' ', s).strip()

# ── Original arms from HEAD ──
head_body = find_match_body(HEAD_SRC, 'HEAD')
head_arms = parse_arms(head_body)
head_norm = [norm(b) for _, b in head_arms]
print(f"HEAD arms: {len(head_arms)}")
for p, b in head_arms:
    print(f"   {p[:45]:45s} -> {norm(b)[:60]}")

# ── New fns from current file ──
# Find all "unsafe fn on_*(...) -> LRESULT { ... }" bodies
fn_pat = re.compile(r'unsafe fn (on_\w+)\(_?hwnd: HWND, _?msg: u32, _?wparam: WPARAM, _?lparam: LPARAM\) -> LRESULT \{')
new_bodies = []
for m in fn_pat.finditer(NEW_SRC):
    brace = m.end() - 1
    depth = 0
    j = brace
    while j < len(NEW_SRC):
        if NEW_SRC[j] == '{': depth += 1
        elif NEW_SRC[j] == '}':
            depth -= 1
            if depth == 0: break
        j += 1
    new_bodies.append((m.group(1), norm(NEW_SRC[brace + 1:j])))
print(f"\nNew on_* fns: {len(new_bodies)}")

# ── Compare in order ──
ok = True
if len(head_norm) != len(new_bodies):
    print(f"COUNT MISMATCH: HEAD={len(head_norm)} NEW={len(new_bodies)}")
    ok = False
else:
    for idx, ((pat, _), (fn, body)) in enumerate(zip(head_arms, new_bodies)):
        same = (head_norm[idx] == body)
        marker = "OK " if same else "DIFF"
        if not same:
            ok = False
            print(f"[{marker}] arm {idx} {pat} vs {fn}")
            print(f"    HEAD: {head_norm[idx][:180]}")
            print(f"    NEW : {body[:180]}")
        else:
            print(f"[{marker}] {pat[:35]:35s} == {fn}")

print("\n" + ("ALL BODIES IDENTICAL ✓" if ok else "MISMATCHES FOUND ✗"))
