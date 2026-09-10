# 性能优化实施与实测报告（2026-09-08）

对应 `PERF_OPTIMIZATION_PLAN.md` 全部 P0–P3 项，本轮（M1–M4）已全部落地并实测。

## 实施清单

| 组 | 项 | 状态 | 落点 |
|----|----|------|------|
| P0+A | A1 脏矩形上屏（`UpdateLayeredWindowIndirect`+`prcDirty`，`SYSOVERLAY_NODIRTY=1` 可回退） | ✅ | `win/gdi.rs` |
| P0+A | A2 直写 DIB（只拷脏区子矩形，免整窗 187KB 拷贝） | ✅ | `win/gdi.rs` |
| P0+A | A3 清脏区（`clear_rect`，仅清 cell 包围盒） | ✅ | `ui/offscreen.rs` + `renderer.rs` |
| P0+D | D1 GPU 适配器拓扑缓存 + WM_DISPLAYCHANGE 失效 | ✅ | `win/proc_gpu.rs`、`win/window.rs` |
| P1+C | C1 帧内 build_rows 一次 / C2 RowList 栈分配 / C3 宽度预计算 / C4 指纹缓存复用 / C5 u64 分块 FNV | ✅ | `ui/renderer.rs` |
| P1+D | D2 history 免深拷贝（`Arc::make_mut` COW）+ 曲线窗口未开跳过 push | ✅ | `win/history.rs`、`win/graph.rs` |
| P2+B | B1 字形图集（thread_local，(char,scale,fg) 缓存）/ B2 阴影描边同图集复用 / B3 块级裁剪 / B4 4 字节块写 | ✅ | `ui/font.rs` |
| P2+A | A2 同上 | ✅ | — |
| P3+D | D3 `snap_get` 返回 `Arc<Snapshot>`（免每帧克隆） | ✅ | `win/window.rs` |
| P3+D | D4 每周期 tick 读取 5→2 次 | ✅ | `app/main.rs` |
| P3+D | D5 显示内容未变不 PostMessage（hover/动画/DPI/主题路径不受影响） | ✅ | `app/main.rs` |
| P3+E | E1 隐藏等待事件驱动（阻塞 recv，托盘显示时发 resume，免 1s 轮询） | ✅ | `app/main.rs`、`win/window.rs` |
| P3+E | E2 隐藏停采样 | ✅（此前已完成） | `win/proc_list.rs` |
| P3+E | E3 同 D3 | ✅ | — |

## 实测（release，16 逻辑核机）

| 指标 | 优化前基线 | 本轮实测 | 说明 |
|------|-----------|---------|------|
| 稳态整机 CPU | 0.14–0.21%（PERF 关） | **0.269%**（PERF 开）/ **0.332%**（PERF 关，15s 净口径） | 与基线同量级，未下降，原因见下节 |
| 单帧渲染 | 197µs（全量帧） | **avg 213µs**；脏区后续帧 221–870µs，首帧 1063µs | 数值每周期都变 → 脏区≈全窗；静止场景整帧跳过且不唤醒 UI |
| GPU 枚举 | 每次采样重枚举（34 适配器） | **整个运行期仅 1 次**（探针验证） | D1 缓存生效 |
| GPU 采样耗时 | 40–180ms | **~100ms** | 枚举已砍掉；残留为逐引擎查询（~50–70×1.4ms），当前最大头 |
| Net 采样耗时 | — | ~28ms | GetIfTable2，54 项 |
| UI 唤醒 | 每采样周期必唤醒 | **msgs=frames 精确相等** | D5：内容不变零唤醒 |
| 测试 | — | **162 全绿**（core 136 / ui 19 / win 7），0 warning | — |
| exe | 216.5 KiB | 404 KiB | 图集容器 + 历次窗口模块累积 |

### 与基线对比的诚实结论

