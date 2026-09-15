# AAXSysPulseC 全项目深度审计报告

- 审计日期：2026-09-15
- 代码基线：workspace `sysoverlay/`（Rust，4 crates，79 个 .rs，约 31,500 行）
- 审计范围：模块 1–12（线程/竞态、unsafe、panic、资源生命周期、配置解析、退出路径、错误处理、消息与窗口、渲染、采样器、性能、可维护性）
- 约束遵循：只审计不改代码；所有结论附文件:行号证据；未找到的项明确标注"未找到"
- 工具链：rustc 1.96.0（**关键前提**：`extern "system"` 中 panic 会 abort）、`panic = "unwind"`（Cargo.toml）

---

## 一、总览表

| # | 模块 | 结论 | Critical | High | Medium | Low |
|---|------|------|----------|------|--------|-----|
| 1 | 线程模型与竞态 | **有问题** | 1 | 3 | 3 | 0 |
| 2 | unsafe 审计 | 有问题 | 0 | 0 | 2 | 2 |
| 3 | unwrap / expect / panic | **有问题** | 0 | 1 | 2 | 3 |
| 4 | 资源生命周期 | 有问题 | 0 | 0 | 2 | 2 |
| 5 | 配置解析边界 | **有问题** | 0 | 1 | 4 | 2 |
| 6 | 线程退出路径 | **有问题** | 0 | 1 | 1 | 1 |
| 7 | 错误处理 | 有问题 | 0 | 0 | 1 | 1 |
| 8 | Win32 消息与窗口 | **有问题** | 0 | 2 | 2 | 2 |
| 9 | 渲染与 GDI/D2D | 通过（有待确认项） | 0 | 0 | 1 | 2 |
| 10 | 采样器专项 | **有问题** | 0 | 1 | 2 | 1 |
| 11 | 性能与资源占用 | 通过 | 0 | 0 | 1 | 2 |
| 12 | 可维护性与未来风险 | 通过 | 0 | 0 | 1 | 1 |
| | **合计** | | **1** | **10** | **22** | **19** |

共 52 项发现。Critical 1 项（配置异步落盘完全失效），High 10 项。

---

## 二、问题详情

### 【1-1】模块1-1 · 配置异步落盘链路完全断开

**【严重程度】Critical**
**【位置】** `crates/win/src/config_io.rs:432-516`（`start_writer_thread` / `save_config_async` / `shutdown_writer`）

**【代码片段】**
```rust
// config_io.rs:432-435  —— start_writer_thread 内部的函数级 static
fn start_writer_thread() {
    static PENDING:  OnceLock<Arc<Mutex<Option<Bundle>>>> = OnceLock::new();
    static SHUTDOWN: OnceLock<Arc<AtomicBool>>            = OnceLock::new();
    static NOTIFY:   OnceLock<mpsc::Sender<()>>           = OnceLock::new();
    ...
    let _ = NOTIFY.set(notify_tx);          // :478  ← 设置的是"本函数"的 NOTIFY
}

// config_io.rs:486-503  —— save_config_async 内部**同名但不同项**的 static
pub fn save_config_async(cfg: &Config) -> bool {
    start_writer_thread();
    static PENDING: OnceLock<Arc<Mutex<Option<Bundle>>>> = OnceLock::new();  // :490  ← 另一个
    *pending.lock().unwrap() = Some(Bundle { files: bundle_for(cfg) });      // :494  写线程读不到
    static NOTIFY: OnceLock<mpsc::Sender<()>> = OnceLock::new();             // :498  ← 另一个，从未 set
    if let Some(n) = NOTIFY.get() { return n.send(()).is_ok(); }
    false                                                                     // :502  永远走这里
}

// config_io.rs:506-516 —— shutdown_writer 又是另外两个 static，全部为空 → 空操作
```

**【问题描述】**
Rust 中声明在函数体内的 `static` 是**各自独立的项**，不同函数里的同名 `static` 不共享存储。因此：

1. `save_config_async` 写入的 `PENDING` 与写线程读取的 `PENDING` 是两个对象 → 待写数据永远无人消费；
2. `save_config_async` 读取的 `NOTIFY` 从未被 `set` → 恒返回 `false`，且从不通知写线程；
3. 写线程（`:452`）永远阻塞在 `rx.recv()`，`notify_tx` 从未发出过任何信号 → **写线程自始至终没有写过一次盘**；
4. `shutdown_writer()`（**:506**）的两个 static 也各自独立且为空 → 完全空操作（全项目亦无任何调用点）；
5. `start_writer_thread` 里 `if NOTIFY.get().is_some() { return; }`（**:441**）检查的是它自己的 NOTIFY，所以线程只会被启动一次 —— 一个常驻但永不工作的线程。

**【证据】**
调用点：`crates/win/src/settings/mod.rs:1087`（`apply_config` 防抖落盘）、`settings/mod.rs:1713`。
同步落盘点仅三处：`settings/mod.rs:477`（`flush_config`，关窗 flush）、`main.rs:241/303/709`（启动与退出）。

**【影响】**
- 设置页改动在**关窗前不落盘**；进程被强杀、崩溃、系统关机时改动全部丢失。
- `ctx.save.on_saved(now)`（`settings/mod.rs:1089`）永不执行 → `DebouncedSave.last_save_ms` 恒为 `None` → `on_change()` 恒返回 `true`（`settings/tasks.rs:28-34`）→ **防抖语义失效**，本应被合并的高频改动每次都尝试走一遍落盘路径（虽然写不出去，但白做了 `bundle_for(cfg)` 的 4 次字符串序列化）。
- 与【6-1】（关机不保存）叠加后，用户改设置 → 直接关机 = 改动全丢。

**【修复方案】**
把三个 static 提升为**模块级**单一状态（推荐）：

```rust
struct WriterState { pending: Mutex<Option<Bundle>>,
                     shutdown: AtomicBool,
                     notify: OnceLock<mpsc::Sender<()>> }
static WRITER: OnceLock<Arc<WriterState>> = OnceLock::new();
```
三个函数统一从 `WRITER` 取。改动约 30 行，局限在 `config_io.rs`。

**【修复风险】**
低。但**必须同时做**：
- 修好后异步写会真正生效 → 滑杆拖动会高频触发 `sync_all()`（fsync）→ 必须确认 `DebouncedSave`（500ms）确实生效（修好后 `on_saved` 会正常调用，防抖自然恢复）；
- 必须同时处理【5-8】的并发写问题（同步 `save_config` 与异步写线程争用同一个 `.tmp`）。

**【代价】** CPU 0；内存 0；exe 体积 0。

---

### 【1-2】模块1-2 · 采样线程跨线程写 UI 状态（真实数据竞争）

**【严重程度】High**
**【位置】** `crates/win/src/gdi.rs:488-497`（调用方 `main.rs:514-517`，采样线程）

**【代码片段】**
```rust
// gdi.rs:488-497  mark_d2d_broken_and_persist_gdi  —— 运行在**采样线程**
fn mark_d2d_broken_and_persist_gdi(hwnd: HWND) {
    D2D_BROKEN.store(true, AtomicOrdering::Relaxed);
    if let Some(mut cfg) = crate::window::get_config_of(hwnd) {        // 读：裸指针 → Config copy
        if cfg.render_engine == RenderEngine::D2d {
            cfg.render_engine = RenderEngine::Gdi;
            crate::window::modify_config(hwnd, |c| *c = cfg);          // ← 写：&mut ThreadState.cfg
            let _ = crate::config_io::save_config(&cfg);               // ← 采样线程上做 fsync 写盘
        }
    }
}

// window.rs:1003-1014
pub fn modify_config(hwnd: HWND, f: impl FnOnce(&mut Config)) {
    let p = unsafe { get_state_ptr(hwnd) };
    if !p.is_null() { unsafe { f(&mut (*p).cfg) }; ... }               // 无锁跨线程写
}
```

**【问题描述】**
`ThreadState` 归 **UI 线程**所有，wndproc 在多处取 `&mut ThreadState` 并读写 `cfg`
（`window.rs:548/566/599/609/645/674/768/790/843/894`）。采样线程通过 `modify_config` 直接写同一块内存，**无任何同步原语** → 教科书级 data race（UB）。

同时，主线程退出路径 `main.rs:709 save_config(&final_cfg)` 与本处的 `save_config` 可能并发执行 → 两个线程同时写同名 `.tmp` + 同目标文件。

**【触发条件】** 渲染看门狗判定"新线程也卡死"→ `WatchdogAction::DisableD2d`（`gdi.rs:410-415`）。不常见，但一旦发生就是 UB + 可能损坏配置文件。

**【证据】** `main.rs:510-519` 显示该调用发生在采样线程每轮循环内。

**【修复方案】**
改为向 UI 线程投递消息（新增 `WM_DISABLE_D2D`），由 UI 线程自己 `modify_config` + `save_config`。采样线程只负责 `D2D_BROKEN.store(true, Release)`。约 15 行。

**【修复风险】** 低。需回归"D2D 卡死 → 自动回退 GDI 并持久化"这条自愈路径。

**【代价】** 0。

---

### 【1-3】模块1-3 · SAMPLER_SUSPENDED 丢失唤醒通知 → 采样永久冻结，兜底逻辑不可达

**【严重程度】High**
**【位置】** `crates/app/src/main.rs:455-488`；`crates/win/src/window.rs:917/922`

**【代码片段】**
```rust
// main.rs:455-462  —— 循环最前部的挂起等待
while aaxsyspulse_win::sampler_suspended() {
    if resume_rx.recv_timeout(Duration::from_millis(100)).is_err() { }
}

// main.rs:480-488  —— **在挂起循环之后**的兜底唤醒检测
let now_before_wait = get_tick_count_ms();
const FALLBACK_WAKE_GAP_MS: u64 = (REFRESH_FLOOR_MS as u64) * 3;   // = 4500
if now_before_wait.saturating_sub(last_sample_ms) > FALLBACK_WAKE_GAP_MS {
    sampler.reset_backoff();
    probe::log("sampler: fallback wake detected ...");
}
```

