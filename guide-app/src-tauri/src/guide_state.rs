//! 引导程序中断状态模块
//! 用于断点续引导：记录上次引导进行到哪一页

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StateError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// 状态文件目录：%APPDATA%\AAXSysPulseC\
pub fn state_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(aaxsyspulse_core::contract::APP_DATA_DIR_NAME)
}

/// 引导中断状态
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GuideState {
    pub incomplete: bool,
    #[serde(rename = "last_page")]
    pub last_page: u32,
    pub path: String,
    pub timestamp: u64,
    /// 上次选中的样式预设（第 4 页 A–F）。仅供引导自身恢复第 4 页高亮用，
    /// **绝不**写入主程序配置（主程序没有对应字段，且 A–F 与 appearance 语义不同）。
    /// `#[serde(default)]` 保证旧版 guide_state.json（无此字段）仍可反序列化。
    #[serde(default)]
    pub style: String,
    /// 引导自身的**完整前端快照**备份（前端 `snapshot()` 产出的原始 JSON）。
    /// 断点续引导时据此**无损**恢复全部设置 —— 只从 INI 反推是有损的
    /// （`name_mode` / 逐项颜色 / 逐项字号 / 逐项名称模式 无法精确还原）。
    /// 旧版状态文件无此字段时为 `null`，前端回退到 `config_load` 路径。
    #[serde(default)]
    pub snapshot: serde_json::Value,
}

impl GuideState {
    pub fn load() -> Result<Self, StateError> {
        let path = Self::state_file_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path)?;
        serde_json::from_str(&content).map_err(StateError::Json)
    }

    pub fn save(&self) -> Result<(), StateError> {
        let dir = state_dir();
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }
        let path = Self::state_file_path();
        let content = serde_json::to_string_pretty(self)?;
        crate::config::atomic_write(&path, content.as_bytes())
    }

    /// 删除状态文件（重新引导 / 不保存时清空断点续引导记忆）。
    /// 文件不存在即视为已清空，返回 Ok。
    pub fn reset() -> Result<(), StateError> {
        let path = Self::state_file_path();
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }

    fn state_file_path() -> PathBuf {
        state_dir().join("guide_state.json")
    }
}
