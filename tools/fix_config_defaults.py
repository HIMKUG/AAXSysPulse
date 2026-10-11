# -*- coding: utf-8 -*-
"""一次性脚本：重写 guide-app config.rs 的 Default 实现 + 追加 ensure_all_files()。

背景：
  * `#[derive(Default)]` 让数值字段全为 0 —— `SystemConfig::normal_opacity = 0`
    会让悬浮窗完全透明、`settings_w/h = 0` 被主程序钳到下限；
    `AppearanceConfig::font_size = 0.0` 被钳到 4px；`MonitorConfig::items` 为空
    则 `save()` 按下标 0..8 写入时**越界 panic**。
  * 这些 Default 只在「文件缺失 / `unwrap_or_default()`」时被使用，
    所以必须与各自 `load()` 的缺失分支保持同一套默认值。
"""
import sys

PATH = r"D:\code\sys-overlay-c-master\guide-app\src-tauri\src\config.rs"

with open(PATH, "r", encoding="utf-8", newline="") as f:
    src = f.read()

orig = src


def reps(old, new, count=1):
    global src
    n = src.count(old)
    assert n == count, "expected %d occurrence(s), got %d for: %r" % (count, n, old[:90])
    src = src.replace(old, new)


DERIVE = "#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]"
DERIVE_NO_DEF = "#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]"

# ── 1. 去掉这 4 个结构体的 derive(Default)，改为手写 ───────────────────────────
for name in ("SystemConfig", "AppearanceConfig", "MonitorConfig", "LayoutConfig"):
    reps(DERIVE + "\npub struct " + name + " {",
         DERIVE_NO_DEF + "\npub struct " + name + " {")

# ── 2. SystemConfig::default ─────────────────────────────────────────────────
SYS = '''impl Default for SystemConfig {
    /// 与 [`SystemConfig::load`] 的「文件不存在」分支**同一套默认值**。
    ///
    /// ⚠️ 早期 `#[derive(Default)]` 让数值全为 0：`normal_opacity = 0` 会让悬浮窗
    /// 完全透明，`settings_w/h = 0` 会被主程序钳到下限，`interval_ms = 0` 会被吸附
    /// 到 100ms（最快档）。文件缺失时写出的就是这份垃圾配置。
    fn default() -> Self {
        Self {
            version: 3,
            x: -1,
            y: -1,
            w: 0,
            h: 0,
            locked: false,
            layout: "horizontal".to_string(),
            zoom: 100,
            net_unit: "mb".to_string(),
            net_mode: "both".to_string(),
            interval_ms: 1000,
            refresh_auto: true,
            topmost: true,
            normal_opacity: 100,
            anim_ms: 300,
            autostart: false,
            debug_log: false,
            render_mode: "quality".to_string(),
            render_engine: "0".to_string(),
            appearance: 0,
            settings_w: 800,
            settings_h: 640,
            click_through: false,
            snap_corner: 0,
            fullscreen_auto_hide: true,
            gpu_primary_only: false,
            gpu_3d_only: false,
            net_low_freq: false,
            lang: "en".to_string(),
            admin_check_continue_count: 0,
            driver_check_continue_count: 0,
            admin_check_last_date: 0,
            driver_check_last_date: 0,
        }
    }
}

'''
reps("impl SystemConfig {", SYS + "impl SystemConfig {")

# ── 3. AppearanceConfig::default ────────────────────────────────────────────
APP = '''impl Default for AppearanceConfig {
    /// 与 [`AppearanceConfig::load`] 缺失分支同默认值（否则 `font_size = 0.0`
    /// 会被主程序钳到 4px、间距全为 0）。
    fn default() -> Self {
        Self {
            version: 3,
            bg: "#000000".to_string(),
            fg: "#FFFFFF".to_string(),
            font_size: 12.0,
            font_weight: 0,
            metric_gap: 16,
            label_value_gap: 4,
            shadow: true,
            outline: false,
        }
    }
}

'''
reps("impl AppearanceConfig {", APP + "impl AppearanceConfig {")

# ── 4. MonitorConfig::default ───────────────────────────────────────────────
MON = '''impl Default for MonitorConfig {
    /// ⚠️ 必须自带 9 个 item（id = 1..9）：`save()` 会按 `items[(id-1)]` 写
    /// `[item.1]..[item.9]`，空 `items` 会**越界 panic**。`unwrap_or_default()`
    /// 是一条常见回退路径，不能在这里踩坑。
    fn default() -> Self {
        Self {
            version: 3,
            util_decimal: true,
            mem_gb: true,
            short_names: false,
            show_time: true,
            time_seconds: true,
            items: (1..=9)
                .map(|id| MonitorItemConfig {
                    id,
                    show: true,
                    name: true,
                    ..Default::default()
                })
                .collect(),
            power_enabled: true,
            power_query: true,
            power_display: "cpupackage".to_string(),
            power_interval_ms: 3000,
            exclude_virtual_nic: true,
        }
    }
}

'''
reps("impl MonitorConfig {", MON + "impl MonitorConfig {")

