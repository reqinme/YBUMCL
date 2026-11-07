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

fn join(instance_dir: &Path, relative: &str) -> PathBuf {
    let mut out = instance_dir.to_path_buf();
    for part in relative.split('/') {
        if part.is_empty() {
            continue;
        }
        out.push(part);
    }
    out
}

/// 读实例里的整合包记录；没有或解析失败都当作「没有来源」。
pub fn read_meta(instance_dir: &Path) -> Option<PackMeta> {
    let raw = fs::read_to_string(instance_dir.join(META_FILE)).ok()?;
    serde_json::from_str::<PackMeta>(&raw).ok()
}

/// 写整合包记录。
pub fn write_meta(instance_dir: &Path, meta: &PackMeta) -> Result<()> {
    let raw = serde_json::to_string_pretty(meta)
        .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    fs::write(instance_dir.join(META_FILE), raw).map_err(io_error)
}

/// 安装流程装完之后调用：把来源与文件清单记进实例目录。
pub fn record_install_from_plan(
    instance_dir: &Path,
    pack: &crate::modpack::PackOrigin,
    version_name: &str,
    game_version: &str,
    loaders: &[(String, String)],
    entries: &[(String, Option<String>)],
    file_name: &str,
) -> Result<()> {
    let meta = PackMeta {
        source: pack.source.clone(),
        project_id: pack.project_id.clone(),
        version_id: String::new(),
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
    let meta = PackMeta {
        source: files.pack.source.clone(),
        project_id: files.pack.project_id.clone(),
        version_id: files.version.clone(),
        version_name: files.pack.version_label.clone(),
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
    let Some(meta) = read_meta(instance_dir) else {
        return Ok(None);
    };
    if meta.project_id.trim().is_empty() {
        return Ok(None);
    }
    let game = meta.game_version.clone();
    let list = match meta.source.as_str() {
        "modrinth" => crate::modrinth::versions(&meta.project_id, Some(&game), None)?,
        "curseforge" => crate::curseforge::versions(&meta.project_id, Some(&game), None)?,
        _ => return Ok(None),
    };
    let Some(latest) = list.first() else {
        return Ok(None);
    };
    let latest_label = if latest.version_number.trim().is_empty() {
        latest.name.trim()
    } else {
        latest.version_number.trim()
    };
    if !meta.version_name.trim().is_empty()
        && latest_label.eq_ignore_ascii_case(meta.version_name.trim())
    {
        return Ok(None);
    }
    Ok(Some(UpdateInfo {
        source: meta.source.clone(),
        project_id: meta.project_id.clone(),
        current_version: meta.version_name.clone(),
        latest_version: if latest.version_number.trim().is_empty() {
            latest.name.clone()
        } else {
            latest.version_number.clone()
        },
        latest_version_id: latest.id.clone(),
        latest_url: latest.file_url.clone(),
        latest_file_name: latest.file_name.clone(),
        game_version: meta.game_version.clone(),
    }))
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

/// 算差异。`sha1` 有一侧缺失时按路径判断（CurseForge 的清单只有文件名）。
pub fn plan(instance_dir: &Path, meta: &PackMeta, next: &PackFiles, keep_custom: bool) -> UpdatePlan {
    let old = table(meta);
    let fresh = next_table(next);
    let mut plan = UpdatePlan::default();
    for (path, sha1) in &fresh {
        match old.get(path) {
            None => plan.add.push(path.clone()),
            Some(previous) => {
                let changed = match (previous, sha1) {
                    (Some(left), Some(right)) => !left.eq_ignore_ascii_case(right),
                    _ => false,
                };
                if changed {
                    plan.replace.push(path.clone());
                }
            }
        }
    }
    for path in old.keys() {
        if !fresh.contains_key(path) {
            plan.remove.push(path.clone());
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
                    plan.extra.push(relative);
                }
            }
        }
    }
    plan.add.sort();
    plan.replace.sort();
    plan.remove.sort();
    plan.extra.sort();
    plan
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

/// 增量更新：备份 → 下载新增/替换 → 删除多余 → 写新记录。
pub fn apply(
    instance_dir: &Path,
    meta: &PackMeta,
    next: &PackFiles,
    keep_custom: bool,
    progress: &mut dyn FnMut(&str, u64, u64),
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

    // 会被替换/删除/清理的文件先备份。
    let affected: Vec<String> = plan
        .replace
        .iter()
        .chain(plan.remove.iter())
        .chain(plan.extra.iter())
        .cloned()
        .collect();
    for relative in &affected {
        copy_into(
            &join(instance_dir, relative),
            &backup_dir.join("files").join(relative.replace('/', "\\")),
        )
        .ok();
    }

    // 下载新增与替换的文件。
    let fresh: BTreeMap<&str, (Option<String>, &Vec<String>)> = next
        .entries
        .iter()
        .map(|(path, sha1, urls, _)| (path.as_str(), (sha1.clone(), urls)))
        .collect();
    let todo: Vec<String> = plan.add.iter().chain(plan.replace.iter()).cloned().collect();
    let total = todo.len() as u64;
    progress("下载更新文件", 0, total);
    let mut added: Vec<String> = Vec::new();
    for (index, relative) in todo.iter().enumerate() {
        let Some((_, urls)) = fresh.get(relative.as_str()) else {
            continue;
        };
        let bytes = urls
            .iter()
            .find_map(|url| crate::download::fetch_file_bytes(url, &Default::default()).ok())
            .ok_or_else(|| CoreError::InvalidInput(format!("下载失败：{relative}")))?;
        let target = join(instance_dir, relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        fs::write(&target, &bytes).map_err(io_error)?;
        added.push(relative.clone());
        progress("下载更新文件", index as u64 + 1, total);
    }

    // 删除新包里已经没有的文件（以及关掉「保留我加的 mod」时的多余文件）。
    let drops: Vec<String> = plan.remove.iter().chain(plan.extra.iter()).cloned().collect();
    let drop_total = drops.len() as u64;
    progress("替换旧版文件", 0, drop_total);
    for (index, relative) in drops.iter().enumerate() {
        let path = join(instance_dir, relative);
        if path.exists() {
            crate::mods::recycle_path(&path).ok();
        }
        progress("替换旧版文件", index as u64 + 1, drop_total);
    }

    fs::write(backup_dir.join("added.txt"), added.join("\n")).ok();

    let updated = PackMeta {
        source: next.pack.source.clone(),
        project_id: next.pack.project_id.clone(),
        version_id: next.version.clone(),
        version_name: next.pack.version_label.clone(),
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
            to: next.pack.version_label.clone(),
            backup: backup_rel,
        }),
    };
    write_meta(instance_dir, &updated)?;
    Ok(updated)
}

/// 恢复上次更新：把备份里的文件放回去，删掉那次更新新增的文件。
pub fn restore(instance_dir: &Path) -> Result<()> {
    let Some(meta) = read_meta(instance_dir) else {
        return Err(CoreError::InvalidInput("这个实例没有整合包更新记录".into()));
    };
    let Some(last) = meta.last_update.clone() else {
        return Err(CoreError::InvalidInput("没有可恢复的更新".into()));
    };
    let backup_dir = instance_dir.join(last.backup.replace('/', std::path::MAIN_SEPARATOR_STR));
    if !backup_dir.is_dir() {
        return Err(CoreError::InvalidInput("备份目录不在了".into()));
    }
    if let Ok(added) = fs::read_to_string(backup_dir.join("added.txt")) {
        for line in added.lines() {
            let relative = line.trim();
            if relative.is_empty() {
                continue;
            }
            let path = join(instance_dir, relative);
            if path.exists() {
                crate::mods::recycle_path(&path).ok();
            }
        }
    }
    let files_dir = backup_dir.join("files");
    if let Ok(read) = walk(&files_dir) {
        for source in read {
            let Ok(relative) = source.strip_prefix(&files_dir) else {
                continue;
            };
            let target = instance_dir.join(relative);
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
