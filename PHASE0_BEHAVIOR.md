# PHASE0_BEHAVIOR.md - SysOverlay-C 对外行为清单（冻结版）

> 本文档基于 `overlay.c` v1.0 代码完整提取，作为后续重构的基准。所有行为均从源代码反向得出，供重构验证使用。

---

## 1. 数值格式规范

### 1.1 CPU/GPU 百分比

- **格式**：`%.0f%%`（无小数位，直接取整）
- **来源**：`StringCchPrintfW(buf, ..., L"%.0f%%", g_m.cpu)`（第456、459、462、477、479、481、497、540、551行）
- **范围**：0~100%（代码内有 clamp，见第311行 `pct < 0 ? 0 : (pct > 100 ? 100 : pct)`）
- **阈值颜色**：
  - `>= 85%` → `CLR_HOT` = `RGB(248, 113, 113)`（红色）
  - `>= 60%` → `CLR_WARN` = `RGB(251, 191, 36)`（黄色）
  - `<= 1%` → `CLR_IDLE` = `RGB(63, 69, 80)`（灰色）
  - 其他 → `CLR_OK` = `RGB(74, 222, 128)`（绿色）

### 1.2 内存百分比 + GB 显示

- **百分比格式**：`%.0f%%`（同 CPU）
- **GB 格式**：`%.1f/%.0fG`（已用 1 位小数，总量 0 位小数）
- **来源**：第541-542行 `StringCchPrintfW(buf, ARRAYSIZE(buf), L"%.0f%% · %.1f/%.0fG", g_m.mem, g_m.memUsedGb, g_m.memTotalGb);`
- **计算公式**：
  - `memTotalGb = ms.ullTotalPhys / 1073741824.0`（第319行，即除以 1024^3）
  - `memUsedGb = (ms.ullTotalPhys - ms.ullAvailPhys) / 1073741824.0`（第320行）

### 1.3 网速单位换算

| 设置档位 | 显示字符串 | 内部换算基准 |
|----------|-----------|-------------|
| `g_netUnit=0`（KB/s） | `%.0f KB/s` | `mbps * 1024.0`（MB/s→KB/s，乘以 1024） |
| `g_netUnit=1`（MB/s） | `>=100 时 %.0f MB/s`，`<100 时 %.2f MB/s` | 直接显示 MB/s 值 |
| `g_netUnit=2`（GB/s） | `%.2f GB/s` | `mbps / 1024.0`（MB/s→GB/s，除以 1024） |

- **内部数据单位**：MB/s（兆字节/秒）
- **来源**：`m->netR = arraySum(g_nr) / 1048576.0`（第326行），即字节/秒 ÷ 1048576 = MB/s
- **极简模式短格式**：无空格后缀（`K`/`M`/`G`），如 `StringCchPrintfW(out, cap, L"%.0fK", ...)`（第401-404行）

---

## 2. 刷新间隔

### 2.1 可选值与默认值

| 档位索引 `i` | 存储值 `INTERVAL_DDJ[i]` | 实际毫秒 | 菜单显示 |
|-------------|------------------------|---------|---------|
| 0 | 5 | 500ms | `0.5 秒` |
| 1 | 10 | 1000ms | `1.0 秒` |
| 2 | 20 | 2000ms | `2.0 秒` |

- **默认值**：`g_intervalDdj = 10`（第87行，初始值）
- **菜单显示格式**：`%.1f 秒`（第700行）
- **IDM_IV(i)**：`IDM_IV(i) = 20 + (i)`（第45行）

### 2.2 控制字段

- **全局变量**：`g_intervalDdj`（int，第87行）
- **Timer 设置**：`SetTimer(g_hwnd, 1, (UINT)(g_intervalDdj * 100), NULL)`（第987行，初始化时；第813行，菜单修改时）
- **定时器消息处理**：WM_TIMER / wp==1 → `sample(&g_m); g_mValid = TRUE; repaint();`（第823-834行）

---

## 3. 悬停阈值与渐隐

### 3.1 触发机制

- **悬停模式开关**：`g_hoverMode`（BOOL，第90行）
- **鼠标是否在卡片上**：`g_hovering`（BOOL，第91行）
- **点亮截止时间**：`g_revealUntil`（DWORD，第92行），`GetTickCount() + 10000`（启动时初始化 10 秒）

### 3.2 悬停检测流程

