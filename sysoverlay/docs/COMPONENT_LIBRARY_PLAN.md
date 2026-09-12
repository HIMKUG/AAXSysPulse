# 设置页组件库化计划（COMPONENT LIBRARY PLAN）

> 状态：**待确认**（本文档是计划，不是变更记录；确认后按 S0→S10 执行）
> 前置分析：Quel'Thalas 动画库评价结论见 §5；动效缺口清单见 §3。

---

## 0. 问题定义

现状是「支离破碎的手写控件」：

| 碎片化点 | 现状 |
|---|---|
| 巨石绘制 | `settings/mod.rs` ~3080 行（wndproc + 状态机 + 全量绘制），`widget.rs` ~1083 行（10 种控件的绘制散在一堆 `paint_*` 函数里） |
| 控件无边界 | 10 种控件（Button / Disclosure / MetricFontStatus / MetricHeader / MetricSwatches / Segmented / Slider / Stepper / Swatches / Toggle）没有统一的命中/绘制/动效契约，靠 `downcast_ref` 大 match 串起来 |
| 状态与视图耦合 | `widget.rs` 的控件直接持有 `CfgRef`（读 Config），UI 层绑死了业务配置层 |
| 动效归属错位 | 每控件的三根弹簧（hover/press/val）定义在 `mod.rs::Transition`，推进逻辑写在 `paint_page` 里 |
| token 分裂 | 颜色在 `theme.rs`、间距/字号/尺寸在 `layout.rs`，两处各管一半 |
| 动效缺口 | 滚动条热态、导航悬停、toast、主题切换、窗口入场全是离散硬切（评价结论，见 §3） |

**目标**：设置页 = 「小型组件库（crates/ui）+ 组合根（settings/）」，控件可复用、可独立测试、动效内聚于组件。

## 1. 不做什么（红线）

- ❌ **不采用 Quel'Thalas 的子 HWND 模型**（每控件一个子窗口）。与单窗口自绘、统一状态机、空间网格命中、卡片拖拽排序根本冲突，且丢失全部已有打磨。
- ❌ **不引入 WAM / COM 动画**。现有弹簧系统可中断、有动量/橡胶带，能力是超集（详见 §5）。
- ❌ **不大爆炸重写**。绞杀式渐进，每步可交付、可回退、有测试。
- ❌ **不做过度抽象**。只有 ≥2 个消费者的东西才进库（ponytail 原则）。

## 2. 目标架构

```
crates/
  ui/                        ← 新 workspace 成员：组件库
    src/
      tokens.rs              颜色/圆角/间距/字号 token（theme.rs + layout.rs 常量合并，单一真值）
      theme.rs               Theme 结构 + 浅/深 token 表 + ThemeLerp（主题交叉渐变，P2 落点）
      motion.rs              弹簧内核 + 动效参数（现 settings/motion.rs 平移，含测试）
      anim.rs                AnimState{hover_t, press_t, val_t} + 时钟协议 trait Clock
      icon.rs                内嵌 SVG path 图标集（现 icons.rs 平移 + 按 §5 吸收 Fluent 图标）
      components/
        mod.rs               Component trait + HitZone + 布局求解（RowSpec/CardSpec → rects）
        toggle.rs  segmented.rs  slider.rs  stepper.rs  button.rs
        swatches.rs  disclosure.rs
        scrollbar.rs         现 settings/scrollbar.rs 平移（它已是库形态：几何唯一真值）
        card.rs              分组卡 / 行 / 分隔线 / hint 排版
        sidebar.rs           导航 + 选中胶囊 + 悬停弹簧
        toast.rs             状态提示（入离场动画）
  win/src/settings/          ← 组合根（变薄）
    mod.rs                   wndproc + 状态机 Interaction + dispatch + 动画主时钟
    pages.rs                 页面装配：Config/CfgRef → 组件 props（解耦层，替代控件直持 CfgRef）
```

**依赖方向（编译期锁死）**：`ui` 不依赖 `win` / `core` 的 Config / CfgRef / i18n（文案由组合根传入 `String`）。`win` → `ui` 单向。

## 3. 动效方案（组件库的第一批验收项）

动效语言**保持 Apple 弹簧**（不混 Fluent bezier，不引入第二套方言）。组件库化之后，把评价发现的 5 处离散硬切作为验收项：

| # | 缺口 | 现状 | 方案 | 落点 |
|---|---|---|---|---|
| M1 | 滚动条热态 | alpha 0.55↔0.80、宽 12↔16 瞬跳 | 热态弹簧驱动 alpha+宽度插值 | `components/scrollbar.rs` |
| M2 | 导航悬停 | 高亮底瞬现瞬失 | y 位置弹簧 + alpha 弹簧（复用 nav_sel 模式） | `components/sidebar.rs` |
| M3 | toast | 出现/消失硬切 | 淡入+8px 上滑入场；到期先淡出再清除 | `components/toast.rs` |
| M4 | 主题切换 | 浅/深全帧硬切 | 250ms 主题快照交叉渐变：`ThemeLerp::between(from, to, t)`，paint 统一取插值 token，组件无感知 | `ui/theme.rs` |
| M5 | 窗口入场 | `SW_SHOW` 无过渡 | 首帧淡入 + 12px 上滑（复用 page_fade 弹簧模式） | `mod.rs` 组合根 |

组件库化**之前**不做这些（先换地基，再装修）。

## 4. 组件模型（核心抽象）

受控组件语义（状态在宿主，组件纯展示 + 命中）：

