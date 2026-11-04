<script lang="ts">
  import { api } from "$lib/api";
  import logo from "$lib/assets/logo.png";

  let { project, size = 44 }: { project: { id: string; title: string; iconUrl?: string | null }; size?: number } =
    $props();

  /// 结果按 url 缓存：翻页、切分类回来不用重新取一次。
  const cache = new Map<string, string | null>();
  /// loading 时先放启动器 logo，拿到图再换；确认没有图标才显示项目名。
  let phase = $state<"loading" | "image" | "missing">("loading");
  let dataUrl = $state<string | null>(null);

  /// 单个图标最多等这么久，超时当作没有图标。
  const TIMEOUT = 6000;

  /// 换项目时组件会被复用（详情面板没重建），所以跟着 iconUrl 走，不能只在 onMount 里取一次。
  let token = 0;
  $effect(() => {
    const url = project.iconUrl;
    const mine = ++token;
    if (!url) {
      dataUrl = null;
      phase = "missing";
      return;
    }
    if (cache.has(url)) {
      const cached = cache.get(url) ?? null;
      dataUrl = cached;
      phase = cached ? "image" : "missing";
      return;
    }
    dataUrl = null;
    phase = "loading";
    void (async () => {
      const timeout = new Promise<null>((resolve) => setTimeout(() => resolve(null), TIMEOUT));
      let source: string | null = null;
      try {
        source = await Promise.race([api.fetchIcon(url), timeout]);
      } catch {
        source = null;
      }
      cache.set(url, source);
      if (mine !== token) return;
      dataUrl = source;
      phase = source ? "image" : "missing";
    })();
  });
</script>

{#if phase === "image" && dataUrl}
  <img class="icon" src={dataUrl} alt="" style="width: {size}px; height: {size}px" />
{:else if phase === "loading"}
  <span class="tile loading" style="width: {size}px; height: {size}px">
    <img src={logo} alt="" />
  </span>
{:else}
  <span class="tile missing" style="width: {size}px; height: {size}px">
    <span class="name">{project.title}</span>
  </span>
{/if}

<style>
  .icon,
  .tile {
    display: block;
    width: 44px;
    height: 44px;
    border-radius: 10px;
    flex: none;
  }
  .icon {
    object-fit: cover;
    background: var(--surface-2);
    animation: fade-in 0.18s ease;
  }
  @keyframes fade-in {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }
  .tile {
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
  }
  /* 加载中：压暗的启动器 logo，等真图来了再换掉 */
  .loading {
    background: var(--surface-2);
  }
  .loading img {
    width: 72%;
    height: 72%;
    object-fit: contain;
    opacity: 0.55;
  }
  /* 确实没有图标：白板 + 项目名，最多两行 */
  .missing {
    background: rgba(255, 255, 255, 0.92);
    padding: 4px;
  }
  .name {
    font-size: 9px;
    line-height: 1.15;
    font-weight: 600;
    color: #2a2f26;
    text-align: center;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    word-break: break-word;
  }
</style>
