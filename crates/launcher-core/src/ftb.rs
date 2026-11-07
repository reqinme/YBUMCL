//! FTB（Feed The Beast）整合包。
//!
//! 只用 `api.modpacks.ch` 的 `public` 接口：搜索、整合包详情、版本详情。
//! 它不给整包 zip，只给「每个文件一条」的清单（url + sha1 + size），
//! 所以安装时把清单转成 modrinth.index.json 塞进一个临时 mrpack，
//! 交给 `modpack::install_modpack` 走同一套下载 / 校验 / 实例创建流程。

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::modrinth::{MrProject, MrVersion, SearchPage};
use crate::{CoreError, Result};

const API: &str = "https://api.modpacks.ch";
/// 详情是一条一包，列表页最多拉 40 条，串行太慢，这里并发取。
const DETAIL_THREADS: usize = 8;
const MAX_LIMIT: u32 = 100;

fn client() -> Result<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .user_agent("YBUMCL/0.1 (Minecraft launcher; https://github.com/ybumcl)")
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| CoreError::InvalidInput("无法创建 HTTP 客户端".into()))
}

fn get_json(url: &str) -> Result<Value> {
    // 复用的 keep-alive 连接偶尔会被服务端提前关掉，第一次发不出去时立刻重试一次。
    let mut last: Option<String> = None;
    let mut response = None;
    for attempt in 0..2 {
        match client()?.get(url).send() {
            Ok(item) => {
                response = Some(item);
                break;
            }
            Err(error) => {
                last = Some(error.to_string());
                if attempt == 0 {
                    std::thread::sleep(Duration::from_millis(300));
                }
            }
        }
    }
    let response = response.ok_or_else(|| {
        CoreError::InvalidInput(format!(
            "FTB 请求失败：{}",
            last.unwrap_or_else(|| "未知原因".into())
        ))
    })?;
    if !response.status().is_success() {
        return Err(CoreError::InvalidInput(format!(
            "FTB 返回 {}：{url}",
            response.status()
        )));
    }
    let body = response
        .text()
        .map_err(|error| CoreError::InvalidInput(format!("FTB 读取响应失败：{error}")))?;
    serde_json::from_str::<Value>(&body)
        .map_err(|error| CoreError::InvalidInput(format!("FTB 返回的不是 JSON：{error}")))
}

// ---------------------------------------------------------------- 接口模型

#[derive(Debug, Clone, Deserialize)]
struct SearchIds {
    #[serde(default)]
    packs: Vec<i64>,
    #[serde(default)]
    total: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct Pack {
    id: i64,
    name: String,
    #[serde(default)]
    synopsis: String,
    #[serde(default)]
    installs: u64,
    #[serde(default)]
    plays: u64,
    #[serde(default)]
    tags: Vec<Tag>,
    #[serde(default)]
    authors: Vec<Author>,
    #[serde(default)]
    art: Vec<Art>,
    #[serde(default)]
    versions: Vec<PackVersion>,
}

#[derive(Debug, Clone, Deserialize)]
struct Tag {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Author {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Art {
    #[serde(rename = "type")]
    kind: String,
    url: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PackVersion {
    id: i64,
    name: String,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    updated: u64,
    #[serde(default)]
    targets: Vec<Target>,
}

#[derive(Debug, Clone, Deserialize)]
struct Target {
    name: String,
    version: String,
    #[serde(rename = "type", default)]
    kind: String,
}

#[derive(Debug, Clone, Deserialize)]
struct VersionDetail {
    #[serde(default)]
    files: Vec<FileEntry>,
    #[serde(default)]
    targets: Vec<Target>,
}

#[derive(Debug, Clone, Deserialize)]
struct FileEntry {
    #[serde(default)]
    path: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    mirrors: Vec<String>,
    #[serde(default)]
    sha1: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    serveronly: bool,
}

// ---------------------------------------------------------------- 详情缓存

fn cache() -> &'static Mutex<HashMap<i64, Pack>> {
    static CACHE: OnceLock<Mutex<HashMap<i64, Pack>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pack_of(id: i64) -> Result<Pack> {
    if let Some(pack) = cache().lock().ok().and_then(|map| map.get(&id).cloned()) {
        return Ok(pack);
    }
    let value = get_json(&format!("{API}/public/modpack/{id}"))?;
    let pack: Pack = serde_json::from_value(value)
        .map_err(|error| CoreError::InvalidInput(format!("FTB 整合包数据不完整：{error}")))?;
    if let Ok(mut map) = cache().lock() {
        map.insert(id, pack.clone());
    }
    Ok(pack)
}

/// 并发取多个整合包详情，保持传入顺序。
fn packs_of(ids: &[i64]) -> Vec<Option<Pack>> {
    let mut slots: Vec<Option<Pack>> = vec![None; ids.len()];
    let chunks = (ids.len() + DETAIL_THREADS - 1) / DETAIL_THREADS.max(1);
    if chunks == 0 {
        return slots;
    }
    let mut results: Vec<(usize, Option<Pack>)> = Vec::new();
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (chunk_index, chunk) in ids.chunks(chunks).enumerate() {
            handles.push(scope.spawn(move || {
                chunk
                    .iter()
                    .enumerate()
                    .map(|(offset, id)| (chunk_index * chunks + offset, pack_of(*id).ok()))
                    .collect::<Vec<_>>()
            }));
        }
        for handle in handles {
            if let Ok(items) = handle.join() {
                results.extend(items);
            }
        }
    });
    for (index, pack) in results {
        if index < slots.len() {
            slots[index] = pack;
        }
    }
    slots
}

// ---------------------------------------------------------------- 映射

fn game_versions_of(pack: &Pack) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut list = Vec::new();
    for version in &pack.versions {
        for target in &version.targets {
            if target.kind == "game" && seen.insert(target.version.clone()) {
                list.push(target.version.clone());
            }
        }
    }
    list
}

fn loaders_of(pack: &Pack) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut list = Vec::new();
    for version in &pack.versions {
        for target in &version.targets {
            if target.kind == "modloader" && seen.insert(target.name.clone()) {
                list.push(target.name.clone());
            }
        }
    }
    list
}

