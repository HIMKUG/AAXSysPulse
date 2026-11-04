# AAXSysPulse 分层代码审查建议 · 续篇（第二轮全量深挖）

- 审查基准：`CODE_INDEX.md` + 首份报告 `docs/reviews/2026-11-04-CODE_INDEX-分层审查建议.md`（001–300）
- 本轮性质：**首轮 300 条是人为上限，不是发现上限**。本轮对同一批源码做了第二轮通读，重点覆盖首轮只抽样的大文件区间、14 个 Vue 页面、CSS、测试有效性、构建/CI、文档与工具脚本
- 去重方式：五路并行审查先各自比对首轮 300 条精简索引，再交叉去重；编号从 **301** 起连续编排
- 建议总数：**606 条**（编号 301–906），分属 14 个新增层面；与首份合计 906
- **范围约束不变**：全部为局部、可落地的小改动；不含新增功能、不含新增/替换依赖包、不含架构重写、不含无关扩张型建议（如 2FA 等）；对"删除未使用依赖"一类建议仅指向已有依赖的清理
- 严重度：`[高]` = 静默错误/数据丢失/崩溃或 UB 风险；`[中]` = 行为偏差、口径不一致、可维护性；`[低]` = 注释、命名、测试与工程卫生

> 关于"所有问题"：代码审查在数学上不可穷尽（同一行可以不断叠加更细的边界、平台、并发与可访问性推论）。本轮交付的是**在当前上下文与静态取证条件下能逐条落到 `文件:行号` 的全部新增结论**，不做无证据的凑数，也不把首轮结论换个说法重复一遍。

---

## 本轮最值得先处理的 18 条（按文件定位）

| 文件:行号 | 一句话 |
|-----------|--------|
| `sysoverlay/crates/win/src/settings/mod.rs:102` | `WM_MOUSELEAVE` 定义成 `0x02A2`（实为 `WM_NCMOUSELEAVE`），TrackMouseEvent 发的是 `0x02A3` → 所有 hover 离窗永不复位 |
| `sysoverlay/crates/win/src/window.rs:1104` | `WM_WTSSESSION_CHANGE` 用 lParam（会话 ID）去匹配 LOCK/UNLOCK，状态在 wParam → 锁屏检测基本失效 |
| `sysoverlay/crates/win/src/window.rs:1682` | UIPI 白名单漏 `WM_OPEN_SETTINGS`，不同完整性级别下"双击打开设置"被静默丢弃 |
| `sysoverlay/crates/win/src/settings/mod.rs:948` | 过渡动画预插入键用控件下标、读取键用 widget.id，两套键不互通，预插入条目永不命中 |
| `sysoverlay/crates/win/src/settings/mod.rs:560` | `Gfx::new` 失败直接 return，但 `CreateWindowExW` 已成功且 GWLP_UD 未设 → 窗口句柄泄漏 |
| `sysoverlay/crates/win/src/settings/mod.rs:1741` | 逻辑 px 字段写入未除 DPI 的物理像素，重启后 `s(dpi, …)` 二次放大 |
| `sysoverlay/crates/win/src/settings/layout.rs:1002` | 关于页调 `page_footnote(5,…)`，而该函数只在 `6` 返回 Some → 版权脚注永不显示 |
| `sysoverlay/crates/win/src/settings/layout.rs:1142` | 已缩放的 rows_h/foot_h 与未缩放常量相加后又整体缩放一次，仅 96dpi 自洽，测试固定 96 抓不到 |
| `sysoverlay/crates/core/src/config.rs:297` | 默认 `window_pos=Some((625,-9))` 让 window.rs 的回退坐标与启动贴角两条分支恒不可达 |
| `sysoverlay/crates/ui/src/font.rs:792` | 字号 `to_bits()` 精确位型进图集键，字号微抖会生成无穷多键填满 256 槽 |
| `sysoverlay/crates/ui/src/offscreen.rs:72` | over 合成的 out_r/g/b 无 `.min(255)`，`as u8` 回绕；RGB 与 alpha 钳位不对称 |
| `sysoverlay/crates/win/src/gfx_d2d.rs:191` | `LayoutKey` 首字段是 String，每次 draw_text 都 `text.to_string()` 堆分配做查表键 |
| `sysoverlay/crates/win/src/render_d2d.rs:582` | `GetMetrics` 失败被吞、tm 全 0 → 右对齐修正与自愈扩窗全部基于 0 宽度 |
| `sysoverlay/crates/win/src/proc_gpu.rs:67` | `available` 注释声称失败后置 false，全文件无任何写 false 路径 → 枚举失败每帧重跑 40~180ms |
| `sysoverlay/crates/win/src/msr.rs:554` | PKG 读失败用 `?` 返回 None，绕过注释承诺的"沿用 last_watts 避免掉成 `--`" |
| `guide-app/src-tauri/src/install.rs:467` | emit 失败走 `log_install(...,"失败",…)` 被计入 STEP_FAIL，安装成功却总结"失败 1" |
| `guide-app/src-tauri/src/lib.rs:471` | Run 值用 `to_string_lossy()` 且只首尾加引号，尾随反斜杠会写坏启动命令 |
| `guide-app/src-tauri/build.rs:116` | `WinRing0x64.sys` 被 `.gitignore:11` 全局忽略却是构建期硬依赖，全新 clone / CI 的 `cargo test` 必 panic |

---

## 十四、设置面板状态机、消息与命中（`settings/mod.rs`，首轮大文件未覆盖区）

301 [高] `sysoverlay/crates/win/src/settings/mod.rs:102` — `WM_MOUSELEAVE` 定义为 `0x02A2`（实为 `WM_NCMOUSELEAVE`），而 `window.rs:488` 用 `0x02A3`，TrackMouseEvent 发的也是 0x02A3 → 改为 0x02A3 并与 window.rs 共用契约常量。
302 [中] `sysoverlay/crates/win/src/settings/mod.rs:336` — `invalidate_widget` 在 `dirty == None`（注释 200 定义为全量重绘）时走 else 把 None 改成 `Some(vec![idx])`，静默丢掉全量重绘 → None 分支保持 None 不降级。
303 [中] `sysoverlay/crates/win/src/settings/mod.rs:359` — `Vec::with_capacity(6)` 与 `pages:[Page;7]`(:180)、`sidebar_at` 硬编码 7(:712)、`IconKind::ALL.len()`(:2965) 四处页数不一致 → 统一由 `pages.len()`/单一常量派生。
304 [低] `sysoverlay/crates/win/src/settings/mod.rs:396` — :396/:412/:434 三份 `(k,damp)=if animations_enabled(){K_SHEET,C_SHEET}else{K_REDUCED,C_REDUCED}` 逐字重复 → 抽 `sheet_params()`。
305 [中] `sysoverlay/crates/win/src/settings/mod.rs:514` — `RegisterClassExW` 返回值被 `let _ =` 吞，而 `REGISTERED.call_once` 已置位；注册失败后永不重试、后续建窗必失败 → 记录返回值并在失败时不标记 once。
306 [高] `sysoverlay/crates/win/src/settings/mod.rs:560` — `Gfx::new` 失败直接 return，但 :542 的 `CreateWindowExW` 已成功、GWLP_UD 未设 → 失败路径 `DestroyWindow(hwnd)`。
307 [低] `sysoverlay/crates/win/src/settings/mod.rs:559` — `0x80020000_isize` 硬编码 `WS_POPUP|WS_MINIMIZEBOX` 组合 → 改用具名常量运算。
308 [中] `sysoverlay/crates/win/src/settings/mod.rs:621` — :621/:623/:624/:627 与 `open_settings`(:459-464) 完全相同的四连 `let _ =`（ShowWindow/SetForegroundWindow/SetWindowPos/PostMessageW）→ 抽 `bring_to_front()` 并逐个判返回值。
309 [低] `sysoverlay/crates/win/src/settings/mod.rs:633` — `pref=2` 与两处 `4`（DwmSetWindowAttribute cbSize）为裸魔数 → 用具名常量/`size_of::<i32>()`。
310 [中] `sysoverlay/crates/win/src/settings/mod.rs:682` — `spatial_grids.get(ctx.page)?` 在 cfg 不可用时（:384-386 不填 grids）直接返回 None，控件命中整体静默失效 → 失败分支补 probe 并回退线性命中。
311 [中] `sysoverlay/crates/win/src/settings/mod.rs:685` — grid.query 返回的下标直接 `widgets[i]` 裸索引；rebuild_pages 后网格与控件树可能不同步 → 用 `.get(i)` 或重建后校验。
312 [中] `sysoverlay/crates/win/src/settings/mod.rs:711` — `let i = rel / step`，`step = s(dpi, SB_ITEM_H+SB_ITEM_GAP)` 在 dpi 异常为 0 时除零 panic → `step.max(1)`。
313 [中] `sysoverlay/crates/win/src/settings/mod.rs:724` — `in_content` 只判 x 不判 y，标题栏区域也算内容区 → 补 y 下界。
314 [高] `sysoverlay/crates/win/src/settings/mod.rs:809` — `ctx.card_slide[neighbor_metric]` 中 `neighbor_metric = order[npos] as usize` 未校验 < N_METRICS，损坏的 metric_order 越界 panic → `.get_mut()`。
315 [中] `sysoverlay/crates/win/src/settings/mod.rs:893` — `metrics_all_off_after` 手写 9 个开关，与 `is_metric_toggle`(:877)、`N_METRICS` 三处无关联 → 由同一 Metric 列表生成并补穷尽断言。
316 [中] `sysoverlay/crates/win/src/settings/mod.rs:946` — `SetPage` 里 `transitions.clear()` 清掉所有页，与 :374 注释"按 (页,id) 持久化，无需随重排重建"矛盾 → 只清本页键。
317 [高] `sysoverlay/crates/win/src/settings/mod.rs:948` — 预插入键用 `(p, wi as i32)`（控件下标），:3243 读取键用 `(page_idx, widget.id)`，两套键不互通 → 统一用 widget.id。
318 [中] `sysoverlay/crates/win/src/settings/mod.rs:957` — `ctx.cfg().map(...).unwrap_or(false)` 在 cfg 读失败时兜底"不拒绝"，放行关闭最后一个监控项 → 兜底改 true 或显式留日志。
319 [中] `sysoverlay/crates/win/src/settings/mod.rs:1021` — `Action::Select(r,i)` 的 Style 分支仅 `i==1` 判 Vertical，`i>=2` 静默落 Strip → 对该 CfgRef 选项数做边界校验。
320 [中] `sysoverlay/crates/win/src/settings/mod.rs:1051` — Select 的 Lang 分支未判 `applied` 就 `rebuild_pages()`，而 Toggle 的 :1011 分支在 `if applied` 内 → 补条件。
321 [低] `sysoverlay/crates/win/src/settings/mod.rs:1328` — `dp==0 ? v.round() : (v*10.0).round()/10` 把 10.0 写死，`dp>=2` 仍按 1 位小数显示 → 用 `10f64.powi(dp as i32)`。
322 [中] `sysoverlay/crates/win/src/settings/mod.rs:1347` — e.buf 的 u16 逐个 `char::from_u32`，非 BMP/代理对被拆成 U+FFFD；与 `widget.rs:941` 是第二份相同实现 → 用 `String::from_utf16_lossy` 并抽公共函数。
323 [中] `sysoverlay/crates/win/src/settings/mod.rs:1414` — `on_click_plain` 直接 `widgets[idx]`，而 `activate_focused`(:1242) 有 get 越界保护 → 统一加边界检查。
324 [低] `sysoverlay/crates/win/src/settings/mod.rs:1475` — :1475/:1529/:1550 三份相同的 `(x-left)/seg_w.max(1) as usize).min(n-1)` 分段命中 → 抽 `hit_segment(pt, rect, n)`。
325 [中] `sysoverlay/crates/win/src/settings/mod.rs:1668` — `start_timer`/`kill_timer` 吞掉 SetTimer/KillTimer 返回值，定时器建不起来时动画静默停摆 → 失败写 probe。
326 [低] `sysoverlay/crates/win/src/settings/mod.rs:1682` — `on_wm_paint` 仅 `!paint_d2d` 时 `ValidateRect`，成功路径完全依赖 :2842 内部调用 → 显式注释该隐含契约或统一在 on_wm_paint 验证。
327 [中] `sysoverlay/crates/win/src/settings/mod.rs:1720` — WM_GETMINMAXINFO 只写 ptMin/MaxTrackSize，未写 ptMaxSize/ptMaxPosition，与 :1694"任何路径都无法改变窗口尺寸"不符 → 补齐四个字段。
328 [高] `sysoverlay/crates/win/src/settings/mod.rs:1741` — `settings_size` 注释(config.rs:229)为逻辑 px，此处写入未除 DPI 的 `client_size`（:1738 自述物理像素），重启后二次放大 → 落盘前换算回逻辑值。
329 [中] `sysoverlay/crates/win/src/settings/mod.rs:1766` — 滚轮归属 `in_content` 只判 x，鼠标在标题栏且 x 落在内容区时滚轮仍滚动 → 排除标题栏 y 区间。
330 [中] `sysoverlay/crates/win/src/settings/mod.rs:1804` — NCHITTEST 用 `get_system_dpi()` 而其余分支(:1842/:1855)用 `ctx.dpi` → 统一取 ctx.dpi。
331 [中] `sysoverlay/crates/win/src/settings/mod.rs:1877` — `Interaction::Dragging{widget:idx}` 保存的 idx 直接 `widgets[idx]`，翻页/rebuild 后控件数变化会越界 → `.get(idx)`。
332 [中] `sysoverlay/crates/win/src/settings/mod.rs:1968` — WM_MOUSELEAVE 只清 `Interaction::Hover`，Pressing/Editing/Dragging 离窗时残留（无捕获路径不走 WM_CAPTURECHANGED）→ 补状态复位。
333 [低] `sysoverlay/crates/win/src/settings/mod.rs:2044` — :2044/:2084/:2149 三处 `start_timer(hwnd, ANIM_TIMER, 16)` 硬编码 16ms → 提 `ANIM_FRAME_MS`。
334 [低] `sysoverlay/crates/win/src/settings/mod.rs:2102` — 长按首延迟 350ms 裸魔数，而重复周期已有具名 `STEPPER_REPEAT_MS=60`(:137) → 补 `STEPPER_FIRST_DELAY_MS`。
335 [中] `sysoverlay/crates/win/src/settings/mod.rs:2131` — `unwrap_or([0,1,2,6,3,5,4,7,8])` 是 `config.rs:355` 默认 metric_order 的第二份硬编码副本 → 改取 `Config::default().metric_order`。
336 [中] `sysoverlay/crates/win/src/settings/mod.rs:2302` — EDIT_TIMER 分支 `edit=None` 时只 kill_timer，未复位 `Interaction::Editing` → 同时置 Idle。
337 [中] `sysoverlay/crates/win/src/settings/mod.rs:2316` — STATUS_TIMER 到期无条件 `status=None`，不校验 :243 的 `Instant` 时间戳，与 :1166"2.5s 自动清除"无保障 → 比对 elapsed。
338 [中] `sysoverlay/crates/win/src/settings/mod.rs:2385` — 编辑态下 ←/→ 落到 `_ => {}`，而 `EditState`(:1334) 只有 bool caret 无位置字段，光标无法移动 → 补 caret 位置或显式注明不可移动。
339 [中] `sysoverlay/crates/win/src/settings/mod.rs:2391` — 键盘链只处理 ESC/Tab/Space/←→，缺 Enter(0x0D) 触发焦点控件及 ↑/↓ → 补 VK_RETURN 与方向键。
340 [中] `sysoverlay/crates/win/src/settings/mod.rs:2403` — `is_input` 只接收数字与 `.`，不接受 `-`，而偏移步进器范围为 -64..64 → 允许负号。
341 [低] `sysoverlay/crates/win/src/settings/mod.rs:2420` — 输入长度上限 `16` 裸魔数且超长静默丢字符 → 提常量并给用户反馈。
342 [低] `sysoverlay/crates/win/src/settings/mod.rs:2449` — 注释"切到颜色页（index 2）"而实现用 `find_map` 按 id=200 在任意页查找 → 修正注释。
343 [高] `sysoverlay/crates/win/src/settings/mod.rs:2473` — `on_wm_sampler_diag` 未对重复投递去重，同一 lparam 裸指针二次 `Arc::from_raw` 会 double free → 用序号/旧值比较拒绝重复。
344 [中] `sysoverlay/crates/win/src/settings/mod.rs:2491` — `hand` 在 :569 加载失败为 null 时 `SetCursor(null)` 会隐藏光标 → 回退 `LoadCursorW(None, IDC_ARROW)`。
345 [低] `sysoverlay/crates/win/src/settings/mod.rs:2591` — `c(_t:&Theme, v)` 忽略 Theme 参数，与 `widget.rs:403` 逐字重复 → 删形参并合并。
346 [中] `sysoverlay/crates/win/src/settings/mod.rs:2661` — 滚动惯性分支自承 :2656"当前无输入源会写入非零 scroll_vel、保留给将来触控板" → 整段不可达死代码，删除或标注。
347 [中] `sysoverlay/crates/win/src/settings/mod.rs:2924` — `display_px = 20.0*dpi/72.0`（pt 换算）与 body_px/caption_px 的 `dpi/96.0`（DIP）口径不一致 → 统一到 /96。
348 [中] `sysoverlay/crates/win/src/settings/mod.rs:2965` — 注释称导航项数以 `IconKind::ALL` 为唯一真源，实际 `sidebar_at`:712=7、`pages`:180=7、`grids`:359=6 三处写死 → 统一常量并加 const assert。
349 [低] `sysoverlay/crates/win/src/settings/mod.rs:3066` — `paint_page` 的 `_diag` 形参未使用，实际诊断数据取自 `pc.diag`(:3268) → 删除冗余形参。
350 [低] `sysoverlay/crates/win/src/settings/mod.rs:3071` — `_metric_expanded` 形参注释 :3088"折叠卡将消费"但全程未用 → 补接线或删参数并改注释。
351 [中] `sysoverlay/crates/win/src/settings/mod.rs:3191` — `card.rows[ri-1]` 依赖"首行 divider 恒 false"这一隐含约束，无显式 `ri>0` 守卫 → 补 guard 并加首行 divider 单测。

## 十五、悬浮窗 `window.rs`：消息、坐标与生命周期（首轮未覆盖区）

