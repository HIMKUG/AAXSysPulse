<script setup>
import { computed } from 'vue'
import { state, activeItems, notifyConfig } from '../composables/useGuide.js'

const GLOBAL = [
  { key: 'none', label: '不显示名称' },
  { key: 'simple', label: '简单名称' },
  { key: 'full', label: '完整名称' }
]
const MODE_LABEL = { follow: '跟随', none: '隐藏', simple: '简单', full: '完整' }
/** 点击循环顺序：跟随 → 简单 → 完整 → 隐藏 → 跟随 */
const CYCLE = ['follow', 'simple', 'full', 'none']

const items = computed(() => activeItems.value)

function pickGlobal(key) {
  state.nameMode = key
  state.perNameMode = null // 统一设置与逐项设置互斥；null 会让后端把逐项复位为跟随
  notifyConfig({ name_mode: key, per_item_name_mode: null })
}

function cycle(key) {
  if (!state.perNameMode) state.perNameMode = {}
  const cur = state.perNameMode[key] || 'follow'
  state.perNameMode[key] = CYCLE[(CYCLE.indexOf(cur) + 1) % CYCLE.length]
  // 始终带上全局 name_mode，确保后端拿到完整状态（全局先设、单项后覆盖）
  notifyConfig({ name_mode: state.nameMode, per_item_name_mode: { ...state.perNameMode } })
}

const modeOf = key => (state.perNameMode && state.perNameMode[key]) || 'follow'
/** 芯片副行：跟随档显示「跟随 · 生效值」，显式档直接显示自己的模式 */
function chipMode(key) {
  const m = modeOf(key)
  return m === 'follow' ? `跟随 · ${MODE_LABEL[state.nameMode]}` : MODE_LABEL[m]
}
</script>

<template>
  <section class="page">
    <div class="page-head r1">
      <h1 class="t-title">名称怎么显示？</h1>
    </div>

    <div class="page-body">
      <div class="nm-global r2">
        <span class="nm-label">统一设置</span>
        <div class="seg">
          <button
            v-for="g in GLOBAL"
            :key="g.key"
            class="seg-item"
            :class="{ active: !state.perNameMode && state.nameMode === g.key }"
            :aria-pressed="(!state.perNameMode && state.nameMode === g.key) ? 'true' : 'false'"
            @click="pickGlobal(g.key)"
          >{{ g.label }}</button>
        </div>
      </div>

      <p class="nm-hint r2">单独调整 · 点击卡片循环：跟随 → 简单 → 完整 → 隐藏（蓝框 = 已单独设置）</p>

      <div class="nm-grid r3">
        <button
          v-for="it in items"
          :key="it.key"
          class="nm-chip"
          :class="{ 'nm-chip--set': modeOf(it.key) !== 'follow' }"
          @click="cycle(it.key)"
        >
          <span class="nm-chip__name">{{ it.label }}</span>
          <span class="nm-chip__mode">{{ chipMode(it.key) }}</span>
        </button>
      </div>
    </div>
  </section>
</template>

<style scoped>
.nm-grid {
  /* 项少时（预设过滤后可能不足 9 项）整体垂直居中，避免出现大片空洞 */
  align-content: center;
  height: calc(100% - 86px);
  min-height: 0;
}
</style>
