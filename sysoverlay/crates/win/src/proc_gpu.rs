//! GPU sampler using PDH (Performance Data Helper) API.

use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterArrayW, PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT_DOUBLE,
};
use sysoverlay_core::metrics::node_key;
use sysoverlay_core::sampler::{SampleOutcome, Sampler};
use sysoverlay_core::snapshot::{Metric, Snapshot};

/// GPU sampler using PDH API.
pub struct GpuSampler {
    query: isize,
    gpu_counter: isize,
    pdh_available: bool,
    gpu: Metric<f32>,
}

impl GpuSampler {
    /// Create a new GPU sampler.
    pub fn new() -> Self {
        let mut sampler = Self {
            query: 0,
            gpu_counter: 0,
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
            query: 0,
            gpu_counter: 0,
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
            let mut query: isize = 0;
            let result = PdhOpenQueryW(None, 0, &mut query);
            if result != 0 {
                return false;
            }

            let gpu_path = windows::core::w!(r"\GPU Engine(*)\Utilization Percentage");
            let mut gpu_counter: isize = 0;

            let result = PdhAddEnglishCounterW(query, gpu_path, 0, &mut gpu_counter);
            if result != 0 {
                PdhCloseQuery(query);
                return false;
            }

            let result = PdhCollectQueryData(query);
            if result != 0 {
                PdhCloseQuery(query);
                return false;
            }

            self.query = query;
            self.gpu_counter = gpu_counter;
            true
        }
    }

    fn close_pdh(&mut self) {
        if self.query != 0 {
            unsafe { PdhCloseQuery(self.query); }
            self.query = 0;
        }
        self.gpu_counter = 0;
        self.pdh_available = false;
    }

    fn sample_gpu_internal(&mut self) -> Metric<f32> {
        if !self.pdh_available || self.query == 0 {
            return Metric::Unavailable;
        }

        unsafe {
            let result = PdhCollectQueryData(self.query);
            if result != 0 {
                return Metric::Unavailable;
            }

            let mut buffer_size: u32 = 0;
            let mut item_count: u32 = 0;

            let ret = PdhGetFormattedCounterArrayW(
                self.gpu_counter, PDH_FMT_DOUBLE, &mut buffer_size, &mut item_count, None,
            );

            // PDH_MORE_DATA = 0x800007D
            if ret != 0x800007D_u32 {
                return Metric::Unavailable;
            }

            if buffer_size == 0 || item_count == 0 {
                return Metric::Unavailable;
            }

            let mut buffer: Vec<u8> = vec![0u8; buffer_size as usize];

            let ret = PdhGetFormattedCounterArrayW(
                self.gpu_counter, PDH_FMT_DOUBLE, &mut buffer_size, &mut item_count,
                Some(buffer.as_mut_ptr() as *mut _),
            );

            if ret != 0 {
                return Metric::Unavailable;
            }

            let items = std::slice::from_raw_parts(
                buffer.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W,
                item_count as usize,
            );

            // Use owned Strings to avoid lifetime issues
            let mut node_totals: Vec<(String, f32)> = Vec::new();

            for item in items {
                let name_ptr = item.szName.as_ptr();
                if name_ptr.is_null() {
                    continue;
                }

                let name_str = wide_to_string(name_ptr);

                if item.FmtValue.CStatus != 0 {
                    continue;
                }

                let util = (item.FmtValue.Anonymous.doubleValue as f32).max(0.0).min(100.0);

                if let Some(key) = node_key(&name_str) {
                    if let Some(entry) = node_totals.iter_mut().find(|e| e.0 == key) {
                        entry.1 += util;
                    } else {
                        node_totals.push((key.to_string(), util));
                    }
                }
            }

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

    fn sample(&mut self, _prev: Option<&Snapshot>, _now_ms: u64) -> SampleOutcome {
        self.gpu = self.sample_gpu_internal();

        if matches!(self.gpu, Metric::Unavailable) {
            SampleOutcome::Transient("GPU sampling failed")
        } else {
            SampleOutcome::Ok
        }
    }
}

fn wide_to_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe {
        let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    }
}
