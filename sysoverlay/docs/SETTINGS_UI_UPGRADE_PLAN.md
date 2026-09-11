# 设置页前端 UI 升级 —— 实施计划

> 方法论：superpowers（规格 → 计划 → 红绿 TDD → 收尾）
> 设计依据：apple-design（Apple 流体交互 / WWDC 弹簧参数）
> 决策方式：grill-me（关键分支逐条裁决后冻结）
> 冻结日期：2026-09-11

---

## 0. 决策冻结

以下 9 项由用户裁决，本轮不再回头改；计划中出现歧义时以本表为准。

| # | 决策项 | 冻结取值 | 连带影响 |
|---|--------|----------|----------|
| 1 | 布局骨架 | **左侧边栏 + 加宽窗口** | 导航/命中/窗口尺寸/坐标原点全部重写 |
| 2 | 窗口尺寸 | **可调整大小 + 记忆**，默认 720×560，最小 560×460，最大 1400×1000（逻辑 px） | 需 WM_NCHITTEST 无边框 resize、尺寸落盘、resize 后重排 |
| 3 | 滑杆范式 | **标准填充式绝对滑杆** | 摇杆模型（`SliderFx` / `slider_tick`）整体废弃 |
| 4 | 动效深度 | **完整 Apple 流体动效**（弹簧 / 可中断 / 按压反馈 / 减弱动效） | 需弹簧积分器 + 减弱动效探测 + 逐控件过渡态 |
| 5 | 侧边栏图标 | **D2D 路径**（先占位几何，SVG 到位后翻译） | 新建 `icons.rs` 抽象层，零新依赖 |
| 6 | 标题栏 | **macOS 一体化**：标题在侧边栏顶部，窗口控制按钮在右上 | `paint_titlebar` 重写；`titlebar.rs` 按钮矩形适配 |
| 7 | 外观切换 | **新增三态（跟随系统 / 浅色 / 深色）+ 实时切换** | `Appearance` 枚举 + `Theme::resolve` + 运行期换肤 |
| 8 | 个性化页组织 | **交付方设计**：每指标折叠为一张卡 | 需折叠态 + 展开重排 |
| 9 | 资源基线 | **设置窗口放宽**；常驻悬浮窗仍守严格基线 | 设置窗可用更高帧率与更多缓存，但**不得**新增常驻线程 |

---

## 0.5 进度快照（2026-09-11）

> 实现严格对齐本轮冻结架构。**全部阶段 A–I 已完成**：`cargo build --release` 0 errors / 0 warnings；`cargo test` 全绿（core 125 + ui 25 + win 33 = 183 passed / 0 failed）。

| 阶段 | 状态 | 落点（文件） |
|------|------|--------------|
| A 配置层扩展（`Appearance` / `settings_size` / `CfgRef::Appearance`） | 已完成 | `core/config.rs`、`settings/cfg_ref.rs` |
| B 主题热切换（`Theme::resolve` + 侧边栏语义色槽） | 已完成 | `theme.rs` |
| C1 图标抽象层（`IconKind` + 占位几何） | 已完成 | `settings/icons.rs` |
| C2 字体族降级探测 | 已完成 | `gfx_d2d.rs` |
| D 侧边栏骨架（双区域原点 R7 / `paint_sidebar` / 滚动 / 命中） | 已完成 | `settings/layout.rs`、`state.rs`、`mod.rs` |
| E 窗口尺寸与一体化标题（`WS_THICKFRAME` + `WM_GETMINMAXINFO` / `WM_SIZE` / `WM_EXITSIZEMOVE` / `WM_NCHITTEST` 边框热区） | 已完成 | `settings/mod.rs` |
| F1 值↔位置映射（纯函数 TDD） | 已完成 | `settings/widget.rs` |
| F2 滑杆绝对位置 + 轨道填充 | 已完成 | `settings/widget.rs` |
| F3 拖动语义绝对定位（摇杆模型整体删除） | 已完成 | `settings/mod.rs` |
| G1–G3 个性化页折叠卡（每指标一卡 + 折叠箭头 + 悬停才显色的重置图标） | 已完成 | `settings/layout.rs`、`widget.rs`、`mod.rs` |
| G 附带：常规页新增「外观」三态分段行 | 已完成 | `settings/layout.rs` |
| H1 弹簧积分 / 动量投射 / 橡胶边界 / 可访问性探测 | 已完成 | `settings/motion.rs` |
| H2 按压与悬停弹簧过渡（`Transition{hover,press}` + `lerp_color` + 收敛停帧） | 已完成 | `settings/mod.rs`、`widget.rs` |
| H3 翻页弹簧化 + 可中断（`page_slide` / `page_fade`） | 已完成 | `settings/mod.rs` |
| H4 滚轮惯性 + 橡胶边界（`scroll_vel` / `scroll_spring` / `rubber_band`） | 已完成 | `settings/mod.rs` |
| I1 光学字号与高对比度（侧栏标题 20pt、项 11pt/500、高对比实心选中） | 已完成 | `settings/mod.rs` |
| I2 i18n 新键（Appearance / Expand / Collapse 等） | 已完成 | `i18n.rs`（B 阶段已随附，无需重复） |
| I3 死代码清理（`paint_nav` / `SliderFx` / `joystick` / `Action::Scroll` 等全删，移除全部 WIP `allow(dead_code)`） | 已完成 | 多处 |
| I4 全量验证（release 0 警告 + 183 测试全绿 + 配置往返不变式） | 已完成 | — |

**本轮不变式落实情况**

- **R7 双区域坐标系**：`content_origin(dpi, scroll_y) = (s(dpi, SIDEBAR_W), -scroll_y)`；`Widget.rect` / `Card.rect` 一律存内容坐标，绘制与命中共用同一原点（`paint_page` 与 `paint_widget` 均改收 `(ox, oy)`）。
- **滑杆（F2/F3）**：`SliderWidget.joystick` 字段与整套 `slider_*` / `SliderFx` 已删除；绘制 = 轨道底色 + accent 填充段 + 绝对位置滑块；拖动 = 按下即跳值、按当前 x 绝对落值（`slider_value_from_x`）。
- **窗口（E1/E2）**：`WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX`；边框热区返回 `HT*`（角优先），标题栏空白 `HTCAPTION`、按钮 `HTCLIENT`；尺寸钳制 `MIN/MAX`；`WM_EXITSIZEMOVE` 落盘 `settings_size`；标题移入侧边栏顶部。
- **性能**：几何 / 字体 / 画刷沿用既有缓存；滚动条仅当内容溢出时绘制；滚轮仅在内容区生效；不新增常驻线程。
- **动效（H）**：`Transition{hover,press}` 逐控件弹簧 + `motion::K_UI`；翻页 / 滚动 / 折叠均由弹簧驱动；全部收敛后不再请求新帧（防空转烧 CPU）。
- **占位清理（I3）**：`state.rs` / `motion.rs` 的 `#![allow(dead_code)]`、`CfgRef::Appearance` 与 `CARD_GAP` 的 `#[allow(dead_code)]` 已全部移除（相关变体/字段均已被 G/H 消费）。

---

## 1. 现状与目标

### 1.1 现状（代码事实）

设置页是**自绘 Win32 + Direct2D** 原生窗口，非 Web。模块 3075 行：

| 文件 | 行数 | 职责 |
|------|------|------|
| `crates/win/src/settings/mod.rs` | 1605 | 窗口过程、命中、dispatch、D2D 绘制 |
| `crates/win/src/settings/widget.rs` | 445 | 六种控件的 D2D 绘制 |
| `crates/win/src/settings/layout.rs` | 383 | 纯函数建页（`build_pages`） |
| `crates/win/src/settings/cfg_ref.rs` | 322 | 控件 ↔ Config 唯一映射表 |
| `crates/win/src/settings/state.rs` | 225 | 控件树 / 空间网格 / 交互状态机 |
| `crates/win/src/settings/tasks.rs` | 95 | 定时任务 |

窗口：`WS_POPUP`，`w = s(dpi, 420)`，`h = window_total_h(dpi)`（**取 6 页最高者**）。
导航：顶部**一个分段胶囊控件**，6 段等宽均分 388px。
内容：每页**一张卡片**，`caption` 在卡外上方，行 = `label + 若干控件`。
主题：`Theme::system()` 在**开窗那一刻**读注册表定死，之后不随系统变化。
无滚动、无滚轮、无键盘遍历。

### 1.2 目标

1. macOS Ventura+ 系统设置的信息架构：左侧边栏 + 右侧分组卡片，窗口可缩放。
2. 控件语义可读：滑杆绝对位置一眼可见，不再"拇指恒居中"。
3. 动效达到 Apple 流体标准：按下即反馈、弹簧而非固定时长、动画可中断、尊重"减弱动效"。
4. 信息架构收敛：个性化页从 16 行压到 5 张可折叠卡，默认零噪音。
5. 主题可实时切换，且设置窗与悬浮窗同步换肤。

---

## 2. 不变式与回归红线

> 本项目的 bug 史几乎全是"**显示对、写不回**"或"**两处度量不一致**"的静默错配。以下红线本轮绝不可触碰。

| # | 红线 | 出处 |
|---|------|------|
| R1 | 分段控件 `labels` 顺序必须与 `CfgRef` 索引映射**逐位对齐**。Style / NetMode / FontWeight 都栽过这个跟头（点「下载」写成「双向」）。 | `layout.rs` 240-252 行注释 |
| R2 | 滑杆必须有**真实上界**。旧版用 `f32::MAX/2` 做"无上限"，`rate × range × factor` 轻轻一碰爆到天文数字。 | `layout.rs` 35-40 行 |
| R3 | `CfgRef::set` 类型错配：debug `panic` / release 返回 `false`。**不得**退化为静默写值。 | `cfg_ref.rs` 211-218 行 |
| R4 | `Config` 序列化必须保持 **C 版兼容格式**（空格分隔、数值枚举、COLORREF、100ms 间隔）。新增键只能追加、不可改既有键语义。 | `config.rs` 442-521 行 |
| R5 | `Config::parse` **零 panic**，未知键静默忽略，`parse ∘ serialize` 必须幂等。 | `config.rs` 882-929 行测试 |
| R6 | 布局几何与文字绘制必须用**同一套度量**。位图等宽度量 vs D2D 比例字形的不一致曾导致 Net 行内缩 20px。 | `2026-09-11.md` 根因 2 |
| R7 | 新 UI 的坐标必须**统一经一个原点常量**换算，漏加会让整页内容盖住标题栏（2026-09-09 截图实证）。 | `mod.rs` 1538-1541 行注释 |
| R8 | 指标开关不得全部关闭（`metrics_all_off_after` 已有守卫），重排后此守卫必须继续生效。 | `mod.rs` 445 行 |

---

## 3. 范围边界（本轮不做）

- 不动常驻悬浮窗的渲染管线（`gdi.rs` / `render_d2d.rs` / `renderer.rs`），仅接受"主题联动"这一处干预。
- 不动右键菜单（`menu.rs`）。
- 不动指标采样（`perf.rs` / `proc_gpu.rs` / `sampler.rs`）。
- 不引入任何外部 crate（`Cargo.toml` 依赖不变）。
- 不做设置项的搜索/过滤。
- 不新增常驻线程或常驻定时器。

---

## 4. 设计规格冻结

### 4.1 窗口几何（逻辑 px，经 `s(dpi, v)` 缩放）

| 量 | 值 | 说明 |
|----|-----|------|
| 默认客户区 | 720 × 560 | 首次打开 |
| 最小客户区 | 560 × 460 | `WM_GETMINMAXINFO` 钳制 |
| 最大客户区 | 1400 × 1000 | 防止异常尺寸撑爆 DIB |
| 侧边栏宽 | 200 | 固定宽，不随窗口拉伸 |
| 侧边栏内边距 | 8 | 四周 |
| 侧边栏项高 | 34 | 圆角胶囊 |
| 侧边栏项间距 | 2 | |
| 内容区左右内边距 | 20 | 相对内容区左边界 |
| 内容区上内边距 | 20 | |
| 卡片圆角 | 10 | 沿用 |
| 卡片间距 | 16 | 新增（多卡时） |
| 行高 | 28 / 控件高 24 | 沿用 |
| 标签列宽 | 120 | 沿用 |

**坐标原点（R7 强制）**：侧边栏区 = `x ∈ [0, s(SIDEBAR_W))`，`y ∈ [0, client_h)`。
内容区 = `x ∈ [s(SIDEBAR_W), client_w)`，`y ∈ [0, client_h)`。

