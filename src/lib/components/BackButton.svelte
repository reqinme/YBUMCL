<script lang="ts">
  import { canGoBackPage, goBackPage } from "$lib/page-history.svelte";
</script>

<!-- 页面标题左侧的返回键：和 Esc / Alt+← 走同一个返回栈，没有上一页时整块不渲染。 -->
{#if canGoBackPage()}
  <button class="back" type="button" title="返回上一页（Esc 或 Alt+←）" onclick={() => void goBackPage()}>
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M10 3 5 8l5 5"
        fill="none"
        stroke="currentColor"
        stroke-width="1.7"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  </button>
{/if}

<style>
  .back {
    flex: none;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: 999px;
    border: 1px solid rgba(255, 255, 255, 0.14);
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.12), rgba(255, 255, 255, 0.045));
    color: var(--text);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.18);
    transition: background 0.15s ease, border-color 0.15s ease;
  }
  .back:hover {
    background: rgba(255, 255, 255, 0.14);
    border-color: rgba(255, 255, 255, 0.24);
  }
</style>
