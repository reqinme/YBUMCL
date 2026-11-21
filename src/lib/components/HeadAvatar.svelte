<script lang="ts">
  import { headImage } from "$lib/skin-head";

  type Props = {
    /// 完整皮肤（data URI 或 URL）。
    skin?: string | null;
    size?: number;
    /// 皮肤用不了时显示的首字母来源。
    fallback?: string;
  };

  let { skin = null, size = 30, fallback = "" }: Props = $props();

  let src = $state<string | null>(null);

  $effect(() => {
    const current = skin;
    src = null;
    if (!current) return;
    let alive = true;
    void headImage(current).then((url) => {
      if (alive) src = url;
    });
    return () => {
      alive = false;
    };
  });
</script>

{#if src}
  <img class="head" {src} alt="" style="width: {size}px; height: {size}px" />
{:else}
  <span class="head letter" style="width: {size}px; height: {size}px" aria-hidden="true">
    {fallback.slice(0, 1).toUpperCase()}
  </span>
{/if}

<style>
  .head {
    display: block;
    flex: none;
    object-fit: contain;
    /* 脸和帽子层叠出来的形状本身就是轮廓，不裁圆角、不加底。 */
    background: transparent;
    image-rendering: pixelated;
  }
  .letter {
    display: grid;
    place-items: center;
    color: var(--text);
    font-size: 0.8em;
    font-weight: 600;
    background: rgba(120, 220, 160, 0.16);
  }
</style>
