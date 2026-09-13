//! GPU sampler using D3DKMTQueryStatistics (WDDM 2.0+).
//!
//! 实现（RunningTime 增量法，与任务管理器「GPU 引擎」同一数据源）：
//! 1. `D3DKMTEnumAdapters2` 枚举适配器（LUID）；
//! 2. 每个适配器 `Type=ADAPTER` 查询拿 `NodeCount`（引擎数）；
//! 3. 每个引擎 `Type=NODE` 查询拿 `GlobalInformation.RunningTime`
//!    （该引擎累计运行时间，100ns 单位）；
//! 4. 引擎利用率% = ΔRunningTime / (Δ墙钟ms × 100)，取所有引擎的**最大值**
//!    （与任务管理器默认视图一致）。
//!
//! 说明：绑定里的 `D3DKMT_NODE_PERFDATA` 缺少 `UtilizationPercentage` 字段，
//! 因此不使用 NodePerfData 路线；RunningTime 法在 WDDM 2.0+ 通用且免驱动。
//!
//! 探针（`AAXSYSPULSE_PROBE=1`）记录每个适配器的 LUID / 引擎数 / 每引擎
//! RunningTime 与算得的百分比——GPU 一直显示 `--` 时看这里。
//!
//! # D1：拓扑缓存
//!
//! 适配器枚举（`D3DKMTEnumAdapters2` 两次调用）是逐帧最贵的一步（~40–180ms），
//! 但拓扑在显示器不变时完全稳定。因此把「(LUID, 引擎数)」缓存进 `GpuSampler`，
//! 只在首次或 `invalidate_topology()`（WM_DISPLAYCHANGE）时重建；
//! 平时每帧只做必需的逐引擎 RunningTime 查询（~2–8ms）。

use crate::probe;
use aaxsyspulse_core::sampler::{SampleOutcome, Sampler, SamplerId};
use aaxsyspulse_core::snapshot::{Metric, Snapshot};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::OnceLock;
use windows::Win32::Foundation::LUID;
use windows::Wdk::Graphics::Direct3D::{
    D3DKMTEnumAdapters2, D3DKMTQueryStatistics, D3DKMT_ADAPTERINFO, D3DKMT_ENUMADAPTERS2,
    D3DKMT_QUERYSTATISTICS, D3DKMT_QUERYSTATISTICS_QUERY_NODE,
};
use windows::Wdk::Graphics::Direct3D::{
    D3DKMT_QUERYSTATISTICS_ADAPTER as QUERY_TYPE_ADAPTER,
    D3DKMT_QUERYSTATISTICS_NODE as QUERY_TYPE_NODE,
};

/// 速率计算的最小间隔（毫秒）。
const MIN_DT_MS: u64 = 50;

/// 显示器变化等导致适配器拓扑失效时由外部置位；下次采样重建（D1）。
static TOPO_INVALID: AtomicBool = AtomicBool::new(false);

/// GPU 查询范围开关（v3 性能档，默认最佳性能）：
///   - `PRIMARY_ONLY`：只查节点数最大的适配器（= 主 dGPU），跳过虚拟/副卡；
///   - `ENGINE_3D_ONLY`：只查节点 0（3D 引擎），不再逐引擎取 max。
///   两者叠加时每周期仅 **1 次**内核查询（~1.4ms）。
static PRIMARY_ONLY: AtomicBool = AtomicBool::new(true);
static ENGINE_3D_ONLY: AtomicBool = AtomicBool::new(true);

/// 设置页/配置发布时更新 GPU 查询范围（跨线程原子，下次 sample 生效）。
///
/// 范围变化会同时置位拓扑失效：主适配器选择依赖完整拓扑，
/// 且引擎数变化后 `prev` 长度不再匹配，需重建基线。
pub fn set_gpu_scope(primary_only: bool, engine_3d_only: bool) {
    let changed = PRIMARY_ONLY.swap(primary_only, AtomicOrdering::SeqCst) != primary_only
        || ENGINE_3D_ONLY.swap(engine_3d_only, AtomicOrdering::SeqCst) != engine_3d_only;
    if changed {
        TOPO_INVALID.store(true, AtomicOrdering::SeqCst);
    }
}

/// GPU sampler using D3DKMTQueryStatistics.
pub struct GpuSampler {
    /// 首次硬失败后置 false（诚实降级：驱动不支持 WDDM 2.0 统计时）。
    available: bool,
    /// (上次墙钟 ms, 各引擎 RunningTime 的扁平数组, 引擎数)
    prev: Option<(u64, Vec<i64>)>,
    /// 缓存的适配器拓扑（LUID + 引擎数）：避免逐帧重新枚举（D1）。
    topology: Vec<(LUID, u32)>,
}

