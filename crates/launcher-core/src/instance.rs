//! 按 PCL2 的做法把「原版 + 若干加载器」装成一个自包含实例。
//!
//! 加载器各自的 profile 先按旧流程装进 `versions/<loader-id>`，随后由
//! [`crate::merge`] 合并成 `versions/<实例名>/<实例名>.json` 与同名 jar；
//! 只有实例目录留在版本列表里，其余中间产物在成功后被删掉。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::download::{
    self, CancellationToken, DownloadError, InstallProgress, InstallRequest, InstallSummary,
};
use crate::merge::{self, LoaderKind, LoaderPart};
use crate::modloader::{self, ModLoader, ModLoaderInstallRequest};
use crate::{CoreError, Result};

/// 用户在下载页选的一个加载器（名字用界面上常见的小写写法）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LoaderChoice {
    pub loader: String,
    pub version: String,
}

/// 一次实例安装：可能只是原版，也可能是原版 + 多个加载器。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceRequest {
    pub game_dir: String,
    pub game_version: String,
    /// 实例名；留空则按 PCL2 的规则从原版名与加载器拼出来。
    #[serde(default)]
    pub instance_name: Option<String>,
    #[serde(default)]
    pub loaders: Vec<LoaderChoice>,
    /// 装加载器要跑安装器，缺省时自动挑一个满足要求的 Java。
    #[serde(default)]
    pub java_path: Option<String>,
    /// 下载源策略（竞速/官方优先/镜像优先/仅镜像/仅官方）。
    #[serde(default)]
    pub strategy: download::DownloadStrategy,
    #[serde(default)]
    pub threads: Option<u32>,
    /// 允许覆盖同名实例（实例内追加/更换加载器的场景）。
    #[serde(default)]
    pub overwrite: bool,
}

/// 合并安装的结果：`InstallSummary` 的 `version_id` 就是实例名。
pub type InstanceSummary = InstallSummary;

/// 装一个实例。中途失败会回滚：覆盖安装还原备份，新建安装删掉半成品。
/// 这次请求最终会落成哪个目录（`<游戏目录>/versions/<实例名>`）。
/// 名字留空时按 PCL2 的规则现拼，和 `install_instance` 用的是同一套。
/// 只拼路径，不碰磁盘 —— 目录还不存在也能给出答案。
pub fn instance_target(request: &InstanceRequest) -> Option<PathBuf> {
    let parts = loader_parts(&request.loaders).ok()?;
    let name = match request.instance_name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => merge::instance_name(&request.game_version, &parts),
    };
    Some(
        PathBuf::from(&request.game_dir)
            .join("versions")
            .join(name),
    )
}


/// 实例已经按同一套「原版版本 + 加载器版本」装好时，返回现成的 summary。
fn same_instance(
    root: &Path,
    game: &str,
    name: &str,
    parts: &[LoaderPart],
    progress: &impl Fn(InstallProgress),
) -> Option<InstallSummary> {
    let version = merge::read_version(root, name).ok()?;
    let recorded = version.get("_ybmcl")?;
    if recorded.get("gameVersion").and_then(|value| value.as_str())? != game {
        return None;
    }
    let loaders = recorded.get("loaders")?.as_array()?;
    let expected: Vec<(&str, &str)> = parts
        .iter()
        .map(|part| (part.kind.key(), part.version.as_str()))
        .collect();
    if loaders.len() != expected.len() {
        return None;
    }
    for (slot, (kind, version)) in loaders.iter().zip(expected) {
        if slot.get("loader").and_then(|value| value.as_str())? != kind {
            return None;
        }
        if slot.get("version").and_then(|value| value.as_str()).unwrap_or("") != version {
            return None;
        }
    }
    progress(InstallProgress {
        version_id: name.to_owned(),
        stage: "complete".into(),
        path: String::new(),
        completed_files: 0,
        total_files: 0,
        downloaded_bytes: 0,
        total_bytes: 0,
    });
    Some(InstallSummary {
        version_id: name.to_owned(),
        path: crate::download::tidy_path(&root.join("versions").join(name)),
        downloaded_files: 0,
        skipped_files: 0,
        downloaded_bytes: 0,
    })
}

