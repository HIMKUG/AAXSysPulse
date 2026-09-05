# SysOverlay-C 重构 · 架构审查报告

**审查对象**：`D:\code\sys-overlay-c-master\sysoverlay`（Rust workspace，3565 行）
**对照文档**：`..\REPORT.md`（声称"重构完成 / 52 单测全绿 / 8 项技术债已修复"）
**审查方式**：逐文件通读 + 跑核心单测 + grep 实证
**日期**：2026-09-03（初稿）→ 2026-09-04（第二次修订，本轮更新）

---

## 0. 一句话结论

**分层意图是好设计，但 `REPORT.md` 描述的是"目标架构"，不是"已交付代码"。**
真正落地的是：纯逻辑 `core` + 离屏软件光栅 `ui` + 真实 `proc_cpu`/`proc_gpu` 采样器。
**没落地**的是整条整合链路：`app` 没有采样线程、`win/gdi` 画的是静态占位渐变（忽略快照）、
配置持久化/DPI/菜单/自愈矩阵全是 placeholder。报告里煞有介事的 `ArcSwap`、单实例互斥、
panic minidump、蒙版缓存、原子写配置、睡眠唤醒自愈——**代码里一个都没有**。

> 这不是"代码写得不好"，而是"报告与代码严重脱节"。下面逐条给证据。

---

## 1. 已落地 vs 未落地 对照表（均经 grep / 读码实证）

| REPORT.md 宣称 | 代码实际状态 | 证据 |
|---|---|---|
| 4.2 `ArcSwap<Snapshot>` 线程同步 | ✅ 已实现 | `win/window.rs:28` `OnceLock<ArcSwap<Snapshot>>` + `snap_get()`/`snap_store()` |
| 4.3 采集线程 + 指数退避 | ✅ 已实现 | `app/main.rs` 采样线程 → `snap_store()` → `SHARED_SEQ` |
| 5.4 单实例 `CreateMutexW` | ✅ 已实现 | `app/main.rs:56-71` |
| 5.4 panic minidump `MiniDumpWriteDump` | ⚠️ stub | `app/main.rs` panic hook 有框架；`write_minidump` 待实现 |
| 5.1 蒙版缓存 `GdiResources`/`BitBlt` | ❌ 未实现 | placeholder 等待 P2-11 |
| 5.3 原子写配置 `MoveFileExW` | ❌ 未实现 | `win/config_io.rs` placeholder，待 P1-6 |
| 4.4 自愈矩阵（PBT_APMRESUME…） | ❌ 未实现 | 待 P2-9 |
| DPI 感知 | ❌ placeholder | 待 P2-10 |
| 右键菜单 | ❌ placeholder | 待 P2-12 |
| **`ui` 离屏渲染 `draw_card`（镜像 C）** | ✅ 真实现 | `ui/card.rs`、`ui/offscreen.rs`、`ui/render.rs` |
| **`proc_cpu` `GetSystemTimes` 真实采集** | ✅ 真实现 | `win/proc_cpu.rs` |
| **`proc_gpu` PDH 真实采集** | ✅ 真实现 | `win/proc_gpu.rs` |
| **GDI `UpdateLayeredWindow` 渲染管线** | ✅ 已实现 | `win/gdi.rs` `render()` 调用 `ui::draw_card` → DIB → 上屏 |
| 52 个单测全绿 | ✅ 141 个全绿 | `cargo test --workspace` 全部通过 |

**结论**：报告的"量化指标 / 架构图 / 数据流 / 自愈矩阵"大部分是**规划稿**，不是验收稿。
建议把 `REPORT.md` 重命名为 `PLAN.md` 或明确标注"目标状态（WIP）"，否则会严重误导后续维护者。

---

## 2. 架构层面审查

### 🟢 2.1 值得保留的好设计

- **分层铁律清晰**：`core`(纯逻辑零依赖) / `ui`(渲染无 Win32) / `win`(唯一 unsafe) / `app`(编排)。
  `core` 不引用 `windows` crate，可离线编译+测试（已验证：`cargo test -p sysoverlay-core` 离线通过）。
