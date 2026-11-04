<script lang="ts">
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import { announce } from "$lib/tasks.svelte";
  import { showError } from "$lib/dialogs.svelte";
  import { api, type Settings, type VersionView } from "$lib/api";
  import { startVersion } from "$lib/launch";
  import { resolveIconUrl } from "$lib/version-icon";
  import { currentVersion, pickCurrent, setCurrentVersion } from "$lib/current-version.svelte";
  import { absorbCategories, isStarred, starredIds } from "$lib/starred.svelte";

  let versions = $state<VersionView[]>([]);
  /// 当前游戏目录，星标按目录分开记。
  let gameDir = $state("");
  let settings = $state<Settings | null>(null);
  /// 选中的实例同时记到全局：下载内容时拿它当默认落点。
  let selectedId = $state<string | null>(currentVersion.id);
  let busy = $state(false);
  /// 实例自己的图标（`versions/<id>/icon.png` 或 PCL 设过的），没有就按状态用内置方块图。
  let icons = $state<Record<string, string>>({});

  /// 左栏只列打了星的实例；没打星的一个都不占位。
  const shown = $derived(versions.filter((item) => isStarred(gameDir, item.id)));
  const selected = $derived(shown.find((item) => item.id === selectedId) ?? shown[0] ?? null);
  const collapsed = $derived(settings?.homeRailCollapsed ?? false);
  const memory = $derived(selected?.maxMemoryMb || settings?.memoryMb || 0);
  const username = $derived(settings?.username || "");

  onMount(async () => {
    try {
      const [settingsValue, list, dir, cats] = await Promise.all([
        api.loadSettings(),
        api.listVersions(),
        api.defaultGameDir(),
        api.listCategories()
      ]);
      settings = settingsValue;
      versions = list;
      gameDir = dir;
      absorbCategories(dir, cats.tags);
      selectedId = pickCurrent(shown);
      void loadIcons(list);
    } catch (e) {
      showError("读取版本列表失败", "主页没拿到版本清单。", String(e));
    }
  });

  async function loadIcons(list: VersionView[]): Promise<void> {
    const found: Record<string, string> = {};
    await Promise.all(
      list.map(async (version) => {
        const url = await api.versionIcon(version.id).catch(() => null);
        if (url) found[version.id] = url;
      })
    );
    icons = found;
  }

  async function toggleRail() {
    if (!settings) return;
    const next: Settings = { ...settings, homeRailCollapsed: !settings.homeRailCollapsed };
    settings = next; // 先动界面，再落盘：折叠这种小动作不该等 IO。
    try {
      await api.saveSettings(next);
    } catch (e) {
      showError("保存设置失败", "侧栏折叠状态没有写进配置文件。", String(e));
    }
  }

  async function launch() {
    if (!selected) return;
    busy = true;
    try {
      await startVersion(selected);
    } catch (e) {
      showError("启动失败", "游戏没有起来，看日志里的最后几行。", String(e), "game");
    } finally {
      busy = false;
    }
  }
</script>

