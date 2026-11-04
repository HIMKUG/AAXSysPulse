//! 安装模块
//! 负责释放主程序和驱动到安装位置。
//! 第二部分 2.2：一次性写入 + 原子替换（写 .tmp 后 rename）+ 大小校验 + 增量跳过。
//! 第四部分：run_install 按进度表逐步执行并通过 install-progress 事件推进度。

use crate::resources::{get_driver_bytes, get_main_program_bytes};
// 跨进程契约唯一真源（审查项 3.1.14 / 7.1.1–7.1.3 / P1-22）：
// 名字/键/路径/消息号一律从这里取，本 crate 不再留第二份字面量。
use aaxsyspulse_core::contract as c;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tauri::{AppHandle, Emitter};

#[derive(Error, Debug)]
pub enum InstallError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to get LocalAppData folder")]
    NoLocalAppData,
    #[error("内置资源不可用: {0}")]
    Resource(String),
    #[error("释放的文件已损坏（大小不一致）")]
    FileCorrupted,
    #[error("目标文件正在被使用，无法替换：{0}")]
    Locked(String),
}

/// 目标文件是否与给定字节**逐字节等价**（先比大小，再分块流式比对）。
///
/// 首轮 220：旧实现只比大小 + FNV-1a 指纹 —— 哈希碰撞（概率极低但非零）会把
/// "同尺寸的新版本"误判成最新并跳过更新；且续篇 670：bytes 本身已在内存里，
/// 再整块 `fs::read` 翻倍安装器内存峰值。改为 64KB 分块流式逐字节比对：
/// 零碰撞、内存峰值不增。
fn same_content(path: &Path, bytes: &[u8]) -> bool {
    use std::io::Read;
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() != bytes.len() as u64 {
        return false;
    }
    let Ok(mut f) = fs::File::open(path) else {
        return false;
    };
    let mut chunk = [0u8; 64 * 1024];
    let mut consumed = 0usize;
    loop {
        let n = match f.read(&mut chunk) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if n == 0 {
            break;
        }
        if bytes.get(consumed..consumed + n) != Some(&chunk[..n]) {
            return false;
        }
        consumed += n;
    }
    consumed == bytes.len()
}

/// Win32 错误码（首轮 222：不用裸魔数 `5 | 32`）。非 Windows 平台 raw_os_error
/// 不会命中这两个值。
const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_SHARING_VIOLATION: i32 = 32;

