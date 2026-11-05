<script lang="ts">
  import { onMount } from "svelte";
  import { page } from "$app/state";
  import { api } from "$lib/api";
  import Logo from "./Logo.svelte";
  import Island from "./Island.svelte";
  import TaskRing from "./TaskRing.svelte";
  import ErrorDialog from "./ErrorDialog.svelte";
  import JavaChoiceDialog from "./JavaChoiceDialog.svelte";
  import avatarOffline from "$lib/assets/avatar-offline.png";
  import { settingsLive } from "$lib/settings-live.svelte";
  import { canGoBackPage, goBackPage, rememberPage } from "$lib/page-history.svelte";

  const items = [
    { href: "/", label: "主页" },
    { href: "/versions", label: "版本" },
    { href: "/downloads", label: "下载" },
    { href: "/accounts", label: "账户" },
    { href: "/logs", label: "日志" },
    { href: "/toolbox", label: "工具" },
    { href: "/settings", label: "设置" },
    { href: "/about", label: "关于" }
  ];

  let username = $state("Steve");
  /// 药丸滑动的时长（毫秒）；0 表示直接跳位，不做动画。
  let slideMs = $state(220);
  let linksEl = $state<HTMLDivElement | null>(null);
  /// 药丸相对 `.links` 的位置与大小，单位 px。
  let pillX = $state(0);
  let pillY = $state(0);
  let pillW = $state(0);
  let pillH = $state(0);
  /// 首帧先无动画摆好位置，量完两帧再打开过渡，避免从最左边飞过来。
  let pillReady = $state(false);

  /// 按当前选中项重新量一次药丸。用 getBoundingClientRect 而不是 offsetLeft，
  /// 这样 `.links` 的内边距不必写死成数字。
  function measurePill() {
    const box = linksEl;
    const active = box?.querySelector<HTMLElement>("a.link.active");
    if (!box || !active) return;
    const outer = box.getBoundingClientRect();
    const inner = active.getBoundingClientRect();
    pillX = inner.left - outer.left;
    pillY = inner.top - outer.top;
    pillW = inner.width;
    pillH = inner.height;
    if (!pillReady) {
      requestAnimationFrame(() => requestAnimationFrame(() => (pillReady = true)));
    }
  }

  function isActive(href: string) {
    const path = page.url.pathname;
    return href === "/" ? path === "/" : path.startsWith(href);
  }

  // 账户页改完名字回到主页要立刻反映；设置页保存后也要立刻生效，所以跟着这两件事重读设置。
  $effect(() => {
    settingsLive.version;
    page.url.pathname;
    void api
      .loadSettings()
      .then((settings) => {
        username = settings.username;
        slideMs = Math.max(0, Math.min(600, settings.navSlideMs ?? 220));
      })
      .catch(() => {});
  });

  /// 路由一变就重新量药丸；量到之后它自己会滑过去。
  $effect(() => {
    page.url.pathname;
    measurePill();
  });

  /// 路由一变就把当前页记进返回栈，供「跨页面返回」用。
  $effect(() => {
    rememberPage(page.url.pathname);
  });

  /// Esc / Alt+← 跨页面返回。
  ///
  /// 页面自己那几层（下载任务面板、右侧详情）在捕获阶段就 `preventDefault` 了，
  /// 这里放在冒泡阶段读 `defaultPrevented`，被页面处理过就不插手。
  function onGlobalKey(event: KeyboardEvent) {
    if (event.key === "Escape" && !event.defaultPrevented) {
      if (!canGoBackPage()) return;
      event.preventDefault();
      void goBackPage();
      return;
    }
    if (event.key === "ArrowLeft" && event.altKey) {
      if (!canGoBackPage()) return;
      event.preventDefault();
      void goBackPage();
    }
  }

  onMount(() => {
    // 字体加载完、窗口缩放都会改变每个词的宽度，跟着重量。
    const observer = new ResizeObserver(() => measurePill());
    if (linksEl) observer.observe(linksEl);
    void document.fonts?.ready.then(() => measurePill());
    window.addEventListener("keydown", onGlobalKey);
    return () => {
      observer.disconnect();
      window.removeEventListener("keydown", onGlobalKey);
    };
  });
</script>

