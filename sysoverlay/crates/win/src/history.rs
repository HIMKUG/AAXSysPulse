//! 共享历史时序（采样线程 → 曲线窗口，ArcSwap 无锁）。
//!
//! 采样线程每轮 `push` 一份最新点；曲线窗口定时器 1s 读取整份快照自绘。

use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwap;
use sysoverlay_core::history::Trends;
use sysoverlay_core::snapshot::Snapshot;

static SHARED: OnceLock<ArcSwap<Trends>> = OnceLock::new();

/// 采样线程每轮调用：追加 CPU/内存/网络四个点。
///
/// 网络以 **KB/s**（>>10）存储：值域 ~10^6，f32 24 位尾数可无损表示；
/// 直接存 bps（~10^9）会掉精度。
pub fn push(snap: &Snapshot) {
    let t = SHARED.get_or_init(|| ArcSwap::from_pointee(Trends::new()));
    let mut cur = t.load_full(); // Arc<Trends>
    {
        let c = Arc::make_mut(&mut cur);
        c.seq = snap.seq;
        c.at_ms = snap.timestamp_ms;
        c.cpu.push(snap.cpu.value().unwrap_or(0.0));
        c.mem.push(snap.memory.load_pct);
        c.net_rx.push((snap.net.rx_bps >> 10) as f32);
        c.net_tx.push((snap.net.tx_bps >> 10) as f32);
    }
    t.store(cur);
}

/// 曲线窗口读取整份时序（克隆，避免持有 Arc 影响写入）。
pub fn get() -> Trends {
    SHARED
        .get_or_init(|| ArcSwap::from_pointee(Trends::new()))
        .load_full()
        .as_ref()
        .clone()
}

/// 轻量读取当前时序序号（无克隆；曲线窗口定时器脏检查用，
/// 一次原子 load，避免每秒为「有没有新数据」克隆整份 Trends）。
pub fn peek_seq() -> u64 {
    SHARED
        .get_or_init(|| ArcSwap::from_pointee(Trends::new()))
        .load()
        .seq
}
