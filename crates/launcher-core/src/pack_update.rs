//! 整合包更新：记录来源与文件清单，算差异，做增量更新与恢复。
//!
//! 增量 = 按新旧清单求差（包内文件以新包为准，覆盖前备份，可以「恢复上次更新」）；
//! 用户自己加的文件默认保留，关掉「保留我加的 mod」时按新包对齐（多余的删掉）。
//!
//! 重装不走这里：重装由 `commands` 挪走 `saves/`、`screenshots/` 等用户数据、
//! 删掉实例目录，再用现有安装流程按新包完整重建。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::modpack::PackFiles;
use crate::{CoreError, Result};

/// 实例目录里记录整合包来源与清单的文件。
pub const META_FILE: &str = ".ybmcl-pack.json";

/// 更新前备份放这里（相对实例目录）。
const BACKUP_DIR: &str = ".ybmcl-update-backup";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaFile {
    pub path: String,
    #[serde(default)]
    pub sha1: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastUpdate {
    pub at: u64,
    pub from: String,
    pub to: String,
    /// 备份目录（相对实例目录）。
    pub backup: String,
}

/// 实例里记下来的整合包身份 + 它装过哪些文件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackMeta {
    pub source: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub version_id: String,
    #[serde(default)]
    pub version_name: String,
    #[serde(default)]
    pub game_version: String,
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub installed_at: u64,
    #[serde(default)]
    pub files: Vec<MetaFile>,
    #[serde(default)]
    pub last_update: Option<LastUpdate>,
}

/// 查到的更新。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub source: String,
    pub project_id: String,
    pub current_version: String,
    pub latest_version: String,
    pub latest_version_id: String,
    pub latest_url: String,
    pub latest_file_name: String,
    pub game_version: String,
    /// 增量更新是否安全（加载器或大版本变了就不行）。
    pub allow_incremental: bool,
    /// 不能增量的原因，空表示可以。
    #[serde(default)]
    pub reason: String,
}

/// 算出来的差异。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePlan {
    pub add: Vec<String>,
    pub replace: Vec<String>,
    pub remove: Vec<String>,
    /// 用户自己加的、新包里没有的文件（只有关掉「保留我加的 mod」时才收集）。
    pub extra: Vec<String>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

fn io_error(error: std::io::Error) -> CoreError {
    CoreError::InvalidInput(error.to_string())
}

/// 把清单里的相对路径落到实例目录下；拒绝 `..`、盘符与反斜杠，防止清单写到别处。
fn safe_join(instance_dir: &Path, relative: &str) -> Result<PathBuf> {
    let mut out = instance_dir.to_path_buf();
    for part in relative.split('/') {
        let part = part.trim();
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains(':') || part.contains('\\') {
            return Err(CoreError::InvalidInput(format!("清单里有非法路径：{relative}")));
        }
        out.push(part);
    }
    Ok(out)
}

/// 读实例里的整合包记录；没有或解析失败都当作「没有来源」。
pub fn read_meta(instance_dir: &Path) -> Option<PackMeta> {
    read_meta_checked(instance_dir).ok().flatten()
}

/// 读记录，但把「文件坏了」与「没有记录」区分开——界面上要能提示。
pub fn read_meta_checked(instance_dir: &Path) -> Result<Option<PackMeta>> {
    let path = instance_dir.join(META_FILE);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    let meta = serde_json::from_str::<PackMeta>(&raw)
        .map_err(|error| CoreError::InvalidInput(format!("整合包记录解析失败：{error}")))?;
    Ok(Some(meta))
}

/// 写整合包记录。
pub fn write_meta(instance_dir: &Path, meta: &PackMeta) -> Result<()> {
    let raw = serde_json::to_string_pretty(meta)
        .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    fs::write(instance_dir.join(META_FILE), raw).map_err(io_error)
}

