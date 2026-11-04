/// 版本页左栏的「游戏文件夹」列表。
///
/// 列表本身由后端扫出来（当前文件夹、官方启动器文件夹、用户添加的自建目录），
/// 这里只负责：读一次、切换、添加、移出、改名、拖拽排序。
/// 当前选中的目录存在 `settings.json` 里，重启之后还是它。
import { api, type GameFolder } from "$lib/api";
import { announce, activeCount } from "$lib/tasks.svelte";
import { forgetScan } from "$lib/detect";

export const folders = $state<GameFolder[]>([]);
/// 标量放进一个对象：模块级 `$state` 的 const 不能在函数里重新赋值。
export const folderState = $state({
  active: "",
  loading: true,
  error: "",
  /// 本次运行里手动「移出」的自动发现目录：不落盘，下次启动自然回来。
  hidden: [] as string[]
});

export function visibleFolders(): GameFolder[] {
  return folders.filter((folder) => !folderState.hidden.includes(folder.path));
}

/// 有任务在跑的时候不许换目录：正在写文件的那些任务会落到别的目录里。
export function blockedByTask(): boolean {
  if (activeCount() > 0) {
    announce("有任务在进行中，先等它结束");
    return true;
  }
  return false;
}

function adopt(list: GameFolder[]): void {
  folders.length = 0;
  folders.push(...list);
  folderState.active = list.find((folder) => folder.active)?.path ?? list[0]?.path ?? "";
  folderState.loading = false;
  folderState.error = "";
}

export async function loadFolders(): Promise<void> {
  try {
    adopt(await api.listGameFolders());
  } catch (e) {
    folderState.error = String(e);
    folderState.loading = false;
  }
}

/// 切换当前目录；返回 true 表示真的换了（调用方据此重扫版本）。
export async function selectFolder(path: string): Promise<boolean> {
  if (path === folderState.active) return false;
  if (blockedByTask()) return false;
  try {
    adopt(await api.setActiveGameFolder(path));
    return true;
  } catch (e) {
    announce(`切换失败：${String(e)}`);
    return false;
  }
}

/// 选一个文件夹加进列表并切过去。
export async function addFolder(): Promise<boolean> {
  if (blockedByTask()) return false;
  try {
    const picked = await api.pickGameFolder();
    if (!picked) return false;
    adopt(await api.addGameFolder(picked));
    return true;
  } catch (e) {
    announce(String(e));
    return false;
  }
}

/// 移出列表：自建目录记进设置（真的删掉这一项）；自动扫出来的只在本进程里藏起来。
export async function removeFolder(path: string): Promise<boolean> {
  if (blockedByTask()) return false;
  const folder = folders.find((item) => item.path === path);
  if (!folder) return false;
  try {
    if (folder.kind === "custom") {
      adopt(await api.removeGameFolder(path));
      return true;
    }
    folderState.hidden.push(path);
    if (path === folderState.active) {
      const next = visibleFolders()[0];
      if (next) adopt(await api.setActiveGameFolder(next.path));
    }
    return true;
  } catch (e) {
    announce(String(e));
    return false;
  }
}

/// 改名；传空串等于复原。
export async function renameFolder(path: string, name: string): Promise<boolean> {
  try {
    adopt(await api.renameGameFolder(path, name));
    return true;
  } catch (e) {
    announce(String(e));
    return false;
  }
}

/// 拖拽结束后按新顺序落盘；顺带本地重排，界面不用等后端回话。
export async function reorderFolders(order: string[]): Promise<void> {
  const byPath = new Map(folders.map((folder) => [folder.path, folder]));
  const next = order.map((path) => byPath.get(path)).filter((item): item is GameFolder => !!item);
  if (next.length === folders.length) {
    folders.length = 0;
    folders.push(...next);
  }
  try {
    await api.setFolderOrder(order);
  } catch (e) {
    announce(`保存顺序失败：${String(e)}`);
  }
}

export function activeFolder(): GameFolder | null {
  return folders.find((folder) => folder.path === folderState.active) ?? null;
}

/// 「刷新」：把这个目录的扫描缓存作废，再重新读列表。
export async function refreshScan(): Promise<void> {
  if (folderState.active) forgetScan(folderState.active);
  await loadFolders();
}
