//! SysOverlay Application Entry Point
//!
//! This is the main executable entry point for the system overlay application.

use sysoverlay_core::config::Config;
use sysoverlay_win::OverlayWindow;

fn main() {
    // Load configuration (placeholder - uses defaults)
    let cfg = Config::default();

    // Create the overlay window
    let window = match OverlayWindow::new(cfg) {
        Ok(window) => window,
        Err(e) => {
            eprintln!("Failed to create overlay window: {}", e);
            std::process::exit(1);
        }
    };

    // Run the message loop - this blocks until the window is closed
    window.run_message_loop();
}
