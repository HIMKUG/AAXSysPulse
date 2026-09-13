import re
snippet = '''(
    "FootnoteRefresh",
    false,
) => "Auto adapts the refresh rate to how fast system counters actually update, and eases off automatically when sampling gets expensive.",
("FootnoteRefresh", true) => "ZH text here.",
("RowCpu", false) => "CPU",
'''
ARM = re.compile(
    r'\(\s*"([^"]+)"\s*,\s*(false|true)\s*\)\s*=>\s*("(?:\\.|[^"\\])*"|\{[^{}]*\})',
    re.DOTALL,
)
for m in ARM.finditer(snippet):
    print("KEY=", m.group(1), "zh=", m.group(2))
print("count", len(ARM.findall(snippet)))
