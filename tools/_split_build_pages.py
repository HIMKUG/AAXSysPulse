"""
Split build_pages (layout.rs, 665 lines) into per-page free functions.

  - 原 build_pages 内的 8 个 helper 闭包（tr/seg/item/ctrl_item/...）收进
    `struct Ux { lang, rows_l, widget_r, ctrl_l }` + 方法。
  - `match p { 0..4, _ }` 的 6 个 arm 各抽成一个自由函数
    `fn page_<name>(ux: &Ux, cfg, expand, order, groups: &mut Vec<Group>)`。
  - 组装段保持不动。
纯代码运动：arm 体逐字搬运（仅把 helper 调用加 `ux.` 前缀、几何量加 `ux.`）。
"""
import re

PATH = r'sysoverlay\crates\win\src\settings\layout.rs'
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
            # Rust 里 `'` 可能是 char 字面量，也可能是生命周期/标签；必须区分，
            # 否则 `&'static str` 会被当成未闭合的 char 字面量而吞掉大段代码。
            lit_end = -1
            if i+1 < n and s[i+1] == '\\':            # '\n'  '\''  '\\'  '\u{..}'
                if i+2 < n and s[i+2] == 'u':
                    k = s.find('}', i+2)
                    k = (k+1) if k != -1 else i+3
                else:
                    k = i+3
                if k < n and s[k] == "'":
                    lit_end = k
            elif i+2 < n and s[i+2] == "'":           # 'x'
                lit_end = i+2
            if lit_end != -1:
                for t in range(i, lit_end+1):
                    out[t] = ' '
                i = lit_end + 1
                continue
            i += 1
            continue
        i += 1
    return ''.join(out)

msrc = mask(src)

# ── locate build_pages ────────────────────────────────────────────────────────
bp = src.find('pub fn build_pages(')
assert bp > 0
bp_brace = src.find('{', bp)
d=0; j=bp_brace
while j < len(src):
    if msrc[j] == '{': d += 1
    elif msrc[j] == '}':
        d -= 1
        if d == 0: break
    j += 1
bp_end = j
print(f"build_pages: {bp}..{bp_end}")

# ── locate the ARM `match p {` inside build_pages ─────────────────────────────
# （前面还有一个 `let sub = match p {` 是页副标题映射，必须跳过）
mm = -1
for mo in re.finditer(r'match p \{', src[bp:bp_end]):
    cand = bp + mo.start()
    if re.search(r'\n\s+0\s*=>\s*\{', src[cand:cand+80]):
        mm = cand
        break
assert mm > 0, "arm match p not found"
m_brace = mm + len('match p {') - 1
d=0; k=m_brace
while k < len(src):
    if msrc[k] == '{': d += 1
    elif msrc[k] == '}':
        d -= 1
        if d == 0: break
    k += 1
m_end = k
print(f"match p: {m_brace}..{m_end}")

# ── parse arms ────────────────────────────────────────────────────────────────
match_body = src[m_brace+1:m_end]
mmask = mask(match_body)
arm_re = re.compile(r'\n(\s+)(\d+|_)\s*=>\s*\{')
arms = []
pos = 0
for m in arm_re.finditer(match_body):
    label = m.group(2)
    ob = m.end() - 1  # position of '{'
    d=0; q=ob
    while q < len(mmask):
        if mmask[q] == '{': d += 1
        elif mmask[q] == '}':
            d -= 1
            if d == 0: break
        q += 1
    arms.append((label, match_body[ob+1:q]))
    pos = q + 1
print(f"arms: {[a[0] for a in arms]}")
assert [a[0] for a in arms] == ['0','1','2','3','4','_'], "unexpected arms"

PAGE_NAMES = {'0':'prefs','1':'layout','2':'metrics','3':'window','4':'perf','_':'about'}

# ── rewrite an arm body into a free-fn body ───────────────────────────────────
def sub_code(text, pattern, repl):
    """只在「代码」区（非注释/非字符串）内做正则替换。"""
    m = mask(text)
    out, last = [], 0
    for mo in re.finditer(pattern, m):
        s, e = mo.span()
        if not m[s:e].strip():
            continue
        out.append(text[last:s])
        out.append(re.sub(pattern, repl, text[s:e]))
        last = e
    out.append(text[last:])
    return ''.join(out)

