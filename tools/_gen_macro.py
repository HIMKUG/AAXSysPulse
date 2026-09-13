#!/usr/bin/env python3
# Transforms the existing core `for_each_cfg_field!` table (old comma format)
# into the uniform single-token-tree format and emits the win-side macro +
# generation callbacks. Edits config.rs (remove dead core macro) and cfg_ref.rs
# (insert macro block + replace hand-written enum/impl with macro invocations).
# ORDER MATTERS: the real enum/impl are replaced BEFORE inserting the macro
# block, so the `pub enum CfgRef {` / `impl CfgRef {` inside the callback macro
# bodies are never mistaken for the real items.
import os

ROOT = r"D:\code\sys-overlay-c-master\sysoverlay\crates"
CORE = os.path.join(ROOT, "core", "src", "config.rs")
WIN = os.path.join(ROOT, "win", "src", "settings", "cfg_ref.rs")

core_src = open(CORE, encoding="utf-8").read()
win_src = open(WIN, encoding="utf-8").read()

# ---- 1. extract entries from core macro ----
start = core_src.index("$cb! {")
i = core_src.index("{", start)
depth = 0
j = i
while j < len(core_src):
    if core_src[j] == "{": depth += 1
    elif core_src[j] == "}":
        depth -= 1
        if depth == 0:
            break
    j += 1
inner = core_src[i + 1:j]

entries = []
k = 0
n = len(inner)
while k < n:
    while k < n and inner[k] in " \t\r\n,":
        k += 1
    if k >= n:
        break
    if inner[k] != "(":
        k += 1
        continue
    depth = 0
    m = k
    while m < n:
        if inner[m] == "(": depth += 1
        elif inner[m] == ")":
            depth -= 1
            if depth == 0:
                break
        m += 1
    entries.append(inner[k:m + 1])
    k = m + 1

def split_top_commas(s):
    parts = []
    depth = 0
    cur = ""
    for ch in s:
        if ch in "([{": depth += 1
        elif ch in ")]}": depth -= 1
        if ch == "," and depth == 0:
            parts.append(cur.strip())
            cur = ""
        else:
            cur += ch
    if cur.strip():
        parts.append(cur.strip())
    return parts

new_entries = []
for e in entries:
    body = e[1:-1].strip()
    fields = split_top_commas(body)
    assert len(fields) == 5, (e, fields)
    f1, f2, f3, f4, f5 = fields
    new_entries.append("( ({f1}) ({f2}) {f3} {f4} {f5} )".format(f1=f1, f2=f2, f3=f3, f4=f4, f5=f5))

new_body = "\n".join("            " + ne for ne in new_entries)

macro = (
    "#[macro_export]\n"
    "macro_rules! for_each_cfg_field {\n"
    "    ($cb:ident) => {\n"
    "        $cb! {\n"
    + new_body + "\n"
    "        }\n"
    "    };\n"
    "}\n\n"
)

callbacks = (
    "/// 由 `for_each_cfg_field!` 的变体类型字段 `(Variant)` / `(Variant(T))` 还原出枚举变体声明。\n"
    "macro_rules! cfg_ref_enum_variant {\n"
    "    ( ($v:ident) ) => { $v };\n"
    "    ( ($v:ident ( $($ty:ty),* )) ) => { $v ( $($ty),* ) };\n"
    "}\n\n"
    "/// 生成 `CfgRef` 枚举。\n"
    "macro_rules! cfg_ref_enum {\n"
    "    ( $( ( $vty:tt $vpat:tt $get:tt $set:tt $ser:tt ) )* ) => {\n"
    "        #[derive(Clone, Copy, Debug, PartialEq, Eq)]\n"
    "        pub enum CfgRef {\n"
    "            $(\n"
    "                cfg_ref_enum_variant!($vty),\n"
    "            )*\n"
    "        }\n"
    "    };\n"
    "}\n\n"
    "/// 生成 `CfgRef::get`。\n"
    "macro_rules! cfg_ref_get {\n"
    "    ( $( ( $vty:tt $vpat:tt $get:tt $set:tt $ser:tt ) )* ) => {\n"
    "        impl CfgRef {\n"
    "            pub fn get(self, cfg: &Config) -> Value {\n"
    "                match self {\n"
    "                    $( $vpat => $get, )*\n"
    "                }\n"
    "            }\n"
    "        }\n"
    "    };\n"
    "}\n\n"
    "/// 生成 `CfgRef::set`（含类型错配 catch-all：debug panic / release 拒绝）。\n"
    "macro_rules! cfg_ref_set {\n"
    "    ( $( ( $vty:tt $vpat:tt $get:tt $set:tt $ser:tt ) )* ) => {\n"
    "        impl CfgRef {\n"
    "            pub fn set(self, cfg: &mut Config, v: Value) -> bool {\n"
    "                match (self, v) {\n"
    "                    $( ($vpat, v) => $set(cfg, v), )*\n"
    "                    (r, v) => {\n"
    "                        debug_assert!(false, \"CfgRef 类型错配: {r:?} <- {v:?}\");\n"
    "                        return false;\n"
    "                    }\n"
    "                }\n"
    "            }\n"
    "        }\n"
    "    };\n"
    "}\n\n"
)

