//! Network sampler — stub implementation.
//!
//! TODO (P0-7): Replace with real `GetIfTable2` / `GetIfTable2Ex` sampling once
//! windows-rs 0.58 API path is confirmed for these types:
//!   - `windows::Win32::NetworkManagement::IpHelper::GetIfTable2`
//!   - `windows::Win32::NetworkManagement::IpHelper::MIB_IF_ROW2`
//!   - `windows::Win32::NetworkManagement::IpHelper::MIB_IF_TYPE`
//!
//! The stub always returns zero rates and `SampleOutcome::Ok`.
//! Integration test will verify the stub compiles; real impl is a follow-up patch.

use sysoverlay_core::sampler::{SampleOutcome, Sampler, SamplerId};
use sysoverlay_core::snapshot::{NetStat, Snapshot};

pub struct NetSampler {
    _exclude_virtual: bool,
}

impl NetSampler {
    pub fn new(exclude_virtual: bool) -> Self {
        Self { _exclude_virtual: exclude_virtual }
    }

    pub fn reset_baselines(&mut self) {
        // No-op for stub.
    }
}

impl Default for NetSampler {
    fn default() -> Self {
        Self::new(true)
    }
}

impl Sampler for NetSampler {
    fn name(&self) -> &'static str {
        "net"
    }

    fn id(&self) -> SamplerId {
        SamplerId::Net
    }

    fn sample(
        &mut self,
        _prev: Option<&Snapshot>,
        now_ms: u64,
        out: &mut Snapshot,
    ) -> SampleOutcome {
        // Stub: return zero rates (real impl pending GetIfTable2 confirmation).
        out.net = NetStat { rx_bps: 0, tx_bps: 0 };
        out.timestamp_ms = now_ms;
        SampleOutcome::Ok
    }
}
