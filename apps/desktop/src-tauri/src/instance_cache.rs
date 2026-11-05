//! 实例概述缓存。
//!
//! 照 PCL 的做法（`ModMinecraft.vb` 的 `InstanceCache`）：把版本列表 / 实例页
//! 概述要用的那份数据整批算一次，写进数据目录；下次打开先看它，键对得上就
//! 直接拿来用，不再逐个读实例的 json。
//!
//! 键 = 缓存格式版本 + 游戏目录 + `versions/` 下每个实例目录的名字与关键文件
//! （`<id>.json`、`ybmcl.json`）的修改时间和大小。改名、改设置、装完、删掉
//! 之后键自然就变了，不必让每个命令都记得来清缓存。
//!
//! 只缓存「概述」：模组、设置页、导出等子页面仍然各走各的读取。

use serde::{de::DeserializeOwned, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// 缓存格式版本：以后改了 `VersionView` 的字段就把它 +1，旧缓存自动作废。
const CACHE_VERSION: u32 = 1;
const FILE_NAME: &str = "instance-cache.json";

#[derive(Serialize, serde::Deserialize)]
struct CacheFile<T> {
    version: u32,
    key: String,
    value: T,
}

fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

/// 版本概述的「指纹」：目录里有哪些实例、每个实例的 json 改过没有。
pub fn key(game_dir: &Path) -> String {
    let mut entries: Vec<(String, u64, u64)> = Vec::new();
    if let Ok(read) = std::fs::read_dir(game_dir.join("versions")) {
        for entry in read.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if !kind.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let dir = entry.path();
            let mut stamp = 0u64;
            for file in [
                format!("{name}.json"),
                "ybmcl.json".to_string(),
                ".pclignore".to_string(),
            ] {
                let path = dir.join(&file);
                if let Ok(meta) = std::fs::metadata(&path) {
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|time| time.as_millis() as u64)
                        .unwrap_or(0);
                    stamp = stamp
                        .wrapping_mul(31)
                        .wrapping_add(mtime)
                        .wrapping_add(meta.len());
                }
            }
            entries.push((name, stamp, 0));
        }
    }
    entries.sort();
    let mut hasher = DefaultHasher::new();
    CACHE_VERSION.hash(&mut hasher);
    game_dir.to_string_lossy().hash(&mut hasher);
    entries.hash(&mut hasher);
    format!("{CACHE_VERSION}-{:016x}", hasher.finish())
}

/// 键对得上就返回缓存里的值。
pub fn read<T: DeserializeOwned>(data_dir: &Path, key: &str) -> Option<T> {
    let bytes = std::fs::read(cache_path(data_dir)).ok()?;
    let file: CacheFile<T> = serde_json::from_slice(&bytes).ok()?;
    if file.version != CACHE_VERSION || file.key != key {
        return None;
    }
    Some(file.value)
}

/// 写回缓存；失败不影响主流程（下次再算一遍）。
pub fn write<T: Serialize>(data_dir: &Path, key: &str, value: &T) {
    let file = CacheFile {
        version: CACHE_VERSION,
        key: key.to_string(),
        value,
    };
    let Ok(bytes) = serde_json::to_vec(&file) else {
        return;
    };
    let path = cache_path(data_dir);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, bytes);
}
