/**
 * 「这个实例正在启动」的状态。
 *
 * 从点下启动到游戏窗口出现之前，按钮锁着并显示一条从左往右走的进度线；
 * 窗口一出现（后端 `game-ready`）就解锁，于是可以再开第二个、第三个同一实例。
 */
import { onGameCrash, onGameExit, onGameReady } from "$lib/api";

/// 正在启动的实例 id → 进度（0–100）。
let progress = $state<Record<string, number>>({});
let timer: ReturnType<typeof setInterval> | null = null;

function tick() {
  const ids = Object.keys(progress);
  if (ids.length === 0) {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
    return;
  }
  for (const id of ids) {
    const value = progress[id];
    if (value == null || value >= 95) continue;
    // 前面几步（读设置、选 Java、拼命令行）是瞬间的，这里让线自己慢慢爬；
    // 真到窗口出现之前最多爬到 95%，不会假装已经启动完。
    progress[id] = Math.min(95, value + Math.max(0.6, (95 - value) * 0.035));
  }
}

export function markLaunching(id: string) {
  progress[id] = 6;
  if (!timer) timer = setInterval(tick, 220);
}

export function clearLaunching(id: string) {
  if (progress[id] == null) return;
  progress[id] = 100;
  setTimeout(() => {
    delete progress[id];
  }, 420);
}

/// 启动本身失败（还没起进程）：立刻把线撤掉，不留一个假的「启动中」。
export function failLaunching(id: string) {
  delete progress[id];
}

export function launchProgress(id: string): number | null {
  return progress[id] ?? null;
}

export function isLaunching(id: string): boolean {
  return progress[id] != null;
}

/// 事件统一在这里订阅一次：窗口出现、进程退出、崩溃都算这次启动结束。
export async function watchLaunchEvents(): Promise<() => void> {
  const stops = await Promise.all([
    onGameReady((event) => clearLaunching(event.versionId)),
    onGameExit((event) => clearLaunching(event.versionId)),
    onGameCrash((event) => clearLaunching(event.versionId)),
  ]);
  return () => stops.forEach((stop) => stop());
}
