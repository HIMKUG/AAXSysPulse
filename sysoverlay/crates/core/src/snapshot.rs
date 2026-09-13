//! Snapshot: immutable result of a single sampling cycle.
//!
//! All fields are scalar types, so `Clone` is cheap (shallow copy).

/// A complete sampling result, immutable once produced.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Snapshot {
    /// Monotonically increasing sequence number.
    pub seq: u64,
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: u64,
    /// 系统已运行时长（毫秒，来自 `GetTickCount64`）。
    ///
    /// 与 `timestamp_ms` 是两回事：后者是墙钟时间（量级 ~1.7e12），
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
            power: Metric::Unavailable,
            util: Metric::Unavailable,
            util_partial: false,
        }
    }
}

/// A metric value with optional staleness information.
#[derive(Clone, Copy, Debug, PartialEq)]
#[derive(Default)]
pub enum Metric<T> {
    /// Fresh value.
    Value(T),
    /// Value is stale: we are still showing `value` but it hasn't been updated
    /// for `stale_for` sampling cycles.
    Stale { value: T, stale_for: u32 },
    /// No data available (sensor unavailable, etc.).
    #[default]
    Unavailable,
}


impl<T: Copy> Metric<T> {
    /// Returns the value if present, otherwise `None`.
    pub fn value(&self) -> Option<T> {
        match self {
            Metric::Value(v) => Some(*v),
            Metric::Stale { value, .. } => Some(*value),
            Metric::Unavailable => None,
        }
    }

    /// Maps the inner value through `f`, preserving staleness.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Metric<U> {
        match self {
            Metric::Value(v) => Metric::Value(f(v)),
            Metric::Stale { value, stale_for } => Metric::Stale {
                value: f(value),
                stale_for,
            },
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
