<script lang="ts">
  // 目录浏览：地址栏（从 `root` 起算，出不去）+ 右侧 1/3 搜索框 + 文件列表 + 预览。
  // 日志、崩溃报告共用这一个组件，将来「文件」页也能直接复用。
  import { onDestroy, onMount, tick } from "svelte";
  import {
    api,
    onFileSearch,
    type EntryInfo,
    type FileHit,
  } from "$lib/api";
  import { showError } from "$lib/dialogs.svelte";

  let {
    root,
    rootName,
    onDiagnose,
    emptyText = "这个目录还是空的。",
  }: {
    root: string;
    rootName: string;
    /// 崩溃报告用：打开一份报告时让页面去做诊断。
    onDiagnose?: (name: string) => void;
    emptyText?: string;
  } = $props();

  const SEP = String.fromCharCode(92);
  /// 预览最多读这么多字节（尾部）。
  const PREVIEW_BYTES = 512 * 1024;
  /// 递归结果上限，后端也按这个数收口。
  const MAX_HITS = 500;

  /// 相对根目录的层级；`[]` 就是根目录本身。
  let rel = $state<string[]>([]);
  let entries = $state<EntryInfo[]>([]);
  let busy = $state(false);
  let preview = $state<{ name: string; body: string } | null>(null);
  let preEl = $state<HTMLPreElement | null>(null);

  let query = $state("");
  let searchOn = $state(false);
  let hits = $state<FileHit[]>([]);
  let scanned = $state(0);
  let scanning = $state(false);
  let token = "";

  let unlisten: (() => void) | undefined;

  const cwd = $derived([root, ...rel].join(SEP));
  const crumbs = $derived([rootName, ...rel]);

  async function load() {
    busy = true;
    try {
      entries = await api.listEntries(cwd);
    } catch (error) {
      entries = [];
      showError("读取目录失败", cwd, String(error));
    } finally {
      busy = false;
    }
  }

  /// 预览打开时落到末尾：日志最新的内容都在最后。
  $effect(() => {
    const node = preEl;
    const body = preview?.body;
    if (!node || body === undefined) return;
    void tick().then(() => {
      node.scrollTop = node.scrollHeight;
    });
  });

  function clearSearch() {    if (scanning) void api.stopSearch(token);
    scanning = false;
    searchOn = false;
    hits = [];
    scanned = 0;
  }

  async function enter(name: string) {
    clearSearch();
    rel = [...rel, name];
    await load();
  }

  async function jump(index: number) {
    if (index === rel.length) return;
    clearSearch();
    rel = rel.slice(0, index);
    await load();
  }

  async function stopSearch() {
    if (!scanning) return;
    scanning = false;
    await api.stopSearch(token).catch(() => {});
  }

  /// 回车才搜：先在当前目录里按文件名匹配，命中立刻列出来；
  /// 同时起一个后台递归，边找边追加。
  async function search() {
    const needle = query.trim().toLowerCase();
    if (!needle) {
      clearSearch();
      return;
    }
    await stopSearch();
    searchOn = true;
    const local = entries
      .filter((item) => !item.dir && item.name.toLowerCase().includes(needle))
      .map((item) => ({
        path: cwd + SEP + item.name,
        name: item.name,
        size: item.size,
        modifiedMs: item.modifiedMs,
      }));
    hits = local;
    scanned = entries.filter((item) => !item.dir).length;
    token = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
    scanning = true;
    try {
      await api.searchFiles(token, cwd, needle, MAX_HITS);
    } catch (error) {
      scanning = false;
      showError("搜索失败", cwd, String(error));
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === "Enter") {
      event.preventDefault();
      void search();
    } else if (event.key === "Escape" && searchOn) {
      query = "";
      clearSearch();
    }
  }

  function relative(path: string): string {
    return path.startsWith(cwd + SEP) ? path.slice(cwd.length + 1) : path;
  }

  async function openFile(path: string, name: string) {
    try {
      preview = { name, body: await api.readTextFile(path, PREVIEW_BYTES, true) };
      onDiagnose?.(name);
    } catch (error) {
      showError("打开文件失败", path, String(error));
    }
  }

  async function reveal() {
    try {
      await api.ensureDir(cwd);
      await api.revealPath(cwd);
    } catch (error) {
      showError("打开目录失败", cwd, String(error));
    }
  }

  function sizeText(bytes: number): string {
    if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
    if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
    return `${bytes} B`;
  }

  /// 换实例/换标签时回到根目录重读。
  let currentRoot = root;
  $effect(() => {
    const next = root;
    if (next === currentRoot) return;
    currentRoot = next;
    clearSearch();
    rel = [];
    void load();
  });

  onMount(async () => {
    unlisten = await onFileSearch((progress) => {
      if (progress.token !== token) return;
      if (progress.hits.length > 0) {
        // 当前目录的文件在发请求前就已经列出来了，递归结果要把这部分去掉，否则键重复。
        const seen = new Set(hits.map((hit) => hit.path));
        const fresh = progress.hits.filter((hit) => !seen.has(hit.path));
        if (fresh.length > 0) hits = [...hits, ...fresh].slice(0, MAX_HITS);
      }
      scanned = progress.scanned;
      if (progress.done) scanning = false;
    });
    await load();
  });

  onDestroy(() => {
    unlisten?.();
    if (scanning) void api.stopSearch(token).catch(() => {});
  });
