// 批次 6·①分类基线：与 core/ui 同一口径 —— 测试 fixture 用「default() 后逐字段
// 赋值」的既定写法，只在测试编译时放过该 lint（生产代码的命中已改写：
// `config.rs::MonitorConfig::load`）。
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]
// 批次 6·②：非测试代码禁 `unwrap()`（实测生产代码 0 处命中；`build.rs` 里的
// `expect`/`panic` 是构建期断言，正确行为，不在本门禁口径内）。
#![cfg_attr(not(test), warn(clippy::unwrap_used))]

mod config;
mod driver;
mod guide_state;
mod install;
mod notify;
mod process;
mod resources;
mod shortcut;

use guide_state::GuideState;
use install::get_install_dir;
use notify::notify_main_reload_config;
use process::{acquire_guide_single_instance, is_main_process_running, start_main_process};
use shortcut::create_desktop_shortcut;

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
// `tauri::Emitter` 不再在本文件直接 use：四处 `app.emit` 已收敛到
// `install::emit_traced`（失败会留痕进 install.log，审查项 7.2.3）。

/// 主程序版本号：由 build.rs 同源编译时注入（草案 2.3），引导程序与主程序版本保持一致
pub const MAIN_PROGRAM_VERSION: &str = env!("MAIN_PROGRAM_VERSION");

// ============ 错误类型 ============

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuideError {
    pub code: String,
    pub message: String,
    /// 出错时所处的安装/卸载步骤名（有步骤上下文的命令填写）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// 给用户的下一步建议；缺省时前端只显示 code + message
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl GuideError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.to_string(),
            message: message.to_string(),
            step: None,
            hint: None,
        }
    }
    pub fn with_step(mut self, step: &str) -> Self {
        self.step = Some(step.to_string());
        self
    }
    pub fn with_hint(mut self, hint: &str) -> Self {
        self.hint = Some(hint.to_string());
        self
    }
}

impl From<guide_state::StateError> for GuideError {
    fn from(e: guide_state::StateError) -> Self {
        GuideError::new("CONFIG_ERROR", &e.to_string())
    }
}

type CmdResult<T = ()> = Result<T, GuideError>;

// ============ 重入保护（P0-11） ============
//
// 安装/卸载都是"改写同一批文件 + 注册同一批服务"的长流程，并发跑一次就会
// 留下半成品安装（`.tmp` 互相覆盖、服务被自己删掉后重注册失败）。触发点不少：
// 双击按钮、`install_run` 超时后用户重试、Page03 挂载自动开跑与手动重试并发。
// 进程内用原子标志互斥，`Drop` 里释放 —— 出错、panic、被 tokio 取消都不会漏锁。

