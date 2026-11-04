<script lang="ts">
  // 关于：品牌、版本、技术栈和致谢。全部静态展示，不读写任何配置。
  import { onMount } from "svelte";
  import { api } from "$lib/api";
  import type { LauncherInfo } from "$lib/api";
  import Logo from "$lib/components/Logo.svelte";

  let info = $state<LauncherInfo | null>(null);

  onMount(() => {
    void api
      .info()
      .then((value) => (info = value))
      .catch(() => {});
  });

  const features = [
    "版本即实例",
    "双源竞速下载",
    "断点续传",
    "版本隔离",
    "Forge / NeoForge",
    "Fabric / Quilt",
    "Cleanroom / LiteLoader / OptiFine",
    "离线账户",
    "模组管理",
    "实时日志"
  ];
  import BackButton from "$lib/components/BackButton.svelte";
</script>

<div class="page-head">
  <BackButton />
<h1>关于</h1>
  <p class="dim">延边大学我的世界启动器。一个 exe，双击即用，不写注册表，数据都放在 exe 旁边。</p>
</div>

<section class="panel hero">
  <Logo size={64} />
  <div class="hero-text">
    <h2>YBUMCL</h2>
    <p class="dim">便携式 Minecraft 启动器 · Tauri 2 + SvelteKit 5 + Rust</p>
    <p class="ver">
      版本 <code>{info?.version ?? "…"}</code>
      {#if info?.portable}<span class="tag">便携模式</span>{/if}
    </p>
  </div>
</section>

<section class="panel">
  <h2>能做什么</h2>
  <ul class="chips">
    {#each features as feature (feature)}
      <li>{feature}</li>
    {/each}
  </ul>
  <p class="dim small">
    下载走官方源与镜像竞速，文件按 SHA1 校验；装好的版本在「版本」页里单独调内存、Java 和玩家名。
  </p>
</section>

<section class="panel">
  <h2>数据放在哪</h2>
  <dl class="paths">
    <div><dt>程序目录</dt><dd><code>{info?.exeDir ?? "…"}</code></dd></div>
    <div><dt>数据目录</dt><dd><code>{info?.dataDir ?? "…"}</code></dd></div>
    <div><dt>游戏目录</dt><dd><code>{info?.gameDir ?? "…"}</code></dd></div>
  </dl>
  <p class="dim small">
    整个文件夹可以直接改名或搬走，配置里记的是相对位置。想换地方就用
    <code>YBUMCL.exe --data-dir &lt;目录&gt;</code> 启动一次。
  </p>
</section>

<section class="panel">
  <h2>致谢</h2>
  <p class="dim">
    下载加速与版本清单来自 <a href="https://bmclapi2.bangbang93.com/" target="_blank" rel="noreferrer">BMCLAPI</a>、阿里云与 Cleanroom 的 maven 镜像；
    安装器走 ghfast 代理。游戏文件版权归 Mojang 与各自作者，本启动器只从官方或官方授权镜像下载，不分发任何游戏资源。
  </p>
</section>

<style>
  .page-head {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 14px;
  }
  .page-head h1 {
    margin: 0;
    font-size: 24px;
  }
  .dim {
    color: var(--text-dim);
    margin: 0;
  }
  .small {
    font-size: 12.5px;
  }
  .panel {
    padding: 16px 18px;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: rgba(16, 19, 26, 0.5);
    margin-bottom: 14px;
  }
  .panel h2 {
    margin: 0 0 10px;
    font-size: 16px;
  }
  code {
    font-family: var(--mono, ui-monospace, "Cascadia Mono", Consolas, monospace);
    font-size: 12.5px;
  }
  a {
    color: var(--accent);
  }
  a:hover {
    text-decoration: underline;
  }

  .hero {
    display: flex;
    align-items: center;
    gap: 18px;
  }
  .hero-text h2 {
    margin: 0 0 2px;
    font-size: 26px;
    letter-spacing: 0.06em;
  }
  .ver {
    margin: 8px 0 0;
    font-size: 13px;
    color: var(--text-dim);
  }
  .tag {
    margin-left: 8px;
    padding: 2px 8px;
    border-radius: 999px;
    font-size: 11.5px;
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 30%, transparent);
  }

  .chips {
    list-style: none;
    margin: 0 0 10px;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .chips li {
    padding: 4px 11px;
    border-radius: 999px;
    font-size: 12.5px;
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid var(--border);
  }

  .paths {
    margin: 0 0 10px;
    display: grid;
    gap: 8px;
  }
  .paths div {
    display: grid;
    grid-template-columns: 84px 1fr;
    align-items: baseline;
    gap: 10px;
  }
  .paths dt {
    color: var(--text-dim);
    font-size: 12.5px;
  }
  .paths dd {
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
