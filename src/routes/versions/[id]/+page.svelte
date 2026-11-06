<script lang="ts">
  import Dropdown from "$lib/components/Dropdown.svelte";
  import ConfirmDialog from "$lib/components/ConfirmDialog.svelte";
  import Modal from "$lib/components/Modal.svelte";
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
  import { loaderLabels } from "$lib/loader-ranges";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import type { UnlistenFn } from "@tauri-apps/api/event";
  import { startVersion } from "$lib/launch";
  import { announce } from "$lib/tasks.svelte";
  import { showError } from "$lib/dialogs.svelte";
  import { cachedJava } from "$lib/detect";
  import MinecraftInstall from "$lib/components/MinecraftInstall.svelte";
  import PackExport from "$lib/components/PackExport.svelte";
  import type { InstanceLoader } from "$lib/api";
  import { blockIcons, resolveIconUrl } from "$lib/version-icon";
  import { absorbCategories, isStarred } from "$lib/starred.svelte";
  import grassIcon from "$lib/assets/blocks/Grass.png";
  import redstoneBlockIcon from "$lib/assets/blocks/RedstoneBlock.png";
  import grassPathIcon from "$lib/assets/blocks/GrassPath.png";
  import { beginTask, endTask } from "$lib/tasks.svelte";
  import { saveDownloadsState, type DownloadsState } from "$lib/downloads-state.svelte";
  import {
    api,
    type CategorySnapshot,
    type CrashDiagnosis,
    type EntryInfo,
    type JavaInstallation,
    type ModEntry,
    type ModUpdate,
    type UpdateReport,
    onModUpdatesProgress,
    type ServerEntry,
    type VersionSettings,
    type VersionView
  } from "$lib/api";

  type Tab =
    | "overview"
    | "settings"
    | "modify"
    | "export"
    | "saves"
    | "screenshots"
    | "mods"
    | "resourcepacks"
    | "shaders"
    | "schematics"
    | "servers"
    | "logs"
    | "crashes";

  let version = $state<VersionView | null>(null);
  let javas = $state<JavaInstallation[]>([]);
  let mods = $state<ModEntry[]>([]);
  let gameDir = $state("");
  let tab = $state<Tab>("overview");
  let busy = $state("");
  let dragging = $state(false);
  /// 待确认的删除：版本本身或某个模组，确认框关掉前不动磁盘。
  let pendingDelete = $state<
    { kind: "version" } | { kind: "mod"; mod: ModEntry } | null
  >(null);
  let pendingReset = $state(false);

  /// 实例实际根目录（隔离实例是 versions/<id>），各标签都从这里拼子目录。
  let root = $state("");
  /// 每个列表标签的数据，键是标签名。
  let entries = $state<Record<string, EntryInfo[]>>({});
  let entryBusy = $state("");
  let servers = $state<ServerEntry[]>([]);
  /// 点开的文本文件内容（日志、崩溃报告）。
  let textView = $state<{ title: string; body: string } | null>(null);
  /// 崩溃报告 tab 顶部那份诊断：看的是哪份文件、认出了什么。
  let crashDiag = $state<{ file: string; items: CrashDiagnosis[] } | null>(null);
  let diagBusy = $state(false);
  let shots = $state<Record<string, string>>({});
  let exporting = $state(false);

  /// 模组页：搜索词、筛选、勾选、图标缓存、更新结果。
  let modQuery = $state("");
  let modFilter = $state<"all" | "enabled" | "disabled" | "updatable">("all");
  let selectedMods = $state(new Set<string>());

  /// 批量栏的按钮跟着选中项的实际状态变灰（PCL 也是这个逻辑）。
  const selectedModList = $derived(mods.filter((mod) => selectedMods.has(mod.fileName)));
  const selectedAllEnabled = $derived(
    selectedModList.length > 0 && selectedModList.every((mod) => mod.enabled)
  );
  const selectedAllDisabled = $derived(
    selectedModList.length > 0 && selectedModList.every((mod) => !mod.enabled)
  );
  const selectedAnyUpdatable = $derived(
    selectedModList.some((mod) => modUpdates[mod.fileName]?.canUpdate)
  );
  let modIcons = $state<Record<string, string | null>>({});
  let modUpdates = $state<Record<string, ModUpdate>>({});
  let checkingUpdates = $state(false);
  let updateChecked = $state(false);
  /// 这一轮查的结果：认出来多少、多少个没查通。
  let updateReport = $state<{ identified: number; failed: number; cached: boolean } | null>(null);
  /// 查更新的进度（阶段 + 已完成 / 总数），只在查的时候有值。
  let updateProgress = $state<{ stage: string; done: number; total: number } | null>(null);
  let modDetail = $state<ModEntry | null>(null);
  /// 批量更新前的风险确认。
  let updateConfirm = $state(false);

  /// 自定义图标的图片数据（`icon` 是 custom 时才有）。
  let iconUrl = $state<string | null>(null);
  /// 概览页「个性化」的三个弹窗：改名、描述、加入分类。
  let renameOpen = $state(false);
  let renameDraft = $state("");
  let renameError = $state("");
  let descOpen = $state(false);
  let descDraft = $state("");
  let catOpen = $state(false);
  let catDraft = $state<string[]>([]);
  let catDraftName = $state("");
  let catError = $state("");
  let starred = $state(false);

  /// 设置页四个卡片共用一份表单，点「保存」一起写进 ybmcl.json。
  let form = $state({
    isolated: true,
    javaPath: "",
    javaAuto: false,
    memoryType: 2,
    memoryCustom: 4096,
    memoryInitial: 0,
    windowTitle: "",
    versionType: "",
    jvmArgs: "",
    gameArgs: "",
    classpathHead: "",
    preLaunch: "",
    preLaunchWait: true,
    skipFileCheck: false,
    debugLog4j: false,
    ignoreJavaWarning: false,
    loginRequire: 0,
    serverEntry: ""
  });
  /// 本机物理内存（跟随全局那档不显示具体数，所以允许为空）。
  let memory = $state<{ totalMb: number; availableMb: number } | null>(null);

  /// 这个游戏目录的分类表和本版本的归属（`chosen` 只放真实分类，星标单独存）。
  let cats = $state<CategorySnapshot>({ categories: [], tags: {} });
  let chosen = $state<string[]>([]);

  const id = $derived(decodeURIComponent(page.params.id ?? ""));
  /// 已配置但当前没探测到的 Java 也要能在下拉里显示出来，否则框会是空的。
  const javaOptions = $derived.by(() => {
    const options = javas.map((java) => ({ value: java.path, label: javaLabel(java) }));
    if (form.javaPath && !options.some((option) => option.value === form.javaPath)) {
      options.unshift({ value: form.javaPath, label: `${cleanPath(form.javaPath)}（已配置）` });
    }
    return options;
  });
  /// 「自动配置内存」按模组数量和可用内存现算一遍，给界面显示。
  const autoMemory = $derived.by(() => {
    if (!memory) return null;
    const count = mods.length;
    const optifine = /optifine/i.test(version?.id ?? "");
    const [ramMin, t1, t2, t3] =
      count === 0
        ? optifine
          ? [0.5, 1.5, 3, 5]
          : [0.5, 1.5, 2.5, 4]
        : [0.5 + count / 150, 1.5 + count / 90, 2.7 + count / 50, 4.5 + count / 25];
    const available = memory.availableMb / 1024;
    let ram = Math.min(available, t1);
    ram += Math.min(available * 0.7, t2 - t1);
    ram += Math.min(available * 0.4, t3 - t2);
    ram += Math.min(available * 0.15, t3);
    ram = Math.max(ram, ramMin);
    ram = Math.min(ram, (memory.totalMb / 1024) * 0.9);
    return Math.max(512, Math.round(ram * 1024));
  });
  /// 这次启动实际会分多少内存（跟随全局那档交给后端算，这里不显示）。
  const gameMemory = $derived(
    form.memoryType === 0 ? autoMemory : form.memoryType === 1 ? form.memoryCustom : null
  );
  const customMax = $derived(
    memory ? Math.min(32768, Math.max(1024, Math.round(memory.totalMb / 2 / 256) * 256)) : 32768
  );
  const gameDirectory = $derived(version?.isolated ? `versions/${version.id}` : null);

  /// 卡片和概览页共用：实例目录里的图标（自定义或整合包带来的）优先，
  /// 其次是内置方块图或按状态自动挑的那张。
  const iconSrc = $derived.by(() => {
    if (!version) return grassIcon;
    // 后端返回 `data:` 说明实例目录里真有图标文件（自动或自定义都算）。
    if (iconUrl?.startsWith("data:")) return iconUrl;
    return resolveIconUrl(version.icon, version.channel, version.loaders);
  });

  const iconOptions = $derived([
    { value: "", label: "自动" },
    { value: "custom", label: "自定义..." },
    ...blockIcons.map((block) => ({ value: `builtin:${block.key}`, label: block.label }))
  ]);

  const launchText = $derived(
    !version || version.launches <= 0 ? "从未启动" : `${version.launches} 次`
  );

  const sourceLabel = $derived.by(() => {
    switch (version?.pack?.source) {
      case "modrinth":
        return "Modrinth";
      case "curseforge":
        return "CurseForge";
      case "ftb":
        return "FTB";
      case "zip":
        return "本地整合包";
      default:
        return "来源未知";
    }
  });

  function javaLabel(java: JavaInstallation): string {
    const parts = java.path.replaceAll("\\", "/").split("/").filter(Boolean);
    const vendor = parts.length > 2 ? parts[parts.length - 3] : "";
    return `Java ${java.major} · ${java.version}${vendor ? ` · ${vendor}` : ""}`;
  }

  function formatMb(mb: number): string {
    return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${Math.round(mb)} MB`;
  }

  function cleanPath(path: string): string {
    return path.replace(/^\\\\\?\\/, "");
  }

  /// 包身份卡能不能点：有来源、有项目 id 才算（本地导入的 zip 没有项目，就不做成链接）。
  const packLink = $derived(
    version?.pack &&
      Boolean(version.pack.projectId) &&
      ["modrinth", "curseforge", "ftb"].includes(version.pack.source)
  );

  /// 点包身份卡：把「照这个项目打开详情」写进下载页状态，再切过去。
  /// 下载页的 restoreFromState 会把项目和版本列表一起摆好，返回键回到实例页。
  function openPackInDownloads() {
    const pack = version?.pack;
    if (!pack?.projectId || !packLink) return;
    const source = pack.source as "modrinth" | "curseforge" | "ftb";
    const next: DownloadsState = {
      cat: "packs",
      filters: {
        packs: {
          query: "",
          source,
          sort: "relevance",
          game: "",
          loader: "",
          env: "all",
          tag: "全部",
          page: 1
        }
      },
      query: "",
      source,
      sort: "relevance",
      loader: "",
      env: "all",
      tag: "全部",
      project: {
        id: pack.projectId,
        title: pack.projectName || (version?.id ?? ""),
        slug: "",
        description: pack.summary,
        iconUrl: null,
        downloads: pack.downloads,
        author: "",
        categories: pack.tags,
        projectType: "modpack",
        loaders: [],
        gameVersions: [],
        chineseTitle: null
      },
      instance: null,
      loaderFilter: null,
      expanded: [],
      scrollTop: 0
    };
    saveDownloadsState(next);
    void goto("/downloads");
  }

  async function load() {
    try {
      const [dir, list] = await Promise.all([api.defaultGameDir(), api.listVersions()]);
      gameDir = dir;
      version = list.find((item) => item.id === id) ?? null;
      if (!version) {
        showError("找不到这个版本", `versions\${id} 不在列表里，可能已经被删了。`);
        return;
      }
      form.isolated = version.isolated;
      form.javaPath = version.javaPath;
      form.javaAuto = version.javaAuto;
      form.memoryType = version.memoryType;
      form.memoryCustom = version.memoryCustomMb || version.maxMemoryMb || 4096;
      form.memoryInitial = version.memoryInitialMb;
      form.windowTitle = version.windowTitle;
      form.versionType = version.versionType;
      form.jvmArgs = version.jvmArgs;
      form.gameArgs = version.gameArgs;
      form.classpathHead = version.classpathHead;
      form.preLaunch = version.preLaunch;
      form.preLaunchWait = version.preLaunchWait;
      form.skipFileCheck = version.skipFileCheck;
      form.debugLog4j = version.debugLog4j;
      form.ignoreJavaWarning = version.ignoreJavaWarning;
      form.loginRequire = version.loginRequire;
      form.serverEntry = version.serverEntry;
      chosen = version.categories.filter((cid) => cid !== "starred");
      root = await api.instanceRoot(version.id).catch(() => "");
      starred = isStarred(dir, version.id);
      void api
        .versionIcon(version.id)
        .then((url) => (iconUrl = url))
        .catch(() => (iconUrl = null));
      void api
        .listCategories()
        .then((snapshot) => {
          cats = snapshot;
          absorbCategories(dir, snapshot.tags);
          starred = isStarred(dir, version!.id);
        })
        .catch(() => {});
      void api
        .memoryInfo()
        .then((info) => (memory = info))
        .catch(() => (memory = null));
      void cachedJava().then((cached) => (javas = cached.value)).catch(() => {});
      await loadMods();
    } catch (e) {
      showError("读取版本失败", "版本目录或者模组列表没读出来。", String(e));
    } finally {
    }
  }

  async function loadMods() {
    if (!version) return;
    mods = await api.listMods(gameDir, gameDirectory);
    // 列表变了：把勾选里已经不存在的文件名去掉，再补图标（更新结果保留，用户可能正要更新）。
    const alive = new Set(mods.map((mod) => mod.fileName));
    selectedMods = new Set([...selectedMods].filter((name) => alive.has(name)));
    void ensureModIcons();
    // 照 PCL：打开模组页就自动补项目信息/更新（6 小时内有缓存就不联网）。
    if (!updateChecked && canUseMods && mods.length > 0) void refreshUpdates(false);
    else if (updateChecked) void refreshUpdates(false);
  }

  /// 更新检查用的加载器 id：取实例的第一个加载器。
  const modLoader = $derived(version?.loadersDetail?.[0]?.loader ?? "");
  /// 这个实例能不能装模组：有 Forge/NeoForge/Fabric/Quilt/LiteLoader/Cleanroom 之一才算。
  /// OptiFine 只是补丁，单独存在时原版照样读不了 mod。
  const canUseMods = $derived(
    (version?.loadersDetail ?? []).some(
      (item) => item.loader !== "optifine",
    ),
  );
  /** 头部信息行里的加载器标签：每个加载器一格，带版本号；原版实例显示「原版」。 */
  const headerLoaders = $derived(
    version ? loaderLabels(version.loadersDetail) : [],
  );


  /// 展示名：描述符里写了 displayName / name 就用它，否则把文件名的版本尾巴削掉。
  /// PCL 跳下载页搜索时用的也是显示名，不会把 "-1.0.14-1.21.1-alpha" 一起塞进去。
  function stripModVersion(stem: string): string {
    let name = stem;
    name = name.replace(/[-_ ](?:neoforge|forge|fabric|quilt|liteloader)[-_ ].*$/i, "");
    name = name.replace(/[-_ ]v?\d[\w.+-]*$/i, "");
    return name.trim() || stem;
  }

  function modDisplayName(mod: ModEntry): string {
    const declared = mod.metadata?.name?.trim();
    if (declared) return declared;
    return stripModVersion(mod.modName || mod.fileName.replace(/\.jar(?:\.disabled)?$/i, ""));
  }

  /// 搜索 + 筛选后的列表：启用的排前面，再按名字排。
  const shownMods = $derived.by(() => {
    const needle = modQuery.trim().toLowerCase();
    const hits = mods.filter((mod) => {
      if (modFilter === "enabled" && !mod.enabled) return false;
      if (modFilter === "disabled" && mod.enabled) return false;
      if (modFilter === "updatable" && !modUpdates[mod.fileName]?.canUpdate) return false;
      if (!needle) return true;
      const meta = mod.metadata;
      const haystack = [
        modDisplayName(mod),
        mod.modName,
        mod.fileName,
        meta?.name ?? "",
        meta?.modId ?? "",
        meta?.description ?? "",
        ...(meta?.authors ?? [])
      ]
        .join(" ")
        .toLowerCase();
      return haystack.includes(needle);
    });
    return hits.sort((a, b) => {
      if (a.enabled !== b.enabled) return a.enabled ? -1 : 1;
      return modDisplayName(a).localeCompare(modDisplayName(b), "zh-CN");
    });
  });

  const enabledCount = $derived(mods.filter((mod) => mod.enabled).length);
  const updatableCount = $derived(
    Object.values(modUpdates).filter((item) => item.canUpdate).length
  );
  const allShownSelected = $derived(
    shownMods.length > 0 && shownMods.every((mod) => selectedMods.has(mod.fileName))
  );
  /// 筛选按钮带计数，按条件隐藏（PCL 的做法）：全部永远在。
  const modFilterOptions = $derived.by(() => {
    const options: { id: "all" | "enabled" | "disabled" | "updatable"; label: string; count: number }[] = [
      { id: "all", label: "全部", count: mods.length }
    ];
    if (enabledCount > 0) options.push({ id: "enabled", label: "已启用", count: enabledCount });
    if (mods.length - enabledCount > 0)
      options.push({ id: "disabled", label: "已禁用", count: mods.length - enabledCount });
    if (updatableCount > 0) options.push({ id: "updatable", label: "可更新", count: updatableCount });
    return options;
  });

  function downloadsModUrl(query = "") {
    return query ? `/downloads?mod=${encodeURIComponent(query)}` : "/downloads?mod=";
  }

  /// 右键模组：照 PCL 的「信息」行为。
  /// 认领取到项目 → 打开下载页并直接把项目详情摆出来（PCL 的 ResourceDetail）；
  /// 认不出来 → 和 PCL 一样打开 mcmod.cn 的百科搜索，而不是在站内瞎搜。
  async function openModDownload(mod: ModEntry) {
    const name = modDisplayName(mod);
    let project = "";
    try {
      const found = await api.modProject(gameDir, gameDirectory, mod.fileName);
      if (found) project = `&project=${encodeURIComponent(`${found.source}:${found.projectId}`)}`;
    } catch {
      /* 认领失败走下面的百科搜索 */
    }
    if (!project) {
      await openModWikiByName(name);
      return;
    }
    const mc = version?.gameVersion?.trim();
    const loader = modLoader;
    const extra =
      (mc ? `&mc=${encodeURIComponent(mc)}` : "") + (loader ? `&loader=${encodeURIComponent(loader)}` : "");
    goto(`${downloadsModUrl(name)}${extra}${project}`);
  }

  /// 认不出项目时的去处：PCL 用 `https://www.mcmod.cn/s?key=<显示名>`。
  async function openModWikiByName(name: string) {
    try {
      await api.openUrl(`https://www.mcmod.cn/s?key=${encodeURIComponent(name)}`);
    } catch (e) {
      showError("打开百科失败", "默认浏览器没起来。", String(e));
    }
  }

  function toggleSelect(mod: ModEntry) {
    const next = new Set(selectedMods);
    if (next.has(mod.fileName)) next.delete(mod.fileName);
    else next.add(mod.fileName);
    selectedMods = next;
  }

  function toggleSelectAll() {
    selectedMods = allShownSelected
      ? new Set()
      : new Set(shownMods.map((mod) => mod.fileName));
  }

  /// 逐个补图标：只问一次，拿不到的记成 null，滚动时不重复请求。
  /// 图标分两步：先读 jar 自带的（快，离线也有），再让后端按哈希去
  /// Modrinth / CurseForge 反查项目图标，补上 jar 里没带图的那批。
  async function ensureModIcons() {
    const pending = mods.filter((mod) => !(mod.fileName in modIcons));
    if (pending.length === 0) return;
    for (let index = 0; index < pending.length; index += 8) {
      const batch = pending.slice(index, index + 8);
      await Promise.all(
        batch.map(async (mod) => {
          let icon: string | null = null;
          try {
            // 描述符里没写 icon 的模组也可能有 icon.png / logo.png，
            // 交给后端按常见文件名再找一层，别在元数据这一层就放弃。
            icon = await api.modIcon(gameDir, gameDirectory, mod.fileName);
          } catch {
            icon = null;
          }
          if (!(mod.fileName in modIcons)) {
            modIcons = { ...modIcons, [mod.fileName]: icon };
          }
        }),
      );
    }

    const missing = pending
      .filter((mod) => !modIcons[mod.fileName])
      .map((mod) => mod.fileName);
    if (missing.length === 0) return;
    let urls: Record<string, string> = {};
    try {
      urls = await api.modProjectIcons(gameDir, gameDirectory, missing);
    } catch {
      return;
    }
    const entries = Object.entries(urls);
    for (let index = 0; index < entries.length; index += 4) {
      const batch = entries.slice(index, index + 4);
      await Promise.all(
        batch.map(async ([fileName, url]) => {
          try {
            const icon = await api.fetchIcon(url);
            if (icon) modIcons = { ...modIcons, [fileName]: icon };
          } catch {
            // 拉不到就保持 jar 里的结果（多半是空），下次进页面再试。
          }
        }),
      );
    }
  }

  /// 查更新。打开模组页时自动跑一次（结果缓存 6 小时，命中缓存就不联网）；
  /// `force` 只给手动的「检查更新」用，会忽略缓存整份重查。
  async function refreshUpdates(notify: boolean, force = false) {
    if (!version || checkingUpdates) return;
    checkingUpdates = true;
    updateChecked = true;
    updateProgress = null;
    try {
      const report: UpdateReport = await api.checkModUpdates(
        gameDir,
        gameDirectory,
        version.gameVersion ?? version.id,
        modLoader,
        force
      );
      const map: Record<string, ModUpdate> = {};
      for (const item of report.updates) {
        if (item.canUpdate) map[item.fileName] = item;
      }
      modUpdates = map;
      updateReport = {
        identified: report.identified,
        failed: report.failed,
        cached: report.cached
      };
      if (notify) {
        const count = Object.keys(map).length;
        if (count > 0) announce(`有 ${count} 个模组可以更新`);
        else if (report.failed > 0) announce(`没查到更新的（${report.failed} 个没查通）`);
        else announce("模组都是最新的");
      }
    } catch (e) {
      // 用户点了取消：保留上一次的结果，不当成错误。
      const message = String(e);
      if (message.includes("已取消")) {
        if (notify) announce("已取消检查更新");
      } else {
        updateReport = null;
        if (notify) showError("检查更新失败", "两个来源都没查通，过会儿再试。", message);
      }
    } finally {
      checkingUpdates = false;
      updateProgress = null;
    }
  }

  /// 取消正在跑的更新检查：后端在阶段之间、每读几个 jar 时看这个标志。
  async function cancelUpdateCheck() {
    try {
      await api.cancelModUpdates();
    } catch {
      // 取消失败也没关系，检查跑完会自己停。
    }
  }

  /// 行内标签：Modrinth 显示「旧版本 → 新版本」，CurseForge 只有文件名，就只写新文件名。
  function updateLabel(update: ModUpdate): string {
    const latest = update.latestVersion.replace(/\.jar(?:\.disabled)?$/i, "");
    // 后端只记「目标文件」，旧版本号优先用本地 jar 描述符里的 version，
    // 拿不到再退回文件名（CurseForge 本来就只给文件名）。
    let current = update.currentVersion;
    if (/\.jar(?:\.disabled)?$/i.test(current)) {
      const mod = mods.find((item) => item.fileName === update.fileName);
      current = mod?.metadata?.version?.trim() ?? "";
    }
    if (current && current !== latest) return `${current} → ${latest}`;
    return latest;
  }

  /// 阶段名翻译成一句话；进度条上的文字用它。
  function stageText(stage: string): string {
    if (stage === "hash") return "正在读模组文件";
    if (stage === "modrinth") return "正在问 Modrinth";
    if (stage === "curseforge") return "正在问 CurseForge";
    return "正在检查更新";
  }

  /// 从磁盘选 .jar 装进 mods。
  async function installModFiles() {
    if (!version) return;
    try {
      const picked = await openFileDialog({
        multiple: true,
        filters: [{ name: "Mod", extensions: ["jar"] }]
      });
      if (!picked) return;
      const paths = Array.isArray(picked) ? picked : [picked];
      await importPaths(paths);
    } catch (e) {
      showError("选择文件失败", "系统文件对话框没打开。", String(e));
    }
  }

  async function openModWiki() {
    const name = modDetail ? modDisplayName(modDetail) : "";
    if (!name) return;
    await openModWikiByName(name);
  }

  async function revealMod(mod: ModEntry) {
    try {
      await api.revealPath(`${root}${SEP}mods${SEP}${mod.fileName}`);
    } catch (e) {
      showError("打开位置失败", "这个文件可能已经被移走了。", String(e));
    }
  }

  /// 批量改启用状态：只处理状态需要变化的文件。
  async function batchSetEnabled(enabled: boolean) {
    if (!version || selectedMods.size === 0) return;
    busy = "mods";
    try {
      for (const name of selectedMods) {
        const mod = mods.find((item) => item.fileName === name);
        if (!mod || mod.enabled === enabled) continue;
        await api.setModEnabled(gameDir, gameDirectory, name, enabled);
      }
      await loadMods();
      announce(enabled ? "已启用选中的模组" : "已禁用选中的模组");
    } catch (e) {
      showError("改模组状态失败", "文件改不了名，可能被占用。", String(e));
    } finally {
      busy = "";
    }
  }

  async function batchDelete() {
    if (!version || selectedMods.size === 0) return;
    busy = "mods";
    try {
      for (const name of selectedMods) {
        await api.removeMod(gameDir, gameDirectory, name, false);
      }
      selectedMods = new Set();
      await loadMods();
      announce("模组已删除");
    } catch (e) {
      showError("删除模组失败", "文件删不掉，可能被游戏占用。", String(e));
    } finally {
      busy = "";
    }
  }

  /// 选中的模组里有能更新的才弹确认框。
  function askUpdate() {
    const targets = [...selectedMods].filter((name) => modUpdates[name]);
    if (targets.length === 0) {
      announce("选中的模组没有新版本");
      return;
    }
    updateConfirm = true;
  }

  /// 更新选中的模组：先警告再替换，一次一个，失败就停。
  async function batchUpdate() {
    if (!version || selectedMods.size === 0) return;
    const targets = [...selectedMods]
      .map((name) => modUpdates[name])
      .filter((item): item is ModUpdate => Boolean(item));
    if (targets.length === 0) {
      announce("选中的模组没有新版本");
      return;
    }
    busy = "mods";
    let done = 0;
    try {
      for (const update of targets) {
        await api.applyModUpdate(gameDir, gameDirectory, update);
        done += 1;
      }
      selectedMods = new Set();
      await loadMods();
      announce(`已更新 ${done} 个模组`);
    } catch (e) {
      showError(
        "更新模组失败",
        done > 0 ? `已更新 ${done} 个，剩下的没换成功。` : "新版本没下载下来，旧文件还在。",
        String(e)
      );
      await loadMods();
    } finally {
      busy = "";
    }
  }

  /// 空着时框里显示这个（跟游戏里真用的一样），实际存的是空字符串。
  const defaultTitle = $derived(`Minecraft ${version?.gameVersion ?? version?.id ?? ""}`);
  const defaultInfo = $derived(version?.channel ?? "release");

  /// 两个说明浮层：框的左上角就贴在鼠标上。
  const TITLE_TIP =
    "自定义游戏窗口的标题，若留空则跟随全局设置的值。\n" +
    "支持以下替换标记：\n" +
    "· {user}：玩家名字\n" +
    "· {login}：玩家的登录方式\n" +
    "· {name}：游戏实例名\n" +
    "· {date}、{time}：当前的系统时间\n" +
    "· {version}：游戏对应的原版版本号\n" +
    "注意：如果你的实例安装了 Modern UI 模组，那么这里的设置将会失效";
  const INFO_TIP =
    "该信息会显示在游戏主界面的左下角，与 F3 调试页面的左上角。\n若留空，则跟随全局设置的值。";
  let tip = $state<{ text: string; x: number; y: number } | null>(null);

  function tipAt(event: MouseEvent): { x: number; y: number } {
    return {
      x: Math.max(8, Math.min(event.clientX, innerWidth - 360)),
      y: Math.max(8, Math.min(event.clientY, innerHeight - 170))
    };
  }

  function showTip(event: MouseEvent, text: string) {
    tip = { text, ...tipAt(event) };
  }

  function moveTip(event: MouseEvent) {
    if (!tip) return;
    tip = { text: tip.text, ...tipAt(event) };
  }

  function hideTip() {
    tip = null;
  }

  /// 浮层要挂到 body 上：卡片那层有 backdrop-filter，会把 fixed 关在卡片里。
  function float(ref: HTMLElement) {
    document.body.appendChild(ref);
    return {
      destroy() {
        ref.remove();
      }
    };
  }

  /// 下拉里那个 Java 只露尾段：`jdk-21.0.12.1in\java.exe`。
  function javaHint(path: string): string {
    // 探测出来的路径里分隔符可能混着 `/`，统一以后再取最后三段。
    const parts = cleanPath(path).replaceAll("\\", "/").split("/").filter(Boolean);
    return parts.length >= 3 ? parts.slice(-3).join("\\") : cleanPath(path);
  }

  function settingsFromForm(): VersionSettings | null {
    if (!version) return null;
    const custom = Math.max(512, Math.min(32768, Math.round(form.memoryCustom)));
    const initial = Math.max(0, Math.min(32768, Math.round(form.memoryInitial)));
    return {
      isolated: form.isolated,
      javaPath: form.javaPath,
      javaAuto: form.javaAuto,
      memoryType: form.memoryType,
      memoryCustomMb: form.memoryType === 1 ? custom : 0,
      memoryInitialMb: form.memoryType === 1 ? initial : 0,
      maxMemoryMb: form.memoryType === 1 ? custom : 0,
      icon: version.icon,
      description: version.description,
      launches: version.launches,
      pack: version.pack,
      windowTitle: form.windowTitle.trim(),
      versionType: form.versionType.trim(),
      jvmArgs: form.jvmArgs,
      gameArgs: form.gameArgs,
      classpathHead: form.classpathHead,
      preLaunch: form.preLaunch,
      preLaunchWait: form.preLaunchWait,
      skipFileCheck: form.skipFileCheck,
      debugLog4j: form.debugLog4j,
      ignoreJavaWarning: form.ignoreJavaWarning,
      loginRequire: form.loginRequire,
      serverEntry: form.serverEntry.trim()
    };
  }

  /// 版本设置没有保存按钮：改一下就写进 ybmcl.json（和 PCL 一样）。
  let writing = false;
  let pendingWrite = false;
  let writeTimer: ReturnType<typeof setTimeout> | null = null;

  async function flush() {
    if (!version) return;
    if (writeTimer !== null) {
      clearTimeout(writeTimer);
      writeTimer = null;
    }
    if (writing) {
      pendingWrite = true;
      return;
    }
    writing = true;
    const wasIsolated = version.isolated;
    try {
      do {
        pendingWrite = false;
        const settings = settingsFromForm();
        if (!settings) return;
        await api.saveVersionSettings(version.id, settings);
        version = { ...version, ...settings };
      } while (pendingWrite);
      if (version.isolated !== wasIsolated) await loadMods();
    } catch (e) {
      showError("保存版本设置失败", "这次改动没有写进 ybmcl.json。", String(e));
      await load();
    } finally {
      writing = false;
    }
  }

  /// 文本框边打边存：停手 250ms 落盘，失焦立刻落盘。
  function scheduleSave() {
    pendingWrite = true;
    if (writeTimer !== null) clearTimeout(writeTimer);
    writeTimer = setTimeout(() => {
      writeTimer = null;
      void flush();
    }, 250);
  }

  async function createDraftCategory() {
    const name = catDraftName.trim();
    if (!name) return;
    catError = "";
    busy = "cats";
    try {
      const before = new Set(cats.categories.map((item) => item.id));
      const snapshot = await api.createCategory(name);
      cats = snapshot;
      catDraftName = "";
      const created = snapshot.categories.find((item) => !before.has(item.id));
      if (created) catDraft = [...catDraft, created.id];
    } catch (e) {
      catError = String(e);
    } finally {
      busy = "";
    }
  }

  /// 打开加入分类弹窗：草稿里先放当前归属（星标也在里面，勾上就是打星）。
  function openCategories() {
    catDraft = [...chosen, ...(starred ? ["starred"] : [])];
    catDraftName = "";
    catError = "";
    catOpen = true;
  }

  async function saveCategories() {
    if (!version) return;
    const known = new Set(cats.categories.map((item) => item.id));
    const ids = catDraft.filter((cid) => known.has(cid));
    busy = "cats";
    try {
      const snapshot = await api.setVersionCategories(version.id, ids);
      cats = snapshot;
      absorbCategories(gameDir, snapshot.tags);
      chosen = ids.filter((cid) => cid !== "starred");
      starred = ids.includes("starred");
      version = { ...version, categories: ids };
      catOpen = false;
    } catch (e) {
      catError = String(e);
    } finally {
      busy = "";
    }
  }

  /// 打开改名弹窗：默认填当前名字。
  function openRename() {
    if (!version) return;
    renameDraft = version.id;
    renameError = "";
    renameOpen = true;
  }

  /// Windows 不允许出现在文件夹名里的字符，先在前端挡掉，别等后端报错。
  function renameProblem(value: string): string {
    if (!value) return "名字不能为空。";
    if (/[\\/:*?"<>|]/.test(value)) return "名字里不能有 \\ / : * ? \" < > | 这些字符。";
    if (value.startsWith(".")) return "名字不能以点开头。";
    if (value.length > 80) return "名字太长了。";
    return "";
  }

  async function commitRename() {
    if (!version) return;
    const next = renameDraft.trim();
    if (next === version.id) {
      renameOpen = false;
      return;
    }
    const problem = renameProblem(next);
    if (problem) {
      renameError = problem;
      return;
    }
    busy = "rename";
    renameError = "";
    try {
      const name = await api.renameInstance(version.id, next);
      renameOpen = false;
      announce(`已改名为 ${name}`);
      await goto(`/versions/${encodeURIComponent(name)}`, { invalidateAll: true });
    } catch (e) {
      renameError = String(e);
    } finally {
      busy = "";
    }
  }

  function openDescription() {
    if (!version) return;
    descDraft = version.description ?? "";
    descOpen = true;
  }

  async function commitDescription() {
    if (!version) return;
    busy = "desc";
    try {
      const updated = await api.setVersionDescription(version.id, descDraft.trim());
      version = { ...version, description: updated.description };
      descOpen = false;
      announce("描述已保存");
    } catch (e) {
      showError("保存描述失败", "描述没写进实例的 ybmcl.json。", String(e));
    } finally {
      busy = "";
    }
  }

  async function launch() {
    if (!version) return;
    busy = "launch";
    try {
      await startVersion(version);
    } catch (e) {
      showError("启动失败", "游戏没有起来，看日志里的最后几行。", String(e), "game");
    } finally {
      busy = "";
    }
  }

  async function refreshIcon() {
    if (!version) return;
    iconUrl = await api.versionIcon(version.id).catch(() => null);
  }

  async function chooseIcon(value: string) {
    if (!version) return;
    if (value === "custom") {
      busy = "icon";
      try {
        const updated = await api.pickVersionIcon(version.id);
        if (!updated) return;
        version = { ...version, icon: updated.icon };
        await refreshIcon();
        announce("图标已换好");
      } catch (e) {
        showError("换图标失败", "选好的图片没能拷进实例目录。", String(e));
      } finally {
        busy = "";
      }
      return;
    }
    busy = "icon";
    try {
      await api.setVersionIcon(version.id, value);
      version = { ...version, icon: value };
      await refreshIcon();
    } catch (e) {
      showError("换图标失败", "图标设置没写进实例。", String(e));
    } finally {
      busy = "";
    }
  }

  async function exportScript() {
    if (!version) return;
    busy = "script";
    try {
      const path = await api.exportLaunchScript(version.id);
      announce(`启动脚本已存到 ${path}`);
      await api.revealPath(path).catch(() => {});
    } catch (e) {
      showError("导出启动脚本失败", "拿不到这个实例的启动命令。", String(e));
    } finally {
      busy = "";
    }
  }

  /// 测试游戏：PCL CE 里就是把实例正常启动一次、并把实时日志摊出来看，
  /// 不另做检查（`McLaunchOptions.IsTest` 只影响「输出实时日志」）。
  async function testGame() {
    if (!version) return;
    busy = "test";
    try {
      await startVersion(version);
    } catch (e) {
      showError("测试游戏失败", "游戏没有起来，看日志里的最后几行。", String(e), "game");
    } finally {
      busy = "";
    }
  }

  async function repairFiles() {
    if (!version) return;
    busy = "repair";
    announce("正在校验基础版本的客户端、库与资源…");
    try {
      await api.repairInstanceFiles(version.id);
      announce("文件补全完成");
    } catch (e) {
      showError("补全文件失败", "下载或校验中断了，检查网络后重试。", String(e));
    } finally {
      busy = "";
    }
  }

  /// 修补核心：照 PCL CE 的 `GameCore.AddToCore`——选一个 jar/zip，
  /// 把里面的文件合并进实例的客户端核心（同名覆盖）。
  async function patchCore() {
    if (!version) return;
    busy = "repair";
    try {
      const written = await api.patchInstanceCore(version.id);
      if (written === 0) return;
      announce(`核心已修补（合并 ${written} 个文件）`);
    } catch (e) {
      showError("修补核心失败", "这个 jar 没能合并进客户端核心。", String(e));
    } finally {
      busy = "";
    }
  }


/// 版本列表里的加载器显示名（`Forge` / `NeoForge` / `Fabric` …）换回安装参数用的键。
function loaderKey(name: string): InstanceLoader["loader"] {
  const key = name.toLowerCase().replace(/[^a-z]/g, "");
  if (key === "neoforge") return "neoforge";
  if (key === "fabric") return "fabric";
  if (key === "quilt") return "quilt";
  if (key === "forge") return "forge";
  if (key === "cleanroom") return "cleanroom";
  if (key === "liteloader") return "liteloader";
  if (key === "optifine") return "optifine";
  return "fabric";
}

  /// 重置：照 PCL CE——先备份 `<实例>.json` / `<实例>.jar`，再从互联网把那套东西装回来，
  /// 存档、模组、资源包不动。加载器版本记在实例 json 的 `_ybmcl` 里；
  /// 早期装的实例没有这段记录，就只重下有记录的部分（原版核心与库），不动加载器。
  function startReset() {
    pendingReset = true;
  }

  async function resetInstance() {
    if (!version) return;
    const target = version;
    busy = "reset";
    try {
      const backup = await api.backupInstanceCore(target.id);
      if (target.loadersDetail.length > 0 || target.loaders.length === 0) {
        // 老实例没记过加载器版本时 loadersDetail 是空的；游戏版本也可能没认出来，
        // 那就退回用实例 id（无加载器的实例，id 就是原版版本号）。
        const gameVersion = target.gameVersion?.trim() ? target.gameVersion : target.id;
        announce(`核心已备份到 ${backup}，正在重装…`);
        await api.installGameInstance({
          gameDir,
          gameVersion,
          instanceName: target.id,
          loaders: target.loadersDetail,
          overwrite: true
        });
        announce("实例已重置");
      } else {
        announce(`核心已备份到 ${backup}；这个实例没记加载器版本，只补下原版核心与库`);
        await api.repairInstanceFiles(target.id);
        announce("原版核心与库已补全，加载器没动");
      }
      const all = await api.listVersions().catch(() => null);
      const fresh = all?.find((item) => item.id === id);
      if (fresh) version = fresh;
    } catch (e) {
      showError("重置失败", "重装没走完，核心备份还在 versions\\install-backups 里。", String(e));
    } finally {
      busy = "";
    }
  }

  async function removeVersion() {
    const target = pendingDelete;
    pendingDelete = null;
    if (!target || target.kind !== "version" || !version) return;
    busy = "delete";
    try {
      await api.deleteVersion(version.id);
      announce("版本已删除");
      await goto("/versions");
    } catch (e) {
      showError("删除版本失败", "目录可能正被占用（游戏还开着？）。", String(e));
    } finally {
      busy = "";
    }
  }

  /// 路径分隔符：写成字符码，避免在模板字符串里处理反斜杠转义。
  const SEP = String.fromCharCode(92);

  /// 详情页的十三个标签，顺序照用户给的。
  const EMPTY_TEXT: Partial<Record<Tab, string>> = {
    saves: "还没有存档。",
    resourcepacks: "还没有资源包。",
    shaders: "还没有光影包。",
    schematics: "还没有投影。",
    mods: "还没有模组。"
  };

  function sizeText(bytes: number): string {
    if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
    if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
    return `${bytes} B`;
  }

  function formatCount(n: number): string {
    if (!n) return "0";
    if (n >= 100000000) return `${(n / 100000000).toFixed(1)} 亿`;
    if (n >= 10000) return `${Math.round(n / 10000)} 万`;
    return String(n);
  }

  const TABS: { id: Tab; label: string }[] = [
    { id: "overview", label: "概览" },
    { id: "settings", label: "设置" },
    { id: "modify", label: "修改" },
    { id: "export", label: "导出" },
    { id: "mods", label: "模组" },
    { id: "resourcepacks", label: "资源包" },
    { id: "shaders", label: "光影包" },
    { id: "saves", label: "存档" },
    { id: "screenshots", label: "截图" },
    { id: "schematics", label: "投影" },
    { id: "servers", label: "服务器" },
    { id: "logs", label: "日志" },
    { id: "crashes", label: "崩溃报告" }
  ];

  /// 各标签对应的子目录名（和 PCL 一致）。
  const SUB_DIRS: Record<string, string> = {
    saves: "saves",
    screenshots: "screenshots",
    resourcepacks: "resourcepacks",
    shaders: "shaderpacks",
    schematics: "schematics",
    logs: "logs",
    crashes: "crash-reports"
  };

  function subPath(kind: string): string {
    const sub = SUB_DIRS[kind];
    if (!root) return "";
    return sub ? root + SEP + sub : root;
  }

  /// 点标签时按需读目录，没数据才读。
  async function loadEntries(kind: string) {
    const path = subPath(kind);
    if (!path) return;
    entryBusy = kind;
    try {
      const list = await api.listEntries(path);
      entries = { ...entries, [kind]: list };
      if (kind === "screenshots") await loadShots(list);
      if (kind === "servers") await loadServers();
    } catch (error) {
      entries = { ...entries, [kind]: [] };
      showError("读取目录失败", path, String(error));
    } finally {
      entryBusy = "";
    }
  }

  async function loadShots(list: EntryInfo[]) {
    const next: Record<string, string> = {};
    await Promise.all(
      list
        .filter((item) => !item.dir)
        .slice(0, 60)
        .map(async (item) => {
          const url = await api.imageDataUrl(subPath("screenshots") + SEP + item.name).catch(() => "");
          if (url) next[item.name] = url;
        })
    );
    shots = next;
  }

  async function loadServers() {
    if (!root) return;
    servers = await api.readServers(root + SEP + "servers.dat").catch(() => []);
  }

  async function openText(name: string) {
    const path = subPath(tab) + SEP + name;
    try {
      const body = await api.readTextFile(path);
      textView = { title: name, body };
      if (tab === "crashes") void diagnoseCrash(name);
    } catch (error) {
      showError("打开文件失败", path, String(error));
    }
  }

  /// 打开某个子目录（不存在的时候就先建出来）。
  async function openSub(kind: string) {
    const path = subPath(kind);
    if (!path) return;
    try {
      await api.ensureDir(path);
      await api.revealPath(path);
    } catch (error) {
      showError("打开目录失败", path, String(error));
    }
  }

  async function switchTab(next: Tab) {
    tab = next;
    textView = null;
    if (next === "servers") await loadServers();
    if (next in SUB_DIRS) await loadEntries(next);
    if (next === "crashes") await diagnoseNewest();
  }

  /// 崩溃报告目录里最新的一份，装完崩溃后点进来就能看到结论。
  async function diagnoseNewest() {
    const list = (entries.crashes ?? []).filter((item) => !item.dir);
    if (list.length === 0) {
      crashDiag = null;
      return;
    }
    const newest = list.reduce((a, b) => (b.modifiedMs > a.modifiedMs ? b : a));
    await diagnoseCrash(newest.name);
  }

  /// 让后端看一份报告。认不出原因就只显示原文，不硬编一个结论。
  async function diagnoseCrash(name: string) {
    const dir = subPath("crashes");
    if (!dir) return;
    diagBusy = true;
    crashDiag = { file: name, items: [] };
    try {
      crashDiag = { file: name, items: await api.diagnoseCrashReport(dir, name) };
    } catch (error) {
      crashDiag = null;
      showError("诊断失败", name, String(error));
    } finally {
      diagBusy = false;
    }
  }

  async function exportZip() {
    if (!version || exporting) return;
    exporting = true;
    const task = beginTask("导出", version.id);
    try {
      const saved = await api.exportInstanceZip(version.id, task);
      if (saved) {
        endTask(task, `已导出 ${saved.split(SEP).pop() ?? saved}`);
        announce(`已导出 ${saved.split(SEP).pop() ?? saved}`);
      } else {
        endTask(task, "已取消导出", true);
      }
    } catch (error) {
      endTask(task, "导出没有完成", true);
      showError("导出失败", "实例目录可能被占用，换个位置再试。", String(error));
    } finally {
      exporting = false;
    }
  }

  function openFolder(which: "root" | "mods" | "saves") {
    if (version) void api.openVersionFolder(version.id, which);
  }

  async function toggle(mod: ModEntry) {
    if (!version) return;
    busy = mod.fileName;
    try {
      await api.setModEnabled(gameDir, gameDirectory, mod.fileName, !mod.enabled);
      await loadMods();
    } catch (e) {
      showError("切换模组失败", "文件改不了名，可能被占用。", String(e));
    } finally {
      busy = "";
    }
  }

  /// 删除单个模组：默认进回收站，按住 Shift 确认才是永久删除。
  async function removeMod(mod: ModEntry, permanent: boolean) {
    pendingDelete = null;
    if (!version) return;
    busy = mod.fileName;
    try {
      await api.removeMod(gameDir, gameDirectory, mod.fileName, permanent);
      await loadMods();
      announce(permanent ? "模组已永久删除" : "模组已删除，可在回收站找回");
    } catch (e) {
      showError("删除模组失败", "文件删不掉，可能被游戏占用。", String(e));
    } finally {
      busy = "";
    }
  }

  async function importPaths(paths: string[]) {
    if (!version || paths.length === 0) return;
    busy = "import";
    try {
      const outcomes = await api.addMods(gameDir, gameDirectory, paths, false);
      const imported = outcomes.filter((item) => item.status === "imported").length;
      announce(`导入 ${imported} 个模组${outcomes.length - imported ? `，${outcomes.length - imported} 个被跳过` : ""}`);
      await loadMods();
    } catch (e) {
      showError("导入模组失败", "拖进来的文件没有装进 mods。", String(e));
    } finally {
      busy = "";
    }
  }

  onMount(() => {
    void load();
    let unlisten: UnlistenFn | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter") dragging = true;
        if (event.payload.type === "leave") dragging = false;
        if (event.payload.type === "drop") {
          dragging = false;
          void importPaths(event.payload.paths.filter((path) => path.endsWith(".jar")));
        }
      })
      .then((fn) => (unlisten = fn))
      .catch(() => {});
    let unlistenUpdate: UnlistenFn | undefined;
    void onModUpdatesProgress((progress) => {
      updateProgress = progress;
    })
      .then((fn) => (unlistenUpdate = fn))
      .catch(() => {});
    return () => {
      unlisten?.();
      unlistenUpdate?.();
    };
  });
</script>

<div class="route">
<header class="head">
  <button class="btn ghost" onclick={() => goto("/versions")}>← 返回</button>
  <h1>{version?.id ?? id}<span class="kind">{version?.kind ?? ""}</span></h1>
  <span class="spacer"></span>
  <button class="btn primary" disabled={!version || busy !== ""} onclick={launch}>
    {busy === "launch" ? "启动中…" : "启动"}
  </button>
</header>

{#if version}
  <nav class="tabs">
    {#each TABS as item (item.id)}
      <button class="tab" class:active={tab === item.id} onclick={() => void switchTab(item.id)}>
        {item.label}
      </button>
    {/each}
  </nav>

  {#if tab === "overview"}
    {#if version.pack}
      <section class="panel strip">
        <img class="strip-icon" src={iconSrc} alt="" />
        <div class="strip-main">
          <strong>{version.pack.projectName || version.id}</strong>
          <p class="dim" title={version.pack.summary}>{version.pack.summary || "—"}</p>
        </div>
      </section>
    {/if}

    <section class="panel" class:compact={!version.pack}>
      {#snippet headBody(v: VersionView)}
        <img class="big-icon" src={iconSrc} alt="" />
        <div class="head-main">
          <div class="head-title">
            <strong>{v.id}</strong>
            {#if v.pack && v.pack.tags.length > 0}
              <div class="tags">
                {#each v.pack.tags as tag (tag)}<span class="tag">{tag}</span>{/each}
              </div>
            {/if}
          </div>
          {#if v.description}
            <p class="dim desc">{v.description}</p>
          {:else if v.pack && v.pack.summary}
            <p class="dim desc">{v.pack.summary}</p>
          {/if}
          {#if v.pack}
            <p class="dim meta">
              <span>↓ {formatCount(v.pack.downloads)}</span>
              <span>·</span>
              <span>{sourceLabel}</span>
            </p>
          {/if}
        </div>
      {/snippet}

      {#if packLink}
        <button
          type="button"
          class="head linked"
          title="在下载页打开这个整合包"
          onclick={openPackInDownloads}
        >
          {@render headBody(version)}
        </button>
      {:else}
        <div class="head">
          {@render headBody(version)}
        </div>
      {/if}

      <div class="stats">
        <div class="stat">
          <img src={redstoneBlockIcon} alt="" />
          <div><span>启动次数</span><strong>{launchText}</strong></div>
        </div>
        {#if version.pack}
          <div class="stat">
            <img src={grassPathIcon} alt="" />
            <div><span>整合包版本</span><strong title={version.pack.versionLabel}>{version.pack.versionLabel || "—"}</strong></div>
          </div>
        {/if}
        <div class="stat">
          <img src={grassIcon} alt="" />
          <div><span>Minecraft</span><strong>{version.gameVersion || "未知"}</strong></div>
        </div>
        {#each headerLoaders as label}
          <div class="stat">
            <span class="loader-tag">{label}</span>
          </div>
        {/each}
      </div>
    </section>

    <section class="panel">
      <h2>个性化</h2>
      <div class="opt">
        <span class="opt-label">图标</span>
        <div class="opt-control">
          <Dropdown
            options={iconOptions}
            value={version.icon === "custom" ? "custom" : version.icon}
            onchange={(value) => void chooseIcon(value)}
          />
        </div>
      </div>
      <div class="opt">
        <span class="opt-label">分类</span>
        <div class="chips">
          {#each cats.categories as category (category.id)}
            {#if category.id !== "starred" && chosen.includes(category.id)}
              <span class="chip on">{category.name}</span>
            {/if}
          {/each}
          {#if chosen.length === 0}<span class="dim">还没有分类</span>{/if}
        </div>
      </div>
      <div class="row">
        <button class="btn" disabled={busy !== ""} onclick={openRename}>修改实例名</button>
        <button class="btn" disabled={busy !== ""} onclick={openDescription}>修改实例描述</button>
        <button class="btn" disabled={busy !== ""} onclick={openCategories}>加入分类</button>
      </div>
    </section>

    <section class="panel">
      <h2>快捷方式</h2>
      <div class="row">
        <button class="btn" onclick={() => openFolder("root")}>实例文件夹</button>
        <button class="btn" onclick={() => openFolder("saves")}>存档文件夹</button>
        <button class="btn" onclick={() => openFolder("mods")}>模组文件夹</button>
      </div>
    </section>

    <section class="panel">
      <h2>高级管理</h2>
      <div class="row">
        <button class="btn" disabled={busy !== ""} onclick={() => void exportScript()}>导出启动脚本</button>
        <button class="btn" disabled={busy !== ""} onclick={() => void testGame()}>测试游戏</button>
        <button class="btn" disabled={busy !== ""} onclick={() => void repairFiles()}>补全文件</button>
        <button class="btn" disabled={busy !== ""} onclick={() => startReset()}>重置</button>
        <button class="btn" disabled={busy !== ""} onclick={() => void patchCore()}>修补核心</button>
        <button
          class="btn danger"
          disabled={busy !== ""}
          onclick={() => (pendingDelete = { kind: "version" })}>删除实例</button
        >
      </div>
    </section>
  {:else if tab === "mods"}
    <section class="mods-pane">
      {#if !canUseMods}
        <div class="no-loader">
          <h3>该版本不可使用 Mod</h3>
          <p>你需要先安装 Forge、Fabric 等 Mod 加载器才能使用 Mod，请在下载页面安装这些版本。</p>
          <p>
            如果你已经安装过了 Mod 加载器，那么你很可能会选择了错误的版本，请点击版本选择按钮切换版本。
          </p>
          <div class="no-loader-actions">
            <button class="btn" onclick={() => goto("/downloads")}>转到下载页面</button>
            <button class="btn ghost" onclick={() => goto("/versions")}>版本选择</button>
          </div>
        </div>
      {:else if mods.length === 0}
        <div class="empty-card">
          <h3>还没有模组</h3>
          <p class="dim">把 .jar 拖进这个窗口就能装。模组要和版本、加载器匹配，否则游戏起不来。</p>
          <div class="empty-actions">
            <button class="btn" onclick={() => goto(downloadsModUrl())}>下载 Mod</button>
            <button class="btn" onclick={() => void installModFiles()}>从文件安装</button>
            <button class="btn ghost" onclick={() => openFolder("mods")}>打开文件夹</button>
          </div>
        </div>
      {:else}
        <div class="mods-head">
          <input class="mods-search" placeholder="搜索名称、描述或标签" bind:value={modQuery} />
          <div class="mods-tools">
            <button class="btn" onclick={() => openFolder("mods")}>打开文件夹</button>
            <button class="btn" disabled={busy !== ""} onclick={() => void installModFiles()}>从文件安装</button>
            <button class="btn" onclick={() => goto(downloadsModUrl())}>下载 Mod</button>
            <button class="btn" onclick={toggleSelectAll}>{allShownSelected ? "取消全选" : "全选"}</button>
          </div>
        </div>
        <div class="mods-filters">
          {#each modFilterOptions as option (option.id)}
            <button
              class="chip"
              class:active={modFilter === option.id}
              onclick={() => (modFilter = option.id)}
            >{option.label}（{option.count}）</button>
          {/each}
          <span class="grow"></span>
          {#if checkingUpdates}
            <span class="dim">
              {stageText(updateProgress?.stage ?? "")}{#if updateProgress && updateProgress.total > 0}
                {updateProgress.done}/{updateProgress.total}{/if}
            </span>
            <button class="btn ghost" onclick={() => void cancelUpdateCheck()}>取消</button>
          {:else}
            <button class="btn ghost" onclick={() => void refreshUpdates(true, true)}>检查更新</button>
          {/if}
        </div>
        <p class="dim">
          共 {mods.length} 个模组{#if modQuery.trim()}，匹配 {shownMods.length} 个{/if}{#if updatableCount > 0}，其中 {updatableCount} 个可更新{/if}{#if updateReport && updateReport.failed > 0}，{updateReport.failed} 个这次没查通{/if}
        </p>
        <div class="mod-list">
          {#each shownMods as mod (mod.fileName)}
            <div
              class="mod-row"
              class:disabled={!mod.enabled}
              class:picked={selectedMods.has(mod.fileName)}
              role="presentation"
              title="右键打开这个模组的下载页"
              onclick={() => toggleSelect(mod)}
              oncontextmenu={(event) => {
                event.preventDefault();
                void openModDownload(mod);
              }}
              onkeydown={() => {}}
            >
              <input
                type="checkbox"
                checked={selectedMods.has(mod.fileName)}
                onclick={(event) => event.stopPropagation()}
                onchange={() => toggleSelect(mod)}
              />
              <span class="mod-icon">
                {#if modIcons[mod.fileName]}
                  <img src={modIcons[mod.fileName]} alt="" />
                {:else}
                  <span class="mod-icon-fallback" aria-hidden="true">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
                      <path d="M12 3 20 7.3v9.4L12 21 4 16.7V7.3Z" />
                      <path d="M12 12.2 20 7.3M12 12.2 4 7.3M12 12.2V21" />
                    </svg>
                  </span>
                {/if}
              </span>
              <div class="mod-name">
                <strong title={mod.fileName}>{modDisplayName(mod)}</strong>
                <small>
                  {mod.metadata?.version ?? "—"} · {mod.metadata?.loader ?? "未知加载器"}
                </small>
              </div>
              {#if modUpdates[mod.fileName]?.canUpdate}
                <span
                  class="tag-update"
                  title={`${modUpdates[mod.fileName].currentVersion} → ${modUpdates[mod.fileName].latestVersion}`}
                >{updateLabel(modUpdates[mod.fileName])}</span>
              {/if}
              <span class="mod-meta">{(mod.sizeBytes / 1024).toFixed(0)} KB</span>
              <div class="mod-actions">
                <button class="mini" onclick={(event) => { event.stopPropagation(); modDetail = mod; }}>详情</button>
                <button class="mini" onclick={(event) => { event.stopPropagation(); void revealMod(mod); }}>打开位置</button>
                <button
                  class="mini"
                  disabled={busy !== ""}
                  onclick={(event) => { event.stopPropagation(); void toggle(mod); }}
                >{mod.enabled ? "禁用" : "启用"}</button>
                <button
                  class="mini danger"
                  disabled={busy !== ""}
                  onclick={(event) => { event.stopPropagation(); pendingDelete = { kind: "mod", mod }; }}
                >删除</button>
              </div>
            </div>
          {/each}
          {#if shownMods.length === 0}
            <p class="dim">没有匹配的模组。</p>
          {/if}
        </div>
        {#if selectedMods.size > 0}
          <div class="mod-bar">
            <span class="dim">已选 {selectedMods.size} 个文件</span>
            <div class="mod-bar-actions">
              <button
                class="btn"
                disabled={busy !== "" || checkingUpdates || !selectedAnyUpdatable}
                onclick={askUpdate}>更新</button
              >
              <button
                class="btn"
                disabled={busy !== "" || selectedAllEnabled}
                onclick={() => void batchSetEnabled(true)}>启用</button
              >
              <button
                class="btn"
                disabled={busy !== "" || selectedAllDisabled}
                onclick={() => void batchSetEnabled(false)}>禁用</button
              >
              <button class="btn danger" disabled={busy !== ""} onclick={() => void batchDelete()}>删除</button>
              <button class="btn ghost" onclick={() => (selectedMods = new Set())}>取消选择</button>
            </div>
          </div>
        {/if}
      {/if}
    </section>
  {:else if tab === "settings"}
    <div class="cards">
      <section class="scard">
        <h3>启动选项</h3>
        <div class="grid">
          <label class="field">
            <span>版本隔离</span>
            <Dropdown
              value={form.isolated ? "on" : "off"}
              label="版本隔离"
              options={[
                { value: "on", label: "隔离" },
                { value: "off", label: "不隔离" }
              ]}
              onchange={(value) => {
                form.isolated = value === "on";
                void flush();
              }}
            />
          </label>
          <label
            class="field wide"
            onmouseenter={(event) => showTip(event, TITLE_TIP)}
            onmousemove={moveTip}
            onmouseleave={hideTip}
          >
            <span>游戏窗口标题</span>
            <input
              type="text"
              value={form.windowTitle}
              maxlength="128"
              placeholder={defaultTitle}
              oninput={(event) => {
                form.windowTitle = event.currentTarget.value;
                scheduleSave();
              }}
              onblur={() => void flush()}
            />
          </label>
          <label
            class="field"
            onmouseenter={(event) => showTip(event, INFO_TIP)}
            onmousemove={moveTip}
            onmouseleave={hideTip}
          >
            <span>自定义信息</span>
            <input
              type="text"
              value={form.versionType}
              maxlength="32"
              placeholder={defaultInfo}
              oninput={(event) => {
                form.versionType = event.currentTarget.value;
                scheduleSave();
              }}
              onblur={() => void flush()}
            />
          </label>
          <label class="field wide">
            <span>游戏 Java</span>
            <Dropdown
              value={form.javaAuto ? "@auto" : form.javaPath}
              label="游戏 Java"
              options={[
                { value: "", label: "跟随全局设置" },
                { value: "@auto", label: "自动选择" },
                ...javaOptions.map((option) => ({
                  value: option.value,
                  label: option.label,
                  hint: javaHint(option.value),
                  hintTitle: cleanPath(option.value)
                }))
              ]}
              onchange={(value) => {
                form.javaAuto = value === "@auto";
                form.javaPath = form.javaAuto ? "" : value;
                void flush();
              }}
            />
          </label>
        </div>
      </section>

      <section class="scard">
        <h3>游戏内存</h3>
        <div class="radios">
          <label class="radio">
            <input
              type="radio"
              checked={form.memoryType === 2}
              onchange={() => {
                form.memoryType = 2;
                void flush();
              }}
            />
            跟随全局设置
          </label>
          <label class="radio">
            <input
              type="radio"
              checked={form.memoryType === 0}
              onchange={() => {
                form.memoryType = 0;
                void flush();
              }}
            />
            自动配置
          </label>
          <label class="radio">
            <input
              type="radio"
              checked={form.memoryType === 1}
              onchange={() => {
                form.memoryType = 1;
                void flush();
              }}
            />
            自定义
          </label>
        </div>
        {#if form.memoryType === 2}
          <p class="dim">用设置页里那份全局内存。</p>
        {:else if form.memoryType === 0}
          <p class="dim">
            {#if autoMemory}
              按 {mods.length} 个模组和当前可用内存算出来 {formatMb(autoMemory)}，每次启动现算。
            {:else}
              读不到本机内存，启动时再算。
            {/if}
          </p>
        {:else}
          <div class="grid">
            <label class="field">
              <span>最大内存（MB）</span>
              <input
                type="number"
                min="512"
                max={customMax}
                step="256"
                value={form.memoryCustom}
                oninput={(event) => {
                  form.memoryCustom = Number(event.currentTarget.value);
                  scheduleSave();
                }}
                onblur={() => void flush()}
              />
            </label>
            <label class="field">
              <span>初始内存（MB，0 表示同最大）</span>
              <input
                type="number"
                min="0"
                max={customMax}
                step="256"
                value={form.memoryInitial}
                oninput={(event) => {
                  form.memoryInitial = Number(event.currentTarget.value);
                  scheduleSave();
                }}
                onblur={() => void flush()}
              />
            </label>
          </div>
          <input
            class="slider"
            type="range"
            min="512"
            max={customMax}
            step="256"
            value={form.memoryCustom}
            oninput={(event) => {
              form.memoryCustom = Number(event.currentTarget.value);
              scheduleSave();
            }}
          />
        {/if}
        {#if memory}
          <div class="mem">
            <div class="mem-bar">
              <i class="mem-used" style={`width:${((memory.totalMb - memory.availableMb) / memory.totalMb) * 100}%`}></i>
              {#if gameMemory}
                <i class="mem-game" style={`width:${Math.min(100, (gameMemory / memory.totalMb) * 100)}%`}></i>
              {/if}
            </div>
            <div class="mem-legend">
              <span>系统已用 {formatMb(memory.totalMb - memory.availableMb)}</span>
              {#if gameMemory}<span>给游戏 {formatMb(gameMemory)}</span>{/if}
              <span>共 {formatMb(memory.totalMb)}</span>
            </div>
          </div>
        {/if}
      </section>

      <section class="scard">
        <h3>服务器</h3>
        <div class="grid">
          <label class="field wide">
            <span>限制验证方式</span>
            <Dropdown
              value={String(form.loginRequire)}
              label="限制验证方式"
              options={[
                { value: "0", label: "不限制" },
                { value: "1", label: "仅正版" },
                { value: "2", label: "仅离线模式" },
                { value: "3", label: "仅第三方登录" }
              ]}
              onchange={(value) => {
                form.loginRequire = Number(value);
                void flush();
              }}
            />
          </label>
          <label class="field wide">
            <span>自动进入服务器</span>
            <input
              type="text"
              value={form.serverEntry}
              maxlength="200"
              placeholder="例如 play.example.com:25565"
              oninput={(event) => {
                form.serverEntry = event.currentTarget.value;
                scheduleSave();
              }}
              onblur={() => void flush()}
            />
          </label>
        </div>
      </section>

      <section class="scard">
        <h3>高级启动选项</h3>
        <div class="grid">
          <label class="field wide">
            <span>JVM 参数头部（填了就用它替换版本自带的参数）</span>
            <textarea
              rows="3"
              value={form.jvmArgs}
              maxlength="4000"
              placeholder="-XX:+UseG1GC"
              oninput={(event) => {
                form.jvmArgs = event.currentTarget.value;
                scheduleSave();
              }}
              onblur={() => void flush()}
            ></textarea>
          </label>
          <label class="field wide">
            <span>游戏参数尾部</span>
            <textarea
              rows="2"
              value={form.gameArgs}
              maxlength="2000"
              placeholder="比如 --width 1280 --height 720"
              oninput={(event) => {
                form.gameArgs = event.currentTarget.value;
                scheduleSave();
              }}
              onblur={() => void flush()}
            ></textarea>
          </label>
          <label class="field wide">
            <span>Classpath 头部附加</span>
            <input
              type="text"
              value={form.classpathHead}
              maxlength="2000"
              oninput={(event) => {
                form.classpathHead = event.currentTarget.value;
                scheduleSave();
              }}
              onblur={() => void flush()}
            />
          </label>
          <label class="field wide">
            <span>启动前执行命令</span>
            <input
              type="text"
              value={form.preLaunch}
              maxlength="500"
              placeholder="在游戏目录里执行"
              oninput={(event) => {
                form.preLaunch = event.currentTarget.value;
                scheduleSave();
              }}
              onblur={() => void flush()}
            />
          </label>
        </div>
        <label class="check">
          <input
            type="checkbox"
            checked={form.preLaunchWait}
            onchange={(event) => {
              form.preLaunchWait = event.currentTarget.checked;
              void flush();
            }}
          />
          等启动前命令跑完
        </label>
        <label class="check">
          <input
            type="checkbox"
            checked={form.ignoreJavaWarning}
            onchange={(event) => {
              form.ignoreJavaWarning = event.currentTarget.checked;
              void flush();
            }}
          />
          忽略 Java 兼容性警告
        </label>
        <label class="check">
          <input
            type="checkbox"
            checked={form.skipFileCheck}
            onchange={(event) => {
              form.skipFileCheck = event.currentTarget.checked;
              void flush();
            }}
          />
          关闭文件校验
        </label>
        <label class="check">
          <input
            type="checkbox"
            checked={form.debugLog4j}
            onchange={(event) => {
              form.debugLog4j = event.currentTarget.checked;
              void flush();
            }}
          />
          使用调试级别 log4j 配置文件
        </label>
      </section>
    </div>

    {#if tip}
      <div class="float-tip" use:float style={`left:${tip.x}px; top:${tip.y}px`}>{tip.text}</div>
    {/if}
  {:else if tab === "modify"}
    <section class="fill">
      <MinecraftInstall
        gameDir={gameDir}
        lockedName={version.id}
        currentGame={version.gameVersion}
        currentLoaders={version.loaders}
        currentLoaderParts={version.loadersDetail}
        oninstalled={() => void load()}
      />
    </section>
  {:else if tab === "export"}
    <section class="fill">
      <PackExport id={version.id} />
    </section>
  {:else if tab === "servers"}
    <section class="panel">
      {#if servers.length === 0}
        <p class="dim">还没有记录服务器。</p>
      {:else}
        <table class="servers">
          <thead><tr><th>名称</th><th>地址</th></tr></thead>
          <tbody>
            {#each servers as server (server.ip)}
              <tr><td>{server.name}</td><td class="mono">{server.ip}</td></tr>
            {/each}
          </tbody>
        </table>
      {/if}
      <div class="row"><button class="btn" onclick={() => void openSub("servers")}>打开目录</button></div>
    </section>
  {:else if tab === "screenshots"}
    <section class="panel">
      {#if (entries.screenshots ?? []).length === 0}
        <p class="dim">还没有截图。</p>
      {:else}
        <div class="shots">
          {#each entries.screenshots ?? [] as shot (shot.name)}
            {#if !shot.dir && shots[shot.name]}
              <button class="shot" onclick={() => void api.revealPath(subPath("screenshots") + SEP + shot.name)}>
                <img src={shots[shot.name]} alt={shot.name} />
              </button>
            {/if}
          {/each}
        </div>
      {/if}
      <div class="row"><button class="btn" onclick={() => void openSub("screenshots")}>打开目录</button></div>
    </section>
  {:else if tab === "logs" || tab === "crashes"}
    <section class="panel">
      {#if (entries[tab] ?? []).length === 0}
        <p class="dim">{tab === "logs" ? "还没有日志。" : "还没有崩溃报告。"}</p>
      {:else}
        {#if tab === "crashes" && crashDiag}
          <div class="diag">
            <p class="diag-file mono">{crashDiag.file}</p>
            {#if diagBusy}
              <p class="dim">正在看这份报告…</p>
            {:else if crashDiag.items.length === 0}
              <p class="dim">这份报告里没有认出常见原因，原文在下面。</p>
            {:else}
              {#each crashDiag.items as item (item.key)}
                <div class="diag-item">
                  <strong>{item.title}</strong>
                  <p>{item.detail}</p>
                  <ul>
                    {#each item.advice as line}
                      <li>{line}</li>
                    {/each}
                  </ul>
                </div>
              {/each}
            {/if}
          </div>
        {/if}
        <div class="files">
          {#each entries[tab] ?? [] as file (file.name)}
            <button class="file" onclick={() => void openText(file.name)}>
              <span class="mono">{file.name}</span>
              <span class="dim">{file.dir ? "文件夹" : sizeText(file.size)}</span>
            </button>
          {/each}
        </div>
      {/if}
      <div class="row"><button class="btn" onclick={() => void openSub(tab)}>打开目录</button></div>
    </section>
  {:else}
    <section class="panel">
      {#if (entries[tab] ?? []).length === 0}
        <p class="dim">{EMPTY_TEXT[tab] ?? "这个目录还是空的。"}</p>
      {:else}
        <div class="files">
          {#each entries[tab] ?? [] as file (file.name)}
            <div class="file">
              <span class="mono">{file.name}</span>
              <span class="dim">{file.dir ? "文件夹" : sizeText(file.size)}</span>
            </div>
          {/each}
        </div>
      {/if}
      <div class="row"><button class="btn" onclick={() => void openSub(tab)}>打开目录</button></div>
    </section>
  {/if}
{/if}

{#if textView}
  <div class="textview">
    <header>
      <strong class="mono">{textView.title}</strong>
      <button class="btn ghost" onclick={() => (textView = null)}>关闭</button>
    </header>
    <pre class="mono">{textView.body}</pre>
  </div>
{/if}

{#if dragging}
  <div class="drop">松开即可导入模组</div>
{/if}

<ConfirmDialog
  open={pendingReset}
  message="重新从互联网获取这个实例的安装信息并重装一遍？你的存档、模组、资源包不会丢失，游戏核心会先备份到 versions\install-backups。"
  confirmLabel="重置"
  oncancel={() => (pendingReset = false)}
  onconfirm={() => {
    pendingReset = false;
    void resetInstance();
  }}
/>

<ConfirmDialog
  open={pendingDelete !== null}
  message={pendingDelete === null
    ? ""
    : pendingDelete.kind === "version"
      ? `删除实例「${id}」？\n\nversions\\${id} 整个目录会被丢进回收站，里面的存档、模组、配置都在其中；按住 Shift 再点是永久删除。`
      : `删除模组「${pendingDelete.mod.modName}」？

不按 Shift 会进回收站，还能捞回来；按住 Shift 点击是永久删除。`}
  oncancel={() => (pendingDelete = null)}
  onconfirm={(event) =>
    void (pendingDelete?.kind === "mod"
      ? removeMod(pendingDelete.mod, event.shiftKey)
      : removeVersion())}
/>

<ConfirmDialog
  open={updateConfirm}
  message="更新会先下载新版本，再替换掉旧文件。新版本可能不再兼容现在的存档或其他模组；装坏了可以从回收站找回旧文件。要继续吗？"
  confirmLabel="继续更新"
  oncancel={() => (updateConfirm = false)}
  onconfirm={() => {
    updateConfirm = false;
    void batchUpdate();
  }}
/>

<Modal open={modDetail !== null} title="模组详情" onclose={() => (modDetail = null)}>
  {#if modDetail}
    <div class="mod-head">
      {#if modIcons[modDetail.fileName]}
        <img class="mod-face" src={modIcons[modDetail.fileName]} alt="" />
      {/if}
      <p><strong>{modDetail.modName}</strong></p>
    </div>
    <p class="hint mono">{modDetail.fileName}</p>
    {#if modDetail.metadata?.description}
      <p class="hint">{modDetail.metadata.description}</p>
    {/if}
    <p class="hint">
      版本 {modDetail.metadata?.version ?? "—"} · 加载器 {modDetail.metadata?.loader ?? "未知"} · 大小 {(modDetail.sizeBytes / 1024).toFixed(0)} KB{#if modDetail.metadata?.authors?.length} · 作者 {modDetail.metadata.authors.join("、")}{/if}
    </p>
    {#if modUpdates[modDetail.fileName]}
      <p class="hint">
        来源 {modUpdates[modDetail.fileName].source === "modrinth" ? "Modrinth" : "CurseForge"} · 最新 {modUpdates[modDetail.fileName].latestVersion}
      </p>
    {/if}
  {/if}
  {#snippet footer()}
    <button class="btn" onclick={() => void openModWiki()}>百科搜索</button>
    <button
      class="btn ghost"
      onclick={() => {
        const target = modDetail;
        modDetail = null;
        if (target) void openModDownload(target);
      }}>下载页搜索</button
    >
    <button class="btn ghost" onclick={() => (modDetail = null)}>关闭</button>
  {/snippet}
</Modal>

<Modal open={renameOpen} title="修改实例名" onclose={() => (renameOpen = false)}>
  <input
    type="text"
    bind:value={renameDraft}
    maxlength="80"
    disabled={busy !== ""}
    onkeydown={(event) => {
      if (event.key === "Enter") void commitRename();
    }}
  />
  <p class="hint">
    实例目录 <code class="mono">versions\{id}</code> 里的 json 与 jar 会一起改名，失败会改回原样。
  </p>
  {#if renameError}<p class="error">{renameError}</p>{/if}
  {#snippet footer()}
    <button class="btn ghost" onclick={() => (renameOpen = false)}>取消</button>
    <button class="btn primary" disabled={busy !== ""} onclick={() => void commitRename()}>
      {busy === "rename" ? "改名中…" : "改名"}
    </button>
  {/snippet}
</Modal>

<Modal open={descOpen} title="修改实例描述" onclose={() => (descOpen = false)}>
  <textarea bind:value={descDraft} rows="3" maxlength="200" placeholder="一句话说明这个实例"></textarea>
  <p class="hint">显示在实例标题下面，最多 200 字。</p>
  {#snippet footer()}
    <button class="btn ghost" onclick={() => (descOpen = false)}>取消</button>
    <button class="btn primary" disabled={busy !== ""} onclick={() => void commitDescription()}>
      {busy === "desc" ? "保存中…" : "保存"}
    </button>
  {/snippet}
</Modal>

<Modal open={catOpen} title="加入分类" onclose={() => (catOpen = false)}>
  <div class="cat-list">
    {#each cats.categories as category (category.id)}
      <label class="cat-row">
        <input
          type="checkbox"
          checked={catDraft.includes(category.id)}
          onchange={(event) => {
            catDraft = event.currentTarget.checked
              ? [...catDraft, category.id]
              : catDraft.filter((item) => item !== category.id);
          }}
        />
        {category.name}
      </label>
    {/each}
    {#if cats.categories.filter((item) => item.id !== "starred").length === 0}
      <p class="hint">还没有自己建的分类，可以在下面新建一个；勾上「标星实例」会显示在主页左栏。</p>
    {/if}
  </div>
  <div class="cat-new">
    <input
      type="text"
      bind:value={catDraftName}
      maxlength="20"
      placeholder="新建分类"
      disabled={busy !== ""}
      onkeydown={(event) => {
        if (event.key === "Enter") void createDraftCategory();
      }}
    />
    <button class="btn ghost" disabled={busy !== "" || !catDraftName.trim()} onclick={() => void createDraftCategory()}>
      新建分类
    </button>
  </div>
  {#if catError}<p class="error">{catError}</p>{/if}
  {#snippet footer()}
    <button class="btn ghost" onclick={() => (catOpen = false)}>取消</button>
    <button class="btn primary" disabled={busy !== ""} onclick={() => void saveCategories()}>
      {busy === "cats" ? "保存中…" : "保存"}
    </button>
  {/snippet}
</Modal>
</div>

<style>
  .mod-head {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .mod-face {
    width: 40px;
    height: 40px;
    border-radius: 8px;
    object-fit: cover;
    flex: none;
  }

  .servers {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }
  .servers th,
  .servers td {
    text-align: left;
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
  }
  .servers th {
    color: var(--text-dim);
    font-weight: 500;
  }
  .files {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .diag {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px 14px;
    margin-bottom: 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: rgba(8, 10, 14, 0.35);
    font-size: 13px;
  }
  .diag-file {
    color: var(--text-dim);
    font-size: 12px;
  }
  .diag-item strong {
    font-weight: 600;
  }
  .diag-item p {
    margin: 4px 0 6px;
    color: var(--text-dim);
    line-height: 1.6;
  }
  .diag-item ul {
    margin: 0;
    padding-left: 18px;
    color: var(--text-dim);
    line-height: 1.7;
  }
  .file {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    width: 100%;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: rgba(8, 10, 14, 0.35);
    color: var(--text);
    padding: 7px 12px;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }
  button.file:hover {
    background: rgba(255, 255, 255, 0.06);
  }
  .shots {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 10px;
  }
  .shot {
    border: 1px solid var(--border);
    border-radius: 10px;
    overflow: hidden;
    background: rgba(8, 10, 14, 0.35);
    padding: 0;
    cursor: pointer;
  }
  .shot img {
    display: block;
    width: 100%;
    height: 120px;
    object-fit: cover;
  }
  .textview {
    position: fixed;
    inset: 8% 12%;
    z-index: 40;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px 16px;
    border-radius: 14px;
    border: 1px solid var(--border);
    background: rgba(12, 15, 21, 0.96);
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.5);
  }
  .textview header {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .textview pre {
    flex: 1;
    min-height: 0;
    overflow: auto;
    margin: 0;
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-all;
    color: var(--text-dim);
  }

  /* 整页是一条纵向流：头部与标签栏按内容占高，剩下的全给下面的内容区。 */
  .route {
    display: flex;
    flex-direction: column;
    min-height: 100%;
    max-height: 100%;
  }
  /* 「修改」页照下载页的版式：双栏吃掉剩下的全部空间，底下不再露背景。 */
  .fill {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
  .fill :global(.install) {
    flex: 1;
    min-height: 0;
  }
  /* 导出页两栏各自滚动，别让整页跟着跑。 */
  .fill :global(.wrap) {
    flex: 1;
    min-height: 0;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 4px 2px 14px;
    flex: none;
  }
  .head h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 650;
  }
  .spacer {
    flex: 1;
  }
  .kind {
    margin-left: 10px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 1px 8px;
    vertical-align: middle;
  }
  .tabs {
    display: flex;
    gap: 8px;
    padding: 0 2px 12px;
    flex: none;
  }
  .tab {
    border: 1px solid transparent;
    background: transparent;
    color: var(--text-dim);
    border-radius: 999px;
    padding: 5px 14px;
    font-size: 13px;
    cursor: pointer;
  }
  .tab.active {
    color: var(--text);
    border-color: var(--border);
    background: var(--surface);
  }
  .panel {
    border: 1px solid var(--border);
    background: var(--surface);
    border-radius: var(--radius-lg);
    padding: 18px 20px;
  }
  /* 没有整合包时这一块只有图标 + 名字 + 一行统计，按整合包那套留白会显得空。 */
  .panel.compact {
    padding: 14px 16px;
  }
  .panel.compact .big-icon {
    width: 52px;
    height: 52px;
    border-radius: 10px;
  }
  .panel.compact .head {
    gap: 12px;
    align-items: center;
  }
  .panel.compact .head-title strong {
    font-size: 18px;
  }
  .panel.compact .desc {
    -webkit-line-clamp: 1;
    line-clamp: 1;
  }
  .panel.compact .stats {
    margin-top: 10px;
    padding-top: 10px;
    gap: 18px;
  }
  .strip {
    display: flex;
    gap: 12px;
    align-items: center;
  }
  .strip-icon {
    width: 34px;
    height: 34px;
    border-radius: 8px;
    image-rendering: pixelated;
  }
  .strip-main {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .strip-main p {
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .head {
    display: flex;
    gap: 14px;
    align-items: flex-start;
  }
  /* 整合包的身份区可以点进去看它在下载页的样子。 */
  .head.linked {
    width: 100%;
    margin: -6px -8px;
    padding: 6px 8px;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
    border-radius: 10px;
  }
  .head.linked:hover {
    background: rgb(255 255 255 / 0.05);
  }
  .head.linked:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .big-icon {
    width: 64px;
    height: 64px;
    border-radius: 12px;
    image-rendering: pixelated;
    flex: none;
  }
  .head-main {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .head-title {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .head-title strong {
    font-size: 19px;
  }
  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .tag {
    border-radius: 999px;
    padding: 2px 9px;
    font-size: 12px;
    background: rgb(255 255 255 / 0.08);
    color: var(--text-dim);
  }
  .desc {
    margin: 0;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .meta {
    margin: 0;
    display: flex;
    gap: 6px;
    font-size: 12px;
  }
  .stats {
    display: flex;
    flex-wrap: wrap;
    gap: 22px;
    margin-top: 16px;
    padding-top: 14px;
    border-top: 1px solid var(--border);
  }
  .stat {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .loader-tag {
    display: inline-flex;
    align-items: center;
    height: 26px;
    padding: 0 10px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: rgb(255 255 255 / 0.04);
    color: var(--text);
    font-size: 12px;
    white-space: nowrap;
  }
  .stat img {
    width: 26px;
    height: 26px;
    image-rendering: pixelated;
  }
  .stat div {
    display: flex;
    flex-direction: column;
    line-height: 1.3;
  }
  .stat span {
    font-size: 12px;
    color: var(--text-dim);
  }
  .opt {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 12px;
  }
  .opt-label {
    flex: none;
    width: 44px;
    color: var(--text-dim);
  }
  .opt-control {
    flex: 1;
    max-width: 280px;
  }
  .mono {
    font-family: var(--mono, ui-monospace, monospace);
    font-size: 12.5px;
    word-break: break-all;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 14px;
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .field.wide {
    grid-column: 1 / -1;
  }
  .field input {
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--bg-1);
    color: var(--text);
    font-size: 13px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    flex: 1;
    min-width: 0;
  }
  .chip {
    background: none;
    border: 1px solid var(--border);
    color: var(--text-dim);
    border-radius: 999px;
    padding: 4px 12px;
    font: inherit;
    font-size: 12.5px;
    cursor: pointer;
  }
  .chip.on {
    border-color: transparent;
    background: rgb(120 200 120 / 0.18);
    color: var(--yb-green-bright);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 16px 0 4px;
    font-size: 13px;
    color: var(--text-dim);
  }
  .cards {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  /* 悬浮说明：左上角贴着鼠标，不挡点击 */
  .float-tip {
    position: fixed;
    z-index: 80;
    max-width: 340px;
    padding: 10px 12px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: var(--panel-solid, rgb(24 26 32 / 0.98));
    color: var(--text);
    font-size: 12px;
    line-height: 1.6;
    white-space: pre-line;
    pointer-events: none;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.45);
  }
  .scard {
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: 16px 18px;
    background: rgb(255 255 255 / 0.028);
  }
  .scard h3 {
    margin: 0 0 12px;
    font-size: 14px;
    font-weight: 600;
  }
  .scard .field textarea {
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--bg-1);
    color: var(--text);
    font: inherit;
    font-size: 13px;
    resize: vertical;
  }
  .radios {
    display: flex;
    gap: 18px;
    flex-wrap: wrap;
    margin-bottom: 10px;
  }
  .radio {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 13px;
    color: var(--text);
  }
  .slider {
    width: 100%;
    margin-top: 10px;
    accent-color: var(--yb-green-bright);
  }
  .mem {
    margin-top: 14px;
  }
  .mem-bar {
    display: flex;
    height: 10px;
    border-radius: 999px;
    overflow: hidden;
    background: var(--surface-2, rgb(0 0 0 / 0.25));
  }
  .mem-used {
    background: rgb(160 160 160 / 0.55);
  }
  .mem-game {
    background: rgb(120 200 120 / 0.75);
  }
  .mem-legend {
    display: flex;
    gap: 14px;
    margin-top: 6px;
    font-size: 12px;
    color: var(--text-dim);
  }
  /* 模组页：搜索 + 筛选 + 44px 列表 + 底部批量栏 */
  .mods-pane {
    display: flex;
    flex-direction: column;
    gap: 10px;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
  .mods-head {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
    align-items: center;
    flex: none;
  }
  .mods-search {
    flex: 1 1 220px;
    min-width: 160px;
    padding: 8px 12px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface-2, rgb(0 0 0 / 0.25));
    color: var(--text);
    font: inherit;
  }
  .mods-tools {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .mods-filters {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    flex: none;
  }
  .mods-filters .grow {
    flex: 1;
  }
  .chip {
    padding: 5px 12px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-dim);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .chip.active {
    background: rgb(127 176 105 / 0.22);
    border-color: rgb(127 176 105 / 0.55);
    color: var(--text);
  }
  .mod-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }
  .mod-row {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 44px;
    padding: 5px 10px;
    border-radius: 10px;
    border: 1px solid transparent;
    background: rgb(255 255 255 / 0.03);
    cursor: pointer;
  }
  .mod-row:hover {
    background: rgb(255 255 255 / 0.07);
  }
  .mod-row.picked {
    border-color: rgb(127 176 105 / 0.5);
  }
  .mod-row.disabled strong,
  .mod-row.disabled .mod-meta {
    opacity: 0.55;
  }
  .mod-row.disabled strong {
    text-decoration: line-through;
  }
  .mod-icon {
    width: 34px;
    height: 34px;
    flex: none;
    border-radius: 8px;
    overflow: hidden;
    display: grid;
    place-items: center;
    background: rgb(255 255 255 / 0.06);
  }
  .mod-icon img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .mod-icon-fallback {
    width: 18px;
    height: 18px;
    border-radius: 4px;
    background: rgb(255 255 255 / 0.1);
    color: rgb(255 255 255 / 0.55);
    display: grid;
    place-items: center;
  }
  .mod-icon-fallback :global(svg) {
    width: 14px;
    height: 14px;
  }
  .mod-name {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: 1;
    min-width: 0;
  }
  .mod-name strong {
    font-size: 13px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mod-name small,
  .mod-meta {
    color: var(--text-dim);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mod-actions {
    display: flex;
    gap: 6px;
    opacity: 0;
    transition: opacity 0.12s;
  }
  .mod-row:hover .mod-actions {
    opacity: 1;
  }
  .mini {
    font: inherit;
    font-size: 11px;
    padding: 3px 8px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: rgb(255 255 255 / 0.06);
    color: var(--text);
    cursor: pointer;
  }
  .mini.danger {
    color: var(--danger, #e2725b);
  }
  .tag-update {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
    background: rgb(127 176 105 / 0.3);
    flex: none;
  }
  .mod-bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: var(--panel-solid, rgb(20 22 26 / 0.95));
    flex: none;
  }
  .mod-bar-actions {
    margin-left: auto;
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  /* 原版实例的模组页：照 PCL 的提示，说明要先装加载器。 */
  .no-loader {
    margin: 8px 0;
    padding: 26px;
    border-radius: 14px;
    border: 1px solid var(--border);
    background: var(--surface);
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .no-loader h3 {
    margin: 0 0 4px;
    font-size: 18px;
    color: var(--yb-green);
  }
  .no-loader p {
    margin: 0;
    line-height: 1.6;
    color: var(--text);
  }
  .no-loader-actions {
    margin-top: 6px;
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
  }

  .empty-card {
    padding: 26px;
    border-radius: 14px;
    border: 1px dashed var(--border);
    display: flex;
    flex-direction: column;
    gap: 10px;
    align-items: flex-start;
  }
  .empty-card h3 {
    margin: 0;
    font-size: 15px;
  }
  .empty-actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .drop {
    position: fixed;
    inset: 0;
    display: grid;
    place-items: center;
    background: rgba(8, 10, 14, 0.6);
    border: 2px dashed var(--yb-green);
    font-size: 18px;
    z-index: 40;
    pointer-events: none;
  }
  .dim {
    color: var(--text-dim);
    font-size: 13px;
  }
  .cat-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .cat-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }
  .cat-new {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .cat-new input {
    flex: 1;
  }
</style>