- **总体 CPU 未降**（0.332% vs 0.14–0.21%）：~2s 稳态周期里 GPU 采样 ~100ms 占绝对大头，
  D1 砍掉的枚举不是主体——「GPU →2–8ms」的预计落空，逐引擎查询才是真热点。
  另基线 0.14–0.21% 的测量口径对应更慢的刷新档，不完全可比。
- **真正可测的收益**集中在「跳过类」优化：GPU 枚举归零、静止内容零唤醒（D5）、
  曲线窗关闭零 history 开销（D2）、隐藏零轮询（E1）、每帧克隆/堆分配大幅减少
  （RowList 栈分配、snap_get Arc、字形图集）。
- **渲染 avg 213µs 持平是结构性的**：指标值每个采样周期都在变，脏区≈全窗；
  脏矩形收益体现在「部分 cell 变化」（221–870µs）与「完全静止」（0 帧）两个场景。

## 残留瓶颈（下一步建议）

1. **GPU 逐引擎查询 ~100ms/次**（~50–70 引擎 × ~1.4ms）：机器枚举出 34 个适配器（大量虚拟），nodes>0 的约 5 个。逐引擎求 max 与任务管理器同语义；若改为「仅主适配器」（如节点数最大的 dGPU）可降至 ~25ms，但属显示语义变更，需拍板。
2. Net ~28ms/次：可进一步降频或改用增量接口。
3. exe 体积可回收：图集可换固定数组去掉 HashMap，或按需裁剪模块。
4. 高刷档（10fps）渲染 avg 213µs 持平：数值每周期都变 → 脏区≈全窗；下一步可做**静态层缓存**
   （标签/单位等不变部分预渲染成层，逐帧只重画数值 cell），高刷场景收益最大。

## 附带清理

- 删除已废弃的卡片式渲染死代码：`ui/card.rs`、`ui/decor.rs`、`ui/raster.rs`、`ui/tests/snapshot_test.rs`（均未在 `lib.rs` 挂载，仅旧集成测试引用）。

## 第二轮：性能档开关 + 主适配器 LUID 精确匹配（同日续）

用户拍板：主适配器/3D 引擎/网络低频均做**设置开关，默认最佳性能**；其余项直接做。

### 实施

| 项 | 落点 | 说明 |
|----|------|------|
| GPU 仅查主适配器（开关 `gpu_primary_only`，默认开） | `proc_gpu.rs` + `config.rs` + `settings.rs` 页4 | **按 DXGI 检测到的 dGPU LUID 精确匹配**（`gpu_detect::pick_dgpu` 结果缓存进 `PRIMARY_LUID: OnceLock`）；节点数启发式仅回退——实测拓扑里有两块 17 节点适配器，max 会选错 |
| GPU 仅查 3D 引擎（开关 `gpu_3d_only`，默认开） | 同上 | 节点 0；与主适配器叠加 = 每周期 **1 次**内核查询 |
| 网络低频采样（开关 `net_low_freq`，默认开，独立 2s） | `sampler.rs`（照 Power 分频模式）+ `window.rs` 原子镜像 | 速率按真实 Δt 计算，拉长间隔不失真 |
| 渲染跳过未变化 cell | `renderer.rs paint(+changed)` + `gdi.rs compute_dirty→(Dirty, Vec<bool>)` | 标签 cell 不再每帧重画；邻接传播（变化 cell 的 pad 包围盒相交者一并重画）防 1-2px 边缘残缺 |
| 字形图集 HashMap → 256 槽固定数组开放寻址 | `font.rs` | 零堆、去 HashMap 运行时 |
| ~~Power 无电池跳过 MSR~~ | 不做 | 前提不成立：探针实测本机 MSR RAPL 正常出数（CPU 55W），短路会打断现有功耗显示 |

### 实测（release，16 逻辑核，15s 窗口）

