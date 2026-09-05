#![windows_subsystem = "windows"]
//! SysOverlay Application Entry Point
//!
//! Responsibilities:
//! 1. Single-instance enforcement via `CreateMutexW`
//! 2. Load / persist config via `config_io`
//! 3. Apply DPI awareness before creating windows
//! 4. Spawn sampling thread that drives `SystemSampler` → `snap_store()`
//! 5. Create `OverlayWindow` and run message loop
//! 6. Panic minidump on crash
//! 7. Self-heal: resume notification resets sampler backoff

use std::io::Write;
use std::thread;
use std::time::Duration;

use sysoverlay_core::scheduler::Scheduler;
use sysoverlay_win::window::{get_shared_seq, publish_schedule_cfg, schedule_cfg, setup_resume_channel, snap_store, WM_NEW_SNAPSHOT};
use sysoverlay_win::OverlayWindow;
use sysoverlay_win::SystemSampler;
use sysoverlay_win::dpi;
use sysoverlay_win::config_io;
use sysoverlay_win::{tray, autostart};

/// Enables the panic hook that writes a minidump before aborting.
fn setup_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let location = panic_info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());
        let message = panic_info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic_info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());

        eprintln!("[SysOverlay] PANIC at {}: {}", location, message);

        #[cfg(feature = "minidump")]
        {
            write_minidump(&location, &message);
        }

        let _ = std::io::stderr().write_all(
            format!("[SysOverlay] PANIC at {}: {}\n", location, message).as_bytes(),
        );
    }));
}