static INSTALL_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static UNINSTALL_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct BusyGuard(&'static std::sync::atomic::AtomicBool);

impl BusyGuard {
    /// 抢占标志；已被占用时返回 `None`，调用方据此给出"正在进行中"的错误。
    fn try_lock(flag: &'static std::sync::atomic::AtomicBool) -> Option<Self> {
        if flag.swap(true, std::sync::atomic::Ordering::SeqCst) {
            None
        } else {
            Some(Self(flag))
        }
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

// ============ 引导 bootstrap ============

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapResume {
    #[serde(rename = "last_page")]
    pub last_page: u32,
    pub path: String,
    /// 完整前端快照（无损恢复设置）。旧版状态文件无此字段时为 `null`，
    /// 前端据此回退到「从 INI 反推」的有损路径。
    #[serde(default)]
    pub snapshot: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapScreen {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapResult {
    #[serde(rename = "has_privilege")]
    pub has_privilege: bool,
    pub resume: Option<BootstrapResume>,
    #[serde(rename = "config_corrupted")]
    pub config_corrupted: bool,
    #[serde(rename = "install_path")]
    pub install_path: String,
    pub screen: BootstrapScreen,
    /// 第九部分：安装目录已存在主程序 exe → 前端弹「检测到已安装」四选一对话框
    #[serde(rename = "installed_detected")]
    pub installed_detected: bool,
    /// 第九部分：以 --uninstall 参数启动 → 前端直接进入卸载确认
    #[serde(rename = "uninstall_mode")]
    pub uninstall_mode: bool,
}

#[tauri::command]
fn guide_bootstrap() -> CmdResult<BootstrapResult> {
    // 补齐 5 个配置文件（缺失的用默认值补成 v3；已存在的一律不动）。
    // 必须早于主程序启动：主程序 `check_version_consistency` 对**任一缺失**的
    // .cfg 判为 v1，与其余 v3 文件组成 span=2 → 弹「版本跨度过大」破坏性弹窗。
    match config::ensure_all_files() {
        Ok(created) if !created.is_empty() => {
            eprintln!("[guide_bootstrap] 已补齐缺失配置: {:?}", created);
        }
        Err(e) => eprintln!("[guide_bootstrap] 补齐配置失败（继续）: {}", e),
        _ => {}
    }

    // 传递 --elevated 参数时（提权重启后），强制 has_privilege=true 跳过权限检测页
    let has_privilege = driver::is_admin()
        || std::env::args().any(|a| a == "--elevated");

    // 检查是否有未完成的引导
    let state = GuideState::load().unwrap_or_default();
    let resume = if state.incomplete {
        Some(BootstrapResume {
            last_page: state.last_page,
            path: state.path,
            snapshot: state.snapshot.clone(),
        })
    } else {
        None
    };

    // 检查配置是否损坏（读取 INI 配置试试）
    let config_corrupted = config::SystemConfig::load().is_err();

    // 安装路径（取不到即空串：前端相应文案整段隐藏，不再伪造 Program Files 路径）
    let install_path = get_install_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // 主屏分辨率
    let screen = get_primary_screen_size();

    // 第九部分：已安装检测 + --uninstall 启动标记
    let installed_detected = get_install_dir()
        .map(|p| p.join(aaxsyspulse_core::contract::MAIN_EXE_NAME).exists())
        .unwrap_or(false);
    let uninstall_mode = std::env::args().any(|a| a == "--uninstall");

    Ok(BootstrapResult {
        has_privilege,
        resume,
        config_corrupted,
        install_path,
        screen,
        installed_detected,
        uninstall_mode,
    })
}

fn get_primary_screen_size() -> BootstrapScreen {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{GetDesktopWindow, GetWindowRect};
        use windows::Win32::Foundation::RECT;

        unsafe {
            let desktop = GetDesktopWindow();
            let mut rect = RECT::default();
            if GetWindowRect(desktop, &mut rect).is_ok() {
                return BootstrapScreen {
                    width: (rect.right - rect.left) as u32,
                    height: (rect.bottom - rect.top) as u32,
                };
            }
        }
    }
    BootstrapScreen { width: 1920, height: 1080 }
}

// ============ 权限 ============

#[tauri::command]
fn privilege_check() -> CmdResult<serde_json::Value> {
    Ok(serde_json::json!({ "elevated": driver::is_admin() }))
}

#[tauri::command]
fn privilege_restart_as_admin() -> CmdResult {
    driver::restart_as_admin().map_err(|e| GuideError::new("PRIVILEGE_ERROR", &e.to_string()))
}

// ============ 安装 ============

#[tauri::command]
async fn install_run(app: tauri::AppHandle, mode: Option<String>) -> CmdResult<serde_json::Value> {
    let _busy = BusyGuard::try_lock(&INSTALL_BUSY).ok_or_else(|| {
        GuideError::new("INSTALL_BUSY", "安装已经在进行中")
            .with_hint("请等待当前安装结束，或取消后再重试。")
    })?;
    let mode = install::InstallMode::from_arg(mode.as_deref().unwrap_or("full"));
    let a = app.clone();
    let outcome = tokio::task::spawn_blocking(move || install::run_install(&a, mode))
        .await
        .map_err(|e| GuideError::new("INSTALL_PANIC", &e.to_string()))?;
    match outcome {
        Ok(o) => {
            // 修复/驱动重试这两条路径不会走到「完成引导」，因此这里补发一次快速
            // 重启（审查项 2.4.4 / P1-14）：主程序里被 teardown 卸载的后端
            // （GPU 厂商库、已 teardown 的 MSR）本会话不热恢复，只有新进程才能
            // 用上新装好的驱动。主程序未运行时该调用静默返回。
            // Full 模式不发：首次安装后必过引导流程，由 guide_finish 统一重启。
            if matches!(mode, install::InstallMode::Repair | install::InstallMode::DriverOnly) {
                notify::notify_main_quick_restart();
                install::log_install(
                    "通知主程序重启",
                    "成功",
                    Some("修复/驱动重试后请求快速重启，以加载新驱动"),
                );
            }
            install::emit_traced(
                &app,
                "install-done",
                serde_json::json!({ "driver_ok": o.driver_ok }),
            );
            Ok(serde_json::json!({ "driver_ok": o.driver_ok }))
        }
        Err(e) => {
            let msg = e.to_string();
            install::emit_traced(
                &app,
                "install-error",
                serde_json::json!({ "code": "INSTALL_ABORT", "message": msg }),
            );
            Err(GuideError::new("INSTALL_ABORT", &msg))
        }
    }
}

/// 第九部分：卸载（尽力执行全部步骤，逐步回报真实结果）
#[tauri::command]
async fn uninstall_run(keep_config: bool) -> CmdResult<serde_json::Value> {
    let _busy = BusyGuard::try_lock(&UNINSTALL_BUSY).ok_or_else(|| {
        GuideError::new("UNINSTALL_BUSY", "卸载已经在进行中")
            .with_hint("请等待当前卸载结束。")
    })?;
    let steps = tokio::task::spawn_blocking(move || install::run_uninstall(keep_config))
        .await
        .map_err(|e| GuideError::new("UNINSTALL_PANIC", &e.to_string()))?;
    let all_done = steps.iter().all(|s| s.status != "fail");
    Ok(serde_json::json!({ "done": all_done, "steps": steps }))
}

// ============ 主程序 / 配置 ============

#[tauri::command]
fn main_launch(app: tauri::AppHandle) -> CmdResult {
    // 幂等：主程序已在跑就不再拉第二个实例（悬浮窗会叠两份、托盘多一个图标）。
    if is_main_process_running() {
        return Ok(());
    }
    start_main_process(Some(&app)).map_err(|e| GuideError::new("MAIN_LAUNCH_FAILED", &e.to_string()))
}

#[tauri::command]
fn config_patch(patch: serde_json::Value) -> CmdResult {
    // 写 system.cfg（style/gdi_mode/orientation/opacity/position）
    {
        let mut sys = config::SystemConfig::load().unwrap_or_default();
        sys.patch_from_json(&patch);
        sys.save().map_err(|e| GuideError::new("CONFIG_WRITE_FAILED", &e.to_string()))?;
    }
    // 写 appearance.cfg（colors/font_name）
    {
        let mut app = config::AppearanceConfig::load().unwrap_or_default();
        app.patch_from_json(&patch);
        app.save().map_err(|e| GuideError::new("CONFIG_WRITE_FAILED", &e.to_string()))?;
    }
    // 写 monitor.cfg（items/per_item_colors/per_item_fonts；order 由 layout 承载）
    {
        let mut mon = config::MonitorConfig::load().unwrap_or_default();
        mon.patch_from_json(&patch);
        mon.save().map_err(|e| GuideError::new("CONFIG_WRITE_FAILED", &e.to_string()))?;
    }
    // 写布局文件（order → layout_h.cfg / layout_v.cfg，按当前激活布局决定文件名）
    {
        let sys_for_layout = config::SystemConfig::load().unwrap_or_default();
        let vertical = sys_for_layout.layout == "vertical";
        let mut lay = config::LayoutConfig::load(vertical).unwrap_or_default();
        lay.patch_from_json(&patch);
        lay.save(vertical).map_err(|e| GuideError::new("CONFIG_WRITE_FAILED", &e.to_string()))?;
    }
    Ok(())
}

#[tauri::command]
fn main_reload_config() -> CmdResult {
    notify_main_reload_config();
    Ok(())
}

// ============ 悬浮窗位置 ============

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    #[serde(rename = "screen_w")]
    pub screen_w: u32,
    #[serde(rename = "screen_h")]
    pub screen_h: u32,
}

#[tauri::command]
fn overlay_get_rect() -> CmdResult<OverlayRect> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowRect};
        use windows::Win32::Foundation::RECT;
        use windows::core::PCWSTR;

        let class_w = aaxsyspulse_core::contract::wide_of(
            aaxsyspulse_core::contract::OVERLAY_WINDOW_CLASS,
        );

        let hwnd = unsafe { FindWindowW(PCWSTR(class_w.as_ptr()), PCWSTR::null()) };
        if let Ok(hwnd) = hwnd {
            let mut rect = RECT::default();
            if unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok() {
                let screen = get_primary_screen_size();
                return Ok(OverlayRect {
                    x: rect.left,
                    y: rect.top,
                    width: rect.right - rect.left,
                    height: rect.bottom - rect.top,
                    screen_w: screen.width,
                    screen_h: screen.height,
                });
            }
        }
    }

    // 主程序没在运行：返回配置文件里的持久化位置（那正是它下次启动会用的位置），
    // 而不是写死的演示坐标 —— 后者会让第 12 页的「应用」把悬浮窗瞬移到假位置。
    let sys = config::SystemConfig::load().unwrap_or_default();
    let screen = get_primary_screen_size();
    let (x, y) = if sys.x >= 0 && sys.y >= 0 {
        (sys.x, sys.y)
    } else {
        // 与主程序 create 时的默认位一致（首次运行未落盘位置）
        (screen.width as i32 / 4, 24)
    };
    let (width, height) = if sys.w > 0 && sys.h > 0 {
        (sys.w, sys.h)
    } else {
        // 尺寸由渲染器按内容决定，未落盘时无从得知；仅用于界面显示，不参与写回
        (168, 76)
    };
    Ok(OverlayRect {
        x,
        y,
        width,
        height,
        screen_w: screen.width,
        screen_h: screen.height,
    })
}