1. **WM_MOUSEMOVE**：当 `g_hoverMode && !g_hovering` 时（第843行）：
   - 设置 `g_hovering = TRUE`
   - 调用 `TrackMouseEvent(&tme)` 其中 `tme = { sizeof(tme), TME_LEAVE, h, 0 }`（一次性离开事件）
   - 调用 `repaint()`

2. **WM_MOUSELEAVE**：当 `g_hovering` 时（第868-869行）：
   - 设置 `g_hovering = FALSE`
   - 调用 `repaint()`

### 3.3 隐藏态透明度

- **隐藏条件**（第617行）：
  ```c
  int ghost = g_hoverMode && !g_hovering && !g_modalHold && GetTickCount() > g_revealUntil;
  ```
  即：悬停模式开启 AND 鼠标不在卡片上 AND 未在菜单/对话框中 AND 已超过启动点亮期

- **隐藏态 alpha 值**：非零像素在 ghost 状态下被强制设为 `3`（第647行 `if (ghost && a) a = 3;`）
  - 约 1.2% 透明度，肉眼不可见
  - 但仍可接收鼠标消息（命中只要求非零）
  - 避免 alpha=0 时分层窗口点击穿透、无法唤出菜单

### 3.4 变量协同关系

| 变量 | 含义 | 初始化/修改 |
|------|------|------------|
| `g_hoverMode` | 悬停模式总开关（菜单切换） | 初始 FALSE；菜单 IDM_HOVER 切换 |
| `g_hovering` | 鼠标当前是否在卡片上 | WM_MOUSEMOVE 设置 TRUE；WM_MOUSELEAVE 设置 FALSE |
| `g_revealUntil` | 启动后强制点亮截止时间 | 初始化为 `GetTickCount() + 10000`（10秒） |
| `g_modalHold` | 菜单/颜色对话框是否打开中 | 打开前 TRUE（第744、762行），关闭后 FALSE |

---

## 4. 透明度映射

### 4.1 公式

```
SourceConstantAlpha = (cov[i] * g_opacityPct) / 100
```

- `cov[i]`：该像素的圆角覆盖率（0~255，来自 2x 下采样蒙版）
- `g_opacityPct`：用户选择的透明度百分比（10~100）
- **注意**：`UpdateLayeredWindow` 的 `SourceConstantAlpha` 被设为 255（第659行），透明度完全由预乘 alpha 实现

### 4.2 取值范围

- **可选值**：`OPACITY_PCT[10] = {10, 20, 30, 40, 50, 60, 70, 80, 90, 100}`（第51行）
- **默认值**：`g_opacityPct = 90`（第87行）
- **菜单 ID**：`IDM_OP(i) = 10 + (i)`（第44行）

### 4.3 纯内容模式（shape=2）的 alpha 计算

```c
int m = p[0] > p[1] ? (p[0] > p[2] ? p[0] : p[2]) : (p[1] > p[2] ? p[1] : p[2]);
a = m * 7 / 5;        // 亮度×1.4 补偿彩色进度条暗通道
if (a > 255) a = 255;
if (!a) a = 1;        // 避免点击穿透
```

---

## 5. 缩放档位

### 5.1 档位定义

| 档位 `i` | `ZOOM_PCT[i]` | 实际缩放 | 菜单显示 |
|----------|--------------|---------|---------|
| 0 | 75 | 0.75x | `75%` |
| 1 | 100 | 1.0x | `100%` |
| 2 | 125 | 1.25x | `125%` |
| 3 | 150 | 1.5x | `150%` |
| 4 | 200 | 2.0x | `200%` |

- **默认值**：`g_zoomPct = 100`（第88行）
- **IDM_ZOOM(i)**：`IDM_ZOOM(i) = 30 + (i)`（第46行）

### 5.2 尺寸换算函数

```c
static int px(int logical) { return (int)(logical * g_scale * g_zoomPct / 100.0 + 0.5); }
```

- `g_scale`：DPI 缩放因子（`GetDpiForMonitor` 取主屏真实 DPI / 96.0）
- `g_zoomPct`：用户档位百分比
- 所有逻辑尺寸（`CARD_W_L=268` 等）经 `px()` 转换后生效

### 5.3 各模式实际像素尺寸（96dpi / 100%档位）

