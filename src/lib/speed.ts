/// 下载限速档位，与 PCL 的 `ToolDownloadSpeed` 一致：0..=42 共 43 档，
/// 0..=14 每档 0.1 MB/s（0.1–1.5），15..=31 每档 0.5 MB/s（2–10），
/// 32..=41 每档 1 MB/s（11–20），42 = 不限速。
export const SPEED_TIER_UNLIMITED = 42;
export const SPEED_TIER_COUNT = 43;

/// 档位对应的 MB/s；不限速返回 0。
export function speedTierMbps(tier: number): number {
  if (tier <= 14) return (tier + 1) / 10;
  if (tier <= 31) return (tier - 11) / 2;
  if (tier <= 41) return tier - 21;
  return 0;
}

/// 档位在界面上的名字。
export function speedTierLabel(tier: number): string {
  const mbps = speedTierMbps(tier);
  if (mbps === 0) return "不限速";
  return `${Number.isInteger(mbps) ? mbps : mbps.toFixed(1)} MB/s`;
}

/// 下拉的完整选项表（0..=42）。
export function speedTierOptions(): { value: string; label: string }[] {
  return Array.from({ length: SPEED_TIER_COUNT }, (_, tier) => ({
    value: String(tier),
    label: speedTierLabel(tier)
  }));
}
