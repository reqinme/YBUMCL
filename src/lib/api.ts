// 与 Rust 命令层一一对应的薄封装。类型字段与后端 serde(camelCase) 对齐。
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type LauncherInfo = {
  version: string;
  exeDir: string;
  dataDir: string;
  gameDir: string;
  portable: boolean;
};

/** 一个版本就是一份可启动单元（版本即实例）。 */
export type VersionView = {
  id: string;
  kind: string;
  supported: boolean;
  reason: string | null;
  isolated: boolean;
  javaPath: string;
  /** 0 表示还没配，界面上用全局默认值展示 */
  maxMemoryMb: number;
  /** 该版本实际用的游戏目录（隔离时是 versions/<id>） */
  gameDirectory: string;
  /** 基础游戏版本（`1.21.10`、`26.3` …）；认不出来是 null */
  gameVersion: string | null;
  /** release / snapshot / april / unknown */
  channel: string;
  /** 加载器显示名，可多个（Forge + OptiFine） */
  loaders: string[];
  /** 加载器 + 版本号（安装时写进实例 json 的 `_ybmcl`，旧实例可能为空） */
  loadersDetail: InstanceLoader[];
  /** 归属的分类 id */
  categories: string[];
  /** 用户写的实例描述 */
  description: string;
  /** 图标设置：空=自动，`builtin:<方块名>`、`custom` */
  icon: string;
  /** 启动次数 */
  launches: number;
  /** 整合包信息；手动装的实例是 null */
  pack: PackInfo | null;
  /** 内存分配方式：0 自动、1 自定义、2 跟随全局 */
  memoryType: number;
  /** 自定义内存（MB），memoryType 为 1 时生效 */
  memoryCustomMb: number;
  /** 初始堆（MB），0 表示和最大堆一样 */
  memoryInitialMb: number;
  /** 游戏窗口标题，空=不改 */
  windowTitle: string;
  /** 覆盖 `${version_type}` 的自定义信息，空=用版本自己的 */
  versionType: string;
  /** JVM 参数头部（非空时整段替换版本自带的 JVM 参数） */
  jvmArgs: string;
  /** 游戏参数尾部 */
  gameArgs: string;
  /** Classpath 头部附加 */
  classpathHead: string;
  /** 启动前执行命令 */
  preLaunch: string;
  /** 启动前命令是否等它跑完 */
  preLaunchWait: boolean;
  /** Java 选「自动选择」 */
  javaAuto: boolean;
  /** 关闭文件校验 */
  skipFileCheck: boolean;
  /** 使用调试级别 log4j 配置文件 */
  debugLog4j: boolean;
  /** 忽略 Java 兼容性警告 */
  ignoreJavaWarning: boolean;
  /** 限制验证方式：0 不限制、1 仅正版、2 仅离线模式、3 仅第三方登录 */
  loginRequire: number;
  /** 自动进入服务器 */
  serverEntry: string;
};

/** 一个游戏文件夹。`kind` 为 current / official / custom。 */
export type GameFolder = {
  path: string;
  name: string;
  kind: "current" | "official" | "custom";
  exists: boolean;
  active: boolean;
};

export type Category = {
  id: string;
  name: string;
  createdAt: string;
};

/** 某个游戏目录的分类表 + 版本 → 分类 id 的归属。 */
export type CategorySnapshot = {
  categories: Category[];
  tags: Record<string, string[]>;
};

export type PackInfo = {
  source: string;
  projectId: string;
  projectName: string;
  versionLabel: string;
  summary: string;
  downloads: number;
  tags: string[];
};

export type VersionSettings = {
  isolated: boolean;
  javaPath: string;
  /// 0 自动、1 自定义、2 跟随全局。
  memoryType: number;
  /// `memoryType === 1` 时生效（MB）。
  memoryCustomMb: number;
  /// 初始堆（MB），0 表示同最大堆。
  memoryInitialMb: number;
  /// 旧字段，只给老 json 兜底；新界面不直接写它。
  maxMemoryMb: number;
  /// 空=自动，`builtin:<方块名>`=内置图，`custom`=实例目录里的 icon.png。
  icon: string;
  /// 概览页「修改实例描述」写的一句话。
  description: string;
  launches: number;
  pack: PackInfo | null;
  /// 游戏窗口标题，空=不改。
  windowTitle: string;
  /// 自定义信息：覆盖版本 json 的 `type`，进 `${version_type}`。
  versionType: string;
  /// JVM 参数头部：非空时整段替换版本自带参数。
  jvmArgs: string;
  /// 游戏参数尾部。
  gameArgs: string;
  /// Classpath 头部附加。
  classpathHead: string;
  /// 启动前执行命令。
  preLaunch: string;
  /// 启动前命令是否等它跑完。
  preLaunchWait: boolean;
  /// Java 选「自动选择」。
  javaAuto: boolean;
  /// 启动前不逐个检查文件在不在。
  skipFileCheck: boolean;
  /// 使用调试级别 log4j 配置文件。
  debugLog4j: boolean;
  /// 忽略 Java 兼容性警告。
  ignoreJavaWarning: boolean;
  /// 限制验证方式：0 不限制、1 仅正版、2 仅离线模式、3 仅第三方登录。
  loginRequire: number;
  /// 自动进入服务器：`host` 或 `host:port`。
  serverEntry: string;
};

export type VersionSummary = {
  id: string;
  kind: string;
  mainClass: string | null;
  supported: boolean;
  reason: string | null;
};

export type JavaInstallation = {
  path: string;
  version: string;
  major: number;
  /// 64 位还是 32 位（读 java.exe 的 PE 头）。
  is64: boolean;
  /// 只有运行时没有 javac。
  isJre: boolean;
  /// `java -version` 里 `(build 17.0.20+8-LTS)` 的括号内容。
  revision: string | null;
};

export type JavaCheck = {
  required: string;
  accepted: boolean;
  ignored: boolean;
  currentPath: string | null;
  currentMajor: number | null;
  suggestedPath: string | null;
  suggestedMajor: number | null;
};

export type JavaPlan = {
  description: string;
  path: string | null;
  major: number | null;
  message: string | null;
};