#[tauri::command]
fn overlay_set_position(x: i32, y: i32) -> CmdResult {
    // 写 system.cfg
    let mut sys = config::SystemConfig::load().unwrap_or_default();
    sys.x = x;
    sys.y = y;
    sys.save().map_err(|e| GuideError::new("CONFIG_WRITE_FAILED", &e.to_string()))?;

    // 通知主程序
    notify_main_reload_config();
    Ok(())
}

// ============ 开机自启 ============

#[tauri::command]
fn autostart_set(enabled: bool) -> CmdResult {
    // 先做 OS 侧、再落盘（审查项 7.2.4 / P1-21）：旧顺序是先 `sys.save()` 再
    // `let _ = RegSetValueExW(...)` —— 注册表写失败时配置里已经是 `autostart=true`，
    // 命令返回 Ok，前端显示"已开启"，而下次开机什么都没有启动。
    #[cfg(windows)]
    set_autostart_run_key(enabled)
        .map_err(|e| GuideError::new("AUTOSTART_WRITE_FAILED", &e))?;

    let mut sys = config::SystemConfig::load().unwrap_or_default();
    sys.autostart = enabled;
    sys.save().map_err(|e| GuideError::new("AUTOSTART_WRITE_FAILED", &e.to_string()))?;

    Ok(())
}

