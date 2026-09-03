# PHASE0_HOTSPOT_ANALYSIS.md - 性能热点分析

> 基于 `overlay.c` 源代码分析，识别每帧性能瓶颈。

---

## 1. Alpha 蒙版计算（`buildCoverage`）

### 1.1 代码位置

- **函数**：`buildCoverage()`（第571-606行）
- **调用时机**：每帧 `repaint()` 内（第620行），在 `drawCard()` 之后

### 1.2 像素数量估算

```
蒙版尺寸：W × H = (g_cardW * 2) × (g_cardH * 2)
每帧处理像素：W × H × 4 bytes（32bpp）

普通模式 100%档位（268×184）：
  W = 536, H = 368
  像素数 = 536 × 368 = 197,248 像素
  数据量 = 197,248 × 4 = 788,992 bytes ≈ 0.75 MB

极简单行 200%档位（640×72）：
  W = 1280, H = 144
  像素数 = 1280 × 144 = 184,320 像素
  数据量 = 184,320 × 4 = 737,280 bytes ≈ 0.70 MB
```

### 1.3 下采样计算量

```c
for (y = 0; y < h; y++) {
    BYTE *r0 = (BYTE *)bits + (size_t)(y * 2)     * W * 4;
    BYTE *r1 = (BYTE *)bits + (size_t)(y * 2 + 1) * W * 4;
    BYTE *dst = cov + (size_t)y * w;
    for (x = 0; x < w; x++) {
        int s = r0[x * 8] + r0[x * 8 + 4] + r1[x * 8] + r1[x * 8 + 4];
        dst[x] = (BYTE)((s + 2) / 4);
    }
}
```

- **每像素操作**：4 次字节读 + 3 次加法 + 1 次移位 + 1 次除以 4（编译优化为右移2）
- **普通模式总操作数**：约 197,248 × 9 ≈ **177 万次操作/帧**

### 1.4 GDI 对象创建

```c
dc = CreateCompatibleDC(ref);         // 创建 DC
bmp = CreateDIBSection(...);          // 创建 DIB 位图
// ... 绘制圆角矩形 ...
SelectObject(dc, old);
DeleteDC(dc);                         // 销毁 DC
DeleteObject(bmp);                    // 销毁位图
```

每帧创建/销毁：
- **1 个兼容 DC**（`CreateCompatibleDC` / `DeleteDC`）
- **1 个 DIB 位图**（`CreateDIBSection` / `DeleteObject`）

---

## 2. PDH 采集分析

### 2.1 `gpuUsage()` 复杂度

**代码位置**：第250-287行

**数据结构**：
```c
struct { WCHAR node[64]; double total; } grp[128];  // 最多128个引擎节点
```

**计算流程**：
1. `PdhGetFormattedCounterArrayW` 获取所有 GPU 实例（约 10-20 个引擎节点 × 进程数）
2. 对每个实例：
   - `wcsstr` 查找 `luid_` 子串（O(n) 字符串搜索）
   - 在 grp 数组中线性查找匹配节点（O(m)，m≤128）
   - 累加或插入
3. 扫描 grp 数组找最大值（O(m)）

**最坏情况复杂度**：O(n × (k + m))，其中 n≈实例数，k≈实例名字符串长度，m≤128

**内存分配**：
```c
it = (FmtItem *)malloc(sz ? sz : 1);  // 动态分配
// ... 使用 ...
free(it);                              // 立即释放
```

每帧一次 malloc/free。

### 2.2 `arraySum()` 复杂度

**代码位置**：第232-245行

**计算流程**：
1. 调用 `PdhGetFormattedCounterArrayW` 获取网卡列表
2. 线性扫描所有计数器，累加有效值

**复杂度**：O(n)，n 为网卡实例数（通常 1-3 个）

**内存分配**：同样每帧一次 malloc/free。

### 2.3 `PdhCollectQueryData` 开销

- **调用频率**：每帧 sample() 时调用（第324行）
- **性质**：跨进程 PDH 查询，需要与 Windows PDH 服务通信
- **开销**：相对较高，但受限于计数器数量（GPU~20 + 网卡~3）

---

## 3. GDI 渲染每帧操作清单

### 3.1 `drawCard()` 创建的对象

| 对象类型 | 创建函数 | 销毁方式 | 每帧数量 |
|---------|---------|---------|---------|
| 兼容 DC | `CreateCompatibleDC(ref)` | `DeleteDC(dc)` | 1 |
| DIB 位图 | `CreateDIBSection(dc, &bi, ...)` | `DeleteObject(bmp)` | 1 |
| 实体画刷 | `CreateSolidBrush(fill)` | `DeleteObject(b)` | 每次 `roundRectFill` 调用 |
| 画刷（临时） | `CreatePen(PS_SOLID, 1, fill)` | `DeleteObject(p)` | 每次 `roundRectFill/Stroke` 调用 |

