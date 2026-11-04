<script lang="ts">
  // 工具：磁盘占用、清理、Java 环境、快捷入口、启动器信息。除了清理不改任何东西。
  import { onMount } from "svelte";
  import { announce } from "$lib/tasks.svelte";
  import { showError } from "$lib/dialogs.svelte";
  import { api } from "$lib/api";
  import type { CleanTarget, JavaInstallation, LauncherInfo, UsageEntry } from "$lib/api";
  import { cachedJava } from "$lib/detect";
  
  let info = $state<LauncherInfo | null>(null);
  let usage = $state<UsageEntry[]>([]);
  let targets = $state<CleanTarget[]>([]);
  let pending = $state<string | null>(null);
  let javas = $state<JavaInstallation[]>([]);
  let scanning = $state(true);
  let detecting = $state(false);

  const totalBytes = $derived(usage.reduce((sum, entry) => sum + entry.bytes, 0));
  const maxBytes = $derived(usage.reduce((max, entry) => Math.max(max, entry.bytes), 0));

  onMount(() => {
    void api
      .info()
      .then((value) => (info = value))
      .catch(() => {});
    void loadUsage();
    void loadTargets();
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

  function javaLabel(java: JavaInstallation): string {
    return `Java ${java.major} · ${java.version}`;
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
</script>

<div class="page-head">
  <BackButton />
<h1>工具</h1>
  <p class="dim">磁盘占用、Java 环境、常用目录都在这里。改配置请去「设置」或版本的「设置」页。</p>
</div>

<section class="panel">
  <header class="panel-head">
    <div>
      <h2>磁盘占用</h2>
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

<section class="panel">
  <header class="panel-head">
    <div>
      <h2>清理</h2>
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

<section class="panel">
  <header class="panel-head">
    <div>
      <h2>Java 环境</h2>
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

<section class="panel">
  <header class="panel-head">
    <div>
      <h2>快捷入口</h2>
      <p class="dim small">在文件管理器里打开对应目录。</p>
    </div>
  </header>
  <div class="actions">
    <button class="btn" onclick={() => void open(info?.dataDir)}>打开数据目录</button>
    <button class="btn" onclick={() => void open(info?.gameDir)}>打开游戏目录</button>
    <button class="btn" onclick={() => void open(info?.dataDir ? `${info.dataDir}/logs` : undefined)}>
      打开日志目录
    </button>
    <button class="btn" onclick={() => void open(info?.gameDir ? `${info.gameDir}/versions` : undefined)}>
      打开 versions
    </button>
  </div>
</section>

<section class="panel">
  <header class="panel-head">
    <div>
      <h2>启动器信息</h2>
      <p class="dim small">反馈问题时把这一段一起发出来。</p>
    </div>
    <button
      class="btn"
      onclick={() =>
        void copy(
          [
            `YBUMCL ${info?.version ?? ""}`,
            `模式：${info?.portable ? "便携" : "常规"}`,
            `exe：${info?.exeDir ?? ""}`,
            `数据目录：${info?.dataDir ?? ""}`,
            `游戏目录：${info?.gameDir ?? ""}`
          ].join("\n")
        )}
    >
      复制信息
    </button>
  </header>
  <dl class="kv">
    <div><dt>版本</dt><dd>{info?.version ?? "…"}</dd></div>
    <div><dt>模式</dt><dd>{info ? (info.portable ? "便携（数据在 exe 旁）" : "常规") : "…"}</dd></div>
    <div><dt>数据目录</dt><dd title={info?.dataDir}>{info?.dataDir ?? "…"}</dd></div>
    <div><dt>游戏目录</dt><dd title={info?.gameDir}>{info?.gameDir ?? "…"}</dd></div>
  </dl>
</section>

<style>
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
    background: rgba(16, 19, 26, 0.5);
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
    background: rgba(255, 255, 255, 0.07);
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
    background: rgba(255, 255, 255, 0.03);
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
    color: #ffd9d3;
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
  /* 启动器信息 */
  .kv {
    margin: 0;
    display: grid;
    gap: 6px;
    max-width: 760px;
  }
  .kv > div {
    display: flex;
    align-items: baseline;
    gap: 12px;
    min-width: 0;
  }
  .kv dt {
    flex: none;
    width: 84px;
    font-size: 12.5px;
    color: var(--text-dim);
  }
  .kv dd {
    margin: 0;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
  }
</style>
