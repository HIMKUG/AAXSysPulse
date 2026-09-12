# SysOverlay-C 多核优化报告（2026-09-08）

> 目的：说明本程序**如何把工作分摊到多个 CPU 核心**，以及为什么稳态整机 CPU 能做到 0.03–0.09%（16 逻辑核机）。
> 配套实测见 `PERF_RESULTS_2026-09-08.md`（第三轮 Net 压缩、第四轮 事件驱动 NIC + 进程榜降频）。

---

## 1. 线程拓扑总览

程序启动后**常驻 3 条线程**，外加按需的瞬态线程；它们互不阻塞、靠无锁结构交接数据：

| 线程 | 角色 | 绑定/优先级 | 是否常驻 | 稳态开销 |
|------|------|------------|---------|---------|
| **UI 线程**（主线程） | `OverlayWindow::run_message_loop()` 消息泵 + 分层窗口重绘 | 系统调度（通常为前台核） | 是 | 仅收到 `WM_NEW_SNAPSHOT` 才画，单帧 ~100–200µs |
| **采样线程** | `SystemSampler` 驱动调度器轮询 CPU/Mem/GPU/Net/Power → `snap_store` | **绑 core 0**，`BELOW_NORMAL` 优先级 | 是 | 分频后每周期 <1ms；隐藏时挂起 |
| **进程榜线程** | 独立 2s 轮询进程表 → `PostMessage` 推给面板 | 系统调度 | 是（仅面板打开时跑） | 仅面板拉起时才有，不计入悬浮条 |
| 历史曲线线程 | 曲线窗口按需，从 `ArcSwap` 无锁读 `Snapshot` | 系统调度 | 按需 | 窗口未开时 `push` 直接跳过 |
| 诊断点击线程 | `SYSOVERLAY_DIAG=click` 临时模拟点击 | 系统调度 | 临时 | 仅排障用 |

> 关键事实：**GPU vendor 探测（`pick_dgpu`）不在热路径**。它走 `OnceLock` 一次性完成，之后每周期只对主适配器做 `D3DKMT RunningTime` 增量查询（~2ms）。`Power`（RAPL MSR）同样在采样线程内、驱动 `OnceLock` 一次性就绪。

---

## 2. 各线程如何并行 + 为什么不抢 CPU

### 2.1 采样线程：绑定 core 0 + 降优先级（P0-1 / P1-16）

```rust
// app/main.rs — 采样线程入口
thread::spawn(move || {
    sysoverlay_win::msr::pin_sampling_thread();          // SetThreadAffinityMask(cur, 1) → core 0
    SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    ...
    loop { /* 分频调度 → sample → snap_store → 选择性 PostMessage → 睡到最近到期源 */ }
});
```

- **绑核 0 的原因**（`msr.rs` 注释）：RAPL 封装功耗寄存器是**每核域**，绑在同一核读才能保持语义一致、避免功耗毛刺；且一次性绑定后每帧零 `SetThreadAffinityMask` 系统调用、零强制迁移，不污染 `dt` 墙钟。
- **降优先级**：后台采集不与前台交互（鼠标/输入/动画）抢时间片。
- 效果：采样负载**集中堆在 CPU0**，与 UI 线程所在核解耦，互不阻塞。

### 2.2 分频调度：每个源按自己的最快更新频率采样（`scheduler.rs`）

| 源 | `MIN_EFFECTIVE_MS` | 原因 |
|----|-------------------|------|
| CPU / Mem | 100ms | `GetSystemTimes`/`GlobalMemoryStatusEx` 即时、廉价 |
| Net | 1000ms | `GetIfTable2` 字节计数器按 1s 粒度更新 |
| GPU | 2000ms | `D3DKMT` 单次查询实测 60–90ms，必须硬限 ≥2s |

- **Auto 档**：刷新上限 = 启用源的**最大** `MIN_EFFECTIVE_MS`（全开 = 2000ms），绝不空转。
- **手动档**：用户间隔生效，但 GPU/Net 仍各自不快于下限。
- **成本反馈**：每轮采样耗时占比 EMA；连续 2 轮 >8% 降一档（间隔 ×2，最多 3 档/5s），连续 10 轮 <2% 回升。自愈，不依赖手调。
- **结果**：采样线程大部分周期只做「已到期源」的增量查询，未到期源沿用上一帧（`SamplePlan` 标记 false）。

### 2.3 UI 线程：推式渲染，0% 空闲

```rust
// 采样线程侧：只有「屏幕内容真的变了」才唤醒 UI
let content_changed = last.as_ref().map_or(true, |p| displayed_changed(&snap, p));
if content_changed {
    PostMessageW(overlay_hwnd, WM_NEW_SNAPSHOT, ...);
}
// 睡到最近到期源，而非固定间隔
thread::sleep(Duration::from_millis(sleep_ms));
```

