<script setup>
import { state, notifyConfig } from '../composables/useGuide.js'

const choose = v => {
  state.orientation = v
  notifyConfig({ orientation: v }) // 主程序悬浮窗实时切换横竖
}
</script>

<template>
  <section class="page">
    <div class="page-head r1">
      <h1 class="t-title">选择布局方向</h1>
    </div>

    <div class="page-body">
      <div class="orient-list r2">
        <button
          class="orient-card"
          :class="{ selected: state.orientation === 'horizontal' }"
          :aria-pressed="state.orientation === 'horizontal' ? 'true' : 'false'"
          @click="choose('horizontal')"
        >
          <div class="mini-preview">
            <span class="mini-preview__bar"></span>
            <span class="mini-preview__bar"></span>
            <span class="mini-preview__bar"></span>
          </div>
          <div>
            <div class="orient-card__title">横版</div>
            <div class="orient-card__desc">悬浮窗横向排列，适合宽屏</div>
          </div>
        </button>

        <button
          class="orient-card"
          :class="{ selected: state.orientation === 'vertical' }"
          :aria-pressed="state.orientation === 'vertical' ? 'true' : 'false'"
          @click="choose('vertical')"
        >
          <div class="mini-preview mini-preview--v">
            <span class="mini-preview__bar"></span>
            <span class="mini-preview__bar"></span>
            <span class="mini-preview__bar"></span>
          </div>
          <div>
            <div class="orient-card__title">竖版</div>
            <div class="orient-card__desc">悬浮窗纵向排列，适合侧边</div>
          </div>
        </button>
      </div>
    </div>
  </section>
</template>
