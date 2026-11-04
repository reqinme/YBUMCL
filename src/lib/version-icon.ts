// 版本卡片左上角那张小图。规则照 PCL（Modules/Minecraft/ModMinecraft.vb:786-809）：
// 先看装了什么加载器，认不出再按版本类型（正式 / 快照 / 愚人节 / 未知）挑方块图。
import anvil from "$lib/assets/blocks/Anvil.png";
import commandBlock from "$lib/assets/blocks/CommandBlock.png";
import egg from "$lib/assets/blocks/Egg.png";
import fabric from "$lib/assets/blocks/Fabric.png";
import goldBlock from "$lib/assets/blocks/GoldBlock.png";
import grass from "$lib/assets/blocks/Grass.png";
import grassPath from "$lib/assets/blocks/GrassPath.png";
import neoForge from "$lib/assets/blocks/NeoForge.png";
import redstoneBlock from "$lib/assets/blocks/RedstoneBlock.png";
import redstoneLampOff from "$lib/assets/blocks/RedstoneLampOff.png";
import redstoneLampOn from "$lib/assets/blocks/RedstoneLampOn.png";
import cobbleStone from "$lib/assets/blocks/CobbleStone.png";
import quilt from "$lib/assets/blocks/Quilt.png";
import cleanroom from "$lib/assets/blocks/Cleanroom.png";

/// 没有自定义图标时按状态取一张。OptiFine 优先于 Forge：PCL 里
/// 「Forge + OptiFine」这种实例用的是草径，不是铁砧。
export function builtinIcon(channel: string, loaders: string[]): string {
  const has = (name: string) =>
    loaders.some((loader) => loader.toLowerCase() === name.toLowerCase());

  if (has("optifine")) return grassPath;
  if (has("neoforge")) return neoForge;
  if (has("cleanroom")) return cleanroom;
  if (has("forge")) return anvil;
  if (has("quilt")) return quilt;
  if (has("fabric")) return fabric;
  if (has("liteloader")) return egg;
  switch (channel) {
    case "april":
      return goldBlock;
    case "snapshot":
      return commandBlock;
    case "release":
      return grass;
    default:
      return redstoneBlock;
  }
}

/// 实例可以手选的方块图，键名和后端 `versions::ICON_BUILTINS` 一致。
export const blockIcons: { key: string; label: string; src: string }[] = [
  { key: "CobbleStone", label: "圆石", src: cobbleStone },
  { key: "CommandBlock", label: "命令方块", src: commandBlock },
  { key: "GoldBlock", label: "金块", src: goldBlock },
  { key: "Grass", label: "草方块", src: grass },
  { key: "GrassPath", label: "土径", src: grassPath },
  { key: "Anvil", label: "铁砧", src: anvil },
  { key: "RedstoneBlock", label: "红石块", src: redstoneBlock },
  { key: "RedstoneLampOn", label: "红石灯（开）", src: redstoneLampOn },
  { key: "RedstoneLampOff", label: "红石灯（关）", src: redstoneLampOff },
  { key: "Egg", label: "鸡蛋", src: egg },
  { key: "Fabric", label: "布料（Fabric）", src: fabric },
  { key: "NeoForge", label: "狐狸（NeoForge）", src: neoForge },
  { key: "Quilt", label: "方格（Quilt）", src: quilt },
  { key: "Cleanroom", label: "Cleanroom", src: cleanroom },
];

/// `version_icon` 可能回图片数据，也可能回 `builtin:<方块名>`；
/// 后者翻成对应的静态图，翻不到就当没有自定义图标。
export function resolveIconUrl(
  raw: string | null,
  channel: string,
  loaders: string[],
): string {
  if (raw) {
    if (!raw.startsWith("builtin:")) return raw;
    const key = raw.slice("builtin:".length);
    const block = blockIcons.find((item) => item.key === key);
    if (block) return block.src;
  }
  return builtinIcon(channel, loaders);
}