### 3.2 `roundRectFill()` 每帧调用次数

| 模式 | 调用次数 | 说明 |
|------|---------|------|
| 普通模式 | 最多 7 次 | 背景(1) + 边框(1) + track(3) + fill(3) + CPU进度条(1) + 内存track(1) + 内存fill(1) + GPU进度条(1) + ... 实际复杂 |
| 极简模式 | 0 次 | 极简模式不调用 roundRectFill（无进度条） |

**精确调用（普通模式）**：
- `roundRectFill(dc, 0, 0, w, h, rad, CLR_BG)` - 背景（1次）
- `roundRectStroke(dc, 0, 0, w-1, h-1, rad, CLR_BORDER)` - 边框（1次）
- `roundRectFill(dc, px(PAD_X), y, barW, bh, bh/2, CLR_TRACK)` - 轨道（3次：CPU/内存/GPU）
- `roundRectFill(dc, px(PAD_X), y, fill, bh, bh/2, accent)` - 填充（3次）

**每次 `roundRectFill` 创建/销毁**：
- 1 个 `HBRUSH`（`CreateSolidBrush`）
- 1 个 `HPEN`（`CreatePen`）
- **每帧总计**：最多约 8 个 GDI 对象在 `drawCard()` 内创建/销毁

### 3.3 `repaint()` 完整对象清单

```
repaint() 每帧执行：
├── drawCard()
│   ├── CreateCompatibleDC() → DeleteDC()
│   ├── CreateDIBSection() → DeleteObject()
│   ├── roundRectFill() × N  (N=5~8)
│   │   ├── CreateSolidBrush() → DeleteObject()
│   │   └── CreatePen() → DeleteObject()
│   └── roundRectStroke() × 1
│       ├── CreatePen() → DeleteObject()
│       └── SelectObject(GetStockObject(NULL_BRUSH))
├── buildCoverage()
│   ├── CreateCompatibleDC() → DeleteDC()
│   └── CreateDIBSection() → DeleteObject()
├── CreateCompatibleDC(NULL)  → SelectObject() → DeleteDC()
└── UpdateLayeredWindow()  // 不需要手动销毁，参数传入
```

**每帧 GDI 对象操作总数**：
- **DC 创建/销毁**：3 次（drawCard/buildCoverage/memdc）
- **位图创建/销毁**：2 次（drawCard/buildCoverage）
- **画刷创建/销毁**：5~8 次（roundRectFill）
- **画笔创建/销毁**：6~9 次（roundRectFill + roundRectStroke）
- **SelectObject**：十几次

---

## 4. 结论与优先级建议

### 4.1 热点判定

| 阶段 | 热点程度 | 原因 |
|------|---------|------|
| **Alpha 蒙版计算** | **中等** | 2x 尺寸 DIB 创建 + 下采样，~0.75MB 内存操作/帧 |
| **PDH 采集** | **中等** | 跨进程通信 + 字符串处理 + malloc/free |
| **GDI 渲染** | **较高** | 每帧创建/销毁大量 GDI 对象（刷/笔/DC/位图） |

### 4.2 主要瓶颈

1. **GDI 对象创建/销毁**：每帧 10+ 次 `Create*`/`Delete*` 调用，这是最昂贵的操作
2. **2x 蒙版位图**：每帧分配约 0.75MB 临时内存并逐像素下采样
3. **PDH 跨进程查询**：虽然开销可控，但每帧都要调用

### 4.3 重构优先级建议

| 优先级 | 优化方向 | 预期收益 |
|-------|---------|---------|
| **P0** | 缓存 GDI 对象 | 消除每帧 Create/Delete，节省 ~10+ 次 Win32 API 调用 |
| **P0** | 缓存蒙版覆盖率 | 蒙版仅依赖 `g_cardW/g_cardH/g_shape/g_scale/g_zoomPct`，这些变化时才需重建 |
| **P1** | 合并 PDH 查询 | 一次性获取所有计数器，避免多次 `PdhGetFormattedCounterArrayW` |
| **P1** | 减少 `drawCard` 内的 GDI 对象创建 | 预创建画刷/画笔，SelectObject 切换而非 Create/Delete |
| **P2** | 评估 Direct2D 替代方案 | GDI 圆角矩形效率低，Direct2D 的 `FillRoundedRectangle` 可能更快 |

### 4.4 当前架构评价

**优点**：
- 静态 GDI 对象（4个恒定）管理良好
- 字符串处理使用固定缓冲区（`grp[128]`），无频繁 malloc

**缺点**：
- 每帧动态创建/销毁大量 GDI 对象（画刷、画笔、DC、位图）
- 蒙版覆盖率每帧重算，即使尺寸未变
- PDH 采集未做帧间缓存，GPU 引擎分组每帧重算

---

*本文档基于 overlay.c v1.0 代码静态分析，未包含运行时 profiling 数据。*
