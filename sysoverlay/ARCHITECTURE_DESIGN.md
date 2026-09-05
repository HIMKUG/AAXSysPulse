# SysOverlay-C · 模块架构设计文档

> **定位**：本文件是 SysOverlay-C Rust 重构的**架构单一事实源**（architecture source of truth）。
> 它描述"应当如何分层、模块各自负责什么、对外暴露哪些接口、依赖方向如何"。
> 与 `REPORT.md` 不同，本文件在「§7 实现状态矩阵」中**如实标注每个模块是「已实现 / 占位 / 待修」**，
> 不允许把目标当现状。
> 审查中发现的具体缺陷见 `ARCHITECTURE_REVIEW.md`。
>
> 范围：`D:\code\sys-overlay-c-master\sysoverlay`（Cargo workspace，4 crate）。
> 目标平台：Windows 10/11 x64，零运行时依赖（仅静态链接 `windows` crate）。

---

## 1. 设计目标与约束

| 目标 | 说明 |
|---|---|
| 轻量级 | 对标原 C 版 ~75KB / 内存 ~7MB；Rust 重构后 exe ≤ 120KB、私有内存 ≤ 2MB（**目标值，待实测回填**） |
| 分层隔离 | `core` 零平台依赖、零 `unsafe`；`ui` 无 Win32 调用；`win` 是**唯一**允许 `unsafe` 的 crate |
| 可测试 | 所有纯逻辑（指标计算、布局、配置解析）必须可在非 Windows 环境离线单测 |
| 主线程不阻塞 | UI 线程只画图 + 处理消息；采集走独立线程，慢了就丢帧，绝不卡 UI |
| 自愈 | 长驻悬浮窗需容忍睡眠唤醒 / 显示器变更 / 锁屏 / 网卡热插拔 / GPU TDR |

**硬约束**：`core` 与 `ui` 不得 `use windows::...`；`unsafe` 仅出现在 `win`。

---

## 2. 架构总览

```
┌──────────────────────────────────────────────────────────────┐
│  app  (可执行入口)                                             │
│   · main.rs：采样线程 spawn + ArcSwap 持有 + 单实例 + panic 兜底 │
│   · 只做编排，不含业务逻辑                                      │
└───────────────┬──────────────────────────────────────────────┘
                │ 持有 ArcSwap<Snapshot>，读 cfg / layout
        ┌───────┴────────┐
        ▼                ▼
┌──────────────────┐  ┌──────────────────────────────────────────┐
│  win  (平台层)    │  │  ui  (渲染，无 Win32)                       │
│  window.rs 消息循环│  │  render.rs  RenderTarget trait             │
│  gdi.rs   →UpdateLayeredWindow │  offscreen.rs SoftwareSurface     │
│  proc_*.rs 采样器 │  │  card.rs  draw_card                        │
│  menu/dpi/config_io │  │  font.rs  8x16 点阵                      │
│  （唯一 unsafe）   │  └──────────────────────────────────────────┘
└────────┬─────────┘
         │ (proc_* 实现 core::Sampler)
         ▼
┌──────────────────────────────────────────────────────────────┐
│  core  (纯逻辑，零依赖，100% 单测)                              │
│  snapshot · sampler · metrics · layout · config · (color/units)│
└──────────────────────────────────────────────────────────────┘
```

**依赖方向（单向，禁止反向）**：
```
app ──▶ win ──▶ core
app ──▶ ui  ──▶ core
win  ──▶ core      (win 实现 core 定义的 trait)
ui   ──▶ core
core 不依赖任何上层
```
> 允许 `ui` 被 `win` 调用（win/gdi 把 `ui::draw_card` 的结果贴上屏），但 `ui` 永不反向依赖 `win`。

---

## 3. 模块职责与对外接口

### 3.1 `core` —— 纯逻辑层（零依赖 / 零 unsafe）