```rust
// window.rs:917 / 922 —— SAMPLER_SUSPENDED 的**全部**写入点
PBT_APMSUSPEND         => SAMPLER_SUSPENDED.store(true,  Relaxed),
PBT_APMRESUMEAUTOMATIC => SAMPLER_SUSPENDED.store(false, Relaxed),   // 唯一的清除点
```

**【问题描述】**
`SAMPLER_SUSPENDED` 只能在收到 `PBT_APMRESUMEAUTOMATIC` 时清除。而"Modern Standby 下 `WM_POWERBROADCAST` 可能不触发"**正是兜底唤醒检测存在的理由**（`main.rs:480-482` 注释自述）。但兜底检测位于挂起循环**之后**：

> 通知丢失 → `SAMPLER_SUSPENDED` 恒为 `true` → 采样线程永久停在 100ms 轮询里 → 永远走不到第 485 行的兜底检测 → **无法自愈**。

即兜底机制对自己宣称要防护的场景完全无效。

**【证据】** 全项目 `SAMPLER_SUSPENDED` 仅 `window.rs:492`（定义）、`495`（读）、`917/922`（写）；`main.rs:455` 唯一消费点。

**【修复方案】**
在挂起循环内引入自愈：累计挂起时长 > `max(30_000, 上次计划 sleep × 2)` 时强制清标志并继续；或把挂起等待改成"带超时的 `wait_cfg_change`"，把兜底检测提到循环最前。约 8 行。

**【修复风险】** 低。需在真实睡眠/唤醒（含 Modern Standby）下回归。

**【代价】** 0。

---

### 【1-4】模块1-4 · 诊断指针经异步消息传递，存在 UAF 窗口

**【严重程度】High**
**【位置】** `crates/win/src/window.rs:875-887`

**【代码片段】**
```rust
WM_GET_SAMPLER_DIAG => {
    let diag = shared_diag();                       // Arc<SamplerDiag>，ArcSwap 中持一份引用
    let ptr = &*diag as *const SamplerDiag;         // 取内部裸指针
    let target = HWND(lparam.0 as *mut _);
    let _ = PostMessageW(target, WM_SAMPLER_DIAG, WPARAM(0), LPARAM(ptr as isize));
    LRESULT(0)                                      // ← diag 在此 drop
}
```

**【问题描述】**
`PostMessageW` 是**异步**的。`diag` 在分支末尾 drop，但 ArcSwap 自身仍持一份引用，对象暂不释放。然而 `update_shared_diag`（`window.rs:265-267`）由采样线程**每轮**执行 `store(Arc::new(...))` —— 一旦 store 新值，旧对象的引用计数归零即被释放，**而 `WM_SAMPLER_DIAG` 可能还在设置窗队列里没被处理**。

接收端（`settings` wndproc）解引用该指针读取 `SamplerDiag` → **use-after-free**。

**【触发条件】** 设置窗口在处理该消息前被阻塞 ≥ 一个采样周期（1.5s）。真实可达场景：设置页正打开"自定义颜色"对话框（`settings/mod.rs:1348` 的 `CUST` 互斥量附近）或任何模态 `MessageBoxW`（本项目大量使用，如 `config_io.rs:649/682/710/737`）。

跨进程场景（二次启动 `open_settings`）下该指针 100% 无效 —— 但那条路径本身就是坏的，见【8-1】。

**【修复方案】**
显式交接所有权：发送端 `Arc::into_raw(diag)`，接收端 `Arc::from_raw(ptr)` 后立即使用并自然 drop。或按值传递（把 `SamplerDiag` 拷进 `WM_COPYDATA`）。

**【修复风险】** 低，但容易漏掉"接收端必须 `from_raw`"这一约定，建议加注释 + 单测。

**【代价】** 一次原子引用计数增减，可忽略。

---

### 【1-5】模块1-5 · 兜底唤醒阈值与真实睡眠时长冲突 → 长期误报

**【严重程度】Medium**
**【位置】** `crates/app/src/main.rs:484-488`、`672-678`

**【代码片段】**
```rust
const FALLBACK_WAKE_GAP_MS: u64 = (REFRESH_FLOOR_MS as u64) * 3;    // 1500*3 = 4500ms
...
let base_timeout_ms: u64 = if any_enabled { ... } else { 30_000u64 };   // :675 全关 → 30s 慢心跳
let remote_factor: u64 = if in_remote_session() { 2 } else { 1 };       // :677
let timeout_ms: u32 = (base_timeout_ms * remote_factor) as u32;         // :678 → 最大 60s
```

**【问题描述】**
阈值固定 4500ms，但实际睡眠可达 30s（全源关闭）或 60s（远程会话）。
→ 全源关闭时**每 30s 必然误判一次"系统刚从睡眠恢复"**，调用 `reset_backoff()` 并写日志；远程会话下每 60s 一次。
用户手动把 `interval_ms` 调到 >4.5s 时同样持续误报。

**【影响】** 日志噪声 + 无意义的退避重置（会掩盖真实的采样失败退避）。不造成崩溃。

**【修复方案】** 阈值改为与"本轮计划睡眠时长"挂钩：
`gap > max(3 * REFRESH_FLOOR_MS, last_planned_sleep_ms * 3 / 2)`。

**【代价】** 0。

---

### 【1-6】模块1-6 · 跨线程标志位与调度镜像一律使用 Relaxed

**【严重程度】Medium**
**【位置】** `window.rs:328-350/495/510/518`、`gdi.rs` 各处

**【代码片段】**
```rust
// window.rs:328-350（写侧，UI 线程）
SCHED_AUTO.store(cfg.refresh_auto, Ordering::Relaxed);
SCHED_INTERVAL_MS.store(cfg.interval_ms, Ordering::Relaxed);
SCHED_ENABLED.store(mask, Ordering::Relaxed);
...
// window.rs:373-387（读侧，采样线程）—— 4 次**独立** Relaxed 读
pub fn schedule_cfg() -> (u32, bool, [bool; 5], u32) {
    let mask = SCHED_ENABLED.load(Ordering::Relaxed);
    ( SCHED_INTERVAL_MS.load(Relaxed), SCHED_AUTO.load(Relaxed), [...], SCHED_POWER_INTERVAL_MS.load(Relaxed) )
}
// window.rs:495/510/518
SAMPLER_SUSPENDED.load(Relaxed); SESSION_LOCKED.load(Relaxed); IN_REMOTE_SESSION.load(Relaxed);
```

**【问题描述】**
1. **语义层**：`Relaxed` 只保证原子性与修改顺序一致性，**不构成 happens-before**，也不保证"何时"对其它线程可见。这些变量全是"标志位"语义（置位后要求对方尽快看到）。x86-64 的 TSO 内存模型使这在实践中无害，但：
   - 按 Rust/LLVM 内存模型这是未定义行为的温床（编译器理论上可把 `while sampler_suspended()` 优化成读一次）；
   - 一旦将来支持 **ARM64 Windows（WoA）**，弱内存序下会真实出现"标志置了但对方看不到"。
2. **撕裂层**：`schedule_cfg()` 把 4 个字段分 4 次独立读取 → 组合快照非原子，可能读到"新 interval + 旧掩码"。下一轮自愈，影响 Low。

**【缓解证据】** `publish_schedule_cfg`（`window.rs:353-356`）在写完所有 Relaxed 后取 `CFG_GEN` 互斥锁并 `notify_all`，而采样线程从 `wait_cfg_change`（`:298`）醒来时必然经历同锁的 acquire → **经由 CFG_GEN 建立了 happens-before**，所以"配置改动"这条路径实际上是安全的。真正只靠 Relaxed 裸奔的是 `SAMPLER_SUSPENDED` / `SESSION_LOCKED` / `IN_REMOTE_SESSION` / `D2D_BROKEN` / `FORCE_FULL_REFRESH`。

**【修复方案】**
- 标志位：`store(.., Release)` / `load(.., Acquire)`（或 SeqCst）；
- 调度镜像：仿照 `SCHED_ENABLED` 的位域做法，把 interval/auto/mask/power_iv 打包进一个 `AtomicU64` 一次读出。

**【代价】** x86-64 上 Release store = 普通 `mov`，Acquire load = 普通 `mov`，**零 CPU 代价**；exe 体积不变。

---

### 【1-7】模块1-7 · 采样/渲染/写线程 JoinHandle 全部丢弃，无退出信号

**【严重程度】Medium**（与【1-2】【4-2】组合后升级为 High）
**【位置】** `main.rs:411`、`render_thread.rs:55`、`config_io.rs:449`

**【问题描述】**
- 采样线程：`let _sampling_thread = thread::spawn(...)`（`main.rs:411`）→ JoinHandle 立即 drop（detach），循环 `loop {}` 无终止条件；
- 渲染线程：`render_thread.rs:55-86`，退出条件是"通道关闭"，但 `RENDER_TX` 里的 Sender 永不 drop → **实际上永不退出**；
- 写线程：`config_io.rs:449-476`，`shutdown_writer()` 全项目无调用点 → 永不退出。

`main` 在 `run_message_loop()` 返回后直接落盘并 return（`main.rs:697-711`），不通知、不 join 任何子线程，靠 OS 进程回收。

**【影响】** 当前不会崩溃（Windows 进程退出会强制回收）；但意味着：
- 采样线程可能在 `sample()` 中途被杀（持 `RING0` Mutex / MSR 设备）；
- 【4-2】的 respawn 会累积线程；
- 无法保证"退出前最后一次配置写入"的原子性。

**【修复方案】** 见"三、修复优先级"P3（加全局 `AtomicBool SHUTDOWN` + 线程内检查）。**或按"不建议修"保持现状**——见第五节。

---

### 【2-1】模块2-1 · `transmute` 可用安全等价写法替代

**【严重程度】Low**
**【位置】** `crates/win/src/msr.rs:106`

