# AAXSysPulse —— Win32 系统监控悬浮卡片

一个轻量级、高性能的 Windows 系统监控工具，以无边框半透明悬浮窗形式常驻屏幕，实时显示 **CPU / 内存 / GPU / 网络** 等核心指标。

本项目由两个程序组成（审查项 9.6 勘误：早期文档把主程序描述为"纯 C 语言 + 四个 DLL"，与仓库现实不符，已按当前代码改写）：

- **主程序 `AAXSysPulse.exe`** —— 纯 Rust，直接调用 Win32 API（窗口/GDI/D2D/PDH），不依赖 Qt、Electron、Python、.NET 或 VC++ 运行时。
- **引导程序 `AAXSysPulse-Guide.exe`** —— Tauri 2（Rust 后端 + Vue 前端），负责安装、驱动安装、14 页引导式设置与卸载。

[![Download](https://img.shields.io/badge/download-latest-blue)](https://github.com/vibecoding666/sys-overlay-c/releases/latest) [![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

---

## 核心特性

### 极致轻量化
| 指标 | 数值 |
|------|------|
| **文件体积** | ~1.0 MB（单文件，2026-09-22 实测 `target/main-dist/release/AAXSysPulse.exe` = 1,027,584 B） |
| **内存占用** | ~7 MB 私有内存（工作集 ~21 MB） |
| **CPU 占用** | ~1.56% 单核 |
| **GDI 对象** | 恒定 4 个，无泄漏 |
| **USER 对象** | 恒定 10 个 |
| **外部依赖** | 无第三方 GUI 运行时（不需要 Python / .NET / Node / Qt） |

> ⚠️ 除"文件体积"一行外，本表其余数值来自 **v1 纯 C 版本**的实测，Rust 版尚未复测，
> 请勿当作当前版本的承诺（批次 7 人工复核项）。

对比 Electron/PySide6 等方案：无需安装 Python、.NET 或 VC++ 运行时，可直接拷贝到 U 盘运行。

### 高保真渲染
渲染有两条后端，对应设置页「渲染模式」的「高画质」/「性能」两档，两条都走同一个上屏通道（画进 DIB → `UpdateLayeredWindow`）：

- **高画质（Direct2D + DirectWrite）**：`ID2D1DCRenderTarget` 通过 `BindDC` 绑到内存 DIB，DWrite 负责文字（含灰度抗锯齿）、D2D 负责填充与描边，直接写预乘 BGRA 像素（`win/src/render_d2d.rs`、`gfx_d2d.rs`）
- **性能（GDI 软位图字体）**：先用 GDI 画完全不透明卡片，再将同一圆角矩形按 2 倍尺寸画成蒙版并下采样得到边缘覆盖率，两者合成预乘 alpha，实现平滑圆角与均匀透明度；字形是 `ui` crate 内置的 8×16 点阵（三档字重由 `win/src/din_bitmap.rs` 用 DWrite 预栅格化后注入），软渲染路径零 Win32 依赖（`ui/src/renderer.rs`）
- **D2D 自愈**：D2D 设备丢失/构造失败时整会话自动回退性能模式，不崩进程（依赖 release profile 的 `panic = "unwind"`，**不可改成 `abort`**）
- **圆角 / 直角 / 纯内容** 三种外形可选
- **颜色派生系统**：背景色与前景色固定，从这两色自动派生标题、次级文字、边框、进度条轨道等角色色
- **DPI 自适应**：自动适配多显示器 DPI 差异，结合用户缩放档位（75%~200%）确保各分辨率下清晰锐利

### 智能交互
| 操作 | 效果 |
|------|------|
| **左键拖动** | 移动卡片位置，重启后自动记忆 |
| **右键点击** | 打开功能菜单 |
| **悬停模式** | 平时近乎透明，鼠标靠近时点亮，避免打扰工作 |

### 极简布局
- **单行**：最紧凑，所有勾选项在一行内显示
- **单列**：每个勾选项各占一行，适合窄屏
- **分列**：名称/数值分两列对齐显示，信息密度最高

布局是柔性框架：勾选 1~6 项都能排下，行数与列宽按勾选项数量自动推算。

---

## 数据来源

悬浮窗可勾选的是下面六项（不是"四项"，v1 文档的口径已过时）：

| 指标 | 实际数据源 | 说明 |
|------|-------------|------|
| **CPU** | `GetSystemTimes` | idle/kernel/user 时间差分，跨所有核心（`win/src/proc_cpu.rs`） |
| **内存** | `GlobalMemoryStatusEx` | 负载百分比 + 已用/总量（GB）（`win/src/proc_mem.rs`） |
| **GPU 利用率** | 厂商模块优先：NVML `nvmlDeviceGetUtilizationRates` / AMD ADL PMLog / Intel IGCL（骨架）；不可用时回退 `D3DKMTQueryStatistics` | D3DKMT 路线用各引擎 `RunningTime` 增量（WDDM 2.0+，与任务管理器同源），取所有引擎**最大值**；适配器拓扑带缓存，显示器不变时不重复枚举（`win/src/gpu_vendor.rs`、`proc_gpu.rs`） |
| **磁盘** | PDH `\PhysicalDisk(_Total)\Disk Read Bytes/sec` + `...\Disk Write Bytes/sec` | 计数路径必须是**单反斜杠**开头的路径；写成 `\\PhysicalDisk(...)` 会被 PDH 当机器名（`win/src/proc_disk.rs:20`） |
| **网络** | `GetIfEntry2` 的 `InOctets`/`OutOctets` 增量 | 默认只汇总物理网卡（ifType 6 以太网 / 71 WiFi），回环 24、隧道 131、Hyper-V 虚拟交换机 53 等按开关排除；单位可切 KB/s / MB/s / GB/s（`win/src/proc_net.rs`） |
| **功耗** | CPU：WinRing0 内核驱动 `DeviceIoControl(IOCTL_OLS_READ_MSR)` 读 RAPL 能量寄存器；GPU：厂商模块；整机 = CPU 实测 + GPU 实测 + 内存条数×5W + 主板 44W + 硬盘数×6W | **需要管理员权限且驱动能加载**（HVCI/签名限制见「已知限制」）。CPU 实测不可用 → 整行关闭显示 `--`，不做估算；GPU 功耗拿不到 → 该项记 0 继续显示（`win/src/msr.rs`、`power_system.rs`） |

> 勘误（批次 8，审查项 9.6/9.7 的延续）：本节原先把 GPU 写成 PDH `\GPU Engine(*)\Utilization Percentage`、
> 把网络写成 PDH `\Network Interface(*)\...`。两条 PDH 路径在当前代码里**已不存在**（`grep "GPU Engine"`
> 在 `sysoverlay/crates` 内 0 命中），按旧说法去排查会找不到对应的采样代码。

---

## 📦 快速开始

### 方式一：直接下载

访问 [Releases](https://github.com/vibecoding666/sys-overlay-c/releases) 页面下载：

- **`sys-overlay-c.zip`（历史产物，v1 纯 C 版）** —— 这是旧流水线打包的**上一代程序**，
  与当前仓库的 Rust 版**不是同一套代码**，功能与配置格式都不保证兼容
- 当前版本（Rust 主程序 + Tauri 引导）**尚未发布过对应产物**：本仓库的 CI 原先只是照着
  v1 的脚本与文件名在写（见审查项 9.14），批次 8 已把它改成"构建 + 静态检查 + 测试"，
  不再产出无人核验的安装包

**所以现阶段请用方式二自行编译**，两个产物分别是（路径相对仓库根，`cargo metadata` 实测
`target_directory = <仓库根>\target`）：

- `target/release/AAXSysPulse.exe` —— 主程序（bin 名来自 `crates/app/Cargo.toml` 的 `[[bin]]`，双击即运行）
- `target/release/guide-app.exe` —— 引导/安装器（crate 名即 bin 名；只有 NSIS 安装包才会把它
  重命名成 `productName` 里的 `AAXSysPulse-Guide.exe`，裸编译产物**不叫这个名字**）

> ⚠️ **注意**: `.exe` 文件不会在主仓库中展示。

### 方式二：自行编译

> 审查项 9.6 勘误：本节原先教的工具链是 v1 时代的 Zig 编译器 + 仓库根构建脚本，
> 两者已随"v1 纯 C → Rust workspace"重写一并删除，仓库里**根本不存在**。
> 以下为当前仓库实跑的步骤。

#### 环境要求

- Windows 10/11 x64
- Rust 工具链 **1.96.0 或更新**（stable，`x86_64-pc-windows-msvc`），需要 VS Build Tools 提供 `link.exe`
- Node.js 20+ 与 npm（仅编译引导程序时需要）

#### 只编译主程序

```bash
# 必须在【仓库根】执行：workspace 清单是根 Cargo.toml，
# sysoverlay/Cargo.toml 只是成员清单，在那里跑 cargo 会报"不是 workspace"。
cargo build --release -p aaxsyspulse-app
# 产物：target/release/AAXSysPulse.exe
```

#### 编译引导程序（自动同源编译主程序并打包）

```bash
cd guide-app
npm install
cargo build --release -p guide-app   # 包名见 guide-app/src-tauri/Cargo.toml
# guide-app/src-tauri/build.rs 会先在 workspace 内编译主程序
# （--target-dir target/main-dist，避免与外层 cargo 争抢构建锁），
# 再把 AAXSysPulse.exe 复制进 src-tauri/resources/ 供 bundle.resources 打包。
```

#### 开发调试（不发版、不打包）

```bash
# 前端热更新（浏览器人工复核就是看这个地址）：
cd guide-app && npm run dev
# → vite 6，监听 http://127.0.0.1:5173/（strictPort，端口被占用直接失败，不漂到 5174）

# 需要原生窗口壳时：
cd guide-app && npm run tauri dev
# → beforeDevCommand 会自动拉起上面的 vite，tauri.conf.json 的 devUrl 同为 5173
```

静态检查 / 测试（仓库根）：

```bash
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets
cargo test --workspace
cd guide-app && npm run build   # = check-bridge 断言 + vite build
```

> 💡 主程序与引导程序**只有两条通信通道**：`%APPDATA%\AAXSysPulseC\` 下的配置文件 +
> `PostMessageW` 自定义消息；号段与键名统一取自 `aaxsyspulse_core::contract`。

---

## 🎮 使用方法

启动后直接双击 `AAXSysPulse.exe`，会出现以下操作：

| 操作 | 效果 |
|------|------|
| **左键拖动** | 移动卡片位置，位置会自动记忆 |
| **右键点击** | 打开功能菜单（置顶、极简、悬停、大小、形状、颜色等） |

### 配置文件

配置目录是 `%APPDATA%\AAXSysPulseC\`（尾 `C` 是 v1 时代的历史包袱，见
`core::contract::APP_DATA_DIR_NAME`）。当前版本按职责拆成 **5 个 INI 文件**，
每份带 `[meta] version`，由主程序与引导程序读写（引导侧走 `config_patch` 扇出）：

| 文件 | 内容 | 定义处 |
|------|------|--------|
| `system.cfg` | 刷新间隔、开机自启、GDI 兼容、日志等 | `core::config::system` |
| `appearance.cfg` | 背景/前景色、字号字重、透明度、形状 | `core::config::appearance` |
| `monitor.cfg` | 监控项开关、顺序、逐项颜色与字号 | `core::config::monitor` |
| `layout_h.cfg` | 横版布局的位置与尺寸记忆 | `core::config::layout` |
| `layout_v.cfg` | 竖版布局的位置与尺寸记忆 | `core::config::layout` |

> 旧版单文件 `config.txt` 在启动时会自动迁移（迁移成功留 `config.txt.bak` 备份，
> 见 `win/src/config_io.rs`）。删除整个配置目录即可恢复所有默认设置。

---

## 🔧 功能详解

### 右键菜单功能

- **窗口置顶** - 始终保持在最上层
- **极简模式** - 紧凑布局，节省空间
  - 单行 / 单列 / 分列
- **悬停显示** - 隐藏态几乎透明，鼠标靠近时显示
- **透明度调节** - 从 1% 到 100% 自由调整
- **刷新间隔** - 100 / 200 / 1000 / 2000 / 5000 ms 五档（`core::config::INTERVAL_MS_ALLOWED`）
- **大小档位** - 75% / 100% / 125% / 150% / 200%
- **网速单位** - KB/s / MB/s / GB/s 切换
- **形状选择** - 圆角 / 直角 / 纯内容（无背景）
- **颜色定制** - 背景色 + 文字色（支持 HEX 输入）

### 技术亮点

- **无依赖渲染**：使用 GDI 绘制圆角矩形，结合 `UpdateLayeredWindow` 实现逐像素透明度
- **GPU 数据聚合**：跨进程累加引擎节点占用，取最繁忙节点作为统计口径
- **PDH 计数器优化**：避免中文本地化问题，直接使用英文计数器名称
- **DPI 感知**：自动适配多显示器 DPI 差异，避免界面模糊或错位

---

## 📊 性能表现

实测数据（i7-1260P / Windows 11 / 1 秒刷新，**v1 纯 C 版本**的实测，Rust 版未复测）：

| 指标 | 数值 |
|------|------|
| **文件大小** | ~1.0 MB（Rust 版实测；v1 为 ~75 KB） |
| **常驻内存** | 7 MB 私有内存（工作集 ~21 MB） |
| **CPU 占用** | 1.56% 单核 |
| **GDI 对象** | 恒定 4 个（无泄漏） |
| **USER 对象** | 恒定 10 个 |
| **句柄数** | 恒定 204 个 |

对比 PySide6 版本（需 Python 环境 + 641MB Qt 依赖）：

| 指标 | PySide6 版 | AAXSysPulse |
|------|-----------|--------------|
| 文件大小 | ~641 MB | **~1.0 MB** |
| 常驻内存 | ~118 MB | **~7 MB** |
| CPU 占用 | ~2.71% | **~1.56%** |

---

## 🛠️ 技术架构

一个 Cargo workspace（**清单在仓库根 `Cargo.toml`，cargo 必须从根目录跑**），
版本号为单一真源 `[workspace.package] version`，由 `guide-app/src-tauri/build.rs`
断言它与 `package.json`、`tauri.conf.json` 三处一致。

### 主程序（`sysoverlay/crates/`）

| crate | 包名 | 职责 |
|-------|------|------|
| `core` | `aaxsyspulse-core` | **零依赖**纯逻辑：配置模型与 INI 编解码（`config/`）、跨进程契约常量（`contract.rs`）、布局/配色/单位/合成/调度等无平台代码。引导程序 path 依赖它，因此不会把两个版本的 `windows` crate 拉进同一次编译 |
| `win` | `aaxsyspulse-win` | Win32 平台层：悬浮窗与消息循环、GDI 与 D2D/DirectWrite 双渲染后端（含脏矩形与失败回退）、采样（`sampler` + `proc_*`）、WinRing0 MSR 读功耗、右键菜单与设置页、DPI、托盘与自启、配置热重载（`config_io`） |
| `ui` | `aaxsyspulse-ui` | 与平台无关的绘制层：字体度量、离屏缓冲、调色板派生、渲染器接口 |
| `app` | `aaxsyspulse-app` | 可执行入口（产物 `AAXSysPulse.exe`）：单实例、启动自检（`pre_launch_checks`）、驱动安装协调、采样→提交闸门→渲染的主循环 |

### 引导程序（`guide-app/`）

- `src-tauri/`（包名 `guide-app`，产物 `AAXSysPulse-Guide.exe`）：`install.rs` 安装/卸载流程与进度、`config.rs` 的 `config_patch` 把一份 JSON 扇出到 4 个写入器（system / appearance / monitor / layout）、`driver.rs` WinRing0、`resources.rs` 从打包资源取主程序与驱动字节
- `src/wizard/`：Vue 3 + 原生 ESM，14 页向导（`composables/useGuide.js` 持状态与预设，`pages/PageXX*.vue`），通过 `bridge.js` 调 `invoke`

### 跨进程契约

主程序与引导程序之间**没有** RPC / 端口 / 共享内存，只有两条通道：
`%APPDATA%\AAXSysPulseC\` 下的配置文件，和 `PostMessageW` 自定义消息（`WM_APP+1 … WM_APP+9`）。
两者的名字都取自 `core::contract`，改一端忘另一端会在运行期静默失联（该模块的单测负责钉死派生名）。

### 关键 API

- **UI 渲染**: `CreateWindowExW`, `UpdateLayeredWindow`, `ID2D1HwndRenderTarget` + DirectWrite
- **性能采集**: `GetSystemTimes`, `GlobalMemoryStatusEx`, `PdhCollectQueryData`, `CreateFileW`\\.\WinRing0 + `DeviceIoControl`（MSR）
- **窗口管理**: `SetWindowPos`, `TrackMouseEvent`, `ChooseColorW`

---

## ⚠️ 已知限制

### 功耗依赖内核驱动；温度不作为独立监控项

> 审查项 9.7 勘误：本节原先写的是"CPU 封装温度（MSR/PECI）需要内核级驱动支持，
> 用户态无法读取，推荐 LibreHardwareMonitor / WMI / HTTP" —— 那与实际实现**直接矛盾**：
> 本软件自己就在装一个内核驱动来读 MSR。现按 `win/src/msr.rs` 的真实行为改写。

- **实测功耗（RAPL）走 WinRing0**：安装期把 `WinRing0x64.sys` 放到 `C:\Windows\AAXSysPulse\`
  并注册服务 `AAXSysPulseWinRing0`，运行期打开设备 `\\.\WinRing0_1_2_0`，用 `DeviceIoControl`
  读 RAPL MSR。**需要管理员权限**，且受 **驱动签名 / HVCI（内核隔离）** 约束 ——
  不满足时装不上或打不开设备。
- **失败不谎报**：驱动任一环节失败 ⇒ 本会话放弃 MSR 路线并留日志（`probe::degrade`），
  功耗行显示 `--` 并转灰/失鲜色；整机功耗的其余部分是按 CPU/GPU 实测 + 内存/主板/硬盘估算得来，
  不是量出来的真值。
- **温度**：本程序**没有独立的"温度"监控项**（引导向导第 4 页的"宿舍"预设因此用 GPU 占位）。
  封装温度理论上同一 MSR 通道可读，但未接线，属未实现功能而非"读不到"。
- **安全披露**：WinRing0 系驱动给**本机所有本地用户**开放 MSR 读能力，
  装了它在本机等同于一个已知攻击面。是否保留该驱动、是否换自建签名驱动，属未决事项
  （审查项 8B，等待用户决策）。

### 不显示频率

Windows 性能计数器返回的频率值恒定为标称频率，不会随负载变化，因此不在此处展示。

---

## 🤝 贡献与反馈

欢迎提交 Issue 和 Pull Request！如有功能建议、Bug 报告或优化想法，请在 [Issues](https://github.com/vibecoding666/sys-overlay-c/issues) 中提出。

---

## 📜 许可证

本项目采用 [MIT License](LICENSE) 开源。

```text
Copyright (c) 2026 vibecoding666

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction...
```

---

## 🙏 致谢

- v1（纯 C + Zig 单文件）版本的探索，为现在的 Rust 重写定了型
- 参考了 [sys-overlay](../sys-overlay) Python 版本的功能设计
- Windows Performance Counters (PDH) API 是数据采集的核心
- [WinRing0](http://libsys.org/)（OpenLibSys）提供了读取 RAPL MSR 的内核驱动通路

---

**最后更新时间**: 2026-09-21（批次 7：按仓库现实改写构建/架构/驱动三节）
