<script lang="ts">
  import "$lib/styles/theme.css";
  import { onMount } from "svelte";
  import { startPrefetch } from "$lib/detect";
  import { api } from "$lib/api";
  import { setTaskProgress } from "$lib/tasks.svelte";
  import { listen } from "@tauri-apps/api/event";
  import { page } from "$app/state";
  import TitleBar from "$lib/components/TitleBar.svelte";
  import NavBar from "$lib/components/NavBar.svelte";
  import Splash from "$lib/components/Splash.svelte";
  import Toast from "$lib/components/Toast.svelte";

  let { children } = $props();

  // 启动后在后台把版本清单和 Java 列表预热好，进「下载」页就不用等。
  onMount(startPrefetch);

  // 下载器按 120 ms 推一次字节进度，这里转给任务队列画进度环。
  onMount(() => {
    const pending = listen<{ id: number; received: number; total: number | null }>(
      "download-progress",
      (event) => setTaskProgress(event.payload.id, event.payload.received, event.payload.total)
    );
    return () => {
      void pending.then((unlisten) => unlisten()).catch(() => {});
    };
  });

  // /launch 页铺满（启动游戏时不要外层卡片），其它页面都装进大圆角卡片里。
  // 主页和游戏日志都不套卡片：主页只留版本栏，背景直接是壁纸。
  const isFullBleed = $derived(
    page.url.pathname === "/" || page.url.pathname.startsWith("/launch")
  );

  // 每 5 秒报一次心跳；主进程 30 秒收不到会主动重载页面。
  onMount(() => {
    const timer = setInterval(() => void api.heartbeat().catch(() => {}), 5000);
    return () => clearInterval(timer);
  });
</script>

<div class="app">
  <TitleBar />
  <NavBar />

  {#if isFullBleed}
    <main class="main full">
      {@render children()}
    </main>
  {:else}
    <main class="main">
      <section class="card">
        {#key page.url.pathname}
          <div class="page">
            {@render children()}
          </div>
        {/key}
      </section>
    </main>
  {/if}

  <Toast />
  <Splash />
</div>

<style>
  .app {
    height: 100vh;
    display: flex;
    flex-direction: column;
    position: relative;
    overflow: hidden;
    background-color: var(--bg-0);
    /* 默认背景图由 Vite 在构建时改写为带哈希的资源地址 */
    background-image:
      linear-gradient(180deg, rgba(6, 8, 12, 0.3), rgba(6, 8, 12, 0.72)),
      url("../lib/assets/background.jpg");
    background-position: center;
    background-size: cover;
    background-repeat: no-repeat;
  }
  /* 渐变已并入 .app 自身的背景层，子元素天然压在背景之上 */
  .main {
    flex: 1;
    min-height: 0;
    display: flex;
    padding: 0 var(--gutter) var(--gutter);
  }
  .main.full {
    padding: 0;
  }
  .card {
    flex: 1;
    min-width: 0;
    border-radius: var(--radius-lg);
    background: rgba(16, 19, 26, 0.62);
    border: 1px solid var(--border);
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.42);
    backdrop-filter: blur(14px) saturate(1.15);
    overflow: hidden;
    display: flex;
  }
  .page {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: 24px 26px;
    animation: fade-in 0.22s ease;
  }
  .full {
    overflow-y: auto;
  }
  @keyframes fade-in {
    from {
      opacity: 0;
      transform: translateY(4px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
</style>
