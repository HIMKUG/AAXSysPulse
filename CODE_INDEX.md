# CODE_INDEX —— AAXSysPulse 代码导航索引

> 用途：让 AI / 协作者在不通读全仓的前提下，快速定位「某功能由哪些代码实现」。
> 本文件是**机器可读的代码地图**，与面向人的 `README.md` 互补（README 讲「是什么/怎么用」，本文件讲「在哪/怎么找」）。
> 最后核对：2026-11-04，基于仓库当前 Rust 主程序 + Tauri 引导程序的真实代码。

---

## 0. 一句话定位

| 项 | 值 |
|----|----|
| 产品 | AAXSysPulse —— Windows 无边框半透明系统监控悬浮窗（CPU/内存/GPU/网络/磁盘/功耗/时间/综合占用） |
| 主程序 | `AAXSysPulse.exe`（纯 Rust，cargo bin，零 GUI 运行时依赖） |
| 引导程序 | `AAXSysPulse-Guide.exe`（Tauri 2：Rust 后端 + Vue 3 前端，负责安装/驱动/14 页向导/卸载） |
| 构建系统 | 单 Cargo workspace（清单在**仓库根 `Cargo.toml`**，cargo 必须从根目录跑） |
| 版本真源 | `[workspace.package] version = "0.7.1"`，由 `guide-app/src-tauri/build.rs` 断言与 `package.json`/`tauri.conf.json` 一致 |

### 产物位置（相对仓库根）
- `target/release/AAXSysPulse.exe` —— 主程序（`-p aaxsyspulse-app`，日常开发态）；`target/main-dist/release/` —— 引导打包时的同源产物（审查 847：两条构建路径并存）
- `target/release/guide-app.exe` —— 引导程序（`-p guide-app`；只有 NSIS 打包后才叫 `AAXSysPulse-Guide.exe`）
- 引导程序 `build.rs` 会先在 `target/main-dist` 同源编译主程序，再作为资源打包进引导。

---

## 1. 双进程架构 & 唯一通信通道（找「跨程序交互」代码前必读）

主程序与引导程序是**两个独立进程**，之间**只有两条通道**，改一端忘另一端会在运行期静默失联：