> ⚠️ 引入**双区域**后，`content_top_y(dpi)` 这个"单一事实源"必须升级为
> `content_origin(dpi, scroll_y) -> (i32, i32)`，返回内容区原点 `(SIDEBAR_W, -scroll_y)`。
> 所有 `Widget.rect` 仍存**内容坐标**（相对内容区左上角，不含滚动），
> 绘制与命中一律先加此原点。**禁止**任何地方直接加裸常量。

### 4.2 排版光学字号层级

现状只有两档：`body_px = 13pt`、`small_px = 11pt`。升级为四档（pt → px 经 `× dpi / 72`）：

| 角色 | pt | 用途 |
|------|-----|------|
| `display` | 20 | 侧边栏顶部标题 |
| `title` | 15 | 页面大标题（滚动时吸顶） |
| `body` | 13 | 行标签、按钮、值框 |
| `caption` | 11 | 分节标题、脚注、侧边栏项 |

### 4.3 精确字体族

Windows 无 SF Pro。按可用性降级探测，**一次性探测并缓存**：

```
"Segoe UI Variable Display" → "Segoe UI Variable Text" → "Segoe UI" → sys fallback
```

探测方式：`IDWriteFactory::GetSystemFontCollection` + `FindFamilyName`。
结果缓存进 `Gfx`（新增字段 `font_family: PCWSTR`-等效的 `Vec<u16>`）。
`gfx_d2d.rs::text_format` 第 306 行的 `w!("Segoe UI")` 改为读该缓存。

### 4.4 弹簧参数（取自 apple-design `spring-param-ref.md`）

Apple 官方值，映射公式 `ω₀ = 2π / response`、`k = ω₀²`、`c = 2ζω₀`（质量 = 1）：

| 交互 | 阻尼比 ζ | response (s) | k (stiffness) | c (damping) |
|------|---------|--------------|---------------|-------------|
| **常规 UI 过渡**（悬停、按压、选中态、外观切换） | 1.0 | 0.4 | 246.74 | 31.416 |
| **面板/抽屉类**（个性化卡展开、设置页翻页） | 0.8 | 0.3 | 438.65 | 33.510 |
| **手势抛动**（滚轮惯性、滑杆甩动） | 0.8 | — | 438.65 | 33.510 |

**动量投射**（滚轮惯性滚动）：

```rust
// 苹果官方公式：decelerationRate 0.998 普通滚动
pub fn project(initial_velocity: f32) -> f32 {
    (initial_velocity / 1000.0) * 0.998 / (1.0 - 0.998)
}
```

**橡胶边界**（越界阻尼，替代硬截断）：

```rust
// Apple 惯用的对数橡胶带：越界越多，位移增量越小
pub fn rubber_band(overshoot: f32, dimension: f32, coeff: f32) -> f32 {
    (1.0 - 1.0 / (overshoot * coeff / dimension + 1.0)) * dimension
}
```
`coeff` 取 0.55（Apple 默认）。

### 4.5 材质与层级

- 侧边栏底：比内容区**低一级**（macOS 侧栏更沉）。浅色 `rgb(246,246,248)`，深色 `rgb(44,44,46)`。
- 内容区底：浅色 `rgb(255,255,255)`，深色 `rgb(30,30,32)`。
- 卡片：浅色纯白 + 极淡描边 `rgb(222,222,227)`；深色 `rgb(54,54,58)`。
- 侧边栏选中态：胶囊填充 `accent` 12% alpha + 图标/文字用 `accent`；悬停态填充 `label` 8% alpha。
- **不做毛玻璃**：D2D 无 backdrop-filter，`WS_POPUP` 下自绘模糊无系统支持；
  强行软件模糊会破坏性能基线。按 apple-design「Web 能力受限处明确标注限制」，此处明确**放弃 vibrancy**，用分层纯色替代。

### 4.6 无障碍适配

| 需求 | Windows 实现 |
|------|--------------|
| 减弱动效 | `SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, …)`；为 `false` 时所有弹簧响应降为 `0.01s`、位移量归零（仅保留即时状态切换） |
| 高对比度 | `SystemParametersInfoW(SPI_GETHIGHCONTRAST, …)`；开启时卡片描边改用 `label` 全色、禁用半透明选中态 |
| 键盘可达 | 保留并扩展：`Tab` 遍历控件、`Space` 触发、`Esc` 关闭、`↑↓` 切页 |

> 现状已有 `WM_KEYDOWN` 分支（`mod.rs` 1175 行），本轮在其上扩展，不新建消息链路。

---

## 5. 任务分解

> 每个任务 2–5 分钟可完成。`文件:行` 为精确定位。
> 纯逻辑任务**必须**先写失败测试（红）再实现（绿）；纯绘制任务无单测，验证为编译通过 + 阶段末视觉核对。
> 每阶段末有一个**里程碑闸门**，未过不进下一阶段。

---

### 阶段 A — 配置层扩展（无 UI 改动，可独立验证）

#### 任务 A1：新增 `Appearance` 枚举
- 文件：`crates/core/src/types.rs`（追加，紧邻 `RenderEngine` 定义之后）
- 要做的：

```rust
/// 外观模式：跟随系统 / 强制浅色 / 强制深色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    /// 解析：接受数值（C 版风格）与字符串（可读风格）。
    pub fn from_config(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "0" | "system" | "auto" => Some(Appearance::System),
            "1" | "light" => Some(Appearance::Light),
            "2" | "dark" => Some(Appearance::Dark),
            _ => None,
        }
    }

    pub fn to_config(self) -> u32 {
        match self {
            Appearance::System => 0,
            Appearance::Light => 1,
            Appearance::Dark => 2,
        }
    }
}
```

- 在 `crates/core/src/lib.rs` 的 `types` 重导出处确认 `Appearance` 可经 `sysoverlay_core::types::Appearance` 访问（若 `types` 是 `pub mod`，无需改）。
- 验证：`cargo test -p sysoverlay-core` 通过；新增测试 `appearance_accepts_numeric_and_string` 断言 `from_config("1") == from_config("light")`。

#### 任务 A2：`Config` 增加 `appearance` 与 `settings_size`
- 文件：`crates/core/src/config.rs`
- 要做的（4 处）：

**(a)** 结构体末尾（`render_engine` 字段之后，149 行附近）追加：

```rust
    // ── 设置窗口（v5 新增） ──────────────────────────────────
    /// 外观模式：跟随系统 / 浅色 / 深色。
    pub appearance: Appearance,
    /// 设置窗口客户区尺寸（逻辑 px）。**独立于** `window_size`（悬浮窗几何），
    /// 因此新增 `set_size` 键而非复用 `size`——复用会让设置窗尺寸污染悬浮窗。
    pub settings_size: (i32, i32),
```

**(b)** `Default::default()`（198 行 `render_engine` 之后）追加：

```rust
            appearance: Appearance::System,
            settings_size: (720, 560),
```

**(c)** `parse()` 内，`"net_low_freq"` 分支之后（433 行附近）追加：

```rust
                "appearance" => {
                    if let Some(a) = Appearance::from_config(value) {
                        cfg.appearance = a;
                    }
                }
                // 设置窗口几何。独立键，避免与悬浮窗 `size` 互相污染。
                // 上限钳制：异常尺寸会让 D2D RT 分配失败 → 无法开窗。
                "set_size" => {
                    let mut it = value.split_whitespace();
                    if let (Some(w), Some(h)) = (it.next(), it.next()) {
                        if let (Ok(w), Ok(h)) = (w.parse::<i32>(), h.parse::<i32>()) {
                            if w >= 560 && h >= 460 && w <= 1400 && h <= 1000 {
                                cfg.settings_size = (w, h);
                            }
                        }
                    }
                }
```

**(d)** `serialize()` 内，`net_low_freq` 行之后（503 行附近）追加：

```rust
        // v5 设置窗口（C 版忽略未知键）
        s.push_str(&format!("appearance {}\n", self.appearance.to_config()));
        s.push_str(&format!("set_size {} {}\n", self.settings_size.0, self.settings_size.1));
```

- 验证：
  1. 新增测试 `appearance_and_set_size_roundtrip`：非默认值 `Appearance::Dark` + `(900, 640)` 经 `serialize → parse` 后相等。
  2. 新增测试 `set_size_out_of_range_rejected`：`set_size 100 100` / `set_size 9999 9999` 均回落到默认 `(720, 560)`。
  3. 在既有 `test_roundtrip_non_default`（705 行）里补两行非默认赋值，确保纳入该测试的覆盖。
  4. `cargo test -p sysoverlay-core` 全绿。

#### 任务 A3：`CfgRef` 增加 `Appearance`
- 文件：`crates/win/src/settings/cfg_ref.rs`
- 要做的（3 处）：

**(a)** 枚举 `SegmentedWidget` 段（`FontWeight` 之后，73 行附近）追加：

```rust
    /// 外观模式：0 = 跟随系统，1 = 浅色，2 = 深色。
    /// ⚠️ 顺序必须与 layout.rs 的 labels 顺序一致（红线 R1）。
    Appearance,
```

**(b)** `get()`（122 行 `FontWeight` 之后）追加：

```rust
            CfgRef::Appearance => Value::Select(cfg.appearance.to_config() as usize),
```

**(c)** `set()`（187 行 `FontWeight` 分支之后）追加：

```rust
            (CfgRef::Appearance, Value::Select(i)) => match i {
                0 => cfg.appearance = Appearance::System,
                1 => cfg.appearance = Appearance::Light,
                2 => cfg.appearance = Appearance::Dark,
                _ => return false,
            },
```

- 同时在文件顶部 `use` 行补 `Appearance`。
- 验证：在 `roundtrip_cases()`（233 行）补三条 `(CfgRef::Appearance, Value::Select(0|1|2))`；
  `cargo test -p sysoverlay-win cfg_ref` 全绿。

> **里程碑闸门 A**：`cargo test` 全绿 + `cargo build --release` 零警告。此时 UI 行为**完全未变**（新字段无消费者），可安全停下核对。

---

### 阶段 B — 主题热切换

#### 任务 B1：`Theme::resolve` 与新增语义色槽
- 文件：`crates/win/src/theme.rs`
- 要做的：

**(a)** `Theme` 结构体（70 行 `button_bg_hover` 之后）追加 4 个字段：

```rust
    /// 侧边栏底色（比内容区低一级）
    pub sidebar_bg: u32,
    /// 内容区底色
    pub content_bg: u32,
    /// 侧边栏项悬停填充
    pub sidebar_hover: u32,
    /// 侧边栏项选中填充
    pub sidebar_selected: u32,
```

**(b)** `LIGHT`（92 行 `button_bg_hover` 之后）追加：

```rust
    sidebar_bg: rgb(246, 246, 248),
    content_bg: rgb(255, 255, 255),
    sidebar_hover: rgb(0, 0, 0),        // 以 alpha 使用，见 paint
    sidebar_selected: rgb(0, 122, 255), // accent
```

**(c)** `DARK`（114 行之后）追加：

```rust
    sidebar_bg: rgb(44, 44, 46),
    content_bg: rgb(30, 30, 32),
    sidebar_hover: rgb(255, 255, 255),
    sidebar_selected: rgb(10, 132, 255),
```

**(d)** `impl Theme`（16 行 `system()` 之后）追加：

```rust
    /// 按外观模式解析主题。System 时读注册表，Light/Dark 强制。
    pub fn resolve(a: Appearance) -> Theme {
        match a {
            Appearance::System => Theme::system(),
            Appearance::Light => LIGHT,
            Appearance::Dark => DARK,
        }
    }
```

**(e)** 在既有测试 `test_light_dark_distinct_layers`（155 行）补断言：

```rust
        assert_ne!(LIGHT.sidebar_bg, DARK.sidebar_bg);
        assert_ne!(LIGHT.content_bg, DARK.content_bg);
        assert_eq!(Theme::resolve(Appearance::Dark).dark, true, "强制深色必须生效");
        assert_eq!(Theme::resolve(Appearance::Light).dark, false, "强制浅色必须生效");
```

- 验证：`cargo test -p sysoverlay-win theme` 全绿。

#### 任务 B2：外观切换时实时换肤
- 文件：`crates/win/src/settings/mod.rs`
- 要做的：

`SettingsCtx` 增加字段（原有 `theme: Theme` 保留，另加记录来源）：

```rust
    /// 上次生效的外观模式；变化时重新 resolve 主题并整帧重绘。
    last_appearance: sysoverlay_core::types::Appearance,
```

