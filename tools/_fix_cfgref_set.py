# -*- coding: utf-8 -*-
"""修复 cfg_ref.rs 的 set()：将每个臂内「类型错配」catch-all（match v 内、
深度 1 的 `_ => false`）升级为 debug_assert!(false, ...); false，并删除因
此变得不可达的外层 (r, v) catch-all。合法同类型但越界/非法值（位于更深的
match）保持返回 false，不 panic。"""

import re

PATH = r"D:\code\sys-overlay-c-master\sysoverlay\crates\win\src\settings\cfg_ref.rs"
src = open(PATH, encoding="utf-8").read()


def split_top_level(s):
    """在深度 0 处按逗号切分（臂分隔符）。臂内函数调用/嵌套 match 的逗号
    处于更深深度，不会被切到。"""
    arms, cur, depth = [], "", 0
    for c in s:
        if c in "({[":
            depth += 1
            cur += c
        elif c in ")}]":
            depth -= 1
            cur += c
        elif c == "," and depth == 0:
            arms.append(cur)
            cur = ""
        else:
            cur += c
    if cur.strip():
        arms.append(cur)
    return arms


def match_close(s, open_pos):
    """从 s[open_pos] 的 '{' 起，找到配对的 '}' 下标（含）。"""
    depth = 0
    i = open_pos
    while i < len(s):
        c = s[i]
        if c in "({[":
            depth += 1
        elif c in ")}]":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    raise ValueError("unbalanced braces")


def replace_type_mismatch(arm, variant):
    """将 arm 内 match v { } 直接子级（深度 1）最后一个 `_ => false`
    替换为 debug_assert!(false, ...); false。"""
    kw = "match v {"
    idx = arm.find(kw)
    if idx < 0:
        return arm
    brace = arm.find("{", idx)
    end = match_close(arm, brace)
    body = arm[brace:end + 1]
    target = "_ => false"
    candidates = []
    d = 0
    j = 0
    while j < len(body):
        c = body[j]
        if c in "({[":
            d += 1
        elif c in ")}]":
            d -= 1
        elif body[j:j + len(target)] == target and d == 1:
            candidates.append(j)
            j += len(target)
            continue
        j += 1
    if not candidates:
        return arm
    last = candidates[-1]
    # 必须保留 `_ => ` 匹配臂前缀，否则 debug_assert 会悬在 match 块里报语法错。
    repl = '_ => { debug_assert!(false, "CfgRef 类型错配: ' + variant + ' <- {v:?}"); false }'
    new_body = body[:last] + repl + body[last + len(target):]
    return arm[:brace] + new_body + arm[end + 1:]


def is_catch_all(arm):
    return re.match(r"\s*\(r,\s*v\)\s*=>", arm) is not None


# 定位 set 函数内的 `match (self, v) {`
anchor = "match (self, v) {"
a = src.find(anchor)
if a < 0:
    raise SystemExit("找不到 match (self, v) {")
brace = src.find("{", a)
end = match_close(src, brace)
inner = src[brace + 1:end]

arms = split_top_level(inner)
kept = []
for arm in arms:
    if is_catch_all(arm):
        continue  # 删除不可达的外层 catch-all
    m = re.search(r"\(CfgRef::(\w+)", arm)
    variant = m.group(1) if m else "?"
    kept.append(replace_type_mismatch(arm, variant))

new_inner = ",".join(kept)
new_block = anchor + new_inner + "}"
# src[:a] 是 match 块之前的所有内容；new_block 自带 "match (self, v) {" 锚点，
# 故不能再拼 src[:brace]（那已含锚点文本），否则会重复 "match (self, v)"。
new_src = src[:a] + new_block + src[end + 1:]

open(PATH, "w", encoding="utf-8").write(new_src)

# 统计做了多少处替换，便于核对
n_dbg = new_src.count('debug_assert!(false, "CfgRef 类型错配:')
n_catch = new_src.count("(r, v) =>")
print(f"type-mismatch debug_assert 替换数: {n_dbg}")
print(f"残留外层 catch-all 数: {n_catch}")
print("done")
