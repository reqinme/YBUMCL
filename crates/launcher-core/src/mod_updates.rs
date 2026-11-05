//! 模组更新检查：按文件哈希反查来源，再问来源有没有适配当前版本/加载器的新文件。
//!
//! 只做「查」和「换」两件事，扫描目录、读写文件都走 `mods` 里已有的校验逻辑。

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use crate::{curseforge, export, modrinth, CoreError};

/// 一条模组的更新情况；只有能识别出来源的模组才会出现。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModUpdate {
    /// 当前文件名（可能带 `.disabled`）。
    pub file_name: String,
    /// `modrinth` 或 `curseforge`。
    pub source: String,
    /// Modrinth 的 project id / CurseForge 的数字 id。
    pub project_id: String,
    /// 当前版本号（Modrinth 用版本号，CurseForge 用文件名）。
    pub current_version: String,
    /// 最新版本号。
    pub latest_version: String,
    pub latest_file_name: String,
    pub latest_url: String,
    pub can_update: bool,
}

/// 认领结果：这个 jar 在某个来源上对应哪个项目。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModProject {
    /// `modrinth` 或 `curseforge`。
    pub source: String,
    /// Modrinth 的 project id / CurseForge 的数字 id。
    pub project_id: String,
}

/// 给单个模组文件认领项目：先查 Modrinth（sha1），查不到再问 CurseForge（指纹）。
pub fn identify(runtime_dir: &Path, file_name: &str) -> Result<Option<ModProject>, CoreError> {
    let dir = mods_dir(runtime_dir);
    let path = dir.join(file_name);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(crate::io_error(&path, error)),
    };
    let mut hasher = Sha1::new();
    hasher.update(&bytes);
    let sha1 = hex(&hasher.finalize());

    if let Ok(hits) = modrinth::lookup_sha1(&[sha1]) {
        if let Some(hit) = hits.into_iter().next() {
            return Ok(Some(ModProject {
                source: "modrinth".into(),
                project_id: hit.project_id,
            }));
        }
    }

    let murmur = export::curseforge_fingerprint(&bytes);
    if let Ok(hits) = curseforge::fingerprint_details(&[murmur]) {
        if let Some((_fingerprint, mod_id, _file_id)) = hits.into_iter().next() {
            return Ok(Some(ModProject {
                source: "curseforge".into(),
                project_id: mod_id.to_string(),
            }));
        }
    }
    Ok(None)
}

struct LocalFile {
    file_name: String,
    sha1: String,
    murmur: u32,
}

/// 检查目录里所有模组的更新情况。认不出来源的文件直接跳过。
pub fn check(runtime_dir: &Path, game: &str, loader: &str) -> Result<Vec<ModUpdate>, CoreError> {
    let dir = mods_dir(runtime_dir);
    let files = local_files(&dir)?;
    if files.is_empty() {
        return Ok(Vec::new());
    }

    let sha1s: Vec<String> = files.iter().map(|file| file.sha1.clone()).collect();
    let by_modrinth = modrinth::lookup_sha1(&sha1s).unwrap_or_default();
    let mut hit_sha: HashMap<String, modrinth::MrFileHit> = HashMap::new();
    for hit in by_modrinth {
        hit_sha.insert(hit.sha1.to_lowercase(), hit);
    }

    let fingerprints: Vec<u32> = files.iter().map(|file| file.murmur).collect();
    let by_curse = curseforge::fingerprint_details(&fingerprints).unwrap_or_default();
    let mut hit_fingerprint: HashMap<u32, u64> = HashMap::new();
    for (fingerprint, mod_id, _file_id) in by_curse {
        hit_fingerprint.insert(fingerprint, mod_id);
    }

    // 每个来源只查一次「最新版本」。
    let mut wanted: Vec<(String, String)> = Vec::new(); // (source, project)
    for file in &files {
        if hit_sha.contains_key(&file.sha1.to_lowercase()) {
            let project = hit_sha[&file.sha1.to_lowercase()].project_id.clone();
            wanted.push(("modrinth".into(), project));
        } else if let Some(project) = hit_fingerprint.get(&file.murmur) {
            wanted.push(("curseforge".into(), project.to_string()));
        }
    }
    wanted.sort();
    wanted.dedup();

    // 空字符串等于「不筛」，别把空条件塞进接口。
    let game = if game.is_empty() { None } else { Some(game) };
    let loader = if loader.is_empty() { None } else { Some(loader) };
    let latest = latest_versions(&wanted, game, loader);
    let mut out = Vec::new();
    for file in &files {
        if let Some(hit) = hit_sha.get(&file.sha1.to_lowercase()) {
            let key = ("modrinth".to_owned(), hit.project_id.clone());
            let Some(Some(newest)) = latest.get(&key) else {
                continue;
            };
            out.push(ModUpdate {
                file_name: file.file_name.clone(),
                source: "modrinth".into(),
                project_id: hit.project_id.clone(),
                current_version: hit.version_number.clone(),
                latest_version: newest.0.clone(),
                latest_file_name: newest.1.clone(),
                latest_url: newest.2.clone(),
                can_update: newest.0 != hit.version_number && !newest.2.is_empty(),
            });
        } else if let Some(project) = hit_fingerprint.get(&file.murmur) {
            let key = ("curseforge".to_owned(), project.to_string());
            let Some(Some(newest)) = latest.get(&key) else {
                continue;
            };
            out.push(ModUpdate {
                file_name: file.file_name.clone(),
                source: "curseforge".into(),
                project_id: project.to_string(),
                current_version: file.file_name.clone(),
                latest_version: newest.0.clone(),
                latest_file_name: newest.1.clone(),
                latest_url: newest.2.clone(),
                can_update: newest.0 != file.file_name && !newest.2.is_empty(),
            });
        }
    }
    Ok(out)
}

