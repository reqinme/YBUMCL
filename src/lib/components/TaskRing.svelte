<script lang="ts">
  import { goto } from "$app/navigation";
  import { page } from "$app/stores";
  import { downloadsNav } from "$lib/downloads-nav.svelte";
  import { activeCount, overallProgress } from "$lib/tasks.svelte";

  /// 圆环画在一个 36×36 的 svg 里，半径 15、线宽 2。
  const SIZE = 36;
  const RADIUS = 15;
  const CIRC = 2 * Math.PI * RADIUS;

  const active = $derived(activeCount());
  const percent = $derived(overallProgress());
  /// 有任务在跑但还拿不到字节数（服务端没给 Content-Length）时转圈，别停在 0 让人以为卡住。
  const spinning = $derived(active > 0 && percent <= 0);
  /// 空闲时不露圆环，只剩图标。
  const show = $derived(active > 0 || percent > 0);
  const offset = $derived(CIRC * (1 - percent / 100));
</script>

<button class="ring" class:on={show} title="下载任务" onclick={() => {
    downloadsNav.tasksOpen = true;
    if ($page.url.pathname !== "/downloads") void goto("/downloads");
  }} aria-label="下载任务">
  <svg class="dial" viewBox={`0 0 ${SIZE} ${SIZE}`} width={SIZE} height={SIZE} aria-hidden="true">
    <circle class="track" cx={SIZE / 2} cy={SIZE / 2} r={RADIUS} />
    <circle
      class="bar"
      class:spinning
      cx={SIZE / 2}
      cy={SIZE / 2}
      r={RADIUS}
      stroke-dasharray={spinning ? `${CIRC * 0.28} ${CIRC}` : CIRC}
      stroke-dashoffset={spinning ? 0 : offset}
    />
  </svg>
  <span class="icon" aria-hidden="true">
    <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M12 3v12" />
      <path d="m7 12 5 5 5-5" />
      <path d="M4 20h16" />
    </svg>
  </span>
</button>

<style>
  .ring {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    border-radius: 50%;
    color: rgba(255, 255, 255, 0.72);
    transition:
      color 0.15s ease,
      background 0.15s ease;
  }
  .ring:hover {
    color: #fff;
    background: rgba(255, 255, 255, 0.07);
  }
  .dial {
    position: absolute;
    inset: 0;
    /* 12 点方向起画 */
    transform: rotate(-90deg);
  }
  .track {
    fill: none;
    stroke: rgba(255, 255, 255, 0.12);
    stroke-width: 2;
    opacity: 0;
    transition: opacity 0.2s ease;
  }
  .bar {
    fill: none;
    stroke: var(--yb-green-bright);
    stroke-width: 2;
    stroke-linecap: round;
    opacity: 0;
    transition:
      stroke-dashoffset 0.18s linear,
      opacity 0.25s ease;
  }
  .ring.on .track {
    opacity: 1;
  }
  .ring.on .bar {
    opacity: 1;
  }
  /* 总量未知：画一段弧匀速转 */
  .bar.spinning {
    animation: spin 1.1s linear infinite;
    transform-origin: 18px 18px;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .icon {
    display: inline-flex;
  }
</style>