# ── 5. LayoutConfig::default ────────────────────────────────────────────────
LAY = '''impl Default for LayoutConfig {
    /// 与 [`LayoutConfig::load`] 缺失分支同默认值：`order = 1..9`、9 组零偏移。
    /// 否则会写出空 `order`（主程序按默认序兜底，但语义不该依赖兜底）。
    fn default() -> Self {
        Self {
            version: 3,
            order: (1..=9).collect(),
            offsets: vec![[0, 0, 0, 0]; 9],
        }
    }
}

'''
reps("impl LayoutConfig {", LAY + "impl LayoutConfig {")

# ── 6. 追加 ensure_all_files() ──────────────────────────────────────────────
ENSURE = '''// ============ 启动期补齐 ============

/// 保证 5 个 `.cfg` **全部存在**（缺失的用默认值补成 v3）。返回本次补写的文件名。
///
/// ⚠️ 为什么必须连「非激活布局」一起补齐：
/// 主程序 `config_io::check_version_consistency` 会遍历**全部 5 个文件**读版本号，
/// 文件缺失 ⇒ 版本读不到 ⇒ 被判为 **v1**；此时其余文件已是 v3 ⇒ span = 3-1 = 2 > 1
/// ⇒ 触发 `VersionSpanTooLarge` 破坏性弹窗。引导流程若只写「当前激活布局」，
/// 在**主程序从未运行过**的全新机器上 `layout_v.cfg` 会永久缺失
/// ⇒ 主程序每次启动都报「版本跨度过大」。
/// 附带好处：主程序的旧配置迁移判定是「`config.txt` 存在 **且任一 `.cfg` 缺失**」，
/// 补齐 5 个文件同时避免了引导写好的设置被旧 `config.txt` 迁移覆盖。
///
/// 只写**缺失**的文件，已存在的一律不动（不覆盖用户 / 主程序的数据）。
pub fn ensure_all_files() -> io::Result<Vec<String>> {
    ensure_config_dir()?;
    let dir = config_dir();
    let mut created = Vec::new();

    if !dir.join("system.cfg").exists() {
        SystemConfig::default().save()?;
        created.push("system.cfg".to_string());
    }
    if !dir.join("appearance.cfg").exists() {
        AppearanceConfig::default().save()?;
        created.push("appearance.cfg".to_string());
    }
    if !dir.join("monitor.cfg").exists() {
        MonitorConfig::default().save()?;
        created.push("monitor.cfg".to_string());
    }
    for vertical in [false, true] {
        let name = if vertical { "layout_v.cfg" } else { "layout_h.cfg" };
        if !dir.join(name).exists() {
            LayoutConfig::default().save(vertical)?;
            created.push(name.to_string());
        }
    }

    Ok(created)
}

'''
reps("// ============ 原子写 ============", ENSURE + "// ============ 原子写 ============")

# ── 7. 字符串/注释感知的花括号平衡校验 ──────────────────────────────────────
def strip_code(text):
    out = []
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c == '/' and i + 1 < n and text[i + 1] == '/':
            while i < n and text[i] != '\n':
                i += 1
        elif c == '"':
            i += 1
            while i < n and text[i] != '"':
                if text[i] == '\\':
                    i += 1
                i += 1
            i += 1
        elif c == "'":
            # 生命周期标注 'a 与字符字面量 'x'
            j = i + 1
            while j < n and (text[j].isalnum() or text[j] == '_'):
                j += 1
            if j < n and text[j] == "'":
                i = j + 1
            else:
                i += 1
        else:
            out.append(c)
            i += 1
    return ''.join(out)


code = strip_code(src)
for open_c, close_c in (('{', '}'), ('(', ')'), ('[', ']')):
    a, b = code.count(open_c), code.count(close_c)
    assert a == b, "unbalanced %s%s : %d vs %d" % (open_c, close_c, a, b)

assert src != orig, "no change made"

with open(PATH, "w", encoding="utf-8", newline="") as f:
    f.write(src)

print("OK: config.rs rewritten, %d -> %d bytes" % (len(orig), len(src)))
