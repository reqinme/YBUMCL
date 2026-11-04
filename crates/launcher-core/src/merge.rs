//! 把「原版 + 若干加载器」的版本 JSON 合并成一个自包含实例，行为对齐 PCL2：
//!
//! - 对象递归合并，**数组逐项追加**（不是替换），重复的库留给启动时按坐标去重；
//! - legacy 的 `minecraftArguments` 把所有来源拼起来、切分、去重后再写回；
//! - 结果里若有 LWJGL 3，就剔掉与之冲突的 LWJGL 2；
//! - 删掉 `inheritsFrom` / `jar` / `_comment_`，`id` 改成实例目录名。
//!
//! 这里只处理 JSON 与文件搬运，不联网。

use crate::{safe_component, CoreError, Result};
use serde_json::{Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// 项目支持的加载器种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderKind {
    Fabric,
    LegacyFabric,
    LabyMod,
    Quilt,
    Forge,
    NeoForge,
    Cleanroom,
    LiteLoader,
    OptiFine,
}

impl LoaderKind {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "fabric" => Some(Self::Fabric),
            "legacyfabric" | "legacy-fabric" => Some(Self::LegacyFabric),
            "labymod" | "laby-mod" => Some(Self::LabyMod),
            "quilt" => Some(Self::Quilt),
            "forge" => Some(Self::Forge),
            "neoforge" => Some(Self::NeoForge),
            "cleanroom" => Some(Self::Cleanroom),
            "liteloader" => Some(Self::LiteLoader),
            "optifine" => Some(Self::OptiFine),
            _ => None,
        }
    }

    /// 写进实例 json 的 `_ybmcl.loaders[].loader`，重启后据此还原加载器与版本号。
    pub fn key(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::LegacyFabric => "legacyfabric",
            Self::LabyMod => "labymod",
            Self::Quilt => "quilt",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
            Self::Cleanroom => "cleanroom",
            Self::LiteLoader => "liteloader",
            Self::OptiFine => "optifine",
        }
    }

    /// 实例名里的片段名（PCL2 的写法）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Fabric => "Fabric",
            Self::LegacyFabric => "LegacyFabric",
            Self::LabyMod => "LabyMod",
            Self::Quilt => "Quilt",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
            Self::Cleanroom => "Cleanroom",
            Self::LiteLoader => "LiteLoader",
            Self::OptiFine => "OptiFine",
        }
    }

    /// 合并顺序：PCL2 依次是 OptiFine → Forge → NeoForge → Cleanroom → LiteLoader → Fabric。
    fn merge_rank(self) -> u8 {
        match self {
            Self::OptiFine => 0,
            Self::Forge => 1,
            Self::NeoForge => 2,
            Self::Cleanroom => 3,
            Self::LiteLoader => 4,
            Self::Fabric => 5,
            Self::LegacyFabric => 5,
            Self::Quilt => 6,
            // LabyMod 的客户端 json 本来就带着完整的入口与参数，放最后压过其它来源。
            Self::LabyMod => 7,
        }
    }

    /// 命名顺序：Fabric → Quilt → Forge → NeoForge → Cleanroom → LiteLoader → OptiFine。
    fn name_rank(self) -> u8 {
        match self {
            Self::Fabric => 0,
            Self::LegacyFabric => 1,
            Self::LabyMod => 2,
            Self::Quilt => 3,
            Self::Forge => 4,
            Self::NeoForge => 5,
            Self::Cleanroom => 6,
            Self::LiteLoader => 7,
            Self::OptiFine => 8,
        }
    }

    /// 实例名里的这一段，例如 `Forge_47.4.9`、`LiteLoader`。
    pub fn tag(self, version: &str) -> String {
        match self {
            Self::LiteLoader => self.label().to_owned(),
            // LabyMod 的版本号后面可能带频道标记 `48.3@snapshot`，名字里写成 `_Snapshot`。
            Self::LabyMod => match version.split_once('@') {
                Some((base, "snapshot")) => format!("LabyMod_{base}_Snapshot"),
                Some((base, _)) => format!("LabyMod_{base}"),
                None => format!("LabyMod_{}", version.replace(' ', "_")),
            },
            _ => format!("{}_{}", self.label(), version.replace(' ', "_")),
        }
    }
}

/// 要合并进来的一个加载器。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderPart {
    pub kind: LoaderKind,
    pub version: String,
    /// 安装器/元数据写出的版本 id（`versions/<version_id>`）。
    pub version_id: String,
}