在 `paint_d2d()` 开头（1337 行 `let theme = ctx.theme;` **之前**）插入：

```rust
    // 实时换肤：外观设置变更（或系统主题变更时 last_appearance 为 System）立即重建主题。
    if let Some(cfg) = ctx.cfg() {
        let need = crate::theme::Theme::resolve(cfg.appearance);
        if need != ctx.theme {
            ctx.theme = need;
            ctx.tb = TitleBarState::default();
            unsafe { apply_window_chrome(hwnd, need.dark); }
            // 悬浮窗同步换肤（主题色派生自同一 Config）
            notify_overlay_reconfig(ctx);
        }
        ctx.last_appearance = cfg.appearance;
    }
```

> `SettingsCtx` 需 `PartialEq`——`Theme` 已 `derive(PartialEq)`，可直接比较，无需改 `SettingsCtx` 派生。

- 文件：`crates/win/src/settings/mod.rs` 的 `dispatch()`（456 行）中 `Action::Select` 分支：
  确认 `CfgRef::Appearance` 走**已有**的通用 `apply_config` 路径即可，
  **不要**新增特殊分支（红线：避免再次出现 Style/NetMode 式的静默错配）。
- 验证：
  1. `cargo build --release` 零警告。
  2. 手改配置 `appearance 1` 与 `appearance 2`，开窗即分别为浅/深（下一阶段补 UI 后再在页内切换核对）。

#### 任务 B3：悬浮窗接受主题联动
- 文件：`crates/win/src/settings/mod.rs` 的 `notify_overlay_reconfig()`（579 行）
- 要做的：**确认为无操作改动**。该函数当前已向 overlay 投递重配置消息；主题色派生自 `Config.bg/fg`，而 `appearance` 不影响悬浮窗配色（悬浮窗配色由用户自选 `bg`/`fg` 决定）。
  → 因此 `notify_overlay_reconfig` **不需要**因 appearance 而调用。删除 B2 中的该行调用，避免无谓重配置开销。
- 验证：确认 B2 补丁中不含 `notify_overlay_reconfig` 调用。

> **里程碑闸门 B**：`cargo test` 全绿；开窗后手动改注册表 `AppsUseLightTheme` 并重开窗，颜色随之变化。

---

### 阶段 C — 图标抽象层

#### 任务 C1：新建 `icons.rs` 与占位几何
- 文件：`crates/win/src/settings/icons.rs`（**新建**）
- 要做的：

```rust
//! 侧边栏图标：D2D 路径绘制。
//!
//! # 占位与替换
//! 当前为占位几何（几何基元拼出可辨识轮廓），待用户提供 SVG 后，
//! **只改本文件**：把 `draw()` 内各分支替换为 `ID2D1PathGeometry` 绘制，
//! 调用方（`paint_sidebar`）与签名均不变。
//!
//! # 尺寸契约
//! 图标绘制在边长 `i32 = rect.h()` 的正方形内，笔画宽 `stroke = rect.h() / 12`，
//! 所有坐标以 rect 的 0..1 归一化后乘以边长，保证任意 DPI 清晰。

use crate::gfx_d2d::{ColorF, Gfx, RectF};
use windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget;

/// 侧边栏导航项标识。顺序即侧边栏顺序，与 `Page` 下标一一对应。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconKind {
    General,
    Layout,
    Personalize,
    Window,
    Performance,
    About,
}

impl IconKind {
    pub const ALL: [IconKind; 6] = [
        IconKind::General,
        IconKind::Layout,
        IconKind::Personalize,
        IconKind::Window,
        IconKind::Performance,
        IconKind::About,
    ];
}

/// 在 `rect` 内以 `color` 绘制图标。`rect` 为正方形。
pub fn draw(
    rt: &ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    kind: IconKind,
    rect: RectF,
    color: ColorF,
) {
    let st = (rect.h() / 12.0).max(1.0);
    match kind {
        // 常规：齿轮简化 —— 中心圆环 + 四条辐条
        IconKind::General => {
            let c = rect_center(rect);
            let r = rect.h() * 0.30;
            gfx.stroke_circle(rt, c, r, color, st);
            for k in 0..4 {
                let ang = std::f32::consts::FRAC_PI_4 + k as f32 * std::f32::consts::FRAC_PI_2;
                let (s, co) = ang.sin_cos();
                gfx.line(rt, c.0 + co * r * 1.15, c.1 + s * r * 1.15,
                             c.0 + co * r * 1.75, c.1 + s * r * 1.75, color, st);
            }
        }
        // 布局：两行长短条
        IconKind::Layout => {
            let (l, t) = (rect.l + rect.w() * 0.18, rect.t + rect.h() * 0.28);
            let w = rect.w() * 0.64;
            gfx.line(rt, l, t, l + w, t, color, st);
            gfx.line(rt, l, t + rect.h() * 0.34, l + w * 0.6, t + rect.h() * 0.34, color, st);
        }
        // 个性化：三色圆点（强调"配色"语义）
        IconKind::Personalize => {
            for i in 0..3 {
                let x = rect.l + rect.w() * (0.26 + i as f32 * 0.24);
                gfx.fill_ellipse(
                    rt,
                    RectF::new(x - rect.h() * 0.11, rect.t + rect.h() * 0.39,
                               x + rect.h() * 0.11, rect.t + rect.h() * 0.61),
                    color,
                );
            }
        }
        // 窗口：矩形外框 + 顶栏横线
        IconKind::Window => {
            gfx.stroke_round(rt,
                RectF::new(rect.l + rect.w() * 0.18, rect.t + rect.h() * 0.22,
                           rect.r - rect.w() * 0.18, rect.b - rect.h() * 0.22),
                st * 1.5, color, st);
            gfx.line(rt, rect.l + rect.w() * 0.18, rect.t + rect.h() * 0.42,
                         rect.r - rect.w() * 0.18, rect.t + rect.h() * 0.42, color, st);
        }
        // 性能：上升折线 + 箭头
        IconKind::Performance => {
            let pts: [(f32, f32); 4] = [(0.18, 0.72), (0.38, 0.50), (0.58, 0.62), (0.82, 0.28)];
            for w in pts.windows(2) {
                gfx.line(rt,
                    rect.l + rect.w() * w[0].0, rect.t + rect.h() * w[0].1,
                    rect.l + rect.w() * w[1].0, rect.t + rect.h() * w[1].1,
                    color, st);
            }
        }
        // 关于：圆环 + 中心点（信息语义）
        IconKind::About => {
            let c = rect_center(rect);
            gfx.stroke_circle(rt, c, rect.h() * 0.32, color, st);
            gfx.fill_ellipse(rt,
                RectF::new(c.0 - rect.h() * 0.06, c.1 - rect.h() * 0.06,
                           c.0 + rect.h() * 0.06, c.1 + rect.h() * 0.06),
                color);
        }
    }
}

fn rect_center(r: RectF) -> (f32, f32) {
    ((r.l + r.r) / 2.0, (r.t + r.b) / 2.0)
}
```

- **依赖前置**：`Gfx` 目前**没有** `stroke_circle`。在 `gfx_d2d.rs` 的 `fill_ellipse`（431 行）之后追加：

```rust
    /// 描边圆（图标用；以轴对齐矩形外接椭圆）。
    pub fn stroke_circle(&mut self, rt: &ID2D1HwndRenderTarget, c: (f32, f32), r: f32, col: ColorF, w: f32) {
        if let Some(b) = self.brush(rt, col) {
            let rect = D2D1_ELLIPSE {
                point: D2D_POINT_2F { x: c.0, y: c.1 },
                radiusX: r,
                radiusY: r,
            };
            unsafe { rt.DrawEllipse(&rect, &b, w, None) };
        }
    }
```

- 在 `crates/win/src/settings/mod.rs` 顶部模块声明处补 `pub mod icons;`
- 验证：`cargo build -p sysoverlay-win` 通过（此时图标尚无调用方，会出现 dead_code 警告 → 在 `icons.rs` 顶部加 `#![allow(dead_code)]` 临时压制，阶段 D 接入后删除）。

#### 任务 C2：字体族降级探测
- 文件：`crates/win/src/gfx_d2d.rs`
- 要做的：

`Gfx` 结构体新增字段：

```rust
    /// 已探测的可用 UI 字体族（首次使用时探测并缓存）。
    ui_family: Option<Vec<u16>>,
```

新增方法：

```rust
    /// 探测可用 UI 字体族，按 Apple 观感优先级降级。
    /// 结果缓存——`GetSystemFontCollection` 是重操作，绝不可逐帧调用。
    fn ui_font_family(&mut self) -> Vec<u16> {
        if let Some(f) = &self.ui_family {
            return f.clone();
        }
        const CANDIDATES: [&str; 3] = [
            "Segoe UI Variable Text",
            "Segoe UI Variable",
            "Segoe UI",
        ];
        let mut chosen: Vec<u16> = "Segoe UI".encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            if let Ok(coll) = self.dwrite.GetSystemFontCollection(false) {
                for cand in CANDIDATES {
                    let w: Vec<u16> = cand.encode_utf16().chain(std::iter::once(0)).collect();
                    let mut idx = 0u32;
                    let mut exists = windows::Win32::Foundation::BOOL(0);
                    if coll.FindFamilyName(windows::core::PCWSTR(w.as_ptr()), &mut idx, &mut exists).is_ok()
                        && exists.as_bool()
                    {
                        chosen = w;
                        break;
                    }
                }
            }
        }
        self.ui_family = Some(chosen.clone());
        chosen
    }
```

把 `text_format()`（306 行）中的 `w!("Segoe UI")` 改为：

```rust
                    windows::core::PCWSTR(self.ui_font_family().as_ptr()),
```

> ⚠️ `text_format` 是 `&mut self`，借用在 `unsafe` 块内要先取出 `Vec<u16>` 到局部变量，
> 避免与 `self.dwrite` 的不可变借用冲突：
> ```rust
> let fam = self.ui_font_family();          // 先取
> let fam_w = windows::core::PCWSTR(fam.as_ptr());
> let fmt = unsafe { self.dwrite.CreateTextFormat(fam_w, …) };
> ```
- 验证：`cargo build --release` 零警告；标签文字字形在 Win11 上比原 Segoe UI 略窄（视觉核对，阶段 H）。

> **里程碑闸门 C**：`cargo build --release` 零警告。

---

### 阶段 D — 侧边栏骨架

#### 任务 D1：布局常量与双区域原点
- 文件：`crates/win/src/settings/layout.rs`
- 要做的：

**(a)** 常量区（16–40 行）改造：

```rust
// ── 布局常量（逻辑像素，按 DPI 缩放） ────────────────────

/// 客户区默认 / 最小 / 最大尺寸。
pub const DEFAULT_W: i32 = 720;
pub const DEFAULT_H: i32 = 560;
pub const MIN_W: i32 = 560;
pub const MIN_H: i32 = 460;
pub const MAX_W: i32 = 1400;
pub const MAX_H: i32 = 1000;

/// 侧边栏（固定宽，不随窗口拉伸）。
pub const SIDEBAR_W: i32 = 200;
pub const SIDEBAR_PAD: i32 = 8;
pub const SB_ITEM_H: i32 = 34;
pub const SB_ITEM_GAP: i32 = 2;
pub const SB_ICON: i32 = 16;
pub const SB_ICON_TEXT_GAP: i32 = 10;

/// 内容区。
pub const CONTENT_PAD_X: i32 = 20;
pub const CONTENT_PAD_TOP: i32 = 20;
pub const CARD_GAP: i32 = 16;

// 下列沿用：
pub const BOTTOM_MARGIN: i32 = 16;
pub const CARD_PAD: i32 = 10;
pub const CAPTION_H: i32 = 14;
pub const CAPTION_GAP: i32 = 6;
pub const ROW_H: i32 = 28;
pub const WIDGET_H: i32 = 24;
pub const CARD_RADIUS: i32 = 10;
pub const LABEL_W: i32 = 120;
pub const TOGGLE_W: i32 = 40;
pub const VALUEBOX_W: i32 = 64;
pub const FOOTNOTE_H: i32 = 60;
pub const SCROLLBAR_W: i32 = 3;
pub const SCROLLBAR_INSET: i32 = 4;
```

**删除**：`CLIENT_W`、`NAV_H`、`NAV_GAP`、`TOP_MARGIN`（被 `CONTENT_PAD_TOP` 取代）。

**(b)** 新的单一事实源（替换 `content_top_y`，109 行）：