export type LogLine = {
  stream: string;
  text: string;
};

export type GameSnapshot = {
  running: boolean;
  pid: number | null;
  lastExit: number | null;
  logs: LogLine[];
};

export type LaunchRequest = {
  gameDir: string;
  gameDirectory?: string | null;
  versionId: string;
  javaPath: string;
  username: string;
  maxMemoryMb: number;
  accessToken?: string | null;
  uuid?: string | null;
};

export type RemoteVersion = {
  id: string;
  type: string;
  url: string;
  time: string;
  releaseTime: string;
  sha1: string | null;
  complianceLevel: number | null;
};

export type RemoteVersionManifest = {
  latest: { release: string; snapshot: string };
  versions: RemoteVersion[];
};

export type FabricLoaderVersion = {
  version: string;
  stable: boolean;
};

export type ModLoaderVersion = {
  version: string;
  recommended?: boolean;
  /// 测试版（beta/alpha/snapshot）：界面把正式版和测试版分开列。
  beta?: boolean;
};

/// 加载器清单里的一行：版本号 + 它对应的游戏版本，附稳定性与所需 Forge 提示。
export type LoaderEntry = {
  version: string;
  game: string;
  stable: boolean;
  forge: string | null;
};

export type InstallProgress = {
  versionId: string;
  stage: string;
  path: string;
  completedFiles: number;
  totalFiles: number;
  downloadedBytes: number;
  totalBytes: number;
  /// 加权百分比（0–100）；后端没算时为 null，界面按字节估算。
  percent: number | null;
};

export type InstallSummary = {
  versionId: string;
  /// 这次装出来的目录（实例安装是 `versions/<实例名>`；空串表示没有可打开的落点）。
  path: string;
  downloadedFiles: number;
  skippedFiles: number;
  downloadedBytes: number;
};

/// 崩溃报告里认出的一条原因。
export type CrashDiagnosis = {
  /// 规则标识（同一份报告里不重复）。
  key: string;
  /// 一句话结论。
  title: string;
  /// 依据。
  detail: string;
  /// 建议。
  advice: string[];
};

export type InstallRequest = {
  gameDir: string;
  versionId: string;
  fabricLoader?: string | null;
  quiltLoader?: string | null;
  strategy?: DownloadStrategy | null;
  threads?: number | null;
};

/** 目录里的一项（实例详情页的列表用）。 */
/// 光影包的一个选项取值。
export type ShaderValue = { id: string; label: string };

/// 光影包的一个选项。
export type ShaderOption = {
  name: string;
  label: string;
  comment: string | null;
  values: ShaderValue[];
  current: string;
  default: string;
};

/// 光影配置的一页。
export type ShaderScreen = {
  name: string;
  label: string;
  columns: number;
  options: ShaderOption[];
};

/// 整个光影包的配置。
export type ShaderConfig = {
  pack: string;
  source: string | null;
  valuesFile: string;
  screens: ShaderScreen[];
};

/// 存档页的一条：图标、显示名、游戏版本都是后端从磁盘上读出来的。
export type SaveEntry = {
  name: string;
  levelName: string | null;
  version: string | null;
  createdMs: number;
  modifiedMs: number;
  size: number;
  icon: string | null;
  seed: number | null;
  playSeconds: number | null;
};

export type EntryInfo = {
  name: string;
  dir: boolean;
  size: number;
  modifiedMs: number;
};

/** `servers.dat` 里的一条服务器。 */
export type ServerEntry = {
  name: string;
  ip: string;
};

/** 一次实例安装里选中的一个加载器。 */
export type InstanceLoader = {
  loader:
    | "fabric"
    | "legacyfabric"
    | "labymod"
    | "quilt"
    | "forge"
    | "neoforge"
    | "cleanroom"
    | "liteloader"
    | "optifine";
  version: string;
};

/** 原版 + 若干加载器合并成一个自包含实例。 */
export type InstanceRequest = {
  gameDir: string;
  gameVersion: string;
  instanceName?: string | null;
  loaders: InstanceLoader[];
  javaPath?: string | null;
  strategy?: DownloadStrategy | null;
  threads?: number | null;
  overwrite?: boolean;
};

export type ModLoaderInstallRequest = {
  gameDir: string;
  gameVersion: string;
  loader: "forge" | "neoforge" | "cleanroom" | "liteloader" | "optifine";
  loaderVersion: string;
  javaPath: string;
  strategy?: DownloadStrategy | null;
  threads?: number | null;
};

/// 工具页「磁盘占用」的一行：游戏目录下某个子目录的体积与文件数。
export type UsageEntry = {
  name: string;
  bytes: number;
  files: number;
};

/// 工具页「清理」的一行：固定几项，key 由后端认。
export type CleanTarget = {
  key: string;
  title: string;
  hint: string;
  path: string;
  bytes: number;
  files: number;
};

/** 下载源策略：有镜像的文件按它处理；没有镜像的一律走官方。 */
export type DownloadStrategy =
  "race" | "official-first" | "mirror-first" | "mirror-only" | "official-only";

export type Settings = {
  schemaVersion: number;
  username: string;
  memoryMb: number;
  javaPath: string;
  showSplash: boolean;
  /** 下载源策略：有镜像的文件按它处理；没有镜像的一律走官方。 */
  downloadStrategy: DownloadStrategy;
  /** 最大线程数：同时下载的线程上限（1–256，PCL 同款，默认 64）。 */
  concurrentDownloads: number;
  /** 下载限速档位（0–42，PCL 同款）：42 不限速，每个任务单独一份额度。 */
  downloadSpeed: number;
  homeRailCollapsed: boolean;
  /// 同时进行的下载任务数（1–8）。
  pageSize: number;
  splitRatio: number;
  /** 版本页左栏宽度百分比（12–30），全局一份。 */
  versionsSplitRatio: number;
  navSlideMs: number;
  ignoreQuilt: boolean;
  /// 内容与实例版本/加载器不匹配时是否给黄字提示。
  compatWarning: boolean;
  askSaveLocation: boolean;
  /** 整合包：true = 下载后直接装成实例，false = 只存 .mrpack。 */
  modpackInstall: boolean;
  /** 世界包：true = 解压进 saves，false = 只下载 zip。 */
  worldExtract: boolean;
  fileNameFormat: number;
  modNameLayout: number;
  /** 装模组时缺失前置怎么办：0 弹窗 / 1 直接下载前置 / 2 不管。 */
  missingDepsMode: number;
};