</script>

<div class="browser">
  <div class="bar">
    <nav class="crumbs" aria-label="目录">
      {#each crumbs as crumb, index (index)}
        <button
          class="crumb"
          disabled={index === rel.length}
          onclick={() => void jump(index)}
        >
          {crumb}
        </button>
        {#if index < crumbs.length - 1}<span class="sep">›</span>{/if}
      {/each}
    </nav>
    <input
      class="search"
      bind:value={query}
      onkeydown={onKeydown}
      placeholder="搜索文件，回车开始"
      spellcheck="false"
    />
  </div>

  {#if searchOn}
    <div class="hits-head">
      <span class="dim">
        {scanning ? `正在搜索… 已扫 ${scanned} 个文件` : `共 ${hits.length} 个结果`}
      </span>
      {#if scanning}
        <button class="btn tiny" onclick={() => void stopSearch()}>停止</button>
      {:else}
        <button
          class="btn tiny"
          onclick={() => {
            query = "";
            clearSearch();
          }}
        >
          清除搜索
        </button>
      {/if}
    </div>
    {#if hits.length === 0 && !scanning}
      <p class="dim">没有找到匹配的文件。</p>
    {:else}
      <div class="files">
        {#each hits as hit (hit.path)}
          <button class="file" onclick={() => void openFile(hit.path, hit.name)}>
            <span class="mono">{relative(hit.path)}</span>
            <span class="dim">{sizeText(hit.size)}</span>
          </button>
        {/each}
      </div>
      {#if hits.length >= MAX_HITS}
        <p class="dim">只显示前 {MAX_HITS} 条，换个更具体的关键词再找。</p>
      {/if}
    {/if}
  {:else}
    {#if busy && entries.length === 0}
      <p class="dim">正在读取目录…</p>
    {:else if entries.length === 0}
      <p class="dim">{emptyText}</p>
    {:else}
      <div class="files">
        {#each entries as entry (entry.name)}
          <button
            class="file"
            onclick={() =>
              entry.dir
                ? void enter(entry.name)
                : void openFile(cwd + SEP + entry.name, entry.name)}
          >
            <span class="mono">{entry.name}{entry.dir ? SEP : ""}</span>
            <span class="dim">{entry.dir ? "文件夹" : sizeText(entry.size)}</span>
          </button>
        {/each}
      </div>
    {/if}
  {/if}

  <div class="row">
    <button class="btn" onclick={() => void reveal()}>打开目录</button>
  </div>

  {#if preview}
    <div class="textview">
      <header>
        <strong class="mono">{preview.name}</strong>
        <button class="btn tiny ghost" onclick={() => (preview = null)}>关闭</button>
      </header>
      <pre bind:this={preEl} class="mono">{preview.body}</pre>
    </div>
  {/if}
</div>

<style>
  .dim {
    color: var(--text-dim);
    font-size: 12px;
  }
  .mono {
    font-family: "JetBrains Mono", "Cascadia Mono", Consolas, monospace;
  }
  .btn.tiny {
    padding: 4px 10px;
    font-size: 12px;
  }
  .btn.ghost {
    margin-left: auto;
    background: none;
    border-color: transparent;
  }
  .browser {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .bar {
    display: grid;
    grid-template-columns: 2fr 1fr;
    gap: 10px;
    align-items: center;
  }
  .crumbs {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    overflow-x: auto;
    padding: 5px 8px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: rgba(8, 10, 14, 0.35);
  }
  .crumb {
    border: 0;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 13px;
    padding: 2px 4px;
    border-radius: 6px;
    white-space: nowrap;
    cursor: pointer;
  }
  .crumb:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.08);
  }
  .crumb:disabled {
    color: var(--text-dim);
    cursor: default;
  }
  .sep {
    color: var(--text-dim);
    font-size: 12px;
  }
  .search {
    width: 100%;
    min-width: 0;
    padding: 7px 10px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: rgba(8, 10, 14, 0.35);
    color: var(--text);
    font-size: 13px;
  }
  .hits-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    font-size: 12px;
  }
  .files {
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 52vh;
    overflow-y: auto;
  }
  .file {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    width: 100%;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: rgba(8, 10, 14, 0.35);
    color: var(--text);
    padding: 7px 12px;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }
  .file:hover {
    background: rgba(255, 255, 255, 0.06);
  }
  .row {
    display: flex;
    gap: 8px;
  }
  .textview {
    position: fixed;
    inset: 8% 12%;
    z-index: 40;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px 16px;
    border-radius: 14px;
    border: 1px solid var(--border);
    background: rgba(12, 15, 21, 0.96);
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.5);
  }
  .textview header {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .textview pre {
    flex: 1;
    min-height: 0;
    overflow: auto;
    margin: 0;
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-all;
    color: var(--text-dim);
    /* 主题在 body 上全局禁选，这里单独放开，好让日志能拖选复制。 */
    -webkit-user-select: text;
    user-select: text;
    cursor: text;
  }
</style>
