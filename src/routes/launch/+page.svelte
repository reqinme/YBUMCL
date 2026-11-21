<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { page } from "$app/state";
  import { goto } from "$app/navigation";
  import BackButton from "$lib/components/BackButton.svelte";
  import { save as saveDialog } from "@tauri-apps/plugin-dialog";
  import {
    api,
    onGameCrash,
    onGameExit,
    onGameLog,
    type CrashHit,
    type GameCrash,
    type LogLine,
    type RunView,
  } from "$lib/api";

  const versionId = $derived(page.url.searchParams.get("version") ?? "");
  const wantedRun = $derived(Number(page.url.searchParams.get("run") ?? "") || 0);

  let logs = $state<LogLine[]>([]);
  let running = $state(true);
  let exitCode = $state<number | null>(null);
  let pid = $state<number | null>(null);
  let stick = $state(true);
  /// 正在看的那一次启动。同一实例可以开多份，日志靠它分开。
  let runId = $state<number>(0);
  /// 这个实例的所有启动（含已结束的），用来切换。
  let runs = $state<RunView[]>([]);
  /// 这一份启动崩了之后，规则表给出的诊断。
  let crash = $state<{ hits: CrashHit[] } | null>(null);
  let exporting = $state(false);
  let exportNote = $state("");

  async function refreshRuns(): Promise<void> {
    const all = await api.listRuns().catch(() => [] as RunView[]);
    runs = all.filter((run) => run.versionId === versionId);
  }

  async function exportCrash(): Promise<void> {
    if (exporting) return;
    exporting = true;
    exportNote = "";
    try {
      const stamp = new Date()
        .toISOString()
        .replace(/[:T]/g, "-")
        .slice(0, 19);
      const target = await saveDialog({
        defaultPath: `错误报告-${versionId}-${stamp}.zip`,
        filters: [{ name: "zip", extensions: ["zip"] }],
      });
      if (!target) return;
      const path = await api.exportCrashReport(target, versionId);
      exportNote = `已导出：${path}`;
    } catch (error) {
      exportNote = `导出失败：${error}`;
    } finally {
      exporting = false;
    }
  }

  function pickRun(id: number): void {
    runId = id;
    crash = null;
    exportNote = "";
    const query = new URLSearchParams(page.url.searchParams);
    query.set("run", String(id));
    void goto(`?${query.toString()}`, { replaceState: true, keepFocus: true, noScroll: true });
  }

  let scroller = $state<HTMLDivElement | undefined>(undefined);
  let unlisten: Array<() => void> = [];
  let runsTimer: ReturnType<typeof setInterval> | null = null;

  function push(line: LogLine) {
    logs.push(line);
    if (logs.length > 2000) logs.splice(0, logs.length - 2000);
  }

  onMount(async () => {
    await refreshRuns();
    // 同一实例可能在看日志期间又开了一份，切页用的列表得跟着变。
    runsTimer = setInterval(() => {
      void refreshRuns();
    }, 2000);
    runId = wantedRun || runs.reduce((newest, run) => Math.max(newest, run.runId), 0);
    try {
      const snap = await api.gameSnapshot(versionId || undefined, runId || undefined);
      logs = snap.logs;
      running = snap.running;
      pid = snap.pid;
      exitCode = snap.lastExit;
    } catch {
      /* 刚打开页面时没有会话也无所谓 */
    }
    unlisten.push(
      await onGameLog((line) => {
        // 多开时只显示这一份启动的日志。
        if (runId !== 0) {
          if (line.runId !== runId) return;
        } else if (line.versionId && versionId && line.versionId !== versionId) {
          return;
        }
        push(line);
      })
    );
    unlisten.push(
      await onGameCrash((event: GameCrash) => {
        if (runId !== 0 && event.runId !== runId) return;
        if (runId === 0 && event.versionId !== versionId) return;
        crash = { hits: event.hits };
      })
    );
    unlisten.push(
      await onGameExit((event) => {
        void refreshRuns();
        if (runId !== 0 && event.runId !== runId) return;
        if (runId === 0 && event.versionId !== versionId) return;
        running = false;
        exitCode = event.code;
      })
    );
  });

  onDestroy(() => {
    for (const stop of unlisten) stop();
    if (runsTimer !== null) clearInterval(runsTimer);
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
    <BackButton />
    <div class="meta">
      <h1>游戏日志</h1>
      {#if versionId}<span class="dim">{versionId}</span>{/if}
      {#if runs.length > 1}
        <div class="runs">
          {#each runs as run, at (run.runId)}
            <button
              class="run-chip"
              class:on={run.runId === runId}
              title={`PID ${run.pid}`}
              onclick={() => pickRun(run.runId)}
            >
              第 {run.index} 份
            </button>
          {/each}
        </div>
      {/if}
    </div>
    <span class="state" class:on={running}>{statusText()}</span>
  </header>

  {#if crash}
    <div class="crash">
      <div class="crash-main">
        <strong>{crash.hits[0]?.title ?? "游戏异常退出"}</strong>
        {#if crash.hits[0]}
          <span class="crash-detail">{crash.hits[0].detail}</span>
        {/if}
        {#if crash.hits[0]?.fix}
          <span class="crash-fix">建议：{crash.hits[0].fix}</span>
        {/if}
        {#if crash.hits.length > 1}
          <span class="crash-more">另有 {crash.hits.length - 1} 条可能原因</span>
        {/if}
        {#if exportNote}<span class="crash-note">{exportNote}</span>{/if}
      </div>
      <div class="crash-act">
        <button class="b" onclick={exportCrash} disabled={exporting}>
          {exporting ? "导出中…" : "导出错误报告"}
        </button>
        <button class="b ghost" onclick={() => (crash = null)}>关闭</button>
      </div>
    </div>
  {/if}

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
  .crash {
    display: flex;
    gap: 16px;
    align-items: flex-start;
    justify-content: space-between;
    margin: 0 24px 12px;
    padding: 12px 16px;
    border: 1px solid color-mix(in srgb, #ff6a6a 45%, transparent);
    border-radius: 12px;
    background: color-mix(in srgb, #ff6a6a 12%, transparent);
  }
  .crash-main {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 13px;
    min-width: 0;
  }
  .crash-main strong {
    font-size: 14px;
    color: #ffb4b4;
  }
  .crash-detail {
    color: var(--dim);
  }
  .crash-fix {
    color: #ffd9a8;
  }
  .crash-more,
  .crash-note {
    color: var(--dim);
    font-size: 12px;
  }
  .crash-act {
    display: flex;
    gap: 8px;
    flex: none;
  }
  .crash-act .b {
    padding: 6px 12px;
    border-radius: 8px;
    border: 1px solid var(--line);
    background: color-mix(in srgb, #ff6a6a 22%, transparent);
    color: var(--fg);
    cursor: pointer;
  }
  .crash-act .b:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .crash-act .ghost {
    background: transparent;
  }

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
  /* 同一实例多开时，这里切换看哪一份。 */
  .runs {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: 4px;
  }
  .run-chip {
    padding: 2px 9px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-dim);
    font-size: 12px;
  }
  .run-chip.on {
    color: var(--accent);
    border-color: var(--border-strong);
    background: var(--surface);
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
