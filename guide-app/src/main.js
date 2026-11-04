import { createApp } from 'vue'
import App from './App.vue'

import './styles/tokens.css'
import './styles/base.css'
import './styles/wizard.css'
import './styles/liquid-glass.css'

// 续篇 770：生产包没有 DevTools —— 渲染异常 / 未捕获 Promise 拒绝曾只表现为
// 白屏或无响应。挂全局 errorHandler 并监听 unhandledrejection，至少进 console
// 与可观测的失败路径。
const app = createApp(App)
app.config.errorHandler = (err, _instance, info) => {
  console.error('[guide] 渲染异常：', err, `（${info}）`)
}
if (typeof window !== 'undefined') {
  window.addEventListener('unhandledrejection', (e) => {
    console.error('[guide] 未处理的 Promise 拒绝：', e.reason)
  })
}
app.mount('#app')
