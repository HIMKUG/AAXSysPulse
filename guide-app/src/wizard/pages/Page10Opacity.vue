<script setup>
import { computed, onUnmounted } from 'vue'
import { state, notifyConfig } from '../composables/useGuide.js'

const fill = computed(() => ({ '--fill': state.opacity + '%' }))

// 续篇 724：挂载时的初值 —— 卸载时对比检测"拖动未松手"的改动并补落盘
const opacityAtMount = state.opacity
onUnmounted(() => {
  // v-model.number 拖动中实时改 state，但落盘只在 @change（松手）——
  // 未松手就翻页/关闭时改动只留内存。卸载前补一次落盘。
  if (state.opacity !== opacityAtMount) notifyConfig({ opacity: state.opacity })
})
</script>

<template>
  <section class="page page--center">
    <div class="page-body">
      <h1 class="t-title r1" style="margin-bottom: 96px">调整透明度</h1>

      <div class="opacity-row r2">
        <!-- 续篇 723：滑条补 aria-label 与 aria-valuetext —— 键盘/读屏知道调的是什么 -->
        <input
          class="slider-fill"
          type="range"
          min="0"
          max="100"
          step="1"
          aria-label="悬浮窗透明度"
          :aria-valuetext="`${state.opacity}%`"
          :style="fill"
          v-model.number="state.opacity"
          @change="notifyConfig({ opacity: state.opacity })"
        />
        <span class="slider-value">{{ state.opacity }}%</span>
      </div>

      <p class="t-mini r3" style="margin-top: var(--sp-6)">0% 完全透明，100% 完全不透明</p>
    </div>
  </section>
</template>

<style scoped>
.opacity-row {
  display: flex;
  align-items: center;
  gap: 18px;
  width: 440px;
}
.opacity-row input { flex: 1; }
.opacity-row .slider-value { min-width: 56px; text-align: right; }
</style>
