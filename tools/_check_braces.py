# -*- coding: utf-8 -*-
"""对 cfg_ref.rs 的 set 函数做括号配平检查，定位首个失配点。"""
PATH = r"D:\code\sys-overlay-c-master\sysoverlay\crates\win\src\settings\cfg_ref.rs"
lines = open(PATH, encoding="utf-8").read().splitlines()

# 找到 set 函数区间
start = None
for i, ln in enumerate(lines):
    if ln.strip().startswith("pub fn set(self, cfg: &mut Config, v: Value) -> bool {"):
        start = i
        break
assert start is not None, "set 函数未找到"

depth = 0
opens = []
pairs = {")": "(", "]": "[", "}": "{"}
for i in range(start, len(lines)):
    ln = lines[i]
    for ch in ln:
        if ch in "([{":
            depth += 1
            opens.append((ch, i + 1))
        elif ch in ")]}":
            if not opens:
                print(f"行 {i+1}: 多余闭合 {ch}（无对应开启）")
                raise SystemExit(1)
            o, oline = opens.pop()
            if pairs[ch] != o:
                print(f"行 {i+1}: 闭合 {ch} 但最近开启是 {o}（行 {oline}）")
                raise SystemExit(1)
            depth -= 1
    # 打印每个臂的结束行深度，便于观察
    if ln.strip().startswith("(CfgRef::") or ln.strip().startswith("pub fn set") or ln.strip() == "}":
        print(f"行 {i+1}: 深度={depth}  | {ln.strip()[:60]}")

print("set 函数扫描结束，最终深度:", depth)
if depth != 0:
    print("警告：set 函数括号未配平，剩余开启：", opens[-5:])
else:
    print("set 函数括号配平 OK")