```rust
/// 组件 = 布局 + 命中 + 绘制；不知道 Config、不知道 Action。
trait Component {
    fn layout(&self, ctx: &LayoutCtx, c: Constraint) -> Size;
    fn hit_test(&self, pt: Point) -> Option<HitZone>;     // HitZone: Body / Thumb / Minus / Plus / Seg(i) / Swatch(i)…
    fn paint(&self, rt: &RenderTarget, gfx: &mut Gfx, ctx: &PaintCtx,
             props: &Props, anim: &mut AnimState);
}
```

- **Props 是纯值**（`f64` / `usize` / `bool` / `String`），组合根在 `pages.rs` 里做 `Config → Props` 映射。现控件直持 `CfgRef` 的耦合被切掉。
- **事件上行**：wndproc 命中 → `HitZone` → 组合根 dispatch `Action`（现单向数据流不变，`apply_config` 唯一写入口不变）。
- **AnimState 内聚**：三根弹簧从 `mod.rs::Transition` 移入库内；`anim::Clock` trait 由组合根用现有 ANIM_TIMER(16ms)+timeBeginPeriod 实现——**时钟协议不变**，防 40fps 空转的收敛判定逻辑随之平移。

## 5. 从 Quel'Thalas 吸收什么（评价结论）

**结论：不引入为依赖，不迁移引擎。** 理由：

1. 架构不兼容：它每控件一个子 HWND + 各自 D2D；本应用是单窗口全量自绘 + 空间网格。引入 = 重写全部交互层。
2. 动画能力是子集：WAM bezier 过渡中断时速度清零（其源码自注 "Initial velocity is ignored"）；本应用弹簧可中断续接，且有动量投射、橡胶带、减弱动效探测——这些它都没有。
3. 两种动效方言不可混用：代码库已有成文的 Apple 弹簧论题，混入 Fluent bezier 会让「有的跟手有的不跟手」。

**吸收清单（正当收益）**：
- ✅ 一控件一文件、anim 与绘制同模块的**组织方式**（本计划的 `components/` 即此）
- ✅ MIT 的 Fluent 图标 SVG path：按需补齐 icons.rs 缺的（chevron / checkmark / copy / dismiss 等）
- ✅ theme token 分层命名思路（fg / bg / stroke / hover 分层）

**明确不吸收**：子 HWND 模型、WAM、Win7 兼容层、其 layout/stack。

## 6. 迁移路线（绞杀式，S0→S10，每步一提交）

| 步 | 内容 | 验收 |
|---|---|---|
| S0 | 基线冻结：全测试通过；6 页 × 浅/深 × 折叠/展开 截图基线 | 基线入库 |
| S1 | 建 `crates/ui`：平移 motion.rs + 合并 theme/layout 常量为 tokens.rs | 纯平移零行为变化，`cargo test` 绿 |
| S2 | 平移 scrollbar.rs、icons.rs → ui | 同上 |
| S3 | 定 Component trait + AnimState；**只迁 button + disclosure**（最简单，验证抽象） | 两控件行为与基线一致 |
| S4 | 迁 toggle、segmented、stepper | 含键盘路径（Space/←→）回归 |
| S5 | 迁 slider（最复杂：拖拽贴手 + 值框编辑态 + LiveValue/Commit 双通道） | **重点回归**：拖动、输入提交、失焦提交、ESC |
| S6 | 迁 swatches / metric-swatches | 选色语义（跟随默认/自动色/自定义）不变 |
| S7 | card/row/group 排版进 `ui::layout`；layout.rs 现有不变式测试随迁 | spacing 系列测试原样通过 |
| S8 | sidebar + toast 组件化 | 侧栏命中（sidebar_at）与绘制几何一致 |
| S9 | 动效验收 M1–M5 | 逐条人工验收 + 收敛停表确认（CPU 回落 0.1% 量级） |
| S10 | 清理：删 widget.rs 旧路径，mod.rs 减重（目标 <1500 行） | 无 downcast 大 match 残留 |

每步 Conventional Commits（`refactor(ui): ...`）；S3 起每组件带单测（命中矩形、弹簧收敛、布局不变式，风格沿用 layout.rs 现有测试）。

## 7. 风险与对策

| 风险 | 对策 |
|---|---|
| Component trait 设计错误 → S4-S6 返工 | S3 只迁 2 个最简单控件当抽象试金石，再铺开 |
| 抽离中交互回归（拖拽排序/滚动惯性） | 每 Step 跑 `SYSOVERLAY_DIAG=settings,click` 诊断链 + 截图对比基线 |
| 拖拽排序与 card 布局耦合深（shift_card 直接改 rect） | S7 保持 `Card.rect = base + y_off` 位置不变式，动画偏移仍由组合根持有 |
| ui crate 过度抽象 | 只抽有 ≥2 消费者的；MetricFontStatus/AboutHeader 这类单页特化件留在组合根不进库 |
| CPU 占用回退（40fps 空转有过前科） | M1-M5 全部沿用「收敛即停表」协议，S9 用 probe 计数验证 |

## 8. 待确认的决策点

1. **库落位**：新 workspace 成员 `crates/ui`（推荐，依赖方向干净）vs `settings/` 内 mod 库化（成本低一档，但耦合仍在 win crate）。
2. **路线**：绞杀式 S0→S10（推荐）vs 直接重写 widget.rs（快但无回退）。
3. **动效时机**：M1–M5 放 S9（推荐）vs 先行动效后动架构（会返工一次）。