<div class="home" class:collapsed>
  <button
    class="handle"
    class:open={!collapsed}
    title={collapsed ? "展开版本栏" : "收起版本栏"}
    onclick={toggleRail}
  >
    <svg viewBox="0 0 24 24" width="10" height="10" fill="none" stroke="currentColor" stroke-width="3.2" stroke-linecap="round" stroke-linejoin="round">
      {#if collapsed}
        <polyline points="9 6 15 12 9 18" />
      {:else}
        <polyline points="15 6 9 12 15 18" />
      {/if}
    </svg>
  </button>

  <aside class="rail" class:hidden={collapsed}>
    <div class="rail-head">
      <span class="rail-title">版本</span>
    </div>

    <div class="rail-list">
      {#each shown as version (version.id)}
        <button
          class="item"
          class:active={version.id === selected?.id}
          title={version.id}
          onclick={() => {
            selectedId = version.id;
            setCurrentVersion(version.id);
          }}
        >
          <img
            class="mark"
            src={resolveIconUrl(icons[version.id] ?? version.icon ?? null, version.channel, version.loaders)}
            alt=""
          />
          <span class="meta">
            <strong>{version.id}</strong>
            <small>{version.kind}</small>
          </span>
        </button>
      {/each}
    </div>

    <button class="btn new" onclick={() => goto("/downloads")} title="去下载一个版本">＋ 下载版本</button>
  </aside>

  <section class="stage">
    <div class="dock">
      <div class="dock-info">
        {#if selected}
          <h1>{selected.id}</h1>
          <p class="sub">
            <span class="tag">{selected.kind}</span>
            <span>{memory} MB</span>
            <span>{username}</span>
            <span>{selected.isolated ? "独立目录" : "共享目录"}</span>
          </p>
        {/if}
      </div>

      <div class="dock-actions">
        <button
          class="btn"
          disabled={!selected}
          onclick={() => selected && goto(`/versions/${encodeURIComponent(selected.id)}`)}
        >
          版本设置
        </button>
        <button class="btn primary" disabled={!selected || busy} onclick={launch}>
          {busy ? "启动中…" : "启动游戏"}
        </button>
      </div>
    </div>
  </section>
</div>

<style>

  .home {
    position: relative;
    width: 100%;
    height: 100%;
    box-sizing: border-box;
    display: grid;
    grid-template-columns: 232px 1fr;
    gap: 16px;
    height: 100%;
    min-height: 440px;
  }
  /* 收起后版本栏是 display:none，再留两列网格会把舞台挤进 0 宽那一列 */
  .home.collapsed {
    display: block;
  }

  .rail.hidden {
    display: none;
  }

  /* 版本栏的把手：收起时贴窗口左边，展开时挂在版本栏右边缘，竖直居中。
     右侧上下各收 8px，斜边形成梯形，像从面板边缘探出来的小舌头。 */
  .handle {
    position: absolute;
    left: 0;
    top: 50%;
    z-index: 5;
    width: 16px;
    height: 64px;
    padding: 0;
    border: none;
    color: var(--text-dim);
    background: rgba(18, 22, 30, 0.62);
    clip-path: polygon(0 0, 100% 8px, 100% calc(100% - 8px), 0 100%);
    transform: translateY(-50%);
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background 0.15s ease, transform 0.15s ease, color 0.15s ease;
  }
  .handle.open {
    left: 232px;
  }
  .handle:hover {
    background: rgba(34, 42, 54, 0.9);
    color: var(--text);
    transform: translateY(-50%) translateX(2px);
  }
  .handle:focus-visible {
    background: rgba(34, 42, 54, 0.9);
    color: var(--text);
  }

  .rail {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px;
    border-radius: var(--radius-lg);
    background: rgba(18, 22, 30, 0.42);
    border: 1px solid rgba(255, 255, 255, 0.08);
    backdrop-filter: blur(22px) saturate(1.25);
    -webkit-backdrop-filter: blur(22px) saturate(1.25);
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.28);
    overflow: hidden;
  }
  .rail-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 2px 2px;
    min-height: 24px;
  }
  .rail-title {
    font-size: 12px;
    color: var(--text-dim);
    letter-spacing: 0.08em;
  }

  .rail-list {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 4px;
    overflow-y: auto;
    padding-right: 2px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px;
    border-radius: var(--radius-sm);
    text-align: left;
    color: var(--text-dim);
    min-width: 0;
  }
  .item:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--text);
  }
  .item.active {
    background: rgba(168, 204, 132, 0.16);
    color: var(--text);
  }
  .mark {
    flex: none;
    width: 26px;
    height: 26px;
    border-radius: 8px;
    object-fit: contain;
    background: rgba(0, 0, 0, 0.22);
  }
  /* 选中态交给行背景，图标保持原色，只加一圈绿边。 */
  .item.active .mark {
    box-shadow: 0 0 0 1px rgba(168, 204, 132, 0.65);
  }
  .meta {
    display: flex;
    flex-direction: column;
    line-height: 1.2;
    min-width: 0;
  }
  .meta strong {
    font-size: 13px;
    font-weight: 560;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .meta small {
    font-size: 11px;
    color: var(--text-dim);
  }
  .new {
    flex: none;
    font-size: 13px;
  }

  /* 舞台：上半截留给壁纸，版本信息与按钮都沉到底部 */
  .stage {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    justify-content: flex-end;
    width: 100%;
    height: 100%;
    box-sizing: border-box;
    min-width: 0;
    padding: 0 2px 2px 0;
  }
  .dock {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 12px;
    padding: 14px 16px;
    border-radius: var(--radius-lg);
    background: rgba(14, 18, 24, 0.34);
    border: 1px solid rgba(255, 255, 255, 0.07);
    backdrop-filter: blur(18px) saturate(1.15);
    -webkit-backdrop-filter: blur(18px) saturate(1.15);
  }
  .dock-info {
    min-width: 0;
    text-align: right;
  }
  .dock-info h1 {
    margin: 0;
    font-size: 26px;
    font-weight: 650;
    letter-spacing: 0.01em;
    text-shadow: 0 2px 12px rgba(0, 0, 0, 0.5);
  }
  .sub {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 10px;
    margin: 8px 0 0;
    color: var(--text-dim);
    font-size: 13px;
  }
  .tag {
    padding: 2px 8px;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.1);
    font-size: 12px;
  }
  .dock-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }
  .dock-actions :global(.btn) {
    min-width: 108px;
  }
</style>
