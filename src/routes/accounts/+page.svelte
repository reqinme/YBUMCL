<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Account, type AccountsView, type DeviceCode, type DeviceLogin } from "$lib/api";
  import { showError } from "$lib/dialogs.svelte";
  import { announce } from "$lib/tasks.svelte";
  import BackButton from "$lib/components/BackButton.svelte";
  import Modal from "$lib/components/Modal.svelte";
  import Dropdown from "$lib/components/Dropdown.svelte";
  import ConfirmDialog from "$lib/components/ConfirmDialog.svelte";
  import { OFFICIAL_CAPES } from "$lib/official-capes";
  import { OFFICIAL_SKINS, officialSheet } from "$lib/official-skins";
  import SkinModel from "$lib/components/SkinModel.svelte";
  import HeadAvatar from "$lib/components/HeadAvatar.svelte";

  /// "all" / "offline" / "microsoft"，或者某个认证服务器的地址。
  type Filter = string;
  type Layout = "grid" | "list";

  let view = $state<AccountsView | null>(null);
  let loaded = $state(false);
  let filter = $state<Filter>("all");
  let layout = $state<Layout>(readLayout());

  let addOpen = $state(false);
  let addKind = $state<"pick" | "offline" | "microsoft" | "thirdparty">("pick");
  let newName = $state("");
  let busy = $state(false);

  let device = $state<DeviceCode | null>(null);
  let msLeft = $state(0);

  let serverOpen = $state(false);
  let serverUrl = $state("");
  let thirdUrl = $state("");
  let thirdUser = $state("");
  let thirdPassword = $state("");
  let openid = $state<DeviceLogin | null>(null);
  let openidNote = $state("");
  let openidTimer: ReturnType<typeof setInterval> | undefined;
  let pollBusy = false;
  let openidBusy = false;

  let menuFor = $state<string | null>(null);
  let renaming = $state<Account | null>(null);
  let renameName = $state("");
  let skinFor = $state<Account | null>(null);
  let skinBusy = $state(false);
  let newUuid = $state("");
  let newSkin = $state<string | null>(null);
  let newSkinModel = $state<string | null>(null);
  let newSkinOfficial = $state<string | null>(null);
  let newCape = $state<string | null>(null);
  let newCapeOfficial = $state<string | null>(null);
  let littleskinName = $state("");
  let removing = $state<Account | null>(null);

  let polite = $state("");

  let timer: ReturnType<typeof setInterval> | undefined;

  const sources = $derived<{ id: Filter; label: string }[]>([
    { id: "all", label: "全部来源" },
    { id: "offline", label: "离线账户" },
    { id: "microsoft", label: "微软账户" },
    ...(view?.servers ?? []).map((server) => ({ id: server.url, label: server.name }))
  ]);

  function countOf(id: Filter) {
    const players = view?.players ?? [];
    if (id === "all") return players.length;
    if (id === "offline" || id === "microsoft") {
      return players.filter((account) => account.kind === id).length;
    }
    return players.filter((account) => account.server === id).length;
  }

  /// 某一档里可见的账户：先按这一档自己存的顺序，没记录的按账户表顺序接在后面。
  function visibleIds(accounts: Account[], order: string[] | undefined) {
    const rank = new Map((order ?? []).map((id, index) => [id, index]));
    return accounts
      .map((account, index) => ({ account, index }))
      .sort((left, right) => {
        const a = rank.get(left.account.id);
        const b = rank.get(right.account.id);
        if (a !== undefined && b !== undefined) return a - b;
        if (a !== undefined) return -1;
        if (b !== undefined) return 1;
        return left.index - right.index;
      })
      .map((item) => item.account);
  }

  const visible = $derived(
    visibleIds(
      (view?.players ?? []).filter((account) => {
        if (filter === "all") return true;
        if (filter === "offline" || filter === "microsoft") return account.kind === filter;
        return account.server === filter;
      }),
      view?.orders?.[filter],
    ),
  );

  /// 第三方账户卡片的副标题用服务器名，没名字就退回地址。
  function sourceName(account: Account) {
    if (account.kind === "microsoft") return "微软账户";
    if (account.kind === "offline") return "离线账户";
    const url = account.server ?? "";
    const server = (view?.servers ?? []).find((item) => item.url === url);
    return server?.name || url;
  }

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
    // 再点一次已选中的账户 = 取消选择，回到「未登录」；账户本身留着，不删除。
    if (view?.current === account.id) {
      try {
        view = await api.clearAccount();
        announce("已取消选择，现在是未登录状态");
      } catch (error) {
        showError("取消选择失败", "", String(error));
      }
      return;
    }
    polite = `已切换到 ${account.name}`;
    try {
      view = await api.selectAccount(account.id);
      announce(`已切换到「${account.name}」`);
    } catch (error) {
      showError("切换账户失败", "", String(error));
    }
  }

  // 拖动排序：按档保存，各档互不影响。把手用 pointer 事件，鼠标和触屏都能拖。
  let dragId = $state<string | null>(null);
  let dropId = $state<string | null>(null);
  let dropBefore = $state(true);
  let dragActive = false;
  let pending: { id: string; x: number; y: number; pointer: number } | null = null;
  let pointer = { x: 0, y: 0 };
  // 拖动到列表上下边缘时自动滚动。
  let scrollBox: HTMLElement | null = null;
  let scrollSpeed = 0;
  let scrollFrame: number | null = null;

  /// 指针下面是哪张卡、插它的前半还是后半。落在空白处就找最近的一张。
  function hitTest(x: number, y: number): { id: string; before: boolean } | null {
    const under = document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-account]");
    let card = under;
    if (!card || card.dataset.account === dragId) {
      card = nearestCard(x, y);
    }
    if (!card?.dataset.account) return null;
    const rect = card.getBoundingClientRect();
    const before =
      layout === "grid" ? x < rect.left + rect.width / 2 : y < rect.top + rect.height / 2;
    return { id: card.dataset.account, before };
  }

  /// 距离指针最近的那张卡（跳过正在拖的那张）。空白处松手时靠它兜底。
  function nearestCard(x: number, y: number): HTMLElement | null {
    let best: HTMLElement | null = null;
    let bestDistance = Number.POSITIVE_INFINITY;
    for (const node of document.querySelectorAll<HTMLElement>("[data-account]")) {
      if (node.dataset.account === dragId) continue;
      const rect = node.getBoundingClientRect();
      const dx = x - (rect.left + rect.width / 2);
      const dy = y - (rect.top + rect.height / 2);
      const distance = dx * dx + dy * dy;
      if (distance < bestDistance) {
        bestDistance = distance;
        best = node;
      }
    }
    return best;
  }

  function syncDrop() {
    const hit = hitTest(pointer.x, pointer.y);
    dropId = hit && hit.id !== dragId ? hit.id : null;
    if (hit && hit.id !== dragId) dropBefore = hit.before;
  }

  function tickScroll() {
    scrollFrame = null;
    if (!scrollBox || scrollSpeed === 0) return;
    scrollBox.scrollTop += scrollSpeed;
    // 列表在滚，指针下面换人了：重算落点，别让松手位置和看到的不一致。
    syncDrop();
    scrollFrame = requestAnimationFrame(tickScroll);
  }

  function setScrollSpeed(speed: number) {
    scrollSpeed = speed;
    if (speed !== 0 && scrollFrame === null) scrollFrame = requestAnimationFrame(tickScroll);
  }

  function dragStart(event: PointerEvent, id: string) {
    // 一次只认一个指针，否则第二根手指会把状态覆盖掉。
    if (pending) return;
    if (event.pointerType === "mouse" && event.button !== 0) return;
    // 不阻止默认行为的话，触屏长按会先去弹菜单/选文字，拖动根本起不来。
    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    handle.setPointerCapture(event.pointerId);
    scrollBox = handle.closest<HTMLElement>(".cards");
    pending = { id, x: event.clientX, y: event.clientY, pointer: event.pointerId };
    pointer = { x: event.clientX, y: event.clientY };
    dragActive = false;
    menuFor = null;
  }

  function dragMove(event: PointerEvent) {
    if (!pending || event.pointerId !== pending.pointer) return;
    pointer = { x: event.clientX, y: event.clientY };
    if (!dragActive) {
      if (Math.hypot(event.clientX - pending.x, event.clientY - pending.y) < 6) return;
      dragActive = true;
      dragId = pending.id;
    }
    syncDrop();
    if (scrollBox) {
      const box = scrollBox.getBoundingClientRect();
      const edge = 40;
      if (event.clientY < box.top + edge) setScrollSpeed(-14);
      else if (event.clientY > box.bottom - edge) setScrollSpeed(14);
      else setScrollSpeed(0);
    }
  }

  function dragEnd(event: PointerEvent) {
    if (!pending || event.pointerId !== pending.pointer) return;
    // 先把「从哪来、插到哪」抄下来：下面会清空状态，再读就永远是 null。
    const from = dragId;
    const target = dragActive ? dropId : null;
    const before = dropBefore;
    pending = null;
    dragActive = false;
    dragId = null;
    dropId = null;
    setScrollSpeed(0);
    scrollBox = null;
    if (from && target && target !== from) void dropOn(from, target, before);
  }

  /// 把 `from` 插到 `target` 的前面或后面（按下标重新算，不依赖拖动的方向）。
  async function dropOn(from: string, targetId: string, before: boolean) {
    if (!view) return;
    const ids = visible.map((account) => account.id);
    const fromIndex = ids.indexOf(from);
    if (fromIndex < 0) return;
    ids.splice(fromIndex, 1);
    const at = ids.indexOf(targetId);
    if (at < 0) return;
    ids.splice(before ? at : at + 1, 0, from);
    await commitOrder(ids);
  }

  /// 键盘排序：把手聚焦时 Alt+↑/↓ 上下挪一格。
  function handleKey(event: KeyboardEvent, id: string) {
    if (!event.altKey) return;
    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
    event.preventDefault();
    if (!view) return;
    const ids = visible.map((account) => account.id);
    const index = ids.indexOf(id);
    const next = index + (event.key === "ArrowUp" ? -1 : 1);
    if (index < 0 || next < 0 || next >= ids.length) return;
    ids.splice(index, 1);
    ids.splice(next, 0, id);
    void commitOrder(ids);
  }

  async function commitOrder(ids: string[]) {
    try {
      view = await api.reorderAccounts(filter, ids);
      polite = "账户顺序已更新";
    } catch (error) {
      showError("调整顺序失败", "", String(error));
    }
  }

  async function addServer() {
    busy = true;
    try {
      view = await api.addAuthServer(serverUrl);
      announce("认证服务器已添加");
      serverOpen = false;
      serverUrl = "";
      addKind = "pick";
      addOpen = true;
    } catch (error) {
      showError("添加认证服务器失败", String(error));
    } finally {
      busy = false;
    }
  }

  async function removeServer() {
    const server = (view?.servers ?? []).find((item) => item.url === filter);
    if (!server) return;
    try {
      view = await api.removeAuthServer(server.url);
      filter = "all";
      announce(`已移除「${server.name}」和它下面的账户`);
    } catch (error) {
      showError("移除认证服务器失败", String(error));
    }
  }

  function startThirdParty(preset?: string) {
    addKind = "thirdparty";
    thirdUrl =
      preset ??
      (filter !== "all" && filter !== "offline" && filter !== "microsoft"
        ? filter
        : (view?.servers[0]?.url ?? ""));
    thirdUser = "";
    thirdPassword = "";
  }

  async function addThirdParty() {
    busy = true;
    try {
      view = await api.addThirdPartyAccount(thirdUrl, thirdUser, thirdPassword);
      announce(`已添加第三方账户「${thirdUser.trim()}」`);
      closeAdd();
    } catch (error) {
      showError("第三方登录失败", String(error));
    } finally {
      busy = false;
    }
  }

  async function addOffline() {
    busy = true;
    try {
      view = await api.addOfflineAccount(newName, newUuid.trim() || undefined);
      const added = view.players.find((player) => player.name === newName.trim());
      if (added && (newSkin || newCape)) {
        view = await api.setOfflineLook({
          id: added.id,
          skin: newSkin,
          skinModel: newSkinModel,
          skinOfficial: newSkinOfficial,
          cape: newCape,
          capeOfficial: newCapeOfficial
        });
      }
      announce(`已添加离线账户「${newName.trim()}」`);
      closeAdd();
    } catch (error) {
      showError("添加离线账户失败", String(error));
    } finally {
      busy = false;
    }
  }

  function stopOpenid() {
    if (openidTimer) clearInterval(openidTimer);
    openidTimer = undefined;
    openid = null;
    openidNote = "";
  }

  async function startOpenid() {
    busy = true;
    try {
      openid = await api.startOpenidLogin(thirdUrl);
      openidNote = "等待授权…";
      void copyUserCode();
      openidTimer = setInterval(pollOpenid, Math.max(1, openid?.interval ?? 5) * 1000);
    } catch (error) {
      showError("外链登录启动失败", String(error));
    } finally {
      busy = false;
    }
  }

  async function pollOpenid() {
    const login = openid;
    if (!login || openidBusy) return;
    openidBusy = true;
    try {
      const result = await api.pollOpenidLogin(login.session);
      if (result.state === "done") {
        if (result.accounts) view = result.accounts;
        stopOpenid();
        closeAdd();
        announce("已通过外链登录添加账户");
        return;
      }
      if (result.state === "slow_down") openidNote = "授权检查太频繁，已放慢。";
      if (result.state === "denied") openidNote = "你在授权页点了拒绝。";
      if (result.state === "expired") openidNote = "设备码过期了，重新开始一次。";
    } catch (error) {
      const message = String(error);
      stopOpenid();
      if (message.toLowerCase().includes("already been used")) {
        void load();
        return;
      }
      showError("外链登录失败", message);
    } finally {
      openidBusy = false;
    }
  }

  function copyToClipboard(text: string) {
    void navigator.clipboard.writeText(text).catch(() => {});
  }

  async function copyUserCode() {
    const code = openid?.userCode;
    if (!code) return;
    try {
      await navigator.clipboard.writeText(code);
      openidNote = "设备码已复制。";
    } catch {
      openidNote = "复制失败，请手动选中。";
    }
  }

  function closeAdd() {
    stopPolling();
    stopOpenid();
    addOpen = false;
    addKind = "pick";
    newName = "";
    newUuid = "";
    newSkin = null;
    newSkinModel = null;
    newSkinOfficial = null;
    newCape = null;
    newCapeOfficial = null;
    littleskinName = "";
    device = null;
    thirdUser = "";
    thirdPassword = "";
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
      copyToClipboard(code.userCode);
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
      // 上一次查询还没回来就跳过，同一个设备码被查两次会报「已被使用」。
      if (pollBusy) return;
      if (msLeft <= 0) {
        stopPolling();
        showError("微软登录超时", "这次设备码过期了，重新点一次「微软账户」。");
        return;
      }
      msLeft -= Math.max(5, code.interval);
      pollBusy = true;
      try {
        const result = await api.msLoginPoll(code.deviceCode);
        if (result.state === "pending") return;
        stopPolling();
        if (result.state === "done" && result.accounts) {
          view = result.accounts;
          announce("微软账户已添加");
          closeAdd();
        } else if ((result.error ?? "").includes("already been used")) {
          // 设备码已经被换过票，说明这次登录其实完成了，只是有一条查询来晚了。
          stopPolling();
          device = null;
          void load();
        } else {
          showError("微软登录没有成功", result.error ?? "微软没有说明原因，再试一次。");
          device = null;
          addKind = "pick";
        }
      } catch (error) {
        stopPolling();
        showError("查询登录状态失败", String(error));
      } finally {
        pollBusy = false;
      }
    }, every);
  }

  function startRename(account: Account) {
    menuFor = null;
    renaming = account;
    renameName = account.name;
  }

  /// 选官方皮肤：贴图按当前手臂模型取，切换手臂时再取一次。
  function chooseOfficial(id: string) {
    const skin = OFFICIAL_SKINS.find((item) => item.id === id);
    if (!skin) return;
    newSkinOfficial = id;
    newSkinModel = newSkinModel === "slim" ? "slim" : skin.model;
    newSkin = officialSheet(id, newSkinModel === "slim" ? "slim" : "classic");
  }

  function chooseArm(model: "classic" | "slim") {
    newSkinModel = model;
    if (newSkinOfficial) {
      newSkin = officialSheet(newSkinOfficial, model);
    }
  }

  function chooseCape(id: string | null) {
    if (id === null) {
      newCape = null;
      newCapeOfficial = null;
      return;
    }
    const cape = OFFICIAL_CAPES.find((item) => item.id === id);
    if (!cape) return;
    newCape = cape.image;
    newCapeOfficial = id;
  }

  async function pickCapeFile() {
    skinBusy = true;
    try {
      const picked = await api.pickCapeFile();
      if (picked) {
        newCape = picked.cape;
        newCapeOfficial = null;
      }
    } catch (error) {
      showError("读取披风失败", String(error));
    } finally {
      skinBusy = false;
    }
  }

  async function pickSkinFile() {
    skinBusy = true;
    try {
      const picked = await api.pickSkinFile();
      if (picked) {
        newSkin = picked.skin;
        newSkinModel = picked.model;
        newSkinOfficial = null;
      }
    } catch (error) {
      showError("读取皮肤失败", String(error));
    } finally {
      skinBusy = false;
    }
  }

  async function pickLittleSkin() {
    if (!littleskinName.trim()) return;
    skinBusy = true;
    try {
      const picked = await api.fetchLittleskinSkin(littleskinName.trim());
      newSkin = picked.skin;
      newSkinModel = picked.model;
      newSkinOfficial = null;
    } catch (error) {
      showError("取 LittleSkin 皮肤失败", String(error));
    } finally {
      skinBusy = false;
    }
  }

  function startSkin(account: Account) {
    skinFor = account;
    newSkin = account.skin ?? null;
    newSkinModel = account.skinModel ?? null;
    newSkinOfficial = account.skinOfficial ?? null;
    newCape = account.cape ?? null;
    newCapeOfficial = account.capeOfficial ?? null;
    littleskinName = account.name;
  }

  async function saveSkin() {
    if (!skinFor) return;
    skinBusy = true;
    try {
      view = await api.setOfflineLook({
        id: skinFor.id,
        skin: newSkin,
        skinModel: newSkinModel,
        skinOfficial: newSkinOfficial,
        cape: newCape,
        capeOfficial: newCapeOfficial
      });
      announce("皮肤已更新");
      skinFor = null;
    } catch (error) {
      showError("保存皮肤失败", String(error));
    } finally {
      skinBusy = false;
    }
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

  /// 账户表一变就喊一声，顶栏那个头像框跟着换人。
  $effect(() => {
    if (!view) return;
    window.dispatchEvent(new CustomEvent("ybmcl:accounts"));
  });
</script>

<svelte:window onclick={() => (menuFor = null)} onpointerup={dragEnd} onpointercancel={dragEnd} />

<section class="page">
  <header class="head">
    <BackButton />
    <div class="head-text">
      <h1>账户</h1>
    </div>
  </header>

  <div class="layout">
    <aside class="rail">
      <p class="rail-title">来源</p>
      {#each sources as source (source.id)}
        <button
          class="src"
          class:active={filter === source.id}
          onclick={() => (filter = source.id)}
        >
          <span class="label">{source.label}</span>
          {#if countOf(source.id) > 0}<span class="count">{countOf(source.id)}</span>{/if}
        </button>
      {/each}
      <button class="src add-src" onclick={() => { serverOpen = true; serverUrl = ""; }}>
        + 添加认证服务器
      </button>
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
        {#if filter !== "all" && filter !== "offline" && filter !== "microsoft"}
          <button class="btn ghost small" onclick={removeServer}>移除这个服务器</button>
        {/if}
        <button class="btn primary add" onclick={() => (addOpen = true)}>+ 添加角色</button>
      </div>

      {#if !loaded}
        <p class="dim pad">读取中…</p>
      {:else if visible.length === 0}
        <div class="empty">
          <p class="dim">还没有账户。</p>
          <button class="btn primary" onclick={() => (addOpen = true)}>+ 添加角色</button>
        </div>
      {:else}
        <div class="cards" class:list={layout === "list"}>
          {#each visible as account (account.id)}
            <div
              class="card"
              class:active={view?.current === account.id}
              class:grid={layout === "grid"}
              class:dragging={dragId === account.id}
              class:over-before={dropId === account.id && dropBefore}
              class:over-after={dropId === account.id && !dropBefore}
              data-account={account.id}
            >
              <button
                class="handle"
                aria-label="拖动排序"
                title="按住拖动调整顺序；聚焦后 Alt+↑ / Alt+↓ 也能挪"
                onpointerdown={(event) => dragStart(event, account.id)}
                onkeydown={(event) => handleKey(event, account.id)}
                onpointermove={dragMove}
                onpointerup={dragEnd}
                onpointercancel={dragEnd}
                onlostpointercapture={dragEnd}
              >
                <span class="grip" aria-hidden="true"></span>
              </button>
              <button class="pick" onclick={() => select(account)} aria-pressed={view?.current === account.id}>
                {#if layout === "grid"}
                  <span class="stage">
                    <SkinModel
                      skin={account.skin}
                      model={account.skinModel}
                      cape={account.cape}
                      mode="image"
                      fill
                    />
                  </span>
                {:else if account.skin}
                  <HeadAvatar skin={account.skin} size={34} fallback={account.name} />
                {:else if account.avatar}
                  <img class="face" src={account.avatar} alt="" />
                {:else}
                  <span class="face letter" aria-hidden="true">{initial(account)}</span>
                {/if}
                <span class="meta">
                  <span class="name">{account.name}</span>
                  <span class="sub">{sourceName(account)}</span>
                </span>
                {#if view?.current === account.id}
                  <span class="now">当前</span>
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
                    <button onclick={() => { menuFor = null; startSkin(account); }}>更换皮肤</button>
                    <button onclick={() => startRename(account)}>重命名</button>
                  {:else if account.kind === "microsoft"}
                    <button onclick={() => refresh(account)}>续期登录</button>
                    <button onclick={() => { menuFor = null; void startMicrosoft(); }}>重新登录</button>
                  {:else}
                    <button
                      onclick={() => {
                        menuFor = null;
                        addOpen = true;
                        startThirdParty(account.server ?? undefined);
                      }}
                    >
                      重新登录
                    </button>
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

<Modal open={addOpen} title="添加角色" width="min(560px, 88vw)" onclose={closeAdd}>
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
      <button
        class="kind"
        disabled={!view?.servers.length}
        onclick={() => startThirdParty()}
      >
        <strong>第三方账户</strong>
        <span class="dim">
          {view?.servers.length
            ? "用认证服务器的账号密码登录，能进这些服务器。"
            : "先点左下角「添加认证服务器」，再加这里的账号。"}
        </span>
      </button>
    </div>
  {:else if addKind === "offline"}
    <div class="form">
      <label>
        <span>玩家名</span>
        <input bind:value={newName} maxlength="16" placeholder="Steve" />
      </label>
      <p class="dim small">1–16 个字符，不能有空格。中文名能建，但多数服务器不认。</p>
      <details class="advanced">
        <summary>高级</summary>
        <label>
          <span>UUID</span>
          <input bind:value={newUuid} placeholder="留空就按名字推导" />
        </label>
        <p class="dim small">服务器要求固定 UUID 时才填。</p>
      </details>
      <div class="skin-box">
        <span class="skin-label">皮肤</span>
        {@render skinControls()}
      </div>
      <div class="row">
        <button class="btn primary" disabled={busy} onclick={addOffline}>
          {busy ? "添加中…" : "添加"}
        </button>
        <button class="btn ghost" onclick={() => (addKind = "pick")}>返回</button>
      </div>
    </div>
  {:else if addKind === "thirdparty"}
    <div class="form">
      <label>
        <span>认证服务器</span>
        <Dropdown
          value={thirdUrl}
          options={(view?.servers ?? []).map((server) => ({ value: server.url, label: server.name }))}
          onchange={(next) => (thirdUrl = next)}
        />
      </label>
      <label>
        <span>账号</span>
        <input bind:value={thirdUser} autocomplete="username" placeholder="邮箱或用户名" />
      </label>
      <label>
        <span>密码</span>
        <input type="password" bind:value={thirdPassword} autocomplete="current-password" placeholder="密码" />
      </label>
      <p class="dim small">密码只用来换一次登录令牌，不会存下来。</p>
      {#if openid}
        <div class="openid">
          <p class="dim small">在浏览器里打开授权页，输入这个设备码：</p>
          <div class="row">
            <code class="code">{openid.userCode}</code>
            <button class="btn small" onclick={copyUserCode}>复制</button>
            <button class="btn small" onclick={() => void api.openUrl(openid?.verificationUri ?? "")}>打开授权页</button>
          </div>
          {#if openidNote}<p class="dim small">{openidNote}</p>{/if}
        </div>
      {:else}
        <div class="row">
          <button class="btn primary" disabled={busy} onclick={addThirdParty}>
            {busy ? "登录中…" : "登录"}
          </button>
          <button class="btn" disabled={busy} onclick={startOpenid}>用外链登录</button>
          <button class="btn ghost" onclick={() => (addKind = "pick")}>返回</button>
        </div>
      {/if}
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

<Modal open={serverOpen} title="添加认证服务器" onclose={() => (serverOpen = false)}>
  <div class="form">
    <label>
      <span>服务器地址</span>
      <input bind:value={serverUrl} placeholder="https://example.com/api/yggdrasil" />
    </label>
    <p class="dim small">
      地址是认证服务器的 yggdrasil 根地址；加进来之后左栏会多一项，点「添加角色」就能用它的账号登录。
    </p>
    <div class="row">
      <button class="btn primary" disabled={busy} onclick={addServer}>
        {busy ? "正在读取服务器信息…" : "添加"}
      </button>
      <button class="btn ghost" onclick={() => (serverOpen = false)}>取消</button>
    </div>
  </div>
</Modal>

{#snippet skinControls()}
  <div class="skin-preview">
    <SkinModel skin={newSkin} model={newSkinModel} cape={newCape} />
  </div>

  <div class="skin-group">
    <span class="skin-label">官方皮肤</span>
    <div class="skin-grid">
      {#each OFFICIAL_SKINS as item (item.id)}
        <button
          class="skin-cell"
          class:on={newSkinOfficial === item.id}
          title={item.name}
          onclick={() => chooseOfficial(item.id)}
        >
          <span class="skin-head" style="background-image: url({item.wide})"></span>
          <span>{item.name}</span>
        </button>
      {/each}
    </div>
  </div>

  <div class="skin-group">
    <span class="skin-label">手臂</span>
    <div class="skin-row">
      <button class="chip" class:on={newSkinModel !== "slim"} onclick={() => chooseArm("classic")}>粗</button>
      <button class="chip" class:on={newSkinModel === "slim"} onclick={() => chooseArm("slim")}>细</button>
    </div>
  </div>

  <div class="skin-group">
    <span class="skin-label">披风</span>
    <div class="skin-grid">
      <button class="skin-cell cape-cell" class:on={newCape === null} onclick={() => chooseCape(null)}>
        <span class="cape-none">无</span>
      </button>
      {#each OFFICIAL_CAPES as item (item.id)}
        <button
          class="skin-cell cape-cell"
          class:on={newCapeOfficial === item.id}
          title={item.name}
          onclick={() => chooseCape(item.id)}
        >
          <span class="cape-thumb" style="background-image: url({item.image})"></span>
        </button>
      {/each}
      <button class="skin-cell cape-cell" class:on={newCape !== null && newCapeOfficial === null} onclick={pickCapeFile} disabled={skinBusy}>
        <span class="cape-none">文件…</span>
      </button>
    </div>
  </div>

  <div class="skin-row">
    <button class="btn small" disabled={skinBusy} onclick={pickSkinFile}>本地皮肤…</button>
    <button class="btn small" disabled={skinBusy || !littleskinName.trim()} onclick={pickLittleSkin}>
      LittleSkin
    </button>
    <input bind:value={littleskinName} placeholder="LittleSkin 用户名" />
    {#if newSkin || newCape}
      <button class="btn small ghost" onclick={() => { newSkin = null; newSkinOfficial = null; newCape = null; newCapeOfficial = null; }}>清空</button>
    {/if}
  </div>
{/snippet}

<Modal open={skinFor !== null} title="更换皮肤" width="min(560px, 88vw)" onclose={() => (skinFor = null)}>
  <div class="form">
    {@render skinControls()}
    <div class="row">
      <button class="btn primary" disabled={skinBusy} onclick={saveSkin}>保存</button>
      <button class="btn ghost" onclick={() => (skinFor = null)}>取消</button>
    </div>
  </div>
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
    /* 撑满可用高度，别让账户面板缩成一条压在页面顶部。 */
    min-height: 100%;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .head-text h1 {
    margin: 0;
    font-size: 22px;
  }
  .layout {
    display: grid;
    grid-template-columns: 236px minmax(0, 1fr);
    align-items: stretch;
    flex: 1;
    min-height: 0;
    border-radius: 16px;
    border: 1px solid var(--border);
    background: var(--panel);
    overflow: hidden;
  }
  .rail-title {
    margin: 2px 10px 6px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .add-src {
    margin-top: auto;
    justify-content: flex-start;
    border: 1px dashed rgba(255, 255, 255, 0.16);
    color: var(--text-dim);
  }
  .add-src:hover {
    border-color: var(--yb-green-bright);
    color: var(--text);
  }
  .btn.small {
    padding: 4px 10px;
    font-size: 12px;
  }
  .rail {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 12px 10px;
    border-right: 1px solid var(--border);
    background: rgba(255, 255, 255, 0.02);
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
    min-height: 420px;
    min-width: 0;
  }
  .pad {
    padding: 16px;
  }
  .empty {
    flex: 1;
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 12px;
    padding: 48px 16px;
  }
  .empty p {
    margin: 0;
  }
  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
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
    grid-template-columns: repeat(auto-fill, minmax(168px, 1fr));
    gap: 14px;
    padding: 16px;
    align-content: start;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }
  .cards.list {
    grid-template-columns: minmax(0, 1fr);
    gap: 8px;
  }
  .card {
    position: relative;
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid var(--border);
    border-radius: 14px;
    overflow: visible;
    transition: border-color 0.15s ease, background 0.15s ease;
  }
  .card.dragging {
    opacity: 0.45;
  }
  /* 拖动时的插入位：插在前面画上边线，插在后面画下边线。 */
  .card.over-before::before,
  .card.over-after::after {
    content: "";
    position: absolute;
    left: 4px;
    right: 4px;
    height: 2px;
    border-radius: 2px;
    background: var(--yb-green-bright);
    z-index: 6;
    pointer-events: none;
  }
  .card.over-before::before {
    top: -6px;
  }
  .card.over-after::after {
    bottom: -6px;
  }
  .card.grid.over-before::before,
  .card.grid.over-after::after {
    top: 4px;
    bottom: 4px;
    left: auto;
    right: auto;
    width: 2px;
    height: auto;
  }
  .card.grid.over-before::before {
    left: -6px;
  }
  .card.grid.over-after::after {
    right: -6px;
  }
  .card:hover {
    border-color: rgba(255, 255, 255, 0.22);
    background: rgba(255, 255, 255, 0.05);
  }
  .card.active {
    border-color: var(--yb-green-bright);
    box-shadow: 0 0 0 1px var(--yb-green-bright) inset;
    background: rgba(126, 200, 120, 0.08);
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
    /* 右侧留出「更多」+ 拖动把手的位置，别让文字和它们重叠。 */
    padding: 10px 92px 10px 16px;
  }
  /* 列表里名字+类型是一整块，当前紧贴在它右边并垂直居中。 */
  .cards.list .meta {
    flex: 0 1 auto;
    max-width: 62%;
  }
  .cards.list .now {
    align-self: center;
    margin-left: 10px;
  }
  .card.grid {
    overflow: hidden;
  }
  .card.grid .pick {
    flex-direction: column;
    align-items: stretch;
    gap: 0;
    padding: 0;
  }
  .card.grid .stage {
    display: block;
    width: 100%;
    aspect-ratio: 1;
    background: radial-gradient(120% 90% at 50% 12%, rgba(255, 255, 255, 0.1), rgba(0, 0, 0, 0.26));
  }
  .card.grid .meta {
    flex-direction: row;
    align-items: baseline;
    justify-content: center;
    gap: 8px;
    min-height: 42px;
    padding: 7px 46px 9px;
    border-top: 1px solid var(--border);
  }
  .card.grid .name {
    font-size: 13px;
    flex: none;
    min-width: 0;
  }
  .card.grid .sub {
    font-size: 11px;
    white-space: nowrap;
    flex: none;
  }
  .card.grid .now {
    position: absolute;
    right: 10px;
    bottom: 12px;
  }
  .card.grid .meta {
    padding-right: 12px;
    min-width: 0;
  }
  .card.grid.active .meta {
    padding-right: 56px;
    justify-content: center;
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
  .advanced {
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 8px 12px 10px;
  }
  .advanced summary {
    cursor: pointer;
    font-size: 12px;
    color: var(--text-dim);
  }
  .advanced[open] summary {
    margin-bottom: 8px;
  }
  .skin-box {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .skin-label {
    font-size: 12px;
    color: var(--text-dim);
  }
  .skin-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .skin-row input {
    flex: 1;
    min-width: 0;
  }
  .skin-preview {
    height: 200px;
    border-radius: 12px;
    display: grid;
    place-items: center;
    background: radial-gradient(circle at 50% 35%, rgba(120, 220, 160, 0.12), transparent 70%),
      rgba(255, 255, 255, 0.03);
    overflow: hidden;
  }
  .skin-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .skin-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(56px, 1fr));
    gap: 6px;
  }
  .skin-cell {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    padding: 4px 2px 3px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: rgba(255, 255, 255, 0.03);
    color: var(--text-dim);
    font-size: 10px;
    cursor: pointer;
  }
  /* 皮肤表是 64×64 的展开图，缩略图只截脸那一块（8,8 起 8×8）。 */
  .skin-head {
    width: 32px;
    height: 32px;
    background-repeat: no-repeat;
    background-size: 256px 256px;
    background-position: -32px -32px;
    image-rendering: pixelated;
    border-radius: 4px;
  }
  /* 披风贴图里有效区域在 (1,1) 起 10×16。 */
  .cape-thumb {
    width: 20px;
    height: 32px;
    background-repeat: no-repeat;
    background-size: 128px 64px;
    background-position: -2px -2px;
    image-rendering: pixelated;
    border-radius: 3px;
  }
  .skin-cell:hover {
    border-color: rgba(120, 220, 160, 0.5);
  }
  .skin-cell.on {
    border-color: var(--accent);
    background: rgba(120, 220, 160, 0.14);
    color: var(--text);
  }
  .cape-cell {
    min-height: 52px;
    justify-content: center;
  }
  .cape-none {
    font-size: 11px;
  }
  .chip {
    padding: 4px 14px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: rgba(255, 255, 255, 0.03);
    color: var(--text-dim);
    font-size: 12px;
    cursor: pointer;
  }
  .chip.on {
    border-color: var(--accent);
    color: var(--text);
    background: rgba(120, 220, 160, 0.14);
  }
  .openid {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .cards.list .face {
    width: 34px;
    height: 34px;
    border-radius: 8px;
  }
  .face.crop {
    background-size: 800% 800%;
    background-position: -12.5% -12.5%;
    background-repeat: no-repeat;
    background-color: transparent;
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
  .now {
    flex: none;
    padding: 2px 8px;
    border-radius: 999px;
    font-size: 11px;
    color: var(--yb-green-bright);
    background: rgba(126, 200, 120, 0.16);
  }
  .handle {
    position: absolute;
    top: 8px;
    z-index: 5;
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border: none;
    border-radius: 9px;
    background: transparent;
    color: var(--text-dim);
    cursor: grab;
    opacity: 0.55;
    touch-action: none;
    user-select: none;
    -webkit-user-select: none;
    -webkit-touch-callout: none;
    transition: opacity 0.15s ease;
  }
  .card.grid .handle {
    left: 8px;
  }
  .cards.list .handle {
    top: 50%;
    transform: translateY(-50%);
    right: 50px;
  }
  .card:hover .handle,
  .handle:focus-visible,
  .card.dragging .handle {
    opacity: 1;
  }
  .handle:hover {
    background: rgba(255, 255, 255, 0.1);
    color: var(--text);
  }
  /* 触屏没有 hover：把手常显，否则根本找不到能拖的地方。 */
  @media (hover: none) {
    .handle,
    .more {
      opacity: 0.7;
    }
  }
  .handle:active {
    cursor: grabbing;
  }
  /* 六点手柄：两列三点 */
  .grip {
    width: 10px;
    height: 16px;
    background-image: radial-gradient(circle, currentColor 1.1px, transparent 1.2px);
    background-size: 5px 5px;
    background-position: 0 1px;
  }
  .more {
    position: absolute;
    top: 8px;
    right: 10px;
    width: 34px;
    height: 34px;
    border: none;
    border-radius: 9px;
    background: transparent;
    color: var(--text-dim);
    font-size: 19px;
    line-height: 1;
    cursor: pointer;
  }
  .more {
    opacity: 0.55;
    transition: opacity 0.15s ease;
  }
  .cards.list .more {
    top: 50%;
    transform: translateY(-50%);
  }
  .card:hover .more,
  .more:focus-visible {
    opacity: 1;
  }
  .more:hover {
    background: rgba(255, 255, 255, 0.1);
    color: var(--text);
  }
  .menu {
    position: absolute;
    top: 46px;
    right: 10px;
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
  }</style>
