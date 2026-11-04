<script lang="ts">
  import { goto } from "$app/navigation";
  import { page } from "$app/stores";
  import { downloadsNav } from "$lib/downloads-nav.svelte";
  import { notice } from "$lib/tasks.svelte";
</script>

<button
  class="island"
  class:hidden={!notice.text}
  title="查看下载任务"
  onclick={() => {
    downloadsNav.tasksOpen = true;
    if ($page.url.pathname !== "/downloads") void goto("/downloads");
  }}
  aria-hidden={!notice.text}
  tabindex={notice.text ? 0 : -1}
>
  <span class="tick" aria-hidden="true">
    <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
      <path d="M20 6 9 17l-5-5" />
    </svg>
  </span>
  <span class="text">{notice.text}</span>
</button>

<style>
  .island {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 32px;
    max-width: 300px;
    padding: 0 12px;
    border-radius: 999px;
    border: 1px solid rgba(255, 255, 255, 0.16);
    background: color-mix(in srgb, #10160d 78%, transparent);
    box-shadow: 0 8px 22px rgba(0, 0, 0, 0.32);
    backdrop-filter: blur(18px) saturate(1.5);
    color: #f4f8ee;
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    transform: scaleX(1);
    transform-origin: center;
    opacity: 1;
    transition:
      max-width 0.3s cubic-bezier(0.2, 0.9, 0.25, 1),
      padding 0.3s cubic-bezier(0.2, 0.9, 0.25, 1),
      opacity 0.22s ease,
      transform 0.3s cubic-bezier(0.2, 0.9, 0.25, 1);
  }
  .island:hover {
    border-color: color-mix(in srgb, var(--yb-green) 60%, transparent);
  }
  /* 收起：宽度先缩掉，同时 scaleX 往中间收，看起来是从中心向两边展开的反过程。 */
  .island.hidden {
    max-width: 0;
    padding: 0;
    border-color: transparent;
    opacity: 0;
    transform: scaleX(0.04);
    pointer-events: none;
  }
  .tick {
    display: inline-flex;
    color: var(--yb-green-bright);
    flex: none;
  }
  .text {
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