352 [低] `sysoverlay/crates/win/src/window.rs:52` — QUNS 注释列 1..7 共 7 个取值，代码只定义 1/3/4 且 `QUNS_NOT_PRESENT` 带 `#[allow(dead_code)]` → 补齐枚举或收窄注释。
353 [中] `sysoverlay/crates/win/src/window.rs:103` — 最小化判定只看 `left==right || top==bottom` 零面积，最小化窗口常返回 (-32000,-32000) 小矩形 → 改用 `IsIconic`。
354 [中] `sysoverlay/crates/win/src/window.rs:160` — `overlay_state_dir` 用 `var("LOCALAPPDATA")`（要求 UTF-8），`config_io:55` 用 `var_os`；非 UTF-8 路径回落 `"."` 把标记写进工作目录 → 统一 var_os + PathBuf。
355 [中] `sysoverlay/crates/win/src/window.rs:181` — `mark_overlay_hidden(false)` 的 `remove_file` 失败被吞，隐藏标记残留导致下次启动误判隐藏 → 失败留 probe 并重试。
356 [中] `sysoverlay/crates/win/src/window.rs:363` — `NumericFields` 不含 :361 设置的 `gpu_primary_only/gpu_3d_only`，改这两项不触发强制屏障提交 → 纳入结构或显式说明。
357 [低] `sysoverlay/crates/win/src/window.rs:479` — 手工 `extern "system"` 声明 `SetCapture/ReleaseCapture`，而 `settings/mod.rs:51` 用 windows crate 版本 → 复用 crate 绑定。
358 [中] `sysoverlay/crates/win/src/window.rs:672` — `GetCursorPos`/`GetWindowRect`/`SetCapture` 返回值全吞；GetCursorPos 失败时 drag_start=(0,0)，拖动把窗口瞬移到角落 → 判返回值并跳过本次拖动。
359 [中] `sysoverlay/crates/win/src/window.rs:749` — `clamp_window_into_view` 返回 None 时仍把 `pos_dirty/size_dirty` 清为 false，位置/尺寸改动不再落盘 → 仅钳制成功时清标志。
360 [中] `sysoverlay/crates/win/src/window.rs:755` — `save_config_async(&state.cfg)` 返回值未判，拖动结束落盘失败无痕 → 判 bool 并留日志。
361 [中] `sysoverlay/crates/win/src/window.rs:812` — WM_SIZE 里 GetWindowRect 失败时 win_w/win_h 保留旧缓存（可能为 0），渲染线程读到 0 尺寸 → 失败置 0 并记 probe。
362 [低] `sysoverlay/crates/win/src/window.rs:876` — `SM_REMOTESESSION` 在函数内以 `SYSTEM_METRICS_INDEX(0x1000i32)` 重复定义（crate 已有具名常量）→ 直接用 crate 常量。
363 [中] `sysoverlay/crates/win/src/window.rs:890` — WM_TIMER 末尾未按 ID 过滤，任何未知 timer ID 都触发一次 `render_thread::request` → 只在已知 ID 分支续帧。
364 [低] `sysoverlay/crates/win/src/window.rs:1066` — 注释 :604 称 wndproc 用 Release store 配对 Acquire，实际 PBT 分支用 SeqCst → 统一内存序或修正注释。
365 [高] `sysoverlay/crates/win/src/window.rs:1104` — `WM_WTSSESSION_CHANGE` 用 `lparam.0 as u32` 匹配 WTS_SESSION_LOCK/UNLOCK；状态在 wParam、lParam 是会话 ID → 改 `_wparam.0 as u32`。
366 [低] `sysoverlay/crates/win/src/window.rs:1153` — 文件头 :8 称"WM_DESTROY 中 Box::from_raw 取回"，实现由 `OverlayWindow::drop`(:1771) 自动释放 → 修正文档，别混写两套生命周期。
367 [中] `sysoverlay/crates/win/src/window.rs:1207` — `save_current_config` 忽略 `save_config` 返回值，快速重启前同步落盘失败无痕 → 判返回值并留日志。
368 [中] `sysoverlay/crates/win/src/window.rs:1279` — `(rect.right-rect.left).unsigned_abs() as i32` 对 `i32::MIN` 回绕成负值，`w>0` 判假返回 (0,0) → 用 `saturating_abs()`/`try_into`。
369 [低] `sysoverlay/crates/win/src/window.rs:1304` — :1304/:1350 的 `GWL_STYLE=-16`/`GWL_EXSTYLE=-20` 手写魔数，而 :83 局部又用 crate 版 → 统一用 crate 常量。
370 [中] `sysoverlay/crates/win/src/window.rs:1315` — :1315/:1425 的 `SetWindowLongPtrW` 返回值被忽略，样式改写失败无痕且旧样式未保存 → 判返回值并留日志。
371 [中] `sysoverlay/crates/win/src/window.rs:1382` — 回滚配置的 `save_config` 被 `let _ =` 吞，与 :1367"内存值与磁盘必须一致"承诺不符 → 失败重试/提示。
372 [中] `sysoverlay/crates/win/src/window.rs:1385` — `ok=false` 时无条件弹失败提示，`real==requested`（写入报失败但系统状态已是所需值）时误报 → 加 `real!=requested` 前置。
373 [中] `sysoverlay/crates/win/src/window.rs:1447` — `snap_overlay_to_corner` 用 `get_screen_size()`（:1460 只用主屏 GetDC(None)），多显示器贴角落到主屏；而 :1488 已改用 MonitorFromPoint → 贴角也按目标显示器取整屏矩形。
374 [中] `sysoverlay/crates/win/src/window.rs:1454` — `match corner` 的 `_ =>` 把 3 及任何 >3 非法值都当左下 → 显式列出 0..=3 并对非法值返回/留日志。
375 [中] `sysoverlay/crates/win/src/window.rs:1462` — `get_screen_size` 未判 `GetDC(None)` 返回 HDC 是否有效就传给 GetDeviceCaps/ReleaseDC → 空 HDC 返回 (0,0) 并留痕。
376 [中] `sysoverlay/crates/win/src/window.rs:1519` — `clamp_window_into_view` 的 SetWindowPos 失败被吞，仍 :1529 把钳制坐标写回 rect（调用方落盘），配置与实际位置不符 → 失败返回 None。
377 [中] `sysoverlay/crates/win/src/window.rs:1599` — `OverlayWindow::new` 每次都 `RegisterClassExW` 无 Once 保护；二次调用同类已存在即返回 Err，而 `settings/mod.rs:498` 有 call_once → 加 once 或容忍 ERROR_CLASS_ALREADY_EXISTS。
378 [中] `sysoverlay/crates/win/src/window.rs:1613` — :1613 的 `unwrap_or((screen_w/4,24))` 与 :1706 的启动贴角分支因 `config.rs:297` 默认 Some((625,-9)) 恒不可达，与 :1695 三级优先级注释不符 → 修正默认值/注释。
379 [高] `sysoverlay/crates/win/src/window.rs:1682` — UIPI 白名单只放行 WM_RELOAD_CONFIG/WM_QUICK_RESTART，漏 :1811 第二实例投递的 WM_OPEN_SETTINGS，而 :1675 自述 UIPI 静默丢弃 → 补进白名单。
380 [中] `sysoverlay/crates/win/src/window.rs:1703` — 启动把 `snap_corner` 归 FREE 只改内存未落盘，每次启动重复执行、磁盘仍冲突 → 归位后一并落盘。
381 [低] `sysoverlay/crates/win/src/window.rs:1728` — FULLSCREEN `SetTimer` 返回值被吞、1000ms 裸魔数（:623 注释"1s"）→ 判返回值并提常量。
382 [中] `sysoverlay/crates/win/src/window.rs:1771` — `Drop` 只清 GWLP_USERDATA，不 DestroyWindow、不清 OVERLAY_HWND(:1167 只在 WM_DESTROY 清) → 与 WM_DESTROY 清理不对称，补齐或注明由调用方销毁。
383 [中] `sysoverlay/crates/win/src/window.rs:1811` — `PostMessageW(hwnd, WM_OPEN_SETTINGS, …)` 返回值被吞（且该消息未过 UIPI 白名单）→ 判返回值并留痕。
384 [低] `sysoverlay/crates/win/src/window.rs:1843` — 指针经 `Box→u64→isize→u64→*const` 四次转换，:1846 注释自承"只需一步" → 直接 `as *mut`/`as isize`。
385 [中] `sysoverlay/crates/win/src/window.rs:1849` — `GetClassNameW` 缓冲固定 `[0u16;256]` 且不判截断，超长类名 `len != class_name.len()` 直接漏匹配 → 用返回值判截断并扩容。
386 [高] `sysoverlay/crates/win/src/window.rs:1854` — `find_overlay_hwnd` 只按类名枚举、不校验归属进程 PID，同名类/残留窗口会取错 HWND → 用 `GetWindowThreadProcessId` 复核。
387 [中] `sysoverlay/crates/win/src/window.rs:1870` — :1870/:1873 的 `EnumWindows` 返回值被吞、末尾 `result.lock().ok()` 在 poison 时静默退化，与 `gdi.rs:1206` 的 `into_inner()` 口径不一致 → 判返回值并统一恢复。

## 十六、GDI 上屏与配置原子写的剩余区域

388 [中] `sysoverlay/crates/win/src/gdi.rs:187` — `AnimState` 默认 from/to=35.0、RENDERED_PCT_X100=3500，而 `Config::default().normal_opacity=100` → 启动首帧必触发一次无意义淡入，默认值改由 Config 派生。
389 [中] `sysoverlay/crates/win/src/gdi.rs:209` — `ANIM_STATE.lock()` poison 后直接返回 `(desired,false)`，动画永久停摆且无日志 → 用 `unwrap_or_else(|e| e.into_inner())` 恢复（与 :1206 一致）。
390 [低] `sysoverlay/crates/win/src/gdi.rs:245` — :245/:298 两个环境变量 LazyLock 重复同一段 `var(..).map(|v| v=="1").unwrap_or(false)`，且只认精确 "1" → 抽 helper 并接受 true/yes。
391 [低] `sysoverlay/crates/win/src/gdi.rs:269` — `should_autocutoff` 注释"纯函数，便于单测边界"，但全文件无 streak=29/30/31 用例 → 补边界单测。
392 [中] `sysoverlay/crates/win/src/gdi.rs:275` — `PRESENT_FAIL_STREAK.fetch_add(1)` 无饱和，稳态失败会累加到 u32 回绕重新计数 → `fetch_update` 钳到阈值。
393 [中] `sysoverlay/crates/win/src/gdi.rs:370` — `take_respawn_budget` 先 fetch_add、超额再 fetch_sub 撤回，两条 respawn 路径(:479/:523)并发时可能各取一次预算 → 用 CAS `fetch_update` 原子取额。
394 [中] `sysoverlay/crates/win/src/gdi.rs:599` — `apply_d2d_fallback_ui` 吞掉 `save_config` 失败，仍 :600 打印 "persisted render_engine=Gdi" → 日志谎报成功，按返回值分支打印。
395 [中] `sysoverlay/crates/win/src/gdi.rs:642` — `present_size_changed` 用 `swap` 提前提交尺寸记录，本帧上屏失败(:1266)后同尺寸下一帧不再强制整窗 → 成功后才提交。
396 [低] `sysoverlay/crates/win/src/gdi.rs:665` — 注释"横条布局 ≤10 格，8 绰绰有余"但 `MAX_DIRTY_RECTS=8`，9 格同变即折叠 Full → 改注释或把上限调到 ≥10。
397 [中] `sysoverlay/crates/win/src/gdi.rs:1096` — `invalidate_dib_cache` 在 Mutex poison 时用 `if let Ok` 静默跳过，HBITMAP/DC 不释放；而 :1206/:1341 都 `into_inner()` → 统一恢复口径。
398 [中] `sysoverlay/crates/win/src/gdi.rs:1119` — `free()` 里 SelectObject/DeleteObject/DeleteDC 三个 GDI 调用返回值全 `let _ =` 吞掉 → 判返回值并留 probe。
399 [中] `sysoverlay/crates/win/src/gdi.rs:1136` — `DibCache::get` 未按 :686 的 MAX_WIN=2048 钳制 w/h，异常尺寸仍尝试 CreateDIBSection 分配巨量内存 → 入口钳制。
400 [中] `sysoverlay/crates/win/src/gdi.rs:1169` — `SelectObject(mem_dc, bitmap)` 返回值未判 HGDI_ERROR，失败时 old_bmp=0，`free()` 不再还原旧对象 → 判失败并回滚。
401 [中] `sysoverlay/crates/win/src/gdi.rs:1308` — `ulw_indirect_rect` 把 prc 钳成空时 `return true` 且不计失败，被裁掉的脏区像素变化永久丢失 → 计为需整窗重传或回滚指纹。
402 [低] `sysoverlay/crates/win/src/gdi.rs:1334` — `upload_dib` 与 `present`(:1190) 是两份几乎逐字重复的 GetDC/lock/bf/Full-Rects/记账/ReleaseDC 逻辑 → 抽公共 `present_impl(..., copy: bool)`。
403 [低] `sysoverlay/crates/win/src/gdi.rs:1359` — upload-begin/end 两条卡死探针常驻，注释 :1356 却写"下次复现即可钉死" → 挂调试开关或事故后移除。
404 [低] `sysoverlay/crates/win/src/gdi.rs:1430` — 诊断探针用裸下标 `metric_colors[0].label/[2].label` 靠文字对应 CPU/GPU → 用 `Metric::Cpu as usize` 等。
405 [中] `sysoverlay/crates/win/src/config_io.rs:167` — `mark_degraded` 在 `BROKEN_FILES` poison 时静默跳过清单，而 :161 用 `into_inner()` 恢复 → 降级生效但清单为空，统一恢复。
406 [中] `sysoverlay/crates/win/src/config_io.rs:182` — 注释"同名重复时复制失败即说明证据已在"，但 `std::fs::copy` 默认覆盖同名目标，同秒重复调用会覆盖证据副本 → 用 create_new 语义或先判存在。
407 [中] `sysoverlay/crates/win/src/config_io.rs:266` — `write_bundle_to_tmp`（迁移事务）未持 :256 的 WRITE_LOCK，与 `write_bundle` 串行化口径不一致，迁移与保存并发会交错写同名 .tmp → 同样加锁。
408 [中] `sysoverlay/crates/win/src/config_io.rs:270` — 写临时文件失败时只清理已 push 进 tmp_paths 的文件，当前 `File::create` 成功但写失败的 temp 未删除 → 一并 remove_file。
409 [中] `sysoverlay/crates/win/src/config_io.rs:306` — `rename_tmp_to_final` 失败恒返回 `Err((0,name))`，与 :288 文档"返回 (成功数, 失败名)"不符（前半已成功数被丢）→ 返回实际计数供回滚使用。
410 [中] `sysoverlay/crates/win/src/config_io.rs:571` — 迁移第 6 步把旧 `config.txt` rename 到 `.bak` 的结果被忽略，仍继续 :574 写一次性标记；下次启动见标记即不再重试 → rename 失败即中止并留痕。
411 [高] `sysoverlay/crates/win/src/config_io.rs:816` — `switch_layout_in_place` 保存旧布局的 `write_file_atomic` 结果被 `let _` 丢弃，写失败仍继续载入新布局并切 render_style，旧布局几何/偏移静默丢失 → 失败中止或回滚。
412 [低] `sysoverlay/crates/win/src/config_io.rs:826` — `export_config` 文档称"失败返回 None"，实现恒 `Some(mcfg::export_config(cfg))`，Option 无意义 → 改返回 String 或补真实失败分支。
413 [中] `sysoverlay/crates/win/src/config_io.rs:1084` — :1034 注释称 `.bak` 带时间戳、仅保留最新一份，实现是固定 `format!("{}.bak", name)` → 修正注释或真正带 ts。
414 [中] `sysoverlay/crates/win/src/config_io.rs:1091` — import_config 备份循环中途 rename 失败直接 return false，前面已 rename 成 .bak 的正式文件不回滚，与 :1094"当前配置未改动"矛盾 → 复用 bak_entries 回滚。
415 [低] `sysoverlay/crates/win/src/config_io.rs:1143` — `.bak.failed.<秒级 fail_ts>` 同秒多次回滚失败会互相覆盖 → 加序号或毫秒。
416 [中] `sysoverlay/crates/win/src/config_io.rs:1202` — `get_config_path` 用 `to_string_lossy()`，非 UTF-8 用户名路径显示成 U+FFFD → 保留 PathBuf 或换展示方式。
417 [低] `sysoverlay/crates/win/src/config_io.rs:1214` — 各测试 fixture 用 `let _ = std::fs::write(...)` 吞掉写入失败，前置失败会让断言变成误导性红 → 改 `expect()`。
418 [中] `sysoverlay/crates/win/src/config_io.rs:1279` — `version_single_step_diff_ok` 名为单步差异，实现 :1283 写的是 5 文件全 v3，未覆盖 span=1 → 补 v2/v3 混合用例。
419 [低] `sysoverlay/crates/win/src/config_io.rs:1390` — 注释称"两个测试需临时替换这两个函数"并提到 `#[swift_bindgen]`，实现并未替换且该属性不存在 → 删掉误导性注释。

## 十七、设置页布局、自绘控件与 core 配置模型的剩余区域

