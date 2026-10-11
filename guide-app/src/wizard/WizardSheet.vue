<script setup>
// 统一面板（sheet）：所有确认框 / 详细调整 / 驱动进度共用同一形态。
// macOS sheet 版式：标题靠左 + 右上角 ×，主按钮右下角。
// size：s=确认框 · m=中等表单 · l=逐项详细调整
import { nextTick, onBeforeUnmount, ref, watch } from 'vue'

const props = defineProps({
  open: { type: Boolean, default: false },
  title: { type: String, default: '' },
  size: { type: String, default: 's' },
  closable: { type: Boolean, default: true },     // 显示 ×
  scrimClose: { type: Boolean, default: true },   // 点遮罩空白 = 关闭
})
const emit = defineEmits(['close'])

/* ── 键盘可达性（审查项 8B.K / P2-9）──────────────────────────
   面板此前完全依赖鼠标：没有 Esc 关闭，Tab 会跑到面板背后的向导页上，
   读屏软件也拿不到"模态"语义。三条都在这个共用组件里补齐，14 个页面
   一次性受益（Sheet 是全项目唯一的弹层实现）。 */

const scrimEl = ref(null)
let opener = null // 打开面板时的焦点，关闭后还原

const FOCUSABLE =
  'button:not([disabled]), input:not([disabled]), select:not([disabled]), ' +
  'textarea:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])'

function focusables() {
  const root = scrimEl.value
  if (!root) return []
  return Array.from(root.querySelectorAll(FOCUSABLE)).filter(
    (el) => el.offsetWidth || el.offsetHeight || el.getClientRects().length
  )
}

function onKeydown(e) {
  if (!props.open) return
  if (e.key === 'Escape') {
    // 不可关闭的面板（安装进行中）连 Esc 一起挡，避免"看起来能取消"。
    e.stopPropagation()
    e.preventDefault()
    if (props.closable) emit('close')
    return
  }
  if (e.key !== 'Tab') return
  const els = focusables()
  if (!els.length) {
    e.preventDefault()
    return
  }
  const first = els[0]
  const last = els[els.length - 1]
  const active = document.activeElement
  if (!scrimEl.value || !scrimEl.value.contains(active)) {
    e.preventDefault()
    first.focus()
    return
  }
  if (e.shiftKey && active === first) {
    e.preventDefault()
    last.focus()
  } else if (!e.shiftKey && active === last) {
    e.preventDefault()
    first.focus()
  }
}

watch(
  () => props.open,
  async (v) => {
    if (typeof document === 'undefined') return
    if (v) {
      opener = document.activeElement
      document.addEventListener('keydown', onKeydown, true)
      await nextTick()
      focusables()[0]?.focus()
    } else {
      document.removeEventListener('keydown', onKeydown, true)
      // 只在"焦点还留在面板里"时还原：面板一销毁，焦点会掉回 <body>，
      // 键盘用户就丢位置了；而若用户已把焦点点到别处，硬塞回去反而打断他。
      const inside =
        opener && scrimEl.value && scrimEl.value.contains(document.activeElement)
      if (inside) opener.focus?.()
      opener = null
    }
  }
)

onBeforeUnmount(() => {
  if (typeof document !== 'undefined') document.removeEventListener('keydown', onKeydown, true)
})
</script>

<template>
  <!-- defer：挂载后才解析目标（首挂时 .guide-window 尚未入文档）。
       必须挂到窗口层：scrim 若留在 .page 内，可用宽只有 529−2×40=449，
       sheet--l 的 560 被 max-width:100% 夹掉 → 面板右侧控件整列被裁。
       挂到窗口后 scrim 内缘 = 680−80 × 560−80，sheet--l 560×480 恰好吃满。 -->
  <Teleport to=".guide-window" defer>
    <Transition name="fade">
      <div v-if="open" ref="scrimEl" class="sheet-scrim" @click.self="scrimClose && emit('close')">
        <div
          class="sheet"
          :class="`sheet--${size}`"
          role="dialog"
          aria-modal="true"
          :aria-label="title"
        >
          <div class="sheet-head">
            <span class="sheet-title">{{ title }}</span>
            <button v-if="closable" class="sheet-close" aria-label="关闭" @click="emit('close')">
              <svg width="12" height="12" viewBox="0 0 14 14" fill="none">
                <path d="M2 2L12 12M12 2L2 12" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
              </svg>
            </button>
          </div>
          <div class="sheet-body">
            <slot />
          </div>
          <div v-if="$slots.footer" class="sheet-foot">
            <slot name="footer" />
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>
