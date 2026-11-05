/**
 * 「检测类」数据的缓存。
 *
 * 版本清单、Java 列表、版本扫描都属于「进去一次查一次」的探测，
 * 但它们在同一秒里不会变：缓存一份，页面之间来回切就不再等网络/不再扫盘。
 * 版本清单额外存进 localStorage：重启启动器后先拿旧清单把列表画出来，
 * 再在后台换成新的（见 downloads 页的 refreshManifestQuietly）。
 * 每个入口都接受 force（用户在界面上点「刷新」时就传 true）。
 */
import {
  api,
  type DownloadStrategy,
  type JavaInstallation,
  type RemoteVersionManifest,
  type VersionSummary
} from "$lib/api";

/**
 * 这份策略下要不要考虑镜像。
 * 「仅官方」永远不用；「竞速 / 镜像优先 / 仅镜像」都要镜像；
 * 「官方优先」也得把镜像当备用，所以同样算「用」。
 */
export function usesMirror(strategy: DownloadStrategy | undefined): boolean {
  return (strategy ?? "race") !== "official-only";
}

type Entry<T> = { at: number; value: T };

/** Mojang 只在发版时改清单，30 分钟足够新。 */
const MANIFEST_TTL = 30 * 60_000;
/** 用户可能刚装了新 Java，别缓存太久。 */
const JAVA_TTL = 5 * 60_000;
/** 版本扫描只是读目录，缓存一分钟防抖。 */
const SCAN_TTL = 60_000;

type StoredManifest = Entry<RemoteVersionManifest> & { strategy: DownloadStrategy };

let manifestCache: StoredManifest | null = null;

const MANIFEST_KEY = "ybumcl.manifest.v1";

/// 旧版缓存里存的是 `mirror: boolean`；换算成策略，读不出来的当没缓存。
function strategyOfStored(raw: unknown): DownloadStrategy | null {
  const legacy = raw as { strategy?: unknown; mirror?: unknown };
  if (typeof legacy?.strategy === "string") return legacy.strategy as DownloadStrategy;
  if (legacy?.mirror === true) return "race";
  if (legacy?.mirror === false) return "official-only";
  return null;
}

/// 读上次写进 localStorage 的清单；只用它先把界面画出来，新旧由调用方判断。
function storedManifest(): StoredManifest | null {
  try {
    const raw = localStorage.getItem(MANIFEST_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    const value = parsed?.value as RemoteVersionManifest | undefined;
    if (!value?.versions?.length) return null;
    const strategy = strategyOfStored(parsed);
    if (!strategy) return null;
    return { at: Number(parsed.at) || 0, value, strategy };
  } catch {
    return null;
  }
}

function storeManifest(entry: StoredManifest) {
  try {
    localStorage.setItem(MANIFEST_KEY, JSON.stringify(entry));
  } catch {
    /* 存不下就算了，内存里的那份仍然有效 */
  }
}
let javaCache: Entry<JavaInstallation[]> | null = null;
const scanCache = new Map<string, Entry<VersionSummary[]>>();

export type Cached<T> = { value: T; ageMs: number; fromCache: boolean };

function hit<T>(entry: Entry<T> | null | undefined, ttl: number): Cached<T> | null {
  if (!entry) return null;
  const age = Date.now() - entry.at;
  return age < ttl ? { value: entry.value, ageMs: age, fromCache: true } : null;
}

export async function cachedManifest(
  strategy: DownloadStrategy = "race",
  force = false
): Promise<Cached<RemoteVersionManifest>> {
  if (!force) {
    if (manifestCache?.strategy === strategy) {
      const cached = hit(manifestCache, MANIFEST_TTL);
      if (cached) return cached;
    }
    // 内存里没有（刚启动）就看上次存盘的：先画出来，过期了再由页面在后台换新。
    const stored = storedManifest();
    if (stored && stored.strategy === strategy) {
      manifestCache = stored;
      return { value: stored.value, ageMs: Date.now() - stored.at, fromCache: true };
    }
  }
  const value = await api.fetchRemoteVersions(strategy);
  const entry: StoredManifest = { at: Date.now(), value, strategy };
  manifestCache = entry;
  storeManifest(entry);
  return { value, ageMs: 0, fromCache: false };
}

export async function cachedJava(force = false): Promise<Cached<JavaInstallation[]>> {
  if (!force) {
    const cached = hit(javaCache, JAVA_TTL);
    if (cached) return cached;
  }
  const value = await api.detectJava(force);
  javaCache = { at: Date.now(), value };
  return { value, ageMs: 0, fromCache: false };
}

export async function cachedScan(
  gameDir: string,
  force = false
): Promise<Cached<VersionSummary[]>> {
  if (!force) {
    const cached = hit(scanCache.get(gameDir), SCAN_TTL);
    if (cached) return cached;
  }
  const value = await api.scanVersions(gameDir);
  scanCache.set(gameDir, { at: Date.now(), value });
  return { value, ageMs: 0, fromCache: false };
}

/** 手动作废：设置里改了数据目录、或用户点「刷新清单」。 */
export function forgetManifest(): void {
  manifestCache = null;
}

export function forgetJava(): void {
  javaCache = null;
}

export function forgetScan(gameDir?: string): void {
  if (gameDir) scanCache.delete(gameDir);
  else scanCache.clear();
}

/**
 * 启动预取：应用打开后稍等一会儿，在后台把清单和 Java 拉好。
 * 这样用户点「下载」时基本是秒开；失败就静默，不打扰谁。
 */
export function startPrefetch(): void {
  if (typeof window === "undefined") return;
  const run = async () => {
    try {
      const settings = await api.loadSettings();
      const gameDir = await api.defaultGameDir();
      await Promise.allSettled([
        cachedManifest(settings.downloadStrategy),
        // 首页就把版本扫描预热掉：点开「版本」页时列表直接出，不再转圈。
        cachedScan(gameDir),
        cachedJava()
      ]);
    } catch {
      /* 预取失败不影响任何功能 */
    }
  };
  // 别和窗口首帧、启动动画抢资源。
  setTimeout(() => void run(), 1800);
}
