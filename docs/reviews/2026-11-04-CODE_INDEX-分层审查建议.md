# AAXSysPulse 分层代码审查建议（基于 CODE_INDEX.md）

- 审查基准：`CODE_INDEX.md`（最后核对 2026-11-04）+ 仓库当前真实源码
- 审查范围：约 48,830 行 Rust / Vue / JS / 配置源码，覆盖 `core`、`ui`、`win`、`app`、`guide-app` 五个 crate 与 14 页向导
- 审查方式：按索引 §2.1–§2.5 的功能分区逐层取证，四路并行通读后交叉去重；对高优先级结论做了源码回读复核
- 建议总数：**300 条**，编号 001–300，分属 13 个层面
- **范围约束（严格遵守）**：全部为局部、可落地的小改动；不含新增功能、不含新增/替换依赖包、不含架构重写、不含与安全/业务无关的扩张型建议（如 2FA 等）
- 严重度：`[高]` = 静默错误/数据丢失/崩溃或 UB 风险；`[中]` = 行为偏差、口径不一致、可维护性；`[低]` = 注释、命名、测试与工程卫生

---

## 最值得先处理的 12 条（摘要，编号对应正文）

| 编号 | 一句话 |
|------|--------|
| 032 | `system.rs` 把 version 写进 `[system]`，却从 `[meta]` 读，版本号永远读不回来 |
| 044 | `legacy.rs` 靠 `contains("zoom"/"interval")` 判旧文件，会把新 `system.cfg` 误判为 legacy |
| 045 | `.mcfg` 的 `--- END ---` 被当成名为 `END` 的段头，导入多出一个空文件 |
| 049 | 主程序是 GUI 子系统，`config_io.rs` 的关键警告全用 `eprintln!`，用户完全看不到 |
| 069 | `proc_gpu.rs` 先写 `self.prev` 再判 `else if self.prev.is_some()`，所有失败帧被伪装成 0% |
| 145 | `window.rs::get_window_size` 在渲染线程裸读 UI 线程写的 `ThreadState.win_w/win_h`，真实数据竞争 |
| 121 | `render_d2d.rs` 硬编码开发机 `C:\Users\VOS_User\...` 与 `D:\code\...`，用户机必然找不到 DIN 字体 |
| 131 | `din_bitmap.rs` 用进程级 `OnceLock` 守 `thread_local` 字重表，渲染线程 respawn 后三档注入永久丢失 |
| 166 | `settings/mod.rs` 的 `on_wm_timer` 没有防抖补写分支，窗口不关就永不落盘 |
| 193 | `install.rs` 的 `UninstallString` 指向盘上并不存在的 `Guide.exe` |
| 240 | `main.rs` 的 `setup_panic_hook` 在 272 行被 `probe::install_panic_hook()` 覆盖，minidump 永不生成 |
| 019 | `Config::opacity` 全仓零引用且不持久化，是一个纯死字段 |

---

## 一、跨进程契约、索引 §1/§3/§6 的架构一致性（001–018）

001 [高] `sysoverlay/crates/core/src/contract.rs:92` — `DRIVER_DIR` 硬编码 `C:\Windows\...`，非 C 系统盘静默失联 → 补注释钉死假设，并加一条"系统盘可变"的单测或改由系统目录派生。
002 [中] `core/src/contract.rs:191` — ASCII 校验只遍历 13 个名字，漏 `RUN_KEY` / `APPROVED_RUN_KEY` / `UNINSTALL_SUBKEY` / `START_MENU_FOLDER` → 补齐所有 `&str` 常量。
003 [中] `core/src/contract.rs:150` — 契约消息数组漏了本文件 `:60` 的 `WM_AUTOSTART_DONE` 与 `:63` 的 `WM_DIAG_CLICK` → 补进派生关系断言。
004 [高] `sysoverlay/crates/app/src/main.rs:166` — 单实例互斥体名用 `w!("AAXSysPulse_SingleInstance_Mutex")` 字面量，绕开了"唯一真源"纪律 → 收进 `contract.rs` 并加派生单测。
005 [中] `guide-app/src-tauri/src/process.rs:18` — `GUIDE_MUTEX_NAME` 是本文件私有字面量，与同文件 20 行走契约的 `MAIN_WINDOW_CLASS` 口径不一致 → 一并移入契约。
006 [中] `guide-app/src-tauri/src/{install.rs:929,driver.rs:130,shortcut.rs:20}` — 三份重复的 UTF-16 编码函数 → 统一调 `contract::wide_of()`。
007 [低] `CODE_INDEX.md:33` — 声称"所有跨进程名字唯一真源"与 004/005 两处字面量冲突 → 收窄措辞或按 004 修正代码。
008 [中] `core/src/contract.rs:68` — 编译期断言只覆盖 `WM_APP` 区间与 `WM_DIAG_CLICK` 上界 → 对每个 `WM_*` 批量生成 `const _: () = assert!(x <= WM_APP_SEG_MAX)`。
009 [低] `README.md:253` — 号段写成 `WM_APP+1 … +9`，实际契约占用到 `+63` → 改为"+1…+63，引导仅投递 +8/+9"。
010 [中] 两进程多处重复 `%APPDATA%\AAXSysPulseC\config` 目录拼接 → 提一个 `config_dir()` helper，避免一端改目录名静默失联。
011 [中] `guide-app/src-tauri/src/guide_state.rs:18` — state 用 `data_dir()`、日志用 `data_local_dir()`、配置用 `config_dir()`，三种口径分散 → 集中到同一模块并各注明归属。
012 [高] `guide-app/src-tauri/src/notify.rs:111` — 只按窗口类名 `FindWindowW` 取第一个 HWND，不校验归属 PID，同名类/残留窗口会收错消息 → 用 `main_process_pid()` 反查或 `EnumWindows` 全量匹配。
013 [中] `guide-app/src-tauri/src/lib.rs:363` — `overlay_get_rect` 同样只取第一个 HWND 且失败静默回退坐标 → 复用 012 的 PID 校验 helper，失败留日志。
014 [中] `guide-app/src-tauri/src/notify.rs:109` — 只对 `PostMessageW` 失败重试 3 次，`FindWindowW` 失败直接 return；主程序重启窗口未建的瞬间恰好丢消息 → 把查找一并纳入重试循环。
015 [中] `guide-app/src-tauri/src/notify.rs:75` — `FLUSHER_ARMED.store(false)` 与上一行 `!PENDING.load` 之间存在竞态，新调用的 `PENDING=true` 会悬挂到下次 → 用 `compare_exchange` 收尾。
016 [中] `guide-app/src-tauri/src/notify.rs:94` — `notify_main_quick_restart` 忽略 `deliver()` 的 bool，全部投递失败零留痕 → 失败写一条 install 日志。
017 [低] `guide-app/src-tauri/src/notify.rs:41` — 注释算"100+50=150ms 最坏延迟"，漏算 `bridge.js:359` 的 50ms 防抖 → 更正为 200ms 或去掉一级重复防抖。
018 [中] `sysoverlay/crates/win/src/lib.rs:86` — `release_section()` 用 `rest.find("[")` 切段、只 `replace(' ', "")`，注释含 `[` 或 `panic =\t"unwind"` 都会误判，而这条断言守护着 §6.1 的自愈命脉 → 改成逐行 trim + 过滤注释后断言。

## 二、配置模型、INI 解析与原子持久化（019–056）

