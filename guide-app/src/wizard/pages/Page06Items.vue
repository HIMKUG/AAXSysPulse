<script setup>
import { MONITOR_ITEMS, state, toggleItem, powerAvailable } from '../composables/useGuide.js'

const picked = k => !!state.selected[k]
const disabled = it => it.key === 'power' && !powerAvailable.value
</script>

<template>
  <section class="page">
    <div class="page-head r1">
      <h1 class="t-title">选择要显示的监控项</h1>
    </div>

    <div class="page-body">
      <div class="item-list r2">
        <!-- 键盘可达性（审查项 8B.K / P2-9）：这是全项目唯一一处 div 型可点卡片，
             改成 <button> 会引入 UA 样式（边框/背景/文本居中）与自定义 hover/active
             打架，所以保留 div 视觉、补 tabindex + role + aria-checked + Space/Enter。 -->
        <div
          v-for="it in MONITOR_ITEMS"
          :key="it.key"
          class="check"
          :class="{ checked: picked(it.key), disabled: disabled(it) }"
          role="checkbox"
          :aria-checked="picked(it.key) ? 'true' : 'false'"
          :tabindex="disabled(it) ? -1 : 0"
          @click="!disabled(it) && toggleItem(it.key, !picked(it.key))"
          @keydown.space.prevent="!disabled(it) && toggleItem(it.key, !picked(it.key))"
          @keydown.enter.prevent="!disabled(it) && toggleItem(it.key, !picked(it.key))"
        >
          <span class="check-box">
            <svg width="13" height="13" viewBox="0 0 12 12" fill="none">
              <path d="M2.5 6.2L4.8 8.5L9.5 3.8" stroke="#fff" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </span>
          <span class="check-label">{{ it.label }}</span>
          <span v-if="disabled(it)" class="check-tag">驱动未安装</span>
        </div>
      </div>
    </div>
  </section>
</template>