/// PCL2 风格的实例名：`<原版>-Fabric_0.20.1-Forge_47.4.9-…`；没有加载器时就是原版号。
pub fn instance_name(game: &str, parts: &[LoaderPart]) -> String {
    let mut parts = parts.to_vec();
    parts.sort_by_key(|part| part.kind.name_rank());
    let mut name = game.to_owned();
    for part in parts {
        name.push('-');
        name.push_str(&part.kind.tag(&part.version));
    }
    name
}

/// 合并顺序（OptiFine 最先，Fabric/Quilt 最后）。
pub fn merge_order(parts: &[LoaderPart]) -> Vec<LoaderPart> {
    let mut parts = parts.to_vec();
    parts.sort_by_key(|part| part.kind.merge_rank());
    parts
}

/// 目标实例是否已经存在（目录里有同名 json，或目录本身非空）。
pub fn instance_taken(root: &Path, id: &str) -> bool {
    let folder = root.join("versions").join(id);
    if folder.join(format!("{id}.json")).is_file() {
        return true;
    }
    fs::read_dir(&folder)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

/// 深度合并：对象递归、数组追加、其余覆盖（PCL2 的 `Merge` 语义）。
pub fn merge_into(target: &mut Value, source: &Value) {
    let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) else {
        return;
    };
    merge_objects(target, source);
}

fn merge_objects(target: &mut Map<String, Value>, source: &Map<String, Value>) {
    for (key, value) in source {
        let Some(existing) = target.get_mut(key) else {
            target.insert(key.clone(), value.clone());
            continue;
        };
        if existing.is_object() && value.is_object() {
            if let (Value::Object(child), Value::Object(patch)) = (&mut *existing, value) {
                merge_objects(child, patch);
            }
        } else if existing.is_array() && value.is_array() {
            if let (Value::Array(items), Value::Array(extra)) = (&mut *existing, value) {
                items.extend(extra.iter().cloned());
            }
        } else {
            *existing = value.clone();
        }
    }
}