019 [高] `core/src/config.rs:113` — `opacity` 字段（默认 90）全仓零引用、序列化与解析均不出现，是纯死字段；真正生效的是 `normal_opacity` → 删除或明确接线，避免后人误改。
020 [中] `core/src/config.rs:173` — `normal_opacity` 注释写"默认 35"，`Default` 在 `:316` 是 100 → 改注释。
021 [中] `core/src/config.rs:158` — `font_size` 注释"8.0..=40.0"，与 `FONT_PX_MIN=4.0/MAX=128.0`(`:83`) 不符 → 改为引用常量。
022 [中] `core/src/config.rs:175` — `anim_ms` 注释 100..=2000、`:467` clamp 0..500、`system.rs:157` clamp 0..2000，三处区间不一致 → 收敛到单一常量对。
023 [中] `core/src/config.rs:898` — `parse_metric_color/show_label/font_size/offset` 四处重复 `"cpu"=>0.."disk_write"=>8` 手写映射 → 统一用 `:476` 已有的 `Metric::ALL.iter().position(...)`。
024 [低] `core/src/config.rs:919` — `parse_metric_color` 只剥小写 `0x`/`#`，`parse_colorref`(`:375`) 还处理 `0X` → 补齐大小写前缀。
025 [中] `core/src/config.rs:875` / `config/system.rs:16` / `types.rs:79` — 三份"找最近档位"逻辑 + 两份 `INTERVAL_MS_ALLOWED` → 抽公共函数 + 单一常量。
026 [中] `core/src/config.rs:1356` — `test_garbage_never_panics` 用硬编码 8.0/40.0 校验字号步进，已与常量 4/128 漂移 → 改用常量。
027 [高] `core/src/config/appearance.rs:47` 与 `config.rs:434` — `"NaN"` 能 `parse::<f32>()` 成功，`clamp` 对 NaN 返回 NaN，字号变 NaN 且可序列化 → 加 `is_finite()` 校验并补单测。
028 [中] `core/src/config/appearance.rs:49` — `(n.clamp()*2.0).round()/2.0` 的 0.5 步进在 `config.rs:434`、`:970` 共三份 → 抽 `snap_font_size()`。
029 [中] `core/src/config/ini.rs:66` — 只识别整行 `#`/`;` 注释，`key = val # 注释` 不剥离，与 `:4` 文档"注释一律忽略"不符 → 解析 value 时去行内注释，或修改文档。
030 [低] `core/src/config/ini.rs:19` — `fmt_color` 把 `[u8;6]` 拆三段再 `from_utf8(...).unwrap_or("00")`，失败静默写 "00" → 整段一次性转换。
031 [低] `core/src/config/ini.rs:34` — `parse_bool` 与 `config.rs:364` 函数体完全相同，且 `to_ascii_lowercase()` 每次分配 → 复用一份并改 `eq_ignore_ascii_case`。
032 [高] `core/src/config/system.rs:101` — `parse` 从 `[meta]` 读 `version`，但 `:29` `serialize` 写进 `[system]`（其余三个文件都写 `[meta]`），版本永远读不回来 → 两侧统一到同一段，改动前先确认 §6.8 的主程序判版依赖。
033 [低] `core/src/config/system.rs:29` — `w.kv("version", 3)` 写死字面量，未用 `CONFIG_VERSION` → 改用常量。
034 [中] `core/src/config/system.rs:16` — `ALLOWED: [u32;5]` 与 `config.rs:76` 重复 → 复用同一常量。
035 [中] `core/src/config/system.rs:109` — `x`/`y` 缺省各自回退 -1，只写 `x=100` 会得到 `Some((100,-1))` 半哨兵坐标 → 缺任一键整体置 `None`。
036 [中] `core/src/config/system.rs:54` — `layout` 锚点用 `_ => "horizontal"` 通配 `Classic/Strip`，新增枚举变体不会报错 → 显式穷尽匹配。
037 [中] `core/src/config/monitor.rs:126` — `show` 解析失败兜底取 `cfg.metric_show_labels[idx]`（名称显隐），语义错位 → 改用 `visibility(cfg)[idx]`。
038 [中] `core/src/config/monitor.rs:127` — `match id { 1..=9 => ..., _ => {} }` 静默丢弃新增 Metric，而 `visibility()`(`:33`) 定长数组会编译报错，两端不对称 → 穷尽映射。
039 [中] `core/src/config/monitor.rs:156` — "与全局字号相等即视为跟随"依赖 `cfg.font_size`，而两个 .cfg 加载顺序独立 → 去掉该推断或显式固定加载顺序。
040 [中] `core/src/config/monitor.rs:160` — 非法 `color_l` 解析得 `None` 与"空=跟随全局"同样清掉已有色槽 → 区分空值与非法值，非法时保留原值。
041 [中] `core/src/config/layout.rs:30` — `file_for` 用 `_ => LAYOUT_H_FILE` 通配 → 显式列出变体。
042 [中] `core/src/config/layout.rs:78` — `split_whitespace().nth(got)` 的 `got` 仅在解析成功时自增，坏 token 会反复读同一个、最终整段丢弃 → 复用 `config.rs:998` 的 `take(4).enumerate()` 写法。
043 [低] `core/src/config/layout.rs:53` — `(i + 1).to_string()` 中 `i: u8`，`i==255` 时 debug 下溢出 panic → 改 `i as u32 + 1`。
044 [高] `core/src/config/legacy.rs:23` — `looks_like_legacy` 用 `contains("topmost"/"zoom"/"interval"/"netunit")` 判旧文件，而新 `system.cfg` 同样含这些键 → 加锚点（如必须含 v2 标记且不含 `=`）。
045 [高] `core/src/config/mcfg.rs:85` — `--- END ---` 先被 `strip_prefix("--- ")/strip_suffix(" ---")` 当成名为 `END` 的段头并 `continue`，`:95` 的 END 分支永不可达 → 把 END 判断前置。
046 [中] `core/src/config/mcfg.rs:146` — `mcfg_roundtrip` 只断言 `contains(...)`，正好漏掉 045 的多出 `("END","")` → 补 `files.len()==4` 与不含 END 的精确断言。
047 [低] `core/src/config/mcfg.rs:75` — `text.starts_with(MAGIC)` 不校验边界，`"MCFG1garbage"` 也被接受 → 校验分隔符。
048 [中] `core/src/config/mcfg.rs:117` — `is_known_file` 白名单在 `parse_mcfg` 内未被调用 → 在解析处直接过滤或返回明确错误。
049 [高] `sysoverlay/crates/win/src/config_io.rs:184` — 关键"配置损坏/禁写"警告用 `eprintln!`，而主程序是 GUI 子系统（`perf.rs:6` 自述无控制台）；全仓同类 `eprintln!` 共 99 处 → 统一走 `logging::write` / `probe`，至少覆盖用户可见警告。
050 [中] `win/src/config_io.rs:243`（`:304` 同） — `MoveFileExW` 失败后二次调 `GetLastError()`，错误码可能已被污染 → 打印 `windows` crate 返回的 `result` 错误码。
051 [中] `win/src/config_io.rs:219` — 临时文件名固定 `<name>.tmp`，进程在 sync 后被杀会永久残留，且并发写者互相覆盖 → 用 pid/seq 唯一名并在启动时清理残留。
052 [高] `win/src/config_io.rs:416` — 就地升级前 `copy(... .bak)` 失败被忽略仍覆盖原文件 → 备份失败即中止升级并留痕，与导入路径的严格备份对齐。
053 [中] `win/src/config_io.rs:560` — 迁移 rename 部分成功时不回滚已改名的正式文件，与 `:488`"失败保持原状、下次重试"注释不符 → 迁移前统一备份或复用 `bak_entries` 回滚。
054 [中] `win/src/config_io.rs:315` — `read_file_version` 把"存在但读不出"当成"缺 version → v1"，可能误触发破坏性弹窗 → 区分读失败与版本缺失。
055 [中] `win/src/config_io.rs:722` — 写线程"检查 notify → spawn → set"非原子，并发首次保存可能各起一个写线程 → 用 `OnceLock::get_or_init` 在闭包内完成。
056 [中] `win/src/config_io.rs:948` — `prompt_max_compat` 缺 `#[cfg(test)]` 变体（其余三个弹窗都有），`:1402` 用例会弹真实 MessageBox 阻塞 CI → 补测试 mock 并覆盖导入成功路径。

## 三、指标采集、Win32 API 与数值口径（057–090）

