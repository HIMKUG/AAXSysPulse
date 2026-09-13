# SysOverlay 性能分析与优化方案

> 2026-09-04 · 静态源码分析（未实测，估算值需 `--perf` 开关校准）
> 结论先行：**最占 CPU 的不是采样，是渲染管线；而渲染之所以吃满，是因为 `WM_PAINT` 忙循环把「按需重绘」变成了「无限重绘」。**

---

## ⚡ 2026-09-12 A/B 实测勘误（本文档估算值已被校准，先读这节）

> 本文其余章节的 µs 数字是**静态估算**。下表为同机 90 秒 A/B 实测（`AAXSYSPULSE_PERF=1`
> 逐帧落盘 + 进程 TotalProcessorTime），对照组用 `AAXSYSPULSE_NODIRTY=1` 切换整窗上传。
> **复现步骤**：
> ```bash
> cd sysoverlay && cargo build --release -p aaxsyspulse-app
> : > "$LOCALAPPDATA/AAXSysPulse/perf.log"
> AAXSYSPULSE_PERF=1 ./target/release/aaxsyspulse-app.exe & sleep 90   # A 模式
> # 对照组加 AAXSYSPULSE_NODIRTY=1，重跑 90s
> # 解析：grep render_d2d_ok perf.log 提取 us= 取分位数（D2D 生产引擎每帧完整耗时）
> ```
> 机器：实际桌面机 Win11；测量窗口 90s；采样节奏 ≈2s/次（46 快照/窗）；另有每秒时钟唤醒。

| 指标（90s 窗口） | A 模式（哈希跳帧+脏区上传） | B 对照（整窗上传 NODIRTY=1） |
|---|---|---|
| D2D 完成帧（render_d2d_ok） | 127 | 90 |
| **每帧 p50** | **3240µs** | **3242µs** |
| 每帧 p90 | 3576µs | 3470µs |
| 每帧 p99 | 3916µs | 180108µs |
| 每帧 max | 298444µs | 180108µs |
| 哈希跳过帧（frame_skip） | 13 | 0 |
| GDI 回退帧（frame_render） | 11（p50 539µs） | 0 |
| **进程总 CPU / 90s** | **1.172s（≈1.3% 单核）** | **1.188s（≈1.3% 单核）** |

### 逐条勘误

1. ❌ **"全帧 0.4-1.5ms" → 实测 p50 3.24ms**，估算低 2~8 倍。D2D 路径的大头不在本文假设的"字形光栅化+ULW"，在 D2D 绘制+present 链路本身。
2. ❌ **"脏区上传每次省 100-300µs+" → p50 实测零差异（3240 vs 3242µs）**。根因：数值型内容变化遍布各卡片，变化帧的脏区≈整窗，`prcDirty` 无可省。脏区上传只在单 cell 级局部变化（如时钟秒）时才有意义。
3. ⚠️ **跳帧机制方向正确但绝对量极小**：实测 duty cycle ≈9%（13/140 唤醒，且另一组运行为 0/90——对内容节奏高度敏感），节省 ≈ 13×3.24ms ≈ 42ms/90s = **0.03% CPU**。
4. ✅ **哈希成本占比实测界 ~0.1%**（≤40 cells 的 FNV ≈1-3µs vs 3.24ms 帧成本），远低于 10%——duty cycle 门槛修正为 **~0.1%**：跳帧几乎永远净赚，但赚的是毫瓦级。
5. ⚠️ **两模式都有 100ms+ 尾刺**（A max 298ms / B max 180ms；B p99 即 180ms），单次运行样本不足无法归因，值得长窗口+ETW 专项追。
6. ⚠️ **仪器缺口（待修）**：D2D 生产路径此前不喂 `end_render`（perf.log 聚合只统计 GDI 兜底引擎），本次改用 `render_d2d_ok` section（t0 自 render() 入口透传）作为每帧成本。**待办：给 D2D 成功路径接 `end_render`。**
7. ⚠️ **A 组出现 11 帧 D2D 瞬时失败**（失败→指纹重置→GDI 全帧兜底，失败率 ~8%），B 组 0 次——D2D 瞬时失败的原因与节流值得单独查。
8. ✅ 进程空闲总 CPU ≈1.3% 单核——本文 P0 宣称的"WM_PAINT 忙循环 6% CPU"确认已修复。
9. ⚠️ NODIRTY 模式下消息循环流量 20×（130-208 条/5s vs 8-11 条）——原因未查，经典 ULW 路径疑似触发额外 repaint 消息。

### 对优化路线的影响

- 悬浮窗渲染管线的"教科书级"评价**撤销**：机制（挂起+合并+哈希）工程上优雅且无负收益，但在 0.5Hz 数据节奏下绝对收益是毫瓦级。真实功耗大头尚未定位（3.24ms/帧的 p50 分解是下一个测量目标）。
- 若未来刷新率提升 10×（→32% CPU）或卡片数量翻倍，3.24ms/帧才成为值得攻的目标；届时优先分解 D2D 绘制 vs present 的占比，而不是继续在脏区/哈希上加码。
- 采样端与渲染端 CPU 各占进程总量的 ~35%——"采样几乎不花钱"的结论（§1）在 2s 间隔下依然成立。

