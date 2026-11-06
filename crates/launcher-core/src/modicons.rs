//! 给本地模组补「项目图标」。
//!
//! 模组 jar 里不一定带图标（常见文件名、`logoFile`、`icon` 都没有），
//! PCL 的做法是拿文件的哈希去 Modrinth / CurseForge 反查项目，用项目图。
//! 这里照做：SHA1 查 Modrinth，CurseForge 用文件指纹，结果按 SHA1 缓存在
//! 数据目录里（6 小时），查不到也记下来，免得每次进模组页都重新问一遍。
//!
//! 缓存记的是「这个文件长什么样」，所以重装、换版本后 SHA1 变了会自动重查。

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CACHE_FILE: &str = "mod-icons.json";
const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const CACHE_VERSION: u32 = 2;

/// 一个模组文件的两种身份：Modrinth 认 SHA1，CurseForge 认指纹。
#[derive(Debug, Clone)]
pub struct ModKey {
    pub file_name: String,
    pub sha1: String,
    pub fingerprint: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    version: u32,
    /// SHA1 → 项目图标地址；`url: null` 表示查过但没有。
    #[serde(default)]
    entries: HashMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    url: Option<String>,
    /// 项目原始名（Modrinth 的 title / CurseForge 的 name）。
    #[serde(default)]
    title: Option<String>,
    /// 项目 slug，用来查内置中文名表。
    #[serde(default)]
    slug: Option<String>,
    at: u64,
}

/// 一个本地模组文件对应的平台项目：图标、原始名、中文名。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub url: Option<String>,
    pub title: Option<String>,
    pub slug: Option<String>,
    pub chinese: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CACHE_FILE)
}

fn load_cache(data_dir: &Path) -> Cache {
    let Ok(text) = fs::read_to_string(cache_path(data_dir)) else {
        return Cache::default();
    };
    let cache: Cache = serde_json::from_str(&text).unwrap_or_default();
    if cache.version != CACHE_VERSION {
        return Cache::default();
    }
    cache
}

fn save_cache(data_dir: &Path, cache: &Cache) {
    let path = cache_path(data_dir);
    if fs::create_dir_all(data_dir).is_err() {
        return;
    }
    let temp = path.with_extension("json.tmp");
    if let Ok(text) = serde_json::to_string(cache) {
        if fs::write(&temp, text).is_ok() {
            let _ = fs::rename(&temp, &path);
        }
    }
}

/// 读一次文件，同时算 SHA1 与 CurseForge 指纹。
pub fn key_of(path: &Path, file_name: &str) -> Result<ModKey, std::io::Error> {
    use sha1::{Digest, Sha1};
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut bytes = Vec::new();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        bytes.extend_from_slice(&buffer[..read]);
    }
    let sha1 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(ModKey {
        file_name: file_name.to_owned(),
        sha1,
        fingerprint: crate::export::curseforge_fingerprint(&bytes),
    })
}

/// 按哈希查项目信息（图标、原名、中文名）：先 Modrinth，剩下的再问 CurseForge。
///
/// 返回「文件名 → 项目信息」，查不到的不会出现在结果里。
pub fn icons_for(data_dir: &Path, keys: &[ModKey]) -> HashMap<String, ProjectInfo> {
    let mut cache = load_cache(data_dir);
    let mut out: HashMap<String, ProjectInfo> = HashMap::new();
    let mut stale: Vec<&ModKey> = Vec::new();

    for key in keys {
        match cache.entries.get(&key.sha1) {
            Some(entry) if now().saturating_sub(entry.at) < CACHE_TTL.as_secs() => {
                out.insert(key.file_name.clone(), info_of(entry));
            }
            _ => stale.push(key),
        }
    }
    if stale.is_empty() {
        return out;
    }

    let mut found: HashMap<String, (Option<String>, Option<String>, Option<String>)> = HashMap::new(); // SHA1 → (url, title, slug)

    let sha1s: Vec<String> = stale.iter().map(|key| key.sha1.clone()).collect();
    if let Ok(hits) = crate::modrinth::lookup_sha1(&sha1s) {
        let mut projects: Vec<String> = hits.iter().map(|hit| hit.project_id.clone()).collect();
        projects.sort();
        projects.dedup();
        if let Ok(meta) = crate::modrinth::project_meta(&projects) {
            for hit in &hits {
                if let Some(project) = meta.get(&hit.project_id) {
                    found.insert(
                        hit.sha1.to_lowercase(),
                        (
                            project.icon.clone(),
                            Some(project.title.clone()).filter(|t| !t.is_empty()),
                            Some(project.slug.clone()).filter(|t| !t.is_empty()),
                        ),
                    );
                }
            }
        }
    }

    let left: Vec<&ModKey> = stale
        .iter()
        .copied()
        .filter(|key| !found.contains_key(&key.sha1))
        .collect();
    if !left.is_empty() {
        let fingerprints: Vec<u32> = left.iter().map(|key| key.fingerprint).collect();
        if let Ok(hits) = crate::curseforge::fingerprint_details(&fingerprints) {
            let mut by_fingerprint: HashMap<u32, u64> = HashMap::new();
            let mut mod_ids: Vec<u64> = Vec::new();
            for (fingerprint, mod_id, _file_id) in hits {
                by_fingerprint.insert(fingerprint, mod_id);
                mod_ids.push(mod_id);
            }
            mod_ids.sort();
            mod_ids.dedup();
            if let Ok(meta) = crate::curseforge::project_meta(&mod_ids) {
                for key in &left {
                    let Some(mod_id) = by_fingerprint.get(&key.fingerprint) else {
                        continue;
                    };
                    let Some(project) = meta.get(mod_id) else {
                        continue;
                    };
                    found.insert(
                        key.sha1.clone(),
                        (
                            project.icon.clone(),
                            Some(project.name.clone()).filter(|t| !t.is_empty()),
                            Some(project.slug.clone()).filter(|t| !t.is_empty()),
                        ),
                    );
                }
            }
        }
    }

    let at = now();
    for key in stale {
        let hit = found.get(&key.sha1).cloned().unwrap_or_default();
        let entry = Entry {
            url: hit.0.clone(),
            title: hit.1,
            slug: hit.2,
            at,
        };
        out.insert(key.file_name.clone(), info_of(&entry));
        cache.entries.insert(key.sha1.clone(), entry);
    }
    // 太久没动过的条目顺手清掉，别让文件无限长。
    if cache.entries.len() > 4096 {
        let cutoff = 30 * 24 * 60 * 60;
        cache
            .entries
            .retain(|_, entry| now().saturating_sub(entry.at) < cutoff);
    }
    save_cache(
        data_dir,
        &Cache {
            version: CACHE_VERSION,
            entries: cache.entries,
        },
    );
    out
}

/// 缓存条目 → 界面用的项目信息（顺便查中文名）。
fn info_of(entry: &Entry) -> ProjectInfo {
    ProjectInfo {
        url: entry.url.clone(),
        title: entry.title.clone(),
        slug: entry.slug.clone(),
        chinese: entry
            .slug
            .as_deref()
            .and_then(crate::names::chinese_name)
            .map(str::to_owned),
    }
}
