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

use sysoverlay_core::cadence::{DELTA_ENTER, DELTA_EXIT, EXIT_TICKS, IDLE_MAX_MS, REFRESH_FLOOR_MS, ACTIVE_CPU_PCT, ACTIVE_NET_BPS};
use sysoverlay_core::scheduler::Scheduler;
use sysoverlay_core::scheduler::Source;
use sysoverlay_win::window::{get_shared_seq, publish_schedule_cfg, schedule_cfg, setup_resume_channel, snap_store, wait_cfg_change};
use sysoverlay_win::OverlayWindow;
use sysoverlay_win::SystemSampler;
use sysoverlay_win::dpi;
use sysoverlay_win::config_io;
use sysoverlay_win::{tray, autostart};

mod pre_launch_checks;
use pre_launch_checks::run_pre_launch_checks;

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
        let restart = std::env::args().any(|a| a == "--restart");
        // 管理员重启 / 托盘「快速重启」路径：带重试抢互斥体，等旧进程让位。
        let attempts = if elevated || restart { 30 } else { 1 };
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

    // ── 2b. Pre-launch checks: admin & driver status ──────────────────────
    // Shows warning dialogs for admin/driver status.
    // User can click Continue (up to 3 times per check) or Exit.
    // Each check is only shown once per week.
    let config_changed = run_pre_launch_checks(&mut cfg);
    if config_changed {
        // Save updated continue counts and dates
        let _ = config_io::save_config(&cfg);
    }

    // ── 2c. 管理员权限（用户拍板：无权限提示+runas 重启，拒绝则关闭 Power 行）──
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
    // P1：渲染线程必须先于窗口创建启动，否则首帧渲染请求无人消费。
    sysoverlay_win::render_thread::start();
    let window = match OverlayWindow::new(cfg) {
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

    // ── 4c. Spawn scheduler-driven sampling thread ────────────────────────
    // 分频调度：GPU/Net 不快于 1s（计数器本身 1s 才更新），
    // Auto 档刷新上限 = 启用源的最大有效间隔，成本超标自动降档。
    // 事件驱动 NIC 重枚举：注册一次网卡变更通知（取代 10s 轮询 GetIfTable2）。
    sysoverlay_win::proc_net::init_net_notify();
    let cfg_for_thread = cfg;
    let shared_seq = get_shared_seq();
    // HWND 不是 Send：跨线程只传裸地址，线程内重建
    let hwnd_addr = window.hwnd().0 as isize;
    let _sampling_thread = thread::spawn(move || {
        sysoverlay_win::probe::log_startup();
        // P0-1：采样线程一次性绑到 0 号逻辑核，MSR 读数语义一致且零强制迁移。
        sysoverlay_win::msr::pin_sampling_thread();
        // P1-16：采样线程降优先级，后台采集不与前台交互抢 CPU。
        unsafe {
            use windows::Win32::System::Threading::{
                GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
            };
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
        let exclude_virtual = cfg_for_thread.exclude_virtual_nic;
        let mut sampler = SystemSampler::new(exclude_virtual);
        let (interval_ms, auto, _, power_iv) = schedule_cfg();
        let mut sched = Scheduler::new(interval_ms, auto);
        sched.set_power_interval(power_iv);
        let overlay_hwnd = windows::Win32::Foundation::HWND(hwnd_addr as *mut _);

        let now_init = get_tick_count_ms();

        let mut last_refresh = now_init.saturating_sub(REFRESH_FLOOR_MS);
        let mut ema_cpu_committed = 0f32;
        let mut ema_mem_committed = 0f32;
        let mut ema_gpu_committed = 0f32;
        let mut ema_net_committed = 0f32;
        let mut quiet_ticks: u32 = 0;
        let mut mode_active = false;
        let mut pending: sysoverlay_win::Snapshot = sysoverlay_win::Snapshot::placeholder(now_init);
        let mut seq: u64 = 0;

        // v3 按需睡眠：上一轮的源启用掩码（检测启停转换 → teardown/基线重置）、
        // 配置代际（设置改动即时唤醒）、Power 行刚关闭的一次性 flush 标记。
        let mut prev_enabled: [bool; 5] = {
            let (_, _, en, _) = schedule_cfg();
            en
        };
        let mut cfg_gen: u64 = 0;
        let mut need_flush = false;

        loop {
            // 覆盖层隐藏时完全停止采样：省 CPU（数值不更新，DWM 无合成）。
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
                while !IsWindowVisible(overlay_hwnd).as_bool() {
                    // E1：事件驱动——挂起等待 resume 信号（托盘显示 / 系统唤醒），
                    // 不再每 1s 轮询一次可见性。显示时 window 线程会发 resume。
                    if resume_rx.recv().is_err() {
                        // 通道已断开（应处在退出路径）：短暂让出后重查，避免忙等
                        thread::sleep(Duration::from_millis(200));
                    }
                }
            }

            // Check for resume notification
            if resume_rx.try_recv().is_ok() {
                sampler.reset_backoff();
            }

            // 设置改动即时生效（原子镜像，无锁；改动态代际 +1 会从 wait_cfg_change 提前唤醒）
            let (interval_ms, auto, enabled, power_iv) = schedule_cfg();
            sched.set_user(interval_ms, auto);
            sched.set_power_interval(power_iv);

            let now_ms = get_tick_count_ms();

            // 源启停转换：禁用 → 释放后端资源（驱动服务/厂商库/缓存）；
            // 启用 → 重置基线防毛刺。Power 行刚关时置 flush 标记，
            // 下一帧把 prev 快照里的旧瓦数覆盖成 Unavailable（行显示 --）。
            if enabled != prev_enabled {
                sampler.on_schedule_change(&prev_enabled, &enabled);
                if prev_enabled[4] && !enabled[4] {
                    need_flush = true;
                }
                prev_enabled = enabled;
            }

            let plan = sched.plan(now_ms, &enabled);
            let power_on = enabled[4];
            // 空计划零开销：无源到期且无需 flush → 不克隆快照、不进采样器。
            let snap = if plan.any() || need_flush {
                need_flush = false;
                sampler.sample(&plan, Some(&pending), now_ms, power_on)
            } else {
                pending.clone()
            };
            let after_ms = get_tick_count_ms();

            if plan.cpu {
                if let Some(v) = snap.cpu.value() {
                    sched.update_ema(Source::Cpu, v);
                    sched.commit_ema(Source::Cpu, v);
                } else if let Some(v) = pending.cpu.value() {
                    // 采样失败但 pending 有旧值：用旧值推进 EMA，避免卡在 0.0
                    sched.update_ema(Source::Cpu, v);
                }
            }
            if plan.mem {
                sched.update_ema(Source::Mem, snap.memory.load_pct);
                sched.commit_ema(Source::Mem, snap.memory.load_pct);
            }
            if plan.gpu {
                if let Some(v) = snap.gpu.value() {
                    sched.update_ema(Source::Gpu, v);
                    sched.commit_ema(Source::Gpu, v);
                }
            }
            if plan.net {
                let v = (snap.net.rx_bps.saturating_add(snap.net.tx_bps)) as f32;
                sched.update_ema(Source::Net, v);
                sched.commit_ema(Source::Net, v);
            }

            pending = snap;

            let since = after_ms.saturating_sub(last_refresh);
            if since >= REFRESH_FLOOR_MS {
                let ema_cpu = sched.ema(Source::Cpu);
                let ema_mem = sched.ema(Source::Mem);
                let ema_gpu = sched.ema(Source::Gpu);
                let ema_net = sched.ema(Source::Net);
                let delta_cpu = (ema_cpu - ema_cpu_committed).abs();
                let delta_mem = (ema_mem - ema_mem_committed).abs();
                let delta_gpu = (ema_gpu - ema_gpu_committed).abs();
                let delta_net = (ema_net - ema_net_committed).abs();
                let max_delta = delta_cpu.max(delta_mem).max(delta_gpu).max(delta_net);

                let cpu_active = ema_cpu >= ACTIVE_CPU_PCT;
                let net_active = ema_net >= ACTIVE_NET_BPS;

                if mode_active {
                    if max_delta < DELTA_EXIT && !cpu_active && !net_active {
                        quiet_ticks += 1;
                        if quiet_ticks >= EXIT_TICKS {
                            mode_active = false;
                            quiet_ticks = 0;
                        }
                    } else {
                        quiet_ticks = 0;
                    }
                } else {
                    if max_delta >= DELTA_ENTER || cpu_active || net_active {
                        mode_active = true;
                        quiet_ticks = 0;
                    }
                }

                let should_commit = if mode_active {
                    true
                } else {
                    max_delta >= DELTA_ENTER || after_ms.saturating_sub(last_refresh) >= IDLE_MAX_MS
                };

                if should_commit {
                    let mut committed = pending.clone();
                    committed.seq = seq;
                    seq += 1;
                    shared_seq.store(committed.seq, std::sync::atomic::Ordering::Relaxed);
                    snap_store(committed.clone());
                    sysoverlay_win::history::push(&committed);
                    last_refresh = after_ms;
                    ema_cpu_committed = ema_cpu;
                    ema_mem_committed = ema_mem;
                    ema_gpu_committed = ema_gpu;
                    ema_net_committed = ema_net;
                    sysoverlay_win::perf::note_barrier_commit(
                        seq,
                        max_delta,
                        mode_active as u32,
                    );
                    // 快照推送 = 确有新内容：走 request_render（自增渲染纪元），
                    // 悬浮窗快速跳过路径据此区分新快照与时钟例行唤醒。
                    sysoverlay_win::gdi::request_render(overlay_hwnd);
                }
            }

            // v3 按需睡眠：睡到下一个源到期（next_wake_ms），配置改动从
            // wait_cfg_change 立即唤醒。全部源关闭 → 30s 慢心跳兜底
            // （可见性变化/系统唤醒走各自的通道，不依赖本循环高频轮询）。
            let now_after = get_tick_count_ms();
            let any_enabled = enabled.iter().any(|&x| x);
            let timeout_ms = if any_enabled {
                sched.next_wake_ms(now_after, &enabled).saturating_sub(now_after).min(30_000).max(1)
            } else {
                30_000
            };
            sysoverlay_win::probe::log_lazy(|| {
                format!(
                    "sleep_ms={} auto={} plan={}{}{}{}{} mode={}",
                    timeout_ms,
                    auto as u8,
                    plan.cpu as u8,
                    plan.mem as u8,
                    plan.gpu as u8,
                    plan.net as u8,
                    plan.power as u8,
                    mode_active as u8,
                )
            });
            cfg_gen = wait_cfg_change(cfg_gen, timeout_ms);
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
