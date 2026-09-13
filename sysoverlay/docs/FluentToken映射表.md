# S1 Fluent Token 映射表

> 本文档是 S1 的**交接物**（`指令包`），供低智模型直接执行 token 值转录。
> 由高智模型（我）预先设计两套体系的取舍与映射决策，低智模型只照表填值。

---

## 1. 映射决策（高智模型产出）

### 决策摘要

| 决策 | 结论 | 理由 |
|---|---|---|
| Token 体系 | **保留现有 Theme（约20字段），Fluent token 作为 `fluent_tokens.rs` 新模块补充** | 现有 Theme 已有 20 字段直接对应 20 个绘图调用点；Fluent ~100 token 若全部替换 Theme 会导致 5× 代码膨胀 |
| 融合方式 | **按需引用**——当 Fluent 组件（switch/slider/button）需要额外 token 时，从 `fluent_tokens.rs` 取值；现有绘制管线继续用 Theme | 符合「直接融合进代码即可」的原则，不做 trait 抽象 |
| 字号体系 | **废弃 layout.rs 的 `body_px/caption_px` 手算，改用 Fluent font token** | Fluent font_size_base300 (14) ≈ 现有 body_px(13)；统一字阶减少手算 |
| 间距体系 | **保留 layout.rs 的 4px 刻度（4/8/12/16/24）** | 现有布局已精确到 4px；Fluent spacing token 与 4px 刻度有明确对应关系（见下表），直接映射而不重建 |

---

## 2. Theme → Fluent Token 映射表

### 2.1 颜色映射

| 现有 Theme 字段 | Fluent Token (Light) | Fluent Token (Dark) | 备注 |
|---|---|---|---|
| `window_bg` | `color_neutral_background1` | `color_neutral_background1` | 明暗共名，值不同 |
| `card` | `color_neutral_background1` (同上) | `color_neutral_background1` | 窗口背景=卡片色（Fluent 无独立 card 色） |
| `card_stroke` | `color_neutral_stroke2` | `color_neutral_stroke2` | 极淡分隔线 |
| `label` | `color_neutral_foreground1` | `color_neutral_foreground1` | 主文字 |
| `secondary` | `color_neutral_foreground2` | `color_neutral_foreground2` | 次级文字 |
| `accent` | `color_brand_background` | `color_brand_background` | 主强调色（按钮/链接） |
| `accent_hover` | `color_brand_background_hover` | `color_brand_background_hover` | 按钮悬停 |
| `toggle_on` | `color_compound_brand_background` | `color_compound_brand_background` | switch 打开填充 |
| `toggle_off` | `color_neutral_background3` | `color_neutral_background3` | switch 关闭底色 |
| `toggle_off_stroke` | `color_neutral_stroke_accessible` | `color_neutral_stroke_accessible` | switch 关闭描边 |
| `segmented_bg` | `color_neutral_background3` | `color_neutral_background3` | 分段控件底色 |
| `seg_selected` | `color_neutral_background1` | `color_neutral_background6` | 选中片（浅色用白，深色用深灰） |
| `seg_selected_stroke` | `color_neutral_stroke1` | `color_neutral_stroke2` | 选中片描边 |
| `knob` | `color_neutral_foreground_inverted` | `color_neutral_foreground_inverted` | switch 旋钮=白色 |
| `knob_stroke` | `color_neutral_stroke1` | `color_neutral_stroke2` | 旋钮描边 |
| `button_bg` | `color_neutral_background3` | `color_neutral_background3` | 次级按钮底 |
| `button_bg_hover` | `color_neutral_background3_hover` | `color_neutral_background3_hover` | 次级按钮悬停 |
| `sidebar_bg` | `color_neutral_background_alpha` | `color_neutral_background_alpha` | 侧边栏底色（带透明度） |
| `content_bg` | `color_neutral_background1` | `color_neutral_background1` | 内容区底色 |

### 2.2 间距映射（layout.rs 常量 → Fluent Token）

