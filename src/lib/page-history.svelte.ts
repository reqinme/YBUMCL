import { untrack } from "svelte";
import { goto } from "$app/navigation";

/// 返回导航：按「层」走，不按访问顺序走。
///
/// 结构（层号由 `levelOf` 决定）：
///   0 主页
///   1 版本 / 下载 / 账户 / 日志 / 设置 / 工具 / 关于
///   2 版本设置（`/versions/<id>`，默认停在「概览」）
/// 特殊页：`/launch`、`/tasks` —— 记住从哪儿打开，返回时回到那里。
///
/// 跨 tab 不构成返回关系：从「下载」点顶栏「版本」，返回直接回主页。

export type Frame = { href: string; scrollY: number };

/// 当前层栈，`frames[0]` 是主页，末尾是当前页。
export const pageStack = $state<{ frames: Frame[] }>({ frames: [] });

/// 打开着的浮层 / 弹窗数量。有浮层时 Esc 先关浮层，返回键不可用。
export const overlays = $state<{ count: number }>({ count: 0 });

/// 特殊页的来源（`/launch`、`/tasks`）。
let origin: string | null = null;

/// 版本设置内部「当前分类」的返回处理，由该页面注册。
let detailBack: (() => boolean) | null = null;

const MAX_DEPTH = 6;

function levelOf(pathname: string): number {
  if (pathname === "/") return 0;
  if (pathname === "/launch" || pathname === "/tasks") return 1;
  if (/^\/versions\/[^/]+/.test(pathname)) return 2;
  return 1;
}

function isSpecial(pathname: string): boolean {
  return pathname === "/launch" || pathname === "/tasks";
}

/// 页面注册「版本设置里还能再退一层分类」的处理函数。
/// 返回 true 表示这次返回已被消费（只是从 Mod 切回概览）。
export function registerDetailBack(handler: (() => boolean) | null) {
  detailBack = handler;
}

/// 浮层开关：打开时 `pushOverlay()`，关闭时 `popOverlay()`。
export function pushOverlay() {
  overlays.count += 1;
}

export function popOverlay() {
  overlays.count = Math.max(0, overlays.count - 1);
}

/// 弹窗打开时返回键置灰。
export function backBlocked(): boolean {
  return overlays.count > 0;
}

/// 记一帧。
///
/// 只按「层」维护：进入更深的层就压栈，同级就替换栈顶，回到更浅的层就截断。
/// 这样「下载 → 版本 → 返回」不会退回下载，而是回主页。
export function rememberPage(href: string) {
  untrack(() => {
    const path = href.split("?")[0];
    const level = levelOf(path);
    // 特殊页（启动 / 任务）只是「临时离开」，不能留在栈里：
    // 否则从它们回到目标页之后，栈里会夹着一个已经过期的中间帧。
    const frames = pageStack.frames.filter((item) => !isSpecial(item.href.split("?")[0]));
    const last = frames[frames.length - 1];
    const frame: Frame = { href, scrollY: 0 };

    if (level === 0) {
      pageStack.frames = [frame];
      origin = null;
      return;
    }
    if (isSpecial(path)) {
      // 特殊页：来源就是「进入它之前的那个栈顶」；它自己只临时占一帧。
      if (last) origin = last.href;
      pageStack.frames = [...frames, frame].slice(-MAX_DEPTH);
      return;
    }
    if (!last) {
      // 深链直接进来：补出父级，保证返回键有去处。
      const home: Frame = { href: "/", scrollY: 0 };
      pageStack.frames =
        level === 2 ? [home, { href: "/versions", scrollY: 0 }, frame] : [home, frame];
      return;
    }
    const current = levelOf(last.href.split("?")[0]);
    if (level > current) {
      pageStack.frames = [...frames, frame].slice(-MAX_DEPTH);
      return;
    }
    if (level === current) {
      pageStack.frames = [...frames.slice(0, -1), frame];
      return;
    }
    // 回到更浅的层：截断到那一层。
    pageStack.frames = [...frames.slice(0, level), frame];
  });
}

export function canGoBackPage(): boolean {
  return pageStack.frames.length > 1;
}

/// 退一层；成功返回 true。
export async function goBackPage(): Promise<boolean> {
  if (backBlocked()) return false;
  const frames = untrack(() => pageStack.frames);
  const current = frames[frames.length - 1];
  if (!current) return false;

  const path = current.href.split("?")[0];
  // 版本设置里还在别的分类：先回「概览」。
  if (levelOf(path) === 2 && detailBack?.()) {
    return true;
  }
  // 特殊页：回到打开它的那一页。
  if (isSpecial(path)) {
    const target = origin ?? (path === "/tasks" ? "/" : "/versions");
    return (await jump(target)) !== null;
  }
  if (frames.length < 2) return false;
  const target = frames[frames.length - 2];
  const okay = (await jump(target.href)) !== null;
  if (okay) {
    untrack(() => {
      pageStack.frames = frames.slice(0, -1);
    });
  }
  return okay;
}

/// 跳转并在落地后恢复滚动位置；失败返回 null。
async function jump(href: string): Promise<true | null> {
  const savedScroll = window.scrollY;
  const frames = untrack(() => pageStack.frames);
  if (frames.length > 0) {
    const top = frames[frames.length - 1];
    top.scrollY = savedScroll;
  }
  if (href === location.pathname + location.search) {
    return true;
  }
  try {
    await goto(href, { noScroll: true });
    requestAnimationFrame(() => {
      const target = untrack(() => pageStack.frames);
      const frame = target[target.length - 1];
      window.scrollTo({ top: frame?.scrollY ?? 0 });
    });
    return true;
  } catch {
    return null;
  }
}