/// 一次性写入 + 原子替换：先写 .tmp（fsync）再 rename 就位，最后**逐字节**校验。
///
/// 任一环节失败都**不留 .tmp**（否则下次安装会看到一份"看着像最新版"的孤儿文件），
/// 并把失败如实抛给调用方（`run_install` 会记为中止失败）。
fn atomic_replace(temp: &Path, target: &Path, bytes: &[u8]) -> Result<(), InstallError> {
    {
        use std::io::Write;
        // 首轮 221：fs::write 不 fsync —— rename 后掉电可能留下指向未落盘内容的文件。
        let mut f = fs::File::create(temp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if let Err(e) = fs::rename(temp, target) {
        let _ = fs::remove_file(temp);
        // ERROR_ACCESS_DENIED / ERROR_SHARING_VIOLATION：目标正在运行。
        let code = e.raw_os_error().unwrap_or(0);
        if code == ERROR_ACCESS_DENIED || code == ERROR_SHARING_VIOLATION {
            return Err(InstallError::Locked(format!(
                "{}（退出正在运行的主程序后重试）",
                target.display()
            )));
        }
        return Err(InstallError::Io(e));
    }
    if !same_content(target, bytes) {
        // rename 已成功 ⇒ temp 已不存在，此刻的半成品就是 target：删掉它，
        // 免得安装中止后「已安装检测」把一份截断文件当成可用主程序报出来。
        // 续篇 671：清理失败也不能吞 —— 截断残留会被 installed_detected 判为已装。
        if let Err(e) = fs::remove_file(target) {
            return Err(InstallError::Io(std::io::Error::other(format!(
                "校验失败且清理半成品失败（{}）：{e}",
                target.display()
            ))));
        }
        return Err(InstallError::FileCorrupted);
    }
    Ok(())
}

/// 释放主程序到安装位置
pub fn release_main_program(app: &AppHandle) -> Result<PathBuf, InstallError> {
    let install_dir = ensure_install_dir()?;
    let bytes = get_main_program_bytes(app)?;
    let target = install_dir.join(c::MAIN_EXE_NAME);

    // 增量更新：逐字节等价才跳过（旧判据只看大小，会把"同尺寸的新版本"判成最新）。
    if same_content(&target, &bytes) {
        log_install("释放主程序", "跳过", Some("已是最新"));
        return Ok(target);
    }

    // 续篇 223：临时名复用 config::unique_temp_path（pid+seq 唯一），不再固定 "<exe>.tmp"。
    atomic_replace(&crate::config::unique_temp_path(&target), &target, &bytes)?;
    log_install("释放主程序", "成功", None);
    Ok(target)
}

/// 释放驱动到安装位置
pub fn release_driver_to_install(app: &AppHandle) -> Result<PathBuf, InstallError> {
    let install_dir = ensure_install_dir()?;
    let bytes = get_driver_bytes(app)?;
    let target = install_dir.join(c::DRIVER_FILE_NAME);

    if same_content(&target, &bytes) {
        log_install("释放驱动文件", "跳过", Some("已是最新"));
        return Ok(target);
    }

    atomic_replace(&crate::config::unique_temp_path(&target), &target, &bytes)?;
    log_install("释放驱动文件", "成功", None);
    Ok(target)
}

/// 获取安装目录：%LOCALAPPDATA%\AAXSysPulse\
pub fn get_install_dir() -> Result<PathBuf, InstallError> {
    dirs::data_local_dir()
        .map(|p| p.join(c::INSTALL_DIR_NAME))
        .ok_or(InstallError::NoLocalAppData)
}

/// 日志目录唯一出处（续篇 672）：`%LOCALAPPDATA%\AAXSysPulse\logs`。
/// 此前 "logs" 在 ensure_install_dir / log_install / lib.rs::guide_log_dir 各拼一份。
pub fn logs_dir() -> Result<PathBuf, InstallError> {
    Ok(get_install_dir()?.join("logs"))
}

/// 确保安装目录存在，返回安装目录路径
pub fn ensure_install_dir() -> Result<PathBuf, InstallError> {
    let dir = get_install_dir()?;
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }
    let logs = logs_dir()?;
    if !logs.exists() {
        fs::create_dir_all(&logs)?;
    }
    Ok(dir)
}

/// 获取主程序完整路径
pub fn get_main_program_path() -> Result<PathBuf, InstallError> {
    Ok(get_install_dir()?.join(c::MAIN_EXE_NAME))
}

/// 获取系统驱动目标路径
pub fn get_system_driver_path() -> PathBuf {
    Path::new(c::DRIVER_DIR).join(c::DRIVER_FILE_NAME)
}

/// 卸载注册的布尔开关（首轮 226：裸值提为具名常量并核对规格）。
/// 规格：NoModify=1 关掉「更改」按钮；NoRepair=0 保留「修复」入口
/// （修复 = 重跑引导安装，行为成立），与产品语义一致。
const UNINSTALL_NO_MODIFY: u32 = 1;
const UNINSTALL_NO_REPAIR: u32 = 0;

/// 写入卸载注册（第七部分）：
/// HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\AAXSysPulse
/// RegCreateKeyExW + RegSetValueExW；值按文档规格。
pub fn write_uninstall_registry() -> Result<(), InstallError> {
    #[cfg(windows)]
    {
        use std::iter::once;
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
            KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
        };

        // 宽字符转换统一走契约 `wide_of`（首轮 006 / 续篇 673：删掉本函数内
        // 又一份私有 w() 副本）。
        // UTF-16 缓冲区须在对应 API 调用期间存活，PCWSTR 仅存指针
        fn wbytes(v: &[u16]) -> &[u8] {
            unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2) }
        }

        let install_dir = get_install_dir()?;
        // 续篇 674：InstallLocation 用 OsStr::encode_wide 直写 REG_SZ ——
        // to_string_lossy() 会把非 UTF-8 用户名变 U+FFFD 写进注册表。
        let install_dir_w: Vec<u16> = install_dir
            .as_os_str()
            .encode_wide()
            .chain(once(0))
            .collect();
        // 首轮 193：run_install 只释放 MAIN_EXE_NAME 与驱动文件 —— 卸载入口必须
        // 指向真实存在的引导 exe，并用 Path::join 拼接（不再手拼 "dir\\Guide.exe"）。
        let guide_exe = install_dir.join(c::GUIDE_EXE_NAME);
        let uninstall_value = format!("\"{}\" --uninstall", guide_exe.display());
        let display_icon_value = format!("\"{}\"", install_dir.join(c::MAIN_EXE_NAME).display());
        let sub_key = c::wide_of(c::UNINSTALL_SUBKEY);

        let str_values: Vec<(&str, Vec<u16>)> = vec![
            ("DisplayName", c::wide_of(c::APP_TECH_NAME)),
            ("DisplayVersion", c::wide_of(crate::MAIN_PROGRAM_VERSION)),
            ("Publisher", c::wide_of(c::APP_TECH_NAME)),
            ("InstallLocation", install_dir_w),
            ("UninstallString", c::wide_of(&uninstall_value)),
            ("DisplayIcon", c::wide_of(&display_icon_value)),
        ];

        unsafe {
            let mut hkey = HKEY::default();
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(sub_key.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut hkey,
                None,
            )
            .ok()
            // 首轮 225：e.code().0 是 HRESULT，塞给 from_raw_os_error（期待 Win32 码）
            // 会得到错误码错位 —— 统一转字符串 Err。
            .map_err(|e| std::io::Error::other(format!("RegCreateKeyExW 失败：{e}")))?;

            let mut first_err = None;
            for (name, value) in &str_values {
                let n = c::wide_of(name);
                if let Err(e) =
                    RegSetValueExW(hkey, PCWSTR(n.as_ptr()), None, REG_SZ, Some(wbytes(value)))
                        .ok()
                        .map_err(|e| std::io::Error::other(format!("RegSetValueExW({name}) 失败：{e}")))
                {
                    first_err.get_or_insert(e);
                }
            }
            for (name, value) in [("NoModify", UNINSTALL_NO_MODIFY), ("NoRepair", UNINSTALL_NO_REPAIR)] {
                let n = c::wide_of(name);
                let data = value.to_le_bytes();
                if let Err(e) =
                    RegSetValueExW(hkey, PCWSTR(n.as_ptr()), None, REG_DWORD, Some(&data))
                        .ok()
                        .map_err(|e| std::io::Error::other(format!("RegSetValueExW({name}) 失败：{e}")))
                {
                    first_err.get_or_insert(e);
                }
            }
            let _ = RegCloseKey(hkey);
            if let Some(e) = first_err {
                return Err(InstallError::Io(e));
            }
        }
    }
    Ok(())
}

// ============ 第四部分：安装步骤与进度映射 ============