057 [中] `win/src/proc_cpu.rs:53` — `GetSystemTimes` 失败仅返回 `Transient`、不记错误码，无法区分 API 不可用与权限问题 → 记 HRESULT 并对连续失败计数降级。
058 [中] `win/src/proc_cpu.rs:64` — 首帧 `prev=None` 既不写 `out.cpu` 也不置 `timestamp_ms`，完全依赖上层 placeholder → 显式置 `Unavailable`/0 并补时间戳。
059 [中] `win/src/proc_mem.rs:53` — `load_pct` 只 `.min(100.0)` 未钳下界，`ullAvailPhys > ullTotalPhys` 时得到负百分比 → 用 `clamp(0.0,100.0)`， `used_gb` 同样 `.max(0.0)`。
060 [中] `win/src/proc_mem.rs:44` — `GlobalMemoryStatusEx` 失败返回 `Transient` 无码无日志 → 补 HRESULT 日志。
061 [中] `win/src/proc_disk.rs:122` — `valid = rc != 0 || wc != 0` 识别不了注释自承的"locale 版假成功"（句柄非 0 却永不出数） → 出数前维持待验证态，N 轮无效后置 invalid。
062 [中] `win/src/proc_disk.rs:92` — 英文接口失败后仍把英文路径传给期望本地化名的 `PdhAddCounterW`，中文系统必然失败 → 去掉该回退或改走 `PdhLookupPerfNameByIndex`。
063 [低] `win/src/proc_disk.rs:48` — 注释称 `last_sample_ms` 为 "GetTickCount64 单位"，实际来自调度器传入的 `now_ms` → 修正注释或统一时钟源。
064 [中] `win/src/proc_net.rs:285` — `(sum_in - pin).saturating_mul(1000) / dt` 大 delta 饱和到 `u64::MAX` 后除 dt 得到错误巨值 → 改 `u128` 中间量或先除后乘。
065 [中] `win/src/proc_net.rs:77` — `NotifyIpInterfaceChange` 的句柄未保存，无法 `CancelMibChangeNotify2` → 存全局句柄并在退出注销。
066 [中] `win/src/proc_net.rs:176` — `not_virtual` 只排除 24/131，注释却点名排除 Hyper-V 虚拟交换机 53 → 补显式白名单或显式列出 53 兑现注释。
067 [低] `win/src/proc_net.rs:18` 与 `win/src/sampler.rs:235` — 注释写"10s 安全间隔"，`NET_ENUM_REFRESH_MS`(`:49`) 实为 60_000 → 统一数值与注释。
068 [低] `win/src/proc_gpu.rs:134` — "gpu enum#2 failed" 打印的是第一次调用的 `st.0` 而非 `st2.0` → 修正变量。
069 [高] `win/src/proc_gpu.rs:306` — 先 `self.prev = Some(...)` 再判 `else if self.prev.is_some()` 恒真，times 为空/长度不匹配/dt 过短全被伪装成 `Value(0.0)` → 用独立"首轮"布尔标记区分。
070 [中] `win/src/proc_gpu.rs:128` — `infos.resize_with(n.NumAdapters as usize, ...)` 无上限，驱动返回垃圾计数会 OOM → 钳到合理上限（如 64）。
071 [低] `win/src/proc_gpu.rs:198` — LUID 编码 `((H as u64)<<32) | L as u64 == w` 依赖 `|` 与 `==` 优先级，且与 `:177` 重复实现 → 加括号并抽公共编码函数。
072 [中] `win/src/gpu_detect.rs:20` — 4 GiB 阈值硬编码且用 `>=`，恰好 4GiB 的卡/大显存核显边界模糊 → 提具名常量并补边界单测。
073 [中] `win/src/gpu_detect.rs:58` — `while let Ok(adapter) = EnumAdapters1(i)` 无 `i` 上限，驱动异常可能死循环 → 加 `MAX_ADAPTERS`。
074 [中] `win/src/gpu_vendor.rs:50` — 注释称 teardown "卸载厂商驱动库"，但 `Drop`(`:266`、`:667`) 只调 shutdown/destroy，从不 `FreeLibrary` → 保存 HMODULE 释放，或据实改注释。
075 [中] `win/src/gpu_vendor.rs:64` — `merge_reading` 的功耗 `cur + wi` 无有限性检查，单后端 NaN 污染整包 → 仅累加 `is_finite()` 的值。
076 [高] `win/src/gpu_vendor.rs:117` — `GPU_UTIL.store(u.to_bits())` 未过滤非有限值，而哨兵 `0xFFFF_FFFF` 恰是 NaN 位型，会被 `cached_util()` 误判 → 写缓存前 `is_finite()`。
077 [中] `win/src/gpu_vendor.rs:396` — `stride * n as usize` 的 `n` 直接来自驱动且无上限 → 钳 `n` 并用 `checked_mul`。
078 [低] `win/src/gpu_vendor.rs:562`（`:609`、`:614` 同） — 800.0 三处硬编码，注释却称"峰值 < 600W" → 提 `MAX_CARD_WATTS` 并统一阈值与注释。
079 [中] `win/src/msr.rs:286` — 注释说忽略 183(`ERROR_ALREADY_EXISTS`)，代码比较的却是 1056(`ERROR_SERVICE_ALREADY_RUNNING`) → 明确要忽略的错误码集合并同步注释。
080 [中] `win/src/msr.rs:556` — DRAM 读失败 `unwrap_or(0)`，下一帧 `wrapping_sub` 得巨大伪回绕量、整帧被拒 → 失败沿用上帧值或置 `dram_ok=false` 重探。
081 [高] `win/src/msr.rs:385` — `while *pw.0.add(len) != 0` 无边界，`lpBinaryPathName` 无 NUL 会越界读 → 按 4KB 缓冲上限循环。
082 [中] `win/src/msr.rs:394` — `driver_file_exists` 只剥 `\??\`，服务路径常为 `\SystemRoot\System32\drivers\...`，误判不存在会反复改服务配置 → 解析 `SystemRoot`/相对路径。
083 [低] `win/src/msr.rs:730` — in_flight 分支注释称"收下迟到结果"，实际 `Some(Ok(_))` 丢弃 frame 只复位标志 → 返回该 frame 或修正注释。
084 [中] `win/src/power_system.rs:26` — 瓦×100 存 `AtomicU32` 却钳到 `u16::MAX`(655.35W)，魔数上界与存储容量不符 → 用具名物理上限常量。
085 [中] `win/src/power_system.rs:114` — GPU 瓦数缺失翻转只走受门控的 `probe::log`，关调试日志即不可见 → 改走不受门控的 degrade/diag 流。
086 [中] `win/src/power_system.rs:123` — 整机 total 无 `is_finite`/上限兜底，累加出 Inf 会返回 `Some(Inf)` → 非有限时返回 `None` 并留探针。
087 [中] `win/src/sampler.rs:293` — `disk.ready = read.is_some() || write.is_some()` 后对缺失侧 `unwrap_or(0.0)`，把"读不到"伪装成 0 B/s → 分侧保留 `Metric`。
088 [高] `win/src/sampler.rs:200` — `set_cached_util(u)` 在 `u.is_finite()` 判断之前执行，NaN 会进缓存 → 先过滤再缓存。
089 [中] `win/src/sampler.rs:269` — Power 返回 None 时记 `Skipped`，而退避只对 `is_failure()` 生效，Power/GPU 失败路径永不退避 → 改记 `Transient` 或显式推进退避。
090 [低] `win/src/sampler.rs:98` — `prev[4]/cur[4]/prev[2]/cur[3]` 裸下标靠文字注释对应 `SamplerId` → 用 `SamplerId::X as usize` 或解构命名。

## 四、调度器、诊断缓冲与纯函数指标（091–102）

091 [中] `core/src/scheduler.rs:5` — 头注释称"死区迟滞 / Idle-Active 自适应"，实现只有 `update_ema` + 分频 → 修正文档或标注未接线。
092 [低] `core/src/scheduler.rs:52` — `Source::min_effective_ms()` 全仓无调用，且与 `:63` 的 `MIN_EFFECTIVE_MS` 数组重复同一组 5 值 → 删方法或让数组由它派生。
093 [中] `core/src/scheduler.rs:123` — `ema_*_committed` 四个字段只写不读，注释称"用于采样决策" → 删除死状态或补齐变化检测。
094 [中] `core/src/scheduler.rs:108` — `IDLE_INTERVAL_MS=1000` 与 `cadence.rs` 的 `IDLE_MAX_MS=4500` 并存且都自称 idle 兜底 → 统一到单一常量。
095 [中] `core/src/scheduler.rs:183` — Power 直接返回 `power_interval_ms` 不做 `.max(...)`，注释 `:114` 却声明 500..=60000 → 在此也 clamp。
096 [中] `core/src/cadence.rs:37` — `DELTA_ENTER/DELTA_EXIT/EXIT_TICKS/ACTIVE_CPU_PCT/ACTIVE_NET_BPS` 全仓无引用，与 `:13` 注释宣称的死区/活动模式不符 → 删除或标注"预留未接线"。
097 [中] `core/src/cadence.rs:62` — `const _: () = assert!(...)` 写在 `#[cfg(test)]` 的测试函数体内，普通构建根本不编译（对比 `contract.rs:68` 放模块顶层） → 移到模块顶层。
098 [低] `core/src/cadence.rs:3` — 头注释"所有采样周期取 1500ms 的约数"与 `:26` 自身 `Gpu: 2000ms` 矛盾 → 改为"多数源对齐 1500，GPU 例外"。
099 [中] `core/src/sampler.rs:43` — `is_fatal`/`is_failure` 用 `matches!` 列变体，新增 `SampleOutcome` 变体不会触发编译错误 → 改穷尽 `match`。
100 [低] `core/src/sampler.rs:85` — `DiagEntry` 未标 `#[repr(C)]`，注释"8 字节"依赖编译器布局，`:442` 测试只断言 `<=8` → 加 `#[repr(C)]` 并断言 `==8`。
101 [低] `core/src/sampler.rs:178` — `.count() as u32` 依赖 `len()<=DIAG_CAP=64`，调大容量会静默截断 → 用 `u32::try_from` 或注释约束。
102 [中] `core/src/metrics.rs:79` / `core/src/composite.rs:270` / `core/src/power.rs:66` — 文档 `0.0 < alpha` 与允许 0 的 `debug_assert` 不符；`MeterConfig.tau` 为负且 `tau+dt==0` 会除零；`share_cpu_power` 的 `clamp` 对 NaN 返回 NaN 且无兜底（而 `composite.rs` 专门防了 NaN） → 分别修文档、构造时校验、加 `is_finite` 兜底与单测。

## 五、离屏光栅化、字体与调色板（103–122）

103 [中] `ui/src/render.rs:44` — `Rect::right()/bottom()` 用裸 `self.x + self.w`，极端几何 i32 溢出 → 改 `saturating_add`。
104 [中] `ui/src/render.rs:58` — `clip_to_surface` 的 `h` 未整体 `.max(0)`，与上一行 `w` 写法不对称且可为负 → 写成 `(self.h-(y-self.y)).max(0).min(surface_h-y)`。
105 [低] `ui/src/render.rs:86` — `Rgba::new` 预乘用 `(r as f32*a_f) as u8` 截断，暗色系统性偏低 → 改 `.round() as u8`。
106 [低] `ui/src/render.rs:109` — `to_colorref()` 返回已预乘的 RGB，半透明色拿去建 GDI 画刷会偏暗 → 注明仅 a=255 可用或先反预乘。
107 [中] `ui/src/offscreen.rs:30` — `new()` 的 `width*height*4` 未 `max(0)`，而同文件 `resize`(`:43`) 有钳制 → 统一口径或改走 `resize`。
108 [中] `ui/src/offscreen.rs:43` — 钳制后仍是 i32 乘法，大 DPI/大窗口溢出 → 用 `usize` + `saturating_mul`。
109 [高] `ui/src/offscreen.rs:166` — `clear_corners` 用 `blend_pixel(..., Rgba::transparent())` 清四角，over 合成下 `src_a=0 → out=dst` 是恒等操作，圆角根本没清掉 → 直接把像素置 0。
110 [中] `ui/src/offscreen.rs:395` — `blit_coverage` 把已预乘的 `color.r/g/b` 再传进 `Rgba::new`（内部再预乘一次），双重预乘变暗 → 按 trait `:185` 契约传直色。
111 [低] `ui/src/font.rs:6` — 文件头仍写"占位实现 / Phase 3 接 GDI"，实际已是 DWrite DIN 注入 + 浮点缩放 + 字形图集 → 更新注释。
112 [中] `ui/src/font.rs:593` — `scale_of_font` 注释"字号允许 1~999，无上下限"自相矛盾，实现只有 `.max(1.0)/.max(0.5)` → 明确钳到 `FONT_PX_MAX`。
113 [中] `ui/src/font.rs:723` — `render_str_scaled_f` 每字把 `cursor` 用 `as i32` 截断，而 `measure_scaled_f`(`:600`) 用 `.ceil()`，字距累积漂移 → 统一取整方式。
114 [中] `ui/src/font.rs:308` — `get_char_bitmap` 对非 ASCII（中文、℃、µ）一律回退 ■，单位/时间标签易缺字 → 补常用符号字形或显式接 DWrite 回退。
115 [低] `ui/src/palette.rs:117` — 测试把 `s.accent(90.0)` 硬编码成 `Rgba::opaque(248,113,113)`，core 调阈值即误报 → 改为与 `s.core.accent_for(90.0)` 比对。
116 [中] `ui/src/clock.rs:26` — `set_local` 文档称越界按位掩码截断，实际只 `&0xFF`，`set_local(99,99,99)` 会写入非法值 → 按值域钳制或 `debug_assert`。
117 [低] `ui/src/clock.rs:14` — 模块头"配一个 250ms 的 CLOCK 定时器"与 `gdi.rs` 的秒/分对齐 `CLOCK_TIMER_ID` 不符 → 更新文档。
118 [低] `ui/src/lib.rs:13` — 称 `font` "支持 0.5 步进"，`scale_of_font`(`:594`) 接受任意 f32、无量化 → 二选一。
119 [高] `ui/src/renderer.rs:431` — `build_rows` 用 `Style::derive(Rgb::default(), cfg.fg)` 把背景写死，`cfg.bg` 完全不参与调色板派生 → 改传 `cfg.bg`。
120 [中] `ui/src/renderer.rs:504` — `push_f64_fixed` 注释"纯整数位拼接、无格式化依赖"却调用 `format!`（堆分配），与索引 §7"零分配"口径冲突 → 手写补零。
121 [高] `win/src/render_d2d.rs:409`、`:423` — DIN 字体路径硬编码 `C:\Users\VOS_User\Downloads\...` 与 `D:\code\sys-overlay-c-master`，用户机必然找不到 → 改为相对 exe/资源目录或读取安装时落地的字体。
122 [中] `ui/src/renderer.rs:349`、`:528` — `metric_index` 手写 9 个 match、:528 又用字面量 0..8 当下标，而 `MAX_METRICS`(`:207`) 自动跟随 `Metric::COUNT` → 统一用枚举索引，避免新增指标必漏改。

## 六、脏检查、D2D 设备自愈与渲染线程（123–144）