| 模块 | 职责 | 关键对外类型 / 接口 | 状态 |
|---|---|---|---|
| `snapshot` | 一次采样周期的不可变结果 | `Snapshot{seq,timestamp_ms,cpu:Metric<f32>,memory:MemoryStat,gpu:Metric<f32>,net:NetStat}`；`Metric<T>{Value/Stale{value,stale_for}/Unavailable}`；`MemoryStat`；`NetStat` | ✅ 已实现 |
| `sampler` | 可插拔采集器 trait | `trait Sampler: Send { name()->&str; sample(prev,now_ms)->SampleOutcome }`；`enum SampleOutcome{Ok,Transient(&str),Fatal(&str)}` | ✅ 已实现 |
| `metrics` | 确定性指标计算 | `cpu_usage(idle,kernel,user)->Metric<f32>`（**待修公式**）；`rate_per_sec(i64,u64)->Option<u64>`；`ema(f32,f32,f32)->f32`；`node_key(&str)->Option<&str>`；`aggregate_gpu_nodes(iter)->Option<f32>`（**待修：死代码+bug**） | ✅ 已实现(待修) |
| `layout` | 卡片布局（DPI/zoom/shape 感知） | `compute(minimal,vert,zoom,dpi,shape)->CardLayout`；`CardLayout{width,height,rows:Vec<RowLayout>}`；`enum Zoom/Shape/VertLayout`（**应与 config 合并为唯一真源**） | ✅ 已实现 |
| `config` | 配置解析/序列化/校验（**宽容读 + 保守写**，见下） | `Config{...}`；`Config::parse(&str)->Config`；`Config::serialize()->String`；`Rgb{from_hex,to_hex,from_colorref,luminance}`；`enum NetUnit/Shape/VertLayout/Zoom/RenderMode/PowerDisplay`；`CONFIG_VERSION=2`；`INTERVAL_MS_ALLOWED` | ✅ **已修复**(2026-09-04) |
| `color` | 调色板 / 强调色派生 | `Palette`；`accent_for(metric)->Rgb`（**目标接口，当前空文件**） | ⬜ 占位 |
| `units` | 数值格式化 | `format_rate(u64)->String`；`format_pct(f32)->String`；`format_uptime(u64)->String`（**目标接口，当前空文件**） | ⬜ 占位 |

**`core` 设计要点**
- `Metric<T>` 的 `Stale` 变体是给 UI 的"陈旧提示"信号：某采样连续失败 N 次时，UI 仍显示旧值但应标灰/加 `~`（见 §6）。
- `cpu_usage` **正确公式**（当前代码算错，见 REVIEW §2.2a）：
  ```rust
  // GetSystemTimes 的 kernel 时间已包含 idle 时间
  let total = kernel_delta.saturating_add(user_delta);
  let busy  = total.saturating_sub(idle_delta);
  let usage = (busy as f64 / total as f64 * 100.0).clamp(0.0,100.0) as f32;
  ```
- `aggregate_gpu_nodes` 应改为 `if let Some(key) = node_key(name)`，单个非法实例名只跳过不整聚合失败；`proc_gpu` 必须复用它，删除 win 内的重复实现。
- **`config` 格式策略：宽容读 + 保守写**（2026-09-04 定案，详见 v2 §6.1 勘误）：
  - **写（serialize）**：严格 C 版格式 —— 空格分隔、`shape`/`vert`/`netunit` 输出**数值**、`bg`/`fg` 输出 `%06X` **COLORREF(BBGGRR)**、`interval` 输出 **100ms 单位**。保证降级回 C 版不丢配置。
  - **读（parse）**：空格分隔与 `key=value` **都收**；枚举**数值/字符串双接受**；颜色按 `from_colorref` 解析；`interval` 读入后 ×100 换算为 ms 并吸附到 `INTERVAL_MS_ALLOWED`（C 版 `INTERVAL_DDJ={5,10,20}` 扩展为 `{5,10,20,30,50}`×100ms）。
  - **未知键静默忽略**（与 C 版行为一致，已实测 C 版不会崩）；**任何字段解析失败保留原值**，永不 panic。
  - v2 新增字段（`version`/`render_mode`/`power*`/`exclude_virtual_nic`）直接写空格分隔行 —— 已核验与 C 的 12 个 `sscanf` 模式无碰撞。

### 3.2 `ui` —— 渲染层（无 Win32）

| 模块 | 职责 | 关键对外类型 / 接口 | 状态 |
|---|---|---|---|
| `render` | 渲染抽象 | `trait RenderTarget { fill_round_rect; fill_rect; draw_text; measure_text }`；`Rect`；`Rgba`(预乘alpha)；`FontSpec` | ✅ 已实现 |
| `offscreen` | 软件光栅（BGRA 预乘，对齐 DIB） | `SoftwareSurface{width,height,pixels:Vec<u8>}`；`to_png()` 对拍 | ✅ 已实现(含单测) |
| `card` | 卡片绘制（镜像 C 版 drawCard） | `draw_card(&mut Surface, &Snapshot, &Config, &CardLayout, alpha:f32)`；`demo_snapshot()`；`Palette` | ✅ 已实现 |
| `font` | 8×16 点阵字体 | `render_char(...)`；`FONT_WIDTH/HEIGHT/SPACING` | ✅ 已实现 |

