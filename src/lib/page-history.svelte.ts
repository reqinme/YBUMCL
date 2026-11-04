import { untrack } from "svelte";
import { goto } from "$app/navigation";

/// 页面级返回栈的第 3 步：跨页面返回。
///
/// 前两步（关掉下载任务面板、收起右侧详情）由各页面自己处理，Esc 处理完会
/// `preventDefault`；这里只负责「当前页已经没有东西可收」时退回上一个页面。
/// 栈放在模块里，`NavBar` 每次路由变化往里记一条。
const MAX_DEPTH = 40;

/// 走过的页面（末尾是当前页）。
export const pageStack = $state<{ paths: string[] }>({ paths: [] });

/// 记一条访问记录。
///
/// 同一个页面在栈里只留一份：再次进入它时把旧的那条删掉、挪到栈顶。否则
/// 在「下载」「版本」之间来回点，栈会变成 下载,版本,下载,版本…，返回键就
/// 在这两页之间来回跳（PCL 是一路退回主页，不乒乓）。
///
/// 注意：整个读写都包在 `untrack` 里。调用方是 `$effect`，如果这里读了
/// `pageStack.paths`，这个 effect 就会订阅栈本身；`goBackPage` 出栈后 effect
/// 会被重新触发，又把当前页压回去——栈永远不缩短，表现就是「按了返回还在
/// 两页之间来回弹」。
export function rememberPage(path: string) {
  untrack(() => {
    const list = pageStack.paths;
    if (list[list.length - 1] === path) return;
    const withoutOld = list.filter((item) => item !== path);
    pageStack.paths = [...withoutOld, path].slice(-MAX_DEPTH);
  });
}

export function canGoBackPage(): boolean {
  return pageStack.paths.length > 1;
}

/// 退回上一个页面；成功返回 true，没有上一页返回 false。
export async function goBackPage(): Promise<boolean> {
  const list = untrack(() => pageStack.paths);
  if (list.length < 2) return false;
  const target = list[list.length - 2];
  // 先出栈再跳，跳转失败就把栈恢复回去，免得连点两次把人带到更早的页。
  untrack(() => {
    pageStack.paths = list.slice(0, -1);
  });
  try {
    await goto(target);
    return true;
  } catch {
    untrack(() => {
      pageStack.paths = list;
    });
    return false;
  }
}