```rust
let vendor = [r0.ebx, r0.edx, r0.ecx];
let vb: [u8; 12] = std::mem::transmute(vendor);      // ← 可避免
let amd = &vb == b"AuthenticAMD";
```
**【问题描述】** 功能正确（x86 小端），但 `transmute` 引入不必要的 UB 面（且强假设目标为小端）。
**【修复方案】**
```rust
let mut vb = [0u8; 12];
vb[0..4].copy_from_slice(&r0.ebx.to_le_bytes());
vb[4..8].copy_from_slice(&r0.edx.to_le_bytes());
vb[8..12].copy_from_slice(&r0.ecx.to_le_bytes());
```
**【代价】** 编译期完成，运行时 0。

---

### 【2-2】模块2-2 · `StartServiceW` 混用 `GetLastError()` 与 `e.code()`

**【严重程度】Low-Medium**
**【位置】** `crates/win/src/msr.rs:272-279`

```rust
if let Err(e) = StartServiceW(svc.0, None) {
    if GetLastError() != ERROR_SERVICE_ALREADY_RUNNING {     // ← 用 GetLastError
        probe::log(&format!("msr StartService failed err={}", e.code().0));  // ← 又用 e.code()
    }
}
```
**【问题描述】** `windows` crate 已把 Win32 错误封装进 `Err(e)`；此处再调 `GetLastError()`，其值是"最近一次 Win32 调用"的错误码，可能被 crate 内部的后续调用覆盖 → 判定 183 的逻辑可能失效（表现为：服务已在运行时仍打一条失败日志，或反之）。
**【修复方案】** 统一用 `e.code().0 as u32 == ERROR_SERVICE_ALREADY_RUNNING.0`。
**【代价】** 0。

---

### 【2-3】模块2-3 · `GWLP_USERDATA` 裸指针可被非拥有线程访问

**【严重程度】Medium**（与【1-2】同源，此处单列 unsafe 视角）
**【位置】** `window.rs:462-464/478-485/993-1014/1019-1042`

**【问题描述】** 模块头（`window.rs:6-13`）明确声明"裸指针仅在窗口存续期有效，消费方全部位于本文件的 wndproc 分支内"。但 `is_window_hovered`（:478）、`get_config_of`（:993）、`modify_config`（:1003）、`get_window_size`（:1019）均为 `pub`，可从任意线程调用 —— 【1-2】已给出一条真实跨线程调用链。**契约与实现不符**。
**【修复方案】** 把这些函数标记为 `pub(crate)` 并加 `// 仅 UI 线程调用` 契约注释；或引入 `HWND → 线程 ID` 断言（`debug_assert_eq!(GetCurrentThreadId(), ui_tid)`，debug 构建下即可抓到违规）。
**【代价】** debug 构建加一次 TLS 读，release 编译掉。

---

### 【2-4】模块2-4 · Win32 返回值判定抽查结论

**【严重程度】—（通过）**
**【抽查结果】**
| 位置 | API | 判定方式 | 结论 |
|---|---|---|---|
| `proc_disk.rs:55/63/72/118` | `PdhOpenQueryW` / `PdhAddCounterW` / `PdhCollectQueryData` | `ret != 0` | ✅ PDH_STATUS 0=成功 |
| `proc_disk.rs:156/174` | `PdhGetFormattedCounterValue` | `ret != 0` → 0.0 | ✅ |
| `config_io.rs:131-142` | `MoveFileExW` | `.is_err()` + `GetLastError()` | ✅ |
| `window.rs:75` | `SHQueryUserNotificationState` | `.is_ok()` | ✅ 返回 WIN32_ERROR |
| `main.rs:175-179` | `CreateMutexW` | `GetLastError() != ERROR_ALREADY_EXISTS` | ✅ |
| `gdi.rs:1000-1025` | `CreateCompatibleDC`/`CreateDIBSection` | `is_invalid()` / `is_null()` | ✅ 且 `bits.is_null()` 额外判了一道 |
| `msr.rs:293` | `CreateFileW` | `Ok(h) if !h.is_invalid()` | ✅ |
| `msr.rs:272` | `StartServiceW` | `GetLastError()` | ⚠️ 见【2-2】 |

字符串：全部使用 `w!` / `encode_wide().chain(Some(0))` → UTF-16 正确。`config_io.rs:63 to_wide` 统一封装 ✅。
**未找到** `transmute` 的其他用法（仅 msr.rs:106 一处）、**未找到** `Vec::from_raw_parts` / `Box::from_raw` 的误用（`window.rs:1400/1425` 的 `Box::into_raw`/`from_raw` 配对正确）。

---

### 【3-1】模块3-1 · `CfgRef::set` 类型错配 `panic!` 位于 `extern "system"` 调用链 → 进程 abort

**【严重程度】High**
**【位置】** `crates/win/src/settings/cfg_ref.rs:98-152`；`settings/mod.rs:2508`

**【代码片段】**
```rust
// cfg_ref.rs:98（其余 40 个臂同构）
(CfgRef::ShowCpu, v) => { match v {
    Value::Bool(x) => { cfg.show_cpu = x; true }
    _ => { eprintln!("[CfgRef] 类型错配: ShowCpu <- {:?}", v);
           panic!("CfgRef 类型错配: ShowCpu <- {:?}", v); }        // ← 41 处
} }

// settings/mod.rs:2508
unsafe extern "system" fn settings_wndproc(...) { ... }
```
调用链：`settings_wndproc` → `apply_config`（`settings/mod.rs:1071`）→ `CfgRef::set`。

**【问题描述】**
rustc 1.96 下，`extern "system"`（以及 `extern "C"`）函数中 unwind 出界即 **abort**。Cargo.toml 虽设 `panic = "unwind"`，那只是让 Rust 侧能 catch，无法跨越 FFI 边界。
`main.rs:264-267` 的注释也承认"wndproc 里的 panic 会跨 FFI 边界直接 abort，表现就是开个设置窗、点一下按钮整个进程无声消失"—— 但那里只装了 panic hook，**panic hook 只负责打印，不能阻止 abort**。

同样的问题也覆盖悬浮窗 `wndproc`（`window.rs:521`，同为 `unsafe extern "system"`）。

**【触发条件】** 类型错配属代码 bug，理论上不可达；但结合【3-2】的越界 panic，可达性并非为零。

**【修复方案】**（两个都做）
1. `settings_wndproc` / `wndproc` 的函数体整体包一层 `std::panic::catch_unwind(AssertUnwindSafe(|| { ... }))`，Err 时返回 `DefWindowProcW` 的默认值；
2. `CfgRef::set` 的 `_ =>` 分支改为 `eprintln! + return false`（UI 线程里"改不动"远优于"进程消失"），`debug_assert!` 保留给调试构建。

**【修复风险】** 低。catch_unwind 有极小运行时开销（设置 unwinding 表），但只在 wndproc 入口，非 hot path。
**【代价】** exe +1~3KB；CPU 0。

---

### 【3-2】模块3-2 · `CfgRef::get` 侧裸下标越界 panic，与 set 侧防护不对称

**【严重程度】Medium**
**【位置】** `cfg_ref.rs:87-92`（get） vs `136-152`（set）

```rust
// get 侧 —— 裸下标，越界即 panic
CfgRef::MetricFont(i)  => Value::Num(cfg.metric_font_sizes[i].unwrap_or(cfg.font_size) as f64),   // :88
CfgRef::MetricLabel(i) => Value::Color(cfg.metric_colors[i].label),                                // :90
CfgRef::MetricValue(i) => Value::Color(cfg.metric_colors[i].value),                                // :91
CfgRef::MetricValueAuto(i) => Value::Bool(cfg.metric_colors[i].value_auto),                        // :92

// set 侧 —— 有边界检查，返回 false
(CfgRef::MetricFont(i), v) => { if i < cfg.metric_font_sizes.len() { ... } else { false } }        // :138-142
```
`MetricShowLabel` 的 get 侧用了 `.get(i).unwrap_or(&true)`（:87）—— 说明作者知道要防护，但另外 4 个漏了。

**【证据】** 测试 `out_of_range_index_is_rejected_not_panicking`（`cfg_ref.rs:271-276`）**只测了 set 侧**。

**【修复方案】** get 侧统一改用 `.get(i)`；并给 `roundtrip_cases` 补一条 get 侧越界断言。
**【代价】** 0。

---

### 【3-3】模块3-3 · 采样线程无 `catch_unwind`，panic 后静默冻结且无自愈

**【严重程度】Medium**（影响面大）
**【位置】** `main.rs:411-694`

**【问题描述】**
渲染线程有 `catch_unwind`（`render_thread.rs:75-84`），设置页布局有（`settings/layout.rs:1417`），**采样线程没有**。
任一子采样器 panic（`proc_gpu`/`proc_net`/`msr`/`proc_disk` 内均有 unsafe 与 unwrap 面）→ 采样线程死亡 → 数值永久冻结。

更糟的是**无自愈**：渲染看门狗（`gdi.rs:382`）只监控渲染线程；采样线程死后 `RENDER_EPOCH` 不再增长，看门狗判定"健康"，永不 respawn。用户看到的现象是"悬浮窗还在，数字不动"，只能重启。

**【修复方案】** 在 `loop` 体内包 `catch_unwind`，`Err` 时记日志 + `sleep(500ms)` 后继续下一轮。
**【代价】** exe +1~2KB；CPU 0（不在 hot path）。

---

### 【3-4】模块3-4 · 写线程 spawn 失败直接 `expect`

**【严重程度】Low**
**【位置】** `config_io.rs:476`
```rust
.expect("config-writer thread spawn failed");
```
**【问题描述】** `save_config_async` 首次调用发生在 UI 线程（`settings/mod.rs:1087`）；线程创建失败（极罕见）→ panic → 经 wndproc FFI 边界 → abort（见【3-1】）。
**【修复方案】** 改为 `if let Err(_) = ... { return; }`，让 `save_config_async` 的 `NOTIFY.get()` 自然返回 None/false。
**【代价】** 0。

