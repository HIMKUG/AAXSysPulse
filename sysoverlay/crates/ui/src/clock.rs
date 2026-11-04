//! 本地时钟源（悬浮窗「时间」行的数据来源）。
//!
//! # 为什么时钟不放 [`Snapshot`](aaxsyspulse_core::snapshot::Snapshot)
//!
//! 采样间隔最长 5s（`INTERVAL_MS_ALLOWED[0]`），若时钟随快照走，
//! 秒数会 5s 一跳，完全不可用。时钟由平台层（`win`）在**每次渲染前**
//! 调用 [`set_local`] 写入（`GetLocalTime`），渲染器构建时间行时经
//! [`get`] 读取。
//!
//! # 脏渲染如何因此成立
//!
//! 渲染器把时间文本纳入 cell 内容指纹（`CellSig::hash` → 帧指纹）：
//! 秒数变化 → 指纹变化 → 只重绘时间 cell 的脏矩形；同一秒内的其余
//! 帧指纹不变 → 整帧跳过。平台层的 CLOCK 定时器（`gdi.rs`，按秒对齐）
//! 驱动即可，采样线程完全不用为此提速（审查 117：原"250ms 定时器"
//! 说法与实现不符）。
//!
//! # 线程模型
//!
//! 打包 `h<<16 | m<<8 | s` 存单个 `AtomicU32`：读侧永远得到自洽的
//! 一组时分秒（不存在跨字段撕裂），无锁、零分配。

use std::sync::atomic::{AtomicU32, Ordering};

static HMS: AtomicU32 = AtomicU32::new(0);

/// 写入本地时间（平台层每次渲染前调用；值域 0..=23 / 0..=59，越界钳制 ——
/// 审查 116：原先只 `&0xFF`，`set_local(99,99,99)` 会写入非法时间）。
pub fn set_local(hour: u32, minute: u32, second: u32) {
    let packed = (hour.min(23) << 16) | (minute.min(59) << 8) | second.min(59);
    HMS.store(packed, Ordering::Relaxed);
}

/// 读取当前 (时, 分, 秒)。尚未写入时返回 (0, 0, 0)。
pub fn get() -> (u32, u32, u32) {
    let v = HMS.load(Ordering::Relaxed);
    ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_then_get_roundtrips() {
        set_local(22, 12, 36);
        assert_eq!(get(), (22, 12, 36));
        set_local(0, 0, 0);
        assert_eq!(get(), (0, 0, 0));
        set_local(23, 59, 59);
        assert_eq!(get(), (23, 59, 59));
        // 审查 116/508：越界输入被钳制，不产生非法时间
        set_local(99, 99, 99);
        assert_eq!(get(), (23, 59, 59));
        // 审查 837：本静态与 renderer/tests 共享，结束时复位避免污染并行测试
        set_local(0, 0, 0);
        assert_eq!(get(), (0, 0, 0));
    }
}
