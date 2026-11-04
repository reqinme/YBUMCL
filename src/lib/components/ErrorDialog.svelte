<script lang="ts">
  import { goto } from "$app/navigation";
  import { api } from "$lib/api";
  import { closeError, errorDialog } from "$lib/dialogs.svelte";
  import { announce } from "$lib/tasks.svelte";

  let busy = $state(false);

  const info = $derived(errorDialog.value?.info ?? null);
  /// 复制只复制真正有用的部分：人话 + 原始错误串。
  const plain = $derived.by(() => {
    if (!info) return "";
    return [info.title, info.message, info.detail].filter(Boolean).join("\n\n");
  });

  async function copyReport() {
    if (!info) return;
    try {
      await navigator.clipboard.writeText(plain);
      announce("已复制错误信息");
      closeError();
    } catch (error) {
      announce(`复制失败：${String(error)}`);
    }
  }

  /// 启动类错误看游戏日志，其余看启动器日志。
  function openLogs() {
    const tab = info?.source === "game" ? "game" : "launcher";
    closeError();
    void goto(`/logs?tab=${tab}`);
  }

  async function exportReport() {
    if (!info || busy) return;
    busy = true;
    try {
      const launcher = await api.info().catch(() => null);
      const path = await api.exportErrorReport({
        title: info.title,
        message: info.message ?? "",
        detail: info.detail ?? "",
        source: info.source ?? "launcher",
        gameDir: launcher?.gameDir ?? null
      });
      if (path) {
        announce("错误报告已导出");
        closeError();
      }
    } catch (error) {
      announce(`导出失败：${String(error)}`);
    } finally {
      busy = false;
    }
  }

  function onKey(event: KeyboardEvent) {
    if (!info) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeError();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if info}
  <div
    class="veil"
    role="presentation"
    onclick={(event) => {
      if (event.target === event.currentTarget) closeError();
    }}
  >
    <div class="card" role="alertdialog" aria-modal="true" aria-labelledby="err-title">
      <header>
        <span class="badge" aria-hidden="true">
          <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round">
            <path d="M12 8v5" />
            <path d="M12 16.5v.5" />
            <circle cx="12" cy="12" r="9" />
          </svg>
        </span>
        <h2 id="err-title">{info.title}</h2>
      </header>

      {#if info.message}
        <p class="message">{info.message}</p>
      {/if}

      {#if info.detail}
        <details>
          <summary>详细信息</summary>
          <pre>{info.detail}</pre>
        </details>
      {/if}

      <footer>
        <button class="act" onclick={openLogs}>打开日志</button>
        <button class="act" onclick={exportReport} disabled={busy}>导出错误报告</button>
        <button class="act" onclick={copyReport}>复制错误信息</button>
        <button class="act primary" onclick={closeError}>确定</button>
      </footer>
    </div>
  </div>
{/if}

<style>
  .veil {
    position: fixed;
    inset: 0;
    display: grid;
    place-items: center;
    padding: 24px;
    background: rgba(6, 9, 6, 0.55);
    backdrop-filter: blur(3px);
    z-index: 120;
  }
  .card {
    width: min(560px, 100%);
    max-height: 82vh;
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 18px 20px 14px;
    border-radius: 16px;
    border: 1px solid rgba(255, 255, 255, 0.14);
    background: color-mix(in srgb, #10160d 92%, transparent);
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.55);
    color: #f4f8ee;
  }
  header {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .badge {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    flex: none;
    border-radius: 999px;
    color: #ffd9a3;
    background: rgba(214, 132, 44, 0.18);
    border: 1px solid rgba(214, 132, 44, 0.45);
  }
  h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
    line-height: 1.4;
  }
  .message {
    margin: 0;
    font-size: 13px;
    line-height: 1.7;
    color: rgba(255, 255, 255, 0.78);
    overflow: auto;
    flex: none;
  }
  details {
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  summary {
    cursor: pointer;
    font-size: 12px;
    color: rgba(255, 255, 255, 0.62);
    user-select: none;
  }
  summary:hover {
    color: rgba(255, 255, 255, 0.85);
  }
  pre {
    margin: 0;
    overflow: auto;
    max-height: 34vh;
    padding: 10px 12px;
    border-radius: 10px;
    background: rgba(0, 0, 0, 0.34);
    border: 1px solid rgba(255, 255, 255, 0.08);
    font-size: 11.5px;
    line-height: 1.6;
    white-space: pre-wrap;
    word-break: break-word;
    color: rgba(255, 255, 255, 0.72);
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    flex: none;
  }
  .act {
    height: 32px;
    padding: 0 14px;
    border-radius: 9px;
    border: 1px solid rgba(255, 255, 255, 0.16);
    background: rgba(255, 255, 255, 0.05);
    color: #eef4e6;
    font-size: 12.5px;
    cursor: pointer;
  }
  .act:hover {
    border-color: rgba(168, 204, 132, 0.55);
    background: rgba(127, 163, 94, 0.16);
  }
  .act:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .act.primary {
    border-color: rgba(168, 204, 132, 0.6);
    background: color-mix(in srgb, var(--yb-green) 30%, transparent);
    font-weight: 600;
  }
</style>
