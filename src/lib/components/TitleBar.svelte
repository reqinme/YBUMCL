<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";

  // 自绘标题栏（窗口 decorations 为 false）。
  let maximized = $state(false);

  function win() {
    return getCurrentWindow();
  }

  async function onMaximize() {
    try {
      await win().toggleMaximize();
      maximized = await win().isMaximized();
    } catch {
      /* 浏览器里预览时忽略 */
    }
  }
</script>

<header class="titlebar" data-tauri-drag-region>
  <div class="drag" data-tauri-drag-region></div>
  <div class="controls">
    <button class="ctl" title="最小化" onclick={() => win().minimize()}>
      <svg width="10" height="10" viewBox="0 0 10 10"><path d="M2 5h6" stroke="currentColor" stroke-width="1.2" /></svg>
    </button>
    <button class="ctl" title={maximized ? "还原" : "最大化"} onclick={onMaximize}>
      {#if maximized}
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none">
          <rect x="2.2" y="3.4" width="4.4" height="4.4" stroke="currentColor" stroke-width="1.1" />
          <path d="M3.6 3.2V2.2h4.2v4.2H6.8" stroke="currentColor" stroke-width="1.1" />
        </svg>
      {:else}
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none">
          <rect x="2.3" y="2.3" width="5.4" height="5.4" stroke="currentColor" stroke-width="1.1" />
        </svg>
      {/if}
    </button>
    <button class="ctl close" title="关闭" onclick={() => win().close()}>
      <svg width="10" height="10" viewBox="0 0 10 10">
        <path d="M2.4 2.4l5.2 5.2M7.6 2.4L2.4 7.6" stroke="currentColor" stroke-width="1.2" />
      </svg>
    </button>
  </div>
</header>

<style>
  .titlebar {
    height: var(--titlebar-h);
    display: flex;
    align-items: stretch;
    flex: none;
    /* 顶部压一层浅色遮罩：壁纸亮的时候窗口按钮也要看得清 */
    background: linear-gradient(180deg, rgba(0, 0, 0, 0.42), rgba(0, 0, 0, 0));
  }
  .drag {
    flex: 1;
  }
  .controls {
    display: flex;
  }
  .ctl {
    width: 44px;
    display: grid;
    place-items: center;
    color: var(--text-dim);
    transition: background 0.12s ease, color 0.12s ease;
  }
  .ctl:hover {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text);
  }
  .ctl.close:hover {
    background: #c0392b;
    color: #fff;
  }
</style>