def rewrite(body):
    b = body
    # helper calls → ux.<helper>(
    for h in ('ctrl_item','toggle_item','off_stepper_item','stepper_item','font_slider_item','seg','item','tr'):
        b = sub_code(b, r'(?<![\w.])' + h + r'\(', 'ux.' + h + '(')
    # 直接引用的几何量 → ux.<geom>
    for g in ('rows_l','widget_r','ctrl_l'):
        b = sub_code(b, r'(?<![\w.])' + g + r'\b', 'ux.' + g)
    # About/指标页对 `lang` 的直接引用：
    #   结构体字面量简写 `{ lang }` / `{ .., lang }` 要补成 `lang: ux.lang`，
    #   调用实参里的裸 `lang`（如 page_footnote(5, lang)）改成 `ux.lang`。
    b = sub_code(b, r'(\{\s*)lang(\s*\})', r'\1lang: ux.lang\2')
    b = sub_code(b, r'(,\s*)lang(\s*\})', r'\1lang: ux.lang\2')
    b = sub_code(b, r'(?<![\w.:])lang(?!\s*:)', 'ux.lang')
    return b

def used(body, name):
    mb = mask(body)
    if name == 'cfg':
        return re.search(r'\bcfg\b(?!\s*:)', mb) is not None
    return re.search(r'\b' + name + r'\b', mb) is not None

page_fns = []
disp_lines = []
for label, body in arms:
    nm = PAGE_NAMES[label]
    nb = rewrite(body)
    # de-indent by 12 (arm body indent) then re-indent by 4
    lines = nb.split('\n')
    out = []
    for ln in lines:
        ln = ln.rstrip()                       # 去掉行尾空白（含纯空白行）
        if ln == '':
            out.append('')
        elif ln.startswith('            '):
            out.append('    ' + ln[12:])
        else:
            out.append(ln)
    inner = '\n'.join(out).strip('\n')

    u_cfg = used(body, 'cfg')
    u_exp = used(body, 'expand')
    u_ord = used(body, 'order')
    p_cfg = 'cfg' if u_cfg else '_cfg'
    p_exp = 'expand' if u_exp else '_expand'
    p_ord = 'order' if u_ord else '_order'

    sig = (f"fn page_{nm}(\n    ux: &Ux,\n    {p_cfg}: &Config,\n    {p_exp}: &[f32; 6],\n"
           f"    {p_ord}: &[u8; 6],\n    groups: &mut Vec<Group>,\n) {{")
    page_fns.append(sig + '\n' + inner + '\n}')
    disp_lines.append(f"            {label} => page_{nm}(&ux, cfg, expand, order, &mut groups),")

PAGE_FNS = '\n\n'.join(page_fns)
DISPATCH = '        match p {\n' + '\n'.join(disp_lines) + '\n        }'

# ── Ux struct + impl ──────────────────────────────────────────────────────────
UX = '''/// 行构建辅助：几何 + 语言 + 控件构造，供各 `page_*` 共用。
/// （原为 `build_pages` 内的闭包；拆出后各页函数不再重复捕获。）
struct Ux {
    lang: Lang,
    rows_l: f32,
    widget_r: f32,
    ctrl_l: f32,
}

impl Ux {
    fn tr(&self, k: &'static str) -> &'static str {
        crate::i18n::t(self.lang, k)
    }
    fn seg(&self, labels: &[&'static str], cfg: CfgRef) -> SegmentedWidget {
        SegmentedWidget {
            labels: labels.iter().map(|k| self.tr(k).to_string()).collect::<Vec<_>>().into(),
            cfg,
        }
    }
    fn item(&self, id: i32, kind: Box<dyn super::state::IWidget>, l: f32, r: f32, tall: bool) -> ItemDef {
        ItemDef { id, kind, l, r, tall }
    }
    fn ctrl_item(&self, id: i32, kind: Box<dyn super::state::IWidget>) -> ItemDef {
        let tall = kind.as_any().is::<SliderWidget>();
        self.item(id, kind, self.ctrl_l, self.widget_r, tall)
    }
    fn toggle_item(&self, id: i32, cfg: CfgRef) -> ItemDef {
        self.item(id, Box::new(ToggleWidget{ cfg }), self.widget_r - TOGGLE_W as f32, self.widget_r, false)
    }
    fn stepper_item(&self, id: i32, cfg: CfgRef) -> ItemDef {
        self.item(
            id,
            Box::new(StepperWidget{ min: GAP_MIN as f64, max: GAP_MAX as f64, step: 1.0, cfg }),
            self.widget_r - STEPPER_W as f32,
            self.widget_r,
            false,
        )
    }
    fn off_stepper_item(&self, id: i32, cfg: CfgRef) -> ItemDef {
        self.item(
            id,
            Box::new(StepperWidget{ min: OFF_MIN as f64, max: OFF_MAX as f64, step: 1.0, cfg }),
            self.widget_r - STEPPER_W as f32,
            self.widget_r,
            false,
        )
    }
    fn font_slider_item(&self, id: i32, cfg: CfgRef, l: f32, r: f32, metric: bool) -> ItemDef {
        ItemDef {
            id,
            kind: Box::new(SliderWidget{ min: FONT_MIN, max: FONT_MAX, step: 1.0, dp: 0, unit: "px", cfg, metric_style: metric }),
            l,
            r,
            tall: true,
        }
    }
}

type Group = (&'static str, Vec<RowDef>, Option<&'static str>, i32);

'''

