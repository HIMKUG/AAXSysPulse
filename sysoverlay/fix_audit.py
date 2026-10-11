# -*- coding: utf-8 -*-
"""AAXSysPulseC 审计修复：一次性批量替换。

原则（来自项目约定）：
- 每个替换 `assert count == 命中次数`，错位立即失败，绝不静默改错位置；
- 结构性/多点改动集中在一个脚本里一次完成，避免 Edit 连环导致行号漂移；
- 全部替换成功后才落盘（先在内存中完成，最后统一写）。
"""
import io
import os
import re

ROOT = os.path.dirname(os.path.abspath(__file__))


def load(rel):
    p = os.path.join(ROOT, rel)
    with io.open(p, "r", encoding="utf-8", newline="") as f:
        return p, f.read()


def norm(text, src):
    """若源文件是 CRLF，则把模式串也转成 CRLF 以匹配。"""
    if "\r\n" in src:
        return text.replace("\n", "\r\n")
    return text


def edit(state, rel, old, new, count=1, label=""):
    old = norm(old, state[rel])
    new = norm(new, state[rel])
    n = state[rel].count(old)
    assert n == count, "[%s] %s 期望命中 %d 次，实际 %d 次\n--- 模式 ---\n%s" % (
        rel,
        label or "替换",
        count,
        n,
        old[:300],
    )
    state[rel] = state[rel].replace(old, new, count)


def bump_ordering(state, rel, names):
    """把指定原子变量所在行的 Ordering::Relaxed 提升为 SeqCst（可见性保证）。"""
    src = state[rel]
    eol = "\r\n" if "\r\n" in src else "\n"
    out = []
    hits = 0
    for ln in src.split(eol):
        if any(nm in ln for nm in names) and "Ordering::Relaxed" in ln:
            ln = ln.replace("Ordering::Relaxed", "Ordering::SeqCst")
            hits += 1
        out.append(ln)
    assert hits > 0, "[%s] 未找到 %s 的 Relaxed 行" % (rel, names)
    state[rel] = eol.join(out)
    print("   ordering 提升 %s: %d 行" % (rel, hits))


FILES = [
    "crates/win/src/config_io.rs",
    "crates/core/src/config/ini.rs",
    "crates/win/src/sampler.rs",
    "crates/win/src/gdi.rs",
    "crates/win/src/window.rs",
    "crates/win/src/settings/mod.rs",
    "crates/win/src/settings/cfg_ref.rs",
    "crates/win/src/msr.rs",
    "crates/win/src/lib.rs",
    "crates/app/src/main.rs",
]

state = {}
for rel in FILES:
    _, state[rel] = load(rel)

# ══════════════════════════════════════════════════════════════════════════════
# 1. config_io.rs
# ══════════════════════════════════════════════════════════════════════════════
CI = "crates/win/src/config_io.rs"

# 1.1 BOM 剥离
edit(
    state,
    CI,
    """fn read_if(dir: &std::path::Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name)).ok()
}""",
    """fn read_if(dir: &std::path::Path, name: &str) -> Option<String> {
    // \u26a0\ufe0f 必须剥离 UTF-8 BOM。记事本 / VSCode「另存为 UTF-8 with BOM」会在文件首
    // 写入 U+FEFF，使首个 `[system]` 头变成 "\\u{feff}[system]" → Ini 查不到该段 →
    // **整个文件的键静默回退默认**；且 version 读不到被当成 v1，与其他 v3 文件
    // 组成 span=2 → 触发「版本跨度过大 → 用默认配置覆盖全部配置」的破坏性弹窗。
    // 2026-09-15 审计定位。
    std::fs::read_to_string(dir.join(name))
        .ok()
        .map(|s| s.strip_prefix('\\u{feff}').map(str::to_string).unwrap_or(s))
}""",
    label="read_if BOM",
)

# 1.2 write_bundle 串行化
edit(
    state,
    CI,
    """fn write_bundle(dir: &std::path::Path, files: &[(String, String)]) -> bool {
    let mut all_ok = true;""",
    """fn write_bundle(dir: &std::path::Path, files: &[(String, String)]) -> bool {
    // 串行化：同步 save_config（退出 / 关窗 flush）与异步写线程可能并发，两者共用
    // 固定名 `<name>.tmp` 且写同一批目标文件 → 无锁会交错出损坏内容。
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all_ok = true;""",
    label="write_bundle 加写锁",
)

# 1.3 异步写线程：函数体 static → 模块级（Critical 修复）
edit(
    state,
    CI,
    """/// 异步写线程传输单元。
struct Bundle {
    files: Vec<(String, String)>,
}

/// 异步写线程传输单元。
fn start_writer_thread() {
    static PENDING: std::sync::OnceLock<Arc<Mutex<Option<Bundle>>>> = std::sync::OnceLock::new();
    static SHUTDOWN: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
    static NOTIFY: std::sync::OnceLock<mpsc::Sender<()>> = std::sync::OnceLock::new();

    let pending = PENDING.get_or_init(|| Arc::new(Mutex::new(None)));
    let shutdown = SHUTDOWN.get_or_init(|| Arc::new(AtomicBool::new(false)));

    // 只启动一次
    if NOTIFY.get().is_some() {
        return;
    }

    let (notify_tx, rx) = mpsc::channel();
    let pending_c = pending.clone();
    let shutdown_c = shutdown.clone();

    std::thread::Builder::new()
        .name("config-writer".into())
        .spawn(move || {
            while rx.recv().is_ok() {
                if shutdown_c.load(Ordering::SeqCst) {
                    break;
                }
                let bundle = {
                    let mut lock = pending_c.lock().unwrap();
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
            if let Some(bundle) = pending_c.lock().unwrap().take() {
                if let Some(d) = config_dir() {
                    write_bundle(&d, &bundle.files);
                }
            }
        })
        .expect("config-writer thread spawn failed");

    let _ = NOTIFY.set(notify_tx);
}

/// 异步落盘（UI 线程调用）：
/// 多次快速调用自动合并——写线程只处理最新一次保存。
/// 返回 `true` = 已成功入队。
///
/// \u26a0\ufe0f 进程退出路径（关窗/隐藏 flush）请用 [`save_config`] 同步落盘保数据。
pub fn save_config_async(cfg: &Config) -> bool {
    // 第一次调用时启动写线程
    start_writer_thread();

    static PENDING: std::sync::OnceLock<Arc<Mutex<Option<Bundle>>>> = std::sync::OnceLock::new();
    let pending = PENDING.get_or_init(|| Arc::new(Mutex::new(None)));

    // 覆盖 pending（合并去重）
    *pending.lock().unwrap() = Some(Bundle {
        files: bundle_for(cfg),
    });

    static NOTIFY: std::sync::OnceLock<mpsc::Sender<()>> = std::sync::OnceLock::new();
    if let Some(n) = NOTIFY.get() {
        return n.send(()).is_ok();
    }
    false
}

/// 通知写线程退出（进程结束时调用）。
pub fn shutdown_writer() {
    static SHUTDOWN: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
    static NOTIFY: std::sync::OnceLock<mpsc::Sender<()>> = std::sync::OnceLock::new();

    if let Some(shutdown) = SHUTDOWN.get() {
        shutdown.store(true, Ordering::SeqCst);
    }
    if let Some(n) = NOTIFY.get() {
        let _ = n.send(());
    }
}""",
    """/// 异步写线程传输单元。
struct Bundle {
    files: Vec<(String, String)>,
}

// ══════════════════════════════════════════════════════════════════════════════
// \u26a0\ufe0f 以下三个 State 访问器**必须**是模块级函数里的 `static`。
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
/// \u26a0\ufe0f 进程退出路径（关窗/隐藏 flush）请用 [`save_config`] 同步落盘保数据。
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
}""",
    label="异步写线程状态提升为模块级",
)