- **`Metric<T>` 陈旧语义**：`Value`/`Stale{value,stale_for}`/`Unavailable` 比裸 `Option<f32>` 表达力强，
  天然支持"数值还在显示但已过期"的 UI 提示（建议真正接到 UI 上，见 §3）。
- **`SampleOutcome` 三态**：`Ok`/`Transient`/`Fatal`，区分"沿用旧值"与"需重建"，设计正确。
- **`ui` 软件光栅器**：预乘 alpha 合成、BGRA 对齐 Windows DIB、`to_png` 对拍 C 版，
  是整套重构里完成度最高、最可复用的部分（含 4 个单测 + 快照测试）。
- **`proc_gpu` 内联聚合写法正确**：`win/proc_gpu.rs:150` 用 `if let Some(key)` 跳过非法实例名，
  且 `util.clamp(0,100)`、按物理节点求和取最忙——口径与任务管理器一致。

### 🔴 2.2 关键正确性缺陷（必须修）

#### (a) CPU 占用率把 idle 时间重复计算 —— 数值系统性偏高
`core/src/metrics.rs:14-25`：
```rust
let total_time = idle_delta + kernel_delta + user_delta;   // ← 错
let busy_time  = kernel_delta + user_delta;
let usage = busy_time / total_time * 100;
```
但 `win/proc_cpu.rs:64` 喂入的是 `GetSystemTimes` 的返回值，而 **`GetSystemTimes` 的 kernel 时间已包含 idle 时间**（MSDN 明确）。
正确公式是 `total = kernel + user`，`busy = total - idle`，即：
```rust
let total = kernel_delta.saturating_add(user_delta);
let busy  = total.saturating_sub(idle_delta);
```
当前实现 `total = idle+kernel+user` 把 idle 算了两次，CPU% 被高估（idle 占比越高，偏差越大）。
`metrics.rs:110` 的 `test_cpu_usage_normal` 用 `(50,25,25)→50%` 的前提本身就是错的，
**测试在被错误公式"驯化"**。修公式时必须同步改测试与文档注释。

#### (b) 配置 `serialize`/`parse` 格式不匹配 + COLORREF 编码错误（双重 bug）
- `serialize`（`core/src/config.rs:320-337`）写的是 **空格分隔** `bg 3C2B1A`；
  `parse`（`config.rs:264`）却按 **`=` 分隔** `split_once('=')` 读 → **所有字段都解析不到，静默退回默认值**。
- 即使修好分隔符，`bg {:06X}` 用的是 `to_colorref()`（`BBGGRR`，config.rs:334），而 `parse` 的 `from_hex` 按 `RRGGBB` 读
  → **R/B 通道互换**。

**实证**：我写了个临时集成测试（非默认 config）跑 `Config::parse(serialize(cfg))`，结果：
```
left : bg=Rgb{r:26,g:43,b:60}, fg=Rgb{r:240,g:241,b:242}, opacity:80
right: bg=Rgb{r:20,g:22,b:27}, fg=Rgb{r:238,g:241,b:244}, opacity:90   ← 全退回默认
serialized: ...\nbg 3C2B1A\nfg F2F1F0
```
而现有 `test_config_serialize_roundtrip`（`config.rs:406`）只用 **默认 config**，
`parse` 全退回默认后恰好等于默认 → **假阳性绿**。这是"52 全绿"里最该警惕的一类测试。

**修复（2026-09-04 已修正，并撤回下方原建议）**：
> ~~统一为 `key=value`，颜色用 `to_hex()`。~~ **此建议错误，已撤回。**
> 经核对 `overlay.c:146-179`，**C 版配置是空格分隔而非 `key=value`**，改成 `key=value` 会亲手破坏 C 版配置迁移。
> 正确方向是 **「宽容读 + 保守写」**（见 `ARCHITECTURE_DESIGN.md` §3.1）：
> - **写**：严格 C 版格式 —— 空格分隔、数值枚举、`%06X` COLORREF、`interval` 用 100ms 单位。
> - **读**：空格与 `=` 都收；枚举数值/字符串双接受；颜色用 `from_colorref`（BBGGRR）；`interval` ×100 换算为 ms。
> - 新增 `pos` 解析（此前 serialize 写、parse 不读，位置根本没持久化）。