export type ModMetadata = {
  modId: string | null;
  name: string | null;
  version: string | null;
  authors: string[];
  description: string | null;
  loader: string | null;
  /// 描述符里声明了图标，或 jar 里带了图标文件。
  icon: boolean;
  /// 描述符条目名，例如 `fabric.mod.json`。
  descriptor: string | null;
};

export type ModProjectInfo = {
  url: string | null;
  title: string | null;
  slug: string | null;
  chinese: string | null;
  /// `modrinth` / `curseforge`；没匹配到项目时是 null。
  source: string | null;
  projectId: string | null;
};

export type ModEntry = {
  fileName: string;
  modName: string;
  enabled: boolean;
  sizeBytes: number;
  modifiedMillis: number;
  error: string | null;
  metadata: ModMetadata | null;
  metadataError: string | null;
};

export type ModImportOutcome = {
  source: string;
  fileName: string | null;
  status: string;
  message: string | null;
};

/// 一条模组的更新情况；只有能识别出来源（Modrinth / CurseForge）的才有。
/// 一次检查的结果：`identified` 是认出来源的模组数，`failed` 是这轮没查通的。
export type UpdateReport = {
  updates: ModUpdate[];
  identified: number;
  failed: number;
  cached: boolean;
};

export type ModUpdate = {
  fileName: string;
  source: "modrinth" | "curseforge";
  projectId: string;
  currentVersion: string;
  latestVersion: string;
  latestFileName: string;
  latestUrl: string;
  canUpdate: boolean;
};


/// 账户：离线档案（名字即身份）、微软账户、第三方认证服务器上的账户。
export type AccountKind = "offline" | "microsoft" | "thirdparty";

export type Account = {
  id: string;
  name: string;
  uuid: string;
  kind: AccountKind;
  /// `data:image/png;base64,…`，离线账户没有。
  avatar: string | null;
  /// 第三方账户所属的认证服务器地址。
  server: string | null;
};

/// 加过的第三方认证服务器（左栏就是按这个列表生成的）。
export type AuthServer = {
  url: string;
  name: string;
};

export type AccountsView = {
  players: Account[];
  current: string | null;
  servers: AuthServer[];
};

/// 设备代码登录：把 userCode + verificationUri 显示给用户。
export type DeviceCode = {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  interval: number;
  expiresIn: number;
  message: string;
};

export type MsLogin = {
  state: "pending" | "done" | "failed";
  error: string | null;
  accounts: AccountsView | null;
};