/// 第五部分：追加一条安装日志到 %LOCALAPPDATA%\AAXSysPulse\logs\install.log
/// 格式 `[YYYY-MM-DD HH:MM:SS] 步骤 | 结果`；带 error 时结果写作 `结果（error）`。
/// 追加模式、不覆盖旧日志；任何写入失败静默忽略（绝不 panic）。
///
/// 三件本函数顺带负责的事（审查项 7.3.2 / 7.3.3 / 7.3.4）：
/// 1. **步骤计数**：`result` 命中 成功/跳过/失败 时累加，会话结束时汇成总结行；
///    所有步骤都走这一个出口 ⇒ 不必改 34 处调用点，计数天然与日志一致。
/// 2. **脱敏**：Win32 错误串里常带完整路径（`C:\Users\<用户名>\...`），
///    写盘前统一替换为 `%USERPROFILE%`（日志常被用户原样贴到反馈里）。
/// 3. **体积上限**：单文件超过 [`INSTALL_LOG_CAP_BYTES`] 时轮转为 `install.old.log`。
pub fn log_install(step: &str, result: &str, error: Option<&str>) {
    match result {
        "成功" => { STEP_OK.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
        "跳过" => { STEP_SKIP.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
        "失败" => { STEP_FAIL.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
        _ => {}
    }
    if let Err(e) = (|| -> std::io::Result<()> {
        use std::io::Write;
        // 续篇 672：日志目录唯一出处 = logs_dir()，不再各自 join("logs")。
        let log_dir = match logs_dir() {
            Ok(d) => d,
            Err(e) => return Err(std::io::Error::other(e.to_string())),
        };
        fs::create_dir_all(&log_dir)?;
        let detail = match error {
            Some(e) => format!("{result}（{e}）"),
            None => result.to_string(),
        };
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let path = log_dir.join("install.log");
        rotate_install_log(&path);
        let line = format!("[{ts}] {} | {}\n", redact(step), redact(&detail));
        let mut f = fs::OpenOptions::new().create(true).append(true).open(&path)?;
        f.write_all(line.as_bytes())
    })() {
        // 首轮 227：写盘失败全部被吞 —— 至少往 stderr 打一条（GUI 子系统下用户
        // 看不见，但调试运行/CI 可见）。绝不能再调 log_install 造成递归。
        eprintln!("[install.log] 写入失败：{e}");
    }
}

/// 本会话步骤计数（7.3.4 总结行依据）。由 [`log_install`] 按 `result` 归类别，
/// 因此与日志逐行天然一致；`begin_install_session` 负责清零。
static STEP_OK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static STEP_SKIP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static STEP_FAIL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 会话开始：清零计数并写一条分隔行（7.3.4 —— 旧日志里看不出"哪次装到哪儿"）。
fn begin_install_session(label: &str, mode: &str) {
    use std::sync::atomic::Ordering::Relaxed;
    STEP_OK.store(0, Relaxed);
    STEP_SKIP.store(0, Relaxed);
    STEP_FAIL.store(0, Relaxed);
    log_install(
        &format!("──────── {label} ────────"),
        &format!("mode={mode} version={}", crate::MAIN_PROGRAM_VERSION),
        None,
    );
}

/// 会话结束：写总结行（含耗时与三类计数），把"这次安装到底做成了什么"
/// 固定成一行可直接 grep 的结论。
fn end_install_session(label: &str, secs: f64) {
    use std::sync::atomic::Ordering::Relaxed;
    let (ok, skip, fail) = (
        STEP_OK.load(Relaxed),
        STEP_SKIP.load(Relaxed),
        STEP_FAIL.load(Relaxed),
    );
    log_install(
        &format!("──────── {label} ────────"),
        &format!("成功 {ok} ｜ 跳过 {skip} ｜ 失败 {fail} ｜ 耗时 {secs:.1} 秒"),
        None,
    );
}

/// 安装日志单文件上限：超过就轮转为 `install.old.log`（只留一代）。
///
/// `install.log` 原先**既不按天切也没有上限** —— 每次装卸写十几行，
/// 用得越久越难打开，`guide_view_logs` 读它也更慢。
const INSTALL_LOG_CAP_BYTES: u64 = 2 * 1024 * 1024;

fn rotate_install_log(path: &Path) {
    let Ok(md) = fs::metadata(path) else { return };
    if md.len() < INSTALL_LOG_CAP_BYTES {
        return;
    }
    // rename 覆盖同名 .old（Windows 上 std::fs::rename 可替换已存在目标）。
    let old = path.with_file_name("install.old.log");
    let _ = fs::rename(path, &old);
}

/// 把日志文本里的用户目录换成 `%USERPROFILE%`（7.3.3）。
///
/// 只处理"能确定归因"的那一段前缀，不改写其余文本 —— 排障时路径本身是有用的，
/// 需要去掉的只是**用户名**。
fn redact(s: &str) -> String {
    // 续篇 675：脱敏完全依赖 USERPROFILE —— 取不到时原样返回，
    // "写盘前统一替换"承诺落空。回退 dirs::home_dir()。
    let profile = std::env::var("USERPROFILE")
        .ok()
        .filter(|p| !p.is_empty())
        .or_else(|| dirs::home_dir().map(|p| p.to_string_lossy().to_string()))
        .unwrap_or_default();
    redact_with(s, &profile)
}

/// 纯字符串部分，便于单测（不读环境变量）。
fn redact_with(s: &str, profile: &str) -> String {
    if profile.is_empty() {
        return s.to_string();
    }
    // Windows 路径大小写不敏感：错误串里出现 `c:\users\vos_user\...` 也要能替掉。
    let hay = s.to_ascii_lowercase();
    let needle = profile.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut base = 0usize;
    let mut from = 0usize;
    while let Some(rel) = hay[from..].find(&needle) {
        let at = from + rel;
        out.push_str(&s[base..at]);
        out.push_str("%USERPROFILE%");
        base = at + needle.len();
        from = base;
    }
    out.push_str(&s[base..]);
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallProgress {
    /// 单调递增序号（首轮 229）：前端据此拒绝迟到/重发事件回退进度
    pub seq: u32,
    pub pct: u32,
    pub text: String,
}

// ─── 步骤心跳（审查项 P2-8 / 3.1.5 / 8.45）──────────────────
//
// 旧行为：只在**步骤边界** emit 一次 ⇒ 慢步骤（释放主程序、`stop_old_service`
// 最多轮询 5 s、`OpenSCManager` 挂起）期间进度条与文字**完全静止**；而前端
// `INVOKE_TIMEOUT_MS = 0`（不设上限）⇒ `state.install.status` 永驻 `running`。
// 于是"在慢慢做"和"已经挂死"在界面上长得一模一样，用户只能强杀 → 留下装到一半的状态。
//
// 心跳线程每 500 ms 重发**当前步骤**，文本追加"（已用 N 秒）"：pct 不动而文字在动，
// 就是"这一步还活着"的可观察证据。线程由 [`Heartbeat`] 的 `Drop` 收尾（安装 panic
// 也一样停 —— `[profile.release] panic = "unwind"`，栈展开必定执行 Drop）。
static HEARTBEAT_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 当前步骤锚点：`(seq, pct, 基准文本, 进入时刻)`，心跳线程每 tick 读它。
static STEP_ANCHOR: std::sync::Mutex<Option<(u32, u32, String, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// 心跳线程的会话句柄：Drop 即停（不可复制，保证同一时刻只有一个在跑）。
struct Heartbeat;

impl Heartbeat {
    /// 启动心跳。已有心跳在跑 ⇒ 返回 `None`（不抢占：让先启动的那个继续负责收尾）。
    fn start(app: &AppHandle) -> Option<Self> {
        if HEARTBEAT_ON.swap(true, std::sync::atomic::Ordering::Relaxed) {
            // 首轮 228：命中后**直接返回** —— 旧实现 `store(false)` 反而把正在跑的
            // 心跳关掉了，与注释"不抢占"正好相反。
            return None;
        }
        let app = app.clone();
        let spawned = std::thread::Builder::new()
            .name("install-heartbeat".into())
            .spawn(move || {
                while HEARTBEAT_ON.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let Some((seq, pct, text, started)) = Self::anchor() else {
                        continue;
                    };
                    // 100% 是终止步：不再追发，免得"安装完成"之后又冒出一行"已用 N 秒"。
                    if pct >= 100 {
                        continue;
                    }
                    let secs = started.elapsed().as_secs();
                    emit_traced(
                        &app,
                        "install-progress",
                        InstallProgress { seq, pct, text: format!("{text}（已用 {secs} 秒）") },
                    );
                }
            });
        if spawned.is_err() {
            HEARTBEAT_ON.store(false, std::sync::atomic::Ordering::Relaxed);
            return None;
        }
        Some(Self)
    }

    fn anchor() -> Option<(u32, u32, String, std::time::Instant)> {
        // 中毒也要读：中毒只可能来自上一次写锚点时的 panic，心跳不该成为二次故障源。
        STEP_ANCHOR
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        HEARTBEAT_ON.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// 已留过痕的事件名（用 `Vec` 而非 `HashSet`：静态初始化必须是 const 构造，
/// `HashSet::new` 尚未 const 化，而这里最多三五个元素）。
static EMIT_WARNED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// 前端事件推送 + **失败留痕**（审查项 7.2.3 定向 `let _ =` 审查的产物）。
///
/// `app.emit` 原先四处都写成 `let _ = …`：窗口已销毁 / 序列化失败时，安装照跑完，
/// 前端进度条永远停在原地，而 `install.log` 一行线索都没有 —— 这正是最贵的一类
/// 静默失败。现在失败写进安装日志，但**每个事件名只记第一次**：
/// `install-progress` 由心跳线程每 500 ms 发一次，全记会把整份日志刷废
/// （与批次 6 给 `probe::degrade` 定的"一次降级只留一次痕"同一口径）。
pub fn emit_traced(app: &AppHandle, event: &str, payload: impl Serialize + Clone) {
    if let Err(e) = app.emit(event, payload) {
        let first = {
            let mut g = EMIT_WARNED
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if g.iter().any(|seen| seen == event) {
                false
            } else {
                g.push(event.to_string());
                true
            }
        };
        if first {
            // 续篇 676：emit 失败不是安装步骤 —— 不能记"失败"（会被 STEP_FAIL
            // 计数，安装成功却总结"失败 1"）。用中性 result 走同一日志出口但不计步数。
            log_install(
                &format!("事件推送 {event}"),
                "不可达",
                Some(&format!("{e}（该事件后续失败不再重复记录）")),
            );
        }
    }
}

/// 进度事件单调序号（首轮 229）：心跳线程重发与迟到 emit 曾让 100% 之后仍可能
/// 收到旧 pct 造成进度回退 —— payload 带 seq，前端只接受更大值。
static PROGRESS_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn emit_progress(app: &AppHandle, pct: u32, text: &str) {
    let seq = PROGRESS_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    {
        let mut g = STEP_ANCHOR.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some((seq, pct, text.to_string(), std::time::Instant::now()));
    }
    emit_traced(app, "install-progress", InstallProgress { seq, pct, text: text.to_string() });
}

/// 跳过类步骤统一记账（首轮 230）：Err 一律记**失败** —— 旧实现把 Err 也记成
/// "跳过"，总结行"失败 0"谎报成功。失败只记账并继续推进，不中断安装。
fn skip_step<T>(step: &str, r: Result<T, impl std::fmt::Display>) {
    match r {
        Ok(_) => log_install(step, "成功", None),
        Err(e) => log_install(step, "失败", Some(&e.to_string())),
    }
}

/// 返回 bool 的跳过类步骤：Ok(false) 表示前置条件不存在（如服务不存在），记为跳过
fn skip_step_bool(step: &str, r: Result<bool, impl std::fmt::Display>) {
    match r {
        Ok(true) => log_install(step, "成功", None),
        Ok(false) => log_install(step, "跳过", Some("服务不存在")),
        Err(e) => log_install(step, "失败", Some(&e.to_string())),
    }
}

/// 安装结果：驱动部分是否可用（第八部分分支依据）
pub struct InstallOutcome {
    pub driver_ok: bool,
}

/// 安装模式（第四 / 八 / 九部分共用同一进度引擎）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    Full,
    DriverOnly,
    Repair,
}

impl InstallMode {
    /// 前端桥传字符串：'full' | 'driver-only' | 'repair'。
    /// 续篇 677：未识别值不再静默回落 Full（拼错 driver_only 曾触发完整重装）——
    /// 返回 Err 由命令层白名单校验。
    pub fn from_arg(s: &str) -> Result<Self, String> {
        match s {
            "full" => Ok(Self::Full),
            "driver-only" => Ok(Self::DriverOnly),
            "repair" => Ok(Self::Repair),
            other => Err(format!("未知安装模式：{other}")),
        }
    }

    /// 日志里的模式标记（与 `from_arg` 同一套拼写，便于对照前端传的什么）。
    fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::DriverOnly => "driver-only",
            Self::Repair => "repair",
        }
    }
}

/// 按进度表顺序执行。`mode`：
/// · Full —— 完整安装（5%~100% 全表）
/// · DriverOnly —— 只重跑 25%~75% 驱动段（第八部分「重试」入口）
/// · Repair —— 修复安装：重放主程序 + 驱动 + 快捷方式，**不动**配置与卸载注册（第九部分）
/// 仅中止类失败（5% 创建目录 / 15% 释放主程序）返回 Err。
pub fn run_install(app: &AppHandle, mode: InstallMode) -> Result<InstallOutcome, InstallError> {
    let started = std::time::Instant::now();
    let head = !matches!(mode, InstallMode::DriverOnly);
    let registry = matches!(mode, InstallMode::Full);
    begin_install_session(
        match mode {
            InstallMode::Full => "安装开始",
            InstallMode::Repair => "修复安装开始",
            InstallMode::DriverOnly => "驱动重试开始",
        },
        mode.as_str(),
    );
    // 心跳守卫：随本函数作用域存活，任何 return / panic 路径都会 Drop 停线程。
    // 续篇 678：start 返回 None（已有心跳 / spawn 失败）曾直接被忽略 —— 安装继续
    // 但进度条静止且零留痕；这里记一条不可达日志（不计步数）。
    let _heartbeat: Option<Heartbeat> = match Heartbeat::start(app) {
        Some(h) => Some(h),
        None => {
            log_install(
                "安装进度心跳",
                "不可达",
                Some("心跳线程未启动（已有心跳或 spawn 失败），进度条可能静止"),
            );
            None
        }
    };

    if head {
        // 5% 创建安装目录（中止）
        emit_progress(app, 5, "创建安装目录");
        if let Err(e) = ensure_install_dir() {
            log_install("创建安装目录", "失败", Some(&e.to_string()));
            return Err(e);
        }
        log_install("创建安装目录", "成功", None);

        // 15% 释放主程序（中止：一次性写入 + 原子替换 + 大小校验，内部含增量跳过）
        // 成功/跳过日志由 release_main_program 内部记录，此处只记失败
        emit_progress(app, 15, "释放主程序");
        if let Err(e) = release_main_program(app) {
            log_install("释放主程序", "失败", Some(&e.to_string()));
            return Err(e);
        }
    }

    // 25% 释放驱动文件（跳过；成功/跳过日志由函数内部记录）
    emit_progress(app, 25, "释放驱动文件");
    if let Err(e) = release_driver_to_install(app) {
        log_install("释放驱动文件", "跳过", Some(&e.to_string()));
    }

    // 35% 停止旧服务（跳过）
    emit_progress(app, 35, "停止旧服务");
    skip_step_bool("停止旧服务", crate::driver::stop_old_service());

    // 45% 删除旧服务（跳过：删除后轮询等待消失）
    emit_progress(app, 45, "删除旧服务");
    skip_step_bool("删除旧服务", crate::driver::delete_old_service());

    // 55% 释放驱动到系统目录（续篇 232：资源读取失败单独记**失败**并带资源名，
    // 不再包成 io::Error 后交给 skip_step 记"跳过"——Resource 变体的文案自带名字）
    emit_progress(app, 55, "释放驱动到系统目录");
    skip_step("释放驱动到系统目录", (|| -> Result<(), String> {
        let data = get_driver_bytes(app).map_err(|e| e.to_string())?;
        crate::driver::release_driver_sys(&data).map_err(|e| e.to_string())
    })());

    // 65% 注册系统服务（跳过）
    emit_progress(app, 65, "注册系统服务");
    skip_step("注册系统服务", crate::driver::register_driver_service());

    // 75% 启动服务（跳过）
    emit_progress(app, 75, "启动服务");
    skip_step("启动服务", crate::driver::start_driver_service());

    if head {
        // 85% 创建桌面快捷方式（跳过）
        emit_progress(app, 85, "创建桌面快捷方式");
        skip_step("创建桌面快捷方式", crate::shortcut::create_desktop_shortcut().map(|_| ()));

        // 90% 创建开始菜单快捷方式（跳过）
        emit_progress(app, 90, "创建开始菜单快捷方式");
        skip_step("创建开始菜单快捷方式", crate::shortcut::create_start_menu_shortcut().map(|_| ()));

        // 95% 写入卸载注册（跳过）——修复安装不动注册
        if registry {
            emit_progress(app, 95, "写入卸载注册");
            skip_step("写入卸载注册", write_uninstall_registry());
        }
    }

    // 100% 完成
    emit_progress(app, 100, "安装完成");
    // 首轮 231：结束标签与 begin 的"驱动重试/修复安装开始"配对，不再一律写"安装结束"
    end_install_session(
        match mode {
            InstallMode::Full => "安装结束",
            InstallMode::Repair => "修复安装结束",
            InstallMode::DriverOnly => "驱动重试结束",
        },
        started.elapsed().as_secs_f64(),
    );

    // 驱动是否可用以实际服务状态为准（第八部分「检测」而非凭标记）。
    // 续篇 679：探测 Err 曾被 unwrap_or(false) 静默当"驱动不可用"，触发误导性
    // 的重试/跳过弹窗 —— 失败单独留痕。
    let driver_ok = match crate::driver::get_driver_status() {
        Ok(s) => s.installed && s.running,
        Err(e) => {
            log_install("驱动状态检测", "失败", Some(&e.to_string()));
            false
        }
    };
    Ok(InstallOutcome { driver_ok })
}

// ============ 第九部分：卸载 ============

/// 卸载单步状态（续篇 680：裸 `&'static str` 拼错（"OK"）能编译通过且前端原样
/// 显示 —— 改枚举 + serde rename，JSON 里仍是 `ok`/`skip`/`fail` 字符串）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StepStatus {
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "skip")]
    Skip,
    #[serde(rename = "fail")]
    Fail,
}

