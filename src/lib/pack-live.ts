// 整合包安装的进度落盘：后台按阶段推事件，这里写成任务卡片的文字与进度。
//
// 以前这个监听挂在下载页的 `onMount` 里，一离开下载页（比如切到「下载任务」
// 看卡片）监听就没了，卡片会永远停在上一次事件上——看起来像「卡在 0%」。
// 现在放进布局：全程只订阅一次，事件按安装号（= 任务卡 id）各写各的卡。

import { onModpackProgress } from "$lib/api";
import {
  setTaskNote,
  setTaskPercent,
  setTaskProgress,
  setTaskSpeed
} from "$lib/tasks.svelte";

/// 当前这次安装的卡片 id。事件里带了 installId 就按它写，没带就写这张。
let current: number | null = null;
let subscribed = false;

function formatBytes(value: number): string {
  if (value >= 1024 * 1024 * 1024) return `${(value / 1024 / 1024 / 1024).toFixed(2)} GB`;
  if (value >= 1024 * 1024) return `${(value / 1024 / 1024).toFixed(1)} MB`;
  if (value >= 1024) return `${(value / 1024).toFixed(0)} KB`;
  return `${value} B`;
}

/// 在布局里调用一次：装包开始之前就订阅好，中途换页也不断。
export async function watchPackProgress() {
  if (subscribed) return;
  subscribed = true;
  await onModpackProgress((event) => {
    const taskId = event.installId ?? current;
    if (taskId === null) return;
    const parts = [event.stage];
    if (event.total > 1 || event.done > 0) parts.push(`${event.done}/${event.total}`);
    if (event.bytes > 0) parts.push(formatBytes(event.bytes));
    if (event.file) parts.push(event.file);
    setTaskNote(taskId, parts.join(" · "));
    // 只有知道总量时卡片才能画真进度条（否则是 indeterminate 的滑动条）：
    // 下包、下内容这两步都会带 totalBytes，装上就能看到「已下 / 总量」。
    const totalBytes = event.totalBytes ?? 0;
    if (totalBytes > 0 && event.bytes >= 0) {
      setTaskProgress(taskId, event.bytes, totalBytes, event.speed);
    } else if (event.speed) {
      setTaskSpeed(taskId, event.speed);
    }
    // 整体百分比由后端按阶段加权算好（解压覆盖、搬文件也有份），
    // 直接写它，进度条才不会在阶段边界上卡住或倒退。
    if (typeof event.overall === "number") {
      setTaskPercent(taskId, event.overall);
    }
  });
}

/// 开始一次整合包安装：之后的进度都写这张卡，直到 `untrackPack`。
export function trackPack(taskId: number) {
  current = taskId;
  void watchPackProgress();
}

export function untrackPack() {
  current = null;
}
