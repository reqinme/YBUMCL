<script lang="ts">
  import { installFlight, landFlight, type Flight } from "$lib/install-flight.svelte";

  /// 一次飞行 560ms：够看清弧线，又不至于让人等。
  const DURATION = 560;
  /// 曲线上取多少个采样点（越多越顺，26 已经看不出折线）。
  const STEPS = 26;

  type Spot = { x: number; y: number };

  /// 把二次贝塞尔按缓出曲线采样成点列，交给 Web Animations 逐帧插值。
  function sample(flight: Flight): Spot[] {
    const spots: Spot[] = [];
    for (let i = 0; i <= STEPS; i += 1) {
      const raw = i / STEPS;
      // 先慢后快再收住，落地那一下才有「撞进去」的感觉。
      const t = raw < 0.5 ? 2 * raw * raw : 1 - (-2 * raw + 2) ** 2 / 2;
      const inv = 1 - t;
      spots.push({
        x: inv * inv * flight.start.x + 2 * inv * t * flight.control.x + t * t * flight.end.x,
        y: inv * inv * flight.start.y + 2 * inv * t * flight.control.y + t * t * flight.end.y,
      });
    }
    return spots;
  }

  /// 球从按钮那一下「挤」出来，飞到最后被圆环吸小。
  function scaleAt(t: number): number {
    if (t < 0.16) return 1.5 - (t / 0.16) * 0.5;
    if (t > 0.86) return 1 - ((t - 0.86) / 0.14) * 0.45;
    return 1;
  }

  function pulseRing() {
    const ring = document.querySelector<HTMLElement>("[data-download-ring]");
    if (!ring) return;
    ring.classList.remove("received");
    // 强制回流，让同一个类名能连续触发两次。
    void ring.offsetWidth;
    ring.classList.add("received");
    window.setTimeout(() => ring.classList.remove("received"), 620);
  }

  function fly(node: HTMLElement, params: { flight: Flight; lag?: number }) {
    const { flight } = params;
    const lag = Math.round((params.lag ?? 0) * STEPS);
    const spots = sample(flight);
    const frames = spots.map((spot, index) => {
      const t = index / STEPS;
      const at = spots[Math.max(0, index - lag)];
      return {
        offset: t,
        opacity: index === 0 && lag > 0 ? 0 : 1,
        transform: `translate3d(${at.x}px, ${at.y}px, 0) translate(-50%, -50%) scale(${scaleAt(t)})`,
      };
    });
    const animation = node.animate(frames, {
      duration: DURATION,
      easing: "linear",
      fill: "forwards",
    });
    if (lag === 0) {
      animation.addEventListener("finish", () => {
        pulseRing();
        window.setTimeout(() => landFlight(flight.id), 140);
      });
    }
    return {
      destroy() {
        animation.cancel();
      },
    };
  }
</script>

<div class="layer" aria-hidden="true">
  {#each installFlight.flights as flight (flight.id)}
    <div class="flight" style="--c: {installFlight.color}">
      <span class="spark s3" use:fly={{ flight, lag: 0.14 }}></span>
      <span class="spark s2" use:fly={{ flight, lag: 0.09 }}></span>
      <span class="spark s1" use:fly={{ flight, lag: 0.05 }}></span>
      <span class="ball" use:fly={{ flight, lag: 0 }}></span>
    </div>
  {/each}
</div>

<style>
  .layer {
    position: fixed;
    inset: 0;
    pointer-events: none;
    z-index: 900;
    overflow: hidden;
  }
  .flight {
    position: absolute;
    inset: 0;
  }
  .ball,
  .spark {
    position: absolute;
    left: 0;
    top: 0;
    border-radius: 50%;
    will-change: transform, opacity;
  }
  /* 一颗球三层：深色内核兜底、主色渐变做体积、外圈提亮 + 光晕。 */
  .ball {
    width: 32px;
    height: 32px;
    background:
      radial-gradient(circle at 34% 28%, rgba(255, 255, 255, 0.92) 0%, rgba(255, 255, 255, 0) 38%),
      radial-gradient(
        circle at 50% 56%,
        color-mix(in srgb, var(--c) 38%, #05070a 62%) 0%,
        var(--c) 56%,
        color-mix(in srgb, var(--c) 42%, #ffffff 58%) 100%
      );
    box-shadow:
      0 0 18px 4px color-mix(in srgb, var(--c) 52%, transparent),
      0 0 46px 12px color-mix(in srgb, var(--c) 18%, transparent),
      inset 0 -4px 9px rgba(0, 0, 0, 0.38),
      inset 0 2px 4px rgba(255, 255, 255, 0.22);
  }
  .spark {
    background: radial-gradient(
      circle,
      color-mix(in srgb, var(--c) 28%, #ffffff 72%) 0%,
      color-mix(in srgb, var(--c) 60%, transparent) 45%,
      transparent 72%
    );
  }
  .s1 {
    width: 24px;
    height: 24px;
    filter: blur(2px);
    opacity: 0.55;
  }
  .s2 {
    width: 18px;
    height: 18px;
    filter: blur(3px);
    opacity: 0.36;
  }
  .s3 {
    width: 12px;
    height: 12px;
    filter: blur(4px);
    opacity: 0.22;
  }
</style>