**`ui` 设计要点**
- `SoftwareSurface.pixels` 的 BGRA 预乘布局与 Windows `BITMAPINFO` DIB 完全一致 →
  `win/gdi` 可直接把这块 buffer 当 `CreateDIBSection` 的 bits 上屏，**零拷贝转换**（仅 PNG 导出时转 RGBA）。
- `draw_card` 不感知窗口、不感知 DPI 原始值，只吃 `CardLayout`（DPI/zoom 已在 layout 阶段折算成像素）→ 渲染与平台解耦，便于单测与对拍。

### 3.3 `win` —— 平台层（唯一 unsafe）

| 模块 | 职责 | 关键对外接口 | 状态 |
|---|---|---|---|
| `window` | 分层窗口 + 消息循环 + 拖拽 + 命中测试 | `OverlayWindow::new(Config)`；`run_message_loop()`；`set_snapshot/get_snapshot` | ✅ 骨架(待完善) |
| `gdi` | 渲染管线：软光栅→DIB→`UpdateLayeredWindow` | `render(hwnd,&Snapshot,&Config)`；目标 `GdiResources`{base_dc,base_bmp,base_key,work_dc} 蒙版缓存 | 🟠 占位(画渐变忽略快照) |
| `proc_cpu` | `GetSystemTimes` 采集 | `impl Sampler for CpuSampler` | ✅ 真实现 |
| `proc_gpu` | PDH GPU Engine 采集 | `impl Sampler for GpuSampler` | ✅ 真实现 |
| `proc_mem` | 内存采集 | `impl Sampler for MemSampler`（目标 `GlobalMemoryStatusEx`） | ⬜ 占位(写死) |
| `proc_net` | 网络采集 | `impl Sampler for NetSampler`（目标 `GetIfTable2`/PDH） | ⬜ 占位(写死0) |
| `sampler` | 组合采样器 | `MockSampler::collect()->Snapshot`（**待改：当前写死 cpu=50/gpu=Unavailable，应真正组合**） | 🟠 占位残留 |
| `menu` | 右键菜单 | `show_menu(hwnd)->Option<u16>`；`handle_command(u16)`（目标 `TrackPopupMenu`） | ⬜ 占位 |
| `dpi` | DPI 感知 | `apply_dpi_awareness()`（目标 `SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2)`） | ⬜ 占位 |
| `config_io` | 配置持久化 | `load_config()/save_config()`（目标原子写 `MoveFileExW` 到 `%APPDATA%\SysOverlayC\config.txt`） | ⬜ 占位 |

**`win` 设计要点**
- `gdi::render` 是 `ui` 与屏幕之间的**唯一接缝**：
  ```
  let mut s = SoftwareSurface::new(layout.width, layout.height);
  draw_card(&mut s, &snap, &cfg, &layout, hover_alpha);
  // s.pixels (BGRA 预乘) → CreateDIBSection bits → UpdateLayeredWindow
  ```
- 蒙版缓存（`GdiResources`）：底图按 `CacheKey{尺寸,DPI,shape,颜色,缩放}` 缓存，
  指纹不变则 `BitBlt` 复用，仅动态部分（数值文字/进度条）每帧重绘 —— 避免每帧全像素重算。
- `OverlayWindow` 状态应通过 `GWLP_USERDATA` 装箱持有（而非全局 `WINDOW_STATE` Mutex）；
  当前 `Box::into_raw(Box::new(()))` 是无用泄漏，应改为真正的 `WindowState`。

### 3.4 `app` —— 编排层（入口）

| 模块 | 职责 | 关键对外 | 状态 |
|---|---|---|---|
| `main` | 启动：加载配置 → 建窗 → 起采样线程 → 跑消息循环 → 单实例 → panic 兜底 | `fn main()` | 🟠 骨架(仅建窗+循环) |

