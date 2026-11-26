<script lang="ts">
  // 工具：磁盘占用、清理、Java 环境、快捷入口、启动器信息。除了清理不改任何东西。
  import { onMount } from "svelte";
  import { announce } from "$lib/tasks.svelte";
  import { showError } from "$lib/dialogs.svelte";
  import { api } from "$lib/api";
  import type { CacheUsage, CleanTarget, JavaInstallation, LauncherInfo, UsageEntry } from "$lib/api";
  import { cachedJava } from "$lib/detect";
  
  let usage = $state<UsageEntry[]>([]);
  let targets = $state<CleanTarget[]>([]);
  let pending = $state<string | null>(null);
  let javas = $state<JavaInstallation[]>([]);
  /// 磁盘占用卡上用来说明统计的是哪个目录。
  let info = $state<LauncherInfo | null>(null);
  let scanning = $state(true);
  let detecting = $state(false);
  let cache = $state<CacheUsage | null>(null);
  let cacheScanning = $state(false);
  /// 正在等第二次点击的清理范围：分组 key，或 "all"。
  let cachePending = $state<string | null>(null);

  const totalBytes = $derived(usage.reduce((sum, entry) => sum + entry.bytes, 0));
  const maxBytes = $derived(usage.reduce((max, entry) => Math.max(max, entry.bytes), 0));

  onMount(() => {
    void api
      .loadSettings()
      .then((value) => (settings = value))
      .catch(() => {});
    void api
      .info()
      .then((value) => (info = value))
      .catch(() => {});
    void loadUsage();
    void loadTargets();
    void loadCache();
    void loadJava();
  });

  async function loadUsage() {
    scanning = true;
    try {
      usage = await api.dirUsage();
    } catch (error) {
      showError("读取磁盘占用失败", "统计目录大小时出了错。", String(error));
    } finally {
      scanning = false;
    }
  }

  async function loadTargets() {
    try {
      targets = await api.cleanupTargets();
    } catch {
      targets = [];
    }
  }

  async function loadCache() {
    cacheScanning = true;
    try {
      cache = await api.cacheUsage();
    } catch (error) {
      showError("读取缓存失败", "统计缓存大小时出了错。", String(error));
    } finally {
      cacheScanning = false;
    }
  }

  /// 第一次点把按钮撑大、变红、改成「真的要清理吗？」；再点一次才删。
  /// 4 秒内没点第二次就复原，和上面「清理」卡一致。
  async function clearCacheGroup(key: string | null) {
    const id = key ?? "all";
    if (cachePending !== id) {
      cachePending = id;
      setTimeout(() => {
        if (cachePending === id) cachePending = null;
      }, 4000);
      return;
    }
    cachePending = null;
    try {
      const done = await api.clearCache(key ?? undefined);
      const tail = done.skipped > 0 ? `，跳过 ${done.skipped} 项正在使用` : "";
      announce(`已清理 ${size(done.freed)}${tail}`);
      await Promise.all([loadCache(), loadUsage()]);
    } catch (error) {
      showError("清理失败", "有文件没删掉，可能正被占用。", String(error));
    }
  }

  function cachePercent(bytes: number): number {
    const total = cache?.totalBytes ?? 0;
    return total === 0 ? 0 : Math.max(0.5, (bytes / total) * 100);
  }

  /// 第一次点只是把按钮变成「确认」，再点一次才真删——比系统确认框稳。
  async function clean(key: string) {
    if (pending !== key) {
      pending = key;
      setTimeout(() => {
        if (pending === key) pending = null;
      }, 4000);
      return;
    }
    pending = null;
    try {
      const freed = await api.cleanupRun(key);
      announce(`已清理 ${size(freed)}`);
      await Promise.all([loadTargets(), loadUsage()]);
    } catch (error) {
      showError("清理失败", "有文件没删掉，可能正被占用。", String(error));
    }
  }

  async function loadJava(force = false) {
    detecting = true;
    try {
      javas = (await cachedJava(force)).value;
    } catch {
      javas = [];
    } finally {
      detecting = false;
    }
  }

  function size(bytes: number): string {
    if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
    if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
    if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
    return `${bytes} B`;
  }

  function percent(bytes: number): number {
    if (!maxBytes) return 0;
    return Math.max(2, Math.round((bytes / maxBytes) * 100));
  }

  /// `Java 17 · 17.0.20 · 64 位 · JRE`：位数和 JRE 是给选 Java 的人看的，
  /// 32 位或 JRE 都可能让整合包装不上/起不来。
  function javaLabel(java: JavaInstallation): string {
    const bits = java.is64 ? "64 位" : "32 位";
    const kind = java.isJre ? "JRE" : "JDK";
    return `Java ${java.major} · ${java.version} · ${bits} · ${kind}`;
  }

  function cleanPath(path: string): string {
    return path.replace(/^\\\\\?\\/, "");
  }

  async function open(path: string | undefined) {
    if (!path) return;
    try {
      await api.openFolder(path);
    } catch (error) {
      showError("打不开目录", "系统没有接受这次打开请求。", String(error));
    }
  }

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      announce("已复制");
    } catch {
      showError("复制失败", "系统剪贴板没有接受这次写入。");
    }
  }
  import BackButton from "$lib/components/BackButton.svelte";
  import Switch from "$lib/components/Switch.svelte";
  import { registerDetailBack } from "$lib/page-history.svelte";
  import type { Settings } from "$lib/api";

  type CardId = "disk" | "cache" | "clean" | "java";
  type CategoryId = "pinned" | "storage" | "runtime";

  const CATEGORIES: { id: CategoryId; title: string }[] = [
    { id: "pinned", title: "便捷功能" },
    { id: "storage", title: "存储" },
    { id: "runtime", title: "运行环境" },
      ];
  /// 卡片留在自己的分类里，便捷功能只是多显示一份。
  const CARD_CATEGORY: Record<CardId, CategoryId> = {
    disk: "storage",
    cache: "storage",
    clean: "storage",
    java: "runtime",
  };

  /// 每次进页面都从便捷功能开始，不记上次看的分类。
  let view = $state<CategoryId>("pinned");
  let settings = $state<Settings | null>(null);
  const pins = $derived(settings?.toolboxPins ?? []);

  /// 返回键先回便捷功能，再回主页。
  $effect(() => {
    registerDetailBack(() => {
      if (view === "pinned") return false;
      view = "pinned";
      return true;
    });
    return () => registerDetailBack(null);
  });

  function show(id: CardId): boolean {
    if (view === "pinned") return pins.includes(id);
    return CARD_CATEGORY[id] === view;
  }

  async function togglePin(id: CardId, on: boolean) {
    if (!settings) return;
    const next = on
      ? [...pins.filter((item) => item !== id), id]
      : pins.filter((item) => item !== id);
    try {
      settings = await api.saveSettings({ ...settings, toolboxPins: next });
    } catch (error) {
      showError("保存失败", "开关状态没有写进配置文件。", String(error));
    }
  }
