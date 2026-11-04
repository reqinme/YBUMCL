<script lang="ts">
  import {
    tasks,
    clearFinished,
    continueTask,
    markStop,
    removeTask,
    type Task
  } from "$lib/tasks.svelte";
  import { api } from "$lib/api";
  import { showError } from "$lib/dialogs.svelte";
  import { forgetScan } from "$lib/detect";
  import ConfirmDialog from "$lib/components/ConfirmDialog.svelte";

  /// 由下载页传进来：点返回或按 Esc 时收回这一层。
  let { onback }: { onback: () => void } = $props();

  const labels: Record<string, string> = {
    queued: "排队中",
    running: "进行中",
    paused: "已暂停",
    done: "已完成",
    failed: "失败"
  };

  function bytes(value: number): string {
    if (!Number.isFinite(value) || value <= 0) return "0 B";
    const units = ["B", "KB", "MB", "GB"];
    let size = value;
    let unit = 0;
    while (size >= 1024 && unit < units.length - 1) {
      size /= 1024;
      unit += 1;
    }
    return `${size < 10 && unit > 0 ? size.toFixed(1) : Math.round(size)} ${units[unit]}`;
  }

  function time(value: number | null): string {
    if (!value) return "";
    const date = new Date(value);
    return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}:${String(
      date.getSeconds()
    ).padStart(2, "0")}`;
  }

  /// 正在跑的安装任务可以从这里停：暂停保留断点，取消清掉半成品。
  let stopping = $state<"pause" | "cancel" | null>(null);

  async function stop(mode: "pause" | "cancel") {
    stopping = mode;
    // 页面那一侧负责把「暂停 / 取消」写进任务卡文案，它只认这个标记。
    markStop(mode);
    try {
      await api.cancelInstall(mode);
    } finally {
      stopping = null;
    }
  }

  const finished = $derived(tasks.filter((task) => task.status === "done" || task.status === "failed").length);

  /// 产出已经被删掉（或者用户在资源管理器里自己删了）的任务：只留「移除」。
  let gone = $state<Record<number, boolean>>({});

  $effect(() => {
    for (const task of tasks) {
      if (!task.output || task.id in gone) continue;
      const id = task.id;
      const path = task.output.path;
      void api
        .pathExists(path)
        .then((exists) => {
          if (!exists) gone[id] = true;
        })
        .catch(() => {});
    }
  });

  /// 在资源管理器里选中这次任务的产出（文件选文件，目录选中目录本身）。
  async function reveal(task: Task) {
    if (!task.output) return;
    try {
      await api.revealPath(task.output.path);
    } catch (error) {
      showError("打不开", `「${task.title}」的产出找不到了。`, String(error));
    }
  }

  /// 等确认的那条任务；确认前不动磁盘。
  let pending = $state<Task | null>(null);

  const pendingMessage = $derived.by(() => {
    const task = pending;
    if (!task?.output) return "";
    const what = task.output.kind === "dir" ? "整个文件夹" : "这个文件";
    return `删除${what} ${task.output.path}？删掉的东西找不回来，这条记录也会一起清掉。`;
  });

  async function removeWithFiles() {
    const task = pending;
    pending = null;
    if (!task?.output) return;
    try {
      await api.deleteOutput(task.output.path);
      // 删掉的可能是一个实例：版本页那一分钟的扫描缓存得作废。
      forgetScan();
      removeTask(task.id);
    } catch (error) {
      showError("删除失败", `「${task.title}」的产出没删掉。`, String(error));
    }
  }
</script>

<div class="tasks-panel">
  <header class="page-head">
    <div class="head-main">
      <button class="back" onclick={onback} title="返回下载页（Esc）" aria-label="返回">
        <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M15 5 8 12l7 7" />
        </svg>
      </button>
      <div>
        <h1>下载任务</h1>
        <p class="dim small">下载在后台进行，完成后这里会留一条记录；失败的任务会写明原因。</p>
      </div>
    </div>
    <button class="btn" onclick={clearFinished} disabled={finished === 0}>清除已完成</button>
  </header>

  {#if tasks.length === 0}
    <section class="panel empty">
      <p class="dim">还没有任务</p>
    </section>
  {:else}
    <ul class="task-list">
      {#each tasks as task (task.id)}
        <li class="panel task" class:failed={task.status === "failed"}>
          <div class="task-head">
            <span class="status {task.status}">{labels[task.status] ?? task.status}</span>
            <span class="title">{task.title}</span>
            <span class="dim small">{time(task.startedAt)}</span>
          </div>
          {#if task.detail}
            <p class="dim small detail">{task.detail}</p>
          {/if}
          {#if task.live}
            <div class="live">
              <div class="bar"><span style:width={`${task.live.percent}%`}></span></div>
              <div class="live-meta">
                <span>{task.live.stage}</span>
                {#if task.live.detail}
                  <span class="dim">{task.live.detail}</span>
                {/if}
              </div>
              {#if task.live.path}
                <p class="path">{task.live.path}</p>
              {/if}
            </div>
          {:else if task.status === "running"}
            <div class="live">
              <div class="bar" class:indeterminate={task.total === null}>
                <span style:width={task.total ? `${task.percent}%` : "40%"}></span>
              </div>
              {#if task.note || task.total !== null}
                <div class="live-meta">
                  {#if task.note}
                    <span>{task.note}</span>
                  {/if}
                  {#if task.total !== null}
                    <span class="dim">{bytes(task.received)} / {bytes(task.total)}</span>
                  {/if}
                </div>
              {/if}
            </div>
          {/if}
          {#if task.note && task.status !== "running"}
            <p class="note" class:error={task.status === "failed"}>{task.note}</p>
          {/if}
          {#if task.status === "running"}
            <div class="task-actions">
              <button class="btn" onclick={() => stop("pause")} disabled={stopping !== null}>暂停</button>
              <button class="btn" onclick={() => stop("cancel")} disabled={stopping !== null}>取消</button>
            </div>
          {/if}
          {#if task.status === "paused"}
            <div class="task-actions">
              <button class="btn primary" onclick={() => continueTask(task.id)}>继续</button>
              <button class="btn" onclick={() => removeTask(task.id)}>移除</button>
            </div>
          {/if}
          {#if task.status === "done" || task.status === "failed"}
            <div class="task-actions">
              {#if task.output && !gone[task.id]}
                <button class="btn" onclick={() => reveal(task)}>打开文件夹</button>
              {/if}
              <button class="btn" onclick={() => removeTask(task.id)}>移除</button>
              {#if task.output && !gone[task.id]}
                <button class="btn danger" onclick={() => (pending = task)}>移除并删除文件</button>
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<ConfirmDialog
  open={pending !== null}
  message={pendingMessage}
  confirmLabel="删除"
  oncancel={() => (pending = null)}
  onconfirm={removeWithFiles}
/>

<style>
  .tasks-panel {
    display: flex;
    flex-direction: column;
    gap: 14px;
    height: 100%;
    min-height: 0;
  }
  .page-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
  }
  .head-main {
    display: flex;
    align-items: flex-start;
    gap: 10px;
  }
  .page-head h1 {
    margin: 0 0 4px;
    font-size: 22px;
  }
  .back {
    flex: none;
    margin-top: 6px;
    width: 30px;
    height: 30px;
    display: grid;
    place-items: center;
    border-radius: 9px;
    border: 1px solid var(--yb-line, rgba(255, 255, 255, 0.16));
    background: rgba(255, 255, 255, 0.06);
    color: inherit;
    cursor: pointer;
  }
  .back:hover {
    background: rgba(255, 255, 255, 0.12);
  }
  .empty {
    padding: 18px;
  }
  .task-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
    overflow-y: auto;
    min-height: 0;
  }
  .task {
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .task.failed {
    border-color: color-mix(in srgb, #e4635a 45%, transparent);
  }
  .task-head {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .title {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .status {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.1);
    flex: none;
  }
  .status.running {
    background: color-mix(in srgb, var(--yb-green) 30%, transparent);
  }
  .status.done {
    opacity: 0.6;
  }
  .status.failed {
    background: color-mix(in srgb, #e4635a 32%, transparent);
  }
  .status.paused {
    background: color-mix(in srgb, #d8a657 32%, transparent);
    color: #f0d9a8;
  }
  .detail,
  .note {
    margin: 0;
    word-break: break-all;
  }
  /* 安装任务的实时进度：进度条 + 阶段 + 明细 + 当前文件。 */
  .live {
    display: flex;
    flex-direction: column;
    gap: 5px;
    margin-top: 2px;
  }
  .live .bar {
    height: 6px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.1);
    overflow: hidden;
  }
  .live .bar span {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--yb-green, #7bd88f);
    transition: width 0.25s ease;
  }
  .live .bar.indeterminate span {
    width: 40% !important;
    animation: sweep 1.2s ease-in-out infinite;
  }
  @keyframes sweep {
    0% {
      transform: translateX(-100%);
    }
    100% {
      transform: translateX(250%);
    }
  }
  .live-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
    font-size: 12px;
  }
  .path {
    margin: 0;
    font-size: 11px;
    opacity: 0.55;
    word-break: break-all;
  }
  .note.error {
    color: #f1a19a;
  }
  .task-actions {
    display: flex;
    justify-content: flex-end;
  }
</style>