**`app` 设计要点（目标，当前未实现）**
```rust
fn main() {
    // 1. 单实例互斥 CreateMutexW("SysOverlayC-SingleInstance")，已存在则静默退出
    // 2. panic hook → MiniDumpWriteDump 到 %TEMP%\SysOverlayC_crash.dmp
    // 3. let snap = ArcSwap::from_poisonable(Arc::new(placeholder_snapshot()));
    // 4. thread::spawn: 循环 { sampler.collect(prev, now) → snap.store(Arc::new(s)); sleep(interval) }
    // 5. window.run_message_loop(|hwnd| { let s = snap.load(); gdi::render(hwnd, &s, &cfg); })
}
```
- 选 `ArcSwap` 而非 channel：UI 线程是 `GetMessage` 驱动，channel 会破坏消息泵；`ArcSwap::load()` 零等待零拷贝拿最新快照。

---

## 4. 关键抽象与数据流

```
采集线程 (每 interval_ms)                UI 线程 (消息循环 / WM_TIMER)
┌─────────────────────────┐              ┌──────────────────────────────────────┐
│ CpuSampler/GpuSampler/   │  store      │ ArcSwap::load() → Snapshot           │
│ MemSampler/NetSampler    │ ──────────▶ │ → Config + CardLayout(compute)       │
│   .sample(prev, now)     │  (ArcSwap)  │ → ui::draw_card → SoftwareSurface    │
│ 产出 Snapshot            │              │ → win::gdi::render → UpdateLayeredWindow │
└─────────────────────────┘              └──────────────────────────────────────┘
        ↑ Sampler trait 由 core 定义，win 实现
```

- **快照不可变 + 廉价 clone**：`Snapshot` 全是标量，`Clone` 为浅拷贝；`ArcSwap` 持有 `Arc<Snapshot>`，
  UI 线程 `load()` 拿到 `Arc` 引用，不拷贝像素、不抢锁。
- **陈旧传播**：采样失败 → `SampleOutcome::Transient` → 沿用上一帧；连续失败 3 次 → 指数退避重建
  （1s→2s→4s→30s 封顶）。UI 据 `Metric::Stale{stale_for}` 显示陈旧提示。

---

## 5. 线程与并发模型

| 线程 | 职责 | 阻塞容忍 | 同步机制 |
|---|---|---|---|
| UI 线程 | 消息循环 / 绘制 / 拖拽 / 菜单 | 绝不阻塞 | `GetMessageW` 泵；绘制前 `ArcSwap::load()` |
| 采集线程 | 四个 Sampler 依次采样 | 可慢，慢则丢帧 | `ArcSwap::store(Arc::new(snapshot))` |

- `Sampler: Send` 保证采集器可移入采集线程。
- 自愈事件来自系统消息（`WM_POWERBROADCAST`/`WM_DISPLAYCHANGE`/`WM_WTSSESSION_CHANGE`），
  在 UI 线程 `wndproc` 内处理（重置差分基准 / 重新枚举工作区 / 暂停采集）。

---

## 6. 依赖与编译边界

- workspace：`members = [core, ui, win, app]`，`resolver = "2"`。
- `core` 无 `[dependencies]` → 可纯离线编译与单测（已验证 `cargo test -p sysoverlay-core --offline` 通过）。
- `win`/`app` 依赖 `windows` crate（目标：升级到含 `GlobalMemoryStatusEx`/`GetIfTable2` 绑定的版本，解决 proc_mem/proc_net 占位）。
- Release profile：`opt-level="z"` + `lto` + `codegen-units=1` + `panic="abort"` + `strip` + 关 `overflow-checks`（体积优先）。
- **建议**（可选）：为 `core` 的纯逻辑单测加 `cfg(test)` 之外不引入 dev-dep 干扰；`ui` 的 `png` 仅作 dev/test 依赖（已在 `ui/Cargo.toml` 的 `[dev-dependencies]`）。

---

## 7. 实现状态矩阵（如实标注，禁止目标当现状）

| 模块/能力 | 状态 | 备注 |
|---|---|---|
| core: snapshot / sampler / metrics / layout / config | ✅ 已实现 | metrics/layout/config 有**待修缺陷**（见 REVIEW） |
| core: color / units | ⬜ 占位 | 空文件，仅有目标接口签名 |
| ui: render / offscreen / card / font | ✅ 已实现 | 含单测 + 对拍 PNG 生成 |
| win: proc_cpu / proc_gpu | ✅ 真实现 | 真实 API 采集 |
| win: window | 🟠 骨架 | 全局单例 + 占位 USERDATA；无采样驱动重绘 |
| win: gdi::render | ⬜ 占位 | 画写死渐变，忽略 Snapshot |
| win: proc_mem / proc_net | ⬜ 占位 | 写死假数据 |
| win: sampler(MockSampler) | 🟠 残留 | 写死 cpu=50/gpu=Unavailable，丢弃真实结果 |
| win: menu / dpi / config_io | ⬜ 占位 | 仅注释 |
| app: 采样线程 + ArcSwap | ⬜ 未实现 | 仅建窗+消息循环 |
| app: 单实例 / panic minidump | ⬜ 未实现 | — |
| 蒙版缓存 GdiResources / BitBlt | ⬜ 未实现 | — |
| 自愈矩阵 | ⬜ 未实现 | — |
| 配置原子落盘 | ⬜ 未实现 | — |

