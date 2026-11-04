// 全站只有一种「出错了」的展示：居中的错误弹窗。
// 页面里的报错别再写行内小字，统一调 showError()。

export type ErrorSource = "launcher" | "game";

export type ErrorInfo = {
  /// 一句话说清楚出了什么事，比如「这个项目没有匹配当前游戏版本/加载器的文件」。
  title: string;
  /// 可以更细一步的说明，会显示在标题下面。
  message?: string;
  /// 原始错误串（后端返回的、异常栈之类），折叠在「详细信息」里。
  detail?: string;
  /// 决定「打开日志」默认看哪一份：启动类错误看游戏日志，其它看启动器日志。
  source?: ErrorSource;
};

type Shown = { id: number; info: ErrorInfo };

let seq = 0;

export const errorDialog = $state<{ value: Shown | null }>({ value: null });

/** 弹一个错误弹窗；同一时间只留最新的一条。 */
export function showError(title: string, message?: string, detail?: string, source: ErrorSource = "launcher") {
  seq += 1;
  errorDialog.value = { id: seq, info: { title, message, detail, source } };
}

/** 关掉错误弹窗。 */
export function closeError() {
  errorDialog.value = null;
}
