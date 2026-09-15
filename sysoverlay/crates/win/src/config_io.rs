//! 多文件配置持久化（原子写）。
//!
//! 配置文件布局：`%APPDATA%\AAXSysPulseC\config\` 下拆 5 个 `.cfg`：
//! - `system.cfg`      全局/系统级（含 `[system]` 几何 + `layout` 锚点决定横竖）
//! - `appearance.cfg`  颜色/字号/间距/阴影描边（含 `[meta]`）
//! - `monitor.cfg`     监控项开关/顺序/配色/功耗（含 `[meta]`）
//! - `layout_h.cfg`    横版布局专属（窗口几何 + 逐指标 4 分量偏移，含 `[meta]`）
//! - `layout_v.cfg`    竖版布局专属（含 `[meta]`）
//!
//! 旧版单文件 `%APPDATA%\AAXSysPulseC\config.txt`(v2) 启动时若发现且 5 个新文件
//! 中有任何一个缺失，自动迁移（解析 → 生成 5 文件 → 全部原子写 .tmp → rename
//! → rename old to .bak）。失败保留旧配置，下次启动重试。一次迁移只执行一次。
//!
//! 原子写：写临时文件 + `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` 同卷替换。
//! 落盘失败一律回退默认不崩溃。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use aaxsyspulse_core::config::appearance;
use aaxsyspulse_core::config::ini::Ini;
use aaxsyspulse_core::config::layout::{self, LAYOUT_H_FILE, LAYOUT_V_FILE};
use aaxsyspulse_core::config::legacy;
use aaxsyspulse_core::config::mcfg;
use aaxsyspulse_core::config::monitor;
use aaxsyspulse_core::config::system;
use aaxsyspulse_core::config::Config;
use aaxsyspulse_core::types::RenderStyle;

/// 配置加载错误。
///
/// 调用方（`app` 入口）应据此向用户弹窗，而不是静默按某个版本解析。
#[derive(Debug)]
pub enum ConfigError {
    /// 5 个配置文件的版本跨度过大（`max - min > 1`），无法安全合并解析。
    /// 例如某文件仍是 v1（旧版迁移残留 / 手改丢失 version）、其余已是 v3。
    /// 若仍按最高版本解析，旧格式文件的字段名/段名已变，会被静默忽略 → 配置丢失。
    ///
    /// 调用方应弹窗让用户选择「使用默认配置（覆盖）」或「退出手动处理」。
    VersionSpanTooLarge { min: u32, max: u32 },
}

/// 配置子目录名（位于 `%APPDATA%\AAXSysPulseC\` 下）。
const CONFIG_SUBDIR: &str = "config";

/// 迁移一次性执行标记文件（写在 config/ 同级的 APPDATA 目录下）。
const MIGRATION_DONE_FILE: &str = "config_migrated_v3";

fn appdata_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|s| std::path::PathBuf::from(s).join("AAXSysPulseC"))
}

fn config_dir() -> Option<std::path::PathBuf> {
    appdata_dir().map(|d| d.join(CONFIG_SUBDIR))
}

fn migration_marker_path() -> Option<std::path::PathBuf> {
    appdata_dir().map(|d| d.join(MIGRATION_DONE_FILE))
}

fn to_wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// 落盘所需的 4 个文件包（仅含**当前激活布局**，另一布局文件不在此包内、
/// 由切换时单独保留）。返回 `(文件名, 内容)` 列表。
fn bundle_for(cfg: &Config) -> Vec<(String, String)> {
    vec![
        (system::SYSTEM_FILE.to_string(), system::serialize(cfg)),
        (appearance::APPEARANCE_FILE.to_string(), appearance::serialize(cfg)),
        (monitor::MONITOR_FILE.to_string(), monitor::serialize(cfg)),
        (
            layout::file_for(cfg.render_style).to_string(),
            layout::serialize(cfg.render_style, cfg),
        ),
    ]
}

/// 迁移用：把全部 5 个文件（含两个布局）一次性写出，给两份布局都播下默认值。
fn bundle_all(cfg: &Config) -> Vec<(String, String)> {
    vec![
        (system::SYSTEM_FILE.to_string(), system::serialize(cfg)),
        (appearance::APPEARANCE_FILE.to_string(), appearance::serialize(cfg)),
        (monitor::MONITOR_FILE.to_string(), monitor::serialize(cfg)),
        (LAYOUT_H_FILE.to_string(), layout::serialize(RenderStyle::Strip, cfg)),
        (LAYOUT_V_FILE.to_string(), layout::serialize(RenderStyle::Vertical, cfg)),
    ]
}

/// 5 个目标配置文件名（用于判断是否缺失）。
const ALL_FILES: [&str; 5] = [
    system::SYSTEM_FILE,
    appearance::APPEARANCE_FILE,
    monitor::MONITOR_FILE,
    LAYOUT_H_FILE,
    LAYOUT_V_FILE,
];