<nav class="nav">
  <a class="brand" href="/">
    <Logo size={34} />
    <div class="name">
      <strong>YBUMCL</strong>
      <span>延边大学我的世界启动器</span>
    </div>
  </a>

  <div class="mid">
    <div class="links" bind:this={linksEl}>
      <span
        class="pill"
        class:ready={pillReady}
        style="--x:{pillX}px; --y:{pillY}px; --w:{pillW}px; --h:{pillH}px; --ms:{slideMs}ms"
        aria-hidden="true"
      ></span>
      {#each items as item (item.href)}
        <a class="link" class:active={isActive(item.href)} href={item.href}>{item.label}</a>
      {/each}
    </div>

    <div class="tasks">
      <TaskRing />
    </div>
  </div>

  <div class="right">
    <Island />
  </div>

  <a class="account" href="/accounts" title="账户设置">
    <img class="head" src={avatarOffline} alt="" />
    <span class="who">
      <strong>{username}</strong>
      <small>离线账户</small>
    </span>
  </a>
</nav>


<ErrorDialog />
<JavaChoiceDialog />

<style>
  .nav {
    height: var(--nav-h);
    flex: none;
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 0 var(--gutter);
  }
  /* 岛和任务按钮贴在右边，岛收起时宽度为 0，不占位置。 */
  .right {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 8px 4px 4px;
    border-radius: var(--radius-sm);
    transition: background 0.15s ease;
  }
  .brand:hover {
    background: rgba(255, 255, 255, 0.05);
  }
  .name {
    display: flex;
    flex-direction: column;
    line-height: 1.15;
  }
  .name strong {
    font-size: 15px;
    letter-spacing: 0.08em;
  }
  .name span {
    font-size: 11px;
    color: var(--text-dim);
  }
  .links {
    position: relative;
    display: flex;
    align-items: center;
    gap: 2px;
    flex: 0 1 auto;
    padding: 4px;
    border-radius: 999px;
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.12), rgba(255, 255, 255, 0.045));
    border: 1px solid rgba(255, 255, 255, 0.14);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.22),
      inset 0 -1px 0 rgba(0, 0, 0, 0.25),
      0 10px 28px rgba(0, 0, 0, 0.26);
    backdrop-filter: blur(22px) saturate(1.7);
    -webkit-backdrop-filter: blur(22px) saturate(1.7);
  }
  .link {
    position: relative;
    padding: 6px 14px;
    border-radius: 999px;
    color: rgba(255, 255, 255, 0.8);
    white-space: nowrap;
    transition: background 0.18s ease, color 0.18s ease, box-shadow 0.18s ease;
  }
  .link:hover {
    background: rgba(255, 255, 255, 0.09);
    color: var(--text);
  }
  .link.active {
    color: #fff;
    text-shadow: 0 1px 2px rgba(0, 0, 0, 0.3);
  }
  /* 整条导航只有这一颗药丸，切换时横向滑到新位置并缓动宽度。 */
  .pill {
    position: absolute;
    left: 0;
    top: 0;
    width: var(--w);
    height: var(--h);
    transform: translate(var(--x), var(--y));
    border-radius: 999px;
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.26), rgba(255, 255, 255, 0.14));
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.45),
      inset 0 -1px 0 rgba(255, 255, 255, 0.08),
      0 4px 14px rgba(0, 0, 0, 0.22);
    opacity: 0;
    pointer-events: none;
  }
  .pill.ready {
    opacity: 1;
    transition:
      transform var(--ms) cubic-bezier(0.22, 0.61, 0.36, 1),
      width var(--ms) cubic-bezier(0.22, 0.61, 0.36, 1),
      opacity 0.14s linear;
  }
  @media (prefers-reduced-motion: reduce) {
    .pill.ready {
      transition: opacity 0.14s linear;
    }
  }

  .account {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 4px 10px 4px 4px;
    border-radius: 999px;
    border: 1px solid transparent;
    transition: background 0.15s ease, border-color 0.15s ease;
  }
  .account:hover {
    background: rgba(255, 255, 255, 0.07);
    border-color: var(--border);
  }
  .head {
    width: 30px;
    height: 30px;
    border-radius: 8px;
    image-rendering: pixelated;
    flex: none;
  }
  .who {
    display: flex;
    flex-direction: column;
    line-height: 1.15;
    max-width: 140px;
  }
  .who strong {
    font-size: 13px;
    font-weight: 560;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .who small {
    font-size: 11px;
    color: var(--text-dim);
  }

  /* 圆钮跟在导航药丸后面，面板从它下面弹出来，盖在页面上但不换页。 */
  .mid {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: 0 1 auto;
    margin-right: auto;
    min-width: 0;
  }
  .tasks {
    position: relative;
    display: flex;
    align-items: center;
    flex: none;
  }
</style>
