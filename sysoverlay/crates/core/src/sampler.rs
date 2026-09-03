//! Sampler trait: pluggable metric collectors.
//!
//! Each sampler is responsible for collecting one category of metrics
/// (CPU, GPU, network, etc.). Samplers are `Send` so they can be used
/// in a separate sampling thread.

/// Outcome of a sampling attempt.
pub enum SampleOutcome {
    /// Sampling succeeded; new data is available.
    Ok,
    /// Transient (soft) failure; the sampler should keep its previous
    /// sample and the caller should reuse the last valid snapshot.
    Transient(&'static str),
    /// Fatal (hard) failure; the sampler is broken and must be rebuilt.
    Fatal(&'static str),
}

/// Trait for metric samplers.
///
/// Implementors hold whatever OS handles or state they need and produce
/// a [`Snapshot`][super::snapshot::Snapshot] on each call to `sample`.
pub trait Sampler: Send {
    /// Human-readable name of this sampler (e.g. "cpu", "gpu").
    fn name(&self) -> &'static str;

    /// Perform one sampling cycle.
    ///
    /// `prev` is the previous complete snapshot (if available) and can be
    /// used to compute deltas (e.g. CPU time differences).
    ///
    /// `now_ms` is the current timestamp in milliseconds since Unix epoch.
    ///
    /// Returns [`SampleOutcome`] indicating success or the class of failure.
    fn sample(&mut self, prev: Option<&crate::snapshot::Snapshot>, now_ms: u64) -> SampleOutcome;
}