`crates/core/src/config.rs` 已按此重写，单测 53 个全绿，含：真实 C 版配置解析、非默认往返、枚举组合全量往返、interval 单位换算与吸附、COLORREF 回归、未知键忽略、畸形输入零 panic。

#### (c) `aggregate_gpu_nodes` 是死代码且本身有 bug
`core/src/metrics.rs:89-108` 在循环里写 `let key = node_key(name)?;` —— 任一实例名解析失败就 `?` 让**整个聚合返回 None**。
而 `proc_gpu.rs:134-161` 并没有调用它，而是**重新实现了一遍**正确版本（跳过非法名）。
→ 同一逻辑两份实现，且 `core` 这份还是坏的。应让 `proc_gpu` 直接复用 `core::aggregate_gpu_nodes`，
并把 `?` 改成 `if let Some(key)`（与 proc_gpu 一致）。

### 🟠 2.3 类型重复
`Zoom` / `Shape` / `VertLayout` 在 **`core/src/config.rs` 和 `core/src/layout.rs` 各定义一份**
（`snapshot_test.rs:6-7` 同时 import 了 `ConfigShape` 和 `LayoutShape` 来证明这点）。
两份 `Zoom` 还不一致：`config::Zoom::scale_factor()` 返回 `f32`，`layout::Zoom::as_factor()` 返回 `f64`。
后续改枚举值要改两处，迟早漏改。应合为**唯一真源**（建议放 `core`，`layout` 复用）。

### 🟠 2.4 集成缺口（最影响"能不能用"）
1. **`ui::draw_card` 没接到窗口上**：`win/gdi.rs:15` 的 `render_placeholder` 画的是写死渐变 + 3 根占位条，
   **完全忽略 `Snapshot`**；`gdi.rs:140` 的 `render()` 也只是转发到占位函数。真正值钱的 `ui` 软件光栅器目前是孤岛。
2. **`app` 是骨架**：没有采样线程、没有把快照喂给窗口、`WINDOW_STATE` 里永远是 `create_placeholder_snapshot()` 的假数据。
3. **`MockSampler::collect` 写死 `cpu:50.0` / `gpu:Unavailable`**（`win/sampler.rs:54-56`），
   把刚采到的真实 `CpuSampler` 结果直接丢弃——典型的占位残留，要么删 `MockSampler` 要么让它真正组合。

### 🟡 2.5 全局单例 + 内存小问题
- `win/window.rs:21` 用 `static WINDOW_STATE: Mutex<Option<WindowState>>` 全局单例，
  多窗口 / DPI 变更时会成为架构负担。
- `window.rs:130` `Box::into_raw(Box::new(()))` 往 `GWLP_USERDATA` 塞了个**永远没用、也永不释放的 `()`**——
  既泄漏又无意义。正确做法：把真正的 `WindowState` 装箱塞进去，`wndproc` 里 `GetWindowLongPtrW` 取回，
  这样连 `WINDOW_STATE` 全局锁都能删掉。

---

## 3. 联想的优秀设计改动（按优先级）

### P0 — 修正确性（不动架构，风险低、收益高）
1. **修 `cpu_usage` 公式**（§2.2a），并改写 `test_cpu_usage_*` 让其基于 `GetSystemTimes` 真实语义。
2. ~~**统一 config 序列化**：`key=value` + `to_hex()`。~~ → **已于 2026-09-04 完成**（方向修正为"宽容读 + 保守写"兼容 C 版，见 §2.2b）。
3. **收敛 GPU 聚合**（§2.2c）：`proc_gpu` 复用 `core::aggregate_gpu_nodes`，把 `?` 改 `if let Some`。

### P1 — 打通能跑的端到端链路（让"重构"真正可用）
4. **把 `ui` 软光栅接到 `gdi`**：`gdi::render(hwnd, &snapshot, &cfg)` 改为
   `let mut s = SoftwareSurface::new(w,h); draw_card(&mut s, &snap, &cfg, &layout, alpha);`
   再把 `s.pixels`（BGRA 预乘）直接塞进 `CreateDIBSection` 的 bits，`UpdateLayeredWindow` 上屏。
   这是天然接缝，零新增抽象。