pub fn install_instance(
    request: InstanceRequest,
    cancel: CancellationToken,
    progress: impl Fn(InstallProgress) + Sync,
) -> std::result::Result<InstanceSummary, DownloadError> {
    let game = request.game_version.clone();
    if !crate::safe_component(&game) {
        return Err(DownloadError::Invalid("unsafe game version".into()));
    }
    let root = instance_root(&request.game_dir)?;
    let parts = loader_parts(&request.loaders)?;
    let name = match request.instance_name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => merge::instance_name(&game, &parts),
    };
    if !crate::safe_component(&name) {
        return Err(DownloadError::Invalid(format!(
            "unsafe instance name: {name}"
        )));
    }
    // 上次中断留下的空壳（只有 .pclignore）不能挡住同名重装。
    let shell = root.join("versions").join(&name);
    if merge::is_incomplete(&shell) {
        merge::remove_version(&root, &name);
    }
    let exists = merge::instance_taken(&root, &name);
    if exists && !request.overwrite {
        return Err(DownloadError::Invalid(format!(
            "版本 {name} 已存在，请换一个名字"
        )));
    }
    // 加载器、版本、实例名都没变时，重装一遍只会把几百个库和 processors 再跑一次；
    // 这种「点了修改但什么都没改」的请求直接复用现有实例（要修文件请用「补全文件」）。
    if exists {
        if let Some(summary) = same_instance(&root, &game, &name, &parts, &progress) {
            return Ok(summary);
        }
    }
    let backup = if exists {
        merge::backup_instance(&root, &name)?
    } else {
        None
    };
    merge::mark_incomplete(&root, &name)?;
    let result = build_instance(&request, &root, &game, &name, &parts, &cancel, &progress);
    match &result {
        Ok(_) => {
            merge::clear_incomplete(&root, &name);
            merge::drop_backup(backup.as_deref());
        }
        Err(_) if cancel.is_paused() => {
            // 暂停：半成品留着（`.pclignore` 也在），下次同名安装会先清空壳再接着下。
            // 不清理 `.part`，那是续传的本钱。
        }
        Err(_) => {
            if exists {
                merge::restore_instance(&root, &name, backup.as_deref());
            } else {
                merge::remove_version(&root, &name);
            }
            merge::clear_incomplete(&root, &name);
            // 取消/失败时「原版底稿」可能只下到一半（还停在暂存态，没有 `<game>.json`）。
            // 它不归实例名管，不在这里清掉就会以「装了但缺文件」的样子留在版本页。
            if cancel.is_cancel_requested() && name != game {
                let base_json = root
                    .join("versions")
                    .join(&game)
                    .join(format!("{game}.json"));
                if !base_json.is_file() {
                    merge::remove_version(&root, &game);
                }
            }
            // 取消就把这次留下的半截文件扫干净；assets/libraries 的完整文件不动。
            if cancel.is_cancel_requested() {
                let swept = merge::sweep_parts(&root);
                if swept > 0 {
                    event(&progress, &name, "instance-cleanup", "", 0, 0, 0);
                }
            }
        }
    }
    result
}