```rust
/// 内容区原点（客户区坐标）：返回 (x, y)。
///
/// `x` = 侧边栏右边界；`y` = 内容滚动偏移的负值。
/// **所有** `Widget.rect` / `Card.rect` 存内容坐标，
/// 绘制与命中一律 `+ origin`，禁止裸加常量（红线 R7）。
pub fn content_origin(d: u32, scroll_y: i32) -> (i32, i32) {
    (s(d, SIDEBAR_W), -scroll_y)
}

/// 内容区可视尺寸（客户区坐标下的宽高）。
pub fn content_viewport(d: u32, client_w: i32, client_h: i32) -> (i32, i32) {
    ((client_w - s(d, SIDEBAR_W)).max(1), client_h)
}
```

**(c)** `window_total_h`（99 行）删除（窗口尺寸由 `Config.settings_size` + 用户拖拽决定，
`content_height` 改为"内容总高"，供滚动范围计算）：

```rust
/// 某页内容总高（含卡片、标题、脚注；**不含**可视区裁剪）。
pub fn content_height(dpi: u32, page: usize) -> i32 {
    let rows = page_rows(page);
    let foot = if page_has_footnote(page) { FOOTNOTE_H } else { 0 };
    s(dpi, CONTENT_PAD_TOP + BOTTOM_MARGIN)
        + s(dpi, CAPTION_H + CAPTION_GAP)
        + 2 * s(dpi, CARD_PAD)
        + s(dpi, rows * ROW_H)
        + s(dpi, foot)
}
```

**(d)** `build_pages` 签名改为接收客户区宽度（去掉对 `CLIENT_W` 的依赖）：

```rust
pub fn build_pages(
    pages: &mut [Page; 6],
    cfg: &Config,
    lang: Lang,
    dpi: u32,
    client_w: i32,
    metric_expanded: &[bool; 5],
) {
    let one = |v: i32| s(dpi, v);
    let card_l = one(CONTENT_PAD_X);
    let card_r = client_w - one(SIDEBAR_W) - one(CONTENT_PAD_X);
    // ⚠️ card_r 是内容坐标，故要减去侧边栏宽
    …
}
```

> 所有 `widget_l / widget_r` 的计算逻辑不变（仍相对 `card_l`/`card_r`）。

- 验证：`cargo build -p sysoverlay-win` 会报错（调用方签名不匹配）——这是**预期红**，
  在 D2/D3 修完。本任务只保证 `layout.rs` 自身内部自洽（`cargo check` 报错点全部集中在调用方）。

#### 任务 D2：`Page` 增加滚动与折叠态
- 文件：`crates/win/src/settings/state.rs`
- 要做的：

**(a)** `Page` 结构体（107 行）追加：

```rust
    /// 内容总高（DPI 缩放后，物理像素）——沿用 `h` 字段，语义不变。
    /// 滚动范围 = `h - viewport_h`，负值视为 0。
    pub scroll_y: i32,
```

**(b)** `Interaction` 枚举（16 行）追加按压态（Apple「按下即反馈」）：

```rust
    /// 已按下但未抬起（按压反馈用；抬起时若命中同一控件则触发动作）。
    Pressing { widget: usize },
```

**(c)** `Action` 枚举（71 行）追加：

```rust
    /// 展开/收起某指标的配置卡。
    ToggleMetricExpand(usize),
    /// 内容区滚动（滚轮 / 键盘 / 滚动条拖拽）。
    Scroll { page: usize, delta: i32 },
```

**(d)** `WidgetState`（60 行）追加过渡进度字段：

```rust
    /// 按压进度 0..1（弹簧驱动）。
    pub press: f32,
    /// 悬停进度 0..1（弹簧驱动）。
    pub hover_t: f32,
```

- 验证：`cargo check -p sysoverlay-win` 仅剩 `layout.rs` 调用方签名错误。

#### 任务 D3：`paint_sidebar` 替换 `paint_nav`
- 文件：`crates/win/src/settings/mod.rs`
- 要做的：

**(a)** `SettingsCtx` 新增字段：

```rust
    /// 各指标配置卡的展开态（UI 状态，不落盘）。
    metric_expanded: [bool; 5],
    /// 滚轮惯性的滚动速度（px/s），橡胶边界用。
    scroll_vel: f32,
```

**(b)** 删除 `paint_nav`（1487–1519 行），新增：

```rust
/// 侧边栏：底色 + 标题 + 6 个导航项（图标 + 文字 + 胶囊选中态）。
fn paint_sidebar(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    gfx: &mut Gfx,
    t: &Theme,
    dpi: u32,
    client_h: i32,
    lang: sysoverlay_core::types::Lang,
    page_idx: usize,
    hover: Option<usize>,
) {
    let w = s(dpi, SIDEBAR_W) as f32;
    // 底色（比内容区低一级）
    gfx.fill_rect(rt, RectF::new(0.0, 0.0, w, client_h as f32), c(t, t.sidebar_bg));

    // 标题（macOS 一体化：标题在侧边栏顶部）
    let title = crate::i18n::t(lang, "WinTitle");
    let tx = RectF::new(
        s(dpi, SIDEBAR_PAD + 6) as f32,
        s(dpi, 8) as f32,
        w - s(dpi, SIDEBAR_PAD) as f32,
        s(dpi, 8 + 30) as f32,
    );
    gfx.text(rt, title, display_px(dpi), 600, HAlign::Leading, tx, c(t, t.label), true);

    // 导航项
    let items_top = s(dpi, 8 + 30 + 12);
    for (i, kind) in icons::IconKind::ALL.iter().enumerate() {
        let top = items_top + s(dpi, i as i32 * (SB_ITEM_H + SB_ITEM_GAP));
        let r = RectF::new(
            s(dpi, SIDEBAR_PAD) as f32,
            top as f32,
            w - s(dpi, SIDEBAR_PAD) as f32,
            (top + s(dpi, SB_ITEM_H)) as f32,
        );
        if i == page_idx {
            // 选中：accent 12% 胶囊
            gfx.fill_round(rt, r, s(dpi, 8) as f32, alpha_of(c(t, t.sidebar_selected), 0.14));
        } else if hover == Some(i) {
            gfx.fill_round(rt, r, s(dpi, 8) as f32, alpha_of(c(t, t.sidebar_hover), 0.07));
        }
        let fg = if i == page_idx { c(t, t.accent) } else { c(t, t.secondary) };
        let icon_sz = s(dpi, SB_ICON);
        let ix = r.l + s(dpi, 10) as f32;
        let iy = r.t + (r.h() - icon_sz as f32) / 2.0;
        icons::draw(rt, gfx, *kind, RectF::new(ix, iy, ix + icon_sz as f32, iy + icon_sz as f32), fg);
        let lr = RectF::new(
            ix + icon_sz as f32 + s(dpi, SB_ICON_TEXT_GAP) as f32,
            r.t,
            r.r - s(dpi, 8) as f32,
            r.b,
        );
        gfx.text(rt, page_caption(i, lang), caption_px(dpi), 500, HAlign::Leading, lr, fg, true);
    }
}

/// 以给定 alpha 重着色。
fn alpha_of(c: ColorF, a: f32) -> ColorF {
    ColorF { r: c.r, g: c.g, b: c.b, a }
}
```

> `ColorF` 字段名以 `gfx_d2d.rs` 实际定义为准；若为私有字段，改为在 `ColorF` 上加
> `pub fn with_alpha(self, a: f32) -> Self`。
> `600` 字重需确认 `text_format` 的 `weight` 参数直通 `DWRITE_FONT_WEIGHT`（已直通，见 308 行）。

**(c)** 新增字号函数（1309 行附近，与 `body_px`/`small_px` 并列；`small_px` 重命名为 `caption_px` 以对齐 4.2 节命名）：

```rust
fn display_px(dpi: u32) -> f32 { 20.0 * dpi as f32 / 72.0 }
fn title_px(dpi: u32) -> f32 { 15.0 * dpi as f32 / 72.0 }
fn body_px(dpi: u32) -> f32 { 13.0 * dpi as f32 / 72.0 }
fn caption_px(dpi: u32) -> f32 { 11.0 * dpi as f32 / 72.0 }
```

**(d)** `paint_d2d`（1322 行）改造：

```rust
    let win_w = r.right - r.left;
    let win_h = r.bottom - r.top;
    …
    let page_idx = ctx.page;
    let scroll_y = pages[page_idx].scroll_y;
    let (ox, oy) = content_origin(dpi, scroll_y);
    let (vp_w, vp_h) = content_viewport(dpi, win_w, win_h);

    // 侧边栏（不随内容滚动）
    paint_sidebar(rt, gfx, &theme, dpi, win_h, lang, page_idx, nav_hover_of(ctx));

    // 内容区：先裁到视口，再平移
    let clip = RectF::new(ox as f32, oy as f32, (ox + vp_w) as f32, (oy + vp_h) as f32);
    unsafe { let _ = rt.PushAxisAlignedClip(&clip.d2d(), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE); }
    gfx.fill_rect(rt, clip, c(&theme, theme.content_bg));
    gfx.set_translate(rt, dx, oy as f32);
    paint_page(rt, gfx, pages, page_idx, &interaction, edit.as_ref(), cfg.as_ref(),
               &theme, dpi, dx, body_px(dpi), caption_px(dpi), slider_thumb, metric_expanded);
    gfx.reset_transform(rt);
    unsafe { rt.PopAxisAlignedClip() };
    paint_scrollbar(rt, gfx, &theme, dpi, win_w, win_h, scroll_y, pages[page_idx].h);
```

> ⚠️ `paint_page` 内部原本自己算 `ct = content_top_y(dpi)`（1541 行）。
> 本轮改为**由调用方传入 `oy`**，`paint_page` 不再自行计算——
> 否则双区域 + 滚动会产生两套原点，正是红线 R7 的情形。

**(e)** `paint_page` 签名末尾追加 `oy: i32, metric_expanded: &[bool; 5]`，内部所有
`… + ct` 改为 `… + oy as f32`（保留 `dx` 的水平偏移）。

**(f)** 新增滚动条绘制（overlay 自动隐藏：静止时不可见，滚动时淡入）：

```rust
/// 浮动滚动条：仅当内容溢出且最近滚动过时淡入。
fn paint_scrollbar(
    rt: &ID2D1HwndRenderTarget, gfx: &mut Gfx, t: &Theme, dpi: u32,
    win_w: i32, win_h: i32, scroll_y: i32, content_h: i32,
) {
    let vp_h = win_h;
    let overflow = content_h - vp_h;
    if overflow <= 0 { return; }
    let bw = s(dpi, SCROLLBAR_W) as f32;
    let inset = s(dpi, SCROLLBAR_INSET) as f32;
    let track_h = vp_h as f32 - inset * 2.0;
    let thumb_h = (track_h * vp_h as f32 / content_h as f32).max(24.0);
    let t0 = (-scroll_y as f32) / overflow as f32;   // 0..1
    let y = inset + (track_h - thumb_h) * t0.clamp(0.0, 1.0);
    let x = win_w as f32 - bw - inset;
    // 8% alpha 灰，不抢视线
    gfx.fill_round(rt, RectF::new(x, y, x + bw, y + thumb_h), bw / 2.0,
                   alpha_of(c(t, t.label), 0.22));
}
```

- 验证：`cargo check -p sysoverlay-win` 通过；`cargo build --release` 零警告。

#### 任务 D4：命中测试改为侧边栏
- 文件：`crates/win/src/settings/mod.rs`
- 要做的：

**(a)** 删除 `nav_at`（426 行），替换为：

```rust
/// 侧边栏导航项命中（客户区坐标 → 项下标）。
fn sidebar_at(dpi: u32, pt: ClientPt) -> Option<usize> {
    let x0 = s(dpi, SIDEBAR_PAD) as i32;
    let x1 = s(dpi, SIDEBAR_W) - s(dpi, SIDEBAR_PAD);
    if pt.x < x0 || pt.x >= x1 { return None; }
    let top = s(dpi, 8 + 30 + 12);
    let step = s(dpi, SB_ITEM_H + SB_ITEM_GAP);
    let rel = pt.y - top;
    if rel < 0 { return None; }
    let i = rel / step;
    if i >= 6 { return None; }
    // 落在项间距（gap）内视为未命中
    if rel % step >= s(dpi, SB_ITEM_H) { return None; }
    Some(i as usize)
}

/// 鼠标是否在内容区（用于滚轮与滚动条拖拽的归属判定）。
fn in_content(dpi: u32, client_w: i32, pt: ClientPt) -> bool {
    pt.x >= s(dpi, SIDEBAR_W) && pt.x < client_w
}
```

