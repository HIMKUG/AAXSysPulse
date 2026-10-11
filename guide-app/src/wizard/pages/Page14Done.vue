<script setup>
import { onMounted } from 'vue'
import { state, fetchInstallInfo } from '../composables/useGuide.js'

onMounted(() => fetchInstallInfo()) // 取真实安装路径与占用数据
</script>

<template>
  <!-- 完成页：动画稍隆重 —— 标题 20px/500ms，两张卡片依次 200 / 300ms，
       提示文字 500ms，底部说明 600ms -->
  <section class="page page--center">
    <div class="page-body">
      <h1 class="t-hero rise" style="--rise-y: 20px; --rise-dur: 500ms; --rise-delay: 0ms">
        大功告成！
      </h1>

      <div class="stat-grid" style="margin-top: var(--sp-10)">
        <div v-if="state.stats.cpu" class="stat-card rise" style="--rise-delay: 200ms">
          <span class="stat-card__num">{{ state.stats.cpu }}</span>
          <span class="stat-card__desc">CPU 平均占用</span>
          <span class="stat-card__desc">自本次启动以来的实测均值</span>
        </div>
        <div v-if="state.stats.mem" class="stat-card rise" style="--rise-delay: 300ms">
          <span class="stat-card__num">{{ state.stats.mem }}</span>
          <span class="stat-card__desc">内存占用</span>
          <span class="stat-card__desc">任务管理器同款工作集口径</span>
        </div>
      </div>

      <p class="done-text t-center rise" style="--rise-delay: 500ms">
        <template v-if="state.boot.installPath">
          软件已成功安装到 <span class="install-path">{{ state.boot.installPath }}</span>。<br />
        </template>
        右键任务栏上的小图标，即可打开设置页面。
      </p>

      <!-- 主程序没起来时以前零提示（mainFailed 只写不读），
           用户按桌面图标找不到东西会以为没装上 -->
      <p v-if="state.mainFailed" class="done-text t-center rise" style="color: var(--text-secondary); --rise-delay: 560ms">
        不过，主程序这次没能自动启动。请双击桌面图标手动打开一次。
      </p>

      <p class="done-foot rise" style="--rise-delay: 600ms">
        如果想重新设置，可以在设置中找到“重新设置”。
      </p>
    </div>
  </section>
</template>
