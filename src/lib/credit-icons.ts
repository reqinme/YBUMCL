/// 鸣谢列表用的图标：编译进前端包，跟着 exe 走，不联网。
const modules = import.meta.glob("./assets/credits/*.png", {
  eager: true,
  query: "?url",
  import: "default"
}) as Record<string, string>;

const byKey: Record<string, string> = {};
for (const [path, url] of Object.entries(modules)) {
  const name = path.split("/").pop()?.replace(/\.png$/, "") ?? "";
  if (name) byKey[name] = url;
}

export function creditIcon(key: string): string | null {
  return byKey[key] ?? null;
}
