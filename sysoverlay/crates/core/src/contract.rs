//! 跨进程契约唯一真源（审查项 7.1.1 / 7.1.2 / 7.1.3 / 1.2.1 / 3.1.14 → P1-22）
//!
//! # 为什么单独一个模块
//!
//! 主程序（`AAXSysPulse.exe`）与引导程序（`AAXSysPulse-Guide.exe`）是**两个进程**，
//! 它们之间只靠三样东西对齐：磁盘上的 INI 配置文件、Win32 窗口类名 + 自定义消息号，
//! 以及一组系统侧名字（计划任务名、Run 键值名、驱动服务名……）。
//! 这些名字过去各有 3–4 份独立字面量副本，**改一端忘另一端不会编译失败、不会测试失败、
//! 不会有任何告警**，只在运行时静默失联 —— 用户看到的就是"改了设置没反应""卸载没清干净"。
//!
//! 本模块把这些事实收敛成一处。`aaxsyspulse-core` 零依赖 ⇒ 引导 path 依赖它不会把
//! `windows` 0.58 / 0.61 两个版本拉进同一次编译（见 4.1.5）。
//!
//! # 使用纪律
//!
//! - 需要 `&str`（拼路径、转宽字符）→ 直接用本模块常量。
//! - 需要 `PCWSTR` → 用 `wide_of` 在调用点做运行期转换，**不要**再写 `w!("…")`：
//!   `w!` 只接受编译期字面量，每写一次就多存一份不可检测的副本。
//! - 名字必须保持 ASCII（单测 `names_are_ascii_for_lossless_wide_literals`），
//!   这样"宽字符副本 == 字面量副本"这类长度/比较优化才成立。
//! - 从两个基础名（`APP_TECH_NAME` / `APP_USER_NAME`）派生的项由单测
//!   `derived_names_track_the_base_names` 钉死，改一处即红。

/// 悬浮窗窗口类名。主程序注册处：`win/src/window.rs`。
pub const OVERLAY_WINDOW_CLASS: &str = "AAXSysPulse_WndClass";
/// 设置窗口类名。注册处：`win/src/settings/mod.rs`。
pub const SETTINGS_WINDOW_CLASS: &str = "AAXSysPulse_Settings";
/// 悬浮窗窗口标题（仅用于任务管理器/辅助工具识别，不注册类名）。
pub const OVERLAY_WINDOW_TITLE: &str = APP_USER_NAME;

// ───────────────────────── IPC 消息号段 ─────────────────────────

/// 自定义消息基址（Win32 `WM_APP`，合法区间 0x8000–0xBFFF）。
pub const WM_APP: u32 = 0x8000;
/// 本应用占用的号段上界（含）：所有 `WM_*` 必须落在 `WM_APP ..= WM_APP + 63`。
pub const WM_APP_SEG_MAX: u32 = WM_APP + 63;

/// 托盘回调消息。
pub const WM_TRAY: u32 = WM_APP + 1;
/// 设置面板改动通知：重排版 + 自适应窗口尺寸。
pub const WM_RECONFIG: u32 = WM_APP + 2;
/// 采样线程新快照通知（推送式渲染）。
pub const WM_NEW_SNAPSHOT: u32 = WM_APP + 3;
/// 设置窗口请求诊断数据：Settings → Overlay。
pub const WM_GET_SAMPLER_DIAG: u32 = WM_APP + 4;
/// Overlay 回复诊断数据。LPARAM = `*const SamplerDiag`。
pub const WM_SAMPLER_DIAG: u32 = WM_APP + 5;
/// 二次启动时请求第一实例打开设置页。
pub const WM_OPEN_SETTINGS: u32 = WM_APP + 6;
/// D2D 自愈回退：由 UI 线程改写 Config 并落盘。
pub const WM_DISABLE_D2D: u32 = WM_APP + 7;
/// **引导 → 主程序**：重新读取配置文件并刷新悬浮窗。
pub const WM_RELOAD_CONFIG: u32 = WM_APP + 8;
/// **引导 → 主程序**：快速重启（先落盘，再拉起新实例并退出）。
pub const WM_QUICK_RESTART: u32 = WM_APP + 9;
/// **后台线程 → UI 线程**：开机自启开关的异步应用结果。
/// `WPARAM` = 请求值（1=开 / 0=关），`LPARAM` = 是否成功。
/// 发送方 `win::autostart::apply_user_async`（阻塞操作必须离开 UI 线程，
/// 见该函数文档）；接收方 `win::window` 的 wndproc。
pub const WM_AUTOSTART_DONE: u32 = WM_APP + 10;
/// 诊断：在悬浮窗上点击某个区域。与上面 1–9 同一基址，但号段靠后，
/// 避免和"外部程序可投递"的 8/9 混在一起。
pub const WM_DIAG_CLICK: u32 = WM_APP + 41;

