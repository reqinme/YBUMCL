import { goto } from "$app/navigation";
import { api, type VersionView } from "$lib/api";

/** 启动一个版本：配置由后端按 version-settings.json 组装，成功后跳到日志页。 */
export async function startVersion(version: VersionView): Promise<void> {
  await api.launchVersion(version.id);
  await goto(`/launch?version=${encodeURIComponent(version.id)}`);
}
