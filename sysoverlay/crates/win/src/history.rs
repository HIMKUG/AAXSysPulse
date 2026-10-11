//! 共享历史时序（采样线程写入，ArcSwap 无锁）。
//!
//! ⚠️ 本模块的文档曾有两处与实现相反，按实测更正（审查项 9.3）：
//!
//! 1. `push` **不是"每轮采样一次"**，而是**每次提交闸门放行一次** ——
//!    调用点在 `app/src/main.rs` 的 `if should_commit { … history::push(&committed) }` 里。
//!    idle（数值不变）时闸门关着 ⇒ 点序密度 ≠ 采样密度，时间轴会随节律浮动，
//!    这正是 8.27 / §11-3 记的那条"按点序而非时间建模"。
//! 2. **"曲线窗口"目前不存在** —— `win/src/graph.rs` 已不在仓库，`get()` / `peek_seq()`
//!    零调用方，只有写侧在跑。读侧（曲线窗口）去留属等待用户决策项（§11-3），
//!    在此之前不要把这里的"曲线窗口定时器 1s 读取"当成既有功能来依赖。

use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwap;
use aaxsyspulse_core::history::Trends;
use aaxsyspulse_core::snapshot::Snapshot;

static SHARED: OnceLock<ArcSwap<Trends>> = OnceLock::new();

/// 提交闸门放行时调用：追加 CPU/内存/网络四个点（**不是每轮采样**，见模块文档第 1 条）。
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

/// 读取整份时序（克隆，避免持有 Arc 影响写入）。
///
/// ⚠️ 当前**零调用方**（原读取方"曲线窗口"已不在仓库）；保留是给 §11-3 的
/// 去留决策留一条现成的读侧接口，不要当成"已接好的功能"。
pub fn get() -> Trends {
    SHARED
        .get_or_init(|| ArcSwap::from_pointee(Trends::new()))
        .load_full()
        .as_ref()
        .clone()
}

/// 轻量读取当前时序序号（无克隆；脏检查用，一次原子 load，
/// 避免每秒为「有没有新数据」克隆整份 Trends）。
///
/// ⚠️ 与 [`get`] 同样**零调用方**。
pub fn peek_seq() -> u64 {
    SHARED
        .get_or_init(|| ArcSwap::from_pointee(Trends::new()))
        .load()
        .seq
}