| layout.rs 常量 | Fluent Token | 值（Light/Dark 相同） | 备注 |
|---|---|---|---|
| `ROW_H = 34` | `spacing_vertical_l` + 内边距微调 | 16+? | **有冲突**：Fluent spacing 最小粒度 4px，34 不是 4 的倍数。<br>**决策**：控件带高度保留 `ROW_H=34`，Fluent token 用于控件**内部**间距 |
| `ROW_GAP = 12` | `spacing_vertical_m` | 8 | 有冲突，保留 12 |
| `CARD_PAD = 16` | `spacing_horizontal_l` | 16 | ✓ 完全一致 |
| `CAPTION_GAP = 8` | `spacing_vertical_s` | 8 | ✓ 完全一致 |
| `SIDEBAR_PAD = 8` | `spacing_horizontal_s` | 4 | **冲突**：sidebar_pad 用 8，Fluent s=4。<br>**决策**：侧边栏保持 8，不强制对齐 Fluent |
| `CONTENT_PAD_X = 20` | `spacing_horizontal_xl` | 16 | **冲突**：内容区内边距 20 > Fluent xl=16。<br>**决策**：内容区保留 20 |
| `HINT_EXTRA = 22` | `line_height_base200` ≈ 22 | 22 | ✓ 字阶一致 |
| `CARD_GAP = 24` | `spacing_horizontal_xxl` | 20 | **冲突**：卡片间距 24 > Fluent xxl=20。<br>**决策**：卡片间距保留 24 |

### 2.3 字号映射（S5 执行时已修订，见决策记录）

| layout.rs 函数 | Fluent Token | 96dpi 值 | 备注 |
|---|---|---|---|
| `body_px(dpi)` → 原 13pt(17.33px) | `font_size_base400` | 16 | **S5 修订**：原定 base300(14) 破坏纵向节奏不变式，改取 base400 |
| `caption_px(dpi)` → 原 11pt(14.67px) | `font_size_base300` | 14 | **S5 修订**：同步上调，配合 HINT_EXTRA 22→20 |
| 分组标题 | `font_size_base400` | 16 | 用 caption_px 位阶 + 600 字重 |

> **决策修订记录（S5，2026-09-12）**：原映射 body→base300(14)/caption→base200(12)
> 与"ROW_H=34 保留"决策冲突——纵向节奏不变式（测试 `vertical_rhythm_includes_the_divider`）
> 要求 body+caption = ROW_H+HINT_EXTRA−24，14+12=26 ≠ 32，强行执行必须连动重排
> 滑杆行/折叠头/滚轮步长。修订为 base400/base300 + HINT_EXTRA 22→20，
> 不变式保持成立（9+3=12），改动面收敛为 3 个常量。

### 2.4 圆角与边框

| 现有常量 | Fluent Token | 值 | 备注 |
|---|---|---|---|
| `CARD_RADIUS = 10` | `border_radius_medium` | 4 | **严重冲突**：现有 10，Fluent 中=4。<br>**决策**：卡片圆角**以 Fluent 为准改为 4**，记录于 S5 统一修改 |
| 控件圆角（按钮等） | `border_radius_small` | 2 | ✓ 一致 |

---

## 3. `fluent_tokens.rs` 新增 Token（Fluent 专用，Theme 不含）

> ⚠️ **色值/数值的唯一真值 = Quel'Thalas `qt/src/theme.rs` 的 `web_light()` / `web_dark()`**。
> 下表数值已与源码逐值核对（2026-09-12 高智审阅修正版）。禁止凭"印象中的 Fluent"填值
> ——首轮转录曾混入 Fluent 1 的 `#0078D4` 与 Material Design 的 `#4FC3F7`，已由测试锁定拦截。

