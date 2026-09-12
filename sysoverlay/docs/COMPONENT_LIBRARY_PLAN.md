# 设置页组件融合计划（v2 —— Quel'Thalas 组件融合）

> 状态：**✅ 已完成（2026-09-12，S0-S7 全部验收通过）**
> 路线 A 拆壳移植落地：switch/button/stepper/slider/segmented/swatches 全部 Fluent 化，
> 交互层（命中/Action/弹簧/滚动/拖拽/翻页）零改动；6 枚 Fluent 图标接入；
> 字阶/圆角统一到 Fluent token。提交链 d448b9e → 4a89ce1。
> 用户意图：直接用 Quel'Thalas 组件库里的组件优化手写控件，融合进代码，不另建抽象层。

---

## 0. 关键技术事实（决定融合方式）

读完 `qt/src/component/switch.rs` / `button.rs` 源码，Quel'Thalas 组件的真实形态：

```
每个组件实例 =
  子 HWND（真实窗口）
+ 独立 ID2D1HwndRenderTarget（每控件一个 D2D 渲染目标）
+ 独立 WAM 动画管理器 + IUIAnimationTimer（每控件一套 COM 动画栈）
+ 自己的 wndproc + GDI region 命中 + DWrite TextFormat
```

它与设置页现有核心交互的冲突：

| 现有交互 | 子 HWND 直嵌的后果 |
|---|---|
| 内容滚动（惯性弹簧） | scroll_y 每帧变 → 全部子 HWND 每帧 SetWindowPos（40 控件 × 60fps）；子窗口与父绘卡片背景 DWM 合成不同步 → 撕裂/抖动 |
| 卡片拖拽排序 | 被拖卡及换位邻卡内所有子控件逐帧搬家 |
| 折叠展开动画 | 卡片高度弹簧逐帧变化 → 子控件逐帧重排 |
| 翻页滑入/淡入 | 子窗口不受父窗口 D2D PushLayer 淡出控制，过渡无法整页生效 |
| CPU 经济（收敛停表、看门狗） | 40 个独立 WAM 定时器各自唤醒 |

**结论：把组件当子 HWND 直接嵌入 = 放弃滚动惯性、拖拽排序、翻页过渡和已有的全部性能工程。**

因此「直接融合」有两条可执行路线：

## 1. 两条融合路线

### 路线 A：拆壳移植（推荐）
把 Quel'Thalas 组件的**绘制代码 + Fluent 尺寸标注 + token + 图标**移植进现有自绘体系，剥掉 HWND/WAM 外壳：

- 保留：Fluent 2 观感（switch 轨道 40×20、thumb ⌀14、token 化间距/字阶/圆角、浅深两套色）
- 剥掉：子 HWND、per-instance WAM、GDI region 命中、PCWSTR Props
- 换上：现有空间网格命中 → `Action` dispatch、`motion.rs` 弹簧（hover/press/val 三根，天然可中断）
- 融合形态：`settings/fluent/` 模块，一控件一文件，直接落在代码里（不建新 crate、不做 trait 抽象——遵从「直接融合进代码即可」）

**结果：设置页观感全面 Fluent 2 化，架构零破坏，全部现有交互保留。**

### 路线 B：子 HWND 直嵌（已否决）
真·直接用库组件。需接受 §0 冲突表的全部代价，滚动/拖拽/翻页需改为子窗口重排逻辑重写。已确认不采用。

## 2. 组件对照表（路线 A）

| Quel'Thalas 组件 | 替换的手写控件 | 移植内容 |
|---|---|---|
| `switch.rs` | ToggleWidget | 轨道/旋钮几何、on/off 颜色交叉渐变、Fluent 动效节奏 |
| `slider.rs` | SliderWidget | 轨道/填充/thumb/刻度观感；保留现有值框编辑态（库无此物） |
| `spin_button.rs` | StepperWidget | −/＋ 按钮观感与按压态 |
| `button.rs` | ButtonWidget | Primary/Secondary 外观、Shape/Size token、图标位 |
| `tab_list.rs` | SegmentedWidget | 选中片滑动观感（Fluent tab 语义） |
| `checkbox.rs` / `radio.rs` | （新增能力） | 未来扩展项可用 |
| `theme.rs` tokens | 现 `Theme` + `layout.rs` 常量 | Fluent token 词表（colorNeutral* / spacing* / fontSize*）浅深两套，映射到现有 Theme 字段 |
| `icon/*` | 现 `icons.rs` | chevron / checkmark / dismiss / copy 等 MIT SVG path 直接搬 |

不移植：`dialog` / `task_dialog` / `menu` / `data_grid` 等设置页用不到的重组件；`calendar` / `date_picker`。

## 3. 执行阶段（S0→S7，每步一提交；各任务的模型分工见 §6）

