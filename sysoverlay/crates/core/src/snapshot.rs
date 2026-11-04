//! Snapshot: immutable result of a single sampling cycle.
//!
//! All fields are scalar types, so `Clone` is cheap (shallow copy).

/// A complete sampling result, immutable once produced.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Snapshot {
    /// Monotonically increasing sequence number.
    pub seq: u64,
    /// 采样时间戳（毫秒）。
    ///
    /// 审查 629：当前实现里 `win::sampler` 把 `GetTickCount64()`（开机以来的
    /// 单调毫秒）同时写进本字段与 `uptime_ms`，两者数值恒等 —— 本字段**不是**
    /// Unix 墙钟（量级 ~1.7e12），不要拿它换算日历时间。
    pub timestamp_ms: u64,
    /// 系统已运行时长（毫秒，来自 `GetTickCount64`）。
    ///
    /// 早期版本直接拿 `timestamp_ms / 1000` 当"运行时长"显示，是个明显错误。
    pub uptime_ms: u64,
    /// CPU utilization percentage (0.0..=100.0).
    pub cpu: Metric<f32>,
    /// Memory statistics.
    pub memory: MemoryStat,
    /// GPU utilization percentage (0.0..=100.0).
    pub gpu: Metric<f32>,
    /// Network statistics (bytes per second).
    pub net: NetStat,
    /// Disk I/O statistics (bytes per second). 默认全零，采样器接管后按需填充。
    pub disk: DiskStat,
    /// CPU 封装功耗（瓦）。
    ///
    /// 默认 `Unavailable`：功耗是可选附加组件（v2 决策「驱动边界」），
    /// 驱动没装时主程序**不会**因此降级，这一行直接隐藏。
    pub power: Metric<f32>,
    /// 综合占用（Util，%）。由 CPU/内存/GPU 占用率经排队论闭式解聚合而成，
    /// 详见 [`crate::composite`]。
    pub util: Metric<f32>,
    /// Util 是否为**降级值**：≥1 个数据源缺失时按剩余源重归一化计算。
    /// 仅 `util` 有值时有意义；渲染层据此把数值染成降级色。
    pub util_partial: bool,
}

impl Snapshot {
    /// 启动时的占位快照：什么都还没有，全部 `Unavailable`。
    ///
    /// `app` 用它初始化 `ArcSwap`，这样第一帧就有东西可画
    /// （渲染为 "Initializing..."），而不是等采样线程的第一帧。
    pub fn placeholder(now_ms: u64) -> Self {
        Self {
            seq: 0,
            timestamp_ms: now_ms,
            uptime_ms: 0,
            cpu: Metric::Unavailable,
            memory: MemoryStat::default(),
            gpu: Metric::Unavailable,
            net: NetStat::default(),
            disk: DiskStat::default(),
            power: Metric::Unavailable,
            util: Metric::Unavailable,
            util_partial: false,
        }
    }
}

/// A metric value with optional staleness information.
///
/// 审查 630：`Stale` 变体生产代码零构造（陈旧语义由 ui 侧 SnapMetric 承担），
/// 已删除以简化枚举；如需恢复"带陈旧标记的值"，先在采样层接线再扩枚举。
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Metric<T> {
    /// Fresh value.
    Value(T),
    /// No data available (sensor unavailable, etc.).
    #[default]
    Unavailable,
}


impl<T: Copy> Metric<T> {
    /// Returns the value if present, otherwise `None`.
    pub fn value(&self) -> Option<T> {
        match self {
            Metric::Value(v) => Some(*v),
            Metric::Unavailable => None,
        }
    }

    /// Maps the inner value through `f`, preserving staleness.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Metric<U> {
        match self {
            Metric::Value(v) => Metric::Value(f(v)),
            Metric::Unavailable => Metric::Unavailable,
        }
    }
}

/// Memory usage statistics.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct MemoryStat {
    /// Memory load percentage (0.0..=100.0).
    pub load_pct: f32,
    /// Used memory in GiB.
    pub used_gb: f32,
    /// Total physical memory in GiB.
    pub total_gb: f32,
}

/// Network traffic statistics (bytes per second).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct NetStat {
    /// Received bytes per second.
    pub rx_bps: u64,
    /// Transmitted bytes per second.
    pub tx_bps: u64,
}

/// Disk I/O statistics, via PDH.
///
/// 审查 278：PDH 原生返回 double，故与 [`NetStat`] 的 u64 口径不同，属有意差异。
/// 审查 087：速率分侧用 [`Metric`] 包装 —— 单侧读不到显示 `--`，不再伪装 0 B/s。
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct DiskStat {
    /// Read throughput in bytes/sec.
    pub read_bps: Metric<f64>,
    /// Write throughput in bytes/sec.
    pub write_bps: Metric<f64>,
    /// PDH 任一侧拿到过有效读数（首轮基线未建立前为 false）。
    pub ready: bool,
}
