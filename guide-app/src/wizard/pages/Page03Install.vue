<script setup>
import { onMounted } from 'vue'
import { state, startInstall } from '../composables/useGuide.js'

// 安装页：进入即自动推进；不显示上一步、不保存引导进度、不启动主程序（第十部分）
// 事件订阅不在这里 —— 常驻在 GuideWizard（P0-10），换页不再丢进度/终态事件。
onMounted(() => startInstall())
</script>

<template>
  <section class="page page--center">
    <div class="page-body install-body">
      <h1 class="install-title">正在安装 AAXSysPulse</h1>

      <!-- 进度条补 progressbar 语义（审查项 8B.K 余部）：旧实现只有 div 宽度，
           读屏拿到的是两个无名元素；数值播报走 aria-valuetext，避免每 5% 刷屏。 -->
      <div
        class="progress-wrap"
        role="progressbar"
        :aria-valuenow="state.install.pct"
        aria-valuemin="0"
        aria-valuemax="100"
        :aria-valuetext="`安装进度 ${state.install.pct}%`"
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
