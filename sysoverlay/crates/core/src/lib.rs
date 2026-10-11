//! AAXSysPulse Core Library
//!
//! Pure Rust core library for the system overlay application.
//! Contains data structures, metrics computation, layout calculation,
//! and sampler traits. Platform-specific implementation lives in other crates.
//!
//! # 分层约束
//!
//! 本 crate 是**纯逻辑层**：零平台依赖、零 `unsafe`。
//! 所有 `windows::...` 调用只允许出现在 `aaxsyspulse_win`。
//!
//! # 模块依赖方向
//!
//! ```text
//! types  ←── 被所有人依赖（领域枚举唯一真源）
//! color  ←── config / ui
//! units  ←── ui
//! layout ←── ui / win（不依赖 config）
//! config ←── app / ui（不依赖 layout）
//! contract ←── app / ui / win / **引导程序**（跨进程契约唯一真源，零依赖）
//! ```

// 批次 6·①分类基线：`field_reassign_with_default` 在本 crate 的 25 处命中全部
// 位于 `#[cfg(test)]`，写法是 fixture 的既定风格（`let mut c = Config::default();
// c.show_x = true;`）——逐字段赋值比 struct-update 更贴近「只改这一项、断言这一项」
// 的意图。生产代码 0 处命中，故只在测试编译时放过。
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]
// 批次 6·②：非测试代码禁 `unwrap()`。实测本 crate 生产代码 0 处命中 ⇒ 零噪声，
// 只挡未来的回归（warn 级，不阻塞构建；升级为 deny 属用户决策项）。
#![cfg_attr(not(test), warn(clippy::unwrap_used))]

pub mod types;
pub mod contract;

pub mod snapshot;
pub mod sampler;
pub mod cadence;
pub mod scheduler;
pub mod metrics;
pub mod composite;
pub mod layout;
pub mod config;
pub mod color;
pub mod units;
pub mod history;
pub mod power;