// 号段约束放在**编译期**（批次 6 clippy 分类 / 勘误：这里原先是 `#[test]` 里两条
// `assert!(常量比较)` —— 两侧操作数都是常量，运行时**永远不可能失败**，属假测试）。
// 与 `cadence.rs` / `theme.rs` 同法：越界直接变编译错误，而不是等某次 `cargo test`。
const _: () = assert!(WM_APP >= 0x8000 && WM_APP <= 0xBFFF);
const _: () = assert!(WM_DIAG_CLICK <= WM_APP_SEG_MAX, "越过号段上界");

// ───────────────────────── 磁盘 / 系统侧名字 ─────────────────────────

/// 技术名（目录、exe、快捷方式、注册表键前缀）。
pub const APP_TECH_NAME: &str = "AAXSysPulse";
/// 用户可见名（窗口标题、托盘提示、启动项显示名）。
pub const APP_USER_NAME: &str = "AAX SysPulse";

/// 主程序可执行文件名。
pub const MAIN_EXE_NAME: &str = "AAXSysPulse.exe";
/// 引导可执行文件名（卸载注册表 `UninstallString` 指向它）。
pub const GUIDE_EXE_NAME: &str = "AAXSysPulse-Guide.exe";
/// 安装目录名：`%LOCALAPPDATA%\AAXSysPulse\`。
pub const INSTALL_DIR_NAME: &str = APP_TECH_NAME;
/// 配置 / 日志目录名：`%APPDATA%\AAXSysPulseC\`。
/// 尾 `C` 是历史包袱（v1 时代的 "Client"），统一掉需要一次性迁移（见 1.1.8），
/// 因此这里显式与 `APP_TECH_NAME` 区分，并由单测钉死差异是有意的。
pub const APP_DATA_DIR_NAME: &str = "AAXSysPulseC";

/// 驱动文件名。
pub const DRIVER_FILE_NAME: &str = "WinRing0x64.sys";
/// 驱动落地目录（引导写、主程序按同一路径打开）。
pub const DRIVER_DIR: &str = "C:\\Windows\\AAXSysPulse\\";
/// 驱动服务名（`CreateServiceW` / `OpenServiceW` 两端同名）。
pub const DRIVER_SERVICE_NAME: &str = "AAXSysPulseWinRing0";
/// 驱动服务显示名（services.msc 里用户看到的名字）。
/// 建服务的有两条路径（引导 `driver.rs` 正常安装、主程序 `msr.rs` 自愈重建），
/// 取**先建者胜**：两端不同名时用户看到的名字取决于哪条路径先跑，故收敛到一处。
pub const DRIVER_DISPLAY_NAME: &str = "AAXSysPulse WinRing0 Driver";

