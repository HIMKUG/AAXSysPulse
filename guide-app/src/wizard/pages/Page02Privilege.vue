<template>
  <section class="page page--center page--top">
    <div class="page-body">
      <h1 class="t-title r1">需要管理员权限</h1>

      <p class="t-body r2" style="margin-top: var(--sp-8); color: var(--text-secondary); font-style: italic; font-weight: 400; line-height: 1.6; max-width: 460px">
        功耗监控需要安装驱动，需要管理员权限。<br />
        点击下方按钮，以管理员身份重新启动。
      </p>
    </div>
  </section>
</template>

<script setup>
// 第 2 页：权限检测（仅 hasPrivilege === false 时进入）
// 「以管理员身份重启」由底部导航 → restartAsAdmin() → ShellExecuteW(runas)
</script>