```
// 颜色（浅色 ← web_light / 深色 ← web_dark）
color_neutral_background1          // #ffffff / #292929
color_neutral_background3          // #f5f5f5 / #141414（dark 控件底比背景更暗！）
color_neutral_background6          // #e6e6e6 / #333333
color_brand_background             // #0f6cbd / #115ea3（不是 Fluent1 #0078D4，不是 Material #4FC3F7）
color_brand_background_hover       // #115ea3 / #0f6cbd
color_brand_background_pressed     // #0c3b5e / #0c3b5e
color_compound_brand_background    // #0f6cbd / #479ef5（switch 填充）
color_neutral_foreground_inverted  // #ffffff / #242424（dark 旋钮=深灰，不是白！）
color_neutral_stroke_accessible    // #616161 / #adadad（switch 关闭描边）
color_neutral_stroke1              // #d1d1d1 / #666666（light 不是黑，dark 不是白！）
color_neutral_stroke2              // #e0e0e0 / #525252
color_subtle_background_hover      // #f5f5f5 / #383838

// 尺寸
stroke_width_thin: 1.0
border_radius_medium: 4.0          // 卡片圆角 ← 现有 10 → S5 统一改
border_radius_small: 2.0

// 字体
font_family_base: "Segoe UI"
font_weight_regular: 400
font_weight_semibold: 600
font_size_base200: 12.0
font_size_base300: 14.0
font_size_base400: 16.0
line_height_base200: 16.0          // ← 修正：首轮误写 22.0（那是 base400 的值）
line_height_base300: 20.0

// 间距（ Quel'Thalas 原值；vertical_m=12 恰与 layout ROW_GAP=12 一致，无冲突）
spacing_horizontal_s: 8.0          // ← 修正：首轮误写 4.0（那是 xs 的值）
spacing_horizontal_m: 12.0
spacing_horizontal_l: 16.0         // = 现有 CARD_PAD
spacing_vertical_s: 8.0            // = 现有 CAPTION_GAP
spacing_vertical_m: 12.0           // = 现有 ROW_GAP
spacing_vertical_l: 16.0

// 动效（WAM 真值，durations 以秒计）
duration_faster: 0.1               // ← 修正：首轮误写 0.050
duration_normal: 0.2               // ← 修正：首轮误写 0.167
curve_easy_ease: [0.33, 0.0, 0.67, 1.0]        // ← 修正：首轮误写 [0.16,1,0.3,1]
curve_decelerate_mid: [0.0, 0.0, 0.0, 1.0]     // ← 修正：首轮误写 [0.8,0,0.8,1]
```

---

## 4. 文件变更清单（S1 执行时）

| 文件 | 动作 | 内容 |
|---|---|---|
| `settings/fluent/tokens.rs` | **新建** | `FluentTokens { light, dark }` 结构体 + 全部 token 值 |
| `settings/mod.rs` | **修改** | 在 `Theme` 旁引入 `use fluent_tokens::*`，`paint_*` 调用处按需引用 |
| `settings/theme.rs` | **修改** | 注释掉被 Fluent 替代的字段（toggle_on/off/knob 等）并注明「已迁移至 fluent_tokens」 |
| `settings/layout.rs` | **修改** | `body_px`/`caption_px` 改用 Fluent font_size；`CARD_RADIUS` 从 10 改为 4（**S5 统一处理**） |

---

## 5. 验收命令

```bash
# 1. 编译通过
cargo build -p aaxsyspulse-win 2>&1 | head -50

# 2. 测试全部通过（特别是 slider_tests）
cargo test -p aaxsyspulse-win --lib 2>&1 | tail -20

# 3. S0 基线截图对比（人工）
# 对比 docs/baseline/ 与当前渲染输出，确认颜色/间距/圆角符合映射表
```

---

## 6. 红线（低智模型禁止修改）

- ❌ `motion.rs`（弹簧系统不变）
- ❌ `scrollbar.rs`（坐标不变式不变）
- ❌ `widget.rs` 的 `paint_widget` 匹配逻辑（替换绘制代码但保留 dispatch）
- ❌ `cfg_ref.rs`（单向数据流不变）
- ❌ `state.rs`（状态机不变）
