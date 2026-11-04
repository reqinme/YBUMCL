<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { goto } from "$app/navigation";
  import BackButton from "$lib/components/BackButton.svelte";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import ConfirmDialog from "$lib/components/ConfirmDialog.svelte";
  import { resolveIconUrl } from "$lib/version-icon";
  import Dropdown from "$lib/components/Dropdown.svelte";
  import { announce, submitTask } from "$lib/tasks.svelte";
  import { showError } from "$lib/dialogs.svelte";
  import { api, type CategorySnapshot, type GameFolder, type Settings, type VersionView } from "$lib/api";
  import { startVersion } from "$lib/launch";
  import {
    addFolder,
    blockedByTask,
    folderState,
    folders,
    loadFolders,
    refreshScan,
    removeFolder,
    renameFolder,
    reorderFolders,
    selectFolder,
    visibleFolders
  } from "$lib/game-folders.svelte";
  import { absorbCategories, isStarred, toggleStar } from "$lib/starred.svelte";

  type Menu = { folder: GameFolder; x: number; y: number };

  const CHANNELS = [
    { value: "all", label: "全部" },
    { value: "release", label: "正式版" },
    { value: "april", label: "愚人节版" },
    { value: "snapshot", label: "快照版" }
  ];
  const CHANNEL_LABEL: Record<string, string> = {
    all: "全部",
    release: "正式版",
    april: "愚人节版",
    snapshot: "快照版",
    unknown: "未知"
  };
  const FILTER_KEY = "ybumcl.versions.filters";

  let versions = $state<VersionView[]>([]);
  let settings = $state<Settings | null>(null);
  let categories = $state<CategorySnapshot>({ categories: [], tags: {} });
  let loading = $state(true);
  let busy = $state("");
  let pendingDelete = $state<VersionView | null>(null);
  let pendingDeletePermanent = $state(false);
  /// 实例自己的图标（`versions/<id>/icon.png` 或 PCL 设过的）；没有就按状态用内置方块图。
  let icons = $state<Record<string, string>>({});

  let channel = $state("all");
  let gamePick = $state("all");
  let loaderPick = $state("all");
  /// 分类筛选：all / none / 分类 id。切目录时重置回 all。
  let categoryPick = $state("all");

  let ratio = $state(16);
  let resizing = $state(false);
  let renaming = $state<{ path: string; value: string } | null>(null);
  let renameBox = $state<HTMLInputElement | null>(null);
  let menu = $state<Menu | null>(null);
  let dragIndex = $state<number | null>(null);
  let overIndex = $state<number | null>(null);

  const shownFolders = $derived(visibleFolders());
  const activePath = $derived(folderState.active);
  const foldersLoading = $derived(folderState.loading);

  /// 第一级筛完的版本（第二级、加载器、分类都在它之上再筛）。
  const byChannel = $derived(
    channel === "all" ? versions : versions.filter((version) => version.channel === channel)
  );

  const channelOptions = $derived(
    CHANNELS.map((item) => ({
      value: item.value,
      label:
        item.value === "all"
          ? item.label
          : `${item.label} ${
              versions.filter((version) => version.channel === item.value).length || ""
            }`.trim()
    }))
  );

  const gameOptions = $derived.by(() => {
    const seen = new Map<string, number>();
    for (const version of byChannel) {
      const key = version.gameVersion ?? version.id;
      seen.set(key, (seen.get(key) ?? 0) + 1);
    }
    const options = [{ value: "all", label: "全部版本" }];
    for (const [key, count] of seen) options.push({ value: key, label: `${key} ${count}` });
    return options;
  });

  const loaderOptions = $derived.by(() => {
    const seen = new Map<string, number>();
    for (const version of byChannel) {
      for (const loader of version.loaders) seen.set(loader, (seen.get(loader) ?? 0) + 1);
    }
    const options = [{ value: "all", label: "全部加载器" }];
    for (const [key, count] of seen) options.push({ value: key, label: `${key} ${count}` });
    return options;
  });

  const categoryOptions = $derived.by(() => {
    const options = [
      { value: "all", label: "全部分类" },
      { value: "none", label: "未分类" },
      { value: "starred", label: "标星实例" }
    ];
    for (const item of categories.categories) {
      if (item.id === "starred") continue;
      options.push({ value: item.id, label: item.name });
    }
    return options;
  });

  const shown = $derived(
    byChannel.filter((version) => {
      if (gamePick !== "all" && (version.gameVersion ?? version.id) !== gamePick) return false;
      if (loaderPick !== "all" && !version.loaders.includes(loaderPick)) return false;
      if (categoryPick === "starred") return isStarred(activePath, version.id);
      const real = version.categories.filter((id) => id !== "starred");
      if (categoryPick === "none" && real.length > 0) return false;
      if (categoryPick !== "all" && categoryPick !== "none" && !real.includes(categoryPick)) return false;
      return true;
    })
  );

  function categoryName(id: string): string | null {
    return categories.categories.find((item) => item.id === id)?.name ?? null;
  }

  // 改名框一出来就聚焦并全选，省得再点一下。
  $effect(() => {
    if (renaming && renameBox) {
      renameBox.focus();
      renameBox.select();
    }
  });

  /// 默认名字：末级目录名（末级是 `.minecraft` 取上一级），和后端 `default_name` 一致。
  function defaultName(path: string): string {
    const parts = path.replaceAll("\\", "/").split("/").filter(Boolean);
    let name = parts.at(-1) ?? path;
    if (name.toLowerCase() === ".minecraft") name = parts.at(-2) ?? name;
    return name.slice(0, 30);
  }

  function canReset(folder: GameFolder): boolean {
    const label =
      folder.kind === "current"
        ? "当前文件夹"
        : folder.kind === "official"
          ? "官方启动器文件夹"
          : defaultName(folder.path);
    return folder.name !== label;
  }

  function saveFilters(): void {
    try {
      localStorage.setItem(FILTER_KEY, JSON.stringify({ channel, gamePick, loaderPick }));
    } catch {
      /* 存不了就算了，不影响功能 */
    }
  }

  function restoreFilters(): void {
    try {
      const raw = localStorage.getItem(FILTER_KEY);
      if (!raw) return;
      const saved = JSON.parse(raw) as Partial<{ channel: string; gamePick: string; loaderPick: string }>;
      if (saved.channel) channel = saved.channel;
      if (saved.gamePick) gamePick = saved.gamePick;
      if (saved.loaderPick) loaderPick = saved.loaderPick;
    } catch {
      /* 坏数据当没存过 */
    }
  }

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

  async function loadVersions(): Promise<void> {
    loading = true;
    try {
      const [list, cats] = await Promise.all([api.listVersions(), api.listCategories()]);
      versions = list;
      categories = cats;
      absorbCategories(activePath, cats.tags);
      void loadIcons(list);
      if (!gameOptions.some((option) => option.value === gamePick)) gamePick = "all";
      if (!loaderOptions.some((option) => option.value === loaderPick)) loaderPick = "all";
    } catch (e) {
      showError("读取版本列表失败", "版本清单没有读出来。", String(e));
    } finally {
      loading = false;
    }
  }

  async function boot(): Promise<void> {
    restoreFilters();
    try {
      settings = await api.loadSettings();
      ratio = settings.versionsSplitRatio || 16;
    } catch {
      /* 读不到设置就用默认宽度 */
    }
    await loadFolders();
    if (!activePath) {
      loading = false;
      return;
    }
    await loadVersions();
  }

  async function pick(folder: GameFolder): Promise<void> {
    if (folder.path === activePath) return;
    if (!(await selectFolder(folder.path))) return;
    categoryPick = "all";
    await loadVersions();
  }

  async function added(): Promise<void> {
    if (!(await addFolder())) return;
    categoryPick = "all";
    await loadVersions();
  }

  async function refresh(): Promise<void> {
    await refreshScan();
    await loadVersions();
    announce("已刷新");
  }

  async function launch(version: VersionView) {
    busy = version.id;
    try {
      await startVersion(version);
    } catch (e) {
      showError("启动失败", "游戏没有起来，看日志里的最后几行。", String(e), "game");
    } finally {
      busy = "";
    }
  }

  async function remove() {
    const version = pendingDelete;
    pendingDelete = null;
    if (!version) return;
    busy = version.id;
    try {
      await api.deleteVersion(version.id, pendingDeletePermanent);
      await loadVersions();
      announce("版本已删除");
    } catch (e) {
      showError("删除版本失败", "目录可能正被占用（游戏还开着？）。", String(e));
    } finally {
      busy = "";
    }
  }

  async function importPack() {
    const dir = activePath;
    if (!dir) return;
    if (blockedByTask()) return;
    let picked: string | null = null;
    try {
      const choice = await openDialog({
        multiple: false,
        directory: false,
        filters: [{ name: "整合包", extensions: ["mrpack", "zip"] }]
      });
      picked = typeof choice === "string" ? choice : null;
    } catch (e) {
      showError("打不开文件选择框", "系统没有给出选中的文件。", String(e));
      return;
    }
    if (!picked) return;
    const file = picked.replaceAll("\\", "/").split("/").at(-1) ?? picked;
    submitTask(file, dir, async () => {
      const summary = await api.installModpack({
        gameDir: dir,
        url: null,
        path: picked,
        fileName: file,
        name: null,
        javaPath: null,
        strategy: settings?.downloadStrategy ?? "race",
        threads: settings?.concurrentDownloads ?? 64
      });
      await loadVersions();
      if (summary.missing.length > 0 || summary.failed > 0) {
        showError("整合包没有装全", `已装好 ${summary.versionId}`, summary.missing.join("、"));
      }
      return {
        note: `已装好 ${summary.versionId}`,
        output: { path: summary.path, kind: "dir" as const }
      };
    });
  }

  // ------------------------------------------------------------ 右键菜单
  function openMenu(event: MouseEvent, folder: GameFolder) {
    event.preventDefault();
    menu = { folder, x: event.clientX, y: event.clientY };
  }

  // 浮层挂到 body 上再定位：面板带 backdrop-filter，fixed 会相对面板算，左上角就对不上光标。
  // 贴窗口右/下边缘时往回缩，保证整块菜单都在窗口里。
  function anchorMenu(node: HTMLElement) {
    document.body.appendChild(node);
    const place = () => {
      const left = Number.parseFloat(node.style.left);
      const top = Number.parseFloat(node.style.top);
      if (Number.isNaN(left) || Number.isNaN(top)) return;
      const rect = node.getBoundingClientRect();
      node.style.left = `${Math.max(8, Math.min(left, window.innerWidth - rect.width - 8))}px`;
      node.style.top = `${Math.max(8, Math.min(top, window.innerHeight - rect.height - 8))}px`;
    };
    place();
    // 节点被挪出了 Svelte 的子树，卸载时得自己摘，否则会跟着页面留在屏幕上。
    return {
      update: place,
      destroy() {
        node.remove();
      }
    };
  }

  function closeMenu() {
    menu = null;
  }

  // 换页时组件被销毁，浮层挂在 body 上，得主动关掉。
  onDestroy(closeMenu);

  function startRename(folder: GameFolder) {
    closeMenu();
    renaming = { path: folder.path, value: folder.name };
  }

  async function commitRename() {
    const target = renaming;
    renaming = null;
    if (!target) return;
    const name = target.value.trim();
    const folder = folders.find((item) => item.path === target.path);
    if (!folder || name === folder.name) return;
    await renameFolder(target.path, name.slice(0, 30));
    if (name === "") announce("已复原名称");
  }

  async function resetName(folder: GameFolder) {
    closeMenu();
    await renameFolder(folder.path, "");
    announce("已复原名称");
  }

  async function openInExplorer(folder: GameFolder) {
    closeMenu();
    try {
      await api.openFolder(folder.path);
    } catch (e) {
      showError("打不开目录", "资源管理器没有打开这个文件夹。", String(e));
    }
  }

  async function removeFromList(folder: GameFolder) {
    closeMenu();
    if (!(await removeFolder(folder.path))) return;
    await loadVersions();
  }

  // -------------------------------------------------------------- 拖拽
  function startReorder(event: PointerEvent, index: number) {
    event.preventDefault();
    event.stopPropagation();
    dragIndex = index;
    overIndex = index;
    /* 不抓 pointer capture：一抓，后面的 pointerenter 全被手柄吃掉，
       换位置就永远停在原地（旧 bug）。改成在 window 的 pointermove 里自己找目标。 */
  }

  async function endReorder() {
    const from = dragIndex;
    const to = overIndex;
    dragIndex = null;
    overIndex = null;
    if (from === null || to === null || from === to) return;
    const order = shownFolders.map((folder) => folder.path);
    const [moved] = order.splice(from, 1);
    if (moved === undefined) return;
    order.splice(to, 0, moved);
    await reorderFolders(order);
  }

  /// 拖文件夹排序时，用指针位置找它压在哪个列表项上。
  function trackReorder(event: PointerEvent) {
    if (dragIndex === null) return;
    const hit = document
      .elementFromPoint(event.clientX, event.clientY)
      ?.closest<HTMLElement>("[data-folder]");
    const index = hit?.dataset.folder;
    if (index !== undefined) {
      const next = Number(index);
      if (!Number.isNaN(next)) overIndex = next;
    }
  }

  // 左右分栏拖拽
  let splitRoot = $state<HTMLDivElement | null>(null);

  function startResize(event: PointerEvent) {
    resizing = true;
    (event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId);
  }

  function moveResize(event: PointerEvent) {
    if (!resizing || !splitRoot) return;
    const rect = splitRoot.getBoundingClientRect();
    if (rect.width <= 0) return;
    ratio = Math.min(30, Math.max(12, Math.round(((event.clientX - rect.left) / rect.width) * 100)));
  }

  async function endResize() {
    if (!resizing) return;
    resizing = false;
    if (!settings) return;
    const next = { ...settings, versionsSplitRatio: ratio };
    settings = next;
    try {
      await api.saveSettings(next);
    } catch {
      /* 记不住宽度不算错，下次还是默认值 */
    }
  }

  onMount(() => {
    void boot();
  });
