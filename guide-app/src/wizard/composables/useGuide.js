import { reactive, computed } from 'vue'
import { cmd, on, patchConfig, flushConfig, setBridgeErrorHandler, sealWrites } from '../bridge.js'

/* ============================================================
   引导程序状态与页面流 · v2
   所有后端调用经 bridge.js；无 Tauri 环境自动降级为演示模式
   ============================================================ */

/** 全部可监控项（第 6 页数据源，顺序即展示顺序）
 *  必须与主程序 `types::Metric` 枚举序一致：
 *  cpu(1) mem(2) gpu(3) net(4) power(5) time(6) util(7) disk_read(8) disk_write(9)
 */
export const MONITOR_ITEMS = [
  { key: 'cpu', label: 'CPU 占用' },
  { key: 'mem', label: '内存占用' },
  { key: 'gpu', label: 'GPU 占用' },
  { key: 'net', label: '网络上下行' },
  { key: 'power', label: '功耗' },
  { key: 'time', label: '系统时间' },
  { key: 'util', label: '综合占用' },
  { key: 'disk_read', label: '磁盘读取' },
  { key: 'disk_write', label: '磁盘写入' }
]

/** 预设色块（第 9 页）
 *  与 UI 色板一致：全部降饱和、混入灰调，避免高饱和刺眼。
 *  白/黑也改为 #FFFFFF / #1A1D21，与 --surface-white / --text-primary 对齐。
 */
export const PRESET_COLORS = [
  { name: '白', value: '#FFFFFF' },
  { name: '黑', value: '#1A1D21' },
  { name: '红', value: '#D93025' },
  { name: '橙', value: '#E8A317' },
  { name: '黄', value: '#C8A415' },
  { name: '绿', value: '#34A853' },
  { name: '蓝', value: '#3B7DD8' },
  { name: '紫', value: '#7A6BA8' }
]

/** 两种路径的页面序列：预设 / 自定义。3 = 安装页（自动推进，不保存进度）
 *  2 = 权限说明页：两个序列都不含它 —— 是否接入（或整页删除）属产品决策，见审查报告 8B.B。 */
