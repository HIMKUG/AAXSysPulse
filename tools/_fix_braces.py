# -*- coding: utf-8 -*-
"""为每个 set 臂补回缺失的「arm 体闭合括号」。
脚本给每个臂加了 `_ => { debug_assert!(...); false }` 后，多了一层嵌套，
但原臂体闭合 `}` 被「借用」去关 match v，导致 arm 体本身未闭合。
修复：在 `match v` 的闭合 `}` 之后再补一个 `}`（arm 体闭合）。"""
import io

PATH = r"D:\code\sys-overlay-c-master\sysoverlay\crates\win\src\settings\cfg_ref.rs"
with io.open(PATH, encoding="utf-8") as f:
    s = f.read()

old1 = "false }  },"
new1 = "false }  } },"
n1 = s.count(old1)
s = s.replace(old1, new1)

old2 = "false }  }}"
new2 = "false }  } }"
n2 = s.count(old2)
s = s.replace(old2, new2)

with io.open(PATH, "w", encoding="utf-8") as f:
    f.write(s)

print("replaced 2-close->3-close:", n1)
print("replaced last-arm:", n2)
# 自检：不应再有 2-close 形式
print("remaining 'false }  },':", s.count("false }  },"))
print("remaining 'false }  }}':", s.count("false }  }}"))