420 [中] `sysoverlay/crates/win/src/settings/layout.rs:233` — `page_caption` 的 `_ => PgAbout` 把 p>=6 全当关于页，与 :242 `6 => ...`、:1002 传 5 三处口径不一致 → 显式穷尽 0..=6。
421 [中] `sysoverlay/crates/win/src/settings/layout.rs:242` — About 页码 6 硬编码，注释 :241 自述"由 5 顺延到 6"但调用点 :1002 仍传 5 → 用单一常量。
422 [低] `sysoverlay/crates/win/src/settings/layout.rs:310` — `RowDef::height` 注释"行内 hint 每行 +15"，实现 `HINT_EXTRA(20) * hint_lines` → 修正数值。
423 [低] `sysoverlay/crates/win/src/settings/layout.rs:392` — `type Group = (&str, Vec<RowDef>, Option<&str>, i32)` 第 4 个 i32（卡后间距）无具名无注释 → 改具名结构或补注释。
424 [高] `sysoverlay/crates/win/src/settings/layout.rs:1002` — `page_about` 调 `page_footnote(5, lang)`，而 :242 只在 `6` 返回 Some → 关于页版权脚注永不显示，传页码常量并补断言。
425 [低] `sysoverlay/crates/win/src/settings/layout.rs:1031` — 卡片最小宽 `240.0` 裸魔数，与 `CONTENT_MAX_W=620`、`MIN_W=560` 无关联 → 提具名常量。
426 [中] `sysoverlay/crates/win/src/settings/layout.rs:1130` — `divider: rd.divider || want_div[ri]`，而 `rd.divider` 在 :317/:325 全构造为 false → 该字段恒死状态，删除或接线。
427 [高] `sysoverlay/crates/win/src/settings/layout.rs:1142` — card_bottom 把已缩放的 rows_h/foot_h(:1139/:1140) 与未缩放的 rows_top/CARD_PAD 相加，:1149 又整体 `sf(dpi,…)` 二次缩放；仅 96dpi 自洽，测试固定 96(:1167) 抓不到 → 统一逻辑坐标后一次性缩放并补 192dpi 用例。
428 [中] `sysoverlay/crates/win/src/settings/layout.rs:1172` — `build_all` 的 order `[0,1,2,6,3,4,5,0,0]` 含重复 0、缺 7/8，是 `config.rs:355` 之外的第三份非法默认顺序 → 改用 `Config::default().metric_order`。
429 [低] `sysoverlay/crates/win/src/settings/layout.rs:1303` — `assert_eq!(checked, 18)` 写死 2×9，新增监控项即误报 → 改 `2 * N_METRICS`。
430 [中] `sysoverlay/crates/win/src/settings/layout.rs:1390` — `page_rebuild_is_within_budget` 用墙钟 `<5_000µs` 断言，注释 :1386 自承"release 下供参考"却仍 assert → 改操作计数或仅打印。
431 [中] `sysoverlay/crates/win/src/settings/widget.rs:171` — `SW_AUTO=8` 与 `sw_follow(false)=8`(:178) 下标重叠，仅靠调用方 auto 前置条件区分 → 重排下标或加 debug_assert。
432 [低] `sysoverlay/crates/win/src/settings/widget.rs:174` — `sw_n` 手写 `8+3`/`8+2`，未用同文件 `SW_COLORS=8`(:169) 与 `METRIC_SWATCHES.len()` → 由常量推导。
433 [中] `sysoverlay/crates/win/src/settings/widget.rs:285` — `SwatchesWidget::sync` 颜色不在预设中时 `unwrap_or(0)`，与真选中第 0 格不可区分，`mod.rs:1533` 的 `i != st.selected` 会吞掉点第 0 格 → 置 `has_sel=false`。
434 [中] `sysoverlay/crates/win/src/settings/widget.rs:384` — `slider_t_from_value` 用 `f64::EPSILON` 作 min==max 的绝对判据，近等区间仍算出极端 t → 用相对阈值或 `==`。
435 [中] `sysoverlay/crates/win/src/settings/widget.rs:394` — `slider_value_from_x` 的 `step<=0.0` 判不住 NaN，`clamp` 对 NaN 返回 NaN → 加 `!step.is_finite()` 过滤。
436 [中] `sysoverlay/crates/win/src/settings/widget.rs:403` — `c(_t:&Theme, v)` 忽略 Theme 参数且与 `mod.rs:2591` 重复 → 删形参并合并。
437 [中] `sysoverlay/crates/win/src/settings/widget.rs:799` — `METRIC_SWATCHES[i]` 依赖硬编码 `SW_COLORS=8` 与数组长度脱钩，改色板长度即越界 → 用 `METRIC_SWATCHES.len()`。
438 [中] `sysoverlay/crates/win/src/settings/widget.rs:845` — `anim_v == 0.0` 判"未初始化"，与 `mod.rs:3245` 的显式 `tr.val_init` 重复且真实值恰为 0 时误判 → 传 val_init 或直接用 value_f。
439 [低] `sysoverlay/crates/win/src/settings/widget.rs:868` — 注释写"首 28 行""滑杆行 46"，实际 `ROW_H=34`、`ROW_SLIDER_H=48`(layout.rs:90/:106) → 更新为引用常量。
440 [中] `sysoverlay/crates/win/src/settings/widget.rs:941` — 与 `mod.rs:1347` 两份相同的 u16→char 还原（代理对拆 U+FFFD）→ 用 `String::from_utf16_lossy` 并抽公共函数。
441 [低] `sysoverlay/crates/win/src/settings/widget.rs:951` — `dp!=0` 分支固定 `format!("{val:.1}")`，dp=2 仍只显示 1 位小数 → 按 dp 生成精度。
442 [中] `sysoverlay/crates/win/src/settings/widget.rs:1074` — `KEYS` 硬编码 9 个 i18n key，与 `Metric` 枚举顺序无编译期绑定，新增指标不报错 → 由 `Metric::ALL` + key_name 派生并加 const assert。
443 [中] `sysoverlay/crates/win/src/settings/widget.rs:1075` — `KEYS[index.min(N_METRICS-1)]` 把越界 index 静默钳到最后一个，坏 index 显示错名 → 用 `.get()` 并留日志。
444 [中] `sysoverlay/crates/win/src/settings/widget.rs:1146` — `paint_diag_row` 只检查 `d.last()` 一条，若最后一条属别的采样器，本行既有故障仍显示 DiagOk → 按 sampler 过滤后取最新一条。
445 [中] `sysoverlay/crates/win/src/settings/widget.rs:1150` — `SampleOutcome::Skipped` 被渲染成 "DiagOk"（正常色），用户看不到采样被跳过 → 单独文案/次级色。
446 [中] `sysoverlay/crates/win/src/settings/widget.rs:1153` — 诊断原因用 `1/2/3/4` 裸魔数映射 DiagNoDriver/DiagPerm/… → 提具名错误码常量并注明来源。
447 [中] `sysoverlay/crates/win/src/settings/widget.rs:1178` — dot_r 与 :1182/:1195 的 80/72/300/310/88 是裸物理像素未按 dpi 缩放，而同文件其余几何全用 `s(dpi,…)` → 全部改 s(dpi,…)。
448 [低] `sysoverlay/crates/win/src/settings/widget.rs:1252` — body_px/caption_px 转发函数与 `mod.rs:2583` 重复 → 删一份。
449 [中] `sysoverlay/crates/win/src/settings/widget.rs:1266` — slider_tests 全用 `8.0/256.0` 硬编码区间，与 `FONT_PX_MIN/MAX=4/128` 漂移 → 改引用常量并补 min==max/step=0 用例。
450 [低] `sysoverlay/crates/core/src/config.rs:204` — `render_style` 字段注释只列 Classic/Strip，漏 Vertical，与文件头 :26 及三变体不符 → 修正注释。
451 [中] `sysoverlay/crates/core/src/config.rs:223` — `render_engine` 注释"默认 Gdi"，但 `Default`(:336) 是 `RenderEngine::D2d` → 二选一。
452 [高] `sysoverlay/crates/core/src/config.rs:297` — 默认 `window_pos=Some((625,-9))` 硬编码且 y 为负，小屏/异 DPI 初始窗口顶部出屏，并使 `window.rs:1613/:1706` 兜底与贴角分支不可达 → 由屏幕尺寸计算或置 None。
453 [低] `sysoverlay/crates/core/src/config.rs:328` — `debug_log` 注释"默认开（诊断优先）"，`Default` 实为 false → 修正注释。
454 [中] `sysoverlay/crates/core/src/config.rs:355` — `metric_order` 默认手写 9 个下标，与 :265"容量唯一来源 Metric::COUNT"不符，新增指标 Default 不自动扩展 → 由 `Metric::ALL` 生成并加 const assert。
455 [中] `sysoverlay/crates/core/src/config.rs:396` — "version" 分支 `parse::<u32>()` 无上限、无 CONFIG_VERSION 校验，手写 `version 999999` 直接进入一致性检查 → 钳到合理上界并留痕。
456 [中] `sysoverlay/crates/core/src/config.rs:415` — size 解析下限 100/60、上限 2048 裸魔数（2048 亦见 `gdi.rs:686`）→ 提共享常量并补边界单测。
457 [中] `sysoverlay/crates/core/src/config.rs:440` — `font_scale` 迁移 `×16` 后只 clamp、无 0.5 步进量化，与 :434 的 `(v*2).round()/2` 分叉 → 复用同一量化函数。
458 [中] `sysoverlay/crates/core/src/config.rs:452` — :452/:457 的 `metric_gap`/`label_value_gap` clamp 上界 64 两处硬编码，与 `settings/layout.rs:156 GAP_MAX=64.0` 各自独立 → 提 core 常量供双方引用。
459 [中] `sysoverlay/crates/core/src/config.rs:462` — `normal_opacity` 解析 `clamp(0,100)`，与 :87 `OPACITY_PCT_MIN=15`（滑杆下限）矛盾；手写 5 生效但滑杆无法表达 → 统一区间常量。
460 [中] `sysoverlay/crates/core/src/config.rs:487` — parse() 内还有第五份 `Metric=>下标` 手写 match（首轮 023 只列四处）→ 改 `Metric::ALL.iter().position()` 并抽 `metric_index()`。
461 [高] `sysoverlay/crates/core/src/config.rs:515` — `cfg.metric_offsets[idx] = off` 用裸下标、未校验 < Metric::COUNT，破坏 :21"零 panic"承诺 → `.get_mut(idx)`。
462 [中] `sysoverlay/crates/core/src/config.rs:545` — `interval 0` 经 `snap_interval_ms(0)` 吸附到 100ms 最快档而非拒绝 → 0 视为非法并保留默认档。
463 [中] `sysoverlay/crates/core/src/config.rs:608` — `power_interval_ms` 的 `clamp(500, 60_000)` 硬编码，与 :195 注释、scheduler 口径无共享常量 → 提常量。
464 [低] `sysoverlay/crates/core/src/config.rs:659` — :659/:664"达到 3 次"阈值在 admin/driver 两处硬编码 `min(3)` → 提 `PRELAUNCH_SKIP_LIMIT`。
465 [低] `sysoverlay/crates/core/src/config.rs:681` — `set_size` 的 560/460/1400/1000 与 `settings/layout.rs:42` 的 MIN_W/MIN_H/MAX_W/MAX_H 逐字重复 → 由 core 常量下发。
466 [中] `sysoverlay/crates/core/src/config.rs:729` — `Metric::ALL.get(i).unwrap_or("")` 在 i 越界时写出 `msl_ 0` 空指标名垃圾行 → 跳过并留痕。
467 [低] `sysoverlay/crates/core/src/config.rs:855` — `split_kv` 与 `ini.rs` 一样只识别整行 `#`/`;`，`topmost 1 # 注释` 的 value 带尾注使 parse_bool 回退旧值 → 剥行内注释或改文档。
468 [低] `sysoverlay/crates/core/src/config.rs:890` — `parse_metric_color` 文档只列 cpu/mem/gpu/net/power 五个指标，:899 实现已支持 9 个 → 更新文档。

## 十八、`ui` crate：字体图集、渲染器、离屏光栅与测试有效性

469 [低] `sysoverlay/crates/ui/src/font.rs:206` — FONT_BITMAP 第 96 项（DEL 0x7F）是全空白槽，但 :303 判定上界 `<=0x7E` 恒取不到它，0x7F 反而回退 ■ → 删死条目或把上界改 0x7F。
470 [中] `sysoverlay/crates/ui/src/font.rs:220` — GLYPH_DOWN 墨迹落在 row1..8（index8=0x18），超出 :240 `GLYPH_VISIBLE_ROWS=8` 声明的 row0..7 → 把箭头收进 8 行或校正常量。
471 [中] `sysoverlay/crates/ui/src/font.rs:282` — `set_active_weight` 接受任意 u8 不校验，:306 越界静默兜底到粗(2)，:291 `set_weight_glyph` 越界静默忽略，两处口径不一 → 统一钳 0..2 并返回是否生效。
472 [中] `sysoverlay/crates/ui/src/font.rs:370` — `((py*surface_w+px)*4)` 为裸 i32 乘法，大表面溢出后 as usize 成巨值虽被 :371 拦下但整字漏画 → 改 i64/usize 安全计算。
473 [高] `sysoverlay/crates/ui/src/font.rs:381` — over 合成 out_r/g/b 无 `.min(255.0)`，而 :384 out_a 有；`as u8` 对 >255 回绕而非饱和 → RGB 同步钳位。
474 [中] `sysoverlay/crates/ui/src/font.rs:622` — 入口只判 `scale<=0.0`，NaN 不满足而放行，随后 :811 tile 尺寸 ceil→0，整字静默不画 → 加 `!scale.is_finite()`。
475 [低] `sysoverlay/crates/ui/src/font.rs:755` — `TileKey.a` 恒为 255（图集只在 :629 fg.a==255 时构建），却占键字段并进 :779 散列 → 删字段或真正支持半透明图集。
476 [高] `sysoverlay/crates/ui/src/font.rs:792` — `scale_bits` 用 `scale.to_bits()` 精确位型进键，字号浮点微抖会生成无穷多键、迅速填满 :761 的 256 槽 → 量化字号后进键。
477 [中] `sysoverlay/crates/ui/src/font.rs:813` — `vec![0u8;(tw*th*4).max(0)]` 中 tw*th*4 是 i32 乘法，大 scale 溢出为负→0 长度空瓦片 → 用 usize saturating_mul。
478 [中] `sysoverlay/crates/ui/src/font.rs:825` — 图集写满时"覆盖探测起点槽"而非 LRU，被踢字形下帧重建又踢别人，满表后持续抖动 → 引入计数/LRU 淘汰。
479 [低] `sysoverlay/crates/ui/src/renderer.rs:95` — pad 的 `if outline{2}else if shadow{1}else{0}` 与 :287 cell_sig 各写一份 → 抽 `CellSig::pad()`。
480 [中] `sysoverlay/crates/ui/src/renderer.rs:96` — c.w/c.h 在 cell_sig(:299/:316) 已含 2*pad 外扩，paint 的 clear_rect 又 +2*pad 重复外扩；配合 :89 跳过未变 cell，会误清相邻未重画 cell 的像素 → 去掉一次 pad。
481 [低] `sysoverlay/crates/ui/src/renderer.rs:230` — `RowList::is_empty` 带 `#[allow(dead_code)]` 且零调用（:1006/:1102 已 as_slice）→ 删方法。
482 [中] `sysoverlay/crates/ui/src/renderer.rs:301` — `cell_sig` 算出的 CellSig.hash（供 `gdi.rs:964` 脏判定）与 :907 `hash_cells` 门面指纹字段集合不一致且各自重算 → 抽公共 cell 哈希。
483 [中] `sysoverlay/crates/ui/src/renderer.rs:368` — :368 metric_colors、:381 metric_font_sizes、:832 metric_offsets 用裸 `[]` 索引，而 :797 metric_show_labels 用 `.get` 兜底 → 统一 .get 防长度不齐 panic。
484 [低] `sysoverlay/crates/ui/src/renderer.rs:417` — visible_rows 的 16.0 硬编码，未复用 `font::FONT_HEIGHT`(=16) → 引用常量。
485 [中] `sysoverlay/crates/ui/src/renderer.rs:499` — `push_f64_fixed` 用 `v as u64` 与 `(v-whole)*powi` 两段截断，1.999→"1.99"，无四舍五入也无进位 → 先 round 再拆。
486 [中] `sysoverlay/crates/ui/src/renderer.rs:557` — `metric_label` 的 mi（及 :584/:636/:664/:682/:723/:752/:778）全用字面量 0..8，与 :349 `metric_index` 手工映射重复耦合 → 改传 Metric 枚举。
487 [低] `sysoverlay/crates/ui/src/renderer.rs:743` — DiskRead/DiskWrite 两分支各写一份 `const DISK_SLOT: u32 = 9`，与 :138 的 pub SLOT_*、测试 :510 共三份 → 提到模块常量区。
488 [中] `sysoverlay/crates/ui/src/renderer.rs:826` — `slot_w = measure_scaled_f("0",scale)*slot` 用单字符宽×slot，忽略字符间距累加，系统性低估 (slot-1)*scale 像素 → 按 :602 公式算 slot 字符宽。
489 [中] `sysoverlay/crates/ui/src/renderer/tests.rs:39` — `renderers_render_pixels` 只断言 lit>0，任一渲染器画 1 个像素即通过 → 断言 cell 数或关键文字像素。
490 [中] `sysoverlay/crates/ui/src/renderer/tests.rs:74` — 几何/偏移用例全用 `layout(...,0,0)` 固定 h=0（注释 :73 自承只覆盖 PAD_Y 下限），`renderer.rs:1017` 的垂直居中分支零覆盖 → 参数化多组 h。
491 [中] `sysoverlay/crates/ui/src/renderer/tests.rs:135` — 用 `find(|c| c.text=="Mem")` 定位，依赖 short_names 默认关闭，默认值一改即 panic → 用 metric/right_align 定位。
492 [中] `sysoverlay/crates/ui/src/renderer/tests.rs:184` — 名为 `float_font_measure_matches_render`，却只断言 measure>0 且渲染非空，从未比对测宽与渲染步进 → 补逐字符右缘比对。
493 [中] `sysoverlay/crates/ui/src/renderer/tests.rs:311` — 右对齐锚点断言用 `measure_scaled_f`（位图等宽度量）复算，与被测实现同源，D2D 下比例字形真实右缘漂移抓不到 → 用独立度量或宽度桩。
494 [中] `sysoverlay/crates/ui/src/renderer/tests.rs:408` — 配色断言走 `Style::default().accent()`，生产是 `Style::derive(Rgb::default(),cfg.fg)`(:431)，两套入口分离会静默放过回归 → 用生产同一入口构造期望。
495 [中] `sysoverlay/crates/ui/src/renderer/tests.rs:447` — 边界用例产出 6 字符 "100.0%" 却断言 `slot==SLOT_PCT1(5)`，把槽位不足固化为期望 → 断言 slot>=文本宽或补越界用例。
496 [低] `sysoverlay/crates/ui/src/renderer/tests.rs:510` — `assert_eq!(rd.slot, 9)` 硬编码，与生产两处 DISK_SLOT=9 共三份魔数 → 断言具名常量。
497 [低] `sysoverlay/crates/ui/src/render.rs:6` — 模块头宣称 "Text rendering with font control"，但 `offscreen.rs:334` 与 `render_d2d.rs:712` 都忽略 FontSpec → 更新文档或注明未接线。
498 [中] `sysoverlay/crates/ui/src/render.rs:167` — trait 文档称文本按 at 矩形裁剪，实现 `offscreen.rs:345` 只横向裁、:343 丢弃 `_clip_h` → 补纵向裁剪或改文档。
499 [高] `sysoverlay/crates/ui/src/offscreen.rs:72` — `blend_pixel` 的 out_r/g/b 无 `.min(255)` 而 :75 out_a 有，`as u8` 回绕 → RGB 同步钳位。
500 [低] `sysoverlay/crates/ui/src/offscreen.rs:194` — `_point_in_rect` 带下划线且全模块零调用（圆角路径用 :206）→ 删除死函数。
501 [中] `sysoverlay/crates/ui/src/offscreen.rs:260` — `factor.clamp(0.0,1.0)` 对 NaN 返回 NaN，随后把整帧抹成透明 → 先 is_finite 过滤。
502 [中] `sysoverlay/crates/ui/src/offscreen.rs:288` — `fill_round_rect` 标准路径未 `clip_to_surface`（`fill_rect` :303 有），负/超大 r 会遍历巨大范围 → 先裁剪。
503 [中] `sysoverlay/crates/ui/src/offscreen.rs:451` — `test_fill_round_rect_small` 只断言中心 alpha>0，不验四角透明，恰好漏掉 :166 清角失效 → 补四角透明断言。
504 [低] `sysoverlay/crates/ui/src/palette.rs:43` — name_stale/value_stale 的 `desaturate(0.55)` 两处硬编码，:83 stale_accent 走 core 另一套系数 → 提常量统一。
505 [低] `sysoverlay/crates/ui/src/palette.rs:89` — Default 硬编码前景 `Rgb::new(238,241,244)`，与 :125/:127 测试及 core 默认值重复三份 → 引用 core 常量。
506 [中] `sysoverlay/crates/ui/src/palette.rs:107` — `spread=|c|(c.r-b).abs()` 只量 R-B 单轴，R==B 时恒 0，"更灰"断言退化 → 用三通道极差或饱和度。
507 [低] `sysoverlay/crates/ui/src/clock.rs:12` — 文档称 "CellSig::hash → 帧指纹"，实际帧指纹由 `renderer.rs:907` 重算、CellSig.hash 走 `gdi.rs:964` 脏判定，是两条链 → 厘清文档。
508 [中] `sysoverlay/crates/ui/src/clock.rs:43` — `set_then_get_roundtrips` 只测合法值，未覆盖 :28 `&0xFF` 静默收下的越界输入 → 补越界用例或断言。
509 [低] `sysoverlay/crates/ui/src/lib.rs:11` — 模块清单只列 render/offscreen/font/palette/renderer 五个，漏 :28 声明的 clock → 补上。
510 [中] `sysoverlay/crates/ui/src/lib.rs:21` — `#![cfg_attr(not(test), warn(clippy::unwrap_used))]` 用 warn 而非 deny，注释 :20 却称"禁 unwrap()" → 改 deny 或改注释。

## 十九、D2D / gfx / 渲染线程 / 图标 / Fluent token / 脏检查测试