# 1.4 导入：备份顺序 + 新建文件回滚
edit(
    state,
    CI,
    """    let mut bak_entries: Vec<(String, std::path::PathBuf)> = Vec::new();
    for (name, _) in &files {
        let target = dir.join(name);
        if target.exists() {
            // 删除上一次成功导入留下的旧备份（不带 .failed 后缀），
            // 但保留 `.bak.failed.<ts>`（上次回滚失败的残留，等用户手动处理）。
            let old_bak = dir.join(format!("{}.bak", name));
            let _ = std::fs::remove_file(&old_bak);

            let bak = dir.join(format!("{}.bak", name));
            if std::fs::rename(&target, &bak).is_ok() {
                bak_entries.push((name.clone(), bak));
            }
        }
    }""",
    """    let mut bak_entries: Vec<(String, std::path::PathBuf)> = Vec::new();
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
            // \u26a0\ufe0f 不得先 `remove_file(old_bak)`（旧实现）：那样一旦 rename 失败，
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
    }""",
    label="导入备份顺序",
)

edit(
    state,
    CI,
    """        let mut rollback_ok = true;
        for (orig_name, bak) in &bak_entries {
            let target = dir.join(orig_name);
            if std::fs::rename(bak, &target).is_err() {
                rollback_ok = false;
                eprintln!("[config_io] 回滚失败：{} 无法恢复", orig_name);
            }
        }
        if !rollback_ok {""",
    """        let mut rollback_ok = true;
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
        if !rollback_ok {""",
    label="导入回滚删除新建文件",
)

edit(
    state,
    CI,
    """        if !rollback_ok {
            // 保留未能回滚的备份：改名 .bak.failed.<ts>，避免被下次导入清除""",
    """        if !rollback_ok {
            eprintln!(
                "[config_io] 未能回滚的备份保留在 {}（.bak.failed.<ts>），请手动恢复",
                dir.display()
            );
            // 保留未能回滚的备份：改名 .bak.failed.<ts>，避免被下次导入清除""",
    label="回滚失败提示目录",
)

# ══════════════════════════════════════════════════════════════════════════════
# 2. ini.rs —— 重复 section 逐键合并
# ══════════════════════════════════════════════════════════════════════════════
edit(
    state,
    "crates/core/src/config/ini.rs",
    """                if let Some(sec) = rest.strip_suffix(']') {
                    let _ = std::mem::replace(
                        sections.entry(std::mem::take(&mut cur)).or_insert_with(HashMap::new),
                        std::mem::take(&mut cur_map),
                    );
                    cur = sec.trim().to_ascii_lowercase();
                    continue;
                }""",
    """                if let Some(sec) = rest.strip_suffix(']') {
                    // 逐 key 合并，而非整段替换：同名 section 第二次出现时（手改配置
                    // 复制粘贴很容易留下两个 [monitor]），旧实现用 mem::replace 把
                    // 前一段**整段丢弃** → 静默丢键。后写覆盖先写才是 INI 惯例。
                    sections
                        .entry(std::mem::take(&mut cur))
                        .or_default()
                        .extend(cur_map.drain());
                    cur = sec.trim().to_ascii_lowercase();
                    continue;
                }""",
    label="重复 section 合并",
)

# ══════════════════════════════════════════════════════════════════════════════
# 3. sampler.rs —— 退避消费 / 磁盘 ready / GPU NaN
# ══════════════════════════════════════════════════════════════════════════════
SA = "crates/win/src/sampler.rs"

edit(
    state,
    SA,
    """        let idx = id as usize;
        if outcome.is_failure() {
            self.backoff[idx].next_delay_ms();
        } else {
            self.backoff[idx].reset();
        }""",
    """        // \u26a0\ufe0f 只有**真正成功**才清零退避。旧实现 `else { reset() }` 会让 Skipped
        // （未到期 / 被退避门挡下）也清零 → 退避永远被自己重置，失败源每轮重试，
        // Backoff 形同虚设（2026-09-15 审计）。
        let idx = id as usize;
        if outcome.is_failure() {
            self.backoff[idx].next_delay_ms();
        } else if matches!(outcome, SampleOutcome::Ok) {
            self.backoff[idx].reset();
        }""",
    label="push_diag 退避清零条件",
)

edit(
    state,
    SA,
    """        // CPU
        if plan.cpu {
            let t = crate::perf::begin();""",
    """        // CPU（退避中则跳过本轮：指数退避此前只被记录、从未被消费）
        if plan.cpu && self.backoff[SamplerId::Cpu as usize].is_backing_off() {
            self.push_diag(SamplerId::Cpu, SampleOutcome::Skipped, 0);
        } else if plan.cpu {
            let t = crate::perf::begin();""",
    label="CPU 退避门",
)