---

## 1. 先排除：采样端几乎不花钱

| 采样器 | 实现 | 单次成本 |
|---|---|---|
| CPU | `proc_cpu.rs:36-70` — `GetSystemTimes` + 三次 u64 拼装 | ~1-5 µs（1 次 syscall） |
| Memory | `GlobalMemoryStatusEx`（一次 syscall + 结构体填充） | ~1-5 µs |
| **GPU** | `proc_gpu.rs:24-39` — **TODO 桩**，恒返回 `Unavailable` | **0** |
| **Net** | `proc_net.rs:44-54` — **TODO 桩**，恒返回 `{0,0}` | **0** |

也就是说：即便 10fps 全量采样，采样端总成本 < 0.01% CPU。
**你观察到的 6% 与采样频率无关，改刷新率档位不会有任何变化** —— 这与你的观察完全吻合。

> 附带发现：GPU/Net 目前是桩，界面上 GPU 恒显示 `--`、Net 恒显示 `0.0M/s`。
> 这是**功能缺口**，不是性能问题（见 §6 待拍板项）。

---

## 2. 根因：`WM_PAINT` 永动机（P0）

### 证据

1. `crates/win/src/window.rs` 的 `WM_PAINT` 分支：

```rust
WM_PAINT => {
    if let Some(state) = get_state_ptr(hwnd).as_mut() {
        let snap = snap_get();
        crate::gdi::render(hwnd, &snap, &state.cfg);   // 只画，不校验更新区域
    }
    LRESULT(0)      // ← 没有 BeginPaint/EndPaint，也没有 ValidateRect
}
```

2. 全仓库 `BeginPaint`/`ValidateRect` 调用统计：**只有设置面板 `settings.rs:765` 做了**，悬浮窗一处都没有。

3. 三处 `InvalidateRect` 会留下**永久无效区**：
   - `window.rs:287`（`WM_SIZE`）
   - `window.rs:363`（`WM_DISPLAYCHANGE`）
   - `gdi.rs:403`（`fit_window_to_strip`，**启动时与每次设置改动都会调用**）

### 机制

Windows 只在「更新区域非空且队列空闲」时投递 `WM_PAINT`，但**清空更新区域是应用的责任**
（`BeginPaint`/`EndPaint` 或 `ValidateRect`）。我们没有清 → 区域永远非空 →
系统**持续不断地重新投递 `WM_PAINT`**。

后果链：

- 旧消息循环 `PeekMessageW → 有消息就 dispatch，没消息才 sleep(interval)`：
  因为永远有 `WM_PAINT`，**`sleep` 分支几乎从不执行** → 刷新率设置形同虚设；
- 我把循环换成阻塞式 `GetMessageW` 后：`GetMessage` 立刻返回 `WM_PAINT`，
  **同样不会阻塞**。所以我上一轮声称的「空闲 0% CPU」**在这条缺陷修好前并不成立** —— 这点我必须更正。

### 量级估算（解释那 6%）

单帧成本估算见 §3（约 0.2-1.5 ms，取决于字号/描边）。
若 `WM_PAINT` 以 100-300 Hz 重投递：

```
150 Hz × 0.4 ms ≈ 60 ms/s ≈ 一核的 6%
```

**与实测 6% 高度吻合** —— 这就是根因。

---

## 3. 单帧成本拆解（1170×40 窗口、font_scale=2、约 65 字符）

| 阶段 | 位置 | 估算成本 | 说明 |
|---|---|---|---|
| `GetWindowRect` | `gdi.rs:282` | ~1 µs | 可省（尺寸变化才需要） |
| `Style::derive` | `gdi.rs:292` | ~1 µs | 每帧重算调色板 |
| `build_cells` | `gdi.rs:69-157` | ~3-10 µs | `Vec<Cell>` 每帧分配（`StrBuf` 是栈缓冲，无堆分配 ✅） |
| **离屏缓冲分配+清零** | `offscreen.rs:29-36` `vec![0u8; 187_200]` | **15-30 µs** | 每帧 187 KB 分配 + memset |
| **字形光栅化** | `font.rs:348-427` | **200-1100 µs** | 逐像素 f32 混合；`outline` 开 = **6 个 pass**（4 描边+1 阴影+1 正文），默认只开阴影 = 2 pass |
| **创建 DIB** | `gdi.rs:342` `CreateDIBSection` | **30-100 µs** | **每帧新建/销毁 GDI 位图**（内核分配 187 KB） |
| 全量拷贝 | `gdi.rs:360-363` | ~15 µs | 187 KB `copy_from_slice` |
| **上屏** | `gdi.rs:378` `UpdateLayeredWindow` | **100-300 µs+** | DWM 重合成，抖动大 |

**单帧合计 ≈ 0.4-1.5 ms**。这些成本本来 1 fps 下可以忽略，但被 `WM_PAINT` 永动放大几百倍。

---

