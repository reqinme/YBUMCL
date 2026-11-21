<div align="center">

<img src="static/logo.png" width="128" height="128" alt="YBUMCL logo">

# YBUMCL

**A Minecraft launcher for Windows that is just one exe.**

[English](README.md) | [简体中文](README.zh_CN.md)

</div>

## About

YBUMCL is a Minecraft launcher for Windows, written with Tauri, SvelteKit and Rust. You download one `YBUMCL.exe`, drop it in any folder and run it. Settings, logs and the game directory are created right next to the exe, so you can copy the whole folder to another PC and keep playing.

## Install

You need Windows 10 or 11 (64-bit). The WebView2 runtime is required, and most systems already have it.

1. Get the latest `YBUMCL-*.exe` from [Releases](https://github.com/reqinme/YBUMCL/releases).
2. Put it in any folder and double-click it.
3. The launcher creates these files on first start:

```
YBUMCL.exe
ybumcl.cfg       # remembers where the data folder is
YBUMCL/          # settings, logs, caches
.minecraft/      # appears after the first install, portable like PCL
```

Java is found automatically through `JAVA_HOME`, `PATH`, vendor install folders and the official launcher's `runtime/`. If that fails, point the launcher at a `java.exe` in the settings.

## What it does

**Installing versions.** Vanilla plus Forge, NeoForge, Fabric, Quilt, Cleanroom, LiteLoader and OptiFine. Loader builds are listed newest first, with stable and beta builds marked. Forge 1.13+, NeoForge, Fabric and Quilt are installed by the launcher itself: it reads `install_profile.json` and `version.json`, merges them into one library list and runs the processors, without launching the official installer jar. Only OptiFine and Cleanroom still use their own installers. Installs show up as tasks that you can pause, resume or cancel, and several of them can run at once, even two copies of the same version.

**Downloading.** Modrinth and CurseForge are both searched. Mods, resource packs, shader packs, data packs, worlds and modpacks all show real progress, speed and ETA. Mojang's own hosts are always tried first, mirrors handle the rest, BMCLAPI is rate-limited per host, and every failed file is retried once at the end. Modpacks install in one click and create their instance; world archives are extracted into `saves` for you.

**Instances.** An instance is a single folder under `versions/`: the merged version JSON, its jar, and its own `mods`, `saves` and `config`. The vanilla base used during installation is moved to `cache/vanilla/<version>`, so the version list only shows what you installed. The detail page has 13 tabs: Overview, Settings, Modify, Export, Saves, Screenshots, Mods, Resource packs, Shader packs, Projection, Servers, Logs and Crash reports.

- **Modify** changes the game version or loader in place and keeps your mods, saves and config.
- **Export** packs the instance into a zip with a `manifest.json`.
- **Overview** holds the icon, name, description, categories, pin and launch count.
- **Advanced** can export a launch script, test the game, verify files, reset, patch the core or delete the instance (to the recycle bin unless you hold Shift).
- **Mods** scans, enables, disables and deletes mods, accepts dropped `.jar` files, and checks Modrinth and CurseForge for updates.

The version list can be sorted by install order, name, game version, loader or last launch, and each game folder remembers its own choice. Pinned instances stay on top, and the sidebar on the home page uses the same order.

**Game folders.** Add, remove, rename and drag-sort folders in the sidebar. Each one keeps its own versions, filters, categories and sort order. The folder you select is shared by the version page, the sidebar and the download destination.

**Accounts.** Offline accounts, Microsoft accounts with device-code login, and authlib-injector servers. Skins can come from Mojang or LittleSkin, and avatars are drawn from the skin's face and hat layer so the shape follows the skin. Drag the handle to reorder accounts, or click the selected one again to sign out; every source keeps its own order.

**Logs and crashes.** `logs/launcher.log` keeps five rotated files. Every launch also writes its own log under `logs/runs/` with an `index.json`, so several running instances do not mix their output. A crash is matched against 60 built-in rules and shows up as a red bar and a dialog, with a one-click zip containing the run log, launcher logs, game log, latest crash report, version JSON, system information and a `LatestLaunch.bat`.

**Interface.** When you press Install, the button turns into a ball of light and flies to the download icon in the top bar; the colour is set in Settings → Appearance. Going back follows the page structure (category → overview → version list → home), restores the scroll position, leaves dialogs out of the history, and also works with Esc and Alt+←.

## Files

| Path | What is there |
| --- | --- |
| `YBUMCL/settings.json` | launcher settings |
| `YBUMCL/accounts.json` | accounts and skins |
| `YBUMCL/logs/` | `launcher.log` and `runs/` with one log per launch |
| `.minecraft/versions/<id>/` | one instance |
| `.minecraft/cache/vanilla/<version>/` | vanilla base shared by installs |
| `.minecraft/libraries/`, `assets/` | shared libraries and assets |

## Thanks

A lot of the launcher was designed by looking at how other launchers solved the same problems. Ideas for the downloader, the version layout and the interface come from [PCL2](https://github.com/Meloong-Git/PCL), [PCL-CE](https://github.com/PCL-Community/PCL-CE), [HMCL](https://github.com/HMCL-dev/HMCL) and [SJMCL](https://github.com/UNIkeEN/SJMCL); thanks to their authors for keeping their work open.

Downloads would not be usable from here without [BMCLAPI](https://bmclapi2.bangbang93.com/). Thanks to it and to the mirror operators for keeping the files reachable.