/// 写/删 HKCU 的 Run 值。返回 `Err(描述)` 而不是静默吞掉返回值。
#[cfg(windows)]
fn set_autostart_run_key(enabled: bool) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::System::Registry::{
        RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, RegCloseKey, HKEY_CURRENT_USER, KEY_WRITE,
        REG_SZ,
    };

    let subkey = aaxsyspulse_core::contract::RUN_KEY;
    // 值名走契约常量（审查项 3.1.14 / P1-22）：主程序自启写 RUN_VALUE_MAIN，
    // 引导写 RUN_VALUE_GUIDE，两者**必须不同**，否则互相覆盖启动命令。
    let value_name_w =
        aaxsyspulse_core::contract::wide_of(aaxsyspulse_core::contract::RUN_VALUE_GUIDE);
    let value_name = windows::core::PCWSTR::from_raw(value_name_w.as_ptr());
    let subkey_wide: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let mut hkey = std::mem::zeroed();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR::from_raw(subkey_wide.as_ptr()),
            Some(0),
            KEY_WRITE,
            &mut hkey,
        )
        .is_err()
        {
            return Err(format!("无法打开注册表项 {subkey}"));
        }
        let result = if enabled {
            // 获取引导程序 exe 路径，并附加 --autostart 参数
            let exe_path = std::env::current_exe().map_err(|e| format!("取不到自身路径：{e}"))?;
            let exe_str = format!("\"{}\" --autostart", exe_path.to_string_lossy());
            let exe_wide: Vec<u16> =
                OsStr::new(&exe_str).encode_wide().chain(std::iter::once(0)).collect();
            // windows 0.61 的这两个注册表函数返回 `WIN32_ERROR` 而非 `Result`，
            // 必须显式判码 —— 旧代码的 `let _ =` 连错误码都不看。
            let err = RegSetValueExW(
                hkey,
                value_name,
                Some(0),
                REG_SZ,
                Some(&exe_wide.iter().flat_map(|&c| c.to_le_bytes()).collect::<Vec<u8>>()),
            );
            if err.is_ok() {
                Ok(())
            } else {
                Err(format!("写入 Run 值失败：{err:?}"))
            }
        } else {
            // 「值本来就不存在」等同已关闭，不算失败。
            let err = RegDeleteValueW(hkey, value_name);
            if err.is_ok() || err == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(format!("删除 Run 值失败：{err:?}"))
            }
        };
        let _ = RegCloseKey(hkey);
        result
    }
}

// ============ 第 14 页数据 ============

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallInfo {
    #[serde(rename = "install_path")]
    pub install_path: String,
    /// 主程序生命周期 CPU 平均占用；`None` = 主程序未运行或取不到，前端显示"未取到"
    #[serde(rename = "cpu_avg_percent")]
    pub cpu_avg_percent: Option<f64>,
    #[serde(rename = "mem_bytes")]
    pub mem_bytes: Option<u64>,
}

#[tauri::command]
fn app_get_install_info() -> CmdResult<InstallInfo> {
    let install_path = get_install_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| GuideError::new("NO_INSTALL_DIR", &e.to_string()))?;

    let (cpu_avg_percent, mem_bytes) = match process::main_process_usage() {
        Some((cpu, mem)) => (Some(cpu), Some(mem)),
        None => (None, None),
    };

    Ok(InstallInfo {
        install_path,
        cpu_avg_percent,
        mem_bytes,
    })
}

// ============ 恢复引导时读取配置 ============

