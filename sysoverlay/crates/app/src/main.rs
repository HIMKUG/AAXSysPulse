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
use sysoverlay_win::window::{get_shared_seq, publish_schedule_cfg, schedule_cfg, schedule_power, setup_resume_channel, snap_store, WM_NEW_SNAPSHOT};
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

    // ── 1. Single instance（--elevated 重启路径带 3s 重试，等旧进程让位）────
    unsafe {
        use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
        use windows::Win32::System::Threading::CreateMutexW;

        let name = windows::core::w!("SysOverlay_SingleInstance_Mutex");
        let elevated = std::env::args().any(|a| a == "--elevated");
        let attempts = if elevated { 30 } else { 1 };
        let mut owned: Option<HANDLE> = None;
        for _ in 0..attempts {
            let m = CreateMutexW(None, true, windows::core::PCWSTR(name.as_ptr()));
            if m.is_err() {
                eprintln!("[SysOverlay] Failed to create single-instance mutex");
                std::process::exit(1);
            }
            let h: HANDLE = m.unwrap();
            if GetLastError() != ERROR_ALREADY_EXISTS {
                owned = Some(h);
                break;
            }
            let _ = CloseHandle(h);
            if elevated {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        let Some(_handle) = owned else {
            eprintln!("[SysOverlay] Another instance is already running. Exiting.");
            std::process::exit(0);
        };
    };

    // ── 2. Load config ─────────────────────────────────────────────────────
    let mut cfg = config_io::load_config();
    // 调试日志开关：按配置初始化（必须在下面任何 probe::log 之前，
    // 否则本次启动的 cfg / 启动头会被自己吞掉）。设置面板可实时改。
    sysoverlay_win::probe::set_debug(cfg.debug_log);
    // 探针：记录运行时实际生效的配置（裁决「power 分支没跑」这类问题）
    {
        let appdata = std::env::var_os("APPDATA");
        let cfg_exists = appdata
            .as_ref()
            .map(|a| std::path::PathBuf::from(a).join("SysOverlayC\\config.txt").exists())
            .unwrap_or(false);
        sysoverlay_win::probe::log(&format!(
            "cfg show_cpu={} show_mem={} show_gpu={} show_net={} power={} interval={} auto={} exclude_virtual={} power_interval={} appdata_set={} cfg_file_exists={}",
            cfg.show_cpu as u8, cfg.show_mem as u8, cfg.show_gpu as u8, cfg.show_net as u8,
            cfg.power_enabled as u8, cfg.interval_ms, cfg.refresh_auto,
            cfg.exclude_virtual_nic as u8, cfg.power_interval_ms,
            appdata.is_some() as u8, cfg_exists as u8,
        ));
    }

    // ── 2b. 管理员权限（用户拍板：无权限提示+runas 重启，拒绝则关闭 Power 行）──
    if cfg.power_enabled && !sysoverlay_win::elevate::is_admin() {
        if sysoverlay_win::elevate::prompt_and_relaunch() {
            // 已发起管理员重启：立即退出让位（新进程带 --elevated 重试抢互斥体）
            std::process::exit(0);
        }
        // 拒绝提权：强制关闭 Power 行（不显示、不计算）
        cfg.power_enabled = false;
        sysoverlay_win::probe::log("elevate declined -> power_enabled forced off");
    }

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

    // ── 4b'. 诊断模式：SYSOVERLAY_DIAG 自动打开辅助窗口（子串匹配）──
    // 无人值守复现与二分定位用（配合 SYSOVERLAY_PERF=1 / SYSOVERLAY_PROBE=1
    // 收集 UI 线程卡顿与 CPU 占用证据）：
    //   "1"/"all"     → 全部窗口
    //   "settings"    → 仅设置窗口    "graph" → 仅曲线    "procs" → 仅进程榜
    //   ",click"      → 附加：每 3s 自动模拟点击设置页 CPU 开关（复现 UI 不刷新）
    let diag = std::env::var("SYSOVERLAY_DIAG").unwrap_or_default();
    let diag_on = |token: &str| {
        diag == "1" || diag == "all" || diag.split(',').any(|t| t == token)
    };
    if diag_on("settings") {
        sysoverlay_win::settings::open_settings(window.hwnd());
        if diag_on("click") {
            sysoverlay_win::probe::log("diag: auto click loop started");
            thread::spawn(move || {
                // 先等设置窗口完成首帧，再每 3s 模拟点击一次 CPU 开关（共 10 次）
                thread::sleep(Duration::from_millis(2_500));
                sysoverlay_win::probe::log("diag: first click");
                sysoverlay_win::settings::diag_click_cpu();
                for i in 0..9 {
                    thread::sleep(Duration::from_millis(3_000));
                    sysoverlay_win::probe::log(&format!("diag: click #{}", i + 2));
                    sysoverlay_win::settings::diag_click_cpu();
                }
            });
        }
    }
    if diag_on("graph") {
        sysoverlay_win::graph::open();
    }
    if diag_on("procs") {
        sysoverlay_win::proc_list::open();
    }

    // ── 4c. Spawn scheduler-driven sampling thread ────────────────────────
    // 分频调度：GPU/Net 不快于 1s（计数器本身 1s 才更新），
    // Auto 档刷新上限 = 启用源的最大有效间隔，成本超标自动降档。
    let cfg_for_thread = cfg.clone();
    let shared_seq = get_shared_seq();
    // HWND 不是 Send：跨线程只传裸地址，线程内重建
    let hwnd_addr = window.hwnd().0 as isize;
    let _sampling_thread = thread::spawn(move || {
        sysoverlay_win::probe::log_startup();
        let exclude_virtual = cfg_for_thread.exclude_virtual_nic;
        let power_interval = cfg_for_thread.power_interval_ms;
        let mut sampler = SystemSampler::new(exclude_virtual, power_interval);
        let (interval_ms, auto, _) = schedule_cfg();
        let mut sched = Scheduler::new(interval_ms, auto);
        let mut last: Option<sysoverlay_win::Snapshot> = None;
        let overlay_hwnd = windows::Win32::Foundation::HWND(hwnd_addr as *mut _);

        loop {
            // 覆盖层隐藏时完全停止采样：省 CPU + 彻底消除 AFMF 冲突（数值不更新，DWM 无合成）。
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
                while !IsWindowVisible(overlay_hwnd).as_bool() {
                    thread::sleep(Duration::from_secs(1));
                    // 等待显示时用 resume 通知唤醒（系统唤醒/重新显示时发信号）
                    let _ = resume_rx.recv_timeout(Duration::from_secs(5));
                }
            }

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
            let power_on = schedule_power();
            let snap = sampler.sample(&plan, last.as_ref(), now_ms, power_on);
            let cost_ms = (get_tick_count_ms() - t0) as u32;
            sched.record_cycle(cost_ms, &enabled);

            shared_seq.store(snap.seq, std::sync::atomic::Ordering::Relaxed);
            snap_store(snap.clone());
            // 曲线窗口数据源：每轮追加 CPU/内存/网络时序点（ArcSwap 无锁）
            sysoverlay_win::history::push(&snap);
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
            // 采样循环探针：记录每轮采样耗时与休眠时长（sched 忙等/采样超支一眼可见）
            sysoverlay_win::probe::log(&format!(
                "cycle cost_ms={} sleep_ms={} penalty={} auto={} plan={}{}{}{}",
                cost_ms,
                sleep_ms,
                sched.penalty(),
                auto as u8,
                plan.cpu as u8,
                plan.mem as u8,
                plan.gpu as u8,
                plan.net as u8,
            ));
            sysoverlay_win::perf::note_cycle(cost_ms, sleep_ms);
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