---

### 【3-5】模块3-5 · 锁中毒后静默 return，与同项目其他位置风格不一致

**【严重程度】Low**
**【位置】** `gdi.rs:714`（SURFACE）、`gdi.rs:1062`/`1340`（DIB）、`gdi.rs:798`（PREV_CELLS）

```rust
let Ok(mut surface) = surface.lock() else { return };      // :714  中毒 → 永久不再渲染
```
**【问题描述】** `render_thread.rs:114-120`、`window.rs:299`、`settings/tasks.rs` 等处都用了 `unwrap_or_else(|p| p.into_inner())` 做中毒恢复，唯独渲染资源这几处直接放弃。一旦某次 panic 恰好发生在持锁期间（D2D panic 已被 catch_unwind 兜在**锁外**，但 `compute_dirty_in` 在锁内），**渲染永久停止**且无任何日志。
**【修复方案】** 统一为 `unwrap_or_else(|p| p.into_inner())` + 一条 `probe::log`。
**【代价】** 0。

---

### 【4-1】模块4-1 · 渲染线程 respawn 不回收旧线程

**【严重程度】Medium**
**【位置】** `render_thread.rs:44-87`、`gdi.rs:311/362/404`

**【代码片段】**
```rust
// render_thread.rs:44-48
pub fn start() {
    let (tx, rx) = mpsc::channel::<isize>();
    if let Ok(mut g) = RENDER_TX.lock() { *g = Some(tx); }    // 旧 Sender 被**覆盖丢弃**？
```
实际 `*g = Some(tx)` 会先 drop 旧的 `Option<Sender>` → 旧 Sender 被释放 → 旧线程 `rx.recv()` 返回 Err → 旧线程**确实会退出**。
但 `gdi.rs:56` 的旧线程闭包里 `let Ok(hwnd_addr) = rx.recv() else { log; return; }` —— 逻辑上能退出。

真正的风险点：`WEDGE_RESPAWNED`（`gdi.rs:311`）闸门在 `clear_wedge_gate()`（:362）后复位，因此"卡死→respawn→恢复→再卡死→respawn"可**无限循环**，每次 +1 线程。旧线程若不是阻塞在 `recv()` 而是**卡在原生调用里（`render_frame_begin` 之后的 D2D/GDI 调用）**，它永不返回 `recv()` → 永不退出 → **线程真泄漏**。

**【证据】** `gdi.rs:388-391` 注释："旧线程（卡在原生调用里）迟到的 `render_frame_end` 带旧 gen，被忽略" —— 作者明确知道旧线程可能永久卡住。

**【影响】** 每次"D2D 卡死"泄漏 1 个线程（默认 1MB 栈 + TEB）。反复发生会累积。

**【修复方案】** 在 `WEDGE_RESPAWNED` 之外再加一个"本会话最多 respawn N 次（如 3 次）"的硬闸门；或把 `start()` 的旧 Sender 显式 `drop` 后再建新通道（当前是覆盖，等价，无需改）。
**【代价】** 3 行。

---

### 【4-2】模块4-2 · 配置写线程永不退出且从不工作

**【严重程度】Medium**（与【1-1】合并处理）
**【位置】** `config_io.rs:449-476`
**【问题描述】** 见【1-1】。线程常驻阻塞在 `rx.recv()`（:452），`shutdown_writer()` 全项目无调用点。随【1-1】一并修复。

---

### 【4-3】模块4-3 · DIB 缓存未在 WM_DESTROY 释放

**【严重程度】Low**
**【位置】** `window.rs:974-984`（WM_DESTROY） vs `gdi.rs:950`（`invalidate_dib_cache`）
**【问题描述】** `WM_DESTROY` 清理了 WTS 通知与全屏定时器，未调 `invalidate_dib_cache()`。进程退出时 OS 回收 GDI 对象，当前无实际泄漏；但 `FULLSCREEN_TIMER`(16) 被 Kill，而 `CLOCK`(8) 与 `ANIM`(7) 定时器也未 Kill（窗口销毁时系统自动清理，同样无实际泄漏）。
**【修复方案】** WM_DESTROY 补 `invalidate_dib_cache()` + KillTimer(7)/KillTimer(8)，让资源生命周期显式对称。
**【代价】** 0。

---

### 【4-4】模块4-4 · 资源生命周期抽查结论（通过项）

| 资源 | 位置 | 结论 |
|---|---|---|
| DIB（HBITMAP/HDC） | `gdi.rs:974-1036` `DibCache` | ✅ `free()` 正确配对 `SelectObject/DeleteObject/DeleteDC`；仅尺寸变化/显示器变化时重建 |
| `GetDC`/`ReleaseDC` | `gdi.rs:1056-1128`、`1189-1251`、`1335-1464` | ✅ 全部 4 处 early-return 都先 `ReleaseDC` |
| MSR 设备句柄 | `msr.rs:77-84` `Drop for Ring0` | ✅ 且注释说明了"没 Drop 就每次唤醒泄漏一个 HANDLE" |
| SCM/Svc 句柄 | `msr.rs:150-158` `SvcHandleGuard` | ✅ RAII，含事故复盘注释 |
| MSR 唤醒重建 | `msr.rs:91 RING0_STALE` ← `window.rs:929 invalidate_on_resume()` | ✅ 与 GPU `invalidate_topology` 对称 |
| PDH 查询 | `proc_disk.rs:181-187 Drop` | ✅；关闭磁盘监控时 `sampler.rs:260 self.disk = None` 触发 |
| WTS 通知 | `window.rs:1265` / `978` | ✅ 注册/注销配对 |
| D2D 资源缓存 | `render_d2d.rs:57/79/101/112` `thread_local! RefCell` | ✅ 工厂/RT/格式/画刷均缓存，非每帧创建 |
| 字形图集 | `ui/font.rs:770` 固定容量 `ATLAS_CAP` | ✅ 无无界增长 |
| `BeginDraw/EndDraw` | `gdi.rs:1363/1375` | ✅ 配对；`end_draw()` 返回 false 时回滚指纹（:1377-1384） |
| **D2D 设备丢失后 RT 是否真被丢弃重建** | `render_d2d.rs:89 RT_DEVICE_LOST` | **待确认** —— 未在 bind() 中读到显式的 RT 丢弃代码，注释称"下帧 bind 会重建"，需核对 `D2DSurface::bind` 实现 |

---

### 【5-1】模块5-1 · 配置文件 BOM 未剥离 → 整个文件静默回退默认

**【严重程度】High**
**【位置】** `config_io.rs:102-105`（`read_if`）、`core/src/config/ini.rs:59-94`（`Ini::parse`）

**【代码片段】**
```rust
fn read_if(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name)).ok()      // ← 保留 UTF-8 BOM（U+FEFF）
}

// ini.rs:70-77
if let Some(rest) = line.strip_prefix('[') {
    if let Some(sec) = rest.strip_suffix(']') { ... cur = sec.trim().to_ascii_lowercase(); }
}
```
BOM 位于**第一个字符**，`line.strip_prefix('[')` 对 `"\u{feff}[system]"` 失败 → 该 section 名变成 `\u{feff}system`（实际会落到 key 解析分支，被 `split_once('=')` 丢弃或产生垃圾 key）。

**【问题描述】** 用户用记事本/"另存为 UTF-8 with BOM"编辑 `system.cfg` 后：
- 该文件所有键查不到 → **整个文件回退默认值**；
- `read_file_version`（`config_io.rs:206-217`）读不到 version → 视为 **v1**（:230-237）；
- 其余文件为 v3 → `max - min = 2 > 1` → 触发"版本跨度过大"弹窗（main.rs:204-259），用户点"是"= **全部配置被默认覆盖**。

一次编码意外 → 全套设置丢失。这在 Windows 中文用户里是高频操作（记事本默认带 BOM）。

**【修复方案】** `read_if` 返回前 `text.strip_prefix('\u{feff}').unwrap_or(&text).to_string()`；或在 `Ini::parse` 开头 `let text = text.trim_start_matches('\u{feff}');`。**一行**。
**【代价】** 0。

---

### 【5-2】模块5-2 · 导入部分失败后残留新写入的文件；备份删除过早

**【严重程度】Medium**
**【位置】** `config_io.rs:795-847`

**【代码片段】**
```rust
for (name, _) in &files {
    let target = dir.join(name);
    if target.exists() {
        let old_bak = dir.join(format!("{}.bak", name));
        let _ = std::fs::remove_file(&old_bak);                  // :801 ← 先删旧备份
        let bak = dir.join(format!("{}.bak", name));
        if std::fs::rename(&target, &bak).is_ok() {              // :804 ← 再建新备份
            bak_entries.push((name.clone(), bak));
        }
    }
}
// ...
for (orig_name, bak) in &bak_entries {                            // :825 只回滚有备份的
    let _ = std::fs::rename(bak, &dir.join(orig_name));
}
```

**【问题描述】**
1. **删除早于创建**：`remove_file(old_bak)` 成功但 `rename(target, bak)` 失败 → 旧备份已删、新备份未建 → **备份丢失**；
2. **新建文件不回滚**：原本不存在、本次写入成功的文件不在 `bak_entries` 里 → 回滚时不会被删除。例：`layout_v.cfg` 不存在（首次导入竖版布局）且写成功，而 `monitor.cfg` 写失败 → 返回 false 但 `layout_v.cfg` 已被写入 → **半吊子配置**。

**【修复方案】**
- 先 `rename` 成功再 `remove_file(old_bak)`；
- 记录"本次新建的文件名"列表，回滚时一并 `remove_file`。

**【代价】** 0。

---

### 【5-3】模块5-3 · 重复 section 整体覆盖而非逐键合并

