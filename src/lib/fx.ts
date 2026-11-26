/// 界面材质：动态（毛玻璃 + 动效）/ 静态（不透明面板）。
export type EffectsMode = "dynamic" | "static";

export const EFFECTS_OPTIONS = [
  { value: "dynamic", label: "动态" },
  { value: "static", label: "静态" }
];

/// 切换 <html data-fx>，同时镜像到 localStorage，供 app.html 在首帧前读取。
export function applyEffects(value: string): void {
  const mode: EffectsMode = value === "static" ? "static" : "dynamic";
  document.documentElement.dataset.fx = mode;
  try {
    localStorage.setItem("ybumcl.effects", mode);
  } catch {
    /* 隐私模式之类存不了就算了，只影响下次启动的首帧 */
  }
}
