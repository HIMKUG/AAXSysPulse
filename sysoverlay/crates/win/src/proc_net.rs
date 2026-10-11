//! Network sampler — real `GetIfEntry2` incremental implementation.
//!
//! 语义（与历史 `GetIfTable2` 全表版本一致）：
//! - 汇总所有「物理在线」网卡的 `InOctets`/`OutOctets`（以太网 6 + WiFi 71）；
//! - `exclude_virtual=true` 时其余类型一律排除（回环 24 / 隧道 131 /
//!   Hyper-V 虚拟交换机 53 等），只保留物理 NIC；
//! - 速率 = 字节增量 ÷ 墙钟间隔（`rx_bps`/`tx_bps` 单位是**字节每秒**，
//!   与 `fmt_rate(bytes_per_sec)` 对齐）；
//! - 计数器回绕/网卡重置（sum 变小）时丢弃本轮速率并重置基线，防止负值。
//!
//! ## 性能策略（本轮压缩点）
//!
//! 旧实现每周期调一次 `GetIfTable2`，枚举**全部**接口（本机 ~54 项，
//! 含大量虚拟/过滤器子接口），单次 ~28ms，是采样主循环最大头。
//!
//! 新实现：
//! 1. **拓扑缓存**：首次 / 全部掉线 / 安全间隔（10s）到期时，才跑一次
//!    `GetIfTable2` 过滤出「在线物理 NIC」的接口索引缓存（`nic_cache`）；
//! 2. **增量查询**：每周期仅对缓存里的 ~1–2 个 NIC 调 `GetIfEntry2`
//!    （O(活跃NIC数)，单接口 <0.1ms），按 `InterfaceIndex`（LUID 置零）精确取数；
//! 3. 过滤器子接口（与主接口共享计数器）在枚举阶段就被排除，缓存里只留主接口索引，
//!    不存在重复计数。
//!
//! 探针（`AAXSYSPULSE_PROBE=1`）会记录枚举结果与每周期入选 NIC，
//! 是「检测完全错误」类问题的第一手证据。

use crate::probe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;
use windows::Win32::Foundation::{BOOLEAN, HANDLE};
use windows::Win32::Networking::WinSock::ADDRESS_FAMILY;
use windows::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetIfEntry2, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2, MIB_IPINTERFACE_ROW,
    MIB_NOTIFICATION_TYPE, NotifyIpInterfaceChange,
};
use windows::Win32::NetworkManagement::Ndis::{IfOperStatusUp, IF_OPER_STATUS};
use aaxsyspulse_core::sampler::{SampleOutcome, Sampler, SamplerId};
use aaxsyspulse_core::snapshot::{NetStat, Snapshot};

const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
const IF_TYPE_IEEE80211: u32 = 71;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const IF_TYPE_TUNNEL: u32 = 131;

/// 重新枚举活跃 NIC 的兜底间隔（ms）：正常情况下 NIC 增删/UP-DOWN 由
/// `NotifyIpInterfaceChange` 事件驱动重枚举（见 `init_net_notify`），
/// 此处 60s 仅用于覆盖「睡眠恢复等事件漏报」场景，避免拓扑永久失效。
/// 事件驱动生效后，稳态下 `GetIfTable2` 全表枚举（~28ms）几乎不再发生。
const NET_ENUM_REFRESH_MS: u64 = 60_000;

/// 网卡拓扑失效标记：由 `NotifyIpInterfaceChange` 回调置位，下次 net 采样时重枚举。
/// 初始为 true，保证首采样一定枚举一次。
static NET_TOPO_INVALID: AtomicBool = AtomicBool::new(true);

/// 注册只发生一次（`NetSampler::new` 可能被多次构造，例如测试）。
static NET_NOTIFY_INIT: Once = Once::new();

/// 网卡变更回调：运行在 IPHLPAPI 的系统线程上，**只置一个原子**，绝不做任何
/// 分配/日志/锁（避免回调路径死锁）。真正的重枚举推迟到下次 net 采样。
///
/// 签名须匹配 `PIPINTERFACE_CHANGE_CALLBACK`：`(callercontext, row, notificationtype)`。
unsafe extern "system" fn on_net_change(
    _callercontext: *const core::ffi::c_void,
    _row: *const MIB_IPINTERFACE_ROW,
    _notificationtype: MIB_NOTIFICATION_TYPE,
) {
    NET_TOPO_INVALID.store(true, Ordering::SeqCst);
}

