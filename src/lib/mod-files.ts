//! 社区资源「文件列表」的分组、筛选与排序。
//!
//! 逻辑照 PCL-CE 的 `PageDownloadCompDetail`（`UpdateFilterResult` / `GetGroupedVersionName`）：
//! 先按「加载器 + 游戏版本」分组，再把版本号粒度自适应成小版本号 → Drop → 折叠远古版，
//! 卡片按版本号倒序、「其他/快照/远古」这类特殊组沉底，「已选版本」卡片永远置顶。

/// 与 api.MrVersion 同构：分组逻辑只读这些字段，写宽一点方便直接传 MrVersion。
export type FileVersion = {
  id: string;
  name: string;
  versionNumber: string;
  gameVersions: string[];
  loaders: string[];
  /// `client` / `server`：文件支持的端。两个都有就是双端，空数组表示来源没给这项数据。
  environments: string[];
  versionType: string;
  datePublished: string;
  downloads: number;
  fileName: string;
  fileUrl: string;
  fileSize: number;
};

export const OTHER_GROUP = "其他";
export const SNAPSHOT_GROUP = "快照版";
export const OLD_GROUP = "远古版";

export const LOADER_NAMES: Record<string, string> = {
  fabric: "Fabric",
  forge: "Forge",
  neoforge: "NeoForge",
  quilt: "Quilt",
  liteloader: "LiteLoader"
};

/// 这些标签是加载器，不属于「分类」。
const LOADER_TAGS = new Set(Object.keys(LOADER_NAMES));

export function loaderName(id: string): string {
  const key = id.toLowerCase();
  return LOADER_NAMES[key] ?? id;
}

/// 客户端/服务端筛选：`客户端` / `服务端` / `双端`。
export type EnvironmentFilter = "client" | "server" | "both";

export const ENVIRONMENT_LABELS: Record<EnvironmentFilter, string> = {
  client: "客户端",
  server: "服务端",
  both: "双端"
};

export function environmentLabel(filter: EnvironmentFilter): string {
  return ENVIRONMENT_LABELS[filter];
}

/// 文件是否支持这一端（`双端` 要求同时支持客户端与服务端）。
export function fileSupports(file: FileVersion, filter: EnvironmentFilter): boolean {
  const client = file.environments.includes("client");
  const server = file.environments.includes("server");
  if (filter === "client") return client;
  if (filter === "server") return server;
  return client && server;
}

/// 只认 `主版本.次版本[.补丁]`，其余（`26.3-pre1`、`1.20.1-rc1`）取前缀。
export function parseGameVersion(name: string): { major: number; minor: number; patch: number } | null {
  const match = /^(\d+)\.(\d+)(?:\.(\d+))?/.exec(name.trim());
  if (!match) return null;
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3] ?? 0)
  };
}

/// 版本号倒序比较（新的在前）。
export function compareVersionName(left: string, right: string): number {
  const a = parseGameVersion(left);
  const b = parseGameVersion(right);
  if (a && b) {
    if (a.major !== b.major) return b.major - a.major;
    if (a.minor !== b.minor) return b.minor - a.minor;
    if (a.patch !== b.patch) return b.patch - a.patch;
  }
  if (a) return -1;
  if (b) return 1;
  return right.localeCompare(left);
}

/// PCL 的 Drop：`1.20.4` → `1.20`，`26.3` 仍然是 `26.3`。
export function dropName(name: string): string {
  const parsed = parseGameVersion(name);
  if (!parsed) return name;
  return `${parsed.major}.${parsed.minor}`;
}

/// 快照（PCL 认版本名里带 `w`，例如 `26.3w01a`）。
export function isSnapshot(name: string): boolean {
  return /w\d|snapshot/i.test(name);
}

/// 折进「远古版」：1.20 以前（PCL 是 `VersionToDrop < 120`）。
export function isOldVersion(name: string): boolean {
  const parsed = parseGameVersion(name);
  if (!parsed) return false;
  return parsed.major === 1 && parsed.minor < 20;
}

export type GroupFlags = { drop: boolean; foldOld: boolean };

/// 分组用的版本名（`GetGroupedVersionName`）。
export function groupedName(name: string, flags: GroupFlags): string {
  if (!name) return OTHER_GROUP;
  if (isSnapshot(name)) return SNAPSHOT_GROUP;
  if (flags.foldOld && isOldVersion(name)) return OLD_GROUP;
  return flags.drop ? dropName(name) : name;
}

/// chips 的粒度自适应：小版本号 → Drop → 折叠远古 → 两者叠加，每级都以「≥ 9 个」为界。
export function computeFlags(gameVersions: string[]): GroupFlags {
  const count = (flags: GroupFlags) =>
    new Set(gameVersions.map((name) => groupedName(name, flags))).size;
  const fallback: GroupFlags = { drop: false, foldOld: false };
  if (count(fallback) < 9) return fallback;
  const dropOnly: GroupFlags = { drop: true, foldOld: false };
  if (count(dropOnly) < 9) return dropOnly;
  const oldOnly: GroupFlags = { drop: false, foldOld: true };
  if (count(oldOnly) < 9) return oldOnly;
  return { drop: true, foldOld: true };
}

