"""
独立校验 build_pages 拆分：证明 6 个 page_* 自由函数体与拆分前 match arm 体
在 token 层面等价（仅允许 `ux.` 前缀与结构体字面量 `lang` 简写补全）。

对照物：tools/_layout_before_split.rs.bak（拆分前） vs 线上 layout.rs（拆分后）。
"""
import re

BAK = r'tools\_layout_before_split.rs.bak'
LIVE = r'sysoverlay\crates\win\src\settings\layout.rs'

PAGE_NAMES = {'0': 'prefs', '1': 'layout', '2': 'metrics', '3': 'window', '4': 'perf', '_': 'about'}


def mask(s):
    out = list(s); n = len(s); i = 0
    while i < n:
        c = s[i]
        if c == '/' and i + 1 < n and s[i + 1] == '/':
            while i < n and s[i] != '\n': out[i] = ' '; i += 1
            continue
        if c == '/' and i + 1 < n and s[i + 1] == '*':
            i += 2
            while i < n - 1:
                out[i] = ' '
                if s[i] == '*' and s[i + 1] == '/': out[i] = ' '; out[i + 1] = ' '; i += 2; break
                i += 1
            continue
        if c == '"':
            i += 1
            while i < n:
                if s[i] == '\\': out[i] = ' '; i += 1; out[i] = ' '; i += 1; continue
                if s[i] == '"': i += 1; break
                out[i] = ' '; i += 1
            continue
        if c == "'":
            lit_end = -1
            if i + 1 < n and s[i + 1] == '\\':
                if i + 2 < n and s[i + 2] == 'u':
                    k = s.find('}', i + 2)
                    k = (k + 1) if k != -1 else i + 3
                else:
                    k = i + 3
                if k < n and s[k] == "'": lit_end = k
            elif i + 2 < n and s[i + 2] == "'":
                lit_end = i + 2
            if lit_end != -1:
                for t in range(i, lit_end + 1): out[t] = ' '
                i = lit_end + 1
                continue
            i += 1
            continue
        i += 1
    return ''.join(out)


def match_end(s, ms, open_idx):
    d = 0; j = open_idx
    while j < len(s):
        if ms[j] == '{': d += 1
        elif ms[j] == '}':
            d -= 1
            if d == 0: return j
        j += 1
    raise ValueError('unbalanced')


# ── before：解析 match p 的 6 个 arm ──────────────────────────────────────────
bak = open(BAK, encoding='utf-8').read()
mbak = mask(bak)
bp = bak.find('pub fn build_pages(')
bp_brace = bak.find('{', bp)
bp_end = match_end(bak, mbak, bp_brace)

mm = -1
for mo in re.finditer(r'match p \{', bak[bp:bp_end]):
    cand = bp + mo.start()
    if re.search(r'\n\s+0\s*=>\s*\{', bak[cand:cand + 80]):
        mm = cand; break
m_brace = bak.find('{', mm)
m_end = match_end(bak, mbak, m_brace)

match_body = bak[m_brace + 1:m_end]
mmask = mask(match_body)
arms = {}
for m in re.finditer(r'\n(\s+)(\d+|_)\s*=>\s*\{', match_body):
    label = m.group(2); ob = m.end() - 1
    q = ob; d = 0
    while q < len(mmask):
        if mmask[q] == '{': d += 1
        elif mmask[q] == '}':
            d -= 1
            if d == 0: break
        q += 1
    arms[label] = match_body[ob + 1:q]

# ── after：解析 6 个 page_* 函数体 ────────────────────────────────────────────
live = open(LIVE, encoding='utf-8').read()
mlive = mask(live)
after = {}
for label, nm in PAGE_NAMES.items():
    anchor = 'fn page_%s(' % nm
    i = live.find(anchor)
    assert i > 0, 'missing ' + anchor
    ob = live.find('{', i)
    # 跳过参数里的花括号风险：签名是扁平的，直接取第一个 '{'
    end = match_end(live, mlive, ob)
    after[label] = live[ob + 1:end]


def canon(s):
    t = ' '.join(s.split())          # 折叠空白（含缩进）
    t = t.replace('ux.', '')         # 撤掉 ux. 前缀
    t = t.replace('lang: lang', 'lang')   # 结构体字面量简写还原
    return t


ok = True
for label in ('0', '1', '2', '3', '4', '_'):
    a = canon(arms[label])
    b = canon(after[label])
    same = (a == b)
    ok &= same
    print(('OK  ' if same else 'DIFF') + '  arm %-2s -> page_%s' % (label, PAGE_NAMES[label]))
    if not same:
        import difflib
        for line in difflib.unified_diff(a.split(' '), b.split(' '), lineterm='', n=6):
            print('   ', line)

print('ALL EQUIVALENT' if ok else 'MISMATCH FOUND')
raise SystemExit(0 if ok else 1)