fn build_instance(
    request: &InstanceRequest,
    root: &Path,
    game: &str,
    name: &str,
    parts: &[LoaderPart],
    cancel: &CancellationToken,
    progress: &(impl Fn(InstallProgress) + Sync),
) -> std::result::Result<InstanceSummary, DownloadError> {
    let strategy = request.strategy;
    let threads = request.threads;
    // 1. 原版：合并的底稿，缺了就装。
    let installed_base = !merge::version_json_path(root, game).is_file();
    if installed_base {
        event(progress, name, "instance-vanilla", "", 0, 0, 0);
        download::install_version(
            InstallRequest {
                game_dir: request.game_dir.clone(),
                version_id: game.to_owned(),
                fabric_loader: None,
                quilt_loader: None,
                strategy,
                threads,
            },
            cancel.clone(),
            |step| progress(step),
        )?;
    }
    // 2. 逐个装加载器，拿到它们各自产出的版本 id。
    let java = pick_java(request, game, parts)?;
    let mut summary = InstallSummary {
        version_id: name.to_owned(),
        path: crate::download::tidy_path(&root.join("versions").join(name)),
        downloaded_files: 0,
        skipped_files: 0,
        downloaded_bytes: 0,
    };
    // OptiFine 与其它加载器一起装时按 PCL2 的做法当 mod 放进 mods/，不再合并成一个版本：
    // 它是 launchwrapper 的独立入口，和 Forge/Fabric 的 mainClass 无法拼在一起。
    let optifine_as_mod =
        parts.len() > 1 && parts.iter().any(|part| part.kind == LoaderKind::OptiFine);
    let mut produced: Vec<LoaderPart> = Vec::with_capacity(parts.len());
    for part in parts {
        check_cancelled(cancel)?;
        if optifine_as_mod && part.kind == LoaderKind::OptiFine {
            continue;
        }
        let stage_path = if part.version.is_empty() {
            game
        } else {
            part.version.as_str()
        };
        event(progress, name, "instance-loader", stage_path, 0, 0, 0);
        // 同名加载器外壳是上次没清掉的中间产物，直接重写；真正的实例不碰。
        clear_loader_profile(root, &profile_id(part.kind, game, &part.version));
        let id = match part.kind {
            LoaderKind::Fabric | LoaderKind::Quilt => {
                let fabric = part.kind == LoaderKind::Fabric;
                let done = download::install_version(
                    InstallRequest {
                        game_dir: request.game_dir.clone(),
                        version_id: game.to_owned(),
                        fabric_loader: fabric.then(|| part.version.clone()),
                        quilt_loader: (!fabric).then(|| part.version.clone()),
                        strategy,
                        threads,
                    },
                    cancel.clone(),
                    |step| progress(step),
                )?;
                summary.downloaded_files += done.downloaded_files;
                summary.skipped_files += done.skipped_files;
                summary.downloaded_bytes += done.downloaded_bytes;
                done.version_id
            }
            LoaderKind::LegacyFabric => {
                let done = download::install_legacy_fabric(
                    &request.game_dir,
                    game,
                    &part.version,
                    strategy,
                    threads.map(|n| n as usize),
                    cancel.clone(),
                    |step| progress(step),
                )?;
                summary.downloaded_files += done.downloaded_files;
                summary.skipped_files += done.skipped_files;
                summary.downloaded_bytes += done.downloaded_bytes;
                done.version_id
            }
            LoaderKind::LabyMod => {
                // `48.3` 是正式版，`48.3@snapshot` 是测试频道。
                let (base, channel) = match part.version.split_once('@') {
                    Some((base, channel)) => (base, channel),
                    None => (part.version.as_str(), "production"),
                };
                let done = download::install_labymod(
                    &request.game_dir,
                    game,
                    base,
                    channel,
                    strategy,
                    threads.map(|n| n as usize),
                    cancel.clone(),
                    |step| progress(step),
                )?;
                summary.downloaded_files += done.downloaded_files;
                summary.skipped_files += done.skipped_files;
                summary.downloaded_bytes += done.downloaded_bytes;
                done.version_id
            }
            kind => {
                if kind == LoaderKind::OptiFine {
                    clear_optifine_artifact(root, game, &part.version);
                }
                let done = modloader::install_modloader(
                    ModLoaderInstallRequest {
                        game_dir: request.game_dir.clone(),
                        game_version: game.to_owned(),
                        loader: mod_loader(kind),
                        loader_version: part.version.clone(),
                        java_path: java.clone(),
                        strategy,
                        threads: threads.map(|n| n.min(16)),
                    },
                    cancel.clone(),
                    |step| progress(step),
                )?;
                summary.downloaded_files += done.downloaded_files;
                summary.skipped_files += done.skipped_files;
                summary.downloaded_bytes += done.downloaded_bytes;
                done.version_id
            }
        };
        produced.push(LoaderPart {
            kind: part.kind,
            version: part.version.clone(),
            version_id: id,
        });
    }
    if optifine_as_mod {
        let optifine = parts
            .iter()
            .find(|part| part.kind == LoaderKind::OptiFine)
            .expect("optifine part");
        clear_optifine_artifact(root, game, &optifine.version);
        clear_loader_profile(
            root,
            &profile_id(LoaderKind::OptiFine, game, &optifine.version),
        );
        let done = modloader::install_modloader(
            ModLoaderInstallRequest {
                game_dir: request.game_dir.clone(),
                game_version: game.to_owned(),
                loader: ModLoader::OptiFine,
                loader_version: optifine.version.clone(),
                java_path: java.clone(),
                strategy,
                threads: threads.map(|n| n.min(16)),
            },
            cancel.clone(),
            |step| progress(step),
        )?;
        let lite = parts.iter().any(|part| part.kind == LoaderKind::LiteLoader);
        // 放进实例自己的 mods：放共享的 .minecraft/mods 会被别的版本一起加载。
        let mods = optifine_mods_dir(root, name, game, lite);
        fs::create_dir_all(&mods).map_err(|error| {
            DownloadError::Core(CoreError::Io {
                path: mods.display().to_string(),
                source: error,
            })
        })?;
        let jar = root
            .join("libraries")
            .join("optifine")
            .join("OptiFine")
            .join(format!("{game}_{}", optifine.version))
            .join(format!("OptiFine-{game}_{}.jar", optifine.version));
        if jar.is_file() {
            let name = format!("OptiFine_{}_{}.jar", optifine.version, game);
            fs::copy(&jar, mods.join(name)).map_err(|error| {
                DownloadError::Core(CoreError::Io {
                    path: jar.display().to_string(),
                    source: error,
                })
            })?;
        }
        // 装成 mod 之后，它自己的版本目录不再需要。
        merge::remove_version(root, &done.version_id);
    }
    // 3. 合并成自包含实例。
    event(progress, name, "instance-merge", "", 0, 0, 0);
    // LabyMod 给出的版本 json 本身就是完整的一份（带参数、主类和它自己的库），
    // 拿它当底子，其它加载器再往上叠，否则参数与库都会重复一份。
    let labymod = produced
        .iter()
        .find(|part| part.kind == LoaderKind::LabyMod)
        .cloned();
    let base = labymod
        .as_ref()
        .map(|part| part.version_id.clone())
        .unwrap_or_else(|| game.to_owned());
    let rest: Vec<LoaderPart> = produced
        .iter()
        .filter(|part| part.kind != LoaderKind::LabyMod)
        .cloned()
        .collect();
    let merged_parts = merge::merge_order(&rest);
    merge::write_instance(root, &base, &merged_parts, name)?;
    // 合并后的实例 json 才是最终库清单。Forge 1.16.5 这类新版安装器只把库下到
    // 暂存目录、发布时漏掉（启动会报 Cannot find launch target fmlclient），
    // 这里按实例 json 再补一次；缓存命中时只做校验，几乎不花时间。
    let filled = download::install_profile_libraries(
        &request.game_dir,
        name,
        strategy,
        threads.map(|n| n as usize),
        cancel.clone(),
        |step| progress(step),
    )?;
    summary.downloaded_files += filled.downloaded_files;
    summary.skipped_files += filled.skipped_files;
    summary.downloaded_bytes += filled.downloaded_bytes;
    // 4. 中间产物清理：加载器 profile 目录、以及这次才装出来且无人引用的原版目录。
    for part in &merged_parts {
        if part.version_id != name && part.version_id != game {
            merge::remove_version(root, &part.version_id);
        }
    }
    if let Some(part) = &labymod {
        if part.version_id != name && part.version_id != game {
            merge::remove_version(root, &part.version_id);
        }
    }
    if installed_base && name != game && !referenced_by_others(root, game, name) {
        merge::remove_version(root, game);
    }
    event(progress, name, "complete", "", 0, 0, 0);
    Ok(summary)
}