| 模式 | 逻辑尺寸 | 档位 75% | 档位 100% | 档位 125% | 档位 150% | 档位 200% |
|------|---------|---------|-----------|-----------|-----------|-----------|
| 普通 | 268×184 | 201×138 | 268×184 | 335×230 | 402×276 | 536×368 |
| 极简-单行 | 320×36 | 240×27 | 320×36 | 400×45 | 480×54 | 640×72 |
| 极简-单列 | 128×96 | 96×72 | 128×96 | 160×120 | 192×144 | 256×192 |
| 极简-分列 | 88×162 | 66×122 | 88×162 | 110×203 | 132×243 | 176×324 |

### 5.4 字号换算

```c
g_uiFont   = CreateFontW(-px(12), ...);   // 12×缩放
g_monoFont = CreateFontW(-px(12), ...);   // 12×缩放，Cascadia Mono
```

字号随档位线性变化，无独立倍数关系。

---

## 6. 配置字段

### 6.1 完整键名列表

| 存储键名 | C 变量 | 类型 | 默认值 | 取值范围 | 说明 |
|---------|--------|------|--------|---------|------|
| `pos X Y` | `g_posX`, `g_posY` | int | `-99999, -99999`（未设置标记） | 屏幕坐标 | 窗口位置 |
| `topmost 0/1` | `g_topmost` | BOOL | `TRUE` | 0/1 | 是否置顶 |
| `opacity N` | `g_opacityPct` | int | 90 | 10/20/.../100 | 透明度% |
| `interval N` | `g_intervalDdj` | int | 10 | 5/10/20 | 刷新间隔×10（秒） |
| `minimal 0/1` | `g_minimal` | BOOL | `FALSE` | 0/1 | 极简模式 |
| `zoom N` | `g_zoomPct` | int | 100 | 75/100/125/150/200 | 缩放% |
| `hover 0/1` | `g_hoverMode` | BOOL | `FALSE` | 0/1 | 悬停显示 |
| `netunit N` | `g_netUnit` | int | 1 | 0/1/2 | 0=KB/s,1=MB/s,2=GB/s |
| `shape N` | `g_shape` | int | 0 | 0/1/2 | 0=圆角,1=直角,2=纯内容 |
| `vert N` | `g_vert` | int | 0 | 0/1/2 | 极简排列：0=单行,1=单列,2=分列 |
| `bg 0xRRGGBB` | `g_bgColor` | COLORREF | `RGB(20,22,27)` | 0x000000-0xFFFFFF | 背景色 |
| `fg 0xRRGGBB` | `g_fgColor` | COLORREF | `RGB(238,241,244)` | 0x000000-0xFFFFFF | 前景/文字色 |

### 6.2 解析方式

```c
sscanf(line, "pos %ld %ld", &a, &b)      // 位置
sscanf(line, "topmost %ld", &a)          // 置顶
sscanf(line, "opacity %ld", &a)          // 透明度
sscanf(line, "interval %ld", &a)         // 刷新间隔
sscanf(line, "minimal %ld", &a)          // 极简模式
sscanf(line, "zoom %ld", &a)             // 缩放
sscanf(line, "hover %ld", &a)            // 悬停
sscanf(line, "netunit %ld", &a)          // 网速单位
sscanf(line, "shape %ld", &a)            // 形状
sscanf(line, "vert %ld", &a)             // 竖排
sscanf(line, "bg %lx", &a)               // 背景色
sscanf(line, "fg %lx", &a)               // 前景色
```

颜色解析时 `a` 为 BGR 顺序存储的 HEX 值（注意 `%lx` 读取顺序与 RGB 相反），第163-164行：
```c
g_bgColor = RGB(a & 255, (a >> 8) & 255, (a >> 16) & 255);
g_fgColor = RGB(a & 255, (a >> 8) & 255, (a >> 16) & 255);
```

---

## 7. 菜单项完整列表

### 7.1 菜单 ID 映射

