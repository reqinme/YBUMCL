import { goto } from "$app/navigation";
import { api, onJavaRetry, type VersionView } from "$lib/api";
import { askJava, showError, type JavaChoice } from "$lib/dialogs.svelte";
import { failLaunching, markLaunching } from "$lib/launch-pending.svelte";
import { toast } from "$lib/toast";

/**
 * 启动一个版本：配置由后端按 version-settings.json 组装，成功后跳到日志页。
 *
 * Java 版本对不上时不直接换、也不硬拦：弹窗让用户挑「换一套」还是「照旧启动」。
 */
/// 自动档启动失败且后端判定「Java 选错了」时，最多自动换一次 Java 重试。
/// 换过之后不再重试：两套 Java 都起不来就是别的问题，别在用户眼前反复闪窗口。
let autoRetry: { versionId: string; attempts: number } | null = null;

void onJavaRetry(async (payload) => {
  if (
    !autoRetry ||
    autoRetry.versionId !== payload.versionId ||
    autoRetry.attempts >= 1
  ) {
    return;
  }
  autoRetry.attempts += 1;
  toast(`换一套 Java 再试：${payload.reason}`, "info", 2600);
  try {
    await api.launchVersion(payload.versionId, { java: payload.javaPath });
  } catch (error) {
    toast(String(error), "info", 3200);
  }
});

export async function startVersion(
  version: VersionView,
  options?: { quickPlay?: string | null },
): Promise<void> {
  const quickPlay = options?.quickPlay ?? null;
  // 没有选中账户就不让启动：启动参数里的 username/accessToken 都来自当前账户。
  const accounts = await api.listAccounts();
  if (!accounts.current) {
    showError(
      "先选一个账户",
      "还没有选中账户，游戏不能启动。",
      "去「账户」页点一下要用的账户，再回来启动。",
      "game",
    );
    return;
  }
  const check = await api.checkJava(version.id);
  let choice: JavaChoice | null = null;
  if (!check.accepted) {
    choice = await askJava(check);
    if (!choice) return;
  }
  markLaunching(version.id);
  try {
    if (choice === "suggested" || choice === "ignore") {
      await api.launchVersion(version.id, {
        java: choice === "suggested" ? check.suggestedPath : null,
        ignoreJava: choice === "ignore",
        quickPlay,
      });
    } else {
      await api.launchVersion(version.id, { quickPlay });
    }
  } catch (error) {
    failLaunching(version.id);
    showError(
      "启动失败",
      "游戏没有起来，看日志里的最后几行。",
      String(error),
      "game",
    );
    return;
  }
  autoRetry = { versionId: version.id, attempts: 0 };
  await goto(`/launch?version=${encodeURIComponent(version.id)}`);
}
