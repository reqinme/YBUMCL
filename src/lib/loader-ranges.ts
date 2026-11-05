// 加载器的静态范围：版本一选定就先按它列出加载器，不等网络。
// 清单回来是空的，说明这个版本确实没有，再把那一项摘掉；范围表只是初筛。

export type LoaderKind =
  | "fabric"
  | "legacyfabric"
  | "labymod"
  | "quilt"
  | "forge"
  | "neoforge"
  | "cleanroom"
  | "liteloader"
  | "optifine";

export const LOADER_ORDER: LoaderKind[] = [
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

/// 互斥的主加载器；OptiFine 是补丁位，可以跟主加载器并存。
export const PRIMARY: LoaderKind[] = [
  "fabric",
  "legacyfabric",
  "labymod",
  "quilt",
  "forge",
  "neoforge",
  "cleanroom"
];

export const PATCH: LoaderKind = "optifine";

export const LOADER_NAME: Record<LoaderKind, string> = {
  fabric: "Fabric",
  legacyfabric: "Legacy Fabric",
  labymod: "LabyMod",
  quilt: "Quilt",
  forge: "Forge",
  neoforge: "NeoForge",
  cleanroom: "Cleanroom",
  liteloader: "LiteLoader",
  optifine: "OptiFine"
};

/// 自动拼实例名用的标签，与核心层 `merge::instance_name` 一致。
export const LOADER_TAG: Record<LoaderKind, string> = {
  fabric: "Fabric",
  legacyfabric: "LegacyFabric",
  labymod: "LabyMod",
  quilt: "Quilt",
  forge: "Forge",
  neoforge: "NeoForge",
  cleanroom: "Cleanroom",
  liteloader: "LiteLoader",
  optifine: "OptiFine"
};

export function tagOf(kind: LoaderKind, version: string): string {
  const tag = LOADER_TAG[kind];
  if (kind === "liteloader") return tag;
  if (kind === "labymod") {
    const [base, mode] = version.split("@");
    return mode === "snapshot" ? `${tag}_${base}_Snapshot` : `${tag}_${base}`;
  }
  return `${tag}_${version.replaceAll(" ", "_")}`;
}

/// 版本号按数字段比较（`47.4.28` > `47.4.9`），非数字段退化成字符串比较。
export function compareVersion(left: string, right: string): number {
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

/// 只认 `1.20.1` 这种正式版号；快照（`24w14a`）返回 null。
function releaseNumber(game: string): [number, number, number] | null {
  const match = /^(\d+)\.(\d+)(?:\.(\d+))?$/.exec(game.trim());
  if (!match) return null;
  return [Number(match[1]), Number(match[2]), Number(match[3] ?? 0)];
}

function atLeast(game: string, version: string): boolean {
  const one = releaseNumber(game);
  const two = releaseNumber(version);
  if (!one || !two) return true;
  return compareVersion(game, version) >= 0;
}

function atMost(game: string, version: string): boolean {
  const one = releaseNumber(game);
  const two = releaseNumber(version);
  if (!one || !two) return false;
  return compareVersion(game, version) <= 0;
}

/// 这个基础版本有没有可能提供这个加载器。只是初筛，最终以清单为准。
export function supportsLoader(kind: LoaderKind, game: string): boolean {
  if (!game) return false;
  const release = releaseNumber(game);
  switch (kind) {
    case "forge":
    case "optifine":
      return true;
    case "fabric":
      return release ? atLeast(game, "1.14") : true;
    case "quilt":
      return release ? atLeast(game, "1.17") : true;
    case "neoforge":
      return release ? atLeast(game, "1.20.1") : true;
    case "cleanroom":
      return game === "1.12.2";
    case "liteloader":
      return release ? atMost(game, "1.12.2") : false;
    case "legacyfabric":
      return release ? atLeast(game, "1.3.2") && atMost(game, "1.14.4") : false;
    case "labymod":
      if (!release) return true;
      if (game === "1.8.9" || game === "1.12.2") return true;
      return atLeast(game, "1.16.5");
  }
}

/** 加载器标签：只认实例 json 解析出来的记录；没有版本号的项不显示，全空就是原版。 */
export function loaderLabels(
  detail: { loader: string; version?: string }[] | null | undefined,
): string[] {
  const items = (detail ?? []).filter((item) => item && item.loader && item.version);
  if (items.length === 0) return ["原版"];
  return items.map((item) => {
    const label = LOADER_NAME[item.loader as LoaderKind] ?? item.loader;
    return `${label} ${item.version}`;
  });
}
