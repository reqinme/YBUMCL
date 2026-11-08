/**
 * 后台下载任务的队列。
 *
 * 下载不占界面：点「下载」后任务进队列，任务按钮上的圆环跟着总进度走，
 * 每完成一个就弹一次「已保存」胶囊；失败的任务留在 `/tasks` 里等用户看。
 */

import { showError } from "$lib/dialogs.svelte";

export type TaskStatus = "queued" | "running" | "paused" | "done" | "failed";

/** 跑着的安装任务推到卡片上的实时进度：进度条 + 一行明细 + 当前文件。 */
export type TaskLive = {
  /** 0–100。 */
  percent: number;
  /** 阶段，比如「下载游戏文件」「安装加载器」。 */
  stage: string;
  /** 文件数 / 已下 / 总量 / 速度 / 剩余时间，已经拼成一行。 */
  detail: string;
  /** 当前正在下的那个文件。 */
  path: string;
};

/** 任务产出落在哪儿：卡片上的「打开文件夹」「移除并删除文件」都靠它。 */
export type TaskOutput = {
  path: string;
  /** 目录就选中目录本身，文件就选中文件本身。 */
  kind: "file" | "dir";
};

export type Task = {
  id: number;
  title: string;
  detail: string;
  status: TaskStatus;
  note: string;
  startedAt: number;
  finishedAt: number | null;
  /** 0–100，未知总量时停在 0（圆环改成转圈动画）。 */
  percent: number;
  /** 已下字节数，配合 `total` 在卡片上显示「1.2 MB / 3.4 MB」。 */
  received: number;
  /** 已知总字节数；null 表示服务端没给 Content-Length。 */
  total: number | null;
  /** 当前速度（字节/秒）；null 表示这一步没有速度可算（解压、搬文件）。 */
  speed: number | null;
  /** 安装这种自己管进度的任务，把实时状态挂在这里给列表渲染。 */
  live: TaskLive | null;
  /** 这次任务的落盘位置；null 表示没有可打开/可删的东西。 */
  output: TaskOutput | null;
  /** 走下载队列的任务才计入 `batch` 的总数（安装任务自己管并发）。 */
  queued: boolean;
};

/** `run` 的返回值：可以只给一句提示，也可以连产出路径一起给。 */
export type TaskResult = string | { note?: string; output?: TaskOutput | null } | void;

type Job = {
  id: number;
  run: () => Promise<TaskResult>;
};

let nextId = 1;

/// 最后一次点的是「暂停」还是「取消」。下载任务面板离下载页很远（它只拿到后端命令），
/// 不记下来，页面的 catch 就只能把暂停也当成取消，弹「半成品已清理」。
let lastStop: "pause" | "cancel" | null = null;

/// 这一轮安装是怎么被停的，也留给进度行的「阶段文案」用（`已暂停` / `已取消`）。
export const stopState = $state<{ mode: "pause" | "cancel" | null }>({ mode: null });

export function markStop(mode: "pause" | "cancel") {
  lastStop = mode;
  stopState.mode = mode;
}

export function clearStop() {
  lastStop = null;
  stopState.mode = null;
}

/** 读走（并清空）停止方式；页面 catch 里用它区分暂停与取消。 */
export function takeStop(): "pause" | "cancel" | null {
  const mode = lastStop;
  lastStop = null;
  return mode;
}

/** 所有任务（新的在前）。 */
export const tasks = $state<Task[]>([]);

/** 这一批下载的整体进度：`done` 个已完成 + 正在跑的按字节比例算。 */
export const batch = $state({ total: 0, done: 0 });

/** 最近一次「已保存」的提示，胶囊显示几秒后自动清空。 */
export const notice = $state<{ id: number; text: string; at: number }>({ id: 0, text: "", at: 0 });

const queue: Job[] = [];
/// 暂停的任务把 job 留着，点「继续」时重新排进队列。
const jobs = new Map<number, Job>();

let noticeTimer: ReturnType<typeof setTimeout> | null = null;

function find(id: number): Task | undefined {
  return tasks.find((task) => task.id === id);
}

let batchTimer: ReturnType<typeof setTimeout> | null = null;