edit(
    state,
    SA,
    """        // Memory
        if plan.mem {
            let t = crate::perf::begin();""",
    """        // Memory（退避门同上）
        if plan.mem && self.backoff[SamplerId::Memory as usize].is_backing_off() {
            self.push_diag(SamplerId::Memory, SampleOutcome::Skipped, 0);
        } else if plan.mem {
            let t = crate::perf::begin();""",
    label="Mem 退避门",
)

edit(
    state,
    SA,
    """        if plan.net && net_due {
            self.last_net_ms = now_ms;""",
    """        if plan.net && net_due && self.backoff[SamplerId::Net as usize].is_backing_off() {
            self.push_diag(SamplerId::Net, SampleOutcome::Skipped, 0);
        } else if plan.net && net_due {
            self.last_net_ms = now_ms;""",
    label="Net 退避门",
)

edit(
    state,
    SA,
    """        if plan.power {
            let t = crate::perf::begin();
            match self.syspower.sample(now_ms) {""",
    """        if plan.power && self.backoff[SamplerId::Power as usize].is_backing_off() {
            // 退避中不重复走 MSR 打开流程（SCM + 服务查询 + 设备打开，代价最高）
            self.push_diag(SamplerId::Power, SampleOutcome::Skipped, 0);
        } else if plan.power {
            let t = crate::perf::begin();
            match self.syspower.sample(now_ms) {""",
    label="Power 退避门",
)

edit(
    state,
    SA,
    """            if let Some(ref mut d) = self.disk {
                let (read, write) = d.sample(now_ms);
                snap.disk.read_bps = read.value().unwrap_or(0.0);
                snap.disk.write_bps = write.value().unwrap_or(0.0);
                snap.disk.ready = true;
            }""",
    """            if let Some(ref mut d) = self.disk {
                let (read, write) = d.sample(now_ms);
                // ready 只在**确实拿到有效读数**时为 true：PDH 首轮只建立基线，
                // 旧实现无条件置 true → 前 2s 显示 "0 B/s" 而不是未就绪。
                snap.disk.ready = read.value().is_some() || write.value().is_some();
                snap.disk.read_bps = read.value().unwrap_or(0.0);
                snap.disk.write_bps = write.value().unwrap_or(0.0);
            }""",
    label="disk ready 语义",
)

edit(
    state,
    SA,
    """                if let Some(u) = v_util {
                    gpu_vendor::set_cached_util(u);
                    snap.gpu = Metric::Value(u.clamp(0.0, 100.0));""",
    """                if let Some(u) = v_util {
                    gpu_vendor::set_cached_util(u);
                    // NaN 防护：f32::clamp 对 NaN 返回 NaN（厂商库异常时会漏进快照，
                    // 渲染成 "NaN%"）。
                    snap.gpu = if u.is_finite() {
                        Metric::Value(u.clamp(0.0, 100.0))
                    } else {
                        Metric::Unavailable
                    };""",
    label="GPU NaN 1",
)

edit(
    state,
    SA,
    """                    if let Some(u) = gpu_vendor::cached_util() {
                        snap.gpu = Metric::Value(u.clamp(0.0, 100.0));
                    }""",
    """                    if let Some(u) = gpu_vendor::cached_util() {
                        snap.gpu = if u.is_finite() {
                            Metric::Value(u.clamp(0.0, 100.0))
                        } else {
                            Metric::Unavailable
                        };
                    }""",
    label="GPU NaN 2",
)

# ══════════════════════════════════════════════════════════════════════════════
# 4. gdi.rs —— D2D 回退改投递消息 / respawn 上限 / 锁中毒恢复
# ══════════════════════════════════════════════════════════════════════════════
GD = "crates/win/src/gdi.rs"

edit(
    state,
    GD,
    """/// D2D 渲染失败后的自愈合：本会话禁用 D2D，并把持久配置回退为 Gdi。
fn mark_d2d_broken_and_persist_gdi(hwnd: HWND) {
    D2D_BROKEN.store(true, AtomicOrdering::Relaxed);
    if let Some(mut cfg) = crate::window::get_config_of(hwnd) {
        if cfg.render_engine == RenderEngine::D2d {
            cfg.render_engine = RenderEngine::Gdi;
            crate::window::modify_config(hwnd, |c| *c = cfg);
            let _ = crate::config_io::save_config(&cfg);
            crate::probe::log("d2d disabled (session) + persisted render_engine=Gdi");
        }
    }
}""",
    """/// **UI 线程侧**的 D2D 回退落地：改写 Config 并持久化。
/// 只能由 wndproc 收到 `WM_DISABLE_D2D` 时调用 —— `ThreadState` 归 UI 线程所有。
pub fn apply_d2d_fallback_ui(hwnd: HWND) {
    if let Some(mut cfg) = crate::window::get_config_of(hwnd) {
        if cfg.render_engine == RenderEngine::D2d {
            cfg.render_engine = RenderEngine::Gdi;
            crate::window::modify_config(hwnd, |c| *c = cfg);
            let _ = crate::config_io::save_config(&cfg);
            crate::probe::log("d2d disabled (session) + persisted render_engine=Gdi");
        }
    }
}

/// D2D 渲染失败后的自愈合：本会话禁用 D2D，并**请求 UI 线程**把持久配置回退为 Gdi。
///
/// \u26a0\ufe0f 本函数会被**渲染线程**（`render` 内 D2D 失败 / panic 分支）与**采样线程**
/// （看门狗 `DisableD2d` 分支）调用，都不是 UI 线程。旧实现直接在这里调
/// `get_config_of` / `modify_config`（裸指针读写 `ThreadState.cfg`），并在采样线程上
/// 同步 fsync 写盘 —— 与 UI 线程 wndproc 构成真实 data race（UB），且与退出落盘
/// 竞争同一个 `.tmp` 文件。现改为只置原子标志 + 投递消息，配置改动全部由 UI 线程做。
fn mark_d2d_broken_and_persist_gdi(hwnd: HWND) {
    D2D_BROKEN.store(true, AtomicOrdering::Release);
    unsafe {
        let _ = PostMessageW(
            hwnd,
            crate::window::WM_DISABLE_D2D,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(0),
        );
    }
}""",
    label="D2D 回退改消息投递",
)