/// 启动期调用一次：注册网卡变更通知，用事件驱动取代 10s 轮询重枚举。
///
/// 回调仅置 `NET_TOPO_INVALID`；`sample()` 在重枚举前清掉它。
/// 注册句柄由 OS 在进程退出时回收（回调只引用全局原子，安全），无需保存。
pub fn init_net_notify() {
    NET_NOTIFY_INIT.call_once(|| {
        unsafe {
            let mut handle = HANDLE::default();
            let err = NotifyIpInterfaceChange(
                ADDRESS_FAMILY(0), // AF_UNSPEC：同时监听 IPv4 / IPv6 接口变化
                Some(on_net_change),
                None,
                BOOLEAN(0), // 不立即发一次初始通知（首采样本来就会枚举）
                &mut handle,
            );
            if err.is_ok() {
                crate::probe::log("net NotifyIpInterfaceChange registered");
            } else {
                crate::probe::log(&format!("net NotifyIpInterfaceChange failed err={}", err.0));
            }
        }
    });
}

/// 系统唤醒后调用：标记网卡拓扑待重枚举（与 GPU `invalidate_topology` /
/// MSR `invalidate_on_resume` 对称的自愈入口）。睡眠期间网卡驱动可能重枚举
/// （虚拟交换机重建 / 无线重连后接口序号变化），`NotifyIpInterfaceChange`
/// 回调不保证覆盖唤醒事件；置位后下一次 net 采样即全表重枚举，不必等 60s 兜底。
pub fn invalidate_topology() {
    NET_TOPO_INVALID.store(true, Ordering::SeqCst);
}

/// Npcap / WFP / QoS 过滤器子接口的别名关键字。
///
/// `GetIfTable2` 会把每个物理网卡展开成**多个接口行**：主接口
/// （如 `WLAN`、`以太网`）加上一批过滤器微端口（如
/// `WLAN-WFP Native MAC Layer`、`WLAN-Npcap Packet Driver`、
/// `WLAN-QoS Packet Scheduler`、`WLAN-Virtual WiFi Filter`）。
/// 这些过滤器接口与主接口**共享同一组字节计数器**，若全部加总，
/// 网速会被放大 N 倍（实测同一 WLAN 被计 7 次）。
///
/// 这些后缀是驱动厂商的固定英文名，不随系统语言本地化
/// （中文系统里主接口叫 `以太网`，过滤器仍叫 `...-WFP ...`）。
const FILTER_ALIAS_MARKERS: [&str; 7] = [
    "wfp native mac",
    "wfp 802.3",
    "wfp mac",
    "npcap",
    "qos packet",
    "native wifi",
    "virtual wifi",
];

/// 判断是否为 Npcap/WFP/QoS 过滤器子接口（跳过，避免流量重复计数）。
fn is_filter_interface(alias: &str) -> bool {
    let a = alias.to_ascii_lowercase();
    FILTER_ALIAS_MARKERS.iter().any(|k| a.contains(k))
}

/// 速率计算的最小间隔：太短会被计数器粒度放大成尖峰。
const MIN_DT_MS: u64 = 50;

pub struct NetSampler {
    exclude_virtual: bool,
    /// (上次墙钟 ms, ΣInOctets, ΣOutOctets)
    prev: Option<(u64, u64, u64)>,
    /// 缓存的「在线物理 NIC」接口索引。枚举一次后复用，避免每周期 `GetIfTable2`。
    nic_cache: Vec<u32>,
    /// 上次重新枚举的时间（墙钟 ms）。
    last_enum_ms: u64,
}

impl NetSampler {
    pub fn new(exclude_virtual: bool) -> Self {
        Self {
            exclude_virtual,
            prev: None,
            nic_cache: Vec::new(),
            last_enum_ms: 0,
        }
    }

    pub fn reset_baselines(&mut self) {
        self.prev = None;
    }

