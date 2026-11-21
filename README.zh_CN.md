<div align="center">

<img src="static/logo.png" width="128" height="128" alt="YBUMCL 图标">

# YBUMCL · 延边大学我的世界启动器

**面向 Windows 的便携式 Minecraft 启动器。**

[English](README.md) | 简体中文

</div>

## 概述

YBUMCL 是一个使用 Tauri、SvelteKit 与 Rust 开发的 Windows Minecraft 启动器，以单个可执行文件分发，无需安装。设置、日志与游戏目录均创建在可执行文件所在的文件夹内，因此整个文件夹可以直接复制到其他计算机继续使用。

## 安装

运行环境：Windows 10 / 11（64 位），以及 WebView2 运行时（绝大多数系统已自带）。

1. 从 [Releases](https://github.com/reqinme/YBUMCL/releases) 下载最新的 `YBUMCL-*.exe`。
2. 将可执行文件放入任意文件夹并运行。
3. 首次启动时，会在其旁边创建以下文件与文件夹：

```
YBUMCL.exe
ybumcl.cfg       # 记录数据文件夹名称
YBUMCL/          # 设置、日志与缓存
.minecraft/      # 首次安装后创建，可随文件夹一起迁移
```

Java 会从 `JAVA_HOME`、`PATH`、各厂商的安装目录以及官方启动器的 runtime 目录中自动查找；若未找到合适的运行时，可在设置中手动指定 `java.exe`。

## 功能

### 版本安装

支持原版，以及 Forge、NeoForge、Fabric、Quilt、Cleanroom、LiteLoader 与 OptiFine。加载器版本按从新到旧排列，并区分正式版与测试版。

Forge 1.13+、NeoForge、Fabric 与 Quilt 由启动器自行安装：读取 `install_profile.json` 与 `version.json`，合并为一份库清单并执行处理器，不调用官方安装器。仅 OptiFine 与 Cleanroom 仍使用其自身的安装器。

安装以任务形式执行，可暂停、继续或取消；多个安装可以同时进行，同一版本也可以安装两份。

### 下载

Modrinth 与 CurseForge 会同时查询。模组、资源包、光影包、数据包、世界与整合包均显示实际传输字节、速度与剩余时间。

Mojang 自有主机始终优先，其余流量由镜像承担；BMCLAPI 按主机限制并发。下载失败的文件会在批次末尾统一重试一次。整合包安装完成后自动创建实例，世界压缩包会解压到实例的 `saves` 目录。

### 实例

一个实例对应 `versions/` 下的一个文件夹，包含合并后的版本 JSON、客户端 jar，以及它自己的 `mods`、`saves` 与 `config`。安装过程中使用的原版底稿会被移动到 `cache/vanilla/<版本>`，因此版本列表中只保留实际安装的实例。

实例页面包含十三个标签：概览、设置、修改、导出、存档、截图、模组、资源包、光影包、投影、服务器、日志与崩溃报告。

- **修改**：原地更换游戏版本或加载器，保留模组、存档与配置。
- **导出**：将实例打包为 zip，内含 `manifest.json`。
- **概览**：管理图标、名称、描述、分类、置顶状态与启动次数。
- **高级管理**：提供启动脚本导出、测试游戏、补全文件、重置、修补核心与删除实例；删除默认移入回收站，按住 Shift 则永久删除。
- **模组**：扫描、启用、禁用与删除模组，支持拖入 `.jar` 导入，并可向 Modrinth 与 CurseForge 检查更新。

版本列表可按安装顺序、名称、游戏版本、加载器或最近启动排序，排序方式按游戏目录分别保存。置顶实例始终排在最前，主页左栏使用同一套排序。

### 游戏目录

左栏中的游戏目录可以添加、移除、重命名与拖动排序。每个目录维护各自的版本列表、筛选、分类与排序方式；当前选中的目录同时作用于版本页面、左栏与下载目标。

### 账户

支持离线账户、设备码登录的微软账户，以及 authlib-injector 外置登录。皮肤可来自 Mojang 或 LittleSkin，头像由皮肤的「脸」与「帽子层」合成，轮廓随皮肤变化。拖动把手可调整账户顺序，再次点击已选中的账户会清除选择；每个来源分别保存自己的顺序。

### 日志与崩溃报告

`logs/launcher.log` 会轮转，保留五份。每次启动还会在 `logs/runs/` 下单独写一份日志，并维护 `index.json`，同时运行的多个实例不会相互混淆。

崩溃会与 60 条内置规则进行匹配，并在日志页与对话框中提示。错误报告可导出为 zip，包含本次运行日志、启动器日志、游戏日志、最新崩溃报告、版本 JSON、系统信息以及用于复现的 `LatestLaunch.bat`。

### 界面

点击安装按钮后，按钮会变为一颗光球并飞向顶栏的下载图标；颜色可在「设置 → 外观」中配置。

返回导航按页面层级执行（分类 → 概览 → 版本列表 → 主页），会恢复滚动位置，弹窗不进入历史记录，并支持 Esc 与 Alt+←。

## 文件

| 路径 | 内容 |
| --- | --- |
| `YBUMCL/settings.json` | 启动器设置 |
| `YBUMCL/accounts.json` | 账户与皮肤 |
| `YBUMCL/logs/` | `launcher.log` 以及 `runs/` 下每次启动的日志 |
| `.minecraft/versions/<实例>/` | 一个实例 |
| `.minecraft/cache/vanilla/<版本>/` | 安装共用的原版底稿 |
| `.minecraft/libraries/`、`assets/` | 共享的库与资源 |

## 致谢

本启动器的实现参考了现有启动器的做法。下载器、版本布局与界面设计中的部分思路来自 [PCL2](https://github.com/Meloong-Git/PCL)、[PCL-CE](https://github.com/PCL-Community/PCL-CE)、[HMCL](https://github.com/HMCL-dev/HMCL) 与 [SJMCL](https://github.com/UNIkeEN/SJMCL)，感谢这些项目的作者公开其工作。

下载依赖 [BMCLAPI](https://bmclapi2.bangbang93.com/) 及各镜像维护者，感谢他们保证文件的可用性。

## 许可证

YBUMCL 是自由软件：你可以依据自由软件基金会发布的 GNU 通用公共许可证第 3 版，或（由你选择）任何更新版本，重新发布和 / 或修改它。

YBUMCL 的分发是希望它有用，但不提供任何担保，也不含适销性或特定用途适用性的默示担保。详情请参阅 GNU 通用公共许可证。

许可证全文见 [LICENSE](LICENSE)。
