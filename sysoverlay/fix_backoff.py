# -*- coding: utf-8 -*-
"""修正退避门语义（第二个审计修复脚本）。

背景：第一批修复里把「退避门」接进了 CPU/Mem/Net/Power 的采样路径，但
`Backoff` 只存 `failures: u32`，`is_backing_off()` 等价于 `failures > 0`。
用它当跳过门 = **失败一次就永久跳过**（被门挡下时推 Skipped，Skipped 不复位
failures）→ 指标在首次瞬时失败后永久冻结，反而比修复前更糟。

正确做法：`Backoff` 必须记录「下次允许尝试的时刻」，门只在窗口未到期时挡住，
到期即放行一次重试；再失败则档位继续上升（1s→2s→…→30s 封顶）。

原则（同 fix_audit.py）：每个替换 assert 命中次数，全部成功才落盘。
"""
import io
import os
import re

ROOT = os.path.dirname(os.path.abspath(__file__))


def load(rel):
    p = os.path.join(ROOT, rel)
    with io.open(p, "r", encoding="utf-8", newline="") as f:
        return p, f.read()


def norm(text, src):
    if "\r\n" in src:
        return text.replace("\n", "\r\n")
    return text


def edit(state, rel, old, new, count=1, label=""):
    old = norm(old, state[rel])
    new = norm(new, state[rel])
    n = state[rel].count(old)
    assert n == count, "[%s] %s 期望命中 %d 次，实际 %d 次\n--- 模式 ---\n%s" % (
        rel,
        label or "替换",
        count,
        n,
        old[:300],
    )
    state[rel] = state[rel].replace(old, new, count)


FILES = [
    "crates/core/src/sampler.rs",
    "crates/win/src/sampler.rs",
]

state = {}
for rel in FILES:
    _, state[rel] = load(rel)

CO = "crates/core/src/sampler.rs"
WN = "crates/win/src/sampler.rs"

# ══════════════════════════════════════════════════════════════════════════════
# 1. core: Backoff 增加「下次允许尝试时刻」
# ══════════════════════════════════════════════════════════════════════════════

edit(
    state,
    CO,
    """#[derive(Clone, Copy, Debug, Default)]
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
}""",
    """#[derive(Clone, Copy, Debug, Default)]
pub struct Backoff {
    failures: u32,
    /// 下次允许尝试的时刻（与采样时钟同源的毫秒时间戳；`reset` 后为 0 = 立即可试）。
    ///
    /// ⚠️ 没有这个字段时，「是否退避中」只能由 `failures > 0` 表达，而它**无法说明
    /// 还要等多久**。一旦把它当作跳过门使用，就会退化成「失败一次 → 永久跳过」：
    /// 被门挡下的轮次记为 `Skipped`，`Skipped` 不复位 `failures`，于是门永远关着，
    /// 指标在首次瞬时失败后永久冻结（2026-09-15 审计：第一批修复曾引入此回归）。
    next_attempt_ms: u64,
}

impl Backoff {
    /// 最大档位对应的毫秒数。
    pub const MAX_DELAY_MS: u32 = 30_000;

    /// 记录一次失败；`now_ms` 为当前采样时钟。返回本次应等待的毫秒数，
    /// 并据此刻画「下次允许尝试的时刻」。
    pub fn next_delay_ms(&mut self, now_ms: u64) -> u32 {
        let shift = self.failures.min(5); // 1s,2s,4s,8s,16s,32s→封顶 30s
        self.failures = self.failures.saturating_add(1);
        let delay = (1_000u32 << shift).min(Self::MAX_DELAY_MS);
        self.next_attempt_ms = now_ms.saturating_add(delay as u64);
        delay
    }

    /// 记录一次成功，复位退避（同时允许立即重试）。
    pub fn reset(&mut self) {
        self.failures = 0;
        self.next_attempt_ms = 0;
    }

    /// 当前连续失败次数。
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// 退避窗口是否**仍未到期**（`now_ms` 早于下次允许尝试的时刻）。
    ///
    /// 到期后返回 `false` —— 允许再试一次；若再失败档位继续上升。这是它唯一正确的
    /// 用法：门只会挡住**窗口内**的轮次，不会永久关死。
    pub fn is_backing_off(&self, now_ms: u64) -> bool {
        self.failures > 0 && now_ms < self.next_attempt_ms
    }

    /// 距下次允许尝试还剩多少毫秒（0 = 已可重试）。供 UI/调度显示。
    pub fn remaining_delay_ms(&self, now_ms: u64) -> u32 {
        if self.failures == 0 {
            return 0;
        }
        let left = self.next_attempt_ms.saturating_sub(now_ms);
        left.min(u32::MAX as u64) as u32
    }
}""",
    label="Backoff 增加到期时刻",
)