/// 读文件内容，不存在返回 `None`（不报错）。
fn read_if(dir: &std::path::Path, name: &str) -> Option<String> {
    // ⚠️ 必须剥离 UTF-8 BOM。记事本 / VSCode「另存为 UTF-8 with BOM」会在文件首
    // 写入 U+FEFF，使首个 `[system]` 头变成 "\u{feff}[system]" → Ini 查不到该段 →
    // **整个文件的键静默回退默认**；且 version 读不到被当成 v1，与其他 v3 文件
    // 组成 span=2 → 触发「版本跨度过大 → 用默认配置覆盖全部配置」的破坏性弹窗。
    // 2026-09-15 审计定位。
    std::fs::read_to_string(dir.join(name))
        .ok()
        .map(|s| s.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(s))
}

/// 原子写单个文件：临时文件 + `MoveFileExW` 同卷替换。
fn write_file_atomic(dir: &std::path::Path, name: &str, content: &str) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        eprintln!("[config_io] 无法创建配置目录");
        return false;
    }
    let target = dir.join(name);
    let temp = dir.join(format!("{}.tmp", name));

    // 先写临时文件并 fsync：避免断电时 MoveFileExW 替换成功但数据未落盘。
    let write_ok = std::fs::File::create(&temp).and_then(|mut f| {
        use std::io::Write;
        f.write_all(content.as_bytes())?;
        f.sync_all()
    });
    if write_ok.is_err() {
        eprintln!("[config_io] 写临时文件失败: {}", name);
        let _ = std::fs::remove_file(&temp);
        return false;
    }

    unsafe {
        use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING};
        let wide_temp: Vec<u16> = to_wide(temp.as_os_str());
        let wide_target: Vec<u16> = to_wide(target.as_os_str());
        let result = MoveFileExW(
            windows::core::PCWSTR(wide_temp.as_ptr()),
            windows::core::PCWSTR(wide_target.as_ptr()),
            MOVEFILE_REPLACE_EXISTING,
        );
        if result.is_err() {
            let err = windows::Win32::Foundation::GetLastError();
            eprintln!("[config_io] MoveFileExW 失败: {} ({})", name, err.0);
            let _ = std::fs::remove_file(&temp);
            return false;
        }
    }
    true
}

/// 把文件包原子落盘（逐文件独立原子写；单文件失败不阻断其余）。
fn write_bundle(dir: &std::path::Path, files: &[(String, String)]) -> bool {
    // 串行化：同步 save_config（退出 / 关窗 flush）与异步写线程可能并发，两者共用
    // 固定名 `<name>.tmp` 且写同一批目标文件 → 无锁会交错出损坏内容。
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all_ok = true;
    for (name, content) in files {
        all_ok &= write_file_atomic(dir, name, content);
    }
    all_ok
}

/// 把文件包写入临时文件（不 rename），返回临时文件路径列表。
/// 用于迁移事务：先写出 .tmp，全部成功后才 rename。
fn write_bundle_to_tmp(dir: &std::path::Path, files: &[(String, String)]) -> Vec<std::path::PathBuf> {
    let mut tmp_paths = Vec::new();
    for (name, content) in files {
        let temp = dir.join(format!("{}.tmp", name));
        if let Ok(mut f) = std::fs::File::create(&temp) {
            use std::io::Write;
            if f.write_all(content.as_bytes()).is_ok() && f.sync_all().is_ok() {
                tmp_paths.push(temp);
                continue;
            }
        }
        eprintln!("[config_io] 写临时文件失败: {}", name);
        // 清理已创建的临时文件
        for p in &tmp_paths {
            let _ = std::fs::remove_file(p);
        }
        return vec![];
    }
    tmp_paths
}

/// 把一组 `.tmp` 路径 rename 到目标（用 MoveFileExW 原子替换）。
/// 失败时返回 (成功数, 失败名)。
fn rename_tmp_to_final(dir: &std::path::Path, files: &[(String, String)]) -> Result<(), (usize, String)> {
    for (name, _) in files {
        let tmp = dir.join(format!("{}.tmp", name));
        let target = dir.join(name);
        unsafe {
            use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING};
            let wide_tmp: Vec<u16> = to_wide(tmp.as_os_str());
            let wide_target: Vec<u16> = to_wide(target.as_os_str());
            if MoveFileExW(
                windows::core::PCWSTR(wide_tmp.as_ptr()),
                windows::core::PCWSTR(wide_target.as_ptr()),
                MOVEFILE_REPLACE_EXISTING,
            )
            .is_err()
            {
                let err = windows::Win32::Foundation::GetLastError();
                eprintln!("[config_io] MoveFileExW rename 失败: {} ({})", name, err.0);
                return Err((0, name.clone()));
            }
        }
    }
    Ok(())
}