/// HKCU 登录自启键（主程序与引导都往这里写，靠不同的值名区分）。
pub const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// 任务管理器「启动」页记录的启用/禁用状态（12 字节二进制块）所在键。
pub const APPROVED_RUN_KEY: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";
/// 计划任务名（`schtasks /Create /TN`，主程序 `autostart` 写入方）。
pub const TASK_NAME: &str = APP_USER_NAME;
/// 历史版本用过的无空格任务名 —— 只用于卸载时清理孤儿任务，**不再写入**。
pub const TASK_NAME_LEGACY: &str = APP_TECH_NAME;
/// HKCU Run 键值名（主程序自启，含空格，与任务名一致）。
pub const RUN_VALUE_MAIN: &str = APP_USER_NAME;
/// HKCU Run 键值名（引导自启）。与 `RUN_VALUE_MAIN` **必须不同**，
/// 否则两程序互相覆盖对方的启动命令。
pub const RUN_VALUE_GUIDE: &str = "AAXSysPulseGuide";
/// 卸载注册表子键。
pub const UNINSTALL_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\AAXSysPulse";
/// 桌面快捷方式文件名（创建方 `guide/shortcut.rs`，删除方 `guide/install.rs` 卸载清单）。
pub const SHORTCUT_FILE_NAME: &str = "AAXSysPulse.lnk";
/// 开始菜单程序文件夹名（同上，路径为 `%APPDATA%\Microsoft\Windows\Start Menu\Programs\<此名>`）。
pub const START_MENU_FOLDER: &str = "Microsoft\\Windows\\Start Menu\\Programs\\AAXSysPulse";

/// 配置文件格式版本。引导写盘的 `version =` 必须等于主程序当前值，
/// 否则主程序启动时的版本跨度校验会触发迁移甚至破坏性弹窗。
pub const CONFIG_VERSION: u32 = 3;

/// 迁移一次性标记文件名：由 `CONFIG_VERSION` 推导，升版不会忘改名（7.1.3）。
pub fn migration_marker_name() -> String {
    format!("config_migrated_v{CONFIG_VERSION}")
}

