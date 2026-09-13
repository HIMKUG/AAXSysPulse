import re, sys, subprocess

def strip_comments(s):
    out=[]; in_str=False; esc=False; j=0; n=len(s)
    while j<n:
        c=s[j]
        if in_str:
            out.append(c)
            if esc: esc=False
            elif c=='\\': esc=True
            elif c=='"': in_str=False
        else:
            if c=='"': in_str=True; out.append(c)
            elif c=='/' and j+1<n and s[j+1]=='/':
                while j<n and s[j]!='\n': j+=1
                continue
            else: out.append(c)
        j+=1
    return "".join(out)

ARM=re.compile(r'\(\s*"([^"]+)"\s*,\s*(false|true)\s*\)\s*=>\s*("(?:\\.|[^"\\])*"|\{[^{}]*\})', re.DOTALL)

def parse_arms(text):
    clean=strip_comments(text)
    e={}
    for m in ARM.finditer(clean):
        key=m.group(1); is_zh=(m.group(2)=="true"); val=m.group(3)
        if val.startswith('"'):
            expr=val
        else:
            inner=val[1:-1].strip()
            cm=re.match(r'const\s+S\s*:\s*&str\s*=\s*(.+?)\s*;\s*S\s*$', inner, re.DOTALL)
            expr=cm.group(1).strip() if cm else inner
        e.setdefault(key,{})[is_zh]=expr
    return e

# ORIGINAL canonical from git HEAD
orig_b=subprocess.check_output(["git","show","HEAD:crates/win/src/i18n.rs"]).decode("utf-8")
ms=orig_b.index("match (key, zh) {")
end=orig_b.index("_ => key,")  # arms region ends here
orig_arms=parse_arms(orig_b[ms:end])
assert len(orig_arms)==188, f"orig arm count {len(orig_arms)}"

# NEW: parse TABLE array robustly (handle concat!(...) with commas)
new=open(r"D:\code\sys-overlay-c-master\sysoverlay\crates\win\src\i18n.rs",encoding="utf-8").read()
ts=new.index("const TABLE:"); te=new.index("];", ts)
tbl=new[ts:te+2]

def parse_table(text):
    # entries: ( EN , ZH , "KEY" )
    res={}
    i=0; n=len(text)
    while i<n:
        if text[i]!='(':
            i+=1; continue
        # parse expr until matching close paren at depth 0
        depth=0; j=i; in_s=False; esc=False; parts=[]
        while j<n:
            c=text[j]
            if in_s:
                if esc: esc=False
                elif c=='\\': esc=True
                elif c=='"': in_s=False
            else:
                if c=='"': in_s=True
                elif c=='(': depth+=1
                elif c==')':
                    depth-=1
                    if depth==0:
                        break
            j+=1
        entry=text[i+1:j]  # content between parens
        # split into 3 by top-level commas
        segs=[]; d=0; k=0; in_s2=False; esc2=False; cur=""
        while k<len(entry):
            c=entry[k]
            if in_s2:
                cur+=c
                if esc2: esc2=False
                elif c=='\\': esc2=True
                elif c=='"': in_s2=False
            else:
                if c=='"': in_s2=True; cur+=c
                elif c=='(': d+=1
                elif c==')': d-=1
                elif c==',' and d==0:
                    segs.append(cur); cur=""
                else: cur+=c
            k+=1
        segs.append(cur)
        if len(segs)==3:
            en=segs[0].strip(); zh=segs[1].strip(); key=segs[2].strip().strip('"')
            res[key]=(en,zh)
        i=j+1
    return res

newd=parse_table(tbl)
assert len(newd)==188, f"new table count {len(newd)}"

ok=True
for k in sorted(set(orig_arms)|set(newd)):
    if k not in orig_arms:
        sys.stderr.write(f"NEW ONLY {k}\n"); ok=False; continue
    if k not in newd:
        sys.stderr.write(f"MISSING {k}\n"); ok=False; continue
    oe=orig_arms[k][False]; oz=orig_arms[k][True]; ne2,nz2=newd[k]
    if oe!=ne2 or oz!=nz2:
        sys.stderr.write(f"DIFF {k}\n  o_en={oe!r}\n  n_en={ne2!r}\n  o_zh={oz!r}\n  n_zh={nz2!r}\n"); ok=False
sys.stderr.write(f"keys orig={len(orig_arms)} new={len(newd)} MATCH={'YES' if ok else 'NO'}\n")
sys.exit(0 if ok else 1)