# 现有两个测试同步新签名
edit(
    state,
    CO,
    """    #[test]
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
    }""",
    """    #[test]
    fn backoff_doubles_then_caps() {
        let mut b = Backoff::default();
        let t0: u64 = 10_000;
        assert!(!b.is_backing_off(t0));

        assert_eq!(b.next_delay_ms(t0), 1_000);
        assert_eq!(b.next_delay_ms(t0), 2_000);
        assert_eq!(b.next_delay_ms(t0), 4_000);
        assert_eq!(b.next_delay_ms(t0), 8_000);
        assert_eq!(b.next_delay_ms(t0), 16_000);
        assert_eq!(b.next_delay_ms(t0), 30_000, "封顶 30s");
        assert_eq!(b.next_delay_ms(t0), 30_000, "之后保持不变");
        assert_eq!(b.failures(), 7);
    }

    #[test]
    fn backoff_resets_on_success() {
        let mut b = Backoff::default();
        let t0: u64 = 10_000;
        b.next_delay_ms(t0);
        b.next_delay_ms(t0);
        assert!(b.is_backing_off(t0));
        b.reset();
        assert!(!b.is_backing_off(t0));
        assert_eq!(b.next_delay_ms(t0), 1_000, "复位后从第一档重新开始");
    }

    #[test]
    fn backoff_window_expires_and_allows_retry() {
        // 回归锁：退避门必须**到期后放行**。若门写成 `failures > 0`，则一次瞬时失败
        // 之后永远挡住重试（被挡的轮次记 Skipped，不复位 failures）→ 指标永久冻结。
        let mut b = Backoff::default();
        let t0: u64 = 10_000;
        assert_eq!(b.next_delay_ms(t0), 1_000);
        assert!(b.is_backing_off(t0), "刚失败：窗口内");
        assert!(b.is_backing_off(t0 + 999), "窗口未到期");
        assert!(!b.is_backing_off(t0 + 1_000), "到点必须放行一次重试");
        assert_eq!(b.remaining_delay_ms(t0 + 400), 600);
        assert_eq!(b.remaining_delay_ms(t0 + 1_000), 0);
        b.reset();
        assert_eq!(b.remaining_delay_ms(t0), 0);
    }""",
    label="backoff 测试同步签名 + 新增到期回归锁",
)

# ══════════════════════════════════════════════════════════════════════════════
# 2. win: 退避门传 now_ms；push_diag 传 now_ms
# ══════════════════════════════════════════════════════════════════════════════

for id_name in ("Cpu", "Memory", "Net", "Power"):
    # 门条件（Net 那条多了 net_due，单独列在下面）
    if id_name == "Net":
        continue
    edit(
        state,
        WN,
        "if plan.%s && self.backoff[SamplerId::%s as usize].is_backing_off() {"
        % ("cpu" if id_name == "Cpu" else "mem" if id_name == "Memory" else "power", id_name),
        "if plan.%s && self.backoff[SamplerId::%s as usize].is_backing_off(now_ms) {"
        % ("cpu" if id_name == "Cpu" else "mem" if id_name == "Memory" else "power", id_name),
        label="退避门 %s" % id_name,
    )