/// 卸载单步结果。`status`：`ok` 做完 ｜ `skip` 无需做/本就不存在 ｜ `fail` 想做但没做成。
///
/// 以前 `run_uninstall` 返回 `()`，前端那句"部分卸载步骤未完成"永远进不去（报告 P0-9/8B.H）。
/// 现在每步都进这个结构，由前端如实列清单。
#[derive(Debug, Clone, Serialize)]
pub struct StepResult {
    pub step: String,
    pub status: StepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 边执行边记账：写安装日志 + 收集 `StepResult`。
struct UninstallReport {
    steps: Vec<StepResult>,
}

impl UninstallReport {
    fn new() -> Self {
        Self { steps: Vec::new() }
    }

    fn record(&mut self, step: &str, status: StepStatus, detail: Option<String>) {
        let label = match status {
            StepStatus::Ok => "成功",
            StepStatus::Skip => "跳过",
            StepStatus::Fail => "失败",
        };
        log_install(step, label, detail.as_deref());
        self.steps.push(StepResult {
            step: step.to_string(),
            status,
            detail,
        });
    }

    fn ok(&mut self, step: &str) {
        self.record(step, StepStatus::Ok, None);
    }

    fn ok_with(&mut self, step: &str, detail: impl std::fmt::Display) {
        self.record(step, StepStatus::Ok, Some(detail.to_string()));
    }