#[cfg(feature = "minidump")]
fn write_minidump(_location: &str, _message: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::time::SystemTime;
    use windows::Win32::System::Diagnostics::Debug::{
        MiniDumpWriteDump, MiniDumpNormal, MiniDumpWithIndirectlyReferencedMemory,
        MINIDUMP_EXCEPTION_INFORMATION,
    };
    use windows::Win32::Foundation::{GetCurrentProcess, HANDLE};
    use windows::Win32::System::Threading::GetCurrentProcessId;

    // Build minidump path: %LOCALAPPDATA%\SysOverlayC\dumps\
    let dump_dir = std::env::var_os("LOCALAPPDATA")
        .map(|p| {
            std::path::PathBuf::from(p)
                .join("SysOverlayC")
                .join("dumps")
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    // Format: SysOverlay_YYYY-MM-DD_HH-MM-SS_PID.dmp
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| {
            let secs = d.as_secs();
            let (y, m, d, hh, mm, ss) = (
                ((secs / 31556926 + 1970) as u16),          // year
                ((secs % 31556926) / 2629746 + 1) as u8,   // month
                (((secs % 31556926) % 2629746) / 86400 + 1) as u8, // day
                ((secs % 86400) / 3600) as u8,             // hour
                (((secs % 86400) % 3600) / 60) as u8,      // minute
                ((secs % 86400) % 3600 % 60) as u8,        // second
            );
            (y, m, d, hh, mm, ss)
        });

    let Some((y, m, d, hh, mm, ss)) = now else {
        return;
    };

    let pid = unsafe { GetCurrentProcessId() };
    let name = format!(
        "SysOverlay_{:04}-{:02}-{:02}_{:02}-{:02}-{:02}_{}.dmp",
        y, m, d, hh, mm, ss, pid,
    );

    if std::fs::create_dir_all(&dump_dir).is_err() {
        return;
    }

    let dump_path = dump_dir.join(&name);
    let path_wide: Vec<u16> = OsStr::new(dump_path.to_string_lossy().as_ref())
        .encode_wide()
        .chain(Some(0))
        .collect();

    let process = unsafe { GetCurrentProcess() };
    // MiniDumpWriteDump takes a HANDLE (which wraps a raw pointer in windows-rs)
    let process_handle = HANDLE(process.0);

    let dump_type = MiniDumpNormal | MiniDumpWithIndirectlyReferencedMemory;

    // panic hook — no exception context available, pass null
    let exception_info = std::ptr::null::<MINIDUMP_EXCEPTION_INFORMATION>();

    let result = unsafe {
        MiniDumpWriteDump(
            process_handle,
            pid,
            windows::core::PCWSTR(path_wide.as_ptr()),
            dump_type,
            exception_info,
            None,
            None,
        )
    };

    if result.is_ok() {
        eprintln!("[SysOverlay] Minidump written to: {:?}", dump_path);
    } else {
        eprintln!("[SysOverlay] Failed to write minidump");
    }
}

fn main() {
    setup_panic_hook();

    // ── 0. DPI awareness (must be before any window creation) ─────────────
    let _ = dpi::apply_dpi_awareness();

    // ── 1. Single instance ──────────────────────────────────────────────────
    unsafe {
        use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
        use windows::Win32::System::Threading::CreateMutexW;

        let name = windows::core::w!("SysOverlay_SingleInstance_Mutex");
        let mutex = CreateMutexW(None, true, windows::core::PCWSTR(name.as_ptr()));
        if mutex.is_err() {
            eprintln!("[SysOverlay] Failed to create single-instance mutex");
            std::process::exit(1);
        }
        let handle: HANDLE = mutex.unwrap();

        // 经典判定：CreateMutexW 对已存在的互斥体照样成功（返回同一句柄），
        // 需要用 GetLastError() == ERROR_ALREADY_EXISTS 识别——
        // WaitForSingleObject 对已拥有的互斥体只会 WAIT_TIMEOUT，不能用于此判定。
        if GetLastError() == ERROR_ALREADY_EXISTS {
            eprintln!("[SysOverlay] Another instance is already running. Exiting.");
            let _ = CloseHandle(handle);
            std::process::exit(0);
        }

        let _handle = handle;
    };

    // ── 2. Load config ─────────────────────────────────────────────────────
    let cfg = config_io::load_config();

    // ── 3. Set up resume notification channel ────────────────────────────────
    let resume_rx = setup_resume_channel();

    // ── 4. Create overlay window（先于采样线程：线程需要 hwnd 推送消息）────
    let window = match OverlayWindow::new(cfg.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[SysOverlay] Failed to create overlay window: {}", e);
            std::process::exit(1);
        }
    };

    // ── 4b. Tray icon + autostart sync + 调度参数发布 ─────────────────────
    tray::add(window.hwnd());
    autostart::apply(cfg.autostart);
    publish_schedule_cfg(&cfg);

    // ── 4c. Spawn scheduler-driven sampling thread ────────────────────────
    // 分频调度：GPU/Net 不快于 1s（计数器本身 1s 才更新），
    // Auto 档刷新上限 = 启用源的最大有效间隔，成本超标自动降档。
    let cfg_for_thread = cfg.clone();
    let shared_seq = get_shared_seq();
    // HWND 不是 Send：跨线程只传裸地址，线程内重建
    let hwnd_addr = window.hwnd().0 as isize;
    let _sampling_thread = thread::spawn(move || {
        let exclude_virtual = cfg_for_thread.exclude_virtual_nic;
        let mut sampler = SystemSampler::new(exclude_virtual);
        let (interval_ms, auto, _) = schedule_cfg();
        let mut sched = Scheduler::new(interval_ms, auto);
        let mut last: Option<sysoverlay_win::Snapshot> = None;
        let overlay_hwnd = windows::Win32::Foundation::HWND(hwnd_addr as *mut _);

        loop {
            // Check for resume notification
            if resume_rx.try_recv().is_ok() {
                sampler.reset_backoff();
            }

            // 设置改动即时生效（原子镜像，无锁）
            let (interval_ms, auto, enabled) = schedule_cfg();
            sched.set_user(interval_ms, auto);

            let now_ms = get_tick_count_ms();
            let plan = sched.plan(now_ms, &enabled);

            let t0 = get_tick_count_ms();
            let snap = sampler.sample(&plan, last.as_ref(), now_ms);
            let cost_ms = (get_tick_count_ms() - t0) as u32;
            sched.record_cycle(cost_ms, &enabled);

            shared_seq.store(snap.seq, std::sync::atomic::Ordering::Relaxed);
            snap_store(snap.clone());
            last = Some(snap);

            // 推送式渲染：UI 线程阻塞在 GetMessageW，收到才画
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
                use windows::Win32::Foundation::{LPARAM, WPARAM};
                let _ = PostMessageW(
                    overlay_hwnd,
                    WM_NEW_SNAPSHOT,
                    WPARAM(0),
                    LPARAM(0),
                );
            }

            // 睡到最近的到期源，而不是固定间隔——分频调度的节能关键
            let wake = sched.next_wake_ms(get_tick_count_ms(), &enabled);
            let sleep_ms = wake.saturating_sub(get_tick_count_ms()).max(1).min(5000);
            thread::sleep(Duration::from_millis(sleep_ms));
        }
    });

    // ── 5. Enter message loop ───────────────────────────────────────────────
    window.run_message_loop();

    // ── 6. Save config on exit ────────────────────────────────────────────
    let final_cfg = window.get_config();
    if !config_io::save_config(&final_cfg) {
        eprintln!("[SysOverlay] Warning: failed to save config on exit");
    }
}

/// Returns the current tick count in milliseconds since system boot.
fn get_tick_count_ms() -> u64 {
    unsafe {
        use windows::Win32::System::SystemInformation::GetTickCount64;
        GetTickCount64() as u64
    }
}
