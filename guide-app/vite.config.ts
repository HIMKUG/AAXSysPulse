import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// 唯一一份 Vite 配置（P2-11）。
// 原先 vite.config.js 与 vite.config.ts 并存，Vite 按扩展名优先级取 **.js** 那份：
//   · .js：port 5173、无 strictPort、无 Tauri 相关设置 —— 实际生效的就是它；
//   · .ts：port 1420 + strictPort + TAURI_DEV_HOST/HMR —— 从未被执行过。
// 于是 `tauri dev` 按 devUrl(1420) 去找一个根本没监听的端口 ⇒ 白屏，
// 只有 `npm run dev`(5173) 能用 ⇒ 团队长期只在浏览器里验收 ⇒ 打包期缺陷无人发现。
// 端口统一取 **5173**（人工复核就是在这个端口看的），两边一起改：
// 这里 + `src-tauri/tauri.conf.json` 的 build.devUrl。
export default defineConfig(() => ({
  plugins: [vue()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. Tauri 期望固定端口：被占用就直接失败，绝不静默漂到 5174
  server: {
    port: 5173,
    strictPort: true,
    // 续篇 772：显式只监听回环 —— TAURI_DEV_HOST 只用于 HMR 客户端，
    // 不再把 dev server 绑到外部地址（esbuild 文件读取面一旦上 0.0.0.0 就是局域网敞口）。
    host: "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 5174,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`，但保留对 tauri.conf.json 与
      //    capabilities/ 的监听 —— 续篇 773：改窗口 label / 权限后 tauri dev
      //    必须重建上下文，否则前端一直用旧权限（"listen 被拒"那类静默失败）。
      ignored: ["**/src-tauri/target/**", "**/src-tauri/gen/**"],
    },
  },
}));
