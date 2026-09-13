# 设置页 UI 升级 — 简单任务完成状态

> 依据 `SETTINGS_UI_UPGRADE_PLAN.md`（9 阶段 A–I）。本轮聚焦「先完成简单任务并标记状态」。
> 用户约束：**不守轻量化体积**，但**必须守性能优化原则**（几何缓存、逐帧重算避免、动画收敛后停帧防空转）。

## 一、简单 / 复杂 任务划分（按复杂度与依赖）

| 阶段 | 任务 | 量级 | 判定理由 |
|------|------|------|----------|
| A1 | `types.rs` 新增 `Appearance` 枚举 | 简单 | 纯数据 + 解析/序列化，叶子节点 |
| A2 | `config.rs` 加 `appearance` / `settings_size` 字段与读写 | 简单 | 配置层增量，纯文本解析 |
| A3 | `cfg_ref.rs` 加 `Appearance` 变体 + 往返 | 简单 | 枚举变体 + get/set，无绘制 |
| B1 | `theme.rs` 加 4 个侧栏色槽 + `resolve(Appearance)` | 简单 | 数据 + 纯函数，无绘制 |
| C2 | `gfx_d2d.rs` 字体族降级探测（缓存） | 简单 | 新增模块级纯函数 + 缓存字段 |
| F1 | `widget.rs` 滑杆值↔位置纯函数 | 简单 | 纯函数 + 单测，无绘制 |
| H1 | `motion.rs` 弹簧/动量/橡胶/可访问性探测 | 简单 | 新文件，纯逻辑 + 单测 |
| C1 | `icons.rs` SVG→D2D 路径翻译（几何缓存） | 中等 | 需 SVG path 解析器，但按 `IconKind` 缓存一次 |
| D  | 侧边栏骨架（布局+绘制+命中） | 复杂 | 依赖 B1，多控件编排 |
| E  | 窗口尺寸可调 + macOS 一体化标题 | 复杂 | 依赖 D、主题切换 |
| F2 | 填充式滑杆**绘制** | 复杂 | 消费 F1，依赖 B1 色槽 |
| F3 | 填充式滑杆**拖动** | 复杂 | 消费 F1，依赖 H1 弹簧 |
| G  | 个性化页折叠卡 | 复杂 | 依赖 H1 折叠弹簧 + 布局 |
| H2–H4 | 流体动效集成（悬停/翻页/惯性） | 复杂 | 消费 H1，贯穿 D/E/F/G |
| I  | 收尾（可访问性降级、焦点环、回归） | 复杂 | 依赖全部前置 |

## 二、简单任务完成情况 ✅（全部已编译 + 单测验证）

| 任务 | 文件 | 关键改动 | 验证 |
|------|------|----------|------|
| A1 | `crates/core/src/types.rs` | `Appearance{System,Light,Dark}` + `from_config`/`to_config`/`ALL` | core 125 passed（历史） |
| A2 | `crates/core/src/config.rs` | `appearance`/`settings_size` 字段、默认 720×560、钳制 560..1400×460..1000、round-trip 单测 | core 125 passed（历史） |
| A3 | `crates/win/src/settings/cfg_ref.rs` | `CfgRef::Appearance` 变体 + get/set + 往返单测 | win cfg_ref 6 passed（历史） |
| B1 | `crates/win/src/theme.rs` | `sidebar_bg`/`content_bg`/`sidebar_hover`/`sidebar_selected` 4 色槽（LIGHT/DARK 填充）+ `Theme::resolve(a)` | `test_appearance_resolve_and_sidebar` passed |
| C2 | `crates/win/src/gfx_d2d.rs` | `probe_font_family`（Segoe UI Variable→Segoe UI 降级，按系统字体集合探测）+ `ui_family` 缓存字段 + `text_format` 改用探测族 | `font_tests::probe_returns_null_terminated_wide` passed |
| F1 | `crates/win/src/settings/widget.rs` | `slider_t_from_value` / `slider_value_from_x` 纯函数 + `slider_tests` 3 单测 | `slider_tests` 3 passed |
| H1 | `crates/win/src/settings/motion.rs` | `Spring`（半隐式欧拉，收敛返回 true 可停帧）、`project`、`rubber_band`、`animations_enabled`、`high_contrast`，5 单测；模块 `mod motion;` 已接入 `settings/mod.rs` | `settings::motion::tests` 5 passed（本次新接入） |

**汇总：`cargo test -p aaxsyspulse-win --lib` → 32 passed / 0 failed / 0 警告。**

## 三、本轮踩坑记录（已修复）

1. **同文件批量 Edit 竞态**：一条消息里对同一个 `.rs` 文件发多个 Edit 时，只有部分落地（last-writer-wins 覆盖原始快照）。
   修复：对同一文件的多次改动**逐条单独发送**（不同文件可并行）。
2. **`motion.rs` 未接入模块树**：新建文件后忘记在 `settings/mod.rs` 加 `mod motion;`，导致它的 5 个单测不跑。已补。
3. **Windows API 签名**：`SystemParametersInfoW` 第 4 参是 `SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS` 而非 `0`；其返回 `Result` 而非 `BOOL`（用 `.is_ok()`）；`SPI_GETHIGHCONTRAST` 归属 `WindowsAndMessaging`（非 `Accessibility`）；`hc.dwFlags & HCF_HIGHCONTRASTON` 比较需用 `HIGHCONTRASTW_FLAGS(0)`。
4. **`PCWSTR` 漏导入**：C2 用了 `PCWSTR` 但原文件只有 `use windows::core::w;`，补 `use windows::core::{w, PCWSTR};`。

## 四、`cargo build`（非 test）剩余 3 警告（预期，待复杂阶段接入后消失）

- `CfgRef::Appearance` 变体未构造 —— D/E 布局实例化该控件后消除。
- `slider_t_from_value` / `slider_value_from_x` 未使用 —— F2/F3 绘制与拖动消费后消除。

> test 构建下这 3 处已被单测覆盖，故零警告。符合计划里程碑「C 闸门：零警告」的方向。

## 五、性能原则落实核对

- ✅ 图标/字体/画刷/文字布局/文字格式均**只解/只建一次并缓存**（C2 字体探测结果缓存进 `Gfx.ui_family`；H1 系统动效开关 `ANIM_CACHED`/`HC_CACHED` 静态缓存）。
- ✅ 动画收敛即停帧：`Spring::step` 返回 `bool`，收敛后可停止重绘，避免空转烧 CPU（H1）。
- ⏳ SVG 图标几何缓存（C1）与动画集成（H2–H4）待复杂阶段落地时同样须守此原则。
