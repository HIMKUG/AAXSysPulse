/* ============================================================
   bridge.js —— 前端与 Rust 后端的唯一对接层（Tauri v2）
   ------------------------------------------------------------
   · 页面与 useGuide.js 一律不直接调后端，全部走这里
   · 命令统一 async，返回 { ok, data?, code?, message? }
   · 无 Tauri 环境时自动降级为演示模式（本地模拟），便于 npm run dev 预览
   ============================================================ */

/* ---------------- Tauri 运行时探测 ---------------- */
let tauriCore = null
let tauriEvent = null

async function ensureTauri() {
  if (tauriCore) return true
  try {
    // 方式一：npm 包（推荐，需安装 @tauri-apps/api）
    tauriCore = await import('@tauri-apps/api/core')
    tauriEvent = await import('@tauri-apps/api/event')
    return true
  } catch {
    try {
      // 方式二：withGlobalTauri（config 里 app.withGlobalTauri = true）
      const g = window.__TAURI__
      if (g?.core?.invoke && g?.event?.listen) {
        tauriCore = g.core
        tauriEvent = g.event
        return true
      }
    } catch { /* ignore */ }
  }
  return false
}

/** 后端桥是否可用（续篇 253：加 typeof window 守卫 —— 非 SSR 安全） */
export const isAvailable = () => {
  if (typeof window === 'undefined') return false
  return !!tauriCore || !!window.__TAURI__?.core
}

/* 只有纯浏览器预览（vite dev）才允许"演示模式"。
   打包后若 Tauri API 因任何原因取不到，必须硬失败：
   演示数据会把整条安装流程演完并显示"安装完成"（报告 P1-12）。
   条件一律写成 `import.meta.env.DEV` 字面量形式，Vite 构建期会把它替换成
   false 并连带删掉演示分支 —— 假数据与模拟安装器不进生产包。 */

/* ---------------- 桥层错误上报 ----------------
   capability 权限被拒、命令未注册、超时等失败原本只会落进 console，
   生产包里没有 DevTools ⇒ 用户看到的就是"点了没反应"。
   这里把这类失败统一交给宿主（useGuide 注册为 toast + 诊断记录）。 */
let bridgeErrorHandler = null
export function setBridgeErrorHandler(fn) { bridgeErrorHandler = fn }
function reportBridgeError(where, detail) {
  console.warn(`[bridge] ${where}:`, detail)
  try { bridgeErrorHandler?.(where, detail) } catch { /* 上报失败不能影响主流程 */ }
}

/* ---------------- 演示模式模拟数据 ---------------- */
const DEMO = {
  bootstrap: {
    has_privilege: false,
    resume: null,
    config_corrupted: false,
    install_path: 'C:\\Users\\Demo\\AppData\\Local\\AAXSysPulse',
    screen: { width: 1920, height: 1080 },
    installed_detected: false,   // 演示：改 true 可预览「检测到已安装」对话框
    uninstall_mode: false        // 演示：改 true 可预览卸载确认流程
  },
  overlay: { x: 834, y: 200, width: 168, height: 76 },
  // 演示数据留 null：与"主程序未运行、取不到真值"时的生产行为一致
  install_info: { install_path: 'C:\\Users\\Demo\\AppData\\Local\\AAXSysPulse', cpu_avg_percent: null, mem_bytes: null }
}

/* ---------------- 写侧闸门（seal） ----------------
   审查项 3.2.9 / 9.11：卸载态下任何"会重建已删之物"的调用都必须被拒。
   以前靠每个函数各自 `if (state.mode === 'uninstall') return` —— 报告实测**另有 12 个
   会写盘/起进程的入口完全没有守卫**（`startInstall` 恰恰在内，9.11 记的"已加守卫"
   是记账错误），当时的唯一屏障只是"页面切不过去 ⇒ 组件不挂载"这一层 UI 可达性。
   漏口的后果是真会咬人的：`run_uninstall` 之后任何一次
   `config_patch` / `guide_finish` 都会把刚删掉的 cfg、桌面快捷方式重新建出来。
   所以守卫下移到**唯一出口层**：这里拦，前端谁忘了判 mode 都拦得住。

   刻意**不在**清单里的三个：
   · `uninstall_run` —— 它就是卸载本身；
   · `app_quit_all` —— 只退进程，实测 `lib.rs:785` 不做任何 flush，不会复活配置；
   · `guide_delete_self` —— 删引导自己的文件，属卸载的收尾动作，且在卸载完成之后才提供。 */
