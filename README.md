# YBUMCL · 延边大学我的世界启动器

Windows 上的 Minecraft 启动器。只有一个 exe，双击就能用；游戏和数据都放在 exe 旁边，整个文件夹拷到别的电脑照样跑。

## 下载与使用

到 [Releases](https://github.com/reqinme/YBUMCL/releases) 下最新的 `YBUMCL-*.exe`，放进任意文件夹双击运行。首次启动会在它旁边建这些东西：

```
YBUMCL.exe
ybumcl.cfg      # 记住数据文件夹叫什么
YBUMCL\         # 设置、日志、界面运行时数据
.minecraft\     # 装游戏之后才出现，和 PCL 一样可以直接搬走
```

Windows 10 / 11。Java 会自动找（JAVA_HOME、PATH、各家厂商的安装目录、官方启动器的 `runtime\`），找不到也可以在设置里手动指一个。

## 能做什么

### 装游戏

- 原版、Forge、NeoForge、Fabric、Quilt、Cleanroom、LiteLoader、OptiFine 都能装，加载器版本可以选稳定版、测试版、快照版。
- 安装过程在「下载任务」里，可以暂停、继续、取消；取消会把半成品清干净。
- 一个版本就是一个实例，设置跟着实例走。默认开启版本隔离：存档、模组、配置都在 `versions\<实例>\` 里，删实例就删这一包。

### 下模组、整合包和材质

- Modrinth 与 CurseForge 两个源同时请求，谁快用谁，失效镜像自动跳过。
- 进度、速度、剩余时间都是真实字节。
- 整合包一键安装：装好自动建实例，概览页直接显示整合包名称、标签、简介、下载数和来源；点那块信息能跳回下载页看同一个项目。
- 世界包下完自动解压到实例的 `saves`。
- 模组、资源包、数据包、光影包会落到当前实例对应的目录。搜索框下方有「下载到」，可以在「默认」（游戏目录）和某个具体实例之间切，每类各记一份。
- 下载完成的任务卡上能直接打开所在文件夹、移除记录，或者连文件一起删掉。

### 管理实例

实例详情页有 13 个标签：概览 / 设置 / 修改 / 导出 / 存档 / 截图 / 模组 / 资源包 / 光影包 / 投影 / 服务器 / 日志 / 崩溃报告。

- **修改**：原地换游戏版本或加载器，`mods`、`saves`、`config` 都保留；换加载器之前会提醒旧的模组可能不适用。
- **导出**：把实例打成 zip，里面有 `manifest.json`（名称、游戏版本、加载器、文件数、字节数），换机器也能照着重装。
- **概览**：改图标（内置方块图或自己的图片）、实例名、描述、分类，把实例加进收藏夹——主页左栏只显示打星的实例；还能看到启动次数。
- **高级管理**：导出启动脚本（`.bat`）、测试游戏、补全文件、重置（先备份核心再原地重装）、修补核心（拿一个 jar/zip 合并进客户端核心）、删除实例（进回收站，按住 Shift 才是永久删除）。
- **模组**：扫描、启用/禁用、删除，也可以把 `.jar` 拖进窗口导入。

### 多个游戏目录

左栏可以添加、移出、重命名、拖拽排序游戏目录，随时切换。每个目录有自己的版本列表、筛选和分类，互不干扰。

### 账户

离线账户，玩家名保存在设置里，新建实例默认沿用。

## 目前不做

- 微软正版登录
- 自动更新
- macOS / Linux

## 从源码构建

```bash
pnpm install
pnpm desktop          # 开发模式（前端热更新 + Tauri 窗口）
pnpm build            # 只构建前端到 build/
pnpm desktop:build    # 发布版 exe，产物在 target/release/YBUMCL.exe
pnpm check            # svelte-check
pnpm test:core        # Rust 核心测试
```

只改了 `.svelte` 的话，打包前要先 `pnpm build`，再 `touch apps/desktop/src-tauri/src/lib.rs apps/desktop/src-tauri/build.rs`，否则 exe 里还是旧界面。

目录：

```
apps/desktop/src-tauri/   # 壳：便携路径、单实例、Tauri 命令、设置与实例存储
crates/launcher-core/     # 与界面无关的核心：扫版本、装游戏、装加载器、下载器、启动参数、模组
src/                      # SvelteKit 前端（Svelte 5 runes，没有第三方 UI 库）
tools/                    # 图标与壁纸源文件
```

核心 crate 不依赖 Tauri，可以直接跑：`cargo run -p launcher-core --example install_vanilla`，`examples/` 里还有 `install_modloader`、`preview_launch`、`detect_java`。

## 致谢

下载调度的设计参考了 [PCL-CE](https://github.com/PCL-Community/PCL-CE)：官方源和 BMCLAPI 镜像同时请求、先到先用，失败回落，半截文件留着续传。

界面与操作的取舍也大量对照 PCL 与 PCL-CE。

## 法律

本启动器不分发任何游戏本体文件：客户端、库、资源全部从 Mojang / Fabric 官方地址或其镜像下载，并按 SHA1 校验。