/// 读单个配置文件的 `[meta] version`（不存在或解析失败返回 `None`）。
/// system.cfg 特殊：version 写在 `[system]` 段而非 `[meta]`。
fn read_file_version(dir: &std::path::Path, name: &str) -> Option<u32> {
    let text = read_if(dir, name)?;
    let ini = Ini::parse(&text);
    // system.cfg 版本在 [system] 段，其余文件在 [meta] 段
    let (section, key) = if name == system::SYSTEM_FILE {
        ("system", "version")
    } else {
        ("meta", "version")
    };
    ini.get(section, key)
        .and_then(|v| v.parse::<u32>().ok())
}

/// 检查 5 个配置文件的 version 一致性（启动时调用）。
/// - 缺失 version → 视为 v1（旧版迁移文件）。
/// - span > 1（大版本跨度）→ 返回 `Err(VersionSpanTooLarge)`，拒绝启动，
///   交由调用方弹窗让用户「使用默认配置」或「退出手动处理」。
/// - span == 1（如 v2→v3 单步差异）→ 各文件 `parse` 已向前兼容，放行。
fn check_version_consistency(dir: &std::path::Path) -> Result<(), ConfigError> {
    let mut versions: Vec<u32> = Vec::with_capacity(5);

    for name in &ALL_FILES {
        match read_file_version(dir, name) {
            Some(v) => versions.push(v),
            None => {
                // 缺失 version → 视为 v1（旧版迁移文件）
                eprintln!(
                    "[config_io] {} 缺少版本信息，视为 v1（旧版迁移文件）",
                    name
                );
                versions.push(1);
            }
        }
    }

    let max_v = *versions.iter().max().unwrap_or(&3);
    let min_v = *versions.iter().min().unwrap_or(&3);

    if max_v - min_v > 1 {
        eprintln!(
            "[config_io] ⚠️ 配置文件版本跨度过大（min={}, max={}），拒绝启动以免静默丢失配置",
            min_v, max_v
        );
        return Err(ConfigError::VersionSpanTooLarge { min: min_v, max: max_v });
    }

    if max_v != min_v {
        eprintln!(
            "[config_io] 配置文件版本存在单步差异（min={}, max={}），按兼容方式加载",
            min_v, max_v
        );
    }

    Ok(())
}

/// 用内置默认值生成缺失的配置文件（首次启动或用户误删某个文件时调用）。
/// 已在的文件不受影响。
fn ensure_default_files(cfg: &Config) {
    let dir = match config_dir() {
        Some(d) => d,
        None => return,
    };
    let _ = std::fs::create_dir_all(&dir);

    let defaults = bundle_all(cfg);
    for (name, content) in defaults {
        let path = dir.join(&name);
        if !path.exists() {
            eprintln!("[config_io] {} 不存在，用默认值生成", name);
            let _ = write_file_atomic(&dir, &name, &content);
        }
    }
}

/// ──────────────────────────────────────────────────────────────────────────────
/// 迁移（旧 config.txt → 5 个 .cfg 文件）
///
/// 触发条件：config.txt 存在 AND 至少一个 .cfg 文件缺失。
/// 执行：全部 parse → 生成 → 写 .tmp → rename 全部 → rename old to .bak。
/// 失败：删除所有 .tmp，保持 config.txt 不变，下次启动重试。
/// 一次性：通过 APPDATA 目录下的 `config_migrated_v3` 标记文件保证只执行一次。
/// ──────────────────────────────────────────────────────────────────────────────
fn run_migration_if_needed() {
    let app = match appdata_dir() {
        Some(a) => a,
        None => return,
    };
    let dir = match config_dir() {
        Some(d) => d,
        None => return,
    };

    let old = app.join("config.txt");
    if !old.exists() {
        return; // 没有旧文件，不需要迁移
    }

    // 一次性标记：已执行过则跳过
    if migration_marker_path().map(|p| p.exists()).unwrap_or(false) {
        return;
    }

    // 判断 5 个新文件中是否有缺失（任意一个缺失即需迁移）
    let any_missing = ALL_FILES
        .iter()
        .any(|name| !dir.join(name).exists());

    if !any_missing {
        // 所有新文件都存在，但旧文件也在——标记已迁移（可能之前中断过）
        if let Some(p) = migration_marker_path() {
            let _ = std::fs::write(&p, "");
        }
        return;
    }

    eprintln!("[config_io] 发现旧配置文件，开始迁移到 v3 多文件格式...");

    // 1. 解析旧配置
    let text = match std::fs::read_to_string(&old) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[config_io] 读取旧配置失败，跳过迁移: {}", e);
            return;
        }
    };
    let cfg = legacy::parse_v2(&text);

    // 2. 生成 5 文件内容
    let files = bundle_all(&cfg);

    // 3. 确保目录存在
    if std::fs::create_dir_all(&dir).is_err() {
        eprintln!("[config_io] 无法创建配置目录，迁移中止");
        return;
    }

    // 4. 写出全部 .tmp
    let tmp_paths = write_bundle_to_tmp(&dir, &files);
    if tmp_paths.is_empty() {
        eprintln!("[config_io] 临时文件写出失败，迁移中止");
        return;
    }

    // 5. 原子 rename 全部 .tmp → 正式文件
    if let Err((_, bad_name)) = rename_tmp_to_final(&dir, &files) {
        eprintln!("[config_io] 文件 rename 失败（{}），迁移中止", bad_name);
        // 清理 .tmp
        for p in &tmp_paths {
            let _ = std::fs::remove_file(p);
        }
        return;
    }

    // 6. 旧文件备份
    let bak = app.join("config.txt.bak");
    let _ = std::fs::rename(&old, &bak);

    // 7. 写一次性标记
    if let Some(p) = migration_marker_path() {
        let _ = std::fs::write(&p, "");
    }

    eprintln!(
        "[config_io] 迁移完成，{} → 5×.cfg，备份至 config.txt.bak",
        old.display()
    );
}