const SEALED_COMMANDS = new Set([
  'config_patch',
  'guide_save_config',
  'guide_backup',
  'guide_reset_state',
  'guide_finish',
  'install_run',
  'main_launch',
  'main_reload_config',
  'overlay_set_position',
  'autostart_set',
  'privilege_restart_as_admin'
])

let sealed = false
let sealReason = ''

/** 进入卸载态时调用：此后所有写类命令在桥层直接拒绝。幂等。 */
export function sealWrites(reason = 'uninstall') {
  if (!sealed) sealReason = reason
  sealed = true
}

/** 受控解除（续篇 253）：卸载失败后必须允许重试 —— 旧实现"幂等却不可解除"，
 *  卸载命令失败后用户既不能重试也不能回引导。 */
export function unsealWrites() {
  sealed = false
  sealReason = ''
}

export const writesSealed = () => sealed

/* ---------------- 底层 invoke ---------------- */

/* 单命令超时上限（ms）。0 = 不设上限。
   install_run 内部含文件复制与服务注册，合法耗时可超 5s。 */
const INVOKE_TIMEOUT_MS = {
  install_run: 0,
  uninstall_run: 0
}
const DEFAULT_INVOKE_TIMEOUT_MS = 5000

async function invoke(cmd, args = {}) {
  // 卸载态的写侧闸门：放在最前面，演示模式与生产模式走同一条判定，
  // 因此前端单测可以直接遍历清单断言被拦（审查项 3.2.9 的验收口径）。
  if (sealed && SEALED_COMMANDS.has(cmd)) {
    const msg = `已处于卸载流程（${sealReason}），拒绝再次写入：${cmd}`
    reportBridgeError('写入已封禁', msg)
    return { ok: false, code: 'SEALED', message: msg }
  }
  const ready = await ensureTauri()
  if (!ready) {
    if (!import.meta.env.DEV) {
      reportBridgeError('后端不可用', `${cmd}：Tauri 桥未加载，拒绝以演示模式继续`)
      return { ok: false, code: 'BRIDGE_UNAVAILABLE', message: '与后端通信不可用' }
    }
    console.debug(`[bridge:demo] ${cmd}`, args)
    return demoInvoke(cmd, args)
  }
  console.debug(`[bridge] >>> ${cmd}`, args)
  // 续篇 745：超时定时器句柄 —— 命令正常返回后必须 clearTimeout，
  // 否则每次 invoke 都留一个最多 5s 的悬挂定时器。
  let timeoutTimer = null
  try {
    // 续篇 744：`cmd in INVOKE_TIMEOUT_MS` 会误命中 Object.prototype 上的名字
    //（constructor/toString）—— 改 Object.hasOwn。
    const limit = Object.hasOwn(INVOKE_TIMEOUT_MS, cmd)
      ? INVOKE_TIMEOUT_MS[cmd]
      : DEFAULT_INVOKE_TIMEOUT_MS
    const call = tauriCore.invoke(cmd, args)
    const data = limit > 0
      ? await Promise.race([
          call,
          new Promise((_, rej) => {
            timeoutTimer = setTimeout(() => {
              // 续篇 252：超时给独立 code —— 旧实现被包成 UNKNOWN。
              rej(Object.assign(new Error(`invoke timeout after ${limit}ms: ${cmd}`), {
                bridgeTimeout: true
              }))
            }, limit)
          })
        ])
      : await call
    console.debug(`[bridge] <<< ${cmd} ok:`, data)
    return { ok: true, data: data ?? null }
  } catch (e) {
    // 续篇 252：Tauri 对 struct 型 GuideError reject 的是**对象** —— 旧实现只处理
    // string / e.message 再 JSON.parse，code/step/hint 全丢成 UNKNOWN。
    let code = 'UNKNOWN'
    let message = ''
    let step = null
    let hint = null
    if (e && e.bridgeTimeout) {
      code = 'INVOKE_TIMEOUT'
      message = e.message
    } else if (typeof e === 'string') {
      message = e
      const parsed = tryParseJson(e)
      if (parsed?.code) {
        code = parsed.code
        message = parsed.message ?? e
        step = parsed.step ?? null
        hint = parsed.hint ?? null
      }
    } else if (e && typeof e === 'object') {
      if (e.code) {
        code = e.code
        message = e.message ?? String(e)
        step = e.step ?? null
        hint = e.hint ?? null
      } else {
        message = e.message ?? String(e)
      }
    } else {
      message = String(e)
    }
    console.warn(`[bridge] !!! ${cmd} failed:`, code, message)
    if (code === 'UNKNOWN') {
      if (/not found|Command/i.test(message)) {
        reportBridgeError('命令未注册', `${cmd} → ${message}`)
      } else if (/not permitted|forbidden|denied|capability/i.test(message)) {
        // 续篇 746：capability 权限拒绝曾被归 UNKNOWN 且不上报 —— 诊断面板零记录。
        reportBridgeError('权限被拒', `${cmd} → ${message}`)
      }
    }
    return { ok: false, code, message, step, hint }
  } finally {
    if (timeoutTimer) clearTimeout(timeoutTimer)
  }
}

