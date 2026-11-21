<div align="center">

<img src="static/logo.png" width="128" height="128" alt="YBUMCL 图标">

# YBUMCL · 延边大学我的世界启动器

**Windows 上的 Minecraft 启动器——只有一个 exe，游戏和数据都放在它旁边。**

[English](README.md) | 简体中文

</div>

## 简介

**YBUMCL** 是用 Tauri 2、SvelteKit 5 和 Rust 写的 Windows Minecraft 启动器。没有安装程序：下载一个 `YBUMCL.exe`，放进任意文件夹双击就能用；设置、日志、缓存和游戏目录都建在 exe 旁边，把整个文件夹拷到别的电脑照样跑。

它是按国内实际网络环境做的：Mojang 官方地址、BMCLAPI 镜像和常用 maven 镜像组合使用，每个文件都按 SHA1 校验，文件下过之后离线也能继续用。

## 前置与安装

**Windows 10 / 11（x64）**，需要 WebView2 运行时（绝大多数系统已自带）。

1. 到 [Releases](https://github.com/reqinme/YBUMCL/releases) 下载最新的 `YBUMCL-*.exe`。
2. 放进任意文件夹，双击运行。
3. 首次启动会在 exe 旁边建这些东西：

```
YBUMCL.exe
ybumcl.cfg       # 记住数据文件夹叫什么
YBUMCL/          # 设置、日志、缓存
.minecraft/      # 装完游戏才出现，和 PCL 一样可以直接搬走
```

Java 会自动找（`JAVA_HOME`、`PATH`、各家厂商的安装目录、官方启动器的 `runtime/`）；没找到合适的，可以在设置里手动指定 `java.exe`。

## 主要功能

### 装游戏

- 支持原版，以及 **Forge、NeoForge、Fabric、Quilt、Cleanroom、LiteLoader、OptiFine**；加载器提供稳定版、测试版、快照版，从新到旧排列。
- Forge 1.13+、NeoForge、Fabric、Quilt 由启动器自己安装：读 `install_profile.json` 与 `version.json`，合成一份库清单后自己跑处理器，**不运行官方安装器 jar**。只有 OptiFine 和 Cleanroom 仍然用它们自己的安装器。
- 安装以任务形式出现在「下载 → 下载任务」里：可以暂停、继续、取消，取消会把半成品清干净。
- 支持同时装多个版本，包括同一个版本开两份；同一个原版的文件会加锁复用，不会重复下载。

### 下载

- Modrinth 与 CurseForge 两个源都会查；模组、资源包、光影包、数据包、世界、整合包都显示真实的字节进度、速度和剩余时间。
- 换源顺序是确定的：Mojang 自营静态主机始终官方优先，其余走镜像；BMCLAPI 按主机限并发，长传输有读超时，失败的文末统一重试一次。
- 整合包一键安装并自动建实例；世界包下载完自动解压到实例的 `saves`。
- 每张任务卡都能打开所在文件夹、移除记录，或者连文件一起删掉。

### 实例

一个实例就是 `versions/` 下一个**自包含**的文件夹：合并后的版本 json、它的核心 jar，以及自己的 `mods`、`saves`、`config`。安装时用到的原版底稿会移出到 `cache/vanilla/<版本>`，所以版本列表里只出现你真正装过的东西。

实例详情页有 13 个标签：概览 / 设置 / 修改 / 导出 / 存档 / 截图 / 模组 / 资源包 / 光影包 / 投影 / 服务器 / 日志 / 崩溃报告。

- **修改**：原地换游戏版本或加载器，`mods`、`saves`、`config` 都保留。
- **导出**：把实例打成 zip，里面带 `manifest.json`。
- **概览**：图标、实例名、描述、分类、置顶、启动次数。
- **高级管理**：导出启动脚本、测试游戏、补全文件、重置、修补核心、删除实例（默认进回收站，按住 Shift 才是永久删除）。
- **模组**：扫描、启用/禁用、删除，也可以把 `.jar` 拖进窗口导入，并按 Modrinth / CurseForge 检查更新。

版本列表支持按安装顺序、名称、游戏版本、加载器、最近启动排序，**每个游戏目录各记一份**；置顶的实例始终在最前面，主页左栏也用同一套排序。

### 多个游戏目录

左栏可以添加、移出、重命名、拖拽排序游戏目录，每个目录有自己的版本列表、筛选、分类和排序方式。当前目录由版本列表、主页左栏和下载落点共享。

### 账户

- **离线账户**：保存玩家名，新建实例默认沿用。
- **微软正版**：设备码登录，令牌过期可以在账户卡片上重新登录。
- **外置登录**（authlib-injector）：填服务器地址后按服务器自己的账户体系登录。
- **皮肤**：支持官方皮肤与 LittleSkin，头像用皮肤的「脸 + 外层帽子」按像素合成，形状跟着皮肤轮廓走。
- 拖动把手可以排序账户；再点一下已选中的账户会取消选择；每个来源各记一份顺序。

### 日志、崩溃与诊断

- `logs/launcher.log` 轮转，保留五份。
- 每次启动单独写一份日志到 `logs/runs/`，并维护 `index.json`；同时开几个实例时日志不会混在一起。
- 崩溃按 60 条内置规则分析，日志页出现红条、同时弹窗；可以一键导出错误报告 zip（含本次运行日志、启动器日志、游戏日志、最新崩溃报告、版本 json、系统信息和可复现的 `LatestLaunch.bat`）。

### 界面

- 安装动画：点「安装」后按钮收成一颗光球飞向顶栏的下载图标，颜色可以在「设置 → 外观」里改。
- 返回导航是分层模型：分类 → 概览 → 版本列表 → 主页，会恢复滚动位置，弹窗不进历史，支持 Esc 与 Alt+←。
- 没有第三方 UI 库；所有下拉都是启动器自己的组件。

## 文件与数据

| 路径 | 内容 |
| --- | --- |
| `YBUMCL/settings.json` | 启动器设置 |
| `YBUMCL/accounts.json` | 账户与皮肤 |
| `YBUMCL/logs/` | `launcher.log`（五份）与 `runs/` 下的每次运行日志 |
| `.minecraft/versions/<实例>/` | 一个自包含实例 |
| `.minecraft/cache/vanilla/<版本>/` | 安装时共用的原版底稿 |
| `.minecraft/libraries/`、`assets/` | 共享的库与资源，按 SHA1 校验 |

## 从源码构建

需要 Node.js 22+、pnpm、带 MSVC 工具链的 Rust stable，以及 WebView2 运行时。

```bash
pnpm install
pnpm desktop          # 开发模式：Vite 热更新 + Tauri 窗口
pnpm build            # 只构建前端到 build/
pnpm desktop:build    # 发布版 exe，产物在 target/release/YBUMCL.exe
pnpm check            # svelte-check
pnpm verify           # 门禁：svelte-kit sync + svelte-check + 前端构建 + cargo check
pnpm test:core        # Rust 核心测试
```

只改了 `.svelte` 时，打包前要先 `pnpm build` 再 `pnpm desktop:build`，否则 exe 里还是旧界面。

目录结构：

```
apps/desktop/src-tauri/   # 壳：便携路径、单实例、Tauri 命令、设置与实例存储
crates/launcher-core/     # 与界面无关的核心：扫版本、装游戏、装加载器、下载器、启动参数、模组
src/                      # SvelteKit 前端（Svelte 5 runes，没有第三方 UI 库）
tools/                    # 图标与壁纸源文件
```

核心 crate 不依赖 Tauri，可以直接跑：

```bash
cargo run -p launcher-core --example install_vanilla
```

`examples/` 里还有 `install_modloader`、`preview_launch`、`detect_java`、`fetch_probe`、`labymod_prefetch`。

## 致谢

下载调度的设计参考了 [PCL-CE](https://github.com/PCL-Community/PCL-CE)：官方源与 BMCLAPI 镜像组合、确定的换源顺序、半截文件保留续传。界面与操作取舍也大量对照 PCL 与 PCL-CE。

感谢 [BMCLAPI](https://bmclapi2.bangbang93.com/) 与各镜像维护者，让 Minecraft 的下载在国内还能正常用。

## 法律

本启动器不分发任何游戏本体文件：客户端、库、资源全部从 Mojang / Fabric 官方地址或其镜像下载，并按 SHA1 校验。
