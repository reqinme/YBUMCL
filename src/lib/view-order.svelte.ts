/// 版本列表的排序方式：每个游戏目录各记一份，存在 `categories.json` 的 `order` 上。
///
/// 版本页读它决定怎么排，主页左栏用同一份；切换目录时从该目录的分类快照里灌进来。
import { api } from "$lib/api";
import { parseSortMode, type SortMode } from "$lib/version-sort";

function key(dir: string): string {
  return dir.replace(/[\\/]+$/, "").trim().toLowerCase();
}

export const viewOrder = $state<{ map: Record<string, SortMode> }>({ map: {} });

/// 从分类快照里读这个目录的排序方式（没记录过就是默认顺序）。
export function absorbOrder(dir: string, order: string | undefined): void {
  const folder = key(dir);
  if (!folder) return;
  const mode = parseSortMode(order);
  if (viewOrder.map[folder] === mode) return;
  viewOrder.map = { ...viewOrder.map, [folder]: mode };
}

export function orderFor(dir: string): SortMode {
  return viewOrder.map[key(dir)] ?? "default";
}

export async function setOrder(dir: string, mode: SortMode): Promise<void> {
  const folder = key(dir);
  if (!folder) return;
  // 先动界面再落盘：排序是纯显示的事，存不下来也不该卡住列表。
  viewOrder.map = { ...viewOrder.map, [folder]: mode };
  try {
    await api.setVersionOrder(mode);
  } catch {
    /* 记不住不算错，下次进来还是默认顺序 */
  }
}