function tryParseJson(raw) {
  try {
    return JSON.parse(raw)
  } catch {
    return null
  }
}

function demoInvoke(cmd, args) {
  switch (cmd) {
    case 'guide_bootstrap':
      // 控制台可注入覆盖演示开关：window.__demoBootstrap = { installed_detected: true }
      return { ok: true, data: { ...DEMO.bootstrap, ...(window.__demoBootstrap || {}) } }
    case 'privilege_check': return { ok: true, data: { elevated: false } }
    case 'overlay_get_rect': return { ok: true, data: DEMO.overlay }
    case 'app_get_install_info': return { ok: true, data: DEMO.install_info }
    case 'install_run':
      demoInstallRun(args?.mode)
      return { ok: true, data: { driver_ok: !window.__demoDriverFail } }
    case 'uninstall_run':
      console.debug('[bridge:demo] uninstall_run', args)
      return {
        ok: true,
        data: {
          done: true,
          steps: [
            { step: '关闭主程序', status: 'skip', detail: '主程序未运行' },
            { step: '停止服务', status: 'skip', detail: '服务不存在' },
            { step: '删除服务', status: 'skip', detail: '服务不存在' },
            { step: '删除驱动目录', status: 'skip', detail: '目录不存在' },
            { step: '删除计划任务', status: 'skip', detail: '无同名计划任务' },
            { step: '删除 Run 键', status: 'skip', detail: '未设置开机自启' },
            { step: '删除快捷方式', status: 'skip', detail: '未创建过快捷方式' },
            { step: '删除安装目录', status: 'skip', detail: '目录不存在' },
            { step: '删除卸载注册', status: 'skip', detail: '注册项不存在' }
          ]
        }
      }
    case 'guide_delete_self': return { ok: true, data: '（演示）已安排删除引导程序自身。' }
    case 'guide_view_logs':
      return {
        ok: true,
        data: { path: '%LOCALAPPDATA%\\AAXSysPulse\\logs（演示目录）', lines: args?.lines ?? 400, content: '（演示模式没有真实日志）' }
      }
    /* 续篇 254：演示模式只显式列出已模拟的写类命令 —— 不再用 catch-all
       {ok:true} 掩盖漏实现。 */
    case 'config_patch':
    case 'guide_save_config':
    case 'guide_backup':
    case 'guide_reset_state':
    case 'guide_finish':
    case 'main_launch':
    case 'main_reload_config':
    case 'overlay_set_position':
    case 'autostart_set':
    case 'privilege_restart_as_admin':
      return { ok: true, data: null }
    default:
      // 未列出的命令如实回失败：演示阶段暴露漏实现，而不是静默假成功。
      return { ok: false, code: 'DEMO_NOT_IMPLEMENTED', message: `演示模式未实现命令：${cmd}` }
  }
}

