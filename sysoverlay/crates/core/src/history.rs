//! 历史时序环形缓冲（曲线窗口数据底座，纯逻辑零依赖）。

/// 固定容量环形缓冲：写入覆盖最旧值，读取按时间顺序（旧 → 新）。
///
/// # 为什么不直接 `VecDeque`
///
/// 对固定容量趋势场景，`VecDeque` 每次 push 都移动头指针且可能分配；
/// 这里是**纯数组 + 头下标**的环形实现，零分配、局部性好（bottom 的
/// `TimeSeriesData` 同思路，但我们不需要它那套多段式复杂结构）。
#[derive(Clone, Debug)]
pub struct Timeline {
    cap: usize,
    buf: Vec<f32>,
    head: usize,  // 下一次写入位置
    count: usize, // 已存数量（<= cap）
}

impl Timeline {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            buf: vec![0.0; cap.max(1)],
            head: 0,
            count: 0,
        }
    }

    /// 写入一个样本；溢出时覆盖最旧值。
    pub fn push(&mut self, v: f32) {
        self.buf[self.head] = v;
        self.head = (self.head + 1) % self.cap;
        if self.count < self.cap {
            self.count += 1;
        }
    }

    /// 当前样本数。
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// 容量。
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// 按时间顺序（旧 → 新）返回快照（零分配：栈上填充回调或 Vec）。
    pub fn values(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.count);
        for i in 0..self.count {
            let idx = if self.count < self.cap {
                i
            } else {
                (self.head + i) % self.cap
            };
            out.push(self.buf[idx]);
        }
        out
    }

    /// 最大值（空则 0.0）。
    pub fn max(&self) -> f32 {
        let mut m = 0.0f32;
        for i in 0..self.count {
            let idx = if self.count < self.cap {
                i
            } else {
                (self.head + i) % self.cap
            };
            m = m.max(self.buf[idx]);
        }
        m
    }
}

/// 曲线窗口需要的一整套时序（CPU / 内存 / 网络上下行）。
#[derive(Clone, Debug)]
pub struct Trends {
    /// 样本序号（诊断用）。
    pub seq: u64,
    /// 最近采样毫秒（GetTickCount64）。
    pub at_ms: u64,
    pub cpu: Timeline,
    pub mem: Timeline,
    pub net_rx: Timeline,
    pub net_tx: Timeline,
}

impl Trends {
    /// 默认容量：120 样本（默认 1s 刷新 = 120 秒窗口）。
    pub const CAP: usize = 120;

    pub fn new() -> Self {
        Self {
            seq: 0,
            at_ms: 0,
            cpu: Timeline::new(Self::CAP),
            mem: Timeline::new(Self::CAP),
            net_rx: Timeline::new(Self::CAP),
            net_tx: Timeline::new(Self::CAP),
        }
    }

    /// 提取当前所有曲线序列（供曲线窗口绘制）。
    pub fn series(&self) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
        (self.cpu.values(), self.mem.values(), self.net_rx.values(), self.net_tx.values())
    }
}

impl Default for Trends {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_returns_oldest_first() {
        let mut t = Timeline::new(4);
        t.push(1.0);
        t.push(2.0);
        t.push(3.0);
        assert_eq!(t.values(), vec![1.0, 2.0, 3.0]);
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn timeline_overwrites_oldest() {
        let mut t = Timeline::new(3);
        for v in [1.0, 2.0, 3.0, 4.0, 5.0] {
            t.push(v);
        }
        assert_eq!(t.len(), 3);
        assert_eq!(t.values(), vec![3.0, 4.0, 5.0], "满后覆盖最旧");
    }

    #[test]
    fn timeline_max() {
        let mut t = Timeline::new(4);
        assert_eq!(t.max(), 0.0);
        t.push(1.0);
        t.push(9.0);
        t.push(3.0);
        assert_eq!(t.max(), 9.0);
    }

    #[test]
    fn trends_series_length_equals_pushes() {
        let mut tr = Trends::new();
        for i in 0..200 {
            tr.cpu.push(i as f32);
            tr.mem.push(10.0);
            tr.net_rx.push(5.0);
            tr.net_tx.push(0.0);
            tr.seq += 1;
        }
        let (cpu, mem, rx, tx) = tr.series();
        assert_eq!(cpu.len(), Trends::CAP, "超容量后截断为容量");
        assert_eq!(mem.len(), Trends::CAP);
        assert_eq!(rx.len(), Trends::CAP);
        assert_eq!(tx.len(), Trends::CAP);
        // 环形：最后的样本是 199
        assert_eq!(cpu.last(), Some(&199.0));
    }
}