<script setup>
import { onMounted } from 'vue'
import { state, pickStyle, ensureStyleApplied, launchMain } from '../composables/useGuide.js'

const STYLES = [
  { key: 'A', label: '极简' },
  { key: 'B', label: '游戏' },
  { key: 'C', label: '宿舍' },
  { key: 'D', label: '开发者' },
  { key: 'E', label: 'GDI兼容' },
  { key: 'F', label: '自定义' }
]

// 预设（监控项/顺序/颜色/字号/字重/透明度/横竖/GDI 开关）由 pickStyle 一次性应用并落盘
const choose = key => pickStyle(key)

// 3.3.3：卡片 A 一进来就显示为选中，但落盘只由点击触发 ⇒ 挂载时把界面上
// 已选中的样式真正应用一次，否则"什么都不选直接继续"= 界面承诺了磁盘上没有的东西。
onMounted(() => {
  ensureStyleApplied()
  launchMain() // 第 4 页加载时启动主程序
})
</script>

<template>
  <section class="page">
    <div class="page-head r1">
      <h1 class="t-title">选择你喜欢的样式</h1>
    </div>

    <div class="page-body">
      <div class="style-grid r2">
        <button
          v-for="s in STYLES"
          :key="s.key"
          class="style-card"
          :class="{ selected: state.style === s.key }"
          :aria-pressed="state.style === s.key ? 'true' : 'false'"
          :title="s.key === 'E' ? 'GDI 引擎：降低约 8 倍显卡占用' : ''"
          @click="choose(s.key)"
        >
          <div class="mini-preview">
            <span class="mini-preview__bar"></span>
            <span class="mini-preview__bar"></span>
            <span class="mini-preview__bar"></span>
          </div>
          <span class="style-card__key">{{ s.key }}</span>
          <span class="style-card__label">
            {{ s.key === 'E' ? 'GDI兼容' : s.label }}
          </span>
        </button>
      </div>
    </div>
  </section>
</template>
