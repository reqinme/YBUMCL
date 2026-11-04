<script lang="ts">
  // 悬浮提示：底部居中的小胶囊，出现后淡出，不打断操作。
  import { onMount } from "svelte";
  import { subscribeToast, type ToastKind } from "$lib/toast";

  type Item = { id: number; text: string; kind: ToastKind };

  let item = $state<Item | null>(null);

  onMount(() =>
    subscribeToast((value) => {
      item = value as Item | null;
    })
  );
</script>

<div class="toast-layer" aria-live="polite">
  {#if item}
    {#key item.id}
      <div class="toast" class:info={item.kind === "info"}>
        <span class="dot"></span>
        {item.text}
      </div>
    {/key}
  {/if}
</div>

<style>
  .toast-layer {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 26px;
    display: flex;
    justify-content: center;
    pointer-events: none;
    z-index: 90;
  }
  .toast {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 9px 16px;
    border-radius: 999px;
    font-size: 13px;
    color: #eaf3e0;
    background: rgba(24, 30, 22, 0.86);
    border: 1px solid rgba(168, 204, 132, 0.45);
    box-shadow: 0 10px 28px rgba(0, 0, 0, 0.42);
    backdrop-filter: blur(14px);
    -webkit-backdrop-filter: blur(14px);
    animation: toast-in 0.18s ease-out, toast-out 0.35s ease-in 1.25s forwards;
  }
  .toast.info {
    color: #e8eefb;
    background: rgba(22, 26, 34, 0.86);
    border-color: rgba(255, 255, 255, 0.18);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--yb-green-bright);
  }
  .info .dot {
    background: #9fb4d8;
  }

  @keyframes toast-in {
    from {
      opacity: 0;
      transform: translateY(8px) scale(0.97);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  @keyframes toast-out {
    to {
      opacity: 0;
      transform: translateY(-4px);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .toast {
      animation: none;
    }
  }
</style>
