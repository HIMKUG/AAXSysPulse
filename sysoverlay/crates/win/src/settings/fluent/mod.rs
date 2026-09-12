//! Fluent Design token 模块（路线 A：拆壳移植）。
//!
//! S1 交接物：按 `FluentToken映射表.md` §3 新增 token 转录。

// S1 为纯新增，消费方在 S2（switch 试点）接入；接入后移除本 allow。
#[allow(dead_code)]
pub mod tokens;