edit(
    state,
    GD,
    """/// 已为「本次停摆」触发过 respawn（幂等闸门）：避免一次卡死被每个采样 tick
/// 重复 respawn 出多个线程。帧恢复完成后清零。
static WEDGE_RESPAWNED: AtomicBool = AtomicBool::new(false);""",
    """/// 已为「本次停摆」触发过 respawn（幂等闸门）：避免一次卡死被每个采样 tick
/// 重复 respawn 出多个线程。帧恢复完成后清零。
static WEDGE_RESPAWNED: AtomicBool = AtomicBool::new(false);
/// 本进程累计 respawn 次数（硬上限闸门）。
///
/// 每次 respawn 都会**遗留**一个可能永久卡在原生调用里的旧线程（Windows 无法取消
/// 这类阻塞）——`WEDGE_RESPAWNED` 只在"一次停摆"内幂等，帧恢复后 `clear_wedge_gate`
/// 会复位它，于是"卡死→恢复→再卡死"可以无限循环、线程无限累积。这里加会话级硬上限。
static RESPAWN_COUNT: AtomicU32 = AtomicU32::new(0);""",
    label="RESPAWN_COUNT 声明",
)

edit(
    state,
    GD,
    """            if !WEDGE_RESPAWNED.swap(true, AtomicOrdering::Relaxed) {
                return WatchdogAction::Respawn; // 触发一次 respawn
            }""",
    """            if !WEDGE_RESPAWNED.swap(true, AtomicOrdering::Relaxed) {
                // 会话级硬上限：超过 3 次不再新建线程（旧线程无法回收），
                // 直接落到下面的 D2D 回退分支，避免线程无限累积。
                if RESPAWN_COUNT.fetch_add(1, AtomicOrdering::Relaxed) < 3 {
                    return WatchdogAction::Respawn; // 触发一次 respawn
                }
                crate::probe::log(
                    "render watchdog: respawn cap (3) reached — will not spawn more render threads",
                );
            }""",
    label="respawn 硬上限",
)

edit(
    state,
    GD,
    """        let Ok(mut surface) = surface.lock() else { return };""",
    """        // 中毒恢复：与 render_thread / window 的处理风格保持一致。旧实现直接 return
        // → 一次 panic 之后**所有帧永久静默跳过**，窗口冻结且无任何日志。
        let mut surface = surface.lock().unwrap_or_else(|e| {
            crate::probe::log("render: SURFACE mutex poisoned -> recovering inner value");
            e.into_inner()
        });""",
    label="SURFACE 锁中毒",
)

edit(
    state,
    GD,
    """        let Ok(mut dib) = dib.lock() else {
            let _ = ReleaseDC(hwnd, screen_dc);
            return false;
        };""",
    """        // 中毒恢复（同上），不再静默放弃渲染。
        let mut dib = dib.lock().unwrap_or_else(|e| {
            crate::probe::log("render: DIB mutex poisoned -> recovering inner value");
            e.into_inner()
        });""",
    count=2,
    label="DIB 锁中毒 x2 (8 空格缩进)",
)

# 1194 行那处是 4 空格缩进（函数体在模块级作用域），单独处理。
edit(
    state,
    GD,
    """    let Ok(mut dib) = dib.lock() else {
        let _ = ReleaseDC(hwnd, screen_dc);
        return false;
    };""",
    """    // 中毒恢复（同上），不再静默放弃渲染。
    let mut dib = dib.lock().unwrap_or_else(|e| {
        crate::probe::log("render: DIB mutex poisoned -> recovering inner value");
        e.into_inner()
    });""",
    label="DIB 锁中毒 x1 (4 空格缩进)",
)

edit(
    state,
    GD,
    """    let Ok(mut prev) = prev.lock() else {
        return (Dirty::Full, all());
    };""",
    """    let mut prev = prev.lock().unwrap_or_else(|e| e.into_inner());""",
    label="PREV_CELLS 锁中毒",
)

# ══════════════════════════════════════════════════════════════════════════════
# 5. window.rs
# ══════════════════════════════════════════════════════════════════════════════
WD = "crates/win/src/window.rs"

# 5.1 HWND 缓存声明
edit(
    state,
    WD,
    """// ─── Shared snapshot (sampling thread → UI thread) ───────────────────""",
    """/// 本进程悬浮窗 HWND 缓存（0 = 未知）。
///
/// `find_overlay_hwnd` 的快路径：全屏自动隐藏开启时，1s 定时器每秒都会调用它，
/// 旧实现每次都 `EnumWindows` 遍历**全部**顶层窗口 + 逐窗 `GetClassNameW` ——
/// 浏览器多进程/多开应用时是 UI 线程上的可观周期开销（与 0.03% CPU 目标冲突）。
static OVERLAY_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

// ─── Shared snapshot (sampling thread → UI thread) ───────────────────""",
    label="OVERLAY_HWND 声明",
)

# 5.2 窗口创建后写缓存
edit(
    state,
    WD,
    """        unsafe {
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA.0, state_ptr);
        }""",
    """        unsafe {
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA.0, state_ptr);
        }
        // 缓存自己的 HWND（见 OVERLAY_HWND 注释）。
        OVERLAY_HWND.store(hwnd.0 as isize, Ordering::Release);""",
    label="写入 OVERLAY_HWND",
)

# 5.3 find_overlay_hwnd 快路径
edit(
    state,
    WD,
    """fn find_overlay_hwnd() -> Option<HWND> {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;""",
    """fn find_overlay_hwnd() -> Option<HWND> {
    // 快路径：本进程已缓存（99% 的调用来自 is_foreground_fullscreen 的每秒定时器）。
    let cached = OVERLAY_HWND.load(Ordering::Acquire);
    if cached != 0 {
        let h = HWND(cached as *mut _);
        if unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(h) }.as_bool() {
            return Some(h);
        }
    }
    // 慢路径：跨进程场景（二次启动的第二实例）本进程没有缓存，只能按类名枚举。
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;""",
    label="find_overlay_hwnd 快路径",
)