edit(
    state,
    WN,
    """        if plan.net && net_due && self.backoff[SamplerId::Net as usize].is_backing_off() {""",
    """        if plan.net && net_due && self.backoff[SamplerId::Net as usize].is_backing_off(now_ms) {""",
    label="退避门 Net",
)

# 所有 push_diag 调用点补 now_ms（全部位于 sample() 内，now_ms 在作用域中）
pat = re.compile(r"self\.push_diag\(([^()]*?), 0\)")
calls = pat.findall(state[WN])
assert len(calls) == 16, "push_diag 调用点期望 16 个，实际 %d 个" % len(calls)
state[WN] = pat.sub(r"self.push_diag(\1, 0, now_ms)", state[WN])
print("   push_diag 调用点补 now_ms: 16 处")

edit(
    state,
    WN,
    """    fn push_diag(&mut self, id: SamplerId, outcome: SampleOutcome, code: u16) {""",
    """    fn push_diag(&mut self, id: SamplerId, outcome: SampleOutcome, code: u16, now_ms: u64) {""",
    label="push_diag 签名",
)

edit(
    state,
    WN,
    """        let idx = id as usize;
        if outcome.is_failure() {
            self.backoff[idx].next_delay_ms();
        } else if matches!(outcome, SampleOutcome::Ok) {""",
    """        let idx = id as usize;
        if outcome.is_failure() {
            // 传 now_ms：退避门据此刻画「下次允许尝试的时刻」，到点即放行。
            self.backoff[idx].next_delay_ms(now_ms);
        } else if matches!(outcome, SampleOutcome::Ok) {""",
    label="push_diag 传 now_ms",
)

# 公开访问器：去掉硬编码 1000 的假返回
edit(
    state,
    WN,
    """    /// Back-off state for UI to display.
    pub fn backing_off(&self, id: SamplerId) -> bool {
        self.backoff[id as usize].is_backing_off()
    }

    /// Returns how long to wait before retrying (milliseconds).
    pub fn retry_delay_ms(&self, id: SamplerId) -> u32 {
        let _ = self.backoff[id as usize]
            .failures()
            .saturating_sub(1);
        // Exposed so the caller (sampling thread) knows how long to sleep.
        // A real implementation would use this in the sleep loop.
        1000 // placeholder; actual backoff tracked per-sampler above
    }""",
    """    /// Back-off state for UI to display（`now_ms` 须与采样时钟同源）。
    pub fn backing_off(&self, id: SamplerId, now_ms: u64) -> bool {
        self.backoff[id as usize].is_backing_off(now_ms)
    }

    /// Returns how long to wait before retrying (milliseconds)；0 = 已可重试。
    ///
    /// 旧实现恒返回硬编码 `1000`（自称 placeholder），调用方拿到的等待时长是假的；
    /// 现按 `Backoff` 的实际到期时刻计算（2026-09-15 审计）。
    pub fn retry_delay_ms(&self, id: SamplerId, now_ms: u64) -> u32 {
        self.backoff[id as usize].remaining_delay_ms(now_ms)
    }""",
    label="访问器改为按到期时刻计算",
)

# ══════════════════════════════════════════════════════════════════════════════
# 落盘前自检：win 侧不得残留无参 is_backing_off() / next_delay_ms()
# ══════════════════════════════════════════════════════════════════════════════
leftover = re.findall(r"is_backing_off\(\)|next_delay_ms\(\)", state[WN])
assert not leftover, "win/sampler.rs 仍有无参调用：%s" % leftover
co_left = re.findall(r"is_backing_off\(\)|next_delay_ms\(\)", state[CO])
# core 里只剩定义体内部不出现无参调用，测试已同步
assert not co_left, "core/sampler.rs 仍有无参调用：%s" % co_left

for rel in FILES:
    p = os.path.join(ROOT, rel)
    with io.open(p, "w", encoding="utf-8", newline="") as f:
        f.write(state[rel])
    print("WROTE %s" % rel)

print("\n退避门语义修正完成。")
