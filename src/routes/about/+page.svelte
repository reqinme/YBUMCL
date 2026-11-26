<script lang="ts">
  import { creditIcon } from "$lib/credit-icons";
  // 关于：品牌、帮助、鸣谢与法律信息。左侧分类，右侧内容。
  import { onMount } from "svelte";
  import { api } from "$lib/api";
  import type { LauncherInfo, ReleaseInfo } from "$lib/api";
  import { showError } from "$lib/dialogs.svelte";
  import Logo from "$lib/components/Logo.svelte";
  import logoMark from "$lib/assets/logo.png";
  import { localSiteIcon } from "$lib/site-icons";
  import BackButton from "$lib/components/BackButton.svelte";

  let info = $state<LauncherInfo | null>(null);
  let update = $state<ReleaseInfo | null>(null);
  let checking = $state(false);
  /// 开发者头像：后端优先读本地缓存，第一次才联网。
  let avatar = $state<string | null>(null);

  async function check() {
    if (checking) return;
    checking = true;
    try {
      update = await api.checkUpdate();
    } catch (error) {
      update = null;
      showError("检查更新失败", "没能连上 GitHub，或者返回的内容看不懂。", String(error));
    } finally {
      checking = false;
    }
  }
  let view = $state<"about" | "docs" | "thanks" | "legal">("about");

  const CATEGORIES: { id: typeof view; title: string }[] = [
    { id: "about", title: "关于" },
    { id: "docs", title: "文档与帮助" },
    { id: "thanks", title: "鸣谢" },
    { id: "legal", title: "法律信息" }
  ];


  /// 相关网站：按类别分组，组内按名称排序。
  type Site = { name: string; url: string; desc: string; group: string };
  const SITES: Site[] = [
    // 官方
    { name: "Minecraft 官网", url: "https://www.minecraft.net/", desc: "正版购买与官方公告", group: "官方" },
    { name: "Minecraft 中文 Wiki", url: "https://zh.minecraft.wiki/", desc: "合成、机制与版本改动速查", group: "百科" },
    // 中文社区
    { name: "BBSMC", url: "https://bbsmc.net/", desc: "模组、整合包与光影的资源社区", group: "社区" },
    { name: "MCBBS", url: "https://www.mcbbs.co/forum.php", desc: "老牌综合论坛，现以存档与交流为主", group: "社区" },
    { name: "Reden", url: "https://redenmc.com/", desc: "技术向 MC 平台", group: "社区" },
    { name: "XyeBBS", url: "https://www.xyebbs.com/", desc: "中文讨论与技术交流论坛", group: "社区" },
    { name: "拾木空间", url: "https://www.mcimg.com/", desc: "投影文件下载与建筑交流", group: "资源" },
    { name: "联壁计划", url: "https://barrierslink.cn/", desc: "海外地图汉化的公益团队", group: "组织" },
    { name: "苦力怕论坛", url: "https://klpbbs.com/", desc: "国内玩家社区，地图与整合包分享", group: "社区" },
    { name: "红石中继站", url: "https://forum.mczwlt.net/", desc: "资源与作品分享社区", group: "社区" },
    { name: "红河素材站", url: "https://honghemc.cn/", desc: "Minecraft 图标与素材库", group: "资源" },
    // 模组与整合包
    { name: "CurseForge", url: "https://www.curseforge.com/minecraft", desc: "模组、整合包与资源包的最大发布站", group: "资源" },
    { name: "FTB", url: "https://www.feed-the-beast.com/", desc: "老牌整合包团队与整合包下载", group: "资源" },
    { name: "MC 百科", url: "https://www.mcmod.cn/", desc: "国内模组与整合包的中文资料库", group: "百科" },
    { name: "Modrinth", url: "https://modrinth.com/", desc: "开源模组与数据包的发布平台", group: "资源" },
    // 皮肤
    { name: "LittleSkin", url: "https://littleskin.cn/", desc: "快速、可靠的 Minecraft 皮肤站", group: "皮肤" },
    { name: "RedstoneSkin", url: "https://mcskin.com.cn/", desc: "红石皮肤站", group: "皮肤" },
    // 工具
    { name: "Blockbench", url: "https://www.blockbench.net/", desc: "建模与材质编辑工具", group: "工具" },
    { name: "Chunkbase", url: "https://www.chunkbase.com/", desc: "种子地图、结构与生物群系查询", group: "工具" },
    { name: "MCS 蓝图站", url: "https://www.mcschematic.top/", desc: "蓝图与投影文件下载", group: "资源" },
    { name: "Minecraft Search", url: "https://minecraftsearch.com/zh-CN", desc: "物品、生物与方块查询", group: "工具" },
    { name: "Textcraft", url: "https://textcraft.net/", desc: "像素风文字与 Logo 生成器", group: "工具" },
    { name: "盔甲纹饰生成器", url: "https://www.armortrims.com/", desc: "盔甲纹饰搭配与预览", group: "工具" },
    // 建筑与作品
    { name: "MC 建筑库", url: "https://mcblock.top/", desc: "建筑与地图作品", group: "资源" },
    { name: "Planet Minecraft", url: "https://www.planetminecraft.com/", desc: "国际作品与皮肤社区", group: "资源" },
    // 组织
    { name: "MC 高校联盟", url: "https://www.mualliance.cn/", desc: "高校 MC 社团的联合组织", group: "组织" },
    { name: "YBUMC", url: "https://ybumua.homes/", desc: "延边大学方块同好会", group: "组织" }
  ];

  /// 资源组里先按这个顺序排，没列到的按名称。
  const SITE_ORDER: Record<string, number> = { CurseForge: 0, Modrinth: 1, FTB: 2 };

  /// 法律信息：一行一条，右侧按需给一个跳转按钮（照 SJMCL 的法律信息页）。
  type LegalItem = { name: string; desc: string; url?: string; action?: string };
  const LEGAL: LegalItem[] = [
    { name: "版权", desc: "版权所有 © 2026 YBUMCL 开发团队" },
    {
      name: "用户协议",
      desc: "使用本软件即表示同意 Minecraft 最终用户许可协议，并自行承担使用风险",
      url: "https://www.minecraft.net/eula",
      action: "查看"
    },
    {
      name: "开源协议",
      desc: "本项目遵循 GNU General Public License v3.0 协议（或更新版本）",
      url: "https://github.com/reqinme/YBUMCL/blob/main/LICENSE",
      action: "查看"
    },
    {
      name: "非官方声明",
      desc: "与 Mojang Studios、Microsoft 没有隶属或授权关系；Minecraft 是 Mojang Studios 的商标"
    },
    {
      name: "第三方内容",
      desc: "通过本启动器下载的模组、资源包、光影包、数据包与整合包，版权归各自作者"
    }
  ];

  /// 鸣谢：每行一个来源，右侧一个跳转按钮（照 PCL 的鸣谢页）。
  type Credit = {
    /// 图标文件名（`src/lib/assets/credits/<key>.png`），没有就退回首字母。
    key: string;
    name: string;
    desc: string;
    url: string;
    action: string;
    /// 左侧圆形标记的底色，缺省用主题绿。
    tone?: string;
  };
  const CREDITS: Credit[] = [
    {
      name: "PCL2",
      key: "pcl2",
      desc: "下载器、版本布局和界面上不少做法都参考了它",
      url: "https://github.com/Meloong-Git/PCL",
      action: "GitHub"
    },
    {
      name: "PCL-CE",
      key: "pclce",
      desc: "PCL 的社区维护版，同样参考了实现",
      url: "https://github.com/PCL-Community/PCL-CE",
      action: "GitHub"
    },
    {
      name: "HMCL",
      key: "hmcl",
      desc: "加载器安装流程与图标素材",
      url: "https://github.com/HMCL-dev/HMCL",
      action: "GitHub"
    },
    {
      name: "SJMCL",
      key: "sjmcl",
      desc: "界面组织方式与设置项的设计",
      url: "https://github.com/UNIkeEN/SJMCL",
      action: "GitHub"
    },
    {
      name: "BMCLAPI",
      key: "bmclapi",
      desc: "提供镜像与安装工具，下载能跑起来多半靠它",
      url: "https://bmclapi2.bangbang93.com/",
      action: "官网",
      tone: "#e8a33d"
    },
    {
      name: "mcmod-info-mirror",
      key: "mcim",
      desc: "Modrinth / CurseForge 的国内镜像与中文简介翻译",
      url: "https://mcimirror.top/",
      action: "主页",
      tone: "#5aa9e6"
    },
    {
      name: "MC 百科",
      key: "mcmod",
      desc: "提供模组的中文名与译名表",
      url: "https://www.mcmod.cn/",
      action: "打开百科",
      tone: "#3fae6a"
    },
    {
      name: "LittleSkin",
      key: "littleskin",
      desc: "登录流程与皮肤站的参考",
      url: "https://littleskin.cn/",
      action: "官网",
      tone: "#1bb0a0"
    }
  ];

  const GROUP_ORDER = ["官方", "社区", "百科", "资源", "皮肤", "工具", "组织"];

  const siteGroups = $derived.by(() => {
    const byGroup = new Map<string, Site[]>();
    for (const site of SITES) {
      const list = byGroup.get(site.group) ?? [];
      list.push(site);
      byGroup.set(site.group, list);
    }
    return GROUP_ORDER.filter((name) => byGroup.has(name)).map((name) => ({
      name,
      sites: (byGroup.get(name) ?? []).sort((a, b) => {
        const rank = (site: Site) => SITE_ORDER[site.name] ?? 99;
        const diff = rank(a) - rank(b);
        return diff !== 0 ? diff : a.name.localeCompare(b.name, "zh-Hans-CN");
      })
    }));
  });

  /// 站点图标：先试网站自己的 favicon，拿不到就用启动器 logo。
  function iconFor(url: string): string {
    try {
      return localSiteIcon(new URL(url).hostname) ?? logoMark;
    } catch {
      return logoMark;
    }
  }

  onMount(() => {
    void api
      .info()
      .then((value) => (info = value))
      .catch(() => {});
    void api
      .developerAvatar("reqinme")
      .then((value) => (avatar = value))
      .catch(() => {});
  });