123 [中] `ui/src/renderer.rs:907` — `hash_cells` 未纳入 `CellSig.bbox_x/right_align/w/h`，包围盒变而内容不变会漏重绘 → 混入这些几何字段。
124 [低] `ui/src/renderer.rs:139` — `SLOT_PCT1=5`（注释 99.9%），但 `fmt_pct1(100.0)` 产出 6 字符 → 改 6 或钳位 99.9。
125 [中] `ui/src/renderer/tests.rs:326` — 两个用例都写全局 `clock::set_local` 静态 HMS，并行测试相互覆盖 → 串行化或改注入式时钟。
126 [低] `ui/src/renderer/tests.rs:480` — `overlay_layout_is_within_budget` 用墙钟 `<500µs` 断言，CI/负载下 flaky → 改为操作计数。
127 [低] `ui/src/renderer/tests.rs:367` — `metric_order` 用硬编码下标 → 用 `Metric::X as u8` 构造。
128 [中] `win/src/render_thread.rs:141` — 注释称"panic 被兜住后 frame_end 照常调用"，但 `render_frame_end` 是 `render_once` 内的普通语句，panic 会展开到 `:75` 的 `catch_unwind` 而跳过，`FRAME_START_MS` 不复位 → 用 RAII guard 或 Err 分支补调。
129 [低] `win/src/render_thread.rs:14` — 模块头称配置/快照"arc_swap 共享、无锁读"，`RENDER_CFG`(`:40`) 实为 `Mutex<Option<Config>>` → 改述。
130 [中] `win/src/render_thread.rs:148` — `respawn()` 无条件 `start()`，不探测旧线程是否已恢复 → 加健康/幂等判断，避免遗留线程。
131 [高] `win/src/din_bitmap.rs:172` — 用进程级 `OnceLock` 守护写入 `thread_local` 的 `FONT_WEIGHT_TABLES`；渲染线程 respawn 后新线程本地表回到内置点阵，DIN 三档注入永久丢失 → 改 thread_local `OnceCell` 或按代重注入。
132 [中] `win/src/din_bitmap.rs:81` — `ManuallyDrop::new(Some(face.clone()))` 每次 clone 一个 COM 引用却不释放（3×95 次） → 显式释放或改传不 clone 的接口。
133 [低] `win/src/din_bitmap.rs:57` — `DWRITE_FONT_FACE_TYPE_CFF` 硬编码，DIN 若为 TrueType 会静默回退 → 先探测字体文件类型。
134 [中] `win/src/render_d2d.rs:327` — `needs_create` 不比较 `cache.mem_dc != mem_dc`（`:83` 存了却没用），换新 mem_dc 而尺寸不变时 RT 仍绑在已删除 DC 上 → 把 mem_dc 纳入失效条件。
135 [中] `win/src/render_d2d.rs:664` — `scale_alpha` 与 `blit_coverage`(`:745`) 是空实现却满足 `RenderTarget` 契约 → 至少 `debug_assert` 或文档标注"不可用于生产路径"。
136 [低] `win/src/render_d2d.rs:478` — `straight()` 用 `1/a` 反预乘，而 `Rgba::new` 预乘是截断，往返精度损失 → 统一舍入口径。
137 [低] `win/src/render_d2d.rs:22` / `win/src/gfx_d2d.rs:11` — 两处模块头都声称"由 Drop 统一释放 COM"，文件内并无 `impl Drop` → 补显式 Drop 或改述。
138 [中] `win/src/gfx_d2d.rs:627` — `fill_path` 缓存键 `(key,w,h)` 不含 `rect.l/t`，而 `build_path`(`:284`) 把绝对坐标烘进几何，图标随窗口宽度移动后复用旧几何 → 几何改局部坐标 + transform，或把 l/t 纳入键。
139 [中] `win/src/gfx_d2d.rs:478` — `LayoutKey` 只含布局宽度不含高度，`wrap=true` 时高度影响换行 → 把高度纳入键。
140 [高] `win/src/gfx_d2d.rs:546` — `Gfx::frame` 的 painter 无 `catch_unwind`，painter panic 时 `EndDraw` 不执行，RT 卡在 BeginDraw → 加 guard 并在异常路径 `EndDraw`/rebuild。
141 [中] `win/src/gfx_d2d.rs:550` — 仅 `D2DERR_RECREATE_TARGET` 触发 `rebuild()`，其它 `EndDraw` 错误只打日志，RT 永久带错 → 对不可恢复错误也重建。
142 [中] `win/src/gdi.rs:718` — 快速跳过条件不含 `NEEDS_FULL_REUPLOAD`，上屏失败(`:896` 回滚指纹)后同秒下一帧直接返回，重试要等翻秒 → 把闸门纳入跳过条件。
143 [中] `win/src/gdi.rs:1051`（`:1012` 同） — 脏矩形 `top = cells[i].y`，但 `cell_sig`(`:316`) 只把 pad 加进 `h`，描边/阴影会画到 `y-1`，顶行像素漏上传 → `top = y - pad`。
144 [中] `win/src/gdi.rs:959` — `compute_dirty_in` 只比 `a.x/a.y` 判布局漂移，不比 `bbox_x/w/h` → 补比较，与 123 呼应。

## 七、窗口消息循环、线程安全与系统集成（145–164）

145 [高] `win/src/window.rs:1269` — `get_window_size` 被 `gdi.rs:732`（渲染线程）通过裸指针读 `ThreadState.win_w/win_h`，而 UI 线程 `WM_SIZE`(`:813`)/`WM_DISPLAYCHANGE`(`:1044`) 写同字段，无原子无锁；`:1177`"同线程"注释已不成立 → 改 `AtomicI32` 或走消息传递。
146 [高] `win/src/window.rs:641` — 悬浮窗 `wndproc` 是 `extern "system"` 且无 `catch_unwind`，任一分支 panic 跨 FFI 即 abort → 外层包 guard（与 `settings_wndproc` 同）。
147 [中] `win/src/window.rs:484` — 常量表无 `WM_DPICHANGED` 分支，运行中改 DPI/跨屏不重算 → 补分支并触发 dpi + 布局重算。
148 [高] `win/src/window.rs:1026` — `WM_GET_SAMPLER_DIAG` 先 `Arc::into_raw` 再 `PostMessageW`，`:1029` 用 `let _` 吞掉失败，Arc 强引用永久泄漏 → 失败时 `Arc::from_raw` 回收。
149 [中] `win/src/window.rs:1741` — `GetMessageW` 返回 -1（错误）直接 break 且无日志 → 取 `GetLastError` 留痕再退出。
150 [中] `win/src/window.rs:1669` — `WTSRegisterSessionNotification` 返回值被忽略，失败则锁屏检测失效 → 判返回值并记日志。
151 [中] `win/src/gdi.rs:1071` — `SURFACE` 注释"只在 UI 线程访问、实际无竞争"，但 `render()` 已跑渲染线程且 respawn 后可两代线程并存 → 改述并加世代独占。
152 [低] `win/src/gdi.rs:1709` — 文档写"+15% 安全余量"，实现 `*1.4`（`:1713` 内部注释也写 40%） → 统一数值。
153 [中] `win/src/gdi.rs:1687`、`:1699` — `SetWindowPos` 返回值被 `let _ =` 吞掉，尺寸自愈失败无痕 → 判返回值并记日志。
154 [中] `win/src/settings/mod.rs:2547` — `settings_wndproc` 同为 `extern "system"` 且无 `catch_unwind`（`cfg_ref.rs:315` 亦提示此风险） → 包 guard。
155 [中] `win/src/settings/mod.rs:2819` — `timeBeginPeriod(1)` 只在 `!animating` 时配对 `timeEndPeriod`，动画中 `DESTROY_TIMER` 销毁窗口且 `SettingsCtx` 无 Drop → 在 Drop/WM_DESTROY 无条件收尾。
156 [中] `win/src/titlebar.rs:218` — `draw_text` 只还原 `SelectObject(font)`，`SetTextColor`/`SetBkMode` 旧值被丢弃，DC 状态泄漏到后续绘制 → 保存并还原。
157 [中] `win/src/titlebar.rs:33` / `settings/layout.rs:164` / `dpi.rs:41` — 三份 `v*dpi/96` 缩放，取整口径各不相同 → 统一到 `dpi.rs` 并四舍五入。
158 [中] `win/src/titlebar.rs:41` — `CreateFontW` 返回值未判空（HFONT 可为 null） → 检查并回退默认字体。
159 [中] `win/src/dpi.rs:20` — `SetProcessDPIAware()` 返回值被 `let _` 吞、`apply_dpi_awareness` 恒返回 true → 返回实际结果。
160 [中] `win/src/dpi.rs:28` — 未检查 `GetDC` 返回的 HDC 是否有效就传给 `GetDeviceCaps`/`ReleaseDC` → 空 HDC 直接返回 96。
161 [中] `win/src/dpi.rs:36` — `get_primary_monitor_dpi()` 与 `get_system_dpi()` 完全相同，`:13` 注释自承无 per-monitor 支持 → 用 `GetDpiForMonitor` 或据实改名并注明限制。
162 [中] `win/src/tray.rs:31` — `add()` 手写循环拷贝 tip 到 `szTip`，超长截断后可能无 NUL 结尾；同文件已有正确的 `write_w`(`:65`) 却未复用 → 统一用 `write_w`。
163 [中] `win/src/tray.rs:65` — `write_w` 对空 `dst` 会执行 `dst[0] = 0` 越界 panic → 开头 `if dst.is_empty() { return; }` 并补单测。
164 [中] `win/src/menu.rs:38`、`:15` — `GetCursorPos` 失败被 `let _` 忽略使菜单弹到 (0,0)；`AppendMenuW` 返回值一律忽略导致静默缺项 → 分别回退到托盘/屏幕中心、检查并记录。

## 八、设置面板、控件映射与自绘控件（165–192）

