//! SysOverlay Core Library
//!
//! Pure Rust core library for the system overlay application.
//! Contains data structures, metrics computation, layout calculation,
//! and sampler traits. Platform-specific implementation lives in other crates.

pub mod snapshot;
pub mod sampler;
pub mod metrics;
pub mod layout;

// Placeholder modules (implemented in 1B phase).
pub mod config;
pub mod color;
pub mod units;
