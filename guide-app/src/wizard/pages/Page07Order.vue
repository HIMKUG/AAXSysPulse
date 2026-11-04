<script setup>
import { ref, computed, nextTick, onUnmounted } from 'vue'
import { state, MONITOR_ITEMS, notifyConfig } from '../composables/useGuide.js'

/* 拖动排序：全程不改动 state.order，只用 translate 做视觉位移，
   松手时才提交数组。行高由 flex:1 均分（单页无滚动），
   拖动步长不再是写死常量——按下时实测相邻行的真实间距，
   任意项目数量/任意行高都成立。 */

const pitch = ref(0)          // 行间距（px，含 gap），onDown 时实测
                              // 续篇 714：量不出真实行距就禁用拖动（旧回退 58px 会跳格）
const fromIndex = ref(-1)     // 拖动起始下标
const toIndex = ref(-1)       // 当前目标下标
const offset = ref(0)         // 鼠标位移（px）
const settling = ref(false)   // 松手后的落位动画中
let startY = 0
// 续篇 715：落位定时器句柄 —— 卸载时清理，翻页后不再持有组件闭包
let settleTimer = null

const list = computed(() =>
  state.order.map(k => MONITOR_ITEMS.find(i => i.key === k)).filter(Boolean)
)

function measurePitch(el) {
  const rows = el?.parentElement?.children
  if (rows && rows.length > 1) {
    const p = rows[1].getBoundingClientRect().top - rows[0].getBoundingClientRect().top
    if (p > 0) {
      pitch.value = p
      return true
    }
  }
  return false
}

function onDown(e, i) {
  // 续篇 714：量不出真实行距（布局异常）就不进入拖动，不再回落硬编码 58px
  if (!measurePitch(e.target.closest('.drag-item'))) return
  fromIndex.value = i
  toIndex.value = i
  offset.value = 0
  settling.value = false
  startY = e.clientY
  window.addEventListener('pointermove', onMove)
  window.addEventListener('pointerup', onUp)
}

function onMove(e) {
  if (fromIndex.value < 0) return
  if (pitch.value <= 0) return
  offset.value = e.clientY - startY
  const max = state.order.length - 1
  const t = Math.max(0, Math.min(max, fromIndex.value + Math.round(offset.value / pitch.value)))
  if (t !== toIndex.value) toIndex.value = t
}

async function onUp() {
  if (fromIndex.value < 0) return
  window.removeEventListener('pointermove', onMove)
  window.removeEventListener('pointerup', onUp)

  const from = fromIndex.value
  const to = toIndex.value

  if (from === to) {
    reset()
    return
  }

  // 1) 提交顺序
  const arr = state.order
  const [key] = arr.splice(from, 1)
  arr.splice(to, 0, key)
  notifyConfig({ order: [...arr] }) // 松手才落盘 + 通知主程序

  // 2) 数组已重排，把 offset 换算成「相对新位置」的残差，
  //    再动画归零 —— 避免松手瞬间跳变
  offset.value = offset.value - (to - from) * pitch.value
  fromIndex.value = to
  settling.value = true // 落位时恢复 transition

  await nextTick()
  offset.value = 0
  // 续篇 715：句柄保存，卸载时 clear（260：换页卸载不再遗留全局监听/定时器）
  clearTimeout(settleTimer)
  settleTimer = setTimeout(reset, 260)
}

function reset() {
  fromIndex.value = -1
  toIndex.value = -1
  offset.value = 0
  settling.value = false
}

// 续篇 260/715：换页卸载时清理全局监听与定时器，不留悬挂闭包
onUnmounted(() => {
  window.removeEventListener('pointermove', onMove)
  window.removeEventListener('pointerup', onUp)
  clearTimeout(settleTimer)
})

/** 续篇 716：键盘重排 —— 拖动把手对键盘用户曾完全不可用 */
function moveByKeyboard(i, delta) {
  const j = i + delta
  if (j < 0 || j >= state.order.length) return
  const arr = state.order
  const [key] = arr.splice(i, 1)
  arr.splice(j, 0, key)
  notifyConfig({ order: [...arr] })
}

function itemStyle(i) {
  const f = fromIndex.value
  const t = toIndex.value
  if (f < 0) return null

  // 被拖动项：跟随鼠标，拖动中禁用过渡（实时响应）
  // 用独立的 translate 属性，避免覆盖 .dragging 的 scale / transform
  if (i === f) {
    return {
      translate: `0 ${offset.value}px`,
      transition: settling.value ? '' : 'none',
      zIndex: 3
    }
  }

  // 让位项：按区间整体平移一个行距
  const step = pitch.value
  let dy = 0
  if (t > f && i > f && i <= t) dy = -step
  else if (t < f && i >= t && i < f) dy = step
  return dy ? { translate: `0 ${dy}px` } : null
}
</script>

<template>
  <section class="page page--order">
    <div class="page-head r1">
      <h1 class="t-title">调整监控项顺序</h1>
    </div>

    <div class="page-body r2">
      <div class="drag-list">
        <div
          v-for="(it, i) in list"
          :key="it.key"
          class="drag-item"
          :class="{ dragging: i === fromIndex }"
          :style="itemStyle(i)"
        >
          <!-- 续篇 716：排序对键盘可用 —— tabindex/role/上下键重排 -->
          <span
            class="drag-handle"
            role="button"
            tabindex="0"
            :aria-label="`调整 ${it.label} 的顺序，按上下方向键移动`"
            @pointerdown.prevent="onDown($event, i)"
            @keydown.up.prevent="moveByKeyboard(i, -1)"
            @keydown.down.prevent="moveByKeyboard(i, 1)"
          >≡</span>
          <span class="drag-name">{{ it.label }}</span>
        </div>
      </div>
    </div>
  </section>
</template>