# 5.4 二次启动改投递消息
edit(
    state,
    WD,
    """            } else {
                crate::settings::open_settings(hwnd);
            }""",
    """            } else {
                // \u26a0\ufe0f 必须投递给**第一实例**：本函数运行在第二实例，而 SETTINGS_HWND
                // 是本进程的模块静态（= -1）→ 直接 open_settings 会在这里 create_dialog
                // 建窗，随后 main 立刻 `std::process::exit(0)`，窗口从未进入消息循环即
                // 被销毁（用户现象：悬浮窗可见时双击 exe 毫无反应）。
                unsafe {
                    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
                    let _ = PostMessageW(hwnd, WM_OPEN_SETTINGS, WPARAM(0), LPARAM(0));
                }
            }""",
    label="二次启动改消息",
)

# 5.5 新增消息常量
edit(
    state,
    WD,
    """pub const WM_SAMPLER_DIAG: u32 = 0x8000 + 5;""",
    """pub const WM_SAMPLER_DIAG: u32 = 0x8000 + 5;
/// 二次启动时请求**第一实例**在自己进程内打开设置页（跨进程 PostMessage）。
pub const WM_OPEN_SETTINGS: u32 = 0x8000 + 6;
/// D2D 自愈回退：渲染 / 采样线程只置 D2D_BROKEN 并投递本消息，
/// 由 UI 线程改写 Config 并落盘（ThreadState 归 UI 线程所有，不可跨线程写）。
pub const WM_DISABLE_D2D: u32 = 0x8000 + 7;

/// 关机 / 注销。旧实现完全没处理这两个消息（见 wndproc 分支注释）。
const WM_QUERYENDSESSION: u32 = 0x0011;
const WM_ENDSESSION: u32 = 0x0016;""",
    label="新增消息常量",
)

# 5.6 挂起自愈出口
edit(
    state,
    WD,
    """/// 采样线程查询：是否处于系统挂起状态（睡眠期间采样线程busy-wait低频重试）。
pub fn sampler_suspended() -> bool {
    SAMPLER_SUSPENDED.load(std::sync::atomic::Ordering::Relaxed)
}""",
    """/// 采样线程查询：是否处于系统挂起状态（睡眠期间采样线程busy-wait低频重试）。
pub fn sampler_suspended() -> bool {
    // Acquire：与下方 force_clear / wndproc 的 Release store 配对，保证标志位
    // 的跨线程可见性有正式的内存序保证（不再只依赖 x86 的实际行为）。
    SAMPLER_SUSPENDED.load(std::sync::atomic::Ordering::Acquire)
}

/// 强制清除挂起标志（采样线程的自愈出口）。
///
/// `SAMPLER_SUSPENDED` 的唯一正常清除点是 `PBT_APMRESUMEAUTOMATIC`；Modern Standby
/// 下该通知可能丢失 —— 那正是采样循环里"兜底唤醒检测"存在的理由，但兜底检测位于
/// 挂起等待循环**之后**，挂起期间永远执行不到 → 通知一丢采样就永久冻结。
/// 因此采样线程在挂起超过阈值时调用本函数自救。
pub fn force_clear_sampler_suspended() {
    SAMPLER_SUSPENDED.store(false, std::sync::atomic::Ordering::Release);
}""",
    label="force_clear_sampler_suspended",
)

# 5.7 诊断数据 UAF
edit(
    state,
    WD,
    """        WM_GET_SAMPLER_DIAG => {
            let diag = shared_diag();
            let ptr = &*diag as *const SamplerDiag;
            // 发送给 settings 窗口（由 lparam 传入目标 HWND）
            let target = HWND(lparam.0 as *mut _);""",
    """        WM_GET_SAMPLER_DIAG => {
            // \u26a0\ufe0f 所有权交接：PostMessage 是**异步**的。旧实现只送 `&*diag` 的裸指针，
            // `diag` 在本分支结束时 drop；采样线程下一轮 `update_shared_diag` 会 store
            // 新 Arc 并释放旧对象 → 设置窗稍后解引用即 **use-after-free**。设置页只要
            // 开着一个模态 MessageBox / 取色对话框（本项目大量使用），消息迟迟不被
            // 处理，则必现。改为 `Arc::into_raw` 把这一份强引用**移交**给接收端，
            // 接收端必须以 `Arc::from_raw` 接管（见 settings::on_wm_sampler_diag）。
            let ptr = Arc::into_raw(shared_diag());
            // 发送给 settings 窗口（由 lparam 传入目标 HWND）
            let target = HWND(lparam.0 as *mut _);""",
    label="诊断指针所有权交接",
)

# 5.8 关机 / 注销 + 新消息处理
edit(
    state,
    WD,
    """        WM_ERASEBKGND => LRESULT(1),

        WM_DESTROY => {""",
    """        // ── 关机 / 注销 ──
        // 旧实现完全没处理：WM_QUERYENDSESSION 落 DefWindowProc 返回 TRUE（不阻止关机，
        // 这点是对的），但 WM_ENDSESSION 也落 DefWindowProc → **不发 PostQuitMessage**
        // → run_message_loop 不返回 → main 的退出落盘永不执行 → 关机/注销丢配置。
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION => {
            if _wparam.0 != 0 {
                // 会话确实结束：销毁窗口走正常收尾（WM_DESTROY → PostQuitMessage →
                // main 同步落盘）。宽限期内完成，比被 TerminateProcess 干净。
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }

        // 二次启动（第二实例）→ 由**本实例**打开设置页
        WM_OPEN_SETTINGS => {
            crate::settings::open_settings(hwnd);
            LRESULT(0)
        }

        // D2D 自愈：渲染/采样线程只投递消息，配置改写与落盘在 UI 线程完成
        WM_DISABLE_D2D => {
            crate::gdi::apply_d2d_fallback_ui(hwnd);
            LRESULT(0)
        }

        WM_ERASEBKGND => LRESULT(1),

        WM_DESTROY => {""",
    label="WM_ENDSESSION / 新消息分支",
)