511 [中] `sysoverlay/crates/win/src/gfx_d2d.rs:84` — `key()` 把 a clamp 到 0..1 量化，:98 `with_alpha` 直接写 `self.0.a=a` 不钳位，a=2 与 a=1 同缓存键但颜色不同 → with_alpha 同步钳位。
512 [中] `sysoverlay/crates/win/src/gfx_d2d.rs:191` — `LayoutKey` 首字段 String 使 :478 每次 draw_text 都 `text.to_string()` 堆分配做查表键，与 :188"静态标签逐帧重建纯属浪费"相悖 → 内部化文本或缓存哈希。
513 [中] `sysoverlay/crates/win/src/gfx_d2d.rs:284` — `pt(x,y)` 不校验 segs 坐标的 NaN/Inf 直接送 D2D sink，`icons.rs:267` 解析出的 ±inf 控制点会生成非法几何 → 加 is_finite 过滤。
514 [中] `sysoverlay/crates/win/src/gfx_d2d.rs:293` — 所有 EndFigure 都用 FIGURE_END_CLOSED，多个未闭合子路径（Move 触发的隐式结束）被强制连回起点 → 增补开放结束变体。
515 [中] `sysoverlay/crates/win/src/gfx_d2d.rs:324` — `let _ = sink.Close()` 忽略 HRESULT，Close 失败仍 :325 返回 Some(geom) 未定稿几何 → 判返回值。
516 [中] `sysoverlay/crates/win/src/gfx_d2d.rs:367` — `rebuild()` 只置 rt=None + 清 brushes，未清 layer（:230 注释声明随 RT 失效），清理被拆到 :354，两入口职责不一 → 把失效集中到 rebuild。
517 [中] `sysoverlay/crates/win/src/gfx_d2d.rs:417` — `format_key` 用 `(px*4.0).round()` 量化到 0.25px，与配置侧 0.5 步进不符产生双倍格式对象；px=NaN 时键恒 0 → 对齐步进并过滤 NaN。
518 [中] `sysoverlay/crates/win/src/gfx_d2d.rs:450` — `let _ = fmt.SetTextAlignment/SetWordWrapping` 忽略 HRESULT，设置失败仍以错误对齐/换行缓存复用 → 判返回值。
519 [中] `sysoverlay/crates/win/src/gfx_d2d.rs:488` — 布局缓存到 512 条即整表 clear()，而 :513 measure_text 也往同表插动态文本，会周期性冲掉全部静态标签缓存引起 shaping 尖峰 → 改 LRU/限额淘汰。
520 [中] `sysoverlay/crates/win/src/gfx_d2d.rs:757` — `create_rt` 把 GetClientRect/CreateHwndRenderTarget(:779) 失败都折叠成 None 无 HRESULT 留痕，设备异常时静默黑屏 → 记错误码。
521 [中] `sysoverlay/crates/win/src/gfx_d2d.rs:769` — dpiX/dpiY 恒 96.0 硬编码，与 :20"与 s(dpi,v) 体系无缝对接"矛盾，高 DPI 下 DWrite 字号按物理像素给却不随系统 DPI → 接 DPI 或注明局限。
522 [中] `sysoverlay/crates/win/src/render_d2d.rs:71` — `d2d_factory` 的 get_or_init 把创建失败的 None 也缓存进 OnceCell，偶发失败后本线程永不重试 → 失败不缓存或加重试。
523 [中] `sysoverlay/crates/win/src/render_d2d.rs:112` — FORMAT_CACHE 无容量上限也无淘汰，字号×字重组合持续堆入 IDWriteTextFormat，与 `gfx_d2d.rs:201 LAYOUT_CACHE_MAX` 口径不一 → 加上限/淘汰。
524 [低] `sysoverlay/crates/win/src/render_d2d.rs:273` — warmup 的 sizes 不去重（全局字号+9 个逐指标字号常重复），每个重复字号都做一次 1e6×1e6 CreateTextLayout → 先去重再循环。
525 [中] `sysoverlay/crates/win/src/render_d2d.rs:284` — 预热字符集不含实际渲染的 ↓↑ 箭头（renderer.rs:628/630），Net 行首帧仍触发 DWrite 回退查找，与 :278"覆盖实际会渲染的字符集"不符 → 补箭头与单位符号。
526 [中] `sysoverlay/crates/win/src/render_d2d.rs:357` — BindDC 失败用 `?` 提前返回，跳过 :365 gen 自增、:366 device_lost 复位与日志，旧 cache.rt 未清，每帧重试且无 HRESULT → 失败留痕并复位状态。
527 [中] `sysoverlay/crates/win/src/render_d2d.rs:440` — 字体路径用 `to_string_lossy()` 转 wide，含非 UTF-8 目录变 U+FFFD 致 CreateFontFileReference 静默失败、回退 Segoe UI → 用宽字符路径 API。
528 [中] `sysoverlay/crates/win/src/render_d2d.rs:445` — `load_custom_font` 链式 `.ok()?` 把每步失败折叠成 None 且仅成功(:459)有日志，DIN 缺失/损坏时静默降级无从诊断 → 逐步留 HRESULT。
529 [高] `sysoverlay/crates/win/src/render_d2d.rs:582` — `let _ = l.GetMetrics(&mut tm)` 忽略 HRESULT，失败时 tm 全 0 → actual_w=0，右对齐修正(:590)与自愈扩窗(:596)全基于 0 宽度 → 判返回值。
530 [中] `sysoverlay/crates/win/src/render_d2d.rs:611` — outline 与 shadow 用 else if 互斥，而 `ui::draw_row_text`(renderer.rs:877/891) 是两个独立 if 可叠加，同配置两引擎描边+阴影次数不一致 → 改为独立分支。
531 [中] `sysoverlay/crates/win/src/render_d2d.rs:641` — `end_draw` 对任意 EndDraw 错误都 mark_device_lost 重建 RT，而 `gfx_d2d.rs:547` 只对 D2DERR_RECREATE_TARGET；WRONG_STATE 重建无用且每帧重建 → 按错误码分支。
532 [中] `sysoverlay/crates/win/src/render_d2d.rs:683` — `fill_round_rect` 的 radius 未钳到短边一半（对比 `gfx_d2d.rs:146` 明确钳位并注释自交风险），大圆角矮矩形画出残角 → 复用钳位逻辑。
533 [中] `sysoverlay/crates/win/src/render_d2d.rs:717` — fallback 用 `s.len()`（字节数）×8 估宽，非 ASCII 高估数倍；且 :721 measure 用 Segoe UI 而绘制用 DIN，度量与绘制字体不一致 → 按字符数并从同字体度量。
534 [中] `sysoverlay/crates/win/src/render_thread.rs:46` — `start()` 对 `RENDER_TX.lock()` 失败静默跳过 Sender 发布仍 spawn 线程，request() 永远 send 失败；与 :107/:126 的 into_inner 恢复口径不一 → 中毒恢复或中止。
535 [中] `sysoverlay/crates/win/src/render_thread.rs:133` — 配置镜像未发布时 `let Some(cfg) else { return }` 直接静默跳帧，首帧黑洞难诊断 → 留一条 probe 日志。
536 [中] `sysoverlay/crates/win/src/render_thread.rs:176` — `RENDER_TX.lock().ok()` 把中毒当 None 丢弃（同锁在 :159 做了 into_inner 恢复），一次 panic 污染后所有 request 永久失败直到 30s respawn → 统一恢复。
537 [低] `sysoverlay/crates/win/src/din_bitmap.rs:33` — CELL_W/CELL_H=8.0 硬编码，注释 :31 却声明与 `ui::font::FONT_WIDTH`/`GLYPH_VISIBLE_ROWS` 对齐，ui 侧一改即错位 → 引用 ui 常量。
538 [低] `sysoverlay/crates/win/src/din_bitmap.rs:78` — run 闭包的 origin_y 参数两处调用(:110/:131)都传 0.0，是恒死参数 → 删除并直接用 0.0。
539 [低] `sysoverlay/crates/win/src/din_bitmap.rs:103` — `DWRITE_TEXTURE_TYPE(2)` 在 :103/:140 两处硬编码且各写一份"windows-0.58 未导出"注释 → 提具名常量。
540 [中] `sysoverlay/crates/win/src/din_bitmap.rs:186` — 日志无条件报 "weight {} injected"，不统计 rasterize_char 实际成功数，DIN 存在但全字符栅格化失败仍显示成功 → 打成功计数。
541 [中] `sysoverlay/crates/win/src/titlebar.rs:42` — `title_font` 高度用 `-(12*dpi)/72`，与 :33 `s(dpi,v)=v*dpi/96` 口径不同（96dpi 下 16px vs 12px），是第四套缩放 → 统一到 s()。
542 [中] `sysoverlay/crates/win/src/titlebar.rs:94` — `btn_rects` 未对 win_w 设下限，窄窗口下 `win_w-mr-w` 为负、RECT 反向 → 钳到 >=0。
543 [中] `sysoverlay/crates/win/src/titlebar.rs:150` — `CreateSolidBrush`/`CreatePen`(:153) 返回值未判空，GDI 资源耗尽时 FillRect/SelectObject 收无效句柄静默失败 → 判空回退。
544 [中] `sysoverlay/crates/win/src/titlebar.rs:166` — 标题 rect 右缘 `win_w - s(dpi,200)` 硬编码 200，窄窗口下 right<left 生成反向矩形 → max(left,…) 钳位。
545 [低] `sysoverlay/crates/win/src/titlebar.rs:275` — `draw_icon_btn` 的 `TitleBtn::Unlock => {}` 臂恒不可达（Unlock 走 :178 draw_unlock_btn）→ 删死分支。
546 [低] `sysoverlay/crates/win/src/settings/icons.rs:21` — VIEW=1024.0 硬编码，`gfx_d2d.rs:275` 又以文字约定 1024，两侧无单一真源 → 共享常量。
547 [中] `sysoverlay/crates/win/src/settings/icons.rs:86` — SOURCES/PATHS(:105)/:108 长度 14 三处硬编码，与 :81 index() 最大值 13 无编译期断言 → 用 const N 并断言。
548 [中] `sysoverlay/crates/win/src/settings/icons.rs:267` — `read_num` 用 `parse::<f32>()`，"1e999" 解析为 Ok(inf) 且无 is_finite 检查，Inf 坐标流入 PathSeg → 过滤非有限值。
549 [中] `sysoverlay/crates/win/src/settings/icons.rs:494` — draw 不约束 rect 为正方形，`gfx_d2d.rs:282` 的 sx/sy 独立缩放会非等比拉伸图标 → 钳等比或改文档。
550 [中] `sysoverlay/crates/win/src/settings/icons.rs:515` — `all_icons_parse_non_empty` 只断言 !segs.is_empty()，解析成孤立 Move 段（图形丢失）也通过 → 补最少段数/闭合断言。
551 [低] `sysoverlay/crates/win/src/settings/fluent/tokens.rs:77` — 多处注释以 "= 现有 CARD_PAD/CAPTION_GAP/ROW_GAP" 声明与 layout 常量一致，仅文字约定无断言 → 加 const 断言。
552 [低] `sysoverlay/crates/win/src/settings/fluent/tokens.rs:97` — `const fn ca`（带 alpha）定义后全文件零调用（所有 token 都用 :92 c()）→ 删除死函数。
553 [中] `sysoverlay/crates/win/src/settings/fluent/tokens.rs:211` — 注释自承 dark 与 light 非颜色 token 全同，却仍逐字段复制 20+ 值(:212-231)，改一处漏一处 → 用结构更新或共享常量。
554 [中] `sysoverlay/crates/win/src/settings/fluent/tokens.rs:294` — 字体/动效/曲线单测(:294/:321/:327/:334)只断言 FLUENT.light，dark 侧逐字复制的 20 个字段零覆盖 → 参数化 light/dark。
555 [中] `sysoverlay/crates/win/src/settings/fluent/tokens.rs:306` — `light_dark_colors_distinct` 只断言 2 个颜色 token 的浅深差异，其余 ~26 个（含 :195 fg_inverted、:199 stroke1）误填同值无锁 → 遍历比对。
556 [中] `sysoverlay/crates/win/src/gdi/dirty_tests.rs:15` — `cell()` 把 bbox_x 恒等于 x，无法构造二者分离（renderer.rs:294 右对齐情形），脏区对 bbox_x 的处理零覆盖 → 加独立参数。
557 [中] `sysoverlay/crates/win/src/gdi/dirty_tests.rs:16` — right_align 恒 false，右对齐数值 cell 的脏区/bbox 行为完全无测试 → 补 right_align=true 用例。
558 [中] `sysoverlay/crates/win/src/gdi/dirty_tests.rs:18` — text 恒空、靠手工改 :17 hash 数字模拟内容变化，未验证 cell_sig() 把文本/颜色编入 CellSig.hash 的链路 → 补哈希生成用例。
559 [中] `sysoverlay/crates/win/src/gdi/dirty_tests.rs:108` — `position_drift_is_full` 用 cell() 构造使 x 与 bbox_x 同步变化，测不到 bbox_x/w/h 单独漂移（对应 gdi.rs:959 只比 x/y）→ 加只改 bbox_x 的用例。
560 [低] `sysoverlay/crates/win/src/gdi/dirty_tests.rs:115` — `too_many_components` 只测 10 个分量(>8)，未测恰好 8/9 边界，折叠阈值 off-by-one 抓不到 → 参数化 7/8/9。
561 [中] `sysoverlay/crates/win/src/gdi/dirty_tests.rs:128` — 唯一一处写真实全局 `NEEDS_FULL_REUPLOAD`，其余状态都用 fresh_state()；断言中途 panic 会把闸门留在 true 污染后续测试 → 用 RAII 复位或改注入。

## 二十、指标采集、GPU 厂商后端、MSR 与功耗（首轮未覆盖行）

562 [中] `sysoverlay/crates/win/src/proc_cpu.rs:65` — idle/kernel/user 三个 delta 全用 `saturating_sub`，计数器回拨（休眠恢复/时间调整）时静默归零显示 0% → 任一倒退置 Unavailable 并 reset_baseline。
563 [中] `sysoverlay/crates/win/src/proc_cpu.rs:74` — 首帧 `prev=None` 未写任何数据仍返回 `SampleOutcome::Ok`，诊断/退避把零数据帧记成成功 → 首帧返回 Transient/Skipped。
564 [中] `sysoverlay/crates/win/src/proc_mem.rs:59` — used_gb/total_gb 由 f64 直接 `as f32` 且无 `ullTotalPhys==0` 兜底，异常/VM 下显示 0.0/0.0G → total>0 才写值。
565 [中] `sysoverlay/crates/win/src/proc_disk.rs:128` — 建立基线的 `PdhCollectQueryData` 返回值被 `let _` 丢弃，基线失败仍 valid=true → 失败记探针并维持待验证态。
566 [中] `sysoverlay/crates/win/src/proc_disk.rs:175` — 收集失败返回 Unavailable 却不清 last_read/last_write，下一帧命中 1Hz 节流(:154)会把上次成功值当新值返回 → 失败时两者置 None。
567 [低] `sysoverlay/crates/win/src/proc_disk.rs:193` — `read_counter_value` 与 :212 `write_counter_value` 除句柄成员外逐字重复 → 抽 `fn counter_value(&self, h: isize) -> Option<f64>`。
568 [低] `sysoverlay/crates/win/src/proc_net.rs:67` — 回调 `on_net_change` 在系统线程用 SeqCst store 一个纯标志位，:229 消费侧也用 SeqCst swap → 两侧降为 Relaxed。
569 [中] `sysoverlay/crates/win/src/proc_net.rs:232` — `enum_cache` 失败时不推进 last_enum_ms、nic_cache 仍空，`need_enum` 每帧为真 → 每周期重跑 ~28ms 全表枚举空转，失败也写 last_enum_ms 做退避。
570 [中] `sysoverlay/crates/win/src/proc_net.rs:283` — `dt<MIN_DT_MS` 时 rx_bps/tx_bps 保持 0 并写入 out.net，把"间隔太短"显示成 0 B/s 尖谷 → 沿用上帧速率或置 Unavailable。
571 [低] `sysoverlay/crates/win/src/proc_net.rs:316` — `oper_dbg` 全仓零调用方，靠 `#[allow(dead_code)]`(:315) 掩盖 → 删除或接入诊断输出。
572 [高] `sysoverlay/crates/win/src/proc_gpu.rs:67` — `available` 注释称"首次硬失败后置 false"，全文件仅在 `new()`(:78) 置 true、无任何写 false 路径 → 枚举失败每帧重跑 40~180ms，失败分支置 false 降级。
573 [中] `sysoverlay/crates/win/src/proc_gpu.rs:94` — `invalidate_topology()` 只清 TOPO_INVALID，不重置 :172 的 PRIMARY_LUID OnceLock，换主显卡后仍按旧 LUID 精确匹配 → 一并失效缓存。
574 [中] `sysoverlay/crates/win/src/proc_gpu.rs:219` — `node_count` 直接来自 D3DKMT 且无上限，`engine_3d_only=false` 时逐节点内核查询可能上万次 → 钳上限并 checked_mul。
575 [中] `sysoverlay/crates/win/src/proc_gpu.rs:232` — `D3DKMTQueryStatistics` 失败压入 rt=0 而非跳过，污染逐节点基线；下一帧真实值 delta 巨大 → 钳成 100% 假尖峰，失败沿用上帧或整帧作废。
576 [中] `sysoverlay/crates/win/src/gpu_detect.rs:65` — `Description` 用 `String::from_utf16_lossy`，非法 UTF-16 静默成 U+FFFD 且 :72 再截断 48 字符 → 按 NUL 截断后用 from_utf16，失败留探针。
577 [中] `sysoverlay/crates/win/src/gpu_vendor.rs:111` — 本轮 r.1/r.0 为 None（后端全失败）时不清 GPU_WATTS/GPU_UTIL，cached_util()/cached_watts() 持续返回陈旧值 → 失败时复位为哨兵/0。
578 [中] `sysoverlay/crates/win/src/gpu_vendor.rs:152` — `current_exe().ok()?.parent()?` 的 `?` 短路：取不到 exe 目录就整体返回 None，连 :160 系统 DLL 搜索与 :164 GetModuleHandleW 都不执行 → 把本地探测包在 if let 内，失败继续回退。
579 [中] `sysoverlay/crates/win/src/gpu_vendor.rs:188` — `Igcl{_init_ok: bool}` 本就自动 Send+Sync，却手写 `unsafe impl Send/Sync`(:186/:188) 绕过编译器校验 → 删掉这两行。
580 [中] `sysoverlay/crates/win/src/gpu_vendor.rs:225` — `nvmlInit_v2()` 成功后，:227 无设备 / :235 无可用句柄的 early-return 都不调 nvmlShutdown，NVML 全局状态残留 → 失败路径统一 shutdown 或 RAII。
581 [中] `sysoverlay/crates/win/src/gpu_vendor.rs:247` — NVML 利用率 `util.gpu as f32` 未钳 0..=100，与 ADL :570 的 clamp 口径不一致，驱动垃圾值直接进缓存 → 统一钳位。
582 [中] `sysoverlay/crates/win/src/gpu_vendor.rs:256` — NVML 功耗逐卡累加无单卡上限守卫（ADL :562 有 800W 闸门），垃圾 mW 直接并入整机功耗 → 加同一上限并留探针。
583 [低] `sysoverlay/crates/win/src/gpu_vendor.rs:419` — `stride` 由锚点算出后仅用于 :428 探针日志，收集循环(:461-484)完全不用，与 :416"相邻锚点即真实步长"注释不符 → 删死计算或真正使用。
584 [低] `sysoverlay/crates/win/src/msr.rs:48` — 注释称"ring0() 内部的同名局部 use 会遮蔽此处导入"，实际局部 use 在 open_ring0()，且 :135 仍重复引入 SC_HANDLE 与 :50 冲突 → 去掉重复导入并修正函数名。
585 [中] `sysoverlay/crates/win/src/msr.rs:361` — `ring0()` 在 open_ring0() 失败、`*g=None` 后仍返回 Some(MutexGuard)，语义误导；:359 还无条件清 RING0_STALE 吞掉唤醒自愈标记 → 仅成功时清 stale 并返回 Option<&Ring0>。
586 [中] `sysoverlay/crates/win/src/msr.rs:414` — teardown 的 try_lock 失败分支既不 take()，注释却称"其调用返回后 Ring0 的 Drop 仍会 CloseHandle"；Ring0 仍被静态 Mutex 持有且 RING0_TORN 使 ring0() 永不替换 → 置待关闭标志或改注释。
587 [中] `sysoverlay/crates/win/src/msr.rs:525` — invalid 连续异常计数只被 :622 的日志文本读取，从不触发降级/重探/放弃，与"连续异常计数"名实不符 → 接阈值决策或删字段。
588 [高] `sysoverlay/crates/win/src/msr.rs:554` — PKG 读失败用 `?` 直接返回 None，绕过 :522 注释承诺的"沿用 last_watts 避免掉成 `--`" → 失败时 return self.last_watts。
589 [中] `sysoverlay/crates/win/src/msr.rs:635` — `unit_j = 1/(1<<n)` 未校验 n 范围，驱动返回垃圾 u（n=0）时 1J/计数把功耗算爆 → 钳 n 到 0..=20，越界拒绝本帧。
590 [中] `sysoverlay/crates/win/src/power_system.rs:31` — `cpu_watts_global()` 全仓零调用方；CPU_WATTS_CENTI(:22)/store_cpu_watts(:25) 是只写不读的死状态，"进程功耗分摊"无消费端 → 接通读侧或删整条链。
591 [中] `sysoverlay/crates/win/src/power_system.rs:80` — `set_gpu_watts` 直接透传 `Option<f32>`，未像 :26 store_cpu_watts 一样钳有限性/上界，NaN 经 :110 并入 total → 过滤 !is_finite() 并钳上限。
592 [中] `sysoverlay/crates/win/src/sampler.rs:40` — `backoff: [Backoff; 5]` 与 SamplerId 变体数硬绑定，:352 `id as usize` 直接下标，新增变体即越界 panic → 用 `SamplerId::ALL.len()` 或编译期断言。
593 [中] `sysoverlay/crates/win/src/sampler.rs:214` — vendor 失败分支先跑 D3DKMT(:212) 再用 cached_util() 覆盖 snap.gpu；该缓存是上一轮旧值且失败时不失效 → GPU 利用率冻结，仅当 D3DKMT 也无值时才回退缓存。
594 [中] `sysoverlay/crates/win/src/sampler.rs:276` — `syspower.sample()` 返回 None 时仍把 cpu_watts()（上次成功值）发布给进程分摊，功耗已失效却继续参与计算 → 失败分支传 None。
595 [中] `sysoverlay/crates/win/src/sysinfo.rs:25` — 第二次 `GetSystemFirmwareTable` 的 got 从不与 size 比较，got<size 时尾部零字节被当作 SMBIOS 结构解析 → 按 got 截断 data。
596 [中] `sysoverlay/crates/win/src/sysinfo.rs:98` — `hd.is_invalid()` 为真时仍执行 :101 `CloseHandle(hd)`，对 INVALID_HANDLE_VALUE 调关闭 → 把 CloseHandle 移进有效分支。

