#![windows_subsystem = "windows"]
//! AAX SysPulse Application Entry Point
//!
//! Responsibilities:
//! 1. Single-instance enforcement via `CreateMutexW`
//! 2. Load / persist config via `config_io`
//! 3. Apply DPI awareness before creating windows
//! 4. Spawn sampling thread that drives `SystemSampler` → `snap_store()`
//! 5. Create `OverlayWindow` and run message loop
//! 6. Panic minidump on crash
//! 7. Self-heal: resume notification resets sampler backoff

use std::thread;
use std::time::Duration;

use aaxsyspulse_core::cadence::{DELTA_ENTER, DELTA_EXIT, EXIT_TICKS, IDLE_MAX_MS, REFRESH_FLOOR_MS, ACTIVE_CPU_PCT, ACTIVE_NET_BPS};
use aaxsyspulse_core::scheduler::Scheduler;
use aaxsyspulse_core::scheduler::Source;
use aaxsyspulse_win::window::{get_shared_seq, publish_schedule_cfg, schedule_cfg, schedule_disk, schedule_util, setup_resume_channel, snap_store, update_shared_diag, wait_cfg_change};
use aaxsyspulse_win::OverlayWindow;
use aaxsyspulse_win::SystemSampler;
use aaxsyspulse_win::dpi;
use aaxsyspulse_win::config_io;
use aaxsyspulse_win::{tray, autostart};

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

        // stderr 只写一次：eprintln 自带换行与 flush；此前在 minidump 之后又
        // write_all 了同一条消息，属纯重复输出。
        eprintln!("[AAX SysPulse] PANIC at {}: {}", location, message);

        #[cfg(feature = "minidump")]
        {
            write_minidump(&location, &message);
        }
    }));
}

