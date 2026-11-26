/// 站点图标随前端一起打包：离线也能显示，不依赖任何图标接口。
/// 缺图标的站点由调用方回落到启动器 logo。
const files = import.meta.glob("./assets/sites/*.png", {
  eager: true,
  query: "?url",
  import: "default"
}) as Record<string, string>;

export function localSiteIcon(host: string): string | null {
  const suffix = `/${host}.png`;
  for (const [path, url] of Object.entries(files)) {
    if (path.endsWith(suffix)) return url;
  }
  return null;
}