## 二十一、系统层：日志、性能探针、自启、主题、DPI、时钟（首轮未覆盖行）

597 [中] `sysoverlay/crates/win/src/probe.rs:113` — `log_startup` 硬编码打印 `probe=1`，但 enabled() 可能只因 DEBUG 默认开而为真，日志里的环境变量状态是假的 → 打印 env_probe_force() 与 debug() 的实际值。
598 [低] `sysoverlay/crates/win/src/lib.rs:17` — 注释称"实测本 crate 生产代码 0 处命中 unwrap"，但 `gfx_d2d.rs:791` 有 `DWriteCreateFactory(...).unwrap()`，:19 的 warn 已实际告警 → 修掉该 unwrap 或更正注释。
599 [中] `sysoverlay/crates/win/src/perf.rs:137` — HAS_DATA 在 :96/:121 置 true 后从不复位，聚合报告输出后空闲期仍每 5s 打一行 frames=0 全零聚合 → 输出后 store(false)。
600 [中] `sysoverlay/crates/win/src/perf.rs:234` — `msg_end` 每处理一条消息就对整个 MSG_TRAIL 排序(:234/:239)，O(n log n) 落在消息热路径 → 改为一次有序插入/替换最慢项。
601 [中] `sysoverlay/crates/win/src/logging.rs:159` — `prefixed` 的 `with_capacity(text.len()+prefix.len()*4)` 按 4 行预估，perf 批量段行可达上万行 → 反复 realloc，按实际行数估算容量。
602 [中] `sysoverlay/crates/win/src/logging.rs:194` — `prune_plan` 把 mtime=0（:242 modified() 失败的兜底）判成 `e.1<cutoff`，未知时间的日志被当成 14 天前旧文件直接删 → mtime 未知时保留而非淘汰。
603 [中] `sysoverlay/crates/win/src/elevate.rs:52` — 提权重启用 `fMask=SEE_MASK_DEFAULT`（异步），而 `restart.rs:37` 明确加 SEE_MASK_NOASYNC 保证 exit 前新进程已派生 → 提权路径存在"两个实例都没了"竞态，补 NOASYNC。
604 [低] `sysoverlay/crates/win/src/restart.rs:15` — `quick_restart` 用 `current_exe()` 取路径，而 `elevate.rs:43` 用 `GetModuleFileNameW(None,[u16;512])`（会截断），同仓两处取自身 exe 口径不一 → 统一走 current_exe()。
605 [中] `sysoverlay/crates/win/src/mem_tune.rs:25` — LAST_TRIM_MS 初值 0 搭配 GetTickCount64，系统开机不足 60s 时 `now-0<60_000` 恒真，启动 30s 的一次性 trim(:38) 被永久挡掉 → 初值用 Option 或首帧豁免。
606 [中] `sysoverlay/crates/win/src/theme.rs:126` — `apps_use_light_theme()` 只在 `Theme::system()` 调用时读一次注册表，无 WM_SETTINGCHANGE/定时重探，运行中切换深浅色不跟随 → 订阅设置变更或每次 resolve 重读。
607 [中] `sysoverlay/crates/win/src/theme.rs:176` — `test_appearance_resolve_and_sidebar` 断言 `sys.dark==LIGHT.dark||sys.dark==DARK.dark` 恒真（resolve 只可能返回二者之一），测不到任何行为 → 改断言具体主题或删除。
608 [中] `sysoverlay/crates/win/src/app_icon.rs:26` — `pick_frame` 在 :25 声称"越界/畸形数据一律跳过、绝不 panic"，但全文件无任何 #[cfg(test)] 覆盖 0 帧 / off+len 越界 / 非 ICO 头 → 补畸形 ICO 单测。
609 [中] `sysoverlay/crates/win/src/tray.rs:81` — `remove()` 把 `Shell_NotifyIconW(NIM_DELETE)` 结果 `let _` 吞掉，注销失败残留托盘图标且无日志 → 判 as_bool() 并记探针。
610 [中] `sysoverlay/crates/win/src/menu.rs:35` — TrackPopupMenu 前未调 SetForegroundWindow，注释以"TPM_NOACTIVATE 不抢焦点"为由跳过；Win32 文档明确无此前台切换会导致点击菜单外不消失 → 弹菜单前临时置前再还原。
611 [低] `sysoverlay/crates/win/src/dpi.rs:8` — 模块头 :5 声称用 LOGPIXELSY 取 Y 方向 DPI，代码只导入/使用 LOGPIXELSX → 删描述或补 Y 方向。
612 [中] `sysoverlay/crates/win/src/dpi.rs:48` — screen_w()/screen_h() 注释称"工作区宽高"，实现用 SM_CXSCREEN/SM_CYSCREEN（含任务栏的全屏尺寸）→ 改 SPI_GETWORKAREA 或更正注释。
613 [中] `sysoverlay/crates/win/src/history.rs:32` — CPU 读数不可用时 `push(0.0)` 伪装成有效点，与 Metric::Unavailable 语义冲突，曲线出现假的 0% → 跳过该点或推哨兵并在读侧过滤。
614 [中] `sysoverlay/crates/win/src/autostart.rs:47` — `launched_at_logon` 用 `std::env::args()`，参数含非 UTF-8 字节时直接 panic → 改 args_os() + 按 OsStr 比较。
615 [中] `sysoverlay/crates/win/src/autostart.rs:85` — `value_exists` 用 `[0u16;512]` 定长缓冲，Run 值超长时 RegGetValueW 返回 ERROR_MORE_DATA 被当成"启动项不存在" → 按 needed 二次分配或改判存在性。
616 [中] `sysoverlay/crates/win/src/autostart.rs:242` — `schtasks_path` 把 SystemRoot 缺失时的回退硬编码 `C:\Windows`，非 C 盘系统盘上 schtasks 静默失效 → 用 GetSystemDirectoryW/GetWindowsDirectoryW。
617 [中] `sysoverlay/crates/win/src/autostart.rs:265` — `task_exists` 把 schtasks /Query 的任何非零退出（含拒绝访问、参数错误）都当"任务不存在"，mode() 静默退化到 Run 检查 → 区分"不存在"与真实错误。
618 [中] `sysoverlay/crates/win/src/autostart.rs:374` — `disable()` 用 `delete() && clear_approved_flag()` 短路，delete 失败时禁用标记不被清，下次开启仍被系统挡 → 各自执行后再汇总结果。
619 [中] `sysoverlay/crates/win/src/autostart.rs:427` — `apply_user_async` 每次调用都 spawn 一个后台线程，快速连点开关会并发跑多个 apply_user（各自弹 UAC/改注册表）→ 加单飞或请求序号去重。

## 二十二、`core` 纯逻辑、契约与快照的剩余区域

620 [低] `sysoverlay/crates/core/src/metrics.rs:59` — `rate_per_sec` 全仓零生产调用方（仅本文件单测引用），与 proc_net 自算速率重复 → 删除或接入唯一速率计算路径。
621 [中] `sysoverlay/crates/core/src/metrics.rs:82` — `ema` 不过滤 prev/next 的 NaN/Inf，单帧 NaN 使 EMA 永久变 NaN 且无自恢复 → 非有限输入时保持 prev 或重置。
622 [中] `sysoverlay/crates/core/src/cadence.rs:57` — `refresh_floor_is_1500` 断言 `REFRESH_FLOOR_MS==1500` 与 :20 常量定义同义，是恒真测试 → 改为断言派生关系（如 `1500%TICK_MS==0`）。
623 [中] `sysoverlay/crates/core/src/scheduler.rs:132` — `new()` 与 `set_user()`(:149) 各自硬编码 `clamp(100, 5_000)`，同一区间两份魔数 → 提具名常量 USER_MS_MIN/MAX。
624 [中] `sysoverlay/crates/core/src/scheduler.rs:185` — 非 Power 源全部用木桶 `effective_interval_ms()`(=base) 再 `.max(各自 min)`，CPU/Mem 被 GPU 2000ms 慢源拖到同节奏，与 :10"每个源独立分频"不符 → 真正按各源 min_effective_ms 独立到期。
625 [中] `sysoverlay/crates/core/src/scheduler.rs:228` — `update_ema` 不过滤 raw 的 NaN/Inf，单帧 NaN 永久污染 EMA，plan() 亦无防御 → 非有限输入跳过更新。
626 [低] `sysoverlay/crates/core/src/sampler.rs:81` — `DIAG_CAP=64` 注释称"≈覆盖最近一分钟（1s × 5 采样器）"，64/5≈12.8 帧即约 13 秒，算术错误 → 更正为"约 13 秒"或据此调容量。
627 [中] `sysoverlay/crates/core/src/sampler.rs:182` — `clear()` 只复位 head/written，不清空 entries 数组；模块头称诊断随 MiniDump 带出，崩溃转储里仍含已"清空"的旧记录 → 显式覆写为 EMPTY。
628 [中] `sysoverlay/crates/core/src/sampler.rs:293` — trait 文档称 now_ms 是 "milliseconds since Unix epoch"，实际 `app/main.rs:577` 传的是 GetTickCount64()（开机以来单调毫秒） → 改述为单调时钟并统一时间源。
629 [中] `sysoverlay/crates/core/src/snapshot.rs:10` — `timestamp_ms` 注释称 Unix timestamp、:14 称墙钟 ~1.7e12，但 `win/sampler.rs:158` 与 uptime_ms(:159) 都赋同一个 GetTickCount64 值，两字段实际恒等 → 修正文档或让 timestamp_ms 走真实墙钟。
630 [低] `sysoverlay/crates/core/src/snapshot.rs:70` — `Metric::Stale` 生产代码零构造（仅 core/layout.rs:558 测试夹具），是只被 value()/map() 处理的死变体 → 接线或删除并简化 Metric。
631 [低] `sysoverlay/crates/core/src/composite.rs:203` — `Palette::derive` 的 ok/warn/hot/idle 是硬编码常量，与 :158/:186"由 (bg, fg) 两色派生整块配色"不符 → 收窄文档措辞或真正派生。
632 [中] `sysoverlay/crates/core/src/composite.rs:268` — `MeterConfig` 的 tau_*/spike_release 无校验，`Meter::new`(:309) 直接接受；负 tau 使 alpha(:268) 产出负系数、EMA 发散，且无 spike_release<=spike 断言 → 构造期钳位/校验。
633 [低] `sysoverlay/crates/core/src/composite.rs:281` — 注释称 Meter 是"5 个 f64 + 2 个整数"，实际 MeterConfig 6 个 f64 + 状态 3 个 f64 + bool + 2 个 u32 → 更正数字。
634 [低] `sysoverlay/crates/core/src/composite.rs:301` — `MeterSnapshot` 的 stutter_milli/spikes/grade 全仓零消费方（只有 smooth_milli/instant_milli 被用）→ 删除或接上 UI/诊断。
635 [中] `sysoverlay/crates/core/src/history.rs:29` — `Timeline::push` 不过滤 NaN/Inf，单点污染后 values()/max() 结果不可信且无净化出口 → 非有限值拒绝入缓冲或写哨兵。
636 [中] `sysoverlay/crates/core/src/color.rs:89` — `scale(factor)` 未过滤 NaN/Inf，clamp 对 NaN 返回 NaN、as u8 得 0，NaN 因子静默把颜色变全黑 → 非有限 factor 直接返回 self。
637 [中] `sysoverlay/crates/core/src/color.rs:145` — `Accent::for_pct(NaN)` 所有比较为 false，落到 else 的 Ok（绿色"正常"），掩盖坏读数 → NaN 时返回 Idle/不可用档。
638 [低] `sysoverlay/crates/core/src/units.rs:267` — `with_estimate_prefix` 全仓零调用方（fmt_power 自己处理 `~`）→ 删除死函数。
639 [中] `sysoverlay/crates/core/src/layout.rs:187` — `_has_power` 取了 input.rows.power 后全程未用，是残留死变量 → 删除。
640 [低] `sysoverlay/crates/core/src/contract.rs:61` — 注释称"与上面 1–9 同一基址"，但 :60 WM_AUTOSTART_DONE 已是 +10，号段描述漏一条 → 改为"1–10"。
641 [中] `sysoverlay/crates/core/src/contract.rs:245` — `migration_marker_tracks_version` 两侧同为 `format!("config_migrated_v{CONFIG_VERSION}")`，恒真，测不到"升版忘改名"的真实风险 → 断言固定历史字符串或改测派生关系。
642 [低] `sysoverlay/crates/core/src/types.rs:249` — 注释称 Util"追加在末尾（下标 6）"，但 DiskRead/DiskWrite(:261/:263) 已占 7/8，Util 不再位于末尾 → 更新描述。
643 [低] `sysoverlay/crates/core/src/types.rs:477` — 注释称功耗显示语义 A/B/C/D 四类，枚举只有 CpuPackage/Battery/Estimated 三个变体（缺 B）→ 补齐或改"三类"。
644 [中] `sysoverlay/crates/win/src/i18n.rs:333` — `lookup_hits_every_entry` 用 TABLE 自身当期望值，恒真，抓不到二分查找排序错位 → 改用独立硬编码的 key→文案对照。
645 [低] `sysoverlay/crates/core/src/lib.rs:24` — 注释写"本 crate 的 25 处命中全部位于 #[cfg(test)]"，25 是无法复核的历史统计值 → 去掉数字或改为不写死的描述。
646 [中] `sysoverlay/crates/core/src/types.rs:601` — `c_enum_ordering_is_preserved` 两测都是 `from_config("1")==from_config("single_col")` 式自比较，若 from_config 整体退化返回 None 仍通过，且未与 C 版数值序比对 → 断言具体枚举值。
647 [中] `sysoverlay/crates/core/src/config/mcfg.rs:158` — `mcfg_roundtrip` 从 Config::default() 起、只比 render_style 一个字段，parse 什么都没改也通过 → 用非默认 Config 并逐字段/文件数断言。
648 [中] `sysoverlay/crates/core/src/config/mcfg.rs:170` — 脱敏断言用精确串 `"admin_check_continue_count = 3"`，序列化格式一变（如无空格）即假通过 → 解析后再断言字段缺失。

## 二十三、引导程序 Rust 后端：命令、安装、驱动与配置扇出（首轮未覆盖行）

