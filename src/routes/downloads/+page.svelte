<script lang="ts">
  import type { RefDetail } from "$lib/api";
  import { trackPack, untrackPack } from "$lib/pack-live";
  import { onMount, onDestroy, tick } from "svelte";
  import { registerDetailBack } from "$lib/page-history.svelte";
  import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
  import { cachedJava, cachedManifest, forgetScan } from "$lib/detect";
  import {
    announce,
    beginTask,
    clearStop,
    continueTask,
    endTask,
    markStop,
    pauseTask,
    requeueTask,
    resumeTask,
    setTaskOutput,
    setTaskResume,
    submitTask,
    setTaskNote,
    setTaskPercent,
    setTaskProgress,
    setTaskSpeed,
    takeStop,
    type Task,
    type TaskResult
  } from "$lib/tasks.svelte";
  import { showError } from "$lib/dialogs.svelte";
  import { trackInstall, untrackInstall } from "$lib/install-live";
  import { launchInstallFlight, setBallColor } from "$lib/install-flight.svelte";
  import { supportsLoader, type LoaderKind } from "$lib/loader-ranges";
  import Dropdown from "$lib/components/Dropdown.svelte";
  import Segmented from "$lib/components/Segmented.svelte";
  import { SIDES, TAGS, type SideFilter, type TagOption } from "$lib/content-tags";
  import ProjectIcon from "$lib/components/ProjectIcon.svelte";
  import { badgeOf, foolAnchor, foolOf } from "$lib/version-types";
  import { currentVersion } from "$lib/current-version.svelte";
  import { downloadsState, saveDownloadsState, type CatFilters, type DownloadsState } from "$lib/downloads-state.svelte";
  import {
    buildFilters,
    categoryTags,
    compareVersionName,
    computeFlags,
    environmentLabel,
    fileSupports,
    filesMatchingLeft,
    groupFiles,
    leftFilterTitle,
    loaderName,
    type EnvironmentFilter,
    type FileVersion,
    type LeftFilters
  } from "$lib/mod-files";
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import {
    api,
    type ChosenExtra,
    type CleanroomExtraMod,
    type DownloadStrategy,
    type FabricLoaderVersion,
    type InstallSummary,
    type InstanceLoader,
    type JavaInstallation,
    type JavaPlan,
    type LoaderEntry,
    type ModLoaderVersion,
    type FavoriteFolder,
    type FavoriteEntry,
    type Favorites,
    type MrProject,
    type MrVersion,
    type RemoteVersion,
    type Settings,
    type VersionView
  } from "$lib/api";

  type Loader =
    | "vanilla"
    | "fabric"
    | "legacyfabric"
    | "labymod"
    | "quilt"
    | "forge"
    | "neoforge"
    | "cleanroom"
    | "liteloader"
    | "optifine";

  const LOADER_NAME: Record<string, string> = {
    forge: "Forge",
    neoforge: "NeoForge",
    fabric: "Fabric",
    legacyfabric: "Legacy Fabric",
    labymod: "LabyMod",
    quilt: "Quilt",
    cleanroom: "Cleanroom",
    liteloader: "LiteLoader",
    optifine: "OptiFine"
  };

  let versions = $state<RemoteVersion[]>([]);
  let filter = $state<"release" | "snapshot" | "fools" | "all">("release");
  let query = $state("");
  let selected = $state<RemoteVersion | null>(null);
  let gameDir = $state("");

  // PCL2 的做法：勾选的加载器一起装进同一个实例，名字由原版号 + 加载器拼出来。
  const LOADER_ORDER: Loader[] = [
    "fabric",
    "legacyfabric",
    "labymod",
    "quilt",
    "forge",
    "neoforge",
    "cleanroom",
    "liteloader",
    "optifine"
  ];
  /// 互斥的主加载器：它们抢同一个注入点，同时只能留一个。
  const PRIMARY: Loader[] = [
    "fabric",
    "legacyfabric",
    "labymod",
    "quilt",
    "forge",
    "neoforge",
    "cleanroom"
  ];
  /// OptiFine 是补丁型加载器，可以跟任意主加载器叠在一起。
  const PATCH: Loader = "optifine";
  /// 每个加载器对自己的构建通道有官方叫法，界面跟着它走，不统一改成「正式版/测试版」。
  const CHANNEL: Record<Loader, { stable: string; beta: string }> = {
    vanilla: { stable: "正式版", beta: "测试版" },
    fabric: { stable: "稳定版", beta: "测试版" },
    legacyfabric: { stable: "稳定版", beta: "测试版" },
    quilt: { stable: "稳定版", beta: "测试版" },
    forge: { stable: "正式版", beta: "测试版" },
    neoforge: { stable: "稳定版", beta: "测试版" },
    cleanroom: { stable: "正式版", beta: "测试版" },
    liteloader: { stable: "正式版", beta: "快照" },
    optifine: { stable: "正式版", beta: "预览版" },
    labymod: { stable: "正式版", beta: "快照版" }
  };

  type Choice = {
    version: string;
    stable: boolean;
    beta: boolean;
    recommended: boolean;
    /// 这个游戏版本里最新的那个构建（Forge / NeoForge 用「最新版」徽标标出来）。
    latest?: boolean;
    /// 发行日期 `YYYY-MM-DD`，拿不到就是 undefined。
    date?: string;
  };

  let picked = $state<Partial<Record<Loader, string>>>({});
  /// 当前游戏版本下每个加载器有哪些构建；undefined 表示还没查完。
  let entries = $state<Partial<Record<Loader, Choice[]>>>({});
  /// 版本下拉上面的三段筛选（默认「全部」）。
  let typeFilter = $state<Partial<Record<Loader, "all" | "stable" | "beta">>>({});
  /// 展开中的版本下拉。
  let openMenu = $state<Loader | null>(null);
  /// 每个版本对应的发行日期，按加载器分组。
  let dates = $state<Partial<Record<Loader, Record<string, string>>>>({});
  /// 清单按 (加载器, 游戏版本) 缓存，切换回来不用重查。
  const probes = new Map<string, Choice[]>();
  /// 拉取失败（超时/断网）：那一行显示「未拉取到」，按钮跟着置灰。
  let failed = $state<Partial<Record<Loader, boolean>>>({});
  /// 清单回来是空的：这个版本确实没有，连 chip 一起摘掉。
  let missing = $state<Partial<Record<Loader, boolean>>>({});
  let active = $state<Loader | null>(null);
  let instName = $state("");
  let nameTouched = $state(false);
  let nameTaken = $state(false);
  let javaPath = $state("");
  let javas = $state<JavaInstallation[]>([]);
  // 安装器用的 Java 不让人手选：由后端按「原版基线 + 加载器收窄」算出区间再挑。
  let javaPlan = $state<JavaPlan | null>(null);
  const javaHint = $derived(javaPlan ? `${javaPlan.description}${javaPlan.path ? "" : "（未安装）"}` : "");

  let settings = $state<Settings | null>(null);
  /** 下载源策略：有镜像的文件按它处理，没镜像的一律走官方。 */
  const strategy = $derived<DownloadStrategy>(settings?.downloadStrategy ?? "sequential");
  const threads = $derived(settings?.concurrentDownloads ?? 64);

  let loading = $state(true);
  /// 命中缓存时清单的年龄；0 表示刚拉的。
  let manifestAgeMs = $state(0);
  /// 正在跑的安装（可以不止一个）的任务卡 id；面板上的暂停/取消对着最近开始的那个。
  let activeInstalls = $state<number[]>([]);
  let summary = $state<InstallSummary | null>(null);
  /// 最近一次开始的安装任务 id：面板与暂停/取消都对着它，进度推到对应的任务卡上。
  let installTaskId = $state<number | null>(null);
  /// 安装按钮本体：光球从它的中心起飞。
  let installButton = $state<HTMLButtonElement | null>(null);
  let unlisten: Array<() => void> = [];

  /// 清单顺序当底子，愚人节版挪到它基于的正式版后面（否则按名字会飞到最上面）。
  const orderedVersions = $derived.by(() => {
    const indexOf = new Map(versions.map((item, index) => [item.id, index]));
    return versions
      .map((item, index) => ({ item, key: foolAnchor(item.id, indexOf) ?? index }))
      .sort((a, b) => a.key - b.key)
      .map((entry) => entry.item);
  });

  const shown = $derived(
    orderedVersions.filter((v) => {
      const fool = foolOf(v.id);
      if (filter === "fools") {
        if (!fool) return false;
      } else if (filter === "snapshot") {
        // 愚人节单独一段，快照段里不再重复出现
        if (fool || v.type !== "snapshot") return false;
      } else if (filter !== "all" && v.type !== filter) {
        return false;
      }
      return !query || v.id.toLowerCase().includes(query.trim().toLowerCase());
    })
  );

  const chosen = $derived(LOADER_ORDER.filter((item) => picked[item] !== undefined));
  /// 一个加载器都没选：这次是装纯原版。
  const plain = $derived(chosen.length === 0);
  /// 已选的加载器里还有没拿到版本的（正在拉取或拉取失败）：不能开始安装。
  const waiting = $derived(chosen.some((kind) => failed[kind] || !(entries[kind]?.length)));
  /// 某个已选加载器在当前筛选段里没有任何版本（右侧会显示「无」）。
  const missingBuild = $derived(
    LOADER_ORDER.filter((kind) => picked[kind] !== undefined && visible(kind).length === 0)
  );
  const isModloader = $derived(
    chosen.some(
      (item) =>
        item === "forge" ||
        item === "neoforge" ||
        item === "cleanroom" ||
        item === "liteloader" ||
        item === "optifine"
    )
  );

  /// 实例名里用的标记 = 核心层 LoaderKind::tag（不带空格的驼峰写法）。
  const LOADER_TAG: Record<string, string> = {
    forge: "Forge",
    neoforge: "NeoForge",
    fabric: "Fabric",
    legacyfabric: "LegacyFabric",
    labymod: "LabyMod",
    quilt: "Quilt",
    cleanroom: "Cleanroom",
    liteloader: "LiteLoader",
    optifine: "OptiFine"
  };

  function tagOf(kind: Loader, version: string): string {
    const tag = LOADER_TAG[kind] ?? kind;
    if (kind === "liteloader") return tag;
    if (kind === "labymod") {
      const [base, channel] = version.split("@");
      return channel === "snapshot" ? `${tag}_${base}_Snapshot` : `${tag}_${base}`;
    }
    return `${tag}_${version.replaceAll(" ", "_")}`;
  }

  /// 与核心层 merge::instance_name 一致的默认名。
  const defaultName = $derived(
    [
      selected?.id ?? "",
      ...chosen.filter((kind) => picked[kind]).map((kind) => tagOf(kind, picked[kind] ?? ""))
    ]
      .filter(Boolean)
      .join("-")
  );

  $effect(() => {
    if (!nameTouched) instName = defaultName;
  });

  $effect(() => {
    const name = instName.trim();
    if (!name || !gameDir) {
      nameTaken = false;
      return;
    }
    const timer = setTimeout(async () => {
      try {
        nameTaken = await api.instanceNameTaken(gameDir, name);
      } catch {
        nameTaken = false;
      }
    }, 250);
    return () => clearTimeout(timer);
  });

  function formatBytes(bytes: number): string {
    if (!bytes) return "0 B";
    const units = ["B", "KB", "MB", "GB"];
    const index = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
    return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
  }

  async function loadManifest(force = false) {
    loading = true;
    try {
      settings = await api.loadSettings().catch(() => null);
      setBallColor(settings?.installBallColor);
          const [cached, dir] = await Promise.all([
        cachedManifest(strategy, force),
        api.defaultGameDir()
      ]);
      const manifest = cached.value;
      manifestAgeMs = cached.ageMs;
      versions = manifest.versions;
      gameDir = dir;
      instances = await api.listVersions().catch(() => []);
      // 打开页面不预选任何版本，右侧保持占位；只有用户点列表才选中。
      // 刷新清单后如果选中的版本没了，就退回占位状态。
      const kept = selected;
      if (kept && !versions.some((item) => item.id === kept.id)) selected = null;
      if (force) announce("清单已刷新");
      // 版本列表先显示出来，Java 探测在后台继续（它要扫好几个目录，可能几秒）。
      void (async () => {
        try {
          javas = (await cachedJava(force)).value;
          javaPath ||= javas[0]?.path ?? "";
          await refreshJavaPlan();
        } catch {
          /* Java 探测失败不影响原版安装 */
        }
      })();
    } catch (e) {
      showError("读取版本清单失败", "没有连上 Mojang 的版本清单。", String(e));
    } finally {
      loading = false;
    }
  }

  async function refreshJavaPlan(force = false) {
    if (!selected) return;
    const main = chosen.find((item) => item !== "optifine") ?? chosen[0];
    try {
      if (force) javas = (await cachedJava(true)).value;
      javaPlan = await api.planJava(
        selected.id,
        main,
        main ? picked[main] || undefined : undefined
      );
    } catch (e) {
      javaPlan = null;
      showError("Java 需求算不出来", "这个加载器需要先确定 Java 版本。", String(e));
    }
  }

  /// 游戏版本不高于 1.12.2 才允许 LiteLoader（它是 1.12.2 时代的老加载器）。
  function legacyLite(game: string | undefined): boolean {
    if (!game) return false;
    const parts = game.split(".").map((n) => Number.parseInt(n, 10) || 0);
    const [major = 0, minor = 0, patch = 0] = parts;
    return major < 1 || (major === 1 && (minor < 12 || (minor === 12 && patch <= 2)));
  }

  /// 探测清单时给每个源 20 秒上限：某个站连不上不能把整个选择器拖住
  /// （Legacy Fabric 的清单有 250 KB，慢的时候要十几秒）。
  function withTimeout<T>(work: Promise<T>, ms = 20000): Promise<T> {
    return Promise.race([
      work,
      new Promise<T>((_, reject) => setTimeout(() => reject(new Error("timeout")), ms))
    ]);
  }

  /// 不管上游接口怎么排，统一成「新→旧」，并把最上面那条标成最新版。
  ///
  /// Quilt 的接口顺序是乱的（0.20.0-beta.9 排在 0.29.2 前面），Fabric 的倒是新→旧，
  /// 所以不能信上游，出口统一排一遍。同版本号时正式版排前面。
  function ordered(entries: Choice[]): Choice[] {
    const sorted = [...entries].sort((left, right) => {
      const byVersion = compareVersion(right.version, left.version);
      if (byVersion !== 0) return byVersion;
      if (left.stable !== right.stable) return left.stable ? -1 : 1;
      return 0;
    });
    return sorted.map((item, index) => ({ ...item, latest: index === 0 }));
  }

  /// 拉某个加载器在当前游戏版本下的构建列表。
  async function fetchEntries(kind: Loader, game: string): Promise<Choice[]> {
      if (kind === "fabric" || kind === "legacyfabric" || kind === "quilt") {
        const list: FabricLoaderVersion[] =
          kind === "fabric"
            ? await withTimeout(api.fetchFabricLoaders(game, strategy))
            : kind === "legacyfabric"
              ? await withTimeout(api.fetchLegacyFabricLoaders(game))
              : await withTimeout(api.fetchQuiltLoaders(game));
        return ordered(
          list.map((item) => ({
            version: item.version,
            stable: item.stable,
            beta: !item.stable,
            recommended: false
          }))
        );
      }
      if (kind === "labymod") {
        const stable = await withTimeout(api.fetchLabymodVersions("production"));
        const beta = await withTimeout(api.fetchLabymodVersions("snapshot"));
        const chosen = [...stable, ...beta].find((item) => item.game === game);
        if (!chosen) return [];
        return [
          { version: `${chosen.version}@snapshot`, stable: false, beta: true, recommended: false },
          // 正式版的版本号不带频道后缀，排在前面并作为默认。
          { version: chosen.version, stable: true, beta: false, recommended: true }
        ];
      }
      if (kind === "forge" || kind === "neoforge") {
        const list: ModLoaderVersion[] = await withTimeout(api.fetchModloaderVersions(kind, game));
        // 通道只看版本号里有没有 beta/alpha；「推荐版」「最新版」是徽标，不是通道：
        // 后端把官方 promotions 的那一个标成 recommended，最新版就是排序后的第一个。
        const sorted = [...list].sort((left, right) => compareVersion(right.version, left.version));
        return ordered(
          sorted.map((item) => ({
            version: item.version,
            stable: !item.beta,
            beta: !!item.beta,
            recommended: !!item.recommended
          }))
        );
      }
      const list: LoaderEntry[] = await withTimeout(api.fetchLoaderCatalog(kind, game));
      return ordered(
        list.map((item) => ({
          version: item.version,
          stable: item.stable,
          beta: !item.stable,
          recommended: false
        }))
      );
  }

  /// 选中游戏版本后，按静态范围并发查所有加载器；范围里没有的连请求都不发。
  async function probeLoaders() {
    const game = selected?.id;
    if (!game) return;
    failed = {};
    missing = {};
    entries = {};
    await Promise.all(
      LOADER_ORDER.filter((kind) => supportsLoader(kind as LoaderKind, game)).map(async (kind) => {
        const key = `${kind}:${game}`;
        let list = probes.get(key);
        if (!list) {
          try {
            list = await fetchEntries(kind, game);
          } catch {
            if (selected?.id === game) failed = { ...failed, [kind]: true };
            return;
          }
          if (selected?.id !== game) return;
          probes.set(key, list);
        }
        if (list.length === 0) {
          missing = { ...missing, [kind]: true };
          const rest = { ...picked };
          delete rest[kind];
          picked = rest;
          return;
        }
        entries = { ...entries, [kind]: list };
        void loadDates(kind, game, list);
      })
    );
  }

  /// 只重拉这一个加载器。
  function retry(kind: Loader) {
    const game = selected?.id;
    if (!game) return;
    failed = { ...failed, [kind]: false };
    probes.delete(`${kind}:${game}`);
    void (async () => {
      try {
        const list = await fetchEntries(kind, game);
        if (selected?.id !== game) return;
        probes.set(`${kind}:${game}`, list);
        if (list.length === 0) {
          missing = { ...missing, [kind]: true };
          const rest = { ...picked };
          delete rest[kind];
          picked = rest;
          return;
        }
        entries = { ...entries, [kind]: list };
        void loadDates(kind, game, list);
      } catch {
        if (selected?.id === game) failed = { ...failed, [kind]: true };
      }
    })();
  }

  /// 这个版本有没有这个加载器：先按静态范围摆出来，清单为空再摘掉。
  function supports(kind: Loader): boolean {
    return supportsLoader(kind as LoaderKind, selected?.id ?? "") && !missing[kind];
  }

  function groupsOf(kind: Loader): { stable: Choice[]; beta: Choice[] } {
    const list = entries[kind] ?? [];
    return { stable: list.filter((item) => !item.beta), beta: list.filter((item) => item.beta) };
  }

  /// 三段筛选之后真正可选的版本（正式版在前，测试版在后）。
  function visible(kind: Loader): Choice[] {
    const list = entries[kind] ?? [];
    const { stable, beta } = groupsOf(kind);
    const mode = typeFilter[kind] ?? "all";
    if (mode === "stable") return stable;
    if (mode === "beta") return beta;
    return list;
  }

  /// 默认挑：正式版优先（有推荐版就选推荐版），没有正式版才用测试版。
  function defaultChoice(kind: Loader): string {
    const { stable, beta } = groupsOf(kind);
    typeFilter[kind] = typeFilter[kind] ?? "all";
    const pool = stable.length ? stable : beta;
    return (pool.find((item) => item.recommended) ?? pool[0] ?? { version: "" }).version;
  }

  /// 版本号按数字段比较（`47.4.28` > `47.4.9`），非数字段退化成字符串比较。
  function compareVersion(left: string, right: string): number {
    const a = left.split(/[.-]/);
    const b = right.split(/[.-]/);
    for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
      const one = a[index] ?? "";
      const two = b[index] ?? "";
      const na = Number.parseInt(one, 10);
      const nb = Number.parseInt(two, 10);
      if (!Number.isNaN(na) && !Number.isNaN(nb)) {
        if (na !== nb) return na - nb;
        continue;
      }
      const order = one.localeCompare(two);
      if (order !== 0) return order;
    }
    return 0;
  }

  /// 发行日期另发一批 HEAD 请求，回来后就地补进 entries，界面不卡。
  async function loadDates(kind: Loader, game: string, list: Choice[]) {
    if (dates[kind]) return;
    const versions = list.map((item) => item.version);
    if (versions.length === 0) return;
    try {
      const found = await withTimeout(api.fetchLoaderDates(kind, game, versions), 30000);
      const map: Record<string, string> = {};
      for (const item of found) map[item.version] = item.date;
      dates = { ...dates, [kind]: map };
    } catch {
      dates = { ...dates, [kind]: {} };
    }
  }

  function dateOf(kind: Loader, version: string): string | undefined {
    return dates[kind]?.[version];
  }

  function labelOf(kind: Loader, item: Choice): string {
    const words = [item.beta ? CHANNEL[kind].beta : CHANNEL[kind].stable];
    // 推荐版只有 Forge 有官方来源（promotions_slim.json 的 recommended），
    // NeoForge 没有官方推荐，只标最新版。
    if (kind === "forge" && item.recommended) words.push("推荐版");
    if (item.latest) words.push("最新版");
    return `${item.version}（${words.join("·")}）`;
  }

  function toggleLoader(kind: Loader) {
    active = kind;
    if (picked[kind] !== undefined) {
      const rest = { ...picked };
      delete rest[kind];
      picked = rest;
      void refreshJavaPlan();
      return;
    }
    if (!supports(kind)) return;
    const next = { ...picked };
    // 主加载器单选：选了新的就把其它主加载器顶掉，OptiFine 保留。
    if (PRIMARY.includes(kind)) {
      for (const item of PRIMARY) if (item !== kind) delete next[item];
      void next[PATCH];
    }
    typeFilter[kind] = "all";
    // 清单还没回来也先选中，行里显示「正在拉取中…」。
    next[kind] = entries[kind]?.length ? defaultChoice(kind) : "";
    picked = next;
    nameTouched = false;
    void refreshJavaPlan();
  }

  function setFilter(kind: Loader, mode: "all" | "stable" | "beta") {
    typeFilter[kind] = mode;
    const pool = visible(kind);
    if (pool.length === 0) return;
    // 当前选择还在这段筛选里就不动它。
    const current = picked[kind];
    if (current && pool.some((item) => item.version === current)) return;
    const next = pool.find((item) => item.recommended) ?? pool[0];
    if (next) setLoaderVersion(kind, next.version);
  }

  function setLoaderVersion(kind: Loader, version: string) {
    picked = { ...picked, [kind]: version };
    nameTouched = false;
    void refreshJavaPlan();
  }

  function clearLoaders() {
    picked = {};
    active = null;
    nameTouched = false;
    void refreshJavaPlan();
  }

  /// 加载器清单回来得比点击晚的时候，`picked` 里是空串；等清单到了补上默认版本，
  /// 否则安装请求会带着空版本号被后端拒掉。
  $effect(() => {
    const next = { ...picked };
    let changed = false;
    for (const kind of chosen) {
      if (!next[kind] && entries[kind]?.length) {
        next[kind] = defaultChoice(kind);
        changed = true;
      }
    }
    if (changed) picked = next;
  });

  $effect(() => {
    if (chosen.includes("cleanroom") && selectedGame) void loadCleanroomExtras(selectedGame);
  });

  /// 一次安装请求的参数快照：「继续」要拿它原样重跑，不能读界面上的当前选择
  /// （那会儿用户可能已经换了版本或加载器）。
  type InstallPlan = {
    name: string;
    gameVersion: string;
    loaders: InstanceLoader[];
    javaPath: string;
    detail: string;
  };

  const plans = new Map<number, InstallPlan>();
  /// 实例名 → 暂停中的任务：同名再点「安装」就是继续，而不是报「名字重复了」。
  let pausedNames = $state<Record<string, number>>({});

  async function install() {
    if (!selected) return;
    // 名字就是落盘目录名：改了就另建一份，没改就就地装这个版本。
    const name = instName.trim() || defaultName;
    if (!name) throw new Error("请填实例名");
    if (chosen.some((kind) => !picked[kind])) {
      showError("加载器还没选好", "等一下让版本清单加载完，再点安装。");
      return;
    }
    const pausedTask = pausedNames[name];
    if (pausedTask) {
      continueTask(pausedTask);
      return;
    }
    if (nameTaken) {
      showError("名字重复了", `版本「${name}」已经存在，换一个名字再装。`);
      return;
    }
    // 同一个实例名已经在装了：不新建任务，直接把那张卡亮出来。
    const running = activeInstalls.find((id) => plans.get(id)?.name === name);
    if (running !== undefined) {
      installTaskId = running;
      announce(`「${name}」正在安装，任务卡已经在这里了`);
      return;
    }
    const plan: InstallPlan = {
      name,
      gameVersion: selected.id,
      loaders: chosen.map(
        (kind) => ({ loader: kind, version: picked[kind] ?? "" }) as InstanceLoader
      ),
      javaPath: isModloader ? javaPlan?.path ?? "" : "",
      detail: `${selected.id}${chosen.length > 0 ? ` · ${chosen.join(" + ")}` : ""}`
    };
    const taskId = beginTask(`安装 ${name}`, plan.detail);
    plans.set(taskId, plan);
    setTaskResume(taskId, () => void runInstall(taskId));
    await runInstall(taskId);
  }

  /// 真正跑一次安装。首次安装和「继续」都走这里：同一套参数、同一个任务卡。
  async function runInstall(taskId: number) {
    const plan = plans.get(taskId);
    if (!plan) return;
    const { name } = plan;
    if (installTaskId !== taskId) resumeTask(taskId);
    installTaskId = taskId;
    activeInstalls = [...activeInstalls.filter((id) => id !== taskId), taskId];
    trackInstall(taskId);
    clearStop();
    pausedRun = false;
    summary = null;
    announce(`开始安装 ${name} · 正在下载游戏文件`);
    try {
      if (plan.javaPath === "" && plan.loaders.length > 0 && isModloader && !javaPlan?.path) {
        throw new Error(javaPlan?.message ?? "没有检测到可用的 Java");
      }
      // 同名就地装走普通版本安装；改了名（哪怕没选加载器）都要另立一份。
      summary =
        plan.loaders.length === 0 && plan.name === plan.gameVersion
          ? await api.installGameVersion(
              {
                gameDir,
                versionId: plan.gameVersion,
                strategy,
                threads
              },
              taskId
            )
          : await api.installGameInstance(
              {
                gameDir,
                gameVersion: plan.gameVersion,
                instanceName: name,
                loaders: plan.loaders,
                javaPath: plan.javaPath,
                strategy,
                threads,
                cleanroomMods: chosenExtras()
              },
              taskId
            );
      // 刚装完，版本页的扫描缓存已经不准了。
      forgetScan(gameDir);
      setTaskOutput(taskId, { path: summary.path, kind: "dir" });
      endTask(taskId, `已装好 ${name}`);
      announce(`已装好 ${name}`);
      delete pausedNames[name];
      plans.delete(taskId);
      setTaskResume(taskId, null);
    } catch (e) {
      const message = describeError(e);
      // 暂停 / 取消是在「下载任务」页里点的，那边只留一个标记；
      // 并行安装时按任务 id 各查各的，别把别人的暂停算到自己头上。
      const stop = stops.get(taskId) ?? stopping ?? takeStop();
      stops.delete(taskId);
      stopping = null;
      if (message === "安装已取消") {
        if (stop === "pause") {
          // 暂停：文件都留着，点「继续」用同一套参数接着装。
          const note = "已暂停，点「继续」接着装";
          pauseTask(taskId, note);
          pausedNames[name] = taskId;
          announce(note);
        } else {
          const note = "已取消，半成品已清理";
          announce(note);
          endTask(taskId, note, true);
          delete pausedNames[name];
          plans.delete(taskId);
          setTaskResume(taskId, null);
        }
      } else {
        showError("安装失败", message, String(e), "game");
        endTask(taskId, message, true);
        delete pausedNames[name];
        plans.delete(taskId);
        setTaskResume(taskId, null);
      }
    } finally {
      stopping = null;
      activeInstalls = activeInstalls.filter((id) => id !== taskId);
      untrackInstall(taskId);
      // 面板对着最近开始的那次；它结束了就退回到还在跑的下一个。
      installTaskId = activeInstalls.length ? activeInstalls[activeInstalls.length - 1] : null;
    }
  }

  /// 后端把错误串前面挂了 `Invalid input: ` 之类的枚举名，给用户看的时候剥掉。
  function stripError(text: string): string {
    return text.replace(/^(Invalid input|Invalid download metadata|Download failed):\s*/i, "");
  }

  function describeError(e: unknown): string {
    const text = stripError(String(e));
    if (/cancel/i.test(text)) return "安装已取消";
    if (/Network|sending request|timed out|dns|connection/i.test(text)) return `网络错误：${text}`;
    if (/Integrity/i.test(text)) return `文件校验失败：${text}`;
    return text;
  }

  /// 暂停：只停下载，`.part` 留着，点「重试」从断点接着下。
  let stopping: "pause" | "cancel" | null = $state(null);
  /// 这一轮是被「暂停」停下的：进度行的阶段文案要说「已暂停」而不是「已取消」。
  let pausedRun = $state(false);
  /// 每次安装各自是怎么被停的：并行时不能用一个全局变量分辨谁被暂停、谁被取消。
  const stops = new Map<number, "pause" | "cancel">();

  async function cancel(mode: "pause" | "cancel") {
    const target = installTaskId ?? packTaskId;
    if (target === null) return;
    await cancelTask(target, mode);
  }

  /// 取消某一个具体的安装（并行时面板上会逐个列出，各自都能停）。
  async function cancelTask(taskId: number, mode: "pause" | "cancel") {
    if (activeInstalls.includes(taskId)) stops.set(taskId, mode);
    if (installTaskId === taskId) {
      stopping = mode;
      pausedRun = mode === "pause";
    }
    try {
      markStop(mode);
      await api.cancelInstall(taskId, mode);
      // 取消是「这一轮作废」：面板上不留进度，进度只在任务列表里查。
      // 暂停要保留，好让「重试」接着下。
      if (mode === "cancel" && installTaskId === taskId) summary = null;
    } catch (e) {
      stops.delete(taskId);
      stopping = null;
      showError("停不下来", "后台没有接住这次的停止请求。", String(e));
    }
  }

  /// 下载页现在是「内容中心」：第一项是原来那套「原版 + 加载器装成实例」，
  /// 其余分类是各类资源，安装包是单独下载加载器文件。
  type Category =
    | "all"
    | "minecraft"
    | "packs"
    | "mods"
    | "datapacks"
    | "resourcepacks"
    | "shaders"
    | "worlds"
    | "favorites"
    | "packages";
  const CATEGORIES: { id: Category; label: string }[] = [
    { id: "all", label: "全部" },
    { id: "minecraft", label: "Minecraft" },
    { id: "packs", label: "整合包" },
    { id: "mods", label: "模组" },
    { id: "datapacks", label: "数据包" },
    { id: "resourcepacks", label: "资源包" },
    { id: "shaders", label: "光影包" },
    { id: "worlds", label: "世界" },
    { id: "packages", label: "安装包" },
    { id: "favorites", label: "收藏夹" }
  ];
  const CATEGORY_NOTE: Record<Category, string> = {
    all: "全部内容：模组、资源包、光影、数据包、世界、整合包的最新更新都会汇到这里，下一步接入 Modrinth。",
    packs: "整合包：装好就是「游戏 + 加载器 + 一堆模组」的成品实例，也可以只把 .mrpack 存下来。",
    mods: "模组：按上面的筛选条件挑，存放位置看上面「下载到」。",
    datapacks: "数据包：按上面「下载到」的位置存放，之后自己放进世界的 datapacks 目录。",
    resourcepacks: "资源包：按上面「下载到」的位置存放。",
    shaders: "光影包：按上面「下载到」的位置存放。",
    worlds: "世界：下载后解压到 saves 目录。",
    favorites: "收藏夹：左侧是收藏夹名称，右侧是收藏的内容，顶部可以筛选。",
    packages: "安装包：单独下载加载器安装器或加载器本体，不创建实例。",
    minecraft: ""
  };
  let cat = $state<Category>("all");
  /// 分类胶囊下面那块会左右滑动的底片：按当前按钮的位置和宽度算。
  let catButtons: HTMLButtonElement[] = [];
  let catThumb = $state({ left: 0, width: 0, ready: false });

  /// 安装包的类别：这些是「一个文件就能拿走」的东西。
  const PACKAGE_KINDS: { id: string; label: string }[] = [
    { id: "minecraft", label: "Minecraft" },
    { id: "forge", label: "Forge" },
    { id: "neoforge", label: "NeoForge" },
    { id: "cleanroom", label: "Cleanroom" },
    { id: "fabric", label: "Fabric" },
    { id: "legacyfabric", label: "Legacy Fabric" },
    { id: "quilt", label: "Quilt" },
    { id: "labymod", label: "LabyMod" },
    { id: "liteloader", label: "LiteLoader" },
    { id: "optifine", label: "OptiFine" }
  ];
  let pkgKind = $state("forge");
  let pkgGame = $state("");
  let pkgVersion = $state("");
  let pkgDir = $state("");
  /// 加载器版本：稳定/预览都列出来，各自带标记。
  let pkgChoices = $state<Choice[]>([]);
  /// 游戏版本的四段分类，跟 Minecraft 页保持一致。
  let pkgFilter = $state<"release" | "snapshot" | "fools" | "all">("release");
  /// 每次切类别自增，异步清单回来时用它丢掉过期结果（否则快速切换会被上一次的结果覆盖）。
  let pkgSeq = 0;
  let pkgBusy = $state(false);
  let pkgLoading = $state(false);
  let pkgNote = $state("");

  /// 安装包页的游戏版本：跟 Minecraft 页同一份顺序、同一套分类。
  const pkgGames = $derived(
    orderedVersions.filter((version) => {
      const fool = foolOf(version.id);
      if (pkgFilter === "fools") return Boolean(fool);
      if (pkgFilter === "snapshot") return !fool && version.type === "snapshot";
      if (pkgFilter !== "all") return !fool && version.type === pkgFilter;
      return true;
    })
  );

  /// 默认保存目录：<游戏目录>\downloads。
  function defaultPkgDir(): string {
    return gameDir ? `${gameDir}\\downloads` : "";
  }

  /// 换了游戏版本：加载器版本跟着重查。
  function pickPkgGame(id: string) {
    pkgGame = id;
    pkgVersion = "";
    pkgChoices = [];
    void loadPackages();
  }

  /// 换分类后选中的游戏版本不在新列表里，就退到第一条。
  function pickPkgFilter(next: "release" | "snapshot" | "fools" | "all") {
    pkgFilter = next;
    if (pkgGames.some((item) => item.id === pkgGame)) return;
    const first = pkgGames[0]?.id;
    if (first) pickPkgGame(first);
    else {
      pkgGame = "";
      pkgVersion = "";
      pkgChoices = [];
    }
  }

  // 版本清单是异步来的：清单到了、或当前选中的游戏版本不在新分类里时，自动落到第一条。
  $effect(() => {
    if (cat !== "packages") return;
    if (pkgGames.length === 0) return;
    if (pkgGame && pkgGames.some((item) => item.id === pkgGame)) return;
    pickPkgGame(pkgGames[0].id);
  });

  async function loadPackages() {
    const seq = ++pkgSeq;
    if (!pkgGame) {
      pkgGame = pkgGames.find((item) => item.id === selected?.id)?.id ?? pkgGames[0]?.id ?? "";
    }
    pkgNote = "";
    if (pkgKind === "minecraft") {
      // 原版：游戏版本本身就是下载目标，没有第二级。
      pkgChoices = [];
      pkgVersion = pkgGame;
      pkgLoading = false;
      return;
    }
    if (!pkgGame) {
      pkgChoices = [];
      pkgVersion = "";
      return;
    }
    pkgLoading = true;
    try {
      const list = await fetchEntries(pkgKind as Loader, pkgGame);
      if (seq !== pkgSeq) return;
      pkgChoices = list;
      pkgVersion = (list.find((item) => item.stable) ?? list[0])?.version ?? "";
    } catch {
      if (seq !== pkgSeq) return;
      pkgChoices = [];
      pkgVersion = "";
    } finally {
      if (seq === pkgSeq) pkgLoading = false;
    }
  }

  /// Modrinth 的四类内容：Modrinth 的 project_type 与实例里的目标子目录。
  /// 世界只有 CurseForge 提供（Modrinth 没有 world 这个 project_type），这里用 CF 的 class 名。
  const MR_TYPES: Partial<Record<Category, string>> = {
    packs: "modpack",
    mods: "mod",
    resourcepacks: "resourcepack",
    shaders: "shader",
    datapacks: "datapack",
    worlds: "worlds"
  };
  const MR_SUBDIR: Partial<Record<Category, string>> = {
    packs: "modpacks",
    mods: "mods",
    resourcepacks: "resourcepacks",
    shaders: "shaderpacks",
    datapacks: "datapacks"
  };
  const MR_HINT: Record<string, string> = {
    mod: "搜索模组，比如 sodium、jei、create",
    resourcepack: "搜索资源包",
    shader: "搜索光影包",
    datapack: "搜索数据包",
    worlds: "搜索世界，比如 oneblock、skyblock"
  };
  const MR_LOADERS = ["fabric", "forge", "neoforge", "quilt"];
  /// 光影只看这三档（与 PCL 光影页的「加载器」分组一致）。
  const SHADER_LOADERS = [
    { value: "vanilla", label: "原版可用" },
    { value: "iris", label: "Iris" },
    { value: "optifine", label: "OptiFine" }
  ];

  let mrQuery = $state("");
  let mrLoader = $state("");
  /// 内容来源：像 PCL 一样三选一，`all` 时两个站的结果交替混排。
  let mrSource = $state<"all" | "curseforge" | "modrinth" | "ftb">("all");
  /// 排序（与 PCL 一致）：相关性 / 下载量 / 关注量 / 最新发布 / 最近更新。
  let mrSort = $state("relevance");
  /// 游戏版本（可手输，留空=任意）。
  /// 游戏版本只推荐这几档（社区模组最多的版本），其余让别人自己敲；
  /// 全量列表几十个反而不好选。
  const POPULAR_GAMES = [
    "1.21.1",
    "1.21",
    "1.20.1",
    "1.19.2",
    "1.18.2",
    "1.16.5",
    "1.12.2",
    "1.8.9",
    "1.7.10"
  ];
  const gameSuggestions = $derived(
    versions.length === 0
      ? POPULAR_GAMES
      : POPULAR_GAMES.filter((id) => versions.some((item) => item.id === id))
  );
  let mrGame = $state("");
  /// 标签下拉是否展开。
  let tagOpen = $state(false);
  /// 左栏滚动容器，翻页后回到顶部。
  let listEl = $state<HTMLElement | null>(null);
  /// 标签（分类）：用 PCL 那张表，两边各自取 id / facet 名。
  let mrTag = $state<TagOption>({ name: "全部" });
  /// 支持端筛选。
  let mrEnv = $state<SideFilter>("all");
  /// 当前页码（1 起）与总数，翻页用。
  let mrPage = $state(1);
  const mrPageSize = $derived(settings?.pageSize ?? 40);
  let mrTotal = $state(0);

  /// 每个内容分类各记一份筛选（搜索词、来源、排序、版本、加载器、支持端、标签、页码）。
  /// 切分类时先把当前的存进去，再把目标分类的那份摆出来，互不串味。
  let mrFilters: Record<string, CatFilters> = {};

  function defaultFilters(category: string): CatFilters {
    return {
      query: "",
      source: "all",
      sort: "relevance",
      game: "",
      loader: "",
      env: "all",
      tag: (TAGS[category as keyof typeof TAGS]?.[0] ?? { name: "全部" }).name,
      page: 1
    };
  }

  /// 标签存的是名字，取回来按当前分类的标签表查，避免拿上一个分类的标签对象。
  function labelFor(category: string, name: string): TagOption {
    const list = TAGS[category as keyof typeof TAGS] ?? [];
    return list.find((item) => item.name === name) ?? list[0] ?? { name: "全部" };
  }

  function saveFiltersFor(category: string) {
    if (!MR_TYPES[category as Category]) return;
    mrFilters[category] = {
      query: mrQuery,
      source: mrSource,
      sort: mrSort,
      game: mrGame,
      loader: mrLoader,
      env: mrEnv,
      tag: mrTag.name,
      page: mrPage
    };
  }

  function applyFiltersFor(category: string) {
    const saved = mrFilters[category] ?? defaultFilters(category);
    mrQuery = saved.query;
    mrSource = (saved.source ?? "all") as typeof mrSource;
    // FTB 只有整合包：别的分类（或旧状态）里留着的 ftb 要退回去。
    if (mrSource === "ftb" && category !== "packs") mrSource = "all";
    mrSort = saved.sort || "relevance";
    mrGame = saved.game ?? "";
    mrLoader = saved.loader ?? "";
    mrEnv = (saved.env ?? "all") as typeof mrEnv;
    mrTag = labelFor(category, saved.tag ?? "全部");
    mrPage = Math.max(1, saved.page ?? 1);
  }
  /// 两个来源各自的命中总数（响应式：页数由它算）。
  let mrTotals = $state<Record<"modrinth" | "curseforge", number>>({ modrinth: 0, curseforge: 0 });
  let mrProjects = $state<MrProject[]>([]);
  let mrProject = $state<MrProject | null>(null);
  let mrVersions = $state<MrVersion[]>([]);
  /// 项目简介的中文翻译（MCIM 翻译库），没有就不显示。
  let mrTranslation = $state<string | null>(null);
  /// Cleanroom 只装加载器本身，Fugue / Scalar 得自己放进 `mods/`。
  /// 面板上三条各一个下拉，默认「不下载」；Scalar 与 Scalar Legacy 是二选一。
  const CLEANROOM_EXCLUSIVE = ["scalar", "scalar-legacy"];
  let extras = $state<CleanroomExtraMod[]>([]);
  let extraPicked = $state<Record<string, string>>({});
  let extraBusy = $state(false);
  let extraLoadedFor = "";

  /// 选了 Cleanroom 才去问后端这三个模组有哪些版本。
  async function loadCleanroomExtras(game: string) {
    if (!game || extraBusy || extraLoadedFor === game) return;
    extraBusy = true;
    try {
      extras = await api.cleanroomExtras(game);
      extraLoadedFor = game;
      const picked = { ...extraPicked };
      for (const mod of extras) if (!(mod.key in picked)) picked[mod.key] = "";
      extraPicked = picked;
    } catch (e) {
      showError("读取 Cleanroom 组件失败", "没取到这三个模组的版本列表，可以稍后再试。", describeError(e));
    } finally {
      extraBusy = false;
    }
  }

  /// 挑了一条：把与之互斥的那条清回「不下载」。
  function pickExtra(key: string, file: string) {
    const next = { ...extraPicked, [key]: file };
    if (file && CLEANROOM_EXCLUSIVE.includes(key)) {
      for (const other of CLEANROOM_EXCLUSIVE) if (other !== key) next[other] = "";
    }
    extraPicked = next;
  }

  /// 只把真正选了版本的条目交给后端；没选的传空数组就是不装。
  function chosenExtras(): ChosenExtra[] {
    const out: ChosenExtra[] = [];
    for (const mod of extras) {
      const file = extraPicked[mod.key];
      if (!file) continue;
      const version = mod.versions.find((item) => String(item.file) === file);
      if (version) out.push({ project: mod.project, file: version.file, fileName: version.fileName });
    }
    return out;
  }

  /// 实例名可能是「1.12.2-Cleanroom_0.7.14-alpha」这种合成 id，
  /// 过滤内容时要用它前面的纯游戏版本号（PCL 也是拿实例的基础版本去筛）。
  const selectedGame = $derived(
    selected ? (selected.id.match(/^\d+(?:\.\d+){1,2}/)?.[0] ?? selected.id) : null
  );

  /// 右侧版本列表的当前页（每页条数跟左栏同一个设置）。
  let mrFilePage = $state(1);
  let mrBusy = $state(false);
  /// 这一轮加载是否真的落地过一次（用于「没有搜索结果」的判定）。
  let mrLoaded = $state(false);

  /// 用户手动点过分类之后，就不再让「恢复上次状态」抢方向。
  let userPicked = false;
  /// MC 百科条目地址；localStorage 里没有的条目是 null，按钮不显示。
  let mrWiki = $state<string | null>(null);
  /// 每发一次列表请求就 +1；回来的响应如果不是最新那一次就丢弃。
  let mrToken = 0;
  /// 「下载到哪」：按内容类别记一个实例名（空表示默认，落 `<游戏目录>\downloads`），
  /// 按游戏目录分开记，跨重启保留。
  let dirPicks = $state<Record<string, Record<string, string>>>(readDirPicks());
  /// 下拉里能选的实例都是「开了版本隔离」的，内容才落进它自己的目录。
  let instances = $state<VersionView[]>([]);

  function dirFolderKey(): string {
    return gameDir.replace(/[\\/]+$/, "").trim().toLowerCase();
  }

  function readDirPicks(): Record<string, Record<string, string>> {
    try {
      const raw = localStorage.getItem("ybumcl.downloadTarget");
      const parsed: unknown = raw ? JSON.parse(raw) : null;
      if (!parsed || typeof parsed !== "object") return {};
      const out: Record<string, Record<string, string>> = {};
      for (const [folder, value] of Object.entries(parsed as Record<string, unknown>)) {
        if (!value || typeof value !== "object") continue;
        const picks: Record<string, string> = {};
        for (const [kind, id] of Object.entries(value as Record<string, unknown>)) {
          if (typeof id === "string" && id) picks[kind] = id;
        }
        if (Object.keys(picks).length > 0) out[folder] = picks;
      }
      return out;
    } catch {
      return {};
    }
  }

  function saveDirPicks(): void {
    try {
      localStorage.setItem("ybumcl.downloadTarget", JSON.stringify(dirPicks));
    } catch {
      // 记不住只影响默认落点，不打断下载
    }
  }

  /// 把加载器名字归一成 key：两边都可能是 Fabric/fabric、NeoForge/neoforge。
  function loaderKeys(values: string[]): Set<string> {
    const keys = new Set<string>();
    for (const raw of values) {
      const name = raw.trim().toLowerCase();
      if (name === "fabric") keys.add("fabric");
      else if (name === "quilt") keys.add("quilt");
      else if (name === "forge") keys.add("forge");
      else if (name === "neoforge") keys.add("neoforge");
      else if (name === "optifine" || name === "iris") keys.add("optifine");
      else if (name === "liteloader") keys.add("liteloader");
    }
    return keys;
  }

  /// 这一类内容装得进去的实例：先得是隔离实例；模组还要求带加载器（原版装不了模组）。
  function canHold(instance: VersionView, kind: Category): boolean {
    if (!instance.isolated) return false;
    if (kind !== "mods") return true;
    const have = loaderKeys(instance.loaders);
    return ["fabric", "quilt", "forge", "neoforge", "liteloader"].some((key) => have.has(key));
  }

  function holdable(kind: Category): VersionView[] {
    return instances.filter((instance) => canHold(instance, kind));
  }

  /// 记住的实例可能已经被删、或不再隔离：那样就当默认。
  function chosenInstance(kind: Category): string {
    const folder = dirFolderKey();
    const picked = dirPicks[folder]?.[kind] ?? "";
    if (!picked) return "";
    return holdable(kind).some((instance) => instance.id === picked) ? picked : "";
  }

  function setDirPick(kind: Category, value: string): void {
    const folder = dirFolderKey();
    if (!folder) return;
    const picks = { ...(dirPicks[folder] ?? {}) };
    if (value) picks[kind] = value;
    else delete picks[kind];
    const next = { ...dirPicks };
    if (Object.keys(picks).length > 0) next[folder] = picks;
    else delete next[folder];
    dirPicks = next;
    saveDirPicks();
  }

  const CONTENT_SUB: Partial<Record<Category, string>> = {
    mods: "mods",
    resourcepacks: "resourcepacks",
    shaders: "shaderpacks"
  };

  /// 这一类内容的落点：默认 → `<游戏目录>\downloads`；选了实例 → 实例目录（数据包停在实例根）。
  /// 这个文件自己适配的游戏版本与加载器，和某个实例对得上吗。
  function instanceAccepts(instance: VersionView, version: FileVersion): boolean {
    const game = instance.gameVersion ?? "";
    if (!version.gameVersions.includes(game)) return false;
    if (version.loaders.length === 0) return true;
    const wanted = version.loaders.map((name) => name.toLowerCase());
    const have = (instance.loaders ?? []).map((name) => name.toLowerCase());
    return wanted.some((name) => have.includes(name));
  }

  /// 落点：当前实例适配就用它；否则挑一个版本与加载器都一致的（最近启动优先）；
  /// 一个都没有就落到 `downloads`，不硬塞进不相干的实例。
  function instanceFor(version: FileVersion): VersionView | null {
    const candidates = instances.filter((item) => instanceAccepts(item, version));
    if (candidates.length === 0) return null;
    const current = candidates.find((item) => item.id === selected?.id);
    if (current) return current;
    const sorted = [...candidates].sort(
      (a, b) =>
        (b.lastStarted ?? "").localeCompare(a.lastStarted ?? "") ||
        (b.installedAt ?? 0) - (a.installedAt ?? 0)
    );
    return sorted[0] ?? null;
  }

  function targetFor(version: FileVersion): string {
    if (!gameDir) return "";
    const hit = instanceFor(version);
    if (!hit) return `${gameDir}\\downloads`;
    const sub = CONTENT_SUB[cat] ?? "";
    return `${gameDir}\\versions\\${hit.id}\\${sub}`;
  }

  function targetDir(kind: Category, pick = chosenInstance(kind)): string {
    if (!gameDir) return "";
    if (!pick) return gameDir + "\\" + "downloads";
    const root = gameDir + "\\" + "versions" + "\\" + pick;
    const sub = CONTENT_SUB[kind] ?? "";
    return sub ? root + "\\" + sub : root;
  }

  /// 世界解压进哪：选了实例就进它的 saves，没选（或实例没隔离）就进游戏根 saves。
  function worldSaveDir(): string {
    const pick = chosenInstance("worlds");
    const instance = pick ? instances.find((item) => item.id === pick) : undefined;
    const root = instance && instance.isolated ? gameDir + "\\" + "versions" + "\\" + instance.id : gameDir;
    return gameDir ? root + "\\" + "saves" : "";
  }

  /// 下拉选项：第一项是默认，之后是能装这一类内容的实例。
  const dirOptions = $derived.by(() => {
    const kind = cat as Category;
    return [
      { value: "", label: "默认" },
      ...holdable(kind).map((instance) => ({ value: instance.id, label: instance.id }))
    ];
  });

  /// 换落点时先建目录：目录不存在的话 Windows 会忽略对话框的起始位置。
  async function prepareDir(path: string): Promise<string> {
    if (path) await api.ensureDir(path).catch(() => {});
    return path;
  }

  /// 清单里**正式版**的顺序（新的在前）当作排序轴——预览版/快照夹在中间会把区间切碎。
  const versionOrder = $derived(
    new Map(
      versions
        .filter((item) => item.type === "release")
        .map((item, index) => [item.id, index])
    )
  );
  /// 排序轴里的正式版 id 列表（下标与上面的 Map 一致）。
  const releaseIDs = $derived(versions.filter((item) => item.type === "release").map((item) => item.id));

  const LOADER_LABEL: Record<string, string> = {
    fabric: "Fabric",
    forge: "Forge",
    neoforge: "NeoForge",
    quilt: "Quilt"
  };

  function loaderText(list: string[]): string {
    const names = list
      .map((item) => LOADER_LABEL[item.toLowerCase()] ?? "")
      .filter((item) => item.length > 0);
    return [...new Set(names)].join(" / ");
  }

  /// 把一堆游戏版本压成 PCL 那种区间串：连续段 `起~止`，最新的一段 `起+`，多段用「, 」连接。
  function versionRanges(list: string[]): string {
    const marks = list
      .map((id) => versionOrder.get(id))
      .filter((index): index is number => index !== undefined)
      .sort((left, right) => left - right);
    if (marks.length === 0) return "";
    const groups: number[][] = [];
    for (const mark of marks) {
      const last = groups[groups.length - 1];
      if (last && mark === last[last.length - 1] + 1) last.push(mark);
      else groups.push([mark]);
    }
    const ids = releaseIDs;
    /// 段太多就没法读，直接退化成「最老的那一版及以上」。
    if (groups.length > 3) {
      const oldest = ids[groups[groups.length - 1][groups[groups.length - 1].length - 1]] ?? "";
      return oldest ? `${oldest}+` : "";
    }
    return groups
      .map((group) => {
        const newest = ids[group[0]];
        const oldest = ids[group[group.length - 1]];
        if (group.includes(0)) return `${oldest}+`;
        return group.length === 1 ? oldest : `${oldest}~${newest}`;
      })
      .filter((text) => typeof text === "string" && text.length > 0)
      .join(", ");
  }

  /// 右上角那行：「Fabric / NeoForge 1.16.5~1.18.2, 1.20+」。
  function supportText(item: MrProject): string {
    const loaders = loaderText(item.loaders);
    const range = versionRanges(item.gameVersions);
    return [loaders, range].filter((part) => part.length > 0).join(" ");
  }

  /// 右下角的来源（CurseForge 的 id 是纯数字）。
  /// FTB 的 id 带前缀，其余靠「纯数字 = CurseForge」推。
  function isFtb(project: MrProject): boolean {
    return project.id.startsWith("ftb:");
  }

  function sourceOf(item: MrProject): string {
    if (mergedProjects.has(item)) return "Modrinth + CurseForge";
    if (isFtb(item)) return "FTB";
    return /^\d+$/.test(item.id) ? "CurseForge" : "Modrinth";
  }

  function formatCount(value: number): string {
    if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
    if (value >= 10_000) return `${(value / 10_000).toFixed(1)}万`;
    return String(value);
  }

  function formatSize(value: number): string {
    if (value >= 1024 * 1024) return `${(value / 1024 / 1024).toFixed(1)} MB`;
    if (value >= 1024) return `${Math.round(value / 1024)} KB`;
    return `${value} B`;
  }

  /// 当前内容类型下可用的标签（PCL 的表）。
  const mrTags = $derived(cat in TAGS ? TAGS[cat as keyof typeof TAGS] : []);

  /// 标签 → 某个来源该用的值（没有值表示这个标签对那一站无效）。
  function mrTagValue(source: "modrinth" | "curseforge"): string | null {
    if (!mrTag.cf && !mrTag.mr) return null;
    if (source === "curseforge") return mrTag.cf ? String(mrTag.cf) : null;
    return mrTag.mr ?? null;
  }

  /// 两个来源交替混排（PCL 的「全部来源」就是这个观感）。
  function interleave(cf: MrProject[], mr: MrProject[]): MrProject[] {
    const out: MrProject[] = [];
    for (let index = 0; index < Math.max(cf.length, mr.length); index += 1) {
      if (cf[index]) out.push(cf[index]);
      if (mr[index]) out.push(mr[index]);
    }
    return out;
  }

  /// 同一项目在两站各有一条时的去重键（slug 一般两边一致，退化成标题）。
  function railKey(item: MrProject): string {
    return (item.slug || item.title).trim().toLowerCase();
  }

  /// 集市里两站都收录的项目：以 Modrinth 那条为底（详情/文件列表都用它），
  /// 下载量取两站之和，来源标成「Modrinth + CurseForge」。
  const mergedProjects = new WeakSet<MrProject>();

  function dedupeRails(list: MrProject[]): MrProject[] {
    const picked = new Map<string, MrProject>();
    for (const item of list) {
      const key = railKey(item);
      if (!key) continue;
      const prev = picked.get(key);
      if (!prev) {
        picked.set(key, item);
        continue;
      }
      const prevFromCurseForge = /^\d+$/.test(prev.id);
      const fromCurseForge = /^\d+$/.test(item.id);
      if (prevFromCurseForge === fromCurseForge) continue;
      const base = prevFromCurseForge ? item : prev;
      const merged: MrProject = { ...base, downloads: prev.downloads + item.downloads };
      mergedProjects.add(merged);
      picked.set(key, merged);
    }
    return [...picked.values()];
  }

  /// 这次要查哪些来源：标签与支持端都可能让某一站整段不查；世界只有 CurseForge。
  const mrSources = $derived.by(() => {
    // FTB 是另一套接口，专门在上面那条分支里处理，这里不参与。
    const wanted: Array<"modrinth" | "curseforge"> =
      cat === "worlds"
        ? ["curseforge"]
        : mrSource === "all"
          ? ["curseforge", "modrinth"]
          : mrSource === "ftb"
            ? []
            : [mrSource];
    return wanted.filter((source) => {
      // CurseForge 没有「支持端」这个维度，选了就只查 Modrinth。
      if (source === "curseforge" && mrEnv !== "all") return false;
      if ((mrTag.cf || mrTag.mr) && mrTagValue(source) === null) return false;
      return true;
    });
  });

  /// 来源下拉：世界只有一个源，其余照旧。
  const sourceOptions = $derived(
    cat === "worlds"
      ? [{ value: "curseforge", label: "CurseForge" }]
      : cat === "packs"
        ? [
            { value: "all", label: "全部来源" },
            { value: "modrinth", label: "Modrinth" },
            { value: "curseforge", label: "CurseForge" },
            { value: "ftb", label: "FTB" }
          ]
        : [
            { value: "all", label: "全部来源" },
            { value: "modrinth", label: "Modrinth" },
            { value: "curseforge", label: "CurseForge" }
          ]
  );

  /// 加载器下拉：模组/整合包用模组加载器，光影用原版可用 / Iris / OptiFine。
  const loaderOptions = $derived(
    cat === "shaders"
      ? [{ value: "", label: "全部" }, ...SHADER_LOADERS]
      : [
          { value: "", label: "全部加载器" },
          ...MR_LOADERS.map((item) => ({ value: item, label: loaderName(item) }))
        ]
  );

  /// 当前分类真正要传给接口的加载器（别的分类忽略它）。
  function activeLoader(): string | null {
    if (!mrLoader) return null;
    return cat === "mods" || cat === "packs" || cat === "shaders" ? mrLoader : null;
  }

  /// 一次查询的累积表：两站各自推进自己的偏移量，翻页只补缺的那一条。
  /// 好处是翻回上一页不用重新请求，两站混排的顺序也不会在页与页之间乱跳。
  interface ListState {
    key: string;
    items: MrProject[];
    cfOffset: number;
    mrOffset: number;
    cfTotal: number;
    mrTotal: number;
    cfEnded: boolean;
    mrEnded: boolean;
    cfError: boolean;
    mrError: boolean;
    /// 滑动窗口丢掉的前缀条数（全局下标）。
    dropped: number;
  }
  let listState = $state<ListState | null>(null);
  /// 「更早的结果已释放」这类一次性提示。
  let listHint = $state("");
  /// 单源失败时的黄字。
  const listSourceWarning = $derived.by(() => {
    const state = listState;
    if (!state) return "";
    if (state.cfError && !state.mrError) return "CurseForge 没回话，只显示了 Modrinth 的内容，结果可能不全。";
    if (state.mrError && !state.cfError) return "Modrinth 没回话，只显示了 CurseForge 的内容，结果可能不全。";
    if (state.cfError && state.mrError) return "两站都没回话，稍后再试。";
    return "";
  });
  /// 累积表最多留这么多个查询（切分类来回翻时命中缓存）。
  const listCache = new Map<string, ListState>();
  const LIST_CACHE_MAX = 3;
  /// 累积超过这么多条就裁掉最早的那些。
  const LIST_WINDOW_LIMIT = 2000;
  /// 裁完留下最近这么多条。
  const LIST_WINDOW_KEEP = 1000;

  /// 页数按两站总数相加算（CF 先截 10000，因为它的搜索最多只能翻到那儿）。
  const listTotal = $derived(
    listState ? listState.cfTotal + listState.mrTotal : cat === "packs" && mrSource === "ftb" ? mrTotal : 0
  );
  const mrPageCount = $derived(Math.max(1, Math.ceil(listTotal / Math.max(1, mrPageSize))));
  /// 两站都到头了才禁「下一页」。
  const listEnded = $derived(
    cat === "packs" && mrSource === "ftb"
      ? mrPage >= mrPageCount
      : !!listState && listState.cfEnded && listState.mrEnded
  );

  /// 一次查询的键：筛选条件一变就是另一张表。
  function listQueryKey(): string {
    const gameVersion = mrGame.trim() || selectedGame;
    const loader = activeLoader();
    return [
      MR_TYPES[cat] ?? "",
      mrQuery.trim(),
      mrSource,
      gameVersion ?? "",
      loader ?? "",
      mrSort,
      mrTag.cf ? String(mrTag.cf) : "",
      mrTag.mr ?? "",
      mrEnv
    ].join("|");
  }

  function stateFor(key: string): ListState {
    const cached = listCache.get(key);
    if (cached) {
      // 命中即刷新它的位置（Map 的插入顺序当 LRU 用）。
      listCache.delete(key);
      listCache.set(key, cached);
      return cached;
    }
    const fresh: ListState = {
      key,
      items: [],
      cfOffset: 0,
      mrOffset: 0,
      cfTotal: 0,
      mrTotal: 0,
      cfEnded: false,
      mrEnded: false,
      cfError: false,
      mrError: false,
      dropped: 0
    };
    listCache.set(key, fresh);
    while (listCache.size > LIST_CACHE_MAX) {
      const oldest = listCache.keys().next().value;
      if (oldest === undefined || oldest === key) break;
      listCache.delete(oldest);
    }
    return fresh;
  }

  /// 固定排序键（照 PCL 的来源权重）：下载量乘权重，分数相同的按进来的先后。
  function sourceWeight(source: "modrinth" | "curseforge"): number {
    const kind = MR_TYPES[cat];
    if (kind === "mod" || kind === "modpack") return source === "modrinth" ? 7 : 1;
    if (kind === "datapack") return source === "curseforge" ? 10 : 1;
    return source === "curseforge" ? 1 : 5;
  }

  function listScore(item: MrProject): number {
    return item.downloads * sourceWeight(isCurseForge(item) ? "curseforge" : "modrinth");
  }

  /// 把新到的一批并进累积表：按项目去重（两站都有时留 CurseForge 那条），
  /// 再按固定分数插到该在的位置——已显示的条目不会因为补数据而换页。
  function mergeList(state: ListState, incoming: MrProject[]) {
    const picked = new Map<string, MrProject>();
    const order = new Map<string, number>();
    let index = 0;
    for (const item of [...state.items, ...incoming]) {
      const key = railKey(item);
      if (!key) continue;
      const prev = picked.get(key);
      if (!prev) {
        picked.set(key, item);
        order.set(key, index);
      } else if (isCurseForge(item) && !isCurseForge(prev)) {
        picked.set(key, item);
      }
      index += 1;
    }
    // 两家合表时的顺序：按下载量排就老老实实比下载量；其它排序保持各自服务器的顺序，
    // 不能再拿「下载量 × 来源权重」重排，否则看起来根本没按所选方式排。
    const byDownloads = mrSort === "downloads";
    state.items = [...picked.entries()]
      .map(([key, item]) => ({ item, order: order.get(key) ?? 0 }))
      .sort((a, b) =>
        byDownloads ? b.item.downloads - a.item.downloads || a.order - b.order : a.order - b.order
      )
      .map((entry) => entry.item);
  }

  /// 拉一批：两站各推进一页，谁先回都行，结果一起并表。
  async function fetchBatch(state: ListState, token: number) {
    const type = MR_TYPES[cat];
    if (!type) return;
    const gameVersion = mrGame.trim() || selectedGame;
    const loader = activeLoader();
    const sources = mrSources.filter((source) => (source === "curseforge" ? !state.cfEnded : !state.mrEnded));
    if (sources.length === 0) return;
    // 两家各发各的：谁先回来先把谁并进表里，别等慢的那家。
    await Promise.all(
      sources.map(async (source) => {
        const offset = source === "curseforge" ? state.cfOffset : state.mrOffset;
        const key = [source, type, mrQuery, gameVersion ?? "", loader ?? "", mrSort, mrTagValue(source) ?? "", mrEnv, offset].join("|");
        const hit = searchCache.get(key);
        if (hit && Date.now() - hit.at < SEARCH_CACHE_MS) {
          // 走缓存也得并进表里：以前这里直接 return，页面上就什么都不显示。
          if (token === mrToken) applyBatch(state, source, { items: hit.items, total: hit.total });
          return;
        }
        try {
          const page = await api.contentSearch({
            source,
            kind: type,
            query: mrQuery,
            gameVersion,
            loader,
            sort: mrSort,
            category: mrTagValue(source),
            env: mrEnv,
            offset,
            limit: mrPageSize,
            // 资源站只认英文，中文关键词得先在本地的「中文名 → 英文名」表里换一手
            translate: cat === "mods" || cat === "datapacks"
          });
          searchCache.set(key, { at: Date.now(), items: page.items, total: page.total });
          if (token === mrToken) applyBatch(state, source, page);
          return;
        } catch {
          if (token !== mrToken) return;
          if (source === "curseforge") {
            state.cfError = true;
            state.cfEnded = true;
          } else {
            state.mrError = true;
            state.mrEnded = true;
          }
        }
      })
    );
  }

  /// 一家回来了就并一家：先回来的先上屏，慢的那家到了再追加。
  function applyBatch(state: ListState, source: string, page: { items: MrProject[]; total: number }) {
    const fromCf = source === "curseforge";
    const total = fromCf ? Math.min(page.total, 10000) : page.total;
    const count = page.items.length;
    if (fromCf) {
      state.cfTotal = total;
      state.cfOffset += count;
      if (count < mrPageSize || state.cfOffset >= total) state.cfEnded = true;
    } else {
      state.mrTotal = total;
      state.mrOffset += count;
      if (count < mrPageSize || state.mrOffset >= total) state.mrEnded = true;
    }
    mergeList(state, page.items);
    if (listState === state && mrLoaded) renderList(state);
  }

  /// 滑动窗口：只裁到当前页之前，保证正在看的这页不会被裁掉。
  function trimWindow(state: ListState, pageStart: number) {
    const total = state.dropped + state.items.length;
    if (total <= LIST_WINDOW_LIMIT) return;
    const drop = Math.min(state.items.length - LIST_WINDOW_KEEP, pageStart - state.dropped);
    if (drop <= 0) return;
    state.items = state.items.slice(drop);
    state.dropped += drop;
    listHint = "更早的结果已经释放，翻回去会重新加载";
  }

  /// 把累积表里当前页那一段摆到列表上。
  function renderList(state: ListState) {
    const size = Math.max(1, mrPageSize);
    const begin = (mrPage - 1) * size - state.dropped;
    mrProjects = begin >= 0 ? state.items.slice(begin, begin + size) : [];
    mrTotals = { modrinth: state.mrTotal, curseforge: state.cfTotal };
    mrTotal = state.cfTotal + state.mrTotal;
  }

  /// 保证第 page 页已经有数据（没有就补），然后渲染。
  async function ensurePage(page: number) {
    const type = MR_TYPES[cat];
    if (!type) return;
    const token = ++mrToken;
    const state = stateFor(listQueryKey());
    listState = state;
    mrPage = page;
    mrBusy = true;
    mrLoaded = false;
    mrProject = null;
    const size = Math.max(1, mrPageSize);
    const begin = (page - 1) * size;
    // 翻回被窗口裁掉的那一段：这张表从第 1 页重新补。
    if (begin < state.dropped) {
      state.items = [];
      state.dropped = 0;
      state.cfOffset = 0;
      state.mrOffset = 0;
      state.cfEnded = false;
      state.mrEnded = false;
      state.cfError = false;
      state.mrError = false;
      listHint = "更早的结果已经释放，正在重新加载…";
    }
    let guard = 0;
    while (state.dropped + state.items.length < begin + size && !(state.cfEnded && state.mrEnded) && guard < 25) {
      await fetchBatch(state, token);
      if (token !== mrToken) return;
      guard += 1;
      trimWindow(state, begin);
    }
    if (token !== mrToken) return;
    renderList(state);
    mrBusy = false;
    mrLoaded = true;
    if (listEl) listEl.scrollTop = 0;
    persistState();
    if (page === 1 && state.dropped === 0) void prefetchNext(state, token);
  }

  /// 第 1 页渲染完之后顺手把第 2 页也拉了，点「下一页」就是瞬时的。
  async function prefetchNext(state: ListState, token: number) {
    const size = Math.max(1, mrPageSize);
    let guard = 0;
    // 只补一批：以前最多连拉 3 轮，等于后台又发了 6 个请求，切分类/翻页反而更慢。
    while (state.dropped + state.items.length < size * 2 && !(state.cfEnded && state.mrEnded) && guard < 1) {
      await fetchBatch(state, token);
      if (token !== mrToken) return;
      guard += 1;
    }
    if (token === mrToken && listState === state) renderList(state);
  }

  /// 切到某个内容分类时自动把默认列表拉出来（PCL 切分类也一样，不用先点「搜索」）。
  function autoLoadCategory(id: Category) {
    if (!MR_TYPES[id]) return;
    mrPage = 1;
    void mrSearch();
  }

  /// 点「搜索」或者换筛选：清掉这一轮的选择，从第 1 页重新开始。
  async function mrSearch() {
    const type = MR_TYPES[cat];
    if (!type) return;
    saveFiltersFor(cat);
    mrPage = 1;
    mrProjects = [];
    mrProject = null;
    mrVersions = [];
    mrFilePage = 1;
    listHint = "";
    await mrLoad();
  }

  async function mrLoad() {
    if (cat === "packs" && mrSource === "ftb") {
      await ftbLoad();
      return;
    }
    await ensurePage(mrPage);
  }

  /// FTB 只有整合包一个入口，接口也不一样：单独走一条，不进累积表。
  async function ftbLoad() {
    const token = ++mrToken;
    const page = mrPage;
    mrBusy = true;
    mrLoaded = false;
    listState = null;
    mrProjects = [];
    if (listEl) listEl.scrollTop = 0;
    try {
      const result = await api.ftbSearch(mrQuery, mrSort, page - 1, mrPageSize);
      if (token !== mrToken || page !== mrPage) return;
      mrProjects = result.items;
      mrTotal = result.total;
      mrTotals = { modrinth: 0, curseforge: 0 };
    } catch (error) {
      mrProjects = [];
      showError("加载列表失败", "没取到 FTB 的这一页，稍后再试。", String(error));
    } finally {
      if (token === mrToken) {
        mrBusy = false;
        mrLoaded = true;
      }
      persistState();
    }
  }

  async function mrGo(page: number) {
    const next = Math.min(Math.max(1, page), mrPageCount);
    if (next === mrPage) return;
    await ensurePage(next);
  }

  /// 详情页的三轴筛选与折叠状态（照 PCL 的 PageDownloadCompDetail）。
  let fileGame = $state<string | null>(null);
  let fileLoader = $state<string | null>(null);
  let fileEnvironment = $state<EnvironmentFilter | null>(null);
  let fileExpanded = $state(new Set<string>());
  /// 「符合左栏筛选」那组的折叠状态：默认展开，所以用反向标记。
  let leftCollapsed = $state(false);
  /// 快照恢复完成前不要回写状态，否则会用默认值覆盖掉上次的。
  let stateRestored = $state(false);
  let fileBusy = $state(false);

  const fileVersions: FileVersion[] = $derived(mrVersions);
  /// 详情页只用两个下拉：加载器、游戏版本；版本列表平铺，按发布时间倒序。
  const fileLoaderChoices = $derived(
    // 下拉里的加载器也按 Forge → NeoForge → Fabric → Quilt → 其它 排。
    [...new Set(fileVersions.flatMap((item) => item.loaders))].sort(
      (a, b) => loaderRank(a) - loaderRank(b) || a.localeCompare(b)
    )
  );
  /// 加载器优先级：NeoForge → Forge → Fabric → Quilt → 其它（按名字）。
  const FILTER_LOADER_ORDER = ["forge", "neoforge", "fabric", "quilt"];
  function loaderRank(id: string | null): number {
    if (!id) return FILTER_LOADER_ORDER.length + 1;
    const index = FILTER_LOADER_ORDER.indexOf(id.toLowerCase());
    return index < 0 ? FILTER_LOADER_ORDER.length : index;
  }
  type FileBlock = { key: string; title: string; version: string; loader: string | null; files: FileVersion[] };
  /// 一个「加载器 + 游戏版本」就是一个方块：先按版本从新到旧，再按加载器，块内按发布时间从新到旧。
  const fileBlocks = $derived.by<FileBlock[]>(() => {
    const map = new Map<string, FileBlock>();
    for (const item of fileVersions) {
      const loaders = item.loaders ?? [];
      if (fileLoader && !loaders.some((name) => name === fileLoader)) continue;
      if (fileEnvironment && !fileSupports(item, fileEnvironment)) continue;
      for (const version of item.gameVersions ?? []) {
        const combos: (string | null)[] = loaders.length > 0 ? [...new Set(loaders)] : [null];
        for (const loader of combos) {
          const key = `${version}|${loader ?? ""}`;
          let block = map.get(key);
          if (!block) {
            block = {
              key,
              title: loader ? `${loaderName(loader)} ${version}` : version,
              version,
              loader,
              files: []
            };
            map.set(key, block);
          }
          if (!block.files.some((file) => file.id === item.id)) block.files.push(item);
        }
      }
    }
    const list = [...map.values()];
    for (const block of list) {
      block.files.sort((a, b) => (b.datePublished ?? "").localeCompare(a.datePublished ?? ""));
    }
    return list.sort((a, b) => {
      // compareVersionName 本身就是「新的在前」，别再反过来。
      const byVersion = compareVersionName(a.version, b.version);
      if (byVersion !== 0) return byVersion;
      const byLoader = loaderRank(a.loader) - loaderRank(b.loader);
      if (byLoader !== 0) return byLoader;
      // 同属「其他」加载器时按名字，别按建块顺序。
      return (a.loader ?? "").localeCompare(b.loader ?? "");
    });
  });
  /// 方块上出现过的游戏版本（顺序就是方块的顺序：新的在前）。
  const shownBlocks = $derived(fileGame ? fileBlocks.filter((block) => block.version === fileGame) : fileBlocks);
  const fileGameChoices = $derived([...new Set(fileBlocks.map((block) => block.version))]);
  /// 每个方块顶部的「前置资源」：该方块里所有文件声明的必需前置并集。
  let blockDeps = $state<Record<string, RefDetail[]>>({});
  let blockDepBusy = $state<Record<string, boolean>>({});
  async function loadBlockDeps(block: FileBlock) {
    if (blockDeps[block.key] || blockDepBusy[block.key]) return;
    const ids: string[] = [];
    for (const file of block.files) {
      for (const dep of file.dependencies ?? []) {
        if (dep.dependencyType !== "required" || !dep.projectId) continue;
        if (!ids.includes(dep.projectId)) ids.push(dep.projectId);
      }
    }
    if (ids.length === 0) {
      blockDeps = { ...blockDeps, [block.key]: [] };
      return;
    }
    blockDepBusy = { ...blockDepBusy, [block.key]: true };
    try {
      const source = mrProject && isCurseForge(mrProject) ? "curseforge" : "modrinth";
      const list = await api.referenceDetails(source, ids);
      blockDeps = { ...blockDeps, [block.key]: list };
    } catch {
      blockDeps = { ...blockDeps, [block.key]: [] };
    } finally {
      blockDepBusy = { ...blockDepBusy, [block.key]: false };
    }
  }
  /// 点「前置资源」里的某一行时弹出的窗口：左边是前置本身的资料，右边是它的文件列表。
  type DepView = { dep: RefDetail; versions: FileVersion[]; loading: boolean };
  /// 前置详情是模组详情的下级页面：可以一层层往下钻，返回时一层层退回来。
  let depStack = $state<DepView[]>([]);
  const depView = $derived(depStack[depStack.length - 1] ?? null);

  function depBack() {
    if (depStack.length === 0) return false;
    depStack = depStack.slice(0, -1);
    return true;
  }
  /// 左右两栏的宽度比例（%），拖动中间的竖条调节。
  let depSplit = $state(46);

  async function openDep(dep: RefDetail) {
    resetDepFilters();
    void tick().then(() => {
      if (depRightEl) depRightEl.scrollTop = 0;
    });
    depWiki = null;
    void api
      .wikiLink(dep.source === "CurseForge" ? "curseforge" : "modrinth", dep.slug)
      .then((url) => {
        if (depView?.dep.id === dep.id) depWiki = url;
      })
      .catch(() => {});
    depStack = [...depStack, { dep, versions: [], loading: true }];
    try {
      const versions =
        dep.source === "CurseForge"
          ? await api.curseforgeVersions(dep.id)
          : await api.modrinthVersions(dep.id);
      const last = depStack[depStack.length - 1];
      if (last?.dep.id === dep.id) depStack = [...depStack.slice(0, -1), { dep, versions, loading: false }];
    } catch {
      const last = depStack[depStack.length - 1];
      if (last?.dep.id === dep.id) depStack = [...depStack.slice(0, -1), { dep, versions: [], loading: false }];
    }
  }

  /// 前置弹窗里的筛选（放在左栏底部）与右栏风琴的展开状态。
  let depGame = $state("all");
  let depLoader = $state("all");
  let depEnv = $state("all");
  let depOpen = $state<Record<string, boolean>>({});
  /// 前置的 MC 百科词条（有才显示「转到 MC 百科」）。
  let depWiki = $state<string | null>(null);
  let depRightEl: HTMLElement | null = null;

  /// 把前置包成项目对象，复用主详情那一排按钮（收藏、复制链接都要 MrProject）。
  function depProject(dep: RefDetail): MrProject {
    return {
      id: dep.id,
      slug: dep.slug,
      title: dep.title,
      description: dep.description,
      iconUrl: dep.icon,
      downloads: dep.downloads,
      author: "",
      categories: [],
      projectType: "mod",
      loaders: [],
      gameVersions: [],
      chineseTitle: null
    };
  }

  const depGames = $derived.by(() => {
    const set = new Set<string>();
    for (const item of depView?.versions ?? []) {
      for (const version of item.gameVersions ?? []) set.add(version);
    }
    return [...set].sort((a, b) => -compareVersionName(a, b));
  });
  const depLoaders = $derived.by(() => {
    const set = new Set<string>();
    for (const item of depView?.versions ?? []) {
      for (const loader of item.loaders ?? []) set.add(loader);
    }
    return [...set].sort((a, b) => loaderRank(a) - loaderRank(b) || a.localeCompare(b));
  });

  /// 右栏的文件按「游戏版本 + 加载器」分块，块内按发布时间从新到旧。
  const depBlocks = $derived.by(() => {
    const map = new Map<string, { key: string; title: string; version: string; loader: string | null; files: FileVersion[] }>();
    for (const item of depView?.versions ?? []) {
      if (depLoader !== "all" && !(item.loaders ?? []).includes(depLoader)) continue;
      if (depGame !== "all" && !(item.gameVersions ?? []).includes(depGame)) continue;
      if (depEnv !== "all" && !fileSupports(item, depEnv as EnvironmentFilter)) continue;
      const version = (item.gameVersions ?? [])[depGame === "all" ? 0 : 0] ?? "";
      const loaders = item.loaders ?? [];
      const combos: (string | null)[] = loaders.length > 0 ? [...new Set(loaders)] : [null];
      for (const loader of combos) {
        const key = `${version}|${loader ?? ""}`;
        let block = map.get(key);
        if (!block) {
          block = {
            key,
            title: loader ? `${loaderName(loader)} ${version}` : version,
            version,
            loader,
            files: []
          };
          map.set(key, block);
        }
        if (!block.files.some((file) => file.id === item.id)) block.files.push(item);
      }
    }
    const list = [...map.values()];
    for (const block of list) {
      block.files.sort((a, b) => (b.datePublished ?? "").localeCompare(a.datePublished ?? ""));
    }
    return list.sort(
      (a, b) => compareVersionName(a.version, b.version) || loaderRank(a.loader) - loaderRank(b.loader)
    );
  });

  /// 每次打开新的前置，筛选与展开状态都重置。
  function resetDepFilters() {
    depGame = "all";
    depLoader = "all";
    depEnv = "all";
    depOpen = {};
  }

  function depGutterDown(event: PointerEvent) {
    const handle = event.currentTarget as HTMLElement;
    const sheet = handle.parentElement;
    if (!sheet) return;
    const rect = sheet.getBoundingClientRect();
    const move = (moveEvent: PointerEvent) => {
      const percent = ((moveEvent.clientX - rect.left) / rect.width) * 100;
      depSplit = Math.min(72, Math.max(24, percent));
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  /// 左侧改筛选，右侧跟着变；右侧自己改不回写左侧。
  $effect(() => {
    const nextGame = mrGame.trim() || null;
    const nextLoader = mrLoader || null;
    if (fileGame !== nextGame) fileGame = nextGame;
    if (fileLoader !== nextLoader) fileLoader = nextLoader;
  });

  /// 返回键在前置详情里先退一级（前置 → 原模组详情），退完才离开下载页。
  $effect(() => {
    registerDetailBack(() => depBack());
    return () => registerDetailBack(null);
  });

  function toggleBlock(key: string, block: FileBlock) {
    const next = new Set(fileExpanded);
    if (next.has(key)) next.delete(key);
    else {
      next.add(key);
      void loadBlockDeps(block);
    }
    fileExpanded = next;
  }
  const fileFlags = $derived(computeFlags(fileVersions.flatMap((item) => item.gameVersions)));
  /// 详情头部的「最后更新」跟着当前选中的版本走。
  const fileUpdated = $derived(pickedVersion()?.datePublished ?? "");
  const fileFilterOptions = $derived(buildFilters(fileVersions, fileFlags));
  /// 当前实例对应的加载器（`1.20.1-Forge_47.4.9` → forge）。
  const instanceLoader = $derived.by(() => {
    const id = (selected?.id ?? "").toLowerCase();
    for (const name of ["neoforge", "forge", "quilt", "fabric", "liteloader"]) {
      if (id.includes(name)) return name;
    }
    return null;
  });
  /// 每组是否点了「查看全部」；默认只显示前 15 条。
  let groupAll = $state<Record<string, boolean>>({});

  async function copyText(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      announce("已复制到剪贴板");
    } catch (e) {
      showError("复制失败", "系统剪贴板没有接受这次写入。", String(e));
    } finally {
      fileBusy = false;
    }
  }

  /// CurseForge 的 id 是纯数字，Modrinth 是乱序短 id，混排时按这个判断归属。
  function isCurseForge(project: MrProject): boolean {
    return mrSource === "curseforge" || (mrSource === "all" && /^\d+$/.test(project.id));
  }

  /// 项目主页：Modrinth 与 CurseForge 的路径规则不同。
  function projectUrl(project: MrProject): string {
    const type = project.projectType || "mod";
    if (isFtb(project)) {
      return `https://www.feed-the-beast.com/modpacks/${project.id.slice(4)}`;
    }
    if (/^\d+$/.test(project.id)) {
      const slug =
        type === "mod"
          ? "mc-mods"
          : type === "datapack"
            ? "data-packs"
            : type === "resourcepack"
              ? "texture-packs"
              : type === "shader"
                ? "shaders"
                : type === "modpack"
                  ? "modpacks"
                  : "worlds";
      return `https://www.curseforge.com/minecraft/${slug}/${project.slug}`;
    }
    return `https://modrinth.com/${type}/${project.slug}`;
  }

  /// 详情页的版本列表按「项目 + 游戏版本 + 加载器」缓存：没选实例时
  /// Modrinth 要回 2.6 MB 全量版本（十几秒），来回切项目不该每次都等。
  /// 搜索结果缓存：键是「来源|分类|关键词|版本|加载器|排序|标签|支持端|偏移量」。
  interface SearchCacheEntry {
    at: number;
    items: MrProject[];
    total: number;
  }
  const searchCache = new Map<string, SearchCacheEntry>();
  // 搜索结果缓存 10 分钟：重复进同一个分类、来回切回来都不用重发请求。
  const SEARCH_CACHE_MS = 10 * 60 * 1000;

  const mrVersionCache = new Map<string, MrVersion[]>();

  /// 前置：项目信息、已装 modid、当前选中的版本、待补前置弹窗。
  let depMeta = $state<Record<string, { title: string; slug: string; icon: string | null }>>({});
  let depInstalled = $state<string[]>([]);
  let pickedVersionId = $state<string | null>(null);
  let depPrompt = $state<{ version: MrVersion; items: DepItem[] } | null>(null);
  let depBusy = $state(false);

  /// 连点两个项目时，只让最后一次的结果落地（Modrinth 全量版本可能好几十秒）。
  let mrOpenSeq = 0;

  /// 当前选中的版本（没点过就取最新的那个）。
  function pickedVersion(): MrVersion | null {
    return mrVersions.find((item) => item.id === pickedVersionId) ?? mrVersions[0] ?? null;
  }

  /// 已装 modid 只在第一次用到时读一次。
  async function ensureInstalledMods() {
    if (depInstalled.length > 0 || !gameDir) return;
    try {
      depInstalled = await api.installedModIds(gameDir, "");
    } catch {
      depInstalled = [];
    }
  }

  /// 把版本里声明的必需前置补齐项目名，并判断本地有没有。
  async function loadDeps(version: MrVersion | null | undefined) {
    if (!version) return;
    const ids = (version.dependencies ?? [])
      .filter((dep) => dep.dependencyType === "required" && dep.projectId)
      .map((dep) => dep.projectId as string);
    const missingMeta = ids.filter((id) => !depMeta[id]);
    try {
      await ensureInstalledMods();
      if (missingMeta.length > 0) {
        const loaded = await api.dependencyProjects(missingMeta);
        depMeta = { ...depMeta, ...loaded };
      }
    } catch {
      // 名字取不到就退回显示 id。
    }
  }

  type DepItem = { id: string; title: string; slug: string; installed: boolean };

  /// 当前版本的必需前置（去重），带「本地是否已装」。
  function requiredDeps(explicit?: MrVersion | null): DepItem[] {
    const version = explicit ?? pickedVersion();
    if (!version) return [];
    const out: DepItem[] = [];
    for (const dep of version.dependencies ?? []) {
      if (dep.dependencyType !== "required") continue;
      const id = dep.projectId ?? "";
      if (!id || out.some((item) => item.id === id)) continue;
      const meta = depMeta[id];
      const slug = meta?.slug ?? "";
      out.push({
        id,
        title: meta?.title ?? dep.fileName ?? id,
        slug,
        installed: depInstalled.includes(slug) || depInstalled.includes(id),
      });
    }
    return out;
  }

  /// 点前置：右侧切到它的详情，左侧列表不动。
  function openDependency(dep: { id: string; title: string; slug: string }) {
    if (!dep.slug) return;
    void mrOpen({
      id: dep.id,
      slug: dep.slug,
      title: dep.title,
      description: "",
      iconUrl: depMeta[dep.id]?.icon ?? null,
      downloads: 0,
      author: "",
      categories: [],
      projectType: "mod",
      loaders: [],
      gameVersions: [],
      chineseTitle: null,
    });
  }

  /// 点前置列表里的一条：切到它的详情页（带上已经取到的图标）。
  function openRef(dep: RefDetail) {
    if (!dep.slug) return;
    void mrOpen({
      id: dep.id,
      slug: dep.slug,
      title: dep.title,
      description: dep.description,
      iconUrl: dep.icon,
      downloads: dep.downloads,
      author: "",
      categories: [],
      projectType: "mod",
      loaders: [],
      gameVersions: [],
      chineseTitle: null,
    });
  }

  /// 缺的前置里挑一个能装的版本（先按当前实例版本/加载器过滤）。
  /// 前置版本必须跟「所点的那个文件」一致：同一个游戏版本、同一个加载器。
  /// 之前这里用的是实例的加载器，Fabric 模组会被配成 NeoForge 的前置。
  async function resolveDependencyFile(
    id: string,
    reference?: MrVersion | null
  ): Promise<MrVersion | null> {
    try {
      const game = reference?.gameVersions?.[0] ?? selectedGame;
      const loader = reference?.loaders?.[0] ?? activeLoader();
      const list = await api.modrinthVersions(id, game, loader);
      const sameGame = (item: MrVersion) => !game || item.gameVersions.includes(game);
      const sameLoader = (item: MrVersion) =>
        !loader || item.loaders.length === 0 || item.loaders.includes(loader);
      const exact = list.filter((item) => sameGame(item) && sameLoader(item));
      const byLoader = list.filter((item) => sameLoader(item));
      const pool = exact.length > 0 ? exact : byLoader.length > 0 ? byLoader : list;
      const sorted = [...pool].sort((a, b) => {
        const release = Number(b.versionType === "release") - Number(a.versionType === "release");
        return release !== 0 ? release : b.datePublished.localeCompare(a.datePublished);
      });
      return sorted[0] ?? null;
    } catch {
      return null;
    }
  }

  /// 按设置处理刚装完的模组的缺失前置：0 弹窗、1 直接装、2 不管。
  async function afterModDownload(version: MrVersion, dir?: string) {
    if (cat !== "mods" || !version.dependencies?.length) return;
    const mode = settings?.missingDepsMode ?? 0;
    if (mode === 2) return;
    depInstalled = [];
    await loadDeps(version);
    const missing = requiredDeps().filter((item) => !item.installed);
    if (missing.length === 0) return;
    if (mode === 0) {
      return;
    }
    depBusy = true;
    try {
      for (const dep of missing) {
        if (!dep.slug) continue;
        const file = await resolveDependencyFile(dep.id, version);
        if (file) await mrDownload(file, true, dir);
      }
    } finally {
      depBusy = false;
    }
  }

  /// 把缺的前置逐个装上（先挑兼容版本）。
  async function installMissingDeps(list: DepItem[], reference?: MrVersion | null, dir?: string) {
    const pending = list.filter((item) => !item.installed && item.slug);
    if (pending.length === 0) return;
    depBusy = true;
    try {
      for (const dep of pending) {
        const file = await resolveDependencyFile(dep.id, reference);
        if (file) await mrDownload(file, true, dir);
      }
    } finally {
      depBusy = false;
      depInstalled = [];
      await ensureInstalledMods();
    }
  }

  async function mrOpen(project: MrProject) {
    const seq = ++mrOpenSeq;
    mrProject = project;
    mrVersions = [];
    mrTranslation = null;
    {
      const source = isCurseForge(project) ? "curseforge" : "modrinth";
      const id = project.id;
      void api.projectTranslation(source, id).then((text) => {
        if (mrProject?.id === id) mrTranslation = text;
      });
    }
    mrWiki = null;
    mrFilePage = 1;
    fileGame = null;
    fileLoader = null;
    fileEnvironment = null;
    fileExpanded = new Set();
    groupAll = {};
    leftCollapsed = false;
    fileBusy = true;
    try {
      const game = selectedGame;
      const loader = activeLoader();
      const fromCurseForge = isCurseForge(project);
      const cacheKey = `${project.id}|${game ?? ""}|${loader ?? ""}`;
      const cached = mrVersionCache.get(cacheKey);
      // 百科按钮只对有词条的项目出现；查表是本地 JSON，失败就当没有。
      if (isFtb(project)) {
        mrWiki = null;
      } else void api
        .wikiLink(fromCurseForge ? "curseforge" : "modrinth", project.slug)
        .then((url) => {
          if (mrProject?.id === project.id) mrWiki = url;
        })
        .catch(() => {
          if (mrProject?.id === project.id) mrWiki = null;
        });
      const loaded = cached
        ? cached
        : isFtb(project)
          ? await api.ftbVersions(project.id, game, loader)
          : fromCurseForge
            ? await api.curseforgeVersions(project.id, game, loader)
            : await api.modrinthVersions(project.id, game, loader);
      if (seq !== mrOpenSeq) return;
      mrVersions = loaded;
      pickedVersionId = loaded[0]?.id ?? null;
      void loadDeps(loaded[0]);
      if (!cached && loaded.length > 0) mrVersionCache.set(cacheKey, loaded);
    } catch (e) {
      if (seq === mrOpenSeq) {
        showError("读取版本列表失败", "这个项目的文件没取回来。", String(e));
      }
    } finally {
      if (seq === mrOpenSeq) fileBusy = false;
    }
    if (seq === mrOpenSeq) persistState();
  }

  /// 结果提示里只留文件名，路径太长放不进胶囊。
  function baseName(path: string): string {
    const parts = path.split(/[\\/]/);
    return parts[parts.length - 1] || path;
  }

  function packNote(summary: { versionId: string; files: number; failed: number; missing: string[] }): string {
    if (summary.missing.length > 0) {
      const names = summary.missing.slice(0, 3).join("、");
      const more = summary.missing.length > 3 ? ` 等 ${summary.missing.length} 个` : "";
      return `已装好 ${summary.versionId}，但作者删掉了 ${names}${more}，这些文件没能装上`;
    }
    if (summary.failed > 0) {
      return `已装好 ${summary.versionId}，但有 ${summary.failed} 个文件没下成（重装可补）`;
    }
    return `已装好 ${summary.versionId}`;
  }

  /// 装完整合包后统一收口：缺文件/没下成的部分单独弹窗说清楚，其它走灵动岛。
  /// 顺手把实例目录带出去，任务卡才有「打开文件夹 / 移除并删除文件」。
  /// 把整合包来源写进实例的 `ybmcl.json`，概览页据此显示「整合包版」那一套。
  /// 只有从搜索页点进来、手上有确切项目时才调用；本地导入的包由后端照 manifest 写。
  async function rememberPack(
    versionId: string,
    versionLabel: string,
    project: MrProject | null = mrProject
  ) {
    if (!project || !versionId) return;
    const source = isFtb(project)
      ? "ftb"
      : isCurseForge(project)
        ? "curseforge"
        : "modrinth";
    try {
      await api.setVersionPack(versionId, {
        source,
        projectId: project.id,
        projectName: project.title,
        versionLabel,
        summary: project.description ?? "",
        downloads: project.downloads ?? 0,
        tags: (project.categories ?? []).slice(0, 8)
      });
    } catch {
      // 写不进去也不该把「装好了」变成失败，概览页少显示两行而已。
    }
  }

  function reportPack(summary: {
    versionId: string;
    files: number;
    failed: number;
    missing: string[];
    path: string;
  }): { note: string; output: { path: string; kind: "dir" } } {
    const note = packNote(summary);
    if (summary.missing.length > 0 || summary.failed > 0) {
      showError("整合包没有装全", note, summary.missing.join("、"));
    }
    return { note, output: { path: summary.path, kind: "dir" } };
  }

  /// 走下载队列、但允许暂停的任务（整合包安装）：暂停时卡片换成「继续」，
  /// 「继续」把同一条 job 重新排进队列，重跑的是同一段 `body`。
  function submitResumable(
    title: string,
    detail: string,
    body: () => Promise<TaskResult>
  ): Task {
    let task: Task;
    const run = async (): Promise<TaskResult> => {
      try {
        return await body();
      } catch (e) {
        const message = describeError(e);
        const stop = stopping ?? takeStop();
        if (message === "安装已取消" && stop === "pause") {
          const note = "已暂停，点「继续」接着装";
          pauseTask(task.id, note);
          announce(note);
          setTaskResume(task.id, () => requeueTask(task.id));
          return note;
        }
        throw e;
      }
    };
    task = submitTask(title, detail, run);
    return task;
  }

  /// 导入本地整合包（.mrpack / .zip / 带 manifest.json 的包），走同一套安装流程。
  async function importPack() {
    const dir = gameDir;
    if (!dir) {
      showError("还没有游戏目录", "先在「版本」页装一个实例，再回来导入整合包。");
      return;
    }
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
    const file = baseName(picked);
    const task = submitResumable(
      file,
      dir,
      async () => {
        try {
          const summary = await api.installModpack({
            gameDir: dir,
            url: null,
            path: picked,
            fileName: file,
            name: null,
            javaPath: null,
            strategy,
            threads
          }, task.id);
          // 来源由后端照包里的 manifest 写进实例，这里不再拿手头正开着的项目去冒充。
          await loadManifest(true);
          return reportPack(summary);
        } finally {
          packTaskId = null;
          untrackPack();
        }
      }
    );
    packTaskId = task.id;
  }

  /// 整合包一键安装：下载 .mrpack → 建实例 → 铺 mods/overrides，然后刷新版本列表。
  /// 正在跑的整合包安装任务（同一时刻只允许一个，后台进度事件据此写回任务卡）。
  let packTaskId: number | null = null;

  async function installPack(version: MrVersion) {
    const dir = gameDir;
    const name = mrProject?.title ?? null;
    announce(`开始安装整合包 ${version.versionNumber}`);
    const task = submitResumable(
      `${name ?? "整合包"} ${version.versionNumber}`,
      dir,
      async () => {
        try {
          const ftb = mrProject ? isFtb(mrProject) : false;
          // 数字 id 是 CurseForge，其余是 Modrinth；FTB 走另一条命令。装进实例后靠这份
          // 来源信息「检查更新」，包里自己的 manifest 认不出来。
          const projectId = mrProject?.id ?? null;
          const projectSource = ftb
            ? "ftb"
            : projectId && /^\d+$/.test(projectId)
              ? "curseforge"
              : "modrinth";
          const summary = ftb
            ? await api.installFtb({
                gameDir: dir,
                project: mrProject?.id ?? "",
                version: version.id.split(":")[1] ?? version.id,
                name,
                javaPath: null,
                strategy,
                iconUrl: mrProject?.iconUrl ?? null
              }, task.id)
            : await api.installModpack({
                gameDir: dir,
                url: version.fileUrl,
                path: null,
                fileName: version.fileName,
                name,
                javaPath: null,
                strategy,
                threads,
                iconUrl: mrProject?.iconUrl ?? null,
                size: version.fileSize,
                source: mrProject ? projectSource : null,
                projectId,
                projectName: mrProject?.title ?? null
              }, task.id);
          await rememberPack(summary.versionId, version.versionNumber);
          await loadManifest(true);
          return reportPack(summary);
        } finally {
          packTaskId = null;
          untrackPack();
        }
      }
    );
    packTaskId = task.id;
  }

  /// 系统保存对话框只弹一次：默认目录按落点规则算，用户在对话框里改到哪就下到哪。
  async function pickSaveDir(version: MrVersion): Promise<string | null> {
    const dir = await prepareDir(targetFor(version));
    const picked = await saveDialog({
      defaultPath: dir ? `${dir}\\${version.fileName}` : version.fileName,
      filters: [{ name: "文件", extensions: [version.fileName.split(".").pop() ?? "jar"] }]
    });
    if (!picked) return null;
    return picked.replace(/[\\/][^\\/]*$/, "");
  }

  /// 把一个文件下到指定目录，各自一张任务卡。
  function downloadInto(version: MrVersion, dir: string, fromDependency = false) {
    const kind = CATEGORIES.find((item) => item.id === cat)?.label ?? cat;
    const title = `${kind} · ${mrProject?.title ?? "内容"} ${version.versionNumber}`;
    announce(`已加入下载队列：${version.fileName}`);
    submitTask(title, dir, async () => {
      const written = await api.modrinthDownload(
        version.fileUrl,
        dir,
        version.fileName,
        mrProject?.slug,
        mrProject?.title,
        settings?.fileNameFormat
      );
      return { note: `已保存 ${baseName(written)}`, output: { path: written, kind: "file" as const } };
    });
    void fromDependency;
  }

  /// 点「下载」：先看这个文件缺哪些必需前置，缺就弹窗让你选「一起装 / 只装本体 / 取消」。
  async function requestDownload(version: MrVersion) {
    await loadDeps(version);
    const missing = requiredDeps(version).filter((item) => !item.installed);
    if (missing.length === 0) {
      const dir = await pickSaveDir(version);
      if (dir) downloadInto(version, dir);
      return;
    }
    depPrompt = { version, items: missing };
  }

  async function confirmDownload(withDeps: boolean) {
    const prompt = depPrompt;
    if (!prompt) return;
    depPrompt = null;
    const dir = await pickSaveDir(prompt.version);
    if (!dir) return;
    downloadInto(prompt.version, dir);
    if (!withDeps) return;
    depBusy = true;
    try {
      for (const dep of prompt.items) {
        const file = await resolveDependencyFile(dep.id, prompt.version);
        if (file) downloadInto(file, dir, true);
      }
    } finally {
      depBusy = false;
      depInstalled = [];
      await ensureInstalledMods();
    }
  }

  /// 点整行的动作：整合包/世界走安装，其余按普通文件下载。
  function rowAction(item: FileVersion) {
    if (cat === "packs") {
      if (settings?.modpackInstall ?? true) installPack(item);
      return;
    }
    if (cat === "worlds") {
      installWorld(item);
      return;
    }
    void mrDownload(item);
  }

  async function mrDownload(version: MrVersion, fromDependency = false, forcedDir?: string) {
    if (fromDependency) {
      const dir = await prepareDir(forcedDir ?? targetFor(version));
      downloadInto(version, dir, true);
      return;
    }
    await requestDownload(version);
  }

  /// 世界一键安装：下载 zip，按设置解压进 `saves`，或者让你另存为（默认落在 `downloads`）。
  async function installWorld(version: MrVersion) {
    const dir = gameDir;
    const extract = settings?.worldExtract ?? true;
    const title = `世界 · ${mrProject?.title ?? "存档"} ${version.versionNumber}`;
    const saves = worldSaveDir();
    const target = extract ? saves : `${dir}\\downloads`;
    announce(extract ? "正在下载世界包，下完自动解压" : "选个位置保存世界包");
    const task = submitTask(title, target, async () => {
      if (!extract) {
        const written = await api.saveWorld(version.fileUrl, version.fileName, target, task.id);
        return written
          ? { note: `已保存 ${baseName(written)}`, output: { path: written, kind: "file" as const } }
          : "已取消保存";
      }
      const path = await api.installWorld(version.fileUrl, version.fileName, dir, true, task.id, saves);
      return { note: `已解压 ${baseName(path)}`, output: { path, kind: "dir" as const } };
    });
  }

  // ---------------- 收藏夹 ----------------

  let favs = $state<Favorites>({ folders: [] });
  let favFolder = $state<string | null>(null);
  let favLoaded = $state(false);
  let favFilter = $state("");
  let favKind = $state("");
  let favVersion = $state("");
  let favLoader = $state("");
  let favQuery = $state("");
  /// 管理模式下才显示勾选框；勾中的条目 id 放在这里。
  let favManage = $state(false);
  let favSelected = $state(new Set<string>());
  /// 改名 / 新建共用的居中弹窗：null 表示没打开。
  let favDialog = $state<{ mode: "create" | "rename"; id: string | null; text: string } | null>(null);
  /// 自绘的确认框（删除收藏夹 / 批量删除）。WebView 里 window.confirm 不阻塞，不能用。
  let favConfirm = $state<{ message: string; run: () => void } | null>(null);
  let favMove = $state("");
  /// 非管理态下单选的收藏条目（右侧才显示「删除」）。
  let favPeek = $state<string | null>(null);
  /// 「☆ 收藏」弹窗：勾选只在点「保存」时一次性落盘。
  let favPick = $state<{ project: MrProject; selected: Set<string>; name: string; creating: boolean } | null>(null);
  let dragFolder = $state<string | null>(null);
  /// 拖动排序时指针悬停的目标夹子（用来画落点提示线）。
  let favDropTarget = $state<string | null>(null);

  const FAV_KINDS: { value: string; label: string }[] = [
    { value: "mod", label: "模组" },
    { value: "datapack", label: "数据包" },
    { value: "resourcepack", label: "资源包" },
    { value: "shader", label: "光影包" },
    { value: "modpack", label: "整合包" },
    { value: "worlds", label: "世界" }
  ];
  function favKindLabel(kind: string): string {
    return FAV_KINDS.find((item) => item.value === kind)?.label ?? kind;
  }

  const favCurrent = $derived(favs.folders.find((folder) => folder.id === favFolder) ?? null);
  /// 左栏收藏夹列表：按名称搜索。
  const favFolderList = $derived(
    favQuery.trim()
      ? favs.folders.filter((folder) => folder.name.toLowerCase().includes(favQuery.trim().toLowerCase()))
      : favs.folders
  );
  const favEntries = $derived(
    (favCurrent?.entries ?? [])
      .filter((entry) => (favKind ? entry.kind === favKind : true))
      .filter((entry) => (favVersion ? entry.versions.includes(favVersion) : true))
      .filter((entry) => (favLoader ? entry.loaders.includes(favLoader) : true))
      .filter((entry) =>
        favFilter.trim()
          ? `${entry.title} ${entry.description}`.toLowerCase().includes(favFilter.trim().toLowerCase())
          : true
      )
      .slice()
      .sort((left, right) => right.addedAt - left.addedAt)
  );
  /// 版本、加载器两个下拉的选项：当前收藏夹里所有条目支持项的并集，版本从新到旧。
  const favVersionOptions = $derived(
    [...new Set((favCurrent?.entries ?? []).flatMap((entry) => entry.versions))].sort(compareVersionName)
  );
  const favLoaderOptions = $derived(
    [...new Set((favCurrent?.entries ?? []).flatMap((entry) => entry.loaders))].sort((left, right) =>
      left.localeCompare(right)
    )
  );

  async function loadFavorites() {
    if (favLoaded) return;
    try {
      favs = await api.favoritesLoad();
      favFolder = favs.folders[0]?.id ?? null;
    } catch (e) {
      showError("读收藏夹失败", "本地收藏数据没有读出来。", String(e));
    } finally {
      favLoaded = true;
    }
  }

  async function persistFavorites() {
    favs = { ...favs };
    try {
      await api.favoritesSave(favs);
    } catch (e) {
      showError("保存收藏失败", "收藏没有写进本地文件。", String(e));
    }
  }

  function openFavDialog(mode: "create" | "rename", folder?: FavoriteFolder) {
    favDialog = { mode, id: folder?.id ?? null, text: mode === "rename" ? (folder?.name ?? "") : "" };
  }

  async function submitFavDialog() {
    const dialog = favDialog;
    const name = dialog?.text.trim();
    if (!dialog || !name) return;
    favDialog = null;
    if (dialog.mode === "create") {
      const folder: FavoriteFolder = {
        id: `f${Date.now()}${Math.floor(Math.random() * 1000)}`,
        name,
        createdAt: Date.now(),
        entries: []
      };
      favs = { ...favs, folders: [...favs.folders, folder] };
      favFolder = folder.id;
    } else {
      const folder = favs.folders.find((item) => item.id === dialog.id);
      if (!folder) return;
      folder.name = name;
      favs = { ...favs };
    }
    await persistFavorites();
  }

  async function removeFavoriteFolder(folder: FavoriteFolder) {
    // 不用 window.confirm：WebView 里它不阻塞，会先执行删除再弹框。
    favConfirm = {
      message: `删除收藏夹「${folder.name}」？`,
      run: () => {
        void (async () => {
          favs = { ...favs, folders: favs.folders.filter((item) => item.id !== folder.id) };
          if (favFolder === folder.id) favFolder = favs.folders[0]?.id ?? null;
          await persistFavorites();
        })();
      }
    };
  }

  async function moveFavoriteFolder(targetId: string) {
    if (!dragFolder || dragFolder === targetId) return;
    const list = [...favs.folders];
    const from = list.findIndex((item) => item.id === dragFolder);
    const to = list.findIndex((item) => item.id === targetId);
    if (from < 0 || to < 0) return;
    const [moved] = list.splice(from, 1);
    list.splice(to, 0, moved);
    favs = { ...favs, folders: list };
    dragFolder = null;
    await persistFavorites();
  }

  /**
   * 按住 ⠿ 上下拖动排序。
   *
   * 不用 HTML5 拖放：WebView 里从按钮上起拖经常起不来，改为自己跟指针，
   * 移动超过 6px 才算开始拖，落下时把夹子挪到指针下方的那个夹子前面。
   */
  function startFolderDrag(event: PointerEvent, folder: FavoriteFolder) {
    if (event.button !== 0) return;
    event.preventDefault();
    const startX = event.clientX;
    const startY = event.clientY;
    let started = false;

    const move = (moveEvent: PointerEvent) => {
      if (!started) {
        if (Math.abs(moveEvent.clientX - startX) < 6 && Math.abs(moveEvent.clientY - startY) < 6) return;
        started = true;
        dragFolder = folder.id;
      }
      const row = document.elementFromPoint(moveEvent.clientX, moveEvent.clientY)?.closest<HTMLElement>("[data-folder-id]");
      const target = row?.dataset.folderId ?? null;
      favDropTarget = target && target !== folder.id ? target : null;
    };

    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const target = favDropTarget;
      favDropTarget = null;
      if (started && target) void moveFavoriteFolder(target);
      else dragFolder = null;
    };

    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  function favToggleSelect(projectId: string) {
    const next = new Set(favSelected);
    if (next.has(projectId)) next.delete(projectId);
    else next.add(projectId);
    favSelected = next;
  }

  function favSelectAll() {
    favSelected = new Set(favEntries.map((entry) => entry.projectId));
  }

  function favClearSelect() {
    favSelected = new Set();
  }

  function favToggleManage() {
    favManage = !favManage;
    favSelected = new Set();
    favPeek = null;
  }

  function favDeleteSelected() {
    const folder = favCurrent;
    if (!folder || favSelected.size === 0) return;
    const count = favSelected.size;
    favConfirm = {
      message: `从「${folder.name}」里删除选中的 ${count} 项？`,
      run: () => {
        void (async () => {
          folder.entries = folder.entries.filter((entry) => !favSelected.has(entry.projectId));
          favSelected = new Set();
          favs = { ...favs };
          await persistFavorites();
        })();
      }
    };
  }

  async function favMoveSelected(targetId: string) {
    const folder = favCurrent;
    const target = favs.folders.find((item) => item.id === targetId);
    if (!folder || !target || target.id === folder.id || favSelected.size === 0) return;
    const moved = folder.entries.filter((entry) => favSelected.has(entry.projectId));
    folder.entries = folder.entries.filter((entry) => !favSelected.has(entry.projectId));
    target.entries = [...moved.filter((entry) => !target.entries.some((item) => item.projectId === entry.projectId)), ...target.entries];
    favSelected = new Set();
    favMove = "";
    favs = { ...favs };
    await persistFavorites();
  }

  function inFolder(folder: FavoriteFolder, projectId: string): boolean {
    return folder.entries.some((entry) => entry.projectId === projectId);
  }

  /// 收藏条目只有项目的基础字段，够 mrOpen 用了（它按 id 拉文件列表、按 id 形式判来源）。
  function entryProject(entry: FavoriteEntry): MrProject {
    return {
      id: entry.projectId,
      slug: entry.slug,
      title: entry.title,
      description: entry.description,
      iconUrl: entry.iconUrl,
      downloads: 0,
      author: "",
      categories: [],
      projectType: entry.kind,
      loaders: entry.loaders,
      gameVersions: entry.versions,
      chineseTitle: null
    };
  }

  const FAV_KIND_CATEGORY: Record<string, Category> = {
    mod: "mods",
    datapack: "datapacks",
    resourcepack: "resourcepacks",
    shader: "shaders",
    modpack: "packs",
    worlds: "worlds"
  };

  /// 右键收藏条目：切到它对应的下载页分类，并打开项目详情。
  async function openFavorite(entry: FavoriteEntry) {
    const target = FAV_KIND_CATEGORY[entry.kind] ?? "mods";
    if (cat !== target) await switchCategory(target);
    await mrOpen(entryProject(entry));
  }

  /// 非管理态：点行单选；管理态：点行多选。
  function favRowClick(entry: FavoriteEntry) {
    if (favManage) {
      favToggleSelect(entry.projectId);
      return;
    }
    favPeek = favPeek === entry.projectId ? null : entry.projectId;
  }

  function favPeekDelete(entry: FavoriteEntry) {
    const folder = favCurrent;
    if (!folder) return;
    favConfirm = {
      message: `从「${folder.name}」里删除「${entry.title}」？`,
      run: () => {
        void (async () => {
          folder.entries = folder.entries.filter((item) => item.projectId !== entry.projectId);
          if (favPeek === entry.projectId) favPeek = null;
          favs = { ...favs };
          await persistFavorites();
        })();
      }
    };
  }

  function openFavPick(project: MrProject) {
    const selected = new Set(favs.folders.filter((folder) => inFolder(folder, project.id)).map((folder) => folder.id));
    favPick = { project, selected, name: "", creating: false };
  }

  function toggleFavPick(folderId: string) {
    if (!favPick) return;
    const next = new Set(favPick.selected);
    if (next.has(folderId)) next.delete(folderId);
    else next.add(folderId);
    favPick = { ...favPick, selected: next };
  }

  async function createFolderInPick() {
    const pick = favPick;
    const name = pick?.name.trim();
    if (!pick || !name) return;
    const folder: FavoriteFolder = {
      id: `f${Date.now()}${Math.floor(Math.random() * 1000)}`,
      name,
      createdAt: Date.now(),
      entries: []
    };
    favs.folders = [...favs.folders, folder];
    favs = { ...favs };
    await persistFavorites();
    favPick = { ...pick, selected: new Set([...pick.selected, folder.id]), name: "", creating: false };
  }

  async function saveFavPick() {
    const pick = favPick;
    if (!pick) return;
    for (const folder of favs.folders) {
      const has = inFolder(folder, pick.project.id);
      const want = pick.selected.has(folder.id);
      if (want && !has) {
        folder.entries = [
          {
            projectId: pick.project.id,
            title: pick.project.title,
            slug: pick.project.slug,
            description: pick.project.description,
            iconUrl: pick.project.iconUrl,
            kind: MR_TYPES[cat] ?? "mod",
            addedAt: Date.now(),
            versions: [...new Set(pick.project.gameVersions)],
            loaders: [...new Set(pick.project.loaders)]
          },
          ...folder.entries
        ];
      } else if (!want && has) {
        folder.entries = folder.entries.filter((entry) => entry.projectId !== pick.project.id);
      }
    }
    favs = { ...favs };
    await persistFavorites();
    favPick = null;
  }

  function formatDay(value: number): string {
    if (!value) return "";
    const date = new Date(value);
    return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
  }

  // ---------------- 集市（全部） ----------------

  type Rail = { id: string; title: string; kind: Category; sort: string };

  /// 集市的主题行：混着各类内容，做出「大杂烩」的铺法。
  const RAILS: Rail[] = [
    { id: "mods-hot", title: "热门模组", kind: "mods", sort: "downloads" },
    { id: "mods-new", title: "新上架的模组", kind: "mods", sort: "newest" },
    { id: "packs-hot", title: "热门资源包", kind: "resourcepacks", sort: "downloads" },
    { id: "shaders-hot", title: "光影精选", kind: "shaders", sort: "downloads" },
    { id: "datapacks-hot", title: "热门数据包", kind: "datapacks", sort: "downloads" },
    { id: "mods-updated", title: "最近更新的模组", kind: "mods", sort: "updated" }
  ];

  /// 集市单条请求最多等多久（毫秒）。慢过这个就当那一站这次没回话。
  const RAIL_DEADLINE_MS = 15_000;

  let rails = $state<Record<string, MrProject[]>>({});
  let railsBusy = $state(false);
  let railsLoaded = $state(false);
  /// 最近一次拉集市是不是整片失败（两站都没回话）。失败要能看见、能重试，
  /// 不能像以前那样把空白当成「已经加载完」。
  let railsFailed = $state(false);
  const hero = $derived((rails["mods-hot"] ?? []).slice(0, 3));
  /// 一行都没拿到：这时摆空状态（加载中 / 失败 / 真的没有）而不是留一块白。
  const railsEmpty = $derived(RAILS.every((rail) => (rails[rail.id] ?? []).length === 0));

  /// 集市的列表请求单独给个短期限。Modrinth 在这边经常整条超时（后端单请求最长
  /// 40 秒），让它把一整页按住不值当：到点就用手上已有的那一站先摆出来。
  function withDeadline<T>(promise: Promise<T>, ms: number): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`超过 ${ms} 毫秒没回话`)), ms);
      promise.then(
        (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        (error) => {
          clearTimeout(timer);
          reject(error);
        }
      );
    });
  }

  async function loadRails(force = false) {
    // 已经拿到过内容就不再拉；但一片空白时总要重试——上一次可能只是两站都超时了。
    if (railsBusy || (!force && railsLoaded && !railsEmpty)) return;
    railsBusy = true;
    railsFailed = false;
    const game = selectedGame;
    const settled = new Set<string>();
    try {
      // 每一行两站各取 24 条：两站都收录的项目会被合并成一条、下载量相加，
      // 再多取一些才能把「一边榜上有名、另一边排名靠后」的项目也对上。
      // 两站分开落地：谁先回来谁先摆，不再等两条站十二条请求全部到齐。
      await Promise.all(
        RAILS.map(async (rail) => {
          let curseforge: MrProject[] | null = null;
          let modrinth: MrProject[] | null = null;
          const place = () => {
            if (curseforge === null && modrinth === null) return;
            const merged = dedupeRails([...(curseforge ?? []), ...(modrinth ?? [])]);
            const ordered =
              rail.sort === "downloads"
                ? merged.sort((left, right) => right.downloads - left.downloads)
                : dedupeRails(interleave(curseforge ?? [], modrinth ?? []));
            rails = { ...rails, [rail.id]: ordered.slice(0, 12) };
          };
          await Promise.all([
            withDeadline(
              api.curseforgeSearch(rail.kind, "", game ?? null, null, rail.sort, 24, 0),
              RAIL_DEADLINE_MS
            )
              .then((items) => {
                curseforge = items;
                place();
              })
              .catch(() => {
                /* 这一站没回话就用另一站的结果 */
              }),
            withDeadline(api.modrinthBrowse(rail.kind, rail.sort, game, 24), RAIL_DEADLINE_MS)
              .then((items) => {
                modrinth = items;
                place();
              })
              .catch(() => {
                /* 同上 */
              })
          ]);
          if (curseforge === null && modrinth === null) settled.add(rail.id);
        })
      );
      railsFailed = settled.size === RAILS.length;
      railsLoaded = true;
    } finally {
      railsBusy = false;
    }
  }

  /// 从集市的卡片点进某个项目：切到对应分类页并打开详情。
  async function openFromMarket(kind: Category, project: MrProject, list: MrProject[]) {
    cat = kind;
    mrProjects = list;
    // 集市卡片可能来自 CurseForge，交给 mrOpen 统一判断来源并按实例筛选。
    await mrOpen(project);
  }

  async function showRail(rail: Rail) {
    saveFiltersFor(cat);
    cat = rail.kind;
    mrProjects = [];
    mrProject = null;
    mrVersions = [];
    mrFilePage = 1;
    // 这一排自己的排序要带过去：「新上架」是按最新，「最近更新」是按更新时间。
    const base = defaultFilters(rail.kind);
    base.sort = rail.sort;
    mrFilters[rail.kind] = base;
    applyFiltersFor(rail.kind);
    // 拉真正的一页结果：以前这里只把集市缓存的那 12 条摆上来，翻页和「查看全部」都是死的。
    await ensurePage(1);
    // 打开第一条，右边详情不空着。
    const first = mrProjects[0];
    if (first) void mrOpen(first);
  }

  async function pickFolder() {
    try {
      const pickedFolder = await openDialog({
        directory: true,
        defaultPath: pkgDir || defaultPkgDir() || undefined
      });
      if (typeof pickedFolder === "string") pkgDir = pickedFolder;
    } catch (e) {
      pkgNote = String(e);
    }
  }

  async function downloadPackageOnce() {
    if (pkgBusy || !pkgVersion) return;
    // 原版没有第二级清单，版本号就是游戏版本；其他加载器要确认真在列表里。
    if (pkgKind !== "minecraft" && !pkgChoices.some((item) => item.version === pkgVersion)) return;
    const label = PACKAGE_KINDS.find((item) => item.id === pkgKind)?.label ?? pkgKind;
    pkgNote = `已加入下载队列：${label} ${pkgVersion}`;
    const dir = defaultPkgDir();
    const kind = pkgKind;
    const game = pkgGame;
    const version = pkgVersion;
    const ask = settings?.askSaveLocation ?? true;
    const pkgTask = submitTask(`${label} ${version}`, dir, async () => {
      if (ask) {
        const written = await api.savePackage(kind, game, version, strategy, dir, pkgTask.id);
        return written
          ? { note: `已保存 ${baseName(written)}`, output: { path: written, kind: "file" as const } }
          : "已取消保存";
      }
      const written = await api.downloadPackage(kind, game, version, dir, strategy, pkgTask.id);
      if (written.length === 0) return "没有可下载的文件";
      return {
        note: `已保存 ${written.length} 个文件到 ${dir}`,
        // 一次落了多个文件时不给「删除」：删一个留一堆比不删更糟。
        output: written.length === 1 ? { path: written[0], kind: "file" as const } : null
      };
    });
  }

  /// 载入当前分类需要的列表（恢复快照时用）。
  async function reloadForCategory() {
    if (MR_TYPES[cat]) {
      await mrSearch();
    } else if (cat === "packages") {
      await loadPackages();
    } else if (cat === "all") {
      await loadRails();
    } else if (cat === "favorites") {
      await loadFavorites();
    }
  }

  /// 按上一次留下的快照把界面摆回去：分类、筛选、标签、正在看的项目、展开的分组、滚动位置。
  /// 切换分类。页面刚起来、状态还没恢复完时先记下来，等恢复完再切，
  /// 免得「恢复上次状态」和用户点击同时改 cat/筛选，两边各跑一次加载。
  let restored = false;
  let pendingCategory: Category | null = null;

  $effect(() => {
    const current = cat;
    void tick().then(() => {
      const index = CATEGORIES.findIndex((item) => item.id === current);
      const node = catButtons[index];
      if (!node) return;
      catThumb = { left: node.offsetLeft, width: node.offsetWidth, ready: true };
    });
  });

  async function switchCategory(id: Category) {
    if (!restored) {
      pendingCategory = id;
      return;
    }
    const same = cat === id;
    // 点的就是当前分类：什么都不用做。以前这里会照常往下走，把列表清空、
    // 又因为 !same 为假而不重载，结果就是「点了当前标签，列表变空」。
    if (same) return;
    userPicked = true;
    // 各分类的筛选各存一份：切走前先把自己的记下，切回来再摆出来。
    saveFiltersFor(cat);
    cat = id;
    mrProject = null;
    mrVersions = [];
    if (MR_TYPES[id]) {
      applyFiltersFor(id);
      mrFilePage = 1;
      mrProjects = [];
      mrTotal = 0;
    }
    if (id === "packages") void loadPackages();
    if (id === "all") void loadRails();
    if (id === "favorites") void loadFavorites();
    if (!same && MR_TYPES[id]) void mrLoad();
    persistState();
  }

  async function restoreFromState(saved: DownloadsState) {
    cat = saved.cat as Category;
    mrFilters = { ...(saved.filters ?? {}) };
    // 旧格式只有一份扁平状态：把它当成当前分类的那份，别让升级后的第一次打开丢筛选。
    if (MR_TYPES[cat] && !mrFilters[cat]) {
      mrFilters[cat] = {
        query: saved.query ?? "",
        source: saved.source ?? "all",
        sort: saved.sort ?? "relevance",
        game: "",
        loader: saved.loader ?? "",
        env: saved.env ?? "all",
        tag: saved.tag ?? "全部",
        page: 1
      };
    }
    applyFiltersFor(cat);

    // 恢复页码：有存过第 2 页以后就照原页取，否则按第一页走。
    // 不 await：集市那十二条请求只要有一条慢，await 就会把 restored 卡在 false，
    // 期间点标签只会被排进 pendingCategory，看着就是「点了没反应、拉半天出不来」。
    if (MR_TYPES[cat]) void mrLoad();
    else void reloadForCategory();

    if (saved.project && MR_TYPES[cat]) {
      // 列表里找得到就用列表那份（字段最全）；找不到（比如实例页按项目 id 跳过来、
      // 但列表第一页没这个项目）就用存下来的那份，缺的字段补默认值。
      const stored = saved.project;
      const project: MrProject =
        mrProjects.find((item) => item.id === stored.id) ?? {
          id: stored.id,
          slug: stored.slug ?? "",
          title: stored.title,
          description: stored.description ?? "",
          iconUrl: stored.iconUrl ?? null,
          downloads: stored.downloads ?? 0,
          author: stored.author ?? "",
          categories: stored.categories ?? [],
          projectType: stored.projectType ?? "modpack",
          loaders: stored.loaders ?? [],
          gameVersions: stored.gameVersions ?? [],
          chineseTitle: stored.chineseTitle ?? null
        };
      await mrOpen(project);
      if (saved.loaderFilter) fileLoader = saved.loaderFilter;
      if (saved.expanded?.length) fileExpanded = new Set(saved.expanded);
    }

    await tick();
    if (listEl && saved.scrollTop > 0) listEl.scrollTop = saved.scrollTop;
  }

  function finishRestore() {
    restored = true;
    const pending = pendingCategory;
    pendingCategory = null;
    if (pending) void switchCategory(pending);
  }

  /// Esc：先关右侧详情；都没有就交给别的处理（不做全局返回）。
  function onEscape(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    if (favDialog) {
      favDialog = null;
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    if (favPick) {
      favPick = null;
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    if (favConfirm) {
      favConfirm = null;
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    if (tagOpen) {
      tagOpen = false;
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    if (mrProject) {
      mrProject = null;
      mrVersions = [];
      mrFilePage = 1;
      fileLoader = null;
      fileEnvironment = null;
      fileExpanded = new Set();
      leftCollapsed = false;
      persistState();
      event.preventDefault();
      event.stopPropagation();
    }
  }

  /// 点空白处收起标签下拉（它是浮层，不处理就会一直盖着下面的列表）。
  function onDocPointer(event: PointerEvent) {
    if (!tagOpen) return;
    const target = event.target as Element | null;
    if (target?.closest(".tag-select")) return;
    tagOpen = false;
  }

  onMount(async () => {
    window.addEventListener("keydown", onEscape, true);
    document.addEventListener("pointerdown", onDocPointer);
    const saved = downloadsState.value;
    if (saved) {
      try {
        await restoreFromState(saved);
      } catch (error) {
        // 恢复失败就退回默认状态，绝不能把页面卡住。
        console.warn("恢复下载页状态失败", error);
        cat = "all";
        void loadRails();
      }
    } else {
      void loadRails();
      if (MR_TYPES[cat]) {
        mrTag = TAGS[cat as keyof typeof TAGS]?.[0] ?? { name: "全部" };
        autoLoadCategory(cat);
      }
    }

    finishRestore();
    stateRestored = true;
    // 实例页的「详情 / 下载」带着搜索词过来：落到对应分类直接搜，然后把参数抹掉。
    const preset = page.url.searchParams.get("mod");
    if (preset !== null) {
      const wanted = page.url.searchParams.get("kind") ?? "";
      const kindMap: Record<string, Category> = {
        resourcepacks: "resourcepacks",
        shaders: "shaders",
        worlds: "worlds",
        saves: "worlds",
        datapacks: "datapacks",
        packs: "packs"
      };
      const kindCat: Category = kindMap[wanted] ?? "mods";
      cat = kindCat;
      // 实例页带过来的上下文：游戏版本 / 加载器只用来收窄筛选，搜索词永远是显示名。
      const presetGame = page.url.searchParams.get("mc") ?? "";
      const presetLoader = page.url.searchParams.get("loader") ?? "";
      mrFilters[kindCat] = {
        ...defaultFilters(kindCat),
        query: preset,
        game: presetGame,
        loader: kindCat === "mods" ? presetLoader : ""
      };
      applyFiltersFor(kindCat);
      await mrSearch();
      // 认领到项目时直接把详情打开（PCL 的 ResourceDetail）；认不到就停在搜索结果。
      const presetProject = page.url.searchParams.get("project") ?? "";
      const [source, projectId] = presetProject.split(":");
      if (projectId) {
        const hit = mrProjects.find(
          (item) =>
            item.id === projectId &&
            (source === "curseforge" ? isCurseForge(item) : !isCurseForge(item))
        );
        if (hit) {
          void mrOpen(hit);
        } else {
          // 列表里没搜到（名字不完全一致 / 被筛选挡掉）也把详情开出来，版本列表按 id 直接拉。
          void mrOpen({
            id: projectId,
            slug: "",
            title: preset,
            description: "",
            iconUrl: null,
            downloads: 0,
            author: "",
            categories: [],
            projectType: MR_TYPES[kindCat] ?? "mod",
            loaders: [],
            gameVersions: [],
            chineseTitle: null
          } satisfies MrProject);
        }
      }
      void goto("/downloads", { replaceState: true, keepFocus: true, noScroll: true });
    }
    try {
      await loadManifest();
      // 用的是上次存下来的清单：先把列表摆出来，再悄悄换一份新的。
      if (manifestAgeMs > 5 * 60_000) void refreshManifestQuietly();
    } catch (error) {
      console.warn("读取清单失败", error);
    }
  });

  /// 后台换新清单：不显示加载态、不发灵动岛，只把列表与缓存更新掉。
  async function refreshManifestQuietly() {
    try {
      const cached = await cachedManifest(strategy, true);
      versions = cached.value.versions;
      manifestAgeMs = 0;
      const kept = selected;
      if (kept && !versions.some((item) => item.id === kept.id)) selected = null;
    } catch {
      /* 后台刷新失败就继续用旧的 */
    }
  }

  /// 把当前界面状态记下来（模块 + localStorage），下次回到下载页时按它摆回去。
  /// 只在关键动作后显式调用——不用整页 $effect 监听，避免多个状态互相触发时反复重写。
  function persistState() {
    if (!stateRestored) return;
    saveFiltersFor(cat);
    saveDownloadsState({
      cat,
      filters: { ...mrFilters },
      query: mrQuery,
      source: mrSource,
      sort: mrSort,
      loader: mrLoader,
      env: mrEnv,
      tag: mrTag.name,
      project: mrProject
        ? {
            id: mrProject.id,
            title: mrProject.title,
            slug: mrProject.slug,
            description: mrProject.description,
            iconUrl: mrProject.iconUrl,
            downloads: mrProject.downloads,
            author: mrProject.author,
            categories: mrProject.categories,
            projectType: mrProject.projectType,
            loaders: mrProject.loaders,
            gameVersions: mrProject.gameVersions,
            chineseTitle: mrProject.chineseTitle
          }
        : null,
      loaderFilter: fileLoader,
      expanded: [...fileExpanded],
      scrollTop: listEl?.scrollTop ?? 0
    });
  }

  onDestroy(() => {
    persistState();
    window.removeEventListener("keydown", onEscape, true);
    document.removeEventListener("pointerdown", onDocPointer);
    for (const stop of unlisten) stop();
  });
  import BackButton from "$lib/components/BackButton.svelte";
</script>

<div class="page">
  <header class="head">
    <BackButton />
<h1>下载</h1>
    {#if manifestAgeMs > 60_000}
      <span class="dim">清单缓存于 {Math.round(manifestAgeMs / 60_000)} 分钟前</span>
    {/if}
    <div class="spacer"></div>
    {#if cat === "packs"}
      <button class="btn" onclick={importPack} disabled={!gameDir || mrBusy}>导入整合包</button>
    {/if}
    <button class="btn" onclick={() => loadManifest(true)} disabled={loading}>刷新清单</button>
  </header>

  <nav class="cats">
    <span
      class="cats-thumb"
      class:ready={catThumb.ready}
      style="width: {catThumb.width}px; transform: translateX({catThumb.left}px)"
    ></span>
    {#each CATEGORIES as item, index (item.id)}
      <button
        bind:this={catButtons[index]}
        class="cat"
        class:on={cat === item.id}
        onclick={() => void switchCategory(item.id)}>{item.label}</button
      >
    {/each}
  </nav>


  {#if cat === "minecraft"}
  <div class="split">
    <section class="list panel">
      <div class="tools">
        <input placeholder="搜索版本…" bind:value={query} />
        <Segmented bind:value={filter} options={[{ value: "release", label: "正式版" }, { value: "snapshot", label: "快照" }, { value: "fools", label: "愚人节" }, { value: "all", label: "全部" }]} />
      </div>

      {#if loading}
        <p class="dim pad">正在获取 Mojang 版本清单…</p>
      {:else}
        <ul>
          {#each shown as version (version.id)}
            {@const badge = badgeOf(version.type, version.id)}
            <li>
              <button
                class="row"
                class:active={selected?.id === version.id}
                onclick={() => {
                  selected = version;
                  clearLoaders();
                  entries = {};
                  typeFilter = {};
                  active = null;
                  void probeLoaders();
                }}
              >
                <span class="id">{version.id}</span>
                <span class="kind {badge.tone}" title={badge.label}>{badge.letter}</span>
                <span class="date">{version.releaseTime.slice(0, 10)}</span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section class="detail panel">
      {#if selected}
        <header class="detail-head">
          <h2>{selected.id}</h2>
          <span class="tag">{selected.type}</span>
          <span class="dim">{selected.releaseTime.slice(0, 10)}</span>
        </header>

        <div class="field">
          <div class="chips">
            <button class="chip" class:on={chosen.length === 0} onclick={clearLoaders}>
              原版
            </button>
            {#each LOADER_ORDER as item (item)}
              {#if supports(item)}
                <button
                  class="chip"
                  class:on={picked[item] !== undefined}
                  onclick={() => toggleLoader(item)}
                  disabled={false}
                  title={PRIMARY.includes(item) ? "主加载器同时只能选一个" : undefined}
                >
                  {LOADER_NAME[item]}
                </button>
              {/if}
            {/each}
          </div>

        </div>

        <label class="field">
          <span class="label">实例名</span>
          <input
            bind:value={instName}
            oninput={() => (nameTouched = true)}
            class:warn={nameTaken}
            placeholder={defaultName}
          />
          {#if nameTaken}
            <p class="hint error">版本 {instName} 已存在，请换一个名字。</p>
          {/if}
        </label>

        {#each chosen as kind (kind)}
          {@const options = visible(kind)}
          {@const current = options.find((item) => item.version === picked[kind]) ?? options[0]}
          <div class="loader-row">
            <span class="label">{LOADER_NAME[kind]}</span>
            {#if failed[kind]}
              <span class="loader-state">未拉取到</span>
              <button class="btn ghost loader-retry" onclick={() => retry(kind)}>
                重试
              </button>
            {:else if !(entries[kind]?.length)}
              <span class="loader-state">正在拉取中…</span>
            {:else}
            <div class="picker-row">
              <Segmented
                ariaLabel={`${LOADER_NAME[kind]} 版本类型`}
                value={typeFilter[kind] ?? "all"}
                options={[
                  { value: "all", label: "全部" },
                  { value: "stable", label: CHANNEL[kind].stable },
                  { value: "beta", label: CHANNEL[kind].beta }
                ]}
                onchange={(next) => setFilter(kind, next as "all" | "stable" | "beta")}
              />
              <div class="select">
                <button
                  class="select-btn"
                  class:empty={!current}
                  onclick={() => {
                    if (!current) return;
                    openMenu = openMenu === kind ? null : kind;
                  }}
                  disabled={!current}
                >
                  <span class="select-version">{current ? labelOf(kind, current) : "无"}</span>
                  {#if current && dateOf(kind, current.version)}
                    <span class="select-date">{dateOf(kind, current.version)}</span>
                  {/if}
                  <span class="select-caret">▾</span>
                </button>
                {#if openMenu === kind && current}
                  <div
                    class="menu"
                    role="listbox"
                    tabindex="-1"
                    onpointerdown={(event) => event.stopPropagation()}
                  >
                    {#each options as item (item.version)}
                      <button
                        class="menu-item"
                        class:on={item.version === picked[kind]}
                        onclick={() => {
                          setLoaderVersion(kind, item.version);
                          openMenu = null;
                        }}
                      >
                        <span class="menu-version">{labelOf(kind, item)}</span>
                        {#if dateOf(kind, item.version)}
                          <span class="menu-date">{dateOf(kind, item.version)}</span>
                        {/if}
                      </button>
                    {:else}
                      <p class="menu-empty">这一段没有可用的版本</p>
                    {/each}
                  </div>
                {/if}
              </div>
            </div>
            {/if}
          </div>
        {/each}

        {#if isModloader && !javaPlan?.path}
          <p class="hint">
            {#if javaPlan?.message}
              {javaPlan.message}
            {:else}
              本机还没检测到 Java，安装器可能跑不起来，请先在「设置」里指定。
            {/if}
          </p>
        {/if}

        {#if chosen.includes("cleanroom") && extras.length > 0}
          <div class="extra-mods">
            {#each extras as mod (mod.key)}
              {@const blocked = CLEANROOM_EXCLUSIVE.includes(mod.key) && CLEANROOM_EXCLUSIVE.some((other) => other !== mod.key && extraPicked[other])}
              <div class="field" class:dimmed={blocked}>
                <span class="label">{mod.label}</span>
                <Dropdown
                  value={extraPicked[mod.key] ?? ""}
                  label={mod.label}
                  disabled={extraBusy || blocked}
                  onchange={(file) => pickExtra(mod.key, file)}
                  options={[
                    { value: "", label: "不下载" },
                    ...mod.versions.map((version) => ({
                      value: String(version.file),
                      label: `${version.version} · ${version.fileName} · ${(version.size / 1048576).toFixed(1)} MB`
                    }))
                  ]}
                />
              </div>
            {/each}
            {#if extraBusy && extras.length === 0}
              <p class="hint">正在读取版本列表…</p>
            {/if}
          </div>
        {/if}

        <div class="actions">
          <button
            class="btn primary"
            bind:this={installButton}
            onclick={() => {
              launchInstallFlight(installButton?.getBoundingClientRect() ?? null);
              void install();
            }}
            disabled={waiting || !selected || nameTaken || missingBuild.length > 0}
          >
            安装
          </button>
        </div>

      {/if}
    
</section>
  </div>
  {:else if cat === "all"}
    <div class="market">
      {#if railsEmpty}
        <section class="panel empty-cat">
          {#if railsBusy}
            <p class="dim">正在从 Modrinth 和 CurseForge 拉集市内容…</p>
          {:else if railsFailed}
            <p class="dim">集市没拉到内容：Modrinth 和 CurseForge 这次都没回话。</p>
            <button class="btn" onclick={() => void loadRails(true)}>重试</button>
          {:else}
            <p class="dim">集市暂时没有内容。</p>
            <button class="btn" onclick={() => void loadRails(true)}>重试</button>
          {/if}
        </section>
      {:else}
        {#if hero.length > 0}
          <section class="hero-row">
            {#each hero as item (item.id)}
              <button class="hero-card" onclick={() => openFromMarket("mods", item, rails["mods-hot"] ?? [])}>
                <div class="hero-top">
                  {#if item.iconUrl}
                    <img class="hero-icon" src={item.iconUrl} alt="" />
                  {/if}
                  <span class="hero-tag">热门模组</span>
                </div>
                <h3>{item.title}</h3>
                <p class="hero-desc">{item.description}</p>
                <p class="mr-meta">{item.author} · {formatCount(item.downloads)} 次下载</p>
              </button>
            {/each}
          </section>
        {/if}

        {#if railsBusy}
          <p class="dim rail-loading">还有几行正在从两站拉…</p>
        {/if}

        {#each RAILS as rail (rail.id)}
          {#if (rails[rail.id] ?? []).length > 0}
            <section class="rail">
              <header class="rail-head">
                <h3>{rail.title}</h3>
                <button class="rail-more" onclick={() => void showRail(rail)}>查看全部 →</button>
              </header>
              <div class="rail-track">
                {#each rails[rail.id] as item (item.id)}
                  <button class="card" onclick={() => openFromMarket(rail.kind, item, rails[rail.id])}>
                    {#if item.iconUrl}
                      <img class="card-icon" src={item.iconUrl} alt="" />
                    {:else}
                      <span class="card-icon placeholder"></span>
                    {/if}
                    <span class="card-title">{item.title}</span>
                    <span class="card-author">{item.author}</span>
                    <span class="mr-meta">{formatCount(item.downloads)} 次下载</span>
                  </button>
                {/each}
              </div>
            </section>
          {/if}
        {/each}
      {/if}
    </div>
  {:else if cat === "favorites"}
    <div class="split fav-split">
      <section class="list panel">
        <div class="tools">
          <input class="search" bind:value={favQuery} placeholder="搜索收藏夹名称" />
          <button class="btn wide" onclick={() => openFavDialog("create")}>创建收藏夹</button>
        </div>
        {#if favFolderList.length === 0}
          <p class="pad">
            <span class="dim small">{favs.folders.length === 0 ? "还没有收藏夹。" : "没有匹配的收藏夹。"}</span>
          </p>
        {:else}
          <ul class="fav-folders">
            {#each favFolderList as folder (folder.id)}
              <li
                class:on={folder.id === favFolder}
                class:drop={favDropTarget === folder.id}
                data-folder-id={folder.id}
              >
                <button
                  class="fav-name"
                  onclick={() => {
                    favFolder = folder.id;
                    favPeek = null;
                    favSelected = new Set();
                  }}
                >{folder.name}</button>
                <div class="fav-actions">
                  <button class="tiny-btn" title="改名" onclick={() => openFavDialog("rename", folder)}>✎</button>
                  <button class="tiny-btn" title="删除" onclick={() => removeFavoriteFolder(folder)}>✕</button>
                  <button
                    class="tiny-btn handle"
                    title="按住拖动排序"
                    onpointerdown={(event) => startFolderDrag(event, folder)}
                  >⠿</button>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      <section class="detail panel">
        {#if favCurrent}
          <div class="tools">
            <div class="row-line">
              <strong>{favCurrent.name}</strong>
              <span class="dim small">创建于 {formatDay(favCurrent.createdAt)}</span>
            </div>
            <div class="fav-bar">
              <input bind:value={favFilter} placeholder="搜索收藏夹内容" />
              <Dropdown
                bind:value={favKind}
                label="种类"
                options={[{ value: "", label: "全部种类" }, ...FAV_KINDS]}
              />
              <Dropdown
                bind:value={favVersion}
                label="版本"
                options={[{ value: "", label: "全部版本" }, ...favVersionOptions.map((version) => ({ value: version, label: version }))]}
              />
              <Dropdown
                bind:value={favLoader}
                label="加载器"
                options={[{ value: "", label: "全部加载器" }, ...favLoaderOptions.map((loader) => ({ value: loader, label: loader }))]}
              />
              <button class="btn" class:on={favManage} onclick={favToggleManage}>{favManage ? "完成" : "管理"}</button>
            </div>
          </div>

          {#if favEntries.length === 0}
            <p class="dim small">{favCurrent.entries.length === 0 ? "这个收藏夹还是空的。" : "没有符合筛选的内容。"}</p>
          {:else}
            <ul class="mr-versions fav-entries">
              {#each favEntries as entry (entry.projectId)}
                <li class:on={favManage ? favSelected.has(entry.projectId) : favPeek === entry.projectId}>
                  <button
                    class="fav-row"
                    title="右键打开它的下载页"
                    onclick={() => favRowClick(entry)}
                    oncontextmenu={(event) => {
                      event.preventDefault();
                      void openFavorite(entry);
                    }}
                  >
                    <ProjectIcon
                      project={{ id: entry.projectId, title: entry.title, iconUrl: entry.iconUrl }}
                      size={36}
                    />
                    <span class="mr-version-text">
                      <span class="mr-title">{entry.title}</span>
                      <span class="mr-desc">{entry.description}</span>
                      <span class="mr-meta">{favKindLabel(entry.kind)} · 收藏于 {formatDay(entry.addedAt)}</span>
                    </span>
                  </button>
                  {#if !favManage && favPeek === entry.projectId}
                    <button
                      class="btn"
                      onclick={(event) => {
                        event.stopPropagation();
                        favPeekDelete(entry);
                      }}
                    >
                      删除
                    </button>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}

          {#if favManage}
            <div class="fav-batch">
              <span class="dim small">已选择 {favSelected.size} 项</span>
              <div class="fav-batch-actions">
                {#if favSelected.size < favEntries.length}
                  <button class="btn ghost" onclick={favSelectAll}>全选</button>
                {/if}
                {#if favSelected.size > 0}
                  <button class="btn ghost" onclick={favClearSelect}>取消选择</button>
                  <button class="btn" onclick={favDeleteSelected}>删除</button>
                  <Dropdown
                    bind:value={favMove}
                    label="转移到"
                    placeholder="转移到"
                    onchange={(value) => void favMoveSelected(value)}
                    options={favs.folders
                      .filter((folder) => folder.id !== favCurrent.id)
                      .map((folder) => ({ value: folder.id, label: folder.name }))}
                  />
                {/if}
              </div>
            </div>
          {/if}
        {/if}
      </section>
    </div>

  {:else if MR_TYPES[cat]}
    <div class="split" style="grid-template-columns: {settings?.splitRatio ?? 38}% minmax(0, 1fr)">
      <section class="list panel" bind:this={listEl}>
        <div class="tools">
          <input
            class="search"
            bind:value={mrQuery}
            placeholder={MR_HINT[MR_TYPES[cat] ?? ""] ?? "搜索"}
            onkeydown={(event) => {
              if (event.key === "Enter") void mrSearch();
            }}
          />
          <div class="filters">
            <Dropdown
              value={cat === "worlds" ? "curseforge" : mrSource}
              label="来源"
              onchange={(value) => {
                mrSource = value as "all" | "modrinth" | "curseforge";
                void mrSearch();
              }}
              options={sourceOptions}
            />
            <Dropdown
              bind:value={mrSort}
              label="排序"
              onchange={() => mrSearch()}
              options={[
                { value: "relevance", label: "相关性" },
                { value: "downloads", label: "下载量" },
                { value: "follows", label: "关注量" },
                { value: "newest", label: "最新发布" },
                { value: "updated", label: "最近更新" }
              ]}
            />
            <Dropdown
              bind:value={mrGame}
              label="游戏版本"
              editable
              placeholder="游戏版本（任意）"
              onchange={() => mrSearch()}
              options={gameSuggestions.map((id) => ({ value: id, label: id }))}
            />
            {#if cat === "mods" || cat === "packs" || cat === "shaders"}
              <Dropdown
                bind:value={mrLoader}
                label="加载器"
                onchange={() => mrSearch()}
                options={loaderOptions}
              />
            {/if}
            <div class="tag-select" class:open={tagOpen}>
              <button
                class="tag-btn"
                onclick={() => (tagOpen = !tagOpen)}
                aria-haspopup="listbox"
                aria-expanded={tagOpen}
              >
                <span class="tag-value">{mrTag.name}</span>
                <span class="caret" aria-hidden="true"></span>
              </button>
              {#if tagOpen}
                <div class="tag-menu" role="listbox">
                  {#each mrTags as item (item.name)}
                    <button
                      class="tag-item"
                      class:child={item.child}
                      class:on={item.name === mrTag.name}
                      role="option"
                      aria-selected={item.name === mrTag.name}
                      onclick={() => {
                        mrTag = item;
                        tagOpen = false;
                        void mrSearch();
                      }}
                    >
                      {item.name}
                    </button>
                  {/each}
                </div>
              {/if}
            </div>
            {#if cat === "mods" || cat === "resourcepacks" || cat === "datapacks"}
              <Dropdown
                bind:value={mrEnv}
                label="支持端"
                onchange={() => mrSearch()}
                options={SIDES.map((item) => ({ value: item.value, label: item.label }))}
              />
            {/if}
            {#if cat === "packs" || cat === "shaders" || cat === "resourcepacks" || cat === "datapacks"}
              <button class="btn" onclick={() => mrSearch()} disabled={mrBusy}>搜索</button>
            {/if}
          </div>
          {#if cat === "mods"}
            <button class="btn" onclick={() => mrSearch()} disabled={mrBusy}>搜索</button>
          {:else if cat === "worlds"}
            <button class="btn wide" onclick={() => mrSearch()} disabled={mrBusy}>搜索</button>
          {/if}
        </div>
        {#if mrBusy && mrProjects.length === 0}
          <ul class="mr-list skeleton">
            {#each Array(8) as _, index (index)}
              <li class="skeleton-row">
                <span class="skeleton-icon"></span>
                <span class="skeleton-lines">
                  <span class="skeleton-line w60"></span>
                  <span class="skeleton-line w90"></span>
                </span>
              </li>
            {/each}
          </ul>
        {/if}
        {#if listSourceWarning}
          <p class="rail-warn">{listSourceWarning}</p>
        {/if}
        {#if mrProjects.length > 0}
          <p class="dim small">
            共 {Math.max(mrTotal, mrProjects.length)} 个结果 · 每页 {mrPageSize} 条 · 第 {mrPage} / {mrPageCount} 页
          </p>
        {/if}
        {#if listHint}
          <p class="dim small">{listHint}</p>
        {/if}
        <ul class="mr-list">
          {#each mrProjects as item (item.id)}
            <li>
              <button class="mr-item" class:active={mrProject?.id === item.id} onclick={() => mrOpen(item)}>
                <ProjectIcon project={item} />
                <span class="mr-text">
                  <span class="mr-head">
                    <span class="mr-title">{item.chineseTitle ?? item.title}</span>
                    {#if item.chineseTitle}
                      <span class="mr-sub">{item.title}</span>
                    {/if}
                  </span>
                  {#if supportText(item)}
                    <span class="mr-support">{supportText(item)}</span>
                  {/if}
                  <span class="mr-desc">{item.description}</span>
                  <span class="mr-meta">
                    <span>{item.author} · {formatCount(item.downloads)} 次下载</span>
                    <span class="mr-source">{sourceOf(item)}</span>
                  </span>
                </span>
              </button>
            </li>
          {/each}
        </ul>
        {#if mrProjects.length > 0}
          <div class="pager">
            <button
              class="page-btn"
              aria-label="上一页"
              onclick={() => mrGo(mrPage - 1)}
              disabled={mrBusy || mrPage <= 1}
            >
              <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
                <path d="M15 6l-6 6 6 6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
              </svg>
            </button>
            <span class="page-num">第 {mrPage} / {mrPageCount} 页</span>
            <button
              class="page-btn next"
              aria-label="下一页"
              onclick={() => mrGo(mrPage + 1)}
              disabled={mrBusy || listEnded || mrPage >= mrPageCount}
            >
              <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
                <path d="M9 6l6 6-6 6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
              </svg>
            </button>
          </div>
          {#if mrBusy}
            <p class="dim small">正在补第 {mrPage} 页…</p>
          {:else if listEnded}
            <p class="dim small">没有更多了</p>
          {/if}
        {/if}
      </section>

      <section class="detail panel">
        {#if mrProject}
          {@const project = mrProject}
          <header class="comp-head">
            <ProjectIcon {project} size={44} />
            <div class="comp-title">
              <div class="comp-name">
                {project.chineseTitle ?? project.title}
                {#if project.chineseTitle}
                  <span class="comp-sub">{project.title}</span>
                {/if}
              </div>
              <div class="comp-meta">
                {#if supportText(project)}
                  <span class="comp-support">{supportText(project)}</span>
                {/if}
                <span>↓ {formatCount(project.downloads)}</span>
                {#if fileUpdated}
                  <span>↑ {fileUpdated.slice(0, 10)}</span>
                {/if}
                <span class="comp-source">{sourceOf(project)}</span>
              </div>
            </div>
          </header>

          {#if mrTranslation}
            <p class="comp-desc-line">{mrTranslation}</p>
          {/if}

          {#if fileBusy && mrVersions.length === 0}
            <p class="dim small">
              正在读取文件列表…
              {selected ? "" : "（没选实例时要取回全部版本，可能要十几秒）"}
            </p>
          {/if}
          <div class="comp-actions">
            <button class="btn ghost" onclick={() => api.openUrl(projectUrl(project))}>
              {sourceOf(project)}
            </button>
            {#if mrWiki}
              <button class="btn ghost" onclick={() => api.openUrl(mrWiki ?? "")}>转到 MC 百科</button>
            {/if}
            <button class="btn ghost" onclick={() => copyText(project.title)}>复制名称</button>
            <button class="btn ghost" onclick={() => copyText(projectUrl(project))}>复制链接</button>
            <button class="btn ghost" onclick={() => openFavPick(project)}>☆ 收藏</button>
          </div>

          {#if depPrompt}
            <div class="dep-backdrop" role="presentation" onclick={() => (depPrompt = null)}>
              <div class="dep-modal" role="dialog" onclick={(event) => event.stopPropagation()}>
                <h3>需要前置</h3>
                <ul class="dep-modal-list">
                  {#each depPrompt.items as dep (dep.id)}
                    <li>
                      <span class="dep-name">{dep.title}</span>
                      <span class="dep-state">未安装</span>
                    </li>
                  {/each}
                </ul>
                <div class="dep-modal-actions">
                  <button class="btn" onclick={() => void confirmDownload(true)}>一起装</button>
                  <button class="btn ghost" onclick={() => void confirmDownload(false)}>只装本体</button>
                  <button class="btn ghost" onclick={() => (depPrompt = null)}>取消</button>
                </div>
              </div>
            </div>
          {/if}

          {#if depView}
            <div class="dep-backdrop" role="presentation" onclick={() => depBack()}>
              <div
                class="dep-sheet"
                style="--dep-split: {depSplit}%"
                role="dialog"
                aria-label={depView.dep.title}
                onclick={(event) => event.stopPropagation()}
              >
                <section class="dep-left">
                  <header class="dep-head">
                    {#if depView.dep.icon}
                      <img class="dep-icon-lg" src={depView.dep.icon} alt="" />
                    {:else}
                      <span class="dep-icon-lg"></span>
                    {/if}
                    <div class="dep-head-text">
                      <h3>{depView.dep.title}</h3>
                      <span class="dep-from">{depView.dep.source}</span>
                    </div>
                  </header>
                  <p class="dep-desc">{depView.dep.description}</p>
                  <div class="dep-stats">
                    <span>↓ {formatCount(depView.dep.downloads)}</span>
                    {#if depView.dep.updated}
                      <span>↑ {depView.dep.updated.slice(0, 10)}</span>
                    {/if}
                  </div>
                  <div class="dep-filters">
                    <label class="dep-filter">
                      <span>游戏版本</span>
                      <Dropdown
                        value={depGame}
                        label="游戏版本"
                        onchange={(value) => (depGame = value)}
                        options={[
                          { value: "all", label: "全部" },
                          ...depGames.map((name) => ({ value: name, label: name }))
                        ]}
                      />
                    </label>
                    <label class="dep-filter">
                      <span>加载器</span>
                      <Dropdown
                        value={depLoader}
                        label="加载器"
                        onchange={(value) => (depLoader = value)}
                        options={[
                          { value: "all", label: "全部" },
                          ...depLoaders.map((name) => ({ value: name, label: loaderName(name) }))
                        ]}
                      />
                    </label>
                    <label class="dep-filter">
                      <span>客户端/服务端</span>
                      <Dropdown
                        value={depEnv}
                        label="客户端/服务端"
                        onchange={(value) => (depEnv = value)}
                        options={[
                          { value: "all", label: "全部" },
                          { value: "client", label: "客户端" },
                          { value: "server", label: "服务端" },
                          { value: "both", label: "双端" }
                        ]}
                      />
                    </label>
                  </div>
                  <div class="dep-actions">
                    <button class="btn ghost" onclick={() => void api.openUrl(projectUrl(depProject(depView!.dep)))}>
                      {depView!.dep.source}
                    </button>
                    {#if depWiki}
                      <button class="btn ghost" onclick={() => void api.openUrl(depWiki ?? "")}>转到 MC 百科</button>
                    {/if}
                    <button class="btn ghost" onclick={() => copyText(depView!.dep.title)}>复制名称</button>
                    <button class="btn ghost" onclick={() => copyText(projectUrl(depProject(depView!.dep)))}>
                      复制链接
                    </button>
                    <button class="btn ghost" onclick={() => openFavPick(depProject(depView!.dep))}>☆ 收藏</button>
                  </div>
                </section>
                <div
                  class="dep-gutter"
                  role="separator"
                  aria-orientation="vertical"
                  onpointerdown={depGutterDown}
                ></div>
                <section class="dep-right" bind:this={depRightEl}>
                  {#if depView.loading}
                    <p class="dim small">正在读取文件列表…</p>
                  {:else if depView.versions.length === 0}
                    <p class="dim small">没有可用的文件。</p>
                  {:else}
                    {#if depBlocks.length === 0}
                      <p class="dim small">没有符合这些条件的文件。</p>
                    {:else}
                      {#each depBlocks as block (block.key)}
                        <section class="group-card">
                          <button
                            class="group-head"
                            onclick={() => (depOpen = { ...depOpen, [block.key]: !depOpen[block.key] })}
                          >
                            <span class="group-title">{block.title}</span>
                            <span class="group-count">{block.files.length}</span>
                            <svg
                              class="group-arrow"
                              class:open={depOpen[block.key]}
                              viewBox="0 0 24 24"
                              width="16"
                              height="16"
                              aria-hidden="true"
                            >
                              <path d="M6 9l6 6 6-6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
                            </svg>
                          </button>
                          {#if depOpen[block.key]}
                            <ul class="mr-versions">
                              {#each block.files as item (item.id)}
                                <li onclick={() => void mrDownload(item)}>
                                  <div class="mr-version-text">
                                    <span class="mr-title">{item.versionNumber}</span>
                                    <span class="mr-desc">{item.name}</span>
                                    <span class="mr-meta">
                                      {item.gameVersions.slice(0, 4).join(" / ")}{item.loaders.length > 0
                                        ? ` · ${item.loaders.map(loaderName).join(" / ")}`
                                        : ""}{item.fileSize > 0 ? ` · ${formatSize(item.fileSize)}` : ""}{item.datePublished
                                        ? ` · ${item.datePublished.slice(0, 10)}`
                                        : ""}
                                    </span>
                                  </div>
                                </li>
                              {/each}
                            </ul>
                          {/if}
                        </section>
                      {/each}
                    {/if}
                  {/if}
                </section>
              </div>
            </div>
          {/if}

          {#snippet versionRows(items: FileVersion[], key: string)}
            {@const shown = items}
            <ul class="mr-versions">
              {#each shown as item (item.id)}
                <li class:active={pickedVersionId === item.id} onclick={() => rowAction(item)}>
                  <div
                    class="mr-version-text"
                    role="button"
                    tabindex="0"
                    onclick={(event) => {
                      event.stopPropagation();
                      pickedVersionId = item.id;
                      void loadDeps(item);
                    }}
                  >
                    <span class="mr-title">{item.versionNumber}</span>
                    <span class="mr-desc">{item.name}</span>
                    <span class="mr-meta">
                      {item.gameVersions.slice(0, 4).join(" / ")}{item.loaders.length > 0 ? ` · ${item.loaders.map(loaderName).join(" / ")}` : ""}{item.fileSize > 0 ? ` · ${formatSize(item.fileSize)}` : ""}{item.datePublished ? ` · ${item.datePublished.slice(0, 10)}` : ""}
                    </span>
                    {#if settings?.compatWarning && selected && !item.gameVersions.includes(selectedGame ?? "")}
                      <span class="mr-warn">与实例 {selected.id} 的版本不匹配，仍然可以下载</span>
                    {/if}
                  </div>
                  {#if cat === "packs" && (settings?.modpackInstall ?? true)}
                    <button class="btn" onclick={(event) => { event.stopPropagation(); installPack(item); }} disabled={mrBusy}>安装</button>
                  {:else if cat === "worlds"}
                    <button class="btn" onclick={(event) => { event.stopPropagation(); installWorld(item); }} disabled={mrBusy}>
                      {(settings?.worldExtract ?? true) ? "安装" : "下载"}
                    </button>
                  {/if}
                </li>
              {/each}
            </ul>
          {/snippet}

          <div class="filter-card">
            <div class="filter-row">
              <span class="filter-pair">
                <span class="filter-label">游戏版本：</span>
                <Dropdown
                  value={fileGame ?? "all"}
                  label="游戏版本"
                  onchange={(value) => (fileGame = value === "all" ? null : value)}
                  options={[
                    { value: "all", label: "全部" },
                    ...fileGameChoices.map((name) => ({ value: name, label: name }))
                  ]}
                />
              </span>
              <span class="filter-pair">
                <span class="filter-label">加载器：</span>
                <Dropdown
                  value={fileLoader ?? "all"}
                  label="加载器"
                  onchange={(value) => (fileLoader = value === "all" ? null : value)}
                  options={[
                    { value: "all", label: "全部" },
                    ...fileLoaderChoices.map((name) => ({ value: name, label: loaderName(name) }))
                  ]}
                />
              </span>
              <span class="filter-pair">
                <span class="filter-label">客户端/服务端：</span>
                <Dropdown
                  value={fileEnvironment ?? "all"}
                  label="客户端/服务端"
                  onchange={(value) => (fileEnvironment = value === "all" ? null : (value as EnvironmentFilter))}
                  options={[
                    { value: "all", label: "全部" },
                    { value: "client", label: "客户端" },
                    { value: "server", label: "服务端" },
                    { value: "both", label: "双端" }
                  ]}
                />
              </span>
            </div>
          </div>

          {#if shownBlocks.length === 0}
            <p class="dim small">没有符合这些条件的文件。</p>
          {:else}
            {#each shownBlocks as block (block.key)}
              <section class="group-card">
                <button class="group-head" onclick={() => toggleBlock(block.key, block)}>
                  <span class="group-title">{block.title}</span>
                  <span class="group-count">{block.files.length}</span>
                  <svg class="group-arrow" class:open={fileExpanded.has(block.key)} viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
                    <path d="M6 9l6 6 6-6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
                  </svg>
                </button>
                {#if fileExpanded.has(block.key)}
                  {#if (blockDeps[block.key] ?? []).length > 0}
                    <div class="block-deps">
                      <p class="block-deps-head">前置资源</p>
                      <ul class="block-deps-list">
                        {#each blockDeps[block.key] as dep (dep.id)}
                          <li
                            class="block-dep"
                            role="button"
                            tabindex="0"
                            onclick={() => void openDep(dep)}
                          >
                            {#if dep.icon}
                              <img class="block-dep-icon" src={dep.icon} alt="" loading="lazy" />
                            {:else}
                              <span class="block-dep-icon"></span>
                            {/if}
                            <div class="block-dep-main">
                              <span class="block-dep-name">{dep.title}</span>
                              <span class="block-dep-desc">{dep.description}</span>
                            </div>
                            <span class="block-dep-stat">↓ {formatCount(dep.downloads)}</span>
                            <span class="block-dep-stat">↑ {dep.updated.slice(0, 10)}</span>
                            <span class="block-dep-source">{dep.source}</span>
                          </li>
                        {/each}
                      </ul>
                    </div>
                  {/if}
                  {@render versionRows(block.files, block.key)}
                {/if}
              </section>
            {/each}
          {/if}
        {/if}
      </section>
    </div>
  {:else if cat === "packages"}
    <div class="split pkg-split">
      <section class="list panel pkg-kinds">
        <div class="tools">
          <span class="label">安装包（单独下载，不创建实例）</span>
        </div>
        <ul>
          {#each PACKAGE_KINDS as item (item.id)}
            <li>
              <button
                class="row"
                class:active={pkgKind === item.id}
                onclick={() => {
                  pkgKind = item.id;
                  pkgVersion = "";
                  pkgChoices = [];
                  void loadPackages();
                }}
              >
                <span class="id">{item.label}</span>
              </button>
            </li>
          {/each}
        </ul>
      </section>

      <section class="detail panel pkg-form">
        <Segmented bind:value={pkgFilter} options={[{ value: "release", label: "正式版" }, { value: "snapshot", label: "快照" }, { value: "fools", label: "愚人节" }, { value: "all", label: "全部" }]} />

        <label class="field">
          <span class="label">游戏版本</span>
          {#if pkgGames.length === 0}
            <p class="hint">清单里没有这一类的版本。</p>
          {:else}
            <Dropdown
              value={pkgGame}
              options={pkgGames.map((version) => ({
                value: version.id,
                label: version.id,
                hint: `${badgeOf(version.type, version.id).label} · ${version.releaseTime.slice(0, 10)}`
              }))}
              onchange={(next) => pickPkgGame(next)}
            />
          {/if}
        </label>

        {#if pkgKind !== "minecraft"}
          <label class="field">
            <span class="label">{PACKAGE_KINDS.find((item) => item.id === pkgKind)?.label} 版本</span>
            {#if pkgLoading}
              <p class="hint">正在读取这个游戏版本可用的加载器版本…</p>
            {:else if pkgChoices.length === 0}
              <p class="hint">这个游戏版本没有可用的加载器版本。</p>
            {:else}
              <Dropdown
                value={pkgVersion}
                options={pkgChoices.map((item) => ({
                  value: item.version,
                  label: item.version,
                  hint: item.recommended ? "推荐" : item.stable ? "正式版" : "测试版"
                }))}
                onchange={(next) => (pkgVersion = next)}
              />
            {/if}
          </label>
        {/if}

        <div class="pkg-foot">
          <span class="dim">
            {#if pkgKind === "minecraft"}
              下载 Minecraft {pkgGame || "原版"} · 打包成 zip
            {:else}
              {@const label = PACKAGE_KINDS.find((item) => item.id === pkgKind)?.label ?? ""}
              {label}{pkgVersion ? ` ${pkgVersion}` : ""} · {pkgGame || "先挑游戏版本"}
            {/if}
          </span>
          {#if pkgNote}
            <span class="hint">{pkgNote}</span>
          {/if}
          <button class="btn primary" onclick={downloadPackageOnce} disabled={pkgBusy || !pkgVersion}>
            {pkgBusy ? "下载中…" : "下载"}
          </button>
        </div>
      </section>
    </div>
  {:else}
    <section class="panel empty-cat">
      <p class="dim">{CATEGORY_NOTE[cat]}</p>
    </section>
  {/if}
    {#if favDialog}
      <div
        class="fav-mask"
        role="presentation"
        onclick={() => (favDialog = null)}
        onkeydown={(event) => {
          if (event.key === "Escape") favDialog = null;
        }}
      >
        <div
          class="fav-dialog"
          role="dialog"
          aria-modal="true"
          tabindex="-1"
          onclick={(event) => event.stopPropagation()}
          onkeydown={(event) => event.stopPropagation()}
        >
          <p>{favDialog.mode === "create" ? "新建收藏夹" : "修改为"}</p>
          <div class="row-line">
            <!-- svelte-ignore a11y_autofocus -->
            <input
              bind:value={favDialog.text}
              autofocus
              onkeydown={(event) => {
                if (event.key === "Enter") void submitFavDialog();
                if (event.key === "Escape") favDialog = null;
              }}
            />
            <button class="btn" disabled={!favDialog.text.trim()} onclick={() => void submitFavDialog()}>确认</button>
          </div>
        </div>
      </div>
    {/if}

    {#if favConfirm}
      <div
        class="fav-mask"
        role="presentation"
        onclick={() => (favConfirm = null)}
        onkeydown={(event) => {
          if (event.key === "Escape") favConfirm = null;
        }}
      >
        <div
          class="fav-dialog"
          role="dialog"
          aria-modal="true"
          tabindex="-1"
          onclick={(event) => event.stopPropagation()}
          onkeydown={(event) => event.stopPropagation()}
        >
          <p>{favConfirm.message}</p>
          <div class="row-line fav-confirm">
            <button class="btn ghost" onclick={() => (favConfirm = null)}>取消</button>
            <button
              class="btn"
              onclick={() => {
                const run = favConfirm?.run;
                favConfirm = null;
                run?.();
              }}>删除</button
            >
          </div>
        </div>
      </div>
    {/if}

    {#if favPick}
      <div
        class="fav-mask"
        role="presentation"
        onclick={() => (favPick = null)}
        onkeydown={(event) => {
          if (event.key === "Escape") favPick = null;
        }}
      >
        <div
          class="fav-dialog"
          role="dialog"
          aria-modal="true"
          tabindex="-1"
          onclick={(event) => event.stopPropagation()}
          onkeydown={(event) => event.stopPropagation()}
        >
          <p>收藏到收藏夹</p>
          {#if favs.folders.length === 0}
            <p class="dim small">还没有收藏夹，先建一个。</p>
          {:else}
            <ul class="fav-pick-list">
              {#each favs.folders as folder (folder.id)}
                <li>
                  <label class="star-row">
                    <input
                      type="checkbox"
                      checked={favPick.selected.has(folder.id)}
                      onchange={() => toggleFavPick(folder.id)}
                    />
                    <span>{folder.name}</span>
                  </label>
                </li>
              {/each}
            </ul>
          {/if}
          {#if favPick.creating}
            <div class="row-line">
              <input class="search" bind:value={favPick.name} placeholder="收藏夹名称" />
              <button class="btn" disabled={!favPick.name.trim()} onclick={() => void createFolderInPick()}
                >新建</button
              >
            </div>
          {:else}
            <button
              class="btn ghost"
              onclick={() => {
                if (favPick) favPick = { ...favPick, creating: true };
              }}>新建收藏夹</button
            >
          {/if}
          <div class="row-line fav-confirm">
            <button class="btn ghost" onclick={() => (favPick = null)}>取消</button>
            <button class="btn primary" onclick={() => void saveFavPick()}>保存</button>
          </div>
        </div>
      </div>
    {/if}
</div>

<style>
  .page {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 14px;
    height: 100%;
    min-height: 0;
  }
  .cats {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    position: relative;
  }
  /* 会滑动的绿色底片：位置和宽度由当前按钮决定，切换分类时左右移动。 */
  .cats-thumb {
    position: absolute;
    top: 2px;
    bottom: 2px;
    left: 0;
    border-radius: 999px;
    background: color-mix(in srgb, var(--yb-green) 24%, transparent);
    border: 1px solid color-mix(in srgb, var(--yb-green) 40%, transparent);
    pointer-events: none;
    opacity: 0;
  }
  .cats-thumb.ready {
    opacity: 1;
    transition:
      transform 0.2s cubic-bezier(0.2, 0.7, 0.3, 1),
      width 0.2s cubic-bezier(0.2, 0.7, 0.3, 1);
  }
  .cat {
    padding: 5px 14px;
    border-radius: 999px;
    border: 1px solid transparent;
    background: transparent;
    color: var(--text-dim);
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }
  .cat:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  .cat.on {
    background: transparent;
    border-color: transparent;
    color: var(--text);
  }
  .row-line {
    display: flex;
    gap: 8px;
  }
  .row-line input {
    flex: 1;
  }
  /* 左栏整块滚动：工具条、分类、结果连同「加载更多」一起走，
     这样按钮永远不会被面板裁掉（PCL 也是列表滚到底再翻页）。 */
  .list {
    overflow-y: auto;
  }
  .mr-list,
  .mr-versions {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0;
    padding: 0;
  }
  /* 翻页/搜索时的骨架行：没有内容就先占位，避免旧列表留在屏幕上 */
  .skeleton-row {
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 8px;
  }
  .skeleton-icon,
  .skeleton-line {
    background: color-mix(in srgb, var(--surface-2) 82%, transparent);
    border-radius: 8px;
    animation: skeleton-pulse 1.1s ease-in-out infinite;
  }
  .skeleton-icon {
    width: 44px;
    height: 44px;
    border-radius: 10px;
    flex: 0 0 auto;
  }
  .skeleton-lines {
    display: flex;
    flex-direction: column;
    gap: 8px;
    flex: 1;
  }
  .skeleton-line {
    height: 10px;
  }
  .skeleton-line.w60 {
    width: 60%;
  }
  .skeleton-line.w90 {
    width: 90%;
  }
  @keyframes skeleton-pulse {
    0%, 100% { opacity: 0.45; }
    50% { opacity: 0.85; }
  }

  /* 左栏的列表不参与压缩，撑开面板让它整块滚动 */
  .empty-result {
    margin: 18px 0 0;
    color: var(--danger);
    font-size: 13px;
  }

  .mr-list {
    flex: none;
  }
  /* 右栏的版本列表自己滚，避免把项目信息顶走 */
  .mr-versions {
    flex: 1;
    min-height: 0;
    max-height: 46vh;
    overflow-y: auto;
  }
  .mr-item {
    display: flex;
    gap: 10px;
    width: 100%;
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    text-align: left;
    align-items: flex-start;
  }
  .mr-item.active {
    background: color-mix(in srgb, var(--yb-green) 26%, transparent);
  }
  /* 标题行：中文名 + 灰色英文名整体最多折两行，不再省略（名字优先）。 */
  .mr-head {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    overflow: hidden;
    width: 100%;
    min-width: 0;
    word-break: break-word;
  }
  .mr-title {
    font-weight: 600;
  }
  .mr-sub {
    margin-left: 6px;
    font-size: 12px;
    color: var(--text-dim);
    opacity: 0.85;
  }
  /* 支持版本单独一行，只有一行，太长就省略。 */
  .mr-support {
    display: block;
    margin-top: 1px;
    max-width: 100%;
    font-size: 11px;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mr-meta {
    display: flex;
    gap: 8px;
    width: 100%;
  }
  .mr-source {
    margin-left: auto;
    flex: 0 0 auto;
    opacity: 0.75;
  }
  .mr-icon.fallback {
    display: flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in srgb, var(--yb-green) 22%, transparent);
    color: var(--text);
    font-weight: 700;
  }
  .mr-icon {
    width: 36px;
    height: 36px;
    border-radius: 8px;
    object-fit: cover;
    flex: none;
  }
  .mr-text,
  .mr-version-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .mr-title {
    font-weight: 600;
  }
  .mr-desc {
    font-size: 12px;
    opacity: 0.72;
    overflow: hidden;
    text-overflow: ellipsis;
    display: -webkit-box;
    line-clamp: 2;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .mr-meta {
    font-size: 11px;
    opacity: 0.55;
  }
  .mr-versions li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
  }
  .market {
    display: flex;
    flex-direction: column;
    gap: 18px;
    overflow-y: auto;
    padding-right: 4px;
  }
  .hero-row {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 10px;
  }
  .hero-card {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 14px;
    border-radius: var(--radius-md);
    background: linear-gradient(160deg, var(--line-soft), var(--tint-1));
    border: 1px solid var(--border);
    text-align: left;
    min-height: 132px;
  }
  .hero-card:hover {
    border-color: color-mix(in srgb, var(--yb-green) 45%, transparent);
  }
  .hero-top {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .hero-icon {
    width: 34px;
    height: 34px;
    border-radius: 9px;
    object-fit: cover;
  }
  .hero-tag {
    font-size: 11px;
    padding: 1px 8px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--yb-green) 26%, transparent);
  }
  .hero-card h3 {
    margin: 4px 0 0;
    font-size: 15px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hero-desc {
    margin: 0;
    font-size: 12px;
    opacity: 0.72;
    overflow: hidden;
    display: -webkit-box;
    line-clamp: 2;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .rail-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    margin-bottom: 8px;
  }
  .rail-head h3 {
    margin: 0;
    font-size: 14px;
  }
  .rail-more {
    font-size: 12px;
    opacity: 0.65;
  }
  .rail-more:hover {
    opacity: 1;
  }
  .rail-track {
    display: flex;
    gap: 8px;
    overflow-x: auto;
    padding-bottom: 4px;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 3px;
    width: 148px;
    flex: none;
    padding: 10px;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
    text-align: left;
  }
  .card:hover {
    background: var(--surface-hover);
  }
  .card-icon {
    width: 40px;
    height: 40px;
    border-radius: 10px;
    object-fit: cover;
    margin-bottom: 4px;
  }
  .card-icon.placeholder {
    background: var(--line-soft);
  }
  .card-title {
    font-size: 13px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .card-author {
    font-size: 11px;
    opacity: 0.6;
  }
  .fav-folders {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    overflow-y: auto;
  }
  .fav-folders li {
    display: flex;
    align-items: center;
    gap: 4px;
    border-radius: var(--radius-sm);
    padding: 2px 4px;
  }
  .fav-folders li:hover {
    background: var(--surface-hover);
  }
  .fav-folders li.on {
    background: color-mix(in srgb, var(--yb-green) 18%, transparent);
  }
  .fav-folders li.drop {
    box-shadow: inset 0 2px 0 var(--yb-green-bright);
  }
  .fav-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: left;
  }
  .fav-actions {
    display: flex;
    gap: 2px;
    opacity: 0.55;
    transition: opacity 0.15s ease;
  }
  .fav-folders li:hover .fav-actions,
  .fav-folders li.on .fav-actions {
    opacity: 1;
  }
  .tiny-btn {
    min-width: 20px;
    padding: 0 3px;
    border-radius: 6px;
    background: transparent;
    color: inherit;
    font-size: 12px;
    line-height: 18px;
    opacity: 0.75;
  }
  .tiny-btn:hover {
    background: var(--surface-hover);
    opacity: 1;
  }
  .tiny-btn.handle {
    cursor: grab;
  }
  .btn.on {
    border-color: color-mix(in srgb, var(--yb-green) 60%, transparent);
    background: color-mix(in srgb, var(--yb-green) 22%, transparent);
  }
  .fav-split {
    position: relative;
  }
  .fav-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: nowrap;
  }
  .fav-bar input {
    flex: 1 1 auto;
    min-width: 0;
  }
  .fav-bar :global(.dropdown) {
    flex: 0 0 132px;
  }
  .fav-bar .btn {
    flex: 0 0 auto;
  }
  .fav-batch {
    position: sticky;
    bottom: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-top: auto;
    padding: 8px 12px;
    border-top: 1px solid var(--border);
    background: color-mix(in srgb, var(--surface) 88%, transparent);
    backdrop-filter: var(--glass-8);
  }
  .fav-batch-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .fav-entries li {
    justify-content: flex-start;
    gap: 10px;
    align-items: flex-start;
    background: transparent;
    cursor: pointer;
  }
  .fav-row {
    flex: 1;
    display: flex;
    align-items: flex-start;
    gap: 10px;
    min-width: 0;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    text-align: left;
    cursor: pointer;
  }
  .fav-entries li > .btn {
    margin-left: auto;
    align-self: center;
  }
  .fav-entries li:hover {
    background: var(--surface-hover);
  }
  .fav-entries li.on:hover {
    background: color-mix(in srgb, var(--yb-green) 16%, transparent);
  }
  .fav-entries li.on {
    background: color-mix(in srgb, var(--yb-green) 12%, transparent);
  }
  .fav-pick-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 40vh;
    overflow-y: auto;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .fav-mask {
    position: fixed;
    inset: 0;
    z-index: 80;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.62);
  }
  .fav-dialog {
    width: min(420px, 82vw);
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 16px 18px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border-strong);
    background: var(--panel-solid);
    box-shadow: 0 18px 48px rgba(0, 0, 0, 0.45);
  }
  .fav-dialog p {
    margin: 0;
    font-size: 14px;
  }
  .fav-confirm {
    justify-content: flex-end;
  }
  .tiny {
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 6px;
    opacity: 0.7;
  }
  .tiny:hover {
    opacity: 1;
    background: var(--surface-2);
  }
  .star-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 8px 0;
    max-height: 132px;
    overflow-y: auto;
  }
  /* Cleanroom 自带的两个 mod：只剩名字 + 下拉，纵向排开。 */
  .extra-mods {
    display: grid;
    gap: 10px;
    margin: 2px 0 4px;
  }
  .extra-mods .field.dimmed {
    opacity: 0.45;
  }
  .chip {
    padding: 4px 10px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .chip.on {
    background: color-mix(in srgb, var(--yb-green) 28%, transparent);
    border-color: color-mix(in srgb, var(--yb-green) 60%, transparent);
  }
  .more-row {
    padding: 10px 12px 4px;
    display: flex;
    justify-content: center;
  }
  .mr-warn {
    font-size: 11px;
    color: var(--warn-text);
  }
  .empty-cat {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    min-height: 220px;
  }
  .rail-loading {
    font-size: 12px;
    margin: -6px 0 0 2px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 650;
  }
  .spacer {
    flex: 1;
  }
  .dim {
    color: var(--text-dim);
    font-size: 12px;
    -webkit-user-select: text;
    user-select: text;
    word-break: break-all;
  }
  .split {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 300px minmax(0, 1fr);
    /* 行高必须确定，子面板才能靠 flex 撑出滚动区域 */
    grid-template-rows: minmax(0, 1fr);
    gap: 12px;
  }
  /* 安装包页：类别 / 游戏版本 / 加载器版本三列 + 底部下载条 */
  .pkg-split {
    grid-template-columns: 210px minmax(0, 1fr);
  }
  .pkg-form {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 16px;
    overflow-y: auto;
  }
  /* 一行四个小按钮，别拉满整行 */
  .pkg-form .pkg-foot {
    margin-top: auto;
  }
  .pkg-foot {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 12px;
    padding: 10px 14px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: var(--surface);
  }
  .pkg-foot .dim {
    margin-right: auto;
  }
  @media (max-width: 1120px) {
    .pkg-split {
      grid-template-columns: 170px minmax(0, 1fr);
    }
  }
  .list {
    display: flex;
    flex-direction: column;
    min-height: 0;
    /* 左栏整块滚动：结果多时「加载更多」在列表末尾，不能被裁掉 */
    overflow-y: auto;
    overflow-x: hidden;
  }
  .tools {
    /* 滚动容器里的子项默认会被压缩，筛选栏一展开就把搜索按钮和列表挤没了 */
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
    border-bottom: 1px solid var(--border);
  }
  input,
  input:focus,
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    padding: 5px 12px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--tint-1);
    color: var(--text-dim);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .chip.on {
    border-color: rgba(120, 200, 140, 0.55);
    background: rgba(120, 200, 140, 0.18);
    color: var(--text);
  }
  .chip:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .loader-row {
    display: grid;
    gap: 8px;
  }
  .loader-state {
    font-size: 13px;
    color: var(--text-dim);
    padding: 8px 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--wall-chip, rgba(8, 10, 14, 0.35));
  }
  .loader-retry {
    justify-self: start;
    padding: 6px 12px;
    font-size: 12px;
  }
  .picker-row {
    display: flex;
    gap: 8px;
    align-items: stretch;
    flex-wrap: wrap;
  }
  .seg {
    display: inline-flex;
    padding: 3px;
    border-radius: 10px;
    background: var(--tint-1);
    border: 1px solid var(--tint-3);
  }
  .seg-btn {
    border: 0;
    background: transparent;
    color: var(--text-4);
    font: inherit;
    font-size: 12px;
    padding: 5px 12px;
    border-radius: 8px;
    cursor: pointer;
  }
  .seg-btn:hover:not(:disabled) {
    color: #fff;
  }
  .seg-btn.on {
    background: var(--line);
    color: #fff;
    box-shadow: inset 0 1px 0 var(--line);
  }
  .seg-btn:disabled {
    cursor: default;
    opacity: 0.5;
  }
  .select {
    position: relative;
    flex: 1 1 240px;
  }
  .select-btn {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 12px;
    border-radius: 10px;
    border: 1px solid var(--line);
    background: var(--tint-2);
    color: #fff;
    font: inherit;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }
  .select-btn.empty {
    color: var(--glint-strong);
    border-style: dashed;
  }
  .select-btn:hover:not(:disabled) {
    border-color: var(--line-strong);
    background: var(--line-soft);
  }
  .select-version {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .select-date {
    color: var(--text-3);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .select-caret {
    color: var(--text-3);
    font-size: 11px;
  }
  .menu {
    position: absolute;
    z-index: 20;
    left: 0;
    right: 0;
    top: calc(100% + 6px);
    max-height: 320px;
    overflow-y: auto;
    padding: 6px;
    border-radius: 14px;
    border: 1px solid var(--line);
    /* 毛玻璃：半透明面板 + 背景模糊 + 内高光，和 Dropdown 组件同一套材质 */
    background: rgba(24, 30, 22, 0.78);
    backdrop-filter: var(--glass-22);
    -webkit-backdrop-filter: var(--glass-22);
    box-shadow:
      0 18px 40px rgba(0, 0, 0, 0.42),
      inset 0 1px 0 var(--line);
    animation: menu-in 0.14s ease;
  }
  @keyframes menu-in {
    from {
      opacity: 0;
      transform: translateY(-4px) scaleY(0.96);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  .menu-item {
    display: flex;
    align-items: baseline;
    gap: 12px;
    width: 100%;
    padding: 7px 10px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }
  .menu-item:hover {
    background: var(--tint-3);
  }
  .menu-item.on {
    background: rgba(120, 180, 255, 0.22);
    color: #fff;
  }
  .menu-version {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .menu-date {
    color: var(--text-3);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .menu-empty {
    margin: 6px 10px;
    color: var(--text-3);
    font-size: 12px;
  }
  input.warn {
    border-color: rgba(230, 110, 110, 0.7);
  }
  .hint.error {
    color: rgb(240, 140, 140);
  }
  ul {
    margin: 0;
    padding: 6px;
    list-style: none;
    overflow-y: auto;
    flex: 1;
    min-height: 0;
  }
  .row {
    width: 100%;
    display: grid;
    grid-template-columns: 1fr auto auto;
    gap: 8px;
    align-items: center;
    padding: 8px 10px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .row:hover {
    background: var(--tint-2);
  }
  .row.active {
    background: rgba(127, 163, 94, 0.2);
  }
  .id {
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* 版本类型徽章：R 正式 / S 快照 / B 旧 Beta / A 旧 Alpha / F 愚人节 */
  .kind {
    flex: none;
    width: 18px;
    height: 18px;
    display: grid;
    place-items: center;
    font-size: 11px;
    font-weight: 600;
    line-height: 1;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 5px;
  }
  .kind.release {
    color: #9cc7ff;
    border-color: rgba(120, 170, 255, 0.45);
    background: rgba(90, 140, 220, 0.16);
  }
  .kind.snapshot {
    color: var(--text-dim);
    background: var(--tint-1);
  }
  .kind.beta {
    color: var(--warn-text);
    border-color: rgba(240, 198, 116, 0.4);
    background: rgba(240, 198, 116, 0.12);
  }
  .kind.alpha {
    color: var(--warn-text);
    border-color: rgba(232, 160, 106, 0.4);
    background: rgba(232, 160, 106, 0.12);
  }
  .kind.fool {
    color: #c9a0ff;
    border-color: rgba(200, 160, 255, 0.45);
    background: rgba(160, 120, 220, 0.16);
  }
  .date {
    font-size: 11px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  .detail {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 18px;
    overflow-y: auto;
  }

  /* 详情页顶部项目卡片（对齐 PCL 的 compDetail 头部） */
  .comp-head {
    flex: none;
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }
  .comp-desc-line {
    margin: 0 18px 10px;
    font-size: 13px;
    line-height: 1.6;
    color: var(--text-dim);
  }
  .comp-title {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
    flex: 1;
  }
  .comp-name {
    font-size: 17px;
    font-weight: 600;
    line-height: 1.3;
    word-break: break-word;
  }
  .comp-sub {
    margin-left: 6px;
    font-size: 12px;
    font-weight: 400;
    color: var(--text-dim);
  }
  .comp-line {
    display: flex;
    gap: 6px;
    align-items: baseline;
    min-width: 0;
    font-size: 12px;
    color: var(--text-dim);
  }
  .comp-tag {
    flex: none;
    padding: 1px 7px;
    border-radius: 6px;
    background: rgba(120, 170, 240, 0.18);
    color: var(--text);
    font-size: 11px;
  }
  .comp-desc {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .comp-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    align-items: baseline;
    font-size: 11px;
    color: var(--text-dim);
  }
  .comp-source {
    margin-left: auto;
    opacity: 0.8;
  }
  .comp-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin: 10px 0 4px;
  }
  .btn.ghost {
    padding: 5px 12px;
    font-size: 12px;
    border-radius: 999px;
    background: var(--tint-1);
  }
  /* 两级筛选卡片 */
  .filter-card {
    flex: none;
    margin: 12px 0;
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--tint-1);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .filter-row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }
  /* 每一组筛选各占一行，标签定宽，三行的第一个胶囊上下对齐 */
  .filter-label {
    flex: none;
    width: 104px;
    font-size: 12px;
    color: var(--text-dim);
    white-space: nowrap;
  }
  /* 版本分组卡片 */
  .more-files {
    margin-top: 8px;
    width: 100%;
    justify-content: center;
  }

  .dep-block {
    margin: 0 0 10px;
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: var(--radius-md);
    background: var(--tint-1);
  }
  .dep-head {
    font-size: 12px;
    color: var(--muted);
    margin-bottom: 8px;
  }
  .dep-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .dep-row {
    display: grid;
    grid-template-columns: 34px minmax(0, 1fr) auto;
    gap: 10px;
    align-items: center;
    width: 100%;
    padding: 6px 8px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: inherit;
    text-align: left;
    cursor: pointer;
  }
  .dep-row:hover {
    background: var(--tint-1);
  }
  .dep-icon {
    width: 34px;
    height: 34px;
    border-radius: 8px;
    object-fit: cover;
    background: var(--tint-2);
  }
  .dep-main {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .dep-name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dep-desc {
    font-size: 12px;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dep-meta {
    display: flex;
    gap: 10px;
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }

  .group-card {
    flex: none;
    margin-bottom: 10px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--tint-1);
    overflow: hidden;
  }
  .group-head {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 11px 14px;
    border: 0;
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: 14px;
    font-weight: 600;
    text-align: left;
    cursor: pointer;
  }
  .group-title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .group-arrow {
    color: var(--text-dim);
    transition: transform 0.15s ease;
  }
  .group-arrow.open {
    transform: rotate(180deg);
  }
  .detail-head {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }
  .detail-head h2 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
  }
  .tag {
    font-size: 11px;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 1px 7px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .label {
    font-size: 12px;
    color: var(--text-dim);
  }
  .hint {
    margin: 0;
    font-size: 12px;
    color: var(--text-dim);
  }
  .hint.warn {
    color: var(--accent);
  }
  .actions {
    display: flex;
    gap: 10px;
  }
  .pad {
    padding: 14px;
  }

  /* 三行筛选：来源/排序、版本/加载器、标签/支持端 */
  .filters {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: 8px;
  }
  /* 搜索按钮占满一行（资源包/数据包/世界是第三行） */
  .btn.wide {
    width: 100%;
    justify-content: center;
  }
  /* 标签下拉：分组 + 缩进子项，和 PCL 的「标签」一致 */
  .tag-select {
    position: relative;
    min-width: 0;
  }
  /* 收起态必须和 Dropdown 的 .trigger 逐条一致（含毛玻璃）；
     少一条 backdrop-filter，渲染出来就会比旁边的下拉亮一档 */
  .tag-btn {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 8px 12px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--tint-1);
    backdrop-filter: var(--glass-14-plus);
    color: var(--text);
    font: inherit;
    text-align: left;
    cursor: pointer;
    transition:
      border-color 0.15s ease,
      background 0.15s ease;
  }
  .tag-btn:hover {
    background: var(--tint-2);
  }
  .tag-select.open .tag-btn {
    border-color: color-mix(in srgb, var(--yb-green) 55%, transparent);
  }
  /* 和 Dropdown 一样：键盘聚焦用淡绿描边，不用浏览器默认黑框 */
  .tag-btn:focus-visible {
    outline: 2px solid color-mix(in srgb, var(--yb-green) 60%, transparent);
    outline-offset: 2px;
  }
  .tag-value {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* 箭头直接复刻 Dropdown 的 .caret：两条 1.5px 边转 45 度 */
  .tag-btn .caret {
    flex: none;
    width: 7px;
    height: 7px;
    border-right: 1.5px solid currentColor;
    border-bottom: 1.5px solid currentColor;
    transform: translateY(-2px) rotate(45deg);
    opacity: 0.7;
  }
  /* 弹层浮在内容上，不能占着流里的位置，否则展开就把下面的搜索按钮和列表挤走 */
  .tag-menu {
    position: absolute;
    z-index: 30;
    top: calc(100% + 6px);
    left: 0;
    right: 0;
    max-height: 320px;
    overflow-y: auto;
    padding: 6px;
    border-radius: 14px;
    border: 1px solid var(--line);
    /* 标签菜单直接盖在项目列表上，比 Dropdown 的 .menu 更实一点，否则后面的标题会透出来 */
    background: rgba(24, 30, 22, 0.94);
    backdrop-filter: var(--glass-22);
    -webkit-backdrop-filter: var(--glass-22);
    box-shadow:
      0 18px 40px rgba(0, 0, 0, 0.42),
      inset 0 1px 0 var(--line);
    display: flex;
    flex-direction: column;
    gap: 2px;
    animation: menu-in 0.14s ease;
  }
  @keyframes menu-in {
    from {
      opacity: 0;
      transform: translateY(-4px) scaleY(0.96);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  .tag-item {
    padding: 8px 10px;
    border-radius: 9px;
    border: 0;
    background: transparent;
    color: var(--text);
    font: inherit;
    text-align: left;
    white-space: nowrap;
    cursor: pointer;
  }
  .tag-item:hover {
    background: var(--line-soft);
  }
  .tag-item.child {
    padding-left: 28px;
    color: var(--text-dim);
  }
  .tag-item.on {
    background: color-mix(in srgb, var(--yb-green) 26%, transparent);
    color: var(--text);
  }
  /* 单源失败/窗口释放这类提醒 */
  .rail-warn {
    margin: 0 0 8px;
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    border: 1px solid color-mix(in srgb, #e0b04a 40%, var(--border));
    background: color-mix(in srgb, #e0b04a 12%, transparent);
    color: var(--warn-text);
    font-size: 12.5px;
    line-height: 1.5;
  }
  /* 翻页：只有「‹ 第 N / M 页 ›」，不能跳页 */
  .pager {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 2px;
    margin: 12px auto 14px;
    padding: 6px 10px;
    width: fit-content;
    border-radius: var(--radius-md);
    border: 1px solid var(--border);
    background: color-mix(in srgb, var(--surface-2) 70%, transparent);
    flex: 0 0 auto;
    align-self: center;
  }
  .page-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 24px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--text);
    cursor: pointer;
    transition: background 0.15s ease, opacity 0.15s ease;
  }
  .page-btn:hover:not(:disabled) {
    background: color-mix(in srgb, var(--yb-green) 22%, transparent);
  }
  .page-btn:disabled {
    opacity: 0.2;
    cursor: not-allowed;
  }
  .page-num {
    min-width: 34px;
    text-align: center;
    font-size: 15px;
    color: var(--text);
  }

  .dep-card {
    margin: 0 0 12px;
    padding: 10px 14px;
    border-radius: 10px;
    background: var(--tint-1);
  }
  .dep-head {
    font-size: 13px;
    color: var(--text-dim);
    margin-bottom: 8px;
  }
  .dep-note {
    margin-left: 6px;
    font-size: 12px;
    opacity: 0.7;
  }
  .dep-list {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-bottom: 8px;
  }
  .dep-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 10px;
    border-radius: 999px;
    border: 1px solid var(--tint-3);
    background: transparent;
    color: inherit;
    cursor: pointer;
  }
  .dep-item:hover {
    border-color: var(--line-strong);
  }
  .dep-state {
    font-size: 12px;
    color: #e07a5f;
  }
  .dep-state.on {
    color: var(--green, #7bc96f);
  }
  .dep-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.62);
    backdrop-filter: blur(2px);
    display: grid;
    place-items: center;
    z-index: 60;
  }
  .dep-modal {
    width: min(460px, 86vw);
    padding: 18px 20px;
    border-radius: 14px;
    background: var(--panel-solid, #23262d);
    color: var(--text, #e6e6e6);
  }
  .dep-modal h3 {
    margin: 0 0 6px;
    font-size: 15px;
  }
  .dep-modal p {
    margin: 0 0 10px;
    font-size: 13px;
    color: var(--text-dim);
  }
  .dep-modal-list {
    margin: 0 0 14px;
    padding-left: 18px;
    font-size: 13px;
  }
  .dep-modal-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
  .mr-versions li.active {
    background: var(--tint-2);
    border-radius: 8px;
  }

  .block-deps {
    margin: 0 14px 10px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--surface);
  }
  .block-deps-head {
    margin: 0 0 8px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .block-deps-list {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 220px;
    overflow-y: auto;
  }
  .block-dep {
    display: grid;
    grid-template-columns: 34px minmax(0, 1fr) auto auto auto;
    align-items: center;
    gap: 10px;
    padding: 6px 0;
    border-top: 1px solid var(--border);
  }
  .block-dep:first-child {
    border-top: 0;
  }
  .block-dep-icon {
    width: 34px;
    height: 34px;
    border-radius: 8px;
    object-fit: cover;
    background: var(--surface-2);
  }
  .block-dep-main {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .block-dep-name {
    font-size: 13px;
    color: var(--text);
  }
  .block-dep-desc {
    font-size: 12px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .block-dep-stat,
  .block-dep-source {
    font-size: 12px;
    color: var(--text-dim);
    white-space: nowrap;
  }
  /* 标签和它自己的下拉是一对，别再被 flex 拉开 */
  .filter-pair {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }
  .filter-row {
    flex-wrap: wrap;
    gap: 10px 16px;
  }
  .filter-row :global(.filter-pair .dropdown),
  .filter-pair :global(.dropdown) {
    width: 150px;
    min-width: 132px;
    max-width: 260px;
  }
  .filter-row :global(.filter-label),
  .filter-pair .filter-label {
    width: auto;
    white-space: nowrap;
  }
  .group-count {
    margin-left: auto;
    margin-right: 8px;
    font-size: 12px;
    color: var(--text-dim);
  }

  .dep-sheet {
    width: min(92vw, 1400px);
    /* 高度跟着内容走：内容少就短，超过视口 84% 才在内部滚动 */
    height: auto;
    max-height: 84vh;
    display: grid;
    grid-template-columns: var(--dep-split, 46%) 6px minmax(0, 1fr);
    /* 不透明底：下面是半透明面板，透过去就会看到两层文字叠在一起 */
    background: var(--panel-solid, #171c21);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.45);
    overflow: hidden;
  }
  .dep-left,
  .dep-right {
    padding: 18px;
    min-width: 0;
  }
  .dep-right {
    overflow-y: auto;
    max-height: calc(84vh - 38px);
  }
  .dep-left {
    display: flex;
    flex-direction: column;
    gap: 12px;
    /* 子元素按栏宽拉伸，长简介才会在栏内换行，而不是横穿到右栏 */
    align-items: stretch;
  }
  .dep-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .dep-back {
    align-self: flex-start;
    padding: 2px 10px;
  }
  .dep-filters {
    margin-top: 2px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding-top: 12px;
    border-top: 1px solid var(--border);
  }
  .dep-filter {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .dep-head {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
  }
  .dep-icon-lg {
    width: 52px;
    height: 52px;
    border-radius: 10px;
    object-fit: cover;
    background: var(--surface-2);
    flex: none;
  }
  .dep-head-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .dep-head-text h3 {
    margin: 0;
    font-size: 17px;
  }
  .dep-from {
    font-size: 12px;
    color: var(--text-dim);
  }
  .dep-desc {
    margin: 0;
    font-size: 13px;
    line-height: 1.6;
    color: var(--text-dim);
    /* 长英文句子要能断行，否则被右边界裁掉 */
    overflow-wrap: anywhere;
    min-width: 0;
  }
  .dep-stats {
    display: flex;
    gap: 14px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .dep-gutter {
    cursor: col-resize;
    background: var(--border);
    transition: background 0.15s;
  }
  .dep-gutter:hover {
    background: color-mix(in srgb, var(--yb-green) 55%, transparent);
  }

  /* 整行可点：给悬停与按下的反馈，别让人感觉按钮被删了却没东西可点 */
  .mr-versions li {
    cursor: pointer;
    transition: background 0.12s ease, transform 0.08s ease;
  }
  .mr-versions li:hover {
    background: var(--surface-2);
  }
  .mr-versions li:active {
    background: color-mix(in srgb, var(--yb-green) 20%, var(--surface-2));
    transform: scale(0.995);
  }
</style>
