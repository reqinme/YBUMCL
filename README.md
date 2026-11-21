<div align="center">

<img src="static/logo.png" width="128" height="128" alt="YBUMCL logo">

# YBUMCL

**A portable Minecraft launcher for Windows — one exe, everything kept next to it.**

[English](README.md) | [简体中文](README.zh_CN.md)

</div>

## Introduction

**YBUMCL** (Yanbian University Minecraft Launcher) is a launcher for Windows written with Tauri 2, SvelteKit 5 and Rust. There is no installer: download one `YBUMCL.exe`, put it in any folder and run it. Settings, logs, caches and the game directory are created beside the exe, so moving the whole folder to another computer keeps everything working.

It is built for the way launchers are actually used in mainland China: official Mojang endpoints, the BMCLAPI mirror and the usual maven mirrors are combined, every download is verified by SHA1, and everything works offline once the files exist.

## Requirements & Installation

**Windows 10 / 11 (x64).** WebView2 runtime is required; it is already present on most systems.

1. Download the latest `YBUMCL-*.exe` from [Releases](https://github.com/reqinme/YBUMCL/releases).
2. Put it in any folder and double-click it.
3. On first start the launcher creates its data next to the exe:

```
YBUMCL.exe
ybumcl.cfg       # remembers where the data folder lives
YBUMCL/          # settings, logs, caches
.minecraft/      # created after the first install; portable like PCL
```

Java is detected automatically (`JAVA_HOME`, `PATH`, vendor install folders, the official launcher's `runtime/`). If nothing suitable is found, pick a `java.exe` manually in Settings.

## Features

### Installing game versions

- Vanilla plus **Forge, NeoForge, Fabric, Quilt, Cleanroom, LiteLoader and OptiFine**; stable, beta and snapshot loader builds are listed newest first.
- Forge 1.13+, NeoForge, Fabric and Quilt are installed by the launcher itself: it reads `install_profile.json` / `version.json`, merges one library list and runs the processors — the official installer jar is not executed. Only OptiFine and Cleanroom still use their own installers.
- Installs run as tasks in **Downloads → Tasks**: pause, resume, cancel; cancelling cleans up the partial files.
- Several installs can run at the same time, including two copies of the same version. Files shared by one game version are locked and reused instead of being downloaded twice.

### Downloads

- Modrinth and CurseForge are both queried; resources, shader packs, data packs, worlds and modpacks are downloaded with real byte-accurate progress, speed and ETA.
- Source order is deterministic: Mojang's own static hosts always go official first, mirrors are used for the rest, BMCLAPI is rate-limited per host, long transfers have a read timeout and every failed file is retried once at the end.
- Modpacks install in one click and create the instance automatically; world archives are extracted into the instance's `saves`.
- Every task card can open its folder, be removed, or be deleted together with its files.

### Instances

An instance is a single self-contained folder under `versions/` — the merged version JSON, its jar and its own `mods`, `saves`, `config`. The vanilla base used during installation is moved out to `cache/vanilla/<version>`, so the version list only shows what you actually installed.

The detail page has 13 tabs: Overview / Settings / Modify / Export / Saves / Screenshots / Mods / Resource packs / Shader packs / Projection / Servers / Logs / Crash reports.

- **Modify**: change the game version or loader in place, keeping `mods`, `saves` and `config`.
- **Export**: pack the instance into a zip with a `manifest.json`.
- **Overview**: icon, name, description, categories, pin to top, launch count.
- **Advanced**: export a launch script, test the game, verify files, reset, patch the core, delete the instance (to the recycle bin unless Shift is held).
- **Mods**: scan, enable/disable, delete, drop `.jar` files into the window, and check Modrinth/CurseForge for updates.

The version list can be sorted by installation order, name, game version, loader or last launch; the choice is remembered per game folder. Pinned instances stay on top. The same sorting drives the sidebar on the home page.

### Multiple game folders

Add, remove, rename and drag-sort game folders in the sidebar. Each folder keeps its own version list, filters, categories and sort order. The active folder is shared by the version list, the home sidebar and the download destinations.

### Accounts

- **Offline**: a player name, reused by new instances.
- **Microsoft**: device-code login; expired tokens can be refreshed from the account card.
- **authlib-injector**: sign in through the account system of your own server.
- **Skins**: official skins and LittleSkin, with pixel-accurate head avatars drawn from the skin's face and hat layer.
- Drag the handle to reorder accounts, click the selected account again to deselect it, and each source keeps its own order.

### Logs, crashes and diagnostics

- `logs/launcher.log` rotates, keeping five files.
- Every launch writes its own log under `logs/runs/` with an `index.json`; running several instances at once keeps their output apart.
- Crashes are analysed against 60 built-in rules, shown as a red bar in the log page and as a dialog, and can be exported as a zip containing the run log, launcher logs, game log, latest crash report, version JSON, system information and a reproducible `LatestLaunch.bat`.

### Interface

- Install animation: the install button turns into a ball of light that flies to the download icon in the top bar; the colour is configurable in Settings → Appearance.
- Back navigation follows a layered model: category → overview → version list → home, with scroll positions restored, modals excluded from the history and keyboard Esc / Alt+← support.
- Light and dark surfaces, no third-party UI library; every dropdown is the launcher's own component.

## Files and data

| Path | Contents |
| --- | --- |
| `YBUMCL/settings.json` | launcher settings |
| `YBUMCL/accounts.json` | accounts and skins |
| `YBUMCL/logs/` | `launcher.log` (5 files) and `runs/` with per-launch logs |
| `.minecraft/versions/<id>/` | one self-contained instance |
| `.minecraft/cache/vanilla/<version>/` | vanilla base shared by installs |
| `.minecraft/libraries/`, `assets/` | shared libraries and assets, verified by SHA1 |

## Building from source

Requirements: Node.js 22+, pnpm, Rust stable with the MSVC toolchain, and the WebView2 runtime.

```bash
pnpm install
pnpm desktop          # dev mode: Vite HMR + Tauri window
pnpm build            # frontend only, into build/
pnpm desktop:build    # release exe at target/release/YBUMCL.exe
pnpm check            # svelte-check
pnpm verify           # gate: svelte-kit sync + svelte-check + frontend build + cargo check
pnpm test:core        # Rust core tests
```

If only `.svelte` files changed, run `pnpm build` before `pnpm desktop:build`; otherwise the packaged exe still contains the old interface.

Repository layout:

```
apps/desktop/src-tauri/   # shell: portable paths, single instance, Tauri commands, storage
crates/launcher-core/     # UI-independent core: version scan, install, loaders, downloader, launch plan, mods
src/                      # SvelteKit frontend (Svelte 5 runes, no UI framework)
tools/                    # icon and wallpaper sources
```

The core crate does not depend on Tauri and can be used directly:

```bash
cargo run -p launcher-core --example install_vanilla
```

`examples/` also contains `install_modloader`, `preview_launch`, `detect_java`, `fetch_probe` and `labymod_prefetch`.

## Acknowledgements

The download scheduler follows the ideas of [PCL-CE](https://github.com/PCL-Community/PCL-CE): official hosts and the BMCLAPI mirror combined, deterministic source order, resumable partial files. Interface decisions were also compared against PCL and PCL-CE.

Thanks to [BMCLAPI](https://bmclapi2.bangbang93.com/) and the mirror operators for keeping Minecraft downloads usable.

## Legal

This launcher does not distribute any game files. Clients, libraries and assets are downloaded from Mojang / Fabric official endpoints or their mirrors, and every file is verified by SHA1.