## 4. 优化方案（按 ROI 排序）

### P0 · 修掉 `WM_PAINT` 忙循环 —— 预期砍掉 80-95% CPU

- `window.rs` `WM_PAINT` 分支改为 `BeginPaint`+`EndPaint`（或 `ValidateRect`）清空更新区域，**不再在此分支渲染**；
- 三处 `InvalidateRect` 改为「标记 dirty + 主动渲染一次」：
  - `window.rs:287`（`WM_SIZE`）、`window.rs:363`（`WM_DISPLAYCHANGE`）→ 直接调 `gdi::render`；
  - `gdi.rs:403`（`fit_window_to_strip`）→ 去掉 `InvalidateRect`，尺寸变更后由调用方渲染一次；
- 渲染继续由 `WM_NEW_SNAPSHOT`（采样线程推送）驱动。

**风险**：低。**收益**：帧数真正等于刷新率（1fps → 1 帧/秒）。这是**必须做**的一项。

### P1 · 脏帧跳过 —— 静止时接近 0 开销

在 `render()` 入口比较本次要绘制的**字符串序列**（`build_cells` 产出的文本 + 颜色）与上一帧，
完全一致则直接返回：不建 surface、不建 DIB、不 `UpdateLayeredWindow`。

- 静止机器（CPU 0%、内存不变、无流量、GPU/Net 为桩）→ 采样线程照跑但**零渲染**；
- 实现位置：`gdi.rs:279` `render()`，缓存上一帧签名（`Vec<Cell>` 的文本哈希或逐字段比对，约 15 个 cell）。

**风险**：低（需要保证配置变化/尺寸变化强制失效）。**收益**：空闲场景 CPU → 接近 0。

### P2 · 复用离屏缓冲与 DIB —— 每帧省 50-150 µs

- `SoftwareSurface` 增加 `clear()`，改为复用：尺寸不变就 `fill(0)`，不重新 `vec![]`；
- DIB 位图按 `(w,h)` 缓存（`static` 或 `OnceLock`），只在尺寸变化时 `CreateDIBSection`；
- `update_layered` 里的 `mem_dc` 同样可复用。

**风险**：中低（需处理 DPI/尺寸变化与窗口销毁时的 GDI 对象释放）。
**收益**：P0 之后这是每帧最大的固定开销，10fps 下约省 0.5-1.5% CPU。

### P3 · 字形图集（glyph atlas）—— 光栅化省 60-80%

- 预渲染 `96 字符 × 4 档缩放` 的**不透明字形位图**（每种颜色/每种 pass 各一份太多，
  建议缓存「字形掩码 + 颜色」两张表，绘制时按掩码 blit）；
- 或直接缓存 **cell 级渲染结果**：数值文本变化只重画变化的那一个 cell。

**风险**：中（要改 `ui` crate 的渲染接口，需补测试）。
**收益**：描边开启时收益最大（6 pass → 1 pass）。P0+P1 之后才需要考虑。

### P4 · 零散项（各 <5%）

| 项 | 做法 | 收益 |
|---|---|---|
| 采样线程优先级 | `SetThreadPriority(THREAD_PRIORITY_BELOW_NORMAL)` | 不抢前台响应 |
| 精确休眠 | `thread::sleep` 受 15.6 ms 时钟粒度影响，100 ms 档误差可达 15%；改用 `CreateWaitableTimerEx` | 刷新节奏更准 |
| 隐藏时暂停 | 托盘隐藏时不仅跳过渲染，也让采样降到最低档（`schedule_cfg` 增一位） | 隐藏后趋近 0 |
| 减少每帧分配 | `build_cells` 的 `Vec<Cell>` 复用缓冲 | ~5-10 µs/帧 |

### P5 · 加 `--perf` 测量开关（建议做）

`AAXSYSPULSE_PERF=1` 时，用 `QueryPerformanceCounter` 统计并每 5 秒打到 stderr：
**帧数/秒、单帧平均/最大耗时、被脏检查跳过次数**。
这样你我都能用数据校准上面的估算，而不是靠猜。

---

## 5. 建议实施顺序

1. **P0**（必须）→ 2. **P5 测量开关**（先装表，再优化）→ 3. **P1** → 4. **P2** → 5. P3/P4 视数据决定。

P0+P1 落地后，你原本观察到的 6% 应降到 **0.5% 以下**（静态场景接近 0）。
P2/P3 是进一步的「每帧成本」压缩，在 10fps 档位下才有可感知差异。

---

## 6. 待你拍板的点

1. **实施范围**：只做 P0+P1 止血 / P0+P1+P5 / P0+P1+P2+P3（推荐）/ 全做；
2. **是否接受 P5 的 `--perf` 开关**（会往 stderr 打日志，GUI 子系统下不可见，需重定向到文件）；
3. **GPU/Net 真实采样是否纳入本轮**：那是功能补全（目前是桩、界面恒显示 `--`/`0.0`），
   实现后会引入新的采样成本（PDH/D3DKMT 与 `GetIfTable2`），不过分频调度已就位（GPU/Net ≥1s）。
