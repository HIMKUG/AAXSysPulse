//! GPU sampler using PDH (Performance Data Helper) API.
//!
//! Collects GPU utilization by querying the GPU Engine counters.
//! Each instance represents a process using a specific engine node.
//! We group by physical node (extracted from instance name) and return
//! the utilization of the busiest node.

use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterArrayW, PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT_DOUBLE, PDH_MORE_DATA,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use sysoverlay_core::metrics::node_key;
use sysoverlay_core::sampler::{SampleOutcome, Sampler};
use sysoverlay_core::snapshot::{Metric, Snapshot};

/// GPU sampler using PDH API.
pub struct GpuSampler {
    /// PDH query handle
    query: Option<HANDLE>,
    /// GPU utilization counter handle
    gpu_counter: Option<HANDLE>,
    /// Previous time for delta calculation
    prev_time_ms: u64,
    /// Whether PDH is available
    pdh_available: bool,
    /// Current GPU usage
    gpu: Metric<f32>,
}

impl GpuSampler {
    /// Create a new GPU sampler, attempting to initialize PDH.
    pub fn new() -> Self {
        let mut sampler = Self {
            query: None,
            gpu_counter: None,
            prev_time_ms: 0,
            pdh_available: false,
            gpu: Metric::Unavailable,
        };

        if sampler.init_pdh() {
            sampler.pdh_available = true;
        }

        sampler
    }

    /// Create a GPU sampler that always returns Unavailable.
    pub fn unavailable() -> Self {
        Self {
            query: None,
            gpu_counter: None,
            prev_time_ms: 0,
            pdh_available: false,
            gpu: Metric::Unavailable,
        }
    }

    /// Returns the current GPU usage metric.
    pub fn get_gpu(&self) -> Metric<f32> {
        self.gpu
    }

    fn init_pdh(&mut self) -> bool {
        unsafe {
            let mut query: HANDLE = HANDLE::default();
            let result = PdhOpenQueryW(None, 0, &mut query);
            if result != ERROR_SUCCESS.0 as i32 {
                return false;
            }

            let gpu_path = windows::core::w!("\\GPU Engine(*)\\Utilization Percentage");
            let mut gpu_counter: HANDLE = HANDLE::default();

            let result = PdhAddEnglishCounterW(query, gpu_path, 0, &mut gpu_counter);
            if result != ERROR_SUCCESS.0 as i32 {
                PdhCloseQuery(query);
                return false;
            }

            // Collect initial baseline (this first call establishes the baseline)
            let result = PdhCollectQueryData(query);
            if result != ERROR_SUCCESS.0 as i32 {
                PdhCloseQuery(query);
                return false;
            }

            self.query = Some(query);
            self.gpu_counter = Some(gpu_counter);
            true
        }
    }

    fn close_pdh(&mut self) {
        unsafe {
            if let Some(query) = self.query {
                PdhCloseQuery(query);
            }
        }
        self.query = None;
        self.gpu_counter = None;
        self.pdh_available = false;
    }

    fn sample_gpu_internal(&mut self, _now_ms: u64) -> Metric<f32> {
        if !self.pdh_available {
            return Metric::Unavailable;
        }

        unsafe {
            let query = match self.query {
                Some(q) => q,
                None => return Metric::Unavailable,
            };

            let counter = match self.gpu_counter {
                Some(c) => c,
                None => return Metric::Unavailable,
            };

            // Collect new data
            let result = PdhCollectQueryData(query);
            if result != ERROR_SUCCESS.0 as i32 {
                return Metric::Unavailable;
            }

            // Get the formatted counter array
            let mut buffer_size: u32 = 0;
            let mut item_count: u32 = 0;

            // First call to get required buffer size
            let ret = PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut buffer_size,
                &mut item_count,
                None,
            );

            if ret != PDH_MORE_DATA.0 as i32 {
                return Metric::Unavailable;
            }

            if buffer_size == 0 || item_count == 0 {
                return Metric::Unavailable;
            }

            // Allocate buffer
            let mut buffer: Vec<u8> = vec![0u8; buffer_size as usize];

            // Second call to get actual data
            let ret = PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut buffer_size,
                &mut item_count,
                Some(buffer.as_mut_ptr() as *mut _),
            );

            if ret != ERROR_SUCCESS.0 as i32 {
                return Metric::Unavailable;
            }

            // Parse the results - each item is PDH_FMT_COUNTERVALUE_ITEM_W
            let items = std::slice::from_raw_parts(
                buffer.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W,
                item_count as usize,
            );

            // Collect (instance_name, utilization) pairs
            // Group by node key and sum utilization
            let mut node_totals: Vec<(&str, f32)> = Vec::new();

            for item in items {
                let name_ptr = item.szName.as_ptr();
                if name_ptr.is_null() {
                    continue;
                }

                let name_str = wide_to_string(name_ptr);

                // Skip invalid entries
                if item.CStatus != 0 {
                    continue;
                }

                // Get utilization value, clamp negative to 0
                let util = (item.CounterValue.anonymous.doubleValue as f32).max(0.0).min(100.0);

                if let Some(key) = node_key(&name_str) {
                    if let Some(entry) = node_totals.iter_mut().find(|e| e.0 == key) {
                        entry.1 += util;
                    } else {
                        node_totals.push((key, util));
                    }
                }
            }

            // Return max across all nodes
            node_totals.into_iter().map(|(_, total)| total).fold(None, |max, v| {
                Some(max.map_or(v, |m| v.max(m)))
            }).map(Metric::Value).unwrap_or(Metric::Unavailable)
        }
    }
}

impl Default for GpuSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for GpuSampler {
    fn drop(&mut self) {
        self.close_pdh();
    }
}

impl Sampler for GpuSampler {
    fn name(&self) -> &'static str {
        "GPU"
    }

    fn sample(&mut self, _prev: Option<&Snapshot>, now_ms: u64) -> SampleOutcome {
        self.gpu = self.sample_gpu_internal(now_ms);
        self.prev_time_ms = now_ms;

        if matches!(self.gpu, Metric::Unavailable) {
            // If GPU becomes unavailable, it's a transient error (could be TDR)
            SampleOutcome::Transient("GPU sampling failed")
        } else {
            SampleOutcome::Ok
        }
    }
}

/// Convert a wide string pointer to a Rust String
fn wide_to_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }

    unsafe {
        let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    }
}

/// Collect GPU metrics directly (for simple use cases).
pub fn collect_gpu() -> Metric<f32> {
    let mut sampler = GpuSampler::new();
    let metric = sampler.sample_gpu_internal(0);
    sampler.close_pdh();
    metric
}
