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
    // 审查 047：MAGIC 必须独占首行，"MCFG1garbage" 不得被接受。
    let first_line = text.lines().next().unwrap_or("").trim_end();
    if first_line != MAGIC {
        return Err("不是合法的 MCFG 容器（缺少 MCFG1 头）".into());
    }

    let mut files: Vec<(String, String)> = Vec::new();
    let mut cur_name: Option<String> = None;
    let mut cur_body: Vec<String> = Vec::new();

    for line in text.lines() {
        let t = line.trim_end();
        // 审查 045：END 判据必须先于段头判据，否则 "--- END ---" 会被
        // strip_prefix/strip_suffix 拆成名为 "END" 的段头，END 分支永不可达。
        if t == "--- END ---" {
            if let Some(prev) = cur_name.take() {
                files.push((prev, cur_body.join("\n")));
                cur_body.clear();
            }
            break;
        }
        if let Some(name) = t.strip_prefix("--- ") {
            if let Some(n) = name.strip_suffix(" ---") {
                if let Some(prev) = cur_name.take() {
                    files.push((prev, cur_body.join("\n")));
                    cur_body.clear();
                }
                let n = n.trim();
                // 审查 048：解析处直接过滤白名单外段名，防注入任意路径。
                if is_known_file(n) {
                    cur_name = Some(n.to_string());
                }
                continue;
            }
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
        // 审查 647：从非默认配置出发，避免"parse 什么都没改也通过"的假阳性。
        let mut cfg = Config::default();
        cfg.render_style = RenderStyle::Vertical;
        cfg.normal_opacity = 77;
        cfg.interval_ms = 200;
        cfg.autostart = true;
        let text = export_config(&cfg);
        assert!(text.starts_with(MAGIC));
        assert!(text.contains("--- system.cfg ---"));
        assert!(text.contains("--- monitor.cfg ---"));
        assert!(text.contains("--- END ---"));

        let files = parse_mcfg(&text).expect("解析应成功");
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        // 审查 046：精确断言块数量，且 END 不得被当成段名解析进来。
        assert_eq!(files.len(), 4, "应为 4 个文件块（仅当前激活布局）");
        assert!(!names.contains(&"END"));
        assert!(names.contains(&system::SYSTEM_FILE));
        assert!(names.contains(&appearance::APPEARANCE_FILE));
        assert!(names.contains(&monitor::MONITOR_FILE));
        // 激活布局为 Vertical ⇒ 携带 layout_v.cfg
        assert!(names.contains(&LAYOUT_V_FILE));
        assert!(!names.contains(&LAYOUT_H_FILE));

        // 重新落盘后应与直接序列化一致（逐字段）
        for (name, content) in &files {
            if name == system::SYSTEM_FILE {
                let mut c2 = Config::default();
                system::parse(&mut c2, content);
                assert_eq!(c2.render_style, cfg.render_style);
                assert_eq!(c2.normal_opacity, cfg.normal_opacity);
                assert_eq!(c2.interval_ms, cfg.interval_ms);
                assert_eq!(c2.autostart, cfg.autostart);
            }
        }
    }

    #[test]
    fn mcfg_strips_privacy_fields() {
        let mut cfg = Config::default();
        cfg.admin_check_continue_count = 3;
        cfg.admin_check_last_date = 20260914;
        let text = export_config(&cfg);
        // 审查 648：脱敏断言 —— 关键是**值**被抹成 0（键名仍在序列化输出里），
        // 原断言只比"精确串 3"，序列化格式一变（无空格/多空格）即假通过。
        // mcfg 容器走 INI（` = `），断言"键名后跟的值不是原隐私值"的两种形态都不出现。
        assert!(!text.contains("admin_check_continue_count = 3"));
        assert!(!text.contains("admin_check_last_date = 20260914"));
        // 脱敏后必须写出显式的 0 值（而不是整键消失 —— 保留键结构才能被旧版读回）。
        assert!(text.contains("admin_check_continue_count = 0"));
        assert!(text.contains("admin_check_last_date = 0"));
    }

    #[test]
    fn parse_mcfg_rejects_bad_magic() {
        assert!(parse_mcfg("not a container").is_err());
        assert!(parse_mcfg("MCFG1garbage").is_err());
    }

    /// 白名单外的段名在解析处被跳过（审查 048），body 不得混入结果。
    #[test]
    fn parse_mcfg_skips_unknown_sections() {
        let text = "MCFG1\n--- evil.cfg ---\njunk\n--- system.cfg ---\ntopmost = 1\n--- END ---\n";
        let files = parse_mcfg(text).expect("解析应成功");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, system::SYSTEM_FILE);
        assert!(files[0].1.contains("topmost = 1"));
    }
}
