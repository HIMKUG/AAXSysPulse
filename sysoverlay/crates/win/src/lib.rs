//! AAXSysPulse Win32 Platform Layer
//!
//! 本 crate 是主程序的 Win32 平台层，负责：
//!
//! - 悬浮窗的创建与消息循环（layered + toolwindow + 不可激活）
//! - 双渲染后端：GDI 逐帧合成（`gdi` / `render_thread`）与 D2D/DirectWrite（`gfx_d2d`），
//!   含脏矩形优化、D2D 失败回退 GDI 与渲染看门狗
//! - 指标采集（`sampler` + `proc_*`）：CPU / 内存 / 磁盘 / GPU / 网络 / 功耗温度，
//!   其中功耗温度经 WinRing0 内核驱动读 MSR（`msr`，失败即整会话放弃并留痕）
//! - 右键菜单与设置页（`settings`，原生控件 + 自绘图标）
//! - DPI 感知（`dpi`）、托盘与自启（`tray` / `autostart`）
//! - 配置持久化与热重载（`config_io`，与引导程序共用 `core::contract` 的跨进程契约）
//!
//! 与引导程序（`guide-app/`，Tauri）的关系：只有两条通道 —— INI 文件 +
//! `PostMessageW` 自定义消息，号段与键名一律取自 `aaxsyspulse_core::contract`。

// 批次 6·②：非测试代码禁 `unwrap()`（实测本 crate 生产代码 0 处命中，只挡未来回归；
// warn 级不阻塞构建，是否升 deny 属用户决策项）。
#![cfg_attr(not(test), warn(clippy::unwrap_used))]

pub mod window;
pub mod gdi;
pub mod render_thread;
pub mod sampler;
pub mod proc_cpu;
pub mod proc_mem;
pub mod proc_disk;
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
pub use window::{
    snap_store, get_shared_seq, session_locked, sampler_suspended, in_remote_session,
    force_clear_sampler_suspended,
};
pub use sampler::SystemSampler;

#[cfg(test)]
mod profile_guard_tests {
    //! 审查项 9.1 / 7.2.2：`panic = "unwind"` 是**功能依赖**，不是体积调优项。
    //!
    //! `gdi.rs`（D2D 自愈）、`msr.rs`（功耗探测）、`render_thread.rs`（渲染线程帧循环）、
    //! `app/main.rs`（采样循环兜底）四处都包在 `catch_unwind` 里。谁按历史文档
    //! （`docs/history/PHASE5_SIZE_REPORT.md`、`ARCHITECTURE_DESIGN.md`）把 profile
    //! 改回 `abort`，这四条自愈会静默退化成"直接崩进程"，而编译与全部单测**照样通过**
    //! —— 所以必须有一条断言守着。报告建议放 CI，本仓库无 CI ⇒ 落在 `cargo test` 里。

    fn release_section() -> String {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../Cargo.toml"))
            .expect("workspace 根 Cargo.toml 必须与 crate 保持相对位置（见仓库根）");
        let head = "[profile.release]";
        let Some(at) = text.find(head) else {
            panic!("根 Cargo.toml 里没有 [profile.release] 段 —— 断言失效本身就是结论");
        };
        let rest = &text[at + head.len()..];
        let sec = match rest.find("[") {
            Some(next) => &rest[..next],
            None => rest,
        };
        // 只留**赋值行**：根 Cargo.toml 里那段 why-comment 本身就写着
        // "panic=abort 会使其失效"，注释不过滤掉会让下面的 `!contains("abort")`
        // 永远红（实测：第一次跑就是被自己的注释判死的）。
        sec.lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn release_profile_must_keep_unwind() {
        let sec = release_section();
        assert!(
            sec.replace(' ', "").contains("panic=\"unwind\""),
            "[profile.release] 必须是 `panic = \"unwind\"`：D2D/MSR/渲染线程的自愈都靠 catch_unwind（审查项 9.1）\n实际配置：{sec}"
        );
        assert!(
            !sec.contains("abort"),
            "[profile.release] 出现 `abort` 会静默废掉全部 catch_unwind 自愈路径（审查项 9.1）\n实际配置：{sec}"
        );
    }
}
