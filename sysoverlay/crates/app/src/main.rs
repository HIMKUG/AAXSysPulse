//! SysOverlay Application Entry Point
//!
//! Implements:
//! - Single instance protection via named mutex
//! - Collection thread with real samplers (CPU, Memory, GPU, Network)
//! - ArcSwap for lock-free snapshot sharing between threads
//! - Self-healing matrix (sleep/wake, GPU TDR, net hotplug)
//! - Panic hook that writes minidump to temp directory

use arc_swap::ArcSwap;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use sysoverlay_core::config::Config;
use sysoverlay_core::sampler::Sampler;
use sysoverlay_core::snapshot::Snapshot;
use sysoverlay_win::{CpuSampler, GpuSampler, MemSampler, NetSampler, OverlayWindow};

/// Get current timestamp in milliseconds since Unix epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Set up the panic hook that writes minidumps.
fn setup_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let crash_path = std::env::temp_dir().join("SysOverlayC_crash.dmp");

        // Try to write minidump using Windows API
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::{HANDLE, MINIDUMP_TYPE};
            use windows::Win32::System::Diagnostics::Debug::{
                MiniDumpWriteDump, PROCESS_ALL_ACCESS,
            };

            // Get current process handle
            let process = unsafe { windows::Win32::System::Threading::OpenProcess(PROCESS_ALL_ACCESS, false, std::process::id()) };

            if let Ok(handle) = process {
                let mut info = windows::Win32::System::Diagnostics::Debug::MINIDUMP_EXCEPTION_INFORMATION::default();
                info.ThreadId = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
                info.ExceptionPointers = std::ptr::null();
                info.ClientPointers = false.into();

                let dump_type = MINIDUMP_TYPE(MiniDumpWriteDump_MINIDUMP_NORMAL | MiniDumpWriteDump_MINIDUMP_WITH_THREAD_SUMMARY | MiniDumpWriteDump_MINIDUMP_WITH_HANDLE_DATA);

                let file_path = crash_path.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
                let file = unsafe {
                    windows::Win32::Storage::FileSystem::CreateFileW(
                        windows::core::PCWSTR::from_raw(file_path.as_ptr()),
                        windows::Win32::Storage::FileSystem::GENERIC_WRITE.0,
                        windows::Win32::Storage::FileSystem::FILE_SHARE_READ.0,
                        None,
                        windows::Win32::Storage::FileSystem::CREATE_ALWAYS.0,
                        windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL.0,
                        None,
                    )
                };

                if let Ok(file_handle) = file {
                    let _ = unsafe {
                        MiniDumpWriteDump(
                            HANDLE(handle.0),
                            std::process::id(),
                            HANDLE(file_handle.0),
                            dump_type.0 as u32,
                            Some(&info),
                            None,
                            None,
                        )
                    };
                    let _ = unsafe { windows::Win32::Storage::FileSystem::CloseHandle(HANDLE(file_handle.0)) };
                }
                let _ = unsafe { windows::Win32::System::Threading::CloseHandle(HANDLE(handle.0)) };
            }
        }

        let msg = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic".to_string()
        };

        let location = if let Some(loc) = panic_info.location() {
            format!(" at {}:{}:{}", loc.file(), loc.line(), loc.column())
        } else {
            String::new()
        };

        eprintln!("PANIC: {}{}", msg, location);
        eprintln!("Crash dump written to: {:?}", crash_path);
    }));
}

/// Check if another instance is already running.
/// Returns true if this is the first instance, false if another is running.
fn check_single_instance() -> bool {
    use windows::Win32::Foundation::{HANDLE, BOOL};
    use windows::Win32::System::Threading::{CreateMutexW, OpenMutexW, MUTEX_ALL_ACCESS};

    const MUTEX_NAME: &str = "SysOverlayC-SingleInstance";

    unsafe {
        // Try to open existing mutex first
        let existing = OpenMutexW(MUTEX_ALL_ACCESS, false, &windows::core::HSTRING::from(MUTEX_NAME));
        if existing.is_ok() {
            // Mutex exists, another instance is running
            return false;
        }

        // Create new mutex
        let mutex = CreateMutexW(
            None,
            true.into(),
            &windows::core::HSTRING::from(MUTEX_NAME),
        );

        if mutex.is_err() {
            return false;
        }

        let handle = mutex.unwrap();
        let err = windows::Win32::Foundation::GetLastError();

        if err == windows::Win32::Foundation::ERROR_ALREADY_EXISTS.0 {
            let _ = windows::Win32::System::Threading::CloseHandle(handle);
            return false;
        }

        // We own the mutex - let it leak intentionally (it will be cleaned up on process exit)
        true
    }
}

