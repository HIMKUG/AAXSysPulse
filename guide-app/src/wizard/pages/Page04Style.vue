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
// 续篇 706：launchMain 串行 await 并接住错误 —— 主程序可能在预设写盘途中读配置。
onMounted(async () => {
  ensureStyleApplied()
  try {
    await launchMain() // 第 4 页加载时启动主程序
  } catch (e) {
    // launchMain 内部已收敛错误并提示；这里兜底不让异常冒泡打断页面
    console.warn('[Page04] launchMain 异常：', e)
  }
})
</script>

<template>
  <section class="page">
    <div class="page-head r1">
      <h1 class="t-title">选择你喜欢的样式</h1>
    </div>

    <div class="page-body">
      <!-- 续篇 707：六张互斥卡片是单选组 —— 补 radiogroup 语义 -->
      <div class="style-grid r2" role="radiogroup" aria-label="悬浮窗样式预设">
        <button
          v-for="s in STYLES"
          :key="s.key"
          class="style-card"
          :class="{ selected: state.style === s.key }"
          role="radio"
          :aria-checked="state.style === s.key ? 'true' : 'false'"
          :aria-describedby="s.key === 'E' ? 'style-e-hint' : undefined"
          @click="choose(s.key)"
        >
          <div class="mini-preview" aria-hidden="true">
            <span class="mini-preview__bar"></span>
            <span class="mini-preview__bar"></span>
            <span class="mini-preview__bar"></span>
          </div>
          <span class="style-card__key">{{ s.key }}</span>
          <span class="style-card__label">{{ s.label }}</span>
        </button>
      </div>
      <!-- 续篇 708：关键说明不再只挂在 title（键盘/触摸/读屏不可达），
           且去掉未复测的"8 倍"数字 -->
      <p id="style-e-hint" class="t-mini" style="margin-top: var(--sp-3)">
        「GDI兼容」启用 GDI 渲染引擎，可显著降低显卡占用。
      </p>
    </div>
  </section>
</template>
