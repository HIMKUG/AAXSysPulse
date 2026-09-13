import re, sys, io

src = open(r"D:\code\sys-overlay-c-master\sysoverlay\crates\win\src\i18n.rs", encoding="utf-8").read()

# Locate the `pub fn t(...)` body.
start = src.index("pub fn t(lang: Lang, key: &str) -> &str {")
# find the outer opening brace
bo = src.index("{", start)
# Walk to matching close brace (no nested fns, strings don't contain '{'/'}' at top level
# except inside the AboutVer block). We'll balance braces while ignoring string content.
depth = 0
i = bo
in_str = False
esc = False
body_end = None
while i < len(src):
    c = src[i]
    if in_str:
        if esc:
            esc = False
        elif c == '\\':
            esc = True
        elif c == '"':
            in_str = False
    else:
        if c == '"':
            in_str = True
        elif c == '{':
            depth += 1
        elif c == '}':
            depth -= 1
            if depth == 0:
                body_end = i
                break
    i += 1

body = src[bo+1:body_end]

# Strip line comments (// ...) that are NOT inside strings.
# Re-scan body with string awareness to remove // comments.
out = []
in_str = False; esc = False
j = 0
n = len(body)
while j < n:
    c = body[j]
    if in_str:
        out.append(c)
        if esc: esc = False
        elif c == '\\': esc = True
        elif c == '"': in_str = False
    else:
        if c == '"':
            in_str = True; out.append(c)
        elif c == '/' and j+1 < n and body[j+1] == '/':
            # skip to end of line
            while j < n and body[j] != '\n':
                j += 1
            continue
        else:
            out.append(c)
    j += 1
clean = "".join(out)

# Now parse arms: ( "KEY", false|true ) => VALUE
# VALUE is either a string literal or a block { ... }
pat = re.compile(
    r'\(\s*"([^"]+)"\s*,\s*(false|true)\s*\)\s*=>\s*'   # key + bool
    r'("(?:\\.|[^"\\])*"|\{[^{}]*\})',                  # value: string or block
    re.DOTALL
)

entries = {}  # key -> {false: expr, true: expr}
order = []
for m in pat.finditer(clean):
    key = m.group(1)
    is_zh = (m.group(2) == "true")
    val = m.group(3)
    if val.startswith('"'):
        expr = val  # raw string literal
    else:
        # block { ... }: handle `const S: &str = EXPR; S`
        inner = val[1:-1].strip()
        cm = re.match(r'const\s+S\s*:\s*&str\s*=\s*(.+?)\s*;\s*S\s*$', inner, re.DOTALL)
        if cm:
            expr = cm.group(1).strip()
        else:
            expr = inner
    d = entries.setdefault(key, {})
    if is_zh in d:
        sys.stderr.write(f"DUPLICATE arm for key={key} zh={is_zh}\n")
    d[is_zh] = expr
    order.append(key)

# Pairing check: every key needs both en and zh
missing = [k for k,v in entries.items() if False not in v or True not in v]
if missing:
    sys.stderr.write("MISSING PAIR for keys: %s\n" % missing)

# Build sorted list
keys_sorted = sorted(entries.keys())

def rust_escape(s):
    return s  # expr is already valid Rust literal/expression text

lines = []
lines.append("//! 全界面双语（En / Zh）文案表。")
lines.append("//!")
lines.append("//! 覆盖三处界面文本：悬浮窗主界面标签（CPU/内存/…）、托盘菜单、设置面板。")
lines.append("//! 配置键 `lang` 切换；文本源都在这里，便于统一维护。")
lines.append("//!")
lines.append("//! 数据布局：每行是一个 `(key, en, zh)` 三元组，按 key 字典序升序排列，")
lines.append("//! 供 `lookup` 做 const fn 二分查找。新增一种语言只需：")
lines.append("//!   1) 给每个三元组再加一列 `&str`；2) 在 `t()` 里按需选择该列。")
lines.append("//! 编译期 `CHECK_TABLE` 会断言「无重复 key、且按 key 有序」，从根上消除")
lines.append("//! 原先 en/zh 分 arm 编写时两语言漏配对的隐患。")
lines.append("")
lines.append('use sysoverlay_core::types::Lang;')
lines.append('use std::sync::atomic::{AtomicU8, Ordering};')
lines.append("")
lines.append("/// 全局当前语言（标题栏「解锁」等窗口外文案用；设置页切换语言时同步写入）。")
lines.append("static GLOBAL_LANG: AtomicU8 = AtomicU8::new(0); // 0 = En, 1 = Zh")
lines.append("")
lines.append("pub fn set_global_lang(l: Lang) {")
lines.append("    GLOBAL_LANG.store(if l == Lang::Zh { 1 } else { 0 }, Ordering::Relaxed);")
lines.append("}")
lines.append("")
lines.append("pub fn global_lang() -> Lang {")
lines.append("    if GLOBAL_LANG.load(Ordering::Relaxed) == 1 {")
lines.append("        Lang::Zh")
lines.append("    } else {")
lines.append("        Lang::En")
lines.append("    }")
lines.append("}")
lines.append("")
lines.append("/// 翻译表：(key, en, zh)。按 key 字典序升序，供二分查找。")
lines.append("/// 新增语言：给每个条目加一列 `&str`，并在 `t()` 中选取。")
lines.append("const TABLE: &[(&str, &str, &str)] = &[")
for k in keys_sorted:
    en = entries[k][False]
    zh = entries[k][True]
    lines.append(f'    ({en!r}, {zh!r}, {k!r}),'.replace("'", '"'))  # ensure double quotes
    # Wait: repr gives single quotes; we want the raw expr. Handle below.