function finishBatchTick() {
  if (batch.done < batch.total) return;
  if (batchTimer) clearTimeout(batchTimer);
  // 走满 100% 之后留一会儿再归零，让用户看见「跑完了」。
  batchTimer = setTimeout(() => {
    batch.total = 0;
    batch.done = 0;
  }, 900);
}

/// PCL 的做法：任务之间不设总数，点一个跑一个。
function pump() {
  while (queue.length > 0) {
    const job = queue.shift();
    if (!job) break;
    const task = find(job.id);
    if (!task) continue;
    task.status = "running";
    // 延后一拍再跑：调用方需要先拿到 task.id 才能把进度事件对上号。
    void Promise.resolve()
      .then(() => job.run())
      .then((result) => {
        // 暂停：卡片停在那儿等「继续」，既不算完成也不算失败。
        if (task.status === "paused") return;
        task.status = "done";
        task.percent = 100;
        if (typeof result === "string") {
          task.note = result || "完成";
        } else if (result && typeof result === "object") {
          task.note = result.note || "完成";
          task.output = result.output ?? null;
        } else {
          task.note = "完成";
        }
        showNotice(task.note);
      })
      .catch((error) => {
        // 暂停后后台还在收尾（旧任务被取消），错误回调可能比「已暂停」晚到：
        // 这时保持 paused，别把卡片改成「失败」。
        if (task.status === "paused") return;
        task.status = "failed";
        const text = String(error);
        // 取消/暂停不是「失败」：卡片和胶囊都说人话，别把后台的英文错误抛给用户。
        if (/取消|cancel/i.test(text)) {
          task.note = "已取消，半成品已清理";
          showNotice(task.note);
        } else {
          task.note = text;
          showError("任务失败", `「${task.title}」没有完成。`, text);
        }
      })
      .finally(() => {
        if (task.status !== "paused") {
          task.finishedAt = Date.now();
          batch.done += 1;
          finishBatchTick();
          jobs.delete(task.id);
        }
        pump();
      });
  }
}

/** 轻提示走灵动岛：复制 / 删除 / 刷新这类一句话反馈。 */
export function announce(text: string) {
  showNotice(text);
}

function showNotice(text: string) {
  notice.id += 1;
  notice.text = text;
  notice.at = Date.now();
  if (noticeTimer) clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => {
    notice.text = "";
  }, 3200);
}

/** 提交一个后台任务；`run` 返回的字符串会作为结果提示，也可以带出产出路径。 */
export function submitTask(
  title: string,
  detail: string,
  run: () => Promise<TaskResult>
): Task {
  const task: Task = {
    id: nextId++,
    title,
    detail,
    status: "queued",
    note: "",
    startedAt: Date.now(),
    finishedAt: null,
    percent: 0,
    received: 0,
    total: null,
    live: null,
    output: null,
    speed: null,
    queued: true
  };
  tasks.unshift(task);
  batch.total += 1;
  const job = { id: task.id, run };
  jobs.set(task.id, job);
  queue.push(job);
  pump();
  return task;
}

/** 后台推来的阶段性进度（整合包安装这种分好多步的任务），直接写进任务卡片。 */
export function setTaskNote(id: number, note: string) {
  const task = find(id);
  if (task && task.status === "running") task.note = note;
}

/** 记下这次任务的产出路径（`beginTask` 那条路自己管结束时机，所以单独给个入口）。 */
export function setTaskOutput(id: number, output: TaskOutput | null) {
  const task = find(id);
  if (task) task.output = output;
}

/** 下载器推过来的字节进度（按任务 id 对上号）。 */
export function setTaskProgress(
  id: number,
  received: number,
  total: number | null,
  speed?: number,
) {
  const task = find(id);
  if (!task) return;
  task.received = received;
  task.total = total;
  task.percent = total && total > 0 ? Math.min(99, (received / total) * 100) : 0;
  if (speed !== undefined) task.speed = speed > 0 ? speed : null;
}

/**
 * 后台已经算好整体百分比的任务（整合包安装按阶段加权）直接写这个数：
 * 再按字节重算会把「解压覆盖」这种没有字节的阶段打回 0。
 */