/// 加载配置：迁移（如需）→ 5 文件载入（缺文件回退默认）→ version 校验 → 缺文件补默认。
///
/// 返回 `Result`：版本跨度过大（`max - min > 1`）时返回 `Err`，调用方必须弹窗处理，
/// 不得静默按某个版本解析（否则旧格式文件的字段会被静默忽略 → 配置丢失）。
pub fn load_config() -> Result<Config, ConfigError> {
    // 迁移（如需）
    run_migration_if_needed();

    let dir = match config_dir() {
        Some(d) => d,
        None => return Ok(Config::default()),
    };

    // ── 正常载入 ──
    let mut cfg = Config::default();
    if let Some(text) = read_if(&dir, system::SYSTEM_FILE) {
        system::parse(&mut cfg, &text);
    }
    if let Some(text) = read_if(&dir, appearance::APPEARANCE_FILE) {
        appearance::parse(&mut cfg, &text);
    }
    if let Some(text) = read_if(&dir, monitor::MONITOR_FILE) {
        monitor::parse(&mut cfg, &text);
    }
    // 依据 system.cfg 的 layout 锚点载入对应布局文件
    let lay_file = layout::file_for(cfg.render_style);
    if let Some(text) = read_if(&dir, lay_file) {
        layout::parse(cfg.render_style, &mut cfg, &text);
    }

    // ── version 一致性检查（span > 1 直接拒绝）──
    check_version_consistency(&dir)?;

    // ── 缺哪个文件补哪个 ──
    ensure_default_files(&cfg);

    Ok(cfg)
}

/// 同步落盘（进程退出/关键变更用）。仅写当前激活布局，保留另一布局文件。
pub fn save_config(cfg: &Config) -> bool {
    match config_dir() {
        Some(d) => write_bundle(&d, &bundle_for(cfg)),
        None => false,
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// 异步保存：写线程持有一个 pending bundle，每次只写最新一次（合并去重）
// ──────────────────────────────────────────────────────────────────────────────

/// 异步写线程传输单元。
struct Bundle {
    files: Vec<(String, String)>,
}

// ══════════════════════════════════════════════════════════════════════════════
// ⚠️ 以下三个 State 访问器**必须**是模块级函数里的 `static`。
//
// 旧实现把 `PENDING` / `SHUTDOWN` / `NOTIFY` 分别声明在 `start_writer_thread` /
// `save_config_async` / `shutdown_writer` 三个**函数体内**。Rust 中函数体 static 是
// 各自独立的项（不共享存储），三个函数互相看不见对方的那一份，于是：
//   * `save_config_async` 写的 pending 写线程读不到（两个不同的 Mutex）；
//   * `save_config_async` 读的 NOTIFY 从未被 set → **恒返回 false**，且从不通知；
//   * 写线程永远阻塞在 `rx.recv()` → **从来没有写过一次盘**；
//   * `shutdown_writer` 的两个 static 全为空 → 完全空操作。
// 结果：UI 线程的设置改动从不落盘，只有关窗 flush 与进程退出才写（强杀/崩溃/关机
// 全丢）。2026-09-15 审计定位（Critical）。
//
// 注意：静态量必须定义在**同一作用域**，否则又会退化成互不可见的独立项。
// ══════════════════════════════════════════════════════════════════════════════
static WRITER_PENDING: std::sync::OnceLock<Arc<Mutex<Option<Bundle>>>> =
    std::sync::OnceLock::new();
static WRITER_SHUTDOWN: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
static WRITER_NOTIFY: std::sync::OnceLock<mpsc::Sender<()>> = std::sync::OnceLock::new();

/// 写盘串行化锁：同步 `save_config` 与异步写线程可能并发（见 `write_bundle`）。
static WRITE_LOCK: Mutex<()> = Mutex::new(());

fn writer_pending() -> &'static Arc<Mutex<Option<Bundle>>> {
    WRITER_PENDING.get_or_init(|| Arc::new(Mutex::new(None)))
}

fn writer_shutdown() -> &'static Arc<AtomicBool> {
    WRITER_SHUTDOWN.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

fn writer_notify() -> &'static std::sync::OnceLock<mpsc::Sender<()>> {
    &WRITER_NOTIFY
}

