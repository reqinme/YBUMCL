/// Mojang 的版本清单里 `type` 只有 release/snapshot/old_beta/old_alpha 四种，
/// 愚人节版混在 snapshot 里，只能按版本名认——这里照 PCL 的 ModMinecraft.vb
/// （NameToVersion / GetMcFoolName）维护同一张表。

export type FoolInfo = {
  /// 愚人节版发布的年份
  year: number;
  /// PCL 里那句介绍
  text: string;
  /// 它基于的正式版：列表排序按这个走，否则会按名字落到最上面
  basedOn: string;
};

type FoolRule = { match: (id: string) => boolean; info: FoolInfo };

const FOOLS: FoolRule[] = [
  { match: (id) => id.startsWith("2.0"), info: { year: 2013, text: "这个秘密计划了两年的更新将游戏推向了一个新高度！", basedOn: "1.5.1" } },
  { match: (id) => id === "15w14a", info: { year: 2015, text: "作为一款全年龄向的游戏，我们需要和平，需要爱与拥抱。", basedOn: "1.8.3" } },
  { match: (id) => id.includes(".rv-pre"), info: { year: 2016, text: "是时候将现代科技带入 Minecraft 了！", basedOn: "1.9.2" } },
  { match: (id) => id.includes("shareware"), info: { year: 2019, text: "我们从地下室的废墟里找到了这个开发于 1994 年的杰作！", basedOn: "1.13.2" } },
  // 20w14a 是正常快照，只有 20w14infinite（游戏里显示 20w14∞）才是愚人节版
  { match: (id) => id.startsWith("20w14") && id !== "20w14a", info: { year: 2020, text: "我们加入了 20 亿个新的维度，让无限的想象变成了现实！", basedOn: "1.15.2" } },
  { match: (id) => id.includes("oneblockatatime"), info: { year: 2022, text: "一次一个方块更新！迎接全新的挖掘、合成与骑乘玩法吧！", basedOn: "1.18.2" } },
  { match: (id) => id.includes("23w13a"), info: { year: 2023, text: "研究表明：玩家喜欢作出选择——越多越好！", basedOn: "1.19.4" } },
  { match: (id) => id === "24w14potato", info: { year: 2024, text: "毒马铃薯一直都被大家忽视和低估，于是我们超级加强了它！", basedOn: "1.20.4" } },
  { match: (id) => id === "25w14craftmine", info: { year: 2025, text: "你可以合成任何东西——包括合成你的世界！", basedOn: "1.21.4" } },
  { match: (id) => id === "26w14a", info: { year: 2026, text: "为什么需要物品栏？让方块们跟着你走吧！", basedOn: "26.1.1" } }
];

/// 认不出来的返回 null。
export function foolOf(id: string): FoolInfo | null {
  const name = id.toLowerCase().replace(/_unobfuscated/g, "").replace(/ unobfuscated/g, "");
  for (const rule of FOOLS) {
    if (rule.match(name)) return rule.info;
  }
  return null;
}

export type VersionTone = "release" | "snapshot" | "beta" | "alpha" | "fool";

export type VersionBadge = {
  /// 列表里那个字母
  letter: string;
  /// 悬停时显示的全称
  label: string;
  tone: VersionTone;
};

/// 徽章：愚人节优先，其余按清单给的 type。
export function badgeOf(type: string, id: string): VersionBadge {
  const fool = foolOf(id);
  if (fool) {
    return {
      letter: "F",
      label: `${fool.year} 年愚人节版本 · ${fool.text}`,
      tone: "fool"
    };
  }
  switch (type) {
    case "release":
      return { letter: "R", label: "正式版", tone: "release" };
    case "snapshot":
      return { letter: "S", label: "快照", tone: "snapshot" };
    case "old_beta":
      return { letter: "B", label: "旧 Beta", tone: "beta" };
    case "old_alpha":
      return { letter: "A", label: "旧 Alpha", tone: "alpha" };
    default:
      return { letter: "?", label: type, tone: "snapshot" };
  }
}

/// 排序用：愚人节版要排在「它基于的正式版」后面，其余保持清单里的顺序。
/// 返回 null 表示按清单位置排。
export function foolAnchor(id: string, indexOf: Map<string, number>): number | null {
  const fool = foolOf(id);
  if (!fool) return null;
  const base = indexOf.get(fool.basedOn);
  return base === undefined ? null : base + 0.5;
}