649 [中] `guide-app/src-tauri/src/lib.rs:146` — `guide_bootstrap` 的补齐结果只用 eprintln!，而 main.rs:2 在 release 用 windows_subsystem="windows" 无控制台 → 改走 tracing::info!/warn! 落进 guide_*.log 才能被日志面板读到。
650 [中] `guide-app/src-tauri/src/lib.rs:333` — `main_reload_config()` 丢弃 `notify_main_reload_config()` 结果且恒返回 Ok(()) → 让投递返回 bool 并映射错误码，前端才知 reload 没送达。
651 [低] `guide-app/src-tauri/src/lib.rs:345` — OverlayRect 的 screen_w/screen_h 计算并序列化，但前端 fetchOverlayRect 只读 x/y/w/h，全仓零读取 → 删除，或交给第 12 页做坐标越界校验。
652 [中] `guide-app/src-tauri/src/lib.rs:410` — `overlay_set_position` 把 x/y 直接写 system.cfg，不校验是否落在屏幕内 → 用 get_primary_screen_size() 钳制或拒绝屏外坐标。
653 [中] `guide-app/src-tauri/src/lib.rs:426` — `#[cfg(windows)]` 只包住 set_autostart_run_key，非 Windows 时仍写 sys.autostart 并返回 Ok → 非 Windows 分支返回"不支持"，避免谎报已开启自启。
654 [高] `guide-app/src-tauri/src/lib.rs:471` — Run 值用 `to_string_lossy()` 且只首尾加引号，非 UTF-8 路径/尾随反斜杠会写坏启动命令 → 用 OsStr::encode_wide 直写并规范引号转义。
655 [中] `guide-app/src-tauri/src/lib.rs:573` — `load_config_from_ini` 把"公共数值色"判据写死 `mon.items.len() == 9` 且取 items[0] → 改用 METRIC_COUNT 常量并按 id 显式取值。
656 [中] `guide-app/src-tauri/src/lib.rs:626` — `config_load` 把可能是 -1 的哨兵坐标原样下发，第 12 页输入面板会显示 -1 → 缺任一坐标时返回 null/不下发，与 core 半哨兵口径对齐。
657 [中] `guide-app/src-tauri/src/lib.rs:828` — `guide_view_logs` 默认 200 行，而 `bridge.js:287` 与 `GuideWizard.vue:186` 都是 400，且 .min(2000) 静默截断 → 统一默认值并把实际生效行数回传前端。
658 [低] `guide-app/src-tauri/src/lib.rs:835` — 日志扩展名 `ext == "log"` 大小写敏感，.LOG 文件被漏读 → 改 eq_ignore_ascii_case。
659 [中] `guide-app/src-tauri/src/lib.rs:974` — 注册 tauri_plugin_opener 但前端零调用，capabilities/default.json:8 还授 opener:default → 移除插件与权限，收敛 csp:null 下的能力面。
660 [中] `guide-app/src-tauri/src/notify.rs:91` — `notify_main_quick_restart` 先 PENDING.swap(false) 清掉挂起的 reload，无论投递成败 → 失败应把 PENDING 置回 true 或留痕，否则最后一次改动永不补发。
661 [低] `guide-app/src-tauri/src/notify.rs:27` — WM_RELOAD_CONFIG/WM_QUICK_RESTART 两个常量无 #[cfg(windows)]，非 Windows 构建 unused 撞 workspace lints → 加 cfg 或统一收进 windows 门。
662 [低] `guide-app/src-tauri/src/process.rs:42` — 又手写 `encode_utf16().chain(once(0))`（另见 :101/:223），与首轮 006 的三份重复同形状 → 统一调 contract::wide_of。
663 [中] `guide-app/src-tauri/src/process.rs:80` — `release_guide_single_instance` 的 CloseHandle 失败被吞，提权重启的新实例仍可能撞锁 → 失败至少 tracing::warn 一次。
664 [中] `guide-app/src-tauri/src/process.rs:171` — `to_u64(&kernel) + to_u64(&user)` 用普通加法，FILETIME 极端值在 debug 下溢出 panic → 改 saturating_add。
665 [中] `guide-app/src-tauri/src/process.rs:175` — 时间回拨回退值 `to_u64(&creation)+1` 未加 FILETIME_UNIX_EPOCH_DIFF，与 :174 主分支口径不一致 → 统一 epoch，避免 alive 变巨值、CPU 均值恒 0。
666 [中] `guide-app/src-tauri/src/process.rs:177` — 生命周期 CPU 均值未钳 100%，多核进程完成页可显示 >100% → .min(100.0) 或按核数归一。
667 [低] `guide-app/src-tauri/src/shortcut.rs:96` — 取不到 config_dir() 复用 ShortcutError::NoDesktop（文案"Failed to get desktop folder"）→ 新增 NoConfigDir 变体，避免排查被误导。
668 [中] `guide-app/src-tauri/src/guide_state.rs:69` — `reset()` 先 exists() 再 remove_file 是 TOCTOU，文件恰好消失时 Err 被当"清空失败" → 直接 remove_file 并把 NotFound 视为成功。
669 [中] `guide-app/src-tauri/src/guide_state.rs:60` — `save()` 读改写无锁，前端 scheduleBackup/flushBackup 可并发发两次 guide_backup → 后端加互斥或单调序号，防后写覆盖先写。
670 [低] `guide-app/src-tauri/src/install.rs:53` — `same_content` 把目标 exe 整块 fs::read 进内存算指纹（bytes 已在内存）→ 流式分块读哈希，压低安装器内存峰值。
671 [中] `guide-app/src-tauri/src/install.rs:80` — verify 失败后删半成品 target 的失败被吞，截断 exe 残留会被 installed_detected(lib.rs:180) 判为已装 → 删除失败升级为 Err。
672 [低] `guide-app/src-tauri/src/install.rs:132` — "logs" 目录名硬编码，与 `lib.rs:821 guide_log_dir()`、:268 各拼一份 → 提 logs_dir() helper。
673 [低] `guide-app/src-tauri/src/install.rs:171` — `write_uninstall_registry` 内部又定义一份 w()，与文件尾 :929 的 w() 及首轮 006 的三份共六份 → 统一 contract::wide_of。
674 [中] `guide-app/src-tauri/src/install.rs:179` — InstallLocation 经 `to_string_lossy()` 写注册表，非 UTF-8 用户名变 U+FFFD → 用 encode_wide 直写 REG_SZ。
675 [中] `guide-app/src-tauri/src/install.rs:339` — 脱敏完全依赖 USERPROFILE 环境变量，取不到时 :341 原样返回，:253"写盘前统一替换"承诺落空 → 回退用 dirs::home_dir()。
676 [高] `guide-app/src-tauri/src/install.rs:467` — `emit_traced` 失败走 `log_install(...,"失败",...)` 被 :260 计入 STEP_FAIL，安装成功却总结"失败 1" → 改用不计步数的日志出口。
677 [中] `guide-app/src-tauri/src/install.rs:520` — `InstallMode::from_arg` 未知字符串静默回落 Full，拼错 driver_only 会触发完整重装 → 命令层白名单校验或改返回 Result。
678 [中] `guide-app/src-tauri/src/install.rs:552` — `Heartbeat::start` 返回 None（已有心跳/spawn 失败）被 `_heartbeat` 直接忽略，安装继续但进度条静止 → None 时至少告警或降级提示。
679 [中] `guide-app/src-tauri/src/install.rs:624` — `get_driver_status()` 的 Err 被 `unwrap_or(false)` 当成"驱动不可用"，触发误导性的重试/跳过弹窗 → 区分探测失败与确实未安装。
680 [中] `guide-app/src-tauri/src/install.rs:637` — `StepResult.status` 用裸 &'static str，拼错（"OK"）编译通过，前端 `UNINSTALL_STATUS[s.status] || s.status` 原样显示 → 改枚举 + serde rename。
681 [中] `guide-app/src-tauri/src/install.rs:829` — RegOpenKeyExW 失败（权限不足）无条件记 skip"Run 键不可打开"→ 应记 fail，否则清单显示"无需处理"而自启残留。
682 [高] `guide-app/src-tauri/src/install.rs:843` — 卸载用 `remove_dir_all` 删整个开始菜单文件夹，创建时只放一个 .lnk → 会连用户同目录其它快捷方式一起删，应只删 .lnk 再试删空目录。
683 [中] `guide-app/src-tauri/src/driver.rs:115` — `stop_service` 中服务处于 RUNNING/PAUSED 等非 STOP_PENDING 状态就 break，随后 :124 直接 Ok(()) → 没停掉也算成功，应返回超时/错误。
684 [中] `guide-app/src-tauri/src/driver.rs:156` — `PCWSTR(lp.0).to_string()` 对 lpBinaryPathName 无长度上限，非 NUL 结尾会越界读 → 限长扫描或用已知 buf 边界。
685 [中] `guide-app/src-tauri/src/driver.rs:264` — 轮询里 OpenServiceW 任意错误都 break 当"已彻底删除"，注释只认 ERROR_SERVICE_DOES_NOT_EXIST → 判错误码，拒绝访问时记失败。
686 [中] `guide-app/src-tauri/src/driver.rs:338` — CreateServiceW 的镜像路径经 `to_string_lossy()`，非 UTF-8 变 U+FFFD → 服务指向坏路径、驱动永远起不来。
687 [中] `guide-app/src-tauri/src/driver.rs:429` — 非管理员 get_driver_status 直接返回全 false 的"正常状态"，上层无法区分没装与没权限 → 返回 Err 或带 reason 字段。
688 [低] `guide-app/src-tauri/src/driver.rs:29` — DriverError 五个变体文案中英混排（"IO error"/"Not administrator"/"SCM error" 与中文）→ 统一语言并挂稳定错误码。
689 [中] `guide-app/src-tauri/src/config.rs:33` — 编译期断言只钉 METRIC_COUNT == Metric::COUNT，:109-:137 两份手写 id↔key 映射无校验 → 主程序中间插入 Metric 时映射全错位，测试 :1204 只比长度抓不到。
690 [中] `guide-app/src-tauri/src/config.rs:65` — `parse_ini` 只跳过整行 #/;，`key = val # 注释` 会把注释并进 value → 解析 value 时剥离行内注释。
691 [中] `guide-app/src-tauri/src/config.rs:80` — `parse_bool` 用 matches! 大小写敏感，True/YES 判 false，而主程序 ini.rs 先 to_ascii_lowercase() → 两端读同一 cfg 结果不同。
692 [中] `guide-app/src-tauri/src/config.rs:219` — 默认 `lang = "en"`，而 index.html 是 zh-CN、全部引导文案中文 → 首次安装后主程序以英文启动，与引导承诺不一致。
693 [中] `guide-app/src-tauri/src/config.rs:390` — `patch_from_json` 的 orientation 无校验，非法值原样落盘（主程序 `_ => horizontal` 兜底），引导 UI 仍高亮竖版 → 加白名单。
694 [中] `guide-app/src-tauri/src/config.rs:395` — opacity 直接 `as u32` 无 clamp，JSON 大数回绕（load 的 :265 才钳）→ 写入前 clamp(0,100)。
695 [中] `guide-app/src-tauri/src/config.rs:461` — font_size 钳位硬编码 8.0..32.0，与主程序 FONT_PX_MIN/MAX(4/128) 不符 → 引用契约常量，避免 40px 被悄悄压到 32。
696 [中] `guide-app/src-tauri/src/config.rs:515` — font_name 只取 as_u64()，主程序支持 0.5 步进（:1157 测试就用 13.5）→ 前端传小数被静默忽略，应改 as_f64()。
697 [中] `guide-app/src-tauri/src/config.rs:709` — 清空逐项配色只在 patch 显式带 `per_item_colors:null` 时发生 → 漏传该键的补丁会留下旧逐项色遮住全局色，应明确缺省语义。
698 [低] `guide-app/src-tauri/src/config.rs:987` — `if full.len() == METRIC_COUNT` 在 :982-:986 补齐后恒真 → 删除死守卫。
699 [中] `guide-app/src-tauri/src/config.rs:1132` — `admin_check_continue_count = 3` 未测 >255，而首轮 248 已记引导侧 u32 / core u8 的口径差 → 补 256/65535 边界断言。

## 二十四、Vue 14 页向导、桥接与状态机（首轮仅抽样几页）

700 [低] `guide-app/src/wizard/pages/Page01Welcome.vue:7` — "30 秒完成配置"是硬编码营销数字，与 14 页向导/实际安装耗时无关 → 改中性文案或抽 i18n 键。
701 [低] `guide-app/src/wizard/pages/Page01Welcome.vue:10` — `style="margin-top: 120px"` 硬编码像素，绕开 `--sp-*` 阶梯且与窗口固定高度耦合 → 改用 token 组合。
702 [低] `guide-app/src/wizard/pages/Page02Privilege.vue:6` — 一长串内联样式含 `max-width:460px`，与 Page10 的 440px、Page08 的 calc 各自硬编码 → 抽到 .t-body 修饰类并统一宽度 token。
703 [中] `guide-app/src/wizard/pages/Page02Privilege.vue:4` — 权限说明页无 aria-live/role=status，切页后读屏不播报"需要管理员权限" → 给标题或 section 加 aria-live="polite"。
704 [中] `guide-app/src/wizard/pages/Page03Install.vue:19` — error 态下容器仍是 role="progressbar" 且无 aria-busy=false/错误语义 → 读屏以为仍在安装，按 status 切换 role 或补 aria-disabled 与错误描述。
705 [中] `guide-app/src/wizard/pages/Page03Install.vue:23` — aria-valuetext 直接绑 state.install.pct，后端每个 pct 变化（含心跳重发）都更新，与 :16 注释"避免每 5% 刷屏"不符 → 量化到 5%/10% 档位。
706 [中] `guide-app/src/wizard/pages/Page04Style.vue:21` — `onMounted` 里 launchMain() 与 ensureStyleApplied() 都 fire-and-forget 并行 → 主程序可能在预设写盘途中读配置，应串行 await 且接住错误。
707 [中] `guide-app/src/wizard/pages/Page04Style.vue:32` — 六张互斥卡片只各自 aria-pressed，.style-grid 无 role="radiogroup"/aria-label → 读屏不知是单选组。
708 [中] `guide-app/src/wizard/pages/Page04Style.vue:39` — 关键说明"降低约 8 倍显卡占用"只放 title 属性，键盘/触摸/读屏不可达 → 改 aria-describedby 指向可见提示，并去掉未复测的倍数。
709 [低] `guide-app/src/wizard/pages/Page04Style.vue:49` — 模板又特判 `s.key === 'E' ? 'GDI兼容' : s.label`，而 :10 的 STYLES 里 E.label 已是"GDI兼容" → 删重复文案。
710 [低] `guide-app/src/wizard/pages/Page05Orientation.vue:24` — 三个 .mini-preview__bar 是纯装饰 span，无 aria-hidden="true" → 读屏会念出三个空元素。
711 [中] `guide-app/src/wizard/pages/Page06Items.vue:26` — 禁用项只靠 `:tabindex="-1"` 与 CSS .disabled(opacity)，未设 aria-disabled → 读屏仍读作可交互复选框。
712 [中] `guide-app/src/wizard/pages/Page06Items.vue:33` — SVG 勾选图标 `stroke="#fff"` 硬编码白色，与 --surface-white 脱钩 → 深色/高对比下不可见，改 currentColor 或 token。
713 [中] `guide-app/src/wizard/pages/Page06Items.vue:37` — "驱动未安装"禁用原因标签与 checkbox 无 aria-describedby 关联 → 读屏不知为何禁用。
714 [中] `guide-app/src/wizard/pages/Page07Order.vue:44` — measurePitch 失败时 pitch 回落硬编码 58px，与 flex 均分的真实行高不符 → 拖动跳格，失败时禁用拖动或按实际 rect 重算。
715 [中] `guide-app/src/wizard/pages/Page07Order.vue:75` — `onUp` 里 `setTimeout(reset, 260)` 无 onUnmounted 清理，翻页后定时器仍持有组件闭包；260ms 与 `wizard.css:661` 的 240ms 过渡也不等 → 存句柄在卸载时 clear，或改 transitionend。
716 [中] `guide-app/src/wizard/pages/Page07Order.vue:124` — .drag-handle 是无 tabindex/role/键盘事件的 span，排序对键盘用户完全不可用（对比 Page06 已补键盘）→ 加 tabindex、role 与上下键重排。
717 [中] `guide-app/src/wizard/pages/Page08NameMode.vue:25` — `CYCLE.indexOf(cur)` 对未知模式返回 -1，`(-1+1)%4` 静默归一为 'follow' → 坏存档值被无声吞掉，应兜底并留痕。
718 [中] `guide-app/src/wizard/pages/Page08NameMode.vue:67` — 循环按钮无 aria-pressed/aria-live，键盘点击后读屏不播报新档位 → 补 aria-pressed 与 aria-live="polite" 的状态文本。
719 [中] `guide-app/src/wizard/pages/Page08NameMode.vue:81` — scoped style `height: calc(100% - 86px)` 硬编码 86px 对应 :59 的 hint 高度 → 文案换行/字号变化即网格错位，改用 flex 布局。
720 [低] `guide-app/src/wizard/pages/Page09Color.vue:18` — 逐项配置用 `JSON.parse(JSON.stringify(...))` 深拷贝，而 :12 用展开浅拷贝 → 同文件两种序列化口径，统一一个 clone helper。
721 [中] `guide-app/src/wizard/pages/Page09Color.vue:42` — 触发 sheet 的按钮无 aria-haspopup="dialog"/aria-expanded → 读屏不知会弹模态（Page11:47、Page12:63 同形）。
722 [中] `guide-app/src/wizard/pages/Page09Color.vue:54` — `label.color-picker` 内无可见文字，input[type=color] 的 accessible name 为空 → 补 aria-label（逐项行 :99/:103 的 title 挂在 label 上不保证成为输入框名）。
723 [中] `guide-app/src/wizard/pages/Page10Opacity.vue:14` — range 控件无关联 label/aria-label，也无 aria-valuetext("80%") → 键盘用户不知调的是透明度。
724 [中] `guide-app/src/wizard/pages/Page10Opacity.vue:21` — v-model.number 拖动中实时改 state，但只在 @change 落盘 → 拖动未松手就翻页/关闭时改动只留内存、主程序 cfg 未更新。
725 [低] `guide-app/src/wizard/pages/Page11FontSize.vue:10` — 滑条填充 `((fontName-8)/24)*100` 硬编码 8/24，与 :62 的 min=8/max=32、:79 的 8/32 三处重复 → 提 MIN/MAX 常量共用，越界时钳 fill。
726 [中] `guide-app/src/wizard/pages/Page11FontSize.vue:17` — 下发 font_value，但 `config.rs:515` 的 patch_from_json 只读 font_name → 无效载荷，删掉或后端明确忽略并注释。
727 [中] `guide-app/src/wizard/pages/Page11FontSize.vue:77` — 9 个逐项滑条全部无 aria-label，相邻 .per-item-name 未用 aria-labelledby 关联 → 读屏分不清是哪一项。
728 [低] `guide-app/src/wizard/pages/Page12Position.vue:5` — `import { cmd } from '../bridge.js'` 全文件未使用（只调 useGuide 的 setOverlayPos/fetchOverlayRect）→ 删掉。
729 [中] `guide-app/src/wizard/pages/Page12Position.vue:34` — parseCoord 只判 Number.isSafeInteger，不校验坐标是否落在屏幕内，而后端 OverlayRect.screen_w/h 已返回却无人读 → 用屏幕尺寸拒绝屏外坐标。
730 [中] `guide-app/src/wizard/pages/Page12Position.vue:47` — `await setOverlayPos(...)` 不检查其结果就 `showPos=false` 关闭面板 → 写盘失败时用户以为已应用，应接住失败并保留面板。
731 [低] `guide-app/src/wizard/pages/Page12Position.vue:80` — 表单校验错误复用 .install-error（wizard.css:491 的安装页专用类）→ 语义/配色绑定到了安装流程，应抽 .form-error。
732 [中] `guide-app/src/wizard/pages/Page13Autostart.vue:9` — pick 直接调 async `setAutostart(v)` 不接 Promise，连点两次会并发两次 autostart_set invoke → 加 in-flight 去重并接住结果。
733 [中] `guide-app/src/wizard/pages/Page14Done.vue:18` — v-if 让统计卡在异步数据到达时才插入，内联 --rise-delay:200/300ms 相对插入时刻而非页面进入时刻 → 标题动画早已结束，错峰节奏失真。
734 [中] `guide-app/src/wizard/GuideWizard.vue:70` — 用 `document.querySelector('.sheet-scrim')` 探测模态，Teleport defer 挂载/卸载帧内查询滞后 → Alt+→ 可能穿过刚打开的面板，改统一 open 状态计数。
735 [中] `guide-app/src/wizard/GuideWizard.vue:83` — `ensureInstallWatch()` 在 setup 顶层调用且三个监听常驻不 unlisten，onUnmounted(:106) 只移除 keydown/offClose → 向导重建后旧监听仍持有 state，补 offs 并在卸载时调用。
736 [中] `guide-app/src/wizard/GuideWizard.vue:89` — offClose 在 `await onCloseRequested` 之后才赋值，卸载早于 await 完成时 :108 的 `offClose && offClose()` 是空操作 → 关闭监听泄漏，用立即占位 + 幂等注销。
737 [中] `guide-app/src/wizard/GuideWizard.vue:153` — `onSave()` 不检查 saveConfig() 结果（其内部也吞 r.ok）仍 `closeStep=2` → 写盘失败也显示"已保存"并推进到退出确认。
738 [低] `guide-app/src/wizard/GuideWizard.vue:224` — 14 个进度点是无文本 span，未 aria-hidden="true" 也无 aria-current → 读屏逐个播报空元素，补 aria-hidden 与当前步语义。
739 [中] `guide-app/src/wizard/WizardSheet.vue:24` — FOCUSABLE 列表漏 [contenteditable]、role="button"、`<summary>` 等 → 焦点陷阱会跳过这些可聚焦控件。
740 [中] `guide-app/src/wizard/WizardSheet.vue:68` — `watch(() => props.open, ...)` 未设 immediate:true → 若面板以 open=true 初始挂载，Esc/Tab 陷阱与初始聚焦全部失效。
741 [中] `guide-app/src/wizard/WizardSheet.vue:89` — onBeforeUnmount 只 removeEventListener，open 状态下直接卸载不会还原 opener.focus()（与 :83 的还原逻辑不对称）→ 卸载时也还原焦点。
742 [中] `guide-app/src/wizard/WizardSheet.vue:107` — `:aria-label="title"` 而 title 默认空串(:9) → 未传标题的 sheet 得到 aria-label=""，读屏是无名对话框，应回退通用名或要求必填。
743 [中] `guide-app/src/wizard/WizardSheet.vue:117` — .sheet-body 是 overflow-y:auto 滚动区却无 tabindex="0"/role="region" → 键盘用户无法聚焦后用方向键滚动逐项面板/日志。
744 [中] `guide-app/src/wizard/bridge.js:137` — 用 `cmd in INVOKE_TIMEOUT_MS` 判键，Object.prototype 上的名字（constructor/toString）会误命中 → 改 Object.hasOwn。
745 [中] `guide-app/src/wizard/bridge.js:142` — `Promise.race` 的超时 setTimeout 在命令正常返回后未 clearTimeout，每次 invoke 都留一个最多 5s 悬挂定时器 → 存句柄并 finally 清理。
746 [中] `guide-app/src/wizard/bridge.js:165` — "命令未注册"检测只匹配 `/not found|Command/`，capability 拒绝（not permitted/forbidden）被归 UNKNOWN 且不上报 → 诊断面板零记录，补权限类错误匹配。
747 [中] `guide-app/src/wizard/bridge.js:268` — uninstallRun 用 keepConfig、:259 用 createShortcut，是全文件仅有的 camelCase 参数，其余命令参数均为 snake_case → 核对 Tauri v2 rename_all 默认并统一，否则 keep_config 反序列化失败。
748 [中] `guide-app/src/wizard/bridge.js:361` — `cmd.reloadConfig()` fire-and-forget，失败（主程序未起/权限被拒）无任何反馈 → 接住 r.ok 并走 reportBridgeError。
749 [中] `guide-app/src/wizard/composables/useGuide.js:153` — notify 的 `setTimeout(dismissToast, ttl)` 句柄未保存，手动关闭后仍悬挂 6/8/10s 定时器 → 高频诊断风暴会累积大量悬挂定时器，存 id 并在 dismiss 时 clear。
750 [中] `guide-app/src/wizard/composables/useGuide.js:230` — hydrate 不校验 cfg.style 是否为 A–F，坏值('Z')会让第 4 页六卡全不高亮且 ensureStyleApplied(:637) 静默跳过 → 加白名单并回落 'A'。
751 [高] `guide-app/src/wizard/composables/useGuide.js:864` — 卸载失败后 state.mode 仍为 'uninstall' 且 sealWrites 不解除（bridge.js:100 自承幂等不解除）→ 用户既不能重试卸载也不能回引导，只能重启进程。
752 [中] `guide-app/src/wizard/composables/useGuide.js:906` — install-done 后 `setTimeout(goTo(4), 500)` 无句柄/无清理 → 500ms 内用户已退出或换页仍被强制拉回第 4 页。