/// 从 INI 文件加载配置并转换为前端 JSON 格式
fn load_config_from_ini() -> serde_json::Value {
    let sys = config::SystemConfig::load().unwrap_or_default();
    let app = config::AppearanceConfig::load().unwrap_or_default();
    let mon = config::MonitorConfig::load().unwrap_or_default();
    // 显示顺序由 layout.cfg 承载（主程序把 order 存在此处）
    let vertical = sys.layout == "vertical";
    let lay = config::LayoutConfig::load(vertical).unwrap_or_default();

    // ⚠️ 引导程序**不再**把「样式预设 A–F」写进主程序配置（见 config.rs 的架构决策说明），
    // 因此这里无法从配置反推用户上次选了哪个预设。恢复引导时无妨：
    //   · 上次到哪一页 / 走 preset 还是 custom，由引导自己的 guide_state 决定（boot.resume）；
    //   · style 只用于第 4 页样式卡片的默认高亮，给中性占位 "A" 即可。
    let style = "A";
    // 主程序口径（types.rs::RenderEngine）：render_engine `"0"` = Gdi（省显卡，默认）、
    // `"1"` = D2d（DirectWrite 高质量）。故前端的 gdi_mode = (render_engine == "0")。
    // （早期写成 `== "1"`，把 GDI 与 D2D 判反了。）
    let gdi_mode = sys.render_engine == "0";

    // items: id → key（与主程序 types::Metric 顺序一致）
    let mut items = std::collections::HashMap::new();
    for item in &mon.items {
        if let Some(key) = config::metric_key_for_id(item.id) {
            items.insert(key.to_string(), item.show);
        }
    }

    // 显示顺序来自 layout.cfg（按当前激活布局读取）
    let mut order = Vec::new();
    for &id in &lay.order {
        if let Some(key) = config::metric_key_for_id(id) {
            order.push(key.to_string());
        }
    }

    // 全局数值色：主程序没有「全局数值色」字段，数值色落在逐项 `[item.N] color_v`。
    // 取 9 项 color_v 的**公共值**；为空或不一致（例如配置只由主程序生成过）
    // 则回落到前端默认绿。
    let value_color = if mon.items.len() == 9
        && mon.items.iter().all(|i| !i.color_v.is_empty())
        && mon.items.iter().all(|i| i.color_v == mon.items[0].color_v)
    {
        mon.items[0].color_v.clone()
    } else {
        "#34A853".to_string()
    };

    // 名称显示方式：从 monitor.cfg 反推（有损恢复用；无损恢复走前端 guide_state 快照）。
    // 逐项优先 `[item.N] short`（1=full 2=simple），无显值时回落 name + 全局 short_names。
    // 全局档位：全部隐藏 → none；任一逐项 short≠0 → mixed（per-item 承载，全局取
    // short_names 语义供前端统一按钮高亮）；否则按 short_names 给 simple/full。
    let all_name_hidden = mon.items.iter().all(|i| !i.name);
    let name_mode = if all_name_hidden { "none" } else if mon.short_names { "simple" } else { "full" };
    let has_explicit_mode = mon.items.iter().any(|i| i.short != 0);
    let mut per_item_name_mode_map = serde_json::Map::new();
    for item in &mon.items {
        if let Some(key) = config::metric_key_for_id(item.id) {
            let m = if !item.name {
                "none"
            } else {
                match item.short {
                    1 => "full",
                    2 => "simple",
                    _ if mon.short_names => "simple",
                    _ => "full",
                }
            };
            per_item_name_mode_map.insert(key.to_string(), serde_json::Value::String(m.to_string()));
        }
    }

    serde_json::json!({
        "version": 1,
        "style": style,
        "gdi_mode": gdi_mode,
        "orientation": sys.layout,
        "items": items,
        "order": order,
        "name_mode": name_mode,
        // 无逐项显式覆盖时回 null：前端把非 null 视为"逐项模式已启用"。
        "per_item_name_mode": if has_explicit_mode { serde_json::Value::Object(per_item_name_mode_map) } else { serde_json::Value::Null },
        "colors": {
            "name": app.fg,
            "value": value_color,
        },
        "per_item_colors": serde_json::Value::Null,
        "opacity": sys.normal_opacity,
        "font_name": app.font_size as u32,
        "font_value": app.font_size as u32,
        "font_weight": app.font_weight,
        "per_item_fonts": serde_json::Value::Null,
        "position": { "x": sys.x, "y": sys.y }
    })
}

#[tauri::command]
fn config_load() -> CmdResult<serde_json::Value> {
    Ok(load_config_from_ini())
}

// ============ 关闭 / 退出 ============

