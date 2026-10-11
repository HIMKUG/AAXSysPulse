<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue'
import WizardSheet from './WizardSheet.vue'
import { cmd, onCloseRequested } from './bridge.js'
import {
  state, stepIndex, totalSteps, canPrev, bootstrap,
  diagnostics,
  next, prev, saveConfig, quitGuide, deleteGuideFiles, finishGuide,
  restartAsAdmin, resumeGuide, restartGuide,
  clearState, retryInstall, quitDuringInstall,
  ensureInstallWatch,
  retryDriverInstall, skipDriverInstall,
  chooseReinstall, chooseRepair, chooseReguide,
  askUninstall, runUninstall, cancelUninstall, finishUninstall,
  toasts, notify, dismissToast
} from './composables/useGuide.js'

import Page01Welcome from './pages/Page01Welcome.vue'
import Page02Privilege from './pages/Page02Privilege.vue'
import Page03Install from './pages/Page03Install.vue'
import Page04Style from './pages/Page04Style.vue'
import Page05Orientation from './pages/Page05Orientation.vue'
import Page06Items from './pages/Page06Items.vue'
import Page07Order from './pages/Page07Order.vue'
import Page08NameMode from './pages/Page08NameMode.vue'
import Page09Color from './pages/Page09Color.vue'
import Page10Opacity from './pages/Page10Opacity.vue'
import Page11FontSize from './pages/Page11FontSize.vue'
import Page12Position from './pages/Page12Position.vue'
import Page13Autostart from './pages/Page13Autostart.vue'
import Page14Done from './pages/Page14Done.vue'

const PAGE_MAP = {
  1: Page01Welcome, 2: Page02Privilege, 3: Page03Install, 4: Page04Style,
  5: Page05Orientation, 6: Page06Items, 7: Page07Order, 8: Page08NameMode,
  9: Page09Color, 10: Page10Opacity, 11: Page11FontSize, 12: Page12Position,
  13: Page13Autostart, 14: Page14Done
}
const currentComp = computed(() => PAGE_MAP[state.page])
const dotClass = i =>
  i === stepIndex.value ? 'dot dot--current' : i < stepIndex.value ? 'dot dot--done' : 'dot'

/* ---------------- 底部按钮模式 ---------------- */
const footerMode = computed(() => {
  if (state.page === 14) return 'done'
  // 安装页：推进中无按钮；中止类失败显示「重试 / 退出」
  if (state.page === 3) return state.install.status === 'error' ? 'install-error' : 'install'
  if (state.page === 2) return 'privilege'
  return 'normal'
})

function handleNext() {
  if (state.page === 13 && state.autostart === null) {
    notify('请先选择以后开机是否自动显示。')
    return
  }
  next()
}

/* ---------------- 页脚快捷键（审查项 8B.K 余部 / P2-9）----------------
   只用键盘也要能翻完 14 页：Alt+→ = 「继续」，Alt+← = 「上一步」，
   与页脚两个按钮完全等价（走同一组函数，因此第 13 页未选自启时的
   校验提示也一致）。三条护栏：
   1. 有 sheet 打开时不翻页 —— 面板是模态的，翻页会把面板丢在背后；
   2. 安装页推进中不翻页 —— 此时页脚本就无按钮，`next()` 还会打断安装；
   3. 只接裸 Alt+方向键，带 Ctrl/Meta/Shift 的组合留给浏览器与输入法。 */
