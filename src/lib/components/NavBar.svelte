<script lang="ts">
  import { onMount } from "svelte";
  import { page } from "$app/state";
  import { OFFICIAL_SKINS } from "$lib/official-skins";
  import { api, onGameCrash } from "$lib/api";
  import { listen } from "@tauri-apps/api/event";
  import { toast } from "$lib/toast";
  import { watchLaunchEvents } from "$lib/launch-pending.svelte";
  import { showCrash } from "$lib/dialogs.svelte";
  import Logo from "./Logo.svelte";
  import HeadAvatar from "./HeadAvatar.svelte";
  import TaskRing from "./TaskRing.svelte";
  import ErrorDialog from "./ErrorDialog.svelte";
  import JavaChoiceDialog from "./JavaChoiceDialog.svelte";
  import type { Account } from "$lib/api";
  import { settingsLive } from "$lib/settings-live.svelte";
  import { backBlocked, canGoBackPage, goBackPage, rememberPage } from "$lib/page-history.svelte";
  import { errorDialog, javaPrompt } from "$lib/dialogs.svelte";

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
  /// 当前选中的账户：头像、名字、登录方式都跟着它走。
  let me = $state<Account | null>(null);
  let servers = $state<{ url: string; name: string }[]>([]);
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

  /// 顶栏那个头像框：读账户表，账户页改了会广播一次。
  async function loadMe() {
    const view = await api.listAccounts().catch(() => null);
    if (!view) return;
    servers = view.servers.map((server) => ({ url: server.url, name: server.name }));
    // 只有「选中了某个账户」才显示它的头像；取消选择后要回到未登录，
    // 不能退到 players[0]（那会让顶栏一直挂着第一个账户）。
    me = view.current
      ? (view.players.find((player) => player.id === view.current) ?? null)
      : null;
  }

  $effect(() => {
    const handler = () => void loadMe();
    void loadMe();
    window.addEventListener("ybmcl:accounts", handler);
    return () => window.removeEventListener("ybmcl:accounts", handler);
  });

  /// 顶栏头像用哪张皮肤：账户自己的皮肤，没有就退回原版 Steve（粗臂）。
  ///
  /// 离线账户通常没有独立皮肤，直接按 `avatar`/`skin` 取会掉到字母占位，
  /// 和下面账户卡片里那张立体皮肤对不上，所以这里统一给一张底图。
  const headSkin = $derived.by(() => {
    const account = me;
    if (!account) return null;
    if (account.skin) return account.skin;
    return account.skinModel === "slim" ? OFFICIAL_SKINS[0].slim : OFFICIAL_SKINS[0].wide;
  });

  /// 当前账户的登录方式：第三方服务器显示服务器名。
  const meKind = $derived.by(() => {
    const account = me;
    if (!account) return "离线账户";
    if (account.kind === "microsoft") return "微软账户";
    if (account.kind === "thirdparty") {
      const server = servers.find((item) => item.url === account.server);
      return server?.name ?? "第三方账户";
    }
    return "离线账户";
  });

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
    rememberPage(`${page.url.pathname}${page.url.search}`);
  });

  /// Esc / Alt+← 跨页面返回。
  ///
  /// 页面自己那几层（下载任务面板、右侧详情）在捕获阶段就 `preventDefault` 了，
  /// 这里放在冒泡阶段读 `defaultPrevented`，被页面处理过就不插手。
  function onGlobalKey(event: KeyboardEvent) {
    // 有弹窗时 Esc 归弹窗，不在这里翻页。
    const modal = errorDialog.value !== null || javaPrompt.value !== null;
    if (event.key === "Escape" && !event.defaultPrevented) {
      if (modal || backBlocked() || !canGoBackPage()) return;
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
    // 游戏非正常退出时弹诊断窗，不管用户当前在哪个页面。
    let stopCrash: (() => void) | null = null;
    const stopMemorySkip = listen<string>("memory-skip", (event) => {
      toast(`这次跳过了内存优化：${event.payload}`, "info", 3200);
    });
    void onGameCrash((event) => showCrash(event.versionId, event.code, event.hits)).then((off) => {
      stopCrash = off;
    });
    // 启动按钮的进度线：窗口出现 / 进程退出 / 崩溃都算这次启动结束。
    let stopLaunch: (() => void) | null = null;
    void watchLaunchEvents().then((off) => {
      stopLaunch = off;
    });
    return () => {
      observer.disconnect();
      window.removeEventListener("keydown", onGlobalKey);
      stopCrash?.();
      stopLaunch?.();
      void stopMemorySkip.then((off) => off());
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

  <a
    class="account"
    class:signed-out={!me}
    href="/accounts"
    title={me ? "账户设置" : "未登录，点这里添加账户"}
  >
    {#if me}
      <HeadAvatar skin={headSkin} size={30} fallback={me.name} />
      <span class="who">
        <strong>{me.name}</strong>
        <small>{meKind}</small>
      </span>
    {:else}
      <span class="head letter guest" aria-hidden="true">?</span>
      <span class="who">
        <strong>未登录</strong>
      </span>
    {/if}
  </a>
</nav>


<ErrorDialog />
<JavaChoiceDialog />

<style>
  .nav {
    height: var(--nav-h);
    flex: none;
    display: grid;
    /* 三块各占固定位置：品牌 268、导航占剩余、岛自动、账户 184。 */
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    column-gap: 16px;
    padding: 0 var(--gutter);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    max-width: 268px;
    overflow: hidden;
    padding: 4px 8px 4px 4px;
    border-radius: var(--radius-sm);
    transition: background 0.15s ease;
  }
  .brand:hover {
    background: var(--tint-1);
  }
  .name {
    display: flex;
    flex-direction: column;
    line-height: 1.15;
    min-width: 0;
  }
  .name strong {
    font-size: 15px;
    letter-spacing: 0.08em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .name span {
    font-size: 11px;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .links {
    position: relative;
    display: flex;
    align-items: center;
    gap: 2px;
    flex: 0 1 auto;
    padding: 4px;
    border-radius: 999px;
    background: linear-gradient(180deg, var(--tint-3), rgba(255, 255, 255, 0.045));
    border: 1px solid var(--tint-4);
    box-shadow:
      inset 0 1px 0 var(--glint),
      inset 0 -1px 0 rgba(0, 0, 0, 0.25),
      0 10px 28px rgba(0, 0, 0, 0.26);
    backdrop-filter: var(--glass-22);
    -webkit-backdrop-filter: var(--glass-22);
  }
  .link {
    position: relative;
    padding: 6px 14px;
    border-radius: 999px;
    color: var(--text-2);
    white-space: nowrap;
    transition: background 0.18s ease, color 0.18s ease, box-shadow 0.18s ease;
  }
  .link:hover {
    background: var(--line-soft);
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
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.26), var(--tint-4));
    box-shadow:
      inset 0 1px 0 var(--glint-strong),
      inset 0 -1px 0 var(--line-soft),
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
    min-width: 0;
    max-width: 184px;
    padding: 4px 10px 4px 4px;
    border-radius: 999px;
    border: 1px solid transparent;
    transition: background 0.15s ease, border-color 0.15s ease;
  }
  .account:hover {
    background: var(--tint-2);
    border-color: var(--border);
  }
  .head {
    display: block;
    width: 30px;
    height: 30px;
    /* 整张皮肤放大后只露出脸，多出来的部分裁掉。 */
    overflow: hidden;
    image-rendering: pixelated;
    flex: none;
  }
  .head.letter.guest {
    background: var(--line-soft);
    color: var(--text-dim);
  }
  .account.signed-out .who strong {
    color: var(--text-dim);
    font-weight: 520;
  }
  .head.letter {
    display: grid;
    place-items: center;
    background: rgba(120, 220, 160, 0.18);
    color: var(--text);
    font-size: 14px;
    font-weight: 600;
  }
  .who {
    display: flex;
    flex-direction: column;
    line-height: 1.15;
    min-width: 0;
    flex: 1;
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
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* 圆钮跟在导航药丸后面，面板从它下面弹出来，盖在页面上但不换页。 */
  .mid {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .tasks {
    position: relative;
    display: flex;
    align-items: center;
    flex: none;
  }
</style>