    fn skip(&mut self, step: &str, why: impl std::fmt::Display) {
        self.record(step, StepStatus::Skip, Some(why.to_string()));
    }

    fn fail(&mut self, step: &str, why: impl std::fmt::Display) {
        self.record(step, StepStatus::Fail, Some(why.to_string()));
    }
}

/// 删除计划任务：`Ok(true)`=已删除、`Ok(false)`=本来就没有、`Err`=想删但没删成。
///
/// **先 `/Query` 再 `/Delete`**。续篇 233：`/Query` 的任何非 0 退出码曾被一律当
/// "任务不存在"（拒绝访问也被当成跳过，与注释矛盾）—— 现按输出内容区分：
/// 只有命中"找不到文件"语义的输出才视为不存在，其余按失败上报。
/// 用绝对路径而不裸写 `schtasks`：不依赖 PATH（主程序 `autostart.rs:229` 同做法）。
#[cfg(windows)]
fn delete_scheduled_task(name: &str) -> Result<bool, String> {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let exe = format!("{root}\\System32\\schtasks.exe");
    let query = std::process::Command::new(&exe)
        .args(["/Query", "/TN", name])
        .output()
        .map_err(|e| format!("schtasks 不可用：{e}"))?;
    if !query.status.success() {
        // schtasks 不区分错误码（"任务不存在"与"拒绝访问"都是非 0），
        // 只能按输出文本识别"不存在"语义（中英文各覆盖一种常见形态）。
        let out = format!(
            "{}{}",
            String::from_utf8_lossy(&query.stdout),
            String::from_utf8_lossy(&query.stderr)
        )
        .to_ascii_lowercase();
        if out.contains("find the file") || out.contains("找不到") {
            return Ok(false);
        }
        return Err(format!(
            "无法确认任务是否存在（退出码 {:?}）：{}",
            query.status.code(),
            out.trim()
        ));
    }
    let del = std::process::Command::new(&exe)
        .args(["/Delete", "/TN", name, "/F"])
        .output()
        .map_err(|e| format!("schtasks 不可用：{e}"))?;
    if del.status.success() {
        return Ok(true);
    }
    Err(format!(
        "拒绝访问或权限不足（退出码 {:?}）：{}",
        del.status.code(),
        String::from_utf8_lossy(&del.stderr).trim()
    ))
}

/// 卸载：关闭主程序 → 停止/删除服务 → 清理驱动与计划任务 → 删除 Run 键
/// → 删除快捷方式 → 删除安装目录 → **最后**删除卸载注册。
/// 每步尽力执行、失败只记账并继续（绝不中断后续步骤）；`keep_config` 时保留
/// 安装目录下的 config\ 子目录。返回每一步的真实结果。
pub fn run_uninstall(keep_config: bool) -> Vec<StepResult> {
    let started = std::time::Instant::now();
    begin_install_session("卸载开始", &format!("keep_config={keep_config}"));
    let mut r = UninstallReport::new();

    // 1) 关闭主程序：WM_CLOSE 正常退出（见 process::request_main_close 的说明）
    if crate::process::is_main_process_running() {
        if !crate::process::request_main_close() {
            r.fail("关闭主程序", "无法向主程序窗口投递关闭消息");
        } else if crate::process::wait_main_exit(std::time::Duration::from_secs(3)) {
            r.ok("关闭主程序");
        } else {
            r.fail("关闭主程序", "主程序 3 秒内未退出，后续文件可能仍被占用");
        }
    } else {
        r.skip("关闭主程序", "主程序未运行");
    }

    // 2) 停止服务 / 删除服务（delete_old_service 内部会删 C:\Windows\AAXSysPulse\*.sys 并轮询消失）
    match crate::driver::stop_old_service() {
        Ok(true) => r.ok("停止服务"),
        Ok(false) => r.skip("停止服务", "服务不存在"),
        Err(e) => r.fail("停止服务", e),
    }
    match crate::driver::delete_old_service() {
        Ok(true) => r.ok("删除服务"),
        Ok(false) => r.skip("删除服务", "服务不存在"),
        Err(e) => r.fail("删除服务", e),
    }

    // 3) 删除驱动残留目录。续篇 234：只 `remove_dir` 对非空目录必然失败且残留的
    //    .sys 无人清理 —— 先逐个删文件/子目录，再尝试删目录本身。
    match get_system_driver_path().parent() {
        Some(dir) => {
            if !dir.exists() {
                r.skip("删除驱动目录", "目录不存在");
            } else {
                let mut left: Vec<String> = Vec::new();
                match fs::read_dir(dir) {
                    Ok(entries) => {
                        for entry in entries {
                            let Ok(e) = entry else {
                                left.push(format!("{}（枚举失败）", dir.display()));
                                continue;
                            };
                            let p = e.path();
                            let outcome =
                                if p.is_dir() { fs::remove_dir_all(&p) } else { fs::remove_file(&p) };
                            if outcome.is_err() && p.exists() {
                                left.push(format!("{}（残留）", p.display()));
                            }
                        }
                    }
                    Err(e) => left.push(format!("{}（目录不可枚举：{e}）", dir.display())),
                }
                match fs::remove_dir(dir) {
                    Ok(()) => {
                        if left.is_empty() {
                            r.ok("删除驱动目录");
                        } else {
                            r.fail("删除驱动目录", format!("目录已删但部分内容残留：{}", left.join("；")));
                        }
                    }
                    Err(e) => r.fail("删除驱动目录", format!("{}：{}", dir.display(), e)),
                }
            }
        }
        None => r.skip("删除驱动目录", "驱动路径无父目录"),
    }

    // 4) 删除计划任务。任务名 = 契约常量（审查项 3.1.14 / P1-22），与主程序
    //    `win/src/autostart.rs` 创建的 `TASK_NAME`（**含空格**）同源 —— 旧代码删的是
    //    `"AAXSysPulse"`，于是那个 `/SC ONLOGON /RL HIGHEST` 的登录触发任务
    //    **永远删不掉**（P0-9 的"两处名字写错"之一）；顺带旧代码用裸 `schtasks`
    //    依赖 PATH，未提权时连进程都起不来却被记成"跳过：无同名计划任务"。
    //    `TASK_NAME_LEGACY` 是历史版本用过的无空格名，留着清孤儿任务。
    #[cfg(windows)]
    {
        let mut deleted: Vec<&str> = Vec::new();
        let mut errs: Vec<String> = Vec::new();
        for name in [c::TASK_NAME, c::TASK_NAME_LEGACY] {
            match delete_scheduled_task(name) {
                Ok(true) => deleted.push(name),
                Ok(false) => {}
                Err(e) => errs.push(format!("{name}：{e}")),
            }
        }
        if !errs.is_empty() {
            r.fail("删除计划任务", errs.join("；"));
        } else if !deleted.is_empty() {
            r.ok("删除计划任务");
        } else {
            r.skip("删除计划任务", "无同名计划任务");
        }
    }
    #[cfg(not(windows))]
    r.skip("删除计划任务", "非 Windows 平台");

    // 5) 删除 Run 键自启项：**两条都要删**（值名同为契约常量，3.1.14 / P1-22）。
    //    `RUN_VALUE_GUIDE` 是引导自己写的（`lib.rs`），
    //    `RUN_VALUE_MAIN` 是主程序写的（`win/src/autostart.rs`，含空格）。
    //    旧代码只删前者，注释还写着"与 autostart_set 写入的键名一致" —— 主程序那条
    //    自启项因此永久残留（P0-9 的"两处名字写错"之二）。
    #[cfg(windows)]
    {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
        use windows::Win32::System::Registry::{
            RegCloseKey, RegDeleteValueW, RegOpenKeyExW, HKEY,
            HKEY_CURRENT_USER, KEY_SET_VALUE,
        };
        let mut h = HKEY::default();
        let run_key = c::wide_of(c::RUN_KEY);
        if unsafe {
            RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(run_key.as_ptr()), None, KEY_SET_VALUE, &mut h)
                .ok()
        }
        .is_ok()
        {
            let mut removed: Vec<&str> = Vec::new();
            let mut errs: Vec<String> = Vec::new();
            for name in [c::RUN_VALUE_GUIDE, c::RUN_VALUE_MAIN] {
                let wide = c::wide_of(name);
                let code = unsafe { RegDeleteValueW(h, PCWSTR(wide.as_ptr())) };
                if code.is_ok() {
                    removed.push(name);
                } else if code != ERROR_FILE_NOT_FOUND {
                    errs.push(format!("{name}：{code:?}"));
                }
            }
            unsafe {
                let _ = RegCloseKey(h);
            }
            if !errs.is_empty() {
                r.fail("删除 Run 键", format!("自启项可能仍在：{}", errs.join("；")));
            } else if !removed.is_empty() {
                r.ok("删除 Run 键");
            } else {
                r.skip("删除 Run 键", "未设置开机自启");
            }
        } else {
            // 续篇 681：权限不足打不开 Run 键 ≠ "无需处理" —— 记失败，
            // 清单里如实显示自启残留，而不是谎报跳过。
            r.fail("删除 Run 键", "Run 键不可打开（权限不足？），自启项可能残留");
        }
    }
    #[cfg(not(windows))]
    r.skip("删除 Run 键", "非 Windows 平台");