# 5.9 WM_DESTROY 资源清理
edit(
    state,
    WD,
    """            unsafe {
                use windows::Win32::System::RemoteDesktop::WTSUnRegisterSessionNotification;
                use windows::Win32::UI::WindowsAndMessaging::KillTimer;
                let _ = WTSUnRegisterSessionNotification(hwnd);
                let _ = KillTimer(hwnd, FULLSCREEN_TIMER_ID as _);
            }
            crate::tray::remove(hwnd);""",
    """            unsafe {
                use windows::Win32::System::RemoteDesktop::WTSUnRegisterSessionNotification;
                use windows::Win32::UI::WindowsAndMessaging::KillTimer;
                let _ = WTSUnRegisterSessionNotification(hwnd);
                let _ = KillTimer(hwnd, FULLSCREEN_TIMER_ID as _);
                // 三个定时器全部显式回收（窗口销毁时系统虽会连带清理，但显式对称，
                // 便于日后改成"关窗不销毁"的模型）。
                let _ = KillTimer(hwnd, crate::gdi::CLOCK_TIMER_ID);
                let _ = KillTimer(hwnd, crate::gdi::ANIM_TIMER_ID);
            }
            // DIB 缓存（HBITMAP + 内存 DC）显式释放，避免"窗口已销毁但 GDI 对象
            // 一直留到进程退出"。同时清掉 HWND 缓存。
            crate::gdi::invalidate_dib_cache();
            OVERLAY_HWND.store(0, Ordering::Release);
            crate::tray::remove(hwnd);""",
    label="WM_DESTROY 资源清理",
)

# ══════════════════════════════════════════════════════════════════════════════
# 6. settings/mod.rs —— 诊断数据所有权接管
# ══════════════════════════════════════════════════════════════════════════════
ST = "crates/win/src/settings/mod.rs"

edit(
    state,
    ST,
    """    /// 待重绘的控件下标集合（None = 全量重绘）。
    dirty: Option<Vec<usize>>,
    /// 共享诊断数据指针（从 overlay 窗口 WM_SAMPLER_DIAG 消息获得）。
    diag: Option<*const SamplerDiag>,""",
    """    /// 待重绘的控件下标集合（None = 全量重绘）。
    dirty: Option<Vec<usize>>,
    /// 共享诊断数据指针（从 overlay 窗口 WM_SAMPLER_DIAG 消息获得）。
    ///
    /// \u26a0\ufe0f 其有效性由 `diag_arc` 保证：指针指向的是 overlay 侧 `Arc::into_raw`
    /// 移交过来的 `SamplerDiag`，本结构持有那份强引用。旧实现只存裸指针，而发送端
    /// PostMessage 后即 drop、采样线程下一轮就释放旧对象 → 设置页此后**每一帧**都在
    /// 解引用已释放内存（use-after-free）。
    diag: Option<*const SamplerDiag>,
    /// 与 `diag` 指针同一分配的那份 `Arc`（保活用，勿删）。
    diag_arc: Option<std::sync::Arc<SamplerDiag>>,""",
    label="SettingsCtx 增加 diag_arc",
)

edit(
    state,
    ST,
    """            diag: None,""",
    """            diag: None,
            diag_arc: None,""",
    label="diag_arc 初始化",
)

edit(
    state,
    ST,
    """        unsafe fn on_wm_sampler_diag(hwnd: HWND, _msg: u32, _wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            if let Some(ctx) = ctx_of(hwnd) {
                let ptr = lparam.0 as *const SamplerDiag;
                ctx.diag = Some(ptr);
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }""",
    """        unsafe fn on_wm_sampler_diag(hwnd: HWND, _msg: u32, _wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            if let Some(ctx) = ctx_of(hwnd) {
                // 接管 overlay 侧 `Arc::into_raw` 移交的所有权（见 window::wndproc 的
                // WM_GET_SAMPLER_DIAG）。必须 from_raw，否则每个消息漏一份 Arc 强引用。
                if lparam.0 != 0 {
                    let arc = std::sync::Arc::from_raw(lparam.0 as *const SamplerDiag);
                    ctx.diag = Some(std::sync::Arc::as_ptr(&arc));
                    ctx.diag_arc = Some(arc);
                } else {
                    ctx.diag = None;
                    ctx.diag_arc = None;
                }
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }""",
    label="on_wm_sampler_diag 接管所有权",
)

# ══════════════════════════════════════════════════════════════════════════════
# 7. cfg_ref.rs —— panic → false；get 侧边界；补 roundtrip 用例
# ══════════════════════════════════════════════════════════════════════════════
CR = "crates/win/src/settings/cfg_ref.rs"

_src = state[CR]
_pat = re.compile(r'panic!\("CfgRef 类型错配: [^"]*", v\);')
_cnt = len(_pat.findall(_src))
assert _cnt >= 35, "cfg_ref panic 数量异常: %d" % _cnt
state[CR] = _pat.sub("false", _src)
print("   cfg_ref.rs: panic! -> false 共 %d 处" % _cnt)

edit(
    state,
    CR,
    """            CfgRef::MetricFont(i) => Value::Num(cfg.metric_font_sizes[i].unwrap_or(cfg.font_size) as f64),
            CfgRef::MetricOffset(i, c) => Value::Num(cfg.metric_offsets.get(i).and_then(|o| o.get(c as usize)).copied().unwrap_or(0) as f64),
            CfgRef::MetricLabel(i) => Value::Color(cfg.metric_colors[i].label),
            CfgRef::MetricValue(i) => Value::Color(cfg.metric_colors[i].value),
            CfgRef::MetricValueAuto(i) => Value::Bool(cfg.metric_colors[i].value_auto),""",
    """            // \u26a0\ufe0f get 侧一律用 `.get(i)` 防护：旧实现是裸下标
            // （`metric_font_sizes[i]` / `metric_colors[i]`），越界即 panic；而 set 侧
            // 有 `i < len` 检查返回 false —— 两侧不对称，且 get 挂在 wndproc 调用链上
            // （越界 = 经 extern "system" 边界 abort 整个进程）。
            CfgRef::MetricFont(i) => Value::Num(
                cfg.metric_font_sizes.get(i).copied().flatten().unwrap_or(cfg.font_size) as f64,
            ),
            CfgRef::MetricOffset(i, c) => Value::Num(
                cfg.metric_offsets.get(i).and_then(|o| o.get(c as usize)).copied().unwrap_or(0)
                    as f64,
            ),
            CfgRef::MetricLabel(i) => Value::Color(cfg.metric_colors.get(i).and_then(|m| m.label)),
            CfgRef::MetricValue(i) => Value::Color(cfg.metric_colors.get(i).and_then(|m| m.value)),
            CfgRef::MetricValueAuto(i) => {
                Value::Bool(cfg.metric_colors.get(i).map(|m| m.value_auto).unwrap_or(false))
            }""",
    label="get 侧边界防护",
)