## 二十五、前端样式、全局入口与构建配置

753 [中] `guide-app/src/styles/base.css:28` — `:where(button,[tabindex]):focus-visible` 漏了 input[type=range]/color/number/a[href]/select → Page10/11/12/09 的输入框无可见焦点环。
754 [低] `guide-app/src/styles/base.css:68` — .t-aux 先 `font-weight: var(--fw-regular)` 又 `font-weight: 400`，后者覆盖前者；tokens.css:99 定义 700 → 同一规则自相矛盾，层级被硬编码架空。
755 [低] `guide-app/src/styles/base.css:399` — 注释称"纯色，不用渐变"，实现却是 linear-gradient 硬色标，且与 tokens.css:11"进度条、填充一律纯色"冲突 → 改注释或改实现。
756 [中] `guide-app/src/styles/base.css:416` — 全局隐藏滚动条（配合 wizard.css:288），键盘/读屏用户感知不到可滚动区域，且相关容器无 tabindex → 保留可见滚动条或补 tabindex=0 与 aria 提示。
757 [低] `guide-app/src/styles/tokens.css:83` — --font-family 前三项是 macOS/iOS 字体（-apple-system/SF Pro Display/PingFang SC），本项目 targets 只有 nsis/Windows → 实际总回退到 Segoe UI，字体栈顺序与平台不符。
758 [中] `guide-app/src/styles/tokens.css:99` — --fw-regular/medium/semi/bold 四个全等于 700，token 名失去区分意义，且 base.css:68/232 又硬编码 400 覆盖 → 恢复分级或合并成单一 token。
759 [中] `guide-app/src/styles/tokens.css:193` — --win-w:680px/--win-h:560px 与 tauri.conf.json:17 的 680/560 是两份独立副本，wizard.css 多处"680−80"算式也依赖它 → 加构建期断言或让 CSS 从单一来源取值。
760 [中] `guide-app/src/styles/liquid-glass.css:47` — 14 个玻璃类名的选择器列表在 :47/:65/:83/:209 逐字重复四份 → 新增一个玻璃控件要改四处，漏一处即无 ::before/::after/回退。
761 [高] `guide-app/src/styles/liquid-glass.css:106` — 无条件 `color:black` 覆盖 base.css:221/246 的 `color:#fff` → 主按钮蓝底黑字、danger 深红底黑字，对比度不足；仅 @supports 回退(:243)恢复白字。
762 [中] `guide-app/src/styles/liquid-glass.css:208` — @supports 只探 backdrop-filter，注释 :205 却断言"支持的引擎必然支持本路线全部特性" → 漏探 color-mix()/独立 translate/内联 SVG filter 能力。
763 [中] `guide-app/src/styles/wizard.css:28` — `@supports not (backdrop-filter:blur(1px))` 未带 -webkit- 前缀（对比 liquid-glass.css:208）→ 仅支持前缀的引擎误走实色回退，大窗失去透明。
764 [中] `guide-app/src/styles/wizard.css:33` — `.guide-window.translucent` 全仓唯一出现处即本行，GuideWizard.vue:200 的 class 是静态的 → "看得到桌面悬浮窗"的透明窗口从未生效。
765 [中] `guide-app/src/styles/wizard.css:160` — pageIn ... both 及 :164/:171/:175 全用 fill both，而 base.css:167、liquid-glass.css:24 实证要求 backwards → 页面切换期间祖先常驻 transform 动画会截死玻璃折射采样。
766 [低] `guide-app/src/styles/wizard.css:510` — .status-failed .page-title 全仓无对应类（错误态用 .install-error role=alert）→ 死规则，删除或接到真实状态。
767 [低] `guide-app/src/styles/wizard.css:583` — .gdi-row/.gdi-text/.gdi-title/.gdi-desc 四条规则全仓无组件引用（第 4 页已是六卡预设）→ 删除死样式。
768 [低] `guide-app/src/App.vue:24` — feSpecularLighting + feComposite 产出 litImage 后无任何节点引用，feDisplacementMap(:25) 只用 SourceGraphic/softMap → 高光链是白算的滤镜开销。
769 [中] `guide-app/src/App.vue:42` — 背景图无 background-color 兜底，webp 加载失败/系统禁图片时背景全透明 → 玻璃层失去注释 :40 自承的"高频折射信号源"。
770 [中] `guide-app/src/main.js:9` — `createApp(App).mount('#app')` 未设 app.config.errorHandler/warnHandler，也未监听 unhandledrejection → 生产包无 DevTools，渲染异常/未捕获 Promise 拒绝只表现为白屏或无响应。
771 [中] `guide-app/index.html:3` — `<head>` 未声明 color-scheme → 系统深色模式下 WebView2 的 range/color/number 控件用深色 UA 样式，与浅色设计冲突。
772 [中] `guide-app/vite.config.ts:28` — `host: host || "127.0.0.1"` 在设置 TAURI_DEV_HOST 时把 dev server 绑到外部地址，与 :26"显式只监听回环"注释矛盾 → 只把该变量用于 HMR 客户端，不改 server.host。
773 [中] `guide-app/vite.config.ts:38` — `ignored: ["**/src-tauri/**"]` 把 tauri.conf.json 与 capabilities/ 一起屏蔽 → 改窗口 label/权限后 tauri dev 不重建上下文，前端仍用旧权限。
774 [中] `guide-app/package.json:10` — build 只有 check-bridge + vite build，不跑 vue-tsc/tsc → tsconfig.json:18 strict/:19 noUnusedLocals 形同虚设（如 Page12Position.vue:5 未使用 import 不会被拦）。
775 [中] `guide-app/package.json:18` — devDependencies 无 typescript/vue-tsc/@types/node，却存在 tsconfig.json、tsconfig.node.json 和 vite.config.ts:3 的 @ts-expect-error → 类型检查工具链根本没装。
776 [中] `guide-app/tsconfig.json:23` — include 列了 src/**/*.vue，但项目无 vue-tsc，裸 tsc 不解析 .vue 单文件组件 → 这些 .vue 永远不会被检查，配置给出虚假的类型覆盖承诺。
777 [中] `guide-app/src-tauri/tauri.conf.json:19` — resizable:false + 固定 680×560，与 tokens.css:193 及 wizard.css 多处"680−80"算式耦合 → 高 DPI/系统文本缩放下无最小尺寸与自适应兜底。
778 [中] `guide-app/src-tauri/capabilities/default.json:8` — opener:default 对应插件在 lib.rs:974 注册但前端零调用 → 权限白授予，移除该权限与插件以收敛能力面。
779 [中] `guide-app/scripts/check-bridge.mjs:31` — 命令名正则 `^[a-z_][a-z0-9_]*$` 只认 snake_case → 后端若改用 camelCase 命令名（Tauri 2 允许）会被静默跳过并误报。
780 [中] `guide-app/scripts/check-bridge.mjs:85` — 门禁只比对"命令名是否在注册表"，不检测封装函数是否被实际调用 → cmd.privilegeCheck(bridge.js:263) 定义了却全仓无调用点，这类死封装门禁零感知。
781 [低] `guide-app/README.md:1` — 内容完全是 Tauri+Vue+TypeScript 官方模板 README（"This template should help..."），而项目向导是 .js 非 TS、有 14 页向导/驱动安装/卸载 → 重写为项目说明并列出运行前提（管理员权限、WinRing0、仅 Windows）。

## 二十六、工程化：Cargo / build.rs / CI / 测试有效性（首轮几乎未触及）

782 [中] `Cargo.toml:15` — `[workspace.package]` 只有 version/edition/license，缺 rust-version；README.md:106 口头要求 Rust 1.96.0+，cargo 不会校验该下限 → 补 `rust-version = "1.96"` 或加 rust-toolchain.toml。
783 [低] `Cargo.toml:33` — 注释称 panic=unwind 只因 gdi.rs 的 catch_unwind，但 win/src/lib.rs:73 自述有 gdi/msr/render_thread/app 四处 → 补全或改为"多处自愈"不点名单文件。
784 [中] `Cargo.toml:39` — `strip = true` 会剥掉符号，而 app/src/main.rs:58 的 minidump 意图是崩溃可排障 → 注明 release 产物无符号，或为 dump 单独保留 debug 信息。
785 [中] `sysoverlay/Cargo.toml:1` — 整个文件只有 3 行注释、无 [package] 无表，却叫 Cargo.toml，易被工具/cargo 误认 manifest → 改名 README.workspace.md 或删除，说明并入根 README。
786 [低] `sysoverlay/Cargo.toml:3` — 声称"本目录残留的 Cargo.lock 已不再生效"，但 `sysoverlay/Cargo.lock` 并不存在，真正残留的是 `sysoverlay/backup/noprobe-src/Cargo.lock` → 更正指向或删句。
787 [中] `sysoverlay/crates/app/Cargo.toml:11` — `minidump = []` 默认不启用，且全仓无 `--features minidump` 构建路径，但 main.rs:10 文档把 minidump 列为既有职责 → 落一个启用点或把文档改成"可选、默认关闭"。
788 [中] `sysoverlay/crates/app/Cargo.toml:24` — `arc-swap = "1.7"` 在 `crates/app/src` 全目录零引用（只有 win crate 用）→ 删除该依赖。
789 [中] `sysoverlay/crates/app/Cargo.toml:31` — 注释自承 Storage_FileSystem/Kernel/Memory 是 minidump 专属，却无条件写进 [dependencies]，默认构建白编译 → 用可选依赖或按 feature 门控。
790 [中] `guide-app/src-tauri/Cargo.toml:40` — `windows = "0.61"` 与 app/win 的 `0.58` 并存，Cargo.lock:4628/4638 同时锁 0.58.0 与 0.61.3，编译两份 → 统一到单一版本或注明为何必须双版本。
791 [低] `sysoverlay/crates/ui/Cargo.toml:15` — 注释"生产构建不应引入 png"，但 Cargo.lock:2702/2715 有 png 0.17.16 与 0.18.1 两份 → 收窄为"ui 自身不引入"，避免绝对化表述。
792 [中] `Cargo.lock:2702` — 与 :2715 同时锁 png 0.17.16 / 0.18.1（外加 windows 双版本、双 bitflags），重复编译 → 在 Cargo.toml 侧对齐版本后重生成锁文件。
793 [中] `sysoverlay/.cargo/config.toml:1` — 配置放在 `sysoverlay/.cargo/`，而 workspace 清单已上移到仓库根（README.md:112 要求从根跑 cargo），cargo 不会读此目录 → 移到仓库根 .cargo/config.toml 或标注仅历史有效。
794 [中] `sysoverlay/.cargo/config.toml:5` — linker 写死 `...MSVC\14.44.35207\bin\Hostx64\x64\link.exe`，VS 小版本升级或换机即失效 → 改用 link.exe + PATH 或 vcvars 推导。
795 [低] `sysoverlay/.cargo/config.toml:7` — `/LIBPATH:C:\Program Files (x86)\...` 路径含空格却未加引号 → 用引号包裹 link-arg 值。
796 [中] `sysoverlay/.cargo/config.toml:8` — Windows Kits Lib 写死 `10.0.26100.0`，而 crates/app/build.rs:79 动态扫描最大版本目录，两处 SDK 口径不一致 → 统一为动态探测。
797 [中] `sysoverlay/crates/app/build.rs:45` — `env::var("OUT_DIR")` 失败直接 return，既不发 warning 也不声明 rerun，静默跳过图标 → 补一条 cargo:warning=。
798 [中] `sysoverlay/crates/app/build.rs:51` — `ico.to_string_lossy()` 对非 UTF-8 路径产出 U+FFFD，rc.exe 拿到错路径 → 用 to_str() 失败即 warning 退出。
799 [中] `sysoverlay/crates/app/build.rs:71` — `rerun-if-changed=ico` 只在 `ok && res_file.exists()` 分支打印；rc.exe 失败时不声明任何重跑条件，改 ico 后行为与成功分支不一致 → 无条件声明 ico + build.rs。
800 [中] `sysoverlay/crates/app/build.rs:85` — 版本号只取第三段排序，多个 SDK 目录第三段相同（或缺失→0）时 `build > *b` 恒 false，固定取第一个而非最新 → 用完整四段元组比较。
801 [中] `guide-app/src-tauri/build.rs:24` — 无条件 `cargo build --release -p aaxsyspulse-app`，连 cargo check 都触发完整 release 编译 → 用 rerun-if-changed 或按 PROFILE 跳过，只在打包时编译。
802 [中] `guide-app/src-tauri/build.rs:47` — 手写 parse_version 读根 Cargo.toml，而 :77 已有 `env!("CARGO_PKG_VERSION")`，两处版本来源重复 → 直接以 CARGO_PKG_VERSION 为真源。
803 [中] `guide-app/src-tauri/build.rs:57` — rerun-if-changed 只声明 package.json / tauri.conf.json / crates / build.rs，漏了 :47 实际读取的版本源根 Cargo.toml → 补 `rerun-if-changed=../../Cargo.toml`。
804 [中] `guide-app/src-tauri/build.rs:64` — 驱动资源名硬编码 "WinRing0x64.sys"，与 contract::DRIVER_FILE_NAME(resources.rs:19)、tauri.conf.json:36 构成第三份副本 → 改契约名即构建期漏检，应复用常量。
805 [中] `guide-app/src-tauri/build.rs:105` — `json_version` 用 `split("\"version\"").nth(1)`，会命中任意层级的 "version" 字符串（如依赖块内）而非顶层 → 用 serde_json 取顶层字段。
806 [高] `guide-app/src-tauri/build.rs:116` — `check_driver_resource` 读不到 WinRing0x64.sys 即 panic，但 .gitignore:11 全局忽略该名、resources/ 无任何 git 跟踪文件 → 全新 clone/CI 必失败；需提交真实资源或把校验降级为 warning。
807 [中] `guide-app/src-tauri/build.rs:129` — `parse_version` 用 `split("[workspace.package]").nth(1)` 手写 TOML 切段，注释里出现同名串或段落顺序调整即误取 → 用 toml 或 CARGO_PKG_VERSION。
808 [中] `guide-app/src-tauri/build.rs:136` — `parse_version` 用 `strip_prefix("version")` 不校验后随字符，任何以 version 开头的键（version_note）会让 :137 的 strip_prefix('=')? 提前返回 None → 整个版本解析失败、构建中断。
809 [高] `.gitignore:11` — `WinRing0x64.sys` 被全局忽略，却是 build.rs:64 的构建期硬依赖 → 取消忽略或显式 git add -f 资源文件。
810 [高] `.gitignore:21` — `*.ico` 被忽略，而 crates/app/build.rs:34 依赖 win/assets/app_icon.ico → 为 assets 图标加 ! 豁免（首轮 265 只提了 build.rs 侧，此处是根因）。
811 [低] `.gitignore:39` — 与 :51 重复声明 `.trae/` 两次 → 删去其一。
812 [中] `.github/workflows/build-test.yml:34` — `dtolnay/rust-toolchain@stable` 滚动最新版，叠加仓库无 rust-toolchain.toml，与 README.md:106 的 1.96 下限无法复现 → 固定版本或读取 toolchain 文件。
813 [中] `.github/workflows/build-test.yml:43` — `hashFiles('**/Cargo.lock')` 会命中 sysoverlay/backup/noprobe-src/Cargo.lock 备份锁，改备份内容即让 CI 缓存失效 → 收窄为 Cargo.lock。
814 [低] `.github/workflows/build-test.yml:47` — 注释称 sysoverlay/Cargo.toml 是"app crate 的成员清单"，实际该文件只有 3 行注释，members 在根 Cargo.toml:7 → 更正。
815 [中] `.github/workflows/build-test.yml:49` — check/clippy/test 三步都会执行 guide-app/build.rs 的内层 release 构建，主程序被重复编译三次 → 合并步骤或缓存 target/main-dist。
816 [高] `.github/workflows/build-test.yml:56` — `cargo test --workspace` 触发 build.rs:64 的驱动资源断言，而 WinRing0x64.sys 未入库 → CI 必然 panic；需先解决资源入库。
817 [中] `sysoverlay/crates/app/src/main.rs:1` — `#![windows_subsystem = "windows"]` 下，文件内 :48/:123/:134/:175/:196/:239/:370/:820 八处 eprintln! 全部丢进空控制台 → 统一改走 probe::log/日志文件。
818 [低] `sysoverlay/crates/app/src/main.rs:10` — Responsibilities 第 6 条写 "Panic minidump on crash"，但 :50 的 #[cfg(feature = "minidump")] 默认关闭 → 标注"仅 minidump feature"。
819 [中] `sysoverlay/crates/app/src/main.rs:58` — write_minidump 整段从未在默认构建编译，:108 注释自承"此前从未对本 windows 版本编译通过" → 至少加一条 --features minidump 的编译/检查路径。
820 [中] `sysoverlay/crates/app/src/main.rs:97` — `dump_path.to_string_lossy().as_ref()` 对含非 UTF-8 的用户目录产出替换字符 → 用 OsStr::encode_wide() 直接编码。
821 [低] `sysoverlay/crates/app/src/main.rs:167` — `--elevated` 与 `--restart` 各做一次 `std::env::args().any`，重复全量扫描 → 一次 args().collect() 后判定。
822 [中] `sysoverlay/crates/app/src/main.rs:402` — `diag.split(',').any(|t| t == token)` 不 trim，`AAXSYSPULSE_DIAG=settings, click` 的 click 永远匹配不上 → 逐 token .trim()。
823 [中] `sysoverlay/crates/app/src/main.rs:410` — diag 自动点击线程用 2500ms + 9×3000ms 裸魔数，且无停止/幂等，重复触发会起多个线程 → 提常量并加一次性开关。
824 [低] `sysoverlay/crates/app/src/driver_installer.rs:50` — 文档说 Ok(false) 仅表示"找不到/不能运行"，但 :93 用户取消（退出码 3）也返回 Ok(false) → 用枚举或新增变体。
825 [低] `sysoverlay/crates/app/src/driver_installer.rs:85` — 该注释行尾有尾随空格 → 清理（顺带全仓 git diff --check）。
826 [中] `sysoverlay/crates/app/src/lib.rs:1` — 空 lib target 会被 Cargo 自动编译，而 app 的测试写在 bin（main.rs `mod pre_launch_checks`），`cargo test --lib` 跑不到 → 在 lib.rs 注明或把纯函数测试移入 lib。
827 [中] `sysoverlay/crates/app/src/pre_launch_checks.rs:71` — `is_multiple_of` 需较新 Rust 才稳定，但仓库无 rust-toolchain.toml、workspace.package 无 rust-version → 落实 MSRV 声明或改回 `% 4 == 0`。
828 [中] `sysoverlay/crates/app/src/pre_launch_checks.rs:265` — 注释称"四个检查闸门的正确性建立在这几个函数上"，但 should_check（3 次跳过 + 每周一次）的两个组合闸门零测试 → 补纯逻辑闸门单测。
829 [中] `sysoverlay/crates/app/src/pre_launch_checks.rs:348` — `assert_eq!(time_since(u64::MAX), 0)` 后紧接 `assert!(time_since(u64::MAX) < WEEK_MS)`，前者已断言为 0、后者恒真 → 删冗余断言或改测真实闸门。
830 [中] `sysoverlay/crates/app/src/pre_launch_checks.rs:356` — 用 `(2026..=2100).contains(&y)` 断言墙钟年份，时钟被调回 2025 或 2101 年后即失败 → 改为与 now_ms() 推出的年份比较。
831 [中] `sysoverlay/crates/app/src/pre_launch_checks.rs:365` — current_date() 与 now_ms() 两次独立取墙钟，跨 UTC 0 点时 `now - midnight` 断言 flaky → 固定一次时钟快照。
832 [中] `sysoverlay/crates/app/src/pre_launch_checks.rs:379` — `time_since(now_ms() + 10 * WEEK_MS)` 断言 0 是 saturating_sub 的必然结果；:381 又用 `WEEK_MS - 5_000` 魔数容差 → 改测 saturating_sub 语义并提常量。
833 [中] `sysoverlay/crates/win/src/lib.rs:78` — `profile_guard_tests` 靠 `read_to_string(concat!(..., "/../../../Cargo.toml"))` 读磁盘 TOML 文本，而非 `env!("CARGO_PROFILE_RELEASE_PANIC")` → 改用编译期注入的 profile 变量。
834 [中] `sysoverlay/crates/win/src/lib.rs:79` — 三级 `..` 相对路径硬编码 crate 与仓库根的位置关系，目录一移动即 panic → 用 CARGO_MANIFEST_DIR 逐层 join 并断言存在。
835 [中] `sysoverlay/crates/win/src/settings/motion.rs:167` — `max > 1.0 && max < 1.15` 把过冲上界写死，改 K_SHEET/C_SHEET 即误报 → 由阻尼比推导期望或至少提常量。
836 [中] `sysoverlay/crates/win/src/settings/motion.rs:181` — `project(2000.0) > project(1000.0)` 只测单调，未覆盖 0 / 负速度 / 上限钳位 → 补边界用例。
837 [中] `sysoverlay/crates/ui/src/clock.rs:44` — `set_then_get_roundtrips` 写进程级 HMS 且结尾停在 `set_local(23,59,59)` 不复位，与 renderer/tests.rs 的 set_local 用例共享静态 → 用局部注入或测试结束复位。