**【严重程度】Medium**
**【位置】** `core/src/config/ini.rs:72-75`
```rust
let _ = std::mem::replace(
    sections.entry(std::mem::take(&mut cur)).or_insert_with(HashMap::new),
    std::mem::take(&mut cur_map),        // ← replace：同名 section 第二次出现时**整段丢弃**前一段
);
```
**【问题描述】** 手改配置时误留两个 `[monitor]`（常见于手工复制粘贴）→ 前一个 section 的全部键丢失，静默回退默认。逐 key 覆盖（后写覆盖先写）更符合 INI 惯例和直觉。
**【修复方案】** 改为 `sections.entry(cur).or_default().extend(cur_map.drain())`。
**【代价】** 0。

---

### 【5-4】模块5-4 · 并发写同一 `.tmp` 文件

**【严重程度】Medium**（修好【1-1】后自动升为必改）
**【位置】** `config_io.rs:107-144`
```rust
let temp = dir.join(format!("{}.tmp", name));      // :113 固定名，无 pid/tid 区分
```
**【问题描述】** 主线程 `save_config`（`main.rs:709` 退出落盘 / `settings/mod.rs:477` 关窗 flush）与（修复后的）异步写线程可能并发执行 → 两个线程 `File::create` 同一个 `.tmp`、交错 `write_all`、竞争 `MoveFileExW` → 配置内容损坏或写入半截。
**【修复方案】** 全局写串行化：`static WRITE_LOCK: Mutex<()>` 包住 `write_bundle`；或临时文件名加 pid（仍需锁，因为目标文件相同）。推荐前者。
**【代价】** 一次无竞争互斥量，可忽略。

---

### 【5-5】模块5-5 · version 键损坏/缺失直接按 v1 处理，可能触发破坏性弹窗

**【严重程度】Medium**
**【位置】** `config_io.rs:226-250`
**【问题描述】** 用户把 `version = 3` 误写成 `v3` / `3.0` → `parse::<u32>()` 失败 → `None` → 视为 v1（:236）→ 与其余 v3 文件 span=2 → main.rs 弹窗"是=**用默认配置覆盖当前【所有】配置**"。一个字符的笔误导致全套设置被覆盖（虽有备份，但用户未必知道去哪找）。
**【修复方案】** 弹窗文案里列出**具体是哪个文件、读到的 version 原文是什么**；或区分"文件完全缺 version（真旧版）"与"version 存在但解析失败（疑似损坏）"，后者给出不同的提示。
**【代价】** 0。

---

### 【5-6】模块5-6 · `.bak.failed.<ts>` 无清理也无路径提示

**【严重程度】Low**
**【位置】** `config_io.rs:834-843`、`726-745`
**【问题描述】** 回滚失败时把备份改名 `.bak.failed.<ts>` 并提示"请手动恢复"，但弹窗**未给出目录路径**（用户不知道 config 目录在哪），且这些文件永不清空 → 长期堆积。
**【修复方案】** 弹窗里附上 `dir.display()`；并在成功导入时（:850）顺带清理超过 N 天的 `.bak.failed.*`。
**【代价】** 0。

---

### 【5-7】模块5-7 · 无等号行被当作 key=value

**【严重程度】Low**
**【位置】** `ini.rs:83`
```rust
} else if let Some((k, v)) = line.split_once(char::is_whitespace) {
    (k.trim().to_ascii_lowercase(), v.trim().to_string())
```
**【问题描述】** 任意含空格的文本行（如残留的说明文字）被解析成 `key=第一个词`。因未知 key 被忽略，实际无害；但会污染"用户以为写对了"的场景。
**【修复方案】** 有则保留（宽容是设计目标），仅建议在文档里注明。

---

### 【5-8】模块5-8 · 配置解析边界测试矩阵（逐项结论）

| 畸形输入 | 代码路径 | 实际行为 | 期望 | 结论 |
|---|---|---|---|---|
| 空文件 | `read_if` → 空串 → `Ini::parse` 空 | 全默认 | 全默认 | ✅ |
| 只有 `[meta]` | 同上 | 除 version 外全默认 | 同 | ✅ |
| section 名拼错 | `Ini::get` 查不到 | 该段静默忽略 | 提示更好 | ⚠️ Low |
| key 名拼错 | 同上 | 静默忽略 | 同 | ⚠️ Low |
| value 类型错误 | `parse_u32/i32/bool`（`ini.rs:34-50`）`unwrap_or(default)` | 回退默认 | ✅ | ✅ 不 panic |
| 10MB 单行 | `read_to_string` + `lines()` | 全部读入内存，不崩 | 可接受 | ✅ |
| 非法控制字符 | 无过滤 | 随值存/被忽略 | 同 | ✅ |
| **UTF-8 BOM** | `ini.rs:70` | **整文件回退默认 + 可能触发覆盖弹窗** | 应剥离 BOM | ❌ 见【5-1】 |
| UTF-16 混入 | `read_to_string` 失败 → `.ok()` = None | 视为文件不存在 → 默认 + `ensure_default_files` **覆盖写** | 可接受但静默 | ⚠️ Low |
| 缺失 version | `config_io.rs:230` | 视为 v1 → 可能触发覆盖弹窗 | 更明确提示 | ⚠️ 见【5-5】 |
| version 负数 | `parse::<u32>` 失败 → None → v1 | 同 | 同 | ⚠️ |
| version 超大 | `parse::<u32>` 溢出 → Err → None → v1 | 同 | 同 | ⚠️ |
| 重复 key | `cur_map.insert`（`ini.rs:89`） | 后者胜 | ✅ | ✅ |
| **重复 section** | `mem::replace`（`ini.rs:72`） | **整段后者覆盖前者** | 逐键合并 | ❌ 见【5-3】 |
| 循环引用 | 配置不支持引用 | N/A | N/A | ✅ |

**版本兼容**：`check_version_consistency`（:224-260）span>1 → Err → main.rs 弹窗（是=覆盖并备份；否/取消=**退出不动配置**）→ 语义正确 ✅。span==1 放行 ✅。
**导入白名单**：`mcfg::is_known_file`（`mcfg.rs:117-126`）用 `matches!` 精确匹配 5 个文件名 → **路径注入不可能** ✅。
**脱敏**：`mcfg.rs:38-59` 清 4 个使用痕迹字段，`window_pos/size` 按设计保留 ✅（若认为分辨率属隐私可再加）。

---

### 【6-1】模块6-1 · 未处理 WM_QUERYENDSESSION / WM_ENDSESSION → 关机注销丢失配置

**【严重程度】High**
**【位置】** `crates/win/src/window.rs:521-988`（wndproc match，无这两个分支）

**【问题描述】**
wndproc 未处理 `WM_QUERYENDSESSION`（0x0011）/ `WM_ENDSESSION`（0x0016），两者都落到 `DefWindowProcW`（:986）：
- `WM_QUERYENDSESSION` 返回 TRUE → 不阻止关机（正确）；
- `WM_ENDSESSION` 返回 0 且 **不会 `PostQuitMessage`** → `GetMessageW` 继续阻塞 → `run_message_loop` 不返回 → `main.rs:697-711` 的**退出落盘永不执行**。

Windows 随后强制终止进程。**结果：关机 / 注销 / 重启时，本次会话所有未同步落盘的配置改动全部丢失。**
叠加【1-1】（异步写完全失效）后，这是"改了设置 → 关机 → 白改"的确定路径。

**【修复方案】**
```rust
const WM_QUERYENDSESSION: u32 = 0x0011;
const WM_ENDSESSION:      u32 = 0x0016;
WM_QUERYENDSESSION => LRESULT(1),                       // 不阻止关机
WM_ENDSESSION => { let _ = DestroyWindow(hwnd); LRESULT(0) },   // 走 WM_DESTROY → PostQuitMessage
```
或更稳妥：在 `WM_QUERYENDSESSION` 里同步 flush 配置（此时还有约 5s 宽限）。
**【代价】** 0。
**【修复风险】** 低。需实测关机/注销/正常退出三条路径都能落盘。

---

### 【6-2】模块6-2 · 无全局停止信号，退出靠 OS 强杀

**【严重程度】Medium**
**【位置】** `main.rs:697-711`
**【问题描述】** 见【1-7】。三个工作线程均 detach 且无停止标志。`main` 返回即进程退出。
**【影响】** 当前无实际危害；但若采样线程正持有 `RING0` Mutex 或在 `save_config`（【1-2】路径）中途被杀，配置文件可能停在半写状态（虽然原子写通过 rename 保证了"要么全旧要么全新"）。
**【修复方案】** 见"三、修复优先级"P3。

---

### 【6-3】模块6-3 · 异常退出（任务管理器强杀）无保护

**【严重程度】Low**
**【问题描述】** 强杀时无最后一次落盘机会，依赖异步写。修好【1-1】后，最坏丢失 ≤500ms（防抖窗口）内的改动。可接受。
**【修复方案】** 无需额外处理（除非要求 0 丢失，那需要每次改动同步 fsync，与性能目标冲突）。

---

### 【7-1】模块7-1 · 磁盘空闲（0 B/s）显示为 `--` 而非 `0`

**【严重程度】Medium**
**【位置】** `crates/win/src/proc_disk.rs:107-108`、`139-140`

```rust
// 节流分支
return ( if self.last_read  > 0.0 { Metric::Value(self.last_read)  } else { Metric::Unavailable },
         if self.last_write > 0.0 { Metric::Value(self.last_write) } else { Metric::Unavailable } );

// 正常分支
( if read_val  > 0.0 { Metric::Value(read_val)  } else { Metric::Unavailable },
  if write_val > 0.0 { Metric::Value(write_val) } else { Metric::Unavailable } )
```
**【问题描述】** 把"数值为 0"与"数据不可用"混为一谈。磁盘真的空闲时读速为 0 是**完全合法且常见**的读数，却被渲染成 `--`（未就绪）。用户会误以为磁盘监控坏了。
**【修复方案】** 有效性应来自 PDH 本身：`PdhGetFormattedCounterValue` 的 `CStatus` 字段（`PDH_CSTATUS_VALID_DATA` / `PDH_CSTATUS_NEW_DATA`）判定，与数值大小解耦：
```rust
let valid = ret == 0 && unsafe { buf.Anonymous.doubleValue } 所在 buf.CStatus == PDH_CSTATUS_VALID_DATA;
if valid { Metric::Value(v) } else { Metric::Unavailable }
```
**【代价】** 0。