/// 把契约名字转成 Win32 宽字符缓冲（末尾带 NUL），供 `PCWSTR(buf.as_ptr())` 用。
///
/// 为什么需要它：`windows::core::w!()` 只接受**编译期字面量**，无法由常量生成，
/// 于是每个 `PCWSTR` 调用点都会留下一份字面量副本（漂移源头，见 7.1.2）。
/// 用本函数把转换挪到运行期，名字就只剩契约里的一份。
/// 调用方必须让返回的 `Vec` 活到该 `PCWSTR` 被系统读完为止。
pub fn wide_of(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 号段内不得撞号：1–9 连续分配，41 是诊断专用，任何两个都必须不等。
    #[test]
    fn message_ids_are_unique() {
        let all = [
            WM_TRAY,
            WM_RECONFIG,
            WM_NEW_SNAPSHOT,
            WM_GET_SAMPLER_DIAG,
            WM_SAMPLER_DIAG,
            WM_OPEN_SETTINGS,
            WM_DISABLE_D2D,
            WM_RELOAD_CONFIG,
            WM_QUICK_RESTART,
            WM_AUTOSTART_DONE,
            WM_DIAG_CLICK,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b, "消息号撞车：{a:#x}");
            }
        }
    }

    // 号段约束的测试已删除：两条 `assert!` 比较的都是常量，运行时不可能失败
    // ⇒ 现在由本文件顶部的 `const _: () = assert!(…)` 在编译期把关。

    /// 引导只投递这两条：数值必须在号段前段（8/9），改号会让两端契约单测失败。
    #[test]
    fn cross_process_notify_ids_are_stable() {
        assert_eq!(WM_RELOAD_CONFIG, 0x8008);
        assert_eq!(WM_QUICK_RESTART, 0x8009);
        // 自启结果回报：进程内消息（后台线程 → UI 线程），不跨进程，但同样钉死。
        assert_eq!(WM_AUTOSTART_DONE, 0x800A);
    }

    /// 主程序与引导的自启值名必须不同，否则互相覆盖启动命令。
    #[test]
    fn autostart_names_do_not_collide() {
        assert_ne!(RUN_VALUE_MAIN, RUN_VALUE_GUIDE);
        assert_eq!(RUN_VALUE_MAIN, TASK_NAME, "主程序的任务名与 Run 值名同源，勿分家");
    }

    /// 契约里的名字全部是 ASCII —— 这是 `w!("字面量")` 与 `&str` 两种副本
    /// 能够逐字节等价的前提（宽字符转换因此无损）。
    #[test]
    fn names_are_ascii_for_lossless_wide_literals() {
        for s in [
            APP_TECH_NAME,
            APP_USER_NAME,
            OVERLAY_WINDOW_CLASS,
            SETTINGS_WINDOW_CLASS,
            OVERLAY_WINDOW_TITLE,
            MAIN_EXE_NAME,
            GUIDE_EXE_NAME,
            DRIVER_FILE_NAME,
            DRIVER_SERVICE_NAME,
            DRIVER_DISPLAY_NAME,
            TASK_NAME,
            RUN_VALUE_MAIN,
            RUN_VALUE_GUIDE,
        ] {
            assert!(
                s.is_ascii(),
                "{s:?} 含非 ASCII，w! 字面量副本与 &str 不再等价"
            );
        }
    }

    /// 派生关系必须成立：exe / 目录 / 驱动路径 / 卸载键都从两个基础名拼出来。
    /// 任何人改 `APP_TECH_NAME` 而不改派生项 → 这条测试红。
    #[test]
    fn derived_names_track_the_base_names() {
        assert_eq!(MAIN_EXE_NAME, format!("{APP_TECH_NAME}.exe"));
        assert_eq!(GUIDE_EXE_NAME, format!("{APP_TECH_NAME}-Guide.exe"));
        assert_eq!(INSTALL_DIR_NAME, APP_TECH_NAME);
        assert_eq!(APP_DATA_DIR_NAME, format!("{APP_TECH_NAME}C"));
        assert!(DRIVER_DIR.ends_with(&format!("\\{APP_TECH_NAME}\\")), "{DRIVER_DIR}");
        assert!(UNINSTALL_SUBKEY.ends_with(APP_TECH_NAME), "{UNINSTALL_SUBKEY}");
        assert_eq!(OVERLAY_WINDOW_TITLE, APP_USER_NAME);
        assert_eq!(RUN_VALUE_MAIN, APP_USER_NAME);
        assert_eq!(TASK_NAME, APP_USER_NAME);
        assert_eq!(TASK_NAME_LEGACY, APP_TECH_NAME);
        assert_eq!(RUN_VALUE_GUIDE, format!("{APP_TECH_NAME}Guide"));
        assert_eq!(DRIVER_SERVICE_NAME, format!("{APP_TECH_NAME}WinRing0"));
        assert_eq!(
            DRIVER_DISPLAY_NAME,
            format!("{APP_TECH_NAME} WinRing0 Driver")
        );
        assert_eq!(
            DRIVER_DIR,
            format!("C:\\Windows\\{APP_TECH_NAME}\\")
        );
        assert_eq!(SHORTCUT_FILE_NAME, format!("{APP_TECH_NAME}.lnk"));
        assert!(START_MENU_FOLDER.ends_with(&format!("\\{APP_TECH_NAME}")));
    }

    /// 标记文件名跟着 CONFIG_VERSION 走（7.1.3 的"下次升版忘改名"）。
    #[test]
    fn migration_marker_tracks_version() {
        assert_eq!(
            migration_marker_name(),
            format!("config_migrated_v{CONFIG_VERSION}")
        );
    }

    /// `wide_of` 产出的宽字符必须与原字面量逐码元一致且以 NUL 收尾 ——
    /// 运行期转换能替代 `w!()` 的前提。
    #[test]
    fn wide_of_is_nul_terminated_utf16() {
        let buf = wide_of(OVERLAY_WINDOW_CLASS);
        assert_eq!(buf.last(), Some(&0));
        let body = &buf[..buf.len() - 1];
        assert_eq!(String::from_utf16(body).unwrap(), OVERLAY_WINDOW_CLASS);
        assert_eq!(body.len(), OVERLAY_WINDOW_CLASS.len(), "ASCII ⇒ 码元数=字节数");
    }
}
