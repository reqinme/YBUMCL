<script lang="ts">
  import { announce } from "$lib/tasks.svelte";
  import { showError } from "$lib/dialogs.svelte";
  import Dropdown from "$lib/components/Dropdown.svelte";
  /// 装了多套同版本 JDK 时，光看版本号分不清，补上发行目录名（Zulu / jdk-17.0.20 …）。
  function javaLabel(java: { major: number; version: string; path: string }): string {
    const parts = java.path.replaceAll("\\", "/").split("/").filter(Boolean);
    const vendor = parts.length > 2 ? parts[parts.length - 3] : "";
    return `Java ${java.major} · ${java.version}${vendor ? ` · ${vendor}` : ""}`;
  }

  import { onMount } from "svelte";
    import { cachedJava } from "$lib/detect";
  import { api, type JavaInstallation, type LauncherInfo, type Settings } from "$lib/api";
  import { notifySettingsSaved } from "$lib/settings-live.svelte";
  import { speedTierOptions } from "$lib/speed";
  import { setBallColor } from "$lib/install-flight.svelte";

  /// 安装光球的预设配色，第一个就是主题绿。
  const ballPresets = [
    { value: "#a8cc84", label: "主题绿" },
    { value: "#6fd3c9", label: "青" },
    { value: "#6aa8ff", label: "蓝" },
    { value: "#b48cff", label: "紫" },
    { value: "#f0c46a", label: "金" },
    { value: "#ff7a6a", label: "红" }
  ];

  let settings = $state<Settings | null>(null);
  let info = $state<LauncherInfo | null>(null);
  let javas = $state<JavaInstallation[]>([]);
  let busy = $state("");

  async function load() {
    try {
      [settings, info, javas] = await Promise.all([
        api.loadSettings(),
        api.info(),
        cachedJava().then((cached) => cached.value)
      ]);
      setBallColor(settings?.installBallColor);
    } catch (e) {
      showError("读取设置失败", "设置文件或者环境信息没有读出来。", String(e));
    }
  }

  async function save() {
    if (!settings) return;
    busy = "save";
    try {
      settings = await api.saveSettings(settings);
      notifySettingsSaved();
      announce("设置已保存");
    } catch (e) {
      showError("保存设置失败", "设置没有写进配置文件。", String(e));
    } finally {
      busy = "";
    }
  }

  onMount(load);
  import BackButton from "$lib/components/BackButton.svelte";
</script>

<div class="page">
  <header class="head">
    <BackButton />