/// 把 legacy 的 `minecraftArguments` 拼起来：切分、把选项和它的值并回去、去重。
pub fn merged_arguments(jsons: &[&Value]) -> Option<String> {
    let joined = jsons
        .iter()
        .filter_map(|json| json.get("minecraftArguments").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    if joined.trim().is_empty() {
        return None;
    }
    let mut arguments: Vec<String> = Vec::new();
    for token in joined.split_whitespace() {
        match arguments.last_mut() {
            Some(last)
                if !token.starts_with('-') && last.starts_with('-') && !last.contains(' ') =>
            {
                last.push(' ');
                last.push_str(token);
            }
            _ => arguments.push(token.to_owned()),
        }
    }
    let mut unique: Vec<String> = Vec::new();
    for argument in arguments {
        if !unique.contains(&argument) {
            unique.push(argument);
        }
    }
    Some(unique.join(" "))
}

fn library_coordinates(value: &Value) -> Option<(&str, Vec<&str>)> {
    let name = value["name"].as_str()?;
    Some((name, name.split(':').collect()))
}

/// 有 LWJGL 3 时剔掉 LWJGL 2（PCL2 的 `IsLwjgl3Library` / `IsLwjgl2Library`）。
pub fn drop_conflicting_lwjgl2(value: &mut Value) {
    let Some(libraries) = value.get("libraries").and_then(Value::as_array) else {
        return;
    };
    if !libraries.iter().any(is_lwjgl3) {
        return;
    }
    if let Some(libraries) = value.get_mut("libraries").and_then(Value::as_array_mut) {
        libraries.retain(|library| !is_lwjgl2(library));
    }
}

fn is_lwjgl3(value: &Value) -> bool {
    library_coordinates(value).is_some_and(|(_, segments)| {
        segments.len() >= 3 && segments[0] == "org.lwjgl" && segments[2].starts_with("3.")
    })
}

fn is_lwjgl2(value: &Value) -> bool {
    library_coordinates(value).is_some_and(|(_, segments)| {
        segments.len() >= 3
            && (segments[0] == "org.lwjgl.lwjgl"
                || (segments[0] == "org.lwjgl" && segments[2].starts_with("2.")))
    })
}

/// 合成后的完整版本 JSON。
pub fn merged_version(base: &Value, parts: &[&Value], id: &str) -> Value {
    let mut output = base.clone();
    for part in parts {
        merge_into(&mut output, part);
    }
    let mut sources = vec![base];
    sources.extend(parts.iter().copied());
    if let Some(arguments) = merged_arguments(&sources) {
        output["minecraftArguments"] = Value::String(arguments);
    }
    drop_conflicting_lwjgl2(&mut output);
    if let Some(object) = output.as_object_mut() {
        object.remove("inheritsFrom");
        object.remove("jar");
        object.remove("_comment_");
        object.insert("id".into(), Value::String(id.to_owned()));
    }
    output
}

/// 读取 `<root>/versions/<id>/<id>.json`。
pub fn read_version(root: &Path, id: &str) -> Result<Value> {
    let path = version_json(root, id);
    let bytes = fs::read(&path).map_err(|error| io(&path, error))?;
    serde_json::from_slice(&bytes).map_err(|error| CoreError::InvalidInput(error.to_string()))
}

fn io(path: &Path, error: std::io::Error) -> CoreError {
    CoreError::Io {
        path: path.display().to_string(),
        source: error,
    }
}

fn version_json(root: &Path, id: &str) -> PathBuf {
    root.join("versions").join(id).join(format!("{id}.json"))
}

/// `<root>/versions/<id>/<id>.json`，安装流程判断原版是否已装时用。
pub fn version_json_path(root: &Path, id: &str) -> PathBuf {
    version_json(root, id)
}

/// 写出合并后的实例：json + 从原版拷来的 client jar。
///
/// `base_id` 是原版版本 id，合并后的 `id` 是实例目录名；两者不同才拷 jar。
pub fn write_instance(root: &Path, base_id: &str, parts: &[LoaderPart], id: &str) -> Result<Value> {
    if !safe_component(id) {
        return Err(CoreError::InvalidInput("Unsafe instance name".into()));
    }
    let base = read_version(root, base_id)?;
    let ordered = merge_order(parts);
    let mut jsons = Vec::with_capacity(ordered.len());
    for part in &ordered {
        jsons.push(read_version(root, &part.version_id)?);
    }
    let references: Vec<&Value> = jsons.iter().collect();
    let mut merged = merged_version(&base, &references, id);
    // 记下「原版版本 + 各加载器版本」，重装/重置时才能装回同一套东西。
    let loaders: Vec<Value> = ordered
        .iter()
        .map(|part| {
            serde_json::json!({
                "loader": part.kind.key(),
                "version": if part.version.is_empty() { None } else { Some(part.version.clone()) },
            })
        })
        .collect();
    if let Some(object) = merged.as_object_mut() {
        object.insert(
            "_ybmcl".to_string(),
            serde_json::json!({ "gameVersion": base_id, "loaders": loaders }),
        );
    }

    let folder = root.join("versions").join(id);
    fs::create_dir_all(&folder).map_err(|error| io(&folder, error))?;
    let path = folder.join(format!("{id}.json"));
    let text = serde_json::to_vec_pretty(&merged)
        .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    fs::write(&path, text).map_err(|error| io(&path, error))?;

    if id != base_id {
        let source = root
            .join("versions")
            .join(base_id)
            .join(format!("{base_id}.jar"));
        if source.is_file() {
            let target = folder.join(format!("{id}.jar"));
            if target.exists() {
                fs::remove_file(&target).map_err(|error| io(&target, error))?;
            }
            fs::copy(&source, &target).map_err(|error| io(&target, error))?;
        }
    }
    Ok(merged)
}

/// 给不走合并流程的实例 json 补上 `_ybmcl`（原版实例、加载器外壳都是直接写 json 的）。
///
/// `loaders` 是 `(loader key, 版本号)`，原版实例传空切片；版本号为空串写 `null`。
/// 已经有 `_ybmcl` 的文件原样不动，免得把合并时记下的那套加载器覆盖掉。
pub fn stamp_meta(root: &Path, id: &str, game_version: &str, loaders: &[(&str, &str)]) -> Result<()> {
    if !safe_component(id) {
        return Err(CoreError::InvalidInput("Unsafe instance name".into()));
    }
    let path = version_json(root, id);
    let bytes = fs::read(&path).map_err(|error| io(&path, error))?;
    let mut value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    let Some(object) = value.as_object_mut() else {
        return Err(CoreError::InvalidInput(format!(
            "{} 不是版本 json 对象",
            path.display()
        )));
    };
    if object.contains_key("_ybmcl") {
        return Ok(());
    }
    let list: Vec<Value> = loaders
        .iter()
        .map(|(loader, version)| {
            serde_json::json!({
                "loader": loader,
                "version": if version.is_empty() { None } else { Some(*version) },
            })
        })
        .collect();
    object.insert(
        "_ybmcl".to_string(),
        serde_json::json!({ "gameVersion": game_version, "loaders": list }),
    );
    let text = serde_json::to_vec_pretty(&value)
        .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    fs::write(&path, text).map_err(|error| io(&path, error))
}

/// 删除合并过程中用掉的临时版本目录（只删 `versions/<id>`，不碰其它）。
pub fn remove_version(root: &Path, id: &str) {
    if !safe_component(id) || id.is_empty() {
        return;
    }
    let folder = root.join("versions").join(id);
    let _ = fs::remove_dir_all(folder);
}

/// 备份实例的 json 与 jar（覆盖安装前的保险）。
pub fn backup_instance(root: &Path, id: &str) -> Result<Option<PathBuf>> {
    let folder = root.join("versions").join(id);
    if !folder.exists() {
        return Ok(None);
    }
    let backup = folder.join("PCLInstallBackups");
    fs::create_dir_all(&backup).map_err(|error| io(&backup, error))?;
    for name in [format!("{id}.json"), format!("{id}.jar")] {
        let source = folder.join(&name);
        if source.is_file() {
            let target = backup.join(&name);
            fs::copy(&source, &target).map_err(|error| io(&target, error))?;
        }
    }
    Ok(Some(backup))
}

/// 回滚：把备份拷回去并清掉备份目录。
pub fn restore_instance(root: &Path, id: &str, backup: Option<&Path>) {
    let Some(backup) = backup else {
        return;
    };
    let folder = root.join("versions").join(id);
    if let Ok(entries) = fs::read_dir(backup) {
        for entry in entries.flatten() {
            let target = folder.join(entry.file_name());
            let _ = fs::copy(entry.path(), target);
        }
    }
    let _ = fs::remove_dir_all(backup);
}

/// 安装成功后清掉备份。
pub fn drop_backup(backup: Option<&Path>) {
    if let Some(backup) = backup {
        let _ = fs::remove_dir_all(backup);
    }
}

/// 半成品标记：安装期间让启动器把该目录当不存在（与 PCL2 的 `.pclignore` 同名互认）。
pub const IGNORE_FILE: &str = ".pclignore";

pub fn mark_incomplete(root: &Path, id: &str) -> Result<PathBuf> {
    let folder = root.join("versions").join(id);
    fs::create_dir_all(&folder).map_err(|error| io(&folder, error))?;
    let marker = folder.join(IGNORE_FILE);
    fs::write(&marker, "安装中，启动器会暂时忽略这个实例。").map_err(|error| io(&marker, error))?;
    Ok(marker)
}

pub fn clear_incomplete(root: &Path, id: &str) {
    let _ = fs::remove_file(root.join("versions").join(id).join(IGNORE_FILE));
}

/// 删掉下载到一半的 `.part` 文件（取消时用）：assets/libraries/.ybmcl/versions 都扫一遍。
/// 完整文件（SHA1 已经对上的）一个都不动，否则别的实例又要重下一遍。
pub fn sweep_parts(root: &Path) -> usize {
    fn walk(dir: &Path, depth: usize, removed: &mut usize) {
        if depth > 8 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                walk(&path, depth + 1, removed);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("part")
                && fs::remove_file(&path).is_ok()
            {
                *removed += 1;
            }
        }
    }
    let mut removed = 0;
    for folder in ["assets", "libraries", "versions", ".ybmcl"] {
        walk(&root.join(folder), 0, &mut removed);
    }
    removed
}

