/// 内容页的「标签」表，语义与 PCL CE 的 `PageDownloadMod.xaml` 等文件一致：
/// `Tag="cfId/mrSlug"`，左边是 CurseForge 的 categoryId，右边是 Modrinth 的 categories facet。
/// 缺一侧表示该标签只对那一站生效（我们就只查那一站）。
export type TagOption = {
  /// 显示名（与 PCL 的中文译文一致）。
  name: string;
  /// CurseForge 的 categoryId。
  cf?: number;
  /// Modrinth 的 categories facet。
  mr?: string;
  /// 是否属于上一行的分组（界面上缩进显示）。
  child?: boolean;
};

export type ContentKind = "mods" | "packs" | "datapacks" | "resourcepacks" | "shaders" | "worlds";

export const TAGS: Record<ContentKind, TagOption[]> = {
  mods: [
    { name: "全部" },
    { name: "世界元素", cf: 406, mr: "worldgen" },
    { name: "生物群系", cf: 407, child: true },
    { name: "维度", cf: 410, child: true },
    { name: "矿物与资源", cf: 408, child: true },
    { name: "天然结构", cf: 409, child: true },
    { name: "科技", cf: 412, mr: "technology" },
    { name: "管道与物流", cf: 415, child: true },
    { name: "自动化", cf: 4843, child: true },
    { name: "能源", cf: 417, child: true },
    { name: "红石", cf: 4558, child: true },
    { name: "食物与烹饪", cf: 436, mr: "food" },
    { name: "农业", cf: 416, child: true },
    { name: "游戏机制", mr: "game-mechanics" },
    { name: "运输", cf: 414, mr: "transportation" },
    { name: "仓储", cf: 420, mr: "storage" },
    { name: "魔法", cf: 419, mr: "magic" },
    { name: "冒险", cf: 422, mr: "adventure" },
    { name: "装饰", cf: 424, mr: "decoration" },
    { name: "生物", cf: 411, mr: "mobs" },
    { name: "实用", cf: 5191, mr: "utility" },
    { name: "装备与工具", cf: 434, mr: "equipment" },
    { name: "创造模式", cf: 9026 },
    { name: "性能优化", cf: 6814, mr: "optimization" },
    { name: "信息显示", cf: 423 },
    { name: "多人", cf: 435, mr: "social" },
    { name: "支持库", cf: 421, mr: "library" },
  ],
  packs: [
    { name: "全部" },
    { name: "冒险与 RPG", cf: 4475, mr: "adventure" },
    { name: "科技", cf: 4478, mr: "technology" },
    { name: "魔法", cf: 4484, mr: "magic" },
    { name: "天空岛", cf: 4736 },
    { name: "硬核", cf: 4476, mr: "hardcore" },
    { name: "小游戏", cf: 4485, mr: "minigame" },
    { name: "探索", mr: "exploration" },
    { name: "任务", mr: "quests" },
    { name: "多人", mr: "multiplayer" },
    { name: "挑战", mr: "challenging" }
  ],
  datapacks: [
    { name: "全部" },
    { name: "世界元素", mr: "worldgen" },
    { name: "科技", cf: 6951, mr: "technology" },
    { name: "游戏机制", mr: "game-mechanics" },
    { name: "运输", mr: "transportation" },
    { name: "仓储", mr: "storage" },
    { name: "魔法", cf: 6952, mr: "magic" },
    { name: "冒险", cf: 6948, mr: "adventure" },
    { name: "幻想", cf: 6949 },
    { name: "装饰", mr: "decoration" },
    { name: "生物", mr: "mobs" },
    { name: "实用", cf: 6953, mr: "utility" },
    { name: "装备与工具", mr: "equipment" },
    { name: "性能优化", mr: "optimization" },
    { name: "多人", mr: "social" },
    { name: "支持库", cf: 6950, mr: "library" },
    { name: "模组相关", cf: 6946 },
  ],
  resourcepacks: [
    { name: "全部" },
    { name: "原版风", cf: 403, mr: "vanilla-like" },
    { name: "写实风", cf: 400, mr: "realistic" },
    { name: "现代风", cf: 401 },
    { name: "中世纪", cf: 402 },
    { name: "蒸汽朋克", cf: 399 },
    { name: "主题化", mr: "themed" },
    { name: "简洁", mr: "simplistic" },
    { name: "装饰", mr: "decoration" },
    { name: "战斗", mr: "combat" },
    { name: "实用", mr: "utility" },
    { name: "改良", mr: "tweaks" },
    { name: "鬼畜", mr: "cursed" },
    { name: "含实体", mr: "entities" },
    { name: "含声音", mr: "audio" },
    { name: "含字体", cf: 5244, mr: "fonts" },
    { name: "含模型", mr: "models" },
    { name: "含语言", mr: "locale" },
    { name: "含 UI", mr: "gui" },
    { name: "核心着色器", mr: "core-shaders" },
    { name: "动态效果", cf: 404 },
    { name: "兼容模组", cf: 4465, mr: "modded" },
    { name: "8x 或更低", mr: "8x-" },
    { name: "16x", cf: 393, mr: "16x" },
    { name: "32x", cf: 394, mr: "32x" },
    { name: "48x", mr: "48x" },
    { name: "64x", cf: 395, mr: "64x" },
    { name: "128x", cf: 396, mr: "128x" },
    { name: "256x", cf: 397, mr: "256x" },
    { name: "512x 或更高", cf: 398, mr: "512x+" },
  ],
  shaders: [
    { name: "全部" },
    { name: "原版风", cf: 6555, mr: "vanilla-like" },
    { name: "幻想风", cf: 6554, mr: "fantasy" },
    { name: "写实风", cf: 6553, mr: "realistic" },
    { name: "半写实风", mr: "semi-realistic" },
    { name: "卡通风", mr: "cartoon" },
    { name: "彩色光照", mr: "colored-lighting" },
    { name: "路径追踪", mr: "path-tracing" },
    { name: "PBR", mr: "pbr" },
    { name: "反射", mr: "reflections" },
    { name: "性能负荷·极低", mr: "potato" },
    { name: "性能负荷·低", mr: "low" },
    { name: "性能负荷·中", mr: "medium" },
    { name: "性能负荷·高", mr: "high" },
  ],
  // 世界只有 CurseForge 提供，标签也就只有 CF 的 categoryId（PCL 世界页那张表）
  worlds: [
    { name: "全部" },
    { name: "冒险", cf: 248 },
    { name: "创造", cf: 249 },
    { name: "小游戏", cf: 250 },
    { name: "跑酷", cf: 251 },
    { name: "解谜", cf: 252 },
    { name: "生存", cf: 253 },
    { name: "模组世界", cf: 4464, child: true },
  ],
};

/// 支持端筛选：PCL 的四项。
export type SideFilter = "all" | "client" | "server" | "both";

export const SIDES: Array<{ value: SideFilter; label: string }> = [
  { value: "all", label: "全部" },
  { value: "client", label: "支持客户端" },
  { value: "server", label: "支持服务端" },
  { value: "both", label: "支持两端" },
];
