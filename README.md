# SysOverlay-C —— Win32 系统监控悬浮卡片（纯 C 实现）

一个轻量级、高性能的 Windows 系统监控工具，以无边框半透明悬浮窗形式常驻屏幕，实时显示 **CPU / 内存 / GPU / 网络** 使用情况。

[![Download](https://img.shields.io/badge/download-latest-blue)](https://github.com/vibecoding666/sys-overlay-c/releases/latest) [![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

---

## 🚀 特性

- ✅ **极致轻量化**：仅约 70KB，常驻内存约 7MB
- ✅ **无需依赖**：独立可执行文件，无需 Python、.NET 或 VC++ 运行时
- ✅ **高清渲染**：支持圆角、透明度、自定义配色
- ✅ **智能交互**：拖动定位、右键菜单、置顶显示、悬停点亮
- ✅ **极简模式**：单行 / 单列 / 分列三种紧凑布局
- ✅ **DPI 自适应**：完美适配高分屏、多显示器

---

## 📦 快速开始

### 方式一：直接下载（推荐）

访问 [Releases](https://github.com/vibecoding666/sys-overlay-c/releases) 页面，下载最新的发布包：

- **sys-overlay-c.zip** - 完整版包含 exe 文件和配置文件示例

解压后双击 `sysoverlay-c.exe` 即可运行！

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

编译完成后，会在根目录生成 `sysoverlay-c.exe`。

> 💡 **提示**：编译脚本默认使用 `E:\dev\zig\zig.exe`，如需更改请编辑 `build.bat`。

---

## 🎮 使用方法

启动后直接双击 `sysoverlay-c.exe`，会出现以下操作：

| 操作 | 效果 |
|------|------|
| **左键拖动** | 移动卡片位置，位置会自动记忆 |
| **右键点击** | 打开功能菜单（置顶、极简、悬停、大小、形状、颜色等） |

### 配置文件

配置保存在 `%APPDATA%\SysOverlayC\config.txt`，格式如下：

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

| 指标 | PySide6 版 | SysOverlay-C |
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
