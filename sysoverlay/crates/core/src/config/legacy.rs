//! 旧版配置兼容：把单文件 `config.txt`(v2, C 版格式) 解析为 [`Config`]。
//!
//! 迁移只发生在启动时的 IO 层（`config_io::load_config`）：用本函数解析旧文本，
//! 再用各 section 序列化器写出 5 个新 `.cfg`，并把原文件备份为 `config.txt.bak`。
//! 旧文件采用「空格分隔 + 数值枚举 + COLORREF + 100ms 间隔」格式，[`Config::parse`]
//! 已实现宽容读，直接复用。

use crate::config::Config;

/// 解析旧版单文件配置（v2）为 [`Config`]。
///
/// 解析失败（空/损坏）时回退 [`Config::default`]，绝不 panic。
pub fn parse_v2(text: &str) -> Config {
    Config::parse(text)
}

/// 旧配置文件的特征识别：含 `version 2`（或 C 版无 version 但含 `overlay.c` 风格键）。
///
/// 仅用于 IO 层判断是否需要迁移；返回 true 表示这是旧单文件格式。
pub fn looks_like_legacy(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    // C 版默认不含 `=`，且一定会有这些典型键之一
    lower.contains("topmost")
        || lower.contains("netunit")
        || lower.contains("interval")
        || lower.contains("zoom")
}