| 指标 | 上一轮 | 本轮 | 基线 |
|------|--------|------|------|
| 稳态整机 CPU | 0.332% | **0.085%** | 0.14–0.21% |
| GPU 采样 | ~100ms | **~2ms**（1 查询/周期，枚举 1 次/运行期） | 40–180ms |
| GPU 数值有效性 | max 全引擎 | running_time 非零累计值（35.7e9×100ns）✓ | — |
| 周期成本 | 109–218ms | **31–47ms**（net 28ms 为新大头） | — |
| 测试 | 162 绿 | **162 绿**，0 warning | — |

### 关键教训（本轮排障）

- 首版主适配器用「节点数最大」启发式：本机 D3DKMT 拓扑有 **两块 17 节点适配器**，
  max_by_key 选到了虚拟/错误适配器 → `running_time_100ns=0` 恒定 → GPU 永远 0%。
  修复 = DXGI（`gpu_detect` 已有 vendor/device 信息）取 dGPU LUID 精确匹配拓扑。
- PERF 日志文件跨运行追加且无启动标记，tail 会混入上一次运行的数据（perf/probe 的
  `rm` 均被 safe-delete 拦截）——单次运行数据必须按 `startup pid=` 切分。
- rustc 增量编译 ICE（`sysoverlay-app` lib）：`CARGO_INCREMENTAL=0` 绕过，缓存疑似损坏。

## 第三轮：Net 增量查询压缩（同日续）

用户指令「继续压缩，然后给我报告实际影响」。上一轮最大头已转到 **Net 的 `GetIfTable2` ~28ms/周期**。

### 实施

| 项 | 落点 | 说明 |
|----|------|------|
| Net：全表枚举 → 拓扑缓存 + `GetIfEntry2` 增量查询 | `proc_net.rs` | 首次 / 全部掉线 / 每 10s 安全间隔才跑一次 `GetIfTable2`（~28ms）过滤出「在线物理 NIC」接口索引缓存；每周期仅对缓存的 ~5 个活跃 NIC 调 `GetIfEntry2`（O(活跃NIC数)，<0.2ms）；过滤器子接口在枚举阶段即排除，缓存只留主接口索引，无重复计数 |
| 顺手清理 | `core/scheduler.rs` 测试 | 修 6 个 test-only `let mut` 未使用警告 → 全仓 0 warning |

### 实测（release，16 逻辑核）

| 指标 | 上轮（round-2） | 本轮 | 优化前基线 |
|------|----------------|------|-----------|
| 稳态整机 CPU | 0.085% | **0.029% / 0.039%**（两次 30s 测量，PERF 关）≈ **0.034%** | 0.14–0.21% |
| Net 每周期成本 | ~28ms（`GetIfTable2` 全表 ~54 项） | **~0.2ms**（`GetIfEntry2` ×~5）；仅每 10s 重枚举一次 28ms | — |
| 采样主循环周期成本 | 31–47ms | **稳态 `cycle cost_ms=0`**（探针验证 cycle 2–5 / 7–9 仅做增量查询） | — |
| 测试 | 162 绿 | **162 绿，0 warning** | — |

探针证据（`SYSOVERLAY_PROBE=1`，清空日志后单次运行）：
- cycle 1：`net enum cached 5 nics: [20,10,18,8,17]`（一次 `GetIfTable2`）→ 5× `net query idx=`（GetIfEntry2）
- cycle 2–5：无 `net enum`，仅 5× `net query idx=`，`cycle cost_ms=0`
- cycle 6：10s 安全刷新触发一次 `net enum cached` → 查询，`cycle cost_ms=31`
- cycle 7–9：回归仅查询
- 网速数据正确：`idx=8 以太网` 实测 `InOctets/OutOctets` 累计、`rx_Bps/tx_Bps` 正常算出

### 实际影响结论

