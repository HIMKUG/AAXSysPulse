# -*- coding: utf-8 -*-
"""字符串感知配平检查，记录每个开启括号的行号，定位失配位置。"""
import re

PATH = r"D:\code\sys-overlay-c-master\sysoverlay\crates\win\src\settings\cfg_ref.rs"
src = open(PATH, encoding="utf-8").read()
src = re.sub(r"/\*.*?\*/", " ", src, flags=re.S)
src = re.sub(r"//[^\n]*", " ", src)

def strip_strings(s):
    out = []
    i = 0
    while i < len(s):
        c = s[i]
        if c == '"':
            i += 1
            while i < len(s):
                if s[i] == '\\':
                    i += 2
                    continue
                if s[i] == '"':
                    i += 1
                    break
                i += 1
            out.append(' ')
        elif c == "'":
            j = i + 1
            if j < len(s) and s[j] == '\\':
                i += 3
                out.append(' ')
                continue
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
lines = clean.split("\n")
opens = []  # (ch, line_no)
depth = 0
pairs = {")": "(", "]": "[", "}": "{"}
for li, ln in enumerate(lines, 1):
    for ch in ln:
        if ch in "([{":
            depth += 1
            opens.append((ch, li))
        elif ch in ")]}":
            if not opens:
                print(f"行 {li}: 多余闭合 {ch}")
                raise SystemExit(1)
            o, ol = opens.pop()
            if pairs[ch] != o:
                print(f"行 {li}: 闭合 {ch} 与开启 {o}(行 {ol}) 不匹配")
                raise SystemExit(1)
            depth -= 1

print("最终深度:", depth)
if depth != 0:
    print("未闭合的开启括号（行号）：")
    for ch, li in opens:
        print(f"  行 {li}: {ch}")
