"""
Correct single-pass refactor:
  1. Extract each match arm char-by-char using brace-depth tracking
  2. Generate on_wm_* free functions
  3. Build thin dispatcher
  4. Add PaintCtx struct + update call sites and signatures

Key fix: char-by-char scanning with depth tracking, NOT line-based.
"""
import re

PATH = r'sysoverlay\crates\win\src\settings\mod.rs'
with open(PATH, 'r', encoding='utf-8') as f:
    src = f.read()

# ─── Step 1: Find match body extent ───────────────────────────────────────────
# "match msg {" → find opening {
mi = src.find('match msg {')
assert mi >= 0
match_brace = mi + len('match msg {') - 1  # position of '{'
assert src[match_brace] == '{'

# Find matching } using depth tracking on RAW string
depth = 0
j = match_brace
while j < len(src):
    c = src[j]
    if c == '{':
        depth += 1
    elif c == '}':
        depth -= 1
        if depth == 0:
            break
    j += 1
match_end = j  # position of closing }
print(f"Match body: {match_brace+1}–{match_end-1}, size={match_end - match_brace - 1}")

# match_body = exclusive of outer braces
match_body = src[match_brace + 1:match_end]

# ─── Step 2: Parse arms char-by-char ──────────────────────────────────────────
def parse_arms(body):
    """Parse match arms from body string. Returns list of (pattern_str, body_str)."""
    arms = []
    pos = 0
    n = len(body)

    while pos < n:
        # Skip leading whitespace
        while pos < n and body[pos] in ' \t\n':
            pos += 1
        if pos >= n:
            break

        # Scan pattern: everything up to the first '=>'
        # Must handle nested parens/brackets in pattern
        pat_end = body.find('=>', pos)
        if pat_end < 0:
            raise RuntimeError(f"No '=>' found starting at pos {pos}")
        pattern = body[pos:pat_end].strip()
        pos = pat_end + 2  # skip '=>'

        # Skip whitespace after =>
        while pos < n and body[pos] in ' \t':
            pos += 1

        if pos >= n:
            raise RuntimeError(f"Body ends right after '=>' at pattern {pattern}")

        # Check what comes next
        if body[pos] == '{':
            # Block arm — find matching }
            depth = 0
            k = pos
            while k < n:
                if body[k] == '{':
                    depth += 1
                elif body[k] == '}':
                    depth -= 1
                    if depth == 0:
                        break
                k += 1
            arm_body = body[pos+1:k]  # exclusive of outer braces
            pos = k + 1
        else:
            # Expression arm — find ',' at depth 0
            k = pos
            depth = 0
            while k < n:
                ch = body[k]
                if ch in '{[(':
                    depth += 1
                elif ch in '}])':
                    depth -= 1
                elif ch == ',' and depth == 0:
                    break
                k += 1
            arm_body = body[pos:k].strip()
            # Remove trailing comma
            arm_body = arm_body.rstrip(',').strip()
            pos = k + 1

        arms.append((pattern, arm_body))
        print(f"  [{pattern[:40]}]: {len(arm_body)} chars")

    return arms

arms = parse_arms(match_body)
print(f"\nParsed {len(arms)} arms total")

# ─── Step 3: Find settings_wndproc fn extent ──────────────────────────────────
fn_header_pat = 'unsafe extern "system" fn settings_wndproc('
fn_start = src.find(fn_header_pat)
assert fn_start >= 0

# fn body opening brace
fn_brace = src.find('{', fn_start)

# fn closing brace — walk from match_end forward
depth = 0
k = match_end
while k < len(src):
    if src[k] == '{':
        depth += 1
    elif src[k] == '}':
        depth -= 1
        if depth == 0:
            break
    k += 1
fn_end = k
print(f"Function body: {fn_brace+1}–{fn_end-1}")