/// 清掉上次被强行中断留下的半成品目录（里面只有一个 `.pclignore`）。
/// 启动时扫一遍，否则同名重装会被判「已存在」。
pub fn prune_incomplete(root: &Path) -> Result<Vec<String>> {
    let versions = root.join("versions");
    let mut removed = Vec::new();
    let Ok(entries) = fs::read_dir(&versions) else {
        return Ok(removed);
    };
    for entry in entries.flatten() {
        let folder = entry.path();
        if !folder.is_dir() || !is_incomplete(&folder) {
            continue;
        }
        let only_ignore = fs::read_dir(&folder)
            .map(|items| {
                items
                    .flatten()
                    .all(|item| item.file_name().to_string_lossy() == IGNORE_FILE)
            })
            .unwrap_or(false);
        if !only_ignore {
            continue;
        }
        let id = folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let _ = fs::remove_dir_all(&folder);
        removed.push(id);
    }
    Ok(removed)
}

/// 实例是否被标记为未完成（扫描版本列表时跳过）。
pub fn is_incomplete(folder: &Path) -> bool {
    folder.join(IGNORE_FILE).is_file()
}

/// 把所有「版本核心 + 加载器 profile」的两层实例就地合并成自包含实例。
///
/// 只改 json 与 jar，不动 mods / 存档，也不改实例名（隔离目录是按名字定位的）。
pub fn migrate_legacy_versions(root: &Path) -> Result<Vec<String>> {
    let versions = root.join("versions");
    let Ok(entries) = fs::read_dir(&versions) else {
        return Ok(Vec::new());
    };
    let mut migrated = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if !safe_component(&id) || is_incomplete(&entry.path()) {
            continue;
        }
        if migrate_legacy_version(root, &id)? {
            migrated.push(id);
        }
    }
    migrated.sort();
    Ok(migrated)
}