| 步 | 内容 | 验收 |
|---|---|---|
| S0 | 基线截图（6 页 × 浅/深 × 折叠/展开） | 基线入库 |
| S1 | 移植 Fluent tokens（`settings/fluent/tokens.rs`）+ Theme 浅/深映射 | 纯新增，无行为变化 |
| S2 | **switch 试点**：ToggleWidget 绘制换 Fluent switch（几何+颜色渐变+动效节奏），命中/Action/弹簧接旧管线 | 与基线对照观感 + 交互全保留；试点验证移植管线后再铺开 |
| S3 | slider / spin_button / button | 滑杆拖拽贴手、值框编辑、长按连发全部回归 |
| S4 | tab_list→segmented；swatches 套 token 重绘 | 选色语义不变 |
| S5 | 卡片/行/字阶按 Fluent typography 微调（全页统一） | spacing 不变式测试通过 |
| S6 | 图标集扩充（chevron/checkmark/dismiss/copy）+ 动效验收 M1–M5（滚动条热态、导航悬停、toast、主题交叉渐变、窗口入场——沿用 v1 评价结论） | 逐条验收 + 收敛停表确认 |
| S7 | 清理 widget.rs 被替换的旧 paint 函数 | 无死代码残留 |

## 4. 不变项（红线）

- 单向数据流（`dispatch` / `apply_config` 唯一写入口）
- 统一状态机 + 空间网格命中 + 卡片拖拽排序
- `motion.rs` 弹簧 + ANIM_TIMER 主时钟 + 收敛停表（不引入 WAM）
- 减弱动效 / 高对比度探测
- 诊断链（`SYSOVERLAY_DIAG=settings,click`）

## 5. 风险与对策

| 风险 | 对策 |
|---|---|
| 移植中观感回归 | S0 基线截图逐页对比；S2 单控件试点先行 |
| Fluent 尺寸与现有 4px 间距刻度冲突 | S1 移植 token 时做一次对表，冲突项以 Fluent 为准并记录 |
| 弹簧 vs Fluent 动效节奏差异 | Fluent 的 cubic-bezier 时长换算成等效弹簧参数（response/ζ 对照表），保持单一弹簧语言 |
| 旧 paint 函数残留两套观感 | 每 S 步替换即删旧路径，不留开关 |

## 6. 模型分工（高智 / 低智）

**分工原则**：高智模型做「定义接口、跑通试点、异常路径、架构层文件、验收」；低智模型做「按已验证模板铺开、纯转录、机械删除、跑验证」。判断标准：一个任务是否需要理解 3000 行 `mod.rs` 的借用结构、坐标系不变式（内容坐标 vs 客户区坐标）、动效时钟协议——需要就是高智，照清单执行 + `cargo test` 兜底就是低智。

**每个低智任务的指令包**（由高智模型预先产出）= ① 交接物（映射表/模板文件/死代码清单）② 红线（不许碰的文件/函数）③ 验收命令（`cargo build` + `cargo test` + 基线截图对比 / `SYSOVERLAY_DIAG=settings,click`）。

| 任务 | 模型 | 理由 / 交接物 |
|---|---|---|
| S0 基线截图 | **低** | 照 checklist 启动应用、逐页截图、按文件名归档 |
| S1 Fluent token 表结构与 Theme 映射设计 | **高** | 两套 token 体系取舍（Fluent ~100 token vs 现 Theme 20 字段 + layout 常量）；产出《映射表》 |
| S1 token 值转录 + 断言测试 | **低** | 拿映射表逐行填值；测试兜底，填错即红 |
| S2 switch 试点（移植管线模板） | **高** | 决定移植模式：绘制适配到自绘管线、Fluent bezier→等效弹簧参数换算、旧 paint 路径删除时机；产出《控件移植模板》文档 |
| S3 button / spin_button 铺开 | **低** | 严格按 S2 模板逐控件替换；验收 = 编译 + 基线对比 |
| S3 slider | **高** | 拖拽贴手 / 值框编辑 / LiveValue-Commit 双通道都在 mod.rs 交互层，借查复杂、回归面大 |
| S4 segmented / swatches 重绘 | **低** | 按模板；红线 = 不碰 `on_click_plain` 选色语义 |
| S5 全页字阶/间距 Fluent 化 | **高** | 牵动行高 → 命中测试/滚动钳制/spacing 不变式测试连锁，需全局判断 |
| S6 图标扩充 | **低** | SVG path 搬运 + 格式适配 icons.rs，机械 |
| S6 动效验收 M1–M5 | **高** | 时钟协议 + 收敛停表有过 40fps 空转前科，收敛判定不可交给低智模型 |
| S7 旧代码清理 | **低** | 高智先产出死代码清单（grep + 调用图），低智照单删 + 编译验证 |

**提交规则**：低智产出必须过全部验收命令才可提交；高智在每个阶段收尾做一次审阅（重点：S2 模板、S5 字阶、M1–M5 收敛判定）。

## 7. 决策记录

- 2026-09-12：确认融合路线 = **A 拆壳移植**（B 子 HWND 直嵌因与滚动/拖拽/翻页冲突被否决）。
- 2026-09-12：S1 执行修正——色值唯一真值 = Quel'Thalas theme.rs（首轮转录混入 Fluent1/Material 色被测试拦截）。
- 2026-09-12：S5 执行修订——原定 body→base300(14) 与 ROW_H=34 保留决策冲突（纵向节奏不变式），
  改为 base400(16)/base300(14) + HINT_EXTRA 22→20，改动面收敛为 3 常量。
- 2026-09-12：S0-S7 全部验收通过，工程完结。后续可选项：图标消费（Chevron* 用于折叠头）、
  checkbox/radio 新控件、S2 模板沉淀为新控件的常备范式。