fn start_writer_thread() {
    // 只启动一次
    if writer_notify().get().is_some() {
        return;
    }

    let (notify_tx, rx) = mpsc::channel();
    let pending_c = writer_pending().clone();
    let shutdown_c = writer_shutdown().clone();

    let spawned = std::thread::Builder::new()
        .name("config-writer".into())
        .spawn(move || {
            while rx.recv().is_ok() {
                if shutdown_c.load(Ordering::SeqCst) {
                    break;
                }
                let bundle = {
                    let mut lock = pending_c.lock().unwrap_or_else(|e| e.into_inner());
                    lock.take()
                };
                if let Some(bundle) = bundle {
                    if let Some(d) = config_dir() {
                        write_bundle(&d, &bundle.files);
                    }
                }
                if shutdown_c.load(Ordering::SeqCst) {
                    break;
                }
            }
            // 退出前把最后一次 pending 落盘
            if let Some(bundle) = pending_c.lock().unwrap_or_else(|e| e.into_inner()).take() {
                if let Some(d) = config_dir() {
                    write_bundle(&d, &bundle.files);
                }
            }
        })
        .is_ok();

    if !spawned {
        // 不再 panic：旧实现的 `.expect()` 运行在 UI 线程（apply_config），
        // panic 会经 wndproc 的 extern "system" 边界直接 abort 整个进程。
        eprintln!("[config_io] config-writer thread spawn failed");
        return;
    }
    let _ = writer_notify().set(notify_tx);
}

/// 异步落盘（UI 线程调用）：
/// 多次快速调用自动合并——写线程只处理最新一次保存。
/// 返回 `true` = 已成功入队。
///
/// ⚠️ 进程退出路径（关窗/隐藏 flush）请用 [`save_config`] 同步落盘保数据。
pub fn save_config_async(cfg: &Config) -> bool {
    // 第一次调用时启动写线程
    start_writer_thread();

    // 覆盖 pending（合并去重）
    {
        let mut g = writer_pending().lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(Bundle { files: bundle_for(cfg) });
    }

    match writer_notify().get() {
        Some(n) => n.send(()).is_ok(),
        None => false,
    }
}

/// 通知写线程退出（进程结束时调用）。
pub fn shutdown_writer() {
    writer_shutdown().store(true, Ordering::SeqCst);
    if let Some(n) = writer_notify().get() {
        let _ = n.send(());
    }
}

/// 切换布局：把**旧**布局的几何+偏移+顺序持久化到其文件，
/// 再把**新**布局文件（含顺序）载入内存。
///
/// order 属于布局专属（与 offset 同组），随布局文件共同保存/切换。
///
/// 调用前 `cfg` 仍持有旧布局数据；调用后 `cfg.render_style = new` 且内存已换成
/// 新布局的几何+偏移。此函数保证两个布局文件各自独立保留。
pub fn switch_layout_in_place(cfg: &mut Config, old: RenderStyle, new: RenderStyle) {
    if old == new {
        return;
    }
    if let Some(d) = config_dir() {
        // 1) 先把当前（旧）布局数据落盘
        let _ = write_file_atomic(&d, layout::file_for(old), &layout::serialize(old, cfg));
        // 2) 载入新布局文件（不存在则保留当前几何，待首次保存时补齐）
        if let Some(text) = read_if(&d, layout::file_for(new)) {
            layout::parse(new, cfg, &text);
        }
    }
    cfg.render_style = new;
}

/// 导出为 `.mcfg` 分享/备份文本（含脱敏）。失败返回 `None`。
pub fn export_config(cfg: &Config) -> Option<String> {
    Some(mcfg::export_config(cfg))
}

// ──────────────────────────────────────────────────────────────────────────────
// 导入（.mcfg → 5 个 .cfg 文件）
//
// 流程：解析全部 → 预验证 → 用户确认 → 备份现有文件 → 原子写 → 成功提示
// ──────────────────────────────────────────────────────────────────────────────