function onWizardKeydown(e) {
  if (!e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return
  if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return
  if (document.querySelector('.sheet-scrim')) return
  if (state.page === 3 && state.install.status !== 'error') return
  e.preventDefault()
  if (e.key === 'ArrowRight') handleNext()
  else if (canPrev.value) prev()
}

/* ---------------- 断点续引导 / 第九部分启动分流 ---------------- */
const showResume = ref(false)
const showInstalled = ref(false)
// 安装进度/终态事件在向导存活期常驻订阅（P0-10）：
// 订阅早于任何换页，install-progress / install-done / install-error 不会再
// 因为 Page03 卸载而落在 unlisten 之后。
ensureInstallWatch()
/* Alt+F4 / 任务栏关闭：与自绘 × 走同一条确认路径（P1-9）。
   桥层已 preventDefault，真正的退出由 askClose 之后的 quitGuide / quitAll 完成。 */
let offClose = null
onMounted(async () => {
  window.addEventListener('keydown', onWizardKeydown)
  offClose = await onCloseRequested(() => askClose())
  const resume = await bootstrap()
  if (state.boot.uninstallMode) {
    // --uninstall 启动：直接进入卸载确认
    askUninstall()
  } else if (state.boot.installedDetected) {
    // 已安装：四选一对话框
    showInstalled.value = true
  } else if (resume) {
    showResume.value = true
  }
})

function onInstalledChoice(fn) {
  showInstalled.value = false
  fn()
}
onUnmounted(() => {
  window.removeEventListener('keydown', onWizardKeydown)
  offClose && offClose()
})

async function onResumeYes() {
  showResume.value = false
  await resumeGuide()
}
function onResumeNo() {
  showResume.value = false
  restartGuide()
}

/* ---------------- 关闭确认：两步 ---------------- */
const closeStep = ref(0)
const quitted = ref(false)
/* 这些页关闭时不弹「是否保存」确认框：直接落盘并退出。
   初始(1) / 选择样式(4) / 大功告成(14) 都是「安全」节点，无需询问。 */
const SILENT_CLOSE_PAGES = [1, 4, 14]
/* 安装页关闭：单独确认「安装尚未完成，确定要退出吗？」，退出 = 中止安装并结束进程 */
const showInstallQuit = ref(false)
const askClose = () => {
  // 卸载态集中短路（问题6）：进行/完成一律忽略关闭，杜绝 flushConfig 回写刚删的文件
  if (state.mode === 'uninstall') return
  if (state.page === 3) {
    showInstallQuit.value = true
    return
  }
  if (SILENT_CLOSE_PAGES.includes(state.page)) {
    onSilentQuit()
    return
  }
  closeStep.value = 1
}
async function onInstallQuit() {
  showInstallQuit.value = false
  await quitDuringInstall() // 直接结束进程：安装中止
  quitted.value = true
}
async function onInstallRetry() {
  retryInstall()
}
async function onSilentQuit() {
  await quitGuide() // 内部 flushConfig + flushBackup + 退出进程
  quitted.value = true
}
async function onSave() { await saveConfig(); closeStep.value = 2 }
async function onDontSave() {
  await clearState()       // 不保存：直接清空断点续引导记忆
  closeStep.value = 2
}
const onCancel = () => { closeStep.value = 0 }
async function onQuit() { await quitGuide(); closeStep.value = 0; quitted.value = true }
const onResumeGuide = () => { closeStep.value = 0 }
const reopen = () => { quitted.value = false; restartGuide() }

/* ---------------- 第 14 页 ---------------- */
const showDeleteSheet = ref(false)
async function confirmDeleteGuide() {
  showDeleteSheet.value = false
  await deleteGuideFiles() // 结果由 useGuide 弹 toast（后端不再静默）
}
async function onFinish() {
  await finishGuide()
  quitted.value = true
}

/* ---------------- 卸载结果清单（P0-9） ---------------- */
const UNINSTALL_STATUS = { ok: '已完成', skip: '无需处理', fail: '未完成' }
const uninstallAllOk = computed(() => !state.uninstall.steps.some(s => s.status === 'fail'))

/* ---------------- 日志与桥层诊断（P1-10） ----------------
   capability 被拒、命令未注册这类失败以前只在 console 里，生产包等于没写。
   现在标题栏挂一个入口：有诊断记录时显示红色角标，点开看诊断 + 安装/引导日志。 */
const showLogs = ref(false)
const logText = ref('')
const logPath = ref('')
async function openLogs() {
  showLogs.value = true
  const r = await cmd.viewLogs(400)
  if (r.ok && r.data) {
    logText.value = r.data.content || '（暂无日志）'
    logPath.value = r.data.path || ''
  } else {
    logText.value = `读取日志失败：${r.message || r.code}`
  }
}

/* ---------------- 权限动作 ---------------- */
const onPrivilege = () => restartAsAdmin()
</script>

<template>
  <div class="guide-window">
    <!-- 顶部拖动区 + 日志入口 + 关闭按钮 -->
    <div class="titlebar">
      <button class="win-diag" @click="openLogs">
        日志
        <span v-if="diagnostics.length" class="win-diag__badge">{{ diagnostics.length }}</span>
      </button>
      <button class="win-close" aria-label="关闭" @click="askClose">
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
          <path d="M2 2L12 12M12 2L2 12" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
        </svg>
      </button>
    </div>

    <!-- 页面内容 -->
    <div class="pages" :class="state.dir === 'next' ? 'is-forward' : 'is-backward'">
      <Transition :name="state.dir === 'next' ? 'page-forward' : 'page-backward'">
        <component :is="currentComp" :key="state.page" />
      </Transition>
    </div>

    <!-- 底部导航 -->
    <footer class="footer">
      <div class="dots">
        <span v-for="i in totalSteps" :key="i" :class="dotClass(i - 1)"></span>
      </div>

      <div class="nav-btns">
        <!-- 第 1 页：只有继续，居中 220px -->
        <template v-if="state.page === 1">
          <span class="nav-spacer"></span>
          <button class="btn btn-primary btn-w220" @click="handleNext">继续 →</button>
          <span class="nav-spacer"></span>
        </template>

        <!-- 第 2 页：跳过 140 / 以管理员身份重启 240 -->
        <template v-else-if="footerMode === 'privilege'">
          <button class="btn btn-secondary btn-w140" @click="next">跳过</button>
          <span class="nav-spacer"></span>
          <button class="btn btn-primary btn-w240" @click="onPrivilege">
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none">
              <path d="M8 1.5l5 2.2v4c0 3.1-2.1 5.6-5 6.8-2.9-1.2-5-3.7-5-6.8v-4l5-2.2z"
                    stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" />
            </svg>
            以管理员身份重启
          </button>
        </template>

        <!-- 安装页推进中：无按钮、无上一步 -->
        <template v-else-if="footerMode === 'install'">
          <span class="nav-spacer"></span>
        </template>

        <!-- 安装页中止类失败：重试 / 退出 -->
        <template v-else-if="footerMode === 'install-error'">
          <button class="btn btn-secondary btn-w140" @click="onInstallRetry">重试</button>
          <span class="nav-spacer"></span>
          <button class="btn btn-danger btn-w140" @click="onInstallQuit">退出</button>
        </template>

        <!-- 第 14 页：删除 180 / 完成 220 -->
        <template v-else-if="footerMode === 'done'">
          <button class="btn btn-secondary btn-w180" @click="showDeleteSheet = true">删除引导文件</button>
          <span class="nav-spacer"></span>
          <button class="btn btn-primary btn-w220" @click="onFinish">完成，开始使用</button>
        </template>

        <!-- 其余页：上一步 / 继续 -->
        <template v-else>
          <button v-if="canPrev" class="btn btn-secondary btn-w140" @click="prev">← 上一步</button>
          <span class="nav-spacer"></span>
          <button class="btn btn-primary btn-w140" @click="handleNext">继续 →</button>
        </template>
      </div>
    </footer>

    <!-- 断点续引导 -->
    <WizardSheet :open="showResume" size="s" :closable="false" :scrim-close="false"
      title="上次未完成，是否继续？">
      <p class="sheet-desc">将从第 {{ state.boot.resume?.lastPage }} 页继续，之前的设置会保留。</p>
      <template #footer>
        <button class="btn btn-secondary" @click="onResumeNo">重新设置</button>
        <button class="btn btn-primary" @click="onResumeYes">继续上次</button>
      </template>
    </WizardSheet>

    <!-- 安装页关闭确认：退出 = 中止安装 -->
    <WizardSheet :open="showInstallQuit" size="s" title="安装尚未完成，确定要退出吗？"
      @close="showInstallQuit = false">
      <template #footer>
        <button class="btn btn-secondary" @click="showInstallQuit = false">继续安装</button>
        <button class="btn btn-danger" @click="onInstallQuit">退出</button>
      </template>
    </WizardSheet>

    <!-- 第八部分：安装完成但驱动失败 → 功耗监控不可用（重试从 25% 驱动段 / 跳过） -->
    <WizardSheet :open="state.install.driverPrompt" size="s" :closable="false" :scrim-close="false"
      title="功耗监控不可用">
      <p class="sheet-desc">驱动安装失败，功耗将显示 --。<br />其他功能正常。</p>
      <template #footer>
        <button class="btn btn-secondary" @click="retryDriverInstall">重试</button>
        <button class="btn btn-primary" @click="skipDriverInstall">跳过</button>
      </template>
    </WizardSheet>

    <!-- 第九部分：已安装检测四选一 -->
    <WizardSheet :open="showInstalled" size="s" :closable="false" :scrim-close="false"
      title="检测到已安装 AAXSysPulse">
      <template #footer>
        <div class="installed-choices">
          <button class="btn btn-secondary" @click="onInstalledChoice(chooseReinstall)">重新安装</button>
          <button class="btn btn-secondary" @click="onInstalledChoice(chooseRepair)">修复安装</button>
          <button class="btn btn-secondary" @click="onInstalledChoice(chooseReguide)">重新设置</button>
          <button class="btn btn-danger" @click="onInstalledChoice(askUninstall)">卸载</button>
        </div>
      </template>
    </WizardSheet>

    <!-- 第九部分：卸载确认（--uninstall 启动或四选一「卸载」进入） -->
    <WizardSheet :open="state.uninstall.confirm" size="s" :closable="false" :scrim-close="false"
      title="确定要卸载 AAXSysPulse 吗？">
      <label class="uninstall-keep">
        <input type="checkbox" v-model="state.uninstall.keepConfig" />
        <span>保留配置文件</span>
      </label>
      <template #footer>
        <button class="btn btn-secondary" @click="cancelUninstall">取消</button>
        <button class="btn btn-danger" @click="runUninstall">卸载</button>
      </template>
    </WizardSheet>

    <!-- 第九部分：卸载完成（逐步如实回报，P0-9：以前不论做成什么样都只说"卸载完成"） -->
    <WizardSheet :open="state.uninstall.done" size="s" :closable="false" :scrim-close="false"
      :title="uninstallAllOk ? '卸载完成' : '部分步骤没能完成'">
      <ul class="uninstall-steps">
        <li v-for="s in state.uninstall.steps" :key="s.step" :class="`is-${s.status}`">
          <span class="uninstall-steps__name">{{ s.step }}</span>
          <span class="uninstall-steps__status">{{ UNINSTALL_STATUS[s.status] || s.status }}</span>
          <span v-if="s.detail" class="uninstall-steps__detail">{{ s.detail }}</span>
        </li>
      </ul>
      <p v-if="!uninstallAllOk" class="sheet-desc">
        标「未完成」的项请按上面的路径手动清理；关闭后可从左上角「日志」看到细节。
      </p>
      <template #footer>
        <button class="btn btn-primary" @click="finishUninstall">关闭</button>
      </template>
    </WizardSheet>

    <!-- 关闭确认：两步（× / 点遮罩 = 取消返回引导） -->
    <WizardSheet :open="closeStep > 0" size="s" :title="closeStep === 1 ? '是否保存当前配置？' : '是否退出引导？'"
      @close="onCancel">
      <template #footer>
        <template v-if="closeStep === 1">
          <button class="btn btn-ghost" @click="onCancel">取消</button>
          <button class="btn btn-secondary" @click="onDontSave">不保存</button>
          <button class="btn btn-primary" @click="onSave">保存</button>
        </template>
        <template v-else>
          <button class="btn btn-secondary" @click="onResumeGuide">继续引导</button>
          <button class="btn btn-danger" @click="onQuit">退出</button>
        </template>
      </template>
    </WizardSheet>

    <!-- 演示用：退出后的占位（正式程序这里直接退出进程） -->
    <WizardSheet :open="quitted" size="s" :closable="false" :scrim-close="false"
      title="引导程序已退出（演示）">
      <template #footer>
        <button class="btn btn-primary" @click="reopen">重新打开引导</button>
      </template>
    </WizardSheet>

    <!-- 删除引导文件确认（第 14 页） -->
    <WizardSheet :open="showDeleteSheet" size="s" title="删除引导文件？"
      @close="showDeleteSheet = false">
      <p class="sheet-desc">
        将删除引导程序本身和它的引导进度存档，下次开机时生效，此操作不可恢复。<br />
        主程序的设置不受影响；要整体卸载请在设置里选「卸载」。
      </p>
      <template #footer>
        <button class="btn btn-secondary" @click="showDeleteSheet = false">取消</button>
        <button class="btn btn-danger" @click="confirmDeleteGuide">删除</button>
      </template>
    </WizardSheet>

    <!-- 日志 / 桥层诊断 -->
    <WizardSheet :open="showLogs" size="l" title="运行日志" @close="showLogs = false">
      <template v-if="diagnostics.length">
        <p class="sheet-desc">近期与后端通信的异常：</p>
        <pre class="log-view">{{ diagnostics.join('\n') }}</pre>
      </template>
      <pre class="log-view" style="margin-top: var(--sp-3)">{{ logText }}</pre>
      <p v-if="logPath" class="log-path">{{ logPath }}</p>
      <template #footer>
        <button class="btn btn-primary" @click="showLogs = false">关闭</button>
      </template>
    </WizardSheet>

    <!-- 顶层消息 toast：悬浮最上层，不占排版，不随翻页消失 -->
    <div class="toast-stack" aria-live="polite">
      <TransitionGroup name="toast">
        <div v-for="t in toasts" :key="t.id" class="toast" :class="`toast--${t.type}`">
          <svg class="toast__icon" width="15" height="15" viewBox="0 0 16 16" fill="none">
            <circle cx="8" cy="8" r="7" stroke="currentColor" stroke-width="1.4" />
            <path d="M8 4.6v4.2M8 11.2v.6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
          </svg>
          <span class="toast__text">{{ t.text }}</span>
          <button class="toast__close" aria-label="关闭消息" @click="dismissToast(t.id)">
            <svg width="10" height="10" viewBox="0 0 12 12" fill="none">
              <path d="M1.5 1.5L10.5 10.5M10.5 1.5L1.5 10.5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
            </svg>
          </button>
        </div>
      </TransitionGroup>
    </div>
  </div>
</template>