edit(
    state,
    CR,
    """            (CfgRef::Opacity, Value::Num(80.0)),
            (CfgRef::Opacity, Value::Num(40.0)),
        ]""",
    """            (CfgRef::Opacity, Value::Num(80.0)),
            (CfgRef::Opacity, Value::Num(40.0)),
            // 审计补全（2026-09-15）：以下变体此前**完全没有** roundtrip 覆盖 ——
            // 编译器只能保证 match 臂存在，抓不到"臂写错字段"这类语义错误。
            (CfgRef::ShowDiskRead, Value::Bool(true)),
            (CfgRef::ShowDiskRead, Value::Bool(false)),
            (CfgRef::ShowDiskWrite, Value::Bool(true)),
            (CfgRef::ShowDiskWrite, Value::Bool(false)),
            (CfgRef::MetricValueAuto(0), Value::Bool(false)),
            (CfgRef::MetricShowLabel(8), Value::Bool(true)),
            (CfgRef::MetricOffset(0, 0), Value::Num(5.0)),
            (CfgRef::MetricOffset(0, 3), Value::Num(-7.0)),
        ]""",
    label="roundtrip 补全",
)

edit(
    state,
    CR,
    """    #[test]
    fn out_of_range_index_is_rejected_not_panicking() {
        let mut c = cfg();
        assert!(!CfgRef::MetricFont(9).set(&mut c, Value::Num(1.0)));
        assert!(!CfgRef::MetricLabel(9).set(&mut c, Value::Color(None)));
        assert!(!CfgRef::Lang.set(&mut c, Value::Select(7)));
    }""",
    """    #[test]
    fn out_of_range_index_is_rejected_not_panicking() {
        let mut c = cfg();
        assert!(!CfgRef::MetricFont(9).set(&mut c, Value::Num(1.0)));
        assert!(!CfgRef::MetricLabel(9).set(&mut c, Value::Color(None)));
        assert!(!CfgRef::Lang.set(&mut c, Value::Select(7)));
        // get 侧同样不得 panic（旧实现是裸下标 → 越界 panic → 进程 abort）
        let _ = CfgRef::MetricFont(99).get(&c);
        let _ = CfgRef::MetricLabel(99).get(&c);
        let _ = CfgRef::MetricValue(99).get(&c);
        let _ = CfgRef::MetricValueAuto(99).get(&c);
        let _ = CfgRef::MetricOffset(99, 9).get(&c);
        let _ = CfgRef::MetricShowLabel(99).get(&c);
    }

    #[test]
    fn type_mismatch_returns_false_instead_of_panicking() {
        // 回归锁：类型错配曾直接 panic!，而调用链位于 extern "system" 的 wndproc
        // 之下 → panic 跨 FFI 边界即 abort，用户看到"点一下按钮整个进程消失"。
        let mut c = cfg();
        assert!(!CfgRef::ShowCpu.set(&mut c, Value::Num(1.0)));
        assert!(!CfgRef::FontSize.set(&mut c, Value::Bool(true)));
    }""",
    label="补越界与类型错配测试",
)

# 类型错配测试：旧的 should_panic 测试已不成立，删除
edit(
    state,
    CR,
    """    // debug_assert! 在 release 下被编译掉 → 本测试只能在 debug 构建中运行
    // （`cargo test --release` 会因「未触发 panic」而失败，故加门控）。
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic]
    fn type_mismatch_panics_in_debug() {
        let mut c = cfg();
        let _ = CfgRef::ShowCpu.set(&mut c, Value::Num(1.0));
    }
""",
    "",
    label="删除 should_panic 测试",
)

# ══════════════════════════════════════════════════════════════════════════════
# 8. msr.rs —— transmute / GetLastError
# ══════════════════════════════════════════════════════════════════════════════
MS = "crates/win/src/msr.rs"

edit(
    state,
    MS,
    """        let r0 = std::arch::x86_64::__cpuid(0);
        let vendor = [r0.ebx, r0.edx, r0.ecx];
        let vb: [u8; 12] = std::mem::transmute(vendor);
        let amd = &vb == b"AuthenticAMD";""",
    """        let r0 = std::arch::x86_64::__cpuid(0);
        // 用 to_le_bytes 拼接，而不是 transmute([u32;3] → [u8;12])：语义等价
        // （x86 小端）但消除了不必要的 UB 面，也不再强假设目标字节序。
        let mut vb = [0u8; 12];
        vb[0..4].copy_from_slice(&r0.ebx.to_le_bytes());
        vb[4..8].copy_from_slice(&r0.edx.to_le_bytes());
        vb[8..12].copy_from_slice(&r0.ecx.to_le_bytes());
        let amd = &vb == b"AuthenticAMD";""",
    label="transmute 消除",
)

edit(
    state,
    MS,
    """            GetLastError, GENERIC_READ, GENERIC_WRITE, ERROR_SERVICE_ALREADY_RUNNING,""",
    """            GENERIC_READ, GENERIC_WRITE, ERROR_SERVICE_ALREADY_RUNNING,""",
    label="移除 GetLastError 导入",
)

edit(
    state,
    MS,
    """            if GetLastError() != ERROR_SERVICE_ALREADY_RUNNING {""",
    """            // 用 e.code() 而非 GetLastError()：后者是"最近一次 Win32 调用"的错误码，
            // 可能被 crate 内部后续调用污染，导致 183 判定失效。
            if e.code().0 as u32 != ERROR_SERVICE_ALREADY_RUNNING.0 {""",
    label="StartServiceW 错误码",
)

# ══════════════════════════════════════════════════════════════════════════════
# 9. lib.rs —— 导出自愈出口
# ══════════════════════════════════════════════════════════════════════════════
edit(
    state,
    "crates/win/src/lib.rs",
    """pub use window::{snap_store, get_shared_seq, session_locked, sampler_suspended, in_remote_session};""",
    """pub use window::{
    snap_store, get_shared_seq, session_locked, sampler_suspended, in_remote_session,
    force_clear_sampler_suspended,
};""",
    label="lib 导出",
)