    // 6) 删除快捷方式：区分"本来就没有"与"删不掉"
    #[cfg(windows)]
    {
        let mut targets: Vec<PathBuf> = Vec::new();
        if let Some(desktop) = dirs::desktop_dir() {
            targets.push(desktop.join(c::SHORTCUT_FILE_NAME));
        }
        if let Some(cfg) = dirs::config_dir() {
            targets.push(cfg.join(c::START_MENU_FOLDER));
        }
        let mut removed = 0usize;
        let mut left: Vec<String> = Vec::new();
        for p in targets {
            let outcome = if p.is_dir() {
                // 续篇 682：开始菜单文件夹里可能有用户/其他软件的快捷方式 ——
                // 只删我们自己的 .lnk，再尝试删空目录（非空会失败并如实记录）。
                let lnk = p.join(c::SHORTCUT_FILE_NAME);
                let _ = fs::remove_file(&lnk);
                fs::remove_dir(&p)
            } else {
                fs::remove_file(&p)
            };
            match outcome {
                Ok(()) => removed += 1,
                Err(_) if !p.exists() => {}
                Err(e) => left.push(format!("{}（{e}）", p.display())),
            }
        }
        if !left.is_empty() {
            r.fail("删除快捷方式", left.join("；"));
        } else if removed > 0 {
            r.ok_with("删除快捷方式", format!("移除 {removed} 处"));
        } else {
            r.skip("删除快捷方式", "未创建过快捷方式");
        }
    }
    #[cfg(not(windows))]
    r.skip("删除快捷方式", "非 Windows 平台");

