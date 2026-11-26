<script lang="ts">
  import type { Snippet } from "svelte";

  type Option = { value: string; label: string };

  let {
    value = $bindable(""),
    options,
    icon,
    block = false,
    ariaLabel = "",
    onchange
  }: {
    value?: string;
    options: Option[];
    /// 传了就画图标而不是文字（账户页的网格/列表就是这种）。
    icon?: Snippet<[string]>;
    /// 撑满父容器宽度（版本详情页那一排标签就是这种）。
    block?: boolean;
    ariaLabel?: string;
    onchange?: (value: string) => void;
  } = $props();

  /// 高亮块的位置：按当前值在选项里的下标算，切换时靠 transform 滑过去。
  const index = $derived(Math.max(0, options.findIndex((item) => item.value === value)));

  function pick(next: string) {
    if (next === value) return;
    value = next;
    onchange?.(next);
  }
</script>

<div class="seg" class:block role="group" aria-label={ariaLabel}>
  <span class="thumb" style="--i: {index}; --n: {options.length}"></span>
  {#each options as option (option.value)}
    <button
      type="button"
      class:active={option.value === value}
      aria-label={option.label}
      aria-pressed={option.value === value}
      onclick={() => pick(option.value)}
    >
      {#if icon}{@render icon(option.value)}{:else}{option.label}{/if}
    </button>
  {/each}
</div>

<style>
  .seg {
    position: relative;
    display: inline-flex;
    padding: 3px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: var(--surface-2);
  }

  .seg.block {
    display: flex;
    width: 100%;
  }

  /* 底片：宽度固定为一份，切换时用 transform 左右滑，颜色变化稍晚一点。 */
  .thumb {
    position: absolute;
    top: 3px;
    bottom: 3px;
    left: 3px;
    width: calc((100% - 6px) / var(--n));
    transform: translateX(calc(var(--i) * 100%));
    border-radius: 8px;
    background: rgb(120 200 120 / 0.22);
    box-shadow: inset 0 0 0 1px rgb(120 200 120 / 0.35);
    transition: transform 0.18s cubic-bezier(0.2, 0.7, 0.3, 1);
    pointer-events: none;
  }

  button {
    position: relative;
    display: grid;
    place-items: center;
    flex: 1 1 0;
    min-width: 34px;
    height: 26px;
    padding: 0 9px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--text-dim);
    font: inherit;
    font-size: 12.5px;
    white-space: nowrap;
    cursor: pointer;
    transition: color 0.18s ease 0.06s;
  }

  button.active {
    color: var(--text);
  }

  button:focus-visible {
    outline: 2px solid color-mix(in srgb, var(--yb-green) 60%, transparent);
    outline-offset: 1px;
  }
</style>
