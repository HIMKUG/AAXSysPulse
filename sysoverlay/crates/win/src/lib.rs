//! AAXSysPulse Win32 Platform Layer
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
pub mod render_thread;
pub mod sampler;
pub mod proc_cpu;
pub mod proc_mem;
pub mod proc_gpu;
pub mod proc_net;
pub mod gpu_detect;
pub mod gpu_vendor;
pub mod msr;
pub mod power_system;
pub mod sysinfo;
pub mod elevate;
pub mod logging;
pub mod probe;
pub mod menu;
pub mod dpi;
pub mod config_io;
pub mod tray;
pub mod settings;
pub mod autostart;
pub mod theme;
pub mod perf;
pub mod history;
pub mod i18n;
pub mod titlebar;
pub mod gfx_d2d;
pub mod render_d2d;
pub mod din_bitmap;
pub mod restart;
pub mod app_icon;
pub mod mem_tune;

// Re-export core types for convenience
pub use aaxsyspulse_core::config::Config;
pub use aaxsyspulse_core::snapshot::Snapshot;

// Re-export main window type
pub use window::OverlayWindow;
pub use window::{snap_store, get_shared_seq};
pub use sampler::SystemSampler;
