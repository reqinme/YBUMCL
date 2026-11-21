/// 顶栏/列表用的小头像：把皮肤的脸和内层（帽子层）叠在一起画出来。
///
/// 64×64 皮肤：脸在内层 (8,8) 起 8×8，外层在 (40,8) 起 8×8。
/// 画布 128 见方，脸占 96 居中（四周留 16 透明），外层再往外扩 `PAD` 像素，
/// 于是帽檐、耳朵这些不透明像素会伸出去，轮廓就是头本身的形状。
const SIZE = 128;
const FACE = 96;
const PAD = 8;

const cache = new Map<string, Promise<string | null>>();

function draw(skin: string): Promise<string | null> {
  return new Promise((resolve) => {
    if (typeof document === "undefined") {
      resolve(null);
      return;
    }
    const image = new Image();
    image.onerror = () => resolve(null);
    image.onload = () => {
      try {
        const canvas = document.createElement("canvas");
        canvas.width = SIZE;
        canvas.height = SIZE;
        const context = canvas.getContext("2d");
        if (!context) {
          resolve(null);
          return;
        }
        context.imageSmoothingEnabled = false;
        const offset = (SIZE - FACE) / 2;
        context.drawImage(image, 8, 8, 8, 8, offset, offset, FACE, FACE);
        context.drawImage(
          image,
          40,
          8,
          8,
          8,
          offset - PAD,
          offset - PAD,
          FACE + PAD * 2,
          FACE + PAD * 2,
        );
        resolve(canvas.toDataURL("image/png"));
      } catch {
        resolve(null);
      }
    };
    image.src = skin;
  });
}

/// 皮肤 → 头像 data URL；同一张皮肤只画一次。
export function headImage(skin: string): Promise<string | null> {
  const cached = cache.get(skin);
  if (cached) return cached;
  const task = draw(skin).catch(() => null);
  cache.set(skin, task);
  return task;
}
