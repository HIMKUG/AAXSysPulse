# -*- coding: utf-8 -*-
"""真正配平检查：先剔除注释与字符串字面量（含 {v:?} 这类伪括号），再统计 {} [] ()。"""
import re

PATH = r"D:\code\sys-overlay-c-master\sysoverlay\crates\win\src\settings\cfg_ref.rs"
src = open(PATH, encoding="utf-8").read()

# 剔除 block 注释
src = re.sub(r"/\*.*?\*/", " ", src, flags=re.S)
# 剔除 line 注释
src = re.sub(r"//[^\n]*", " ", src)

# 剔除字符串字面量（处理转义 \" 与 ""）—— 用正则匹配 "..." 但避免吞掉空串误判
def strip_strings(s):
    out = []
    i = 0
    while i < len(s):
        c = s[i]
        if c == '"':
            # 跳到字符串结束
            i += 1
            while i < len(s):
                if s[i] == '\\':
                    i += 2
                    continue
                if s[i] == '"':
                    i += 1
                    break
                i += 1
            out.append(' ')  # 字符串整体替换为空格
        elif c == "'":
            # 可能是 char 字面量 'x' 或生命周期 'a；粗略：若后跟非空白且再后跟 ' 视为 char
            # 简单处理：跳过可能的 char 字面量 '...'
            j = i + 1
            if j < len(s) and s[j] == '\\':
                i += 3
                out.append(' ')
                continue
            # 尝试匹配 'x'
            if j < len(s) and s[j] != ' ' and (j + 1 < len(s)) and s[j + 1] == "'":
                i += 2
                out.append(' ')
                continue
            out.append(c)
            i += 1
        else:
            out.append(c)
            i += 1
    return "".join(out)

clean = strip_strings(src)

# 现在统计括号
depth = 0
opens = []
pairs = {")": "(", "]": "[", "}": "{"}
for idx, ch in enumerate(clean):
    if ch in "([{":
        depth += 1
        opens.append(ch)
    elif ch in ")]}":
        if not opens:
            print(f"位置 {idx}: 多余闭合 {ch}")
            raise SystemExit(1)
        o = opens.pop()
        if pairs[ch] != o:
            print(f"位置 {idx}: 闭合 {ch} 但最近开启是 {o}")
            raise SystemExit(1)
        depth -= 1

print("最终深度:", depth)
if depth == 0:
    print("OK：剔除字符串/注释后括号配平")
else:
    print("失配！剩余未闭合开启（前 10 个）:", opens[:10])
