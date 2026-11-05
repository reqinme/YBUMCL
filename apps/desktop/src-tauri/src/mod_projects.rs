//! 模组项目认领的持久缓存。
//!
//! 认领要读整个 jar 算 sha1 / 指纹再去两站问，正常几十毫秒，但右键每次都算没必要。
//! 这里按「实例 + 文件名 + 大小 + 修改时间」缓存结果（查不到也缓存成 null），
//! 文件换过自然失效。

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

const FILE_NAME: &str = "mod-projects.json";
/// 缓存超过这个条目数时只留最近的 4000 条，防止玩几年后无限增长。
const MAX_ENTRIES: usize = 4000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CachedProject {
    pub source: String,
    pub project_id: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    /// key = `实例|文件名|大小|mtime`，value = null 表示认领失败（也缓存，别重复算）。
    entries: HashMap<String, Option<CachedProject>>,
}

fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

fn load(data_dir: &Path) -> CacheFile {
    fs::read_to_string(cache_path(data_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(data_dir: &Path, mut cache: CacheFile) {
    if cache.entries.len() > MAX_ENTRIES {
        // 没有时间戳字段，按 key 排序截断即可：key 里带 mtime，越新的越大。
        let mut keys: Vec<String> = cache.entries.keys().cloned().collect();
        keys.sort();
        for key in keys.into_iter().take(cache.entries.len() - MAX_ENTRIES) {
            cache.entries.remove(&key);
        }
    }
    if let Ok(text) = serde_json::to_string(&cache) {
        let _ = fs::write(cache_path(data_dir), text);
    }
}

/// 拼缓存键；文件读不到元数据时返回 None（不缓存）。
pub fn key(instance_dir: &Path, file_name: &str) -> Option<String> {
    let path = instance_dir.join("mods").join(file_name);
    let meta = fs::metadata(&path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    let instance = instance_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(format!("{instance}|{file_name}|{}|{mtime}", meta.len()))
}

pub fn get(data_dir: &Path, key: &str) -> Option<Option<CachedProject>> {
    load(data_dir).entries.remove(key)
}

pub fn put(data_dir: &Path, key: &str, value: Option<CachedProject>) {
    let mut cache = load(data_dir);
    cache.entries.insert(key.to_owned(), value);
    save(data_dir, cache);
}
