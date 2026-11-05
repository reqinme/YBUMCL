<script lang="ts">
  // 安装界面：版本列表 + 加载器（多选，跟下载页同一套规则）+ 实例名 + 安装。
  // 下载页用它建新实例；实例详情的「修改」页用它原地换版本/加载器（名字锁死、overwrite）。
  import Dropdown from "./Dropdown.svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import { builtinIcon } from "$lib/version-icon";
  import {
    api,
    type DownloadStrategy,
    type FabricLoaderVersion,
    type InstanceLoader,
    type LoaderEntry,
    type ModLoaderVersion,
    type RemoteVersionManifest
  } from "$lib/api";
  import {
    announce,
    beginTask,
    clearStop,
    endTask,
    pauseTask,
    setTaskOutput,
    setTaskResume,
    takeStop
  } from "$lib/tasks.svelte";
  import { showError } from "$lib/dialogs.svelte";
  import {
    LOADER_NAME,
    LOADER_ORDER,
    PATCH,
    PRIMARY,
    compareVersion,
    supportsLoader,
    tagOf,
    type LoaderKind
  } from "$lib/loader-ranges";

  type Props = {
    gameDir: string;
    /// 给定就锁死实例名（修改页），留空表示新建（下载页）。
    lockedName?: string;
    /// 当前实例已有的基础版本与加载器，用来判断「换加载器」要不要确认、以及预填。
    currentGame?: string | null;
    currentLoaders?: string[];
    /// `_ybmcl.loaders` 原样记录（加载器 + 版本）。老实例可能没有。
    currentLoaderParts?: InstanceLoader[];
    oninstalled?: (name: string) => void;
  };

  let {
    gameDir,
    lockedName = "",
    currentGame = null,
    currentLoaders = [],
    currentLoaderParts = [],
    oninstalled
  }: Props = $props();

  type Channel = "release" | "snapshot" | "all";

  const CHANNELS: { value: Channel; label: string }[] = [
    { value: "release", label: "正式版" },
    { value: "snapshot", label: "快照" },
    { value: "all", label: "全部" }
  ];

  type Choice = { version: string; stable: boolean };

  let manifest = $state<RemoteVersionManifest | null>(null);
  let strategy = $state<DownloadStrategy>("race");
  let threads = $state<number | undefined>(undefined);
  let channel = $state<Channel>("release");
  let pickedVersion = $state("");
  /// 每个加载器有哪些构建；undefined 表示还没查完。
  let entries = $state<Partial<Record<LoaderKind, Choice[]>>>({});
  /// 勾选着的加载器 → 各自选中的版本（空串表示已选中、版本还没拉到）。
  let picked = $state<Partial<Record<LoaderKind, string>>>({});
  /// 每个加载器的拉取状态：失败的那一行显示「未拉取到」，按钮跟着置灰。
  let failed = $state<Partial<Record<LoaderKind, boolean>>>({});
  /// 清单回来是空的：这个版本确实没有，连 chip 一起摘掉。
  let missing = $state<Partial<Record<LoaderKind, boolean>>>({});
  let name = $state(lockedName);
  let busy = $state(false);
  const chosen = $derived(LOADER_ORDER.filter((kind) => picked[kind] !== undefined));
  /// 选中的加载器里还有没拿到版本的（正在拉取或拉取失败）：不能开始安装/修改。
  const waiting = $derived(
    chosen.some((kind) => failed[kind] || !(entries[kind]?.length))
  );

  let query = $state("");
  const versions = $derived.by(() => {
    if (!manifest) return [];
    if (channel === "all") return manifest.versions;
    return manifest.versions.filter((item) => item.type === channel);
  });
  const filtered = $derived.by(() => {
    const key = query.trim().toLowerCase();
    return versions.filter((item) => !key || item.id.toLowerCase().includes(key));
  });

  /// 右栏头部用：选中版本的方块图标与类型标签，和版本列表卡片保持一致。
  const pickedEntry = $derived(versions.find((item) => item.id === pickedVersion) ?? null);
  const pickedType = $derived(pickedEntry?.type === "snapshot" ? "快照" : "正式版");
  const pickedIcon = $derived(
    builtinIcon(pickedEntry?.type === "snapshot" ? "snapshot" : "release", [])
  );
  /// 修改页锁死名字；下载页跟着「版本 + 加载器」自动拼。
  const instanceName = $derived(lockedName || autoName());

  function autoName(): string {
    if (!pickedVersion) return "";
    const tags = chosen
      .filter((kind) => picked[kind])
      .map((kind) => tagOf(kind, picked[kind] ?? ""));
    return [pickedVersion, ...tags].filter(Boolean).join("-");
  }

  function withTimeout<T>(work: Promise<T>, ms = 20000): Promise<T> {
    return Promise.race([
      work,
      new Promise<T>((_, reject) => setTimeout(() => reject(new Error("timeout")), ms))
    ]);
  }

  async function fetchEntries(kind: LoaderKind, game: string): Promise<Choice[]> {
      if (kind === "fabric") {
        const list: FabricLoaderVersion[] = await withTimeout(api.fetchFabricLoaders(game));
        return list.map((item) => ({ version: item.version, stable: true }));
      }
      if (kind === "quilt") {
        const list: FabricLoaderVersion[] = await withTimeout(api.fetchQuiltLoaders(game));
        return list.map((item) => ({ version: item.version, stable: true }));
      }
      if (kind === "legacyfabric") {
        const list: FabricLoaderVersion[] = await withTimeout(api.fetchLegacyFabricLoaders(game));
        return list.map((item) => ({ version: item.version, stable: true }));
      }
      if (kind === "labymod") {
        const stable = await withTimeout(api.fetchLabymodVersions("production"));
        const beta = await withTimeout(api.fetchLabymodVersions("snapshot"));
        const hit = [...stable, ...beta].find((item) => item.game === game);
        if (!hit) return [];
        return [
          { version: hit.version, stable: true },
          { version: `${hit.version}@snapshot`, stable: false }
        ];
      }
      if (kind === "forge" || kind === "neoforge") {
        const list: ModLoaderVersion[] = await withTimeout(api.fetchModloaderVersions(kind, game));
        const sorted = [...list].sort((left, right) => compareVersion(right.version, left.version));
        return sorted.map((item) => ({ version: item.version, stable: !item.beta }));
      }
      const list: LoaderEntry[] = await withTimeout(api.fetchLoaderCatalog(kind, game));
      return list.map((item) => ({ version: item.version, stable: item.stable }));
  }

  /// LiteLoader 只对 1.12.2 及以下开放。
  function legacyLite(game: string | undefined): boolean {
    if (!game) return false;
    const parts = game.split(".").map((n) => Number.parseInt(n, 10) || 0);
    const [major = 0, minor = 0, patch = 0] = parts;
    return major < 1 || (major === 1 && (minor < 12 || (minor === 12 && patch <= 2)));
  }

  /// 这个版本有没有这个加载器：先按静态范围摆出来，清单为空再摘掉。
  function supports(kind: LoaderKind): boolean {
    return supportsLoader(kind, pickedVersion) && !missing[kind];
  }

  function defaultChoice(kind: LoaderKind): string {
    const list = entries[kind] ?? [];
    return (list.find((item) => item.stable) ?? list[0] ?? { version: "" }).version;
  }

  /// 每个「版本 + 加载器」查到的清单缓存；切回来就不再重拉。
  const cache = new Map<string, Choice[]>();

  /// 拉一个加载器；失败只影响它自己。
  async function loadKind(kind: LoaderKind, game: string) {
    const key = `${kind}:${game}`;
    const cached = cache.get(key);
    if (cached) {
      entries = { ...entries, [kind]: cached };
      failed = { ...failed, [kind]: false };
      afterEntries(kind);
      return;
    }
    let list: Choice[];
    try {
      list = await fetchEntries(kind, game);
    } catch {
      if (game === pickedVersion) failed = { ...failed, [kind]: true };
      return;
    }
    if (game !== pickedVersion) return;
    cache.set(key, list);
    if (list.length === 0) {
      missing = { ...missing, [kind]: true };
      const next = { ...picked };
      delete next[kind];
      picked = next;
      entries = { ...entries, [kind]: [] };
      failed = { ...failed, [kind]: false };
      return;
    }
    entries = { ...entries, [kind]: list };
    failed = { ...failed, [kind]: false };
    afterEntries(kind);
  }

  /// 单个加载器的清单刚到：先按实例原样预填，再把失效的版本换掉。
  function afterEntries(kind: LoaderKind) {
    tryPreset(kind);
    syncKind(kind);
  }

  async function probeLoaders() {
    const game = pickedVersion;
    if (!game) return;
    entries = {};
    failed = {};
    missing = {};
    // 实例原本的加载器先摆出来（版本还空着，行里显示「正在拉取中…」），
    // 清单到了再由 tryPreset / syncKind 填上版本。
    if (lockedName && !presetApplied) {
      const seed: Partial<Record<LoaderKind, string>> = {};
      for (const part of presetParts()) {
        const kind = part.loader as LoaderKind;
        if (!LOADER_ORDER.includes(kind) || !supportsLoader(kind, game)) continue;
        if (PRIMARY.includes(kind)) for (const other of PRIMARY) delete seed[other];
        seed[kind] = picked[kind] ?? "";
      }
      if (Object.keys(seed).length > 0) picked = seed;
    }
    await Promise.all(
      LOADER_ORDER.filter((kind) => supportsLoader(kind, game)).map((kind) => loadKind(kind, game))
    );
    presetApplied = true;
    syncPicked();
  }

  /// 只重拉这一个加载器。
  function retry(kind: LoaderKind) {
    if (!pickedVersion) return;
    failed = { ...failed, [kind]: false };
    void loadKind(kind, pickedVersion);
  }

  /// 实例原有的加载器（优先 `_ybmcl.loaders`，老实例退回识别结果）。
  function presetParts(): InstanceLoader[] {
    if (currentLoaderParts.length > 0) return currentLoaderParts;
    return currentLoaders
      .map((item) => item.toLowerCase())
      .filter((item) => LOADER_ORDER.includes(item as LoaderKind))
      .map((item) => ({ loader: item as InstanceLoader["loader"], version: "" }));
  }

  let presetApplied = false;
  /// 某个加载器的清单回来了，而实例原本就有它：按原样勾上（版本还在就用原版本）。
  function tryPreset(kind: LoaderKind) {
    if (presetApplied || !lockedName) return;
    const part = presetParts().find((item) => item.loader === kind);
    if (!part) return;
    const list = entries[kind] ?? [];
    if (list.length === 0) return;
    const keep = list.some((item) => item.version === part.version);
    const next: Partial<Record<LoaderKind, string>> = { ...picked };
    if (PRIMARY.includes(kind)) {
      for (const other of PRIMARY) if (other !== kind) delete next[other];
    }
    next[kind] = keep ? part.version : defaultChoice(kind);
    picked = next;
  }

  /// 单个加载器的清单刚回来：选中的版本不在里面就换成默认的。
  function syncKind(kind: LoaderKind) {
    if (picked[kind] === undefined) return;
    const list = entries[kind] ?? [];
    if (list.length === 0) return;
    const current = picked[kind];
    if (!list.some((item) => item.version === current)) {
      picked = { ...picked, [kind]: defaultChoice(kind) };
    }
  }

  /// 全部清单回来后：把新基础版本没有的项摘掉。
  function syncPicked() {
    const next: Partial<Record<LoaderKind, string>> = { ...picked };
    for (const kind of LOADER_ORDER) {
      if (next[kind] === undefined) continue;
      if (!supports(kind) || (entries[kind]?.length ?? 0) === 0) {
        delete next[kind];
      }
    }
    picked = next;
  }

  function toggleLoader(kind: LoaderKind) {
    if (busy) return;
    if (picked[kind] !== undefined) {
      const rest = { ...picked };
      delete rest[kind];
      picked = rest;
      return;
    }
    if (!supports(kind)) return;
    const next = { ...picked };
    if (PRIMARY.includes(kind)) {
      for (const other of PRIMARY) if (other !== kind) delete next[other];
    }
    // 清单还没回来也先选中，行里显示「正在拉取中…」。
    next[kind] = entries[kind]?.length ? defaultChoice(kind) : "";
    picked = next;
  }

  function setLoaderVersion(kind: LoaderKind, version: string) {
    picked = { ...picked, [kind]: version };
  }

  function clearLoaders() {
    if (busy) return;
    picked = {};
  }

  function pickVersion(id: string) {
    if (pickedVersion === id) return;
    pickedVersion = id;
    void probeLoaders();
  }

  function sameLoaders(next: InstanceLoader[]): boolean {
    const norm = (list: string[]) =>
      list
        .map((item) => item.toLowerCase())
        .filter((item) => item !== "原版" && item !== "minecraft")
        .sort()
        .join(",");
    const currentIds =
      currentLoaderParts.length > 0
        ? currentLoaderParts.map((item) => item.loader)
        : currentLoaders;
    const current = norm(currentIds);
    return current === norm(next.map((item) => item.loader));
  }

  let pendingSwap = $state<{ next: InstanceLoader[] } | null>(null);

  function startInstall() {
    if (!pickedVersion || busy || waiting) return;
    const next: InstanceLoader[] = chosen.map((kind) => ({
      loader: kind,
      version: picked[kind] ?? ""
    }));
    // 换加载器组合先提一句：旧模组可能不适用。只换版本不弹。
    if (
      next.length > 0 &&
      !sameLoaders(next) &&
      (currentLoaders.length > 0 || currentLoaderParts.length > 0)
    ) {
      pendingSwap = { next };
      return;
    }
    void runInstall(next);
  }

  async function runInstall(next: InstanceLoader[]) {
    const target = instanceName;
    if (!target) return;
    busy = true;
    const what = next.length
      ? next.map((item) => LOADER_NAME[item.loader as LoaderKind]).join(" + ")
      : "原版";
    const task = beginTask("安装", `${pickedVersion} · ${what}`);
    try {
      setTaskResume(task, () => void runInstall(next));
      const summary = await api.installGameInstance({
        gameDir,
        gameVersion: pickedVersion,
        instanceName: target,
        loaders: next,
        strategy,
        threads,
        overwrite: Boolean(lockedName)
      });
      setTaskOutput(task, { path: summary.path, kind: "dir" });
      endTask(task, `已装好 ${summary.versionId}`);
      announce(`已装好 ${summary.versionId}`);
      oninstalled?.(summary.versionId);
    } catch (error) {
      const stop = takeStop();
      if (stop === "pause") {
        pauseTask(task, "已暂停，点「继续」接着装");
      } else {
        endTask(task, "安装没有完成", true);
        showError("安装失败", "看日志里的最后几行，可能是网络或者文件被占用。", String(error));
      }
    } finally {
      clearStop();
      busy = false;
    }
  }

  async function loadVersions() {
    try {
      manifest = await api.fetchRemoteVersions("race" as DownloadStrategy);
      if (pickedVersion) return;
      // 修改页要「原地换」：先把实例现在的基础版本选上，别一上来就挑最新快照。
      const current = currentGame
        ? manifest.versions.find((item) => item.id === currentGame)
        : null;
      if (current) {
        if (current.type === "release" || current.type === "snapshot") channel = current.type;
        pickedVersion = current.id;
      } else {
        pickedVersion = manifest.versions[0].id;
      }
      void probeLoaders();
    } catch (error) {
      showError("读取版本清单失败", "连不上版本清单，检查网络或者换个下载源。", String(error));
    }
  }

  $effect(() => {
    if (!manifest && gameDir) void loadVersions();
  });

  $effect(() => {
    void (async () => {
      try {
        const settings = await api.loadSettings();
        strategy = settings.downloadStrategy ?? "race";
        threads = settings.concurrentDownloads || undefined;
      } catch {
        // 读不到设置就按默认的竞速策略装。
      }
    })();
  });
