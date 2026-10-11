<script setup>
import GuideWizard from './wizard/GuideWizard.vue'
/* 日间模式专用：不提供主题切换 */
</script>

<template>
  <div class="demo-desktop">
    <!-- Liquid Glass 折射滤镜 —— 逐字复制自 lucasromerodb/liquid-glass-effect-macos
         index.html 的 <filter id="glass-distortion">，零参数改动。 -->
    <svg width="0" height="0" style="position: absolute" aria-hidden="true">
      <defs>
        <filter id="glass-distortion" x="0%" y="0%" width="100%" height="100%" filterUnits="objectBoundingBox">
          <feTurbulence type="fractalNoise" baseFrequency="0.001 0.005" numOctaves="1" seed="17" result="turbulence" />
          <!-- Liked Seeds: 5, 14, 17 -->
          <feComponentTransfer in="turbulence" result="mapped">
            <feFuncR type="gamma" amplitude="1" exponent="10" offset="0.5" />
            <feFuncG type="gamma" amplitude="0" exponent="1" offset="0" />
            <feFuncB type="gamma" amplitude="0" exponent="1" offset="0.5" />
          </feComponentTransfer>
          <feGaussianBlur in="turbulence" stdDeviation="3" result="softMap" />
          <feSpecularLighting in="softMap" surfaceScale="5" specularConstant="1" specularExponent="100" lighting-color="white" result="specLight">
            <fePointLight x="-200" y="-200" z="300" />
          </feSpecularLighting>
          <feComposite in="specLight" operator="arithmetic" k1="0" k2="1" k3="1" k4="0" result="litImage" />
          <feDisplacementMap in="SourceGraphic" in2="softMap" scale="200" xChannelSelector="R" yChannelSelector="G" />
        </filter>
      </defs>
    </svg>

    <GuideWizard />
  </div>
</template>

<style>
.demo-desktop {
  min-height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  /* 背景配方复制示例 body 的静态部分：400px 平铺（高频细节 = 折射信号源）。
     漂移动画按用户指令移除 —— 背景保持固定。 */
  background: url("./assets/desktop-refraction.webp") center center;
  background-size: 400px;
}
</style>