insert_block = macro + callbacks

# ---- 2. edit config.rs: remove the now-dead core macro ----
mi = core_src.index("macro_rules! for_each_cfg_field {")
hb = mi
while hb > 0 and core_src[hb - 1] in " \t\r\n":
    hb -= 1
while hb > 0:
    nl = core_src.rfind("\n", 0, hb)
    if nl != -1 and core_src[nl + 1:hb].lstrip().startswith("///"):
        hb = nl
    else:
        break
depth = 0
mj = mi
while mj < len(core_src):
    if core_src[mj] == "{": depth += 1
    elif core_src[mj] == "}":
        depth -= 1
        if depth == 0:
            mj += 1
            break
    mj += 1
k = mj
while k < len(core_src) and core_src[k] in " \t\r\n":
    k += 1
core_new = (core_src[:hb]
            + "\n// NOTE: `for_each_cfg_field!` 已移至 win 侧 `settings/cfg_ref.rs`，\n"
              "// 因为表内引用了 win 专属的 `Value` 与函数参数 `cfg`，跨 crate 宏展开会\n"
              "// 让这些标识符带上 core 卫生上下文而失效。CfgRef 枚举/get/set 现由该表统一生成；\n"
              "// `Config::serialize`（core）保持手写以维持保守写格式与数组循环顺序。\n\n"
            + core_src[k:])

# ---- 3. edit cfg_ref.rs (ORDER: replace real items, THEN insert macro block) ----
win_new = win_src
imp_line = "use sysoverlay_core::config::{Config, OFFSET_MAX, OFFSET_MIN};"
if "SNAP_CORNER_FREE" not in win_new:
    win_new = win_new.replace(imp_line,
        "use sysoverlay_core::config::{Config, OFFSET_MAX, OFFSET_MIN, SNAP_CORNER_FREE};", 1)

# 3c. replace real `pub enum CfgRef { ... }`
ce = win_new.index("pub enum CfgRef {")
depth = 0
cp = ce
while cp < len(win_new):
    if win_new[cp] == "{": depth += 1
    elif win_new[cp] == "}":
        depth -= 1
        if depth == 0:
            cp += 1
            break
    cp += 1
while cp < len(win_new) and win_new[cp] in " \r\n":
    cp += 1
win_new = win_new[:ce] + "for_each_cfg_field!(cfg_ref_enum);" + win_new[cp:]

# 3d. replace real `impl CfgRef { ... }`
ie = win_new.index("impl CfgRef {")
depth = 0
ip = ie
while ip < len(win_new):
    if win_new[ip] == "{": depth += 1
    elif win_new[ip] == "}":
        depth -= 1
        if depth == 0:
            ip += 1
            break
    ip += 1
while ip < len(win_new) and win_new[ip] in " \r\n":
    ip += 1
win_new = win_new[:ie] + "for_each_cfg_field!(cfg_ref_get);\n\nfor_each_cfg_field!(cfg_ref_set);" + win_new[ip:]

# 3b. NOW insert macro block + callbacks after the Value enum closing `}`
ve = win_new.index("pub enum Value {")
depth = 0
vp = ve
while vp < len(win_new):
    if win_new[vp] == "{": depth += 1
    elif win_new[vp] == "}":
        depth -= 1
        if depth == 0:
            vp += 1
            break
    vp += 1
win_new = win_new[:vp] + "\n" + insert_block + "\n" + win_new[vp:]

open(CORE, "w", encoding="utf-8").write(core_new)
open(WIN, "w", encoding="utf-8").write(win_new)

print("entries transformed:", len(new_entries))
print("core macro removed:", "macro_rules! for_each_cfg_field" not in core_new.split("NOTE:")[0])
print("win has enum invocation:", "for_each_cfg_field!(cfg_ref_enum);" in win_new)
print("win has get/set invocations:",
      "for_each_cfg_field!(cfg_ref_get);" in win_new and "for_each_cfg_field!(cfg_ref_set);" in win_new)
print("win macro block present:", "macro_rules! cfg_ref_enum {" in win_new)
print("win still has hand-written enum body:",
      "ShowCpu," in win_new and "MetricValueAuto(usize)," in win_new)