</script>

<div class="shell">
  <div class="page-head">
    <BackButton />
    <h1>关于</h1>
  </div>

  <div class="layout">
    <aside class="rail">
      {#each CATEGORIES as item (item.id)}
        <button class="rail-item" class:active={view === item.id} onclick={() => (view = item.id)}>
          {item.title}
        </button>
      {/each}
    </aside>

    <div class="cards">
      {#if view === "about"}
        <section class="panel hero">
          <Logo size={72} />
          <div class="hero-text">
            <h2>YBUMCL<span class="cn">延边大学《我的世界》启动器</span></h2>
            <p class="en">Yanbian University Minecraft Launcher</p>
            <p class="ver">
              版本 <code>{info?.version ?? "…"}</code>
              {#if info?.portable}<span class="tag">便携模式</span>{/if}
            </p>
          </div>
        </section>

        <section class="panel rows">
          <div class="row">
            <span class="row-name">版本</span>
            <span class="row-value">{info?.version ?? "…"} {info?.portable ? "(Portable)" : ""}</span>
            <button class="btn ghost small-btn" onclick={check} disabled={checking}>
              {checking ? "检查中…" : "检查更新"}
            </button>
            {#if update}
              {#if update.newer}
                <button class="update-note new" onclick={() => void api.openUrl(update!.url)}>
                  有新版本 {update.latest}
                </button>
              {:else}
                <span class="update-note">已是最新</span>
              {/if}
            {/if}
          </div>
          <a class="row" href="https://github.com/reqinme/YBUMCL/graphs/contributors" target="_blank" rel="noreferrer">
            <span class="row-name">开发者名单</span>
            <span class="row-value avatars">
              {#if avatar}
                <img src={avatar} alt="reqinme" />
              {/if}
            </span>
            <span class="row-go" aria-hidden="true">
              <svg viewBox="0 0 24 24" width="16" height="16"><path d="M14 4h6v6M20 4L10 14M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
            </span>
          </a>
          <a class="row" href="https://github.com/reqinme/YBUMCL" target="_blank" rel="noreferrer">
            <span class="row-name">在 GitHub 查看源代码</span>
            <span class="row-go" aria-hidden="true">
              <svg viewBox="0 0 24 24" width="16" height="16"><path d="M14 4h6v6M20 4L10 14M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
            </span>
          </a>
          <a class="row" href="https://ybumua.homes/" target="_blank" rel="noreferrer">
            <span class="row-name">关于延边大学方块同好会</span>
            <span class="row-go" aria-hidden="true">
              <svg viewBox="0 0 24 24" width="16" height="16"><path d="M14 4h6v6M20 4L10 14M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>
            </span>
          </a>
        </section>
            {:else if view === "docs"}
        <section class="panel">
          <h2>YBUMCL 文档</h2>
          <div class="doc-row">
            <span class="doc-name">qinmobox.cn</span>
            <span class="tag muted">ICP 备案中</span>
          </div>
          <p class="dim small">备案完成后开放，文档、社区与下载入口都会放在这里。</p>
        </section>

        <section class="panel">
          <h2>相关网站</h2>
          {#each siteGroups as group (group.name)}
            <p class="group-label">{group.name}</p>
            <div class="site-grid">
              {#each group.sites as site (site.url)}
                <a class="site-card" href={site.url} target="_blank" rel="noreferrer">
                  <img
                    class="site-icon"
                    src={iconFor(site.url)}
                    alt=""
                    loading="lazy"
                    onerror={(event) => {
                      const image = event.currentTarget as HTMLImageElement;
                      image.onerror = null;
                      image.src = logoMark;
                    }}
                  />
                  <span class="site-text">
                    <span class="site-name">{site.name}</span>
                    <span class="site-desc">{site.desc}</span>
                  </span>
                  <span class="site-go" aria-hidden="true">↗</span>
                </a>
              {/each}
            </div>
          {/each}
        </section>
      {:else if view === "thanks"}
        <section class="panel">
          <ul class="credits">
            {#each CREDITS as item (item.name)}
              <li class="credit">
                {#if creditIcon(item.key)}
                  <img class="credit-mark" src={creditIcon(item.key)} alt="" loading="lazy" />
                {:else}
                  <span class="credit-mark" style="--tone: {item.tone ?? "var(--yb-green)"}">
                    {item.name.slice(0, 1)}
                  </span>
                {/if}
                <div class="credit-main">
                  <span class="credit-name">{item.name}</span>
                  <span class="credit-desc">{item.desc}</span>
                </div>
                <button class="btn ghost credit-go" onclick={() => void api.openUrl(item.url)}>
                  {item.action}
                </button>
              </li>
            {/each}
          </ul>
        </section>
      {:else}
        <section class="panel">
          <ul class="legals">
            {#each LEGAL as item (item.name)}
              <li class="legal">
                <div class="legal-main">
                  <span class="legal-name">{item.name}</span>
                  <span class="legal-desc">{item.desc}</span>
                </div>
                {#if item.url && item.action}
                  <button class="btn ghost legal-go" onclick={() => void api.openUrl(item.url ?? "")}>
                    {item.action}
                  </button>
                {/if}
              </li>
            {/each}
          </ul>
        </section>

      {/if}
    </div>
  </div>
</div>

<style>
  .shell {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .page-head {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 14px;
    flex: none;
  }
  .page-head h1 {
    margin: 0;
    font-size: 24px;
  }
  .layout {
    display: flex;
    align-items: stretch;
    gap: 18px;
    flex: 1;
    min-height: 0;
  }
  .rail {
    width: 168px;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    overflow-y: auto;
  }
  .rail-item {
    text-align: left;
    padding: 9px 12px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-dim);
    font: inherit;
    cursor: pointer;
  }
  .rail-item:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  .rail-item.active {
    background: rgb(120 200 120 / 0.2);
    box-shadow: inset 3px 0 0 var(--yb-green-bright);
    color: var(--text);
  }
  .cards {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding-right: 4px;
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
    background: var(--shell-card, rgba(16, 19, 26, 0.5));
    margin-bottom: 14px;
  }
  .panel:last-child {
    margin-bottom: 0;
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
  .hero-text {
    min-width: 0;
    flex: 1;
  }
  .hero-text h2 {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 12px;
  }
  .cn {
    font-size: 15px;
    font-weight: 500;
    color: var(--text-dim);
    letter-spacing: 0;
  }
  .en {
    margin: 4px 0 0;
    font-size: 13px;
    color: var(--text-dim);
    letter-spacing: 0.02em;
  }
  .hero-side {
    margin-left: auto;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 8px;
    flex: none;
  }
  .update-note {
    margin: 0;
    font-size: 12.5px;
    color: var(--text-dim);
  }
  .update-note.new {
    padding: 4px 10px;
    border-radius: 999px;
    font-size: 12.5px;
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 32%, transparent);
    cursor: pointer;
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
    background: var(--tint-2);
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

  .links {
    margin: 0;
    padding-left: 18px;
    display: grid;
    gap: 6px;
  }
  .steps {
    margin: 0;
    padding-left: 20px;
    display: grid;
    gap: 6px;
  }
  .faq {
    margin: 0;
    display: grid;
    gap: 12px;
  }
  .faq dt {
    font-size: 13.5px;
    margin-bottom: 3px;
  }
  .faq dd {
    margin: 0;
    color: var(--text-dim);
    font-size: 12.5px;
  }

  .rows {
    padding: 0;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 13px 18px;
    border-top: 1px solid var(--border);
    color: var(--text);
    text-decoration: none;
  }
  .row:first-child {
    border-top: 0;
  }
  a.row:hover {
    background: var(--surface-hover);
  }
  .row-name {
    font-size: 14px;
  }
  .row-value {
    margin-left: auto;
    color: var(--text-dim);
    font-size: 13px;
  }
  .avatars {
    display: flex;
    align-items: center;
    gap: -6px;
  }
  .avatars img {
    width: 26px;
    height: 26px;
    border-radius: 50%;
    border: 2px solid var(--surface);
    margin-left: -8px;
    background: var(--surface-2);
  }
  .avatars img:first-child {
    margin-left: 0;
  }
  .row-go {
    display: flex;
    align-items: center;
    color: var(--text-dim);
  }
  .small-btn {
    padding: 5px 12px;
    font-size: 12.5px;
  }

  .doc-row {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 6px;
  }
  .doc-name {
    font-size: 15px;
  }
  .tag.muted {
    color: var(--text-dim);
    background: var(--tint-2);
    border-color: var(--border);
  }
  .group-label {
    margin: 14px 0 6px;
    font-size: 12px;
    color: var(--text-dim);
    letter-spacing: 0.04em;
  }
  .group-label:first-of-type {
    margin-top: 4px;
  }
  .site-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(248px, 1fr));
    gap: 8px;
  }
  .site-card {
    display: grid;
    grid-template-columns: 22px minmax(0, 1fr) 14px;
    align-items: center;
    gap: 10px;
    padding: 9px 11px;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--surface);
    color: var(--text);
    text-decoration: none;
  }
  .site-card:hover {
    background: var(--surface-hover);
    border-color: color-mix(in srgb, var(--yb-green) 40%, var(--border));
  }
  .site-icon {
    width: 22px;
    height: 22px;
    border-radius: 6px;
    object-fit: contain;
  }
  .site-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .site-name {
    font-size: 13px;
    color: var(--text);
  }
  .site-desc {
    font-size: 11.5px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .site-go {
    font-size: 12px;
    color: var(--text-dim);
  }

  .credits {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .credit {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 12px 0;
    border-top: 1px solid var(--border);
  }
  .credit:first-child {
    border-top: 0;
  }
  img.credit-mark {
    object-fit: cover;
    background: var(--surface-2);
  }
  .credit-mark {
    width: 36px;
    height: 36px;
    flex: none;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: color-mix(in srgb, var(--tone) 30%, transparent);
    border: 1px solid color-mix(in srgb, var(--tone) 55%, transparent);
    color: var(--text);
    font-weight: 600;
  }
  .credit-main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
  }
  .credit-name {
    font-size: 14px;
  }
  .credit-desc {
    font-size: 12px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .credit-go {
    flex: none;
  }

  .legals {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .legal {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 14px 0;
    border-top: 1px solid var(--border);
  }
  .legal:first-child {
    border-top: 0;
  }
  .legal-main {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  .legal-name {
    font-size: 14px;
    color: var(--text);
  }
  .legal-desc {
    font-size: 13px;
    color: var(--text-dim);
  }
  .legal-go {
    flex: none;
  }
</style>
