import { goto } from "$app/navigation";
import { api, type VersionView } from "$lib/api";
import { askJava } from "$lib/dialogs.svelte";

/**
 * 启动一个版本：配置由后端按 version-settings.json 组装，成功后跳到日志页。
 *
 * Java 版本对不上时不直接换、也不硬拦：弹窗让用户挑「换一套」还是「照旧启动」。
 */
export async function startVersion(version: VersionView): Promise<void> {
  const check = await api.checkJava(version.id);
  if (!check.accepted) {
    const choice = await askJava(check);
    if (!choice) return;
    await api.launchVersion(version.id, {
      java: choice === "suggested" ? check.suggestedPath : null,
      ignoreJava: choice === "ignore",
    });
  } else {
    await api.launchVersion(version.id);
  }
  await goto(`/launch?version=${encodeURIComponent(version.id)}`);
}