**(b)** `widget_at`（408 行）改造：先经 `content_origin` 换算，并**减去滚动偏移**：

```rust
fn widget_at(ctx: &SettingsCtx, pt: ClientPt, client_w: i32) -> Option<usize> {
    if !in_content(ctx.dpi, client_w, pt) { return None; }
    let page = &ctx.pages[ctx.page];
    let (ox, oy) = content_origin(ctx.dpi, page.scroll_y);
    let cpt = ContentPt { x: pt.x - ox, y: pt.y - oy };
    // 空间网格仍以内容坐标查询
    ctx.spatial_grids[ctx.page]
        .query(POINT { x: cpt.x, y: cpt.y })
        .iter()
        .copied()
        .find(|&i| state::pt_in(&page.widgets[i].rect, POINT { x: cpt.x, y: cpt.y }))
}
```

> ⚠️ 网格 `SpatialGrid::new` 的尺寸（`rebuild_pages` 204 行）要用**内容总高**而非视口高，
> 否则滚动到下半部分时命中全部失效。

**(c)** `WM_MOUSEWHEEL` 分支（当前**不存在**，新增）：

```rust
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) as i16) as i32; // 高字 = 滚轮增量，120 的倍数
            let page = &mut ctx.pages[ctx.page];
            let (_, vp_h) = content_viewport(ctx.dpi, client_w, client_h);
            let max = (page.h - vp_h).max(0);
            page.scroll_y = (page.scroll_y - delta / 120 * s(ctx.dpi, 48)).clamp(0, max);
            ctx.scroll_vel = 0.0;
            invalidate_settings();
            LRESULT(0)
        }
```

- 验证：`cargo build --release` 零警告；点击侧边栏各项能切页（阶段末视觉核对）。

#### 任务 D5：`rebuild_pages` 适配
- 文件：`crates/win/src/settings/mod.rs` 的 `rebuild_pages`（198 行）
- 要做的：

```rust
    fn rebuild_pages(&mut self) {
        if let Some(cfg) = self.cfg() {
            let (client_w, client_h) = self.client_size; // 新增字段，见阶段 E
            build_pages(&mut self.pages, &cfg, cfg.lang, self.dpi, client_w, &self.metric_expanded);
            // 网格按「内容总高」建（滚动后仍需全量命中）
            let mut grids = Vec::with_capacity(6);
            for page in &self.pages {
                let mut grid = state::SpatialGrid::new(client_w, page.h.max(s(self.dpi, 200)), s(self.dpi, 30));
                for (idx, w) in page.widgets.iter().enumerate() {
                    grid.insert(idx, w.rect);
                }
                grids.push(grid);
            }
            self.spatial_grids = grids;
            // 尺寸变化后重新钳制滚动位置，防止悬空
            for p in self.pages.iter_mut() {
                let max = (p.h - client_h).max(0);
                p.scroll_y = p.scroll_y.clamp(0, max);
            }
        }
    }
```

`SettingsCtx` 新增字段：`client_size: (i32, i32)`（在 `WM_SIZE` 更新）。

- 验证：`cargo build --release` 零警告。

> **里程碑闸门 D**：可开窗、见侧边栏、点各项能切页、内容区可滚轮滚动。此时**无标题栏按钮**（阶段 E 补）。

---

### 阶段 E — 窗口尺寸与 macOS 一体化标题

#### 任务 E1：窗口尺寸来自配置 + 拖拽 resize
- 文件：`crates/win/src/settings/mod.rs`
- 要做的：

**(a)** `create_dialog`（281 行）改尺寸来源：

```rust
        let (cfg_w, cfg_h) = window::get_config_of(overlay)
            .map(|c| c.settings_size)
            .unwrap_or((layout::DEFAULT_W, layout::DEFAULT_H));
        let w = s(dpi, cfg_w.clamp(layout::MIN_W, layout::MAX_W));
        let h = s(dpi, cfg_h.clamp(layout::MIN_H, layout::MAX_H));
```

**(b)** 窗口样式改为可拖拽（保持无边框自绘，resize 走 `WM_NCHITTEST`）：

```rust
            WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX,
```

> `WS_THICKFRAME` 提供系统 resize 边框与 `WM_NCCALCSIZE` 支持；
> 配合 `WM_NCHITTEST` 返回 `HTLEFT/HTRIGHT/HTTOP/HTBOTTOM/HT*CORNER` 实现自绘边框拖拽。
> 因 `WS_POPUP` 下无标题栏，不会出现系统标题栏视觉。

**(c)** `WM_GETMINMAXINFO` 分支（新增）：

```rust
        WM_GETMINMAXINFO => {
            let mmi = lparam.0 as *mut MINMAXINFO;
            if !mmi.is_null() {
                let (_, cw, ch) = frame_to_client(ctx.dpi, layout::MIN_W, layout::MIN_H);
                unsafe {
                    (*mmi).ptMinTrackSize = POINT { x: cw, y: ch };
                    (*mmi).ptMaxTrackSize = POINT {
                        x: layout::MAX_W * ctx.dpi as i32 / 96,
                        y: layout::MAX_H * ctx.dpi as i32 / 96,
                    };
                }
            }
            LRESULT(0)
        }
```

`frame_to_client` 需按当前样式算非客户区（`AdjustWindowRectEx`）：

```rust
/// 由目标客户区尺寸反推窗口外部尺寸。
fn frame_to_client(dpi: u32, cw_logical: i32, ch_logical: i32) -> (i32, i32, i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: s(dpi, cw_logical), bottom: s(dpi, ch_logical) };
    unsafe {
        let _ = AdjustWindowRectEx(&mut r, WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX, false, WINDOW_EX_STYLE(0));
    }
    (r.left, r.top, r.right - r.left, r.bottom - r.top)
}
```

**(d)** `WM_NCHITTEST` 分支（新增）—— 按客户区坐标判边框（

```rust
        WM_NCHITTEST => {
            let sx = (lparam.0 & 0xFFFF) as i16 as i32;
            let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            let g = s(ctx.dpi, 6); // 抓取热区
            let l = sx < wr.left + g;
            let r = sx >= wr.right - g;
            let t = sy < wr.top + g;
            let b = sy >= wr.bottom - g;
            LRESULT(match (l, r, t, b) {
                (true, _, true, _) => HTTOPLEFT as isize,
                (_, true, true, _) => HTTOPRIGHT as isize,
                (true, _, _, true) => HTBOTTOMLEFT as isize,
                (_, true, _, true) => HTBOTTOMRIGHT as isize,
                (true, _, _, _) => HTLEFT as isize,
                (_, true, _, _) => HTRIGHT as isize,
                (_, _, true, _) => HTTOP as isize,
                (_, _, _, true) => HTBOTTOM as isize,
                _ => HTCLIENT as isize,
            })
        }
```

> ⚠️ 顶部热区与"拖动窗口"冲突：自绘标题栏区域（侧边栏顶部）需返回 `HTCAPTION` 以支持拖动。
> 处理顺序：**先判按钮** → 再判顶部热区（仅最外 6px）→ 再判侧边栏空白 = `HTCAPTION` → 否则 `HTCLIENT`。

**(e)** `WM_SIZE` 分支（新增/确认存在）：更新 `ctx.client_size`、`ctx.gfx.resize(w, h)`、`invalidate_settings()`。

**(f)** `WM_EXITSIZEMOVE`（已存在，用于悬浮窗拖拽）中追加设置窗几何落盘：

```rust
                // 落盘设置窗几何（低优先级，退出拖拽时写一次）
                if let Some(mut cfg) = window::get_config_of(ctx.overlay) {
                    let (cw, ch) = ctx.client_size;
                    let lw = cw * 96 / ctx.dpi.max(1);
                    let lh = ch * 96 / ctx.dpi.max(1);
                    if (lw, lh) != cfg.settings_size {
                        cfg.settings_size = (lw.clamp(layout::MIN_W, layout::MAX_W),
                                             lh.clamp(layout::MIN_H, layout::MAX_H));
                        ctx.save.mark_dirty(cfg);   // 走既有防抖保存
                    }
                }
```

> `DebouncedSave` 的 `mark_dirty`/`flush` 接口以 `mod.rs` 实际定义为准（`ctx.save` 字段已存在）。

- 验证：`cargo build --release` 零警告；拖动窗口边缘可改变大小，重启后尺寸保持。

#### 任务 E2：`paint_titlebar` 改造为一体化标题
- 文件：`crates/win/src/settings/mod.rs`（`paint_titlebar`，1404 行）
- 要做的：

**(a)** 标题已移入 `paint_sidebar`（D3），故 `paint_titlebar` **只保留右上角窗口控制按钮**，重命名为 `paint_window_controls`：

```rust
/// 窗口控制按钮（右上角，浮于内容区之上，不占布局高度）。
fn paint_window_controls(
    rt: &ID2D1HwndRenderTarget, gfx: &mut Gfx, t: &Theme, dpi: u32,
    win_w: i32, tb: &TitleBarState,
) {
    if tb.pinned { … }   // 保留解锁按钮逻辑
    else { for (b, r) in titlebar::btn_rects(dpi, win_w, false) { … } }  // 逐字沿用原绘制
}
```

**(b)** `titlebar.rs::btn_rects`（81 行）无需改（仍按 `win_w` 从右往左排），
但 `BTN_MARGIN_R` 由 6 改为 10（对齐 macOS 更靠内），`TB_H`（25 行，值 30）不再作为内容偏移使用。

**(c)** 按钮区域不参与内容区裁剪：`paint_window_controls` 在 `PopAxisAlignedClip` **之后**调用。

- 验证：`cargo build --release` 零警告；按钮在右上角可见、可点、悬停高亮。

> **里程碑闸门 E**：窗口可拖拽缩放、尺寸记忆、标题在侧边栏顶、控制按钮在右上、内容可滚动。

---

### 阶段 F — 标准填充式滑杆

#### 任务 F1：值 ↔ 位置映射（TDD，纯函数）
- 文件：`crates/win/src/settings/widget.rs`
- 步骤 1（红）：先写测试

```rust
#[cfg(test)]
mod slider_tests {
    use super::*;

    #[test]
    fn t_maps_value_range_to_unit() {
        assert_eq!(slider_t_from_value(8.0, 8.0, 256.0), 0.0);
        assert_eq!(slider_t_from_value(256.0, 8.0, 256.0), 1.0);
        assert!((slider_t_from_value(132.0, 8.0, 256.0) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn t_clamps_out_of_range() {
        assert_eq!(slider_t_from_value(-100.0, 8.0, 256.0), 0.0);
        assert_eq!(slider_t_from_value(9999.0, 8.0, 256.0), 1.0);
    }

    #[test]
    fn value_from_x_snaps_to_step_and_clamps() {
        let track = RectF::new(100.0, 0.0, 300.0, 24.0);
        // 中点 → 132
        assert!((slider_value_from_x(200.0, track, 8.0, 256.0, 0.5) - 132.0).abs() < 1e-3);
        // 越左越右都夹在界内
        assert_eq!(slider_value_from_x(0.0, track, 8.0, 256.0, 0.5), 8.0);
        assert_eq!(slider_value_from_x(9999.0, track, 8.0, 256.0, 0.5), 256.0);
        // 步进吸附：0.5 步进不产生 132.3 这类值
        let v = slider_value_from_x(201.0, track, 8.0, 256.0, 0.5);
        assert!((v * 2.0).fract().abs() < 1e-6, "必须吸附到 0.5 步进，实际 {v}");
    }
}
```

步骤 2（绿）：实现

```rust
/// 值 → 归一化位置 0..1（钳制）。
pub fn slider_t_from_value(v: f64, min: f64, max: f64) -> f32 {
    if (max - min).abs() < f64::EPSILON { return 0.0; }
    (((v - min) / (max - min)).clamp(0.0, 1.0)) as f32
}

/// 轨道内 x 坐标 → 吸附步进后的值（钳制到 min..=max）。
pub fn slider_value_from_x(x: f32, track: RectF, min: f64, max: f64, step: f64) -> f64 {
    let t = ((x - track.l) / track.w().max(1.0)).clamp(0.0, 1.0) as f64;
    let raw = min + t * (max - min);
    if step <= 0.0 { return raw.clamp(min, max); }
    let snapped = ((raw - min) / step).round() * step + min;
    snapped.clamp(min, max)
}
```

- 验证：`cargo test -p sysoverlay-win slider` 全绿（先看红，再看绿）。