/// 生成要合并的加载器列表：同一种加载器只留最后一次选择，按 PCL2 的合并顺序排好。
fn loader_parts(choices: &[LoaderChoice]) -> std::result::Result<Vec<LoaderPart>, DownloadError> {
    let mut parts: Vec<LoaderPart> = Vec::new();
    for choice in choices {
        let Some(kind) = LoaderKind::parse(&choice.loader) else {
            return Err(DownloadError::Invalid(format!(
                "unsupported loader: {}",
                choice.loader
            )));
        };
        if !(kind == LoaderKind::LiteLoader && choice.version.trim().is_empty())
            && !crate::safe_component(&choice.version)
        {
            return Err(DownloadError::Invalid(format!(
                "unsafe loader version: {}",
                choice.version
            )));
        }
        parts.retain(|part| part.kind != kind);
        parts.push(LoaderPart {
            kind,
            version: choice.version.clone(),
            version_id: String::new(),
        });
    }
    Ok(merge::merge_order(&parts))
}

/// OptiFine 的补丁 jar 带时间戳，重打一次哈希就变；先删掉旧的，免得发布时冲突。
fn clear_optifine_artifact(root: &Path, game: &str, version: &str) {
    let jar = root
        .join("libraries")
        .join("optifine")
        .join("OptiFine")
        .join(format!("{game}_{version}"))
        .join(format!("OptiFine-{game}_{version}.jar"));
    let _ = fs::remove_file(jar);
}