1. **磁盘 INI 文件**：`%APPDATA%\AAXSysPulseC\config\` 下 5 个 `.cfg`（见 §4）
2. **`PostMessageW` 自定义消息** + 窗口类名，号段全部定义在 `sysoverlay/crates/core/src/contract.rs`

> ⚠️ 所有跨进程名字（exe 名、服务名、Run 键值名、消息号、目录名）的唯一真源就是 `core/src/contract.rs`。
> 新增/修改任何跨进程名字，**只改这一个文件**，不要写字面量副本（单测会钉死派生关系）。

---

## 2. 按功能查代码（最常用的索引）

> 格式：`功能 → 文件[:关键符号]`（符号指向最该看的函数/结构体）

### 2.1 指标采集（数据来源）
| 功能 | 文件 | 真实数据源 |
|------|------|-----------|
| CPU 占用 | `sysoverlay/crates/win/src/proc_cpu.rs` | `GetSystemTimes` 差分 |
| 内存占用 | `sysoverlay/crates/win/src/proc_mem.rs` | `GlobalMemoryStatusEx` |
| 磁盘读写 | `sysoverlay/crates/win/src/proc_disk.rs` | PDH `\PhysicalDisk(_Total)\...`（单反斜杠，审查 288） |
| 网络速率 | `sysoverlay/crates/win/src/proc_net.rs` | `GetIfEntry2` 增量（物理 NIC 汇总） |
| GPU 利用率（通用） | `sysoverlay/crates/win/src/proc_gpu.rs` | `D3DKMTQueryStatistics` RunningTime 增量 |
| GPU 厂商后端（N/A/I） | `sysoverlay/crates/win/src/gpu_vendor.rs` | NVML / ADL-PMLog / IGCL（骨架） |
| GPU 独显检测 | `sysoverlay/crates/win/src/gpu_detect.rs` | DXGI 适配器枚举 |
| CPU 实测功耗（RAPL） | `sysoverlay/crates/win/src/msr.rs` | WinRing0 驱动 `DeviceIoControl` 读 MSR |
| 整机功耗合成 | `sysoverlay/crates/win/src/power_system.rs` | CPU实测+GPU实测+内存/主板/硬盘估算 |
| 综合占用 Util | `sysoverlay/crates/core/src/composite.rs` | 排队论延迟因子模型 |
| 采样器编排/退避 | `sysoverlay/crates/win/src/sampler.rs`（`SystemSampler`） | 组装 `Snapshot` + 失败退避 |
| 采样 trait / 诊断缓冲 | `sysoverlay/crates/core/src/sampler.rs` | `Sampler` trait + 环形诊断缓冲 |

### 2.2 渲染
| 功能 | 文件 |
|------|------|
| 性能模式（GDI 软渲染 + DIB 上屏 + 脏检查） | `sysoverlay/crates/win/src/gdi.rs` |
| 高画质模式（Direct2D + DirectWrite） | `sysoverlay/crates/win/src/render_d2d.rs` + `gfx_d2d.rs` |
| 渲染线程（搬出 UI 循环，含看门狗） | `sysoverlay/crates/win/src/render_thread.rs` |
| D2D 设备丢失自愈 + 看门狗 | `gdi.rs::render_watchdog_tick` / `disable_d2d_and_persist` |
| 离屏软件光栅化（零 Win32） | `sysoverlay/crates/ui/src/{render,offscreen,font,renderer,palette,clock}.rs` |
| 性能模式三档字重（DIN 点阵注入） | `sysoverlay/crates/win/src/din_bitmap.rs` |
| 渲染器选择（横/竖） | `ui/src/renderer.rs`（`StripRenderer` / `VerticalRenderer`） |

### 2.3 窗口 / 交互 / 系统
| 功能 | 文件 |
|------|------|
| 悬浮窗窗口 + 消息循环 | `sysoverlay/crates/win/src/window.rs`（`OverlayWindow`，GWLP_USERDATA 存 `ThreadState`） |
| 右键托盘菜单 | `sysoverlay/crates/win/src/menu.rs` |
| 托盘图标 | `sysoverlay/crates/win/src/tray.rs` |
| 设置面板（外壳/数据流/全量 D2D 绘制） | `sysoverlay/crates/win/src/settings/mod.rs` |
| 设置控件 ↔ Config 唯一映射 | `sysoverlay/crates/win/src/settings/cfg_ref.rs`（`CfgRef`，全项目唯一映射表） |
| 自绘控件（弹簧过渡） | `sysoverlay/crates/win/src/settings/widget.rs` |
| 设置页布局/间距刻度 | `sysoverlay/crates/win/src/settings/layout.rs` |
| 设置页状态机/控件树 | `sysoverlay/crates/win/src/settings/state.rs` |
| 设置页滚动条（坐标唯一真值） | `sysoverlay/crates/win/src/settings/scrollbar.rs` |
| 设置页弹簧动效 | `sysoverlay/crates/win/src/settings/motion.rs` |
| 设置页图标（SVG→D2D） | `sysoverlay/crates/win/src/settings/icons.rs` |
| Fluent 设计 token | `sysoverlay/crates/win/src/settings/fluent/{mod,tokens}.rs` |
| 防抖落盘任务 | `sysoverlay/crates/win/src/settings/tasks.rs`（`DebouncedSave`，Drop 强制 flush） |
| 自绘标题栏（图表/进程窗口） | `sysoverlay/crates/win/src/titlebar.rs` |
| DPI 感知 | `sysoverlay/crates/win/src/dpi.rs` |
| 开机自启（Run 键 + StartupApproved 双向保障） | `sysoverlay/crates/win/src/autostart.rs` |
| 管理员提权（runas 重启） | `sysoverlay/crates/win/src/elevate.rs` |
| 托盘「快速重启」 | `sysoverlay/crates/win/src/restart.rs` |
| 内存主动适配（工作集修剪） | `sysoverlay/crates/win/src/mem_tune.rs` |
| 双语文案 | `sysoverlay/crates/win/src/i18n.rs` |
| Apple 风格主题（明暗） | `sysoverlay/crates/win/src/theme.rs` |
| 应用图标（内嵌 ICO） | `sysoverlay/crates/win/src/app_icon.rs` |
| 崩溃 minidump | `sysoverlay/crates/app/src/main.rs::setup_panic_hook` |
| 启动前检查（驱动/权限） | `sysoverlay/crates/app/src/pre_launch_checks.rs` |
| 主程序入口 / 采样主循环 | `sysoverlay/crates/app/src/main.rs::main` |

### 2.4 配置 / 数据模型（core crate，纯逻辑零依赖）
| 功能 | 文件 |
|------|------|
| 配置模型 + 类型（Config 主体） | `sysoverlay/crates/core/src/config.rs` |
| 领域枚举唯一真源（Zoom/VertLayout/Metric…；审查 281：Shape 已移除） | `sysoverlay/crates/core/src/types.rs` |
| 5 个 .cfg 的 INI 读写 | `core/src/config/{system,appearance,monitor,layout}.rs` + `core/src/config/ini.rs` |
| 旧版 `config.txt` 迁移 | `core/src/config/legacy.rs` |
| `.mcfg` 分享/备份容器 | `core/src/config/mcfg.rs` |
| 跨进程契约常量/消息号 | `core/src/contract.rs` |
| 快照（单次采样结果） | `core/src/snapshot.rs` |
| 指标纯函数计算 | `core/src/metrics.rs` |
| 自适应刷新调度器 | `core/src/scheduler.rs` + `core/src/cadence.rs`（节拍常量/EMA；审查 285：死区/Idle-Active 状态机在 app/main.rs，不在本 crate） |
| 布局计算（不吃整个 Config） | `core/src/layout.rs` |
| 颜色派生（纯函数） | `core/src/color.rs` + `ui/src/palette.rs` |
| 数值格式化（栈上定长，零分配） | `core/src/units.rs` |
| 历史时序环形缓冲 | `core/src/history.rs`（逻辑） + `win/src/history.rs`（共享，ArcSwap） |

### 2.5 引导程序（guide-app，Tauri）
| 功能 | 文件 |
|------|------|
| 引导入口 / 所有 Tauri 命令 | `guide-app/src-tauri/src/lib.rs`（`@tauri::command` 全在这里） |
| 安装流程 + 进度事件 | `guide-app/src-tauri/src/install.rs`（`run_install`/`run_uninstall`） |
| 驱动安装细分步骤 | `guide-app/src-tauri/src/driver.rs` |
| 从打包资源取主程序/驱动字节 | `guide-app/src-tauri/src/resources.rs` |
| 配置扇出 `config_patch` | `guide-app/src-tauri/src/config.rs`（JSON→4 个写器） |
| 断点续引导状态 | `guide-app/src-tauri/src/guide_state.rs` |
| 通知主程序重载/重启 | `guide-app/src-tauri/src/notify.rs`（`PostMessageW`） |
| 主程序进程检测/启动 | `guide-app/src-tauri/src/process.rs` |
| 桌面/开始菜单快捷方式 | `guide-app/src-tauri/src/shortcut.rs` |
| 前端 14 页向导 | `guide-app/src/wizard/pages/Page01..14*.vue` |
| 向导状态/预设 | `guide-app/src/wizard/composables/useGuide.js` |
| 前端↔后端桥接 | `guide-app/src/wizard/bridge.js`（`invoke`） |

---

## 3. Crate / 模块清单（文件 → 一句话职责）

### `sysoverlay/crates/core`（纯逻辑层：零平台依赖、零 unsafe）
- `lib.rs` —— crate 说明 + 模块依赖方向图
- `types.rs` —— 领域枚举唯一真源（Zoom/VertLayout/Metric/RenderEngine…；审查 281：Shape 已移除）
- `contract.rs` —— **跨进程契约唯一真源**（消息号/窗口类/系统侧名字 + `wide_of`）
- `snapshot.rs` —— `Snapshot` 不可变采样结果
- `sampler.rs` —— `Sampler` trait + 诊断环形缓冲
- `cadence.rs` —— 智能调度节拍常量（REFRESH_FLOOR_MS/DELTA_ENTER…）
- `scheduler.rs` —— 自适应刷新调度器（EMA/分频；审查 285：死区与 Idle-Active 状态机在 app/main.rs）
- `metrics.rs` —— 指标纯函数（cpu_usage 等，可单测）
- `composite.rs` —— 综合占用 Util 模型（排队论，零堆零 panic）
- `layout.rs` —— 卡片布局计算（吃窄参数 `LayoutInput`）
- `config.rs` —— 配置模型 + 宽容读/保守写策略
- `config/ini.rs` —— 通用 INI `[section] key=value` 读写
- `config/{system,appearance,monitor,layout}.rs` —— 4 个 .cfg 的序列化/解析
- `config/legacy.rs` —— 旧版 `config.txt` 迁移
- `config/mcfg.rs` —— `.mcfg` 分享容器（导出脱敏；仅含当前激活布局，另一布局由导入方自动补齐 —— 审查 286）
- `color.rs` / `units.rs` / `history.rs` / `power.rs` —— 颜色派生 / 数值格式化 / 历史环 / 功耗口径纯函数

### `sysoverlay/crates/ui`（离屏软件光栅化，零 Win32、零 unsafe）
- `render.rs` —— 渲染抽象（`Rect`/`Rgba`/`RenderTarget`/`FontSpec`）
- `offscreen.rs` —— `SoftwareSurface`（BGRA 预乘，对齐 DIB）
- `font.rs` —— 8×16 ASCII 点阵字体（占位，Phase 3 接 GDI）
- `palette.rs` —— `Style`：core 调色板 → 预乘色
- `renderer.rs` —— `Renderer` trait + `StripRenderer`/`VerticalRenderer` + 脏检查 `frame_hash`
- `clock.rs` —— 本地时钟源（AtomicU32 时分秒）
- `renderer/tests.rs` —— renderer 单测（独立分文件）

### `sysoverlay/crates/win`（Win32 平台层，主程序）
- 见 §2 各表；模块全清单见 `win/src/lib.rs`（`pub mod …` 共 33 个）
- `examples/balloon_probe.rs` —— 开发探针（验证托盘气泡），不进发布物
- `win/src/lib.rs::profile_guard_tests` —— **断言 `[profile.release] panic="unwind"`**（D2D/MSR/渲染线程自愈依赖 catch_unwind，改 abort 会静默废掉自愈）

### `sysoverlay/crates/app`（主程序 bin 入口）
- `src/main.rs` —— 入口 + 单实例 + 采样主循环（**全部逻辑在此，lib.rs 刻意空**）
- `src/pre_launch_checks.rs` —— 启动前权限/驱动检查对话框
- `src/driver_installer.rs` —— ⚠️ **死代码**（未被任何 `mod` 声明，不编译；真驱动安装在引导侧）
- `build.rs` —— 仅把应用图标编进 exe 资源

### `guide-app`（Tauri 2 引导程序）
- `src-tauri/` —— Rust 后端（见 §2.5）
- `src/wizard/` —— Vue 3 前端（14 页 + `useGuide.js` 状态 + `bridge.js`）

---

## 4. 配置文件映射（%APPDATA%\AAXSysPulseC\config\）

| 文件 | 内容 | 定义处 |
|------|------|--------|
| `system.cfg` | 刷新间隔/自启/GDI 兼容/日志/布局锚点 `layout`/`x,y,w,h` | `core/src/config/system.rs` |
| `appearance.cfg` | 背景/前景色、字号字重、阴影描边（审查 282：透明度在 system.cfg、形状键已废弃） | `core/src/config/appearance.rs` |
| `monitor.cfg` | 监控项开关/逐项颜色字号（审查 283：order 在 layout 文件，不在 monitor） | `core/src/config/monitor.rs` |
| `layout_h.cfg` | 横版布局专属（逐指标偏移+order；审查 284：窗口几何唯一归属 system.cfg） | `core/src/config/layout.rs` |
| `layout_v.cfg` | 竖版布局专属 | `core/src/config/layout.rs` |
| `config.txt`（旧） | v2 单文件，启动时自动迁移为上述 5 个（留 `.bak`） | `core/src/config/legacy.rs` |

持久化/原子写：`win/src/config_io.rs`（写临时 `.tmp` → `MoveFileExW` 同卷替换）。

---

## 5. 工具脚本（仓库根 `tools/` 与 `sysoverlay/tools/`）

> 这些 `_*.py` / `*.ps1` 是**开发期/重构辅助脚本**（拆分 wndproc、修 brace 平衡、生成 cfg_ref、i18n 校验、截图/性能探针等），**不参与运行时、不进发布物**。改业务逻辑时无需看它们；它们是历史重构的副产品，部分已过时（如 `_split_*` 系列）。

---

## 6. AI 改代码前必看的「雷区」（避免静默破坏）

1. **`[profile.release] panic` 必须保持 `"unwind"`**（`Cargo.toml` 根）。改成 `abort` 会让 D2D/MSR/渲染线程的自愈（靠 `catch_unwind`）静默退化为「直接崩进程」，且编译与单测全过——由 `win/src/lib.rs::profile_guard_tests` 守护。
2. **跨进程名字只在 `core/src/contract.rs` 改**。任何 `w!("字面量")` 副本都会漂移；调用点用 `wide_of()` 运行时转。单测钉死派生关系。
3. **引导只投递 `WM_RELOAD_CONFIG`(0x8008) / `WM_QUICK_RESTART`(0x8009)**，改号两端契约单测会红。
4. **PDH 磁盘路径必须是单反斜杠**（`\PhysicalDisk(_Total)\...`），`proc_disk.rs` 头注释与 `DISK_READ_PATH` 常量明确（审查 288：路径在 :28-29、说明在 :16-27）。
5. **GPU 利用率**：当前代码**没有** `GPU Engine(*)\Utilization Percentage` 的 PDH 路径（README 旧文档已勘误），找采样代码看 `proc_gpu.rs` 的 `D3DKMTQueryStatistics` RunningTime 法。
6. **温度不是独立监控项**：本程序未接线（封装温度理论上同 MSR 通道可读，但未实现）；功耗依赖 WinRing0 内核驱动，需管理员+驱动可加载。
7. **设置控件读写必须走 `settings/cfg_ref.rs` 的 `CfgRef`**，不要散写 match（旧实现因此出过「显示对、写不回」的静默 bug）。
8. **`config_io` 写 `version` 的位置**：`system.cfg` 的版本写在 `[system]` 段，不是 `[meta]`（引导侧 `config.rs` 注释明示，否则主程序判为 v1 触发破坏性弹窗）。
9. **cargo 必须从仓库根跑**（workspace 清单在根）；`sysoverlay/Cargo.toml` 只是成员清单。
10. **UI 线程与采样线程模型**：设置窗口/模态框期间悬浮窗数值停更（像素重绘在渲染线程不受影响）。渲染看门狗在采样线程内，UI 线程被饿死不在自愈范围。

---

## 7. 快速检索速查（关键词 → 文件）

- 「CPU/GPU/内存/磁盘/网络 怎么采的」→ `win/src/proc_*.rs`
- 「功耗/温度/驱动」→ `win/src/msr.rs` + `win/src/power_system.rs`
- 「怎么画的/渲染」→ `win/src/gdi.rs` / `win/src/render_d2d.rs` / `ui/src/renderer.rs`
- 「设置页长啥样/怎么改配置项」→ `win/src/settings/*`（先看 `mod.rs` 架构图 + `cfg_ref.rs`）
- 「配置存在哪/什么格式」→ `core/src/config/*` + `win/src/config_io.rs`
- 「两个 exe 怎么通信」→ `core/src/contract.rs` + `guide-app/.../notify.rs` + `win/src/window.rs` 的 WM_* 分支
- 「引导程序怎么装/卸/装驱动」→ `guide-app/src-tauri/src/{install,driver,resources,shortcut}.rs`
- 「前端向导 14 页」→ `guide-app/src/wizard/pages/PageXX*.vue`
- 「双渲染后端自愈/看门狗」→ `win/src/gdi.rs::render_watchdog_tick` + `win/src/render_thread.rs`
- 「为什么这么轻量/零分配」→ `core/src/units.rs`（手写定点数替代 format!）+ `core/src/composite.rs`
