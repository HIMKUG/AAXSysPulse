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
//! 探针（`SYSOVERLAY_PROBE=1`）记录每个适配器的 LUID / 引擎数 / 每引擎
//! RunningTime 与算得的百分比——GPU 一直显示 `--` 时看这里。

use crate::probe;
use sysoverlay_core::sampler::{SampleOutcome, Sampler, SamplerId};
use sysoverlay_core::snapshot::{Metric, Snapshot};
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

/// GPU sampler using D3DKMTQueryStatistics.
pub struct GpuSampler {
    /// 首次硬失败后置 false（诚实降级：驱动不支持 WDDM 2.0 统计时）。
    available: bool,
    /// (上次墙钟 ms, 各引擎 RunningTime 的扁平数组, 引擎数)
    prev: Option<(u64, Vec<i64>)>,
}

impl GpuSampler {
    pub fn new() -> Self {
        Self { available: true, prev: None }
    }

    /// Always returns a sampler; first sample probes driver and degrades honestly.
    pub fn available() -> Self {
        Self::new()
    }
}

impl Default for GpuSampler {
    fn default() -> Self {
        Self::new()
    }
}

/// 一次枚举拿到的全部引擎 RunningTime。
fn collect_running_times(probe_on: bool) -> Option<Vec<i64>> {
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

        let mut times: Vec<i64> = Vec::new();
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
            for node in 0..node_count {
                let mut qn = D3DKMT_QUERYSTATISTICS::default();
                qn.Type = QUERY_TYPE_NODE;
                qn.AdapterLuid = info.AdapterLuid;
                qn.Anonymous.QueryNode = D3DKMT_QUERYSTATISTICS_QUERY_NODE { NodeId: node };
                let rt = if D3DKMTQueryStatistics(&qn).is_ok() {
                    // 100ns 累计运行时间
                    qn.QueryResult.NodeInformation.GlobalInformation.RunningTime
                } else {
                    0
                };
                if probe_on {
                    probe::log(&format!("gpu adapter {ai} node{node} running_time_100ns={rt}"));
                }
                times.push(rt);
            }
        }
        Some(times)
    }
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
        let probe_on = probe::enabled();
        let Some(times) = collect_running_times(probe_on) else {
            // 枚举失败：可能没有 WDDM 2.0 统计支持（部分虚拟机/老驱动）
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