# Oops: above used repr which is wrong for concat! expressions. Rebuild properly.
lines = lines[:lines.index("const TABLE: &[(&str, &str, &str)] = &[")+1]
for k in keys_sorted:
    en = entries[k][False]
    zh = entries[k][True]
    lines.append(f'    ({en}, {zh}, "{k}"),')

lines.append("];")
lines.append("")
lines.append("/// 按字节序比较两个字符串（const 上下文可用的安全比较，避免依赖 str 的 Ord 是否 const 稳定）。")
lines.append("const fn bytes_lt(a: &[u8], b: &[u8]) -> bool {")
lines.append("    let n = if a.len() < b.len() { a.len() } else { b.len() };")
lines.append("    let mut i = 0;")
lines.append("    while i < n {")
lines.append("        if a[i] != b[i] {")
lines.append("            return a[i] < b[i];")
lines.append("        }")
lines.append("        i += 1;")
lines.append("    }")
lines.append("    a.len() < b.len()")
lines.append("}")
lines.append("")
lines.append("/// const fn 二分查找：返回 key 在 TABLE 中的下标。")
lines.append("const fn lookup(key: &str) -> Option<usize> {")
lines.append("    let kb = key.as_bytes();")
lines.append("    let mut lo = 0usize;")
lines.append("    let mut hi = TABLE.len();")
lines.append("    while lo < hi {")
lines.append("        let mid = lo + (hi - lo) / 2;")
lines.append("        let mk = TABLE[mid].0.as_bytes();")
lines.append("        if bytes_lt(mk, kb) {")
lines.append("            lo = mid + 1;")
lines.append("        } else if bytes_lt(kb, mk) {")
lines.append("            hi = mid;")
lines.append("        } else {")
lines.append("            return Some(mid);")
lines.append("        }")
lines.append("    }")
lines.append("    None")
lines.append("}")
lines.append("")
lines.append("/// 编译期自检：TABLE 必须按 key 严格升序且无重复 key。")
lines.append("/// 若有人漏加/错排条目，编译直接失败。")
lines.append("const CHECK_TABLE: () = {")
lines.append("    let mut i = 1;")
lines.append("    while i < TABLE.len() {")
lines.append("        if !bytes_lt(TABLE[i-1].0.as_bytes(), TABLE[i].0.as_bytes()) {")
lines.append("            panic!(\"i18n TABLE not strictly sorted/unique at index\");")
lines.append("        }")
lines.append("        i += 1;")
lines.append("    }")
lines.append("};")
lines.append("")
lines.append("/// 翻译：按当前语言取文案。英文为默认（与 C 版一致）。")
lines.append("/// 未知 key 原样透传（保持旧行为）。")
lines.append("pub fn t(lang: Lang, key: &str) -> &str {")
lines.append("    match lookup(key) {")
lines.append("        Some(i) => if lang == Lang::Zh { TABLE[i].2 } else { TABLE[i].1 },")
lines.append("        None => key,")
lines.append("    }")
lines.append("}")
lines.append("")
lines.append("#[cfg(test)]")
lines.append("mod tests {")
lines.append("    use super::*;")
lines.append("    #[test]")
lines.append("    fn table_covers_both_langs_and_lookup_works() {")
lines.append("        // 每个 key 都成对存在（数组结构已保证），且二分查找命中。")
lines.append("        for (k, en, zh) in TABLE {")
lines.append("            assert_eq!(lookup(k), Some((k, en, zh).0.as_ptr() as usize - TABLE[0].0.as_ptr() as usize) /* placeholder */);")
lines.append("        }")
lines.append("    }")
lines.append("}")

# Actually the test above is nonsense. Replace with a sensible test.
lines = lines[:lines.index("mod tests {")]
lines.append("mod tests {")
lines.append("    use super::*;")
lines.append("    use sysoverlay_core::types::Lang;")
lines.append("    #[test]")
lines.append("    fn unknown_key_passthrough() {")
lines.append("        assert_eq!(t(Lang::En, \"__nope__\"), \"__nope__\");")
lines.append("        assert_eq!(t(Lang::Zh, \"__nope__\"), \"__nope__\");")
lines.append("    }")
lines.append("    #[test]")
lines.append("    fn lookup_hits_every_entry() {")
lines.append("        for (k, en, zh) in TABLE {")
lines.append("            assert_eq!(t(Lang::En, k), *en);")
lines.append("            assert_eq!(t(Lang::Zh, k), *zh);")
lines.append("        }")
lines.append("    }")
lines.append("}")

open(r"D:\code\sys-overlay-c-master\sysoverlay\crates\win\src\i18n.rs", "w", encoding="utf-8").write("\n".join(lines) + "\n")
sys.stderr.write(f"WROTE {len(keys_sorted)} entries\n")