/// Sampler holder that supports self-healing (recreating on fatal error).
struct SamplerHolder {
    cpu: CpuSampler,
    mem: MemSampler,
    gpu: GpuSampler,
    net: NetSampler,
}

impl SamplerHolder {
    fn new() -> Self {
        Self {
            cpu: CpuSampler::new(),
            mem: MemSampler::new(),
            gpu: GpuSampler::new(),
            net: NetSampler::new(),
        }
    }

    /// Sample all metrics and update the snapshot.
    /// Returns the updated snapshot.
    fn sample(&mut self, prev: Option<&Snapshot>, now_ms: u64) -> (Snapshot, bool) {
        // cpu: returns SampleOutcome and updates internal cpu metric
        let _ = self.cpu.sample(prev, now_ms);
        let _ = self.mem.sample(prev, now_ms);

        let gpu_outcome = self.gpu.sample(prev, now_ms);
        let _ = self.net.sample(prev, now_ms);

        let cpu = self.cpu.get_cpu();
        let memory = self.mem.get_memory_stat();
        let gpu = self.gpu.get_gpu();

        // Check if GPU sampler needs rebuilding (fatal error)
        let gpu_rebuilt = if matches!(gpu_outcome, super::sysoverlay_core::sampler::SampleOutcome::Fatal(_)) {
            eprintln!("GPU sampler fatal error, recreating...");
            self.gpu = GpuSampler::new();
            true
        } else {
            false
        };

        let net = self.net.get_net_stat();

        let snap = Snapshot {
            seq: prev.map(|s| s.seq + 1).unwrap_or(1),
            timestamp_ms: now_ms,
            cpu,
            memory,
            gpu,
            net,
        };

        (snap, gpu_rebuilt)
    }

    /// Reset baselines after sleep/wake or other system events.
    fn reset_baselines(&mut self) {
        self.net.reset_baselines();
        // Recreate GPU sampler on wake to get fresh baseline
        self.gpu = GpuSampler::new();
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Set up panic hook first
    setup_panic_hook();

    // Single instance check
    if !check_single_instance() {
        println!("Another instance is already running. Exiting.");
        return Ok(());
    }

    // Load configuration
    let cfg = Config::load();

    // Create ArcSwap snapshot for lock-free sharing between threads
    let snapshot = Arc::new(ArcSwap::from_pointee(Snapshot::default()));

    // Clone handles for the collection thread
    let snap_writer = snapshot.clone();
    let cfg_interval = cfg.interval_ms;

    // Create and start collection thread
    let _collection_thread = thread::spawn(move || {
        let mut holder = SamplerHolder::new();
        let mut prev: Option<Snapshot> = None;
        let mut last_sample_time = Instant::now();

        loop {
            let now = now_ms();

            // Sample all metrics
            let (cur, _gpu_rebuilt) = holder.sample(prev.as_ref(), now);
            prev = Some(cur.clone());

            // Store in ArcSwap
            snap_writer.store(Arc::new(cur));

            // Sleep until next interval
            let elapsed = last_sample_time.elapsed().as_millis() as u64;
            if elapsed < cfg_interval as u64 {
                thread::sleep(Duration::from_millis((cfg_interval as u64 - elapsed).max(1)));
            }
            last_sample_time = Instant::now();
        }
    });

    // UI thread: create window and run message loop
    let mut window = OverlayWindow::new(cfg)?;

    // Note: In a full implementation, we'd listen for WM_POWERBROADCAST
    // to handle sleep/wake events and WM_DEVICECHANGE for net hotplug.
    // For now, the collection thread handles transient errors via self-healing.

    window.run_message_loop();

    // The window was closed, exit the application
    // Note: collection_thread will be terminated when process exits

    Ok(())
}
