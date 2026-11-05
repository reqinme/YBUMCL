<script lang="ts">
  import { onMount } from "svelte";
  import { api, type ExportOptionView } from "$lib/api";
  import { announce, beginTask, endTask, setTaskOutput } from "$lib/tasks.svelte";
  import { showError } from "$lib/dialogs.svelte";
  import ConfirmDialog from "$lib/components/ConfirmDialog.svelte";

  type Props = {
    id: string;
  };

  let { id }: Props = $props();

  let options = $state<ExportOptionView[]>([]);
  let loading = $state(true);
  let name = $state("");
  let version = $state("1.0.0");
  let includeLauncher = $state(true);
  let packResources = $state(false);
  let modrinthOnly = $state(false);
  let checked = $state<Record<string, boolean>>({});
  let entryChecked = $state<Record<string, boolean>>({});
  let target = $state("");
  let busy = $state(false);
  let stage = $state("");

  /// 读过配置文件后的覆盖状态：规则和保存位置都按配置走，左栏不再可勾。
  let override = $state<{ path: string; packPath: string; rules: string[] } | null>(null);
  /// 识别失败时的「继续 / 取消」。
  let ask = $state<{ total: number; found: number } | null>(null);
  let askResolve: ((value: boolean) => void) | null = null;

  const visible = $derived(options.filter((item) => item.visible));
  const roots = $derived(visible.filter((item) => item.parent === null));
  const hasOptifine = $derived(
    options.some((item) => item.id === "optifine_settings" && item.visible),
  );
  const suffix = $derived(includeLauncher ? "zip" : "mrpack");
  const shownName = $derived(name.trim() || id);
  const shownVersion = $derived(version.trim() || "1.0.0");

  function children(parent: string) {
    return visible.filter((item) => item.parent === parent);
  }

  /// 动态项的规则形如 `resourcepacks/`，其余逐条给出文件规则。
  function folderOf(option: ExportOptionView) {
    const rule = option.rules.find((line) => line.endsWith("/"));
    return rule ? rule.replace(/\/$/, "") : "";
  }

  function toggle(option: ExportOptionView, value: boolean) {
    checked = { ...checked, [option.id]: value };
    if (!option.dynamic || !value) return;
    const next = { ...entryChecked };
    for (const entry of option.entries) {
      if (next[entry.rule] === undefined) next[entry.rule] = true;
    }
    entryChecked = next;
  }

  function manualRules(): string[] {
    const lines: string[] = [];
    for (const option of visible) {
      if (!checked[option.id]) continue;
      if (option.parent !== null && !checked[option.parent]) continue;
      if (option.dynamic) {
        const folder = folderOf(option);
        if (folder) lines.push(`${folder}/`);
        for (const entry of option.entries) {
          if (entryChecked[entry.rule] === false) lines.push(`!${entry.rule}`);
        }
      } else {
        lines.push(...option.rules);
      }
    }
    return lines;
  }

  function rules(): string[] {
    return override ? override.rules : manualRules();
  }

  async function load() {
    loading = true;
    try {
      const list = await api.exportPackLayout(id);
      options = list;
      const next: Record<string, boolean> = {};
      const entries: Record<string, boolean> = {};
      for (const item of list) {
        next[item.id] = item.checked;
        if (!item.dynamic) continue;
        for (const entry of item.entries) entries[entry.rule] = true;
      }
      checked = next;
      entryChecked = entries;
    } catch (error) {
      showError("读不出导出选项", "这个实例的目录没扫成。", String(error));
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    name = id;
    void load();
  });

  async function readConfig() {
    try {
      const config = await api.exportPackLoadConfig();
      if (!config) return;
      if (config.name.trim()) name = config.name.trim();
      if (config.version.trim()) version = config.version.trim();
      includeLauncher = config.includeLauncher;
      packResources = config.packResources;
      modrinthOnly = config.modrinthOnly;
      target = config.packPath ?? "";
      // 第三段是追加文件，PCL 里写成 `来源|目标`，我们只取来源那半边当规则。
      const extras = config.extraFiles
        .map((line) => line.split("|")[0]?.trim() ?? "")
        .filter((line) => line.length > 0);
      const lines = [...config.rules, ...extras].filter((line) => {
        const text = line.trim();
        return text.length > 0 && !text.startsWith("#");
      });
      override = { path: config.configPath, packPath: config.packPath ?? "", rules: lines };
      announce(`已读取配置 ${config.configPath}`);
    } catch (error) {
      showError("读取配置失败", "这份配置文件没有读成。", String(error));
    }
  }

  async function saveConfig() {
    try {
      const saved = await api.exportPackSaveConfig({
        name: shownName,
        version: shownVersion,
        includeLauncher,
        packResources,
        modrinthOnly,
        packPath: target || null,
        rules: manualRules(),
        extraFiles: [],
      });
      if (saved) announce(`已保存配置 ${saved}`);
    } catch (error) {
      showError("保存配置失败", "配置文件没有写成。", String(error));
    }
  }

  function clearOverride() {
    override = null;
  }

  function continueAsk(found: number, total: number): Promise<boolean> {
    ask = { found, total };
    return new Promise((resolve) => {
      askResolve = resolve;
    });
  }

  function answerAsk(value: boolean) {
    askResolve?.(value);
    askResolve = null;
    ask = null;
  }

  async function run() {
    if (busy) return;
    busy = true;
    stage = "正在整理文件";
    const taskId = beginTask("导出整合包", `${shownName} · ${shownVersion}`);
    let token = 0;
    try {
      let out = override?.packPath ?? target;
      if (!out) {
        const picked = await api.exportPackPickPath(`${shownName} ${shownVersion}.${suffix}`, includeLauncher);
        if (!picked) {
          stage = "";
          endTask(taskId, "已取消");
          return;
        }
        out = picked;
      }
      target = out;

      token = await api.exportPackBegin(id, rules(), taskId);
      if (!packResources) {
        stage = "正在识别来源";
        try {
          const [found, total] = await api.exportPackIdentify(token, modrinthOnly);
          stage = `识别到 ${found}/${total} 个文件`;
        } catch (error) {
          stage = "";
          const ok = await continueAsk(0, 0);
          if (!ok) {
            void api.exportPackCancel(token);
            endTask(taskId, "已取消");
            return;
          }
          void error;
        }
      }
      stage = "正在打包";
      const written = await api.exportPackFinish({
        id,
        token,
        name: shownName,
        version: shownVersion,
        includeLauncher,
        taskId,
        target: out,
      });
      stage = "";
      if (written) {
        setTaskOutput(taskId, { path: written, kind: "file" });
        endTask(taskId, "已导出整合包");
        announce(`已导出 ${written}`);
      } else {
        endTask(taskId, "已导出整合包");
      }
    } catch (error) {
      stage = "";
      if (token) void api.exportPackCancel(token);
      endTask(taskId, "导出失败", true);
      showError("导出失败", "整合包没有写出来。", String(error));
    } finally {
      busy = false;
    }
  }