| 菜单 ID | 功能 | 勾选语义 | 备注 |
|---------|------|---------|------|
| `IDM_TOPMOST (1)` | 窗口置顶 | checked=置顶中 | |
| `IDM_MINIMAL (2)` | 极简模式 | checked=已开启 | |
| `IDM_HOVER (3)` | 悬停显示 | checked=已开启 | |
| `IDM_BGCLR (6)` | 背景色… | 无 | 打开颜色选择对话框 |
| `IDM_FGCLR (7)` | 文字色… | 无 | 打开颜色选择对话框 |
| `IDM_CLRDEF (8)` | 恢复默认配色 | 无 | |
| `IDM_QUIT (9)` | 退出 | 无 | 销毁窗口触发退出 |
| `IDM_OP(i) (10+i)` | 透明度 i | checked=当前选中 | i=0..9 对应 10%..100% |
| `IDM_IV(i) (20+i)` | 刷新间隔 i | checked=当前选中 | i=0..2 对应 0.5s/1.0s/2.0s |
| `IDM_ZOOM(i) (30+i)` | 大小 i | checked=当前选中 | i=0..4 对应 75%/100%/125%/150%/200% |
| `IDM_UNIT(i) (40+i)` | 网速单位 i | checked=当前选中 | i=0..2 对应 KB/s/MB/s/GB/s |
| `IDM_SHAPE(i) (50+i)` | 形状 i | checked=当前选中 | i=0..2 对应 圆角/直角/纯内容 |
| `IDM_VERT(i) (60+i)` | 竖排(极简) i | checked=当前选中 | i=0..2 对应 单行/单列/分列 |

### 7.2 菜单结构

```
根菜单（右键弹出）
├── 窗口置顶                    [checkbox]
├── 极简模式                    [checkbox]
├── 悬停显示                    [checkbox]
├── 竖排(极简)                  [submenu]
│   ├── 单行                    [radio]
│   ├── 单列                    [radio]
│   └── 分列                    [radio]
├── 透明度                      [submenu]
│   ├── 10% ... 100%            [radio]
├── 刷新间隔                    [submenu]
│   ├── 0.5 秒 / 1.0 秒 / 2.0 秒 [radio]
├── 大小                        [submenu]
│   ├── 75% / 100% / 125% / 150% / 200% [radio]
├── 网速单位                    [submenu]
│   ├── KB/s / MB/s / GB/s     [radio]
├── 形状                        [submenu]
│   ├── 圆角 / 直角 / 纯内容    [radio]
├── 颜色                        [submenu]
│   ├── 背景色…
│   ├── 文字色…
│   ├── ──────────
│   └── 恢复默认配色
├── ──────────
└── 退出
```

---

## 8. 极简模式三种排列的布局细节

### 8.1 单行模式（`g_vert == 0`，默认）

- **卡片尺寸**：320×36（逻辑尺寸，下同）
- **布局策略**：所有四项指标在一行内
  - 右侧：网速（格式 `↓Xs ↑Ys`，短格式无空格）
  - 左侧：CPU/内存/GPU 合并为一行文字 `CPU X% · 内存 Y% · GPU Z%`
  - 左侧文字右端裁切到网速区之前，极长时以省略号收尾（`DT_END_ELLIPSIS`）
- **垂直居中**：`ty = (h - lh) / 2`（第490行）
- **字号**：使用 `g_uiFont`（左）和 `g_monoFont`（右）

### 8.2 单列模式（`g_vert == 1`）

- **卡片尺寸**：128×96
- **布局策略**：四项指标各占一行，左对齐
  ```
  CPU XX%
  内存 XX%
  GPU XX%
  ↓Xs ↑Ys
  ```
- **行高**：`lh = px(18)`，行间距 `lh + px(3)`
- **顶部偏移**：`ty = px(9)`
- **网速格式**：`↓Xs ↑Ys`（短格式，第483行）

### 8.3 分列模式（`g_vert == 2`）

- **卡片尺寸**：88×162
- **布局策略**：名称和数值分两列，每项占两行（名称行+数值行），下载/上传各单独一行
  ```
  CPU
  XX%
  内存
  XX%
  GPU
  XX%
  ↓Xs
  ↑Ys
  ```
- **列对齐**：名称在左列（`px(PAD_X)`），数值紧随其后
- **行高**：`rh = px(16)`
- **顶部偏移**：`ty = px(8)`
- **网速格式**：短格式，`↓Xs` 和 `↑Ys` 各占一行（第464-467行）

---

## 附录：颜色派生规则

```c
g_cTitle  = scaleColor(g_fgColor, 0.76);   // 前景色×0.76
g_cName   = scaleColor(g_fgColor, 0.65);   // 前景色×0.65
g_cSub    = scaleColor(g_fgColor, 0.45);   // 前景色×0.45
g_cTrack  = mixColor(g_bgColor, g_fgColor, 0.10);  // 背景色+10%前景色
g_cBorder = mixColor(g_bgColor, g_fgColor, 0.13);  // 背景色+13%前景色
```

其中 `scaleColor` 为分量乘法并 clamp 255，`mixColor` 为线性插值。

---

*本文档由 Phase 0 从 overlay.c 源代码反向提取生成，代码行号对应 v1.0。*
