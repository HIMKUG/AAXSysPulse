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
  const idx = CYCLE.indexOf(cur)
  // 续篇 717：坏存档值不在循环表内时兜底回 'follow' 并留痕，
  // 不再 `(-1+1)%4` 静默归一成 'follow' 而无任何痕迹。
  if (idx === -1) {
    console.warn('[guide] 逐项命名模式存档值非法，已按跟随档重新循环', { key, cur })
  }
  state.perNameMode[key] = CYCLE[idx === -1 ? 0 : (idx + 1) % CYCLE.length]
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

      <!-- 续篇 718：aria-pressed + aria-live —— 键盘点击循环后读屏播报新档位 -->
      <div class="nm-grid r3" aria-live="polite">
        <button
          v-for="it in items"
          :key="it.key"
          class="nm-chip"
          :class="{ 'nm-chip--set': modeOf(it.key) !== 'follow' }"
          :aria-pressed="modeOf(it.key) !== 'follow' ? 'true' : 'false'"
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
/* 续篇 719：不再 calc(100% - 86px) 硬编码 hint 高度 —— 改 flex 列布局，
   文案换行/字号变化不会让网格错位。 */
.page-body {
  display: flex;
  flex-direction: column;
}
.nm-grid {
  /* 项少时（预设过滤后可能不足 9 项）整体垂直居中，避免出现大片空洞 */
  flex: 1;
  align-content: center;
  min-height: 0;
}
</style>