</script>

<svelte:window
  onpointerdown={(event) => {
    if (!(event.target as HTMLElement).closest(".folder-menu")) closeMenu();
  }}
  onpointerup={() => {
    if (dragIndex !== null) void endReorder();
    if (resizing) void endResize();
  }}
  onpointermove={(event) => {
    moveResize(event);
    trackReorder(event);
  }}
/>

<div class="view">
  <header class="head">
    <BackButton />
    <h1>版本 <span class="count">{versions.length}</span></h1>
    <span class="spacer"></span>
    <button class="btn" onclick={refresh} disabled={foldersLoading || loading}>刷新</button>
    <button class="btn primary" onclick={() => goto("/downloads")}>下载版本</button>
  </header>

{#if foldersLoading}
  <p class="dim">正在读取文件夹…</p>
{:else if shownFolders.length === 0}
  <section class="panel">
    <h2>还没有游戏文件夹</h2>
  </section>
{:else}
  <div class="split" bind:this={splitRoot} style="grid-template-columns: {ratio}% 6px minmax(0, 1fr)">
    <aside class="rail">
      <p class="rail-label">文件夹列表</p>
      <ul class="folders">
        {#each shownFolders as folder, index (folder.path)}
          <li
            class="folder"
            class:active={folder.path === activePath}
            data-folder={index}
            class:over={dragIndex !== null && overIndex === index && dragIndex !== index}
          >
            {#if renaming?.path === folder.path}
              <input
                class="rename"
                bind:this={renameBox}
                bind:value={renaming.value}
                onkeydown={(event) => {
                  if (event.key === "Enter") void commitRename();
                  if (event.key === "Escape") renaming = null;
                }}
                onblur={() => void commitRename()}
              />
            {:else}
              <button
                class="pick"
                onclick={() => void pick(folder)}
                oncontextmenu={(event) => openMenu(event, folder)}
                title={folder.path}
              >
                <span class="lines">
                  <span class="name">
                    {folder.name}
                    {#if folder.kind !== "custom"}
                      <span class="badge">{folder.kind === "current" ? "当前" : "官方"}</span>
                    {/if}
                  </span>
                  <span class="path">{folder.path}</span>
                </span>
              </button>
              <button
                class="handle"
                title="拖拽排序"
                onpointerdown={(event) => startReorder(event, index)}
              >
                ≡
              </button>
            {/if}
          </li>
        {/each}
      </ul>
      <div class="add">
        <button class="entry" onclick={() => void added()}>
          <span class="glyph">＋</span>添加已有文件夹
        </button>
        <button class="entry" onclick={() => void importPack()}>
          <span class="glyph">⟳</span>导入整合包
        </button>
      </div>
    </aside>

    <div
      class="divider"
      class:resizing
      role="separator"
      aria-orientation="vertical"
      onpointerdown={startResize}
    ></div>

    <section class="main">
      <div class="filters">
        <Dropdown
          label="版本"
          bind:value={channel}
          options={channelOptions}
          onchange={() => {
            gamePick = "all";
            saveFilters();
          }}
        />
        <Dropdown
          label="版本号"
          bind:value={gamePick}
          options={gameOptions}
          disabled={gameOptions.length <= 1}
          onchange={saveFilters}
        />
        <Dropdown
          label="加载器"
          bind:value={loaderPick}
          options={loaderOptions}
          onchange={saveFilters}
        />
        <Dropdown label="分类" bind:value={categoryPick} options={categoryOptions} />
      </div>

      <div class="scroll">
      {#if loading}
        <p class="dim">正在读取版本…</p>
      {:else if versions.length === 0}
        <section class="panel">
          <h2>这个文件夹里还没有版本</h2>
        </section>
      {:else if shown.length === 0}
        <section class="panel">
          <h2>没有符合筛选的版本</h2>
        </section>
      {:else}
        <div class="cards">
          {#each shown as version (version.id)}
            <article class="card" class:unsupported={!version.supported}>
              <div class="top">
                <img
                  class="icon"
                  src={resolveIconUrl(icons[version.id] ?? version.icon ?? null, version.channel, version.loaders)}
                  alt=""
                />
                <div class="info">
                  <strong title={version.id}>{version.id}</strong>
                  <p class="meta">
                    <span class="kind"
                      >{CHANNEL_LABEL[version.channel] ?? version.channel}</span
                    >
                    {#if version.gameVersion && version.gameVersion !== version.id}
                      <span class="kind">{version.gameVersion}</span>
                    {/if}
                    <span class="loaders">{version.loaders.join(" + ") || "原版"}</span>
                  </p>
                </div>
                <button
                  class="star"
                  class:on={isStarred(activePath, version.id)}
                  title={isStarred(activePath, version.id) ? "已显示在主页，点一下隐藏" : "在主页显示"}
                  aria-label={isStarred(activePath, version.id) ? "取消在主页显示" : "在主页显示"}
                  onclick={() => toggleStar(activePath, version.id)}
                >
                  <svg viewBox="0 0 24 24" aria-hidden="true">
                    <path
                      d="M12 3.2l2.6 5.5 6 .8-4.4 4.2 1.1 6-5.3-3-5.3 3 1.1-6L3.4 9.5l6-.8z"
                    />
                  </svg>
                </button>
              </div>
              <dl>
                <div><dt>玩家</dt><dd>{settings?.username || "—"}</dd></div>
                <div>
                  <dt>内存</dt>
                  <dd>{version.maxMemoryMb || settings?.memoryMb || 4096} MB</dd>
                </div>
                <div><dt>环境</dt><dd>{version.isolated ? "独立目录" : "共享目录"}</dd></div>
              </dl>
              {#if version.categories.some((id) => id !== "starred")}
                <p class="tags">
                  {#each version.categories.filter((id) => id !== "starred") as id (id)}
                    {#each [categoryName(id)] as label}
                      {#if label}
                        <span class="tag">{label}</span>
                      {/if}
                    {/each}
                  {/each}
                </p>
              {/if}
              {#if !version.supported}
                <p class="warn">{version.reason ?? "这个版本缺少必要文件，无法启动"}</p>
              {/if}
              <div class="actions">
              <div class="row">
                <button
                  class="btn primary"
                  disabled={!version.supported || busy !== ""}
                  onclick={() => launch(version)}
                >
                  {busy === version.id ? "启动中…" : "启动"}
                </button>
                <button
                  class="btn"
                  onclick={() => goto(`/versions/${encodeURIComponent(version.id)}`)}
                >
                  设置
                </button>
              </div>
              <div class="row">
                <button class="btn" onclick={() => api.openVersionFolder(version.id, "root")}>
                  打开文件夹
                </button>
                <button
                  class="btn danger"
                  disabled={busy !== ""}
                  onclick={(event) => {
                    pendingDeletePermanent = event.shiftKey;
                    pendingDelete = version;
                  }}
                >
                  删除
                </button>
              </div>
              </div>
            </article>
          {/each}
        </div>
      {/if}
      </div>
    </section>
  </div>
{/if}
</div>

{#if menu}
  {@const target = menu.folder}
  <div class="folder-menu" use:anchorMenu style="left: {menu.x}px; top: {menu.y}px">
    <button class="menu-item" onclick={() => startRename(target)}>重命名</button>
    <button class="menu-item" onclick={() => void openInExplorer(target)}>打开</button>
    <button class="menu-item" onclick={() => void refresh()}>刷新</button>
    {#if canReset(target)}
      <button class="menu-item" onclick={() => void resetName(target)}>复原名称</button>
    {/if}
    <button class="menu-item danger" onclick={() => void removeFromList(target)}>
      移出列表
    </button>
  </div>
{/if}

<ConfirmDialog
  open={pendingDelete !== null}
  message={pendingDelete
    ? `删除版本「${pendingDelete.id}」？\n\nversions\\${pendingDelete.id} 整个目录会被删除，里面的存档、模组、配置都在其中。` +
      (pendingDeletePermanent ? `\n\n按住 Shift 点的是永久删除，删完找不回来。` : `\n\n不按 Shift 会丢进回收站，还能捞回来。`)
    : ""}
  oncancel={() => (pendingDelete = null)}
  onconfirm={() => void remove()}
/>

<style>
  /* 整页锁在视口内，页面自己不滚；滚动只发生在左栏列表和右栏卡片区。 */
  .view {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 2px 14px;
  }
  .head h1 {
    margin: 0;
    font-size: 24px;
    font-weight: 650;
  }
  .count {
    font-size: 13px;
    color: var(--text-dim);
    margin-left: 6px;
  }
  .spacer {
    flex: 1;
  }
  .split {
    flex: 1;
    display: grid;
    align-items: stretch;
    gap: 0;
    min-height: 0;
  }
  .rail {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
    border: 1px solid var(--border);
    background: var(--surface);
    border-radius: var(--radius-lg);
    padding: 12px 10px 10px;
  }
  .rail-label {
    margin: 0 6px 8px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .folders {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow-y: auto;
    min-height: 0;
    flex: 1;
  }
  .folder {
    display: flex;
    align-items: center;
    gap: 2px;
    border-radius: var(--radius-sm, 8px);
    padding: 2px 4px 2px 2px;
  }
  .folder:hover {
    background: rgb(255 255 255 / 0.05);
  }
  .folder.active {
    background: rgb(120 200 120 / 0.16);
  }
  .folder.over {
    box-shadow: inset 0 2px 0 var(--yb-green-bright);
  }
  .pick {
    flex: 1;
    min-width: 0;
    display: flex;
    text-align: left;
    background: none;
    border: 0;
    color: inherit;
    font: inherit;
    padding: 6px 4px;
    cursor: pointer;
  }
  .lines {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .folder.active .name {
    color: var(--yb-green-bright);
  }
  .badge {
    font-size: 10.5px;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 6px;
    flex: none;
  }
  .path {
    font-size: 11px;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .handle {
    flex: none;
    background: none;
    border: 0;
    color: var(--text-dim);
    cursor: grab;
    font-size: 14px;
    line-height: 1;
    padding: 4px 2px;
    touch-action: none;
  }
  .handle:active {
    cursor: grabbing;
  }
  .rename {
    flex: 1;
    min-width: 0;
    margin: 4px;
    padding: 4px 6px;
    font: inherit;
    font-size: 13px;
    color: var(--text);
    background: var(--surface-2, rgb(0 0 0 / 0.3));
    border: 1px solid var(--border);
    border-radius: var(--radius-sm, 8px);
  }
  .add {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-top: 8px;
    padding-top: 8px;
    border-top: 1px solid var(--border);
  }
  .entry {
    display: flex;
    align-items: center;
    gap: 8px;
    background: none;
    border: 0;
    color: var(--text);
    font: inherit;
    font-size: 13px;
    padding: 7px 6px;
    border-radius: var(--radius-sm, 8px);
    cursor: pointer;
    text-align: left;
  }
  .entry:hover {
    background: rgb(255 255 255 / 0.06);
  }
  .glyph {
    width: 16px;
    text-align: center;
    color: var(--text-dim);
  }
  .divider {
    cursor: col-resize;
    border-radius: 3px;
    touch-action: none;
  }
  .divider:hover,
  .divider.resizing {
    background: var(--yb-green-bright);
  }
  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    gap: 12px;
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    flex: none;
  }
  /* 下拉的根是 `min-width: 0`，在 flex 行里会被压成一条，这里给个下限。 */
  .filters :global(.dropdown) {
    min-width: 132px;
  }
  /* 右栏自己的滚动条：筛选行固定，只有这块滚。 */
  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    padding-right: 4px;
  }
  /* 卡片尺寸固定，宽度只决定排几列，不拉伸卡片。 */
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, 300px);
    justify-content: start;
    align-content: start;
    gap: 14px;
  }
  .card {
    box-sizing: border-box;
    width: 300px;
    height: 264px;
    overflow: hidden;
    border: 1px solid var(--border);
    background: var(--surface);
    border-radius: var(--radius-lg);
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .card.unsupported {
    opacity: 0.72;
  }
  .star {
    margin-left: auto;
    flex: none;
    align-self: flex-start;
    width: 26px;
    height: 26px;
    display: grid;
    place-items: center;
    padding: 0;
    border: none;
    background: none;
    color: rgba(226, 232, 240, 0.32);
    cursor: pointer;
  }
  .star svg {
    width: 20px;
    height: 20px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linejoin: round;
  }
  .star:hover {
    color: rgba(226, 232, 240, 0.72);
  }
  .star.on {
    color: #e7c04a;
  }
  .star.on svg {
    fill: currentColor;
    stroke: currentColor;
  }

  .top {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    min-width: 0;
    flex: none;
  }
  .icon {
    width: 44px;
    height: 44px;
    flex: none;
    object-fit: contain;
    border-radius: 8px;
    background: rgb(0 0 0 / 0.22);
  }
  .info {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
    flex: 1;
    padding-top: 2px;
  }
  .info strong {
    font-size: 16px;
    line-height: 1.15;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0;
    min-width: 0;
    overflow: hidden;
  }
  .kind {
    flex: none;
    font-size: 10.5px;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 7px;
  }
  .loaders {
    margin: 0 0 0 auto;
    font-size: 11.5px;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  dl {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 4px 10px;
    margin: 0;
  }
  dt {
    font-size: 11px;
    color: var(--text-dim);
  }
  dd {
    margin: 0;
    font-size: 12.5px;
  }
  .tags {
    display: flex;
    flex-wrap: nowrap;
    gap: 6px;
    margin: 0;
    max-height: 20px;
    overflow: hidden;
  }
  .tag {
    font-size: 11px;
    color: var(--yb-green-bright);
    background: rgb(120 200 120 / 0.14);
    border-radius: 999px;
    padding: 1px 8px;
  }
  /* 两行按钮抱团贴卡片底部，多出来的空白留在上面。 */
  .actions {
    margin-top: auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
    flex: none;
  }
  .row {
    display: flex;
    flex-wrap: nowrap;
    gap: 6px;
    flex: none;
  }
  .card .row .btn {
    flex: 1;
    min-width: 0;
    height: 30px;
    padding: 0 8px;
    font-size: 12.5px;
  }

  .warn {
    margin: 0;
    font-size: 11px;
    color: var(--yb-green-bright);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .panel {
    border: 1px solid var(--border);
    background: var(--surface);
    border-radius: var(--radius-lg);
    padding: 20px;
  }
  .panel h2 {
    margin: 0;
    font-size: 17px;
  }
  .dim {
    color: var(--text-dim);
    font-size: 13px;
    margin: 0;
  }
  .folder-menu {
    position: fixed;
    z-index: 40;
    min-width: 132px;
    padding: 4px;
    display: flex;
    flex-direction: column;
    background: var(--surface-solid, rgb(28 30 32));
    border: 1px solid var(--border);
    border-radius: var(--radius-md, 10px);
    box-shadow: 0 12px 32px rgb(0 0 0 / 0.45);
  }
  .menu-item {
    background: none;
    border: 0;
    color: var(--text);
    font: inherit;
    font-size: 13px;
    text-align: left;
    padding: 7px 10px;
    border-radius: var(--radius-sm, 8px);
    cursor: pointer;
  }
  .menu-item:hover {
    background: rgb(255 255 255 / 0.08);
  }
  .menu-item.danger {
    color: #ff8f7a;
  }
</style>