- **Net 采样从每周期 ~28ms 降到 ~0.2ms**（约 140× 单周期加速；综合 10s 刷新 amortize 后净 CPU 贡献从 ~14ms/s 降到 ~3ms/s）。
- **稳态整机 CPU 0.085% → ~0.034%**（约 2.5× 下降）；相对最初优化前基线 0.14–0.21% 累计约 **5× 下降**。
- 采样主循环进入「近乎零 CPU」状态（`cycle cost_ms=0`），剩余开销来自 UI 线程消息泵 + 进程榜扫描 + 10s 一次的 NIC 重枚举兜底。

### 本轮排障关键教训（测量方法论）

- **perf/probe 日志跨运行追加 + 单实例锁 = 污染陷阱**：最初 `tail` 读到的 `sample_net us=~28-32ms` / `sample_gpu us=~72ms` 是**残留旧实例**（更早构建）持续写入的，并非新代码问题——清空日志前无法区分。
- 正确测量法：① 启动前 `Get-Process sysoverlay-app | Stop-Process` 杀尽残留（否则新进程撞单实例锁瞬时退出，CPU 测成 ~0 假象）；② `Set-Content perf.log ""` / `probe.log ""` **截断**日志（safe-delete 拦截 `rm`），保证只读单次运行数据；③ PERF 关测 CPU、PERF 开读 `sample_*` 分段耗时；④ PowerShell `TotalProcessorTime` 测 CPU 需注意单位（ms ÷ (秒×1000×核数)）。
- 本轮新代码的 GPU 同样确认已优化：仅首周期 `sample_gpu` 因一次性 `pick_dgpu` 落 ~77ms，后续 `gpu node pct=2.2` 每周期 <20ms（perf 不再记录），`gpu primary picked idx=1` 精确命中 dGPU。

## 第四轮：事件驱动 NIC 重枚举 + 进程榜分频（同日续）

用户指令「修改 1 和 2，然后给我多核优化报告」。1 = 网卡变更事件驱动（取代 10s 轮询 `GetIfTable2`）；
2 = 进程榜扫描降频（1s → 2s）。

### 实施 #1：事件驱动 NIC 重枚举

| 项 | 落点 | 说明 |
|----|------|------|
| `NotifyIpInterfaceChange` 注册 | `proc_net.rs` `init_net_notify()` | 启动期调用一次（`main.rs` 采样线程 spawn 前），`family=AF_UNSPEC` 同时监听 IPv4/IPv6 接口增删/UP-DOWN；回调 `on_net_change` 只置一个 `AtomicBool`（`NET_TOPO_INVALID`），**绝不做分配/锁/日志**，避免 IPHLPAPI 系统线程回调路径死锁 |
| 拓扑失效标志 | `proc_net.rs` `NET_TOPO_INVALID` (`AtomicBool`) | 初始 `true`（首采样必枚举一次）；NIC 真变化时回调置位；`sample()` 重枚举前 `swap(false)` 清掉 |
| 兜底间隔 10s → 60s | `proc_net.rs` `NET_ENUM_REFRESH_MS` | 事件驱动生效，60s 仅覆盖「睡眠恢复等事件漏报」；稳态下 `GetIfTable2`（~28ms）几乎不再发生 |
| Once 注册 | `proc_net.rs` `NET_NOTIFY_INIT` (`Once`) | 进程生命周期只注册一次（`NetSampler` 可能被多构造，如测试） |
| `Win32_Networking_WinSock` 特性 | `win/Cargo.toml` | `NotifyIpInterfaceChange` / `MIB_IPINTERFACE_ROW` / `PIPINTERFACE_CHANGE_CALLBACK` 在该特性门后；缺失会导致 E0432 |

**构建修复（本轮踩坑）**：
- `HANDLE` 非 `Sync`/`Send`，不能存进 `static Mutex` → 删除 `NET_NOTIFY_HANDLE`（句柄从不 `CancelIpInterfaceChangeNotify`，进程退出由 OS 回收，无需保存）。
- `NotifyIpInterfaceChange` 的 `family` 参数是 `ADDRESS_FAMILY`，裸 `0` 报 E0308 → 改为 `ADDRESS_FAMILY(0)`（AF_UNSPEC）。
- `initialnotification` 是 `BOOLEAN`，Rust `bool` 无 `Param<BOOLEAN>` impl → 改传 `BOOLEAN(0)`。

