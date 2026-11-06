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
const CACHE_VERSION: u32 = 1;

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
    at: u64,
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

/// 按哈希查项目图标：先 Modrinth，剩下的再问 CurseForge。
///
/// 返回「文件名 → 图标地址」，查不到的不会出现在结果里。
pub fn icons_for(data_dir: &Path, keys: &[ModKey]) -> HashMap<String, String> {
    let mut cache = load_cache(data_dir);
    let mut out = HashMap::new();
    let mut stale: Vec<&ModKey> = Vec::new();

    for key in keys {
        match cache.entries.get(&key.sha1) {
            Some(entry) if now().saturating_sub(entry.at) < CACHE_TTL.as_secs() => {
                if let Some(url) = &entry.url {
                    out.insert(key.file_name.clone(), url.clone());
                }
            }
            _ => stale.push(key),
        }
    }
    if stale.is_empty() {
        return out;
    }

    let mut found: HashMap<String, String> = HashMap::new(); // SHA1 → 图标地址

    let sha1s: Vec<String> = stale.iter().map(|key| key.sha1.clone()).collect();
    if let Ok(hits) = crate::modrinth::lookup_sha1(&sha1s) {
        let mut projects: Vec<String> = hits.iter().map(|hit| hit.project_id.clone()).collect();
        projects.sort();
        projects.dedup();
        if let Ok(icons) = crate::modrinth::project_icon_urls(&projects) {
            for hit in &hits {
                if let Some(url) = icons.get(&hit.project_id) {
                    found.insert(hit.sha1.to_lowercase(), url.clone());
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
            if let Ok(icons) = crate::curseforge::project_icons(&mod_ids) {
                let mut hits: Vec<(String, String)> = Vec::new();
                for key in &left {
                    let Some(mod_id) = by_fingerprint.get(&key.fingerprint) else {
                        continue;
                    };
                    if let Some(url) = icons.get(mod_id) {
                        hits.push((key.sha1.clone(), url.clone()));
                    }
                }
                found.extend(hits);
            }
        }
    }

    let at = now();
    for key in stale {
        let url = found.get(&key.sha1).cloned();
        if let Some(url) = &url {
            out.insert(key.file_name.clone(), url.clone());
        }
        cache.entries.insert(key.sha1.clone(), Entry { url, at });
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
