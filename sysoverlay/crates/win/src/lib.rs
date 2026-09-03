//! SysOverlay Win32 Platform Layer
//!
//! This crate provides the Win32 platform-specific implementation for
//! the system overlay application. It handles:
//!
//! - Window creation and management (layered, toolwindow, non-activateable)
//! - GDI rendering pipeline
//! - Platform metric collection (CPU, memory, GPU, network)
//! - Context menu handling
//! - DPI awareness
//! - Configuration persistence
//!
//! This is a skeleton implementation that provides minimal functionality
//! to allow compilation and basic operation. Real implementations for
//! metric collection and rendering will be added in later phases.

pub mod window;
pub mod gdi;
pub mod sampler;
pub mod proc_cpu;
pub mod proc_mem;
pub mod proc_gpu;
pub mod proc_net;
pub mod menu;
pub mod dpi;
pub mod config_io;

// Re-export core types for convenience
pub use sysoverlay_core::config::Config;
pub use sysoverlay_core::snapshot::Snapshot;
pub use sysoverlay_core::sampler::Sampler;

// Re-export main window type
pub use window::OverlayWindow;

// Re-export samplers
pub use proc_cpu::CpuSampler;
pub use proc_mem::MemSampler;
pub use proc_gpu::GpuSampler;
pub use proc_net::NetSampler;