---

### 【7-2】模块7-2 · 降级策略整体结论（通过）

| 场景 | 位置 | 行为 | 结论 |
|---|---|---|---|
| 采样失败 | `sampler.rs:233/243/294/300` | `Metric::Unavailable` → 渲染 `--` | ✅ |
| 配置读取失败 | `config_io.rs:385/390-403` | 静默回退默认 | ✅（但缺日志，见下） |
| 渲染失败 | `gdi.rs:752`（ULW 失败回滚指纹）、`1123/1247`（`NEEDS_FULL_REUPLOAD`） | 下帧整窗重试 | ✅ |
| D2D 失败/设备丢失 | `gdi.rs:643-666` | 本帧走 GDI 兜底；连续 5 次或 panic → 持久回退 GDI | ✅ |
| 锁中毒 | `window.rs:299`、`render_thread.rs:114-120` | `into_inner` 恢复 | ✅（渲染资源处不一致，见【3-5】） |

**遗留**：`.ok()` / `let _ =` 大量出现，但抽查全部落在 `SetWindowPos` / `ShowWindow` / `PostMessageW` / `KillTimer` / `remove_file` 这类**无恢复路径**的调用上，属合理取舍 ✅。
**唯一建议**：配置读取失败（`read_if` 返回 None）时打一条日志，便于排障"我的配置怎么没生效"。

---

### 【8-1】模块8-1 · 二次启动"打开设置页"在第二进程里建窗，随即被 exit 销毁

**【严重程度】High**
**【位置】** `main.rs:182-186` → `window.rs:1359-1386` → `settings/mod.rs:440-460`

**【代码片段】**
```rust
// main.rs:182-186（第二进程）
if !elevated && !restart {
    let _ = aaxsyspulse_win::window::handle_second_instance();
    let _ = CloseHandle(h);
    std::process::exit(0);                    // ← 建窗之后立刻退出
}

// window.rs:1368-1379
match find_overlay_hwnd() {
    Some(hwnd) => {
        if hidden { ...ShowWindow(hwnd, SW_SHOW); ... }   // ✅ 跨进程有效
        else { crate::settings::open_settings(hwnd); }     // ❌ 在本进程建窗
    }
    ...
}

// settings/mod.rs:441-459
pub fn open_settings(overlay: HWND) {
    let existing = SETTINGS_HWND.load(Ordering::SeqCst);   // ← **本进程**的静态，= -1
    if existing != -1 { ...显示已有窗口... return; }
    create_dialog(overlay);                                 // ← 在第二进程里建设置窗
}
```

**【问题描述】**
- **悬浮窗隐藏分支有效**：`ShowWindow(hwnd, SW_SHOW)` 是跨进程 Win32 调用，作用于第一实例的窗口 ✅；
- **悬浮窗显示分支无效**：`SETTINGS_HWND` 是第二进程的模块静态（初值 -1），`open_settings` 因此走 `create_dialog`，在**第二进程**创建设置窗口；而 `create_dialog` 返回后 `main.rs:185` 立即 `process::exit(0)` → 窗口从未进入消息循环即被销毁。

**用户可见现象**：悬浮窗可见时双击 exe，什么也没发生（或设置窗一闪而逝）。

**【修复方案】** 新增自定义消息 `WM_OPEN_SETTINGS`（0x8000+6），第二进程只 `PostMessageW(overlay_hwnd, WM_OPEN_SETTINGS, ...)`，由**第一实例**的 wndproc 在自己进程内调用 `settings::open_settings`。约 10 行。
**【代价】** 0。

---

### 【8-2】模块8-2 · 全屏检测每秒 `EnumWindows` 遍历全部顶层窗口

**【严重程度】Medium**
**【位置】** `window.rs:92`（`find_overlay_hwnd` 内嵌于 `is_foreground_fullscreen`）← `window.rs:713` ← `WM_TIMER` 1s 定时器（`window.rs:704/1301`）

```rust
// window.rs:1301  1s 定时器
let _ = SetTimer(hwnd, FULLSCREEN_TIMER_ID as _, 1000, None);

// window.rs:712-713
if enabled {                                   // cfg.fullscreen_auto_hide
    let is_full = is_foreground_fullscreen();  // :92 内部 find_overlay_hwnd() → EnumWindows
```

**【问题描述】** `find_overlay_hwnd()`（`window.rs:1390-1428`）用 `EnumWindows` + 逐窗 `GetClassNameW` 查找自己的窗口。窗口数多时（浏览器多进程、IDE、多开应用）这是 UI 线程上**每秒一次**的完整顶层窗口枚举。
悬浮窗自己的 HWND 其实就在 `OverlayWindow.hwnd` 里（`window.rs:1161`），完全无需枚举。

**【影响】** 与"CPU 平均 0.03%"的目标有张力；且发生在 UI 线程，可能拖慢设置窗口的消息响应（该循环是两者共用的，见 `window.rs:1311-1327`）。

**【修复方案】** 把悬浮窗 HWND 存进模块级 `AtomicIsize`（`WM_NCCREATE` 或 `OverlayWindow::new` 里设置），`find_overlay_hwnd` 改为先查缓存、缓存无效再 EnumWindows。
**【代价】** 收益明确，改动 ~10 行。

---

### 【8-3】模块8-3 · 未处理 WM_DPICHANGED

**【严重程度】Medium（待确认）**
**【位置】** `window.rs:521-988`（无 `WM_DPICHANGED` 分支）、`window.rs:890-904`（只有 `WM_DISPLAYCHANGE`）
**【问题描述】** 高 DPI 行为取决于 `dpi::apply_dpi_awareness()`（`main.rs:156`）选择的模式。**未审 `crates/win/src/dpi.rs`**，标记待确认。
- 若为 `PerMonitorV2`：系统会自动缩放非客户区，客户区位图需自行按 DPI 缩放；本项目用软件位图（DIB）+ `UpdateLayeredWindow`，**若未按 DPI 缩放位图，跨不同 DPI 显示器拖动会模糊**。
- `WM_DISPLAYCHANGE` 已处理（清 DIB 缓存 + 失效尺寸缓存 + 重画），但那是"显示器分辨率变化"，不等于"窗口被拖到另一个 DPI 的显示器"。

**【修复建议】** 若启用 PerMonitorV2，需处理 `WM_DPICHANGED` 并按新 DPI 重建 DIB + 重排版。请先确认 `dpi.rs` 的实际模式再定。

---

### 【8-4】模块8-4 · 消息完整性抽查结论

| 项 | 结论 |
|---|---|
| `WM_POWERBROADCAST` 四事件 | ✅ `window.rs:907-947` 四个常量齐全（0x0004/0x0012/0x0007/0x8013），返回 `LRESULT(1)` 正确 |
| `WM_WTSSESSION_CHANGE` | ✅ 处理 LOCK(0x7)/UNLOCK(0x8)，`WTSRegisterSessionNotification` 已注册（:1265） |
| 定时器 ID 冲突 | ✅ `ANIM=7`（gdi.rs:70）、`CLOCK=8`（gdi.rs:75）、`FULLSCREEN=16`（window.rs:503），无冲突 |
| 未处理消息 | ✅ 全部落 `DefWindowProcW`（:986） |
| 消息循环 | ✅ `GetMessageW` 阻塞，`ret.0 <= 0` 退出并区分 0（WM_QUIT）与 -1（错误）（:1314-1318） |
| `WM_PAINT` 更新区域 | ✅ `BeginPaint/EndPaint` 配对；`hdc.is_invalid()` 时先 `ValidateRect` 再返回（:785-799） |
| 窗口样式 | ✅ `WS_EX_LAYERED \| TOOLWINDOW \| NOACTIVATE (+TOPMOST)`，`WS_POPUP\|WS_VISIBLE`；`WS_EX_TRANSPARENT` 按需切换（`apply_click_through`） |
| `WM_NCHITTEST` 交 DefWindowProc | ✅ 且 `window.rs:532-545` 有长注释论证为何不用 `HTTRANSPARENT`/`HTCAPTION`，论证正确 |
| `WM_MOUSEACTIVATE → MA_NOACTIVATE` | ✅ 不抢焦点 |
| `WM_TIMER` 对未知 ID 也请求渲染 | ⚠️ Low，当前仅 3 个 ID，无冲突 |

---

### 【9-1】模块9-1 · 脏区渲染抽查结论（通过）

| 项 | 位置 | 结论 |
|---|---|---|
| 脏矩形边界钳制 | `gdi.rs:1473-1478`（`copy_rect_to_dib`）、`gdi.rs:1157-1165`（`ulw_indirect_rect`） | ✅ 都做了 `[0,width]/[0,height]` 钳制，钳成空则跳过 → **无越界写入** |
| `pptSrc=(0,0)` | `gdi.rs:1151`、`1188` 唯一出口 | ✅ 注释（:1135-1140）记录了 2× 偏移的历史事故根因 |
| 尺寸变化 → 强制整窗 | `gdi.rs:725` + `present_size_changed`（:501） | ✅ 注释说明了"非脏区变全透明"的实测症状 |
| 连通分量聚类 + 上限折叠 | `gdi.rs:861-864`（>8 折叠为 Full） | ✅ 防系统调用风暴 |
| 邻接传播（防清 pad 擦伤邻居） | `gdi.rs:836-856` | ✅ |
| ULW 失败 → 下帧整窗恢复 | `gdi.rs:1123/1247` `NEEDS_FULL_REUPLOAD` | ✅ |
| 画布/DIB 逐帧复用 | `gdi.rs:713`（SURFACE）、`gdi.rs:947`（DIB） | ✅ 无每帧分配 |
| D2D 文本布局缓存 | `render_d2d.rs:101`（FORMAT_CACHE）、`:112`（BRUSH_CACHE）+ 启动预热（`render_thread.rs:63`） | ✅ 首帧 317ms 已提前到启动期 |
| 动画帧率与停止 | `gdi.rs:759-763` / `1460-1462`（`animating` 为 false 时 `KillTimer`） | ✅ 不常驻 |

