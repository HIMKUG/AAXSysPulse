//! SysOverlay Core Library
//!
//! Pure Rust core library for the system overlay application.
//! Contains data structures, metrics computation, layout calculation,
//! and sampler traits. Platform-specific implementation lives in other crates.
//!
//! # 分层约束
//!
//! 本 crate 是**纯逻辑层**：零平台依赖、零 `unsafe`。
//! 所有 `windows::...` 调用只允许出现在 `sysoverlay_win`。
//!
//! # 模块依赖方向
//!
//! ```text
//! types  ←── 被所有人依赖（领域枚举唯一真源）
//! color  ←── config / ui
//! units  ←── ui
//! layout ←── ui / win（不依赖 config）
//! config ←── app / ui（不依赖 layout）
//! ```

pub mod types;

pub mod snapshot;
pub mod sampler;
pub mod scheduler;
pub mod metrics;
pub mod layout;
pub mod config;
pub mod color;
pub mod units;