    /// 重新枚举一次完整接口表，过滤出「在线物理 NIC」的接口索引缓存起来。
    ///
    /// 仅在首次 / 全部掉线 / `NET_ENUM_REFRESH_MS` 到期时调用（见 `sample`）。
    /// 返回 true 表示成功拿到至少一个可用 NIC。
    fn enum_cache(&mut self, now_ms: u64) -> bool {
        unsafe {
            let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
            let err = GetIfTable2(&mut table);
            if err.is_err() || table.is_null() {
                probe::log(&format!("net enum GetIfTable2 failed err={}", err.0));
                return false;
            }

            let entries = (*table).NumEntries as usize;
            let mut cache: Vec<u32> = Vec::with_capacity(entries.min(8));
            for i in 0..entries {
                let row = &*(*table).Table.as_ptr().add(i);
                let t = row.Type;
                let up = row.OperStatus == IfOperStatusUp;
                let physical = t == IF_TYPE_ETHERNET_CSMACD || t == IF_TYPE_IEEE80211;
                let not_virtual = t != IF_TYPE_SOFTWARE_LOOPBACK && t != IF_TYPE_TUNNEL;
                let alias = wide_to_str(&row.Alias, 128);
                // 过滤器子接口与主接口共享计数器，必须跳过（否则网速放大 N 倍）
                let keep = up
                    && !is_filter_interface(&alias)
                    && if physical { true } else { !self.exclude_virtual && not_virtual };
                if keep {
                    cache.push(row.InterfaceIndex);
                }
                if probe::enabled() {
                    probe::log(&format!(
                        "net enum row {i} type={t} oper={} idx={} alias={:?} keep={}",
                        row.OperStatus.0, row.InterfaceIndex, alias, keep as u8
                    ));
                }
            }
            FreeMibTable(table as *const core::ffi::c_void);

            if probe::enabled() {
                probe::log(&format!("net enum cached {} nics: {:?}", cache.len(), cache));
            }
            self.nic_cache = cache;
            self.last_enum_ms = now_ms;
            !self.nic_cache.is_empty()
        }
    }
}

impl Default for NetSampler {
    fn default() -> Self {
        Self::new(true)
    }
}

fn wide_to_str(w: &[u16], max: usize) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    let s = String::from_utf16_lossy(&w[..end]);
    s.chars().take(max).collect()
}

