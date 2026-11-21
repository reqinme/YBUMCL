<div align="center">

<img src="static/logo.png" width="128" height="128" alt="YBUMCL 图标">

# YBUMCL · 延边大学我的世界启动器

**一个 exe 就能用的 Windows Minecraft 启动器。**

[English](README.md) | 简体中文

</div>

## 关于

YBUMCL 是给 Windows 用的 Minecraft 启动器，用 Tauri、SvelteKit 和 Rust 写的。下载一个 `YBUMCL.exe`，放进随便哪个文件夹双击就行；设置、日志和游戏目录都建在 exe 旁边，整个文件夹拷到别的电脑上照样能玩。

## 安装

需要 Windows 10 或 11（64 位），以及 WebView2 运行时——绝大多数系统已经自带。

1. 到 [Releases](https://github.com/reqinme/YBUMCL/releases) 下载最新的 `YBUMCL-*.exe`。
2. 放进任意文件夹，双击运行。
3. 首次启动会在旁边建这些东西：

```
YBUMCL.exe
ybumcl.cfg       # 记住数据文件夹叫什么
YBUMCL/          # 设置、日志、缓存
.minecraft/      # 装完游戏才出现，和 PCL 一样可以直接搬走
```

Java 会从 `JAVA_HOME`、`PATH`、各家厂商的安装目录和官方启动器的 `runtime/` 里自动找。找不到的话，在设置里指一个 `java.exe` 就行。

## 功能

**装版本。** 支持原版，以及 Forge、NeoForge、Fabric、Quilt、Cleanroom、LiteLoader、OptiFine，加载器的新旧版本都在列表里，正式版和测试版会标出来。Forge 1.13+、NeoForge、Fabric、Quilt 由启动器自己装：读 `install_profile.json` 和 `version.json`，合成一份库清单，再自己跑处理器，不启动官方安装器；只有 OptiFine 和 Cleanroom 还在用它们自己的安装器。安装会变成任务，可以暂停、继续、取消，也能同时装好几个，同一个版本开两份也没问题。

**下载。** Modrinth 和 CurseForge 都会查。模组、资源包、光影包、数据包、世界、整合包的进度、速度、剩余时间都是真实字节。Mojang 自己的地址始终优先，其余走镜像；BMCLAPI 按主机限并发，下载失败的文件会在最后统一重试一次。整合包一键装完直接建好实例，世界包会解压到 `saves` 里。

**实例。** 一个实例就是 `versions/` 下的一个文件夹：合并后的版本 json、它的 jar，以及自己的 `mods`、`saves`、`config`。安装时用到的原版底稿会挪到 `cache/vanilla/<版本>`，所以列表里只出现你装过的东西。详情页有 13 个标签：概览、设置、修改、导出、存档、截图、模组、资源包、光影包、投影、服务器、日志、崩溃报告。

- **修改** 可以原地换游戏版本或加载器，模组、存档、配置都留着。
- **导出** 把实例打成 zip，里面带 `manifest.json`。
- **概览** 管图标、名字、描述、分类、置顶和启动次数。
- **高级管理** 能导出启动脚本、测试游戏、补全文件、重置、修补核心、删除实例（默认进回收站，按住 Shift 才是永久删除）。
- **模组** 可以扫描、启用、禁用、删除，把 `.jar` 拖进窗口也能装，还会去 Modrinth 和 CurseForge 查更新。

版本列表能按安装顺序、名称、游戏版本、加载器、最近启动排序，每个游戏目录各记自己的选择。置顶的实例一直在最前面，主页左栏用的是同一套排序。

**游戏目录。** 左栏可以添加、移出、重命名、拖拽排序目录。每个目录有自己的版本、筛选、分类和排序方式；你选中的那个目录，版本页、左栏和下载落点都跟着它走。

**账户。** 支持离线账户、设备码登录的微软账户，以及 authlib-injector 外置登录。皮肤可以来自 Mojang 或 LittleSkin，头像按皮肤的「脸 + 帽子层」画出来，形状跟着皮肤走。拖动把手能排序账户，再点一下已选中的账户就是退出登录；每个来源各记一份顺序。

**日志与崩溃。** `launcher.log` 保留五份。每次启动还会在 `logs/runs/` 下单独写一份日志，并维护 `index.json`，同时开几个实例也不会混在一起。崩溃会拿 60 条内置规则去对，日志页出现红条，同时弹窗提示；可以一键导出 zip，里面有本次运行日志、启动器日志、游戏日志、最新崩溃报告、版本 json、系统信息和 `LatestLaunch.bat`。

**界面。** 点「安装」时按钮会变成一颗光球飞到顶栏的下载图标，颜色在「设置 → 外观」里改。返回按页面层级走（分类 → 概览 → 版本列表 → 主页），会恢复滚动位置，弹窗不进历史，Esc 和 Alt+← 也能用。

## 文件

| 路径 | 内容 |
| --- | --- |
| `YBUMCL/settings.json` | 启动器设置 |
| `YBUMCL/accounts.json` | 账户与皮肤 |
| `YBUMCL/logs/` | `launcher.log` 与 `runs/` 下每次启动的日志 |
| `.minecraft/versions/<实例>/` | 一个实例 |
| `.minecraft/cache/vanilla/<版本>/` | 安装共用的原版底稿 |
| `.minecraft/libraries/`、`assets/` | 共享的库与资源 |

## 致谢

这个启动器的很多做法，是看着别的启动器怎么解决问题学来的。下载、版本布局和界面上的不少想法来自 [PCL2](https://github.com/Meloong-Git/PCL)、[PCL-CE](https://github.com/PCL-Community/PCL-CE)、[HMCL](https://github.com/HMCL-dev/HMCL) 和 [SJMCL](https://github.com/UNIkeEN/SJMCL)，谢谢他们把源码开放出来。

在这里下载能跑起来，多半要谢 [BMCLAPI](https://bmclapi2.bangbang93.com/) 和各镜像的维护者，文件能到手全靠他们。