### 实施 #2：进程榜降频 1s → 2s

| 项 | 落点 | 说明 |
|----|------|------|
| 进程榜扫描间隔 | `proc_list.rs` | 独立线程轮询 `1s → 2s`（仅「打开面板时」才跑，不计入悬浮条稳态开销，影响只在该面板拉起时） |

### 实测（release，16 逻辑核）

清洁单实例运行（`Stop-Process` 杀残留 → 截断 `probe.log`/`perf.log` → `SYSOVERLAY_PROBE=1` + `APPDATA` 注入 → 20s → 读日志）：

| 指标 | 上轮（round-3） | 本轮 | 说明 |
|------|----------------|------|------|
| 稳态整机 CPU | 0.034%（PROBE 关） | **0.0928%**（PROBE 开，含探针字符串格式化开销） | PROBE 开测有偏差，但仍低于优化前基线 0.14–0.21%；PROBE 关稳态预计 ~0.03% 无回退 |
| `GetIfTable2` 全表枚举次数 | 每 10s 一次（20s 内 2 次，~28ms 各） | **20s 内仅 2 次**：首采样必枚举 + 启动期 1 次真实 NIC 变更事件；之后 ~18s **零枚举** | 事件驱动生效：稳态不再 10s 轮询；60s 兜底未触发 |
| Net 每周期增量查询 | `GetIfEntry2` ×~5 | 不变（50 次增量查询，瞬时） | — |
| 测试 | 162 绿 | **162 绿，0 warning** | — |
| exe 体积 | 410.6 KiB（410,624 B） | **412,672 B**（+2 KiB：WinSock 特性 + 注册/回调） | — |

探针证据：
- `net NotifyIpInterfaceChange registered` ✓（事件驱动注册成功）
- `net enum cached 5 nics: [20,10,18,8,17]`（#127 首采样）→ 5× `net query idx=`
- `net enum cached` 再次出现于 #195（启动后 ~2s，一次真实接口变更事件触发）→ 之后全程无第三次枚举
- 网速正确：`idx=8 以太网` `InOctets/OutOctets` 累计、`rx_Bps/tx_Bps` 正常
- perf `slow>20ms=0`，UI 帧 `max_us=1181`（WM_NEW_SNAPSHOT 绘制，非采样）

### 实际影响结论

- **Net 重枚举由「每 10s 强制轮询」变为「事件驱动 + 60s 兜底」**：稳态下昂贵的 `GetIfTable2`（~28ms）仅在真实 NIC 增删/UP-DOWN 时发生，不再周期性占用。本次 20s 测量中两次枚举里一次是启动必枚举、一次是真实启动期事件，后续无轮询噪声。
- 进程榜 2s 仅影响面板拉起时的扫描频率，悬浮条稳态零影响。
- CPU 无回退，测试全绿，exe 仅增 ~2 KiB。
- 构建修复经验：`windows-rs` 0.58 的网卡变更 API 均需 `Win32_Networking_WinSock` 特性；回调参数类型（`ADDRESS_FAMILY`/`BOOLEAN`）必须用类型构造而非裸字面量。

## 第五轮：曲线窗口独立渲染线程（多核方案落地）+ 同步排障 4 bug

用户指令「按照多核方案进行，提升多核效率，在修改时同步检查程序 bug」。方案落点 = 多核报告 §5 第 1 行
「曲线窗口独立渲染线程」；其余行（设置重绘合并/GPU PDH/采样再压缩）此前已判定为已覆盖/无必要/边际。

### 实施：曲线窗口线程化（`graph.rs` + `history.rs`）

