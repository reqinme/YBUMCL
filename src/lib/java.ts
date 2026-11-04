import type { JavaInstallation } from "$lib/api";

/// 解析 "1.20.1" / "26.3" / "1.7.10" 这类版本号，取主次版本。
export function parseMcVersion(id: string): [number, number, number] {
  const parts = id.split(/[.\-]/).map((piece) => Number.parseInt(piece, 10));
  const safe = (value: number | undefined) => (Number.isFinite(value) ? (value as number) : 0);
  if (safe(parts[0]) === 1) {
    return [1, safe(parts[1]), safe(parts[2])];
  }
  // 新命名（26.x）没有前导 1，直接当主版本用。
  return [safe(parts[0]), safe(parts[1]), safe(parts[2])];
}

/// 按 Minecraft 官方要求推出运行/安装所需的最老 Java 主版本。
export function requiredJavaMajor(mcVersion: string): number {
  const [major, minor, patch] = parseMcVersion(mcVersion);
  if (major > 1) return 21; // 26.x 起一律 Java 21
  if (minor >= 20 && patch >= 5) return 21; // 1.20.5+
  if (minor >= 18) return 17; // 1.18 – 1.20.4
  if (minor === 17) return 17; // 1.17 实际要求 16，17 更稳
  return 8;
}

/// 从已装 Java 里挑一个能跑的：优先精确匹配，其次「不小于要求里最小的那个」。
export function pickJava(
  javas: JavaInstallation[],
  mcVersion: string,
  preferred?: string
): JavaInstallation | null {
  if (javas.length === 0) return null;
  if (preferred) {
    const hit = javas.find((java) => java.path === preferred);
    if (hit) return hit;
  }
  const required = requiredJavaMajor(mcVersion);
  const exact = javas.filter((java) => java.major === required);
  if (exact.length > 0) return exact[0];
  const newer = javas
    .filter((java) => java.major >= required)
    .sort((a, b) => a.major - b.major);
  if (newer.length > 0) return newer[0];
  // 一个够的都没有：用最新的那个，让 Java 自己报错总比启动器什么都不说好。
  return [...javas].sort((a, b) => b.major - a.major)[0] ?? null;
}