165 [高] `win/src/settings/mod.rs:1395` — `MODAL_DIALOG.store(true)` → `ChooseColorW` → `store(false)` 无 RAII；模态内 panic 使标志永久为 true，`ctx_of`(`:661`) 恒 None、设置窗永久冻结 → 用 Drop guard。
166 [高] `win/src/settings/mod.rs:2294` — `on_wm_timer` 只有 ANIM/EDIT/STATUS/STEPPER 分支，没有 pending-flush 分支；防抖窗内的改动只在 `WM_CLOSE`(`:2513`)/`WM_DESTROY`(`:2529`) flush，窗口一直开着就永不落盘 → 加 SAVE_TIMER 到点 flush。
167 [中] `win/src/settings/mod.rs:1116` — `on_saved(now)` 在"入队成功"即调用（`config_io.rs:790` 只表示 `send().is_ok()`），实际写盘失败时 pending 已清、无重试 → 由写线程回调后再 `on_saved`。
168 [中] `win/src/settings/mod.rs:486` — `flush_config` 忽略 `save_config` 返回值且不调 `on_saved()`，失败无日志、pending 不复位导致销毁时重复写 → 判返回值并清 pending。
169 [中] `win/src/settings/mod.rs:2553` — 未处理 `WM_DPICHANGED`、`WM_MOUSEHWHEEL`、`WM_RBUTTONDOWN`、`WM_ACTIVATE`，`ctx.dpi` 只在 `:521` 取系统 DPI → 补 `WM_DPICHANGED` 重算 dpi + rebuild 控件树。
170 [中] `win/src/settings/mod.rs:2613` — `paint_d2d` 在模态时提前 `return true`，绕过 `:2842` 的 `ValidateRect`，而 `on_wm_paint`(`:1683`) 仅在 false 时校验 → 模态期间 WM_PAINT 无限重投，早期返回也校验更新区。
171 [中] `win/src/settings/mod.rs:1824` — 固定态标题栏空白返回 `HTTRANSPARENT`；按 `gdi.rs:1107` 的结论，无同线程后续窗口时鼠标消息被丢弃 → `tb.over` 永不置位、解锁按钮永不显示，改 `HTCLIENT` 并在 WM_MOUSEMOVE 拦拖动。
172 [低] `win/src/settings/mod.rs:1066` — `CfgRef::SnapCorner` 分派副作用仍在，但 `layout.rs:845` 已删对应控件 → 删除不可达分支。
173 [中] `win/src/settings/cfg_ref.rs:13` — 文档称"类型错配时 debug/release panic"，实现只是 `eprintln!` + `return false`（`:314` 测试也断言不 panic） → 改文档。
174 [高] `win/src/settings/cfg_ref.rs:73`、`:133` — `Style::get` 把 `RenderStyle::Classic` 映射成 0，`set` 把 0 写回 `Strip`，读改写静默丢失 Classic → 用 `SelectOpt` 或保留原值。
175 [中] `win/src/settings/cfg_ref.rs:134` — `RenderEngine`/`MemUnit` 的 `set` 对越界 `Select(i)` 落到默认值且返回 true，而 `Lang`(`:135`) 返回 false → 统一 `_ => false`。
176 [中] `win/src/settings/cfg_ref.rs:122` — `FontSize`/`MetricGap`/`LabelValueGap` 的 `set` 用 `as f32`/`as u32` 无钳位（负/NaN 落 0），只有 `Opacity`(`:131`)、`MetricOffset`(`:161`) 有 clamp → 复用 core 的 `FONT_PX_MIN/MAX` 与间距钳位。
177 [中] `win/src/settings/cfg_ref.rs:198` — `roundtrip_cases` 全从 `Config::default()` 起 set→get，抓不到 174 这类不对称 → 补"预置非默认值 → get"用例。
178 [中] `win/src/settings/fluent/mod.rs:5` — `#[allow(dead_code)]` 仍在且留"S2 接入后移除"TODO，`tokens.rs` 多数字段无消费方 → 移除 allow 并列出真正未接线字段。
179 [低] `win/src/settings/fluent/tokens.rs:83` — 注释写 50ms/167ms，实际 `:163` 是 0.1/0.2 秒，`:295` 测试还明确"勿写 0.05/0.167" → 改注释。
180 [低] `win/src/settings/fluent/tokens.rs:4` — 文档"所有 token 均以 D2D ColorF 存储"，结构体还含 `f32/u16/&str/[f32;4]` → 改为"颜色 token"。
181 [中] `win/src/settings/icons.rs:112` — `svg_path_data` 只认 `" d=\""`，`d='…'`/`d = "…"`/多 `<path>` 全返回空，图标静默消失 → 放宽属性解析并遍历所有 path。
182 [中] `win/src/settings/icons.rs:264` — `read_num` 单个数解析失败即 `None`，调用方 `break` 截断整条路径后续段 → 跳过坏数字继续。
183 [中] `win/src/settings/icons.rs:190` — `arc_to_cubics` 的 `rx<=0` 判不住 NaN，`dtheta=NaN` 会生成 NaN 控制点 → 加 `!is_finite()` 早退。
184 [低] `win/src/settings/icons.rs:520` — 单测用 `EPS=24.0`/`w>VIEW*0.3` 魔数且无 malformed/多路径用例 → 提常量并补边界用例。
185 [中] `win/src/settings/layout.rs:121` — `RADIUS=30` 与 `fluent/tokens.rs:141` 的 `border_radius_medium=4.0`（注释"S5 统一改 4"）冲突 → 二选一。
186 [中] `win/src/settings/layout.rs:156` — `GAP_MAX=64.0` 硬编码，而 `FONT_MIN/MAX`(`:153`)、`OFF_MIN/MAX`(`:160`) 都引用 core 常量 → 同样引用 core 间距钳位。
187 [中] `win/src/settings/layout.rs:1437` — `build_pages` 冒烟测试只固定 96dpi/900/Lang::Zh 一组，且从用户真实 `%APPDATA%` 读配置 → 参数化多 DPI/宽度并用固定 fixture。
188 [中] `win/src/settings/state.rs:133` — `Page.scroll_y` 注释"负值向上滚动"与 `scrollbar.rs:13` 的"0..=max 向下变大"及 `layout.rs:253` 的 `-scroll_y` 矛盾 → 统一符号文档。
189 [中] `win/src/settings/state.rs:172` — `sync_via_cfg` 对 `Value::Color(_)` 是空臂，色板控件读不到配置颜色 → 补 `state` 字段或覆盖 `sync`。
190 [中] `win/src/settings/state.rs:187` — `SpatialGrid::new` 未校验 `cell_size > 0`，除零 panic → `.max(1)` 或断言。
191 [中] `win/src/settings/scrollbar.rs:163` — `hit_test` 的 x 上界写到 `win_w`（含边界外 1px），Thumb 用闭区间，与 `state.rs:223` 半开约定及 `+2` 容差不一致 → 统一半开并提常量。
192 [中] `win/src/settings/scrollbar.rs:119` — `win_h=0`/`content_h` 极大时 `track_h` 压到 1、`travel=0`，ratio 恒 0、滚动卡死 → 给 `track_h` 下限或早退。

## 九、动效、防抖、控件数值与安装残留（193–200）

193 [高] `guide-app/src-tauri/src/install.rs:190` — `UninstallString` 手工拼 `"{install_dir}\{GUIDE_EXE_NAME}" --uninstall`，但 `run_install` 只释放 `MAIN_EXE_NAME` 与驱动文件，盘上并无该 exe，卸载入口指向不存在文件 → 指向实际存在的引导 exe 并用 `Path::join` 拼接。
194 [中] `win/src/settings/widget.rs:408` — `lerp_color` 的 RGB 各 `.clamp(0,1)`，alpha 未钳位 → 越界 alpha 直接进 `ColorF::rgba`，同样 clamp。
195 [中] `win/src/settings/widget.rs:347` — `value_box_rect`/`track_rect`(`:358`) 在窄矩形下生成 `left > right` 的 RECT → 用 `.max(left)` 钳制，避免负尺寸绘制/命中。
196 [中] `win/src/settings/motion.rs:99` — 注释"调用失败或显式关闭 → 视为关闭"，代码 `ok.is_err()` 时返回 1（开启动效），括号内"安全兜底：开启动效"又自相矛盾 → 统一并改注释。
197 [中] `win/src/settings/motion.rs:80` — `ANIM_CACHED`/`HC_CACHED` 进程级缓存永不失效，运行中切换"减弱动效/高对比"不生效 → 监听 `WM_SETTINGCHANGE` 清缓存。
198 [低] `win/src/settings/motion.rs:40` — `frame_dt.clamp(0,0.1)` 与 `0.002/0.02` 阈值均为魔数，长帧被截成慢动作 → 提常量并补 `frame_dt>0.1`/NaN 单测。
199 [中] `win/src/settings/tasks.rs:9` — 文档"Drop 时强制 flush"，但 `DebouncedSave` 无 `impl Drop`，flush 实际在 `mod.rs:2513/2529` → 改述或补 Drop。
200 [中] `win/src/settings/tasks.rs:28`、`:89` — `on_change` 在防抖窗内只置 pending、模块内无"到点补写"机制；`failed_save_keeps_pending` 也只测"不调 on_saved"，未测随后重试 → 提供 `is_due(now)` 由调用方驱动，并补重试断言。

## 十、引导后端：安装、驱动、命令与状态（201–230）