## 二十七、文档、开发脚本与仓库卫生

838 [低] `README.md:19` — 表格写"2026-09-22 实测"，但 :320 自称"最后更新时间 2026-09-21" → 统一日期。
839 [低] `README.md:95` — 声称".exe 文件不会在主仓库中展示"，但 `guide-app/src-tauri/resources/AAXSysPulse.exe`、`sysoverlay/backup/noprobe-exe/*.exe` 就在工作区 → 改为"构建产物不入库（备份另论）"。
840 [中] `README.md:106` — 声明 MSRV 1.96.0 却无 rust-toolchain.toml / rust-version 落实，CI 又用 @stable → 补锁定文件。
841 [中] `README.md:113` — "sysoverlay/Cargo.toml 只是成员清单"与实际（3 行注释的空文件）不符 → 改为"该文件已废弃，members 在根 Cargo.toml"。
842 [低] `README.md:213` — §性能表现段头写"v1 纯 C 版本的实测"，表内却注"文件大小 ~1.0 MB（Rust 版实测）" → 拆成两列或统一口径。
843 [低] `README.md:299` — 既链接 LICENSE 又内嵌被 `...` 截断的 MIT 片段，易被误当全文 → 删除片段只留链接。
844 [低] `README.md:314` — `[sys-overlay](../sys-overlay)` 指向仓库外同级目录，GitHub 渲染必 404 → 改成外链或删除。
845 [低] `CODE_INDEX.md:5` — "最后核对 2026-11-04" 与 README.md:320 "最后更新 2026-09-21" 相差一个半月 → 统一或注明各自口径。
846 [低] `CODE_INDEX.md:17` — §0 表格把 `version = "0.7.1"` 写死，而 build.rs:50 已把版本注入 MAIN_PROGRAM_VERSION → 去掉具体值或注明来源。
847 [中] `CODE_INDEX.md:20` — §0 产物写 `target/release/AAXSysPulse.exe`，而 README.md:19 与 build.rs:31 用 `target/main-dist/release/` → 补注两种构建路径。
848 [低] `CODE_INDEX.md:196` — §5 只描述 `_*.py`/`*.ps1`，遗漏 tools/ 下 20+ 张 .png、ovhwnd.txt/settings_hwnd.txt/_new_macro.txt 与两份 .bak → 补全清单或说明归类。
849 [中] `sysoverlay/ARCHITECTURE_DESIGN.md:24` — 硬约束"unsafe 仅出现在 win"，但 app/src/main.rs（:84/:162/:229/:460/:826）与 pre_launch_checks.rs（:86/:94/:123）都有 unsafe → 修正为"win 与 app 的 FFI 胶水层"。
850 [中] `sysoverlay/ARCHITECTURE_DESIGN.md:43` — 架构图与 :103 表格引用 `ui/card.rs` / `draw_card`，该文件在 ui/src/ 下不存在（lib.rs 只有 6 个模块）→ 删除或改为 renderer.rs。
851 [中] `sysoverlay/ARCHITECTURE_DESIGN.md:77` — 写 `CONFIG_VERSION=2`，实际 core/src/contract.rs:123 是 3 → 更正。
852 [中] `sysoverlay/ARCHITECTURE_DESIGN.md:78` — 把 color.rs / units.rs 标为"⬜ 占位 / 当前空文件"，实际分别 349 / 420 行 → 更新 §7 状态矩阵。
853 [低] `sysoverlay/ARCHITECTURE_DESIGN.md:93` — C 版档位写 `{5,10,20,30,50}×100ms`，而 `INTERVAL_MS_ALLOWED = [5000,2000,1000,200,100]` → 对齐常量。
854 [低] `sysoverlay/ARCHITECTURE_REVIEW.md:3` — 头部硬编码 `D:\code\...\sysoverlay` 且写"3565 行"，与当前仓库规模不符 → 去绝对路径与行数。
855 [中] `sysoverlay/ARCHITECTURE_REVIEW.md:14` — 结论"app 没有采样线程 / gdi 画静态占位 / 配置持久化是 placeholder"与当前代码完全相反，却与 DESIGN 并列放在 sysoverlay/ 根且无归档标记 → 移入 docs/history/ 并加"历史"抬头。
856 [低] `sysoverlay/ARCHITECTURE_REVIEW.md:39` — 表格行"52 个单测全绿 → ✅ 141 个全绿"把两个不同数字并列且都称全绿 → 拆成"宣称/实测"两行。
857 [低] `sysoverlay/ARCHITECTURE_REVIEW.md:42` — 建议"把 REPORT.md 重命名为 PLAN.md 或标注 WIP"从未执行，REPORT.md 仍在 docs/history/ → 标注已完成/放弃，避免悬空待办。
858 [低] `sysoverlay/ARCHITECTURE_REVIEW.md:95` — 保留 `~~统一为 key=value~~` 被撤回建议及长篇修订痕迹，对现行维护者无价值 → 压成一行结论。
859 [低] `sysoverlay/ARCHITECTURE_REVIEW.md:180` — "剩余未完成：P0-3、P1-6、P2 全系列"是 2026-09-04 排期，自愈/DPI/自启等大多已实现 → 标注历史快照。
860 [低] `sysoverlay/docs/FluentToken映射表.md:3` — 自称"S1 交接物 / 供低智模型执行"，S1 早已结束、tokens.rs 多数字段仍无消费方 → 加归档抬头或删除角色化措辞。
861 [低] `sysoverlay/docs/FluentToken映射表.md:51` — 间距表 `ROW_H = 34` 的值列写着 `16+?`，是未填完的占位符 → 补值或明示"待定"。
862 [中] `sysoverlay/docs/FluentToken映射表.md:72` — "不变式保持成立（9+3=12）"与 :70 的 "14+12=26 ≠ 32" 数值对不上 → 复核并统一推导。
863 [中] `sysoverlay/docs/FluentToken映射表.md:85` — 把 `qt/src/theme.rs` 的 web_light()/web_dark() 称为唯一真值，但全仓无 qt/ 目录 → 给可访问路径或注明外部来源。
864 [中] `sysoverlay/docs/FluentToken映射表.md:140` — 变更清单写 `fluent_tokens.rs`、`settings/theme.rs`，实际是 `settings/fluent/tokens.rs` 与 `win/src/theme.rs` → 更正路径。
865 [低] `sysoverlay/docs/FluentToken映射表.md:151` — 验收命令用 `2>&1 | head -50` / `tail -20`，项目目标平台是 Windows → 改 PowerShell 等价写法。
866 [低] `sysoverlay/docs/FluentToken映射表.md:157` — "对比 docs/baseline/" 相对路径错误（文档本身在 sysoverlay/docs/）→ 改为 `baseline/`。
867 [低] `sysoverlay/docs/死代码清理清單.md:4` — "必须等 S1-S6 全部完成后执行"的 S7 交接物仍留在现行 docs/ → 移入 history/ 或标 S7 已完成。
868 [中] `sysoverlay/docs/死代码清理清單.md:39` — 计划"CARD_RADIUS 10→4 后旧值成为死代码"，而现状 layout.rs:121 是 RADIUS=30，与 FluentToken 映射表:78、SKILL.md:82 三处口径互斥 → 标明以代码为准。
869 [中] `.workbuddy/skills/sysoverlay-add-metric-setting/SKILL.md:9` — "仓库根 `D:\code\...\sysoverlay`（cargo workspace）"错误，真根是仓库根，且硬编码绝对路径 → 改为相对根说明。
870 [中] `.workbuddy/skills/sysoverlay-add-metric-setting/SKILL.md:12` — 铁律称 `cargo test --workspace --lib` 能覆盖 core/ui，但 --lib 排除 bin 目标，app 的 pre_launch_checks 测试跑不到 → 去掉 --lib 或注明例外。
871 [中] `.workbuddy/skills/sysoverlay-add-metric-setting/SKILL.md:38` — 指标下标表写 "0=CPU…5=Time" 共 6 项，而 core/src/types.rs:273 是 `Metric::COUNT = 9` → 同步为 9 项。
872 [中] `.workbuddy/skills/sysoverlay-add-metric-setting/SKILL.md:82` — "全局圆角真值 RADIUS（CARD_RADIUS = RADIUS 同源）"与 FluentToken映射表.md:78 "以 Fluent 为准改为 4" 的决策冲突 → 以代码为准收敛一处。
873 [低] `.workbuddy/skills/sysoverlay-add-metric-setting/SKILL.md:99` — 基线写"238 项：core 136 / ui 30 / win 72"，缺 app 与 guide-app，且与 ARCHITECTURE_REVIEW.md:39 的 141 不一致 → 去掉写死数字。
874 [中] `.workbuddy/skills/sysoverlay-add-metric-setting/SKILL.md:101` — 部署命令 `cp ../sysoverlay/target/release/AAXSysPulse.exe` 指向不存在的路径（产物在仓库根 target/）→ 更正路径。
875 [中] `.trae/documents/AFMF审查与性能修复计划.md:61` — P2-3 引用 proc_list.rs 的 WM_TIMER，全仓无该文件 → 删除或改为现存模块。
876 [中] `.trae/documents/AFMF审查与性能修复计划.md:80` — "已有 state.cfg.window_pos 持久化逻辑"，当前是 5 个 .cfg、无 state.cfg → 更正。
877 [中] `.trae/documents/AFMF审查与性能修复计划.md:132` — 文件清单把 scheduler.rs 列在 crates/win/src/，实际在 crates/core/src/ → 更正。
878 [低] `.trae/documents/AFMF审查与性能修复计划.md:138` — §五验证标准 7 项全是未勾选 `[ ]`，计划从未回填 → 标注完成/作废状态。
879 [中] `.trae/documents/悬浮窗与设置页问题修复计划.md:12` — 通篇 `file:///d:/code/sys-overlay-c-master/...` 硬编码绝对路径链接，换机即全部失效 → 改仓库相对路径。
880 [中] `.trae/documents/悬浮窗与设置页问题修复计划.md:35` — 把 renderer.rs 写成 `win/src/ui/renderer.rs`，而 :38 又写 `ui/src/renderer.rs`，同文档两处路径 → 统一。
881 [低] `.trae/documents/悬浮窗与设置页问题修复计划.md:44` — C1a 计划把 SliderWidget max 改成 `f32::MAX/2.0`，与生产 FONT_PX_MAX 等钳位常量矛盾且未落地 → 标注为未采纳方案。
882 [中] `sysoverlay/tools/i18n_audit.py:3` — `open('crates/win/src/i18n.rs')` 只在 sysoverlay/ 目录跑才有效，与 CI/根目录口径冲突 → 用 __file__ 定位或接收参数。
883 [中] `sysoverlay/tools/i18n_audit.py:6` — 正则一旦与 i18n.rs 格式脱节会匹配 0 条，脚本仍打印"总条目: 0"不报错 → 加 `len(entries) > 0` 下限断言。
884 [中] `sysoverlay/tools/i18n_audit.py:20` — 全脚本只有 print、无 `sys.exit(1)`，SKILL.md:50 要求的"英文无中文/全部成对"即使违规也返回 0 → 发现问题时置非零退出码。
885 [低] `sysoverlay/tools/check_i18n_keys.py:1` — `import subprocess` 全文件未使用 → 删除。
886 [中] `sysoverlay/tools/check_i18n_keys.py:4` — `r"D:\code\sys-overlay-c-master\..."` 硬编码绝对路径（:12 同样）→ 改相对/参数化。
887 [中] `sysoverlay/tools/check_i18n_keys.py:19` — 裸 `except: continue` 吞掉读失败/解码错误，used 集合可能不完整却照常打印 → 收窄异常并计数告警。
888 [中] `sysoverlay/tools/verify_i18n.py:38` — `git show HEAD:crates/win/src/i18n.rs` 是 workspace 合并前的 git 路径，从仓库根跑必失败 → 改为 `HEAD:sysoverlay/crates/...`。
889 [中] `sysoverlay/tools/verify_i18n.py:42` — `assert len(orig_arms)==188`（:98 同）把条目数写死，i18n 增删一条即误报 → 只断言集合相等。
890 [中] `sysoverlay/tools/gen_i18n.py:238` — 以 `"w"` 原地覆盖生产 i18n.rs，:3 又用硬编码绝对路径，无备份/无 dry-run → 加 --check/先写临时文件。
891 [低] `sysoverlay/tools/_t.py:2` — 内嵌 `"ZH text here."` 的一次性调试 snippet，:15 只 print count 无断言 → 删除或转为带期望值的单测。
892 [中] `tools/fix_config_defaults.py:14` — `PATH = r"D:\code\...\guide-app\src-tauri\src\config.rs"` 硬编码且 :241 原地写回生产源码 → 参数化路径并默认 dry-run。
893 [中] `tools/cap_pages.ps1:14` — 直接用 `$args[0]/$args[1]`，无 param() 无校验，缺参时 `ToInt64($null,16)` 抛异常 → 改 param([Parameter(Mandatory)]...) 并校验 hex。
894 [中] `tools/cap_pages.ps1:17` — `[CP]::GetDpiForWindow($h)` 返回值未判 0，失败时 `$sc=0` 使全部点击坐标算成 0 → 失败即退出并回退 96。
895 [低] `tools/cap_pages.ps1:44` — 硬编码 `"d:\code\sys-overlay-c-master\tools\pshot.ps1"` 调另一脚本 → 用 $PSScriptRoot 拼接。
896 [低] `tools/cap_time.ps1:1` — 默认输出写死 `C:/Users/VOS_User/AppData/Local/Temp/ov_time.png` → 用 [IO.Path]::GetTempPath()。
897 [中] `tools/open_settings.ps1:12` — 硬编码 `d:\code\...\tools\ovhwnd.txt` 读运行期 HWND → 用 $PSScriptRoot 并说明该文件来源。
898 [低] `tools/_check_braces.py:3` — `_check_braces.py/_check_braces2.py/_check_braces3.py` 三份脚本都硬编码同一 cfg_ref.rs 绝对路径且内容重叠 → 合并为一个带参数的脚本。
899 [低] `tools/_gen_macro2.py:9` — `TXT = r"D:\code\...\tools\_new_macro.txt"` 依赖 tools/ 下 12KB 临时中间产物 → 纳入 .gitignore 或改为脚本内生成。
900 [低] `sysoverlay/backup/noprobe-src/Cargo.lock` — 305 行旧 workspace 锁文件残留在备份源码树，远离根 Cargo.lock → 删除，避免 CI 的 hashFiles('**/Cargo.lock') 误命中。
901 [低] `sysoverlay/backup/noprobe-exe/sysoverlay-app-noprobe-20260904-2233.exe` — 又一份 221KB 历史 exe 备份 → 移出仓库或统一归档。
902 [低] `guide-app/src-tauri/resources/AAXSysPulse.exe` — 构建产物落在源码树且被 .gitignore:2 排除，新 clone 无此文件、build.rs:43 才生成 → 在 README 注明它是产物非源码。
903 [低] `.workbuddy/tmp/AAXSysPulse.exe.bak` — agent 私有目录里另有 1MB exe 备份，与 dist/ 的 4 份备份重复 → 仅保留一份归档。
904 [低] `sysoverlay/crates/core/tests/` — 与 ui/tests/、backup/noprobe-src/crates/core/tests/ 三个 tests/ 目录全空 → 删除空目录或补一个集成测试。
905 [低] `tools/ovhwnd.txt` — 与 tools/settings_hwnd.txt 的 9B/3B 运行期 HWND 数值快照混在脚本目录，CODE_INDEX §5 也未列 → 移入 .gitignore 的临时目录。
906 [低] `tools/_layout_before_split.rs.bak` — 与 tools/config.rs.bak_before_defaults 的 58KB/30KB 源码备份，以及 20+ 张 tools/*.png 截图堆在 tools/ → 统一归档，别与工具脚本混淆。


## 附：本轮按层面分布

| 层面 | 编号 | 条数 |
|------|------|------|
| 十四、设置面板状态机、消息与命中 | 301–351 | 51 |
| 十五、悬浮窗 `window.rs`：消息、坐标与生命周期 | 352–387 | 36 |
| 十六、GDI 上屏与配置原子写的剩余区域 | 388–419 | 32 |
| 十七、设置页布局、自绘控件与 core 配置模型的剩余区域 | 420–468 | 49 |
| 十八、`ui` crate：字体图集、渲染器、离屏光栅与测试有效性 | 469–510 | 42 |
| 十九、D2D / gfx / 渲染线程 / 图标 / Fluent token / 脏检查测试 | 511–561 | 51 |
| 二十、指标采集、GPU 厂商后端、MSR 与功耗 | 562–596 | 35 |
| 二十一、系统层：日志、性能探针、自启、主题、DPI、时钟 | 597–619 | 23 |
| 二十二、`core` 纯逻辑、契约与快照的剩余区域 | 620–648 | 29 |
| 二十三、引导程序 Rust 后端：命令、安装、驱动与配置扇出 | 649–699 | 51 |
| 二十四、Vue 14 页向导、桥接与状态机 | 700–752 | 53 |
| 二十五、前端样式、全局入口与构建配置 | 753–781 | 29 |
| 二十六、工程化：Cargo / build.rs / CI / 测试有效性 | 782–837 | 56 |
| 二十七、文档、开发脚本与仓库卫生 | 838–906 | 69 |
| **本轮合计** | **301–906** | **606** |
| 首轮（见首份报告） | 001–300 | 300 |
| **两轮累计** | **001–906** | **906** |

> 去重说明：五路审查先各自比对首轮 300 条精简索引，再交叉去重；同一文件同一行号的同一问题只保留一条，不同行号的不同问题分别保留。
