<script setup>
import { ref, onMounted } from 'vue'
import WizardSheet from '../WizardSheet.vue'
import { state, setOverlayPos, fetchOverlayRect } from '../composables/useGuide.js'

// 续篇 260/728：未使用的 `import { cmd }` 已删除；坐标异步就绪前禁用精确输入，
// 不再用 834/200 假坐标。
const posReady = ref(false)

onMounted(async () => {
  // state.pos 的初值由 fetchOverlayRect 填真值；不取真值的话，用户点「应用」
  // 就等于把悬浮窗瞬移到一个从没存在过的位置。
  await fetchOverlayRect()
  posX.value = String(state.pos.x)
  posY.value = String(state.pos.y)
  posReady.value = true
})

/* 精确输入面板 */
const showPos = ref(false)
const posX = ref('')
const posY = ref('')
const posErr = ref('')
function openPos() {
  if (!posReady.value) return
  posX.value = String(state.pos.x)
  posY.value = String(state.pos.y)
  posErr.value = ''
  showPos.value = true
}

/* 空串 / 非法输入必须报错，不能当成 0：
   `Math.round(Number(''))` === 0，旧实现会把悬浮窗一脚踢到屏幕左上角。
   续篇 729：坐标必须在屏幕范围内（后端 overlay_set_position 同口径钳制），
   屏外坐标会被后端 clamp 而非生效 —— 前端提前拒绝。 */
function parseCoord(raw, label, max) {
  const s = String(raw ?? '').trim()
  if (s === '') return { err: `${label}不能为空` }
  const n = Number(s)
  if (!Number.isFinite(n)) return { err: `${label}不是有效数字` }
  const r = Math.round(n)
  if (!Number.isSafeInteger(r)) return { err: `${label}超出可输入范围` }
  if (r < 0 || r > max) return { err: `${label}超出屏幕范围（0–${max}）` }
  return { value: r }
}

async function applyPos() {
  const sw = (state.boot.screen && state.boot.screen.w) || 1920
  const sh = (state.boot.screen && state.boot.screen.h) || 1080
  const x = parseCoord(posX.value, 'X 坐标', sw - 1)
  const y = parseCoord(posY.value, 'Y 坐标', sh - 1)
  if (x.err || y.err) {
    posErr.value = x.err || y.err
    return
  }
  posErr.value = ''
  // 续篇 730：写盘失败不关闭面板 —— 用户以为已应用是最贵的谎报
  const ok = await setOverlayPos(x.value, y.value)
  if (ok) showPos.value = false
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

      <button
        class="btn btn-secondary rise"
        style="margin-top: var(--sp-8); --rise-delay: 320ms"
        :disabled="!posReady"
        @click="openPos"
      >
        精确输入 X / Y…
      </button>
    </div>

    <!-- 续篇 721：aria-haspopup/aria-expanded -->
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
      <!-- 续篇 731：表单校验错误不再复用安装页专用的 .install-error -->
      <p v-if="posErr" class="form-error" role="alert">{{ posErr }}</p>
      <template #footer>
        <button class="btn btn-secondary" @click="showPos = false">取消</button>
        <button class="btn btn-primary" @click="applyPos">应用</button>
      </template>
    </WizardSheet>
  </section>
</template>