201 [中] `guide-app/src-tauri/src/lib.rs:154` — `--elevated`/`--uninstall`/`--autostart` 三处各自全量扫 `args().any`，且 `driver.rs` 拼成带前导空格的 `" --elevated"` → 在 `run()` 入口集中解析一次为枚举并复用。
202 [中] `guide-app/src-tauri/src/lib.rs:169` — `config_corrupted` 只探 `SystemConfig::load().is_err()`，而 load 对文件缺失返回 `Ok(default)`，其余 4 个 .cfg 损坏完全探不到 → 遍历 5 个文件汇总失败集合。
203 [中] `guide-app/src-tauri/src/lib.rs:180` — `installed_detected` 仅判路径 `exists()`，0 字节/截断残留也会弹"已安装" → 加最小体积或 `MZ` 头校验。
204 [中] `guide-app/src-tauri/src/lib.rs:213` — `get_primary_screen_size` 用 `GetDesktopWindow` 的 RECT，失败回退硬编码 1920x1080 → 改 `GetSystemMetrics(SM_CXSCREEN/SM_CYSCREEN)`，回退值提常量。
205 [高] `guide-app/src-tauri/src/lib.rs:80` — `INSTALL_BUSY` 与 `UNINSTALL_BUSY` 是两把独立锁，安装与卸载可并发改同一批文件和服务 → 共用单把 BUSY 互斥全部装卸流程。
206 [中] `guide-app/src-tauri/src/lib.rs:241` — `install_run` 成功只 emit `install-done`，命令 Ok 值不带终态；前端仅在 `!r.ok` 时收敛 error → 返回值携带 status 兜底。
207 [中] `guide-app/src-tauri/src/lib.rs:301`、`:638` — `config_patch` 与 `guide_save_config` 是两份逐字重复的"4 文件扇出" → 抽 `write_all_configs(patch)`。
208 [高] `guide-app/src-tauri/src/lib.rs:304` — 四次 `load().unwrap_or_default()`：IO 读取失败降级为默认值后被 `save()` 覆盖用户配置 → 把 Err 透传为 `CONFIG_WRITE_FAILED`。
209 [中] `guide-app/src-tauri/src/lib.rs:470` — `RegOpenKeyExW` 成功后、`RegCloseKey`(`:497`) 前用 `current_exe()?` 提前返回，HKEY 泄漏 → 先取 exe 路径再开注册表，或用 RAII 包装。
210 [低] `guide-app/src-tauri/src/lib.rs:105` — `BootstrapResume` 缺 `guide_state.rs:36` 已存、前端 `useGuide.js:679` 读取的 `style` 字段 → 补字段并在 `:158` 构造时填入。
211 [中] `guide-app/src-tauri/src/lib.rs:622` — `config_load` 把 `app.font_size`(f32) 直接 `as u32` 截断（13.5→13） → round 后下发单一 `font_size`，或让前端接受浮点。
212 [中] `guide-app/src-tauri/src/lib.rs:702` — `guide_finish` 用 `if let Ok(mut state)` 静默吞掉 `GuideState::load()` 失败，与 `:699`"必须留痕"注释不符 → load 失败也写一条日志。
213 [中] `guide-app/src-tauri/src/lib.rs:711` — 注释称快捷方式失败"必须可见"，实现只写 install.log 且命令仍返回 Ok → 透传 `shortcut_created` 布尔或错误 hint。
214 [中] `guide-app/src-tauri/src/lib.rs:789` — `app_quit_all` 每次调用 spawn 一个 `sleep(120ms)` 线程，重复点击起多个线程，120 为裸魔数 → 加原子幂等标志、延迟提常量。
215 [中] `guide-app/src-tauri/src/lib.rs:833` — `guide_view_logs` 对 `read_dir` 失败返回空 content 且 `r.ok=true`，用户以为"暂无日志" → 把目录读取失败透传为错误码。
216 [中] `guide-app/src-tauri/src/lib.rs:861` — 日志按修改时间倒序 push 三个文件后再 `lines().rev().take(count)`，取到的是最旧文件末尾的旧日志 → 逐文件倒序取行后合并。
217 [中] `guide-app/src-tauri/src/lib.rs:952`、`:959` — `tracing_subscriber.try_init()` 用 `let _` 丢结果；`--autostart` 分支 `let _ = start_main_process(None)` 后无条件 `exit(0)` → 前者失败留痕，后者检查返回值并非零退出。
218 [中] `guide-app/src-tauri/src/lib.rs:968` — 单实例判定失败直接 `exit(1)`，未把已有实例窗口置前（对比 `app/main.rs:186`） → 向已有窗口投递 `WM_OPEN_SETTINGS` 类消息再退出。
219 [中] `guide-app/src-tauri/src/{lib.rs:1009,main.rs:4}` — `run()` 末尾 `.expect(...)` 且引导侧无 panic hook，窗口线程 panic 静默消失 → 加最小 `set_hook` 写日志/minidump。
220 [中] `guide-app/src-tauri/src/install.rs:46` — `same_content` 注释"逐字节等价"，实现只比大小 + FNV-1a，碰撞即跳过更新 → 大小相同直接逐字节 eq 或改注释。
221 [中] `guide-app/src-tauri/src/install.rs:64` — `atomic_replace` 用 `fs::write` 未 fsync，rename 后 `verify_file` 只校验大小 → 写后 `sync_all()`，校验改用 `same_content` 口径。
222 [低] `guide-app/src-tauri/src/install.rs:68` — `raw_os_error` 判 `5 | 32` 为裸魔数 → 用具名 `ERROR_ACCESS_DENIED`/`ERROR_SHARING_VIOLATION`。
223 [中] `guide-app/src-tauri/src/install.rs:98` — 临时文件固定 `"<exe>.tmp"`，与 `config.rs:1040` 的 pid+seq 唯一名口径不一致 → 复用唯一临时名。
224 [低] `guide-app/src-tauri/src/install.rs:161` — `write_uninstall_registry(_app: &AppHandle)` 的形参全程未用 → 删除形参与实参。
225 [中] `guide-app/src-tauri/src/install.rs:212` — `e.code().0` 是 HRESULT 却传给 `io::Error::from_raw_os_error`（期待 Win32 码） → 统一转字符串 Err 或取底层 Win32 码。
226 [低] `guide-app/src-tauri/src/install.rs:224` — `("NoModify",1)/("NoRepair",0)` 裸值，`NoRepair=0` 实为允许修复 → 提具名常量并核对规格。
227 [中] `guide-app/src-tauri/src/install.rs:281` — `log_install` 用 `let _ = (|| io::Result)()` 吞掉全部写盘错误 → 失败时至少 eprintln 一次（防递归）。
228 [中] `guide-app/src-tauri/src/install.rs:396` — `Heartbeat::start` 注释"不抢占"，实现在 `swap` 命中后 `store(false)` 反而关掉正在跑的心跳 → 命中直接 `return None`。
229 [中] `guide-app/src-tauri/src/install.rs:476` — `emit_progress` 与心跳读锚点无序号/锁序保证，100% 后仍可能收到旧 pct 导致进度回退 → payload 带单调 seq。
230 [中] `guide-app/src-tauri/src/install.rs:488`、`:497` — `skip_step` 把 `Err(e)` 也记成"跳过"，总结行"失败 0"谎报成功 → Err 一律记失败并单独计数。

## 十一、引导后端（续）、前端桥接与向导（231–260）