#[cfg(feature = "minidump")]
fn write_minidump(_location: &str, _message: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, CREATE_ALWAYS, FILE_SHARE_READ,
    };
    use windows::Win32::System::Diagnostics::Debug::{
        MiniDumpWriteDump, MiniDumpNormal, MiniDumpWithIndirectlyReferencedMemory,
    };
    use windows::Win32::System::SystemInformation::GetLocalTime;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId};

    // Build minidump path: %LOCALAPPDATA%\AAXSysPulseC\dumps\
    let dump_dir = std::env::var_os("LOCALAPPDATA")
        .map(|p| {
            std::path::PathBuf::from(p)
                .join("AAXSysPulseC")
                .join("dumps")
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    // Format: AAXSysPulse_YYYY-MM-DD_HH-MM-SS_PID.dmp
    // 本地日历日期必须用 GetLocalTime（与 win::gdi 同源，同样零依赖）。
    // ⚠️ 勿改回 epoch 秒手算年月日：平均年/月长度（31556926s / 2629746s）与
    // 真实日历的偏差会随时间累积，数月后文件名日期就会漂移出真实日期。
    let st = unsafe { GetLocalTime() };

    let pid = unsafe { GetCurrentProcessId() };
    let name = format!(
        "AAXSysPulse_{:04}-{:02}-{:02}_{:02}-{:02}-{:02}_{}.dmp",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, pid,
    );

    if std::fs::create_dir_all(&dump_dir).is_err() {
        return;
    }

    let dump_path = dump_dir.join(&name);
    let path_wide: Vec<u16> = OsStr::new(dump_path.to_string_lossy().as_ref())
        .encode_wide()
        .chain(Some(0))
        .collect();

    // panic hook 里没有异常上下文：exception / user-stream / callback 一律 None。
    // GetCurrentProcess() 返回伪句柄，无需 CloseHandle。
    let process_handle = unsafe { GetCurrentProcess() };
    let dump_type = MiniDumpNormal | MiniDumpWithIndirectlyReferencedMemory;

    // ⚠️ MiniDumpWriteDump 的 hfile 吃的是**文件句柄**，不是路径字符串指针。
    // 旧实现把路径 PCWSTR 直接当 hfile 传——该特性此前从未对本 windows 版本
    // 编译通过，无从暴露；现先 CreateFileW 建文件再传句柄，写完即关。
    let file: HANDLE = match unsafe {
        CreateFileW(
            windows::core::PCWSTR(path_wide.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_READ,
            None,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    } {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[AAX SysPulse] Failed to create minidump file: {e}");
            return;
        }
    };

    let result = unsafe { MiniDumpWriteDump(process_handle, pid, file, dump_type, None, None, None) };
    unsafe {
        let _ = CloseHandle(file);
    }

    if result.is_ok() {
        eprintln!("[AAX SysPulse] Minidump written to: {:?}", dump_path);
    } else {
        eprintln!("[AAX SysPulse] Failed to write minidump");
    }
}

fn main() {
    // ── 0a. 提权辅助模式：只增删计划任务，做完立即退出 ──────────────────
    // ⚠️ 必须在单实例互斥体**之前**：辅助进程由运行中的主进程拉起，
    // 若先抢互斥体就会因"已有实例"静默退出 → 任务永远删不掉（关不掉的自启）。
    if let Some(op) = std::env::args()
        .find(|a| a == autostart::ARG_TASK_CREATE || a == autostart::ARG_TASK_REMOVE)
    {
        let ok = if op == autostart::ARG_TASK_CREATE {
            autostart::create_task()
        } else {
            autostart::delete_task()
        };
        aaxsyspulse_win::probe::log(&format!("autostart helper: {op} ok={ok}"));
        std::process::exit(if ok { 0 } else { 1 });
    }

    setup_panic_hook();

    // ── 0. DPI awareness (must be before any window creation) ─────────────
    let _ = dpi::apply_dpi_awareness();

    // ── 1. Single instance（--elevated 重启路径带 3s 重试，等旧进程让位）────
    unsafe {
        use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
        use windows::Win32::System::Threading::CreateMutexW;

        let name = windows::core::w!("AAXSysPulse_SingleInstance_Mutex");
        let elevated = std::env::args().any(|a| a == "--elevated");
        let restart = std::env::args().any(|a| a == "--restart");
        // 管理员重启 / 托盘「快速重启」路径：带重试抢互斥体，等旧进程让位。
        let attempts = if elevated || restart { 30 } else { 1 };
        let mut owned: Option<HANDLE> = None;
        for _ in 0..attempts {
            let m = CreateMutexW(None, true, windows::core::PCWSTR(name.as_ptr()));
            let Ok(h) = m else {
                eprintln!("[AAX SysPulse] Failed to create single-instance mutex");
                std::process::exit(1);
            };
            let err = GetLastError();
            if err != ERROR_ALREADY_EXISTS {
                owned = Some(h);
                break;
            }
            // 第二次启动（正常模式，非 restart/elevated）：持有互斥体期间通知
            // 主实例显示/开设置，然后立即退出。
            if !elevated && !restart {
                let _ = aaxsyspulse_win::window::handle_second_instance();
                let _ = CloseHandle(h);
                std::process::exit(0);
            }
            let _ = CloseHandle(h);
            if elevated || restart {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        let Some(_handle) = owned else {
            eprintln!("[AAX SysPulse] Another instance is already running. Exiting.");
            std::process::exit(0);
        };
    };

    // （重试间隔在循环内：`--restart` 与 `--elevated` 一样需要 100ms 间隔，
    //   否则 30 次重试在 1ms 内耗尽，旧进程尚未释放互斥体 → 重启静默失败。）

    // ── 2. Load config ─────────────────────────────────────────────────────
    let mut cfg = match config_io::load_config() {
        Ok(cfg) => cfg,
        Err(config_io::ConfigError::VersionSpanTooLarge { min, max }) => {
            // 版本跨度过大：弹窗让用户选择，绝不静默按某个版本解析
            // （否则旧格式文件字段被忽略 → 配置丢失）。
            use std::os::windows::ffi::OsStrExt;
            use windows::Win32::UI::WindowsAndMessaging::{
                MessageBoxW, IDYES, MB_ICONERROR, MB_YESNOCANCEL, MB_OK, MB_ICONINFORMATION,
                MB_SETFOREGROUND,
            };
            let title = "配置版本不兼容";
            let msg = format!(
                "检测到配置文件版本跨度过大（最低 v{}，最高 v{}）。\n\n\
                 若继续用现有文件，部分配置可能无法正确加载。\n\n\
                 选择「是」：用默认配置覆盖当前【所有】配置\n\
                 （system / appearance / monitor / layout_h / layout_v 全部重置为默认值，\n\
                 原文件已自动备份）。\n\
                 选择「否」或「取消」：退出程序，不动任何配置，请手动检查或重新配置。",
                min, max
            );
            let title_w: Vec<u16> =
                std::ffi::OsStr::new(title).encode_wide().chain(std::iter::once(0)).collect();
            let msg_w: Vec<u16> =
                std::ffi::OsStr::new(&msg).encode_wide().chain(std::iter::once(0)).collect();
            let choice = unsafe {
                MessageBoxW(
                    None,
                    windows::core::PCWSTR(msg_w.as_ptr()),
                    windows::core::PCWSTR(title_w.as_ptr()),
                    MB_ICONERROR | MB_YESNOCANCEL | MB_SETFOREGROUND,
                )
            };
            if choice != IDYES {
                // 否 / 取消：退出，不动配置
                eprintln!("[AAX SysPulse] 配置版本跨度过大，用户选择退出（不动配置）。");
                std::process::exit(1);
            }
            // 是：用默认配置覆盖，立即落盘消除版本不一致
            let cfg = aaxsyspulse_core::config::Config::default();
            let _ = config_io::save_config(&cfg);
            let done_w: Vec<u16> = std::ffi::OsStr::new(
                "配置已重置为默认值，原配置已备份在 config 目录。",
            )
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
            let done_title_w: Vec<u16> =
                std::ffi::OsStr::new("配置已重置").encode_wide().chain(std::iter::once(0)).collect();
            unsafe {
                MessageBoxW(
                    None,
                    windows::core::PCWSTR(done_w.as_ptr()),
                    windows::core::PCWSTR(done_title_w.as_ptr()),
                    MB_ICONINFORMATION | MB_OK | MB_SETFOREGROUND,
                );
            }
            cfg
        }
    };
    // 调试日志开关：按配置初始化（必须在下面任何 probe::log 之前，
    // 否则本次启动的 cfg / 启动头会被自己吞掉）。设置面板可实时改。
    aaxsyspulse_win::probe::set_debug(cfg.debug_log);
    // 全局 panic hook：任何线程 panic 都把「位置 + 消息」无条件写进统一日志。
    // 没有它时，wndproc（extern "system"）里的 panic 会跨 FFI 边界直接 abort，
    // 表现就是「开个设置窗、点一下按钮整个进程无声消失」，日志里毫无痕迹。
    aaxsyspulse_win::probe::install_panic_hook();
    // 探针：记录运行时实际生效的配置（裁决「power 分支没跑」这类问题）
    {
        let appdata = std::env::var_os("APPDATA");
        let cfg_exists = appdata
            .as_ref()
            .map(|a| {
                std::path::PathBuf::from(a)
                    .join("AAXSysPulseC\\config\\system.cfg")
                    .exists()
            })
            .unwrap_or(false);
        aaxsyspulse_win::probe::log(&format!(
            "cfg show_cpu={} show_mem={} show_gpu={} show_net={} show_util={} power={} interval={} auto={} exclude_virtual={} power_interval={} appdata_set={} cfg_file_exists={}",
            cfg.show_cpu as u8, cfg.show_mem as u8, cfg.show_gpu as u8, cfg.show_net as u8,
            cfg.show_util as u8,
            cfg.power_enabled as u8, cfg.interval_ms, cfg.refresh_auto,
            cfg.exclude_virtual_nic as u8, cfg.power_interval_ms,
            appdata.is_some() as u8, cfg_exists as u8,
        ));
    }

    // 全局语言同步：托盘提示、提权弹窗等"窗口外"文案依赖它。
    // 默认 En，这里按配置对齐；设置页切换语言时会再同步一次。
    aaxsyspulse_win::i18n::set_global_lang(cfg.lang);

    // ── 2b. Pre-launch checks: admin & driver status ──────────────────────
    // 登录自启（--autostart）时**完全静默**：登录瞬间不该弹任何模态框，
    // 也不该因为用户点了「取消」就让自启变成「启动了但立刻退出」。
    let at_logon = autostart::launched_at_logon();
    if at_logon {
        aaxsyspulse_win::probe::log("pre-launch checks skipped (--autostart quiet mode)");
    } else {
        let outcome = run_pre_launch_checks(&mut cfg);
        if outcome.changed {
            // Save updated continue counts and dates
            let _ = config_io::save_config(&cfg);
        }
        if !outcome.proceed {
            // 弹窗承诺 "Click Cancel to exit" —— 旧实现把它当成"配置没改"，
            // 用户点了取消程序照样启动（语义错位）。
            aaxsyspulse_win::probe::log("pre-launch: user chose exit");
            return;
        }
    }

    // ── 2c. 管理员权限（用户拍板：无权限提示+runas 重启，拒绝则关闭 Power 行）──
    // 自启进程例外：`Run` 项启动**拿不到管理员**，此处不弹 UAC、也**不落盘**
    // 关闭 Power —— 只在本次会话里关掉功耗行，用户下次手动以管理员启动时
    // 功耗照常回来（否则"开了自启 → 功耗监控永久消失"）。
    let mut power_restore_at_exit: Option<bool> = None;
    if cfg.power_enabled && !aaxsyspulse_win::elevate::is_admin() {
        if at_logon {
            aaxsyspulse_win::probe::log(
                "autostart: not elevated -> power row off for this session only (not persisted)",
            );
            power_restore_at_exit = Some(cfg.power_enabled);
            cfg.power_enabled = false;
        } else {
            // 传入配置里的语言：本弹窗在启动阶段显示，早于设置页，
            // 全局语言此时还是默认值，必须显式传。
            if aaxsyspulse_win::elevate::prompt_and_relaunch(cfg.lang) {
                // 已发起管理员重启：立即退出让位（新进程带 --elevated 重试抢互斥体）
                std::process::exit(0);
            }
            // 拒绝提权：强制关闭 Power 行（不显示、不计算）
            cfg.power_enabled = false;
            aaxsyspulse_win::probe::log("elevate declined -> power_enabled forced off");
        }
    }

    // ── 3. Set up resume notification channel ────────────────────────────────
    let resume_rx = setup_resume_channel();

    // ── 4. Create overlay window（先于采样线程：线程需要 hwnd 推送消息）────
    // P1：渲染线程必须先于窗口创建启动，否则首帧渲染请求无人消费。
    // 先发布配置镜像（不投递请求）：让渲染线程一起来就按正确的字重/字号预热
    // DWrite 字体（首次解析实测 317ms），与下面建窗/托盘/自启初始化并行。
    aaxsyspulse_win::render_thread::publish_cfg(&cfg);
    aaxsyspulse_win::render_thread::start();
    let window = match OverlayWindow::new(cfg) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[AAX SysPulse] Failed to create overlay window: {}", e);
            std::process::exit(1);
        }
    };

    // ── 4b. Tray icon + autostart sync + 调度参数发布 ─────────────────────
    tray::add(window.hwnd());
    // 启动时幂等同步一次：既修「exe 被移动/改名后路径失效」，
    // 也修「上次关闭没写成功」的注册表残留。结果记进日志便于排障。
    let autostart_ok = autostart::apply(cfg.autostart);
    aaxsyspulse_win::probe::log(&format!(
        "autostart: sync want={} ok={} mode={:?} os_state={:?} elevated={} launched_at_logon={}",
        cfg.autostart,
        autostart_ok,
        autostart::mode(),
        autostart::state(),
        aaxsyspulse_win::elevate::is_admin(),
        at_logon
    ));
    publish_schedule_cfg(&cfg);

    // ── 4b''. 内存主动适配：内存优先级 VERY_LOW + 启动稳定修剪（见 mem_tune 模块头）──
    aaxsyspulse_win::mem_tune::init();

    // ── 4b'. 诊断模式：AAXSYSPULSE_DIAG 自动打开辅助窗口（子串匹配）──
    // 无人值守复现与二分定位用（配合 AAXSYSPULSE_PERF=1 / AAXSYSPULSE_PROBE=1
    // 收集 UI 线程卡顿与 CPU 占用证据）：
    //   "1"/"all"     → 全部窗口
    //   "settings"    → 仅设置窗口    "graph" → 仅曲线    "procs" → 仅进程榜
    //   ",click"      → 附加：每 3s 自动模拟点击设置页 CPU 开关（复现 UI 不刷新）
    let diag = std::env::var("AAXSYSPULSE_DIAG").unwrap_or_default();
    let diag_on = |token: &str| {
        diag == "1" || diag == "all" || diag.split(',').any(|t| t == token)
    };
    if diag_on("settings") {
        aaxsyspulse_win::settings::open_settings(window.hwnd());
        if diag_on("click") {
            aaxsyspulse_win::probe::log("diag: auto click loop started");
            thread::spawn(move || {
                // 先等设置窗口完成首帧，再每 3s 模拟点击一次 CPU 开关（共 10 次）
                thread::sleep(Duration::from_millis(2_500));
                aaxsyspulse_win::probe::log("diag: first click");
                aaxsyspulse_win::settings::diag_click_cpu();
                for i in 0..9 {
                    thread::sleep(Duration::from_millis(3_000));
                    aaxsyspulse_win::probe::log(&format!("diag: click #{}", i + 2));
                    aaxsyspulse_win::settings::diag_click_cpu();
                }
            });
        }
    }

    // ── 4c. Spawn scheduler-driven sampling thread ────────────────────────
    // 分频调度：GPU/Net 不快于 1s（计数器本身 1s 才更新），
    // Auto 档刷新上限 = 启用源的最大有效间隔，成本超标自动降档。
    // 事件驱动 NIC 重枚举：注册一次网卡变更通知（取代 10s 轮询 GetIfTable2）。
    aaxsyspulse_win::proc_net::init_net_notify();
    let cfg_for_thread = cfg;
    let shared_seq = get_shared_seq();
    // HWND 不是 Send：跨线程只传裸地址，线程内重建
    let hwnd_addr = window.hwnd().0 as isize;
    let _sampling_thread = thread::spawn(move || {
        aaxsyspulse_win::probe::log_startup();
        // P0-1：采样线程一次性绑到 0 号逻辑核，MSR 读数语义一致且零强制迁移。
        aaxsyspulse_win::msr::pin_sampling_thread();
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
        let mut last_sample_ms = now_init;

        let mut last_refresh = now_init.saturating_sub(REFRESH_FLOOR_MS);
        let mut ema_cpu_committed = 0f32;
        let mut ema_mem_committed = 0f32;
        let mut ema_gpu_committed = 0f32;
        let mut ema_net_committed = 0f32;
        let mut quiet_ticks: u32 = 0;
        let mut mode_active = false;
        let mut pending: aaxsyspulse_win::Snapshot = aaxsyspulse_win::Snapshot::placeholder(now_init);
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
            // 采样线程暂停（系统即将睡眠 / 会话已锁定）：低频轮询等待恢复信号，
            // 避免睡眠期间空转 CPU。注意窗口隐藏时采样线程已通过 IsWindowVisible
            // 挂起；这里额外检测 SAMPLER_SUSPENDED 是双保险兜底。
            while aaxsyspulse_win::sampler_suspended() {
                // 100ms 轮询恢复信号，不使用 recv() 阻塞——让窗口隐藏时的
                // resume_rx 信号也能唤醒本线程。
                if resume_rx.recv_timeout(std::time::Duration::from_millis(100)).is_err() {
                    // 无信号，超时继续等待（sampler_suspended 变 false 前每轮睡 100ms）
                }
                // 即使收到信号也再循环一次，让 try_recv() 消费掉后再继续
            }

            // 覆盖层隐藏时完全停止采样：省 CPU（数值不更新，DWM 无合成）。
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
                while !IsWindowVisible(overlay_hwnd).as_bool() {
                    // E1：事件驱动——挂起等待 resume 信号（托盘显示 / 系统唤醒 / 全屏恢复）。
                    // 1s 超时兜底：任何显示路径漏发信号时，最多 1s 后重查可见性自愈，
                    // 不再可能永久卡死（2026-09-14 实测：全屏恢复漏发 → 采样冻结 3 小时）。
                    let _ = resume_rx.recv_timeout(Duration::from_millis(1000));
                }
            }

            // Check for resume notification
            if resume_rx.try_recv().is_ok() {
                sampler.reset_backoff();
            }

            // 兜底唤醒检测（WM_POWERBROADCAST 可能不触发，如 Modern Standby）：
            // 若距离上次采样的时间差超过正常间隔的 3 倍，说明系统刚从睡眠恢复但通知丢了，
            // 自动触发恢复逻辑（重置退避）。
            let now_before_wait = get_tick_count_ms();
            const FALLBACK_WAKE_GAP_MS: u64 = (REFRESH_FLOOR_MS as u64) * 3;
            if now_before_wait.saturating_sub(last_sample_ms) > FALLBACK_WAKE_GAP_MS {
                sampler.reset_backoff();
                aaxsyspulse_win::probe::log("sampler: fallback wake detected — gap exceeded 3x normal interval");
            }

            // 会话锁定（BUG-1）：锁屏安全桌面不显示悬浮窗，采样纯烧 CPU。
            // 挂起等待解锁信号（window 线程 UNLOCK 时发 resume），零轮询；
            // 解锁唤醒后 reset_backoff 并直达采样——立即补采消除数据空白窗口。
            // 5s 超时兜底：解锁信号万一丢失，最多 5s 后重查状态，不会永久阻塞。
            if aaxsyspulse_win::window::session_locked() {
                if resume_rx.recv_timeout(Duration::from_secs(5)).is_ok() {
                    sampler.reset_backoff();
                }
            }

            // 设置改动即时生效（原子镜像，无锁；改动态代际 +1 会从 wait_cfg_change 提前唤醒）
            let (interval_ms, auto, enabled, power_iv) = schedule_cfg();
            sched.set_user(interval_ms, auto);
            sched.set_power_interval(power_iv);

            let now_ms = get_tick_count_ms();

            // 渲染看门狗：停摆 → 重建渲染线程（自愈）。
            // 一帧在途 >5s（线程卡在原生调用）即 respawn，把冻结从 ~30s 大幅缩短；
            // 新线程仍卡死则判为 D2D 原生卡死，禁用 D2D 并持久化回退 GDI。
            match aaxsyspulse_win::gdi::render_watchdog_tick(now_ms) {
                aaxsyspulse_win::gdi::WatchdogAction::Respawn => {
                    aaxsyspulse_win::render_thread::respawn();
                }
                aaxsyspulse_win::gdi::WatchdogAction::DisableD2d => {
                    aaxsyspulse_win::gdi::disable_d2d_and_persist(overlay_hwnd);
                    aaxsyspulse_win::render_thread::respawn();
                }
                aaxsyspulse_win::gdi::WatchdogAction::None => {}
            }

            // 内存自适应：键鼠闲置 ≥5min → 修剪一次工作集（单次闲置期只修一次，
            // 60s 节流在函数内）。挂在采样心跳而非 WM_TIMER：CLOCK/ANIM 定时器
            // 随显示状态启停，采样心跳（全源关闭仍有 30s 慢心跳）才是常驻周期。
            aaxsyspulse_win::mem_tune::tick_idle_check();

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
            let util_on = schedule_util();
            let disk_on = schedule_disk();
            // Util 数据源保障：show_util 开启时强制 CPU/GPU 采样（即使行关闭也不断源）。
            // Memory 的 load_pct 始终有值（来自 prev 快照），无需强制进计划。
            let mut plan = plan;
            if util_on {
                if !plan.cpu {
                    plan.cpu = true;
                }
                if !plan.gpu {
                    plan.gpu = true;
                }
            }
            // 空计划零开销：无源到期且无需 flush → 不克隆快照、不进采样器。
            let snap = if plan.any() || need_flush {
                need_flush = false;
                sampler.sample(&plan, Some(&pending), now_ms, power_on, util_on, disk_on)
            } else {
                pending.clone()
            };
            // 同步诊断数据到共享区（设置窗口可通过消息请求读取）
            update_shared_diag(&sampler.diag);
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
                    aaxsyspulse_win::history::push(&committed);
                    last_refresh = after_ms;
                    ema_cpu_committed = ema_cpu;
                    ema_mem_committed = ema_mem;
                    ema_gpu_committed = ema_gpu;
                    ema_net_committed = ema_net;
                    aaxsyspulse_win::perf::note_barrier_commit(
                        seq,
                        max_delta,
                        mode_active as u32,
                    );
                    if aaxsyspulse_win::probe::enabled() {
                        aaxsyspulse_win::probe::log(&format!(
                            "commit seq={seq} mode={} max_delta={max_delta:.1} util={:?} power={:?} net={} cpu={:?}",
                            mode_active as u8,
                            committed.util,
                            committed.power,
                            committed.net.rx_bps.saturating_add(committed.net.tx_bps),
                            committed.cpu,
                        ));
                    }
                    // 快照推送 = 确有新内容：走 request_render（自增渲染纪元），
                    // 悬浮窗快速跳过路径据此区分新快照与时钟例行唤醒。
                    aaxsyspulse_win::gdi::request_render(overlay_hwnd);
                }
            }

            // v3 按需睡眠：睡到下一个源到期（next_wake_ms），配置改动从
            // wait_cfg_change 立即唤醒。全部源关闭 → 30s 慢心跳兜底
            // （可见性变化/系统唤醒走各自的通道，不依赖本循环高频轮询）。
            let now_after = get_tick_count_ms();
            // 记录本次采样时间戳，用于兜底唤醒检测（两次采样间隔超 3 倍正常值时自动重置退避）。
            last_sample_ms = now_after;

            // 远程会话中降低采样频率：把超时翻倍（原来 1s 变成 2s，原来 2s 变成 4s），
            // 减少资源占用（WM_POWERBROADCAST 在远程会话中不一定触发，靠通知驱动不可能，
            // 所以在兜底检测里额外照顾）。
            let any_enabled = enabled.iter().any(|&x| x);
            let base_timeout_ms: u64 = if any_enabled {
                sched.next_wake_ms(now_after, &enabled).saturating_sub(now_after).min(30_000u64).max(1u64)
            } else {
                30_000u64
            };
            let remote_factor: u64 = if aaxsyspulse_win::in_remote_session() { 2 } else { 1 };
            let timeout_ms: u32 = (base_timeout_ms * remote_factor) as u32;
            aaxsyspulse_win::probe::log_lazy(|| {
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
            cfg_gen = wait_cfg_change(cfg_gen, timeout_ms as u64);
        }
    });

    // ── 5. Enter message loop ───────────────────────────────────────────────
    window.run_message_loop();

    // ── 6. Save config on exit ────────────────────────────────────────────
    let mut final_cfg = window.get_config();
    // 自启静默模式下的「会话内关闭功耗行」不是用户意图 → 退出时还原，不落盘。
    // 若用户在会话里**主动**开了功耗（值已是 true），则以用户为准，不覆盖。
    if let Some(orig) = power_restore_at_exit {
        if !final_cfg.power_enabled {
            final_cfg.power_enabled = orig;
            aaxsyspulse_win::probe::log("exit: restored power_enabled (autostart session-only off)");
        }
    }
    if !config_io::save_config(&final_cfg) {
        eprintln!("[AAX SysPulse] Warning: failed to save config on exit");
    }
}

/// Returns the current tick count in milliseconds since system boot.
fn get_tick_count_ms() -> u64 {
    unsafe {
        use windows::Win32::System::SystemInformation::GetTickCount64;
        GetTickCount64() as u64
    }
}
