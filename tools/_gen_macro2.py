#!/usr/bin/env python3
# v2: edits ONLY cfg_ref.rs, sourcing the already-generated macro block from
# tools/_new_macro.txt (config.rs was restored to HEAD, which has no macro).
# Replace real enum/impl BEFORE inserting the macro block.
import os

ROOT = r"D:\code\sys-overlay-c-master\sysoverlay\crates"
WIN = os.path.join(ROOT, "win", "src", "settings", "cfg_ref.rs")
TXT = r"D:\code\sys-overlay-c-master\tools\_new_macro.txt"

txt = open(TXT, encoding="utf-8").read()

# extract entries from the $cb! { ... } block in the saved macro
start = txt.index("$cb! {")
i = txt.index("{", start)
depth = 0
j = i
while j < len(txt):
    if txt[j] == "{": depth += 1
    elif txt[j] == "}":
        depth -= 1
        if depth == 0:
            break
    j += 1
inner = txt[i + 1:j]

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

new_body = "\n".join("            " + e for e in entries)

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

win_new = open(WIN, encoding="utf-8").read()

# import
imp_line = "use sysoverlay_core::config::{Config, OFFSET_MAX, OFFSET_MIN};"
if "SNAP_CORNER_FREE" not in win_new:
    win_new = win_new.replace(imp_line,
        "use sysoverlay_core::config::{Config, OFFSET_MAX, OFFSET_MIN, SNAP_CORNER_FREE};", 1)

# replace real pub enum CfgRef { ... }
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

# replace real impl CfgRef { ... }
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

# insert macro block after Value enum
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

open(WIN, "w", encoding="utf-8").write(win_new)

print("entries:", len(entries))
print("enum invocation present:", "for_each_cfg_field!(cfg_ref_enum);" in win_new)
print("get/set invocation present:",
      "for_each_cfg_field!(cfg_ref_get);" in win_new and "for_each_cfg_field!(cfg_ref_set);" in win_new)
print("macro block present:", "macro_rules! cfg_ref_enum {" in win_new)
print("hand-written enum still present (should be False):",
      "MetricValueAuto(usize)," in win_new)
print("hand-written impl still present (should be False):",
      "pub fn get(self, cfg: &Config) -> Value {" in win_new)