/// 就地合并一个两层实例；本来就是自包含的返回 `Ok(false)`。
pub fn migrate_legacy_version(root: &Path, id: &str) -> Result<bool> {
    let Ok(raw) = read_version(root, id) else {
        return Ok(false);
    };
    let Some(parent) = raw["inheritsFrom"].as_str().map(str::to_owned) else {
        return Ok(false);
    };
    // 从最近的一层往上走，直到没有 inheritsFrom 的原版。
    let mut chain: Vec<Value> = Vec::new();
    let mut current = parent.clone();
    let mut seen: Vec<String> = vec![id.to_owned()];
    let base;
    loop {
        if seen.contains(&current) || seen.len() > 8 {
            return Ok(false);
        }
        seen.push(current.clone());
        let Ok(value) = read_version(root, &current) else {
            return Ok(false);
        };
        match value["inheritsFrom"].as_str().map(str::to_owned) {
            Some(next) => {
                chain.push(value);
                current = next;
            }
            None => {
                base = value;
                break;
            }
        }
    }
    chain.push(raw.clone());
    // 合并顺序：底稿 → 离原版最近的父层 → 子层。
    chain.reverse();
    let mut base_id = parent;
    if let Some(first) = chain.first() {
        let _ = first;
    }
    if let Some(name) = base["id"].as_str() {
        base_id = name.to_owned();
    }
    let merged = merged_version(&base, &chain.iter().collect::<Vec<_>>(), id);
    let path = version_json(root, id);
    let text = serde_json::to_vec_pretty(&merged)
        .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    fs::write(&path, text).map_err(|error| io(&path, error))?;
    let target_jar = root.join("versions").join(id).join(format!("{id}.jar"));
    if !target_jar.is_file() {
        let base_jar = root
            .join("versions")
            .join(&base_id)
            .join(format!("{base_id}.jar"));
        if base_jar.is_file() {
            fs::copy(&base_jar, &target_jar).map_err(|error| io(&target_jar, error))?;
        }
    }
    // 只删「加载器外壳」（本身也带 inheritsFrom 的中间层），原版目录留着：
    // 用户可能还想单独玩原版，删了会让人莫名其妙少一个版本。
    for layer in &chain {
        let Some(layer_id) = layer["id"].as_str() else {
            continue;
        };
        if layer_id == id || layer.get("inheritsFrom").is_none() {
            continue;
        }
        if !referenced_by(root, layer_id, id) {
            remove_version(root, layer_id);
        }
    }
    let _ = base_id;
    Ok(true)
}

/// 除 `except` 以外，是否还有版本 json 继承/复用 `id`。
fn referenced_by(root: &Path, id: &str, except: &str) -> bool {
    let Ok(entries) = fs::read_dir(root.join("versions")) else {
        return false;
    };
    for entry in entries.flatten() {
        let other = entry.file_name().to_string_lossy().into_owned();
        if other == except || other == id || !entry.path().is_dir() {
            continue;
        }
        let Ok(value) = read_version(root, &other) else {
            continue;
        };
        if value["inheritsFrom"].as_str() == Some(id) || value["jar"].as_str() == Some(id) {
            return true;
        }
    }
    false
}

