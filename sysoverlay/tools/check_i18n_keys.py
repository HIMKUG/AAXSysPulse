import re, os, subprocess

# 1. keys in TABLE
t=open(r"D:\code\sys-overlay-c-master\sysoverlay\crates\win\src\i18n.rs",encoding="utf-8").read()
ts=t.index("const TABLE:"); te=t.index("];",ts)
seg=t[ts:te+2]
keys=set(re.findall(r'"([A-Za-z][A-Za-z0-9]*)"\),', seg))
print("TABLE keys:", len(keys))

# 2. collect all t(..., "KEY") usages across crates (win/src, core maybe)
used=set()
root=r"D:\code\sys-overlay-c-master\sysoverlay\crates"
callpat=re.compile(r'\bt\(\s*[^,]+,\s*"([^"]+)"\s*\)')
for dp,_,fns in os.walk(root):
    for fn in fns:
        if fn.endswith(".rs"):
            p=os.path.join(dp,fn)
            try: txt=open(p,encoding="utf-8").read()
            except: continue
            for m in callpat.finditer(txt):
                used.add(m.group(1))

missing=sorted(k for k in used if k not in keys)
print("used distinct keys:", len(used))
print("USED-BUT-MISSING:", missing)
# Also: keys in table that are never used (informational, not an error)
unused=sorted(k for k in keys if k not in used)
print("TABLE keys never referenced by a literal t() call:", len(unused))
print(unused)