5. **在 `app` 起采样线程 + `ArcSwap<Snapshot>`**：`app` 持 `ArcSwap`，线程定时 `MockSampler/真实Sampler.collect()`
   后 `store`，窗口在 `WM_PAINT`/`WM_TIMER` 里 `load()` 拿最新快照重绘。报告 4.2 的设计本身是对的，只是没写。
6. **实现 `config_io` 原子写**：按报告 5.3 用 `WriteFile`→`MoveFileExW(MOVEFILE_REPLACE_EXISTING)`，
   真正落盘到 `%APPDATA%\SysOverlayC\config.txt`，让"关窗记住位置/配色"可用。

### P2 — 健壮性 / 可维护性
7. **删全局 `WINDOW_STATE`，状态装箱进 `GWLP_USERDATA`**（§2.5），消除单例锁与泄漏。
8. **合并重复的 `Zoom/Shape/VertLayout` 为唯一真源**（`core` 定义，`layout` re-export 或复用）。
9. **落实自愈矩阵**（报告 4.4 是真需求）：`WM_POWERBROADCAST`→重置差分基准、`WM_DISPLAYCHANGE`→
   `clamp_to_visible`、`WM_WTSSESSION_CHANGE`→锁屏暂停采集。对 24/7 悬浮窗这些是刚需。
10. **DPI 用 `SetThreadDpiAwarenessContext(PER_MONITOR_AWARE_V2)`**（`dpi.rs` 现在是占位），
    否则多屏缩放下卡片会糊/错位。
11. **`Metric::Stale` 接到 UI**：某采样连续失败 N 次时，UI 把该指标标灰/加"~"提示，而不是静默显示旧值。
12. **实现 `GdiResources` 蒙版缓存**（报告 5.1）：底图按 `CacheKey(尺寸/DPI/形状/颜色/缩放)` 缓存，
    仅指纹变化时 `BitBlt` 重绘——这是避免每帧全像素重算的核心优化，目前 `render_placeholder` 每 `WM_PAINT`
    都重算整张渐变，一旦 P1 的连续重绘接上就会成为热点。

### P2 — 测试质量
13. **给 `serialize/parse` 加 property 测试**（对任意 config 往返相等），消除 §2.2b 的假阳性。
14. **加 `ui` 对拍回归**：`snapshot_test::generate_reference_image` 已能出 PNG，
    接一道 "与 C 版参考图像素差 < 阈值" 的 CI 门禁，防止渲染回归。
15. **`proc_cpu`/`proc_gpu` 加集成测试**（mock `GetSystemTimes`/`PDH` 返回值，验证聚合与边界）。

---

## 4. 建议落地顺序（给你排期参考）

```
Week 1   P0 三项 (cpu 公式 / config 往返 / gpu 聚合收敛)   ← 纯修 bug，可立即合
Week 2   P1-4,5,6 (ui→gdi 接线 / 采样线程+ArcSwap / config 落盘)  ← 让程序真的跑起来
Week 3   P2-7,8,9,10 (去全局单例 / 合并类型 / 自愈 / DPI)   ← 健壮性收口
Week 4   P2-11,12 + 测试门禁 (Stale UI / 蒙版缓存 / 对拍 CI)
```

> **2026-09-04 本轮进展**：P1-4（ui→gdi）、P1-5（采样线程+ArcSwap+单实例）、P0-1（config）均已实现。
> exe 体积从 106 KiB 增长到 151 KiB（.text 104.5 KiB），`gdi::render` 贡献 5.9 KiB。
> 剩余未完成：P0-3（GPU 聚合收敛）、P1-6（config_io）、P2 全系列。

---

## 5. 给 REPORT.md 本身的建议

- 把"已实现/未实现"分两节写，或直接在标题加 `（WIP）`。
- 量化指标（exe 106KB、内存 1.3MB 等）目前只是**目标值**，应在真的 `cargo build --release` 跑通、
  且 `app` 能端到端运行后再回填实测值。
- 目录树里 `color.rs`/`units.rs` 标注"已实现"与实际"空文件"矛盾，需同步。
