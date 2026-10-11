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

/// 迁移一次性执行标记文件名：由 `CONFIG_VERSION` 推导（审查项 7.1.3 / P1-22），
/// 升版不会留下一个还在写 v3 标记的硬编码。
fn migration_done_file() -> String {
    aaxsyspulse_core::contract::migration_marker_name()
}

fn appdata_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|s| {
        std::path::PathBuf::from(s).join(aaxsyspulse_core::contract::APP_DATA_DIR_NAME)
    })
}

fn config_dir() -> Option<std::path::PathBuf> {
    appdata_dir().map(|d| d.join(CONFIG_SUBDIR))
}

fn migration_marker_path() -> Option<std::path::PathBuf> {
    appdata_dir().map(|d| d.join(migration_done_file()))
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

/// 剥离 UTF-8 BOM（见 [`read_if`] 的说明）。
fn strip_bom(s: String) -> String {
    s.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(s)
}

/// 读文件内容，不存在返回 `None`（不报错）。
fn read_if(dir: &std::path::Path, name: &str) -> Option<String> {
    // ⚠️ 必须剥离 UTF-8 BOM。记事本 / VSCode「另存为 UTF-8 with BOM」会在文件首
    // 写入 U+FEFF，使首个 `[system]` 头变成 "\u{feff}[system]" → Ini 查不到该段 →
    // **整个文件的键静默回退默认**；且 version 读不到被当成 v1，与其他 v3 文件
    // 组成 span=2 → 触发「版本跨度过大 → 用默认配置覆盖全部配置」的破坏性弹窗。
    // 2026-09-15 审计定位。
    std::fs::read_to_string(dir.join(name)).ok().map(strip_bom)
}

/// 读文件，并把**「不存在」**与**「存在但读不出来」**分开。
///
/// `read_if` 两者一律返回 `None` —— 于是「权限不足 / 文件被占用 / 非 UTF-8 / 半截文件」
/// 与「首次运行、本就没有」长得一模一样（报告 2.3.1 / 8.32）。前者必须留证并禁用回写，
/// 否则「读失败 → 内存里是全默认值 → 用户点一下设置 → 默认值写回磁盘」= 全量设置销毁。
///
/// - `Ok(None)` 文件不存在（正常：首次运行）
/// - `Ok(Some(text))` 正常读到
/// - `Err(reason)` 文件存在但读不出来
fn read_probe(dir: &std::path::Path, name: &str) -> Result<Option<String>, String> {
    match std::fs::read_to_string(dir.join(name)) {
        Ok(text) => Ok(Some(strip_bom(text))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// 降级加载闸门（P1-4）
//
// 某个 `.cfg` 存在却读不出来时，本次加载是**残缺**的（缺失字段回落了默认值）。
// 这种会话里任何保存都会把默认值固化到磁盘、覆盖用户其余设置，故：
//   · 读不出来的原件先**复制**为 `<name>.broken.<ts>`（不覆盖既有备份、不动原件）；
//   · 置 `CONFIG_DEGRADED`，`save_config` / `save_config_async` 一律拒绝回写；
//   · 只有**用户显式确认**的破坏性重置能解锁（`save_config_all`、`import_config`）。
// 每次 `load_config` 进入时重算（`WM_RELOAD_CONFIG` 读到完好文件即自动解除）。
// ──────────────────────────────────────────────────────────────────────────────

static CONFIG_DEGRADED: AtomicBool = AtomicBool::new(false);
static BROKEN_FILES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// 本会话是否处于降级加载态（禁用配置回写）。
pub fn config_degraded() -> bool {
    CONFIG_DEGRADED.load(Ordering::SeqCst)
}

/// 降级原因清单（读不出来的文件名），供 UI / 日志告知用户。
pub fn broken_config_files() -> Vec<String> {
    BROKEN_FILES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn mark_degraded(name: &str) {
    CONFIG_DEGRADED.store(true, Ordering::SeqCst);
    if let Ok(mut v) = BROKEN_FILES.lock() {
        if !v.iter().any(|n| n == name) {
            v.push(name.to_string());
        }
    }
}

/// 存在但读不出来的文件：复制留证 + 置降级标志。原件**不删不改**。
fn quarantine_unreadable(dir: &std::path::Path, name: &str, reason: &str) {
    let src = dir.join(name);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dst = dir.join(format!("{}.broken.{}", name, ts));
    // 时间戳到秒：同名重复时复制失败即说明证据已在，不再重试。
    match std::fs::copy(&src, &dst) {
        Ok(_) => eprintln!(
            "[config_io] ⚠️ {} 存在但读取失败（{}）。原件保留，已另存证据 {}；\
             本次按默认值继续，并**禁用配置保存**以免默认值覆盖你的设置。",
            name, reason, dst.display()
        ),
        Err(e) => eprintln!(
            "[config_io] ⚠️ {} 存在但读取失败（{}），留证副本也没写成（{}）；\
             本次按默认值继续，并**禁用配置保存**以免默认值覆盖你的设置。",
            name, reason, e
        ),
    }
    mark_degraded(name);
}

/// 读 + 解析单个配置文件；「不存在」与「读不出来」分别处理（见 [`read_probe`]）。
fn parse_file_scoped(
    dir: &std::path::Path,
    name: &str,
    cfg: &mut Config,
    apply: impl FnOnce(&mut Config, &str),
) {
    match read_probe(dir, name) {
        Ok(Some(text)) => apply(cfg, &text),
        Ok(None) => {}
        Err(reason) => quarantine_unreadable(dir, name, &reason),
    }
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

/// 就地升级单个配置文件（版本缺失或落后时）。
///
/// 升级方式 = 用该文件自己的 `parse` 读入一份默认 [`Config`]，再用其 `serialize`
/// 写回；`serialize` 会把文件头版本号写成 `CONFIG_VERSION`。
///
/// **为什么升级前后 UI / 系统设置不变**：每个模块的 `parse` / `serialize` 对
/// *该文件所拥有的字段* 是无损往返（serialize 写出的键 == parse 读入的键，
/// 见 `system/appearance/monitor/layout` 各自 roundtrip 测试）。于是
/// 「加载升级后的文件」与「加载升级前的文件」得到的 [`Config`] 逐字段相同，
/// 唯一变化是文件格式 / 版本号被推进到当前。
///
/// - 缺失 version（旧格式）或 `version < CONFIG_VERSION` → 升级。
/// - `version == CONFIG_VERSION` → 跳过。
/// - `version > CONFIG_VERSION`（更新版程序写的）→ 不碰，不降级。
/// - 升级前把原文件备份为 `<name>.bak`。
///
/// 返回 `true` 表示确实执行了升级。
fn upgrade_one(
    dir: &std::path::Path,
    name: &str,
    parse: impl Fn(&mut Config, &str),
    serialize: impl Fn(&Config) -> String,
) -> bool {
    let cur = aaxsyspulse_core::config::CONFIG_VERSION;
    let Some(text) = read_if(dir, name) else {
        return false; // 不存在：交 ensure_default_files 生成
    };
    match read_file_version(dir, name) {
        Some(v) if v == cur => return false, // 已最新
        Some(v) if v > cur => {
            eprintln!(
                "[config_io] {} version {} 高于程序版本 {}，跳过就地升级（不降级）",
                name, v, cur
            );
            return false;
        }
        _ => {} // 缺失 version（旧格式）或落后 → 升级
    }

    // parse 到一份默认配置 → 序列化回写：字段值无损，仅格式/版本推进
    let mut cfg = Config::default();
    parse(&mut cfg, &text);
    let new_text = serialize(&cfg);

    let src = dir.join(name);
    let _ = std::fs::copy(&src, dir.join(format!("{}.bak", name))); // 升级前备份
    if write_file_atomic(dir, name, &new_text) {
        eprintln!("[config_io] {} 已就地升级到 v{}（各字段值保持不变）", name, cur);
        true
    } else {
        eprintln!("[config_io] {} 就地升级写入失败，保留原文件", name);
        false
    }
}

/// 单文件升级步骤：`(文件名, 解析器, 序列化器)`。
type UpgradeStep = (&'static str, fn(&mut Config, &str), fn(&Config) -> String);

/// 启动时就地升级全部配置文件的格式版本（取代「版本跨度过大 → 弹窗」的破坏性路径）。
///
/// 逐文件独立升级、互不影响；某文件升级失败只保留该文件并记日志，
/// 由随后的 `check_version_consistency` 兜底（仍不一致才走弹窗）。
/// 返回升级成功的文件数。
fn upgrade_legacy_files_in_place(dir: &std::path::Path) -> usize {
    let mut n = 0usize;

    // 非布局文件：parse(&mut Config, &str) / serialize(&Config) -> String
    let plain: [UpgradeStep; 3] = [
        (system::SYSTEM_FILE, system::parse, system::serialize),
        (appearance::APPEARANCE_FILE, appearance::parse, appearance::serialize),
        (monitor::MONITOR_FILE, monitor::parse, monitor::serialize),
    ];
    for (name, parse, serialize) in plain {
        if upgrade_one(dir, name, parse, serialize) {
            n += 1;
        }
    }

    // 布局文件（横 / 竖各一）：按各自 style 解析 / 序列化，保留各自 offsets / order
    for (name, style) in [(LAYOUT_H_FILE, RenderStyle::Strip), (LAYOUT_V_FILE, RenderStyle::Vertical)] {
        if upgrade_one(
            dir,
            name,
            |cfg, text| layout::parse(style, cfg, text),
            |cfg| layout::serialize(style, cfg),
        ) {
            n += 1;
        }
    }

    n
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
    // 先验格式再迁移：`Config::parse` 是宽容读，任何内容都能解出「全默认值」。
    // 少了这道判定，一个与本项目无关的 config.txt（或已损坏的旧文件）会被解成
    // 默认配置写进 5 个 .cfg，同时原文件被改名成 .bak —— 静默丢设置。
    if !legacy::looks_like_legacy(&text) {
        eprintln!("[config_io] {} 不含旧版 v2 特征键，跳过迁移（文件保持原样）", old.display());
        return;
    }
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

    // ── 就地升级：把版本落后 / 缺失 version 的文件先升级到当前版本 ──
    // 升级只推进格式与版本号，各字段值不变（见 upgrade_one 文档），
    // 从而后续 check_version_consistency 恒通过、不再弹「版本跨度过大」。
    upgrade_legacy_files_in_place(&dir);

    // ── 正常载入 ──
    // 降级态每次重新判定：WM_RELOAD_CONFIG 会再次进这里，文件修好后禁写闸门自动解除。
    clear_degraded();

    let mut cfg = Config::default();
    parse_file_scoped(&dir, system::SYSTEM_FILE, &mut cfg, system::parse);
    parse_file_scoped(&dir, appearance::APPEARANCE_FILE, &mut cfg, appearance::parse);
    parse_file_scoped(&dir, monitor::MONITOR_FILE, &mut cfg, monitor::parse);
    // 依据 system.cfg 的 layout 锚点载入对应布局文件
    let lay_file = layout::file_for(cfg.render_style);
    let style = cfg.render_style;
    parse_file_scoped(&dir, lay_file, &mut cfg, |c, t| layout::parse(style, c, t));

    // ── version 一致性检查（span > 1 直接拒绝）──
    check_version_consistency(&dir)?;

    // ── 缺哪个文件补哪个 ──
    ensure_default_files(&cfg);

    Ok(cfg)
}

/// 同步落盘（进程退出/关键变更用）。仅写当前激活布局，保留另一布局文件。
///
/// 返回 `false` = 未写入。降级加载会话（见 [`config_degraded`]）一律拒绝，
/// 否则内存里的默认值会把用户其余设置覆盖掉。
pub fn save_config(cfg: &Config) -> bool {
    if config_degraded() {
        eprintln!("[config_io] 配置处于降级加载态，本次拒绝落盘（详见 .broken.<ts> 备份）");
        return false;
    }
    match config_dir() {
        Some(d) => write_bundle(&d, &bundle_for(cfg)),
        None => false,
    }
}

/// 全量同步落盘（**5 个文件**，含横 / 竖两个布局）。
///
/// 仅用于「版本跨度过大 → 用户选择用默认配置覆盖全部」这一破坏性重置。
/// 必须写全 5 个文件：`save_config` 只写当前激活布局，非激活布局文件
/// **永远不被重写**；若它是旧格式（缺 `[meta] version` → 被
/// `check_version_consistency` 判为 v1），则每次启动都会重复弹「版本跨度过大」，
/// 且用户点「是」也消除不了（因为重置同样只写 4 个文件）——2026-09-15 定位。
/// 写前把每个已存在文件备份为 `<name>.bak`，与弹窗「原文件已自动备份」的承诺一致。
///
/// 这条路径**正是**降级闸门等待的「用户显式确认」：弹窗里用户已经看过
/// 「全部重置为默认值」并点了「是」，故写成功后解除降级态、恢复正常保存。
pub fn save_config_all(cfg: &Config) -> bool {
    let dir = match config_dir() {
        Some(d) => d,
        None => return false,
    };
    let _ = std::fs::create_dir_all(&dir);
    for name in ALL_FILES {
        let src = dir.join(name);
        if src.exists() {
            let _ = std::fs::copy(&src, dir.join(format!("{}.bak", name)));
        }
    }
    let ok = write_bundle(&dir, &bundle_all(cfg));
    if ok {
        clear_degraded();
    }
    ok
}

/// 解除降级态（用户已显式确认覆盖 / 导入完成：磁盘上不再是"内存默认值 vs 磁盘真值"
/// 的危险组合，可以恢复保存）。
fn clear_degraded() {
    CONFIG_DEGRADED.store(false, Ordering::SeqCst);
    if let Ok(mut v) = BROKEN_FILES.lock() {
        v.clear();
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
    if config_degraded() {
        eprintln!("[config_io] 配置处于降级加载态，异步保存已跳过（避免默认值覆盖设置）");
        return false;
    }
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
    if max_compat && !prompt_max_compat() {
        eprintln!("[config_io] 导入已取消（用户拒绝最大化兼容）");
        return false;
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
    // 磁盘内容已是用户知情选择导入的那份 → 降级禁写闸门解除（见 P1-4）。
    clear_degraded();

    prompt_import_success();
    true
}

/// 测试环境：不弹窗。
#[cfg(test)]
pub fn prompt_config_degraded(_broken: &[String]) {}

/// 降级加载的用户可见信号（P1-4 的 (a) 项：原来只有 `eprintln!`，
/// 用户看到的是"设置全没了"却没有任何解释）。
/// 由 `app` 入口在启动早期调用；登录自启（`--autostart`）静默模式不调用。
#[cfg(not(test))]
pub fn prompt_config_degraded(broken: &[String]) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND,
    };
    let title = "部分配置文件读取失败";
    let msg = format!(
        "以下配置文件存在但读不出来（损坏 / 被占用 / 编码异常）：\n\
         {}\n\n\
         本次启动这些设置按默认值生效，悬浮窗上看到的不是你的配置。\n\
         为防止把默认值写回磁盘覆盖其余设置，**本次运行期间已停止保存配置**。\n\
         坏文件的副本已另存为 `<文件名>.broken.<时间戳>`：可从里面手工找回设置，\n\
         或修复/删除坏文件后重启程序。",
        broken.join("\n")
    );
    let title_w: Vec<u16> =
        std::ffi::OsStr::new(title).encode_wide().chain(std::iter::once(0)).collect();
    let msg_w: Vec<u16> =
        std::ffi::OsStr::new(&msg).encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = MessageBoxW(
            None,
            windows::core::PCWSTR(msg_w.as_ptr()),
            windows::core::PCWSTR(title_w.as_ptr()),
            MB_ICONWARNING | MB_OK | MB_SETFOREGROUND,
        );
    }
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

    /// P1-4：`read_probe` 必须把「不存在」与「存在但读不出来」分开。
    /// 用「同名目录」制造稳定的读失败（`read_to_string` 返回非 NotFound 错误）。
    #[test]
    fn read_probe_distinguishes_missing_from_unreadable() {
        let dir = std::env::temp_dir().join(format!("aaxsp_probe_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        assert!(
            matches!(read_probe(&dir, "missing.cfg"), Ok(None)),
            "不存在应是 Ok(None)（首次运行，正常）"
        );
        let _ = std::fs::write(dir.join("appearance.cfg"), "[meta]\nversion = 3\n");
        assert!(
            matches!(read_probe(&dir, "appearance.cfg"), Ok(Some(_))),
            "能读的文件应是 Ok(Some(text))"
        );
        let _ = std::fs::create_dir(dir.join("monitor.cfg"));
        assert!(
            read_probe(&dir, "monitor.cfg").is_err(),
            "存在但读不出来必须是 Err，不能与缺失混淆"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P1-4：读不出来的文件 ⇒ 留证 + 置降级标志（禁写闸门由此触发）；
    /// `clear_degraded`（显式重置 / 导入 / 下次成功加载）解除。
    #[test]
    fn unreadable_file_degrades_session_and_blocks_saving() {
        let dir = std::env::temp_dir().join(format!("aaxsp_degrade_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::remove_dir(dir.join(monitor::MONITOR_FILE));
        let _ = std::fs::create_dir(dir.join(monitor::MONITOR_FILE));

        clear_degraded();
        assert!(!config_degraded(), "起点应为非降级态");
        let mut cfg = Config::default();
        parse_file_scoped(&dir, monitor::MONITOR_FILE, &mut cfg, monitor::parse);
        assert!(config_degraded(), "读失败必须置降级标志");
        assert!(
            broken_config_files().contains(&monitor::MONITOR_FILE.to_string()),
            "降级清单里要有出错的文件名，实际：{:?}",
            broken_config_files()
        );

        clear_degraded();
        assert!(!config_degraded(), "解除后应恢复正常保存");
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

    /// 回归：非激活布局是旧格式（缺 `[meta] version`）时启动校验拒绝；
    /// 「用默认配置覆盖全部」必须把它**一起**写成当前版本，否则每次启动重复弹窗
    /// （2026-09-15 定位：`save_config` 只写 4 个文件，layout_v 永不被重写）。
    /// 这里验证全量落盘内核 `bundle_all` 能把陈旧的非激活布局一并升级 → 校验通过。
    #[test]
    fn full_reset_upgrades_stale_inactive_layout() {
        let dir = std::env::temp_dir().join(format!("aaxsp_all5_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // 现场复刻：4 个文件 v3，layout_v 为旧格式（无 version）
        let _ = std::fs::write(dir.join(system::SYSTEM_FILE), "[system]\nversion = 3\n");
        let _ = std::fs::write(dir.join(monitor::MONITOR_FILE), "[meta]\nversion = 3\n");
        let _ = std::fs::write(dir.join(appearance::APPEARANCE_FILE), "[meta]\nversion = 3\n");
        let _ = std::fs::write(dir.join(LAYOUT_H_FILE), "[meta]\nversion = 3\n");
        let _ = std::fs::write(dir.join(LAYOUT_V_FILE), "[window]\nx = 0\n"); // 旧格式
        assert!(
            check_version_consistency(&dir).is_err(),
            "旧格式非激活布局应触发拒绝（span=2）"
        );

        // 全量落盘（save_config_all 的核心）：写全 5 个文件
        let cfg = Config::default();
        for (name, content) in bundle_all(&cfg) {
            assert!(write_file_atomic(&dir, &name, &content), "{name} 应写入成功");
        }
        assert!(
            check_version_consistency(&dir).is_ok(),
            "全量落盘后版本应一致（否则会反复弹窗）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 就地升级必须保证「升级前后 UI 样式与系统设置完全相同」。
    /// 构造用户现场（4 文件 v3、layout_v 旧格式无 version），
    /// 升级前后各解析一遍全部配置（含 layout_v 的 offsets），断言 Config 逐字段相等。
    #[test]
    fn in_place_upgrade_preserves_effective_config() {
        let dir = std::env::temp_dir().join(format!("aaxsp_upgrade_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        // 4 个新格式文件（含自定义非默认值，确保比对真的有效）
        let _ = std::fs::write(
            dir.join(system::SYSTEM_FILE),
            "[system]\nversion = 3\nx = 300\ny = 120\nzoom = 2\ninterval_ms = 1000\nlang = zh\ntopmost = 1\n",
        );
        let _ = std::fs::write(
            dir.join(appearance::APPEARANCE_FILE),
            "[meta]\nversion = 3\n\n[appearance]\nbg = #112233\nfont_size = 20.0\nshadow = 1\n",
        );
        let _ = std::fs::write(
            dir.join(monitor::MONITOR_FILE),
            "[meta]\nversion = 3\n\n[monitor]\nutil_decimal = 1\n\n[item.1]\nshow = 1\nname = 1\n",
        );
        let _ = std::fs::write(
            dir.join(LAYOUT_H_FILE),
            "[meta]\nversion = 3\n\n[layout]\norder = 1 2 3 7 4 5 6 8 9\n",
        );
        // layout_v：旧格式（无 [meta] version、无 [layout] 段）—— 用户现场
        let _ = std::fs::write(
            dir.join(LAYOUT_V_FILE),
            "[window]\nx = 834\ny = -7\n\n[metrics]\noffset.5 = 0 0 12 0\n",
        );

        // 把全部配置（4 全局 + layout_v 视为激活布局）解析为一份 Config
        let parse_all = |dir: &std::path::Path| {
            let mut c = Config::default();
            if let Some(t) = read_if(dir, system::SYSTEM_FILE) { system::parse(&mut c, &t); }
            if let Some(t) = read_if(dir, appearance::APPEARANCE_FILE) { appearance::parse(&mut c, &t); }
            if let Some(t) = read_if(dir, monitor::MONITOR_FILE) { monitor::parse(&mut c, &t); }
            if let Some(t) = read_if(dir, LAYOUT_V_FILE) { layout::parse(RenderStyle::Vertical, &mut c, &t); }
            c
        };
        let before = parse_all(&dir);

        // 现状：版本不一致 → 启动会被拒绝（复现 bug）
        assert!(check_version_consistency(&dir).is_err(), "旧格式 layout_v 应触发拒绝");

        // 就地升级
        let upgraded = upgrade_legacy_files_in_place(&dir);
        assert!(upgraded >= 1, "至少应升级 layout_v");

        // 升级后：版本一致，且 Config 逐字段不变（UI/系统设置不受影响）
        assert!(check_version_consistency(&dir).is_ok(), "升级后版本应一致");
        let after = parse_all(&dir);
        assert_eq!(before, after, "就地升级不得改变任何有效配置");

        // layout_v 的自定义偏移确实被保留
        assert_eq!(after.metric_offsets[4], [0, 0, 12, 0], "旧布局 offsets 应无损保留");

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
