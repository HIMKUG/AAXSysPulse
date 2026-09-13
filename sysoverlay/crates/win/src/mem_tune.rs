//! 进程内存主动适配（配合 Windows 待机页回收，把「启动 ~40MB → 稳态 ~6MB」
//! 的自然回落曲线加速并稳定化）。
//!
//! 两类动作：
//! 1. [`init`]：进程内存优先级降为 VERY_LOW。内存紧张时，内存管理器**优先**
//!    回收本进程的待机页；渲染热页是活跃页，不参与回收裁决，不受影响。
//!    （Win8+，`SetProcessInformation(ProcessMemoryPriority)`，进程级一次性。）
//! 2. [`trim`]：`SetProcessWorkingSetSize(-1, -1)` 主动清空工作集一次——把刚被
//!    大交互（拖拽 / 设置页开关）触碰过、即将转冷的页面立即交还系统。挂接点
//!    均为「交互刚结束」的空闲时机，60s 节流防止多挂接点同帧连发造成
//!    page-fault 风暴；渲染热页下一帧软故障即回，无正确性影响。
//!
//! [`tick_idle_check`] 供采样线程每次唤醒调用：键鼠无输入超过阈值视为
//! 「用户离开」，整个闲置期只修剪一次。挂在采样心跳而非 WM_TIMER——
//! CLOCK/ANIM 定时器随显示状态启停，采样心跳（全源关闭时仍有 30s 慢心跳）
//! 才是常驻周期。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// 两次修剪的最小间隔：多挂接点同帧触发只生效一次，杜绝 fault 风暴。
const TRIM_MIN_INTERVAL_MS: u64 = 60_000;
/// 键鼠闲置多久视为「用户离开」，可修剪一次。
const IDLE_TRIM_AFTER_MS: u32 = 5 * 60_000;

static LAST_TRIM_MS: AtomicU64 = AtomicU64::new(0);
/// 本次闲置期是否已修剪过（输入恢复时复位，防止闲置期间每 60s 反复清空
/// 工作集——闲置时每次修剪都会把采样/渲染热页 fault 回来又赶走，纯浪费）。
static IDLE_TRIMMED: AtomicBool = AtomicBool::new(false);

/// 进程级一次性初始化：降低内存优先级 + 安排「启动稳定」修剪。
pub fn init() {
    set_memory_priority();
    // 启动期触碰过一次就不再用的页（初始化代码/表/字体后备）在 30s 后立即
    // 交还系统，不等自然回落；此刻若用户正在交互，热页软故障即回，无感。
    std::thread::Builder::new()
        .name("mem-tune-startup".into())
        .spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(30_000));
            trim("startup settled");
        })
        .ok();
}

/// 主动清空一次工作集（60s 节流）。返回是否真正执行；`reason` 仅用于探针日志。
pub fn trim(reason: &str) -> bool {
    let now = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
    // check-then-store 无锁竞态：最坏情形两个线程连修两次，第二次近乎空操作，无害。
    if now.saturating_sub(LAST_TRIM_MS.load(Ordering::Relaxed)) < TRIM_MIN_INTERVAL_MS {
        return false;
    }
    LAST_TRIM_MS.store(now, Ordering::Relaxed);
    let before = working_set_mb();
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
        // (SIZE_T)-1 / (SIZE_T)-1：立即把工作集页全部交还系统
        // （psapi EmptyWorkingSet 的本体就是这个调用）。
        let r = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
        match r {
            Ok(()) => {
                crate::probe::log(&format!(
                    "mem_tune: working set trimmed ({reason}) ws {before} -> {}",
                    working_set_mb()
                ));
                true
            }
            Err(e) => {
                crate::probe::log(&format!("mem_tune: trim failed ({reason}) err={:#x}", e.code().0));
                false
            }
        }
    }
}

/// 采样线程周期调用：键鼠无输入 ≥ [`IDLE_TRIM_AFTER_MS`] → 修剪一次。
/// `GetLastInputInfo` 为纳秒级查询，随采样心跳（最快 100ms ~ 最慢 30s）调用零负担。
pub fn tick_idle_check() {
    unsafe {
        use windows::Win32::System::SystemInformation::GetTickCount;
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
        let mut lii = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        if !GetLastInputInfo(&mut lii).as_bool() {
            return;
        }
        // GetTickCount 49.7 天回绕：无符号回绕减法天然正确。
        if GetTickCount().wrapping_sub(lii.dwTime) >= IDLE_TRIM_AFTER_MS {
            // 修剪被 60s 节流挡掉时保持未标记，下个心跳重试直到成功一次。
            if !IDLE_TRIMMED.load(Ordering::Relaxed) && trim("system idle") {
                IDLE_TRIMMED.store(true, Ordering::Relaxed);
            }
        } else {
            IDLE_TRIMMED.store(false, Ordering::Relaxed);
        }
    }
}

fn set_memory_priority() {
    use windows::Win32::System::Threading::{
        GetCurrentProcess, ProcessMemoryPriority, SetProcessInformation, MEMORY_PRIORITY_INFORMATION,
        MEMORY_PRIORITY_VERY_LOW,
    };
    let info = MEMORY_PRIORITY_INFORMATION {
        MemoryPriority: MEMORY_PRIORITY_VERY_LOW,
    };
    let r = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessMemoryPriority,
            &info as *const MEMORY_PRIORITY_INFORMATION as *const core::ffi::c_void,
            std::mem::size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
        )
    };
    match r {
        Ok(()) => crate::probe::log("mem_tune: process memory priority = VERY_LOW"),
        Err(e) => crate::probe::log(&format!(
            "mem_tune: SetProcessInformation failed err={:#x}（优先级保持默认，仅影响回收顺序）",
            e.code().0
        )),
    }
}

/// 当前工作集大小（探针日志用），查询失败返回 "?"。
fn working_set_mb() -> String {
    unsafe {
        use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows::Win32::System::Threading::GetCurrentProcess;
        let mut pmc = PROCESS_MEMORY_COUNTERS::default();
        if GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut pmc,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
        .is_ok()
        {
            format!("{:.1}MB", pmc.WorkingSetSize as f64 / (1024.0 * 1024.0))
        } else {
            "?".into()
        }
    }
}
