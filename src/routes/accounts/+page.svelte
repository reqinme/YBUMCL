<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Account, type AccountsView, type DeviceCode } from "$lib/api";
  import { showError } from "$lib/dialogs.svelte";
  import { announce } from "$lib/tasks.svelte";
  import BackButton from "$lib/components/BackButton.svelte";
  import Modal from "$lib/components/Modal.svelte";
  import ConfirmDialog from "$lib/components/ConfirmDialog.svelte";

  type Filter = "all" | "offline" | "microsoft";
  type Layout = "grid" | "list";

  let view = $state<AccountsView | null>(null);
  let loaded = $state(false);
  let filter = $state<Filter>("all");
  let layout = $state<Layout>(readLayout());

  let addOpen = $state(false);
  let addKind = $state<"pick" | "offline" | "microsoft">("pick");
  let newName = $state("");
  let busy = $state(false);

  let device = $state<DeviceCode | null>(null);
  let msLeft = $state(0);

  let menuFor = $state<string | null>(null);
  let renaming = $state<Account | null>(null);
  let renameName = $state("");
  let removing = $state<Account | null>(null);

  let polite = $state("");

  let timer: ReturnType<typeof setInterval> | undefined;

  const sources: { id: Filter; label: string }[] = [
    { id: "all", label: "全部来源" },
    { id: "offline", label: "离线账户" },
    { id: "microsoft", label: "微软账户" }
  ];

  const visible = $derived(
    (view?.players ?? []).filter((account) => filter === "all" || account.kind === filter)
  );
  const counts = $derived({
    all: view?.players.length ?? 0,
    offline: (view?.players ?? []).filter((account) => account.kind === "offline").length,
    microsoft: (view?.players ?? []).filter((account) => account.kind === "microsoft").length
  });

  function readLayout(): Layout {
    try {
      return localStorage.getItem("ybumcl.accounts.layout") === "list" ? "list" : "grid";
    } catch {
      return "grid";
    }
  }

  function writeLayout(next: Layout) {
    layout = next;
    try {
      localStorage.setItem("ybumcl.accounts.layout", next);
    } catch {
      /* 隐私模式下写不进去，不影响使用 */
    }
  }

  function initial(account: Account) {
    return (account.name.trim()[0] ?? "?").toUpperCase();
  }

  async function load() {
    try {
      view = await api.listAccounts();
    } catch (error) {
      showError("读取账户失败", "本地账户表没读出来。", String(error));
    } finally {
      loaded = true;
    }
  }

  async function select(account: Account) {
    menuFor = null;
    if (view?.current === account.id) return;
    polite = `已切换到 ${account.name}`;
    try {
      view = await api.selectAccount(account.id);
      announce(`已切换到「${account.name}」，启动游戏时用它`);
    } catch (error) {
      showError("切换账户失败", "", String(error));
    }
  }

  async function addOffline() {
    busy = true;
    try {
      view = await api.addOfflineAccount(newName);
      announce(`已添加离线账户「${newName.trim()}」`);
      closeAdd();
    } catch (error) {
      showError("添加离线账户失败", String(error));
    } finally {
      busy = false;
    }
  }

  function closeAdd() {
    stopPolling();
    addOpen = false;
    addKind = "pick";
    newName = "";
    device = null;
  }

  function stopPolling() {
    if (timer !== undefined) {
      clearInterval(timer);
      timer = undefined;
    }
  }

  async function startMicrosoft() {
    busy = true;
    addKind = "microsoft";
    addOpen = true;
    device = null;
    try {
      const code = await api.msLoginStart();
      device = code;
      msLeft = code.expiresIn;
      poll(code);
    } catch (error) {
      showError("微软登录没有开始", String(error));
      addKind = "pick";
    } finally {
      busy = false;
    }
  }

  function poll(code: DeviceCode) {
    stopPolling();
    const every = Math.max(5, code.interval) * 1000;
    timer = setInterval(async () => {
      if (msLeft <= 0) {
        stopPolling();
        showError("微软登录超时", "这次设备码过期了，重新点一次「微软账户」。");
        return;
      }
      msLeft -= Math.max(5, code.interval);
      try {
        const result = await api.msLoginPoll(code.deviceCode);
        if (result.state === "pending") return;
        stopPolling();
        if (result.state === "done" && result.accounts) {
          view = result.accounts;
          announce("微软账户已添加");
          closeAdd();
        } else {
          showError("微软登录没有成功", result.error ?? "微软没有说明原因，再试一次。");
          device = null;
          addKind = "pick";
        }
      } catch (error) {
        stopPolling();
        showError("查询登录状态失败", String(error));
      }
    }, every);
  }

  function startRename(account: Account) {
    menuFor = null;
    renaming = account;
    renameName = account.name;
  }

  async function confirmRename() {
    if (!renaming) return;
    busy = true;
    try {
      view = await api.renameAccount(renaming.id, renameName);
      announce("名字已改");
      renaming = null;
    } catch (error) {
      showError("改名失败", String(error));
    } finally {
      busy = false;
    }
  }

  async function confirmRemove() {
    if (!removing) return;
    busy = true;
    try {
      view = await api.removeAccount(removing.id);
      announce(`已删除「${removing.name}」`);
      removing = null;
    } catch (error) {
      showError("删除账户失败", String(error));
    } finally {
      busy = false;
    }
  }

  async function refresh(account: Account) {
    menuFor = null;
    busy = true;
    try {
      view = await api.refreshAccount(account.id);
      announce("登录状态已续期");
    } catch (error) {
      showError(
        "续期失败",
        "微软那边的登录状态已经不能用了，可以用「重新登录」再走一次设备码。",
        String(error)
      );
    } finally {
      busy = false;
    }
  }

  onMount(() => {
    void load();
    return () => stopPolling();
  });