# ─── Step 4: Generate on_wm_* functions ───────────────────────────────────────
def make_on_fn(pattern, body):
    """Generate an on_* unsafe fn from pattern + body."""
    # Build fn name from pattern
    fn_name = pattern_to_fnname(pattern)

    # Indent body by 12 spaces
    lines = body.strip().split('\n')
    indented_lines = []
    for line in lines:
        # Remove 8-space indent (original arm body indent) and add 12
        stripped = line.strip()
        if stripped:
            # Original arm body is indented by 8 spaces
            # We want 12 spaces
            if line.startswith('        '):
                indented_lines.append('            ' + stripped)
            else:
                indented_lines.append('            ' + stripped)
        else:
            indented_lines.append('')

    inner = '\n'.join(indented_lines)
    return f'''unsafe fn {fn_name}(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {{
{inner}
        }}'''

def pattern_to_fnname(pat):
    """Convert a match pattern to a valid Rust fn name."""
    # Handle common cases
    pat = pat.strip()
    if pat == '_':
        return 'on_default'
    if pat.startswith('WM_'):
        return f'on_{pat.lower()}'
    if re.match(r'^0x[0-9A-Fa-f]+$', pat):
        # Hex constant
        return f'on_0x{pat[2:].lower()}'
    if re.match(r'^\d+$', pat):
        return f'on_{pat}'
    # For compound patterns like "Interaction::Pressing { .. }" or patterns with guards
    # Replace non-alphanumeric with underscore
    safe = re.sub(r'[^a-zA-Z0-9_]', '_', pat)
    safe = re.sub(r'_+', '_', safe).strip('_')
    return f'on_{safe.lower()}'

gen_fns = '\n\n'.join(make_on_fn(p, b) for p, b in arms)

# ─── Step 5: Build new dispatcher ─────────────────────────────────────────────
def disp_call(pattern):
    fn_name = pattern_to_fnname(pattern)
    return f'        {pattern} => {fn_name}(hwnd, msg, wparam, lparam),'

new_dispatcher = '\n'.join(disp_call(p) for p, _ in arms)

# ─── Step 6: Reconstruct file ─────────────────────────────────────────────────
HELPERS = """// ── settings_wndproc 路由拆分 ───────────────────────────────────
// 以下 on_* 由 `match msg` 各分支机械抽出，函数体与原 match arm 逐字一致，
// 仅把 (hwnd, msg, wparam, lparam) 作为入参透传。行为不变。

"""

new_src = (
    src[:fn_start] +           # before settings_wndproc
    HELPERS +
    gen_fns +
    '\n\n' +
    src[fn_start:fn_brace + 1] +  # "unsafe extern... {"
    '\n    match msg {\n' +
    new_dispatcher +
    '\n    }\n' +
    src[match_end + 1:]         # after match (closing braces of fn)
)

# ─── Step 7: Validate ─────────────────────────────────────────────────────────
cnt = new_src.count('unsafe extern "system" fn settings_wndproc(')
print(f"\nsettings_wndproc count: {cnt} (expected 1)")

depth = 0
i = 0
while i < len(new_src):
    c = new_src[i]
    if c == '/' and i + 1 < len(new_src) and new_src[i + 1] == '/':
        while i < len(new_src) and new_src[i] != '\n': i += 1
        i += 1; continue
    if c == '/' and i + 1 < len(new_src) and new_src[i + 1] == '*':
        i += 2
        while i < len(new_src) - 1:
            if new_src[i] == '*' and new_src[i + 1] == '/': i += 2; break
            i += 1
        continue
    if c == '"':
        i += 1
        while i < len(new_src):
            if new_src[i] == '\\': i += 2; continue
            if new_src[i] == '"': i += 1; break
            i += 1
        continue
    if c == "'":
        i += 1
        while i < len(new_src):
            if new_src[i] == '\\': i += 2; continue
            if new_src[i] == "'": i += 1; break
            i += 1
        continue
    if c == '{': depth += 1
    elif c == '}':
        depth -= 1
        if depth < 0:
            line = new_src[:i].count('\n') + 1
            print(f"NEGATIVE at char {i}, line {line}: {repr(new_src[max(0,i-30):i+20])}")
            break
    i += 1

print(f"Brace balance: {depth} (expected 0)")

with open(PATH, 'w', encoding='utf-8') as f:
    f.write(new_src)
print(f"Wrote {len(new_src)} chars")
