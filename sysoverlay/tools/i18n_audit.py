import re

src = open('crates/win/src/i18n.rs', encoding='utf-8').read()

# 匹配  ("Key", false|true) => <value>
pat = re.compile(r'\(\s*"([A-Za-z0-9_]+)"\s*,\s*(false|true)\s*\)\s*=>\s*(.*?)(?=\n\s*\(|\n\s*_\s*=>|\Z)', re.S)
entries = {}
for m in pat.finditer(src):
    key, is_zh, val = m.group(1), m.group(2) == 'true', m.group(3).strip().rstrip(',').strip()
    # 若是 concat! 表达式，标记出来
    if val.startswith('const') or 'concat!' in val:
        v = '<dynamic: %s>' % val.replace('\n', ' ')[:60]
    else:
        mm = re.search(r'"((?:[^"\\]|\\.)*)"', val)
        v = mm.group(1) if mm else val
    entries.setdefault(key, {})['zh' if is_zh else 'en'] = v

cjk = re.compile(r'[\u4e00-\u9fff]')

print('=== 1. 英文条目里混入中文（应改） ===')
found = False
for k, d in sorted(entries.items()):
    en = d.get('en', '')
    if en and cjk.search(en):
        print('   %-22s EN="%s"   ZH="%s"' % (k, en, d.get('zh', '')))
        found = True
if not found:
    print('   (无)')

print()
print('=== 2. 中英完全相同（可能漏翻译，专有名词除外） ===')
same = [k for k, d in sorted(entries.items()) if d.get('en') and d.get('en') == d.get('zh')]
print('   ' + ', '.join(same) if same else '   (无)')

print()
print('=== 3. 覆盖统计 ===')
paired = sum(1 for d in entries.values() if 'en' in d and 'zh' in d)
print('   总条目: %d   成对: %d   仅 EN: %s   仅 ZH: %s' % (
    len(entries), paired,
    [k for k, d in entries.items() if 'en' in d and 'zh' not in d] or '无',
    [k for k, d in entries.items() if 'zh' in d and 'en' not in d] or '无',
))