---

## 8. 演进路线（优先级）

- **P0（纯修 bug，可立即合）**
  1. 修 `cpu_usage` 公式 + 同步改写 `test_cpu_usage_*`。
  2. ~~统一 `Config` 序列化（`key=value` + `to_hex()`）。~~ → **已于 2026-09-04 完成**：方向修正为「宽容读 + 保守写」兼容 C 版（详见 §3.1 config 行）。
  3. `proc_gpu` 复用 `core::aggregate_gpu_nodes`，删 win 内重复实现；`?` 改 `if let Some`。
- **P1（端到端跑通，最小可用闭环）**
  4. `win/gdi::render` 接 `ui::draw_card`（SoftwareSurface → DIB → UpdateLayeredWindow）。
  5. `app` 起采样线程 + `ArcSwap<Snapshot>`，UI 线程 `load()` 重绘。
  6. `config_io` 原子写落盘 `%APPDATA%\SysOverlayC\config.txt`。
- **P2（健壮 / 可维护）**
  7. 去全局 `WINDOW_STATE`：状态装箱进 `GWLP_USERDATA`。
  8. 合并 `config.rs`/`layout.rs` 的 `Zoom/Shape/VertLayout` 为唯一真源。
  9. 落实自愈矩阵（PBT_APMRESUME / WM_DISPLAYCHANGE / WM_WTSSESSION_CHANGE）。
  10. DPI：`SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2)`。
  11. 实现 `GdiResources` 蒙版缓存；`Metric::Stale` 接 UI 标灰。
  12. 测试门禁：对拍 PNG 像素差 < 阈值；`proc_*` 加 mock 集成测试。

> 最小可用闭环 = **P0 + P1-4 + P1-5**。其余为锦上添花。

---

## 9. 接口契约速查（给实现者）

```rust
// core/sampler.rs
pub trait Sampler: Send {
    fn name(&self) -> &'static str;
    fn sample(&mut self, prev: Option<&Snapshot>, now_ms: u64) -> SampleOutcome;
}

// core/snapshot.rs
pub enum Metric<T> { Value(T), Stale { value: T, stale_for: u32 }, Unavailable }

// ui/render.rs
pub trait RenderTarget {
    fn fill_round_rect(&mut self, r: Rect, radius: i32, color: Rgba);
    fn fill_rect(&mut self, r: Rect, color: Rgba);
    fn draw_text(&mut self, s: &str, at: Rect, color: Rgba, font: FontSpec);
    fn measure_text(&self, s: &str, font: FontSpec) -> (i32, i32);
}

// ui/card.rs
pub fn draw_card<S: RenderTarget>(s: &mut S, snap: &Snapshot, cfg: &Config, layout: &CardLayout, alpha: f32);

// win/gdi.rs  (目标签名)
pub fn render(hwnd: HWND, snap: &Snapshot, cfg: &Config); // 内部: ui::draw_card → SoftwareSurface → DIB → UpdateLayeredWindow
```

---

## 10. v2 决策记录（2026-09-04 定案）

v2 方案（`SysOverlay-C_优化设计方案_v2.md`）经 16 问核对后定案，关键决策摘录于此，
使本文件保持架构单一事实源。

