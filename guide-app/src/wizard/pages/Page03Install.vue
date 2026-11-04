<script setup>
import { computed, onMounted } from 'vue'
import { state, startInstall } from '../composables/useGuide.js'

// 安装页：进入即自动推进；不显示上一步、不保存引导进度、不启动主程序（第十部分）
// 事件订阅不在这里 —— 常驻在 GuideWizard（P0-10），换页不再丢进度/终态事件。
// 续篇 259：不再无条件 startInstall —— 断点恢复 / 换页回来重挂本页时，
// 只有 idle 状态才自动开跑，避免自动重跑安装。
onMounted(() => {
  if (state.install.status === 'idle') startInstall()
})

// 续篇 705：读屏播报按 5% 量化 —— 后端每个 pct 变化（含心跳重发）都更新曾与
// "避免刷屏"的注释不符。
const announcedPct = computed(() => Math.round(state.install.pct / 5) * 5)
</script>

<template>
  <section class="page page--center">
    <div class="page-body install-body">
      <h1 class="install-title">正在安装 AAXSysPulse</h1>

      <!-- 进度条补 progressbar 语义（审查项 8B.K 余部）：旧实现只有 div 宽度，
           读屏拿到的是两个无名元素；数值播报走 aria-valuetext 且按 5% 量化。
           续篇 704：error 态切换 role 并标 aria-busy —— 读屏不再以为仍在安装。 -->
      <div
        class="progress-wrap"
        :role="state.install.status === 'error' ? 'group' : 'progressbar'"
        :aria-busy="state.install.status === 'running'"
        :aria-valuenow="state.install.status === 'error' ? null : state.install.pct"
        aria-valuemin="0"
        aria-valuemax="100"
        :aria-valuetext="`安装进度约 ${announcedPct}%`"
        aria-label="安装进度"
      >
        <div class="progress-track">
          <div class="progress-fill" :style="{ width: state.install.pct + '%' }"></div>
        </div>
        <span class="progress-pct">{{ state.install.pct }}%</span>
      </div>

      <p class="install-step-text">{{ state.install.text }}</p>
      <p v-if="state.install.status === 'error'" class="install-error" role="alert">
        {{ state.install.error }}
      </p>
    </div>
  </section>
</template>