231 [低] `guide-app/src-tauri/src/install.rs:619` — 三种 `InstallMode` 结束标签都写"安装结束"，与 begin 的"驱动重试/修复安装开始"不配对 → 用 `mode.as_str()` 拼标签。
232 [中] `guide-app/src-tauri/src/install.rs:589` — 驱动资源 `get_driver_bytes` 失败被包成 io::Error 后交给 `skip_step` 记"跳过"，用户不可见 → 单独记失败并带资源名。
233 [中] `guide-app/src-tauri/src/install.rs:698` — `delete_scheduled_task` 的 `/Query` 非 0 一律 `Ok(false)`，把"拒绝访问"当"任务不存在"，与 `:686` 注释矛盾 → 按错误码区分。
234 [中] `guide-app/src-tauri/src/install.rs:750`、`:873` — 卸载第 3 步只 `remove_dir`，非空即 fail 且未清理残留 .sys；`keep_config` 用 `entries.flatten()` 吞掉逐条读取错误 → 先枚举删文件再删目录、逐条记录残留与 Err 原因。
235 [中] `guide-app/src-tauri/src/driver.rs:60` — `is_admin()` 用 `cfgmgr32.dll` 是否存在判管理员，标准用户恒 true，与 `pre_launch_checks.rs:85` 的 `IsUserAnAdmin()` 口径冲突 → 改 `CheckTokenMembership`。
236 [中] `guide-app/src-tauri/src/driver.rs:88` — `stop_service` 丢弃 `ControlService` 返回值，服务已停止也轮询 5s 超时 → 按返回码分支，`ERROR_SERVICE_NOT_ACTIVE` 直接成功。
237 [中] `guide-app/src-tauri/src/driver.rs:169` — `is_our_driver_binary` 用 `starts_with("C:\Windows\AAXSysPulse\")`，SCM 常见的 `\??\C:\...` 被判 foreign → 先剥 `\??\`/`\SystemRoot\`。
238 [中] `guide-app/src-tauri/src/driver.rs:247` — `DeleteService(svc)?` 提前返回会跳过 `:248` 的 `CloseServiceHandle` 与 sc_manager 释放 → 先收集结果、统一关句柄再返回。
239 [中] `guide-app/src-tauri/src/driver.rs:275`、`:315` — 驱动文件删除失败仍 `Ok(true)`；`release_driver_sys` 在 rename 成功后才校验大小，坏 .sys 已留在系统目录 → 失败返回 Err、校验失败同时删 target（或先校验 temp）。
240 [高] `sysoverlay/crates/app/src/main.rs:156`、`:272` — `setup_panic_hook()` 装的 `set_hook` 被 `probe::install_panic_hook()`(`probe.rs:127`) 覆盖，minidump 永不生成 → 在 probe 的 hook 里串联 minidump，或全仓只装一个 hook。
241 [中] `guide-app/src-tauri/src/driver.rs:351` — `CreateServiceW` 注册失败无回滚（55% 已释放的 .sys、45% 已删的旧服务都不复原） → 失败时清理已落地驱动并记录回滚步骤。
242 [中] `guide-app/src-tauri/src/driver.rs:500` — `restart_as_admin` 不检查 `ShellExecuteW` 返回值就 `exit(0)`(`:511`)，拉起失败即"两个窗口都没了" → 判返回值（>32 才算成功），失败返回 `PRIVILEGE_ERROR` 且不退出。
243 [低] `guide-app/src-tauri/src/driver.rs:363`、`:408` — `1073`/`1056` 裸魔数 → 用具名 `ERROR_SERVICE_EXISTS`/`ERROR_SERVICE_ALREADY_RUNNING`。
244 [中] `guide-app/src-tauri/src/resources.rs:27`、`:44` — `std::fs::read` 失败统一转 `InstallError::Io` 丢失资源名；资源断言用 `conf.contains("\"resources/NAME\": \"NAME\"")` 子串匹配，JSON 加空格即假失败 → 两处都带 name、改用 serde_json 解析后按键断言。
245 [中] `guide-app/src-tauri/src/shortcut.rs:44` — `CoInitializeEx` 的 `S_FALSE`(HRESULT(1)) 也被 `is_ok()` 判真，多调一次 `CoUninitialize` 破坏引用计数 → 只把 `S_OK` 计为 initialized。
246 [中] `guide-app/src-tauri/src/shortcut.rs:55`、`:107` — `to_string_lossy()` 转宽字符，含非 UTF-8 用户名会变 U+FFFD；非 Windows 分支把"不支持平台"包成 `ComError` → 改 `OsStr::encode_wide`、加 `Unsupported` 变体。
247 [中] `guide-app/src-tauri/src/config.rs:85` — `parse_u32/parse_i32/parse_f32` 失败一律 `unwrap_or(0)`，`interval_ms=0` 被吸到最快档、坐标变 0 → 失败回落该字段默认值。
248 [中] `guide-app/src-tauri/src/config.rs:177` — `admin_check_continue_count` 引导侧是 u32，core 是 u8 且 `system.rs:211` `.min(3)`；写入 >3 会被钳成 3 立即触发"连点 3 次跳过" → 两端统一类型/钳位并补跨进程单测。
249 [中] `guide-app/src-tauri/src/config.rs:1008` — `ensure_all_files` 只补缺失文件，损坏但存在的 .cfg 一律不动，与 `config_corrupted` 修复语义不配套 → 对解析失败的文件重写默认值。
250 [中] `guide-app/src-tauri/src/process.rs:64`、`:165`、`:280` — `CreateMutexW` 失败仅 warn 后放行；`GetProcessTimes` 失败伪造 0.0% CPU（与 `:122`"不再伪造"矛盾）；`parent().unwrap_or(&exe_path)` 会把 exe 文件当工作目录 → 分别返回 false/返回 None/返回 Err。
251 [低] `guide-app/src-tauri/src/guide_state.rs:52` — `load()` 解析失败被 `lib.rs:157` 的 `unwrap_or_default()` 吞掉，坏存档静默当新引导 → bootstrap 对 Err 单独留痕。
252 [中] `guide-app/src/wizard/bridge.js:150`、`:143` — catch 只处理 string / `e.message` 后 `JSON.parse`，Tauri 对 struct 型 GuideError reject 的是对象，`code/step/hint` 全丢成 UNKNOWN；`Promise.race` 超时后原 invoke 未取消且超时被包成 UNKNOWN → 直接读对象字段、给超时独立 code。
253 [中] `guide-app/src/wizard/bridge.js:259`、`:101`、`:35` — `finish()` 硬编码 `createShortcut:true` 忽略勾选且重复建快捷方式；`sealWrites` 幂等却不可解除，卸载后"重新打开引导"所有写命令仍被拒；`isAvailable()` 无 `typeof window` 守卫 → 分别改为可选参数 / 受控解除入口 / 加守卫。
254 [中] `guide-app/src/wizard/bridge.js:297`、`:208` — 注释称"同步返回取消订阅函数"，实际返回 `setup.then` 闭包，注册完成前卸载会泄漏 listener；`demoInvoke` 的 default 对任何未列命令返回 `{ok:true}` 掩盖漏实现 → 修注释 + 已取消标志、默认返回 `ok:false`。
255 [中] `guide-app/src/wizard/composables/useGuide.js:121`、`:220` — 前端 defaults（opacity 80 / fontName 14 / 6 项）与 `config.rs` 默认（100 / 12.0 / 9 项全开）分散两份且已漂移；`snapshot()` 输出的 `font_value` 后端无人消费 → 抽共享默认值表 + 交叉断言、删死字段。
256 [中] `guide-app/src/wizard/composables/useGuide.js:296`、`:695`、`:895` — `cmd.backup()` 不检查返回值；`config_corrupted` 提示"已恢复为默认设置"但后端只补缺失文件；`install-progress` 直接赋值 pct 无单调保护 → 分别补 `!r.ok` 报错、改文案、带 seq 只接受更大值。
257 [中] `guide-app/src/wizard/composables/useGuide.js:939`、`:959`、`:966`、`:994` — `launchMain`/`saveConfig` 不检查 `r.ok` 仍置 `mainLaunched`；`setOverlayPos` 忽略结果后重复通知；`setAutostart` 先改 state 失败不回滚 → 全部按 `r.ok` 收敛并回滚 UI 态。
258 [中] `guide-app/src/wizard/composables/useGuide.js:724`、`:42`、`:45` — `resumeGuide` 直接赋 `state.page` 绕过 flow 守卫；两条 FLOW 都不含第 2 页使 `Page02Privilege.vue` 成死代码；`LAST_PAGE` 与 `:672`/`:341` 的硬编码 14 分两份 → 走白名单 goTo、删除或接入死分支、统一引用常量。
259 [中] `guide-app/src/wizard/GuideWizard.vue:71`、`:367` 与 `pages/Page03Install.vue:7` — error 态仍允许 Alt+→ 翻页绕过重试；"已退出（演示）"文案无 DEV 守卫；`onMounted` 无条件 `startInstall()`，断点恢复/换页回来会自动重跑安装 → 分别禁止换页、加 `import.meta.env.DEV`、仅 `status==='idle'` 自动开跑。
260 [中] `guide-app/src/wizard/pages/{Page05Orientation.vue:4,Page07Order.vue:36,Page12Position.vue:16,Page13Autostart.vue:9,Page14Done.vue:5}` — 重复点同值仍触发 4 个 cfg 写入；拖动中的全局监听只在 `onUp` 移除、换页卸载会遗留；posX/posY 用硬编码 834/200 假坐标；`pick()` 重复写注册表；`fetchInstallInfo` 失败静默不显示卡片 → 加值变化判断、`onUnmounted` 清理、异步就绪前禁用、失败留提示。

## 十二、入口、构建、工程卫生与文档校准（261–280）

261 [中] `sysoverlay/crates/app/src/main.rs:246`、`:323`、`:186` — `VersionSpanTooLarge` 分支与 `outcome.changed` 分支都 `let _ = save_config(...)`，写盘失败时"配置已重置"提示是假的、连点计数机制静默失效；`handle_second_instance()` 结果也被丢弃 → 判返回值、失败写日志并保留内存态。
262 [中] `sysoverlay/crates/app/src/pre_launch_checks.rs:30`、`:102` — `current_date()` 手算 UTC 日期，与引导 `chrono::Local`、`main.rs:84` 的 `GetLocalTime` 三套日历口径；`is_driver_loaded` 只要 `OpenServiceW` 成功就返回 true，未 `QueryServiceStatus` 判 RUNNING → 统一本地时间源、补状态查询。
263 [中] `sysoverlay/crates/app/src/pre_launch_checks.rs:160`、`:182` — 非法非零 `last_date` 被折成 `u64::MAX` 使检查被永久静默跳过；点"取消"返回 `changed:true` 却一行未改，导致调用方白存一次盘 → 非法值按"未检查"短路、取消返回 `changed:false`。
264 [中] `sysoverlay/crates/app/src/driver_installer.rs:3`、`:88`、`:78` — 整文件未被任何 `mod` 声明（索引 §3 已标注死代码）；`Some(0)` 被当 `Ok(true)` 继续运行可能与已拉起实例组成双实例；`0x08000000` 裸魔数 → 删除文件，或补 mod + 单测、0 直接退出、用 `CREATE_NO_WINDOW` 常量。
265 [中] `sysoverlay/crates/app/build.rs:61`、`:79` — `OUT_DIR` 非 UTF-8 时 `to_str().unwrap_or_default()` 把空串传给 rc.exe；`find_rc_exe` 硬编码 `C:\Program Files (x86)\Windows Kits\10\bin` → 用 `OsStr` 传参、读 `WindowsSdkDir` 环境变量。
266 [中] `guide-app/src-tauri/build.rs:40`、`:66`、`:120` — 主程序 exe 路径硬编码是 `MAIN_EXE_NAME` 的第二份副本；`rerun-if-changed` 未声明 `resources/` 与 `tauri.conf.json`；驱动体检只验 `MZ` + `>4096` 裸阈值 → 共用契约常量、补齐监听、提具名常量并至少校验 `PE\0\0`。
267 [中] `Cargo.toml:26`、`:40` — `[workspace.lints.clippy]` 只有一条 `too_many_arguments=allow`，各 crate 又自设 `cfg_attr warn`，口径分散；`[profile.release] overflow-checks = false` 使本报告多处裸加法/乘法在 release 下静默回绕 → 集中 lint 例外清单、对外部输入处改用 `saturating_*`/`checked_*`。
268 [中] `guide-app/src-tauri/tauri.conf.json:27`、`capabilities/default.json:6` — `"csp": null` 与 `withGlobalTauri:true` 并存；`listen` 依赖隐式 event 权限（`bridge.js:310` 注释正是这类静默拒绝） → 给最小 csp 或关闭全局 Tauri、显式声明 `core:event:default`。
269 [低] `guide-app/package.json:9`、`scripts/check-bridge.mjs:39`、`WizardSheet.vue:99` — `check-bridge` 只挂 build 不挂 dev；门禁只匹配命令名不校验 args 字段名；`<Teleport defer>` 需 Vue 3.5+ 而声明 `^3.4.21` → 加 `predev`、补命令参数名静态比对、把 vue 下限锁到 `^3.5.0`。
270 [中] `win/src/logging.rs:75`、`:98`、`:100`、`:200` — 体积会计按 `text.len()+32` 低估了 `prefixed()`(`:157`) 每行约 23 字节前缀；打开失败后每次写仍重跑 create_dir_all + prune_dir；`LOCALAPPDATA` 缺失静默 return 无任何留痕；字节和用普通加法可能溢出 → 按前缀×行数计、加失败标志短路、写 degrade 事件、用 `saturating_add`。
271 [中] `win/src/perf.rs:110`、`:368`、`:390`、`:261` — `end_render` 每帧无条件落盘；`note_barrier_commit` 注释自承"每次提交落盘一行"；`freq()` 在 QPF 失败兜底 1000 而 `qpc()` 仍返回真实计数，耗时放大 QPC频率/1000 倍；`MSG_COUNT.fetch_add(0,...)` 令每条消息都进 `maybe_report_msgs()` → 只记慢帧/抽样、并入 5s 聚合、兜底改用同源 `SystemTime`、直接按窗口时间门控。
272 [中] `win/src/probe.rs:29`、`:152` — `DEBUG` 默认 true 与"调试日志默认关"语义相反；`perf_flag()` 与 `perf.rs:47` 重复解析 `AAXSYSPULSE_PERF` 且规则不一致 → 默认 false 由配置显式开启、复用同一开关函数。
273 [中] `win/src/mem_tune.rs:47`、`:35` — 注释自承 check-then-store 竞态（两线程可连修两次）；启动 30s 后 trim 的线程 spawn 失败被 `.ok()` 吞掉 → 用 `compare_exchange`、记录失败。
274 [中] `win/src/autostart.rs:56`、`:66`、`:274`、`:342` — `GetModuleFileNameW` 固定 512 缓冲且不检测截断；`command_line()` 只加双引号，路径含尾随反斜杠/引号转义错误；`/TR` 内层引号被 `Command::args` 二次转义；任务创建成功后忽略 Run 项删除失败可能双启动 → 循环扩容、按 `CommandLineToArgvW` 规则转义、用 `raw_arg`、检查并回滚。
275 [中] `win/src/elevate.rs:42`、`:47` 与 `win/src/restart.rs:27`、`:43` — 提权/快速重启同样用 512 缓冲且只传 `--elevated`/`--restart`，原命令行参数在重启后丢失；`ShellExecuteExW` 失败只 log 不返回状态 → 循环扩容、透传并转义原参数、返回 bool 给调用方提示。
276 [中] `win/src/theme.rs:136`、`:146` 与 `win/src/i18n.rs:6`、`:10` 与 `win/src/app_icon.rs:62` — `size=4u32` 硬编码且不校验返回 size；i18n 注释把 `(en, zh, key)` 写成 `(key, en, zh)`、把匿名 `const _` 说成 `CHECK_TABLE`；图标注释称"cx/cy 传 0 用原始尺寸"却未带 `LR_DEFAULTSIZE` → 分别改 `size_of::<u32>()`、修注释、补标志或用帧尺寸。
277 [中] `win/src/history.rs:34`、`:25` 与 `core/src/history.rs:67`、`:55` — `rx_bps >> 10` 把 0~1023 B/s 归 0 且注释称"KB/s 无损"；`load_full→make_mut→store` 非 CAS 且未声明单写者；`max()` 以 0.0 起算，全负样本返回 0；`values()` 与 `max()` 重复环形下标计算 → 分别说明实为 KiB/s、写明单写者约束、`NEG_INFINITY` 起算、抽 `idx_at()`。
278 [低] `core/src/history.rs:51`、`core/src/snapshot.rs:64`、`:124`、`core/src/units.rs:150`、`:103`、`:15`、`core/src/color.rs:53`、`:77`、`core/src/layout.rs:76`、`:184`、`:182`、`core/src/power.rs:28` — 一批小口径：`values()` 实际分配却称"零分配"；`Metric<T>` 拆成两条 derive；`DiskStat` 用 f64 而 `NetStat` 用 u64；`decimals>9` 静默截成 9；`-0.4` 输出 `"-0"`；双大数超 32 字节静默截断；`from_hex` 按字节切片遇非 ASCII panic；亮度权重两处重复；`RowsShown::all()` 却把 power 设 false；`compute` 与 `scale()` 两份缩放公式；`dpi=0` 导致负 `inner_w`；注释引用 `config.rs:323`/`scheduler.rs:154` 行号已漂移 → 逐条修文档/统一实现/补边界单测。
279 [中] 仓库卫生（`dist/` 4 个 `.bak*`、`docs/reviews/_quarantine-*` 两份、`docs/reviews/_quarantine-*/*.dead` 等） — 旧 exe 备份与隔离目录与当前 0.7.1 产物混放，容易被误打包或误当交付物 → 移入单一归档目录并在构建脚本里显式排除；`docs/reviews/2026-09-21-架构全量审查报告.md`(462KB) 与本报告并存，建议在索引 §5 增一行"历史审查产物位置"便于定位。
280 [中] `CODE_INDEX.md` 全局 — 索引自身多处与真实代码漂移（详见下列各条），且缺少"逐条可核对"的机制 → 把关键事实（模块数量、文件路径、常量值、消息号）改成可由单测/构建断言生成的锚点，至少让 §3 的模块数、§4 的文件字段、§6 的雷区行号不再手抄。

## 十三、CODE_INDEX.md / README 逐条校准（281–300）

281 [中] `CODE_INDEX.md:102` / `:137` — 把 `Shape` 列为 `types.rs` 领域枚举，实际 `config.rs:571` 注明"外形已移除"、types.rs 无该枚举 → 删除。
282 [中] `CODE_INDEX.md:184` / `README.md:173` — 称 `appearance.cfg` 含"透明度/形状"，实际透明度在 `system.rs:72`、shape 是死键，`appearance.rs` 只写 bg/fg/字号/字重/间距/阴影/描边 → 改写该行。
283 [中] `CODE_INDEX.md:185` / `README.md:174` — 称 `monitor.cfg` 含"顺序/order"，实际 `monitor.rs:294-305` 测试断言不含 order，order 在 layout 文件 → 改写。
284 [中] `CODE_INDEX.md:186` / `README.md:175` — 称 `layout_h.cfg` 含"窗口几何"，但 `config/layout.rs:38-39` 明确不写 `[window]`，几何唯一归属 `system.cfg`；两个文档还互相矛盾 → 统一为"逐指标偏移 + order"。
285 [中] `CODE_INDEX.md:109` / `:142` — 称 scheduler 有"死区/Idle-Active"，实际无该状态机、`cadence.rs` 死区常量全未引用 → 修正（对应 091/096）。
286 [中] `CODE_INDEX.md:150` — 把 `mcfg.rs` 描述为"分享容器（导出脱敏）"，但 `:51-57` 只导出 4 个块（仅当前激活布局），而 §1/§4 强调 5 个 .cfg → 注明"导出不含另一布局文件"。
287 [中] `CODE_INDEX.md:209` — §6.8 强调 version 写在 `[system]`，但 core `system::parse` 从 `[meta]` 读（032 已证） → 注明读写两侧差异，避免改代码时按文档改错。
288 [中] `CODE_INDEX.md:47` / `:205` — 磁盘 PDH 路径写成双反斜杠 `\\PhysicalDisk(_Total)\...`，与 §6 第 4 条"必须单反斜杠"自相矛盾，且称"proc_disk.rs:20 注释明确"（实际路径在 `:28-29`、说明在 `:16-27`） → 统一文本与行号。
289 [中] `CODE_INDEX.md:163` — 称 `win/src/lib.rs` 的 `pub mod` 共 33 个，实测 34 个 → 改为 34 或去掉写死数字（建议后者，与 280 呼应）。
290 [中] `CODE_INDEX.md:87` — 自启只写"Run 键 + StartupApproved 双向保障"，实际 `autostart.rs:185-203` 主推计划任务（`Mode::Task` / `/RL HIGHEST`），Run 仅无管理员时回退 → 补计划任务机制。
291 [中] `CODE_INDEX.md:48` — 网络只写"GetIfEntry2 增量（物理 NIC 汇总）"，实际还有 `GetIfTable2` 全表枚举、`NotifyIpInterfaceChange` 事件重枚举、别名过滤（`proc_net.rs:113-127`） → 补齐。
292 [中] `CODE_INDEX.md:86` — "DPI 感知"过于笼统，实际仅启动期 `SetProcessDPIAware()`，无 `GetDpiForWindow`/`WM_DPICHANGED`（161/147 已证） → 注明限制。
293 [中] `CODE_INDEX.md:190` — 只说 `config_io` 是"写 .tmp → MoveFileExW 原子写"，实际 1422 行还含降级禁写闸门、就地升级、导入备份回滚、异步写线程 → 补全职责，否则改配置时极易漏改一侧。
294 [中] `CODE_INDEX.md:113` — 把 `win/src/history.rs` 描述为"共享 ArcSwap 缓冲"，但该文件 `:9-11` 自述 `get()`/`peek_seq()` 零调用方、曲线窗口不存在 → 标注"当前仅写侧在跑"。
295 [中] `CODE_INDEX.md:55`、`:52` — `sampler.rs` 只写"组装 Snapshot + 失败退避"，实际还承担 Util EMA、磁盘 PDH 按需实例化、GPU 厂商优先短路；`msr.rs` 只写"读 MSR"，实际大头是 SCM 服务创建/BinaryPath 自愈/绑核/超时隔离线程 → 补全职责描述。
296 [中] `CODE_INDEX.md:156` — `font.rs` 描述为"8×16 ASCII 点阵（占位，Phase 3 接 GDI）"，实际已有 DWrite DIN 注入、浮点缩放、字形图集（111 已证） → 改写，并与 `ui/src/font.rs:6` 头注释一起修。
297 [中] `CODE_INDEX.md:84`、`:85`、`:63`、`:64` — `tasks.rs` "Drop 强制 flush"不成立（199）；`titlebar.rs` 只被 `settings/mod.rs` 引用、并无"图表/进程窗口"；看门狗实体在 `gdi.rs::render_watchdog_tick` 而非 `render_thread.rs`；D2D 设备丢失重建在 `render_d2d.rs`/`gfx_d2d.rs`，`gdi.rs` 只有会话级禁 D2D → 四条指向全部校正。
298 [中] `CODE_INDEX.md:83`、`:158`、`:80` — Fluent token 仍挂 `#[allow(dead_code)]` 且圆角 token(4) 与 `layout.rs:121 RADIUS=30` 冲突；`frame_hash` 生产路径已被 `hash_from_cells` 取代（`:1446` 注释"勿改回"），只剩单测；`scrollbar.rs` 号称"坐标唯一真值"却与 `state.rs:133` 的符号注释相反 → 逐条改述。
299 [中] `CODE_INDEX.md:122`、`:188` / `README.md:178` — `config_patch` 这个 `#[tauri::command]` 实际在 `lib.rs:301-329`，`config.rs` 只有各 `patch_from_json`；`config.txt` 迁移触发条件是"config.txt 存在 **且至少一个 .cfg 缺失**"，而引导 `config.rs:1008` 会先补齐 5 个，跑过引导的机器上迁移被跳过 → 改指向、注明前提。
300 [中] `README.md:59`、`:253`、`:66` / `CODE_INDEX.md:13`、`:33` — 监控项数量三处不一致（README 说 6 项、`useGuide.js` 与 `config.rs` 是 9 项、索引 §0 列 8 项漏了 disk_read/disk_write）；消息号段（009）、PDH 反斜杠（288）、"唯一真源"（007）均需同步 → 以 `Metric::ALL` / `contract.rs` 为唯一真值一次性对齐三个文档。

---

## 附：按层面分布

| 层面 | 编号 | 条数 |
|------|------|------|
| 一 跨进程契约与架构一致性 | 001–018 | 18 |
| 二 配置模型与 INI 持久化 | 019–056 | 38 |
| 三 指标采集与数值口径 | 057–090 | 34 |
| 四 调度器与诊断缓冲 | 091–102 | 12 |
| 五 离屏光栅化与字体 | 103–122 | 20 |
| 六 脏检查、D2D 自愈与渲染线程 | 123–144 | 22 |
| 七 窗口、消息循环与线程安全 | 145–164 | 20 |
| 八 设置面板与控件映射 | 165–192 | 28 |
| 九 动效、防抖、控件数值与安装残留 | 193–200 | 8 |
| 十 引导后端（安装/驱动/命令） | 201–230 | 30 |
| 十一 引导后端续 + 前端桥接向导 | 231–260 | 30 |
| 十二 入口、构建、工程卫生 | 261–280 | 20 |
| 十三 CODE_INDEX / README 校准 | 281–300 | 20 |
| **合计** | **001–300** | **300** |

> 说明：全部 300 条均为对现有代码/文档/配置的局部修正，未引入任何新依赖、新模块体系或新功能；其中 279–300 属于文档与工程卫生层面，与代码修复不冲突，可独立处理。