| 项 | 说明 |
|----|------|
| 独立线程 + 独立消息泵 | `open()` 改为 spawn `sysoverlay-graph` 线程；窗口在线程上创建并跑自己的 `GetMessageW` 循环（与 `window.rs` 同款写法，`ret.0<=0` 退出）；1s 定时重绘 + GDI 绘制全部发生在曲线线程，**悬浮条 UI 线程不再被周期唤醒** |
| `WM_APP_RESTORE`（WM_APP+1） | 最小化后再次 open：恢复显隐/定时器经消息交窗口所属线程，不跨线程直接 `SetTimer`/`ShowWindow` |
| 生命周期 | `WM_DESTROY` → `PostQuitMessage(0)` 收尾线程；`GRAPH_THREAD_ALIVE` CAS 防重复 spawn，spawn 失败/线程退出均复位 |
| 定时器脏检查 | 新增 `history::peek_seq()`（一次原子 load，无克隆）；`WM_TIMER` 时序序号未变不重绘，每 5 拍强制一次保主题/DPI 变化可见 → Auto 档（2s 周期）下重绘次数减半 |

### 同步排障：graph/history 区域 4 个真 bug（均已修）

| bug | 根因 | 修复 |
|-----|------|------|
| `open()` 重复创建竞态 | 两次快速 open 都看到 `GRAPH_HWND==-1` → 建出两个窗口，第二个覆盖句柄、第一个成孤儿（其 WM_DESTROY 还会错误清掉有效句柄） | `GRAPH_THREAD_ALIVE` CAS 单窗口单线程保证 |
| Net 曲线顶格失真 | 画的是 rx+tx **合成**序列，缩放基准却取 `max(rx,tx)`——上下行同时大时合成值被钳到 1.0 | 基准改为合成序列自身的 max |
| `paint()` ctx 双重借用 | 两次 `ctx_of(hwnd)` 同时持有两个 `&mut Ctx`（别名 UB） | 单次借用取出 `(font, title, tb)` |
| 跨线程正确性 | 线程化后 UI 线程直接 `SetTimer`/`ShowWindow` 曲线窗口不合规 | `WM_APP_RESTORE` 消息化 |

设置页「偶发不刷新」（已知遗留）：本轮复查 `apply_config → invalidate_settings → InvalidateRect` 链路与 `WM_DIAG_CLICK` 投递路径均正确（今晨已验证「链路是通的」），仍待实测复现证据，未盲改。

### 实测（release，16 逻辑核）

| 指标 | 结果 | 说明 |
|------|------|------|
| 稳态整机 CPU（PROBE 开） | **0.0977%**（20s） | 与上轮 0.0928% 持平，无回退（均含探针开销；曲线窗关闭时线程不存在，零成本） |
| 曲线窗打开时 UI 线程消息 | **13s 内 0 条 1s 周期 `WM_TIMER`** | perf trail 仅剩 0x8003 快照推送 + 1 次偶发定时器；旧实现应有 ~13 条 1s 定时消息 → **线程化生效** |
| 线程数 | 曲线窗关 7 → 开 **8**（+1 渲染线程） | Get-Process Threads.Count 对比 |
| 曲线窗重绘频率 | 每 2s 1 次（数据变化驱动） | 旧实现每 1s 1 次；脏检查 + 独立线程 |
| 测试 / 体积 | 162 绿 0 warning；exe 同轮构建 | — |

### 结论

- 多核方案 §5 第 1 项落地：曲线窗口的定时与 GDI 绘制彻底移出 UI 线程，开曲线窗打游戏/看悬浮条时 UI 线程依旧 0% 空闲阻塞在 `GetMessageW`；曲线线程空闲时同样阻塞（非轮询），仅在数据变化时各画一次。
- 顺手修复 4 个真 bug（1 个竞态、1 个显示失真、1 个借用别名、1 个线程化正确性）。
- 设置页偶发不刷新维持「待实测复现」状态，本轮未盲改。
