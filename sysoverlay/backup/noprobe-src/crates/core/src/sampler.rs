//! Sampler trait: pluggable metric collectors.
//!
//! Each sampler is responsible for collecting one category of metrics
//! (CPU, GPU, network, etc.). Samplers are `Send` so they can be used
//! in a separate sampling thread.
//!
//! # 诊断为什么不用日志库
//!
//! v2 决策「诊断落点」：内存固定环形缓冲（64 项，**无堆分配、无磁盘 I/O**）。
//! 悬浮窗是常驻进程，不能一边跑一边往磁盘追加日志 —— 那会在用户不知情的情况下
//! 制造无限增长的文件。诊断的三个出口：
//! 1. 默认只在内存（零成本）
//! 2. 崩溃时随 MiniDump 一起带出（`win` 层负责）
//! 3. `--diag` 参数时打到 stdout（人主动要才输出）

/// 采样结果类别。
///
/// 与旧版的区别：**去掉了 `&'static str` payload**。
/// 理由：payload 让 `SampleOutcome` 变大、且鼓励把用户可见文案写进平台层；
/// 现在统一改为 `code: u16` 落在 [`SamplerDiag`] 里，
/// 让"出了什么错"与"怎么描述"解耦（文案留给 UI 层决定）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleOutcome {
    /// 采样成功，有新数据。
    Ok,
    /// 瞬时（软）失败：保留上一次的值，下个周期重试。
    /// 连续失败会触发指数退避，但采样器本身不重建。
    Transient,
    /// 本周期被有意跳过（例如功耗采样间隔还没到、或功能被关闭）。
    /// **不算失败**，不应计入退避。
    Skipped,
    /// 致命（硬）失败：采样器已不可用，必须重建。
    Fatal,
}

impl SampleOutcome {
    /// 是否算作失败（用于决定是否计入退避）。
    pub fn is_failure(self) -> bool {
        matches!(self, SampleOutcome::Transient | SampleOutcome::Fatal)
    }

    /// 是否需要重建采样器。
    pub fn is_fatal(self) -> bool {
        matches!(self, SampleOutcome::Fatal)
    }
}

/// 采样器标识（诊断用定长枚举，避免存 `&str` 指针占 16 字节）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SamplerId {
    Cpu = 0,
    Memory = 1,
    Gpu = 2,
    Net = 3,
    Power = 4,
}

impl SamplerId {
    /// 稳定名称（用于 `--diag` 输出与 minidump 旁注）。
    pub const fn name(self) -> &'static str {
        match self {
            SamplerId::Cpu => "cpu",
            SamplerId::Memory => "memory",
            SamplerId::Gpu => "gpu",
            SamplerId::Net => "net",
            SamplerId::Power => "power",
        }
    }

    /// 全部采样器（诊断遍历用）。
    pub const ALL: [SamplerId; 5] = [
        SamplerId::Cpu,
        SamplerId::Memory,
        SamplerId::Gpu,
        SamplerId::Net,
        SamplerId::Power,
    ];
}

/// 诊断环容量。64 项 ≈ 覆盖最近一分钟（1s 间隔 × 5 采样器）。
pub const DIAG_CAP: usize = 64;

/// 单条诊断记录。8 字节，全部定长，无指针、无堆。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiagEntry {
    /// 哪个采样器。
    pub sampler: SamplerId,
    /// 结果类别。
    pub outcome: SampleOutcome,
    /// 平台层错误码（0 = 无附加信息）。
    pub code: u16,
    /// 采样序号（低 32 位）。
    pub seq: u32,
}

/// 固定容量的诊断环形缓冲。
///
/// 满了就覆盖最旧的一条 —— 常驻进程必须有界内存，不能让诊断把自己撑死。
#[derive(Clone, Debug)]
pub struct SamplerDiag {
    entries: [DiagEntry; DIAG_CAP],
    /// 下一个写入位置。
    head: usize,
    /// 累计写入条数（可大于 `DIAG_CAP`，用于判断是否曾溢出）。
    written: u64,
}

impl SamplerDiag {
    /// 空缓冲。
    pub const fn new() -> Self {
        // 用第一条记录作为填充值；seq=0/outcome=Skipped 视为"未使用"。
        const EMPTY: DiagEntry = DiagEntry {
            sampler: SamplerId::Cpu,
            outcome: SampleOutcome::Skipped,
            code: 0,
            seq: 0,
        };
        Self {
            entries: [EMPTY; DIAG_CAP],
            head: 0,
            written: 0,
        }
    }

    /// 写入一条记录。O(1)，永不分配。
    pub fn push(&mut self, e: DiagEntry) {
        self.entries[self.head] = e;
        self.head = (self.head + 1) % DIAG_CAP;
        self.written += 1;
    }