/* 演示模式：按第四部分进度表模拟安装（真实环境由 Rust 后端 emit） */
let demoInstallTimer = null
function demoInstallRun(mode = 'full') {
  clearInterval(demoInstallTimer)
  const allSteps = [
    [5, '创建安装目录'],
    [15, '释放主程序'],
    [25, '释放驱动文件'],
    [35, '停止旧服务'],
    [45, '删除旧服务'],
    [55, '释放驱动到系统目录'],
    [65, '注册系统服务'],
    [75, '启动服务'],
    [85, '创建桌面快捷方式'],
    [90, '创建开始菜单快捷方式'],
    [95, '写入卸载注册'],
    [100, '安装完成']
  ]
  // 与后端 run_install 的模式保持一致：driver-only 只走驱动段；repair 不写注册
  const steps = mode === 'driver-only'
    ? [...allSteps.filter(([p]) => p >= 25 && p <= 75), [100, '安装完成']]
    : mode === 'repair'
      ? allSteps.filter(([p]) => p !== 95)
      : allSteps
  let i = 0
  demoInstallTimer = setInterval(() => {
    if (!steps[i]) {
      clearInterval(demoInstallTimer)
      // 控制台设 window.__demoDriverFail = true 可预览第八部分「功耗监控不可用」分支
      window.__demoEmit?.('install-done', { driver_ok: !window.__demoDriverFail })
      return
    }
    const [pct, text] = steps[i++]
    window.__demoEmit?.('install-progress', { pct, text })
  }, 500)
}

/* ============================================================
   命令（snake_case 对应 Rust #[tauri::command]）
   ============================================================ */
export const cmd = {
  /* ---- 启动 / 生命周期 ---- */
  bootstrap: () => invoke('guide_bootstrap'),
  saveConfig: (config) => invoke('guide_save_config', { config }),
  backup: (page, path, snapshot) => invoke('guide_backup', { page, path, snapshot }),
  resetState: () => invoke('guide_reset_state'),
  quitAll: () => invoke('app_quit_all'),
  // 续篇 253：createShortcut 改可选参数（默认 false）—— 旧实现硬编码 true，
  // 忽略调用方且重复创建快捷方式（安装流程 85%/90% 已建过）。
  finish: (createShortcut = false) => invoke('guide_finish', { createShortcut }),
  deleteSelf: () => invoke('guide_delete_self'),

  /* ---- 权限（第 2 页） ---- */
  privilegeCheck: () => invoke('privilege_check'),
  restartAsAdmin: () => invoke('privilege_restart_as_admin'),

  /* ---- 安装（page 3）/ 卸载（第九部分） ----
     续篇 747：keepConfig / createShortcut 是仅有的多词参数 —— 已核对 Tauri v2
     默认把前端 camelCase 键映射到 Rust snake_case 形参（无 rename_all 时），
     故保持 camelCase；其余命令均为单词形参无需转换。check-bridge.mjs 已把
     「camelCase 键 ↔ snake_case 形参」纳入门禁静态比对。 */
  installRun: (mode = 'full') => invoke('install_run', { mode }),
  uninstallRun: (keepConfig) => invoke('uninstall_run', { keepConfig }),

  /* ---- 主程序 / 配置 ---- */
  mainLaunch: () => invoke('main_launch'),
  configPatch: (patch) => invoke('config_patch', { patch }),
  reloadConfig: () => invoke('main_reload_config'),

  /* ---- 悬浮窗位置（第 12 页） ---- */
  overlayGetRect: () => invoke('overlay_get_rect'),
  overlaySetPos: (x, y) => invoke('overlay_set_position', { x, y }),

  /* ---- 第 13 / 14 页 ---- */
  autostartSet: (enabled) => invoke('autostart_set', { enabled }),
  getInstallInfo: () => invoke('app_get_install_info'),

  /* ---- 恢复引导时读取已有配置 ---- */
  loadConfig: () => invoke('config_load'),

  /* ---- 日志 / 诊断 ---- */
  viewLogs: (lines = 400) => invoke('guide_view_logs', { lines })
}

/* ============================================================
   事件订阅（后端 → 前端）
   同步返回取消订阅函数 —— 确保 listener 注册完成后再返回。
   ============================================================ */
export function on(event, handler) {
  let unlisten = null

  // 同步注册，立即返回 unlisten（listener 已在后台注册好）
  const setup = (async () => {
    const ready = await ensureTauri()
    if (!ready) {
      if (typeof window === 'undefined') return
      if (!import.meta.env.DEV) {
        reportBridgeError('事件订阅失败', `${event}：Tauri 桥未加载`)
        return
      }
      const cb = (e) => e.detail && handler(e.detail)
      window.addEventListener(`guide:${event}`, cb)
      unlisten = () => window.removeEventListener(`guide:${event}`, cb)
    } else {
      // ⚠️ listen 会被 capability 的窗口 label / event 权限拒绝并 reject。
      // 以前这个 rejection 无人处理 ⇒ 事件永久收不到且零提示（安装页停在 0% 的成因）。
      try {
        const un = await tauriEvent.listen(event, (e) => handler(e.payload))
        unlisten = () => typeof un === 'function' && un()
      } catch (e) {
        const detail = typeof e === 'string' ? e : (e?.message ?? String(e))
        reportBridgeError('事件订阅失败', `${event} → ${detail}`)
      }
    }
  })()

  return () => {
    // 取消时先等待 setup 完成，避免注销一个还在注册中的 listener
    setup.then(() => {
      if (unlisten) { unlisten(); unlisten = null }
    }).catch(() => { unlisten = null })
  }
}