/// 三轴筛选项：版本、加载器倒序；`hasEnvironments` 表示这批文件有没有客户端/服务端数据。
export function buildFilters(files: FileVersion[], flags: GroupFlags) {
  const gameVersions = [...new Set(files.flatMap((file) => file.gameVersions))]
    .map((name) => groupedName(name, flags))
    .filter((name, index, list) => list.indexOf(name) === index)
    .sort(compareVersionName);
  const loaders = [...new Set(files.flatMap((file) => file.loaders))]
    .map(loaderName)
    .filter((name, index, list) => list.indexOf(name) === index)
    .sort((left, right) => right.localeCompare(left));
  const hasEnvironments = files.some((file) => file.environments.length > 0);
  return { gameVersions, loaders, hasEnvironments };
}

export type GroupOptions = {
  flags: GroupFlags;
  /// 右栏三轴筛选：版本（组名）、加载器、客户端/服务端。
  versionFilter: string | null;
  loaderFilter: string | null;
  environmentFilter: EnvironmentFilter | null;
  ignoreQuilt: boolean;
  /// 左栏选中的实例版本与加载器，用来生成置顶的「已选版本」卡片。
  instance: string | null;
  loader: string | null;
};

export type VersionGroup = { title: string; version: string; files: FileVersion[]; special: boolean };

/// 组名里有没有版本号（`其他`/`快照版`/`远古版` 没有，排到最后）。
function isSpecial(version: string): boolean {
  return !version.includes(".");
}

/// 左栏（搜索面板）的筛选条件，用来拼「符合左栏筛选」的组标题。
export type LeftFilters = { game: string | null; loader: string | null };

/// 左栏标题：加载器在前、版本在后（`Forge 1.12.2`）；都是任意时返回 null。
export function leftFilterTitle(left: LeftFilters): string | null {
  const parts: string[] = [];
  if (left.loader) parts.push(left.loader);
  if (left.game) parts.push(left.game);
  return parts.length > 0 ? parts.join(" ") : null;
}

/// 左栏条件命中的文件（游戏版本按显示名比较，加载器按显示名比较）。
export function filesMatchingLeft(
  files: FileVersion[],
  left: LeftFilters,
  flags: GroupFlags
): FileVersion[] {
  if (!left.game && !left.loader) return [];
  return files.filter((file) => {
    if (
      left.game &&
      !file.gameVersions.some(
        (name) => name === left.game || groupedName(name, flags) === left.game
      )
    ) {
      return false;
    }
    if (left.loader && !file.loaders.some((loader) => loaderName(loader) === left.loader)) {
      return false;
    }
    return true;
  });
}

export function groupFiles(files: FileVersion[], options: GroupOptions) {
  const { flags, versionFilter, loaderFilter, environmentFilter, ignoreQuilt } = options;
  // 组名 = 右栏非默认筛选值（加载器 → 环境）+ 版本；版本只写一次。
  const prefixParts: string[] = [];
  if (loaderFilter) prefixParts.push(loaderFilter);
  if (environmentFilter) prefixParts.push(environmentLabel(environmentFilter));
  const prefix = prefixParts.length > 0 ? `${prefixParts.join(" ")} ` : "";
  const groups = new Map<string, VersionGroup>();
  const seen = new Map<string, Set<string>>();

  const push = (version: string, file: FileVersion) => {
    const title = prefix + version;
    let bucket = seen.get(title);
    if (!bucket) {
      bucket = new Set<string>();
      seen.set(title, bucket);
      groups.set(title, { title, version, files: [], special: isSpecial(version) });
    }
    if (bucket.has(file.id)) return;
    bucket.add(file.id);
    groups.get(title)!.files.push(file);
  };

  for (const file of files) {
    if (
      ignoreQuilt &&
      file.loaders.length > 0 &&
      file.loaders.every((loader) => loader.toLowerCase() === "quilt")
    ) {
      continue;
    }
    if (loaderFilter && !file.loaders.some((loader) => loaderName(loader) === loaderFilter)) {
      continue;
    }
    if (environmentFilter && !fileSupports(file, environmentFilter)) continue;
    for (const gameVersion of file.gameVersions) {
      const grouped = groupedName(gameVersion, flags);
      if (versionFilter && grouped !== versionFilter) continue;
      push(grouped, file);
    }
  }

  const list: VersionGroup[] = [...groups.values()].sort((left, right) => {
    if (left.special !== right.special) return left.special ? 1 : -1;
    if (left.special) return left.title.localeCompare(right.title);
    const byVersion = compareVersionName(left.version, right.version);
    return byVersion !== 0 ? byVersion : right.title.localeCompare(left.title);
  });

  let selected: VersionGroup | null = null;
  if (options.instance) {
    const matched = files.filter((file) => {
      if (!file.gameVersions.includes(options.instance!)) return false;
      if (options.loader && !file.loaders.some((loader) => loaderName(loader) === options.loader)) {
        return false;
      }
      if (!versionFilter) return true;
      return file.gameVersions.some(
        (name) => groupedName(name, flags) === versionFilter
      );
    });
    if (matched.length > 0) {
      const prefix = options.loader ? `${options.loader} ` : "";
      selected = {
        title: `已选版本：${prefix}${options.instance}`,
        version: options.instance,
        files: matched,
        special: false
      };
    }
  }

  return { groups: list, selected };
}

/// 分类 chip 用的标签：去掉加载器，最多留三个。
export function categoryTags(categories: string[]): string[] {
  return categories
    .filter((tag) => !LOADER_TAGS.has(tag.toLowerCase()))
    .slice(0, 3);
}
