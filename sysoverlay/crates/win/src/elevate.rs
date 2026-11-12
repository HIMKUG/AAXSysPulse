//! 管理员权限（用户拍板）：
//! - 启动时检测；无管理员且 Power 开启 → 弹窗询问「以管理员身份重启」；
//! - 选是 → `runas` 自重启（弹 UAC），当前进程退出（单实例让位，新进程带
//!   `--elevated` 参数重试抢互斥体）；
//! - 拒绝 UAC / 选否 → **Power 行强制关闭**（禁止显示 + 停止计算）。

use crate::probe;
use aaxsyspulse_core::types::Lang;
use windows::Win32::UI::Shell::IsUserAnAdmin;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONQUESTION, MB_YESNO};

pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

/// 弹窗询问 + runas 自重启。返回 `true` = 已发起重启（调用方应立即退出）。
///
/// `lang` 由调用方从**已加载的配置**传入 —— 本弹窗出现在启动阶段，此时
/// 设置页还没跑过、`global_lang` 仍是默认值，不能依赖它。
/// 文案此前是硬编码中文：英文界面下会整段显示中文。
pub fn prompt_and_relaunch(lang: Lang) -> bool {
    unsafe {
        let text_w: Vec<u16> = crate::i18n::t(lang, "ElevateText")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let caption_w: Vec<u16> = crate::i18n::t(lang, "ElevateCaption")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let choice = MessageBoxW(
            None,
            windows::core::PCWSTR(text_w.as_ptr()),
            windows::core::PCWSTR(caption_w.as_ptr()),
            MB_YESNO | MB_ICONQUESTION,
        );
        if choice != IDYES {
            probe::log("elevate user declined -> power row disabled");
            return false;
        }
        // 自身路径（审查 275：循环扩容防截断 —— 512 定长缓冲对深路径不够）。
        let exe = {
            // 定长数组 + `try_into` 在截断扩容路径上必然失败（1024 → [u16; 512]），
            // 深路径会 panic 而非重试；改用 Vec + resize 直接扩容（审查 275）。
            let mut buf = vec![0u16; 512];
            loop {
                let len = windows::Win32::System::LibraryLoader::GetModuleFileNameW(None, &mut buf);
                let l = len as usize;
                if l < buf.len() || l == 0 {
                    break String::from_utf16_lossy(&buf[..l]);
                }
                // 缓冲写满（可能截断）→ 扩容重试
                buf.resize(buf.len() * 2, 0);
            }
        };
        let exe_w: Vec<u16> = exe.encode_utf16().chain(Some(0)).collect();
        let verb: Vec<u16> = "runas\0".encode_utf16().collect();
        let params: Vec<u16> = "--elevated\0".encode_utf16().collect();

        use windows::Win32::UI::Shell::{
            ShellExecuteExW, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
        };
        let mut sei = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            // 审查 603：与 restart.rs 同口径 —— NOASYNC 保证 exit 前新进程已派生，
            // 否则"两个实例都没了"竞态（SEE_MASK_DEFAULT 是异步）。
            fMask: SEE_MASK_NOASYNC,
            lpVerb: windows::core::PCWSTR(verb.as_ptr()),
            lpFile: windows::core::PCWSTR(exe_w.as_ptr()),
            lpParameters: windows::core::PCWSTR(params.as_ptr()),
            nShow: 1,
            ..Default::default()
        };
        if ShellExecuteExW(&mut sei).is_ok() {
            probe::log("elevate relaunch requested");
            true
        } else {
            probe::log("elevate ShellExecuteExW failed -> power row disabled");
            false
        }
    }
}