</script>

<section class="wrap">
  <div class="cols">
    <div class="list">
      <div class="head">
        <h3>导出内容</h3>
        {#if override}
          <button type="button" class="link" onclick={clearOverride}>取消覆盖</button>
        {/if}
      </div>
      {#if override}
        <p class="dim">这份导出按配置里的规则走，勾选列表先收起。配置：{override.path}</p>
      {:else if loading}
        <p class="dim">正在扫描实例目录</p>
      {:else}
        <ul>
          {#each roots as option (option.id)}
            <li class:group={option.rules.length === 0}>
              <label class="row">
                <input
                  type="checkbox"
                  checked={checked[option.id]}
                  onchange={(event) => toggle(option, event.currentTarget.checked)}
                />
                <span class="title">{option.title}</span>
                {#if option.description}<span class="desc">{option.description}</span>{/if}
              </label>
              {#each children(option.id) as child (child.id)}
                <label class="row child">
                  <input
                    type="checkbox"
                    checked={checked[child.id]}
                    onchange={(event) => toggle(child, event.currentTarget.checked)}
                  />
                  <span class="title">{child.title}</span>
                  {#if child.description}<span class="desc">{child.description}</span>{/if}
                </label>
              {/each}
              {#if option.dynamic && checked[option.id]}
                <ul class="entries">
                  {#each option.entries as entry (entry.rule)}
                    <li>
                      <label class="row entry">
                        <input
                          type="checkbox"
                          checked={entryChecked[entry.rule] !== false}
                          onchange={(event) =>
                            (entryChecked = {
                              ...entryChecked,
                              [entry.rule]: event.currentTarget.checked,
                            })}
                        />
                        <span class="title">{entry.label}</span>
                        {#if entry.hint}<span class="desc">{entry.hint}</span>{/if}
                      </label>
                    </li>
                  {/each}
                </ul>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>

    <div class="side">
      <label class="field">
        <span>整合包名称</span>
        <input bind:value={name} maxlength="64" />
      </label>
      <label class="field">
        <span>整合包版本</span>
        <input bind:value={version} maxlength="16" />
      </label>

      <div class="group">
        <h3>高级选项</h3>
        <label class="check">
          <input type="checkbox" bind:checked={packResources} />
          <span>打包资源文件，以避免在导入时下载</span>
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={modrinthOnly} />
          <span>仅从 Modrinth 下载资源文件</span>
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={includeLauncher} />
          <span>把启动器一起打进去</span>
        </label>
        {#if hasOptifine}
          <p class="warn">OptiFine 不进索引，安装的人要自己补。</p>
        {/if}
      </div>

      <div class="row-buttons">
        <button type="button" class="btn" onclick={() => void readConfig()}>读取配置</button>
        <button type="button" class="btn" onclick={() => void saveConfig()}>保存配置</button>
      </div>

      {#if stage}<p class="dim">{stage}</p>{/if}
      <button
        type="button"
        class="btn primary wide"
        disabled={busy || loading}
        onclick={() => void run()}
      >
        {busy ? "导出中…" : "开始导出"}
      </button>
    </div>
  </div>
</section>

<ConfirmDialog
  open={ask !== null}
  message="有一批文件没在 Modrinth / CurseForge 上认出来，继续的话会把它们原样打进 overrides。"
  confirmLabel="继续"
  cancelLabel="取消"
  oncancel={() => answerAsk(false)}
  onconfirm={() => answerAsk(true)}
/>

<style>
  .wrap {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }

  .cols {
    display: grid;
    grid-template-columns: minmax(0, 1.4fr) minmax(300px, 1fr);
    gap: 14px;
    flex: 1;
    min-height: 0;
  }

  .list,
  .side {
    background: var(--surface-2, rgb(0 0 0 / 0.22));
    border: 1px solid var(--border, rgb(255 255 255 / 0.08));
    border-radius: var(--radius-lg, 16px);
    padding: 14px 16px;
    min-height: 0;
    overflow: auto;
  }

  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  h3 {
    margin: 0 0 8px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-dim, #9aa0aa);
  }

  .link {
    border: 0;
    background: none;
    padding: 0;
    margin-bottom: 8px;
    color: var(--yb-green, #7bd88f);
    font-size: 12px;
    cursor: pointer;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 0;
    cursor: pointer;
    font-size: 13px;
  }

  .row.child,
  .row.entry {
    padding-left: 22px;
  }

  .entries {
    padding-left: 22px;
  }

  .group > .row {
    cursor: default;
    color: var(--text-dim, #9aa0aa);
    padding-top: 10px;
  }

  .title {
    color: var(--text, #e6e8ec);
    white-space: nowrap;
  }

  .desc {
    color: var(--text-dim, #8d93a0);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .side {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim, #9aa0aa);
  }

  .field input {
    height: 34px;
    padding: 0 10px;
    border-radius: var(--radius-sm, 8px);
    border: 1px solid var(--border, rgb(255 255 255 / 0.1));
    background: rgb(0 0 0 / 0.25);
    color: var(--text, #e6e8ec);
    font-size: 13px;
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-top: 4px;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text, #e6e8ec);
  }

  .warn {
    margin: 0;
    font-size: 12px;
    color: #f0c674;
  }

  .row-buttons {
    display: flex;
    gap: 8px;
  }

  .row-buttons .btn {
    flex: 1;
  }

  .dim {
    margin: 0;
    font-size: 12px;
    color: var(--text-dim, #8d93a0);
  }

  .btn {
    height: 32px;
    padding: 0 12px;
    border-radius: var(--radius-sm, 8px);
    border: 1px solid var(--border, rgb(255 255 255 / 0.12));
    background: rgb(255 255 255 / 0.06);
    color: var(--text, #e6e8ec);
    font-size: 12px;
    cursor: pointer;
    white-space: nowrap;
  }

  .btn.primary {
    border-color: transparent;
    background: var(--yb-green, #4caf50);
    color: #0d1210;
    font-weight: 600;
  }

  .btn.wide {
    margin-top: auto;
    height: 38px;
    font-size: 13px;
  }

  .btn:disabled {
    opacity: 0.55;
    cursor: default;
  }
</style>