/// 安装流程装完之后调用：把来源与文件清单记进实例目录。
#[allow(clippy::too_many_arguments)]
pub fn record_install_from_plan(
    instance_dir: &Path,
    pack: &crate::modpack::PackOrigin,
    version_id: &str,
    version_name: &str,
    game_version: &str,
    loaders: &[(String, String)],
    entries: &[(String, Option<String>)],
    file_name: &str,
) -> Result<()> {
    let meta = PackMeta {
        source: pack.source.clone(),
        project_id: pack.project_id.clone(),
        version_id: version_id.to_owned(),
        version_name: version_name.to_owned(),
        game_version: game_version.to_owned(),
        loaders: loaders.iter().map(|(loader, _)| loader.clone()).collect(),
        file_name: file_name.to_owned(),
        installed_at: now_secs(),
        files: entries
            .iter()
            .map(|(path, sha1)| MetaFile {
                path: path.clone(),
                sha1: sha1.clone(),
            })
            .collect(),
        last_update: None,
    };
    write_meta(instance_dir, &meta)
}

/// 一次整合包装完之后，把来源与文件清单记进实例目录。
pub fn record_install(instance_dir: &Path, files: &PackFiles, file_name: &str) -> Result<()> {
    let version_name = if files.version.trim().is_empty() {
        files.pack.version_label.clone()
    } else {
        files.version.trim().to_owned()
    };
    let meta = PackMeta {
        source: files.pack.source.clone(),
        project_id: files.pack.project_id.clone(),
        version_id: files.version.clone(),
        version_name,
        game_version: files.game_version.clone(),
        loaders: files.loaders.iter().map(|(loader, _)| loader.clone()).collect(),
        file_name: file_name.to_owned(),
        installed_at: now_secs(),
        files: files
            .entries
            .iter()
            .map(|(path, sha1, _, _)| MetaFile {
                path: path.clone(),
                sha1: sha1.clone(),
            })
            .collect(),
        last_update: None,
    };
    write_meta(instance_dir, &meta)
}

/// 查这个实例的整合包有没有新版本；没有来源或已是最新都返回 `None`。
pub fn check(instance_dir: &Path) -> Result<Option<UpdateInfo>> {
    let Some(meta) = read_meta_checked(instance_dir)? else {
        return Ok(None);
    };
    if meta.project_id.trim().is_empty() {
        return Ok(None);
    }
    let game = meta.game_version.clone();
    // 只按实例自己的加载器筛，避免提示一个 Fabric 版给 NeoForge 实例。
    let loader = meta
        .loaders
        .first()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty());
    let list = match meta.source.as_str() {
        "modrinth" => crate::modrinth::versions(&meta.project_id, Some(&game), loader.as_deref())?,
        "curseforge" => crate::curseforge::versions(&meta.project_id, Some(&game), loader.as_deref())?,
        _ => return Ok(None),
    };
    if list.is_empty() {
        return Ok(None);
    }
    // 正式版优先；没有正式版才退回第一个（按日期）。
    let latest = list
        .iter()
        .find(|item| item.version_type.eq_ignore_ascii_case("release"))
        .unwrap_or(&list[0]);
    if !meta.version_id.trim().is_empty() && meta.version_id.trim() == latest.id.trim() {
        return Ok(None);
    }
    // CurseForge 的版本号就是文件名，先去后缀再比，避免同版本反复提示。
    let latest_label = {
        let raw = if latest.version_number.trim().is_empty() {
            latest.name.trim()
        } else {
            latest.version_number.trim()
        };
        raw.trim_end_matches(".zip").trim_end_matches(".mrpack").to_owned()
    };
    if same_version(&latest_label, &meta.version_name) {
        return Ok(None);
    }
    let reason = span_reason(&meta, latest);
    let allow_incremental = reason.is_none();
    Ok(Some(UpdateInfo {
        source: meta.source.clone(),
        project_id: meta.project_id.clone(),
        current_version: meta.version_name.trim().to_owned(),
        latest_version: latest_label,
        latest_version_id: latest.id.clone(),
        latest_url: latest.file_url.clone(),
        latest_file_name: latest.file_name.clone(),
        game_version: meta.game_version.clone(),
        allow_incremental,
        reason: reason.unwrap_or_default(),
    }))
}

