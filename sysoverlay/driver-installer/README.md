# 驱动安装程序（Driver Installer）

一个很小的 Windows 图形界面小工具，用于为 AAXSysPulse 安装 `WinRing0x64.sys` 驱动。
驱动文件**内嵌在可执行文件内部**，因此本程序是一个单独的 `.exe`，不依赖任何外部文件。

## 功能说明

1. **请求管理员权限** —— 内嵌的清单（manifest）会申请管理员权限，因此启动时 Windows 会弹出
   UAC 提权提示（运行时也有兜底逻辑：若未提权会自动以管理员身份重新启动自身）。
2. 弹出原生对话框：**“是否安装 AAXSysPulse 的 WinRing0 驱动？”**
3. 用户点击“是”后，**把内嵌的驱动解压**到临时文件。
4. 确保 `C:\Windows\AAXSysPulse\` 存在（并且是一个目录，而不是文件）。
5. **复制**驱动到该目录 —— 当驱动已被加载、`.sys` 被锁定时会容忍此情况。
6. **创建或打开** `AAXSysPulseWinRing0` 内核驱动服务，**自我修复**过期的二进制路径和
   被禁用的启动类型，然后**启动**它。
7. **校验**结果：服务处于 `RUNNING` 状态，和/或设备 `\\.\WinRing0_1_2_0` 可打开。
8. 报告确切的结果。

本程序不会启动主程序 —— 只是：提权 → 确认 → 自解压 → 复制 → 注册服务 → 校验 → 报告。
注册的服务名称/类型与主程序（`msr.rs`）完全一致，因此两者是幂等的。

## 已处理的情况

每一步都做了防御性处理；大部分失败会被自动修复，其余会给出精确的报告。

| 情况 | 处理方式 |
|------|----------|
| 未提权 | 清单强制 UAC 提权；运行时兜底自动以管理员身份重启 |
| 内嵌资源缺失 / 临时文件写入失败 | 报错：“无法解压内嵌的驱动程序” |
| `C:\Windows\AAXSysPulse` 已存在且是一个**文件** | 报错：删除该文件后重试 |
| 创建文件夹失败（权限 / 磁盘） | 报错并附上 Win32 错误码 |
| 复制失败：拒绝访问 | 提示：请以管理员身份运行 |
| 复制失败：磁盘已满 | 提示：系统盘空间已满 |
| 复制失败：共享冲突（驱动已加载、`.sys` 被锁） | 若设备已可用则继续，否则提示重启 |
| `OpenSCManager` 失败 | 报错并附上错误码（通常是因为不是管理员） |
| 服务已存在 | 直接打开并复用 |
| 服务被标记为待删除（卸载未完成） | 轮询约 2 秒后打开；否则提示重启 |
| 服务二进制路径过期（文件被移动/删除） | `ChangeServiceConfig` 将其重新指向当前路径 |
| 服务启动类型被设为 `DISABLED` | `ChangeServiceConfig` 设为 `SERVICE_DEMAND_START` |
| `StartService` = 已在运行 | 视为成功 |
| `StartService` = `183`（名称冲突） | 探测设备；若可用则成功，否则提示关闭另一个实例 |
| `StartService` = 超时 / 其它 | 警告并附上错误码与提示 |
| 驱动签名 / DSE 问题 | 警告：开启测试签名，或信任 WinRing0 证书 |
| 另一个 WinRing0 实例已提供该设备 | 报告为成功（主程序可直接使用） |

## 编译构建

### 前置条件

- Windows 10/11
- Visual Studio 2022 **或** 2019（Build Tools **或** Community 版本）
- Windows 10/11 SDK（任意较新版本 —— `build.bat` 会自动探测已安装的最新 SDK）
- 驱动文件 **`WinRing0x64.sys`** 放在 `build.bat`（或 `CMakeLists.txt`）同目录下

> 驱动是在**构建时**被内嵌进 exe 的。若没有 `WinRing0x64.sys`，构建时无法把它打包进去
> （运行时程序会报告“无法解压内嵌的驱动程序”）。可从 OpenHardwareMonitor /
> LibreHardwareMonitor 的分支获取，或从 OpenLibSys/WinRing0 源码自行编译。

### 使用 build.bat 构建（推荐）

1. 把 `WinRing0x64.sys` 放到本文件夹（与 `build.bat` 同一位置）
2. 运行 `build.bat`（双击，或在开发者命令提示符中运行）

```
cd D:\code\sys-overlay-c-master\sysoverlay\driver-installer
build.bat
```

`build.bat` 会自动探测 VS 与最新 Windows SDK，内嵌驱动**和清单**（requireAdministrator），
并使用 `/MT`（静态 CRT）链接，使 exe 在没有 `vcruntime140.dll` 的机器上也能运行。

### 使用 CMake 构建（备选）

```
mkdir build && cd build
cmake ..
cmake --build . --config Release
```

CMake 同样会内嵌驱动和清单，但需要在配置阶段把 `WinRing0x64.sys` 放在 `CMakeLists.txt` 同目录。

## 使用方法

1. 构建 driver-installer（此时 `WinRing0x64.sys` 已被打包进 exe）
2. 分发 / 运行单独的 `driver-installer.exe`
3. 接受 **UAC** 提权提示（清单会自动申请管理员权限）
4. 被询问是否安装时点击**“是”**，对话框会显示确切的结果

运行时 exe 旁边不需要任何 `WinRing0x64.sys` 文件 —— 它已经在 exe 内部了。

## 对话框

- **确认** —— “是否安装 AAXSysPulse 的 WinRing0 驱动？”（是 / 否）
- **成功** —— “驱动安装成功。”（文件已复制 + `AAXSysPulseWinRing0` 服务正在运行）
- **成功（设备已可用）** —— 文件已复制 + 服务已注册，但 WinRing0 设备（`\\.\WinRing0_1_2_0`）
  已由另一个正在运行的 WinRing0 实例提供（例如 Open Hardware Monitor / LibreHardware Monitor）。
  主程序可直接使用它。
- **警告** —— 文件已复制 + 服务已注册，但驱动未运行。若错误码为 `183`，说明另一个程序已加载了
  WinRing0 驱动（请关闭它 / 重启）；若为超时，请重启；否则通常是驱动签名 / DSE 问题。
- **失败** —— 解释失败原因（内嵌驱动缺失、目标为文件 / 创建文件夹失败、复制失败，或服务注册失败）

## 退出码

| 码 | 含义 |
|----|------|
| 0 | 用户取消；或安装成功；或已注册但未运行（警告） |
| 1 | 硬性失败：解压、文件夹、复制或服务注册失败 |

## 文件清单

- `src/driver-installer.c` - 源代码
- `driver-installer.rc` - 资源脚本（供 build.bat 使用；内嵌驱动 + 清单）
- `driver-installer.rc.in` - 资源脚本的 CMake 模板
- `driver-installer.manifest` - 应用程序清单（requireAdministrator + DPI 感知）
- `build.bat` - 构建脚本（推荐）
- `CMakeLists.txt` - CMake 构建配置（备选）
- `bin/driver-installer.exe` - 输出的可执行文件