#[tauri::command]
fn guide_save_config(config: serde_json::Value) -> CmdResult {
    // 通过 config_patch 逻辑写 INI 文件
    {
        let mut sys = config::SystemConfig::load().unwrap_or_default();
        sys.patch_from_json(&config);
        sys.save().map_err(|e| GuideError::new("CONFIG_WRITE_FAILED", &e.to_string()))?;
    }
    {
        let mut app = config::AppearanceConfig::load().unwrap_or_default();
        app.patch_from_json(&config);
        app.save().map_err(|e| GuideError::new("CONFIG_WRITE_FAILED", &e.to_string()))?;
    }
    {
        let mut mon = config::MonitorConfig::load().unwrap_or_default();
        mon.patch_from_json(&config);
        mon.save().map_err(|e| GuideError::new("CONFIG_WRITE_FAILED", &e.to_string()))?;
    }
    // 写布局文件（order → layout_h.cfg / layout_v.cfg，按当前激活布局决定文件名）
    {
        let sys_for_layout = config::SystemConfig::load().unwrap_or_default();
        let vertical = sys_for_layout.layout == "vertical";
        let mut lay = config::LayoutConfig::load(vertical).unwrap_or_default();
        lay.patch_from_json(&config);
        lay.save(vertical).map_err(|e| GuideError::new("CONFIG_WRITE_FAILED", &e.to_string()))?;
    }

    // 注意：**不清除** incomplete 标记 —— 用户要求「中途退出时保留上一页与之前的设定」，
    // 因此「保存」后退出仍能续引导。只有「完成引导」(guide_finish) 与
    // 「重新引导 / 不保存」(guide_reset_state) 才清空断点状态。
    Ok(())
}

#[tauri::command]
fn guide_backup(page: u32, path: String, snapshot: serde_json::Value) -> CmdResult {
    // 备份「当前页 + 完整设置快照」到 guide_state.json。
    // 中途退出后据此**无损**续引导（含 name_mode / 逐项颜色字号等 INI 无法反推的字段）。
    // 「重新引导 / 不保存」会删除该文件；「完成引导」把 incomplete 置回 false。
    let mut state = GuideState::load().unwrap_or_default();
    state.incomplete = true;
    state.last_page = page;
    state.path = path;
    if let Some(s) = snapshot.get("style").and_then(|v| v.as_str()) {
        state.style = s.to_string();
    }
    state.snapshot = snapshot;
    state.timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    state.save().map_err(|e| GuideError::new("CONFIG_WRITE_FAILED", &e.to_string()))
}

#[tauri::command]
fn guide_reset_state() -> CmdResult {
    // 清空断点续引导记忆（guide_state.json）；文件缺失即视为已清空。
    GuideState::reset().map_err(|e| GuideError::new("STATE_RESET_FAILED", &e.to_string()))
}

#[tauri::command]
fn guide_finish(create_shortcut: bool) -> CmdResult {
    // 1. config.json 已由前端 saveConfig 保存
    // 2. 清除 incomplete 标记（审查项 7.2.4 / P1-21：旧写法 `let _ = state.save()`
    //    把失败吞掉 ⇒ 引导已"完成"，下次启动却仍弹"继续上次引导"。这里不阻断完成
    //    流程（配置本身已写盘成功），但必须留痕。）
    if let Ok(mut state) = GuideState::load() {
        state.incomplete = false;
        if let Err(e) = state.save() {
            install::log_install("清除引导断点标记", "失败", Some(&e.to_string()));
        }
    }
    // 3. 通知主程序刷新
    notify_main_reload_config();
    // 4. 创建快捷方式（可选）——失败必须可见：这是用户勾选项，静默失败 = 谎报成功
    if create_shortcut {
        match create_desktop_shortcut() {
            Ok(path) => install::log_install(
                "创建桌面快捷方式",
                "成功",
                Some(&path.to_string_lossy()),
            ),
            Err(e) => install::log_install("创建桌面快捷方式", "失败", Some(&e.to_string())),
        }
    }
    // 5. 通知主程序快速重启：重启后从盘重读全部配置（含新逐项命名模式），
    //    新进程天然完成采样器/驱动的重新初始化。消息按队列顺序排在 reload 之后。
    notify::notify_main_quick_restart();
    Ok(())
}