impl GpuSampler {
    pub fn new() -> Self {
        Self {
            available: true,
            prev: None,
            topology: Vec::new(),
        }
    }

    /// Always returns a sampler; first sample probes driver and degrades honestly.
    pub fn available() -> Self {
        Self::new()
    }
}

/// 标记拓扑失效（WM_DISPLAYCHANGE 等）；下次采样重建枚举（D1）。
///
/// 模块级自由函数：采样线程持有 `GpuSampler` 实例，窗口线程只能通过
/// 这个原子标记跨线程通知，不碰实例本身。
pub fn invalidate_topology() {
    TOPO_INVALID.store(true, AtomicOrdering::SeqCst);
}

impl Default for GpuSampler {
    fn default() -> Self {
        Self::new()
    }
}

/// 枚举适配器并取每个适配器的引擎数，返回 `(LUID, node_count)` 列表（D1 缓存源）。
fn enum_topology(probe_on: bool) -> Option<Vec<(LUID, u32)>> {
    unsafe {
        // 第一次调用：NumAdapters=0、pAdapters=NULL → 返回需要的适配器数
        let mut n = D3DKMT_ENUMADAPTERS2 {
            NumAdapters: 0,
            pAdapters: std::ptr::null_mut(),
        };
        let st = D3DKMTEnumAdapters2(&mut n);
        if st.is_err() {
            if probe_on {
                probe::log(&format!("gpu enum#1 failed nt=0x{:x}", st.0 as u32));
            }
            if n.NumAdapters == 0 {
                return None;
            }
        }
        if n.NumAdapters == 0 {
            if probe_on {
                probe::log("gpu no adapters");
            }
            return None;
        }

        let mut infos: Vec<D3DKMT_ADAPTERINFO> = Vec::with_capacity(n.NumAdapters as usize);
        infos.resize_with(n.NumAdapters as usize, D3DKMT_ADAPTERINFO::default);
        n.pAdapters = infos.as_mut_ptr();
        let st2 = D3DKMTEnumAdapters2(&mut n);
        if st2.is_err() {
            if probe_on {
                probe::log(&format!("gpu enum#2 failed nt=0x{:x}", st.0 as u32));
            }
            return None;
        }

        let mut topo = Vec::with_capacity(infos.len());
        for (ai, info) in infos.iter().enumerate() {
            // 引擎数
            let mut qa = D3DKMT_QUERYSTATISTICS::default();
            qa.Type = QUERY_TYPE_ADAPTER;
            qa.AdapterLuid = info.AdapterLuid;
            let node_count = if D3DKMTQueryStatistics(&qa).is_ok() {
                qa.QueryResult.AdapterInformation.NodeCount
            } else {
                0
            };
            if probe_on {
                probe::log(&format!(
                    "gpu adapter {ai} luid=0x{:08x}_0x{:08x} nodes={node_count}",
                    info.AdapterLuid.LowPart, info.AdapterLuid.HighPart
                ));
            }
            if node_count > 0 {
                topo.push((info.AdapterLuid, node_count));
            }
        }
        if topo.is_empty() {
            None
        } else {
            Some(topo)
        }
    }
}

/// 主适配器 LUID（高 32 位 | 低 32 位），首个采样解析一次并缓存——
/// `gpu_detect::pick_dgpu()` 每次调用都做完整 DXGI 枚举，不能每周期跑。
static PRIMARY_LUID: OnceLock<Option<u64>> = OnceLock::new();

fn primary_luid() -> Option<u64> {
    *PRIMARY_LUID.get_or_init(|| {
        crate::gpu_detect::pick_dgpu()
            .map(|g| ((g.luid_high as u64) << 32) | g.luid_low as u64)
    })
}

