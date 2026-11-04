// 构建期桥层契约门禁（审查项 3.2.5 / P1-12 余部）
//
// 为什么需要：前端曾经调用过**根本不存在的命令**，而演示模式把它兜住了 ——
// 于是页面看起来在工作，实际全是本地假数据（报告 P1-12 的机理）。开发态有
// DevTools 时没人去点，打包后 Tauri 直接 reject `Command not found`，用户只
// 看到"点了没反应"。既然桥层是 `bridge.js` 里的字符串字面量、注册表是
// `lib.rs` 里的 `generate_handler![]`，这份对应关系就**必须机器校验**，
// 不能靠人记住。
//
// 用法：`npm run build` / `predev` 前置执行（首轮 269：dev 也挂门禁）；
// 退出码非 0 即构建失败。
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const libRs = join(root, 'src-tauri', 'src', 'lib.rs')
const bridgeJs = join(root, 'src', 'wizard', 'bridge.js')
const frontSrc = join(root, 'src')

/** 后端注册表：`generate_handler![...]` 里的函数名（去掉注释行）。
 *  续篇 779：命令名不再只认 snake_case —— Tauri 2 允许 camelCase 命令名，
 *  旧正则会把 camelCase 静默跳过并误报。 */
function registeredCommands() {
  const text = readFileSync(libRs, 'utf8')
  const start = text.indexOf('generate_handler![')
  if (start < 0) fail([`lib.rs 里找不到 generate_handler![ —— 注册表结构变了，请同步本脚本`])
  const body = text.slice(start + 'generate_handler!['.length)
  const end = body.indexOf(']')
  if (end < 0) fail(['generate_handler![ 未闭合'])
  const names = []
  for (const rawLine of body.slice(0, end).split('\n')) {
    const line = rawLine.replace(/\/\/.*$/, '').trim().replace(/,$/, '')
    if (!line) continue
    if (/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(line)) names.push(line)
  }
  return names
}

/** 前端调用点：`invoke('cmd'` / `invoke("cmd"` 的字符串字面量。 */
function invokedCommands() {
  const hits = []
  const re = /invoke\(\s*['"]([A-Za-z0-9_$]+)['"]/g
  for (const file of walk(frontSrc)) {
    if (!/\.(js|ts|vue)$/.test(file)) continue
    const text = readFileSync(file, 'utf8')
    for (const m of text.matchAll(re)) {
      hits.push({ name: m[1], file: file.slice(root.length + 1), line: lineOf(text, m.index) })
    }
  }
  return hits
}

/** 首轮 269：参数名静态比对 —— bridge.js 里传给 invoke 的键（camelCase）
 *  必须能映射到 Rust 形参（snake_case）。多余键 = 后端收不到的拼写错误。 */
function checkInvokeArgNames() {
  const snakeToCamel = (s) => s.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase())
  const text = readFileSync(libRs, 'utf8')
  const params = new Map()
  for (const m of text.matchAll(/fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(([^)]*)\)/g)) {
    const list = []
    for (const raw of m[2].split(',')) {
      const seg = raw.trim()
      const pm = seg.match(/^([a-z_][a-z0-9_]*):/)
      if (pm && pm[1] !== 'app') list.push(pm[1])
    }
    params.set(m[1], list)
  }
  const bridge = readFileSync(bridgeJs, 'utf8')
  const problems = []
  for (const m of bridge.matchAll(/invoke\(\s*['"]([A-Za-z0-9_$]+)['"]\s*,\s*\{([^}]*)\}/gs)) {
    const cmd = m[1]
    const expected = params.get(cmd)
    if (!expected) continue // 命令名一致性由 invokedCommands 把关
    const allowed = new Set(expected.map(snakeToCamel))
    for (const key of m[2].matchAll(/([A-Za-z_$][A-Za-z0-9_$]*)\s*:/g)) {
      if (!allowed.has(key[1])) {
        problems.push(`bridge.js 的 ${cmd}(... { ${key[1]} }) 在后端形参中不存在（后端：${expected.join(', ') || '无'}）`)
      }
    }
  }
  return problems
}

/** 续篇 780：封装死代码检测 —— cmd.* 里定义了却无任何调用点的封装
 *  （如曾经的 privilegeCheck）门禁此前零感知。列出提醒（不算失败）。
 *  扫描范围含 bridge.js 自身（patchConfig 等内部调用也计入调用点）。 */
function unusedWrappers() {
  const bridge = readFileSync(bridgeJs, 'utf8')
  const start = bridge.indexOf('export const cmd')
  if (start < 0) return []
  const defined = [...bridge.slice(start).matchAll(/^\s*([A-Za-z0-9_$]+):\s*\(/gm)].map((m) => m[1])
  const used = new Set()
  const re = new RegExp(`cmd\\.(${defined.join('|')})\\b`, 'g')
  for (const file of walk(frontSrc)) {
    if (!/\.(js|ts|vue)$/.test(file)) continue
    const text = readFileSync(file, 'utf8')
    for (const m of text.matchAll(re)) used.add(m[1])
  }
  return defined.filter((n) => !used.has(n))
}

function* walk(dir) {
  for (const entry of readdirSync(dir)) {
    if (entry === 'node_modules' || entry === 'dist' || entry.startsWith('.')) continue
    const p = join(dir, entry)
    if (statSync(p).isDirectory()) yield* walk(p)
    else yield p
  }
}

function lineOf(text, index) {
  return text.slice(0, index).split('\n').length
}

function fail(problems) {
  console.error('\n[check-bridge] 桥层契约不一致：')
  for (const p of problems) console.error('  ✗ ' + p)
  console.error(
    '\n  修法：前端只调 `bridge.js` 里已封装的 API；确实需要新命令就同时在\n' +
      '  `src-tauri/src/lib.rs` 的 `generate_handler![]` 里注册。\n'
  )
  process.exit(1)
}

const registered = new Set(registeredCommands())
const invoked = invokedCommands()
if (invoked.length === 0) {
  fail([`前端一个 invoke 都没找到 —— 调用写法变了，请同步本脚本（否则门禁形同虚设）`])
}

const unknown = invoked.filter((h) => !registered.has(h.name))
if (unknown.length) {
  fail(unknown.map((h) => `${h.file}:${h.line} 调用了未注册的命令 "${h.name}"`))
}

const argProblems = checkInvokeArgNames()
if (argProblems.length) {
  fail(argProblems)
}

const used = new Set(invoked.map((h) => h.name))
const unused = [...registered].filter((n) => !used.has(n))
if (unused.length) {
  // 悬空命令不是错误（可能被 `--uninstall` 之类的 CLI 路径或外部调用使用），
  // 但值得提醒：P1-11 的 `guide_view_logs` 正是"后端已注册、前端零入口"漏了很久。
  console.warn(`[check-bridge] 提示：${unused.length} 个已注册命令前端无入口： ${unused.join(', ')}`)
}

const deadWrappers = unusedWrappers()
if (deadWrappers.length) {
  // 续篇 780：同理，cmd 封装定义了却无人调用（可能是死封装或为下一页预留）
  console.warn(`[check-bridge] 提示：${deadWrappers.length} 个 cmd 封装前端无调用点： ${deadWrappers.join(', ')}`)
}
console.log(
  `[check-bridge] 通过：前端 ${used.size} 个 invoke 全部在后端注册表内，参数名比对一致（共 ${registered.size} 条命令）`
)
