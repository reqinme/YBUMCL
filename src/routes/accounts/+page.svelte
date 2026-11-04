<script lang="ts">
  import { onMount } from "svelte";
  import { announce } from "$lib/tasks.svelte";
  import { showError } from "$lib/dialogs.svelte";
    import { api, type Settings, type VersionView } from "$lib/api";

  let settings = $state<Settings | null>(null);
  let versions = $state<VersionView[]>([]);
  let name = $state("");
  let loaded = $state(false);
  let saving = $state(false);
  let notice = $state("");

  const initial = $derived((name.trim()[0] ?? "?").toUpperCase());

  async function load() {
    try {
      const [loadedSettings, list] = await Promise.all([api.loadSettings(), api.listVersions()]);
      settings = loadedSettings;
      name = loadedSettings.username;
      versions = list;
    } catch (e) {
      showError("读取账户信息失败", "本地设置和版本列表没读全。", String(e));
    } finally {
      loaded = true;
    }
  }

  async function save() {
    if (!settings) return;
    notice = "";
    saving = true;
    try {
      const cleaned = name.trim().slice(0, 24) || "Steve";
      settings = await api.saveSettings({ ...settings, username: cleaned });
      name = settings.username;
      announce("已保存：新版本会默认使用这个玩家名");
    } catch (e) {
      showError("保存玩家名失败", "新名字没有写进本地设置。", String(e));
    } finally {
      saving = false;
    }
  }

  onMount(load);
  import BackButton from "$lib/components/BackButton.svelte";
</script>

<section class="page">
  <header class="head">
    <BackButton />
    <div class="head-text">
      <h1>账户</h1>
      <p class="dim">离线账户可以进单人与局域网；正版（微软）登录排在后面。</p>
    </div>
  </header>

  {#if !loaded}
    <p class="dim">读取中…</p>
  {:else if settings}
    <div class="rows">
      <section class="panel block">
        <h2>离线账户</h2>
        <div class="profile">
          <div class="avatar" aria-hidden="true">{initial}</div>
          <div class="fields">
            <label>
              <span>玩家名</span>
              <input bind:value={name} maxlength="24" placeholder="Steve" />
            </label>
            <p class="dim small">
              3–16 个字符最佳；只支持英文、数字和下划线（中文名进不了大多数服务器）。
            </p>
          </div>
        </div>
        <div class="actions">
          <button class="btn primary" onclick={save} disabled={saving}>
            {saving ? "保存中…" : "保存"}
          </button>
        </div>
      </section>


      <section class="panel block">
        <h2>正版登录</h2>
        <p class="dim">
          微软账号登录（设备码流程、皮肤与披风）还没做。做的时候会需要你在浏览器里确认一次授权，
          不会让启动器保存你的密码。
        </p>
      </section>
    </div>
  {/if}
</section>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .head-text h1 {
    margin: 0 0 4px;
    font-size: 22px;
  }
  .head-text p {
    margin: 0;
    font-size: 13px;
  }
  .rows {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .block h2 {
    margin: 0 0 12px;
    font-size: 14px;
    font-weight: 600;
  }
  .profile {
    display: flex;
    gap: 16px;
    align-items: flex-start;
  }
  .avatar {
    width: 56px;
    height: 56px;
    flex: none;
    border-radius: 50%;
    display: grid;
    place-items: center;
    font-size: 22px;
    font-weight: 600;
    color: #11150f;
    background: linear-gradient(160deg, var(--yb-green-bright), var(--yb-green));
  }
  .fields {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .fields label {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-width: 320px;
  }
  .fields span {
    font-size: 12px;
    color: var(--text-dim);
  }
  .fields input {
    padding: 9px 12px;
  }
  .small {
    font-size: 12px;
    margin: 0;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-top: 16px;
  }
</style>
