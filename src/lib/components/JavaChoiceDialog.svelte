<script lang="ts">
  /// Java 版本对不上时的弹窗：换一套、还是照旧启动，由用户决定。
  /// 以前后端会悄悄换掉，日志里才有记录，用户根本不知道发生过什么。
  import Modal from "./Modal.svelte";
  import { answerJava, javaPrompt } from "$lib/dialogs.svelte";

  const check = $derived(javaPrompt.value?.check ?? null);
</script>

<Modal
  open={check !== null}
  title={check ? `这个实例要 ${check.required}` : ""}
  onclose={() => answerJava(null)}
>
  {#if check}
    <p>
      {#if check.currentMajor}
        现在选的是 Java {check.currentMajor}，不在这个版本支持的范围内。
      {:else if check.currentPath}
        现在指定的 Java 找不到了：<code>{check.currentPath}</code>
      {:else}
        没检测到能用的 Java。
      {/if}
    </p>
    <p class="hint">
      {#if check.suggestedMajor}
        可以换成 Java {check.suggestedMajor} 启动；也可以在实例的「启动设置」里自己指定。
      {:else}
        没找到符合要求的 Java。装一套之后再来，或者在实例的「启动设置」里指定。
      {/if}
    </p>
  {/if}

  {#snippet footer()}
    {#if check?.suggestedMajor}
      <button class="btn primary" onclick={() => answerJava("suggested")}>
        改用 Java {check.suggestedMajor}
      </button>
    {/if}
    {#if check?.currentMajor}
      <button class="btn ghost" onclick={() => answerJava("ignore")}>
        照旧用 Java {check.currentMajor} 启动
      </button>
    {/if}
    <button class="btn ghost" onclick={() => answerJava(null)}>取消</button>
  {/snippet}
</Modal>