/// 老 YBUMCL／别的启动器留下的加载器式目录名，换成 PCL 的写法。
///
/// `fabric-loader-0.20.1-1.20.1` → `1.20.1-Fabric_0.20.1`、
/// `neoforge-21.1.99` → `1.21.1-NeoForge_21.1.99`。
pub fn pcl_name(root: &Path, id: &str) -> Option<String> {
    let game_of_json = || -> Option<String> {
        let value = read_version(root, id).ok()?;
        for key in ["clientVersion", "inheritsFrom", "jar"] {
            if let Some(game) = value[key].as_str() {
                return Some(game.to_owned());
            }
        }
        None
    };
    let lower = id.to_ascii_lowercase();
    for (prefix, kind) in [
        ("fabric-loader-", LoaderKind::Fabric),
        ("quilt-loader-", LoaderKind::Quilt),
        ("legacy-fabric-loader-", LoaderKind::Fabric),
    ] {
        let Some(rest) = lower.strip_prefix(prefix).map(|_| &id[prefix.len()..]) else {
            continue;
        };
        // 形如 <loader 版本>-<游戏版本>，游戏版本从右边切。
        let Some((loader, game)) = rest.rsplit_once('-') else {
            return None;
        };
        if !looks_like_game(game) || loader.is_empty() {
            return None;
        }
        return Some(format!("{game}-{}", kind.tag(loader)));
    }
    for (prefix, kind) in [
        ("forge-", LoaderKind::Forge),
        ("neoforge-", LoaderKind::NeoForge),
        ("cleanroom-", LoaderKind::Cleanroom),
    ] {
        let Some(rest) = lower.strip_prefix(prefix).map(|_| &id[prefix.len()..]) else {
            continue;
        };
        if kind == LoaderKind::Forge {
            // forge-<游戏版本>-<forge 版本>
            let Some((game, version)) = rest.split_once('-') else {
                return None;
            };
            if !looks_like_game(game) || version.is_empty() {
                return None;
            }
            return Some(format!("{game}-{}", kind.tag(version)));
        }
        // 只写加载器版本的目录，游戏版本要去 json 里找。
        let game = game_of_json()?;
        if rest.is_empty() {
            return None;
        }
        return Some(format!("{game}-{}", kind.tag(rest)));
    }
    None
}

fn looks_like_game(text: &str) -> bool {
    !text.is_empty()
        && text
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// 把一个实例改名：目录、json 名字与 `id`、jar 名字一起换。
pub fn rename_instance(root: &Path, old: &str, new: &str) -> Result<()> {
    if !safe_component(new) || instance_taken(root, new) {
        return Err(CoreError::InvalidInput(format!("实例名 {new} 不可用")));
    }
    let from = root.join("versions").join(old);
    let to = root.join("versions").join(new);
    let from_json = from.join(format!("{old}.json"));
    let to_json = to.join(format!("{new}.json"));
    let raw = read_version(root, old)?;
    let from_jar = from.join(format!("{old}.jar"));
    let to_jar = to.join(format!("{new}.jar"));
    fs::create_dir_all(&to).map_err(|error| io(&to, error))?;
    fs::rename(&from_json, &to_json).map_err(|error| io(&to_json, error))?;
    if from_jar.is_file() {
        fs::rename(&from_jar, &to_jar).map_err(|error| io(&to_jar, error))?;
    }
    let mut value = raw;
    value["id"] = Value::String(new.to_owned());
    let text = serde_json::to_vec_pretty(&value)
        .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    fs::write(&to_json, text).map_err(|error| io(&to_json, error))?;
    // 目录里剩下的东西（launcher-natives、crash-reports、settings）一起搬。
    if let Ok(entries) = fs::read_dir(&from) {
        for entry in entries.flatten() {
            let path = entry.path();
            if to.join(entry.file_name()).exists() {
                continue;
            }
            let _ = fs::rename(&path, to.join(entry.file_name()));
        }
    }
    let _ = fs::remove_dir(&from);
    Ok(())
}

/// 启动时把加载器式的老目录名换成 PCL 写法；目标名已有实例就跳过。
pub fn rename_legacy_versions(root: &Path) -> Result<Vec<(String, String)>> {
    let Ok(entries) = fs::read_dir(root.join("versions")) else {
        return Ok(Vec::new());
    };
    let mut renames = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if is_incomplete(&entry.path()) {
            continue;
        }
        let Some(new) = pcl_name(root, &id) else {
            continue;
        };
        if new == id || instance_taken(root, &new) {
            continue;
        }
        if rename_instance(root, &id, &new).is_ok() {
            renames.push((id, new));
        }
    }
    Ok(renames)
}

/// 一个空的 map，方便测试里构造 JSON。
pub fn empty_object() -> Value {
    Value::Object(Map::new())
}