/// 下载最新文件并替换掉旧文件。返回替换后的新文件名。
pub fn apply(runtime_dir: &Path, update: &ModUpdate) -> Result<String, CoreError> {
    if update.latest_url.is_empty() || update.latest_file_name.is_empty() {
        return Err(CoreError::InvalidInput("没有可下载的新版本".into()));
    }
    let dir = mods_dir(runtime_dir);
    let target = unique_name(&dir, &update.latest_file_name);
    let temp = dir.join(format!(".{}.ybmcl-new", update.latest_file_name));
    let _ = fs::remove_file(&temp);

    let client = reqwest::blocking::Client::builder()
        .user_agent("YBUMCL/0.1 (Minecraft launcher; https://github.com/ybumcl)")
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|error| CoreError::InvalidInput(format!("无法创建 HTTP 客户端：{error}")))?;
    let response = client
        .get(&update.latest_url)
        .send()
        .map_err(|error| CoreError::InvalidInput(format!("下载新版本失败：{error}")))?;
    if !response.status().is_success() {
        return Err(CoreError::InvalidInput(format!(
            "下载新版本失败：HTTP {}",
            response.status()
        )));
    }
    let bytes = response
        .bytes()
        .map_err(|error| CoreError::InvalidInput(format!("读取新版本失败：{error}")))?;
    fs::write(&temp, &bytes).map_err(|error| crate::io_error(&temp, error))?;

    // 旧文件进回收站；替换失败时把新文件删掉，别留半成品。
    let old = update.file_name.clone();
    if let Err(error) = crate::mods::delete_mod(runtime_dir, &old, false) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    fs::rename(&temp, &target).map_err(|error| {
        let _ = fs::remove_file(&temp);
        crate::io_error(&target, error)
    })?;
    Ok(target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| update.latest_file_name.clone()))
}

fn mods_dir(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("mods")
}

fn local_files(dir: &Path) -> Result<Vec<LocalFile>, CoreError> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(error) => return Err(crate::io_error(dir, error)),
    };
    for entry in entries {
        let entry = entry.map_err(|error| crate::io_error(dir, error))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_lowercase();
        if !lower.ends_with(".jar") && !lower.ends_with(".jar.disabled") {
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        let mut hasher = Sha1::new();
        hasher.update(&bytes);
        out.push(LocalFile {
            file_name: name,
            sha1: hex(&hasher.finalize()),
            murmur: export::curseforge_fingerprint(&bytes),
        });
    }
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 结果表的 key 是 `(来源, 项目 id)`，值是新版本的 `(版本号, 文件名, 下载地址)`。
/// 四路并发：几百个模组逐个查会拖到一两分钟。
fn latest_versions(
    wanted: &[(String, String)],
    game: Option<&str>,
    loader: Option<&str>,
) -> HashMap<(String, String), Option<(String, String, String)>> {
    let results: Mutex<HashMap<(String, String), Option<(String, String, String)>>> =
        Mutex::new(HashMap::new());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let workers = wanted.len().min(4).max(1);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let Some((source, project)) = wanted.get(index) else {
                    return;
                };
                let found = if source == "modrinth" {
                    modrinth::versions(project, game, loader)
                        .ok()
                        .and_then(|list| list.into_iter().next())
                        .map(|version| {
                            (
                                version.version_number,
                                version.file_name,
                                version.file_url,
                            )
                        })
                } else {
                    curseforge::versions(project, game, loader)
                        .ok()
                        .and_then(|list| list.into_iter().next())
                        .map(|version| {
                            (
                                version.file_name.clone(),
                                version.file_name,
                                version.file_url,
                            )
                        })
                };
                if let Ok(mut guard) = results.lock() {
                    guard.insert((source.clone(), project.clone()), found);
                }
            });
        }
    });
    results.into_inner().unwrap_or_default()
}

/// 目标文件名已经存在且不是同一个文件时，加序号避免覆盖别人的文件。
fn unique_name(dir: &Path, wanted: &str) -> PathBuf {
    let candidate = dir.join(wanted);
    if !candidate.exists() {
        return candidate;
    }
    let stem = wanted
        .strip_suffix(".jar")
        .or_else(|| wanted.strip_suffix(".JAR"))
        .unwrap_or(wanted);
    for index in 2..1000 {
        let candidate = dir.join(format!("{stem}-{index}.jar"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(wanted)
}
