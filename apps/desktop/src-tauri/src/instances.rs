//! 旧版 `instances.json` 的最小读取器。
//!
//! 现在只有 `versions.rs::migrate_legacy` 在迁移时用它一次，读完整份文件就再也
//! 不写回。所以这里只保留「解析并做基本校验」需要的字段，旧版的写入、删除、
//! 注册表加锁等逻辑都已经随实例模型一起删掉了。

use serde::Deserialize;
use std::{fs::File, io::Read, path::Path};

const REGISTRY_FILE: &str = "instances.json";
const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    pub name: String,
    pub version_id: String,
    pub java_path: String,
    pub max_memory_mb: u32,
    pub username: String,
    pub isolated: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryFile {
    schema_version: u32,
    instances: Vec<Instance>,
}

/// 读取旧注册表；文件不存在视为空。
pub fn list_instances(data_dir: &Path) -> Result<Vec<Instance>, String> {
    let path = data_dir.join(REGISTRY_FILE);
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("failed to open instance registry: {error}")),
    };
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|error| format!("failed to read instance registry: {error}"))?;
    let registry: RegistryFile = serde_json::from_str(&contents)
        .map_err(|error| format!("invalid instance registry: {error}"))?;
    if registry.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported instance registry schema version: {}",
            registry.schema_version
        ));
    }
    Ok(registry.instances)
}
