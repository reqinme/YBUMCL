<script lang="ts">
  // 启动动画占位版：Logo 自绘 + 标题浮现 + 进度条。
  // 之后换成视频/帧序列时，只需替换 <div class="stage"> 里的内容，
  // 外层的「可跳过 / 自动收起 / 淡出」逻辑不用动。
  import { onMount } from "svelte";
  import Logo from "$lib/components/Logo.svelte";
  import { api } from "$lib/api";

  const HOLD_MS = 1900;
  const FADE_MS = 380;

  let visible = $state(true);
  let leaving = $state(false);

  function dismiss() {
    if (leaving) return;
    leaving = true;
    setTimeout(() => (visible = false), FADE_MS);
  }

  onMount(() => {
    if (localStorage.getItem("ybumcl.splash") === "off") {
      visible = false;
      return;
    }
    // 设置里关掉开场动画时立刻收起（设置读取是异步的，不能挡在首帧前）。
    api
      .loadSettings()
      .then((settings) => {
        if (!settings.showSplash) dismiss();
      })
      .catch(() => {});

    const timer = setTimeout(dismiss, HOLD_MS);
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" || event.key === "Enter" || event.key === " ") dismiss();
    };
    window.addEventListener("keydown", onKey);

    return () => {
      clearTimeout(timer);
      window.removeEventListener("keydown", onKey);
    };
  });
</script>

{#if visible}
  <div
    class="splash"
    class:leaving
    role="presentation"
    onclick={dismiss}
    onkeydown={(event) => event.key === "Escape" && dismiss()}
  >
    <div class="stage">
      <Logo size={168} animated />
      <h1>延边大学我的世界启动器</h1>
      <p>Yanbian University Minecraft Launcher</p>
      <div class="bar"><span></span></div>
    </div>
    <span class="hint">点击任意处跳过</span>
  </div>
{/if}

<style>
  .splash {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: grid;
    place-items: center;
    background:
      radial-gradient(120% 90% at 50% 32%, rgba(34, 44, 30, 0.55), transparent 60%),
      radial-gradient(90% 70% at 50% 100%, rgba(20, 26, 40, 0.6), transparent 65%),
      var(--bg-0);
    cursor: pointer;
    opacity: 1;
    transition: opacity 0.38s ease;
  }
  .splash.leaving {
    opacity: 0;
    pointer-events: none;
  }
  .stage {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    transform: translateY(-2vh);
  }
  h1 {
    margin: 8px 0 0;
    font-size: 22px;
    font-weight: 650;
    letter-spacing: 0.02em;
    opacity: 0;
    animation: fade-up 0.6s ease-out 1.35s forwards;
  }
  p {
    margin: 0;
    font-size: 12px;
    color: var(--text-dim);
    letter-spacing: 0.06em;
    opacity: 0;
    animation: fade-up 0.6s ease-out 1.5s forwards;
  }
  .bar {
    margin-top: 16px;
    width: 168px;
    height: 3px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.1);
    overflow: hidden;
    opacity: 0;
    animation: fade-up 0.4s ease-out 1.6s forwards;
  }
  .bar span {
    display: block;
    width: 40%;
    height: 100%;
    border-radius: inherit;
    background: linear-gradient(90deg, transparent, var(--yb-green-bright), transparent);
    animation: sweep 1.1s ease-in-out infinite;
  }
  .hint {
    position: absolute;
    bottom: 24px;
    font-size: 11px;
    color: rgba(255, 255, 255, 0.34);
    opacity: 0;
    animation: fade-up 0.6s ease-out 1.9s forwards;
  }

  @keyframes fade-up {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
  @keyframes sweep {
    0% {
      transform: translateX(-110%);
    }
    100% {
      transform: translateX(260%);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    h1,
    p,
    .bar,
    .hint {
      animation: none;
      opacity: 1;
    }
    .bar span {
      animation: none;
      width: 100%;
    }
  }
</style>