**遗留（Low）**：
- `LAST_FRAME_HASH` 为 64 位 FNV，理论上存在碰撞即"误判内容未变跳帧"；碰撞后若内容持续不变会持续跳过。建议加"每 30s 强制 Full"兜底。
- `compute_dirty_in` 每帧 3 次小分配（`gdi.rs:814/826/836`，n≤10），可用定长数组消除，收益微小。

**待确认**：`Dirty::Rects` 为空时"跳过上屏"依赖"DIB 与窗口显示一致"这一前提。该前提在 DIB 重建时已由 `invalidate_present_size` 打破并强制 Full（已覆盖），但若**外部因素**（DWM 丢弃图层、显卡驱动重置）改变窗口像素则无检测手段。已有 `NEEDS_FULL_REUPLOAD` 兜 ULW 显式失败，但"ULW 返回成功却不生效"的场景无法覆盖。

---

### 【10-1】模块10-1 · 磁盘 PDH 计数器路径硬编码英文 → 非英文系统永久 `--`

**【严重程度】High**
**【位置】** `crates/win/src/proc_disk.rs:62`、`:71`

```rust
let read_path  = windows::core::w!("\\\\PhysicalDisk(_Total)\\\\Disk Read Bytes/sec");
let ret_r = PdhAddCounterW(query, read_path, 0, &mut rc);
if ret_r != 0 { probe::log("disk: PdhAddCounterW read failed"); }
```
**【问题描述】** PDH 计数器路径在**非英文 Windows 上会被本地化**（简体中文为 `\\PhysicalDisk(_Total)\磁盘读取字节数/秒`）。`PdhAddCounterW` 只接受本地化名称 → 在中文/日文/德文等系统上必然失败。
失败后 `rc = 0` 且 `valid = true`（只打了日志），`read_counter_value()`（:146）因 `read_counter == 0` 直接返回 0.0 → 再被 `> 0.0` 判定成 `Unavailable` → **磁盘行永久显示 `--`**，且用户与日志都看不出根因。

**【影响范围】** 面向中文用户的产品，这是**默认路径即失效**的问题。

**【修复方案】**（任选）
1. 改用 `PdhAddEnglishCounterW`（Vista+，直接接受英文路径，PDH 内部做映射）—— **改动最小，推荐**；
2. 或 `PdhLookupPerfIndexByNameW` / `PdhLookupPerfNameByIndexW` 做 索引 ↔ 本地名 转换（`PhysicalDisk` 索引 234，`Disk Read Bytes/sec` 索引 200 系）。

**【代价】** 换一个函数名，≈0。
**【修复风险】** 需在非英文系统上回归（或至少在中文系统上验证）。

---

### 【10-2】模块10-2 · 采样器退避状态被记录但从未被消费

**【严重程度】Medium**
**【位置】** `crates/win/src/sampler.rs:308-323`（`push_diag`）、`134-306`（`sample`）、`336-343`（`retry_delay_ms`）

```rust
// push_diag：记录退避
if outcome.is_failure() { self.backoff[idx].next_delay_ms(); } else { self.backoff[idx].reset(); }

// sample()：**没有任何一处**检查 backoff 来跳过采样
if plan.cpu { let cpu_out = self.cpu.sample(prev, now_ms, &mut snap); ... }

// retry_delay_ms 是占位实现
pub fn retry_delay_ms(&self, id: SamplerId) -> u32 {
    let _ = self.backoff[id as usize].failures().saturating_sub(1);
    1000 // placeholder; actual backoff tracked per-sampler above
}
```
**【问题描述】** 退避状态只被写入，从不被读取来 gate 采样。MSR/GPU/网络任一持续失败时，**每个周期都完整重试一遍失败路径**（如 MSR 的 `open_ring0()` 会走一遍 SCM 打开 + 服务查询 + CreateFile）。
`reset_backoff()`（`sampler.rs:348`，被唤醒/解锁/兜底三条路径调用）也因此只是重置一个无人读取的计数器。

**【影响】** 硬件不支持时（无管理员、无驱动、无 PDH 计数器）的持续 CPU/日志开销；实测影响待评估（退避缺失在 1.5s 周期下不一定显著）。

**【修复方案】** 在 `sample()` 各源前加：
```rust
if plan.cpu && !self.backoff[SamplerId::Cpu as usize].is_backing_off() { ...sample... }
else if plan.cpu { self.push_diag(SamplerId::Cpu, SampleOutcome::Skipped, 0); }
```
**【代价】** 0（失败路径本就该少跑）。

---

### 【10-3】模块10-3 · GPU 利用率 clamp 不防 NaN

**【严重程度】Low**
**【位置】** `crates/win/src/sampler.rs:185`、`:193`
```rust
snap.gpu = Metric::Value(u.clamp(0.0, 100.0));
```
**【问题描述】** `f32::clamp` 对 NaN 输入返回 NaN（`clamp` 只 panic 于 `min > max`）。厂商库（NVML/ADL/IGCL）异常时返回 NaN → NaN 进入快照 → 渲染出 `NaN%`。
**【修复方案】** `if u.is_finite() { Metric::Value(u.clamp(0.0,100.0)) } else { Metric::Unavailable }`。
**【代价】** 0。

---

### 【10-4】模块10-4 · 各子采样器结论

| 子项 | 位置 | 结论 |
|---|---|---|
| CPU / 内存 | `proc_cpu.rs` / `proc_mem.rs`，系统 API | ✅ 未发现明显缺陷（未逐行审） |
| GPU（D3DKMT） | `proc_gpu.rs:43 TOPO_INVALID`、`:169 PRIMARY_LUID`；唤醒时 `window.rs:925 invalidate_topology()` | ✅ 拓扑失效/重建路径完整 |
| 功耗（MSR/WinRing0） | `msr.rs` | ✅ `Ring0` Drop + `SvcHandleGuard` RAII + `RING0_STALE` 唤醒重建 + `RING0_TORN` 一次性 teardown，设计清晰 |
| 磁盘（PDH） | `proc_disk.rs` | ❌ 见【10-1】；⚠️ 0 值判定见【7-1】 |
| 网络 | `proc_net.rs:53 NET_TOPO_INVALID` + `NotifyIpInterfaceChange` 回调只置原子（零分配零锁，避免 IPHLPAPI 回调死锁） | ✅ 设计正确 |
| 温度 | — | **未找到**。`core/src/snapshot.rs` / `metrics.rs` 中无 temp 字段，`SystemSampler` 的子采样器为 cpu/mem/gpu/net/syspower/disk。**温度监控项疑似未实现**，请确认是否为预期 |
| 智能综合占用 | `sampler.rs:271-302` + `core/composite.rs` | ⚠️ 首帧 `dt_secs = 0.0`（`sampler.rs:281` `dt_ms.map(..).unwrap_or(0.0)`）→ 需确认 `Meter::update` 对 dt=0 的除零处理，**待确认** |

---

### 【11-1】模块11-1 · 性能结论

| 项 | 结论 |
|---|---|
| 空闲 CPU | ✅ UI 线程阻塞在 `GetMessageW`（`window.rs:1314`）；采样线程睡在 `wait_cfg_change`（Condvar，`main.rs:692`）→ 无忙等 |
| 采样线程忙等 | ⚠️ 挂起时 100ms 轮询（`main.rs:458`）、锁屏 5s 超时（`main.rs:495`）—— 均是有意的低频兜底，非忙等 |
| 内存增长 | ✅ `SURFACE`/`DIB` 按尺寸复用；字形图集定长（`font.rs:770`）；诊断环形缓冲定长（`sampler.rs:36` 注释 64 项无堆）→ 无持续增长 |
| 句柄稳定 | ✅ PDH/MSR/DIB 均有释放路径；⚠️ respawn 可能累积线程（【4-1】） |
| 磁盘 IO | ⚠️ 每次配置写都 `sync_all()`（`config_io.rs:119`）—— 修好【1-1】后必须保留 500ms 防抖，否则拖滑杆会高频 fsync |
| UI 线程周期开销 | ⚠️ 每秒 `EnumWindows`（【8-2】） |

---

### 【12-1】模块12-1 · CfgRef 三处同步的防线评估

**【严重程度】Medium**
**【位置】** `crates/win/src/settings/cfg_ref.rs`

**【结论：比预期安全，但测试覆盖不完整】**
- **漏改 get/set 会编译失败**：`match` 必须穷尽，所以"新增变体忘了写臂"会被编译器抓住 ✅（比注释里担心的"release 静默失败"要好）；
- **真正的风险是"语义写错"**（如 `ShowMem` 臂里写成 `cfg.show_cpu = x`），编译器抓不到，只有 roundtrip 测试能抓；
- **测试覆盖缺口**（`cfg_ref.rs:183-244`）：`ShowDiskRead`、`ShowDiskWrite`、`MetricOffset`、`MetricValueAuto` 四个变体**完全没有** roundtrip 用例；`MetricLabel/MetricValue` 只覆盖了下标 0 和 4。

**【修复方案】** 加一条"每个枚举变体至少出现一次"的元测试（可用 `strum`/手写清单比对），或把 `roundtrip_cases` 补全。
**【代价】** 0。

---

### 【12-2】模块12-2 · 其余可维护性结论

