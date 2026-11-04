<script lang="ts">
  // 品牌标志：立方体 logo（PNG 位图，1024×1024 透明底，白色面保留）。
  // 同源文件 tools/icon/logo-1024.png 也用于生成 exe / 任务栏图标。
  import logo from "$lib/assets/logo.png";

  let { size = 40, animated = false }: { size?: number; animated?: boolean } = $props();
</script>

<img
  src={logo}
  alt="YBUMCL"
  class="logo"
  class:animated
  style:width="{size}px"
  style:height="{size}px"
/>

<style>
  .logo {
    display: block;
    user-select: none;
    -webkit-user-drag: none;
    object-fit: contain;
  }

  /* 启动动画：淡入 + 轻微放大，不依赖矢量描边，换成任何位图都能用。 */
  .animated {
    animation: reveal 0.95s cubic-bezier(0.2, 0.7, 0.3, 1) both;
  }

  @keyframes reveal {
    0% {
      opacity: 0;
      transform: scale(0.74) translateY(6px);
      filter: blur(8px);
    }
    55% {
      opacity: 1;
      filter: blur(0);
    }
    100% {
      opacity: 1;
      transform: scale(1) translateY(0);
      filter: blur(0);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .animated {
      animation: none;
    }
  }
</style>
