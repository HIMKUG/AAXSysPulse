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

### 2.3 字号映射

| layout.rs 函数 | Fluent Token | 96dpi 值 | 备注 |
|---|---|---|---|
| `body_px(dpi)` → 13pt | `font_size_base300` | 14 | 差 1pt，**决策**：统一改为 14（font_size_base300） |
| `caption_px(dpi)` → 11pt | `font_size_base200` | 12 | 差 1pt，**决策**：统一改为 12（font_size_base200） |
| 分组标题 | `font_size_base400` | 16 | **新增**：Fluent subtitle |

### 2.4 圆角与边框

| 现有常量 | Fluent Token | 值 | 备注 |
|---|---|---|---|
| `CARD_RADIUS = 10` | `border_radius_medium` | 4 | **严重冲突**：现有 10，Fluent 中=4。<br>**决策**：卡片圆角**以 Fluent 为准改为 4**，记录于 S5 统一修改 |
| 控件圆角（按钮等） | `border_radius_small` | 2 | ✓ 一致 |

---

## 3. `fluent_tokens.rs` 新增 Token（Fluent 专用，Theme 不含）

以下 Fluent token 在现有 Theme 中无对应项，需在 `settings/fluent/tokens.rs` 新建：

```
// 颜色
color_neutral_background1          // 窗口/卡片底（白/深灰）
color_neutral_background3          // 控件底（灰/深灰）
color_neutral_background6          // 深色选中片底
color_brand_background            // 品牌色（蓝）
color_brand_background_hover      // 品牌悬停
color_brand_background_pressed    // 品牌按下
color_compound_brand_background   // switch 填充色
color_neutral_foreground_inverted  // 白色（旋钮）
color_neutral_stroke_accessible   // switch 关闭描边
color_neutral_stroke1             // 描边 1
color_neutral_stroke2             // 描边 2（分隔线）
color_subtle_background_hover     // 按钮悬停背景

// 尺寸
stroke_width_thin: 1.0            // 边框宽
border_radius_medium: 4.0         // 卡片圆角 ← 注意：现有 10 → S5 统一改
border_radius_small: 2.0          // 控件圆角

// 字体
font_family_base: "Segoe UI"
font_weight_regular: 400
font_weight_semibold: 600
font_size_base200: 12.0           // ← 现有 caption_px(96dpi) ≈ 11，统一改 12
font_size_base300: 14.0           // ← 现有 body_px(96dpi) ≈ 13，统一改 14
font_size_base400: 16.0           // 分组标题
line_height_base200: 22.0
line_height_base300: 20.0

// 间距（按 layout.rs 4px 刻度校准后的值）
spacing_horizontal_s: 4.0         // ← 侧边栏项内间距（Fluent 原值 4）
spacing_horizontal_m: 12.0        // ← 内容区内边距 20 > Fluent xl=16，保留 20？不，改为 12
spacing_horizontal_l: 16.0        // 卡片内边距 ← 等于现有 CARD_PAD=16 ✓
spacing_vertical_s: 8.0             // ← 等于现有 CAPTION_GAP ✓
spacing_vertical_m: 8.0            // ← 等于现有 ROW_GAP=12？冲突，保留 12
spacing_vertical_l: 16.0

// 动效
duration_faster: 0.050            // 50ms（WAM 等效）
duration_normal: 0.167             // 167ms（WAM 等效）
curve_easy_ease: [0.16, 1.0, 0.3, 1.0]  // Fluent 标准曲线
curve_decelerate_mid: [0.8, 0.0, 0.8, 1.0]
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
cargo build -p sysoverlay-win 2>&1 | head -50

# 2. 测试全部通过（特别是 slider_tests）
cargo test -p sysoverlay-win --lib 2>&1 | tail -20

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
