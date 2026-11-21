/// 外层皮肤（帽子、外套、袖子、裤腿）按贴图逐像素挤出厚度。
///
/// skinview3d 的外层是标准 BoxGeometry，尺寸已经是「本体 + 1 格」，透明像素靠
/// alphaTest 丢掉 → 斜着看能看见壳的内壁，边缘像纸。这里把每个不透明像素
/// 单独挤出 0.5 格厚的小方块，只在邻居是空气的方向补侧面，做出真正的体素壳。
import type { SkinViewer } from "skinview3d";
import {
  BufferGeometry,
  Float32BufferAttribute,
  Mesh,
  Vector3,
  Vector2,
  type MeshStandardMaterial,
  type Object3D,
} from "three";

type AlphaMap = { width: number; height: number; data: Uint8Array };

const cache = new WeakMap<BufferGeometry, Map<string, BufferGeometry>>();

/// 把皮肤贴图读成 alpha 表（64×64；老皮肤只有上半，剩下当透明）。
function alphaOf(image: CanvasImageSource, width: number, height: number): AlphaMap {
  const canvas = document.createElement("canvas");
  canvas.width = 64;
  canvas.height = 64;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) return { width: 64, height: 64, data: new Uint8Array(64 * 64) };
  ctx.imageSmoothingEnabled = false;
  // 老版 64×32 皮肤直接贴到上半部分，下层保持透明。
  ctx.drawImage(image, 0, 0, width, Math.min(height, 32), 0, 0, 64, Math.min(32, 64));
  if (height > 32) ctx.drawImage(image, 0, 32, width, height - 32, 0, 32, 64, 64 - 32);
  const pixels = ctx.getImageData(0, 0, 64, 64).data;
  const out = new Uint8Array(64 * 64);
  for (let index = 0; index < 64 * 64; index += 1) {
    out[index] = pixels[index * 4 + 3];
  }
  return { width: 64, height: 64, data: out };
}

function solid(alpha: AlphaMap, texelX: number, texelY: number): boolean {
  const x = Math.floor(texelX);
  const y = Math.floor(texelY);
  if (x < 0 || y < 0 || x >= alpha.width || y >= alpha.height) return false;
  return alpha.data[y * alpha.width + x] > 8;
}

/// 一个面上的采样：由源几何的四条边推出「贴图格 → 3D 位置」的映射。
function voxelFace(
  source: BufferGeometry,
  face: number,
  alpha: AlphaMap,
  thickness: number,
  out: { positions: number[]; normals: number[]; uvs: number[]; indices: number[] },
) {
  const position = source.getAttribute("position");
  const uv = source.getAttribute("uv");
  if (!position || !uv) return;

  const base = face * 4;
  const corner = (index: number) =>
    new Vector3(position.getX(index), position.getY(index), position.getZ(index));
  const p0 = corner(base);
  const p1 = corner(base + 1);
  const p2 = corner(base + 2);
  const p3 = corner(base + 3);
  const t0 = new Vector2(uv.getX(base), uv.getY(base));
  const t1 = new Vector2(uv.getX(base + 1), uv.getY(base + 1));
  const t3 = new Vector2(uv.getX(base + 3), uv.getY(base + 3));

  const alongX = new Vector3().subVectors(p1, p0);
  const alongY = new Vector3().subVectors(p3, p0);
  const cols = Math.max(1, Math.round(alongX.length()));
  const rows = Math.max(1, Math.round(alongY.length()));
  const unitX = alongX.clone().divideScalar(cols);
  const unitY = alongY.clone().divideScalar(rows);

  // 贴图坐标的「一格」：源 uv 已经按 64 格铺好，除以格数即可。
  const stepU = (t1.x - t0.x) / cols;
  const stepV = (t3.y - t0.y) / rows;

  const normal = new Vector3().crossVectors(alongX, alongY).normalize();
  const faceCenter = new Vector3()
    .addVectors(p0, p2)
    .multiplyScalar(0.5);
  const partCenter = new Vector3();
  for (let index = 0; index < 24; index += 1) {
    partCenter.add(corner(index));
  }
  partCenter.divideScalar(24);
  if (normal.dot(faceCenter.clone().sub(partCenter)) < 0) normal.negate();

  const opaque: boolean[] = [];
  for (let row = 0; row < rows; row += 1) {
    for (let col = 0; col < cols; col += 1) {
      // 3D 位置 → 贴图像素：用 uv 起点推。
      const u = t0.x + stepU * (col + 0.5);
      const v = t0.y + stepV * (row + 0.5);
      opaque.push(solid(alpha, u * alpha.width, (1 - v) * alpha.height));
    }
  }
  if (!opaque.some(Boolean)) return;

  const at = (col: number, row: number, outward: number) =>
    new Vector3()
      .copy(p0)
      .addScaledVector(unitX, col)
      .addScaledVector(unitY, row)
      .addScaledVector(normal, outward);

  const push = (
    a: Vector3,
    b: Vector3,
    c: Vector3,
    d: Vector3,
    uvA: Vector2,
    uvB: Vector2,
    uvC: Vector2,
    uvD: Vector2,
  ) => {
    const base = out.positions.length / 3;
    for (const point of [a, b, c, d]) {
      out.positions.push(point.x, point.y, point.z);
      out.normals.push(normal.x, normal.y, normal.z);
    }
    out.uvs.push(uvA.x, uvA.y, uvB.x, uvB.y, uvC.x, uvC.y, uvD.x, uvD.y);
    out.indices.push(base, base + 1, base + 2, base, base + 2, base + 3);
  };

  const uvAt = (col: number, row: number) =>
    new Vector2(t0.x + stepU * col, t0.y + stepV * row);

  for (let row = 0; row < rows; row += 1) {
    for (let col = 0; col < cols; col += 1) {
      if (!opaque[row * cols + col]) continue;
      const x0 = at(col, row, thickness);
      const x1 = at(col + 1, row, thickness);
      const x2 = at(col + 1, row + 1, thickness);
      const x3 = at(col, row + 1, thickness);
      push(
        x0,
        x1,
        x2,
        x3,
        uvAt(col, row),
        uvAt(col + 1, row),
        uvAt(col + 1, row + 1),
        uvAt(col, row + 1),
      );

      // 四个方向：邻居是空气才补侧面，形成 0.5 格厚的边。
      const neighbours: [number, number, Vector3, Vector3][] = [
        [0, -1, x0, x1],
        [0, 1, x3, x2],
        [-1, 0, x0, x3],
        [1, 0, x1, x2],
      ];
      for (const [dx, dy, nearA, nearB] of neighbours) {
        const nc = col + dx;
        const nr = row + dy;
        const inside = nc >= 0 && nr >= 0 && nc < cols && nr < rows;
        if (inside && opaque[nr * cols + nc]) continue;
        const farA = nearA.clone().addScaledVector(normal, -thickness);
        const farB = nearB.clone().addScaledVector(normal, -thickness);
        const uvA = uvAt(col, row);
        const uvB = uvAt(col + 1, row);
        push(nearA, nearB, farB, farA, uvA, uvB, uvB, uvA);
      }
    }
  }
}

