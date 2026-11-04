//! Fluent Design token 模块（路线 A：拆壳移植）。
//!
//! S1 交接物：按 `FluentToken映射表.md` §3 新增 token 转录。

// 审查 178：S2/S5 已接入（layout.rs 字号、widget.rs 主题消费 FLUENT），
// 移除模块级 allow。真实未接线字段 = FluentPalette 中约 20 个仍无消费方的
// token（bg_disabled / brand 三连 / stroke_disabled / stroke_width / 圆角 /
// 字族与字重 / 行高 / 间距 / 动效 / 曲线），已由编译器逐一点名 —— 逐个接线
// 属 S5 后续路线，不在本次修复范围（保留字段供消费方接入）。
#[allow(dead_code)]
pub mod tokens;