/// OptiFine 当 mod 时落在实例自己的 `mods/` 里；有 LiteLoader 时按它的规矩放 `mods/<mc>/`。
pub(crate) fn optifine_mods_dir(root: &Path, name: &str, game: &str, lite: bool) -> PathBuf {
    let instance = root.join("versions").join(name);
    if lite {
        instance.join("mods").join(game)
    } else {
        instance.join("mods")
    }
}

/// 安装器写进 `versions/` 的加载器外壳目录名（与各自的安装流程保持一致）。
fn profile_id(kind: LoaderKind, game: &str, version: &str) -> String {
    match kind {
        LoaderKind::Fabric => format!("fabric-loader-{version}-{game}"),
        LoaderKind::Quilt => format!("quilt-loader-{version}-{game}"),
        LoaderKind::Forge => format!("{game}-forge-{version}"),
        LoaderKind::NeoForge => format!("neoforge-{version}"),
        LoaderKind::Cleanroom => format!("Cleanroom-{version}"),
        LoaderKind::LegacyFabric => format!("fabric-loader-{version}-{game}"),
        LoaderKind::LabyMod => format!("labymod-{version}-{game}"),
        LoaderKind::LiteLoader => format!("{game}-LiteLoader"),
        LoaderKind::OptiFine => format!("{game}-OptiFine_{version}"),
    }
}

/// 只删「带 inheritsFrom 的加载器外壳」或没装完的目录，用户自己的实例不动。
fn clear_loader_profile(root: &Path, id: &str) {
    if !crate::safe_component(id) {
        return;
    }
    let folder = root.join("versions").join(id);
    if merge::is_incomplete(&folder) {
        merge::remove_version(root, id);
        return;
    }
    let json = folder.join(format!("{id}.json"));
    let Ok(text) = fs::read(&json) else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&text) else {
        return;
    };
    if value.get("inheritsFrom").is_some() {
        merge::remove_version(root, id);
    }
}

fn mod_loader(kind: LoaderKind) -> ModLoader {
    match kind {
        LoaderKind::Forge => ModLoader::Forge,
        LoaderKind::NeoForge => ModLoader::NeoForge,
        LoaderKind::Cleanroom => ModLoader::Cleanroom,
        LoaderKind::LegacyFabric => ModLoader::Cleanroom,
        // LabyMod 走 install_labymod（整份客户端 json），不会到这里。
        LoaderKind::LabyMod => ModLoader::Forge,
        LoaderKind::LiteLoader => ModLoader::LiteLoader,
        LoaderKind::OptiFine => ModLoader::OptiFine,
        // Fabric/Quilt 走 install_version，不会到这里。
        LoaderKind::Fabric | LoaderKind::Quilt => ModLoader::Forge,
    }
}