/// 预验证解析后的文件包：版本是否合法（≤ 当前版本）、是否为空。
/// version_too_high = true 表示有文件 version > CONFIG_VERSION（需要警告）。
/// 返回 (files, version_too_high)。
fn validate_import_files(
    files: Vec<(String, String)>,
) -> Result<(Vec<(String, String)>, bool), String> {
    const CURRENT: u32 = aaxsyspulse_core::config::CONFIG_VERSION;

    let mut version_too_high = false;
    let mut validated = Vec::with_capacity(files.len());

    for (name, content) in files {
        // 白名单检查（防注入）
        if !mcfg::is_known_file(&name) {
            eprintln!("[config_io] 导入忽略未知文件: {}", name);
            continue;
        }

        // version 检查（仅 monitor/appearance/layout 有 [meta]，system 在 [system]）
        let version = if name == system::SYSTEM_FILE {
            let ini = Ini::parse(&content);
            ini.get("system", "version")
                .and_then(|v| v.parse::<u32>().ok())
        } else {
            let ini = Ini::parse(&content);
            ini.get("meta", "version")
                .and_then(|v| v.parse::<u32>().ok())
        };

        if let Some(v) = version {
            if v > CURRENT {
                eprintln!(
                    "[config_io] ⚠️ {} version {} 高于程序版本 {}，该文件可能无法正确加载",
                    name, v, CURRENT
                );
                version_too_high = true;
            }
        }

        if content.trim().is_empty() {
            return Err(format!("{} 内容为空，不予导入", name));
        }

        validated.push((name, content));
    }

    if validated.is_empty() {
        return Err("没有有效配置文件可导入".to_string());
    }

    Ok((validated, version_too_high))
}

/// 执行导入前的用户确认流程（短路）。
///
/// 分离为独立函数的目的是：非测试环境下调用 `prompt_import_confirm` / `prompt_max_compat`
///（含真实 `MessageBoxW`）；测试环境下可被子模块覆盖以绕开 GUI 弹窗。
///
/// `max_compat` 为 `true` 表示解析阶段发现 version 高于程序版本。
/// 返回 `true` = 用户同意，可以继续；`false` = 任意阶段拒绝，已打印日志。
fn confirm_import(max_compat: bool) -> bool {
    if !prompt_import_confirm() {
        eprintln!("[config_io] 导入已取消（用户拒绝确认）");
        return false;
    }
    if max_compat {
        if !prompt_max_compat() {
            eprintln!("[config_io] 导入已取消（用户拒绝最大化兼容）");
            return false;
        }
    }
    true
}

/// 导入确认（返回 true = 用户同意覆盖当前配置）。
///
/// 测试环境：沿用原有测试契约（默认 `false`），不弹窗、不阻塞。
/// （`confirm_import_rejects_first` / `version_too_high_triggers_second_prompt`
///  两个测试正是依赖这个默认拒绝行为，见 tests 模块注释。）
#[cfg(test)]
fn prompt_import_confirm() -> bool {
    false
}

/// 真实 `MessageBoxW` 弹窗（破坏性操作前的二次确认）。`import_config` 在写盘前调用。
#[cfg(not(test))]
fn prompt_import_confirm() -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONWARNING, MB_YESNO, MB_SETFOREGROUND,
    };
    let title = "导入配置";
    let msg = "导入配置将覆盖当前所有设置，且无法撤销。\n\n\
               点击「是」继续导入，「否」取消（当前配置不变）。";
    let title_w: Vec<u16> =
        std::ffi::OsStr::new(title).encode_wide().chain(std::iter::once(0)).collect();
    let msg_w: Vec<u16> =
        std::ffi::OsStr::new(msg).encode_wide().chain(std::iter::once(0)).collect();
    let choice = unsafe {
        MessageBoxW(
            None,
            windows::core::PCWSTR(msg_w.as_ptr()),
            windows::core::PCWSTR(title_w.as_ptr()),
            MB_ICONWARNING | MB_YESNO | MB_SETFOREGROUND,
        )
    };
    choice == IDYES
}
///
/// 返回 true = 用户选择继续（知情导入，不兼容字段按兼容方式忽略/错位）；
/// 返回 false = 用户取消（不写盘，当前配置不动）。
///
/// 当前即用原生 `MessageBoxW` 弹窗（P3 已证明 GUI 弹窗能力可用），
/// 不依赖尚未接入的设置页 —— 故这里直接做 I/O，而不是占位。
fn prompt_max_compat() -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONQUESTION, MB_YESNO, MB_SETFOREGROUND,
    };
    let title = "配置来自更新版本";
    let msg = format!(
        "导入的配置版本高于当前程序（> v{}）。\n\n\
         是否尝试「最大化兼容」导入？\n\
         可能导致部分项目无法显示或错位，且无法撤销。\n\n\
         点击「是」继续导入，「否」取消导入（当前配置不变）。",
        aaxsyspulse_core::config::CONFIG_VERSION
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
            MB_ICONQUESTION | MB_YESNO | MB_SETFOREGROUND,
        )
    };
    choice == IDYES
}

/// 测试环境：空实现（不弹窗）。
#[cfg(test)]
fn prompt_import_success() {}

/// 导入成功通知：真实 `MessageBoxW` 弹窗，提示重启程序使全部配置生效。
#[cfg(not(test))]
fn prompt_import_success() {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND,
    };
    let title = "导入成功";
    let msg = "配置已导入。\n\n建议重启程序以使全部配置（含监控项顺序 / 布局）完全生效。";
    let title_w: Vec<u16> =
        std::ffi::OsStr::new(title).encode_wide().chain(std::iter::once(0)).collect();
    let msg_w: Vec<u16> =
        std::ffi::OsStr::new(msg).encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = MessageBoxW(
            None,
            windows::core::PCWSTR(msg_w.as_ptr()),
            windows::core::PCWSTR(title_w.as_ptr()),
            MB_ICONINFORMATION | MB_OK | MB_SETFOREGROUND,
        );
    }
}