| 议题 | 决策 |
|---|---|
| **配置格式** | 宽容读 + 保守写（见 §3.1）。C 版是空格分隔，不得改 `key=value`。 |
| **排期** | 修正后 **19.5 天**（原 14.5 天低估）。P0+P1 按 **一周** 排（含补 `color.rs`/`units.rs` 空文件与 `proc_mem`/`proc_net` 占位）。 |
| **驱动边界** | 主程序**永不依赖** PawnIO。功耗做成可选附加组件：`sysoverlay-power-setup.exe` **只装驱动、不含业务逻辑**，主程序直接调驱动。驱动不可用 → 回退**估算模型**（非隐藏、非"不可用"），UI 用 `~` 前缀标注。 |
| **`SampleOutcome`** | 本次重构就改：`Ok/Transient/Skipped/Fatal`（去 `&str` payload）+ 独立 `SamplerDiag`。 |
| **诊断落点** | 内存固定环形缓冲（64 项 ≈ 1.5 KB，无堆分配、无磁盘 I/O）；三出口：默认仅内存 / 崩溃随 MiniDump / `--diag` 打印到 stdout。 |
| **双渲染模式** | **删掉 `CardCompositor`**（过度抽象）。`draw_card` 保持唯一骨架，差异经**两个注入点**注入：`&dyn TextRasterizer`（字体）+ `&dyn Decoration`（装饰：`EcoDecoration`/`QualityDecoration`）。 |
| **枚举归属** | 新开 **`core::types`**（`Zoom/Shape/VertLayout/NetUnit/RenderMode/PowerDisplay`），`config` 与 `layout` 共同依赖，避免 `layout → config` 层内依赖。 |
| **`compute` 签名** | 吃**窄参数** `&LayoutInput{zoom,shape,vert,minimal,show_power,dpi,metrics}`，不吃 `&Config`（好测 + 让"布局依赖哪些配置"显式化）。 |
| **时序模型** | 采样线程自睡眠（`WaitForSingleObject`+timeout，**可打断**）→ `store(ArcSwap)` → `PostMessage(WM_APP_SNAPSHOT_READY)`；`CreateTimerQueueTimer` **只服务 60fps 悬停**，alpha 到位后主动取消。悬停只改 `BLENDFUNCTION`，不碰像素。 |
| **功耗间隔** | `power_interval_ms` 是**绝对值**（默认 3000），生效值 = `max(power_interval_ms, interval_ms)`；"3 倍"说法已删。 |
| **Eco 缩放档位** | Eco 只提供 **100% / 200%** 两档（`Zoom75` 移除）；手动/C 版带入的 75% clamp 到 100%，不报错。Quality 保留完整 5 档。 |
| **GPU 数据源** | 主源换 `D3DKMTQueryStatistics`；**PDH 整体移除**（含 `node_key`/`aggregate_gpu_nodes`）。D3DKMT 覆盖 WDDM 2.0+（Win10 1607+），fallback 价值低于其三重代价（pdh.dll 依赖 + 本地化 bug + 带 bug 的死函数）。部分 NVIDIA 驱动返回 `STATUS_INVALID_PARAMETER` → 诚实降级为"不可用"。 |
| **DPI** | manifest 声明 `PerMonitorV2`（`build.rs` + `embed-resource`，**仅 build-dependency，不进 exe、不增运行时体积**，不违背零依赖）；启动时检测未生效则回退 `SetProcessDpiAwarenessContext`。 |
| **测试分层** | Tier 1 = 断言 `layout::compute` 输出（纯函数、换字体不失效）+ 少量像素探针；**不为测试增加生产代码抽象**（不暴露 draw list）。Tier 2 = PNG 对拍（宽松阈值）。 |
| **默认模式** | 默认 **Quality**（首次印象）；提供"电量 <20% 自动切 Eco"选项。Eco 无中文，标记统一用 `~` 前缀（估算）/ 降饱和灰（陈旧）/ 隐藏整行（不可用）。 |

### 10.1 实测基准（2026-09-04）

`cargo build --release` + `cargo bloat --release --crates -p sysoverlay-app`：

| 项 | 实测值 |
|---|---|
| exe 体积 | **106.0 KiB**（108,544 B） |
| `.text` 段 | 74.5 KiB |
| └ `std` | **43.8 KiB（占 `.text` 58.7%）** |
| └ `[Unknown]` | 2.7 KiB |
| └ `sysoverlay_win` | 1.8 KiB |
| └ `sysoverlay_app` | 955 B |
| └ `windows` + `windows_result` | **50 B** |
| 相对 140 KiB 预算 | **剩余约 34 KiB 空间** |

**关键结论**：体积的大头是 **`std`（58.7%），不是 `windows` crate（0.05%）**——
`windows` 是零成本绑定。所以体积优化的正确杠杆是**减少 `std` 使用**
（`format!`/`String`/`Vec`/panic machinery），而不是去砍 `windows` 依赖。
双渲染模式估算 ~13.5 KiB，在 34 KiB 余量内，预算安全。