#### 任务 F2：滑杆绘制改为绝对位置 + 轨道填充
- 文件：`crates/win/src/settings/widget.rs` 的 `paint_slider`（368 行）
- 要做的：替换 `deflect` 相关逻辑（410–421 行）为：

```rust
    // 值 → 位置（绝对，不再用摇杆偏移）
    let t = slider_t_from_value(val, min as f64, max as f64);
    let cx = tr.l + t * tr.w();

    // 轨道：底色 + 已填充段（accent）
    let track_y = (tr.t + tr.b) / 2.0;
    let tw = super::layout::s(dpi, 2).max(1) as f32;
    gfx.line(rt, tr.l, track_y, tr.r, track_y, c(t, t.segmented_bg), tw);
    if cx > tr.l {
        gfx.line(rt, tr.l, track_y, cx, track_y, c(t, t.accent), tw);
    }

    // 拇指：随值移动（绝对定位）
    let thumb_d = super::layout::s(dpi, 13) as f32;
    let thumb_fill = if state.hovered || state.editing { c(t, t.accent_hover) } else { c(t, t.accent) };
    gfx.fill_ellipse(
        rt,
        RectF::new(cx - thumb_d / 2.0, track_y - thumb_d / 2.0,
                   cx + thumb_d / 2.0, track_y + thumb_d / 2.0),
        thumb_fill,
    );
```

**(b)** `paint_slider` 签名去掉 `deflect: f32`（改为 `hovered`/`press` 从 `state` 取），
`paint_widget`（206 行）同步去掉 `slider_deflect` 参数，
`paint_page` 与 `paint_d2d` 中的 `slider_thumb` 传给下游的链路整条删除。

**(c)** `SliderWidget` 的 `joystick: bool` 字段（106 行）**删除**（已废弃且现在无消费者），
同步删除 `layout.rs` 中 3 处构造参数。

- 验证：`cargo build --release` 零警告；`cargo test -p sysoverlay-win` 全绿。

#### 任务 F3：拖动语义改为绝对定位
- 文件：`crates/win/src/settings/mod.rs`
- 要做的：

**删除**：`slider_deflect_of`（600）、`slider_grab`（610）、`slider_move`（629）、`slider_cancel`（639）、`slider_release`（647）、`slider_tick`（659）——整套摇杆积分模型。

**删除 `SettingsCtx.fx: Option<SliderFx>` 字段**，以及 `state.rs` 的 `SliderFx` 结构体（51 行）、
`WM_TIMER` 中驱动 `slider_tick` 的分支、`WM_LBUTTONUP` 中的 `slider_release`。

**替换为**（在 `on_click_plain`（747 行）的滑杆分支中直接落值）：

```rust
    // 滑杆：按下即跳值 + 进入拖动，抬起提交（Apple「按下即反馈」）
    if let Some(s) = ctx.pages[ctx.page].widgets[idx].kind.as_any().downcast_ref::<SliderWidget>() {
        let (min, max, step, dp) = (s.min, s.max, s.step, s.dp);
        let r = ctx.pages[ctx.page].widgets[idx].rect;
        let tr = widget::track_rect(&r, ctx.dpi);
        let new = widget::slider_value_from_x(pt.x as f32, RectF::from_win32(tr), min as f64, max as f64, step as f64);
        dispatch(ctx, Action::LiveValue(s.cfg, new));
        ctx.interaction = Interaction::Dragging { widget: idx, origin: POINT { x: pt.x, y: pt.y } };
        let _ = dp;
        body_px(ctx.dpi); // 保持原分支结构
        return;
    }
```

`WM_MOUSEMOVE` 的 `Dragging` 分支改为直接按当前 x 落值（不再累积、不再看 `origin`）：

```rust
            Interaction::Dragging { widget } => {
                let idx = *widget;
                if let Some(s) = ctx.pages[ctx.page].widgets[idx].kind.as_any().downcast_ref::<SliderWidget>() {
                    let r = ctx.pages[ctx.page].widgets[idx].rect;
                    let tr = widget::track_rect(&r, ctx.dpi);
                    let new = widget::slider_value_from_x(pt.x as f32, RectF::from_win32(tr), s.min as f64, s.max as f64, s.step as f64);
                    dispatch(ctx, Action::LiveValue(s.cfg, new));
                }
                true
            }
```

`WM_LBUTTONUP`：`Dragging` → `dispatch(CommitValue)`，并回到 `Hover`。

> ⚠️ 值框（右端子矩形）在拖动时**不应**参与跳值——`track_rect` 已把值框排除在轨道外，
> 但按下若落在值框内应进入编辑态而非拖动。保持 `begin_edit` 的既有判定优先级：
> **先判值框 → 再判轨道**。

- 验证：`cargo build --release` 零警告；`cargo test` 全绿；`anim_ms` 字段此时只剩设置用途（无滑杆消费者）。

> **里程碑闸门 F**：滑杆拇指随值移动、左段填充、拖动与输入框均可用；无遗留 dead_code 警告。

---

### 阶段 G — 个性化页折叠卡

#### 任务 G1：折叠布局
- 文件：`crates/win/src/settings/layout.rs` 的 page 2 分支（262–349 行）
- 要做的：

**(a)** `page_rows(2)` 由 `16` 改为**动态**——折叠卡行数随展开态变化。
`page_rows` 保持纯函数但增加参数：

```rust
pub fn page_rows(page: usize, metric_expanded: &[bool; 5]) -> i32 {
    match page {
        0 => 4,  // 常规：语言/外观/自启/调试（外观新增）
        1 => 9,  // 布局：不变
        2 => {
            // 每指标：折叠 = 1 行（表头）；展开 = 4 行（表头 + 字号 + 名称色 + 数值色）
            let mut n = 0;
            for e in metric_expanded { n += if *e { 4 } else { 1 }; }
            n + 1 // +1 = 底部「全部重置」行
        }
        3 => 1,
        4 => 4,
        _ => 3,
    }
}
```

**(b)** page 2 重构为**每指标一张独立卡**（不再单卡 16 行）：

```rust
            } | 2 => {
                let metric_cfg: [CfgRef; 5] = [
                    CfgRef::ShowCpu, CfgRef::ShowMem, CfgRef::ShowGpu,
                    CfgRef::ShowNet, CfgRef::PowerEnabled,
                ];
                let labels: [&str; 5] = ["LblCpu", "LblMem", "LblGpu", "LblNet", "LblPower"];
                let mut cursor = 0;   // 全局行游标（跨卡连续，保证行距一致）
                for (i, lbl_key) in labels.iter().enumerate() {
                    // 每指标一张卡：卡内行从 0 起算，卡间加 CARD_GAP
                    push_card(page, tr(lbl_key), /* rows = */ if metric_expanded[i] { 4 } else { 1 },
                              /* gap_before = */ if i > 0 { CARD_GAP } else { 0 });
                    let base = cursor;
                    // 表头行：开关 + 折叠箭头 + 颜色预览
                    add_row(page, base, tr(lbl_key), vec![
                        (200 + i as i32 * 10, Box::new(ToggleWidget { cfg: metric_cfg[i] }),
                         (widget_l, widget_l + one(TOGGLE_W))),
                        (207 + i as i32 * 10, Box::new(DisclosureWidget {
                            expanded: metric_expanded[i],
                            action: ButtonAction::ToggleMetricExpand(i),
                         }), (widget_r - one(24), widget_r)),
                    ]);
                    if metric_expanded[i] {
                        // 字号 + 重置
                        add_row(page, base + 1, tr("LblFontSize"), vec![
                            (201 + i as i32 * 10, Box::new(SliderWidget {
                                min: 8.0, max: FONT_MAX, step: 0.5, dp: 1,
                                cfg: CfgRef::MetricFont(i),
                             }), (widget_l + one(TOGGLE_W + 8), widget_r - one(68))),
                            (202 + i as i32 * 10, Box::new(ButtonWidget {
                                label: tr("BtnReset").to_string(),
                                action: ButtonAction::ResetFont(i),
                             }), (widget_r - one(68), widget_r)),
                        ]);
                        add_row(page, base + 2, tr("LblNameColor"), vec![
                            (203 + i as i32 * 10, Box::new(MetricSwatchesWidget { cfg: CfgRef::MetricLabel(i) }),
                             (widget_l, widget_r - one(76))),
                            (204 + i as i32 * 10, Box::new(IconButtonWidget {
                                icon: IconKind::Reset,
                                action: ButtonAction::ResetColor(CfgRef::MetricLabel(i)),
                             }), (widget_r - one(24), widget_r)),
                        ]);
                        add_row(page, base + 3, tr("LblValueColor"), vec![
                            (205 + i as i32 * 10, Box::new(MetricSwatchesWidget { cfg: CfgRef::MetricValue(i) }),
                             (widget_l, widget_r - one(76))),
                            (206 + i as i32 * 10, Box::new(IconButtonWidget {
                                icon: IconKind::Reset,
                                action: ButtonAction::ResetColor(CfgRef::MetricValue(i)),
                             }), (widget_r - one(24), widget_r)),
                        ]);
                        cursor += 4;
                    } else {
                        cursor += 1;
                    }
                }
                // 独立的「全部重置」卡
                push_card(page, tr("LblResetAll"), 1, CARD_GAP);
                add_row(page, cursor, "", vec![(
                    209, Box::new(ButtonWidget {
                        label: tr("LblResetAll").to_string(),
                        action: ButtonAction::ResetAllColors,
                    }), full,
                )]);
            }
```

**(c)** 新增 `push_card` 辅助（与 `add_row` 并列，替换 143 行的单卡 push）：

```rust
        /// 压入一张卡（`gap_before` 为卡间空隙）。返回行起始下标。
        let mut push_card = |page: &mut Page, caption: &'static str, rows: i32, gap_before: i32| -> i32 {
            let top = rows_top + one(cursor_offset) + one(gap_before);
            …
        };
```

> ⚠️ `add_row` 的行位置基准从"单卡内行序"改为"页面全局行序"。
> 最简做法：把 `add_row` 的 `row_i` 语义改为**页面全局行号**（现已是 `base = i*3` 的全局语义），
> 卡矩形按该卡首末行的 y 范围反推：
> ```rust
> // 卡矩形 = 首行顶 - CARD_PAD .. 末行底 + CARD_PAD
> ```
> 这样 `content_height` 用 `rows * ROW_H` 仍成立，只是需按卡累加 `CARD_GAP`。
> 因此 `page_rows` 的返回值语义改为"**行数 + 卡间空隙行数**"，
> 由 `content_height` 计算总高时把 `CARD_GAP` 折算进去。

**(d)** i18n 补键：`mod.rs` 保留集合一致性检查（若有 `i18n.rs` 的测试枚举全键）。

- 验证：`cargo build --release` 零警告；`cargo test` 全绿（含 i18n key 一致性测试若有）。

#### 任务 G2：折叠箭头与图标按钮控件
- 文件：`crates/win/src/settings/widget.rs`
- 要做的：

**(a)** 新增两个控件：

```rust
/// 折叠箭头（展开 = 朝下，收起 = 朝右）。
pub struct DisclosureWidget {
    pub expanded: bool,
    pub action: ButtonAction,
}

/// 图标按钮（用于行尾「重置」等，悬停才显色）。
pub struct IconButtonWidget {
    pub icon: IconKind,
    pub action: ButtonAction,
}
```

**(b)** `ButtonAction` 追加（70 行）：

```rust
    /// 展开/收起指定指标的配置卡。
    ToggleMetricExpand(usize),
```

**(c)** `IWidget` 实现：`DisclosureWidget` / `IconButtonWidget` 的 `cfg()` 返回 `None`、
`sync()` 空实现（同 `ButtonWidget`）。

**(d)** `paint_widget`（206 行）补两个分支：

```rust
    } else if let Some(d) = w.kind.as_any().downcast_ref::<DisclosureWidget>() {
        paint_disclosure(rt, gfx, state, dpi, theme, r, d.expanded);
    } else if let Some(ib) = w.kind.as_any().downcast_ref::<IconButtonWidget>() {
        paint_icon_button(rt, gfx, state, dpi, theme, r, ib.icon);
    }
```

