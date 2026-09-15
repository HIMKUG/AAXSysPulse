//! `.mcfg` —— 配置分享/备份容器。
//!
//! 把 5 个 `.cfg` 合并进**单个可读文本文件**，便于复制分享或整机迁移；
//! 导入时拆回 5 个文件原子写回。
//!
//! 容器格式（纯文本，兼容手工编辑）：
//! ```text
//! MCFG1
//! # AAXSysPulse shared config (MCFG1)
//!
//! --- system.cfg ---
//! <system.cfg 内容>
//! --- appearance.cfg ---
//! <appearance.cfg 内容>
//! --- monitor.cfg ---
//! <monitor.cfg 内容>
//! --- layout_h.cfg ---
//! <layout_h.cfg 内容>      （仅含当前激活布局）
//! --- END ---
//! ```
//!
//! **脱敏**：导出时抹除与隐私/本机状态相关的字段（启动前检测计数与日期），
//! 避免把使用痕迹随分享文件泄露。窗口几何等布局信息不属于敏感数据，予以保留。

use crate::config::appearance;
use crate::config::layout::{self, LAYOUT_H_FILE, LAYOUT_V_FILE};
use crate::config::monitor;
use crate::config::system;
use crate::config::Config;
use crate::types::RenderStyle;

pub const MAGIC: &str = "MCFG1";

/// 导出当前配置为 `.mcfg` 文本（含脱敏）。
///
/// 仅含**当前激活布局**（`layout_h.cfg` 或 `layout_v.cfg`）；另一布局在导入方
/// 首次切换时由其本机 `config/` 目录自动补齐，故无需携带。
pub fn export_config(cfg: &Config) -> String {
    // 脱敏副本：抹除隐私/状态字段
    let mut c = *cfg;
    c.admin_check_continue_count = 0;
    c.driver_check_continue_count = 0;
    c.admin_check_last_date = 0;
    c.driver_check_last_date = 0;

    let mut out = String::new();
    out.push_str(MAGIC);
    out.push('\n');
    out.push_str("# AAXSysPulse shared config (MCFG1)\n");
    out.push('\n');
    out.push_str(&block(system::SYSTEM_FILE, &system::serialize(&c)));
    out.push_str(&block(appearance::APPEARANCE_FILE, &appearance::serialize(&c)));
    out.push_str(&block(monitor::MONITOR_FILE, &monitor::serialize(&c)));
    out.push_str(&block(
        layout::file_for(c.render_style),
        &layout::serialize(c.render_style, &c),
    ));
    out.push_str("--- END ---\n");
    out
}

fn block(name: &str, content: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!("--- {} ---\n", name));
    s.push_str(content);
    if !content.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// 解析 `.mcfg` 容器，返回若干 `(文件名, 文件内容)`。
/// 失败返回 `Err`（如缺少 `MCFG1` 头）。
pub fn parse_mcfg(text: &str) -> Result<Vec<(String, String)>, String> {
    if !text.starts_with(MAGIC) {
        return Err("不是合法的 MCFG 容器（缺少 MCFG1 头）".into());
    }

    let mut files: Vec<(String, String)> = Vec::new();
    let mut cur_name: Option<String> = None;
    let mut cur_body: Vec<String> = Vec::new();

    for line in text.lines() {
        let t = line.trim_end();
        if let Some(name) = t.strip_prefix("--- ") {
            if let Some(n) = name.strip_suffix(" ---") {
                if let Some(prev) = cur_name.take() {
                    files.push((prev, cur_body.join("\n")));
                    cur_body.clear();
                }
                cur_name = Some(n.trim().to_string());
                continue;
            }
        }
        if t == "--- END ---" {
            if let Some(prev) = cur_name.take() {
                files.push((prev, cur_body.join("\n")));
                cur_body.clear();
            }
            break;
        }
        if cur_name.is_some() {
            cur_body.push(line.to_string());
        }
    }
    if let Some(prev) = cur_name.take() {
        files.push((prev, cur_body.join("\n")));
    }

    if files.is_empty() {
        return Err("MCFG 容器为空".into());
    }
    Ok(files)
}

/// 已知文件名白名单（导入时只接受这些，防注入任意路径）。
pub fn is_known_file(name: &str) -> bool {
    matches!(
        name,
        system::SYSTEM_FILE
            | appearance::APPEARANCE_FILE
            | monitor::MONITOR_FILE
            | LAYOUT_H_FILE
            | LAYOUT_V_FILE
    )
}

/// 仅导出当前激活布局文件名（供上层判断导入后需重载哪个布局）。
pub fn active_layout_file(style: RenderStyle) -> &'static str {
    layout::file_for(style)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcfg_roundtrip() {
        let cfg = Config::default();
        let text = export_config(&cfg);
        assert!(text.starts_with(MAGIC));
        assert!(text.contains("--- system.cfg ---"));
        assert!(text.contains("--- monitor.cfg ---"));
        assert!(text.contains("--- END ---"));

        let files = parse_mcfg(&text).expect("解析应成功");
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&system::SYSTEM_FILE));
        assert!(names.contains(&monitor::MONITOR_FILE));
        // 当前激活布局应为 layout_h.cfg（默认 Strip）
        assert!(names.contains(&LAYOUT_H_FILE));

        // 重新落盘后应与直接序列化一致
        for (name, content) in &files {
            if name == system::SYSTEM_FILE {
                let mut c2 = Config::default();
                system::parse(&mut c2, content);
                assert_eq!(c2.render_style, cfg.render_style);
            }
        }
    }

    #[test]
    fn mcfg_strips_privacy_fields() {
        let mut cfg = Config::default();
        cfg.admin_check_continue_count = 3;
        cfg.admin_check_last_date = 20260914;
        let text = export_config(&cfg);
        // 脱敏后容器里不应再出现这些计数值
        assert!(!text.contains("admin_check_continue_count = 3"));
        assert!(!text.contains("admin_check_last_date = 20260914"));
    }

    #[test]
    fn parse_mcfg_rejects_bad_magic() {
        assert!(parse_mcfg("not a container").is_err());
    }
}