| 项 | 结论 |
|---|---|
| 配置项编号（1–7，新增从 80 起） | **待确认** —— 未审 `core/src/config/monitor.rs`。已知风险：`cfg_ref.rs` get 侧对 `metric_colors[i]` / `metric_font_sizes[i]` 是裸下标（【3-2】），未知/越界编号在 get 侧会 panic |
| 版本迁移 | ✅ v2→v3 已实现（`config_io.rs:289-373`）：备份 + 一次性标记 + 失败保留原文件可重试；v3→v4 **未实现**（当前 `CONFIG_VERSION = 3`，`core/src/config.rs:69`），当前无需 |
| 单元测试 | 有：`cfg_ref`（完整）、`ini`（3）、`mcfg`（3）、`cadence`（3）、`tasks`（5）、`config_io`（4）、`sampler`（1 个 live probe，ignored）。**缺**：电源事件、睡眠唤醒、脏区计算、渲染看门狗、CfgRef get 侧越界 |
| 文档质量 | ✅ **优于平均**：`render_thread.rs` / `gdi.rs` / `window.rs` / `config_io.rs` / `msr.rs` / `proc_net.rs` 的模块头都有"为什么 + 事故复盘日期 + 反例警示"级别的注释；`docs/` 下有大量方案与复盘文档 |

---

## 三、修复优先级清单（影响大 / 成本低 排序）

| # | 级别 | 问题 | 影响 | 成本 |
|---|---|---|---|---|
| P1 | **Critical** | 【1-1】`save_config_async` 三处 static 不通 → 异步落盘全失效 | 设置改动在强杀/崩溃/关机时全丢 | 30 行，1 文件；**须同时修【5-4】并发写 + 保留 500ms 防抖** |
| P2 | High | 【5-1】配置 BOM 未剥离 | 记事本改一次配置 → 全套回退默认，甚至被"默认覆盖"弹窗清掉 | **1 行** |
| P3 | High | 【6-1】未处理 WM_ENDSESSION | 关机/注销必丢配置（与 P1 叠加=确定路径） | 5 行 |
| P4 | High | 【8-1】二次启动"开设置"在第二进程建窗 | 悬浮窗可见时双击 exe 无反应 | 10 行 + 1 个自定义消息 |
| P5 | High | 【10-1】PDH 计数器英文硬编码 | 中文系统上磁盘行永久 `--` | 换 `PdhAddEnglishCounterW`，1 行 |
| P6 | High | 【1-3】SAMPLER_SUSPENDED 丢通知 → 采样永久冻结 | Modern Standby 下数值停更，兜底机制形同虚设 | 8 行 |
| P7 | High | 【1-4】诊断指针 UAF 窗口 | 设置页阻塞时读到已释放内存 | 改成 `Arc::into_raw/from_raw`，8 行 |
| P8 | High | 【3-1】CfgRef panic 经 wndproc FFI → abort | 任何类型错配/越界 = 进程无声消失 | wndproc 包 catch_unwind + panic 改 return false，20 行 |
| P9 | High | 【1-2】采样线程跨线程写 ThreadState（data race） | UB；且可能并发写坏配置文件 | 改走消息投递，15 行 |
| P10 | Medium | 【3-3】采样线程无 catch_unwind | 采样 panic → 数值永久冻结且无自愈 | 10 行 |
| P11 | Medium | 【4-1】respawn 无次数上限 | 反复卡死累积线程 | 3 行 |
| P12 | Medium | 【8-2】每秒 EnumWindows | UI 线程周期开销，与 0.03% 目标冲突 | 缓存 HWND，10 行 |
| P13 | Medium | 【1-5】兜底唤醒阈值误报 | 日志噪声，掩盖真实退避 | 3 行 |
| P14 | Medium | 【5-2】导入残留 + 备份删除过早 | 半吊子导入 / 备份丢失 | 10 行 |
| P15 | Medium | 【5-3】重复 section 整段覆盖 | 手改配置静默丢键 | 2 行 |
| P16 | Medium | 【7-1】磁盘 0 B/s 显示 `--` | 用户误判监控坏了 | 8 行（改用 PDH CStatus） |
| P17 | Medium | 【10-2】退避状态从未消费 | 失败源每轮重试，CPU/日志开销 | 6 处各 1 行 |
| P18 | Medium | 【3-2】CfgRef get 侧裸下标 | 越界 panic（经 P8 后降级为"改不动"） | 4 行 |
| P19 | Medium | 【1-6】标志位 Relaxed | 内存模型层面不严谨（x86 无实害，ARM64 会真出问题） | 全局替换 Ordering，0 性能代价 |
| P20 | Medium | 【2-3】ThreadState pub 接口可被非 UI 线程调用 | 契约与实现不符 | 改 pub(crate) + debug 断言线程 ID |
| P21 | Medium | 【12-1】CfgRef roundtrip 测试覆盖不全 | 4 个变体无回归保护 | 补用例 |
| P22 | Medium | 【8-3】未处理 WM_DPICHANGED | 多显示器不同 DPI 可能模糊（**待确认 dpi 模式**） | 视模式而定 |
| P23 | Low | 【3-4】【3-5】【4-3】【5-5】【5-6】【10-3】【2-2】【2-1】等 | — | 各 1–5 行 |

---

## 四、不建议修的问题

| 问题 | 为什么不修 |
|---|---|
| **【1-7】给三个工作线程加停止信号 + join** | `main` 返回后 Windows 强制回收进程，资源（句柄/GDI 对象/线程栈）全部释放，语义上等价。加 `SHUTDOWN` 原子 + join 会引入"退出时等锁/等通道"的新死锁面，收益为负。**唯一例外**：若将来线程持有必须在退出前干净释放的资源（如要卸载内核驱动服务），才值得加。目前 MSR 驱动服务设计为常驻，不需要。 |
| **【6-3】强杀时的最后一次落盘** | 修好 P1 后，最坏只丢 500ms 防抖窗口内的改动。要做到 0 丢失必须每次改动同步 fsync，与"CPU 0.03% / 无磁盘抖动"的目标直接冲突。不值得。 |
| **【9-1】"ULW 返回成功但不生效"的探测** | 要检测就得每帧读回窗口像素比对，成本远高于收益。现有 `NEEDS_FULL_REUPLOAD`（显式失败）+ `invalidate_present_size`（尺寸/DIB 变化）+ `force_full_refresh`（设置变更）三道闸门已覆盖所有已知触发路径。 |
| **【5-7】无等号行被当 key=value** | 宽容解析是明确的设计目标（`ini.rs:3-6`："宽容——未知 key 一律忽略，绝不 panic"）。改成严格会在用户手工编辑时制造更多问题。保留，写进文档即可。 |
| **【1-6】把 4 个调度字段打包成 AtomicU64** | 组合快照非原子，但下一轮（≤1.5s）自愈，且实际上已通过 `CFG_GEN` 互斥锁建立了 happens-before（见该条"缓解证据"）。改成打包会增加位域编解码的出错面。**注意：标志位本身的 Relaxed → Acquire/Release 仍建议做（P19），那个是 0 成本的**。 |
| **【11-x】`compute_dirty_in` 每帧 3 次小分配** | n ≤ 10，分配的是 10 元素的 `Vec<bool>`，且只在"有变化"的帧发生。消除它换来的是定长数组的边界假设，性价比低。 |

---

## 五、整体评价

### 健康度：**中等偏上，但有一条"静默失效"的主线贯穿配置与电源两条链路**

这个项目的工程质量明显高于同规模个人项目——模块头注释里大量的"为什么 + 事故复盘日期 + 反例警示"（`render_thread.rs`、`gdi.rs`、`msr.rs`、`proc_net.rs`）说明作者是在真实踩坑后回到代码里固化结论的，不是事后补的文档。具体到几个面：

- **做得好的**：Snapshot 用 `ArcSwap` 无锁共享（无撕裂）；MSR 句柄的 `Drop` + `SvcHandleGuard` RAII + 唤醒重建；DIB/画布/D2D 资源的逐帧复用与失效重建；脏区边界钳制；ULW 失败 → 帧指纹回滚 → 下帧整窗的自愈闭环；导入的白名单与备份回滚；配置解析全程不 panic。
- **系统性的弱点**：**存在一类"代码看起来有防护、实际从未生效"的问题**。P1（异步落盘）是典型——三个函数各写各的 `static`，从代码审查上看"写线程、pending、通知、shutdown 一应俱全"，实际一个都没接上；P6（兜底唤醒）同理——机制写在正确的位置之外，永远不会执行；P17（退避）也是——状态记了但没人读。这三处**都不是设计错误，是接线错误**，编译期和单测都抓不到（单测只测了 `DebouncedSave` 的纯逻辑，没测它与 `config_io` 的接线）。

### 最需要关注的三件事

1. **先修 P1 + P3（配置落盘链路）**。这两条叠加构成一条确定性数据丢失路径：用户改设置 → 异步写失效（P1）→ 关机不落盘（P3）→ 改动全丢。修复成本不到 40 行，是全项目性价比最高的一处。修的时候务必同时处理【5-4】的 `.tmp` 并发争用和 500ms 防抖（否则会把"丢失"换成"拖滑杆时高频 fsync"）。

2. **再修 P2 + P5（本地化与编码的两处"默认路径失效"）**。P5 意味着在中文 Windows 上磁盘监控大概率从头到尾显示 `--`；P2 意味着用户一旦用记事本改配置就可能触发"全套配置被默认覆盖"。两处加起来 2 行代码，但都是**面向中文用户的默认路径**，优先级应高于它们的行数。

3. **补上进程级 panic 兜底（P8）**。目前 `wndproc`（`window.rs:521`）与 `settings_wndproc`（`settings/mod.rs:2508`）都是 `unsafe extern "system"`，rustc 1.96 下 panic 跨 FFI 边界即 abort；而 `CfgRef::set` 有 41 个 `panic!` 臂直接挂在这条调用链上（`cfg_ref.rs:98-152`）。`main.rs:264-267` 只装了 panic hook——hook 能打印，但不能阻止 abort。建议两个 wndproc 统一包 `catch_unwind`，并把 `CfgRef` 的 panic 降级为 `return false`：**在 UI 线程里"这个设置改不动"远好过"整个进程没了"**。

---

*审计结束。所有结论均附 `文件:行号` 证据；标注"待确认"的 4 项（D2D RT 重建、DPI 模式、温度监控项、`Meter::update` 对 dt=0 的处理）需要补充阅读对应代码或实测后才能定论。*