/// 删除引导程序自身（第 14 页「删除引导文件」）。
///
/// 旧实现：往 `%TEMP%\delete_guide.bat`（固定可预测名）写 `del "<exe>"` 再
/// `cmd /c start /b` 跑 —— 正在运行的 exe 被系统锁定，`del` 必然失败，
/// 而 `spawn()` 的结果被 `.ok()` 丢弃 ⇒ 用户点了"删除"什么都没发生（报告 3.1.12）。
///
/// 现实现：先删引导自己的断点存档，再把 exe **改名为 `.old`**（运行中的映像允许
/// 重命名，改名即刻让旧路径失效，卸载入口不会再指向一个"看着还在"的文件），
/// 最后对改名后的文件登记 `MOVEFILE_DELAY_UNTIL_REBOOT` 由系统在下次开机删除。
/// 每一步的失败都如实返回 Err，前端弹提示。
#[tauri::command]
fn guide_delete_self() -> CmdResult<String> {
    // 引导自身的进度存档（主程序的 5 个 .cfg 不在这里，由卸载流程按 keepConfig 处置）
    GuideState::reset().map_err(|e| GuideError::new("DELETE_STATE_FAILED", &e.to_string()))?;

    let exe_path = std::env::current_exe()
        .map_err(|_| GuideError::new("IO_ERROR", "无法获取引导程序自身路径"))?;

    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};

        let wide = |p: &std::path::Path| -> Vec<u16> {
            OsStr::new(p).encode_wide().chain(std::iter::once(0)).collect()
        };
        let old_path = {
            let mut s = exe_path.as_os_str().to_os_string();
            s.push(".old");
            std::path::PathBuf::from(s)
        };
        // 上一次自删除遗留的 .old 此刻已不被占用，顺手删掉
        let _ = std::fs::remove_file(&old_path);

        // ① 立即改名（同 MoveFileExW 语义，但同步生效）
        let renamed = std::fs::rename(&exe_path, &old_path).is_ok();
        // ② 登记重启后删除：改名成功则删 .old，否则退回删原路径
        let target = if renamed { old_path.as_path() } else { exe_path.as_path() };
        let target_wide = wide(target);
        match unsafe { MoveFileExW(PCWSTR(target_wide.as_ptr()), PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT) } {
            Ok(()) => Ok(if renamed {
                "引导程序已停用，将在下次开机时彻底删除。".to_string()
            } else {
                "引导程序当前无法改名（文件被占用），已安排在下次开机时删除。".to_string()
            }),
            Err(e) => Err(GuideError::new("DELETE_SELF_FAILED", &e.to_string()).with_hint(
                "系统拒绝了删除安排（可能需要管理员权限）。请稍后手动删除该文件。",
            )),
        }
    }

    #[cfg(not(windows))]
    Ok("非 Windows 平台：已清除引导存档。".to_string())
}

#[tauri::command]
fn app_quit_all(app: tauri::AppHandle) -> CmdResult {
    // 旧实现是 `std::process::exit(0)`：命令在 Tauri 线程里当场终止进程 ⇒
    // 前端的 Promise 永不 settle，`quitted.value = true` 与其后的整条分支不可达
    // （报告 9.9）。现在先把 Ok 回给前端，再让 Tauri 走正常收尾流程。
    std::thread::spawn(move || {
        // 留一点时间让 invoke 的响应送达渲染进程
        std::thread::sleep(std::time::Duration::from_millis(120));
        app.exit(0);
    });
    Ok(())
}

// ============ 日志查看（XOR 混淆日志的还原） ============
// ⚠️ 用词纠正（审查项 9.13）：这一族**不是加密**。密钥就硬编码在本文件里，
// 任何拿到 exe 的人都能解 —— 它的真实作用是"别让日志被顺手读懂"（混淆）。
// 写成"已加密"会让读代码的人以为日志里可以放敏感内容，事实相反：
// 日志仍需按 7.3.3 的口径做用户名/路径脱敏。

/// XOR 密钥（轻量化混淆，非真正加密）
const LOG_XOR_KEY: &[u8] = b"AAX_SysPulse_2024_Guide_Log_Key_v1";

/// 还原 XOR 混淆后的日志文件内容（与 `GuideLogWriter` 的密钥流严格对齐）
fn decrypt_log(data: &[u8]) -> Vec<u8> {
    data.iter()
        .enumerate()
        .map(|(i, &b)| b ^ LOG_XOR_KEY[i % LOG_XOR_KEY.len()])
        .collect()
}

/// 引导程序日志目录：%LOCALAPPDATA%\AAXSysPulse\logs\
/// 写入方（run 的 tracing）与读取方（guide_view_logs）必须共用这一个来源，
/// 之前两处各写一份路径常量，读取方多写了一个 `C`，永远读不到自己写的日志。
fn guide_log_dir() -> std::path::PathBuf {
    dirs::data_local_dir()
        .unwrap_or_default()
        .join(aaxsyspulse_core::contract::INSTALL_DIR_NAME)
        .join("logs")
}

#[tauri::command]
fn guide_view_logs(lines: Option<usize>) -> CmdResult<serde_json::Value> {
    let log_dir = guide_log_dir();

    let count = lines.unwrap_or(200).min(2000);
    let mut all_content = String::new();

    // 读取所有日志文件（按修改时间倒序）
    let mut files: Vec<_> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&log_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            if entry.path().extension().is_some_and(|ext| ext == "log") {
                files.push(entry);
            }
        }
    }

    files.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));

    for entry in files.into_iter().take(3) {
        let Ok(data) = std::fs::read(entry.path()) else { continue };
        // 只有 guide_*.log 经过 XOR 混淆；install.log 是明文，异或后会变成乱码。
        let bytes = if entry
            .path()
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("guide_"))
        {
            decrypt_log(&data)
        } else {
            data
        };
        if let Ok(s) = String::from_utf8(bytes) {
            all_content.push_str(&s);
        }
    }

    // 取最后 count 行
    let all_lines: Vec<_> = all_content.lines().rev().take(count).collect();
    let result: String = all_lines.into_iter().rev().collect::<Vec<_>>().join("\n");

    Ok(serde_json::json!({
        "path": log_dir.to_string_lossy(),
        "lines": count,
        "content": result
    }))
}

