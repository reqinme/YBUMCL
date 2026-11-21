/// 立体人物模型：交给 skinview3d（MIT），它内部用 three 画原版那套盒子、
/// 外层皮肤壳和披风。模块按需加载，避免打包时把 three 拖进首屏。
export type SkinLook = {
  skin: string | null;
  cape: string | null;
  /// `slim` 或 `classic`。
  model: string | null;
};

import { applyVoxelLayers } from "$lib/skin-voxel";

type Lib = typeof import("skinview3d");
let lib: Promise<Lib> | null = null;

function skinvue(): Promise<Lib> {
  lib ??= import("skinview3d");
  return lib;
}

function modelOf(model: string | null): "default" | "slim" {
  return model === "slim" ? "slim" : "default";
}

/// 卡片用的离屏渲染器：全局只有一个 WebGL 上下文，渲染完就出图。
let shared: import("skinview3d").SkinViewer | null = null;
const images = new Map<string, string>();
const pending = new Map<string, Promise<string | null>>();

function keyOf(look: SkinLook): string {
  return `${look.skin ?? ""}|${look.cape ?? ""}|${modelOf(look.model)}`;
}

async function sharedViewer(): Promise<import("skinview3d").SkinViewer> {
  if (shared) return shared;
  const { SkinViewer, IdleAnimation } = await skinvue();
  const canvas = document.createElement("canvas");
  shared = new SkinViewer({
    canvas,
    width: 240,
    height: 360,
    pixelRatio: 2,
    preserveDrawingBuffer: true,
    enableControls: false,
    animation: new IdleAnimation(),
    fov: 32,
    zoom: 1.02,
  });
  return shared;
}

/// 把一个人物渲染成 PNG data URL；同一套皮肤只渲染一次。
export function skinImage(look: SkinLook): Promise<string | null> {
  const key = keyOf(look);
  const cached = images.get(key);
  if (cached) return Promise.resolve(cached);
  const running = pending.get(key);
  if (running) return running;

  const task = (async () => {
    if (!look.skin) return null;
    const viewer = await sharedViewer();
    await viewer.loadSkin(look.skin, { model: modelOf(look.model) });
    if (look.cape) await viewer.loadCape(look.cape);
    else viewer.loadCape(null);
    // 等一帧，姿势才不是初始站姿。
    await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
    const url = viewer.canvas.toDataURL("image/png");
    images.set(key, url);
    return url;
  })()
    .catch(() => null)
    .finally(() => pending.delete(key));

  pending.set(key, task);
  return task;
}

/// 交互用的实例：拖动旋转、滚轮缩放、双击复位（skinview3d 自带 OrbitControls）。
export async function mountViewer(
  canvas: HTMLCanvasElement,
  look: SkinLook,
): Promise<import("skinview3d").SkinViewer> {
  const { SkinViewer, IdleAnimation } = await skinvue();
  const viewer = new SkinViewer({
    canvas,
    width: canvas.clientWidth || 240,
    height: canvas.clientHeight || 360,
    pixelRatio: "match-device",
    enableControls: true,
    animation: new IdleAnimation(),
    fov: 32,
    zoom: 1.02,
  });
  viewer.autoRotate = false;
  viewer.controls.enablePan = false;
  viewer.controls.minDistance = 20;
  viewer.controls.maxDistance = 120;
  await applyLook(viewer, look);
  return viewer;
}

export async function applyLook(
  viewer: import("skinview3d").SkinViewer,
  look: SkinLook,
): Promise<void> {
  if (!look.skin) {
    viewer.loadSkin(null);
  } else {
    await viewer.loadSkin(look.skin, { model: modelOf(look.model) });
  }
  if (look.cape) await viewer.loadCape(look.cape);
  else viewer.loadCape(null);
  // 外层皮肤换成逐像素挤出的体素壳（换皮肤/换手臂时都要重建）。
  try {
    applyVoxelLayers(viewer);
  } catch {
    // 贴图还没就绪就下次再说，不影响本体。
  }
  viewer.adjustCameraDistance();
}