/// 生成一个部件的外层体素壳（像素单位，与源几何同一坐标系）。
export function voxelShell(source: BufferGeometry, image: CanvasImageSource): BufferGeometry {
  const width = (image as HTMLImageElement).naturalWidth ?? 64;
  const height = (image as HTMLImageElement).naturalHeight ?? 64;
  const alpha = alphaOf(image, width, height);
  // 手臂粗细切换会改同一份几何的 uv，所以缓存键里带上 uv 的指纹。
  const uv = source.getAttribute("uv");
  let fingerprint = 0;
  if (uv) {
    for (let index = 0; index < uv.count * 2; index += 1) {
      fingerprint = (fingerprint * 31 + Math.round(uv.array[index] * 4096)) | 0;
    }
  }
  const key = `${width}x${height}|${fingerprint}`;
  const perSource = cache.get(source) ?? new Map<string, BufferGeometry>();
  const hit = perSource.get(key);
  if (hit) return hit;

  const out = { positions: [] as number[], normals: [] as number[], uvs: [] as number[], indices: [] as number[] };
  for (let face = 0; face < 6; face += 1) {
    voxelFace(source, face, alpha, 0.5, out);
  }
  const shell = new BufferGeometry();
  shell.setAttribute("position", new Float32BufferAttribute(out.positions, 3));
  shell.setAttribute("normal", new Float32BufferAttribute(out.normals, 3));
  shell.setAttribute("uv", new Float32BufferAttribute(out.uvs, 2));
  shell.setIndex(out.indices);
  shell.computeBoundingSphere();
  perSource.set(key, shell);
  return shell;
}

/// 把模型上所有外层部件换成体素壳；返回是否真的换了。
export function applyVoxelLayers(viewer: SkinViewer): boolean {
  const { head, body, rightArm, leftArm, rightLeg, leftLeg } = viewer.playerObject.skin;
  let changed = false;
  const parts = [head, body, rightArm, leftArm, rightLeg, leftLeg] as unknown as Part[];
  for (const part of parts) {
    const mesh = part.outerLayer;
    const inner = part.innerLayer;
    if (!(mesh instanceof Mesh) || !(inner instanceof Mesh)) continue;
    const material = inner.material as MeshStandardMaterial;
    const image = material.map?.image as CanvasImageSource | undefined;
    if (!image) continue;
    mesh.geometry = voxelShell(mesh.geometry as BufferGeometry, image);
    changed = true;
  }
  return changed;
}

type Part = { innerLayer: Object3D; outerLayer: Object3D };
