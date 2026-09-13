# AAXSysPulse —— Win32 系统监控悬浮卡片

一个轻量级、高性能的 Windows 系统监控工具，以无边框半透明悬浮窗形式常驻屏幕右上角，实时显示 **CPU / 内存 / GPU / 网络** 四大核心指标。纯 C 语言编写，仅依赖四个 Windows 原生 DLL，无任何运行时依赖。

[![Download](https://img.shields.io/badge/download-latest-blue)](https://github.com/vibecoding666/sys-overlay-c/releases/latest) [![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

---

## 核心特性

### 极致轻量化
| 指标 | 数值 |
|------|------|
| **文件体积** | ~75 KB（单文件） |
| **内存占用** | ~7 MB 私有内存（工作集 ~21 MB） |
| **CPU 占用** | ~1.56% 单核 |
| **GDI 对象** | 恒定 4 个，无泄漏 |
| **USER 对象** | 恒定 10 个 |
| **外部依赖** | 仅 `user32.dll / gdi32.dll / pdh.dll / advapi32.dll` |

对比 Electron/PySide6 等方案：无需安装 Python、.NET 或 VC++ 运行时，可直接拷贝到 U 盘运行。

### 高保真渲染
- **GDI 逐像素透明度**：先用 GDI 画完全不透明卡片，再将同一圆角矩形按 2 倍尺寸画成蒙版并下采样得到边缘覆盖率，两者合成预乘 alpha 交给 `UpdateLayeredWindow`，实现平滑圆角与均匀透明度
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
- **单行**：最紧凑，所有指标在一行内显示
- **单列**：四项指标各占一行，适合窄屏
- **分列**：名称/数值分两列对齐显示，信息密度最高

---

## 数据来源

| 指标 | Windows API | 说明 |
|------|-------------|------|
| **CPU** | `GetSystemTimes` | 通过 idle/kernel/user 时间差分计算，跨所有核心 |
| **内存** | `GlobalMemoryStatusEx` | 内存负载百分比 + 已用/总量（GB） |
| **GPU** | PDH `\GPU Engine(*)\Utilization Percentage` | 按引擎节点（`luid_*`）跨进程累加，取最繁忙节点 |
| **网络** | PDH `\Network Interface(*)\Bytes Received/Sent/sec` | 所有网卡流量求和，单位可切 KB/s / MB/s / GB/s |

> GPU 引擎利用率的特殊性：每个实例是"某进程占某引擎节点的比例"，需去掉 `pid_` 前缀按节点分组累加，再取最繁忙节点作为统计口径。按 `engtype` 平均会被空闲节点稀释成零头。

---

## 📦 快速开始

### 方式一：直接下载（推荐）

访问 [Releases](https://github.com/vibecoding666/sys-overlay-c/releases) 页面，下载最新的发布包：

- **sys-overlay-c.zip** - 完整版包含 exe 文件和配置文件示例

解压后双击 `AAXSysPulse.exe` 即可运行！

> ⚠️ **注意**: `.exe` 文件不会在主仓库中展示，请通过 Releases 页面下载。

### 方式二：自行编译

#### 环境要求

- Windows 10/11 (x64)
- Zig 编译器 0.11+ ([下载地址](https://ziglang.org/download/))

#### 编译步骤

```bash
# 1. 下载 Zig（任意目录，绿色免安装）
# https://ziglang.org/download/

# 2. 设置 Zig 路径（示例）
set ZIG=E:\dev\zig\zig.exe

# 3. 执行构建
build.bat
```

编译完成后，会在根目录生成 `AAXSysPulse.exe`。

> 💡 **提示**：编译脚本默认使用 `E:\dev\zig\zig.exe`，如需更改请编辑 `build.bat`。

---

## 🎮 使用方法

启动后直接双击 `AAXSysPulse.exe`，会出现以下操作：

| 操作 | 效果 |
|------|------|
| **左键拖动** | 移动卡片位置，位置会自动记忆 |
| **右键点击** | 打开功能菜单（置顶、极简、悬停、大小、形状、颜色等） |

### 配置文件

配置保存在 `%APPDATA%\AAXSysPulse\config.txt`，格式如下：

```txt
minimal          # 极简模式 (0=关闭，1=开启)
zoom             # 缩放比例 (100, 125, 150, 200)
hover            # 悬停显示 (0=关闭，1=开启)
netunit          # 网速单位 (KB/MB/GB)
shape            # 形状 (round/square/content)
vert             # 极简排列 (0=单行，1=单列，2=分列)
bg               # 背景色 (HEX 颜色)
fg               # 前景色/文字色 (HEX 颜色)
```

删除该文件即可恢复所有默认设置。

---

## 🔧 功能详解

### 右键菜单功能

- **窗口置顶** - 始终保持在最上层
- **极简模式** - 紧凑布局，节省空间
  - 单行 / 单列 / 分列
- **悬停显示** - 隐藏态几乎透明，鼠标靠近时显示
- **透明度调节** - 从 1% 到 100% 自由调整
- **刷新间隔** - 1s ~ 5s 可选
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

实测数据（i7-1260P / Windows 11 / 1 秒刷新）：

| 指标 | 数值 |
|------|------|
| **文件大小** | ~75 KB |
| **常驻内存** | 7 MB 私有内存（工作集 ~21 MB） |
| **CPU 占用** | 1.56% 单核 |
| **GDI 对象** | 恒定 4 个（无泄漏） |
| **USER 对象** | 恒定 10 个 |
| **句柄数** | 恒定 204 个 |

对比 PySide6 版本（需 Python 环境 + 641MB Qt 依赖）：

| 指标 | PySide6 版 | AAXSysPulse |
|------|-----------|--------------|
| 文件大小 | ~641 MB | **~75 KB** |
| 常驻内存 | ~118 MB | **~7 MB** |
| CPU 占用 | ~2.71% | **~1.56%** |

---

## 🛠️ 技术架构

### 核心模块

- `overlay.c` - 主程序入口，包含：
  - 分层窗口创建与管理
  - GDI 绘图与透明合成
  - PDH 性能计数器采集
  - 右键菜单与配置管理

### 关键 API

- **UI 渲染**: `CreateWindowEx`, `UpdateLayeredWindow`, `Bitblt`
- **性能采集**: `GetSystemTimes`, `GlobalMemoryStatusEx`, `PdhCollectQueryData`
- **窗口管理**: `SetWindowPos`, `TrackMouseEvent`, `ChooseColorW`

---

## ⚠️ 已知限制

### 不显示温度

CPU 封装温度（MSR/PECI）需要内核级驱动支持，用户态无法读取。推荐方案：

- 使用 [LibreHardwareMonitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor) 作为后台服务
- 通过 WMI/HTTP 接口读取温度数据

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

- 感谢 [Zig](https://ziglang.org/) 提供出色的 C 语言编译器
- 参考了 [sys-overlay](../sys-overlay) Python 版本的功能设计
- Windows Performance Counters (PDH) API 是数据采集的核心

---

**最后更新时间**: 2026-09-03
