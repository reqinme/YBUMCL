<script lang="ts">
  // 自绘确认框。WebView 里 window.confirm 不阻塞（先执行再弹框），所以删除类操作
  // 一律走这个组件：遮罩 + 居中弹框，Esc 取消，删除按钮红色。
  interface Props {
    open?: boolean;
    message?: string;
    confirmLabel?: string;
    cancelLabel?: string;
    oncancel: () => void;
    /// 传鼠标事件：调用方据此判断有没有按住 Shift（永久删除）。
    onconfirm: (event: MouseEvent) => void;
  }

  let {
    open = false,
    message = "",
    confirmLabel = "删除",
    cancelLabel = "取消",
    oncancel,
    onconfirm
  }: Props = $props();
</script>

<svelte:window
  onkeydown={(event) => {
    if (open && event.key === "Escape") {
      event.preventDefault();
      oncancel();
    }
  }}
/>

{#if open}
  <div class="mask" role="presentation" onclick={oncancel} onkeydown={() => {}}>
    <div
      class="dialog"
      role="dialog"
      aria-modal="true"
      tabindex="-1"
      onclick={(event) => event.stopPropagation()}
      onkeydown={(event) => event.stopPropagation()}
    >
      <p>{message}</p>
      <div class="actions">
        <button class="btn ghost" onclick={oncancel}>{cancelLabel}</button>
        <button class="btn danger" onclick={(event) => onconfirm(event)}>{confirmLabel}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .mask {
    position: fixed;
    inset: 0;
    z-index: 80;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.62);
  }
  .dialog {
    display: flex;
    flex-direction: column;
    gap: 12px;
    width: min(420px, 82vw);
    padding: 16px 18px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border-strong);
    background: var(--panel-solid);
    box-shadow: 0 18px 48px rgba(0, 0, 0, 0.45);
  }
  .dialog p {
    margin: 0;
    font-size: 14px;
    white-space: pre-line;
    line-height: 1.6;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