    // 7) 删除安装目录（keep_config 时保留 config\ 子目录）
    //    残留必须如实报出来：以前"目录还在就算成功"，用户以为卸干净了。
    match get_install_dir() {
        Ok(dir) => {
            let mut left: Vec<String> = Vec::new();
            if keep_config {
                // 续篇 234：entries.flatten() 曾吞掉逐条读取错误 —— 逐条记录原因。
                if let Ok(entries) = fs::read_dir(&dir) {
                    for entry in entries {
                        let Ok(e) = entry else {
                            left.push(format!("{}（枚举失败）", dir.display()));
                            continue;
                        };
                        let p = e.path();
                        if p.file_name().map(|n| n == "config").unwrap_or(false) {
                            continue;
                        }
                        let outcome = if p.is_dir() { fs::remove_dir_all(&p) } else { fs::remove_file(&p) };
                        if outcome.is_err() && p.exists() {
                            left.push(p.to_string_lossy().to_string());
                        }
                    }
                }
                // 只剩 config\ 时目录本身要保留；仍有其它残留则报失败
                if left.is_empty() {
                    r.ok_with("删除安装目录", format!("保留配置于 {}", dir.display()));
                } else {
                    r.fail("删除安装目录", format!("残留：{}", left.join("；")));
                }
            } else {
                match fs::remove_dir_all(&dir) {
                    Ok(()) => r.ok("删除安装目录"),
                    Err(_) if !dir.exists() => r.ok("删除安装目录"),
                    Err(e) => r.fail("删除安装目录", format!("{}（{e}）", dir.display())),
                }
            }
        }
        Err(e) => r.fail("删除安装目录", e),
    }

