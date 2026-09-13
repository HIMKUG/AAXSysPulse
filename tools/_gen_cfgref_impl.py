#!/usr/bin/env python3
"""Re-emit cfg_ref.rs enum + impl from the existing for_each_cfg_field! table.

The two-level macro (`for_each_cfg_field!` -> `cfg_ref_get!`/`cfg_ref_set!`) broke Rust
macro hygiene: the `cfg`/`i` tokens inside the table carried a different hygiene context
than the `get`/`set` function parameters, so they failed to bind ("expected value, found
macro cfg"). We therefore drop the indirection and emit a single `impl CfgRef` whose
`cfg` parameter and the table's `cfg`/`i` references share one hygiene context.

The table itself (still the single source of truth for behavior) is parsed out of the
current file and reshaped into match arms. No behavior change.
"""
import re
import sys

SRC = r"D:\code\sys-overlay-c-master\sysoverlay\crates\win\src\settings\cfg_ref.rs"

with open(SRC, "r", encoding="utf-8") as f:
    text = f.read()


def split_group(s, i):
    """Starting at opener s[i] in '([{', return (inner_content, end_index_exclusive)."""
    opener = s[i]
    closer = {'(': ')', '[': ']', '{': '}'}[opener]
    depth = 0
    j = i
    while j < len(s):
        c = s[j]
        if c in '([{':
            depth += 1
        elif c in ')]}':
            depth -= 1
            if depth == 0:
                return s[i + 1:j], j + 1
        j += 1
    raise ValueError("unbalanced group")


def split_entry(entry):
    """entry is a top-level `( (VTY) (VPAT) (GET) (SET) {SER} )` group.
    Returns (vty, vpat, get, set)."""
    assert entry[0] == '('
    i = 1
    parts = []
    while len(parts) < 4:
        while i < len(entry) and entry[i].isspace():
            i += 1
        if i >= len(entry) or entry[i] == ')':
            break
        inner, nxt = split_group(entry, i)
        parts.append(inner)
        i = nxt
    return parts[0], parts[1], parts[2], parts[3]


def closure_body(closure):
    """Extract the body of `|..| -> bool { BODY }`."""
    idx = closure.find('-> bool')
    brace = closure.find('{', idx)
    inner, _ = split_group(closure, brace)
    return inner


# --- locate the table inside `for_each_cfg_field!`'s body ---------------------
start = text.find('$cb! {')
if start < 0:
    print("FATAL: $cb! not found", file=sys.stderr)
    sys.exit(1)
ob = text.find('{', start)
depth = 0
j = ob
chars = []
while j < len(text):
    c = text[j]
    if c == '{':
        depth += 1
    elif c == '}':
        depth -= 1
        if depth == 0:
            break
    chars.append(c)
    j += 1
table = ''.join(chars[1:-1])

# --- split into entries -------------------------------------------------------
entries = []
k = 0
n = len(table)
while k < n:
    while k < n and table[k].isspace():
        k += 1
    if k >= n:
        break
    if table[k] == '(':
        depth = 0
        m = k
        while m < n:
            c = table[m]
            if c == '(':
                depth += 1
            elif c == ')':
                depth -= 1
                if depth == 0:
                    break
            m += 1
        entries.append(table[k:m + 1])
        k = m + 1
    else:
        k += 1

print(f"parsed {len(entries)} entries")

get_arms = []
set_arms = []
for e in entries:
    vty, vpat, get, setc = split_entry(e)
    get_arms.append(f"            CfgRef::{vpat} => {get},")
    body = closure_body(setc)
    set_arms.append(f"            (CfgRef::{vpat}, v) => {{ {body} }},", )

# --- build replacement (concatenation; template contains literal Rust braces) --
header = """// `CfgRef` 是「控件 ↔ Config」的唯一映射表。枚举变体 + 下方 `get`/`set` 的
// match 臂一一对应；新增设置项时同步追加：① 枚举变体 ② get 臂 ③ set 臂。
// （历史上曾用 `for_each_cfg_field!` 宏同时生成 getter/setter，但 macro_rules!
// 的卫生机制会让表内 `cfg`/`i` 与生成的 `get`/`set` 形参处于不同卫生上下文，
// 导致 "expected value, found macro cfg"；故改为单一 hygiene 上下文的手写 impl，
// 行为完全等价于原宏生成结果。）
impl CfgRef {
    pub fn get(self, cfg: &Config) -> Value {
        match self {
"""
mid = """
        }
    }

    pub fn set(self, cfg: &mut Config, v: Value) -> bool {
        match (self, v) {
"""
tail = """
            (r, v) => {
                debug_assert!(false, "CfgRef 类型错配: {r:?} <- {v:?}");
                return false;
            }
        }
    }
}

/// `CfgRef` 枚举变体列表与 `get`/`set` 的 match 臂一一对应；
/// `get`/`set` 是控件读写 Config 的唯一入口（单一事实来源）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CfgRef {
    ShowCpu, ShowMem, ShowGpu, ShowNet, ShowTime, TimeSeconds, ShortNames,
    MemUnit, FontSize, FontWeight, MetricGap, LabelValueGap,
    TextShadow, TextOutline, AutoStart, DebugLog, Topmost,
    Opacity, NetMode, Style, RenderEngine, Lang,
    PowerEnabled, PowerQuery, GpuPrimaryOnly, Gpu3dOnly, NetLowFreq,
    Appearance, ClickThrough, SnapCorner,
    MetricShowLabel(usize), MetricFont(usize), MetricOffset(usize, u8),
    MetricLabel(usize), MetricValue(usize), MetricValueAuto(usize),
}
"""
generated = header + "\n".join(get_arms) + mid + "\n".join(set_arms) + tail

region_start = text.find('macro_rules! for_each_cfg_field')
end_marker = 'for_each_cfg_field!(cfg_ref_set);'
region_end = text.find(end_marker) + len(end_marker)
if region_start < 0 or region_end < len(end_marker):
    print("FATAL: region markers not found", file=sys.stderr)
    sys.exit(1)

new_text = text[:region_start] + generated + text[region_end:]

with open(SRC, "w", encoding="utf-8") as f:
    f.write(new_text)
print("wrote", SRC)
