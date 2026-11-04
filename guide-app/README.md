# AAXSysPulse 引导程序（guide-app）

AAXSysPulseC（Windows 桌面性能悬浮窗）的**安装与首配引导**：14 页向导覆盖权限检测、
主程序/驱动释放安装、样式与逐项预设、开机自启与卸载，全部写盘走与主程序共享的
v3 多文件 INI 配置（跨进程兼容性有单测钉住）。

- 前端：Vue 3（`<script setup>`，纯 .js 非 TS）+ Vite，Liquid Glass 视觉
- 后端：Rust + Tauri 2（`src-tauri/`），跨进程名字/消息号唯一真源在
  `sysoverlay/crates/core/src/contract.rs`（本仓库同 workspace）

## 运行前提

1. **仅 Windows**（10/11）。开发预览 `npm run dev` 可在浏览器跑，但安装/驱动流程
   只有 Tauri 桌面壳内可用。
2. **管理员权限**：驱动（WinRing0）安装、服务注册需要提权；标准用户会被引导到
   「以管理员身份重启」页。
3. **WinRing0x64.sys 驱动资源**：`src-tauri/resources/WinRing0x64.sys` 是构建期硬
   依赖，但被仓库 .gitignore 忽略 —— 全新 clone 需自行放置真实签名的 .sys
   （缺失时构建降级为 warning 并继续，产物无法安装驱动）。
4. **资源 exe 为构建产物而非源码**：`src-tauri/resources/AAXSysPulse.exe` 由
   `src-tauri/build.rs` 在打包时从 workspace 同源编译并复制而来，不要手工替换或提交。

## 常用命令

```bash
npm install        # 安装前端依赖
npm run dev        # 浏览器开发预览（vite, 5173）
npm tauri dev      # Tauri 桌面开发（需 Rust toolchain + Windows）
npm run build      # check-bridge 门禁 + vite build
npm run tauri build  # 打包 NSIS 安装器（触发同源编译主程序）
```

## 结构速览

- `src/wizard/bridge.js` —— 前端 ↔ Rust 唯一对接层（命令封装 / 事件订阅 / 写侧闸门）
- `src/wizard/composables/useGuide.js` —— 向导全局状态机与页面流
- `src/wizard/pages/` —— 14 个向导页
- `src-tauri/src/` —— 安装（install.rs）、驱动（driver.rs）、配置扇出（config.rs）、
  通知主程序（notify.rs）等
- `scripts/check-bridge.mjs` —— 前后端命令/参数名一致性门禁（build 与 dev 前置执行）
