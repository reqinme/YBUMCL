<script lang="ts">
  import Dropdown from "$lib/components/Dropdown.svelte";
  // 日志页：启动器日志（launcher.log）与游戏日志（versions/<id>/logs/latest.log）。
  // 只做只读展示：搜索、按级别过滤、自动贴底、换行开关。
  import { onMount } from "svelte";
  import { page } from "$app/state";
  import { api, type VersionView } from "$lib/api";
  import { announce } from "$lib/tasks.svelte";
  import { showError } from "$lib/dialogs.svelte";

  type Tab = "launcher" | "game";
  type Level = "all" | "warn" | "error";

  // 错误弹窗的「打开日志」会带 ?tab=game 跳过来，默认看启动器日志。
  const wanted = page.url.searchParams.get("tab");
  let tab = $state<Tab>(wanted === "game" ? "game" : "launcher");
  let versions = $state<VersionView[]>([]);
  let gameDir = $state("");
  let launcherPath = $state("");
  let text = $state("");
  let loading = $state(false);

  let query = $state("");
  let level = $state<Level>("all");
  let wrap = $state(true);
  let follow = $state(true);

  let box: HTMLDivElement | undefined = $state();

  const lines = $derived(text ? text.split("\n") : []);

  function levelOf(line: string): "error" | "warn" | "info" {
    if (/\b(ERROR|SEVERE|FATAL)\b|Exception|Error:/i.test(line)) return "error";
    if (/\bWARN(ING)?\b/i.test(line)) return "warn";
    return "info";
  }

  const filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return lines.filter((line) => {
      if (!line) return false;
      if (q && !line.toLowerCase().includes(q)) return false;
      if (level === "all") return true;
      const kind = levelOf(line);
      return level === "error" ? kind === "error" : kind !== "info";
    });
  });

  const counts = $derived({
    total: lines.filter((line) => line.length > 0).length,
    shown: filtered.length,
    errors: lines.filter((line) => line && levelOf(line) === "error").length,
  });

  onMount(async () => {
    try {
      const [list, path] = await Promise.all([api.listVersions(), api.launcherLogPath()]);
      versions = list;
      launcherPath = path;
      gameDir = list[0]?.gameDirectory ?? "";
    } catch (e) {
      showError("读取日志位置失败", "版本列表或日志路径没拿到。", String(e));
    }
    await load();
  });

  function scrollToBottom() {
    requestAnimationFrame(() => {
      if (box) box.scrollTop = box.scrollHeight;
    });
  }

  async function load() {
    loading = true;
    try {
      text =
        tab === "launcher"
          ? await api.readLauncherLog(3000)
          : await api.readGameLog(gameDir, 3000);
      if (!text.trim()) {
        text =
          tab === "launcher"
            ? "（启动器日志还是空的。启动或安装一次游戏后再来看）"
            : "（这个版本目录里还没有 logs/latest.log）";
      }
      if (follow) scrollToBottom();
    } catch (e) {
      showError("读取日志失败", "日志文件可能被另一只手占着。", String(e));
      text = "";
    } finally {
      loading = false;
    }
  }

  async function copyAll() {
    try {
      await navigator.clipboard.writeText(filtered.join("\n"));
      announce("日志已复制");
    } catch (e) {
      showError("复制失败", "系统剪贴板没有接受这次写入。", String(e));
    }
  }

  function switchTab(next: Tab) {
    if (tab === next) return;
    tab = next;
    void load();
  }
</script>