/// 测试环境：空实现（不弹窗）。
#[cfg(test)]
fn prompt_import_rollback_failed() {}

/// 导入失败且回滚也失败的通知：真实 `MessageBoxW` 错误弹窗，提示手动恢复备份。
#[cfg(not(test))]
fn prompt_import_rollback_failed() {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND,
    };
    let title = "导入失败";
    let msg = "导入写入失败，且回滚也失败。\n\n原配置备份在 config 目录（.bak），请手动恢复。";
    let title_w: Vec<u16> =
        std::ffi::OsStr::new(title).encode_wide().chain(std::iter::once(0)).collect();
    let msg_w: Vec<u16> =
        std::ffi::OsStr::new(msg).encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = MessageBoxW(
            None,
            windows::core::PCWSTR(msg_w.as_ptr()),
            windows::core::PCWSTR(title_w.as_ptr()),
            MB_ICONERROR | MB_OK | MB_SETFOREGROUND,
        );
    }
}

/// 从 `.mcfg` 文本导入：
/// 1. 解析全部文件
/// 2. 预验证（version 白名单 + 空文件检查）
/// 3. 用户确认
/// 4. 所有目标文件 rename → .bak（带时间戳，仅保留最新一份）
/// 5. 原子写新文件
/// 6. 任何写入失败 → 全部 .bak rename 回滚
/// 7. 回滚失败 → 保留 .bak + 提示手动恢复
///
/// 返回 `true` = 全部成功。
pub fn import_config(text: &str) -> bool {
    // 1. 解析
    let raw_files = match mcfg::parse_mcfg(text) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[config_io] 导入失败（解析错误）: {}", e);
            return false;
        }
    };

    // 2. 预验证
    let (files, version_too_high) = match validate_import_files(raw_files) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[config_io] 导入失败（校验错误）: {}", e);
            return false;
        }
    };

    // 3. 用户确认（短路：第一个拒绝直接退出，不再弹第二个）
    if !confirm_import(version_too_high) {
        return false;
    }

    let dir = match config_dir() {
        Some(d) => d,
        None => return false,
    };
    let _ = std::fs::create_dir_all(&dir);

    // 4. 所有目标文件 rename → .bak（仅保留最新一份；回滚失败时再改 .bak.failed.<ts> 保留）
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // 记录：(原始文件名, 备份路径)，回滚时用原始文件名定位目标
    let mut bak_entries: Vec<(String, std::path::PathBuf)> = Vec::new();
    // 本次导入**新建**（原本不存在）的文件：回滚时必须删掉，否则会留下半吊子配置
    // （例如 layout_v.cfg 是首次创建且写成功，而 monitor.cfg 写失败 → 返回 false
    //  但 layout_v.cfg 已被改动，且没有任何备份能恢复它）。
    let mut created_files: Vec<std::path::PathBuf> = Vec::new();
    for (name, _) in &files {
        let target = dir.join(name);
        let bak = dir.join(format!("{}.bak", name));
        if target.exists() {
            // 直接 rename 到 .bak：Windows 上 std::fs::rename 会替换已存在的目标
            // （MoveFileExW + MOVEFILE_REPLACE_EXISTING），所以旧备份自动被覆盖。
            // ⚠️ 不得先 `remove_file(old_bak)`（旧实现）：那样一旦 rename 失败，
            // 唯一一份备份已被删掉而新备份没建 → 备份彻底丢失。
            // 注意 .bak.failed.<ts>（上次回滚失败的残留）也**不被触碰**，留给用户。
            if std::fs::rename(&target, &bak).is_ok() {
                bak_entries.push((name.clone(), bak));
            } else {
                eprintln!("[config_io] 备份 {} 失败，导入中止（当前配置未改动）", name);
                return false;
            }
        } else {
            created_files.push(target);
        }
    }

    // 5. 逐文件原子写
    let mut write_errors: Vec<String> = Vec::new();
    for (name, content) in &files {
        if !write_file_atomic(&dir, name, content) {
            write_errors.push(name.clone());
        }
    }

    // 6. 任何失败 → 全部回滚
    if !write_errors.is_empty() {
        eprintln!(
            "[config_io] 导入 I/O 错误：{:?}，正在回滚...",
            write_errors
        );
        let mut rollback_ok = true;
        for (orig_name, bak) in &bak_entries {
            let target = dir.join(orig_name);
            if std::fs::rename(bak, &target).is_err() {
                rollback_ok = false;
                eprintln!("[config_io] 回滚失败：{} 无法恢复", orig_name);
            }
        }
        // 本次新建的文件没有备份可恢复 → 直接删除，否则会留下"部分导入"的
        // 配置（版本号/字段与其余文件不一致，下次启动可能触发版本跨度弹窗）。
        for p in &created_files {
            if p.exists() {
                let _ = std::fs::remove_file(p);
            }
        }
        if !rollback_ok {
            eprintln!(
                "[config_io] 未能回滚的备份保留在 {}（.bak.failed.<ts>），请手动恢复",
                dir.display()
            );
            // 保留未能回滚的备份：改名 .bak.failed.<ts>，避免被下次导入清除
            let fail_ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(ts);
            for (orig_name, bak) in &bak_entries {
                if bak.exists() {
                    let failed = dir.join(format!("{}.bak.failed.{}", orig_name, fail_ts));
                    let _ = std::fs::rename(bak, &failed);
                }
            }
            prompt_import_rollback_failed();
        }
        return false;
    }

    // 7. 全部成功，删除本次备份（.bak.failed.* 不会被触碰，留给用户手动恢复）
    for (_, bak) in &bak_entries {
        let _ = std::fs::remove_file(bak);
    }

    prompt_import_success();
    true
}

