<script lang="ts">
  /// 立体皮肤预览。
  ///
  /// `mode="model"` 起一个可拖动的 three 场景（弹窗里用）；
  /// `mode="image"` 平时只显示离屏渲染出的图片（列表里用，避免每张卡片常驻
  /// WebGL），鼠标移上去再挂一个真场景，可以拖动旋转、滚轮缩放，双击复位。
  import type { SkinViewer } from "skinview3d";
  import { OFFICIAL_SKINS } from "$lib/official-skins";
  import { applyLook, mountViewer, skinImage, type SkinLook } from "$lib/skin3d";

  interface Props {
    skin?: string | null;
    model?: string | null;
    cape?: string | null;
    mode?: "model" | "image";
    /// 图片模式下的像素高度。
    height?: number;
    /// 图片模式下铺满父容器（父容器自己给高度），用于方形卡片这类不定高场景。
    fill?: boolean;
  }

  let { skin = null, model = null, cape = null, mode = "model", height = 240, fill = false }: Props = $props();

  /// 没选皮肤时用原版 Steve（wide 版），和以前一致。
  const look = $derived<SkinLook>({ skin: skin ?? OFFICIAL_SKINS[0].wide, cape, model });

  let canvasEl = $state<HTMLCanvasElement | null>(null);
  let hoverEl = $state<HTMLCanvasElement | null>(null);
  let src = $state<string | null>(null);
  let hot = $state(false);
  let dragging = $state(false);
  let viewer: SkinViewer | null = null;
  let hoverViewer: SkinViewer | null = null;

  $effect(() => {
    if (mode !== "image") return;
    const current = look;
    let alive = true;
    void skinImage(current).then((url) => {
      if (alive) src = url;
    });
    return () => {
      alive = false;
    };
  });

  /// 列表卡片：鼠标进来才起场景，出去就销毁（拖动中先留着）。
  $effect(() => {
    if (mode !== "image" || !hot) return;
    const node = hoverEl;
    if (!node) return;
    const box = node.parentElement;
    let instance: SkinViewer | null = null;
    let disposed = false;

    const fit = () => {
      if (!instance || !box) return;
      instance.width = Math.max(80, box.clientWidth);
      instance.height = Math.max(80, box.clientHeight);
      instance.adjustCameraDistance();
    };
    const observer = new ResizeObserver(fit);
    if (box) observer.observe(box);

    void mountViewer(node, look).then((created) => {
      if (disposed) {
        created.dispose();
        return;
      }
      instance = created;
      hoverViewer = created;
      fit();
      created.canvas.addEventListener("pointerdown", () => (dragging = true));
      window.addEventListener(
        "pointerup",
        () => {
          dragging = false;
        },
        { once: true },
      );
    });

    return () => {
      disposed = true;
      observer.disconnect();
      instance?.dispose();
      instance = null;
      hoverViewer = null;
    };
  });

  $effect(() => {
    if (mode !== "model") return;
    const node = canvasEl;
    if (!node) return;
    const box = node.parentElement;
    let instance: SkinViewer | null = null;
    let disposed = false;

    const fit = () => {
      if (!instance || !box) return;
      instance.width = Math.max(80, box.clientWidth);
      instance.height = Math.max(80, box.clientHeight);
      instance.adjustCameraDistance();
    };
    const observer = new ResizeObserver(fit);
    if (box) observer.observe(box);

    void mountViewer(node, look).then((created) => {
      if (disposed) {
        created.dispose();
        return;
      }
      instance = created;
      viewer = created;
      fit();
    });

    return () => {
      disposed = true;
      observer.disconnect();
      instance?.dispose();
      instance = null;
      viewer = null;
    };
  });

  $effect(() => {
    if (mode !== "model") return;
    const instance = viewer;
    if (!instance) return;
    void applyLook(instance, look);
  });

  $effect(() => {
    if (mode !== "image") return;
    const instance = hoverViewer;
    if (!instance) return;
    void applyLook(instance, look);
  });

  function reset() {
    hoverViewer?.controls.reset();
    viewer?.controls.reset();
  }
</script>

{#if mode === "image"}
  <span
    class="wrap"
    style="height: {fill ? "100%" : `${height}px`}"
    onpointerenter={() => (hot = true)}
    onpointerleave={() => {
      if (!dragging) hot = false;
    }}
    ondblclick={reset}
    role="presentation"
  >
    {#if src}
      <img class="shot" class:faded={hot} {src} alt="" />
    {:else}
      <span class="shot blank" class:faded={hot}></span>
    {/if}
    <canvas class="live" class:ready={hot} bind:this={hoverEl}></canvas>
  </span>
{:else}
  <div class="host">
    <canvas bind:this={canvasEl} ondblclick={reset}></canvas>
  </div>
{/if}

<style>
  .host {
    width: 100%;
    height: 100%;
    display: block;
    cursor: grab;
  }
  .host:active {
    cursor: grabbing;
  }
  .host canvas {
    display: block;
    width: 100%;
    height: 100%;
    outline: none;
  }
  .wrap {
    position: relative;
    display: block;
    width: 100%;
    cursor: grab;
  }
  .wrap:active {
    cursor: grabbing;
  }
  .shot {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: contain;
    transition: opacity 120ms ease;
  }
  .shot.faded {
    opacity: 0;
  }
  .shot.blank {
    width: 60px;
    margin: 0 auto;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.06);
  }
  .live {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    display: block;
    opacity: 0;
    pointer-events: none;
    outline: none;
  }
  .live.ready {
    opacity: 1;
    pointer-events: auto;
  }
</style>