</script>

<div class="install">
  <section class="list">
    <div class="tools">
      <input class="search" placeholder="搜索版本…" bind:value={query} />
      <div class="chips">
        {#each CHANNELS as item (item.value)}
          <button class="chip" class:on={channel === item.value} onclick={() => (channel = item.value)}>
            {item.label}
          </button>
        {/each}
      </div>
    </div>
    {#if filtered.length === 0}
      <p class="dim small">正在读取版本清单…</p>
    {:else}
      <p class="dim small">共 {filtered.length} 个版本</p>
    {/if}
    <ul class="ver-list">
      {#each filtered as entry (entry.id)}
        <li>
          <button
            class="ver-item"
            class:active={pickedVersion === entry.id}
            onclick={() => pickVersion(entry.id)}
          >
            <img
              class="ver-icon"
              src={builtinIcon(entry.type === "snapshot" ? "snapshot" : "release", [])}
              alt=""
            />
            <span class="ver-text">
              <span class="ver-title">{entry.id}</span>
              <span class="ver-meta">{entry.type === "release" ? "正式版" : "快照"}</span>
            </span>
          </button>
        </li>
      {/each}
    </ul>
  </section>

  <section class="detail">
    <header class="ver-head">
      <img class="ver-big" src={pickedIcon} alt="" />
      <div class="ver-info">
        <div class="ver-name">{pickedVersion || "选一个版本"}</div>
        <div class="ver-line">
          <span class="tag">{pickedType}</span>
          {#if currentGame}<span class="tag">当前 {currentGame}</span>{/if}
          {#if lockedName}<span class="tag">{lockedName}</span>{/if}
        </div>
        <p class="ver-desc">
          {lockedName
            ? "换版本或加载器后原地重装，模组与存档会保留。"
            : "选好版本和加载器，实例名会按这个拼出来。"}
        </p>
      </div>
    </header>

    <div class="block">
      <span class="label">加载器</span>
      <div class="chips">
        <button class="chip" class:on={chosen.length === 0} onclick={clearLoaders} disabled={busy}>
          原版
        </button>
        {#each LOADER_ORDER as kind (kind)}
          {#if supports(kind)}
            <button
              class="chip"
              class:on={picked[kind] !== undefined}
              onclick={() => toggleLoader(kind)}
              disabled={busy}
              title={PRIMARY.includes(kind) ? "主加载器同时只能选一个" : undefined}
            >
              {LOADER_NAME[kind]}
            </button>
          {/if}
        {/each}
      </div>
      {#each chosen as kind (kind)}
        <div class="loader-row">
          <span class="loader-name">{LOADER_NAME[kind]}</span>
          {#if failed[kind]}
            <span class="loader-state">未拉取到</span>
            <button class="btn ghost loader-retry" onclick={() => retry(kind)} disabled={busy}>
              重试
            </button>
          {:else if !(entries[kind]?.length)}
            <span class="loader-state">正在拉取中…</span>
          {:else}
            <Dropdown
              value={picked[kind] ?? ""}
              options={(entries[kind] ?? []).map((item) => ({ value: item.version, label: item.version }))}
              onchange={(value) => setLoaderVersion(kind, value)}
              disabled={busy}
            />
          {/if}
        </div>
      {/each}
    </div>

    <label class="field">
      <span class="label">实例名</span>
      <input bind:value={name} readonly={Boolean(lockedName)} />
    </label>

    <div class="foot">
      <button class="btn primary wide" disabled={!pickedVersion || busy || waiting} onclick={startInstall}>
        {busy ? "安装中…" : lockedName ? "开始修改" : "安装"}
      </button>
    </div>
  </section>
</div>

<ConfirmDialog
  open={pendingSwap !== null}
  message="换加载器后旧的模组可能不适用"
  confirmLabel="继续"
  oncancel={() => (pendingSwap = null)}
  onconfirm={() => {
    const next = pendingSwap?.next ?? [];
    pendingSwap = null;
    void runInstall(next);
  }}
/>

<style>
  /* 与下载页同一套双栏：左边搜索 + 结果列表，右边选中项详情 + 操作。 */
  .install {
    display: grid;
    grid-template-columns: 38% minmax(0, 1fr);
    gap: 14px;
    flex: 1;
    min-height: 0;
  }
  .list,
  .detail {
    border: 1px solid var(--border);
    border-radius: 14px;
    background: rgba(16, 19, 26, 0.45);
    padding: 14px;
    display: flex;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
  }
  .list {
    overflow-y: auto;
  }
  .tools {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding-bottom: 10px;
    border-bottom: 1px solid var(--border);
  }
  .search {
    background: rgba(8, 10, 14, 0.45);
    border: 1px solid var(--border);
    border-radius: 10px;
    color: var(--text);
    padding: 8px 10px;
    font-size: 13px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-dim);
    border-radius: 999px;
    padding: 4px 12px;
    font-size: 12px;
    cursor: pointer;
  }
  .chip.on {
    color: #16210f;
    background: var(--yb-green, #9ccb6c);
    border-color: transparent;
  }
  .ver-list {
    list-style: none;
    margin: 10px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .ver-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    border: none;
    background: transparent;
    color: var(--text);
    padding: 8px 10px;
    border-radius: 10px;
    cursor: pointer;
    text-align: left;
  }
  .ver-item:hover {
    background: rgba(255, 255, 255, 0.05);
  }
  .ver-item.active {
    background: rgba(168, 204, 132, 0.16);
  }
  .ver-icon {
    width: 28px;
    height: 28px;
    flex: none;
    object-fit: contain;
    image-rendering: pixelated;
  }
  .ver-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .ver-title {
    font-size: 13px;
  }
  .ver-meta {
    font-size: 12px;
    color: var(--text-dim);
  }
  .ver-head {
    display: flex;
    gap: 14px;
    align-items: flex-start;
    padding-bottom: 14px;
    border-bottom: 1px solid var(--border);
  }
  .ver-big {
    width: 52px;
    height: 52px;
    flex: none;
    object-fit: contain;
    image-rendering: pixelated;
  }
  .ver-info {
    min-width: 0;
  }
  .ver-name {
    font-size: 20px;
    font-weight: 650;
  }
  .ver-line {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 6px;
  }
  .tag {
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 2px 10px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .ver-desc {
    margin: 8px 0 0;
    font-size: 12px;
    color: var(--text-dim);
  }
  .block {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px 0;
    border-bottom: 1px solid var(--border);
  }
  .label {
    font-size: 12px;
    color: var(--text-dim);
  }
  .loader-row {
    display: grid;
    grid-template-columns: 108px minmax(0, 1fr) auto;
    gap: 10px;
    align-items: center;
  }
  .loader-name {
    font-size: 12px;
    color: var(--text-dim);
  }
  .loader-state {
    font-size: 13px;
    color: var(--text-dim);
    padding: 8px 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: rgba(8, 10, 14, 0.35);
  }
  .loader-retry {
    padding: 6px 12px;
    font-size: 12px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-top: 14px;
  }
  .field input {
    background: rgba(8, 10, 14, 0.45);
    border: 1px solid var(--border);
    border-radius: 10px;
    color: var(--text);
    padding: 8px 10px;
    font-size: 13px;
  }
  .foot {
    margin-top: auto;
    padding-top: 14px;
  }
  .wide {
    width: 100%;
    padding: 10px;
  }
  .dim {
    color: var(--text-dim);
  }
  .small {
    font-size: 12px;
  }
</style>
