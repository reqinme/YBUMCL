// 极简 toast：成功类提示一律走这里，几秒后自己淡出，不占版面。
export type ToastKind = "ok" | "info";

type Toast = { id: number; text: string; kind: ToastKind };

let seq = 0;
let item = null as Toast | null;
const listeners = new Set<(value: Toast | null) => void>();

function emit(next: Toast | null) {
  item = next;
  for (const listener of listeners) listener(next);
}

/** 订阅当前 toast（组件内部用）。 */
export function subscribeToast(run: (value: Toast | null) => void) {
  listeners.add(run);
  run(item);
  return () => listeners.delete(run);
}

let timer: ReturnType<typeof setTimeout> | undefined;

/** 弹一条 toast，默认 1.6 秒后淡出。 */
export function toast(text: string, kind: ToastKind = "ok", ms = 1600) {
  clearTimeout(timer);
  seq += 1;
  emit({ id: seq, text, kind });
  timer = setTimeout(() => emit(null), ms);
}
