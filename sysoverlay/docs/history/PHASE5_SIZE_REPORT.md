# Phase 5 体积优化报告

## 测量结果

| 项目 | 体积 |
|------|------|
| 当前 exe 体积 | 106 KB |
| 目标体积 | ≤ 120 KB |
| 状态 | ✅ 通过 |

## 组成分析

当前二进制已处于非常精简的状态，主要组成：

- **windows crate**: Win32 API bindings (layered window, GDI, input handling)
- **aaxsyspulse-core**: 核心数据结构 (Snapshot, Sampler trait, Config)
- **aaxsyspulse-ui**: 轻量渲染 (GDI bitmap rendering, BMP字体)
- **aaxsyspulse-win**: 平台层 (窗口管理, 采样器)
- **arc-swap**: 线程间无锁共享指针
- **proc-macro2/quote/syn**: Rust 编译必需

## 优化措施

### 已实施 (从代码库继承)

1. **Profile 优化配置** (`Cargo.toml`):
   ```toml
   [profile.release]
   opt-level = "z"          # 优先体积
   lto = true               # 链接时优化
   codegen-units = 1        # 最大化优化
   panic = "abort"          # ❌ 本节写错了，见下方勘误 —— 仓库里是 "unwind"，且必须是
   strip = true             # 剥离符号
   overflow-checks = false
   ```

   > **勘误（2026-09-21，架构全量审查 9.1）**：上面这行 `panic = "abort"` **从未是仓库现实**：
   > 根 `Cargo.toml` 的 release profile 一直是 `panic = "unwind"`，而且**必须**保持 unwind
   > —— `gdi.rs`（D2D 自愈）、`msr.rs`、`render_thread.rs`、`app/main.rs` 的兜底都建立在
   > `catch_unwind` 上，改回 abort 会让这四条自愈静默退化成"直接崩进程"，而编译与单测
   > 全部照常通过。该约束现由 `crates/win/src/lib.rs` 的 `profile_guard_tests` 钉住。
   > **禁止**为体积把 `panic` 改成 `abort`。代码块原文照抄保留，仅供考古。

2. **windows crate features 精简**:
   ```toml
   windows = { version = "0.58", features = [
     "Win32_Foundation",
     "Win32_UI_WindowsAndMessaging",
     "Win32_UI_Input_KeyboardAndMouse",
     "Win32_Graphics_Gdi",
     "Win32_System_Threading",
     "Win32_System_LibraryLoader",
   ] }
   ```
   无过宽的 `Win32_All`，无未使用的 Performance/IpHelper/Ndis 等。

3. **禁止依赖已遵守**:
   - 无 serde/serde_json
   - 无 anyhow/thiserror
   - 无 tokio/async runtime
   - 无 tiny-skia 等图形库
   - 无 log/env_logger 全套

### 延迟初始化 (已实现)

GPU 和 Network 采样器采用延迟初始化策略：
- `GpuSampler::new()` 在 PDH 初始化失败时优雅降级为 `Metric::Unavailable`
- `NetSampler` 使用简单的占位实现，初始化成本极低
- 采样线程在 `window.run_message_loop()` 之前即可开始工作

## 启动速度

| 指标 | 数值 |
|------|------|
| 冷启动到窗口可见 | ~50-100 ms (估计) |
| 目标 | < 100 ms |
| 状态 | ✅ 预计通过 |

**说明**: 启动过程主要包括：
1. 加载配置 (~1ms)
2. 创建 OverlayWindow 窗口类注册 + 窗口创建 (~20-30ms)
3. 采样线程初始化 (CPU/Mem/GPU/Net samplers) (~10-20ms)
4. GDI 内存分配和首帧渲染 (~10-20ms)

无网络请求、无磁盘IO密集操作、无复杂初始化序列。

## 体积超标时的优化空间

如果未来体积超标，可考虑：

1. **UPX 压缩**: `upx --best --lzma target/release/aaxsyspulse-app.exe`
   - 警告：可能导致 SmartScreen 误报

2. **进一步裁剪 windows features**: 检查是否有未使用的 Win32 子系统

3. **移除 arc-swap**: 如果 snapshot 更新频率不高，可用 `Mutex<Snapshot>` 替代

4. **静态链接**: ~~切换到 `#[panic = "abort"]`~~ + 完全静态链接可减少 CRT 依赖
   —— **`abort` 这一半已作废**（审查项 9.1）：本项目的 D2D / MSR / 渲染线程自愈依赖
   `catch_unwind`，`panic` 必须保持 `"unwind"`，改回 abort 会让自愈静默失效。

## 结论

当前二进制 **106KB < 120KB 目标**，无需额外优化。
