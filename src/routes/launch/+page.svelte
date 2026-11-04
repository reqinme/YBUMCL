<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { page } from "$app/state";
  import { goto } from "$app/navigation";
  import { api, onGameLog, onGameExit, type LogLine } from "$lib/api";

  const versionId = $derived(page.url.searchParams.get("version") ?? "");

  let logs = $state<LogLine[]>([]);
  let running = $state(true);
  let exitCode = $state<number | null>(null);
  let pid = $state<number | null>(null);
  let stick = $state(true);

  let scroller = $state<HTMLDivElement | undefined>(undefined);
  let unlisten: Array<() => void> = [];

  function push(line: LogLine) {
    logs.push(line);
    if (logs.length > 2000) logs.splice(0, logs.length - 2000);
  }

  onMount(async () => {
    try {
      const snap = await api.gameSnapshot();
      logs = snap.logs;
      running = snap.running;
      pid = snap.pid;
      exitCode = snap.lastExit;
    } catch {
      /* 刚打开页面时没有会话也无所谓 */
    }
    unlisten.push(
      await onGameLog((line) => {
        push(line);
      })
    );
    unlisten.push(
      await onGameExit((code) => {
        running = false;
        exitCode = code;
      })
    );
  });

  onDestroy(() => {
    for (const stop of unlisten) stop();
  });

  // 用户自己往上翻时就别再自动滚到底了。
  $effect(() => {
    if (stick && scroller) {
      scroller.scrollTop = scroller.scrollHeight;
    }
  });

  function statusText() {
    if (running) return `运行中${pid ? ` · PID ${pid}` : ""}`;
    if (exitCode === null) return "已结束";
    return exitCode === 0 ? "已正常退出" : `异常退出（码 ${exitCode}）`;
  }
</script>

<div class="launch">
  <header class="bar">
    <button class="btn" onclick={() => goto("/versions")}>← 返回</button>
    <div class="meta">
      <h1>游戏日志</h1>
      {#if versionId}<span class="dim">{versionId}</span>{/if}
    </div>
    <span class="state" class:on={running}>{statusText()}</span>
  </header>

  <div
    class="log"
    bind:this={scroller}
    onscroll={() => {
      if (!scroller) return;
      stick = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 24;
    }}
  >
    {#if logs.length === 0}
      <p class="waiting">等待游戏输出…</p>
    {/if}
    {#each logs as line, index (index)}
      <div class="row {line.stream}">
        <span class="tag">{line.stream === "stdout" ? "out" : line.stream === "stderr" ? "err" : "sys"}</span>
        <span class="text">{line.text}</span>
      </div>
    {/each}
  </div>
</div>

<style>
  .launch {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: rgba(8, 10, 14, 0.82);
    backdrop-filter: blur(10px);
  }
  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 12px var(--gutter);
    border-bottom: 1px solid var(--border);
  }
  .meta {
    display: flex;
    align-items: baseline;
    gap: 10px;
    flex: 1;
  }
  h1 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }
  .dim {
    color: var(--text-dim);
    font-size: 12px;
    -webkit-user-select: text;
    user-select: text;
  }
  .state {
    font-size: 12px;
    padding: 4px 10px;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--text-dim);
  }
  .state.on {
    color: var(--yb-green-bright);
    background: rgba(127, 163, 94, 0.16);
  }
  .log {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 12px var(--gutter) 24px;
    font-family: "Cascadia Mono", Consolas, ui-monospace, monospace;
    font-size: 12px;
    line-height: 1.65;
    -webkit-user-select: text;
    user-select: text;
  }
  .row {
    display: flex;
    gap: 10px;
    white-space: pre-wrap;
    word-break: break-word;
    color: rgba(255, 255, 255, 0.82);
  }
  .tag {
    flex: none;
    width: 26px;
    color: var(--text-dim);
    user-select: none;
  }
  .row.stderr .text {
    color: #e8a3a3;
  }
  .row.system .text {
    color: var(--yb-green-bright);
  }
  .waiting {
    color: var(--text-dim);
  }
</style>