// ============ 入口 ============

/// 混淆写日志的核心 writer（impl Write，返回值给 tracing_subscriber）
///
/// 密钥流按**文件绝对字节偏移**取值，与 `decrypt_log()` 的整文件异或严格对齐。
/// 旧实现每次 write 都从 key[0] 重新开始，而读取端按整文件偏移解密 ——
/// 除第一条记录外全部错位，日志查看器只能看到乱码。
///
/// 句柄与偏移放在 `Arc<Mutex<..>>` 里全进程共用：tracing 每条记录都会
/// 重新调用 MakeWriter，若各自开句柄并自行取长度，多线程写日志时
/// 「取长度」与「落盘」之间有竞态，偏移一旦错位整段日志即报废。
struct EncryptedLogFile {
    /// `None` = 日志文件打不开（日志目录不可写、磁盘满、被安全软件拦截）。
    /// 此时写操作**静默丢弃**：`run()` 早期为日志 panic 的表现是"双击图标后窗口
    /// 根本不出现"，比丢日志严重一个量级（审查项 7.2.1 / P1-20）。
    file: Option<std::fs::File>,
    key: &'static [u8],
    pos: u64,
}

#[derive(Clone)]
struct EncryptedLogWriter(Arc<Mutex<EncryptedLogFile>>);

impl std::io::Write for EncryptedLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // 中毒也要继续写：中毒只可能来自上一次写日志里的 panic，
        // 让日志通道成为二次崩溃源等于把故障放大成整机不可用。
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let key = inner.key;
        let start = inner.pos as usize;
        let encrypted: Vec<u8> = buf
            .iter()
            .enumerate()
            .map(|(i, &b)| b ^ key[(start + i) % key.len()])
            .collect();
        if let Some(file) = inner.file.as_mut() {
            file.write_all(&encrypted)?;
        }
        // 偏移照常推进：日志中途不可用不改变"密钥流按文件绝对偏移取值"的约定。
        inner.pos += buf.len() as u64;
        // 即便没落盘也如实返回 buf.len()：Write 契约要求返回消费字节数，
        // 返回 0 会被 tracing 当成写不进而反复重试并报错。
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(file) = inner.file.as_mut() {
            let _ = file.flush();
        }
        Ok(())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 日志目录：%LOCALAPPDATA%\AAXSysPulse\logs\
    let log_dir = guide_log_dir();
    let _ = std::fs::create_dir_all(&log_dir);

    // 用当天日期做日志文件名（chrono 已在依赖里；手算 365 天/年会在闰年溢出索引 panic）
    let today = chrono::Local::now().format("%Y-%m-%d");
    let log_path = log_dir.join(format!("guide_{today}.log"));

    // 日志文件全进程只开一个句柄：取偏移与落盘在同一把锁内完成。
    // 打不开**不 panic**（见 EncryptedLogFile::file 的注释），降级为静默丢弃。
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();
    let pos = log_file
        .as_ref()
        .and_then(|f| f.metadata().ok())
        .map(|m| m.len())
        .unwrap_or(0);
    let writer = EncryptedLogWriter(Arc::new(Mutex::new(EncryptedLogFile {
        file: log_file,
        key: LOG_XOR_KEY,
        pos,
    })));

    let _ = tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .try_init();

    // --autostart 模式：引导程序自启后启动主程序，然后退出
    if std::env::args().any(|a| a == "--autostart") {
        if !is_main_process_running() {
            let _ = start_main_process(None);
        }
        std::process::exit(0);
    }

    // 单实例：拿到锁才继续；已被占用则直接退出。
    // 锁句柄在 process 模块内故意不 CloseHandle，靠进程退出释放。
    if !acquire_guide_single_instance() {
        eprintln!("Another guide instance is already running");
        std::process::exit(1);
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            // 生命周期
            guide_bootstrap,
            guide_save_config,
            guide_backup,
            guide_reset_state,
            app_quit_all,
            guide_finish,
            guide_delete_self,
            // 权限
            privilege_check,
            privilege_restart_as_admin,
            // 安装
            install_run,
            uninstall_run,
            // 主程序 / 配置
            main_launch,
            config_patch,
            main_reload_config,
            config_load,
            // 悬浮窗
            overlay_get_rect,
            overlay_set_position,
            // 第 13 / 14 页
            autostart_set,
            app_get_install_info,
            // 日志
            guide_view_logs,
        ])
        .setup(|_app| {
            tracing::info!("Guide app started, main program v{}", MAIN_PROGRAM_VERSION);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
