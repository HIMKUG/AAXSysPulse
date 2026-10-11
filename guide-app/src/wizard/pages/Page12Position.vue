<script setup>
import { ref } from 'vue'
import { onMounted } from 'vue'
import WizardSheet from '../WizardSheet.vue'
import { cmd } from '../bridge.js'
import { state, setOverlayPos, fetchOverlayRect } from '../composables/useGuide.js'

onMounted(() => {
  // state.pos 的初值是写死的演示坐标；不取真值的话，用户点「应用」
  // 就等于把悬浮窗瞬移到那个从没存在过的位置。
  fetchOverlayRect()
})

/* 精确输入面板 */
const showPos = ref(false)
const posX = ref(state.pos.x)
const posY = ref(state.pos.y)
const posErr = ref('')
function openPos() {
  posX.value = state.pos.x
  posY.value = state.pos.y
  posErr.value = ''
  showPos.value = true
}

/* 空串 / 非法输入必须报错，不能当成 0：
   `Math.round(Number(''))` === 0，旧实现会把悬浮窗一脚踢到屏幕左上角。 */
function parseCoord(raw, label) {
  const s = String(raw ?? '').trim()
  if (s === '') return { err: `${label}不能为空` }
  const n = Number(s)
  if (!Number.isFinite(n)) return { err: `${label}不是有效数字` }
  const r = Math.round(n)
  if (!Number.isSafeInteger(r)) return { err: `${label}超出可输入范围` }
  return { value: r }
}

async function applyPos() {
  const x = parseCoord(posX.value, 'X 坐标')
  const y = parseCoord(posY.value, 'Y 坐标')
  if (x.err || y.err) {
    posErr.value = x.err || y.err
    return
  }
  posErr.value = ''
  showPos.value = false
  await setOverlayPos(x.value, y.value)
}
</script>

<template>
  <section class="page page--center page--plain">
    <div class="page-body">
      <h1 class="t-title r1" style="margin-bottom: 64px">调整悬浮窗位置</h1>

      <p
        class="t-body rise"
        style="color: var(--text-secondary); font-style: italic; font-weight: 400; max-width: 440px; line-height: 1.6; --rise-delay: 200ms"
      >
        请直接在桌面上拖动悬浮窗到合适的位置。
      </p>

      <button class="btn btn-secondary rise" style="margin-top: var(--sp-8); --rise-delay: 320ms" @click="openPos">
        精确输入 X / Y…
      </button>
    </div>

    <WizardSheet :open="showPos" size="m" title="精确输入位置" @close="showPos = false">
      <div class="pos-form">
        <label class="pos-field">
          <span>X 坐标</span>
          <input v-model="posX" type="number" step="1" placeholder="如 834" @input="posErr = ''" @keyup.enter="applyPos" />
        </label>
        <label class="pos-field">
          <span>Y 坐标</span>
          <input v-model="posY" type="number" step="1" placeholder="如 200" @input="posErr = ''" @keyup.enter="applyPos" />
        </label>
      </div>
      <p class="pos-hint">以主屏左上角为原点，向右 / 向下为正，单位像素。</p>
      <p v-if="posErr" class="install-error" role="alert">{{ posErr }}</p>
      <template #footer>
        <button class="btn btn-secondary" @click="showPos = false">取消</button>
        <button class="btn btn-primary" @click="applyPos">应用</button>
      </template>
    </WizardSheet>
  </section>
</template>