/// 按缓存拓扑查引擎 RunningTime（不再枚举适配器，D1）。
///
/// 范围过滤（零分配，借用切片）：
/// - `primary_only`：优先按 DXGI 检测到的 dGPU LUID **精确匹配**主适配器
///   （节点数启发式仅作回退——拓扑里可能有多块 17 节点适配器，max 会选错）；
/// - `engine_3d_only`：每适配器只查节点 0（3D 引擎）。
fn collect_running_times(
    topo: &[(LUID, u32)],
    primary_only: bool,
    engine_3d_only: bool,
    probe_on: bool,
) -> Option<Vec<i64>> {
    let target: &[(LUID, u32)] = if primary_only && topo.len() > 1 {
        let want = primary_luid();
        let idx = want
            .and_then(|w| {
                topo.iter().position(|(l, _)| {
                    ((l.HighPart as u64) << 32) | l.LowPart as u64 == w
                })
            })
            .unwrap_or_else(|| {
                // 回退：节点数最大者
                topo.iter()
                    .enumerate()
                    .max_by_key(|(_, (_, n))| *n)
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            });
        if probe_on {
            probe::log(&format!("gpu primary picked idx={idx} luid_hi=0x{:08x}", topo[idx].0.HighPart as u32));
        }
        &topo[idx..idx + 1]
    } else {
        topo
    };
    let mut times: Vec<i64> = Vec::new();
    for (ai, (luid, node_count)) in target.iter().enumerate() {
        // 3D-only：节点 0 惯例为 3D/图形引擎（任务管理器默认视图同源）
        let nodes = if engine_3d_only { 1 } else { *node_count };
        for node in 0..nodes {
            let rt = unsafe {
                let mut qn = D3DKMT_QUERYSTATISTICS::default();
                qn.Type = QUERY_TYPE_NODE;
                qn.AdapterLuid = *luid;
                qn.Anonymous.QueryNode = D3DKMT_QUERYSTATISTICS_QUERY_NODE { NodeId: node };
                if D3DKMTQueryStatistics(&qn).is_ok() {
                    // 100ns 累计运行时间
                    qn.QueryResult.NodeInformation.GlobalInformation.RunningTime
                } else {
                    0
                }
            };
            if probe_on {
                probe::log(&format!("gpu adapter {ai} node{node} running_time_100ns={rt}"));
            }
            times.push(rt);
        }
    }
    Some(times)
}

impl Sampler for GpuSampler {
    fn name(&self) -> &'static str {
        "gpu"
    }

    fn id(&self) -> SamplerId {
        SamplerId::Gpu
    }

    fn sample(&mut self, _prev_snap: Option<&Snapshot>, now_ms: u64, out: &mut Snapshot) -> SampleOutcome {
        if !self.available {
            out.gpu = Metric::Unavailable;
            return SampleOutcome::Skipped;
        }
        // D1：拓扑失效（显示器变化）则下次重建
        if TOPO_INVALID.swap(false, AtomicOrdering::SeqCst) {
            self.topology.clear();
        }
        let probe_on = probe::enabled();
        if self.topology.is_empty() {
            match enum_topology(probe_on) {
                Some(t) => self.topology = t,
                None => {
                    // 枚举失败：可能没有 WDDM 2.0 统计支持（部分虚拟机/老驱动）
                    out.gpu = Metric::Unavailable;
                    return SampleOutcome::Transient;
                }
            }
        }

        let Some(times) = collect_running_times(
            &self.topology,
            PRIMARY_ONLY.load(AtomicOrdering::SeqCst),
            ENGINE_3D_ONLY.load(AtomicOrdering::SeqCst),
            probe_on,
        ) else {
            out.gpu = Metric::Unavailable;
            return SampleOutcome::Transient;
        };

        let mut pct: Option<f32> = None;
        if let Some((pms, prev)) = &self.prev {
            let dt = now_ms.saturating_sub(*pms);
            if dt >= MIN_DT_MS && prev.len() == times.len() && !times.is_empty() {
                let mut max_pct: f32 = 0.0;
                for (rt, prt) in times.iter().zip(prev.iter()) {
                    let drt = rt.saturating_sub(*prt);
                    // % = Δ(100ns) / (Δms × 100)
                    let node_pct = (drt as f64 / (dt as f64 * 100.0)) as f32;
                    if node_pct > max_pct {
                        max_pct = node_pct;
                    }
                    if probe_on {
                        probe::log(&format!(
                            "gpu node dt={dt} delta_100ns={drt} pct={node_pct:.1}"
                        ));
                    }
                }
                pct = Some(max_pct.clamp(0.0, 100.0));
            }
        }

        self.prev = Some((now_ms, times));

        if let Some(p) = pct {
            out.gpu = Metric::Value(p);
        } else if self.prev.is_some() {
            // 首轮：只有基线，先给 0 避免 UI 一直 `--`
            out.gpu = Metric::Value(0.0);
        }
        SampleOutcome::Ok
    }
}
