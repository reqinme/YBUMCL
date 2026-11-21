/// 点「安装」后从按钮飞向顶栏下载圆环的那颗光球。
///
/// 这里只管状态（有哪些球在飞、当前颜色），真正的动画在
/// [`InstallFlight.svelte`](./components/InstallFlight.svelte) 里跑。

export type Flight = {
  id: number;
  /// 起点：按钮中心（视口坐标）。
  start: { x: number; y: number };
  /// 终点：顶栏下载圆环中心（视口坐标）。
  end: { x: number; y: number };
  /// 二次贝塞尔的控制点，往上拱一下，让路线像抛物线而不是直线。
  control: { x: number; y: number };
};

/// 默认主色取主题的亮绿，和按钮是同一套颜色。
const DEFAULT_COLOR = "#a8cc84";

export const installFlight = $state({
  flights: [] as Flight[],
  /// 一共落地了几颗球；圆环拿它播一下接收动画。
  landings: 0,
  color: DEFAULT_COLOR,
});

/// 设置里改了颜色就调它；非法值直接忽略，保持上一次的颜色。
export function setBallColor(color: string | null | undefined) {
  if (!color) return;
  if (!/^#[0-9a-fA-F]{6}$/.test(color)) return;
  installFlight.color = color.toLowerCase();
}

let nextId = 1;

/// 从按钮起飞一颗球。拿不到按钮或圆环的位置就悄悄跳过，不影响安装本身。
export function launchInstallFlight(source: DOMRect | null) {
  if (!source) return;
  const ring = document.querySelector<HTMLElement>("[data-download-ring]");
  if (!ring) return;
  const rect = ring.getBoundingClientRect();
  const start = { x: source.left + source.width / 2, y: source.top + source.height / 2 };
  const end = { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
  installFlight.flights.push({
    id: nextId++,
    start,
    end,
    // 控制点抬到两点上方，弧高约为水平距离的 18%（最少 70px，最多 160px）。
    control: {
      x: (start.x + end.x) / 2,
      y: Math.min(start.y, end.y) - Math.min(160, Math.max(70, Math.abs(end.x - start.x) * 0.18)),
    },
  });
}

/// 球撞进圆环之后清掉它。
export function landFlight(id: number) {
  installFlight.flights = installFlight.flights.filter((flight) => flight.id !== id);
  installFlight.landings += 1;
}
