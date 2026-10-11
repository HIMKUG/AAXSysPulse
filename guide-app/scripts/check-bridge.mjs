// 构建期桥层契约门禁（审查项 3.2.5 / P1-12 余部）
//
// 为什么需要：前端曾经调用过**根本不存在的命令**，而演示模式把它兜住了 ——
// 于是页面看起来在工作，实际全是本地假数据（报告 P1-12 的机理）。开发态有
// DevTools 时没人去点，打包后 Tauri 直接 reject `Command not found`，用户只
// 看到"点了没反应"。既然桥层是 `bridge.js` 里的字符串字面量、注册表是
// `lib.rs` 里的 `generate_handler![]`，这份对应关系就**必须机器校验**，
// 不能靠人记住。
//
// 用法：`npm run build` 前置执行；退出码非 0 即构建失败。
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const libRs = join(root, 'src-tauri', 'src', 'lib.rs')
const frontSrc = join(root, 'src')

/** 后端注册表：`generate_handler![...]` 里的函数名（去掉注释行）。 */
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
    if (/^[a-z_][a-z0-9_]*$/.test(line)) names.push(line)
  }
  return names
}

/** 前端调用点：`invoke('cmd'` / `invoke("cmd"` 的字符串字面量。 */
function invokedCommands() {
  const hits = []
  const re = /invoke\(\s*['"]([a-z0-9_]+)['"]/g
  for (const file of walk(frontSrc)) {
    if (!/\.(js|ts|vue)$/.test(file)) continue
    const text = readFileSync(file, 'utf8')
    for (const m of text.matchAll(re)) {
      hits.push({ name: m[1], file: file.slice(root.length + 1), line: lineOf(text, m.index) })
    }
  }
  return hits
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

const used = new Set(invoked.map((h) => h.name))
const unused = [...registered].filter((n) => !used.has(n))
if (unused.length) {
  // 悬空命令不是错误（可能被 `--uninstall` 之类的 CLI 路径或外部调用使用），
  // 但值得提醒：P1-11 的 `guide_view_logs` 正是"后端已注册、前端零入口"漏了很久。
  console.warn(`[check-bridge] 提示：${unused.length} 个已注册命令前端无入口： ${unused.join(', ')}`)
}
console.log(
  `[check-bridge] 通过：前端 ${used.size} 个 invoke 全部在后端注册表内（共 ${registered.size} 条命令）`
)
