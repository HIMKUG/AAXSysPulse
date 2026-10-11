<script setup>
import { ref, computed } from 'vue'
import WizardSheet from '../WizardSheet.vue'
import { state, activeItems, PRESET_COLORS, notifyConfig } from '../composables/useGuide.js'

const showSheet = ref(false)
const items = computed(() => activeItems.value)

function setColor(field, value) {
  state.colors[field] = value.toUpperCase()
  state.perColors = null // 统一设置与逐项设置互斥
  notifyConfig({ colors: { ...state.colors }, per_item_colors: null })
}
function setPerColor(key, field, value) {
  if (!state.perColors) state.perColors = {}
  if (!state.perColors[key]) state.perColors[key] = { ...state.colors }
  state.perColors[key][field] = value.toUpperCase()
  notifyConfig({ per_item_colors: JSON.parse(JSON.stringify(state.perColors)) })
}
const colorOf = (key, field) =>
  state.perColors && state.perColors[key] ? state.perColors[key][field] : state.colors[field]

/* `<input type="color">` 的三个事件语义（审查项 8B.L）—— 本页一律照此分工：
   · `input`：拖选过程中**持续**触发 ⇒ 一律不接（接了就是拖动时每帧写 INI）。
   · `change`：只在拾取器关闭时触发 ⇒ **唯一**的"改状态 + 落盘"入口。
   · `cancel`：用户按 Esc 放弃 ⇒ 浏览器回滚控件值，但我们的绑定是单向 `:value`、
     状态没变化 ⇒ Vue 不会重新 patch，控件可能留着被放弃的那个颜色。
     显式按当前状态回写 DOM，保证"眼睛看到的 == 实际存的"。 */
</script>

<template>
  <section class="page">
    <div class="page-head r1">
      <h1 class="t-title">选择颜色</h1>
    </div>

    <div class="page-body">
      <p class="t-body r2" style="color: var(--text-secondary); font-style: italic; font-weight: 400; line-height: 1.6; max-width: 460px">
        每个监控项都有「名字」与「数值」两种颜色，
        可在面板中统一设置，或逐项单独调整。
      </p>
      <button class="btn btn-secondary btn-block r3" @click="showSheet = true">
        打开颜色面板
      </button>
    </div>

    <!-- 颜色面板：统一 + 逐项同屏，改动即时生效，× 关闭 -->
    <WizardSheet :open="showSheet" size="l" title="颜色设置" @close="showSheet = false">
      <div class="color-block">
        <div class="color-block__title">名字颜色：</div>
        <div class="color-row">
          <label class="color-picker">
            <span class="swatch" style="width: 22px; height: 22px" :style="{ background: state.colors.name }"></span>
            <input type="color" :value="state.colors.name" @change="e => setColor('name', e.target.value)" @cancel="e => (e.target.value = state.colors.name)" />
          </label>
          <span class="color-hex">{{ state.colors.name }}</span>
          <div class="swatch-list">
            <button
              v-for="c in PRESET_COLORS"
              :key="c.value"
              class="swatch"
              :class="{ active: state.colors.name === c.value }"
              :aria-pressed="state.colors.name === c.value ? 'true' : 'false'"
              :style="{ background: c.value }"
              :title="c.name"
              @click="setColor('name', c.value)"
            ></button>
          </div>
        </div>
      </div>

      <div class="color-block">
        <div class="color-block__title">数值颜色：</div>
        <div class="color-row">
          <label class="color-picker">
            <span class="swatch" style="width: 22px; height: 22px" :style="{ background: state.colors.value }"></span>
            <input type="color" :value="state.colors.value" @change="e => setColor('value', e.target.value)" @cancel="e => (e.target.value = state.colors.value)" />
          </label>
          <span class="color-hex">{{ state.colors.value }}</span>
          <div class="swatch-list">
            <button
              v-for="c in PRESET_COLORS"
              :key="c.value"
              class="swatch"
              :class="{ active: state.colors.value === c.value }"
              :aria-pressed="state.colors.value === c.value ? 'true' : 'false'"
              :style="{ background: c.value }"
              :title="c.name"
              @click="setColor('value', c.value)"
            ></button>
          </div>
        </div>
      </div>

      <p class="sub-hint">以下为逐项单独设置（未单独设置的项跟随上方统一颜色）</p>
      <div class="per-grid">
        <div v-for="it in items" :key="it.key" class="per-item-row">
          <span class="per-item-name">{{ it.label }}</span>
          <label class="color-picker" title="名字颜色">
            <span class="swatch" style="width: 18px; height: 18px" :style="{ background: colorOf(it.key, 'name') }"></span>
            <input type="color" :value="colorOf(it.key, 'name')" @change="e => setPerColor(it.key, 'name', e.target.value)" @cancel="e => (e.target.value = colorOf(it.key, 'name'))" />
          </label>
          <label class="color-picker" title="数值颜色">
            <span class="swatch" style="width: 18px; height: 18px" :style="{ background: colorOf(it.key, 'value') }"></span>
            <input type="color" :value="colorOf(it.key, 'value')" @change="e => setPerColor(it.key, 'value', e.target.value)" @cancel="e => (e.target.value = colorOf(it.key, 'value'))" />
          </label>
        </div>
      </div>
    </WizardSheet>
  </section>
</template>
