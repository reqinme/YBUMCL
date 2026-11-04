/// 星标（「标星实例」）。
///
/// 数据在 `categories.json` 里：每个游戏目录各有一套分类，其中 id 为 `starred`
/// 的那条是内置分类「标星实例」，实例打了星就是它的 tag 里含 `starred`。
/// 所以星标跟着 categories.json 走，和分类是一套数据。
///
/// 老版本把星标存在 localStorage（`ybumcl.starred`）里，第一次进某个目录时
/// 并进 categories.json，并删掉那份旧数据。
import { api } from "$lib/api";

const LEGACY_KEY = "ybumcl.starred";
const STARRED = "starred";

type StarMap = Record<string, string[]>;

function key(dir: string): string {
  return dir.replace(/[\\/]+$/, "").trim().toLowerCase();
}

/// 目录 → 打了星的实例 id。页面拿到分类快照后调 `absorbCategories` 灌进来。
export const starredState = $state<{ map: StarMap }>({ map: {} });

const migrated = new Set<string>();

function write(dir: string, ids: string[]): void {
  const folder = key(dir);
  if (!folder) return;
  const map: StarMap = { ...starredState.map };
  if (ids.length > 0) map[folder] = ids;
  else delete map[folder];
  starredState.map = map;
}

/// 从一份分类快照里读出这个目录的星标（快照变了就跟着变）。
export function absorbCategories(dir: string, tags: Record<string, string[]> | undefined): void {
  const ids: string[] = [];
  for (const [id, tagsOfVersion] of Object.entries(tags ?? {})) {
    if (tagsOfVersion.includes(STARRED)) ids.push(id);
  }
  write(dir, ids);
  void migrateLegacy(dir);
}

export function starredIds(dir: string): string[] {
  return starredState.map[key(dir)] ?? [];
}

export function isStarred(dir: string, id: string): boolean {
  return starredIds(dir).includes(id);
}

/// 打星 / 取消打星：界面先乐观更新，写盘失败再回滚。
export function toggleStar(dir: string, id: string): boolean {
  const folder = key(dir);
  const on = !isStarred(dir, id);
  const before = starredIds(dir);
  write(dir, on ? [...before, id] : before.filter((item) => item !== id));
  void api
    .setStarred(id, on)
    .then((snapshot) => {
      absorbCategories(folder, snapshot.tags);
      migrated.add(folder);
    })
    .catch(() => write(dir, before));
  return on;
}

function readLegacy(): StarMap {
  try {
    const raw = localStorage.getItem(LEGACY_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return {};
    return parsed as StarMap;
  } catch {
    return {};
  }
}

/// 把旧版 localStorage 里这个目录的星标并进 categories.json，然后删掉旧键。
async function migrateLegacy(dir: string): Promise<void> {
  const folder = key(dir);
  if (!folder || migrated.has(folder)) return;
  migrated.add(folder);
  const legacy = readLegacy();
  const ids = Array.isArray(legacy[folder]) ? legacy[folder] : [];
  if (ids.length === 0) {
    if (Object.keys(legacy).length > 0) dropLegacyKey(folder);
    return;
  }
  try {
    const snapshot = await api.importStarred(ids);
    absorbCategories(dir, snapshot.tags);
    dropLegacyKey(folder);
  } catch {
    migrated.delete(folder);
  }
}

function dropLegacyKey(folder: string): void {
  try {
    const legacy = readLegacy();
    if (!(folder in legacy)) return;
    delete legacy[folder];
    if (Object.keys(legacy).length === 0) localStorage.removeItem(LEGACY_KEY);
    else localStorage.setItem(LEGACY_KEY, JSON.stringify(legacy));
  } catch {
    // localStorage 不可用就留着，不影响别的
  }
}
