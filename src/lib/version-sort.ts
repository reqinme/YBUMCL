/**
 * 版本列表的排序规则。版本页和主页左栏共用。
 *
 * 排序方式按目录分别记在 `categories.json` 的 `order` 上（和星标同一份数据），
 * 这里只管「怎么比」，不碰存储。
 */

export type SortMode = "default" | "name" | "version" | "loader" | "recent";

export const SORT_OPTIONS: { value: SortMode; label: string }[] = [
  { value: "default", label: "默认顺序" },
  { value: "name", label: "按名称" },
  { value: "version", label: "按版本" },
  { value: "loader", label: "按加载器" },
  { value: "recent", label: "最近启动" }
];

export function parseSortMode(value: string | null | undefined): SortMode {
  return SORT_OPTIONS.some((option) => option.value === value) ? (value as SortMode) : "default";
}

/** 排序只需要这几个字段，所以不要求传完整的 VersionView。 */
export type Sortable = {
  id: string;
  gameVersion?: string | null;
  loaders?: string[];
  installedAt?: number | null;
  lastStarted?: string | null;
};

/**
 * 字符分类：数字 0、英文 1、中文 2、符号开头 3、其它 4。
 *
 * 需求是「数字 < 英文 < 中文 < 符号开头 < 其它」，同组内再按值/字典序比。
 */
function kindOf(text: string): number {
  const first = text.trim().charAt(0);
  if (first === "") return 4;
  if (/[0-9]/.test(first)) return 0;
  if (/[A-Za-z]/.test(first)) return 1;
  if (/[\u4e00-\u9fff]/.test(first)) return 2;
  if (/[\p{P}\p{S}]/u.test(first)) return 3;
  return 4;
}

/** 名称里的任意一段数字：`1.21.2` 的 2 比 `1.21.11` 的 11 小。 */
function naturalCompare(left: string, right: string): number {
  const a = left.toLowerCase();
  const b = right.toLowerCase();
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    const digitA = a[i] >= "0" && a[i] <= "9";
    const digitB = b[j] >= "0" && b[j] <= "9";
    if (digitA && digitB) {
      let startA = i;
      let startB = j;
      while (i < a.length && a[i] >= "0" && a[i] <= "9") i += 1;
      while (j < b.length && b[j] >= "0" && b[j] <= "9") j += 1;
      const numberA = Number.parseInt(a.slice(startA, i), 10);
      const numberB = Number.parseInt(b.slice(startB, j), 10);
      if (numberA !== numberB) return numberA - numberB;
      continue;
    }
    if (a[i] !== b[j]) return a[i] < b[j] ? -1 : 1;
    i += 1;
    j += 1;
  }
  if (a.length !== b.length) return a.length - b.length;
  return left === right ? 0 : left < right ? -1 : 1;
}

/** 名称比较：先按首字符分类，同类再按自然序 / 中文拼音。 */
export function compareName(left: string, right: string): number {
  const kindLeft = kindOf(left);
  const kindRight = kindOf(right);
  if (kindLeft !== kindRight) return kindLeft - kindRight;
  if (kindLeft === 2) {
    // 中文按拼音：浏览器自带的排序规则就够，不用另带一张拼音表。
    return left.localeCompare(right, "zh-Hans-CN");
  }
  return naturalCompare(left, right);
}

/** 游戏版本降序：`1.21.11` 排在 `1.21.2` 前面；认不出时按名称。 */
export function compareGameVersion(left: string, right: string): number {
  const split = (value: string) => value.split(/[^0-9]+/).filter((part) => part !== "").map(Number);
  const a = split(left);
  const b = split(right);
  const length = Math.max(a.length, b.length);
  for (let index = 0; index < length; index += 1) {
    const head = a[index] ?? -1;
    const tail = b[index] ?? -1;
    if (head !== tail) return tail - head;
  }
  return compareName(right, left);
}

/** 加载器分组顺序：原版最前，其次各家加载器，认不出的排最后。 */
const LOADER_RANK = ["Fabric", "Quilt", "Forge", "NeoForge", "Cleanroom", "LabyMod", "OptiFine"];

function loaderRank(version: Sortable): number {
  const loaders = (version.loaders ?? []).filter((loader) => loader !== "原版");
  // 没有加载器就是原版，排最前；只装了 OptiFine 的归「仅 OptiFine」那一档。
  if (loaders.length === 0) return 0;
  // Forge 与 OptiFine 一起装时归 Forge 那一档。
  if (loaders.includes("Forge")) return 3;
  for (let index = 0; index < LOADER_RANK.length; index += 1) {
    if (loaders.includes(LOADER_RANK[index])) return index + 1;
  }
  return LOADER_RANK.length + 1;
}

function installedOrder(version: Sortable): number {
  return version.installedAt ?? Number.MAX_SAFE_INTEGER;
}

function startedOrder(version: Sortable): string {
  // 没启动过排最后：空字符串在降序里天然靠后。
  return version.lastStarted ?? "";
}

/** 按给定方式比较两个版本。 */
export function compareVersions(mode: SortMode, left: Sortable, right: Sortable): number {
  switch (mode) {
    case "name":
      return compareName(left.id, right.id);
    case "version": {
      const order = compareGameVersion(left.gameVersion ?? left.id, right.gameVersion ?? right.id);
      if (order !== 0) return order;
      return compareName(left.id, right.id);
    }
    case "loader": {
      const order = loaderRank(left) - loaderRank(right);
      if (order !== 0) return order;
      return compareName(left.id, right.id);
    }
    case "recent": {
      const a = startedOrder(left);
      const b = startedOrder(right);
      if (a !== b) return a < b ? 1 : -1;
      return compareName(left.id, right.id);
    }
    case "default":
    default: {
      const order = installedOrder(left) - installedOrder(right);
      if (order !== 0) return order;
      return compareName(left.id, right.id);
    }
  }
}

/**
 * 排序并把打了星的置顶。置顶组内部也用同一套排序键。
 *
 * `starred` 由调用方给（星标按目录记，排序函数不关心它存哪）。
 */
export function sortVersions<T extends Sortable>(
  list: readonly T[],
  mode: SortMode,
  starred: (id: string) => boolean
): T[] {
  const pinned: T[] = [];
  const rest: T[] = [];
  for (const version of list) (starred(version.id) ? pinned : rest).push(version);
  pinned.sort((left, right) => compareVersions(mode, left, right));
  rest.sort((left, right) => compareVersions(mode, left, right));
  return [...pinned, ...rest];
}