/// 版本号比对：去掉压缩包后缀、忽略大小写，并允许互相包含
/// （CurseForge 的版本号是 `项目名-1.3.1-Beta.zip` 这种文件名）。
fn same_version(latest: &str, current: &str) -> bool {
    let clean = |value: &str| {
        value
            .trim()
            .trim_end_matches(".zip")
            .trim_end_matches(".mrpack")
            .to_ascii_lowercase()
    };
    let left = clean(latest);
    let right = clean(current);
    if right.is_empty() {
        return false;
    }
    left == right || left.ends_with(&right) || right.ends_with(&left)
}

/// 加载器或大版本变了，增量更新会把实例搞坏，只能重装。
fn span_reason(meta: &PackMeta, latest: &crate::modrinth::MrVersion) -> Option<String> {
    let mine: Vec<String> = meta
        .loaders
        .iter()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .collect();
    let theirs: Vec<String> = latest
        .loaders
        .iter()
        .map(|value| value.trim().to_lowercase())
        .collect();
    if !mine.is_empty()
        && !theirs.is_empty()
        && !mine.iter().any(|left| {
            theirs
                .iter()
                .any(|right| right.contains(left.as_str()) || left.contains(right.as_str()))
        })
    {
        return Some(format!(
            "加载器从 {} 变成了 {}",
            mine.join("/"),
            theirs.join("/")
        ));
    }
    let current = meta.game_version.trim();
    if !current.is_empty()
        && !latest.game_versions.is_empty()
        && !latest
            .game_versions
            .iter()
            .any(|value| value.trim() == current)
    {
        let major = |value: &str| {
            value
                .split('.')
                .take(2)
                .collect::<Vec<_>>()
                .join(".")
        };
        if !latest
            .game_versions
            .iter()
            .any(|value| major(value.trim()) == major(current))
        {
            return Some(format!(
                "游戏版本从 {} 跨到了 {}",
                current,
                latest
                    .game_versions
                    .first()
                    .map(|value| value.trim().to_owned())
                    .unwrap_or_default()
            ));
        }
    }
    None
}

fn table(meta: &PackMeta) -> BTreeMap<String, Option<String>> {
    meta.files
        .iter()
        .map(|file| (file.path.clone(), file.sha1.clone()))
        .collect()
}

fn next_table(files: &PackFiles) -> BTreeMap<String, Option<String>> {
    files
        .entries
        .iter()
        .map(|(path, sha1, _, _)| (path.clone(), sha1.clone()))
        .collect()
}

/// 算差异。两侧都没有 sha1 时按路径当作没变（极少数资源站条目）。
pub fn plan(instance_dir: &Path, meta: &PackMeta, next: &PackFiles, keep_custom: bool) -> UpdatePlan {
    let old = table(meta);
    let fresh = next_table(next);
    let mut result = UpdatePlan::default();
    for (path, sha1) in &fresh {
        match old.get(path) {
            None => result.add.push(path.clone()),
            Some(previous) => {
                let changed = match (previous, sha1) {
                    (Some(left), Some(right)) => !left.eq_ignore_ascii_case(right),
                    _ => false,
                };
                if changed {
                    result.replace.push(path.clone());
                }
            }
        }
    }
    for path in old.keys() {
        if !fresh.contains_key(path) {
            result.remove.push(path.clone());
        }
    }
    if !keep_custom {
        let mut known: BTreeSet<String> = old.keys().cloned().collect();
        known.extend(fresh.keys().cloned());
        for folder in ["mods", "resourcepacks", "shaderpacks"] {
            let dir = instance_dir.join(folder);
            let Ok(read) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in read.flatten() {
                if !entry.path().is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let relative = format!("{folder}/{name}");
                if !known.contains(&relative) {
                    result.extra.push(relative);
                }
            }
        }
    }
    result.add.sort();
    result.replace.sort();
    result.remove.sort();
    result.extra.sort();
    result
}

