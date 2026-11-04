/// 当前实例：首页左栏「版本」列表里选中的那一个。
/// 下载内容时拿它当默认落点（PCL 也是这个口径：当前实例合适就用当前实例）。
const KEY = "ybumcl.currentVersion";

function read(): string | null {
  try {
    const value = localStorage.getItem(KEY);
    return value && value.trim() ? value : null;
  } catch {
    return null;
  }
}

export const currentVersion = $state<{ id: string | null }>({ id: read() });

export function setCurrentVersion(id: string | null) {
  currentVersion.id = id;
  try {
    if (id) localStorage.setItem(KEY, id);
    else localStorage.removeItem(KEY);
  } catch {
    // localStorage 不可用就只在本次运行里记住
  }
}

/// 列表换了之后校正一下：记住的实例还在就用它，不在了退回第一个。
export function pickCurrent(list: { id: string }[]): string | null {
  const stored = currentVersion.id;
  const found = stored && list.some((item) => item.id === stored) ? stored : (list[0]?.id ?? null);
  if (found !== stored) setCurrentVersion(found);
  return found;
}