const PRESET_FLOW = [1, 3, 4, 13, 14]
const CUSTOM_FLOW = [1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
/** 合法页号上界：由两条流程反推，新增页面时不必再改 `goTo` 的校验。 */
const LAST_PAGE = Math.max(...PRESET_FLOW, ...CUSTOM_FLOW)

const defaults = () => ({
  page: 1,
  path: 'preset', // 'preset' | 'custom'
  dir: 'next',
  mode: 'guide',  // 'guide' 正常引导安装 | 'uninstall' 卸载流程

  /* 启动信息 */
  boot: {
    ready: false,
    installPath: '', // 由 guide_bootstrap / app_get_install_info 填真实路径
    screen: { w: 1920, h: 1080 },
    resume: null,          // null | { lastPage, path }
    configCorrupted: false,
    installedDetected: false,  // 第九部分：安装目录已有主程序 → 四选一对话框
    uninstallMode: false       // 第九部分：--uninstall 启动 → 卸载确认
  },

  /* 第九部分：卸载流程（确认 sheet / 完成 sheet） */
  uninstall: {
    confirm: false,
    keepConfig: false,
    running: false,
    done: false,
    steps: []            // 后端逐步回报：{ step, status: ok|skip|fail, detail? }
  },

  /* 第 2 页 */
  hasPrivilege: false,

  /* 进度保存：只在样式选择（page 4 "继续"）之后才保存 */
  lastCheckpoint: 0,      // 上次保存进度的页码，0 表示未到样式选择

  /* 安装页（page 3）：进度由后端 install-progress 事件驱动 */
  install: {
    status: 'idle', // idle | running | done | error
    pct: 0,
    text: '正在准备安装...',
    error: '',
    driverOk: false,        // 驱动相关步骤是否全部成功（第八部分分支用）
    driverSkipped: false,   // 用户跳过驱动 → 功耗不可用
    driverPrompt: false,    // 安装完成但驱动失败 → 「功耗监控不可用」sheet
    pendingMode: 'full',    // 第九部分：四选一对话框带入 'repair'；Page03 挂载时消费
  },

  /* 第 4 页 */
  style: 'A',
  /** 样式预设是否真的落过盘。卡片 A 一挂载就显示为「已选中」，而 `pickStyle`
   *  只由点击触发 ⇒ 什么都不选直接「继续」时，界面承诺的 A 预设从未写盘（3.3.3）。 */
  styleApplied: false,
  gdi: false,
  mainLaunched: false,
  mainFailed: false,

  /* 第 5 页 */
  orientation: 'horizontal',

  /* 第 6 页 */
  selected: {
    cpu: true, mem: true, gpu: true, net: true, power: true,
    time: true, util: false, disk_read: false, disk_write: false
  },

  /* 第 7 页 */
  order: ['cpu', 'mem', 'gpu', 'net', 'power', 'time', 'util', 'disk_read', 'disk_write'],

  /* 第 8 页 */
  nameMode: 'simple',
  perNameMode: null,

  /* 第 9 页 */
  colors: { name: '#FFFFFF', value: '#34A853' },
  perColors: null,

  /* 第 10 页 */
  opacity: 80,

  /* 第 11 页 */
  fontName: 14,
  fontValue: 14,
  perFonts: null,

  /** 字重：主程序口径 0=细 / 1=中 / 2=粗。只由样式预设写入，无独立 UI。 */
  fontWeight: 2,

  /* 第 12 页 */
  pos: { x: 834, y: 200 },
  overlaySize: { w: 168, h: 76 },

  /* 第 13 页 */
  autostart: null,

  /* 第 14 页 */
  stats: { cpu: null, mem: null } // null = 未取到真值，完成页不显示这两张卡
})

export const state = reactive(defaults())

/* ============================================================
   顶层消息 toast：悬浮于窗口最上层，不占排版、不和页面切换挂钩；
   6 秒后自动淡出上浮消失，也可点关闭按钮立即消失。
   ============================================================ */
export const toasts = reactive([])
let toastSeq = 0
export function notify(text, type = 'warn', ttl = 6000) {
  const t = { id: ++toastSeq, text, type }
  toasts.push(t)
  setTimeout(() => dismissToast(t.id), ttl)
}
export function dismissToast(id) {
  const i = toasts.findIndex(t => t.id === id)
  if (i >= 0) toasts.splice(i, 1)   // 离场动画由 <TransitionGroup> 承担
}

/* ============================================================
   桥层诊断：生产包没有 DevTools，console.warn 等于没写。
   把桥层失败（事件订阅被拒 / 命令未注册）收集起来，
   既弹 toast 也留档，供第 14 页「诊断信息」展示。
   ============================================================ */
export const diagnostics = reactive([])

/** 只留档、不弹窗的一条诊断（第 14 页「诊断信息」与日志角标可见）。
 *  内部纠正类信息走这里 —— 弹 toast 会把"其实已自动处理好"的事情说成事故。 */
export function diagNote(line) {
  if (diagnostics[diagnostics.length - 1] === line) return  // 连续重复只留一条
  diagnostics.push(line)
  if (diagnostics.length > 50) diagnostics.shift()
}

setBridgeErrorHandler((where, detail) => {
  diagNote(`${where}：${detail}`)
  notify(`与后端通信异常（${where}），部分功能可能不生效`, 'error', 8000)
})

/* ============================================================
   派生
   ============================================================ */
export const activeFlow = computed(() =>
  state.path === 'custom' ? CUSTOM_FLOW : PRESET_FLOW
)
export const stepIndex = computed(() => activeFlow.value.indexOf(state.page))
export const totalSteps = computed(() => activeFlow.value.length)
export const canPrev = computed(() => stepIndex.value > 0)

/** 功耗是否可用：用户在安装驱动分支点了「跳过」→ 第 6 页禁用功耗项 */
export const powerAvailable = computed(() => !state.install.driverSkipped)

/** 已勾选项（第 7 页排序、第 8/9/11 页子界面数据源） */
export const activeItems = computed(() =>
  state.order
    .filter(k => state.selected[k])
    .map(k => MONITOR_ITEMS.find(i => i.key === k))
    .filter(Boolean)
)

/* ============================================================
   配置序列化 / 水合
   ============================================================ */
export function snapshot() {
  return {
    version: 1,
    style: state.style,
    gdi_mode: state.gdi,
    orientation: state.orientation,
    items: { ...state.selected },
    order: [...state.order],
    name_mode: state.nameMode,
    per_item_name_mode: state.perNameMode ? { ...state.perNameMode } : null,
    colors: { ...state.colors },
    per_item_colors: state.perColors ? JSON.parse(JSON.stringify(state.perColors)) : null,
    opacity: state.opacity,
    font_name: state.fontName,
    font_value: state.fontValue,
    font_weight: state.fontWeight,
    per_item_fonts: state.perFonts ? JSON.parse(JSON.stringify(state.perFonts)) : null
    // 刻意不含 position：坐标唯一权威是主程序 system.cfg（由第 12 页拖动 /
    // cmd.overlaySetPos 直接写入）。若快照带上 state.pos（启动时的硬编码默认值），
    // 退出时的 saveConfig 会把用户拖好的悬浮窗位置回退到默认值。
  }
}

/** 把后端返回的配置灌进 state（恢复引导 / 已有配置时使用） */
export function hydrate(cfg = {}) {
  if (!cfg || typeof cfg !== 'object') return
  if (cfg.style) state.style = cfg.style
  if (typeof cfg.gdi_mode === 'boolean') state.gdi = cfg.gdi_mode
  if (cfg.orientation) state.orientation = cfg.orientation
  if (cfg.items) Object.assign(state.selected, cfg.items)
  if (Array.isArray(cfg.order) && cfg.order.length) state.order = [...cfg.order]
  if (cfg.name_mode) state.nameMode = cfg.name_mode
  if (cfg.per_item_name_mode) state.perNameMode = { ...cfg.per_item_name_mode }
  if (cfg.colors) state.colors = { ...cfg.colors }
  if (cfg.per_item_colors) state.perColors = JSON.parse(JSON.stringify(cfg.per_item_colors))
  if (typeof cfg.opacity === 'number') state.opacity = cfg.opacity
  if (typeof cfg.font_name === 'number') state.fontName = cfg.font_name
  if (typeof cfg.font_value === 'number') state.fontValue = cfg.font_value
  if (typeof cfg.font_weight === 'number') state.fontWeight = cfg.font_weight
  if (cfg.per_item_fonts) state.perFonts = JSON.parse(JSON.stringify(cfg.per_item_fonts))
  if (cfg.position) state.pos = { ...cfg.position }
  // 路径由 style 决定
  state.path = state.style === 'F' ? 'custom' : 'preset'
}

/* ============================================================
   配置变更
   ============================================================ */
export function notifyConfig(patch) {
  if (state.mode === 'uninstall') return
  // 刻意不再兜底成 `snapshot()`：整份快照 15 键里 `per_item_name_mode`/`per_item_colors`
  // 为 null 时后端口径是"全部复位"（config.rs:802/:709）⇒ 一次无参调用就会抹掉用户在
  // 主程序设置页里设过的逐项命名模式与逐项配色（审查项 3.2.1）。落盘必须显式点键。
  if (!patch || typeof patch !== 'object') {
    log.error('notifyConfig 缺少显式 patch，已拒绝整包写入', { patch })
    return
  }
  patchConfig(patch)
  scheduleBackup()
}
export { flushConfig }

/* ============================================================
   防抖档位（审查项 7.1.4）：引导侧其实有**两级串联**的防抖，各自独立，
   不是重复实现 ——
     · `DEBOUNCE.backup = 100`：前端 → 后端的 `guide_backup` IPC 合并窗口
       （本文件 `scheduleBackup`），挡的是"连续拖滑杆/连改颜色 → 几十次 IPC"。
     · `DEBOUNCE_MS = 50`（Rust 侧 `src-tauri/src/notify.rs`）：后端 → 主程序的
       `WM_RELOAD_CONFIG` 投递合并窗口，且是**尾随**防抖（静默 50ms 才发），
       挡的是"一次落盘多写几个文件 → 主程序重复 reload"。
   为什么是这两个量级：100ms 约等于"人手一次连续操作"的间隔，小于它会把用户
   的第二次改动切进两个 IPC；50ms 是"后端连续多次 `save_config`"的合并窗口，
   再短就退化成每笔都发。改任何一个都要同时想另一边：backup 收敛后才会 flush
   配置，两者叠起来的最坏延迟 = 100 + 50 = 150ms。
   ============================================================ */
const DEBOUNCE = { backup: 100 }

/* ============================================================
   断点备份：把「当前页 + 完整设置快照」写入 guide_state.json。
   中途退出后据此**无损**续引导 —— 只从 INI 反推是 lossy 的
   （name_mode / 逐项颜色 / 逐项字号 / 逐项名称模式 无法精确还原）。
   防抖 100ms，避免连续改设置时频繁 IPC。
   ============================================================ */
let backupTimer = null
let backupPending = false
export function scheduleBackup() {
  if (state.mode === 'uninstall') return
  writeMirror() // 同步本地镜像：IPC 是异步的，进程被杀时只有它是确定落下的
  backupPending = true
  clearTimeout(backupTimer)
  backupTimer = setTimeout(() => {
    backupPending = false
    cmd.backup(state.page, state.path, snapshot())
  }, DEBOUNCE.backup)
}
/** 立即冲刷备份（退出 / 保存前调用，确保最后一笔改动落盘） */
export function flushBackup() {
  if (state.mode === 'uninstall') return
  if (!backupPending) return
  clearTimeout(backupTimer)
  backupPending = false
  cmd.backup(state.page, state.path, snapshot())
}

/* ============================================================
   同步落盘兜底：localStorage 镜像（P1-9）
   ------------------------------------------------------------
   断点存档走 IPC（异步）：`beforeunload → flushAll` 里的 invoke 在窗口销毁
   那一刻多半飞不掉，100ms 防抖窗口内的改动就是这么丢的。localStorage 的
   setItem 是**同步**的，且项目此前根本没用过它。
   定位：镜像只是**兜底**，权威仍是后端的 guide_state.json ——
   两者不一致时以后端为准（后端还记着 path/page 的写入序）。
   ============================================================ */
const MIRROR_KEY = 'aaxsp:guide:mirror'

function writeMirror() {
  try {
    localStorage.setItem(MIRROR_KEY, JSON.stringify({
      page: state.page,
      path: state.path,
      style: state.style,
      snapshot: snapshot(),
      ts: Date.now()
    }))
  } catch {
    /* 隐私模式 / 配额满：兜底失效，不阻断引导（后端存档仍然在写） */
  }
}

/** 读出本地镜像；结构不合即视为无镜像（绝不用半截数据 hydrate） */
function readMirror() {
  try {
    const raw = localStorage.getItem(MIRROR_KEY)
    if (!raw) return null
    const m = JSON.parse(raw)
    if (!m || typeof m !== 'object') return null
    if (!m.snapshot || typeof m.snapshot !== 'object') return null
    if (!Number.isInteger(m.page) || m.page < 1 || m.page > 14) return null
    return m
  } catch {
    return null
  }
}

function clearMirror() {
  try { localStorage.removeItem(MIRROR_KEY) } catch { /* 无可清除即已达目的 */ }
}

/** 开关某一监控项，并同步排序数组 */
export function toggleItem(key, on) {
  if (state.mode === 'uninstall') return
  state.selected[key] = on
  if (on && !state.order.includes(key)) {
    const natural = MONITOR_ITEMS.map(i => i.key)
    const last = state.order[state.order.length - 1]
    const at = last ? natural.indexOf(last) + 1 : 0
    state.order.splice(at, 0, key)
  }
  if (!on) {
    const i = state.order.indexOf(key)
    if (i > -1) state.order.splice(i, 1)
  }
  patchConfig({ items: { ...state.selected }, order: [...state.order] })
  scheduleBackup()
}

/* ============================================================
   页面流转
   ------------------------------------------------------------
   activeFlow 是「当前路径有序页」的唯一权威，next/prev 只查它，
   不再另建 TRANSITIONS 表（避免同一事实两处维护而脱同步）。
   所有入口返回布尔：true=已换页，false=被守卫拒绝（调用方可感知）。
   ============================================================ */

/** 极小日志封装：warn/error 生产也打，便于线上排查 */
export const log = {
  info: (...a) => console.info('[guide]', ...a),
  warn: (...a) => console.warn('[guide]', ...a),
  error: (...a) => console.error('[guide]', ...a)
}

/** 状态不变式：DEV 下断言。生产不报错，靠 ensurePageInFlow 主动纠正 */
function assertInvariants() {
  if (!import.meta.env?.DEV) return
  console.assert(activeFlow.value.includes(state.page),
    '[invariant] state.page 不在 activeFlow 内', state.page, activeFlow.value)
  console.assert(state.dir === 'next' || state.dir === 'prev',
    '[invariant] state.dir 非法', state.dir)
}

/** 若当前页脱离当前流程（改路径 / 坏存档后），归位到最近的合法页 */
function ensurePageInFlow() {
  const flow = activeFlow.value
  if (flow.includes(state.page)) return true
  const from = state.page
  const to = flow.includes(4) ? 4 : flow[0]
  log.warn('页号脱离当前流程，已纠正', { from, to, path: state.path })
  // 8.16：console 在生产包里没有 DevTools 可看 ⇒ 归位这件事必须在界面上留痕，
  // 并点名"哪些配置页被跳出流程、它们的值仍留在内存里"（不会自动写盘，
  // 因为 pickStyle 现在只写预设拥有的键 —— 见 PRESET_KEYS 注释）。
  const left = CUSTOM_FLOW.filter(p => !flow.includes(p) && p >= 5 && p <= 12)
  diagNote(`已从第 ${from} 页归位到第 ${to} 页（当前路线不含前者）` +
    (left.length ? `；第 ${left.join('、')} 页不在路线内，其选择不写盘` : ''))
  state.page = to
  return false
}

/** 冲刷所有防抖：配置 + 断点。卸载态下整体短路，绝不回写刚删的文件 */
export function flushAll() {
  if (state.mode === 'uninstall') return
  flushConfig()
  flushBackup()
}

/** 纯查询：下一页是谁，越界返回 null，不动状态 */
export function nextTarget() {
  const flow = activeFlow.value
  const i = flow.indexOf(state.page)
  if (i === -1 || i >= flow.length - 1) return null
  return flow[i + 1]
}
/** 纯查询：上一页是谁；安装页(3)永不作为后退目标，跳过它 */
export function prevTarget() {
  const flow = activeFlow.value
  const i = flow.indexOf(state.page)
  if (i <= 0) return null
  let j = i - 1
  while (flow[j] === 3) j--
  return j >= 0 ? flow[j] : null
}

export function goTo(page, dir = 'next') {
  // P2-15：所有拒绝路径都必须**让用户看见**。以前 `goTo` 返回 false、
  // 而调用方（含 `next()`/`prev()`/程序化跳转）全面忽略返回值 ⇒
  // 点「下一步」什么都不发生且零反馈，用户只能反复点（审查项 3.2.2 / 8.17）。
  if (!Number.isInteger(page) || page < 1 || page > LAST_PAGE) {
    log.error('goTo 收到非法页号，已拒绝', { page })
    notify('内部错误：目标页面不存在，已停在当前页。', 'error')
    return false
  }
  if (state.mode === 'uninstall') return false        // 卸载态：禁换页（界面由卸载清单自己表达）
  if (!['next', 'prev'].includes(dir)) dir = 'next'    // 非法 dir 归一，不崩
  // 安装推进中禁止离开第 3 页：换页会把进度条从视野里撤走，
  // 用户以为「装完了/卡住了」，而 install-done 又会自动 goTo(4) 二次跳转。
  if (state.page === 3 && page !== 3 && state.install.status === 'running') {
    log.warn('安装推进中，已拒绝离开安装页', { target: page })
    notify('安装正在进行中，请等它结束或点「退出安装」。', 'warn')
    return false
  }
  if (!activeFlow.value.includes(page)) {
    log.warn('goTo 目标页不在当前流程，已拒绝', { page, path: state.path, flow: activeFlow.value })
    notify('这一步不在当前引导路线里，已停在当前页。', 'warn')
    return false
  }
  flushConfig() // 离开页面前冲刷防抖，确保配置与消息不丢
  state.dir = dir
  state.page = page
  assertInvariants()
  // 安装页（3）不保存引导进度：断点从样式选择页（4）开始记录
  if (page !== 3) scheduleBackup()
  return true
}
export function next() {
  const p = nextTarget()
  if (p == null) {
    // 到路线末尾：正常不该有按钮可达（第 14 页 footerMode='done'），
    // 但程序化调用与键盘路径都可能撞上来 ⇒ 说明原因，不许静默。
    log.warn('next() 已无下一页', { page: state.page, flow: activeFlow.value })
    notify('已经是最后一步了。', 'warn')
    return false
  }
  return goTo(p, 'next')
}
export function prev() {
  const p = prevTarget()
  if (p == null) {
    log.warn('prev() 已无上一页', { page: state.page })
    notify('已经回到第一步了。', 'warn')
    return false
  }
  return goTo(p, 'prev')
}

/* ============================================================
   悬浮窗样式预设（第 4 页 A–F）
   ------------------------------------------------------------
   ⚠️ 架构决策（2026-09-16）：引导程序**不写**主程序的 `appearance` 字段。
   主程序的 `appearance` =「**设置窗口**主题」（0 跟随系统 / 1 浅色 / 2 深色），
   而 A–F 是「**悬浮窗**样式预设」，两者概念不同。混用会造成：
     · D/E/F → 3/4/5 超出枚举，主程序 `Appearance::from_config` 返回 None ⇒ 静默忽略；
     · 主程序下次保存把它归一化回 0 ⇒ 用户的选择被"吞掉"，重启后退回 A。
   设置页主题由用户自行选择，与引导完全无关。

   预设驱动的字段（已逐一核对主程序
   `sysoverlay/crates/core/src/config/{system,appearance,monitor,layout}.rs`）：

     monitor.cfg     `[item.N] show / color_l / color_v`  ← items / colors
                     `[power] enabled`（N=5 时同步）        ← items.power
     layout_*.cfg    `[layout] order`                      ← order
                     （主程序把显示顺序存在**布局文件**，不在 monitor.cfg）
     appearance.cfg  `[appearance] fg / font_size / font_weight` ← colors.name / font / weight
     system.cfg      `[system] layout`                     ← orientation（横竖）
                     `[system] render_engine`（"0"=Gdi 省显卡 / "1"=D2d 高质量）← gdi
                     `[system] normal_opacity`（**0–100 整数**）← opacity

   与原始设定的差异（一律**以主程序为准**）：
     · 主程序**没有「温度」监控项**（只有 cpu/mem/gpu/net/power/time/util/disk_read/disk_write）
       ⇒ C「宿舍」的"温度"以 **GPU** 代替。
     · 主程序**没有全局数值色**字段 ⇒ 预设的 value 色写入全部 9 项的 `color_v`。
     · `opacity` 是 **0–100 整数**，且属于 **system.cfg**（不在 appearance.cfg）。
     · 主程序**没有等宽 / 像素字体字段** ⇒ D「等宽」只用「小字号 + 细字重」近似。
     · 窗口几何（x/y/w/h）唯一归属 **system.cfg**，布局文件的 `[window]` 段主程序
       已不再解析；位置仍走第 12 页的拖动流程，预设不写死坐标。
   ============================================================ */

/** 全部 9 个监控项 key（顺序 = 主程序 `types::Metric` 枚举序） */
export const ALL_METRIC_KEYS = MONITOR_ITEMS.map(i => i.key)

/** 预设表。`items` 数组顺序即写入 layout 的显示顺序。 */
export const PRESETS = {
  // 极简：CPU / 内存 / 时间，中性色，小字号，横版
  A: {
    items: ['cpu', 'mem', 'time'],
    font: 14, weight: 0,
    name: '#FFFFFF', value: '#E6E6E6',
    opacity: 85, orientation: 'horizontal', gdi: false
  },
  // 游戏：全部监控项，高对比色，中字号，横版
  B: {
    items: [...ALL_METRIC_KEYS],
    font: 16, weight: 2,
    name: '#FFFFFF', value: '#00E676',
    opacity: 90, orientation: 'horizontal', gdi: false
  },
  // 宿舍：功耗 / GPU（代替主程序不存在的"温度"）/ CPU / 内存，柔和色，竖版
  C: {
    items: ['power', 'gpu', 'cpu', 'mem'],
    font: 16, weight: 1,
    name: '#B0BEC5', value: '#80CBC4',
    opacity: 85, orientation: 'vertical', gdi: false
  },
  // 开发者：CPU / 内存 / 网络 / 时间，小字号细字重（主程序无等宽字段），横版
  D: {
    items: ['cpu', 'mem', 'net', 'time'],
    font: 13, weight: 0,
    name: '#E0E0E0', value: '#9CCC65',
    opacity: 80, orientation: 'horizontal', gdi: false
  },
  // GDI 兼容：全部监控项，启用 GDI 引擎（显著降低显卡占用），横版
  E: {
    items: [...ALL_METRIC_KEYS],
    font: 14, weight: 0,
    name: '#FFFFFF', value: '#FFFFFF',
    opacity: 90, orientation: 'horizontal', gdi: true
  }
}

/** 样式预设**拥有**的键：`pickStyle` 只下发这些。
 *
 *  刻意不含 `name_mode` / `per_item_name_mode`（第 8 页负责）与 `version`/`style`
 *  （主程序无对应字段）—— 整份 `snapshot()` 里的 `per_item_name_mode: null`
 *  在 Rust 侧的语义是"把全部逐项命名模式复位为跟随"，选一次样式就抹掉用户在
 *  主程序设置里设过的逐项命名模式（审查项 3.2.1 / P1-8）。 */
const PRESET_KEYS = [
  'gdi_mode', 'orientation', 'items', 'order', 'colors', 'per_item_colors',
  'opacity', 'font_name', 'font_value', 'font_weight', 'per_item_fonts'
]

function presetPatch() {
  const all = snapshot()
  return Object.fromEntries(PRESET_KEYS.map(k => [k, all[k]]))
}

/** 第 4 页选样式：把预设应用到 state，并落盘。
 *  `F` = 自定义：不预设，只切到完整自定义流程。 */
export function pickStyle(key) {
  if (state.mode === 'uninstall') return
  state.style = key
  state.path = key === 'F' ? 'custom' : 'preset'

  const p = PRESETS[key]
  if (p) {
    // 监控项 + 显示顺序
    state.selected = Object.fromEntries(ALL_METRIC_KEYS.map(k => [k, p.items.includes(k)]))
    state.order = [...p.items]
    // 驱动已跳过 → 功耗不可用：预设即便含 power 也不能把它勾回来
    if (state.install.driverSkipped) {
      state.selected.power = false
      state.order = state.order.filter(k => k !== 'power')
    }
    // 颜色：统一设置 ⇒ 复位逐项覆盖（与第 9 页「统一设置」语义一致）
    state.colors = { name: p.name, value: p.value }
    state.perColors = null
    // 字号 / 字重：同样复位逐项覆盖
    state.fontName = p.font
    state.fontValue = p.font
    state.fontWeight = p.weight
    state.perFonts = null
    // 透明度（前端本就是 0–100，直接对应主程序 normal_opacity）
    state.opacity = p.opacity
    // 布局方向 + 渲染引擎
    state.orientation = p.orientation
    state.gdi = p.gdi
  }

  // 预设拥有的键落盘（`style` 本身不落盘，主程序没有对应字段）。
  // presetPatch() 从 snapshot() 里挑键，position 从来不在 snapshot 里 ——
  // 坐标由第 12 页的拖动流程负责，预设不搬窗。
  //
  // F（自定义）**没有预设**：旧写法仍然 `patchConfig(snapshot())`，等于把前端默认值
  // （gdi_mode:false / font_weight:2 / opacity:80 / colors / 全 9 项 items）整包写进
  // 磁盘 —— 用户只是点了一下「自定义」，主程序里已设的渲染引擎与字重就被改掉（3.3.3）。
  // 现在 F 一个键都不写：第 5–11 页各自负责自己的键。
  if (p) {
    patchConfig(presetPatch())
    state.styleApplied = true
  } else {
    log.info('F = 自定义：不写任何配置，等第 5–11 页各自落盘', { key })
    state.styleApplied = false
  }
  scheduleBackup()
  // 改路径后当前页可能脱离新流程（问题5）：归位到合法页，绝不留旧道页码
  ensurePageInFlow()
}

/** 进入第 4 页时把"界面上已经选中的样式"真正应用一次（审查项 3.3.3 / P1-8）。
 *
 *  卡片 A 从一开始就带 `selected` 样式，但 `pickStyle` 只由点击触发 ⇒
 *  什么都不选直接点「继续」的用户，`state.style` 是 'A' 而磁盘上仍是
 *  `defaults()` 那套（6 项 / opacity 80 / 与 A 预设的 3 项 / 85 不同），
 *  即"界面承诺了磁盘上没有的东西"。走与点选完全相同的代码路径，只是不弹提示。 */
export function ensureStyleApplied() {
  if (state.page !== 4 || state.styleApplied) return
  if (state.style === 'F' || !PRESETS[state.style]) return
  pickStyle(state.style)
}

/* ============================================================
   启动引导（App.vue onMounted 调用）
   ============================================================ */
export async function bootstrap() {
  const r = await cmd.bootstrap()
  if (!r.ok) {
    // 后端状态读不出来：先看本地同步镜像有没有可恢复的进度（P1-9 兜底）
    const m = readMirror()
    if (m) {
      state.boot.resume = {
        lastPage: m.page,
        path: m.path === 'custom' ? 'custom' : 'preset',
        style: m.style ?? '',
        snapshot: m.snapshot
      }
      state.boot.ready = true
      notify('无法读取程序状态，已从本地备份恢复引导进度。', 'warn', 8000)
      return state.boot.resume
    }
    notify('无法读取程序状态，将使用默认配置继续。', 'error')
    state.boot.ready = true
    return null
  }
  const d = r.data ?? {}
  state.hasPrivilege = !!d.has_privilege
  state.boot.installPath = d.install_path ?? state.boot.installPath
  if (d.screen) state.boot.screen = { w: d.screen.width, h: d.screen.height }
  state.boot.configCorrupted = !!d.config_corrupted
  // 存档容错（防御#6）：逐字段校验，坏字段降级为「无断点」，绝不崩
  const rs = d.resume
  const validResume = rs
    && Number.isInteger(rs.last_page) && rs.last_page >= 1 && rs.last_page <= 14
    && (rs.path === 'preset' || rs.path === 'custom')
  if (rs && !validResume) log.warn('断点存档非法，忽略续引导', rs)
  state.boot.resume = validResume
    ? {
        lastPage: rs.last_page,
        path: rs.path,
        style: rs.style ?? '',
        snapshot: rs.snapshot && typeof rs.snapshot === 'object' ? rs.snapshot : null,
      }
    : null
  state.boot.ready = true

  // 后端存档没有快照（写快照那笔 IPC 恰好没落地）→ 用本地同步镜像补齐。
  // 页号一致才敢补：对不上说明两边不是同一时刻，以后端为准。
  if (state.boot.resume && !state.boot.resume.snapshot) {
    const m = readMirror()
    if (m && m.page === state.boot.resume.lastPage) {
      state.boot.resume.snapshot = m.snapshot
      if (!state.boot.resume.style && m.style) state.boot.resume.style = m.style
    }
  }

  if (state.boot.configCorrupted) {
    notify('配置文件已损坏，已恢复为默认设置。')
  }
  state.boot.installedDetected = !!d.installed_detected
  state.boot.uninstallMode = !!d.uninstall_mode
  // --uninstall 启动：全程进入卸载态，goTo/落盘一律短路（问题6）
  if (state.boot.uninstallMode) state.mode = 'uninstall'
  return state.boot.resume
}

/** 用户选择「继续上次」 → 恢复**完整快照**（无损）并跳转 */
export async function resumeGuide() {
  const res = state.boot.resume
  if (res?.snapshot && typeof res.snapshot === 'object') {
    // 无损恢复：直接用引导自己备份的完整快照
    // （含 name_mode / 逐项颜色 / 逐项字号 / 逐项名称模式 —— INI 反推拿不到）
    hydrate(res.snapshot)
  } else {
    // 兼容旧状态文件（无快照）：退回从 INI 反推（有损，缺失项回落默认）
    const r = await cmd.loadConfig()
    if (r.ok && r.data) hydrate(r.data)
  }
  if (res) {
    if (res.style) state.style = res.style
    if (res.path) state.path = res.path
    // goTo 现会校验目标页在当前流程内；坏存档导致跳不进时，归位而非卡死。
    // ⚠️ 兜底那句 `state.page = …` 是**直接赋值、绕过 goTo**（审查项 3.2.9 的
    // "resumeGuide 绕过 mode 守卫"），所以卸载态下必须先挡住，否则坏存档会把
    // 用户从卸载清单扔回某个配置页，那条页面上的写盘按钮就又能点了。
    if (state.mode !== 'uninstall' && !goTo(res.lastPage, 'next')) {
      state.page = res.lastPage
      ensurePageInFlow()
      assertInvariants()
      scheduleBackup()
    }
  }
  state.boot.resume = null
}

/** 清空断点续引导记忆（后端 guide_state.json + 本地镜像）。
 *  旧写法 `try { await cmd.resetState() } catch {}` 是死代码：`cmd.*` 从不抛错
 *  （失败以 `{ ok:false }` 返回），catch 永不触发 ⇒ 清空失败静默吞掉，
 *  用户点了「不保存退出」，下次启动照样被问「是否继续上次」（报告 8.19）。 */
export async function clearState() {
  clearMirror()
  const r = await cmd.resetState()
  if (!r.ok) {
    log.warn('清空引导进度存档失败', r.code, r.message)
    notify('未能清除引导进度存档，下次启动可能仍会提示继续上次。', 'warn')
  }
}

/** 用户选择「重新引导」：先清空记忆状态，再重置内存中的向导进度 */
export async function restartGuide() {
  await clearState()
  Object.assign(state, defaults())
  state.boot.ready = true
}

/* ============================================================
   第 2 页：权限
   ============================================================ */
export async function restartAsAdmin() {
  await cmd.restartAsAdmin() // 进程重启，通常不返回
}

/* ============================================================
   安装流程（page 3）：后端 run_install 自动推进，
   进度与终态由 install-progress / install-done / install-error 事件驱动
   ============================================================ */
async function runInstallNow(mode = 'full') {
  state.install.status = 'running'
  state.install.pct = 0
  state.install.text = '正在准备安装...'
  state.install.error = ''
  const r = await cmd.installRun(mode)
  // 事件丢失兜底：命令返回值把状态收敛到终态，避免界面卡死
  if (!r.ok && state.install.status === 'running') {
    state.install.status = 'error'
    state.install.error = r.message || '安装失败'
  }
}

export function startInstall() {
  // 卸载态绝不重跑安装：`run_uninstall` 之后重启安装会把刚删的目录/服务/注册表项
  // 原样装回来。桥层的 SEALED 闸门（bridge.js）已经会拒绝 `install_run`，这里是
  // 第二层，且它负责"静默返回"而不是"弹一条封禁提示"。
  if (state.mode === 'uninstall') return
  if (state.install.status === 'running') return
  // 已装好又回退到本流程：不重跑安装，直接放行到样式页（幂等，问题1）
  if (state.install.status === 'done') { goTo(4, 'next'); return }
  const mode = state.install.pendingMode || 'full'
  state.install.pendingMode = 'full'
  runInstallNow(mode)
}

/** 中止类失败（创建目录 / 释放主程序）后的重试：从头再跑 */
export function retryInstall() {
  startInstall()
}

/** 安装页关闭 → 用户确认退出：直接终止进程（中止安装） */
export async function quitDuringInstall() {
  await cmd.quitAll()
}

/** 第八部分：驱动段失败后的「重试」→ 从 25% 只重跑驱动部分 */
export async function retryDriverInstall() {
  if (state.install.status === 'running') return
  state.install.driverPrompt = false
  state.install.status = 'running'
  state.install.pct = 25
  state.install.text = '正在重试驱动安装...'
  state.install.error = ''
  const r = await cmd.installRun('driver-only')
  if (!r.ok && state.install.status === 'running') {
    state.install.status = 'error'
    state.install.error = r.message || '驱动安装失败'
  }
}

/** 第八部分：驱动失败后「跳过」→ 摘掉功耗项，进样式选择页 */
export function skipDriverInstall() {
  state.install.driverPrompt = false
  state.install.driverSkipped = true
  if (state.selected.power) toggleItem('power', false)
  goTo(4, 'next')
}

/* ============================================================
   第九部分：已安装检测四选一 + 卸载
   ============================================================ */
/** 重置安装页状态，使 startInstall 认定「需要跑」而非「已完成放行」 */
function resetInstallState(mode) {
  Object.assign(state.install, {
    status: 'idle', pct: 0, text: '正在准备安装...', error: '',
    driverOk: false, driverSkipped: false, driverPrompt: false
  })
  state.install.pendingMode = mode
}
/** 四选一：重新安装 → 完整安装流程（Page03 挂载即自动开跑） */
export function chooseReinstall() {
  resetInstallState('full')
  goTo(3, 'next')
}
/** 四选一：修复安装 → 重放主程序 + 驱动 + 快捷方式，不覆盖配置与注册 */
export function chooseRepair() {
  resetInstallState('repair')
  goTo(3, 'next')
}
/** 四选一：重新引导 → 跳过安装，直接从样式选择页开始 */
export function chooseReguide() {
  goTo(4, 'next')
}
/** 四选一 / --uninstall：弹出卸载确认 sheet */
export function askUninstall() {
  state.uninstall.confirm = true
}
/** 卸载确认 → 执行后端卸载，逐步结果如实回报（P0-9） */
export async function runUninstall() {
  if (state.uninstall.running) return
  state.uninstall.confirm = false
  state.uninstall.running = true
  // 进入卸载态：goTo / notifyConfig / scheduleBackup / flushAll 全部短路，
  // 杜绝卸载中途任何换页或配置回写（问题6）
  state.mode = 'uninstall'
  // 并在桥层封死所有写类命令（审查项 3.2.9）：上面那批散落的 mode 判断挡不住
  // 12 个没守卫的入口，`config_patch`/`guide_finish` 一旦被调到就是"卸载完又
  // 冒出一个桌面快捷方式"。封禁必须在 `uninstallRun` 之前生效。
  sealWrites('用户发起卸载')
  const r = await cmd.uninstallRun(state.uninstall.keepConfig)
  state.uninstall.running = false
  if (!r.ok) {
    // 命令整体失败（互斥/线程 panic）：列不出清单，只能给原因
    state.uninstall.steps = [{ step: '卸载', status: 'fail', detail: r.message || r.code }]
  } else {
    state.uninstall.steps = Array.isArray(r.data?.steps) ? r.data.steps : []
  }
  state.uninstall.done = true
}
/** 取消卸载：--uninstall 启动模式下无事可做，直接退出进程 */
export async function cancelUninstall() {
  if (state.boot.uninstallMode) {
    await quitDuringInstall()
  } else {
    state.uninstall.confirm = false
  }
}
/** 「卸载完成」→ 退出引导进程 */
export async function finishUninstall() {
  await cmd.quitAll()
}

function watchInstall() {
  const offs = [
    on('install-progress', p => {
      if (!p) return
      // 事件层常驻（不在页面组件里订阅）：pct 强制 clamp 到 0–100。
      // 不再按 state.page 丢弃迟到事件 —— 安装进度是全局状态，
      // 与「当前是否正看着第 3 页」无关，页面只负责渲染它。
      state.install.pct = Math.max(0, Math.min(100, Number(p.pct) || 0))
      if (typeof p.text === 'string') state.install.text = p.text
    }),
    on('install-done', p => {
      if (state.install.status === 'done') return   // 重复 done 不二次跳转
      state.install.status = 'done'
      state.install.pct = 100
      state.install.text = '安装完成'
      state.install.driverOk = !!(p && p.driver_ok)
      if (state.install.driverOk) {
        // 驱动成功 → 自动进入样式选择页
        setTimeout(() => goTo(4, 'next'), 500)
      } else {
        // 驱动失败 → 「功耗监控不可用」sheet（重试 / 跳过）
        state.install.driverPrompt = true
      }
    }),
    on('install-error', p => {
      state.install.status = 'error'
      state.install.error = (p && p.message) || '安装失败'
    })
  ]
  return () => offs.forEach(f => f && f())
}

/** 常驻订阅：整个进程只订一次、永不 unlisten。
 *  以前订阅挂在 Page03Install 的 onMounted/onUnmounted 上，
 *  换页（或驱动失败弹 sheet）时 unlisten 与后端 emit 竞争 ⇒
 *  install-done / install-error 可能整个丢掉，界面永久卡在推进中。
 *  由 GuideWizard 在 setup 阶段调用一次。
 */
let installWatched = false
export function ensureInstallWatch() {
  if (installWatched) return
  installWatched = true
  watchInstall()
}

/* ============================================================
   第 4 页：启动主程序
   ============================================================ */
export async function launchMain() {
  if (state.mainLaunched) return
  const r = await cmd.mainLaunch()
  state.mainLaunched = true
  if (!r.ok) {
    state.mainFailed = true
    notify('主程序启动失败，请手动启动后再继续。配置仍可正常调整。')
  }
}

/* ============================================================
   第 12 页：悬浮窗位置
   ============================================================ */
export async function fetchOverlayRect() {
  const r = await cmd.overlayGetRect()
  if (r.ok && r.data) {
    state.pos = { x: r.data.x, y: r.data.y }
    state.overlaySize = { w: r.data.width, h: r.data.height }
  }
}
export async function setOverlayPos(x, y) {
  state.pos = { x, y }
  await cmd.overlaySetPos(x, y)
  cmd.reloadConfig()
}

/* ============================================================
   第 13 页：开机自启
   ============================================================ */
export async function setAutostart(v) {
  state.autostart = v
  const r = await cmd.autostartSet(v)
  if (!r.ok) {
    notify('开机自启设置写入失败（可能是权限不足），可在设置中重试。')
  }
}

/* ============================================================
   第 14 页：完成
   ============================================================ */
export async function fetchInstallInfo() {
  const r = await cmd.getInstallInfo()
  if (r.ok && r.data) {
    state.boot.installPath = r.data.install_path ?? state.boot.installPath
    // 后端取不到真值时返回 null（主程序未运行 / 句柄权限不足），
    // 完成页据此隐藏对应卡片 —— 不再显示与本机无关的 0.03% / 1MB。
    const cpu = r.data.cpu_avg_percent
    const mem = r.data.mem_bytes
    state.stats.cpu = typeof cpu === 'number' ? `${cpu.toFixed(1)}%` : null
    state.stats.mem = typeof mem === 'number' && mem > 0 ? `${Math.round(mem / 1048576)}MB` : null
  }
}

/* ============================================================
   关闭 / 退出
   ============================================================ */
export async function saveConfig() {
  flushAll()
  await cmd.saveConfig(snapshot())
}
export async function quitGuide() {
  flushAll() // 退出前落盘「页 + 设置」断点备份，确保下次可续引导
  await cmd.quitAll()
}
export async function deleteGuideFiles() {
  const r = await cmd.deleteSelf()
  // 后端以前用 .ok() 丢掉结果 ⇒ 点了没反应也没人说明（报告 3.1.12）。
  // 现在成功/失败都必须在界面上落一句话。
  if (r.ok) {
    notify(typeof r.data === 'string' ? r.data : '引导程序已安排删除。', 'info', 8000)
  } else {
    notify(`删除引导文件失败：${r.message || r.code}${r.hint ? `｜${r.hint}` : ''}`, 'error', 10000)
  }
}
export async function finishGuide() {
  flushAll() // 先落盘，再由后端 guide_finish 把 incomplete 置回 false
  clearMirror() // 引导已收尾：兜底镜像不该在下次启动再冒出来
  await cmd.finish()
}

/* ============================================================
   兜底 flush（防御#7 / 问题4）
   beforeunload 时把防抖中的配置与断点立即冲刷。跨 IPC 属 best-effort：
   invoke 会在卸载前发起，绝大多数情况可达；关键节点仍另有主动 flushAll。
   ============================================================ */
if (typeof window !== 'undefined') {
  window.addEventListener('beforeunload', () => flushAll())
}