    // 8) 删除 HKCU 卸载注册项 —— 必须放在最后一步：中途失败时入口还在，
    //    用户能从控制面板重跑卸载，而不是留下一个既卸不掉又找不到的僵尸条目。
    #[cfg(windows)]
    {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
        use windows::Win32::System::Registry::{RegDeleteTreeW, HKEY_CURRENT_USER};
        let sub = c::wide_of(c::UNINSTALL_SUBKEY);
        let code = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr())) };
        if code.is_ok() {
            r.ok("删除卸载注册");
        } else if code == ERROR_FILE_NOT_FOUND {
            r.skip("删除卸载注册", "注册项不存在");
        } else {
            r.fail("删除卸载注册", format!("控制面板条目可能残留：{code:?}"));
        }
    }
    #[cfg(not(windows))]
    r.skip("删除卸载注册", "非 Windows 平台");

    // 总结行自带"成功 n ｜ 跳过 m ｜ 失败 k"，与上面 9 步逐条记账同源
    // （计数在 `log_install` 里按 result 累加，见 7.3.4）。
    end_install_session("卸载结束", started.elapsed().as_secs_f64());
    r.steps
}

// 首轮 006：本文件原有的一份私有 `w()` 已删除 —— 宽字符转换统一走契约
// `aaxsyspulse_core::contract::wide_of()`（调用点已就地替换）。

#[cfg(test)]
mod redact_tests {
    use super::redact_with;

    #[test]
    fn replaces_profile_prefix_case_insensitively() {
        let p = "C:\\Users\\VOS_User";
        assert_eq!(
            redact_with(r"copy to C:\users\vos_user\AppData failed", p),
            r"copy to %USERPROFILE%\AppData failed"
        );
    }

    #[test]
    fn replaces_every_occurrence_and_keeps_the_rest() {
        let p = "C:\\Users\\Bob";
        assert_eq!(
            redact_with(r"C:\Users\Bob\a.txt -> C:\Users\Bob\b.txt", p),
            r"%USERPROFILE%\a.txt -> %USERPROFILE%\b.txt"
        );
        assert_eq!(redact_with("no path here", p), "no path here");
    }

    #[test]
    fn empty_profile_is_a_noop() {
        assert_eq!(redact_with(r"C:\Users\Bob\a", ""), r"C:\Users\Bob\a");
    }
}
