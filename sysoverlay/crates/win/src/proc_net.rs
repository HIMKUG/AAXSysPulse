//! Network sampler — real `GetIfTable2` implementation.
//!
//! 语义：
//! - 汇总所有「物理在线」网卡的 `InOctets`/`OutOctets`（以太网 6 + WiFi 71）；
//! - `exclude_virtual=true` 时其余类型一律排除（回环 24 / 隧道 131 /
//!   Hyper-V 虚拟交换机 53 等），只保留物理 NIC；
//! - 速率 = 字节增量 ÷ 墙钟间隔（`rx_bps`/`tx_bps` 单位是**字节每秒**，
//!   与 `fmt_rate(bytes_per_sec)` 对齐）；
//! - 计数器回绕/网卡重置（sum 变小）时丢弃本轮速率并重置基线，防止负值。
//!
//! 探针（`SYSOVERLAY_PROBE=1`）会记录整张接口表：类型/状态/别名/字节计数/
//! 是否入选，这是「检测完全错误」类问题的第一手证据。

use crate::probe;
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::NetworkManagement::Ndis::{IfOperStatusUp, IF_OPER_STATUS};
use sysoverlay_core::sampler::{SampleOutcome, Sampler, SamplerId};
use sysoverlay_core::snapshot::{NetStat, Snapshot};

const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
const IF_TYPE_IEEE80211: u32 = 71;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const IF_TYPE_TUNNEL: u32 = 131;

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
}

impl NetSampler {
    pub fn new(exclude_virtual: bool) -> Self {
        Self { exclude_virtual, prev: None }
    }

    pub fn reset_baselines(&mut self) {
        self.prev = None;
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
        unsafe {
            let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
            let err = GetIfTable2(&mut table);
            if err.is_err() || table.is_null() {
                probe::log(&format!("net GetIfTable2 failed err={}", err.0));
                return SampleOutcome::Transient;
            }

            let entries = (*table).NumEntries as usize;
            let mut sum_in: u64 = 0;
            let mut sum_out: u64 = 0;
            let mut kept = 0usize;

            for i in 0..entries {
                let row = &*(*table).Table.as_ptr().add(i);
                let t = row.Type;
                let oper = row.OperStatus;
                let up = oper == IfOperStatusUp;
                let physical = t == IF_TYPE_ETHERNET_CSMACD || t == IF_TYPE_IEEE80211;
                let not_virtual = t != IF_TYPE_SOFTWARE_LOOPBACK && t != IF_TYPE_TUNNEL;
                let alias = wide_to_str(&row.Alias, 128);
                // 过滤器子接口与主接口共享计数器，必须跳过（否则网速放大 N 倍）
                let keep = up
                    && !is_filter_interface(&alias)
                    && if physical { true } else { !self.exclude_virtual && not_virtual };
                if keep {
                    sum_in = sum_in.saturating_add(row.InOctets);
                    sum_out = sum_out.saturating_add(row.OutOctets);
                    kept += 1;
                }
                if probe::enabled() {
                    probe::log(&format!(
                        "net row {i} type={t} oper={} alias={:?} in={} out={} keep={}",
                        oper.0, alias, row.InOctets, row.OutOctets, keep as u8
                    ));
                }
            }
            FreeMibTable(table as *const core::ffi::c_void);

            let mut rx_bps: u64 = 0;
            let mut tx_bps: u64 = 0;
            let mut counter_reset = false;

            if let Some((pms, pin, pout)) = self.prev {
                let dt = now_ms.saturating_sub(pms);
                if sum_in < pin || sum_out < pout {
                    // 计数器回绕（NIC 重置 / 休眠恢复）：本轮速率无效
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
                    "net sum entries={entries} kept={kept} in={sum_in} out={sum_out} dt={} rx_Bps={rx_bps} tx_Bps={tx_bps} reset={}",
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