export function setTaskPercent(id: number, percent: number) {
  const task = find(id);
  if (!task || !Number.isFinite(percent)) return;
  task.percent = Math.max(0, Math.min(99.5, percent));
}

/** 只更新速度（安装阶段有速度但没有总量时需要它）。 */
export function setTaskSpeed(id: number, speed: number) {
  const task = find(id);
  if (!task) return;
  task.speed = speed > 0 ? speed : null;
}

/**
 * 不走下载队列的任务（安装版本这种自己管并发的）开一条记录，状态直接是「进行中」。
 * 返回任务 id，调用方拿它去推 `setTaskLive` / `endTask`。
 */
export function beginTask(title: string, detail = ""): number {
  clearStop();
  const task: Task = {
    id: nextId++,
    title,
    detail,
    status: "running",
    note: "",
    startedAt: Date.now(),
    finishedAt: null,
    percent: 0,
    received: 0,
    total: null,
    live: null,
    output: null,
    speed: null,
    queued: false
  };
  tasks.unshift(task);
  return task.id;
}

/** 把实时进度挂到任务卡片上；任务已经结束时忽略。 */
export function setTaskLive(id: number, live: TaskLive | null) {
  const task = find(id);
  if (!task || task.status !== "running") return;
  task.live = live;
  if (live) task.percent = live.percent;
}

/** 收尾：`failed` 为真时卡片标红，进度条留在原地当线索。 */
export function endTask(id: number, note: string, failed = false) {
  const task = find(id);
  if (!task) return;
  task.status = failed ? "failed" : "done";
  task.note = note;
  task.finishedAt = Date.now();
  if (!failed) {
    task.percent = 100;
    task.live = null;
  }
}

/// 每个任务「继续」时要做什么：安装任务重跑一遍同样的请求，排队任务重新入队。
const resumes = new Map<number, () => void>();

export function setTaskResume(id: number, run: (() => void) | null) {
  if (run) resumes.set(id, run);
  else resumes.delete(id);
}

/** 暂停：卡片出「继续」和「移除」两个按钮。 */
export function pauseTask(id: number, note: string) {
  const task = find(id);
  if (!task) return;
  task.status = "paused";
  task.note = note;
  task.finishedAt = Date.now();
  if (task.queued && batch.done < batch.total) batch.done += 1;
}

/** 把暂停的任务改回进行中（继续之前调用）。 */
export function resumeTask(id: number) {
  const task = find(id);
  if (!task) return;
  task.status = "running";
  task.note = "";
  task.finishedAt = null;
  if (task.queued && batch.done > 0) batch.done -= 1;
}

/** 点「继续」：交给任务自己登记的恢复动作。 */
export function continueTask(id: number) {
  const run = resumes.get(id);
  if (!run) return;
  run();
}

/** 排进队列的任务（整合包这种）继续：把同一条 job 重新排队。 */
export function requeueTask(id: number) {
  const task = find(id);
  const job = jobs.get(id);
  if (!task || !job || task.status !== "paused") return;
  resumeTask(id);
  queue.push(job);
  pump();
}

/** 任务被删掉 / 结束了，别留着它的恢复动作。 */
export function forgetResume(id: number) {
  resumes.delete(id);
  jobs.delete(id);
}

/** 0–100 的整体进度：已完成的任务算满格，正在跑的按字节比例。 */
export function overallProgress(): number {
  if (batch.total <= 0) return 0;
  let sum = batch.done;
  for (const task of tasks) {
    if (task.status === "running") sum += task.percent / 100;
  }
  return Math.min(100, (sum / batch.total) * 100);
}

/** 当前是否有下载在进行（决定圆环用不用转圈动画）。 */
export function activeCount(): number {
  return tasks.filter((task) => task.status === "running" || task.status === "queued").length;
}

export function clearFinished() {
  for (let index = tasks.length - 1; index >= 0; index -= 1) {
    if (tasks[index].status === "done" || tasks[index].status === "failed") {
      tasks.splice(index, 1);
    }
  }
}

export function removeTask(id: number) {
  const index = tasks.findIndex((task) => task.id === id);
  if (index >= 0 && tasks[index].status !== "running") {
    forgetResume(id);
    tasks.splice(index, 1);
  }
}
