<div align="center">

<img src="static/logo.png" width="128" height="128" alt="YBUMCL logo">

# YBUMCL

**A portable Minecraft launcher for Windows.**

[English](README.md) | [简体中文](README.zh_CN.md)

</div>

## Overview

YBUMCL is a Minecraft launcher for Windows built with Tauri, SvelteKit and Rust. It is distributed as a single executable and requires no installation. Settings, logs and the game directory are created in the same folder as the executable, so the folder can be copied to another computer and continue to work without reconfiguration.

## Installation

Requirements: Windows 10 or 11 (64-bit) and the WebView2 runtime, which is included in most current systems.

1. Download the latest `YBUMCL-*.exe` from [Releases](https://github.com/reqinme/YBUMCL/releases).
2. Place the executable in any folder and run it.
3. On first start, the following files and folders are created next to the executable:

```
YBUMCL.exe
ybumcl.cfg       # records the name of the data folder
YBUMCL/          # settings, logs and caches
.minecraft/      # created after the first installation; portable
```

Java is detected automatically from `JAVA_HOME`, `PATH`, vendor installation directories and the runtime folder of the official launcher. If no suitable runtime is found, a `java.exe` can be selected manually in Settings.

## Features

### Version installation

Vanilla Minecraft is supported, together with Forge, NeoForge, Fabric, Quilt, Cleanroom, LiteLoader and OptiFine. Loader versions are listed from newest to oldest, with release and beta builds distinguished.

Forge 1.13+, NeoForge, Fabric and Quilt are installed by the launcher itself. It reads `install_profile.json` and `version.json`, merges them into a single library list and executes the processors without invoking the official installer. Only OptiFine and Cleanroom still use their own installers.

Installations run as tasks that can be paused, resumed or cancelled, and several installations may run concurrently, including two installations of the same version.

### Downloads

Modrinth and CurseForge are queried together. Mods, resource packs, shader packs, data packs, worlds and modpacks report actual transferred bytes, speed and remaining time.

Mojang's own hosts are always preferred; mirrors serve the remaining traffic; BMCLAPI requests are limited per host. Files that fail are retried once at the end of the batch. Modpacks install in one step and create their instance automatically; world archives are extracted into the instance's `saves` directory.

### Instances

An instance is a single folder under `versions/`, containing the merged version JSON, the client jar and its own `mods`, `saves` and `config`. The vanilla base used during installation is moved to `cache/vanilla/<version>`, so the version list contains only installed instances.

The instance page provides thirteen tabs: Overview, Settings, Modify, Export, Saves, Screenshots, Mods, Resource packs, Shader packs, Projection, Servers, Logs and Crash reports.

- **Modify** changes the game version or loader in place while preserving mods, saves and configuration.
- **Export** packages the instance as a zip archive with a `manifest.json`.
- **Overview** manages the icon, name, description, categories, pin state and launch count.
- **Advanced management** provides launch-script export, game testing, file verification, reset, core patching and instance deletion. Deletion moves the folder to the recycle bin unless Shift is held.
- **Mods** scans, enables, disables and deletes mods, imports dropped `.jar` files and checks Modrinth and CurseForge for updates.

The version list can be sorted by installation order, name, game version, loader or last launch. The selection is stored per game folder. Pinned instances are listed first, and the sidebar on the home page uses the same order.

### Game folders

Game folders can be added, removed, renamed and reordered in the sidebar. Each folder maintains its own version list, filters, categories and sort order. The selected folder is shared by the version page, the sidebar and the download destination.

### Accounts

Offline accounts, Microsoft accounts with device-code login and authlib-injector servers are supported. Skins may come from Mojang or LittleSkin; avatars are composed from the skin's face and hat layer so the silhouette follows the skin. Accounts can be reordered by dragging the handle, and clicking the selected account clears the selection. Every source keeps its own order.

### Logs and crash reports

`logs/launcher.log` is rotated and five files are retained. Each launch writes a separate log under `logs/runs/` with an accompanying `index.json`, so concurrent instances do not share output.

Crashes are matched against 60 built-in rules and reported in the log page and in a dialog. An error report can be exported as a zip containing the run log, launcher logs, game log, the latest crash report, the version JSON, system information and a `LatestLaunch.bat` for reproduction.

### Interface

The install button turns into a ball of light that travels to the download indicator in the top bar; the colour is configurable in Settings → Appearance.

Back navigation follows the page hierarchy (category → overview → version list → home), restores scroll positions, excludes dialogs from the history and supports Esc and Alt+←.

## Files

| Path | Contents |
| --- | --- |
| `YBUMCL/settings.json` | launcher settings |
| `YBUMCL/accounts.json` | accounts and skins |
| `YBUMCL/logs/` | `launcher.log` and one log per launch under `runs/` |
| `.minecraft/versions/<id>/` | an instance |
| `.minecraft/cache/vanilla/<version>/` | vanilla base shared by installations |
| `.minecraft/libraries/`, `assets/` | shared libraries and assets |

## Acknowledgements

The design of this launcher was informed by existing launchers. The downloader, the version layout and parts of the interface draw on ideas from [PCL2](https://github.com/Meloong-Git/PCL), [PCL-CE](https://github.com/PCL-Community/PCL-CE), [HMCL](https://github.com/HMCL-dev/HMCL) and [SJMCL](https://github.com/UNIkeEN/SJMCL). Thanks to their authors for making their work available.

Downloads rely on [BMCLAPI](https://bmclapi2.bangbang93.com/) and its mirror operators. Thanks to them for keeping the files reachable.