# ── replace the helper-closure block with `let ux = ...` ──────────────────────
helper_start = src.find('    // 控件构造辅助（闭包仅捕获几何常量，无可变状态）。', bp)
assert helper_start > 0
fs_end = src.find('    };\n', src.find('let font_slider_item', helper_start))
assert fs_end > 0
fs_end += len('    };\n')
helper_block = src[helper_start:fs_end]
new_ux_binding = ('    // 行构建辅助（几何 + 语言 + 控件构造）。\n'
                  '    let ux = Ux { lang, rows_l, widget_r, ctrl_l };\n')

# ── reconstruct ───────────────────────────────────────────────────────────────
# 1) replace helper block
delta = len(helper_block) - len(new_ux_binding)   # helper 块替换造成的偏移量
assert helper_start < fs_end < mm < m_end, "helper/match 位置顺序异常"
src2 = src[:helper_start] + new_ux_binding + src[fs_end:]
# 2) replace match block with dispatcher（整行替换，避免重复拼接 `match p {`）
#    helper 块位于 match 之前，替换后其后的所有 offset 左移 delta
mm2 = mm - delta
m_end2 = m_end - delta
mm_line = src2.rfind('\n', 0, mm2) + 1
assert src2[mm_line:mm2].strip() == '', "match p 前应只有缩进"
assert src2[mm2:mm2+8] == 'match p ', "mm2 未对准 match p"
print(f"helper {helper_start}..{fs_end}  mm {mm}  m_end {m_end}  delta {delta}  mm2 {mm2}")
new_src = src2[:mm_line] + DISPATCH + src2[m_end2+1:]
# 3) insert Ux + page fns **在 build_pages 的文档注释之前**（否则文档会错挂到 Ux 上）
anchor = 'pub fn build_pages('
i2 = new_src.find(anchor)
assert i2 > 0
ins = new_src.rfind('\n', 0, i2) + 1          # build_pages 所在行的行首
while True:                                    # 连同其上的 /// 文档 / #[attr] 一起前移
    pe = ins - 1
    if pe <= 0:
        break
    ps = new_src.rfind('\n', 0, pe) + 1
    head = new_src[ps:pe].strip()
    if head.startswith('///') or head.startswith('//!') or head.startswith('#['):
        ins = ps
    else:
        break
new_src = new_src[:ins] + UX + PAGE_FNS + '\n\n' + new_src[ins:]

# 4) 组装段里手写的 groups 元组类型改用新别名 Group
OLD_VEC = "Vec<(&'static str, Vec<RowDef>, Option<&'static str>, i32)>"
cnt = new_src.count(OLD_VEC)
assert cnt == 1, "Vec<...> 出现 %d 次（应为 1）" % cnt
new_src = new_src.replace(OLD_VEC, "Vec<Group>")

# ── validate ──────────────────────────────────────────────────────────────────
ms = mask(new_src)
d=0; bad=None
for idx,ch in enumerate(ms):
    if ch=='{': d+=1
    elif ch=='}':
        d-=1
        if d<0 and bad is None:
            bad = new_src[:idx].count('\n')+1
print("brace balance:", d, ("NEGATIVE at line %s" % bad) if bad else "")

if d != 0 or bad:
    raise SystemExit("brace 不平衡，拒绝写盘（文件保持原样）")

# 逐个 page 函数再做一次局部平衡校验
for label, body in arms:
    nb = mask(rewrite(body))
    assert nb.count('{') == nb.count('}'), ('arm %s 不平衡' % label)

with open(PATH, 'w', encoding='utf-8') as f:
    f.write(new_src)
print("written, delta", len(new_src)-len(src))