fn copy_into(from: &Path, to: &Path) -> Result<()> {
    if !from.is_file() {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    fs::copy(from, to).map(|_| ()).map_err(io_error)
}

/// 只留最近一次备份，其余删掉（这些是整包里的文件，留着很占地方）。
fn prune_backups(instance_dir: &Path, keep: &str) {
    let root = instance_dir.join(BACKUP_DIR);
    let Ok(read) = fs::read_dir(&root) else {
        return;
    };
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == keep {
            continue;
        }
        if entry.path().is_dir() {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// 增量更新：备份 → 下载到暂存 → 逐个替换 → 删除多余 → 写新记录。
#[allow(clippy::too_many_arguments)]
pub fn apply(
    instance_dir: &Path,
    meta: &PackMeta,
    next: &PackFiles,
    next_version_name: &str,
    keep_custom: bool,
    cancel: &crate::download::CancellationToken,
    strategy: crate::download::DownloadStrategy,
    progress: &(dyn Fn(&str, u64, u64) + Sync),
) -> Result<PackMeta> {
    let plan = plan(instance_dir, meta, next, keep_custom);
    let stamp = now_secs();
    let backup_rel = format!("{BACKUP_DIR}/{stamp}");
    let backup_dir = instance_dir.join(BACKUP_DIR).join(stamp.to_string());
    fs::create_dir_all(&backup_dir).map_err(io_error)?;

    // 旧记录先进备份，恢复时要用。
    let old_meta_raw = serde_json::to_string_pretty(meta)
        .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    fs::write(backup_dir.join("meta.json"), old_meta_raw).map_err(io_error)?;

    // 会被替换/删除/清理的文件先备份；备份失败就整次失败，别留半套。
    let affected: Vec<String> = plan
        .replace
        .iter()
        .chain(plan.remove.iter())
        .chain(plan.extra.iter())
        .cloned()
        .collect();
    for relative in &affected {
        let from = safe_join(instance_dir, relative)?;
        if from.is_file() {
            copy_into(&from, &safe_join(&backup_dir.join("files"), relative)?)?;
        }
    }

    // 下载新增与替换的文件到暂存区，全部成功再动实例。
    let fresh: BTreeMap<&str, (&Option<String>, &Vec<String>, &Option<u64>)> = next
        .entries
        .iter()
        .map(|(path, sha1, urls, size)| (path.as_str(), (sha1, urls, size)))
        .collect();
    let todo: Vec<String> = plan.add.iter().chain(plan.replace.iter()).cloned().collect();
    let staging = instance_dir.join(".ybmcl").join("pack-update").join("staging");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(io_error)?;
    let mut jobs = Vec::new();
    let mut planned = Vec::new();
    for relative in &todo {
        let Some((sha1, urls, size)) = fresh.get(relative.as_str()) else {
            continue;
        };
        if urls.is_empty() {
            return Err(CoreError::InvalidInput(format!("清单里没有下载地址：{relative}")));
        }
        jobs.push(crate::download::DownloadJob {
            urls: (*urls).clone(),
            dest: safe_join(&staging, relative)?,
            size: **size,
            sha1: (*sha1).clone(),
        });
        planned.push(relative.clone());
    }
    let total = jobs.len() as u64;
    progress("下载更新文件", 0, total);
    let (_, failures) = crate::download::fetch_files_to(&jobs, cancel, 8, strategy, |batch| {
        progress("下载更新文件", batch.done as u64, batch.total as u64);
    })
    .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    if !failures.is_empty() {
        let _ = fs::remove_dir_all(&staging);
        return Err(CoreError::InvalidInput(format!(
            "有 {} 个文件没下成功：{}",
            failures.len(),
            failures.join("、")
        )));
    }

    // 替换：旧文件进回收站，再从暂存区挪过去（rename 是原子的）。
    progress("替换旧版文件", 0, planned.len() as u64);
    for (index, relative) in planned.iter().enumerate() {
        let from = safe_join(&staging, relative)?;
        let to = safe_join(instance_dir, relative)?;
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        if to.exists() {
            let _ = crate::mods::recycle_path(&to);
        }
        fs::rename(&from, &to).map_err(io_error)?;
        progress("替换旧版文件", index as u64 + 1, planned.len() as u64);
    }
    let _ = fs::remove_dir_all(&staging);

    // 删除新包里已经没有的文件（以及关掉「保留我加的 mod」时的多余文件）。
    let drops: Vec<String> = plan.remove.iter().chain(plan.extra.iter()).cloned().collect();
    let drop_total = drops.len() as u64;
    progress("替换旧版文件", 0, drop_total);
    for (index, relative) in drops.iter().enumerate() {
        let path = safe_join(instance_dir, relative)?;
        if path.exists() {
            let _ = crate::mods::recycle_path(&path);
        }
        progress("替换旧版文件", index as u64 + 1, drop_total);
    }

    // 只记真正新增的，恢复时才会先删掉它们、再把备份放回去。
    fs::write(backup_dir.join("added.txt"), plan.add.join("\n")).ok();

    let version_name = if next_version_name.trim().is_empty() {
        next.pack.version_label.clone()
    } else {
        next_version_name.trim().to_owned()
    };
    let updated = PackMeta {
        source: next.pack.source.clone(),
        project_id: next.pack.project_id.clone(),
        version_id: next.version.clone(),
        version_name: version_name.clone(),
        game_version: next.game_version.clone(),
        loaders: next.loaders.iter().map(|(loader, _)| loader.clone()).collect(),
        file_name: meta.file_name.clone(),
        installed_at: meta.installed_at,
        files: next
            .entries
            .iter()
            .map(|(path, sha1, _, _)| MetaFile {
                path: path.clone(),
                sha1: sha1.clone(),
            })
            .collect(),
        last_update: Some(LastUpdate {
            at: stamp,
            from: meta.version_name.clone(),
            to: version_name,
            backup: backup_rel,
        }),
    };
    write_meta(instance_dir, &updated)?;
    prune_backups(instance_dir, &stamp.to_string());
    Ok(updated)
}

/// 恢复上次更新：把备份里的文件放回去，删掉那次更新新增的文件。
pub fn restore(instance_dir: &Path) -> Result<()> {
    let Some(meta) = read_meta_checked(instance_dir)? else {
        return Err(CoreError::InvalidInput("这个实例没有整合包更新记录".into()));
    };
    let Some(last) = meta.last_update.clone() else {
        return Err(CoreError::InvalidInput("没有可恢复的更新".into()));
    };
    let backup_dir = safe_join(instance_dir, &last.backup)?;
    if !backup_dir.is_dir() {
        return Err(CoreError::InvalidInput("备份目录不在了".into()));
    }
    if let Ok(added) = fs::read_to_string(backup_dir.join("added.txt")) {
        for line in added.lines() {
            let relative = line.trim();
            if relative.is_empty() {
                continue;
            }
            let path = safe_join(instance_dir, relative)?;
            if path.exists() {
                let _ = crate::mods::recycle_path(&path);
            }
        }
    }
    let files_dir = backup_dir.join("files");
    if let Ok(read) = walk(&files_dir) {
        for source in read {
            let Ok(relative) = source.strip_prefix(&files_dir) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            let target = safe_join(instance_dir, &relative)?;
            copy_into(&source, &target)?;
        }
    }
    if let Ok(raw) = fs::read_to_string(backup_dir.join("meta.json")) {
        if let Ok(mut old) = serde_json::from_str::<PackMeta>(&raw) {
            old.last_update = None;
            write_meta(instance_dir, &old)?;
        }
    }
    fs::remove_dir_all(&backup_dir).ok();
    Ok(())
}

/// 重装更新：把实例里的用户数据挪到工作区，返回「新位置 + 相对实例根的路径」。
pub fn reinstall_move_out(
    instance_dir: &Path,
    workspace: &Path,
) -> Result<Vec<(PathBuf, String)>> {
    let keep = workspace.join("keep");
    fs::create_dir_all(&keep).map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    let mut moved = Vec::new();
    for entry in ["saves", "screenshots", "options.txt", "servers.dat"] {
        let from = instance_dir.join(entry);
        if !from.exists() {
            continue;
        }
        let to = keep.join(entry);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|error| CoreError::InvalidInput(error.to_string()))?;
        }
        fs::rename(&from, &to).map_err(|error| CoreError::InvalidInput(error.to_string()))?;
        moved.push((to, entry.to_owned()));
    }
    Ok(moved)
}

/// 重装更新：旧实例先改名留底（同一父目录下 `<name>.old-<stamp>`）。
pub fn reinstall_swap_old(instance_dir: &Path, stamp: u64) -> Result<PathBuf> {
    let name = instance_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| CoreError::InvalidInput("实例目录名不合法".to_owned()))?;
    let backup = instance_dir.with_file_name(format!("{name}.old-{stamp}"));
    if backup.exists() {
        fs::remove_dir_all(&backup).map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    }
    fs::rename(instance_dir, &backup).map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    Ok(backup)
}