fn project_of(pack: Pack) -> MrProject {
    let icon = pack
        .art
        .iter()
        .find(|art| art.kind == "square")
        .or_else(|| pack.art.first())
        .map(|art| art.url.clone());
    let author = pack
        .authors
        .iter()
        .map(|author| author.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    MrProject {
        id: format!("ftb:{}", pack.id),
        slug: format!("ftb-{}", pack.id),
        title: pack.name.clone(),
        description: pack.synopsis.clone(),
        icon_url: icon,
        downloads: pack.installs.max(0),
        author,
        categories: pack.tags.iter().map(|tag| tag.name.clone()).collect(),
        project_type: "modpack".into(),
        loaders: loaders_of(&pack),
        game_versions: game_versions_of(&pack),
        chinese_title: None,
    }
}

/// 秒级时间戳 → `YYYY-MM-DD`（只用于列表里显示日期，不做时区换算）。
fn date_of(epoch: u64) -> String {
    if epoch == 0 {
        return String::new();
    }
    let mut days = (epoch / 86_400) as i64;
    // Howard Hinnant 的 civil_from_days。
    days += 719_468;
    let era = days.div_euclid(146_097);
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

fn version_of(pack: &Pack, version: &PackVersion) -> MrVersion {
    let game_versions: Vec<String> = version
        .targets
        .iter()
        .filter(|target| target.kind == "game")
        .map(|target| target.version.clone())
        .collect();
    let loaders: Vec<String> = version
        .targets
        .iter()
        .filter(|target| target.kind == "modloader")
        .map(|target| target.name.clone())
        .collect();
    MrVersion {
        id: format!("{}:{}", pack.id, version.id),
        name: version.name.clone(),
        version_number: version.name.clone(),
        game_versions,
        loaders,
        environments: Vec::new(),
        date_published: date_of(version.updated),
        downloads: 0,
        version_type: version.kind.clone(),
        file_name: format!("{} {}.zip", pack.name, version.name),
        file_url: String::new(),
        file_size: 0,
        dependencies: Vec::new(),
    }
}

// ---------------------------------------------------------------- 对外接口

/// 列表里的项目 id 长这样：`ftb:123`。
pub fn parse_id(id: &str) -> Result<i64> {
    id.strip_prefix("ftb:")
        .and_then(|rest| rest.parse::<i64>().ok())
        .ok_or_else(|| CoreError::InvalidInput(format!("不是 FTB 项目：{id}")))
}

/// 搜索整合包。FTB 只给 id 数组，详情要再一条条取。
pub fn search(term: &str, sort: &str, page: u32, limit: u32) -> Result<SearchPage> {
    let limit = limit.clamp(1, MAX_LIMIT);
    // FTB 的列表接口没有 offset，只能「多取一点再切片」。
    let want = ((page + 1) * limit).min(MAX_LIMIT);
    let trimmed = term.trim();
    let term_query = urlencode(trimmed);
    // FTB 的关键词搜索要求至少两个字符，空关键词就走「热门 / 最近更新」列表。
    let needs_listing = trimmed.chars().count() < 2 || sort != "relevance";
    let url = if needs_listing {
        match sort {
            "follows" => format!("{API}/public/modpack/popular/plays/{want}"),
            "updated" | "newest" => format!("{API}/public/modpack/updated/{want}"),
            _ => format!("{API}/public/modpack/popular/installs/{want}"),
        }
    } else {
        format!("{API}/public/modpack/search/{want}?term={term_query}")
    };
    let value = get_json(&url)?;
    if value.get("status").and_then(|item| item.as_str()) == Some("error") {
        let message = value
            .get("message")
            .and_then(|item| item.as_str())
            .unwrap_or("搜索失败");
        // 「没有匹配的整合包」不是故障，交给界面显示空结果就行。
        if message.to_ascii_lowercase().contains("no matching")
            || message.to_ascii_lowercase().contains("not found")
        {
            return Ok(SearchPage {
                items: Vec::new(),
                total: 0,
            });
        }
        return Err(CoreError::InvalidInput(format!("FTB：{message}")));
    }
    let ids: SearchIds = serde_json::from_value(value)
        .map_err(|error| CoreError::InvalidInput(format!("FTB 搜索数据不对：{error}")))?;
    let start = (page * limit) as usize;
    let page_ids: Vec<i64> = ids
        .packs
        .iter()
        .skip(start)
        .take(limit as usize)
        .copied()
        .collect();
    let projects = packs_of(&page_ids)
        .into_iter()
        .flatten()
        .map(project_of)
        .collect();
    Ok(SearchPage {
        items: projects,
        total: ids.total,
    })
}

/// 整合包的项目详情（单独取一份，列表里已经有的话直接走缓存）。
pub fn project(id: &str) -> Result<MrProject> {
    Ok(project_of(pack_of(parse_id(id)?)?))
}

/// 版本列表；`game` / `loader` 为空表示不过滤。
pub fn versions(id: &str, game: Option<&str>, loader: Option<&str>) -> Result<Vec<MrVersion>> {
    let pack = pack_of(parse_id(id)?)?;
    let game = game.map(str::trim).filter(|value| !value.is_empty());
    let loader = loader.map(str::trim).filter(|value| !value.is_empty());
    let mut list: Vec<MrVersion> = pack
        .versions
        .iter()
        .map(|version| version_of(&pack, version))
        .filter(|version| match game {
            Some(game) => version.game_versions.iter().any(|value| value == game),
            None => true,
        })
        .filter(|version| match loader {
            Some(loader) => version
                .loaders
                .iter()
                .any(|value| value.eq_ignore_ascii_case(loader)),
            None => true,
        })
        .collect();
    list.sort_by(|a, b| b.date_published.cmp(&a.date_published));
    Ok(list)
}

/// 把 FTB 的版本清单写成一份最小 mrpack（只有 modrinth.index.json，没有 overrides），
/// 交给 `modpack::install_modpack` 去下载和建实例。
pub fn write_plan_zip(pack_id: &str, version_id: &str, dest: &Path) -> Result<String> {
    let id = parse_id(pack_id)?;
    let version_number: i64 = version_id
        .parse()
        .map_err(|_| CoreError::InvalidInput(format!("不是 FTB 版本：{version_id}")))?;
    let pack = pack_of(id)?;
    let value = get_json(&format!("{API}/public/modpack/{id}/{version_number}"))?;
    let version: VersionDetail = serde_json::from_value(value)
        .map_err(|error| CoreError::InvalidInput(format!("FTB 版本数据不对：{error}")))?;

    let mut files = Vec::new();
    for entry in &version.files {
        if entry.serveronly || entry.url.is_empty() {
            continue;
        }
        let folder = entry.path.trim_start_matches("./").trim_end_matches('/');
        let relative = if folder.is_empty() || folder == "." {
            entry.name.clone()
        } else {
            format!("{folder}/{}", entry.name)
        };
        if relative.split('/').any(|part| part == ".." || part == "") {
            continue;
        }
        let mut downloads = vec![entry.url.clone()];
        for mirror in &entry.mirrors {
            if !downloads.contains(mirror) {
                downloads.push(mirror.clone());
            }
        }
        let mut hashes = serde_json::Map::new();
        if !entry.sha1.is_empty() {
            hashes.insert("sha1".into(), json!(entry.sha1));
        }
        files.push(json!({
            "path": relative,
            "hashes": hashes,
            "env": { "client": "required", "server": "unsupported" },
            "downloads": downloads,
            "fileSize": entry.size,
        }));
    }
    if files.is_empty() {
        return Err(CoreError::InvalidInput(
            "这个版本没有可下载的文件".into(),
        ));
    }

    let mut dependencies = serde_json::Map::new();
    for target in &version.targets {
        match target.kind.as_str() {
            "game" => {
                dependencies.insert("minecraft".into(), json!(target.version));
            }
            "modloader" => {
                dependencies.insert(target.name.to_lowercase(), json!(target.version));
            }
            _ => {}
        }
    }

    let index = json!({
        "formatVersion": 1,
        "game": "minecraft",
        "versionId": version_id,
        "name": format!("{} {}", pack.name, version_id),
        "files": files,
        "dependencies": dependencies,
    });

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|error| CoreError::Io {
            path: parent.display().to_string(),
            source: error,
        })?;
    }
    let file = std::fs::File::create(dest).map_err(|error| CoreError::Io {
        path: dest.display().to_string(),
        source: error,
    })?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    use std::io::Write;
    zip.start_file("modrinth.index.json", options)
        .map_err(|error| CoreError::Archive(error.to_string()))?;
    zip.write_all(index.to_string().as_bytes())
        .map_err(|error| CoreError::Archive(error.to_string()))?;
    zip.finish()
        .map_err(|error| CoreError::Archive(error.to_string()))?;
    Ok(pack.name)
}

fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
