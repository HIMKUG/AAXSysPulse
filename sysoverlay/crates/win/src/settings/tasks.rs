//! 异步任务托管：配置防抖落盘。
//!
//! # 为什么要有这个模块
//!
//! 旧实现把「防抖判断」散在 `commit_change` 里（`LAST_SAVE: LazyLock<Mutex<Instant>>`
//! 全局静态），**窗口关闭时没有任何东西保证最后一次改动落盘**——用户改完设置
//! 立刻关窗、500ms 未到，改动就丢了。
//!
//! 新模型：[`DebouncedSave`] 是纯逻辑（合成时钟，可单测）；归属 `SettingsWindow`，
//! **Drop 时强制 flush**，关窗必落盘。

/// 防抖窗口（毫秒）：高频操作期间跳过磁盘写入。
pub const SAVE_DEBOUNCE_MS: u64 = 500;

/// 防抖保存状态机（纯逻辑，时钟由调用方注入）。
///
/// `now_ms` 为任意单调毫秒时钟（进程内用 GetTickCount64 / Instant 均可）。
#[derive(Debug, Default)]
pub struct DebouncedSave {
    /// 上次成功落盘时刻（毫秒）；None = 尚未落过盘（立即允许）。
    last_save_ms: Option<u64>,
    /// 防抖期内是否有未落盘的改动。
    pending: bool,
}

impl DebouncedSave {
    /// 配置变更时调用：标记 pending，返回「本次是否应当立即写盘」。
    pub fn on_change(&mut self, now_ms: u64) -> bool {
        self.pending = true;
        match self.last_save_ms {
            None => true,
            Some(t) => now_ms.saturating_sub(t) >= SAVE_DEBOUNCE_MS,
        }
    }

    /// 写盘成功后调用：记录时刻、清除 pending。
    pub fn on_saved(&mut self, now_ms: u64) {
        self.last_save_ms = Some(now_ms);
        self.pending = false;
    }

    /// 是否有未落盘改动（关窗 flush 用）。
    pub fn has_pending(&self) -> bool {
        self.pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_change_saves_immediately() {
        let mut d = DebouncedSave::default();
        assert!(d.on_change(1000), "首次改动必须立即落盘");
        d.on_saved(1000);
        assert!(!d.has_pending());
    }

    #[test]
    fn change_within_window_is_skipped_and_marked_pending() {
        let mut d = DebouncedSave::default();
        d.on_change(1000);
        d.on_saved(1000);
        assert!(!d.on_change(1200), "500ms 内的改动应跳过写盘");
        assert!(d.has_pending(), "但必须标记 pending，供关窗 flush");
    }

    #[test]
    fn change_after_window_saves_again() {
        let mut d = DebouncedSave::default();
        d.on_change(1000);
        d.on_saved(1000);
        assert!(d.on_change(1000 + SAVE_DEBOUNCE_MS), "超窗后恢复写盘");
    }

    #[test]
    fn pending_survives_multiple_skipped_changes() {
        let mut d = DebouncedSave::default();
        d.on_change(0);
        d.on_saved(0);
        for t in [100, 200, 300, 400] {
            assert!(!d.on_change(t));
        }
        assert!(d.has_pending(), "连续防抖跳过后仍须 pending");
    }

    #[test]
    fn failed_save_keeps_pending() {
        // 只调 on_change 不调 on_saved：pending 保持，关窗时会补写。
        let mut d = DebouncedSave::default();
        assert!(d.on_change(0));
        assert!(d.has_pending());
    }
}