```rust
fn paint_disclosure(
    rt: &ID2D1HwndRenderTarget, gfx: &mut Gfx, state: &WidgetState,
    dpi: u32, t: &Theme, r: RectF, expanded: bool,
) {
    let color = if state.hovered { c(t, t.label) } else { c(t, t.secondary) };
    let cx = (r.l + r.r) / 2.0;
    let cy = (r.t + r.b) / 2.0;
    let k = super::layout::s(dpi, 4) as f32;
    let st = (super::layout::s(dpi, 1).max(1)) as f32 * 1.5;
    if expanded {
        // 朝下：v
        gfx.line(rt, cx - k, cy - k * 0.5, cx, cy + k * 0.5, color, st);
        gfx.line(rt, cx + k, cy - k * 0.5, cx, cy + k * 0.5, color, st);
    } else {
        // 朝右：>
        gfx.line(rt, cx - k * 0.5, cy - k, cx + k * 0.5, cy, color, st);
        gfx.line(rt, cx - k * 0.5, cy + k, cx + k * 0.5, cy, color, st);
    }
}

fn paint_icon_button(
    rt: &ID2D1HwndRenderTarget, gfx: &mut Gfx, state: &WidgetState,
    dpi: u32, t: &Theme, r: RectF, icon: IconKind,
) {
    // 悬停才给填充与显色：默认淡到接近隐形，消除 15 个「重置」按钮的噪音
    if state.hovered {
        gfx.fill_round(rt, r, super::layout::s(dpi, 6) as f32, c(t, t.button_bg_hover));
    }
    let color = if state.hovered { c(t, t.accent) } else { c(t, t.secondary) };
    let sz = r.h() * 0.55;
    let pad = (r.h() - sz) / 2.0;
    icons::draw(rt, gfx, icon, RectF::new(r.l + pad, r.t + pad, r.l + pad + sz, r.t + pad + sz), color);
}
```

**(e)** `icons.rs` 的 `IconKind` 追加 `Reset`（回环箭头占位：一段弧 + 箭头三角）
与 `Disclosure`（若需要独立绘制）。
使其定义同时服务于侧边栏与按钮——`IconKind::ALL` 只列侧边栏 6 项，绘制 match 覆盖全部。

- 验证：`cargo build --release` 零警告。

#### 任务 G3：折叠交互接线
- 文件：`crates/win/src/settings/mod.rs`
- 要做的：

**(a)** `dispatch()` 的 `ButtonAction` 派发处（870 行附近，`on_title_btn` 之后）新增：

```rust
                ButtonAction::ToggleMetricExpand(i) => {
                    ctx.metric_expanded[i] = !ctx.metric_expanded[i];
                    ctx.rebuild_pages();      // 行结构变化 → 必须重排
                    invalidate_settings();
                }
```

**(b)** `on_click_plain`（747 行）中新增 `DisclosureWidget` / `IconButtonWidget` 的命中分支
（与既有 `ButtonWidget` 分支同构，从 widget 里取 `action` 后 `dispatch`）。

**(c)** `Action::Scroll` 在 `dispatch` 中处理（更新对应页 `scroll_y` 并钳制）。

- 验证：`cargo build --release` 零警告；展开/收起后卡片重排正确、滚动范围同步（视觉核对）。

> **里程碑闸门 G**：个性化页默认 5 行、点箭头展开、重置按钮悬停才显色、底部独立「全部重置」。

---

### 阶段 H — Apple 流体动效

#### 任务 H1：弹簧积分器与减弱动效探测（TDD，纯函数）
- 文件：`crates/win/src/settings/motion.rs`（**新建**）
- 步骤 1（红）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_converges_to_target() {
        let mut s = Spring::new(0.0);
        for _ in 0..240 { s.step(1.0, 1.0 / 120.0, K_UI, C_UI); }
        assert!((s.x - 1.0).abs() < 0.01, "1s 后应到目标，实际 {}", s.x);
    }

    #[test]
    fn critical_damping_never_overshoots() {
        // ζ = 1.0 临界阻尼：不得越过目标
        let mut s = Spring::new(0.0);
        let mut max = 0.0f32;
        for _ in 0..480 {
            s.step(1.0, 1.0 / 240.0, K_UI, C_UI);
            max = max.max(s.x);
        }
        assert!(max <= 1.001, "临界阻尼不得过冲，峰值 {max}");
    }

    #[test]
    fn panel_spring_slightly_overshoots() {
        // ζ = 0.8：允许小幅过冲（Apple sheet 手感）
        let mut s = Spring::new(0.0);
        let mut max = 0.0f32;
        for _ in 0..480 {
            s.step(1.0, 1.0 / 240.0, K_SHEET, C_SHEET);
            max = max.max(s.x);
        }
        assert!(max > 1.0 && max < 1.15, "ζ=0.8 应小幅过冲，峰值 {max}");
    }

    #[test]
    fn rubber_band_is_compressive() {
        // 越界越多，位移增量越小（二阶差分递减）
        let d1 = rubber_band(10.0, 400.0, 0.55);
        let d2 = rubber_band(20.0, 400.0, 0.55);
        assert!(d2 > d1, "越界位移应单调增");
        assert!(d2 - d1 < d1, "增量必须递减（橡胶感）");
    }

    #[test]
    fn projected_endpoint_grows_with_velocity() {
        assert!(project(2000.0) > project(1000.0));
    }
}
```

步骤 2（绿）：

```rust
//! Apple 流体动效：弹簧积分、动量投射、橡胶边界。
//!
//! 参数取自 apple-design `spring-param-ref.md`（Apple WWDC 官方值）：
//! 常规 UI ζ=1.0 / response=0.4s；面板与手势 ζ=0.8 / response=0.3s。
//! 映射：ω₀ = 2π / response，k = ω₀²，c = 2ζω₀（质量 = 1）。

/// 常规 UI 过渡（悬停 / 按压 / 选中 / 换肤）：ζ=1.0，response=0.4s。
pub const K_UI: f32 = 246.74;
pub const C_UI: f32 = 31.416;

/// 面板与手势（折叠展开 / 翻页 / 滚轮惯性）：ζ=0.8，response=0.3s。
pub const K_SHEET: f32 = 438.65;
pub const C_SHEET: f32 = 33.510;

/// 减弱动效时的响应（≈ 瞬时）。
pub const K_REDUCED: f32 = 4000.0;
pub const C_REDUCED: f32 = 200.0;

/// 定步长子步长：帧率无关且数值稳定。
const DT: f32 = 1.0 / 240.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct Spring {
    pub x: f32,
    pub v: f32,
}

impl Spring {
    pub fn new(x: f32) -> Self { Self { x, v: 0.0 } }

    /// 推进 `frame_dt` 秒；返回 true 表示已收敛（可停止重绘）。
    pub fn step(&mut self, target: f32, frame_dt: f32, k: f32, c: f32) -> bool {
        let mut left = frame_dt.clamp(0.0, 0.1);
        while left > 0.0 {
            let dt = left.min(DT);
            // 半隐式欧拉（先更新速度再更新位置）——比显式欧拉稳定
            let a = k * (target - self.x) - c * self.v;
            self.v += a * dt;
            self.x += self.v * dt;
            left -= dt;
        }
        (target - self.x).abs() < 0.002 && self.v.abs() < 0.02
    }

    pub fn set(&mut self, x: f32) { self.x = x; self.v = 0.0; }
}

/// Apple 官方动量投射（decelerationRate = 0.998）。
pub fn project(initial_velocity: f32) -> f32 {
    (initial_velocity / 1000.0) * 0.998 / (1.0 - 0.998)
}

/// 橡胶边界：越界 `overshoot` 时实际可移动的距离（对数压缩）。
pub fn rubber_band(overshoot: f32, dimension: f32, coeff: f32) -> f32 {
    if overshoot <= 0.0 || dimension <= 0.0 { return 0.0; }
    (1.0 - 1.0 / (overshoot * coeff / dimension + 1.0)) * dimension
}

/// 查询系统是否启用客户端区动画（「减弱动效」）。
/// `SPI_GETCLIENTAREAANIMATION`：false 时应跳过所有弹簧过渡。
pub fn animations_enabled() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION};
    static CACHED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
    let c = CACHED.load(std::sync::atomic::Ordering::Relaxed);
    if c >= 0 { return c == 1; }
    let mut on = windows::Win32::Foundation::BOOL(1);
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut on as *mut _ as *mut std::ffi::c_void),
            Default::default(),
        ).is_ok()
    };
    let v = if ok && !on.as_bool() { 0 } else { 1 };
    CACHED.store(v, std::sync::atomic::Ordering::Relaxed);
    v == 1
}

/// 高对比度模式（卡片描边全色、禁用半透明）。
pub fn high_contrast() -> bool {
    use windows::Win32::UI::Accessibility::{SystemParametersInfoW, HIGHCONTRASTW, HCF_HIGHCONTRASTON, SPI_GETHIGHCONTRAST};
    let mut hc = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, ..Default::default() };
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            std::mem::size_of::<HIGHCONTRASTW>() as u32,
            Some(&mut hc as *mut _ as *mut std::ffi::c_void),
            Default::default(),
        ).is_ok()
    };
    ok && (hc.dwFlags & HCF_HIGHCONTRASTON) != 0
}
```

> ⚠️ `SystemParametersInfoW` 在 `windows` crate 里是 **feature-gated**。
> `SPI_GETCLIENTAREAANIMATION` 属 `Win32_UI_WindowsAndMessaging`，`SPI_GETHIGHCONTRAST`
> 属 `Win32_UI_Accessibility`。检查 `crates/win/Cargo.toml` 的 `windows` features，
> 缺哪个补哪个（**这是本轮唯一可能的 Cargo.toml 改动**）。
> `HIGHCONTRASTW` 在 `windows` crate 中可能叫 `HIGHCONTRASTW`，
> 编译报错则以实际导出名称为准。

- 验证：`cargo test -p sysoverlay-win motion` 全绿（5 个测试）。

#### 任务 H2：按压与悬停过渡
- 文件：`crates/win/src/settings/mod.rs`、`widget.rs`
- 要做的：

**(a)** `WidgetState` 增加 `Spring` 承载的过渡进度（`state.rs`）：

```rust
    /// 悬停过渡进度（弹簧驱动，0..1）。
    pub hover_t: f32,
    /// 按压过渡进度（弹簧驱动，0..1）。
    pub press_t: f32,
```

**(b)** `SettingsCtx` 增加过渡状态存储：

```rust
    /// 逐控件过渡状态（下标 = 当前页 widget 下标），与弹簧一起演进。
    transitions: Vec<Transition>,
```

```rust
#[derive(Default, Clone, Copy)]
struct Transition {
    hover: motion::Spring,
    press: motion::Spring,
}
```

**(c)** `paint_page` 中，逐控件先推进弹簧再绘制：

```rust
            for &wi in &row.widgets {
                let target_hover = if matches!(interaction, Interaction::Hover { widget } if *widget == wi) { 1.0 }
                    else if matches!(interaction, Interaction::Pressing { widget } if *widget == wi) { 1.0 } else { 0.0 };
                let target_press = if matches!(interaction, Interaction::Pressing { widget } if *widget == wi) { 1.0 } else { 0.0 };
                let tr = &mut transitions[wi];
                let (k, cst) = if motion::animations_enabled() { (motion::K_UI, motion::C_UI) }
                               else { (motion::K_REDUCED, motion::C_REDUCED) };
                if tr.hover.step(target_hover, dt, k, cst) { /* 收敛，无需额外重绘 */ }
                tr.press.step(target_press, dt, k, cst);
                ws.hover_t = tr.hover.x;
                ws.press_t = tr.press.x;
                ws.hovered = target_hover > 0.5;
                …
            }
```

> 收敛后仍需在**下一帧**确认停止：由一个 `needs_anim_frame()` 聚合判断驱动
> `InvalidateRect` 的持续重绘；全部收敛则不再请求新帧（**避免空转烧 CPU**）。

**(d)** `widget.rs` 各绘制函数把 `state.hovered` 的硬切换改为按 `hover_t` 插值：

```rust
/// 按进度在两色间线性插值（过渡用）。
pub fn lerp_color(a: ColorF, b: ColorF, t: f32) -> ColorF {
    let t = t.clamp(0.0, 1.0);
    ColorF { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t,
             b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }
}
```

例：`paint_button`（298 行）

```rust
    let fill = lerp_color(c(t, t.accent), c(t, t.accent_hover), state.hover_t);
    // 按压时轻微缩放（0.98），体现「按下即反馈」
    let s = 1.0 - 0.02 * state.press_t;
    let cx = (r.l + r.r) / 2.0;
    let cy = (r.t + r.b) / 2.0;
    let rr = RectF::new(cx - r.w() / 2.0 * s, cy - r.h() / 2.0 * s,
                        cx + r.w() / 2.0 * s, cy + r.h() / 2.0 * s);
    gfx.fill_round(rt, rr, super::layout::s(dpi, 7) as f32, fill);
