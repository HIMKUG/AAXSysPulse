//! `aaxsyspulse-app` 的 lib 目标 —— **当前为空，且是刻意的**。
//!
//! 本 crate 的全部代码都在 bin 目标 `src/main.rs`（产物 `AAXSysPulse.exe`）里，
//! 模块由 `main.rs` 用 `mod …;` 直接挂载；没有任何外部 crate 依赖本 lib，
//! 引导程序也不引用它（它只把主程序 **exe 当字节资源** 打包，见
//! `guide-app/src-tauri/build.rs`）。
//!
//! 这里原先写的是 `// Placeholder: App implementation in 2B agent.`（审查项 9.1 的
//! 同款过时自述：一个不存在的"待办"，读者会以为还有个未完成的重构在进行）。
//! 若哪天真要把逻辑挪进 lib（例如给 `pre_launch_checks` 的纯函数加更多单测），
//! 记得同步删掉 `main.rs` 里的 `mod` 声明，否则两份定义会打架。