</script>

<svelte:window onclick={() => (menuFor = null)} />

<section class="page">
  <header class="head">
    <BackButton />
    <div class="head-text">
      <h1>账户</h1>
      <p class="dim">启动游戏时用这里选中的那个账户；离线账户能进单人和局域网，微软账户能进正版服务器。</p>
    </div>
  </header>

  <div class="layout">
    <aside class="rail">
      {#each sources as source (source.id)}
        <button
          class="src"
          class:active={filter === source.id}
          onclick={() => (filter = source.id)}
        >
          <span class="label">{source.label}</span>
          {#if counts[source.id] > 0}<span class="count">{counts[source.id]}</span>{/if}
        </button>
      {/each}
    </aside>

    <section class="panel">
      <div class="toolbar">
        <div class="seg" role="group" aria-label="排列方式">
          <button class:active={layout === "grid"} onclick={() => writeLayout("grid")} aria-label="网格">
            <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
              <rect x="1" y="1" width="6" height="6" rx="1" />
              <rect x="9" y="1" width="6" height="6" rx="1" />
              <rect x="1" y="9" width="6" height="6" rx="1" />
              <rect x="9" y="9" width="6" height="6" rx="1" />
            </svg>
          </button>
          <button class:active={layout === "list"} onclick={() => writeLayout("list")} aria-label="列表">
            <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
              <rect x="1" y="2" width="14" height="3" rx="1" />
              <rect x="1" y="7" width="14" height="3" rx="1" />
              <rect x="1" y="12" width="14" height="3" rx="1" />
            </svg>
          </button>
        </div>
        <button class="btn primary add" onclick={() => (addOpen = true)}>+ 添加角色</button>
      </div>

      {#if !loaded}
        <p class="dim">读取中…</p>
      {:else if visible.length === 0}
        <p class="dim">还没有账户。</p>
      {:else}
        <div class="cards" class:list={layout === "list"}>
          {#each visible as account (account.id)}
            <div class="card" class:active={view?.current === account.id}>
              <button class="pick" onclick={() => select(account)} aria-pressed={view?.current === account.id}>
                {#if account.avatar}
                  <img class="face" src={account.avatar} alt="" />
                {:else}
                  <span class="face letter" aria-hidden="true">{initial(account)}</span>
                {/if}
                <span class="meta">
                  <span class="name">{account.name}</span>
                  <span class="sub">{account.kind === "microsoft" ? "微软账户" : "离线账户"}</span>
                </span>
                {#if view?.current === account.id}
                  <span class="dot" aria-label="当前账户"></span>
                {/if}
              </button>
              <button
                class="more"
                aria-label="更多操作"
                aria-expanded={menuFor === account.id}
                onclick={(event) => {
                  event.stopPropagation();
                  menuFor = menuFor === account.id ? null : account.id;
                }}
              >
                …
              </button>
              {#if menuFor === account.id}
                <div class="menu" onclick={(event) => event.stopPropagation()} onkeydown={() => {}} role="presentation">
                  {#if view?.current !== account.id}
                    <button onclick={() => select(account)}>设为当前</button>
                  {/if}
                  {#if account.kind === "offline"}
                    <button onclick={() => startRename(account)}>重命名</button>
                  {:else}
                    <button onclick={() => refresh(account)}>续期登录</button>
                    <button onclick={() => { menuFor = null; void startMicrosoft(); }}>重新登录</button>
                  {/if}
                  <button class="danger" onclick={() => { menuFor = null; removing = account; }}>删除</button>
                </div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </section>
  </div>
</section>

<p class="sr-only" aria-live="polite">{polite}</p>

<Modal open={addOpen} title="添加角色" onclose={closeAdd}>
  {#if addKind === "pick"}
    <div class="kinds">
      <button class="kind" onclick={() => (addKind = "offline")}>
        <strong>离线账户</strong>
        <span class="dim">起个名字就能用，进单人游戏和局域网。</span>
      </button>
      <button class="kind" onclick={() => startMicrosoft()}>
        <strong>微软账户</strong>
        <span class="dim">在浏览器里输一次设备码，能进正版服务器。</span>
      </button>
    </div>
  {:else if addKind === "offline"}
    <div class="form">
      <label>
        <span>玩家名</span>
        <input bind:value={newName} maxlength="16" placeholder="Steve" />
      </label>
      <p class="dim small">3–16 个字符，只能用英文、数字和下划线（中文名进不了大多数服务器）。</p>
      <div class="row">
        <button class="btn primary" disabled={busy} onclick={addOffline}>
          {busy ? "添加中…" : "添加"}
        </button>
        <button class="btn ghost" onclick={() => (addKind = "pick")}>返回</button>
      </div>
    </div>
  {:else}
    <div class="ms">
      {#if !device}
        <p class="dim">点了「开始登录」之后，浏览器里会要求你输一串设备码，输完回到这里等几秒。</p>
        <button class="btn primary" disabled={busy} onclick={() => startMicrosoft()}>
          {busy ? "正在跟微软要设备码…" : "开始登录"}
        </button>
      {:else}
        <p class="dim">在浏览器打开下面这个地址，输入设备码：</p>
        <p class="code">{device.userCode}</p>
        <div class="row">
          <button class="btn primary" onclick={() => api.openUrl(device!.verificationUri)}>
            打开浏览器
          </button>
          <span class="dim small">{device.verificationUri}</span>
        </div>
        <p class="dim small">等待确认…（还剩 {msLeft} 秒）</p>
      {/if}
    </div>
  {/if}
</Modal>

<Modal open={renaming !== null} title="重命名" onclose={() => (renaming = null)}>
  <div class="form">
    <label>
      <span>玩家名</span>
      <input bind:value={renameName} maxlength="16" placeholder="Steve" />
    </label>
    <div class="row">
      <button class="btn primary" disabled={busy} onclick={confirmRename}>
        {busy ? "保存中…" : "保存"}
      </button>
      <button class="btn ghost" onclick={() => (renaming = null)}>取消</button>
    </div>
  </div>
</Modal>

<ConfirmDialog
  open={removing !== null}
  message={removing ? `要从启动器里删掉「${removing.name}」吗？删掉之后只能重新添加。` : ""}
  oncancel={() => (removing = null)}
  onconfirm={() => confirmRemove()}
/>

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
  .layout {
    display: grid;
    grid-template-columns: 200px minmax(0, 1fr);
    gap: 16px;
    align-items: start;
  }
  .rail {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 10px 8px;
    border-radius: 14px;
    background: var(--panel);
    border: 1px solid var(--border);
  }
  .src {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 9px 12px;
    border-radius: 10px;
    background: transparent;
    border: none;
    color: var(--text);
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }
  .src:hover {
    background: rgba(255, 255, 255, 0.05);
  }
  .src.active {
    background: rgba(255, 255, 255, 0.1);
    font-weight: 600;
  }
  .count {
    font-size: 12px;
    color: var(--text-dim);
  }
  .panel {
    display: flex;
    flex-direction: column;
    gap: 14px;
    min-height: 320px;
  }
  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }
  .seg {
    display: inline-flex;
    padding: 3px;
    gap: 2px;
    border-radius: 10px;
    background: var(--panel);
    border: 1px solid var(--border);
  }
  .seg button {
    display: grid;
    place-items: center;
    width: 30px;
    height: 26px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--text-dim);
    cursor: pointer;
  }
  .seg button.active {
    background: rgba(255, 255, 255, 0.12);
    color: var(--text);
  }
  .seg svg {
    fill: currentColor;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 12px;
  }
  .cards.list {
    grid-template-columns: minmax(0, 1fr);
  }
  .card {
    position: relative;
    border-radius: 14px;
    background: var(--panel);
    border: 1px solid var(--border);
    overflow: visible;
  }
  .card.active {
    border-color: var(--yb-green-bright);
    box-shadow: 0 0 0 1px var(--yb-green-bright) inset;
  }
  .pick {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 16px;
    border: none;
    background: transparent;
    color: var(--text);
    text-align: left;
    cursor: pointer;
  }
  .cards.list .pick {
    padding: 10px 16px;
  }
  .face {
    width: 48px;
    height: 48px;
    border-radius: 10px;
    flex: none;
    image-rendering: pixelated;
    object-fit: cover;
    background: rgba(255, 255, 255, 0.08);
  }
  .cards.list .face {
    width: 34px;
    height: 34px;
    border-radius: 8px;
  }
  .face.letter {
    display: grid;
    place-items: center;
    font-size: 20px;
    font-weight: 600;
    color: #11150f;
    background: linear-gradient(160deg, var(--yb-green-bright), var(--yb-green));
  }
  .meta {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
    flex: 1;
  }
  .name {
    font-size: 14px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub {
    font-size: 12px;
    color: var(--text-dim);
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    flex: none;
    background: #2a7de1;
  }
  .more {
    position: absolute;
    top: 8px;
    right: 8px;
    width: 26px;
    height: 26px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--text-dim);
    font-size: 16px;
    line-height: 1;
    cursor: pointer;
  }
  .more:hover {
    background: rgba(255, 255, 255, 0.1);
    color: var(--text);
  }
  .menu {
    position: absolute;
    top: 36px;
    right: 8px;
    z-index: 20;
    display: flex;
    flex-direction: column;
    min-width: 132px;
    padding: 4px;
    border-radius: 10px;
    background: #1d2320;
    border: 1px solid var(--border);
    box-shadow: 0 10px 24px rgba(0, 0, 0, 0.45);
  }
  .menu button {
    padding: 8px 10px;
    border: none;
    border-radius: 7px;
    background: transparent;
    color: var(--text);
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }
  .menu button:hover {
    background: rgba(255, 255, 255, 0.08);
  }
  .menu button.danger {
    color: #ff9a8a;
  }
  .kinds {
    display: grid;
    gap: 10px;
  }
  .kind {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 14px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: rgba(255, 255, 255, 0.03);
    color: var(--text);
    text-align: left;
    cursor: pointer;
  }
  .kind:hover {
    border-color: var(--yb-green-bright);
  }
  .form {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .form label {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .form span {
    font-size: 12px;
    color: var(--text-dim);
  }
  .form input {
    padding: 9px 12px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .small {
    font-size: 12px;
    margin: 0;
  }
  .code {
    margin: 4px 0;
    padding: 10px 14px;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.06);
    font-family: ui-monospace, Consolas, monospace;
    font-size: 22px;
    letter-spacing: 3px;
    text-align: center;
  }
  .ms {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .ms p {
    margin: 0;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
</style>
