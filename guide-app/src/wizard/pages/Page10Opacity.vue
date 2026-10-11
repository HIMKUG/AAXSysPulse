<script setup>
import { computed } from 'vue'
import { state, notifyConfig } from '../composables/useGuide.js'

const fill = computed(() => ({ '--fill': state.opacity + '%' }))
</script>

<template>
  <section class="page page--center">
    <div class="page-body">
      <h1 class="t-title r1" style="margin-bottom: 96px">调整透明度</h1>

      <div class="opacity-row r2">
        <input
          class="slider-fill"
          type="range"
          min="0"
          max="100"
          step="1"
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
