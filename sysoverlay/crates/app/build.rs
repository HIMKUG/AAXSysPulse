//! Build script for sysoverlay-app
//!
//! Embeds the driver-installer binary as a resource.

use std::env;
use std::path::Path;

fn main() {
    // Path to driver-installer binary
    let driver_installer_path =
        Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("driver-installer")
        .join("bin")
        .join("driver-installer.exe");

    if driver_installer_path.exists() {
        println!("cargo:rerun-if-changed={}", driver_installer_path.display());

        // Embed the binary as a resource
        //
        // On Windows, we use the Windows API to embed the resource.
        // The embedded binary will be written to a temporary file and executed.
        //
        // We just tell Cargo that this file is a dependency
        println!("cargo:warning=Driver installer found at: {}", driver_installer_path.display());

        // For now, we just check the file exists. The actual embedding
        // happens via the include_bytes! macro in the Rust code.
    } else {
        eprintln!(
            "cargo:warning=Driver installer not found at {}. \
             Run 'D:\\code\\sys-overlay-c-master\\sysoverlay\\driver-installer\\build.bat' to build it.",
            driver_installer_path.display()
        );
    }
}