- UI 线程靠 `GetMessageW` **阻塞**，无事可做时 0% CPU；只在收到 `WM_NEW_SNAPSHOT` 才重绘。
- 重绘走脏矩形（`UpdateLayeredWindowIndirect` + `prcDirty`）、DIB 直写、字形图集 → 单帧 ~100–200µs，不整窗拷贝。

### 2.4 进程榜线程：独立分频，不污染悬浮条

- 进程列表是「打开设置面板才显示」的次要视图，跑在**自己**的线程上，2s 轮询（本轮 1s→2s）。
- 用 `PostMessageW(WM_NEW_SAMPLE)` 推给面板，与悬浮条采样循环完全解耦——**悬浮条稳态开销里没有进程扫描**。

### 2.5 GPU vendor / Power：一次性探测，热路径只做增量

- `primary_luid()` 用 `OnceLock` 缓存；`pick_dgpu` 整进程只跑一次。
- 之后每周期 GPU 采样仅对主适配器 LUID 精确匹配做 `D3DKMT RunningTime` 增量（~2ms），无逐引擎枚举风暴。
- Power（RAPL）驱动 `OnceLock` 一次性 `CreateServiceW`+`OpenDevice`，之后每周期 2 次 `DeviceIoControl` 读 MSR。

---

## 3. 跨线程数据交接（全程无锁）

| 数据 | 交接方式 | 说明 |
|------|---------|------|
| 采样快照 `Snapshot` | `ArcSwap<Snapshot>`（`snap_store` / `snap_get`） | 采样线程 `clone` 存入，UI 线程取 `Arc`；`D3` 用 `Arc::make_mut` COW，免每帧深拷贝 |
| 帧序号 `shared_seq` | `AtomicU64` | UI 线程判断是否已有新帧 |
| 显示/唤醒事件 | `resume channel`（`std::sync::mpsc`） | 覆盖层隐藏时采样线程 `recv` 挂起，托盘显示/系统唤醒时 `send` 复活（取代旧 1s 轮询可见性） |
| 设置改动 | `schedule_cfg()` 原子镜像（`publish_schedule_cfg`） | 改设置即时生效，无需锁、无需重启线程 |
| NIC 拓扑失效 | `NET_TOPO_INVALID: AtomicBool` | `NotifyIpInterfaceChange` 回调（IPHLPAPI 系统线程）只置原子，零分配零锁，避免回调死锁 |

> 这是多核真正生效的底座：**没有任何 mutex 横跨 UI 与采样线程的热路径**，两条线程靠 `ArcSwap` 一次性指针交换 + 原子标志协作，互不等待。

---

## 4. 为什么多核有效（实测对照）

- **采样线程绑核 0 + 降优先级** → 与 UI 线程所在核解耦，前台交互零卡顿、后台采集零争抢。
- **轮询 → 事件/分频**：Net 从「每 10s 全表 `GetIfTable2`（~28ms）」变为「事件驱动 + 60s 兜底」，稳态零轮询；GPU/Net 受 `MIN_EFFECTIVE_MS` 下限约束，不空转。
- **全表枚举 → 增量/事件驱动**：Net 每周期 `GetIfEntry2` ×~5（<0.5ms）；GPU 仅主适配器增量。
- **实测**：16 逻辑核机稳态整机 CPU
  - 优化前基线：0.14–0.21%
  - 第三轮后：~0.034%（PROBE 关）
  - 第四轮（事件驱动 NIC + 进程榜 2s，PROBE 开含日志开销）：0.0928%，仍低于基线；PROBE 关预计 ~0.03% 无回退
  - 测试 162 绿，0 warning，exe 412,672 B

---

## 5. 仍有空间（下一步可选，非必须）

| 方向 | 收益 | 风险/成本 |
|------|------|----------|
| ~~曲线窗口独立渲染线程~~ **✅ 已落地（第五轮，同日）** | 曲线高频绘制不占 UI 线程：1s 定时 + GDI 绘制移入 `sysoverlay-graph` 线程（自有消息泵），UI 线程 0 条周期消息；另加 `peek_seq` 脏检查，重绘 1s→数据变化驱动 | 实测线程 7→8、trail 无 0x0113 周期项、CPU 无回退 |
| 设置页重绘合并（已是增量，D5 已覆盖） | 边际 | 低 |
| GPU 若未来改 PDH 多引擎 | 可复用「事件驱动 + LUID 精确匹配」思路 | 当前单适配器增量已够，无必要 |
| 采样线程进一步压缩 | 已 `cycle cost_ms=0`，收益趋近于零 | 边际 |
| 进程榜窗口线程化（同曲线窗模式） | 仅面板打开期间每 2s 重绘占 UI 线程，使用占比低 | 收益小，暂不做 |

> 结论：本程序**已拿到多核并行的主要红利**——UI 与采样彻底分离（无锁 `ArcSwap` 交接）、采样绑核 0 降优先级、各源分频、Net/GPU 事件驱动增量、**曲线窗口独立渲染线程**。再往下挤压采样成本的边际收益极小，真正的多核价值已经在架构层面兑现。