# ══════════════════════════════════════════════════════════════════════════════
# 10. main.rs —— 挂起自愈 / 兜底阈值 / 采样线程 panic 兜底
# ══════════════════════════════════════════════════════════════════════════════
MN = "crates/app/src/main.rs"

edit(
    state,
    MN,
    """            while aaxsyspulse_win::sampler_suspended() {
                // 100ms 轮询恢复信号，不使用 recv() 阻塞——让窗口隐藏时的
                // resume_rx 信号也能唤醒本线程。
                if resume_rx.recv_timeout(std::time::Duration::from_millis(100)).is_err() {
                    // 无信号，超时继续等待（sampler_suspended 变 false 前每轮睡 100ms）
                }
                // 即使收到信号也再循环一次，让 try_recv() 消费掉后再继续
            }""",
    """            // \u26a0\ufe0f 超时自愈（2026-09-15 审计）：SAMPLER_SUSPENDED 的唯一清除点是
            // `PBT_APMRESUMEAUTOMATIC`，而 Modern Standby 下该通知可能丢失 —— 那正是
            // 下面「兜底唤醒检测」存在的理由，但兜底检测在本循环**之后**，挂起期间
            // 永远执行不到 → 通知一丢采样就永久冻结。因此这里必须有独立的超时自愈。
            // 阈值 60s 远大于任何合法睡眠间隔，不会误伤正常挂起。
            const SUSPEND_SELFHEAL_MS: u64 = 60_000;
            let suspend_enter = get_tick_count_ms();
            while aaxsyspulse_win::sampler_suspended() {
                // 100ms 轮询恢复信号，不使用 recv() 阻塞——让窗口隐藏时的
                // resume_rx 信号也能唤醒本线程。
                let _ = resume_rx.recv_timeout(std::time::Duration::from_millis(100));
                // 即使收到信号也再循环一次，让 try_recv() 消费掉后再继续
                if get_tick_count_ms().saturating_sub(suspend_enter) > SUSPEND_SELFHEAL_MS {
                    aaxsyspulse_win::force_clear_sampler_suspended();
                    sampler.reset_backoff();
                    aaxsyspulse_win::probe::log(
                        "sampler: suspend flag stuck >60s -> forced resume (resume notification lost)",
                    );
                    break;
                }
            }""",
    label="挂起超时自愈",
)

edit(
    state,
    MN,
    """        let now_init = get_tick_count_ms();
        let mut last_sample_ms = now_init;""",
    """        let now_init = get_tick_count_ms();
        let mut last_sample_ms = now_init;
        // 上一轮**实际计划**的睡眠时长（ms）：兜底唤醒检测的阈值必须 ≥ 它，
        // 否则「全源关闭 → 30s 慢心跳」「远程会话 ×2 → 最长 60s」会每轮误判。
        let mut last_sleep_ms: u64 = 0;""",
    label="last_sleep_ms 声明",
)

edit(
    state,
    MN,
    """            let now_before_wait = get_tick_count_ms();
            const FALLBACK_WAKE_GAP_MS: u64 = (REFRESH_FLOOR_MS as u64) * 3;
            if now_before_wait.saturating_sub(last_sample_ms) > FALLBACK_WAKE_GAP_MS {""",
    """            let now_before_wait = get_tick_count_ms();
            // \u26a0\ufe0f 阈值必须与"本轮实际计划的睡眠时长"挂钩：固定 3×REFRESH_FLOOR_MS
            // (=4500ms) 会被 30s 慢心跳（全源关闭）与远程会话 ×2（最长 60s）**每轮**
            // 误判成"系统刚从睡眠恢复"，持续刷日志、重置退避，掩盖真实的采样失败。
            let fallback_gap_ms =
                ((REFRESH_FLOOR_MS as u64) * 3).max(last_sleep_ms.saturating_mul(3) / 2);
            if now_before_wait.saturating_sub(last_sample_ms) > fallback_gap_ms {""",
    label="兜底阈值动态化",
)

# panic 兜底：包住整个循环体（先闭合，再声明；顺序无关，模式各自唯一）
edit(
    state,
    MN,
    """            cfg_gen = wait_cfg_change(cfg_gen, timeout_ms as u64);
        }
    });""",
    """            last_sleep_ms = timeout_ms as u64;
            cfg_gen = wait_cfg_change(cfg_gen, timeout_ms as u64);
            })); // ← 结束 catch_unwind 闭包
            if __iter.is_err() {
                aaxsyspulse_win::probe::log(
                    "sampler: PANIC caught in sampling loop — thread survives (was: silent freeze)",
                );
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    });""",
    label="采样循环 panic 兜底闭合",
)

edit(
    state,
    MN,
    """        loop {
            // 采样线程暂停（系统即将睡眠 / 会话已锁定）：低频轮询等待恢复信号，""",
    """        loop {
            // panic 兜底（2026-09-15 审计）：渲染线程有 catch_unwind，本线程原本没有
            // —— 任一子采样器 panic 会**静默杀死采样线程**，数值永久冻结；而渲染看门狗
            // 只监控渲染线程（RENDER_EPOCH 不再增长被判为"健康"），永不 respawn。
            // 兜住后记日志、短暂退避、继续下一轮，把"必须重启程序"降级为"闪一下"。
            let __iter = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // 采样线程暂停（系统即将睡眠 / 会话已锁定）：低频轮询等待恢复信号，""",
    label="采样循环 panic 兜底开启",
)

# ══════════════════════════════════════════════════════════════════════════════
# 11. 内存序提升（标志位 Relaxed → SeqCst）
# ══════════════════════════════════════════════════════════════════════════════
bump_ordering(state, WD, ["SESSION_LOCKED", "IN_REMOTE_SESSION", "SAMPLER_SUSPENDED"])
bump_ordering(state, GD, ["D2D_BROKEN"])

# ══════════════════════════════════════════════════════════════════════════════
# 落盘
# ══════════════════════════════════════════════════════════════════════════════
for rel in FILES:
    p = os.path.join(ROOT, rel)
    with io.open(p, "w", encoding="utf-8", newline="") as f:
        f.write(state[rel])
    print("WROTE %s" % rel)

print("\n全部替换完成。")