    /// 已写入的总条数（含被覆盖的）。
    pub fn written(&self) -> u64 {
        self.written
    }

    /// 当前缓冲中的有效条数（上限 `DIAG_CAP`）。
    pub fn len(&self) -> usize {
        (self.written as usize).min(DIAG_CAP)
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 是否曾发生覆盖（说明诊断密度超过了 64 条窗口）。
    pub fn overflowed(&self) -> bool {
        self.written > DIAG_CAP as u64
    }

    /// 按**时间顺序**（最旧 → 最新）遍历。
    pub fn iter(&self) -> DiagIter<'_> {
        let n = self.len();
        // 最旧的一条：缓冲未满时是 0；满了就是 head（下一个要被覆盖的位置）
        let start = if n == DIAG_CAP { self.head } else { 0 };
        DiagIter {
            diag: self,
            start,
            remaining: n,
        }
    }

    /// 最新一条。
    pub fn last(&self) -> Option<&DiagEntry> {
        if self.is_empty() {
            return None;
        }
        let idx = (self.head + DIAG_CAP - 1) % DIAG_CAP;
        Some(&self.entries[idx])
    }

    /// 统计某个采样器最近的失败次数（用于 UI 决定是否标灰）。
    pub fn recent_failures(&self, id: SamplerId) -> u32 {
        self.iter()
            .filter(|e| e.sampler == id && e.outcome.is_failure())
            .count() as u32
    }

    /// 清空。
    pub fn clear(&mut self) {
        self.head = 0;
        self.written = 0;
    }
}

impl Default for SamplerDiag {
    fn default() -> Self {
        Self::new()
    }
}

/// [`SamplerDiag::iter`] 的迭代器。
pub struct DiagIter<'a> {
    diag: &'a SamplerDiag,
    start: usize,
    remaining: usize,
}

impl<'a> Iterator for DiagIter<'a> {
    type Item = &'a DiagEntry;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let idx = (self.start + (self.diag.len() - self.remaining)) % DIAG_CAP;
        self.remaining -= 1;
        Some(&self.diag.entries[idx])
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

/// 指数退避：连续失败时拉长重试间隔，成功后立刻复位。
///
/// 1s → 2s → 4s → 8s → 16s → **30s 封顶**（不再增长，
/// 否则一次长时间睡眠唤醒后要等好几分钟才有数据）。
#[derive(Clone, Copy, Debug, Default)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    /// 最大档位对应的毫秒数。
    pub const MAX_DELAY_MS: u32 = 30_000;

    /// 记录一次失败，返回本次应等待的毫秒数。
    pub fn next_delay_ms(&mut self) -> u32 {
        let shift = self.failures.min(5); // 1s,2s,4s,8s,16s,32s→封顶 30s
        self.failures += 1;
        (1_000u32 << shift).min(Self::MAX_DELAY_MS)
    }

    /// 记录一次成功，复位退避。
    pub fn reset(&mut self) {
        self.failures = 0;
    }

    /// 当前连续失败次数。
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// 是否处于退避状态。
    pub fn is_backing_off(&self) -> bool {
        self.failures > 0
    }
}

/// Trait for metric samplers.
///
/// Implementors hold whatever OS handles or state they need and mutate
/// a shared [`Snapshot`][crate::snapshot::Snapshot] on each call to `sample`.
pub trait Sampler: Send {
    /// Human-readable name of this sampler (e.g. "cpu", "gpu").
    fn name(&self) -> &'static str;

    /// 诊断用的稳定标识。
    fn id(&self) -> SamplerId;