<h1>设置</h1>
    <div class="spacer"></div>
    <button class="btn primary" onclick={save} disabled={!settings || busy === "save"}>
      {busy === "save" ? "保存中…" : "保存"}
    </button>
  </header>

  {#if settings}
    <section class="panel block">
      <h2>游戏</h2>
      <div class="grid">
        <label>
          <span>默认内存 (MB)</span>
          <input type="number" min="512" max="32768" step="256" bind:value={settings.memoryMb} />
        </label>
        <label>
          <span>默认 Java</span>
          <Dropdown
            bind:value={settings.javaPath}
            label="默认 Java"
            options={[
              { value: "", label: "每次自动选择" },
              ...javas.map((java) => ({ value: java.path, label: javaLabel(java) }))
            ]}
          />
        </label>
      </div>
    </section>

    <section class="panel block">
      <h2>下载</h2>
      <div class="grid">
        <label>
          <span>下载源策略</span>
          <Dropdown
            bind:value={settings.downloadStrategy}
            label="下载源策略"
            options={[
              { value: "sequential", label: "顺序换源（默认，最稳）" },
              { value: "race", label: "两者竞速（先到先用，请求最多）" },
              { value: "official-first", label: "官方源优先" },
              { value: "mirror-first", label: "镜像源优先" },
              { value: "mirror-only", label: "仅镜像源" },
              { value: "official-only", label: "仅官方源" }
            ]}
          />
        </label>
        <label>
          <span>同时下载的文件数（1–256）</span>
          <input type="number" min="1" max="256" bind:value={settings.concurrentDownloads} />
        </label>
        <label>
          <span>下载协议</span>
          <Dropdown
            value={settings.useHttp2 ? "h2" : "h1"}
            label="下载协议"
            onchange={(next) => {
              if (settings) settings.useHttp2 = next === "h2";
            }}
            options={[
              { value: "h1", label: "HTTP/1.1（默认）" },
              { value: "h2", label: "HTTP/2" }
            ]}
          />
        </label>
        <label>
          <span>速度限制</span>
          <Dropdown
            value={String(settings.downloadSpeed)}
            label="速度限制"
            onchange={(next) => {
              if (settings) settings.downloadSpeed = Number(next);
            }}
            options={speedTierOptions()}
          />
        </label>
      </div>
      <label>
        <span>版本隔离</span>
        <Dropdown
          value={settings.isolationMode}
          label="版本隔离"
          onchange={(next) => {
            if (settings) settings.isolationMode = next as "all" | "off";
          }}
          options={[
            { value: "all", label: "隔离版本" },
            { value: "off", label: "不隔离版本" }
          ]}
        />
      </label>
      <label class="row">
        <input type="checkbox" bind:checked={settings.reuseGameFolders} />
        <span>复用其它游戏目录里下过的资源</span>
      </label>
      <p class="dim">
        官方源在校园网里可能很慢；镜像由社区维护，与官方内容一致（按 SHA1 校验）。只有存在镜像的文件才按这里的策略走，没有镜像的条目（比如原版清单）一律直接连官方。同时下载的文件数调大通常更快。
      </p>
    </section>

    <section class="panel block">
      <h2>内容下载</h2>
      <div class="grid">
        <label>
          <span>内容列表每页条数（10–50）</span>
          <input type="number" min="10" max="50" bind:value={settings.pageSize} />
        </label>
        <label>
          <span>内容页左栏宽度（25–65 %）</span>
          <input type="number" min="25" max="65" bind:value={settings.splitRatio} />
        </label>
      </div>
      <label>
        <span>社区资源的文件名格式</span>
        <Dropdown
          value={String(settings.fileNameFormat)}
          label="社区资源的文件名格式"
          onchange={(next) => {
            if (settings) settings.fileNameFormat = Number(next);
          }}
          options={[
            { value: "0", label: "【中文名】原文件名", hint: "【钠】sodium-fabric-0.9.3.jar" },
            { value: "1", label: "[中文名] 原文件名", hint: "[钠] sodium-fabric-0.9.3.jar" },
            { value: "2", label: "中文名-原文件名", hint: "钠-sodium-fabric-0.9.3.jar" },
            { value: "3", label: "原文件名-中文名", hint: "sodium-fabric-0.9.3-钠.jar" },
            { value: "4", label: "原文件名（默认）", hint: "sodium-fabric-0.9.3.jar" }
          ]}
        />
      </label>
      <label>
        <span>实例模组列表的排版</span>
        <Dropdown
          value={String(settings.modNameLayout)}
          label="实例模组列表的排版"
          onchange={(next) => {
            if (settings) settings.modNameLayout = Number(next);
          }}
          options={[
            { value: "0", label: "名称在上（默认）", hint: "钠 | Sodium | 0.9.3 · neoforge / sodium-fabric-0.9.3.jar" },
            { value: "1", label: "文件名在上", hint: "sodium-fabric-0.9.3.jar / 钠 | Sodium | 0.9.3 · neoforge" }
          ]}
        />
      </label>
      <label>
        <span>装模组时缺前置怎么办</span>
        <Dropdown
          value={String(settings.missingDepsMode)}
          label="装模组时缺前置怎么办"
          onchange={(next) => {
            if (settings) settings.missingDepsMode = Number(next);
          }}
          options={[
            { value: "0", label: "弹窗询问（默认）", hint: "装完列出缺的前置，可一起装" },
            { value: "1", label: "直接下载前置", hint: "按目标版本和加载器自动挑文件装进 mods" },
            { value: "2", label: "什么都不做", hint: "只在下載页详情里显示前置" }
          ]}
        />
      </label>
      <label class="row">
        <input type="checkbox" bind:checked={settings.askSaveLocation} />
        <span>下载时询问保存位置</span>
      </label>
      <label class="row">
        <input type="checkbox" bind:checked={settings.compatWarning} />
        <span>下载的模组 / 资源包与实例版本不匹配时，给黄字提示（仍然允许下载）</span>
      </label>
      <label class="row">
        <input class="num" type="number" min="0" max="600" step="20" bind:value={settings.navSlideMs} />
        <span>导航切换动画时长（毫秒，0 = 不滑动；默认 220）</span>
      </label>
      <label class="row">
        <input type="checkbox" bind:checked={settings.ignoreQuilt} />
        <span>安装时忽略 Quilt（只支持 Quilt 的文件不显示；给 Fabric 也兼容的文件照常显示）</span>
      </label>
      <label class="row">
        <input type="checkbox" bind:checked={settings.modpackInstall} />
        <span>整合包下载后直接安装成实例</span>
      </label>
      <label class="row">
        <input type="checkbox" bind:checked={settings.worldExtract} />
        <span>世界包下载后自动解压到 saves</span>
      </label>
      <p class="dim">
        建议 3：同一时间跑三个下载足够快；开太多会被镜像限速，反而整体更慢。
        文件名格式与 PCL 一致；中文名取自社区整理的名字表（11,603 条），没有译名的模组仍用原文件名。
      </p>
    </section>

    <section class="panel block">
      <h2>外观</h2>
      <label class="row">
        <input type="checkbox" bind:checked={settings.showSplash} />
        <span>启动时播放开场动画（也可以点任意处跳过）</span>
      </label>

      <div class="ball-pick">
        <span class="ball-preview" style="--c: {settings.installBallColor}" aria-hidden="true"></span>
        <div class="ball-copy">
          <strong>安装光球颜色</strong>
          <p class="dim">点「安装」后飞向顶栏下载圆环的那颗球。换颜色后，球的流光和落地光晕一起变。</p>
          <div class="swatches">
            {#each ballPresets as preset (preset.value)}
              <button
                type="button"
                class="swatch"
                class:on={settings.installBallColor === preset.value}
                style="--c: {preset.value}"
                title={preset.label}
                aria-label={preset.label}
                onclick={() => {
                  if (!settings) return;
                  settings.installBallColor = preset.value;
                  setBallColor(preset.value);
                }}
              ></button>
            {/each}
            <label class="custom" title="自定义颜色">
              <input
                type="color"
                bind:value={settings.installBallColor}
                oninput={() => settings && setBallColor(settings.installBallColor)}
              />
              <span>自定义</span>
            </label>
          </div>
        </div>
      </div>
    </section>
  {/if}

  {#if info}
    <section class="panel block about">
      <h2>关于</h2>
      <dl>
        <div><dt>版本</dt><dd>v{info.version}</dd></div>
        <div><dt>模式</dt><dd>{info.portable ? "便携（数据在 exe 旁）" : "独立数据目录"}</dd></div>
        <div><dt>数据目录</dt><dd class="mono">{info.dataDir}</dd></div>
        <div><dt>游戏目录</dt><dd class="mono">{info.gameDir}</dd></div>
      </dl>
      <p class="dim">
        延边大学我的世界启动器 · Yanbian University Minecraft Launcher（YBUMCL）。
        本启动器不分发游戏本体，所有文件均从官方或镜像源下载。
      </p>
    </section>
  {/if}
</div>

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
  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 650;
  }
  h2 {
    margin: 0 0 12px;
    font-size: 15px;
    font-weight: 600;
  }
  /* 安装光球：预览球和真正飞的那颗用同一套渐变。 */
  .ball-pick {
    display: flex;
    gap: 14px;
    align-items: flex-start;
    margin-top: 14px;
    padding: 4px 0 0;
  }
  .ball-preview {
    flex: none;
    width: 38px;
    height: 38px;
    border-radius: 50%;
    background:
      radial-gradient(circle at 34% 28%, rgba(255, 255, 255, 0.92) 0%, rgba(255, 255, 255, 0) 38%),
      radial-gradient(
        circle at 50% 56%,
        color-mix(in srgb, var(--c) 38%, #05070a 62%) 0%,
        var(--c) 56%,
        color-mix(in srgb, var(--c) 42%, #ffffff 58%) 100%
      );
    box-shadow:
      0 0 16px 3px color-mix(in srgb, var(--c) 50%, transparent),
      inset 0 -4px 9px rgba(0, 0, 0, 0.38);
  }
  .ball-copy {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .ball-copy strong {
    font-size: 13px;
    font-weight: 600;
  }
  .ball-copy p {
    margin: 0;
    font-size: 12px;
    line-height: 1.6;
  }
  .swatches {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 2px;
  }
  .swatch {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    border: none;
    padding: 0;
    cursor: pointer;
    background: var(--c);
    box-shadow: inset 0 -3px 6px rgba(0, 0, 0, 0.35);
    outline: 2px solid transparent;
    outline-offset: 2px;
    transition:
      outline-color 0.15s ease,
      transform 0.15s ease;
  }
  .swatch:hover {
    transform: translateY(-1px);
  }
  .swatch.on {
    outline-color: rgba(255, 255, 255, 0.75);
  }
  .custom {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: rgba(255, 255, 255, 0.6);
    cursor: pointer;
  }
  .custom input {
    width: 26px;
    height: 22px;
    padding: 0;
    border: 1px solid rgba(255, 255, 255, 0.18);
    border-radius: 6px;
    background: transparent;
    cursor: pointer;
  }

  .spacer {
    flex: 1;
  }
  .block {
    padding: 16px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 12px 16px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  label.row {
    flex-direction: row;
    align-items: center;
    gap: 8px;
    grid-column: span 2;
  }
  label span {
    font-size: 12px;
    color: var(--text-dim);
  }
  label.row span {
    font-size: 13px;
    color: var(--text);
  }
  input {
    padding: 8px 10px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: rgba(0, 0, 0, 0.32);
    color: var(--text);
    font: inherit;
    outline: none;
  }
  input.num {
    width: 96px;
    flex: none;
  }
  input[type="checkbox"] {
    width: 16px;
    height: 16px;
    padding: 0;
    accent-color: var(--yb-green);
  }
  input:focus {
    border-color: var(--border-strong);
  }
  dl {
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
    font-size: 13px;
  }
  dl div {
    display: flex;
    gap: 12px;
  }
  dt {
    color: var(--text-dim);
    min-width: 64px;
  }
  dd {
    margin: 0;
    -webkit-user-select: text;
    user-select: text;
  }
  .mono {
    font-family: "Cascadia Mono", ui-monospace, monospace;
    font-size: 12px;
  }
  .dim {
    color: var(--text-dim);
    font-size: 12px;
    margin: 0;
    line-height: 1.7;
  }
  .about .dim {
    margin-top: 12px;
  }
</style>