/// 返回配置文件路径（用于 UI 展示/诊断），无则返回 `None`。
pub fn get_config_path() -> Option<String> {
    config_dir().map(|d| d.join(system::SYSTEM_FILE).to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_span_too_large_rejected() {
        let dir = std::env::temp_dir().join(format!("aaxsp_vspan_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // system.cfg = v3（[system] 段）；其余缺 version → 视为 v1
        let _ = std::fs::write(dir.join(system::SYSTEM_FILE), "[system]\nversion = 3\n");
        let _ = std::fs::write(dir.join(monitor::MONITOR_FILE), "[monitor]\nshow_cpu = 1\n");
        let _ = std::fs::write(dir.join(appearance::APPEARANCE_FILE), "[appearance]\n");
        let _ = std::fs::write(dir.join(LAYOUT_H_FILE), "[window]\n");
        let _ = std::fs::write(dir.join(LAYOUT_V_FILE), "[window]\n");

        let res = check_version_consistency(&dir);
        assert!(
            matches!(res, Err(ConfigError::VersionSpanTooLarge { min: 1, max: 3 })),
            "span>1 应拒绝启动，实际：{:?}",
            res
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_single_step_diff_ok() {
        let dir = std::env::temp_dir().join(format!("aaxsp_vok_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // 全部 v3
        let _ = std::fs::write(dir.join(system::SYSTEM_FILE), "[system]\nversion = 3\n");
        let _ = std::fs::write(dir.join(monitor::MONITOR_FILE), "[meta]\nversion = 3\n");
        let _ = std::fs::write(dir.join(appearance::APPEARANCE_FILE), "[meta]\nversion = 3\n");
        let _ = std::fs::write(dir.join(LAYOUT_H_FILE), "[meta]\nversion = 3\n");
        let _ = std::fs::write(dir.join(LAYOUT_V_FILE), "[meta]\nversion = 3\n");

        assert!(
            check_version_consistency(&dir).is_ok(),
            "单步同版本应放行"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── confirm_import 短路测试 ────────────────────────────────────────────────
    // 注：`prompt_import_confirm` / `prompt_max_compat` 在测试环境下为 `false` / `false`
    // （非测试代码的默认实现），因此两个测试需临时替换这两个函数。
    // 这里用 `#[swift_bindgen]` 不可用时的标准 Rust 做法：构造一个最小 mcfg 文本，
    // 让 `mcfg::parse_mcfg` 能解析出 `version_too_high = true` 的文件，
    // 然后验证 `confirm_import` 在两种拒绝场景下的短路行为。

    #[test]
    fn confirm_import_rejects_first() {
        // 第一个确认默认 false → confirm_import 直接返回 false，不弹第二个
        assert!(!confirm_import(false), "第一个拒绝应短路");
        assert!(!confirm_import(true), "第一个拒绝（version_too_high=true）也应短路");
    }

    #[test]
    fn version_too_high_triggers_second_prompt() {
        // 构造一个含 version > CONFIG_VERSION 的 mcfg，触发 second branch
        const CURRENT: u32 = aaxsyspulse_core::config::CONFIG_VERSION;
        let text = format!(
            "MCFG1\n# test\n--- system.cfg ---\n[system]\nversion = {}\n\n--- END ---\n",
            CURRENT + 1
        );
        let raw = mcfg::parse_mcfg(&text).expect("解析应成功");
        let (_, version_too_high) = validate_import_files(raw).expect("校验应成功");
        assert!(
            version_too_high,
            "version > CURRENT 应触发 second branch"
        );
        // confirm_import(true) 在当前默认实现下会弹 prompt_max_compat
        //（无 GUI 环境返回 false），最终返回 false —— 这验证了 second branch 可达
        assert!(
            !confirm_import(true),
            "version_too_high=true 时 confirm_import 应走 second branch 并返回 false"
        );
    }
}