/// 重装失败：删掉半成品新实例，把旧实例改回来。
pub fn reinstall_undo_old(instance_dir: &Path, backup: &Path) -> Result<()> {
    if instance_dir.exists() {
        fs::remove_dir_all(instance_dir).map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    }
    fs::rename(backup, instance_dir).map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    Ok(())
}

/// 重装成功（或回滚后）：把挪出来的用户数据放回指定实例目录。
pub fn reinstall_put_back(instance_dir: &Path, moved: &[(PathBuf, String)]) -> Result<()> {
    for (from, relative) in moved {
        let dest = safe_join(instance_dir, relative).map_err(|error| CoreError::InvalidInput(error.to_string()))?;
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|error| CoreError::InvalidInput(error.to_string()))?;
        }
        if dest.exists() {
            if dest.is_dir() {
                fs::remove_dir_all(&dest).map_err(|error| CoreError::InvalidInput(error.to_string()))?;
            } else {
                fs::remove_file(&dest).map_err(|error| CoreError::InvalidInput(error.to_string()))?;
            }
        }
        fs::rename(from, &dest).map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    }
    Ok(())
}

fn walk(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ybmcl-reinstall-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reinstall_moves_user_data_out_and_back() {
        let root = scratch("move");
        let instance = root.join("MyPack");
        fs::create_dir_all(instance.join("saves").join("World")).unwrap();
        fs::write(instance.join("options.txt"), "lang:zh_cn").unwrap();
        fs::write(instance.join("saves").join("World").join("level.dat"), "nbt").unwrap();

        let workspace = root.join(".ybmcl-tmp").join("reinstall-1");
        let moved = reinstall_move_out(&instance, &workspace).unwrap();
        assert_eq!(moved.len(), 2);
        assert!(!instance.join("saves").exists());
        assert!(!instance.join("options.txt").exists());
        assert!(workspace.join("keep").join("saves").join("World").join("level.dat").exists());

        let backup = reinstall_swap_old(&instance, 1).unwrap();
        assert!(!instance.exists());
        assert!(backup.exists());
        reinstall_undo_old(&instance, &backup).unwrap();
        assert!(instance.exists());
        assert!(backup.exists() == false);

        let fresh = root.join("Fresh");
        fs::create_dir_all(&fresh).unwrap();
        reinstall_put_back(&fresh, &moved).unwrap();
        assert_eq!(
            fs::read_to_string(fresh.join("options.txt")).unwrap(),
            "lang:zh_cn"
        );
        assert!(fresh.join("saves").join("World").join("level.dat").exists());

        fs::remove_dir_all(&root).unwrap();
    }
    use crate::modpack::read_pack_files;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ybumcl-pack-update-{tag}-{}",
            now_secs()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn meta_with(files: &[(&str, &str)]) -> PackMeta {
        PackMeta {
            source: "modrinth".into(),
            project_id: "abc".into(),
            version_id: "v1".into(),
            version_name: "1.0.0".into(),
            game_version: "1.21.1".into(),
            loaders: vec!["fabric".into()],
            file_name: "pack.mrpack".into(),
            installed_at: 0,
            files: files
                .iter()
                .map(|(path, sha1)| MetaFile {
                    path: (*path).to_owned(),
                    sha1: Some((*sha1).to_owned()),
                })
                .collect(),
            last_update: None,
        }
    }

    #[test]
    fn plan_adds_replaces_and_removes() {
        let dir = temp_dir("plan");
        let meta = meta_with(&[
            ("mods/a.jar", "aaa"),
            ("mods/b.jar", "bbb"),
            ("mods/old.jar", "old"),
        ]);
        let next = PackFiles {
            name: "pack".into(),
            version: "v2".into(),
            game_version: "1.21.1".into(),
            loaders: vec![("fabric".into(), "0.16".into())],
            entries: vec![
                (
                    "mods/a.jar".into(),
                    Some("aaa".into()),
                    vec!["https://example.com/a.jar".into()],
                    None,
                ),
                (
                    "mods/b.jar".into(),
                    Some("bbb2".into()),
                    vec!["https://example.com/b.jar".into()],
                    None,
                ),
                (
                    "mods/c.jar".into(),
                    Some("ccc".into()),
                    vec!["https://example.com/c.jar".into()],
                    None,
                ),
            ],
            pack: crate::modpack::PackOrigin {
                source: "modrinth".into(),
                project_id: "abc".into(),
                project_name: "Pack".into(),
                version_label: "2.0.0".into(),
                summary: String::new(),
                downloads: 0,
                tags: Vec::new(),
            },
        };
        let plan = plan(&dir, &meta, &next, true);
        assert_eq!(plan.add, vec!["mods/c.jar"]);
        assert_eq!(plan.replace, vec!["mods/b.jar"]);
        assert_eq!(plan.remove, vec!["mods/old.jar"]);
        assert!(plan.extra.is_empty());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn extra_files_only_show_when_not_keeping_custom() {
        let dir = temp_dir("extra");
        fs::create_dir_all(dir.join("mods")).unwrap();
        fs::write(dir.join("mods/user.jar"), b"x").unwrap();
        let meta = meta_with(&[]);
        let next = PackFiles {
            name: "pack".into(),
            version: "v2".into(),
            game_version: "1.21.1".into(),
            loaders: vec![],
            entries: vec![],
            pack: crate::modpack::PackOrigin {
                source: "modrinth".into(),
                project_id: "abc".into(),
                project_name: "Pack".into(),
                version_label: "2.0.0".into(),
                summary: String::new(),
                downloads: 0,
                tags: Vec::new(),
            },
        };
        assert!(plan(&dir, &meta, &next, true).extra.is_empty());
        assert_eq!(plan(&dir, &meta, &next, false).extra, vec!["mods/user.jar"]);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn meta_round_trips_through_disk() {
        let dir = temp_dir("meta");
        let meta = meta_with(&[("mods/a.jar", "aaa")]);
        write_meta(&dir, &meta).unwrap();
        let back = read_meta(&dir).unwrap();
        assert_eq!(back.version_name, "1.0.0");
        assert_eq!(back.files.len(), 1);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn read_pack_files_reads_mrpack_index() {
        let dir = temp_dir("read");
        let path = dir.join("pack.mrpack");
        {
            let file = fs::File::create(&path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("modrinth.index.json", options).unwrap();
            use std::io::Write;
            zip.write_all(
                br#"{"formatVersion":1,"game":"minecraft","versionId":"v9","name":"Demo",
                "files":[{"path":"mods/a.jar","hashes":{"sha1":"aaa"},
                "downloads":["https://example.com/a.jar"],"fileSize":3}],
                "dependencies":{"minecraft":"1.21.1","fabric-loader":"0.16.0"}}"#,
            )
            .unwrap();
            zip.finish().unwrap();
        }
        let files = read_pack_files(&path).unwrap();
        assert_eq!(files.game_version, "1.21.1");
        assert_eq!(files.entries.len(), 1);
        assert_eq!(files.entries[0].0, "mods/a.jar");
        fs::remove_dir_all(dir).ok();
    }
}