    /// Perform one sampling cycle, writing results into `out`.
    ///
    /// `prev` is the previous complete snapshot (if available) and can be
    /// used to compute deltas (e.g. CPU time differences).
    ///
    /// `now_ms` is the current timestamp in milliseconds since Unix epoch.
    ///
    /// Returns [`SampleOutcome`] indicating success or the class of failure.
    fn sample(
        &mut self,
        prev: Option<&crate::snapshot::Snapshot>,
        now_ms: u64,
        out: &mut crate::snapshot::Snapshot,
    ) -> SampleOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: SamplerId, outcome: SampleOutcome, seq: u32) -> DiagEntry {
        DiagEntry {
            sampler: id,
            outcome,
            code: 0,
            seq,
        }
    }

    #[test]
    fn outcome_classification() {
        assert!(SampleOutcome::Transient.is_failure());
        assert!(SampleOutcome::Fatal.is_failure());
        assert!(!SampleOutcome::Skipped.is_failure(), "主动跳过不算失败");
        assert!(!SampleOutcome::Ok.is_failure());

        assert!(SampleOutcome::Fatal.is_fatal());
        assert!(!SampleOutcome::Transient.is_fatal());
    }

    #[test]
    fn sampler_id_names_are_stable() {
        assert_eq!(SamplerId::Cpu.name(), "cpu");
        assert_eq!(SamplerId::Gpu.name(), "gpu");
        assert_eq!(SamplerId::ALL.len(), 5);
    }

    #[test]
    fn diag_starts_empty() {
        let d = SamplerDiag::new();
        assert!(d.is_empty());
        assert_eq!(d.len(), 0);
        assert!(d.last().is_none());
    }

    #[test]
    fn diag_iterates_in_time_order() {
        let mut d = SamplerDiag::new();
        for i in 0..5u32 {
            d.push(entry(SamplerId::Cpu, SampleOutcome::Ok, i));
        }
        let seqs: Vec<u32> = d.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![0, 1, 2, 3, 4], "必须最旧→最新");
        assert_eq!(d.last().unwrap().seq, 4);
    }

    #[test]
    fn diag_wraps_around_and_keeps_order() {
        let mut d = SamplerDiag::new();
        for i in 0..(DIAG_CAP as u32 + 10) {
            d.push(entry(SamplerId::Cpu, SampleOutcome::Ok, i));
        }
        assert_eq!(d.len(), DIAG_CAP, "容量必须有界");
        assert!(d.overflowed());
        assert_eq!(d.written(), DIAG_CAP as u64 + 10);

        // 环形遍历后仍是最旧→最新，且是最新的 64 条
        let seqs: Vec<u32> = d.iter().map(|e| e.seq).collect();
        assert_eq!(seqs.len(), DIAG_CAP);
        assert_eq!(seqs[0], 10, "最旧的一条应是第 10 号（0..9 被覆盖）");
        assert_eq!(*seqs.last().unwrap(), DIAG_CAP as u32 + 9);
        assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "顺序必须连续");
    }

    #[test]
    fn diag_recent_failures_counts_only_failures() {
        let mut d = SamplerDiag::new();
        d.push(entry(SamplerId::Gpu, SampleOutcome::Transient, 1));
        d.push(entry(SamplerId::Gpu, SampleOutcome::Skipped, 2));
        d.push(entry(SamplerId::Gpu, SampleOutcome::Ok, 3));
        d.push(entry(SamplerId::Cpu, SampleOutcome::Fatal, 4));
        d.push(entry(SamplerId::Gpu, SampleOutcome::Fatal, 5));

        assert_eq!(d.recent_failures(SamplerId::Gpu), 2, "Skipped 与 Ok 不计入");
        assert_eq!(d.recent_failures(SamplerId::Cpu), 1);
        assert_eq!(d.recent_failures(SamplerId::Net), 0);
    }

    #[test]
    fn diag_clear_resets() {
        let mut d = SamplerDiag::new();
        d.push(entry(SamplerId::Cpu, SampleOutcome::Ok, 1));
        d.clear();
        assert!(d.is_empty());
        assert_eq!(d.written(), 0);
    }

    #[test]
    fn backoff_doubles_then_caps() {
        let mut b = Backoff::default();
        assert!(!b.is_backing_off());

        assert_eq!(b.next_delay_ms(), 1_000);
        assert_eq!(b.next_delay_ms(), 2_000);
        assert_eq!(b.next_delay_ms(), 4_000);
        assert_eq!(b.next_delay_ms(), 8_000);
        assert_eq!(b.next_delay_ms(), 16_000);
        assert_eq!(b.next_delay_ms(), 30_000, "封顶 30s");
        assert_eq!(b.next_delay_ms(), 30_000, "之后保持不变");
        assert_eq!(b.failures(), 7);
    }

    #[test]
    fn backoff_resets_on_success() {
        let mut b = Backoff::default();
        b.next_delay_ms();
        b.next_delay_ms();
        assert!(b.is_backing_off());
        b.reset();
        assert!(!b.is_backing_off());
        assert_eq!(b.next_delay_ms(), 1_000, "复位后从第一档重新开始");
    }

    #[test]
    fn diag_entry_is_compact() {
        // 8 字节：id(1) + outcome(1) + code(2) + seq(4)
        assert!(
            core::mem::size_of::<DiagEntry>() <= 8,
            "诊断项必须紧凑，实测 {} 字节",
            core::mem::size_of::<DiagEntry>()
        );
        assert!(
            core::mem::size_of::<SamplerDiag>() <= DIAG_CAP * 8 + 16,
            "整个诊断缓冲必须无堆、有界"
        );
    }
}
