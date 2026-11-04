/// 下载页需要在「离开再回来」之后保持的界面状态。
///
/// PCL 的下载页是常驻的 UserControl，切页只改可见性，所以状态天然还在；
/// SvelteKit 的组件寿命等于路由寿命，离开 `/downloads` 就会销毁。
/// 这里把这几个字段寄存在模块作用域（同一次运行内不会被销毁），
/// 并同步一份到 localStorage（能扛住 WebView 整页重载）。
/// 一个内容分类自己的筛选状态（搜索词、来源、排序、游戏版本、加载器、支持端、标签、页码）。
/// 各分类各存一份：在整合包搜过的词，切到世界页不该还带着。
export type CatFilters = {
  query: string;
  source: string;
  sort: string;
  game: string;
  loader: string;
  env: string;
  tag: string;
  page: number;
};

/// 详情要用的项目字段。
/// 实例页从整合包身份卡跳过来时带全量，这样即便列表第一页没这个项目，
/// 详情也能立刻摆出来（列表里找得到时用列表那份，字段更全）。
export type ProjectRef = { id: string; title: string; slug: string } & Partial<{
  description: string;
  iconUrl: string | null;
  downloads: number;
  author: string;
  categories: string[];
  projectType: string;
  loaders: string[];
  gameVersions: string[];
  chineseTitle: string | null;
}>;

export type DownloadsState = {
  /// 当前分类（all / mods / packs / packages / favorites …）。
  cat: string;
  /// 搜索框内容。
  query: string;
  /// 内容来源筛选。
  source: string;
  /// 排序方式。
  sort: string;
  /// 加载器筛选。
  loader: string;
  /// 支持端筛选。
  env: string;
  /// 标签名（恢复时按名字在 TAGS 里查回对象）。
  tag: string;
  /// 正在看的项目；从实例页跳过来时会带全量字段。
  project: ProjectRef | null;
  /// 详情页里选中的实例版本与加载器筛选。
  instance: string | null;
  loaderFilter: string | null;
  /// 详情页里展开了哪些分组。
  expanded: string[];
  /// 左侧列表的滚动位置。
  scrollTop: number;
  /// 按分类存下来的筛选状态；上面的扁平字段是旧格式，读到时作为当前分类的兜底。
  filters?: Record<string, CatFilters>;
};

const KEY = "ybumcl.downloads.state";

function read(): DownloadsState | null {
  try {
    if (typeof localStorage === "undefined") return null;
    const raw = localStorage.getItem(KEY);
    return raw ? (JSON.parse(raw) as DownloadsState) : null;
  } catch {
    return null;
  }
}

/// 模块级状态：路由切换不会销毁它。
export const downloadsState = $state<{ value: DownloadsState | null }>({ value: read() });

export function saveDownloadsState(next: DownloadsState) {
  downloadsState.value = next;
  try {
    if (typeof localStorage !== "undefined") localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    /* 存不下就算了，别影响界面 */
  }
}

export function resetDownloadsState() {
  downloadsState.value = null;
  try {
    if (typeof localStorage !== "undefined") localStorage.removeItem(KEY);
  } catch {
    /* 同上 */
  }
}