```

同法改 `paint_toggle`（旋钮位置插值）、`paint_segmented`（选中片位置插值）、
`paint_swatches`（选中描边淡入）、`paint_icon_button`。

**(e)** 按下即进入 `Pressing`：`WM_LBUTTONDOWN` 命中控件时设 `Interaction::Pressing`，
`WM_LBUTTONUP` 时若仍命中同一控件则执行动作（Apple「按下即反馈，抬起才确认」）。

- 验证：`cargo build --release` 零警告；鼠标扫过控件时高亮平滑淡入（视觉核对）。

#### 任务 H3：翻页弹簧化 + 可中断
- 文件：`crates/win/src/settings/mod.rs` 的 `anim_params`（230 行）
- 要做的：把三次方缓出（235 行）替换为弹簧：

```rust
/// 翻页过渡：弹簧驱动的水平位移 + 淡入。
/// 可中断：切换目标页时不清空动画，仅更新目标（弹簧从当前位置继续）。
fn anim_params(ctx: &mut SettingsCtx, dt: f32) -> (f32, f32) {
    let target_page = ctx.page;
    // 目标位移方向：向新页滑入 24px，到位后归零
    let (k, c) = if motion::animations_enabled() { (motion::K_SHEET, motion::C_SHEET) }
                 else { (motion::K_REDUCED, motion::C_REDUCED) };
    let settled = ctx.page_slide.step(0.0, dt, k, c);
    let alpha = ctx.page_fade.step(1.0, dt, k, c);
    if settled { ctx.page_anim = None; }
    let _ = target_page;
    (ctx.page_slide.x, alpha.then(1.0).max(ctx.page_fade.x))
}
```

> 具体实现要点：
> - `ctx.page_slide: Spring` 初始 `x = 24.0`（新页从右侧滑入），目标 `0.0`。
> - `ctx.page_fade: Spring` 初始 `0.0`，目标 `1.0`。
> - **切页时不重置弹簧**，只把 `page` 改掉并令 `page_slide.v += 速度移交量`——
>   即 Apple 的"动画随时可打断，从屏幕实时位置开始新动画"。
> - `alpha` 的装配以上面代码为准：`Spring` 没有 `then()`，改为 `let alpha = ctx.page_fade.x;`

**(b)** `paint_d2d` 中 `anim_params(ctx)` 改为传入本帧 `dt`（由 `ctx.clock0` 计算，
或在 `paint_d2d` 开头用 `Instant::now()` 与上次绘制时间的差）。

- 验证：`cargo build --release` 零警告；快速连点侧边栏，动画从中间位置接续而非跳回（视觉核对）。

#### 任务 H4：滚轮惯性与橡胶边界
- 文件：`crates/win/src/settings/mod.rs`
- 要做的：

**(a)** `WM_MOUSEWHEEL` 记录速度（D4 中已置 0），改为：

```rust
            // 记录瞬时速度供动量投射（滚动停止时按 Apple 公式算吸附终点）
            ctx.scroll_vel = -(delta as f32) / 120.0 * s(ctx.dpi, 48) as f32 * 12.0;
```

**(b)** 新增 `SettingsCtx.scroll_spring: Spring`；在 `WM_TIMER`（已存在的定时器）中推进：

```rust
    // 滚动惯性：动量投射 → 吸附终点 → 弹簧趋近
    if ctx.scroll_vel.abs() > 1.0 {
        let projected = motion::project(ctx.scroll_vel);
        let page = &mut ctx.pages[ctx.page];
        let (_, vp_h) = content_viewport(ctx.dpi, ctx.client_size.0, ctx.client_size.1);
        let max = (page.h - vp_h).max(0);
        // 目标 = 当前 + 投射距离，按行高吸附，钳制到有效范围
        let raw = (page.scroll_y as f32 + projected).clamp(0.0, max as f32);
        let snapped = (raw / s(ctx.dpi, ROW_H) as f32).round() * s(ctx.dpi, ROW_H) as f32;
        let target = snapped.clamp(0.0, max as f32);
        let (k, c) = (motion::K_SHEET, motion::C_SHEET);
        let settled = ctx.scroll_spring.step(target, dt, k, c);
        page.scroll_y = ctx.scroll_spring.x.round() as i32;
        if settled { ctx.scroll_vel = 0.0; }
        invalidate_settings();
    }
```

**(c)** 橡胶边界：`WM_MOUSEWHEEL` 越界时不再硬 `clamp`，而是用 `rubber_band` 给出可视位移
（视觉上滚到边界还能再挪一点然后回弹）：

```rust
            let raw = page.scroll_y - delta / 120 * s(ctx.dpi, 48);
            let max = (page.h - vp_h).max(0);
            page.scroll_y = if raw < 0 {
                -motion::rubber_band(-raw as f32, vp_h as f32, 0.55) as i32
            } else if raw > max {
                max + motion::rubber_band((raw - max) as f32, vp_h as f32, 0.55) as i32
            } else { raw };
```

> 松手/停止后由 `scroll_spring` 把橡胶位移弹回边界内。

- 验证：`cargo build --release` 零警告；滚轮快速滚动有惯性并吸附到行、到边界有回弹（视觉核对）。

> **里程碑闸门 H**：动效流畅、可中断、无空转重绘（`probe.log` 中静止时不再持续出帧）。

---

### 阶段 I — 收尾

#### 任务 I1：光学字号与视觉细节同步
- 文件：`crates/win/src/settings/mod.rs`（`paint_page`）、`widget.rs`
- 要做的：
  1. 卡片分节标题由 `caption_px` 保持 11pt；页面大标题（若加）用 `title_px(dpi)` 15pt。
  2. 侧边栏项用 `caption_px` 11pt + 字重 500；侧边栏标题用 `display_px` 20pt + 字重 600。
  3. 卡片**不加投影**（D2D 无 shadow 原语，软阴影需大量绘制，与放宽后的预算仍不划算）；
     改用「描边 + 底色分层」表达深度（4.5 节已定）。
  4. 高对比度模式：`motion::high_contrast()` 为 true 时，
     卡片描边改用 `label` 全色、侧边栏选中态不用半透明而用实心 `accent`。

- 验证：`cargo build --release` 零警告。

#### 任务 I2：i18n 新键与一致性
- 文件：`crates/win/src/i18n.rs`
- 要做的：追加键（En / Zh 各一条）：

| key | En | Zh |
|-----|-----|-----|
| `LblAppearance` | Appearance | 外观 |
| `SegAppearanceSystem` | System | 跟随系统 |
| `SegAppearanceLight` | Light | 浅色 |
| `SegAppearanceDark` | Dark | 深色 |
| `BtnExpand` | Expand | 展开 |
| `BtnCollapse` | Collapse | 收起 |

- 并确认 `LblResetAll` 既可用于行标签也可用于按钮文案（现状已如此）。
- 验证：`cargo test -p sysoverlay-win i18n`（若有全键一致性测试）全绿；`cargo build --release` 零警告。

#### 任务 I3：死代码清理
- 文件：多处
- 要做的：

| 删除对象 | 位置 |
|----------|------|
| `paint_nav` | `mod.rs` 1487 行 |
| `content_top_y` | `layout.rs` 109 行（被 `content_origin` 取代） |
| `window_total_h` | `layout.rs` 99 行 |
| `CLIENT_W` / `NAV_H` / `NAV_GAP` / `TOP_MARGIN` | `layout.rs` 18–23 行 |
| `SliderFx` | `state.rs` 51 行 |
| `SliderWidget.joystick` | `widget.rs` 106 行 |
| `slider_deflect_of` / `slider_grab` / `slider_move` / `slider_cancel` / `slider_release` / `slider_tick` | `mod.rs` 600–695 行 |
| `SettingsCtx.fx` | `mod.rs` 347 行附近 |
| `icons.rs` 顶部 `#![allow(dead_code)]` | 阶段 C 的临时压制，此时删除 |
| `paint_slider` 的 `min`/`max` 未使用抑制（`let _ = min;`） | `widget.rs` 382–383 行（F2 后二者已真正被使用） |

- 验证：`cargo build --release` **零警告**（本项目对 unused 零容忍）；`cargo test` 全绿。

#### 任务 I4：全量验证
- 命令：

```bash
cd sysoverlay
cargo build --release
cargo test
```

- 验收标准：
  1. `cargo build --release` 0 errors / 0 warnings。
  2. `cargo test` 全绿（当前基线 170 项 + 本轮新增 ≈ 20 项）。
  3. `Config` 往返不变式测试仍通过（红线 R4/R5）。
  4. `cfg_ref` 全变体 roundtrip 仍通过（红线 R1/R3），含新增 `Appearance` 三条。

---

## 6. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| **双区域坐标系引入后画错位置**（内容盖住侧边栏 / 滚动后命中全错） | 高 | 高 | 强制单一 `content_origin`；`rebuild_pages` 的网格按**内容总高**建；阶段 D 闸门即验收命中 |
| **`WS_THICKFRAME` 引入系统边框视觉** | 中 | 中 | `WS_POPUP` 下一律走 `WM_NCCALCSIZE` 全客户区；阶段 E 目视确认无系统边框 |
| **弹簧导致空转重绘烧 CPU** | 中 | 中 | 弹簧 `step` 返回收敛标志；全控件收敛后停止 `InvalidateRect`；阶段 H 闸门用 `probe.log` 判定 |
| **图标占位几何观感差，被误认为最终稿** | 高 | 低 | `icons.rs` 文件头明确标注"占位"；替换只改一个文件 |
| **`windows` crate feature 缺失导致 `SPI_*` 编译失败** | 中 | 低 | 允许本轮唯一一次 `Cargo.toml` features 追加；否则用 `GetSystemMetrics` 降级 |
| **个性化页折叠改变了 `page_rows` 语义，高度算错** | 中 | 中 | `page_rows` 增参后必须同步改 `content_height`；加一条 `content_height >= rows * ROW_H` 的护栏测试 |
| **换肤时 `Theme` 比较引入每帧开销** | 低 | 低 | `Theme` 是 `Copy` + 12 个 u32，比较极廉价；无需缓存 |

---

## 7. 交付顺序与依赖

```
A（配置层）──┬─→ B（主题热切换）
             └─→ C（图标层）──→ D（侧边栏骨架）──→ E（尺寸 + 一体化标题）
                                                        │
                        G（个性化折叠卡）←── F（填充式滑杆）
                                │
                                └─→ H（流体动效）──→ I（收尾）
```

- A 必须先做（B/C/D/G 都消费新配置字段）。
- C 必须先于 D（侧边栏要画图标）与 G2（行尾重置图标）。
- F 与 E 可并行（无共享文件冲突：F 主要动 `widget.rs`，E 动 `mod.rs` 的消息分支）。
- H 最后做（它要给 F/G 的控件加过渡，早做会返工）。
- 阶段 A 完成后 UI **行为零变化**，是最安全的检查点。

---

## 8. 需要你提供的东西

| 项 | 用途 | 时机 |
|----|------|------|
| 6 个侧边栏图标的 SVG | 替换 `icons.rs` 占位几何（翻译为 D2D 路径） | 阶段 C 之后任意时刻，替换只改一个文件 |
| 1 个「重置」图标 SVG | 行尾重置按钮（同上） | 随上一并给 |

> 图标规范建议：24×24 viewBox、单色（`currentColor`）、**仅用 path**（避免 `<mask>`/`<filter>`/渐变，
> 这类 D2D 需额外实现）。填充与描边分离给出更好——描边图标用 `stroke-width="2"` 时，
> 我按 `rect.h()/12` 换算笔画宽。

---

## 9. 关于「超轻量」的一次坦诚说明

本项目对外主打「75 KB / 7 MB / 恒定 4 个 GDI 对象」。本计划中被打分放宽的**只有设置窗口**——
它是按需打开的独立窗口，与常驻悬浮窗的生命周期完全不同。因此：

- 常驻悬浮窗的基线（文件体积、私有内存、GDI/USER 对象数、CPU 占用）**不受影响**。
- 设置窗口新增的代码量估计 **+1200 ～ +1500 行**（`icons.rs` ≈ 180、`motion.rs` ≈ 150、
  `widget.rs` +250、`mod.rs` +400、`layout.rs` +200、其余为配置与状态），
  对应二进制增量估计 **+8 ～ 15 KB**。
- 若 README 的「~75 KB」需要保持字面准确，阶段 I 结束后需重新测量并更新该数字
  （**建议**：在 README 中区分「悬浮窗」与「含设置窗」两个体积口径，比改一个数字更诚实）。
