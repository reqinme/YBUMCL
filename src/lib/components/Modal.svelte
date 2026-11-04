<script lang="ts">
  /// 通用弹窗：页面居中的遮罩 + 标题 + 内容 + 底部按钮。
  ///
  /// 跟 `ConfirmDialog` 同一套观感（遮罩、Esc 取消、点遮罩取消），这里多一个标题
  /// 和自由内容，给「修改实例名 / 修改实例描述 / 加入分类」这三个对话框用。
  import type { Snippet } from "svelte";

  interface Props {
    open?: boolean;
    title?: string;
    /// 底部按钮区；不传就只有一个「关闭」。
    footer?: Snippet;
    onclose: () => void;
    children?: Snippet;
  }

  let { open = false, title = "", footer, onclose, children }: Props = $props();
</script>

<svelte:window
  onkeydown={(event) => {
    if (open && event.key === "Escape") {
      event.preventDefault();
      onclose();
    }
  }}
/>

{#if open}
  <div class="mask" role="presentation" onclick={onclose} onkeydown={() => {}}>
    <div
      class="dialog"
      role="dialog"
      aria-modal="true"
      tabindex="-1"
      onclick={(event) => event.stopPropagation()}
      onkeydown={(event) => event.stopPropagation()}
    >
      {#if title}<h3>{title}</h3>{/if}
      <div class="body">{@render children?.()}</div>
      <div class="actions">
        {#if footer}
          {@render footer()}
        {:else}
          <button class="btn ghost" onclick={onclose}>关闭</button>
        {/if}
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
    gap: 14px;
    width: min(460px, 84vw);
    padding: 18px 20px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border-strong);
    background: var(--panel-solid);
    box-shadow: 0 18px 48px rgba(0, 0, 0, 0.45);
  }
  h3 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 10px;
    font-size: 13px;
  }
  .body :global(input[type="text"]),
  .body :global(textarea) {
    width: 100%;
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--surface-2, rgb(0 0 0 / 0.25));
    color: var(--text);
    font: inherit;
    resize: vertical;
  }
  .body :global(.hint) {
    color: var(--text-dim);
    font-size: 12px;
  }
  .body :global(.error) {
    color: var(--danger, #e2725b);
    font-size: 12px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