impl Sampler for NetSampler {
    fn name(&self) -> &'static str {
        "net"
    }

    fn id(&self) -> SamplerId {
        SamplerId::Net
    }

    fn sample(&mut self, _prev_snap: Option<&Snapshot>, now_ms: u64, out: &mut Snapshot) -> SampleOutcome {
        // 决定是否需要重新枚举：首次 / 全部掉线 / 事件标记置位（NIC 变化）/
        // 60s 兜底（覆盖事件漏报）。事件驱动下稳态几乎只在 NIC 真变化时重枚举。
        let need_enum = self.nic_cache.is_empty()
            || NET_TOPO_INVALID.swap(false, Ordering::SeqCst)
            || now_ms.saturating_sub(self.last_enum_ms) >= NET_ENUM_REFRESH_MS;
        if need_enum {
            self.enum_cache(now_ms);
        }

        let mut sum_in: u64 = 0;
        let mut sum_out: u64 = 0;
        let mut kept: usize = 0;

        unsafe {
            for &idx in &self.nic_cache {
                // MIB_IF_ROW2::default() 把 InterfaceLuid 置零 → GetIfEntry2 走 InterfaceIndex 精确取数。
                let mut row = MIB_IF_ROW2 {
                    InterfaceIndex: idx,
                    ..Default::default()
                };
                if GetIfEntry2(&mut row).is_err() {
                    probe::log(&format!("net GetIfEntry2 failed idx={idx}"));
                    continue;
                }
                // NIC 自枚举后掉线：跳过本轮，但保留缓存（间隔到期会重新枚举）。
                if row.OperStatus != IfOperStatusUp {
                    continue;
                }
                sum_in = sum_in.saturating_add(row.InOctets);
                sum_out = sum_out.saturating_add(row.OutOctets);
                kept += 1;
                if probe::enabled() {
                    probe::log(&format!(
                        "net query idx={idx} in={} out={} kept={}",
                        row.InOctets, row.OutOctets, kept
                    ));
                }
            }
        }

        // 缓存的 NIC 全部掉线 → 强制下次重新枚举（如 WiFi 断连后切到有线）。
        if kept == 0 && !self.nic_cache.is_empty() {
            self.nic_cache.clear();
        }

        let mut rx_bps: u64 = 0;
        let mut tx_bps: u64 = 0;
        let mut counter_reset = false;

        if let Some((pms, pin, pout)) = self.prev {
            let dt = now_ms.saturating_sub(pms);
            if sum_in < pin || sum_out < pout {
                // 计数器回绕（NIC 重置 / 休眠恢复 / 网卡集合变化）：本轮速率无效
                counter_reset = true;
                probe::log(&format!(
                    "net counter-reset prev_in={pin} cur_in={sum_in} prev_out={pout} cur_out={sum_out}"
                ));
            } else if dt >= MIN_DT_MS {
                // bytes/s = Δbytes × 1000 / Δms
                rx_bps = (sum_in - pin).saturating_mul(1000) / dt;
                tx_bps = (sum_out - pout).saturating_mul(1000) / dt;
            }
        }

        if probe::enabled() {
            probe::log(&format!(
                "net cached={} kept={} in={sum_in} out={sum_out} dt={} rx_Bps={rx_bps} tx_Bps={tx_bps} reset={}",
                self.nic_cache.len(),
                kept,
                self.prev.as_ref().map(|(p, _, _)| now_ms.saturating_sub(*p)).unwrap_or(0),
                counter_reset as u8
            ));
        }

        // 无论本轮是否算出速率，都更新基线（reset 时相当于重置基线）
        self.prev = Some((now_ms, sum_in, sum_out));
        if counter_reset {
            out.net = NetStat { rx_bps: 0, tx_bps: 0 };
            out.timestamp_ms = now_ms;
            return SampleOutcome::Ok;
        }

        out.net = NetStat { rx_bps, tx_bps };
        out.timestamp_ms = now_ms;
        SampleOutcome::Ok
    }
}

/// `IF_OPER_STATUS` 的数值（诊断输出用），保持与 SDK 枚举一致。
#[allow(dead_code)]
fn oper_dbg(o: IF_OPER_STATUS) -> i32 {
    o.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 过滤器子接口（与主接口共享计数器）必须被判定为跳过。
    #[test]
    fn test_filter_interfaces_detected() {
        for alias in [
            "WLAN-WFP Native MAC Layer",
            "以太网 2-WFP Native MAC Layer",
            "本地连接* 6-WFP Native MAC Layer",
            "WLAN-Npcap Packet Driver",
            "WLAN-QoS Packet Scheduler",
            "WLAN-Native WiFi Filter",
            "本地连接* 9-Virtual WiFi Filter",
            "WLAN-WFP 802.3 MAC Layer",
        ] {
            assert!(is_filter_interface(alias), "应跳过过滤器接口: {}", alias);
        }
    }

    /// 真实物理接口（含中文/本地化别名）绝不能误判为过滤器。
    #[test]
    fn test_real_interfaces_not_filtered() {
        for alias in [
            "WLAN",
            "以太网",
            "以太网 2",
            "Ethernet0",
            "本地连接",
            "本地连接* 6", // Hyper-V 虚拟交换机，但它不是过滤器，不在此过滤
            "Loopback Pseudo-Interface 1",
            "Sangfor SSL VPN Virtual Adapter",
        ] {
            assert!(!is_filter_interface(alias), "不应跳过物理接口: {}", alias);
        }
    }

    /// 大小写与混排必须稳健（sysinfo 风格 FQBN 可能出现的大小写）。
    #[test]
    fn test_case_insensitive() {
        assert!(is_filter_interface("wlan-wfp NATIVE MAC layer"));
        assert!(is_filter_interface("WLAN-NPCAP PACKET DRIVER"));
        assert!(!is_filter_interface("wlan"));
        assert!(!is_filter_interface("wifi adapter"));
    }
}
