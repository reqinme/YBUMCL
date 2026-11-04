<script lang="ts">
  // 下载页「Minecraft」那套安装界面：版本列表 + 加载器 + 实例名 + 安装。
  // 下载页用它建新实例；实例详情的「修改」页用它原地换版本/加载器（名字锁死、overwrite）。
  import Dropdown from "./Dropdown.svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import {
    api,
    type DownloadStrategy,
    type FabricLoaderVersion,
    type InstanceLoader,
    type ModLoaderVersion,
    type RemoteVersionManifest,
    type VersionView
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

  type Props = {
    gameDir: string;
    /// 给定就锁死实例名（修改页），留空表示新建（下载页）。
    lockedName?: string;
    /// 当前实例已有的基础版本与加载器，用来判断「换加载器」要不要确认。
    currentGame?: string | null;
    currentLoaders?: string[];
    oninstalled?: (name: string) => void;
  };

  let { gameDir, lockedName = "", currentGame = null, currentLoaders = [], oninstalled }: Props = $props();

  type Channel = "release" | "snapshot" | "all";
  type LoaderId = "原版" | "Fabric" | "Quilt" | "Forge" | "NeoForge" | "OptiFine";

  const CHANNELS: { value: Channel; label: string }[] = [
    { value: "release", label: "正式版" },
    { value: "snapshot", label: "快照" },
    { value: "all", label: "全部" }
  ];
  const LOADERS: LoaderId[] = ["原版", "Fabric", "Quilt", "Forge", "NeoForge", "OptiFine"];
  /// 界面上的中文加载器名 → 后端标识。
  const LOADER_ID: Record<Exclude<LoaderId, "原版">, InstanceLoader["loader"]> = {
    Fabric: "fabric",
    Quilt: "quilt",
    Forge: "forge",
    NeoForge: "neoforge",
    OptiFine: "optifine"
  };
  const LOADER_LABEL: Record<string, string> = {
    fabric: "Fabric",
    quilt: "Quilt",
    forge: "Forge",
    neoforge: "NeoForge",
    optifine: "OptiFine",
    原版: "原版"
  };

  /// 实例已有的加载器名（VersionView.loaders）→ 界面上的加载器选项。
  const FROM_LOADER: Record<string, LoaderId> = {
    fabric: "Fabric",
    quilt: "Quilt",
    forge: "Forge",
    neoforge: "NeoForge",
    optifine: "OptiFine"
  };

  let manifest = $state<RemoteVersionManifest | null>(null);
  /// 和下载页一致：安装走设置里选的下载策略（默认竞速）与线程数。
  let strategy = $state<DownloadStrategy>("race");
  let threads = $state<number | undefined>(undefined);
  let channel = $state<Channel>("release");
  let pickedVersion = $state("");
  let pickedLoader = $state<LoaderId>("原版");
  let loaderVersions = $state<{ version: string; label: string }[]>([]);
  let loaderVersion = $state<string>("");
  let loadingLoaders = $state(false);
  let name = $state(lockedName);
  let busy = $state(false);
  let pendingSwap = $state<{ loader: InstanceLoader | null } | null>(null);

  let query = $state("");
  const filtered = $derived.by(() => {
    const key = query.trim().toLowerCase();
    return versions.filter((item) => !key || item.id.toLowerCase().includes(key));
  });

  const versions = $derived.by(() => {
    if (!manifest) return [];
    if (channel === "all") return manifest.versions;
    return manifest.versions.filter((item) => item.type === channel);
  });
  /// 实例名：修改页锁死；下载页跟着「版本 + 加载器」自动拼，和下载页原来的规则一致。
  const instanceName = $derived.by(() => {
    if (lockedName) return lockedName;
    return autoName(pickedVersion, pickedLoader);
  });

  function autoName(version: string, loader: LoaderId): string {
    if (!version) return "";
    if (loader === "原版") return version;
    return `${version}-${loader}_${loaderVersion || "latest"}`;
  }

  async function loadVersions() {
    try {
      manifest = await api.fetchRemoteVersions("race" as DownloadStrategy);
      if (!pickedVersion && manifest.versions.length > 0) {
        // 修改页要「原地换」：先把实例现在的基础版本选上，别一上来就挑最新快照。
        const current = currentGame ? manifest.versions.find((item) => item.id === currentGame) : null;
        if (current) {
          if (current.type === "release" || current.type === "snapshot") channel = current.type;
          pickedVersion = current.id;
        } else {
          pickedVersion = manifest.versions[0].id;
        }
        const existing = currentLoaders.map((item) => FROM_LOADER[item.toLowerCase()]).find(Boolean);
        if (lockedName && existing && pickedLoader === "原版") {
          pickedLoader = existing;
          void pickLoader(existing);
        }
      }
    } catch (error) {
      showError("读取版本清单失败", "连不上版本清单，检查网络或者换个下载源。", String(error));
    }
  }

  async function pickLoader(loader: LoaderId) {
    pickedLoader = loader;
    loaderVersions = [];
    loaderVersion = "";
    if (loader === "原版" || !pickedVersion) return;
    loadingLoaders = true;
    try {
      const id = LOADER_ID[loader as Exclude<LoaderId, "原版">];
      if (id === "fabric") {
        const list: FabricLoaderVersion[] = await api.fetchFabricLoaders(pickedVersion);
        loaderVersions = list.map((item) => ({ version: item.version, label: item.version }));
      } else if (id === "quilt") {
        const list: FabricLoaderVersion[] = await api.fetchQuiltLoaders(pickedVersion);
        loaderVersions = list.map((item) => ({ version: item.version, label: item.version }));
      } else {
        const list: ModLoaderVersion[] = await api.fetchModloaderVersions(id, pickedVersion);
        loaderVersions = list.map((item) => ({ version: item.version, label: item.version }));
      }
      loaderVersion = loaderVersions[0]?.version ?? "";
    } catch (error) {
      showError(`读取 ${loader} 版本失败`, "这个加载器没有给出可用版本。", String(error));
    } finally {
      loadingLoaders = false;
    }
  }

  function sameLoaders(next: InstanceLoader[]): boolean {
    const norm = (list: string[]) =>
      list
        .map((item) => item.toLowerCase())
        .filter((item) => item !== "原版" && item !== "minecraft" && item !== "optifine-x")
        .sort()
        .join(",");
    const current = norm(currentLoaders);
    const wanted = next.map((item) => item.loader).sort().join(",");
    return current === wanted;
  }

  async function startInstall() {
    if (!pickedVersion || busy) return;
    const next: InstanceLoader[] = pickedLoader === "原版"
      ? []
      : [{ loader: LOADER_ID[pickedLoader as Exclude<LoaderId, "原版">], version: loaderVersion }];
    // 换加载器（不是换版本）先提一句：旧模组可能不适用。同一加载器换版本不弹。
    const changedLoader = next.length > 0 && !sameLoaders(next);
    if (changedLoader && currentLoaders.length > 0) {
      pendingSwap = { loader: next[0] };
      return;
    }
    await runInstall(next);
  }

  async function runInstall(next: InstanceLoader[]) {
    const target = instanceName;
    if (!target) return;
    busy = true;
    const task = beginTask("安装", `${pickedVersion} · ${next.length ? next[0].loader : "原版"}`);
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

<section class="install">
  <div class="col">
    <input class="search" placeholder="搜索版本…" bind:value={query} />
    <div class="chips">
      {#each CHANNELS as item (item.value)}
        <button class="chip" class:on={channel === item.value} onclick={() => (channel = item.value)}>
          {item.label}
        </button>
      {/each}
    </div>
    <div class="list">
      {#each filtered as entry (entry.id)}
        <button class="entry" class:on={pickedVersion === entry.id} onclick={() => (pickedVersion = entry.id)}>
          <span>{entry.id}</span>
          <span class="kind">{entry.type === "release" ? "正式版" : "快照"}</span>
        </button>
      {/each}
    </div>
  </div>

  <div class="col main">
    <h2>{pickedVersion || "选一个版本"}</h2>
    <div class="chips">
      {#each LOADERS as loader (loader)}
        <button class="chip" class:on={pickedLoader === loader} onclick={() => void pickLoader(loader)}>
          {loader}
        </button>
      {/each}
    </div>
    {#if pickedLoader !== "原版"}
      <div class="field">
        <Dropdown
          bind:value={loaderVersion}
          label={`${pickedLoader} 版本`}
          options={loaderVersions.map((item) => ({ value: item.version, label: item.label }))}
        />
        {#if loadingLoaders}<span class="dim">正在读取可用版本…</span>{/if}
      </div>
    {/if}
    <label class="field">
      <span>实例名</span>
      <input bind:value={name} readonly={Boolean(lockedName)} />
    </label>
    <button class="btn primary" disabled={!pickedVersion || busy} onclick={() => void startInstall()}>
      {busy ? "安装中…" : "安装"}
    </button>
  </div>
</section>

<ConfirmDialog
  open={pendingSwap !== null}
  message="换加载器后旧的模组可能不适用"
  confirmLabel="继续"
  oncancel={() => (pendingSwap = null)}
  onconfirm={() => {
    const next = pendingSwap?.loader ? [pendingSwap.loader] : [];
    pendingSwap = null;
    void runInstall(next);
  }}
/>

<style>
  .install {
    display: grid;
    grid-template-columns: minmax(0, 260px) minmax(0, 1fr);
    gap: 16px;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
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
  .list {
    border: 1px solid var(--border);
    border-radius: 12px;
    overflow-y: auto;
    max-height: 420px;
  }
  .entry {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    width: 100%;
    border: none;
    background: transparent;
    color: var(--text);
    padding: 7px 12px;
    font-size: 13px;
    cursor: pointer;
    text-align: left;
  }
  .entry:hover {
    background: rgba(255, 255, 255, 0.05);
  }
  .entry.on {
    background: rgba(168, 204, 132, 0.18);
  }
  .kind {
    color: var(--text-dim);
    font-size: 12px;
  }
  .main h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .field input {
    background: rgba(8, 10, 14, 0.45);
    border: 1px solid var(--border);
    border-radius: 10px;
    color: var(--text);
    padding: 8px 10px;
    font-size: 13px;
  }
  .dim {
    color: var(--text-dim);
    font-size: 12px;
  }
</style>