<section class="logs">
  <header class="head">
    <div class="tabs">
      <button class="tab" class:active={tab === "launcher"} onclick={() => switchTab("launcher")}>
        启动器日志
      </button>
      <button class="tab" class:active={tab === "game"} onclick={() => switchTab("game")}>
        游戏日志
      </button>
    </div>
    <div class="actions">
      {#if tab === "game"}
        <Dropdown
          bind:value={gameDir}
          label="选择版本"
          onchange={load}
          options={versions.map((version) => ({
            value: version.gameDirectory,
            label: version.id,
            hint: version.kind
          }))}
        />
      {/if}
      <button class="btn sm" onclick={load} disabled={loading}>{loading ? "读取中…" : "刷新"}</button>
      <button class="btn sm" onclick={copyAll} disabled={!filtered.length}>复制</button>
    </div>
  </header>

  <div class="toolbar">
    <input class="search" bind:value={query} placeholder="搜索关键字…" spellcheck="false" />
    <div class="seg" role="group" aria-label="日志级别">
      <button class="seg-btn" class:active={level === "all"} onclick={() => (level = "all")}>全部</button>
      <button class="seg-btn" class:active={level === "warn"} onclick={() => (level = "warn")}>警告+</button>
      <button class="seg-btn" class:active={level === "error"} onclick={() => (level = "error")}>
        仅错误
      </button>
    </div>
    <label class="check"><input type="checkbox" bind:checked={wrap} />换行</label>
    <label class="check"><input type="checkbox" bind:checked={follow} />贴底</label>
    <span class="stats">
      共 {counts.total} 行{#if counts.shown !== counts.total}· 显示 {counts.shown}{/if}{#if counts.errors}· <b
          class="err-count">{counts.errors} 处错误</b
        >{/if}
    </span>
  </div>

  <p class="path" title={tab === "launcher" ? launcherPath : `${gameDir}\logs\latest.log`}>
    {tab === "launcher" ? launcherPath : `${gameDir}\logs\latest.log`}
  </p>

  <div class="box" class:nowrap={!wrap} bind:this={box}>
    {#if !filtered.length && !loading}
      <p class="empty">{query || level !== "all" ? "没有匹配的日志行" : "（暂无日志）"}</p>
    {/if}
    {#each filtered as line, index (index)}
      <p class="line {levelOf(line)}">
        {#if line.startsWith("[")}
          {@const close = line.indexOf("]")}
          <span class="stamp">{line.slice(0, close + 1)}</span>
          <span class="rest">{line.slice(close + 1)}</span>
        {:else}
          {line}
        {/if}
      </p>
    {/each}
  </div>
</section>

<style>
  .logs {
    display: flex;
    flex-direction: column;
    gap: 10px;
    height: 100%;
    min-height: 0;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }
  .tabs {
    display: inline-flex;
    gap: 4px;
    padding: 3px;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.06);
  }
  .tab {
    padding: 6px 14px;
    border-radius: 8px;
    font-size: 13px;
    color: var(--text-dim);
    border: none;
    background: none;
    cursor: pointer;
  }
  .tab.active {
    background: rgba(168, 204, 132, 0.18);
    color: var(--text);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .btn.sm {
    padding: 6px 12px;
    font-size: 12px;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    font-size: 12px;
    color: var(--text-dim);
  }
  .search {
    width: 200px;
    padding: 6px 10px;
    font-size: 12px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: rgba(8, 11, 16, 0.5);
    color: var(--text);
  }
  .search:focus {
    outline: none;
    border-color: rgba(168, 204, 132, 0.5);
  }
  .seg {
    display: inline-flex;
    gap: 2px;
    padding: 2px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.05);
  }
  .seg-btn {
    border: none;
    background: none;
    padding: 4px 10px;
    border-radius: 6px;
    font-size: 12px;
    color: var(--text-dim);
    cursor: pointer;
  }
  .seg-btn.active {
    background: rgba(255, 255, 255, 0.1);
    color: var(--text);
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    cursor: pointer;
    user-select: none;
  }
  .check input {
    accent-color: #8fbf63;
  }
  .stats {
    margin-left: auto;
    font-variant-numeric: tabular-nums;
  }
  .err-count {
    color: #e8a097;
    font-weight: 600;
  }

  .path {
    margin: 0;
    font-size: 11.5px;
    color: var(--text-dim);
    font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    -webkit-user-select: text;
    user-select: text;
  }
  .error {
    margin: 0;
    color: #f0b0a4;
    font-size: 12px;
  }

  .box {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 10px 12px;
    border-radius: var(--radius-md);
    background: rgba(5, 8, 12, 0.72);
    border: 1px solid rgba(255, 255, 255, 0.06);
    font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
    font-size: 12px;
    line-height: 1.6;
    -webkit-user-select: text;
    user-select: text;
  }
  .box.nowrap {
    white-space: nowrap;
  }
  .line {
    margin: 0;
    color: #c9d1de;
    padding: 1px 6px;
    border-radius: 4px;
    white-space: inherit;
    overflow-wrap: anywhere;
  }
  .line:hover {
    background: rgba(255, 255, 255, 0.05);
  }
  .line.warn {
    color: #e6cf9a;
  }
  .line.error {
    color: #f0a79b;
    background: rgba(200, 70, 55, 0.1);
  }
  .stamp {
    color: #7f8b9e;
    margin-right: 6px;
  }
  .empty {
    margin: 6px 4px;
    color: var(--text-dim);
  }
</style>