export const api = {
  info: () => invoke<LauncherInfo>("launcher_info"),
  listVersions: () => invoke<VersionView[]>("list_versions"),
  saveVersionSettings: (id: string, settings: VersionSettings) =>
    invoke<void>("save_version_settings", { id, settings }),
  /// 物理内存（自动配置内存、内存条用）。
  memoryInfo: () => invoke<{ totalMb: number; availableMb: number }>("memory_info"),
  /// `permanent` 为真才直接删目录，否则走回收站（照 PCL CE）。
  deleteVersion: (id: string, permanent = false) =>
    invoke<void>("delete_version", { id, permanent }),
  openVersionFolder: (id: string, which: "root" | "mods" | "saves" = "root") =>
    invoke<void>("open_version_folder", { id, which }),
  /// `java` 是一次性指定的 Java 路径（弹窗里选「改用 Java 21」时用），
  /// `ignoreJava` 为真时照旧用当前这套启动、不做兼容性检查。
  launchVersion: (
    id: string,
    options?: { java?: string | null; ignoreJava?: boolean; quickPlay?: string | null },
  ) =>
    invoke<number>("launch_version", {
      id,
      java: options?.java ?? null,
      ignoreJava: options?.ignoreJava ?? null,
      quickPlay: options?.quickPlay ?? null,
    }),
  /// 这个实例支不支持 `--quickPlaySingleplayer`（1.20+）。
  quickPlaySupported: (gameDir: string, id: string) =>
    invoke<boolean>("quick_play_supported", { gameDir, id }),
  /// 资源包 / 光影包的项目信息（名字、中文名、图标）。
  /// 资源包/光影包自带的 `pack.png`（本地读，不联网），用来先把图标铺上。
  installedModIds: (gameDir: string, gameDirectory: string): Promise<string[]> =>
    invoke<string[]>("installed_mod_ids", { gameDir, gameDirectory }),
  packUpdateCheck: (gameDir: string, gameDirectory: string) =>
    invoke<PackUpdateInfo | null>("pack_update_check", { gameDir, gameDirectory }),
  /// 老实例补一份整合包来源（没有来源记录才写），返回是否真的写了。
  packUpdateBackfill: (
    gameDir: string,
    gameDirectory: string,
    source: string,
    projectId: string,
    versionLabel: string,
    gameVersion: string,
    loaders: string[]
  ) =>
    invoke<boolean>("pack_update_backfill", {
      gameDir,
      gameDirectory,
      source,
      projectId,
      versionLabel,
      gameVersion,
      loaders
    }),
  packUpdateApply: (gameDir: string, gameDirectory: string, keepCustom: boolean) =>
    invoke<unknown>("pack_update_apply", { gameDir, gameDirectory, keepCustom }),
  packReinstall: (gameDir: string, gameDirectory: string, keepCustom: boolean) =>
    invoke<unknown>("pack_reinstall", { gameDir, gameDirectory, keepCustom }),
  packUpdateRestore: (gameDir: string, gameDirectory: string) =>
    invoke<void>("pack_update_restore", { gameDir, gameDirectory }),
  dependencyProjects: (
    ids: string[]
  ): Promise<Record<string, { title: string; slug: string; icon: string | null }>> =>
    invoke<Record<string, { title: string; slug: string; icon: string | null }>>(
      "dependency_projects",
      { ids }
    ),

  /// 界面心跳，主进程用它判断 WebView 是否卡死。
  heartbeat: () => invoke<void>("heartbeat"),
  packLocalIcons: (
    gameDir: string,
    gameDirectory: string | null,
    subdir: string,
    files: string[],
  ): Promise<Record<string, string>> => {
    return invoke<Record<string, string>>("pack_local_icons", {
      gameDir,
      gameDirectory,
      subdir,
      files,
    });
  },

  packProjects: (
    gameDir: string,
    gameDirectory: string | null,
    subdir: string,
    files: string[],
  ) =>
    invoke<Record<string, ModProjectInfo>>("pack_projects", {
      gameDir,
      gameDirectory,
      subdir,
      files,
    }),
  listSaves: (gameDir: string, gameDirectory: string | null) =>
    invoke<SaveEntry[]>("list_saves", { gameDir, gameDirectory }),
  /// 删掉实例子目录里的一个文件/文件夹（默认进回收站）。
  deleteEntry: (
    gameDir: string,
    gameDirectory: string | null,
    subdir: string,
    name: string,
    permanent = false,
  ) => invoke<void>("delete_entry", { gameDir, gameDirectory, subdir, name, permanent }),
  /// 把文件放进剪贴板（截图页「复制」）。
  copyFile: (gameDir: string, gameDirectory: string | null, subdir: string, name: string) =>
    invoke<void>("copy_file", { gameDir, gameDirectory, subdir, name }),
  importFiles: (gameDir: string, gameDirectory: string | null, subdir: string, sources: string[]) =>
    invoke<[number, number]>("import_files", { gameDir, gameDirectory, subdir, sources }),
  copySave: (gameDir: string, gameDirectory: string | null, name: string) =>
    invoke<void>("copy_save", { gameDir, gameDirectory, name }),
  pasteSave: (gameDir: string, gameDirectory: string | null) =>
    invoke<string[]>("paste_save", { gameDir, gameDirectory }),
  /// 读一个光影包的配置项（`shaders/shaders.properties`）。
  shaderConfig: (gameDir: string, gameDirectory: string | null, pack: string) =>
    invoke<ShaderConfig>("shader_config", { gameDir, gameDirectory, pack }),
  /// 保存光影配置：只写改过的值。
  saveShaderConfig: (
    gameDir: string,
    gameDirectory: string | null,
    pack: string,
    values: Record<string, string>,
  ) => invoke<void>("save_shader_config", { gameDir, gameDirectory, pack, values }),
  /// 启动前的 Java 兼容性检查，结果交给界面弹窗。
  checkJava: (id: string) => invoke<JavaCheck>("check_java", { id }),
  defaultGameDir: () => invoke<string>("default_game_dir"),
  versionIcon: (id: string) => invoke<string | null>("version_icon", { id }),
  setVersionIcon: (id: string, icon: string) =>
    invoke<VersionSettings>("set_version_icon", { id, icon }),
  pickVersionIcon: (id: string) =>
    invoke<VersionSettings | null>("pick_version_icon", { id }),
  setVersionPack: (id: string, pack: PackInfo | null) =>
    invoke<VersionSettings>("set_version_pack", { id, pack }),
  setVersionDescription: (id: string, description: string) =>
    invoke<VersionSettings>("set_version_description", { id, description }),
  renameInstance: (from: string, to: string) =>
    invoke<string>("rename_instance", { from, to }),
  resetInstance: (id: string) =>
    invoke<VersionSettings>("reset_instance", { id }),
  repairInstanceFiles: (id: string) =>
    invoke<InstallSummary>("repair_instance_files", { id }),
  /// 诊断一份崩溃报告；认不出原因时返回空数组。
  diagnoseCrashReport: (dir: string, file: string) =>
    invoke<CrashDiagnosis[]>("diagnose_crash_report", { dir, file }),
  /// 选一个 jar/zip，把里面的文件合并进实例的客户端核心；返回合并的条目数。
  patchInstanceCore: (id: string) => invoke<number>("patch_instance_core", { id }),
  /// 重置前把 `<实例>.json` / `<实例>.jar` 备份到 versions\install-backups，返回备份目录。
  backupInstanceCore: (id: string) => invoke<string>("backup_instance_core", { id }),
  exportLaunchScript: (id: string) =>
    invoke<string>("export_launch_script", { id }),
  testInstance: (id: string) => invoke<string>("test_instance", { id }),
  scanVersions: (gameDir: string) =>
    invoke<VersionSummary[]>("scan_versions", { gameDir }),

  // ---------------------------------------------------------- 游戏文件夹
  listGameFolders: () => invoke<GameFolder[]>("list_game_folders"),
  addGameFolder: (path: string) =>
    invoke<GameFolder[]>("add_game_folder", { path }),
  removeGameFolder: (path: string) =>
    invoke<GameFolder[]>("remove_game_folder", { path }),
  renameGameFolder: (path: string, name: string) =>
    invoke<GameFolder[]>("rename_game_folder", { path, name }),
  setFolderOrder: (order: string[]) =>
    invoke<void>("set_folder_order", { order }),
  setActiveGameFolder: (path: string) =>
    invoke<GameFolder[]>("set_active_game_folder", { path }),
  pickGameFolder: () => invoke<string | null>("pick_game_folder"),

  // -------------------------------------------------------------- 分类
  listCategories: () => invoke<CategorySnapshot>("list_categories"),
  createCategory: (name: string) =>
    invoke<CategorySnapshot>("create_category", { name }),
  renameCategory: (id: string, name: string) =>
    invoke<CategorySnapshot>("rename_category", { id, name }),
  deleteCategory: (id: string) =>
    invoke<CategorySnapshot>("delete_category", { id }),
  setVersionCategories: (version: string, categories: string[]) =>
    invoke<CategorySnapshot>("set_version_categories", { version, categories }),
  /// 打星 / 取消打星：星标就是「标星实例」这条内置分类上的 tag。
  setStarred: (version: string, starred: boolean) =>
    invoke<CategorySnapshot>("set_starred", { version, starred }),
  /// 把旧版存在 localStorage 里的星标并进 categories.json。
  importStarred: (versions: string[]) =>
    invoke<CategorySnapshot>("import_starred", { versions }),
  detectJava: (force = false) =>
    invoke<JavaInstallation[]>("detect_java", { force }),
  planJava: (
    gameVersion: string,
    loader?: string,
    loaderVersion?: string,
    versionId?: string,
  ) =>
    invoke<JavaPlan>("plan_java", {
      gameVersion,
      loader,
      loaderVersion,
      versionId,
    }),
  gameSnapshot: () => invoke<GameSnapshot>("game_snapshot"),
  fetchRemoteVersions: (strategy: DownloadStrategy = "race") =>
    invoke<RemoteVersionManifest>("fetch_remote_versions", { strategy }),
  fetchQuiltLoaders: (gameVersion: string) =>
    invoke<FabricLoaderVersion[]>("fetch_quilt_loaders", { gameVersion }),
  fetchFabricLoaders: (
    gameVersion: string,
    strategy: DownloadStrategy = "race",
  ) =>
    invoke<FabricLoaderVersion[]>("fetch_fabric_loaders", {
      gameVersion,
      strategy,
    }),
  fetchLabymodVersions: (channel: string) =>
    invoke<
      { version: string; game: string; channel: string; commit: string }[]
    >("fetch_labymod_versions", { channel }),
  fetchLegacyFabricLoaders: (gameVersion: string) =>
    invoke<FabricLoaderVersion[]>("fetch_legacy_fabric_loaders", {
      gameVersion,
    }),
  fetchModloaderVersions: (loader: string, gameVersion: string) =>
    invoke<ModLoaderVersion[]>("fetch_modloader_versions", {
      loader,
      gameVersion,
    }),
  fetchLoaderCatalog: (loader: string, gameVersion: string) =>
    invoke<LoaderEntry[]>("fetch_loader_catalog", { loader, gameVersion }),
  fetchLoaderDates: (loader: string, gameVersion: string, versions: string[]) =>
    invoke<{ version: string; date: string }[]>("fetch_loader_dates", {
      loader,
      gameVersion,
      versions,
    }),
  installGameVersion: (request: InstallRequest, installId?: number) =>
    invoke<InstallSummary>("install_game_version", { request, installId }),
  installGameModloader: (request: ModLoaderInstallRequest, installId?: number) =>
    invoke<InstallSummary>("install_game_modloader", { request, installId }),
  installGameInstance: (request: InstanceRequest, installId?: number) =>
    invoke<InstallSummary>("install_game_instance", { request, installId }),
  /** 实例实际用的根目录（隔离实例是 `versions/<id>`）。 */
  instanceRoot: (id: string) => invoke<string>("instance_root", { id }),
  listEntries: (path: string) => invoke<EntryInfo[]>("list_entries", { path }),
  /** 读文本；`tail` 为真时取末尾 `limit` 字节（日志用）。 */
  readTextFile: (path: string, limit?: number, tail?: boolean) =>
    invoke<string>("read_text_file", { path, limit, tail }),
  /** 递归搜文件名，结果从 `file-search` 事件里追加。 */
  searchFiles: (token: string, root: string, needle: string, limit?: number) =>
    invoke<void>("search_files", { token, root, needle, limit }),
  stopSearch: (token: string) => invoke<void>("stop_search", { token }),
  imageDataUrl: (path: string) => invoke<string>("image_data_url", { path }),
  readServers: (path: string) =>
    invoke<ServerEntry[]>("read_servers", { path }),
  /** 把实例打成 zip；返回 null 表示用户在另存为里取消。 */
  exportInstanceZip: (id: string, taskId?: number) =>
    invoke<string | null>("export_instance_zip", { id, taskId }),
  /// 整合包导出的选项表（可见性/子项都按实例目录探过）。
  exportPackLayout: (id: string) =>
    invoke<ExportOptionView[]>("export_pack_layout", { id }),
  /// 第一阶段：按规则挑文件、算哈希，返回任务 token。
  exportPackBegin: (id: string, rules: string[], taskId?: number) =>
    invoke<number>("export_pack_begin", { id, rules, taskId }),
  /// 第二阶段：联网识别，返回 `[认出来的, 待识别的]`。
  exportPackIdentify: (token: number, modrinthOnly: boolean) =>
    invoke<[number, number]>("export_pack_identify", { token, modrinthOnly }),
  /// 第三阶段：写索引并打包；`target` 为空时落到游戏的 downloads。
  exportPackFinish: (args: {
    id: string;
    token: number;
    name: string;
    version: string;
    includeLauncher: boolean;
    taskId?: number;
    target?: string | null;
  }) => invoke<string | null>("export_pack_finish", args),
  exportPackPickPath: (fileName: string, includeLauncher: boolean) =>
    invoke<string | null>("export_pack_pick_path", { fileName, includeLauncher }),
  /// 读一份 PCL 格式的导出配置；取消返回 null。
  exportPackLoadConfig: () => invoke<ExportConfigView | null>("export_pack_load_config"),
  /// 把当前勾选写成 PCL 格式的导出配置；取消返回 null。
  exportPackSaveConfig: (args: {
    name: string;
    version: string;
    includeLauncher: boolean;
    packResources: boolean;
    modrinthOnly: boolean;
    packPath: string | null;
    rules: string[];
    extraFiles: string[];
  }) => invoke<string | null>("export_pack_save_config", args),
  exportPackCancel: (token: number) =>
    invoke<void>("export_pack_cancel", { token }),
  instanceNameTaken: (gameDir: string, name: string) =>
    invoke<boolean>("instance_name_taken", { gameDir, name }),
  /// `mode = "pause"` 只停不清（半截 `.part` 留着续传）；其它值等于取消并清理。
  /// 停某一个安装任务（按前端任务卡的 id 找），`mode = "pause"` 只停不清。
  cancelInstall: (installId: number | null | undefined, mode: "pause" | "cancel" = "cancel") =>
    invoke<boolean>("cancel_install", { installId: installId ?? null, mode }),
  /// 安装包：把单个加载器（或原版客户端）的文件下载到指定目录。
  downloadPackage: (
    kind: string,
    gameVersion: string,
    version: string,
    dir: string,
    strategy: DownloadStrategy,
    taskId?: number,
  ) =>
    invoke<string[]>("download_package", {
      kind,
      gameVersion,
      version,
      dir,
      strategy,
      taskId,
    }),
  /// Modrinth：搜索项目、取版本列表、下载主文件。
  modrinthSearch: (
    kind: string,
    query: string,
    gameVersion?: string | null,
    loader?: string | null,
    limit = 20,
    offset = 0,
  ) =>
    invoke<MrProject[]>("modrinth_search", {
      kind,
      query,
      gameVersion,
      loader,
      limit,
      offset,
    }),
  modrinthVersions: (
    project: string,
    gameVersion?: string | null,
    loader?: string | null,
  ) =>
    invoke<MrVersion[]>("modrinth_versions", { project, gameVersion, loader }),
  /// CurseForge：返回与 Modrinth 相同的结构，界面不用区分来源。
  curseforgeSearch: (
    kind: string,
    query: string,
    gameVersion?: string | null,
    loader?: string | null,
    sort = "relevance",
    limit = 20,
    offset = 0,
  ) =>
    invoke<MrProject[]>("curseforge_search", {
      kind,
      query,
      gameVersion,
      loader,
      sort,
      limit,
      offset,
    }),
  curseforgeVersions: (
    project: string,
    gameVersion?: string | null,
    loader?: string | null,
  ) =>
    invoke<MrVersion[]>("curseforge_versions", {
      project,
      gameVersion,
      loader,
    }),
  /// FTB：只有整合包。搜索要按「页」给，后端会把 id 列表并发换成详情。
  ftbSearch: (term: string, sort: string, page: number, limit: number) =>
    invoke<SearchPage>("ftb_search", { term, sort, page, limit }),
  ftbVersions: (
    project: string,
    gameVersion?: string | null,
    loader?: string | null,
  ) => invoke<MrVersion[]>("ftb_versions", { project, gameVersion, loader }),
  /// 取图标并转成 data URL；纯白/纯透明的图返回 null（界面用首字母占位）。
  fetchIcon: (url: string) => invoke<string | null>("fetch_icon", { url }),
  /// 统一的分类搜索（两个来源同一结构，带 total 供「加载更多」）。
  contentSearch: (args: {
    source: "modrinth" | "curseforge";
    kind: string;
    query: string;
    gameVersion?: string | null;
    loader?: string | null;
    sort?: string;
    category?: string | null;
    env?: string | null;
    offset?: number;
    limit?: number;
    /// 关键词是中文时，后端先换成英文再去查（只有模组 / 数据包会传 true）。
    translate?: boolean;
  }) =>
    invoke<SearchPage>("content_search", { ...args, limit: args.limit ?? 40 }),
  contentCategories: (source: "modrinth" | "curseforge", kind: string) =>
    invoke<ContentCategory[]>("content_categories", { source, kind }),
  modrinthBrowse: (
    kind: string,
    sort: string,
    gameVersion?: string | null,
    limit = 8,
  ) =>
    invoke<MrProject[]>("modrinth_browse", { kind, sort, gameVersion, limit }),
  /** 对话框定位之前先建目录（不存在的话 Windows 会忽略起始位置）。 */
  ensureDir: (path: string) => invoke<void>("ensure_dir", { path }),
  saveModrinth: (
    url: string,
    fileName: string,
    slug: string | undefined,
    title: string | undefined,
    format: number | undefined,
    defaultDir: string | undefined,
    taskId: number | undefined,
  ) =>
    invoke<string | null>("save_modrinth", {
      url,
      fileName,
      slug,
      title,
      format,
      defaultDir,
      taskId,
    }),
  /** 世界包：`extract` 为 true 时解压进 `saves`，否则只留 zip（返回落盘路径）。 */
  installWorld: (
    url: string,
    fileName: string,
    gameDir: string,
    extract: boolean,
    taskId?: number,
    /// 解压目标；不传就是 `<gameDir>\saves`（选了实例时传实例的 saves）。
    savesDir?: string,
  ) =>
    invoke<string>("install_world", {
      url,
      fileName,
      gameDir,
      extract,
      taskId,
      savesDir,
    }),
  /** 世界包「只下载」：弹另存为，默认目录给 `defaultDir`；取消返回 null。 */
  saveWorld: (
    url: string,
    fileName: string,
    defaultDir: string | undefined,
    taskId: number | undefined,
  ) =>
    invoke<string | null>("save_world", { url, fileName, defaultDir, taskId }),
  /** MC 百科条目地址；项目在百科上没有条目时返回 null。 */
  wikiLink: (source: string, slug: string) =>
    invoke<string | null>("wiki_link", { source, slug }),
  savePackage: (
    kind: string,
    gameVersion: string,
    version: string,
    strategy: DownloadStrategy | undefined,
    defaultDir: string | undefined,
    taskId: number | undefined,
  ) =>
    invoke<string | null>("save_package", {
      kind,
      gameVersion,
      version,
      strategy,
      defaultDir,
      taskId,
    }),
  modrinthDownload: (
    url: string,
    dir: string,
    fileName: string,
    slug?: string,
    title?: string,
    format?: number,
  ) =>
    invoke<string>("modrinth_download", {
      url,
      dir,
      fileName,
      slug,
      title,
      format,
    }),
  favoritesLoad: () => invoke<Favorites>("favorites_load"),
  favoritesSave: (favorites: Favorites) =>
    invoke<void>("favorites_save", { favorites }),
  loadSettings: () => invoke<Settings>("load_settings"),
  readLauncherLog: (limit = 2000) =>
    invoke<string>("read_launcher_log", { limit }),
  readGameLog: (gameDir: string, limit = 2000) =>
    invoke<string>("read_game_log", { gameDir, limit }),
  launcherLogPath: () => invoke<string>("launcher_log_path"),
  /// 导出错误报告：弹另存为，写一份带日志尾巴的 txt。
  exportErrorReport: (request: {
    title: string;
    message: string;
    detail: string;
    source: string;
    gameDir?: string | null;
  }) =>
    invoke<string | null>("export_error_report", {
      title: request.title,
      message: request.message,
      detail: request.detail,
      source: request.source,
      gameDir: request.gameDir ?? null,
    }),
  saveSettings: (settings: Settings) =>
    invoke<Settings>("save_settings", { settings }),
  listMods: (gameDir: string, gameDirectory: string | null) =>
    invoke<ModEntry[]>("list_mods", { gameDir, gameDirectory }),
  setModEnabled: (
    gameDir: string,
    gameDirectory: string | null,
    fileName: string,
    enabled: boolean,
  ) =>
    invoke<ModEntry>("set_mod_enabled", {
      gameDir,
      gameDirectory,
      fileName,
      enabled,
    }),
  removeMod: (
    gameDir: string,
    gameDirectory: string | null,
    fileName: string,
    permanent = false,
  ) => invoke<void>("remove_mod", { gameDir, gameDirectory, fileName, permanent }),
  modIcon: (gameDir: string, gameDirectory: string | null, fileName: string) =>
    invoke<string | null>("mod_icon", { gameDir, gameDirectory, fileName }),
  /// 给 jar 里没带图标的模组，按 SHA1 / 指纹去 Modrinth、CurseForge 找项目图标。
  modProjectIcons: (
    gameDir: string,
    gameDirectory: string | null,
    files: string[],
  ) =>
    invoke<Record<string, ModProjectInfo>>("mod_project_icons", {
      gameDir,
      gameDirectory,
      files,
    }),
  /// 认领这个 jar 在 Modrinth / CurseForge 上的项目；认不到就是 null。
  modProject: (gameDir: string, gameDirectory: string | null, fileName: string) =>
    invoke<{ source: string; projectId: string } | null>("mod_project", {
      gameDir,
      gameDirectory,
      fileName
    }),
  /// 查更新。`force` 为真是手动的「检查更新」（忽略 6 小时缓存）。
  checkModUpdates: (
    gameDir: string,
    gameDirectory: string | null,
    gameVersion: string,
    loader: string,
    force = false,
  ) =>
    invoke<UpdateReport>("check_mod_updates", {
      gameDir,
      gameDirectory,
      gameVersion,
      loader,
      force,
    }),
  cancelModUpdates: () => invoke<void>("cancel_mod_updates"),
  applyModUpdate: (
    gameDir: string,
    gameDirectory: string | null,
    update: ModUpdate,
  ) => invoke<string>("apply_mod_update", { gameDir, gameDirectory, update }),
  addMods: (
    gameDir: string,
    gameDirectory: string | null,
    sources: string[],
    overwrite: boolean,
  ) =>
    invoke<ModImportOutcome[]>("add_mods", {
      gameDir,
      gameDirectory,
      sources,
      overwrite,
    }),
  dirUsage: () => invoke<UsageEntry[]>("dir_usage"),
  openFolder: (path: string) => invoke<void>("open_folder", { path }),
  /// 在资源管理器里选中这个文件/目录（不是打开它）。
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  /// 删掉某条任务的产出；后端只认它自己这次产出过的路径。
  deleteOutput: (path: string) => invoke<void>("delete_output", { path }),
  pathExists: (path: string) => invoke<boolean>("path_exists", { path }),
  listAccounts: () => invoke<AccountsView>("list_accounts"),
  addOfflineAccount: (name: string) =>
    invoke<AccountsView>("add_offline_account", { name }),
  renameAccount: (id: string, name: string) =>
    invoke<AccountsView>("rename_account", { id, name }),
  removeAccount: (id: string) => invoke<AccountsView>("remove_account", { id }),
  selectAccount: (id: string) => invoke<AccountsView>("select_account", { id }),
  refreshAccount: (id: string) => invoke<AccountsView>("refresh_account", { id }),
  addAuthServer: (url: string) =>
    invoke<AccountsView>("add_auth_server", { url }),
  removeAuthServer: (url: string) =>
    invoke<AccountsView>("remove_auth_server", { url }),
  addThirdPartyAccount: (url: string, username: string, password: string) =>
    invoke<AccountsView>("add_third_party_account", { url, username, password }),
  msLoginStart: () => invoke<DeviceCode>("ms_login_start"),
  msLoginPoll: (deviceCode: string) =>
    invoke<MsLogin>("ms_login_poll", { deviceCode }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  /// 整合包一键安装：下载 → 读 manifest → 建实例 → 铺 mods/overrides。
  installModpack: (request: {
    gameDir: string;
    url: string | null;
    path: string | null;
    fileName: string;
    name: string | null;
    javaPath: string | null;
    /** 下载源策略；没有镜像路由的文件永远只走官方。 */
    strategy: DownloadStrategy;
    /** 并发下载线程数（1–256）。 */
    threads?: number;
    /** 资源站上这个整合包的图标地址；装完由后端存成实例图标。 */
    iconUrl?: string | null;
    /** 整合包文件大小；带上就不用在下载前发一次 Range 探测。 */
    size?: number;
    /** 从搜索页装过来时项目所在的站点，用来给实例补上来源（包内 manifest 没有项目 id）。 */
    source?: "curseforge" | "modrinth" | "ftb" | null;
    projectId?: string | null;
    projectName?: string | null;
  }, installId?: number) =>
    invoke<{
      versionId: string;
      path: string;
      gameVersion: string;
      files: number;
      failed: number;
      missing: string[];
      /** 从包里认出来的来源；界面不用它，后端直接写进实例的 `ybmcl.json`。 */
      pack: {
        source: string;
        projectId: string;
        projectName: string;
        versionLabel: string;
        summary: string;
        downloads: number;
        tags: string[];
      } | null;
    }>("install_modpack", { request, installId }),
  /// FTB 整合包：后端把版本清单写成临时 mrpack，再走同一个安装流程。
  installFtb: (request: {
    gameDir: string;
    project: string;
    version: string;
    name: string | null;
    javaPath: string | null;
    strategy: DownloadStrategy;
    /** FTB 项目在资源站上的图标地址；装完由后端存成实例图标。 */
    iconUrl?: string | null;
  }, installId?: number) =>
    invoke<{
      versionId: string;
      path: string;
      gameVersion: string;
      files: number;
      failed: number;
      missing: string[];
    }>("install_ftb_modpack", { ...request, installId }),

  cleanupTargets: () => invoke<CleanTarget[]>("cleanup_targets"),
  cleanupRun: (key: string) => invoke<number>("cleanup_run", { key }),
};

export function onGameLog(
  handler: (line: LogLine) => void,
): Promise<UnlistenFn> {
  return listen<LogLine>("game-log", (event) => handler(event.payload));
}

/// 递归搜索的一条结果。
export type FileHit = {
  path: string;
  name: string;
  size: number;
  modifiedMs: number;
};

/// 递归搜索的进度：同一 token 的多批结果按到达顺序追加，`done` 为真时结束。
export type FileSearchProgress = {
  token: string;
  hits: FileHit[];
  scanned: number;
  done: boolean;
};

export function onFileSearch(
  handler: (progress: FileSearchProgress) => void,
): Promise<UnlistenFn> {
  return listen<FileSearchProgress>("file-search", (event) =>
    handler(event.payload),
  );
}

export function onGameExit(
  handler: (code: number | null) => void,
): Promise<UnlistenFn> {
  return listen<number | null>("game-exit", (event) => handler(event.payload));
}

export function onInstallProgress(
  handler: (progress: InstallProgress) => void,
): Promise<UnlistenFn> {
  return listen<InstallProgress>("install-progress", (event) =>
    handler(event.payload),
  );
}

/// 整合包安装的阶段性进度：阶段名、完成份数、已下字节、当前文件名。
export type ModpackProgress = {
  /// 哪一次安装发的事件（前端任务卡的 id）；多张卡同时在跑时靠它写回各自的卡。
  installId?: number;
  stage: string;
  done: number;
  total: number;
  bytes: number;
  /// 这一步总共要下多少字节；0 表示未知（比如解压、装加载器）。
  totalBytes?: number;
  file: string | null;
  /// 整体百分比（0–100）：后端把各阶段加权折成一条连续曲线，界面直接用。
  overall?: number;
  /// 当前速度（字节/秒）；没有下载动作的阶段是 0。
  speed?: number;
};
export function onModpackProgress(
  handler: (progress: ModpackProgress) => void,
): Promise<UnlistenFn> {
  return listen<ModpackProgress>("modpack-progress", (event) =>
    handler(event.payload),
  );
}

/// 查更新的进度：阶段名（hash / modrinth / curseforge / done）+ 已完成 + 总数。
export type ModUpdateProgress = {
  stage: string;
  done: number;
  total: number;
  bytes?: number;
  file?: string;
};
export interface PackUpdateProgress {
  stage: string;
  done?: number;
  total?: number;
  bytes?: number;
  totalBytes?: number;
}

/** 整合包更新（下载包本体 + 展开）的进度事件。 */
export function onPackUpdateProgress(
  handler: (progress: PackUpdateProgress) => void,
): Promise<UnlistenFn> {
  return listen<PackUpdateProgress>("pack-update-progress", (event) =>
    handler(event.payload),
  );
}

export function onModUpdatesProgress(
  handler: (progress: ModUpdateProgress) => void,
): Promise<UnlistenFn> {
  return listen<ModUpdateProgress>("mod-updates-progress", (event) =>
    handler(event.payload),
  );
}

export interface FavoriteEntry {
  projectId: string;
  title: string;
  slug: string;
  description: string;
  iconUrl: string | null;
  kind: string;
  addedAt: number;
  /// 这个项目支持的游戏版本（逐个版本存，不并成大版本），收藏夹里按版本筛选用。
  versions: string[];
  /// 支持的加载器显示名（Fabric / Forge ……），收藏夹里按加载器筛选用。
  loaders: string[];
}

export interface FavoriteFolder {
  id: string;
  name: string;
  createdAt: number;
  entries: FavoriteEntry[];
}

export interface Favorites {
  folders: FavoriteFolder[];
}

export interface MrProject {
  id: string;
  slug: string;
  title: string;
  description: string;
  iconUrl: string | null;
  downloads: number;
  author: string;
  categories: string[];
  projectType: string;
  /// 支持的加载器（Fabric / NeoForge…），列表里显示「支持版本」用。
  loaders: string[];
  /// 支持的游戏版本（未排序），列表里压成区间显示。
  gameVersions: string[];
  /// 内置名表里的中文名，没有就是 null。
  chineseTitle: string | null;
}

export interface SearchPage {
  items: MrProject[];
  total: number;
}

export interface ContentCategory {
  source: string;
  kind: string;
  value: string;
  name: string;
  icon: string | null;
}

export interface PackUpdateInfo {
  source: string;
  projectId: string;
  currentVersion: string;
  latestVersion: string;
  latestVersionId: string;
  latestUrl: string;
  latestFileName: string;
  gameVersion: string;
  /// 增量更新是否安全（加载器或大版本变了就只能重装）。
  allowIncremental: boolean;
  /// 不能增量的原因。
  reason: string;
}


export interface MrVersion {
  id: string;
  name: string;
  versionNumber: string;
  gameVersions: string[];
  loaders: string[];
  /// 客户端/服务端：`client`、`server`；两个都有就是双端，空数组表示来源没给这项数据。
  environments: string[];
  datePublished: string;
  downloads: number;
  versionType: string;
  fileName: string;
  fileUrl: string;
  fileSize: number;
  /// 这个文件声明的前置（Modrinth 精确到版本；CurseForge 是最新文件的那份）。
  dependencies?: {
    versionId: string | null;
    projectId: string | null;
    fileName: string | null;
    dependencyType: string;
  }[];
}

/// 导出页的一个选项（分组标题或可勾选项）。
export type ExportConfigView = {
  configPath: string;
  name: string;
  version: string;
  includeLauncher: boolean;
  packResources: boolean;
  modrinthOnly: boolean;
  packPath: string | null;
  rules: string[];
  extraFiles: string[];
};

export type ExportEntryView = {
  label: string;
  hint: string;
  rule: string;
};

export type ExportOptionView = {
  id: string;
  parent: string | null;
  title: string;
  description: string;
  visible: boolean;
  checked: boolean;
  dynamic: boolean;
  entries: ExportEntryView[];
  rules: string[];
};
