"""临时：定位 i18n TABLE 的排序断点/重复 key（编译期 const 断言失败的排查）。

字符串感知：正确处理 `concat!(...)` 与跨行字符串（`\n\` 续行），
取每个顶层 `(...)` 分组里的**最后一个**字符串字面量作为 key。
"""
import re

PATH = r'sysoverlay\crates\win\src\i18n.rs'
src = open(PATH, encoding='utf-8').read()
start = src.find('const TABLE: &[(&str, &str, &str)] = &[')
end = src.find('\n];', start)
body = src[start:end]
# 注意：不能无脑按行剥 `//` 注释 —— 字符串里可能有 URL（含 `//`）。
# 改为在扫描时按「是否在字符串内」判断，见下方主循环。

rows = []          # [(key, line_no)]
cur = []           # 当前分组内的字符串
depth = 0
line = 1
i = 0
n = len(body)
while i < n:
    c = body[i]
    if c == '\n':
        line += 1
        i += 1
        continue
    if c == '/' and i + 1 < n and body[i + 1] == '/':     # 行注释（仅在字符串外）
        while i < n and body[i] != '\n':
            i += 1
        continue
    if c == '"':                      # 字符串字面量（含 \ 续行）
        j = i + 1
        buf = []
        while j < n:
            if body[j] == '\\':
                if j + 1 < n and body[j + 1] == '\n':   # 续行：跳过换行
                    j += 2
                    line += 1
                    continue
                buf.append(body[j:j + 2])
                j += 2
                continue
            if body[j] == '"':
                break
            if body[j] == '\n':
                line += 1
            buf.append(body[j])
            j += 1
        cur.append(''.join(buf))
        i = j + 1
        continue
    if c == '(':
        depth += 1
        if depth == 1:
            cur = []
        i += 1
        continue
    if c == ')':
        if depth == 1 and cur:
            rows.append((cur[-1], line))
        depth -= 1
        i += 1
        continue
    i += 1

print('entries:', len(rows))
print('first key:', rows[0][0] if rows else None)
print('last  key:', rows[-1][0] if rows else None)
print()

bad = 0
for i in range(1, len(rows)):
    if not (rows[i - 1][0] < rows[i][0]):
        bad += 1
        print('SORT BREAK at idx %d (line ~%d):' % (i, rows[i][1]))
        print('    prev = %r' % rows[i - 1][0])
        print('    cur  = %r' % rows[i][0])

seen = {}
for i, (k, ln) in enumerate(rows):
    if k in seen:
        print('DUP key %r  at idx %d (line ~%d) and idx %d (line ~%d)'
              % (k, seen[k][0], seen[k][1], i, ln))
    seen.setdefault(k, (i, ln))

if not bad and len(seen) == len(rows):
    print('OK: TABLE 严格升序、无重复')
