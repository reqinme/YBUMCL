<script lang="ts">
  import { goto } from "$app/navigation";
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

<button
  class="ring"
  class:on={show}
  data-download-ring
  title="下载任务"
  onclick={() => void goto("/tasks")}
  aria-label="下载任务"
>
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
  {#if active > 1}
    <span class="count" aria-hidden="true">{active}</span>
  {/if}
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
    color: var(--text-2);
    transition:
      color 0.15s ease,
      background 0.15s ease;
  }
  .ring:hover {
    color: #fff;
    background: var(--tint-2);
  }
  .dial {
    position: absolute;
    inset: 0;
    /* 12 点方向起画 */
    transform: rotate(-90deg);
  }
  .track {
    fill: none;
    stroke: var(--tint-3);
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
  /* 并行安装时右上角挂个数字，告诉你现在同时在跑几个。 */
  .count {
    position: absolute;
    top: -3px;
    right: -3px;
    min-width: 15px;
    height: 15px;
    padding: 0 4px;
    border-radius: 8px;
    background: var(--yb-green-bright);
    color: #0c1207;
    font-size: 10px;
    font-weight: 700;
    line-height: 15px;
    text-align: center;
    box-shadow: 0 0 0 2px rgba(10, 12, 17, 0.75);
  }
  /* 光球撞进来的一下：圆环亮一下、微微涨开。 */
  :global(.ring.received) {
    animation: received 0.62s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  @keyframes received {
    0% {
      color: #fff;
      box-shadow: 0 0 0 0 rgba(168, 204, 132, 0.55);
      transform: scale(1);
    }
    35% {
      color: #fff;
      box-shadow: 0 0 22px 6px rgba(168, 204, 132, 0.45);
      transform: scale(1.18);
    }
    100% {
      box-shadow: 0 0 0 0 rgba(168, 204, 132, 0);
      transform: scale(1);
    }
  }
</style>
