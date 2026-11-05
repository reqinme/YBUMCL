/**
 * 安装进度 → 任务卡片的翻译，以及后端事件的订阅。
 *
 * 订阅放在模块里而不是页面里：安装是从「下载」页发起的，任务卡片却常在「下载任务」
 * 页看。页面一卸载，页面自己的监听就断了，卡片会停在最后一帧，所以这里只记「当前
 * 是哪张卡片在装」，事件一直收着。
 */

import { onInstallProgress, type InstallProgress } from "$lib/api";
import { setTaskLive, stopState, type TaskLive } from "$lib/tasks.svelte";

/// 后端阶段名 → 界面上那一行字。
export const STAGE_TEXT: Record<string, string> = {
  manifest: "读取版本清单",
  metadata: "下载版本元数据",
  "asset-index": "下载资源索引",
  downloading: "下载游戏文件",
  "fabric-profile": "写入 Fabric 配置",
  "fabric-checksum": "校验 Fabric 元数据",
  "fabric-libraries": "下载 Fabric 库",
  "instance-vanilla": "准备原版文件",
  "instance-loader": "安装加载器",
  "instance-merge": "合并成实例",
  "quilt-profile": "写入 Quilt 配置",
  "quilt-checksum": "校验 Quilt 元数据",
  "quilt-libraries": "下载 Quilt 库",
  forge: "运行 Forge 安装器",
  "modloader-installer": "下载安装器",
  "modloader-prepare": "准备游戏文件",
  "modloader-libraries": "下载加载器库",
  "modloader-install": "运行加载器安装器",
  "modloader-verify": "校验加载器文件",
  "modloader-publish": "写入版本配置",
  complete: "完成",
  cancelled: "已取消",
  failed: "失败"
};

/// 正在更新的任务卡；null 表示没有在装的。
let current: number | null = null;
/// 瞬时速度的采样窗口（事件很密，隔不到 350ms 就不重算）。
let sample = { at: 0, bytes: 0, rate: 0 };
let subscribed = false;

function formatBytes(bytes: number): string {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const index = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function noteProgress(event: InstallProgress) {
  const now = Date.now();
  const previous = sample;
  if (!previous.at || event.downloadedBytes < previous.bytes) {
    sample = { at: now, bytes: event.downloadedBytes, rate: 0 };
    return;
  }
  const elapsed = now - previous.at;
  if (elapsed < 350) return;
  const rate = ((event.downloadedBytes - previous.bytes) * 1000) / elapsed;
  // 滑动平均，避免单次抖动让数字乱跳
  sample = {
    at: now,
    bytes: event.downloadedBytes,
    rate: previous.rate ? previous.rate * 0.6 + rate * 0.4 : rate
  };
}

/// 把一次进度事件翻成任务卡片上的实时状态（进度条 + 一行明细 + 当前文件）。
export function liveFrom(event: InstallProgress): TaskLive {
  // 后端按 PCL 的分段权重算好的百分比优先；没算（null）才退回按字节估。
  const percent =
    event.percent != null
      ? Math.min(100, event.percent)
      : event.totalBytes > 0
        ? Math.min(100, (event.downloadedBytes / event.totalBytes) * 100)
        : event.totalFiles > 0
          ? Math.min(100, (event.completedFiles / event.totalFiles) * 100)
          : 0;
  const parts = [
    `${event.completedFiles}/${event.totalFiles} 文件`,
    `${formatBytes(event.downloadedBytes)} / ${formatBytes(event.totalBytes)}`
  ];
  if (sample.rate > 0) {
    parts.push(`${formatBytes(sample.rate)}/s`);
    const left = event.totalBytes - event.downloadedBytes;
    if (left > 0) {
      const seconds = left / sample.rate;
      if (seconds < 24 * 3600) {
        parts.push(
          seconds < 60
            ? `剩余约 ${Math.max(1, Math.round(seconds))} 秒`
            : seconds < 3600
              ? `剩余约 ${Math.round(seconds / 60)} 分钟`
              : `剩余约 ${(seconds / 3600).toFixed(1)} 小时`
        );
      }
    }
  }
  // 加载器阶段是「一个个文件/一步补丁」在跑，带上完成数，别只写一句「安装中」。
  const stageBase = STAGE_TEXT[event.stage] ?? event.stage;
  const stageCount =
    event.totalFiles > 0 && event.stage !== "complete"
      ? ` ${event.completedFiles}/${event.totalFiles}`
      : "";
  return {
    percent,
    stage:
      stopState.mode === "pause" && event.stage === "cancelled"
        ? "已暂停"
        : `${stageBase}${stageCount}`,
    detail: parts.join(" · "),
    path: event.path ?? ""
  };
}

/// 开始一次安装：之后的进度事件都写到这张卡片上，直到 `untrackInstall`。
export function trackInstall(taskId: number) {
  current = taskId;
  sample = { at: 0, bytes: 0, rate: 0 };
  if (subscribed) return;
  subscribed = true;
  void onInstallProgress((event) => {
    noteProgress(event);
    if (current !== null) setTaskLive(current, liveFrom(event));
  });
}

export function untrackInstall() {
  current = null;
}