/* 演示模式：从控制台手动触发事件（仅 dev；生产包不留注入口）
   window.__demoEmit('install-progress', { pct: 40, text: '正在释放主程序...' }) */
if (import.meta.env.DEV && typeof window !== 'undefined') {
  window.__demoEmit = (event, payload) =>
    window.dispatchEvent(new CustomEvent(`guide:${event}`, { detail: payload }))
}

/* ============================================================
   配置变更：写盘 + 50ms 防抖通知主程序（WM_RELOAD_CONFIG = 0x8008）
   ============================================================ */
let notifyTimer = null

export function patchConfig(patch) {
  // ⚠️ 后端 `config_patch`（`src-tauri/src/lib.rs:301`）是**同步逐个写 4 个 cfg 文件**，
  // 引导侧从来没有"合并待写补丁"的队列（审查项 9.8 说的 `pendingPatch` 在本仓库
  // 任何一处都搜不到；有 pending 合并的是主程序 `win/src/config_io.rs` 的
  // `WRITER_PENDING`，那是另一个进程的另一套代码）。原先这行注释写着
  // "后端需合并异步 I/O"，是把主程序的机制安到了引导头上。
  //
  // 真问题是：`invoke` 返回 Promise 却没人接 ⇒ 写盘失败（CONFIG_WRITE_FAILED：
  // 目录被占用/磁盘满/权限）静默丢失，而 50ms 后照样通知主程序 reload，
  // 用户看到的就是"改了没生效、也没告诉我为什么"。
  cmd.configPatch(patch).then((r) => {
    if (r && !r.ok) {
      reportBridgeError('配置写入失败', `${r.code}：${r.message || '未知原因'}`)
    }
  })

  clearTimeout(notifyTimer)
  notifyTimer = setTimeout(() => {
    notifyTimer = null
    cmd.reloadConfig() // 50ms 内无新变更 → PostMessageW(0x8008)
  }, 50)
}

/** 立即冲刷防抖（离开页面 / 退出前调用，确保消息不丢） */
export function flushConfig() {
  if (notifyTimer) {
    clearTimeout(notifyTimer)
    notifyTimer = null
    cmd.reloadConfig()
  }
}

/* ============================================================
   系统级关闭请求（Alt+F4 / 任务栏「关闭窗口」）
   ------------------------------------------------------------
   窗口的 × 是前端自绘按钮，走 askClose()；但 Alt+F4 不经它。
   以前 `onCloseRequested` 一次都没用过 ⇒ 系统关闭直接销毁窗口：
   「是否保存」确认框不弹、100ms 防抖中的改动不冲刷。
   这里 preventDefault 把关闭请求交回前端，由宿主决定退出路径
   （确认后一律走 app_quit_all，即 Tauri 的 app.exit(0)）。
   ============================================================ */
export async function onCloseRequested(handler) {
  const ready = await ensureTauri()
  if (!ready) return () => {}
  try {
    let getCurrentWindow = null
    try {
      ;({ getCurrentWindow } = await import('@tauri-apps/api/window'))
    } catch {
      getCurrentWindow = window.__TAURI__?.window?.getCurrentWindow ?? null
    }
    if (!getCurrentWindow) {
      reportBridgeError('关闭事件订阅失败', 'onCloseRequested：Tauri window API 不可用')
      return () => {}
    }
    const un = await getCurrentWindow().onCloseRequested((e) => {
      // 阻止默认销毁：交由前端的确认流程 / flushAll 决定何时真正退出
      try { e?.preventDefault?.() } catch { /* 已是默认行为则忽略 */ }
      handler()
    })
    return () => typeof un === 'function' && un()
  } catch (e) {
    const detail = typeof e === 'string' ? e : (e?.message ?? String(e))
    reportBridgeError('关闭事件订阅失败', `onCloseRequested → ${detail}`)
    return () => {}
  }
}