</script>

<div class="shell">
<div class="page-head">
  <BackButton />
<h1>工具</h1>
</div>

<div class="layout">
  <aside class="rail">
    {#each CATEGORIES as category (category.id)}
      <button
        class="rail-item"
        class:active={view === category.id}
        onclick={() => (view = category.id)}
      >
        {category.title}
      </button>
    {/each}
  </aside>

  <div class="cards">
    {#if view === "pinned" && pins.length === 0}
      <p class="dim empty">便捷功能里现在还没有卡片。请前往其他分类里添加。</p>
    {/if}
    {#if show("disk")}
    <section class="panel">
  <header class="panel-head">
    <div>
      <div class="title-row"><h2>磁盘占用</h2><Switch checked={pins.includes("disk")} label={pins.includes("disk") ? "从便捷功能移除" : "放入便捷功能"} onchange={(value) => void togglePin("disk", value)} /></div>
      <p class="dim small">游戏目录：<code>{info?.gameDir ?? "…"}</code></p>
    </div>
    <button class="btn" onclick={() => void loadUsage()} disabled={scanning}>
      {scanning ? "统计中…" : "重新统计"}
    </button>
  </header>

  {#if scanning && usage.length === 0}
    <p class="dim">正在统计…</p>
  {:else if usage.length === 0}
    <p class="dim">这个游戏目录还是空的。</p>
  {:else}
    <ul class="usage">
      {#each usage as entry (entry.name)}
        <li class="usage-row">
          <span class="usage-name">{entry.name}</span>
          <span class="usage-bar"><span class="fill" style:width={`${percent(entry.bytes)}%`}></span></span>
          <span class="usage-size">{size(entry.bytes)}</span>
          <span class="usage-files">{entry.files} 个文件</span>
        </li>
      {/each}
    </ul>
    <p class="dim small total">
      合计 {size(totalBytes)} · {usage.reduce((sum, entry) => sum + entry.files, 0)} 个文件（不含符号链接）
    </p>
  {/if}
</section>
    {/if}
    {#if show("clean")}
    <section class="panel">
  <header class="panel-head">
    <div>
      <div class="title-row"><h2>清理</h2><Switch checked={pins.includes("clean")} label={pins.includes("clean") ? "从便捷功能移除" : "放入便捷功能"} onchange={(value) => void togglePin("clean", value)} /></div>
      <p class="dim small">删掉的是缓存和残留，不碰存档、模组、配置。删「下载缓存」后下次启动游戏会重新下载。</p>
    </div>
    <button class="btn" onclick={() => void loadTargets()}>重新扫描</button>
  </header>

  {#if targets.length === 0}
    <p class="dim">没有可清理的东西，挺干净的。</p>
  {:else}
    <ul class="clean-list">
      {#each targets as target (target.key)}
        <li class="clean-row">
          <div class="clean-main">
            <span class="clean-title">{target.title}</span>
            <span class="clean-hint">{target.hint}</span>
            <code class="clean-path" title={target.path}>{cleanPath(target.path)}</code>
          </div>
          <span class="clean-size">{size(target.bytes)}</span>
          <span class="clean-files">{target.files} 个文件</span>
          <button
            class="btn tiny"
            class:danger={pending === target.key}
            onclick={() => void clean(target.key)}
          >
            {pending === target.key ? "确认清理" : "清理"}
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</section>
    {/if}
    {#if show("cache")}
    <section class="panel">
  <header class="panel-head">
    <div>
      <div class="title-row"><h2>缓存</h2><Switch checked={pins.includes("cache")} label={pins.includes("cache") ? "从便捷功能移除" : "放入便捷功能"} onchange={(value) => void togglePin("cache", value)} /></div>
      <p class="dim small">
        安装时下载的原版底稿和加载器外壳留在这里。清掉不影响已装版本，只是重装时要重新下载。
      </p>
    </div>
    <button class="btn" onclick={() => void loadCache()} disabled={cacheScanning}>
      {cacheScanning ? "统计中…" : "重新统计"}
    </button>
  </header>

  {#if cache === null}
    <p class="dim">正在统计…</p>
  {:else}
    <ul class="cache-list">
      {#each cache.groups as group (group.key)}
        <li class="cache-row">
          <span class="cache-name">{group.title}</span>
          <span class="cache-count">{group.count} 个</span>
          <span class="usage-bar">
            <span class="fill" style:width={`${cachePercent(group.bytes)}%`}></span>
          </span>
          <span class="usage-size">{size(group.bytes)}</span>
          <button
            class="btn tiny"
            class:confirm={cachePending === group.key}
            disabled={group.count === 0}
            onclick={() => void clearCacheGroup(group.key)}
          >
            {cachePending === group.key ? "真的要清理吗？" : "清理"}
          </button>
        </li>
      {/each}
    </ul>
    <p class="dim small total">
      合计 {size(cache.totalBytes)} · {cache.totalFiles} 个文件 · <code>{cache.path}</code>
    </p>
    <div class="actions">
      <button class="btn" onclick={() => void open(cache?.path)}>打开缓存目录</button>
      <button
        class="btn"
        class:confirm={cachePending === "all"}
        disabled={cache.totalBytes === 0}
        onclick={() => void clearCacheGroup(null)}
      >
        {cachePending === "all" ? "真的要清理吗？" : "全部清理"}
      </button>
    </div>
  {/if}
</section>
    {/if}
    {#if show("java")}
    <section class="panel">
  <header class="panel-head">
    <div>
      <div class="title-row"><h2>Java 环境</h2><Switch checked={pins.includes("java")} label={pins.includes("java") ? "从便捷功能移除" : "放入便捷功能"} onchange={(value) => void togglePin("java", value)} /></div>
      <p class="dim small">自动扫描 JAVA_HOME、PATH、常见安装目录和官方启动器的 runtime。</p>
    </div>
    <button class="btn" onclick={() => void loadJava(true)} disabled={detecting}>
      {detecting ? "检测中…" : "重新检测"}
    </button>
  </header>

  {#if detecting && javas.length === 0}
    <p class="dim">正在检测…</p>
  {:else if javas.length === 0}
    <p class="dim">没找到 Java。装一个 JDK 21 或 25 之后回来点「重新检测」。</p>
  {:else}
    <ul class="java-list">
      {#each javas as java (java.path)}
        <li class="java-row">
          <span class="java-ver">{javaLabel(java)}</span>
          <code class="java-path" title={cleanPath(java.path)}>{cleanPath(java.path)}</code>
          <button class="btn tiny" onclick={() => void copy(cleanPath(java.path))}>复制路径</button>
        </li>
      {/each}
    </ul>
  {/if}
</section>
    {/if}
  </div>
</div>
</div>

<style>
  .shell {
    display: flex;
    flex-direction: column;
    gap: 14px;
    height: 100%;
    min-height: 0;
    overflow: hidden;
  }
  .layout {
    display: flex;
    gap: 16px;
    align-items: stretch;
    flex: 1;
    min-height: 0;
  }
  .rail {
    flex: none;
    width: 168px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow-y: auto;
  }
  .rail-item {
    text-align: left;
    padding: 9px 12px;
    border: 1px solid transparent;
    border-radius: 10px;
    background: transparent;
    color: var(--text);
    font: inherit;
    cursor: pointer;
  }
  .rail-item:hover {
    background: var(--tint-1);
  }
  .rail-item.active {
    background: rgb(120 200 120 / 0.2);
    box-shadow: inset 3px 0 0 var(--yb-green-bright);
  }
  .cards {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow-y: auto;
    padding-right: 4px;
  }
  .title-row {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .empty {
    padding: 18px 2px;
  }
  .page-head {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 14px;
  }
  .page-head h1 {
    margin: 0;
    font-size: 24px;
  }
  .dim {
    color: var(--text-dim);
    margin: 0;
  }
  .small {
    font-size: 12.5px;
  }
  code {
    font-family: var(--mono, ui-monospace, "Cascadia Mono", Consolas, monospace);
    font-size: 12.5px;
  }
  .panel {
    padding: 16px 18px;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--shell-card, rgba(16, 19, 26, 0.5));
    margin-bottom: 14px;
  }
  .panel-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 12px;
  }
  .panel-head h2 {
    margin: 0 0 4px;
    font-size: 16px;
  }
  /* 磁盘占用 */
  .usage {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 8px;
  }
  .usage-row {
    display: grid;
    grid-template-columns: 130px 1fr 90px 96px;
    align-items: center;
    gap: 12px;
    font-size: 13px;
  }
  .usage-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .usage-bar {
    height: 8px;
    border-radius: 999px;
    background: var(--tint-2);
    overflow: hidden;
  }
  .usage-bar .fill {
    display: block;
    height: 100%;
    border-radius: 999px;
    background: linear-gradient(90deg, var(--yb-green), var(--yb-green-bright));
  }
  .usage-size {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .usage-files {
    color: var(--text-dim);
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .total {
    margin-top: 10px;
  }
  /* Java */
  .clean-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .clean-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 92px 84px 96px;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--tint-1);
  }

  .clean-main {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .clean-title {
    font-size: 13.5px;
    font-weight: 600;
  }

  .clean-hint {
    font-size: 12px;
    color: var(--text-dim);
  }

  .clean-path {
    font-size: 11.5px;
    color: var(--text-dim);
    opacity: 0.8;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .clean-size {
    font-variant-numeric: tabular-nums;
    text-align: right;
    font-size: 13px;
  }

  .clean-files {
    font-size: 12px;
    color: var(--text-dim);
    text-align: right;
  }

  .btn.danger {
    border-color: rgba(168, 59, 43, 0.9);
    background: rgba(168, 59, 43, 0.28);
    color: var(--danger-text);
  }

  .java-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 6px;
  }
  .java-row {
    display: grid;
    grid-template-columns: 190px 1fr auto;
    align-items: center;
    gap: 12px;
    font-size: 13px;
  }
  .java-ver {
    color: var(--text-dim);
  }
  .java-path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
  }
  .btn.tiny {
    padding: 4px 10px;
    font-size: 12px;
  }
  /* 缓存 */
  .cache-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 0 0 10px;
    padding: 0;
    list-style: none;
  }
  .cache-row {
    display: grid;
    grid-template-columns: 168px 64px 1fr 92px 132px;
    align-items: center;
    gap: 12px;
    font-size: 13px;
    padding: 5px 2px;
  }
  .cache-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cache-count {
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  .cache-row .usage-bar {
    height: 8px;
  }
  .btn.tiny.confirm,
  .btn.confirm {
    padding: 8px 18px;
    font-size: 13px;
    background: rgba(178, 58, 46, 0.92);
    border-color: rgba(214, 84, 70, 0.95);
    color: #fff;
    transition: padding 0.15s ease, background 0.15s ease;
  }
</style>