fn pick_java(
    request: &InstanceRequest,
    game: &str,
    parts: &[LoaderPart],
) -> std::result::Result<String, DownloadError> {
    if let Some(java) = request.java_path.as_deref().filter(|java| !java.is_empty()) {
        return Ok(java.to_owned());
    }
    let mut presence = crate::java::LoaderPresence::default();
    for part in parts {
        match part.kind {
            LoaderKind::Fabric | LoaderKind::LegacyFabric | LoaderKind::Quilt => {
                presence.fabric = true
            }
            LoaderKind::Forge => presence.forge = Some(part.version.as_str()),
            LoaderKind::Cleanroom => presence.cleanroom = Some(part.version.as_str()),
            LoaderKind::LiteLoader => presence.liteloader = true,
            LoaderKind::OptiFine => presence.optifine = true,
            LoaderKind::LabyMod | LoaderKind::NeoForge => {}
        }
    }
    let requirement = crate::java::requirement_with(game, None, &presence);
    let found = crate::detect_java();
    if found.is_empty() {
        return Err(DownloadError::Invalid(
            "没有检测到可用的 Java，请先在设置里指定".into(),
        ));
    }
    let mut matching: Vec<_> = found
        .into_iter()
        .filter(|java| requirement.accepts(java.major))
        .collect();
    matching.sort_by_key(|java| java.major);
    matching
        .into_iter()
        .next()
        .map(|java| java.path)
        .ok_or_else(|| {
            DownloadError::Invalid(format!(
                "没有满足 Java {} 及以上的版本，请先安装或指定",
                requirement.range.min
            ))
        })
}

/// 是否还有别的版本 json 依赖这个原版目录（继承或复用它的 jar）。
fn referenced_by_others(root: &Path, base: &str, except: &str) -> bool {
    let versions = root.join("versions");
    let Ok(entries) = fs::read_dir(versions) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if id == except || id == base {
            continue;
        }
        let json = path.join(format!("{id}.json"));
        let Ok(text) = fs::read_to_string(&json) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if value["inheritsFrom"].as_str() == Some(base) || value["jar"].as_str() == Some(base) {
            return true;
        }
    }
    false
}

fn instance_root(game_dir: &str) -> std::result::Result<PathBuf, DownloadError> {
    let root = PathBuf::from(game_dir);
    if !root.is_absolute() {
        return Err(DownloadError::Invalid(
            "game directory must be absolute".into(),
        ));
    }
    // 目录还不存在（新拷一份 exe 出来、还没装过东西）就先建：canonicalize 对不存在的路径
    // 只会回 os error 2，用户看到的是一句看不懂的「系统找不到指定的文件」。
    if let Err(error) = fs::create_dir_all(&root) {
        return Err(DownloadError::Core(CoreError::Io {
            path: root.display().to_string(),
            source: error,
        }));
    }
    let root = fs::canonicalize(&root).map_err(|error| {
        DownloadError::Core(CoreError::Io {
            path: root.display().to_string(),
            source: error,
        })
    })?;
    if !root.is_dir() {
        return Err(DownloadError::Invalid(format!(
            "game directory does not exist: {}",
            root.display()
        )));
    }
    Ok(root)
}

fn check_cancelled(cancel: &CancellationToken) -> std::result::Result<(), DownloadError> {
    if cancel.is_cancelled() {
        Err(DownloadError::Cancelled)
    } else {
        Ok(())
    }
}

fn event(
    progress: &impl Fn(InstallProgress),
    id: &str,
    stage: &str,
    path: &str,
    completed: u64,
    total: u64,
    bytes: u64,
) {
    progress(InstallProgress {
        version_id: id.to_owned(),
        stage: stage.to_owned(),
        path: path.to_owned(),
        completed_files: completed,
        total_files: total,
        downloaded_bytes: bytes,
        total_bytes: 0,
    });
}

/// 给界面做实时重名校验：实例名是否已被占用。
pub fn instance_name_taken(game_dir: &str, name: &str) -> Result<bool> {
    if !crate::safe_component(name) {
        return Ok(true);
    }
    let root = PathBuf::from(game_dir);
    let root = fs::canonicalize(&root).unwrap_or(root);
    Ok(merge::instance_taken(&root, name))
}
