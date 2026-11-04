<script setup>
import { computed, ref } from 'vue'
import WizardSheet from '../WizardSheet.vue'
import { state, activeItems, notifyConfig } from '../composables/useGuide.js'

const showSheet = ref(false)
const items = computed(() => activeItems.value)

// 续篇 725：字号区间提为常量共用（min/max、填充计算曾三处重复 8/24、8/32）
const FONT_MIN = 8
const FONT_MAX = 32
const clampFill = v => Math.max(0, Math.min(100, ((v - FONT_MIN) / (FONT_MAX - FONT_MIN)) * 100))

// 全局字号：主程序以单一 font_size 同时作用于「名称」与「数值」，故二者统一调整
const globalFill = computed(() => ({ '--fill': clampFill(state.fontName) + '%' }))

function onGlobalFont() {
  // 统一设置与逐项设置互斥（与颜色/名称页一致）：改统一字号即复位逐项覆盖
  // （续篇 255/726：font_value 为后端死载荷，已删除）
  state.perFonts = null
  notifyConfig({ font_name: state.fontName, per_item_fonts: null })
}

/** 拖动中：只更新 state 让数值跟手显示，不发配置通知 */
function onPerFontInput(key, value) {
  if (!state.perFonts) state.perFonts = {}
  if (!state.perFonts[key]) state.perFonts[key] = { value: fontOf(key) }
  state.perFonts[key].value = Number(value)
}
/** 松手（change）才写盘 + 通知主程序 */
function onPerFontCommit() {
  notifyConfig({ per_item_fonts: JSON.parse(JSON.stringify(state.perFonts)) })
}
const fontOf = (key) =>
  state.perFonts && state.perFonts[key]
    ? state.perFonts[key].value
    : state.fontName
const fillOf = (key) => ({ '--fill': clampFill(fontOf(key)) + '%' })
</script>

<template>
  <section class="page">
    <div class="page-head r1">
      <h1 class="t-title">调整字号</h1>
    </div>

    <div class="page-body">
      <p class="t-body r2" style="color: var(--text-secondary); font-style: italic; font-weight: 400; line-height: 1.6; max-width: 460px">
        悬浮窗文字为 8–32px，可整体统一调整，或在面板中逐项单独设置。
      </p>
      <!-- 续篇 721：aria-haspopup/aria-expanded -->
      <button
        class="btn btn-secondary btn-block r3"
        aria-haspopup="dialog"
        :aria-expanded="showSheet ? 'true' : 'false'"
        @click="showSheet = true"
      >
        打开字号面板
      </button>
    </div>

    <!-- 字号面板：统一滑条 + 逐项滑条同屏；逐项松手才写盘，× 关闭 -->
    <WizardSheet :open="showSheet" size="l" title="字号设置" @close="showSheet = false">
      <div class="slider-row" style="margin-bottom: var(--sp-3)">
        <div class="slider-head">
          <span>统一字号：</span>
          <span class="slider-value">{{ state.fontName }}px</span>
        </div>
        <input
          class="slider-fill"
          type="range"
          :min="FONT_MIN"
          :max="FONT_MAX"
          step="1"
          aria-label="统一字号"
          :aria-valuetext="`${state.fontName}px`"
          :style="globalFill"
          v-model.number="state.fontName"
          @change="onGlobalFont"
        />
      </div>

      <p class="sub-hint" style="margin-bottom: var(--sp-2)">
        统一字号调整名称与数值（松开滑条后生效）；下方为逐项单独设置。
      </p>
      <div class="font-grid">
        <div v-for="it in items" :key="it.key" class="font-row">
          <span class="per-item-name">{{ it.label }}</span>
          <!-- 续篇 727：逐项滑条补 aria-label —— 读屏分不清是哪一项 -->
          <input
            class="slider-fill"
            type="range" :min="FONT_MIN" :max="FONT_MAX" step="1"
            :aria-label="`${it.label} 字号`"
            :value="fontOf(it.key)"
            :style="fillOf(it.key)"
            @input="e => onPerFontInput(it.key, e.target.value)"
            @change="onPerFontCommit"
          />
          <span class="slider-value">{{ fontOf(it.key) }}px</span>
        </div>
      </div>
    </WizardSheet>
  </section>
</template>
