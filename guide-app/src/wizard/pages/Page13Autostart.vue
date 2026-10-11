<script setup>
import { state, setAutostart } from '../composables/useGuide.js'

const OPTIONS = [
  { value: true, cls: 'autostart-card--yes', title: '要，开机后自动显示', desc: '安静显示在桌面，不弹窗、不打扰' },
  { value: false, cls: 'autostart-card--no', title: '先不用，我需要时自己打开', desc: '从桌面快捷方式启动' }
]

const pick = v => setAutostart(v) // 写入 system.cfg.autostart
</script>

<template>
  <section class="page">
    <div class="page-head r1">
      <h1 class="t-title">以后开机，还希望它自动出现吗？</h1>
    </div>

    <div class="page-body">
      <div class="autostart-list r2">
        <button
          v-for="o in OPTIONS"
          :key="String(o.value)"
          class="autostart-card"
          :class="[o.cls, { selected: state.autostart === o.value }]"
          :aria-pressed="state.autostart === o.value ? 'true' : 'false'"
          @click="pick(o.value)"
        >
          <span class="autostart-card__title">{{ o.title }}</span>
          <span class="autostart-card__desc">{{ o.desc }}</span>
        </button>
      </div>
    </div>
  </section>
</template>
