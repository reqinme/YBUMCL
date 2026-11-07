//! Modrinth 搜索与下载。
//!
//! 只用公开的 `/v2` 接口：搜索、项目版本列表、按文件直链下载。
//! Modrinth 要求带 User-Agent，且并发有配额，所以这里用一个共享的阻塞客户端。

use std::{collections::HashMap, fs, path::Path, sync::OnceLock, time::Duration};

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{io_error, CoreError, Result};

const API: &str = "https://api.modrinth.com/v2";

fn client() -> Result<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .user_agent("YBUMCL/0.1 (Minecraft launcher; https://github.com/ybumcl)")
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(40))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| CoreError::InvalidInput("无法创建 HTTP 客户端".into()))
}

/// 搜索、版本列表这类 GET 允许吃缓存：60 秒内直接用本地那份，
/// 之后带 ETag 去验证一次（304 就说明本地这份还是最新的）。
const FRESH: Duration = Duration::from_secs(120);

/// 版本列表几乎不动，缓存留久一点：重新打开同一个项目、重启启动器都不用再等。
const FRESH_VERSIONS: Duration = Duration::from_secs(600);

fn freshness(url: &str) -> Duration {
    if url.contains("/version") {
        FRESH_VERSIONS
    } else {
        FRESH
    }
}

fn parse(url: &str, body: &str) -> Result<serde_json::Value> {
    let _ = url;
    serde_json::from_str(body)
        .map_err(|error| CoreError::InvalidInput(format!("Modrinth 响应解析失败：{error}")))
}

fn get_json(url: &str) -> Result<serde_json::Value> {
    if let Some(raw) = crate::httpcache::fresh(url, freshness(url)) {
        if let Ok(text) = std::str::from_utf8(&raw) {
            if let Ok(value) = parse(url, text) {
                return Ok(value);
            }
        }
    }
    let mut request = client()?.get(url);
    if let Some(tag) = crate::httpcache::etag(url) {
        request = request.header("if-none-match", tag);
    }
    let response = request
        .send()
        .map_err(|error| CoreError::InvalidInput(format!("Modrinth 请求失败：{error}")))?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        if let Some(raw) = crate::httpcache::body(url) {
            if let Ok(text) = std::str::from_utf8(&raw) {
                if let Ok(value) = parse(url, text) {
                    crate::httpcache::refresh(url);
                    return Ok(value);
                }
            }
        }
    }
    if !response.status().is_success() {
        return Err(CoreError::InvalidInput(format!(
            "Modrinth 返回 {}：{url}",
            response.status()
        )));
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response
        .text()
        .map_err(|error| CoreError::InvalidInput(format!("Modrinth 响应读取失败：{error}")))?;
    let value = parse(url, &body)?;
    crate::httpcache::store(url, body.as_bytes(), etag.as_deref());
    Ok(value)
}

/// Modrinth 的搜索命中里没有 `loaders` 字段，加载器混在 `categories` 里。
fn loaders_of(hit: &Value) -> Vec<String> {
    const KNOWN: [&str; 5] = ["fabric", "forge", "neoforge", "quilt", "liteloader"];
    let mut list = str_list(&hit["loaders"]);
    for item in str_list(&hit["categories"]) {
        let lower = item.to_ascii_lowercase();
        if KNOWN.contains(&lower.as_str())
            && !list.iter().any(|had| had.eq_ignore_ascii_case(&lower))
        {
            list.push(lower);
        }
    }
    list
}

/// 搜索结果的条目（只保留界面要显示的字段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MrProject {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub author: String,
    /// Modrinth 的分类标签（加载器、用途等）。
    #[serde(default)]
    pub categories: Vec<String>,
    /// Modrinth 的 project_type（mod / resourcepack / shader / datapack / world / modpack）。
    #[serde(default)]
    pub project_type: String,
    /// 支持这款游戏版本的加载器（Fabric / NeoForge…），列表里用来算「支持版本」前缀。
    #[serde(default)]
    pub loaders: Vec<String>,
    /// 支持的游戏版本（未排序，界面自己压区间）。
    #[serde(default)]
    pub game_versions: Vec<String>,
    /// 内置名表里的中文名（查不到就是 None，界面回退到英文原名）。
    #[serde(default)]
    pub chinese_title: Option<String>,
}

/// 版本声明的一个前置：`dependency_type` 是 `required` / `optional` / `incompatible` / `embedded`。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MrDependency {
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub dependency_type: String,
}

/// 项目的一个版本（一个版本里可能有好几个文件，只取主文件）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MrVersion {
    pub id: String,
    pub name: String,
    pub version_number: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    /// 客户端/服务端：`client`、`server`（两个都有就是双端）。
    /// CurseForge 来自文件的 `gameVersions`，Modrinth 来自项目级的 `client_side`/`server_side`，
    /// 都没有就是空数组（界面据此决定要不要显示这一行）。
    #[serde(default)]
    pub environments: Vec<String>,
    pub date_published: String,
    pub downloads: u64,
    /// release / beta / alpha（CurseForge 的 releaseType 归一化到这里）。
    #[serde(default)]
    pub version_type: String,
    /// 主文件：直链 + 文件名 + 大小。
    pub file_name: String,
    pub file_url: String,
    pub file_size: u64,
    /// 这个版本自己声明的依赖（Modrinth 的 `dependencies`；CurseForge 由 `relationType` 归一化）。
    #[serde(default)]
    pub dependencies: Vec<MrDependency>,
}

/// 一页搜索结果：条目 + 总数（界面据此判断还要不要继续「加载更多」）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub items: Vec<MrProject>,
    pub total: u64,
}

/// 分类标签：两个来源各有自己的一套，值也完全不同。
/// `source` 是 "modrinth" / "curseforge"，`kind` 对应界面分类（mods / shaders …）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentCategory {
    pub source: String,
    pub kind: String,
    /// Modrinth 用名字（facet 的 `categories:<name>`），CurseForge 用数字 id。
    pub value: String,
    pub name: String,
    pub icon: Option<String>,
}

/// Modrinth 的 project_type 取值。
pub fn project_type(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "mods" | "mod" => "mod",
        "resourcepacks" | "resourcepack" => "resourcepack",
        "shaders" | "shader" => "shader",
        "datapacks" | "datapack" => "datapack",
        "worlds" | "world" => "world",
        "modpacks" | "modpack" => "modpack",
        _ => return None,
    })
}

fn facets_with(
    kind: &str,
    game: Option<&str>,
    loader: Option<&str>,
    category: Option<&str>,
    env: Option<&str>,
) -> String {
    let mut groups: Vec<String> = Vec::new();
    if let Some(kind) = project_type(kind) {
        groups.push(format!("[\"project_type:{kind}\"]"));
    }
    if let Some(game) = game.filter(|value| !value.is_empty()) {
        groups.push(format!("[\"versions:{game}\"]"));
    }
    if let Some(loader) = loader.filter(|value| !value.is_empty()) {
        groups.push(format!("[\"categories:{loader}\"]"));
    }
    if let Some(category) = category.filter(|value| !value.is_empty()) {
        groups.push(format!("[\"categories:{category}\"]"));
    }
    // 支持端：Modrinth 用 client_side / server_side 两个字段（required / optional / unsupported），
    // 「支持」= required 或 optional。
    match env {
        Some("client") => {
            groups.push("[\"client_side:required\",\"client_side:optional\"]".to_owned())
        }
        Some("server") => {
            groups.push("[\"server_side:required\",\"server_side:optional\"]".to_owned())
        }
        Some("both") => {
            groups.push("[\"client_side:required\",\"client_side:optional\"]".to_owned());
            groups.push("[\"server_side:required\",\"server_side:optional\"]".to_owned());
        }
        _ => {}
    }
    format!("[{}]", groups.join(","))
}

fn facets(kind: &str, game: Option<&str>, loader: Option<&str>) -> String {
    let mut groups: Vec<String> = Vec::new();
    if let Some(kind) = project_type(kind) {
        groups.push(format!("[\"project_type:{kind}\"]"));
    }
    if let Some(game) = game.filter(|value| !value.is_empty()) {
        groups.push(format!("[\"versions:{game}\"]"));
    }
    if let Some(loader) = loader.filter(|value| !value.is_empty()) {
        groups.push(format!("[\"categories:{loader}\"]"));
    }
    format!("[{}]", groups.join(","))
}

/// 搜索项目。`kind` 是界面分类（mods / resourcepacks / shaders / datapacks / worlds / modpacks）。
pub fn search(
    kind: &str,
    query: &str,
    game: Option<&str>,
    loader: Option<&str>,
    limit: u32,
    offset: u32,
) -> Result<Vec<MrProject>> {
    search_sorted(kind, query, game, loader, limit, offset, "relevance")
}

/// Modrinth 的排序名（界面统一用 PCL 那套：相关性 / 下载量 / 关注量 / 最新发布 / 最近更新）。
pub fn sort_name(sort: &str) -> &'static str {
    match sort {
        "downloads" => "downloads",
        "follows" => "follows",
        "newest" => "newest",
        "updated" => "updated",
        _ => "relevance",
    }
}

/// 分页搜索：带总数与分类筛选，供界面的「加载更多」使用。
pub fn search_page(
    kind: &str,
    query: &str,
    game: Option<&str>,
    loader: Option<&str>,
    sort: &str,
    category: Option<&str>,
    env: Option<&str>,
    limit: u32,
    offset: u32,
) -> Result<SearchPage> {
    let url = format!(
        "{API}/search?query={}&limit={}&offset={}&index={}&facets={}",
        urlencode(query),
        limit.clamp(1, 50),
        offset,
        sort_name(sort),
        urlencode(&facets_with(kind, game, loader, category, env))
    );
    let value = get_json(&url)?;
    let total = value["total_hits"].as_u64().unwrap_or(0);
    let items = projects(value["hits"].as_array().cloned().unwrap_or_default());
    Ok(SearchPage { items, total })
}

/// Modrinth 的分类标签（只取 `header == categories` 的那批）。
pub fn categories(kind: &str) -> Result<Vec<ContentCategory>> {
    let wanted = project_type(kind);
    let value = get_json(&format!("{API}/tag/category"))?;
    let list = value.as_array().cloned().unwrap_or_default();
    Ok(list
        .into_iter()
        .filter(|item| item["header"].as_str() == Some("categories"))
        .filter(|item| match (wanted, item["project_type"].as_str()) {
            (Some(wanted), Some(actual)) => actual == wanted,
            _ => false,
        })
        .map(|item| ContentCategory {
            source: "modrinth".into(),
            kind: kind.to_owned(),
            value: item["name"].as_str().unwrap_or_default().to_owned(),
            name: item["name"].as_str().unwrap_or_default().to_owned(),
            icon: item["icon"].as_str().map(str::to_owned),
        })
        .collect())
}

/// 把 hits 数组转成界面用的项目列表。
/// 把 `[{ "loaders": ["fabric"] }]` 这类字符串数组取成 `Vec<String>`。
pub fn str_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// 解析版本的 `dependencies` 数组（`project_id` / `version_id` / `dependency_type`）。
fn dependencies(value: &Value) -> Vec<MrDependency> {
    value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| MrDependency {
            version_id: item["version_id"].as_str().map(str::to_owned),
            project_id: item["project_id"].as_str().map(str::to_owned),
            file_name: item["file_name"].as_str().map(str::to_owned),
            dependency_type: item["dependency_type"]
                .as_str()
                .unwrap_or("required")
                .to_owned(),
        })
        .collect()
}

fn projects(hits: Vec<Value>) -> Vec<MrProject> {
    hits.into_iter()
        .map(|hit| MrProject {
            id: hit["project_id"]
                .as_str()
                .or_else(|| hit["id"].as_str())
                .unwrap_or_default()
                .to_owned(),
            slug: hit["slug"].as_str().unwrap_or_default().to_owned(),
            title: hit["title"]
                .as_str()
                .or_else(|| hit["name"].as_str())
                .unwrap_or_default()
                .to_owned(),
            description: hit["description"]
                .as_str()
                .or_else(|| hit["summary"].as_str())
                .unwrap_or_default()
                .to_owned(),
            icon_url: hit["icon_url"]
                .as_str()
                .or_else(|| hit["logo"]["thumbnailUrl"].as_str())
                .map(str::to_owned),
            downloads: hit["downloads"]
                .as_u64()
                .or_else(|| hit["downloadCount"].as_u64())
                .unwrap_or(0),
            author: hit["author"]
                .as_str()
                .or_else(|| hit["authors"][0]["name"].as_str())
                .unwrap_or_default()
                .to_owned(),
            categories: hit["categories"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            project_type: hit["project_type"].as_str().unwrap_or_default().to_owned(),
            chinese_title: crate::names::chinese_name(hit["slug"].as_str().unwrap_or_default())
                .map(str::to_owned),
            loaders: loaders_of(&hit),
            game_versions: str_list(&hit["versions"]),
        })
        .collect()
}

/// 集市首页用：按 `sort`（`downloads` / `newest` / `updated` / `relevance`）拿一批项目。
pub fn browse(kind: &str, sort: &str, game: Option<&str>, limit: u32) -> Result<Vec<MrProject>> {
    let index = match sort {
        "downloads" | "newest" | "updated" | "relevance" => sort,
        _ => "relevance",
    };
    search_sorted(kind, "", game, None, limit, 0, index)
}

fn search_sorted(
    kind: &str,
    query: &str,
    game: Option<&str>,
    loader: Option<&str>,
    limit: u32,
    offset: u32,
    index: &str,
) -> Result<Vec<MrProject>> {
    let url = format!(
        "{API}/search?query={}&limit={}&offset={}&index={index}&facets={}",
        urlencode(query),
        limit.clamp(1, 50),
        offset,
        urlencode(&facets(kind, game, loader))
    );
    let value = get_json(&url)?;
    let hits = value["hits"].as_array().cloned().unwrap_or_default();
    Ok(hits
        .into_iter()
        .map(|hit| MrProject {
            id: hit["project_id"].as_str().unwrap_or_default().to_owned(),
            slug: hit["slug"].as_str().unwrap_or_default().to_owned(),
            title: hit["title"].as_str().unwrap_or_default().to_owned(),
            description: hit["description"].as_str().unwrap_or_default().to_owned(),
            icon_url: hit["icon_url"].as_str().map(str::to_owned),
            downloads: hit["downloads"].as_u64().unwrap_or(0),
            author: hit["author"].as_str().unwrap_or_default().to_owned(),
            categories: hit["categories"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            project_type: hit["project_type"].as_str().unwrap_or_default().to_owned(),
            loaders: loaders_of(&hit),
            game_versions: str_list(&hit["versions"]),
            chinese_title: crate::names::chinese_name(hit["slug"].as_str().unwrap_or_default())
                .map(str::to_owned),
        })
        .collect())
}

/// 项目在指定游戏版本/加载器下的版本列表，新的排前面。
pub fn versions(project: &str, game: Option<&str>, loader: Option<&str>) -> Result<Vec<MrVersion>> {
    let mut url = format!("{API}/project/{}/version", urlencode(project));
    let mut params: Vec<String> = Vec::new();
    if let Some(game) = game.filter(|value| !value.is_empty()) {
        params.push(format!(
            "game_versions={}",
            urlencode(&format!("[\"{game}\"]"))
        ));
    }
    if let Some(loader) = loader.filter(|value| !value.is_empty()) {
        params.push(format!("loaders={}", urlencode(&format!("[\"{loader}\"]"))));
    }
    if !params.is_empty() {
        url.push('?');
        url.push_str(&params.join("&"));
    }
    let value = get_json(&url)?;
    let mut list: Vec<MrVersion> = value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let files = item["files"].as_array()?;
            let file = files
                .iter()
                .find(|file| file["primary"].as_bool().unwrap_or(false))
                .or_else(|| files.first())?;
            let strings = |value: &serde_json::Value| -> Vec<String> {
                value
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            Some(MrVersion {
                id: item["id"].as_str().unwrap_or_default().to_owned(),
                name: item["name"].as_str().unwrap_or_default().to_owned(),
                version_number: item["version_number"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                game_versions: strings(&item["game_versions"]),
                loaders: strings(&item["loaders"]),
                environments: Vec::new(),
                date_published: item["date_published"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                downloads: item["downloads"].as_u64().unwrap_or(0),
                version_type: item["version_type"]
                    .as_str()
                    .unwrap_or("release")
                    .to_owned(),
                file_name: file["filename"].as_str().unwrap_or_default().to_owned(),
                file_url: file["url"].as_str().unwrap_or_default().to_owned(),
                file_size: file["size"].as_u64().unwrap_or(0),
                dependencies: dependencies(&item["dependencies"]),
            })
        })
        .collect();
    list.sort_by(|left, right| right.date_published.cmp(&left.date_published));
    // 版本接口不带客户端/服务端信息，取项目级的 `client_side` / `server_side`。
    let environments = project_environments(project);
    if !environments.is_empty() {
        for version in &mut list {
            version.environments = environments.clone();
        }
    }
    Ok(list)
}

/// 项目级的支持端：`required` / `optional` 算支持，`unsupported` 不算。
fn project_environments(project: &str) -> Vec<String> {
    let Ok(value) = get_json(&format!("{API}/project/{}", urlencode(project))) else {
        return Vec::new();
    };
    let supports = |side: &str| {
        matches!(
            value[side].as_str(),
            Some("required") | Some("optional")
        )
    };
    let mut list: Vec<String> = Vec::new();
    if supports("client_side") {
        list.push("client".to_owned());
    }
    if supports("server_side") {
        list.push("server".to_owned());
    }
    list
}

/// 用「版本 id」反查它属于哪个项目，返回 `(项目, 版本号)`。
///
/// 本地导入的 `.mrpack` 里只有 `versionId`，没有项目 id；装上之后要显示「哪个整合包、
/// 什么版本」就只能这样反查一次。查不到（离线、被删）返回 `None`，安装照常算成功。
pub fn version_owner(version_id: &str) -> Option<(MrProject, String)> {
    let version = get_json(&format!("{API}/version/{}", urlencode(version_id))).ok()?;
    let project_id = version["project_id"].as_str()?.to_owned();
    let version_number = version["version_number"].as_str().unwrap_or_default().to_owned();
    let value = get_json(&format!("{API}/project/{}", urlencode(&project_id))).ok()?;
    let project = projects(vec![value]).pop()?;
    Some((project, version_number))
}

/// 把文件下载到 `dir`，文件名沿用 Modrinth 给的（重名就覆盖），返回路径。
pub fn download(url: &str, dir: &Path, file_name: &str) -> Result<String> {
    fs::create_dir_all(dir).map_err(|error| io_error(dir, error))?;
    let safe: String = file_name
        .chars()
        .map(|ch| if "<>:\"/\\|?*".contains(ch) { '_' } else { ch })
        .collect();
    let name = if safe.trim().is_empty() {
        "download.bin".to_owned()
    } else {
        safe
    };
    let target = dir.join(name);
    let response = client()?
        .get(url)
        .send()
        .map_err(|error| CoreError::InvalidInput(format!("下载失败：{error}")))?;
    if !response.status().is_success() {
        return Err(CoreError::InvalidInput(format!(
            "下载失败：服务器返回 {}",
            response.status()
        )));
    }
    let bytes = response
        .bytes()
        .map_err(|error| CoreError::InvalidInput(format!("读取下载内容失败：{error}")))?;
    fs::write(&target, &bytes).map_err(|error| io_error(&target, error))?;
    Ok(target.to_string_lossy().into_owned())
}

fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 批量按 sha1 反查文件（导出整合包用）。
/// 返回 `(sha1, 第一个下载地址)`，认不出来的 hash 不会出现在结果里。
pub fn files_by_sha1(hashes: &[String]) -> Result<Vec<(String, String)>> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let body = serde_json::json!({ "hashes": hashes, "algorithm": "sha1" });
    let mut out = Vec::new();
    for chunk in hashes.chunks(1000) {
        let chunk_body = serde_json::json!({ "hashes": chunk, "algorithm": "sha1" });
        let _ = &body;
        let response = client()?
            .post("https://api.modrinth.com/v2/version_files")
            .header("content-type", "application/json")
            .body(chunk_body.to_string())
            .send()
            .map_err(|error| CoreError::InvalidInput(format!("Modrinth 请求失败：{error}")))?;
        if !response.status().is_success() {
            return Err(CoreError::InvalidInput(format!(
                "Modrinth 返回 {}",
                response.status()
            )));
        }
        let text = response
            .text()
            .map_err(|error| CoreError::InvalidInput(format!("Modrinth 响应读取失败：{error}")))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| CoreError::InvalidInput(format!("Modrinth 响应解析失败：{error}")))?;
        if let Some(map) = value.as_object() {
            for (sha1, version) in map {
                // 只有返回文件的 sha1 跟我们要的对得上才用，别认错文件。
                let matches = version["files"][0]["hashes"]["sha1"]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(sha1));
                if !matches {
                    continue;
                }
                if let Some(url) = version["files"][0]["url"].as_str() {
                    out.push((sha1.clone(), url.to_owned()));
                }
            }
        }
    }
    Ok(out)
}

/// 按 sha1 批量反查文件，返回足以判断「有没有新版本」的字段。
pub struct MrFileHit {
    pub sha1: String,
    pub project_id: String,
    pub version_id: String,
    pub version_number: String,
    pub file_name: String,
    pub file_url: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
}

/// 批量按 sha1 反查（比 files_by_sha1 多带 project_id / version_id）。
pub fn lookup_sha1(hashes: &[String]) -> Result<Vec<MrFileHit>> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for chunk in hashes.chunks(1000) {
        let body = serde_json::json!({ "hashes": chunk, "algorithm": "sha1" });
        let response = client()?
            .post("https://api.modrinth.com/v2/version_files")
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .map_err(|error| CoreError::InvalidInput(format!("Modrinth 请求失败：{error}")))?;
        if !response.status().is_success() {
            return Err(CoreError::InvalidInput(format!(
                "Modrinth 返回 {}",
                response.status()
            )));
        }
        let text = response
            .text()
            .map_err(|error| CoreError::InvalidInput(format!("Modrinth 响应读取失败：{error}")))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| CoreError::InvalidInput(format!("Modrinth 响应解析失败：{error}")))?;
        let Some(map) = value.as_object() else {
            continue;
        };
        for (sha1, version) in map {
            let matches = version["files"][0]["hashes"]["sha1"]
                .as_str()
                .is_some_and(|value| value.eq_ignore_ascii_case(sha1));
            if !matches {
                continue;
            }
            let Some(project) = version["project_id"].as_str() else {
                continue;
            };
            out.push(MrFileHit {
                sha1: sha1.clone(),
                project_id: project.to_owned(),
                version_id: text_of(version, "id"),
                version_number: text_of(version, "version_number"),
                file_name: text_of(&version["files"][0], "filename"),
                file_url: text_of(&version["files"][0], "url"),
                game_versions: str_list(&version["game_versions"]),
                loaders: str_list(&version["loaders"]),
            });
        }
    }
    Ok(out)
}

fn text_of(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}

/// 按项目 id 批量取 `icon_url`，给「jar 里没有图标」的模组用项目图标。
pub fn project_icon_urls(ids: &[String]) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    for chunk in ids.chunks(100) {
        let quoted: Vec<String> = chunk.iter().map(|id| format!("\"{id}\"")).collect();
        let url = format!("{API}/projects?ids=[{}]", quoted.join(","));
        let Ok(projects) = get_json(&url) else { continue };
        let Some(list) = projects.as_array() else { continue };
        for project in list {
            let id = project.get("id").and_then(|v| v.as_str());
            let icon = project.get("icon_url").and_then(|v| v.as_str());
            if let (Some(id), Some(icon)) = (id, icon) {
                out.insert(id.to_owned(), icon.to_owned());
            }
        }
    }
    Ok(out)
}

/// `/v2/projects` 返回的项目元信息：图标、原始名与 slug（slug 用来查中文名表）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMeta {
    pub title: String,
    pub slug: String,
    pub icon: Option<String>,
}

/// 批量取项目元信息（每批 100 个），查不到的不会出现在结果里。
pub fn project_meta(ids: &[String]) -> Result<HashMap<String, ProjectMeta>> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    for chunk in ids.chunks(100) {
        let quoted: Vec<String> = chunk.iter().map(|id| format!("\"{id}\"")).collect();
        let url = format!("{API}/projects?ids=[{}]", quoted.join(","));
        let Ok(projects) = get_json(&url) else { continue };
        let Some(list) = projects.as_array() else { continue };
        for project in list {
            let Some(id) = project.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            out.insert(
                id.to_owned(),
                ProjectMeta {
                    title: project
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    slug: project
                        .get("slug")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    icon: project
                        .get("icon_url")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                },
            );
        }
    }
    Ok(out)
}

/// `POST /v2/version_files/update` 的一条：某个本地文件在该版本/加载器下应升级到的目标文件。
#[derive(Debug, Clone)]
pub struct MrUpdateHit {
    pub sha1: String,
    pub project_id: String,
    pub version_number: String,
    pub file_name: String,
    pub file_url: String,
    /// 目标文件的 sha1：跟本地相同就说明已经是最新。
    pub file_sha1: String,
}

/// 一次问清「这批文件在该版本 + 加载器下各自应升级到哪个文件」。
///
/// 这是 Modrinth 专门给启动器用的批量升级接口：不像 `versions(project)` 那样逐项目查，
/// 几百个模组也只要一次请求（PCL 用的就是它）。
pub fn update_version_files(
    hashes: &[String],
    game: Option<&str>,
    loader: Option<&str>,
) -> Result<Vec<MrUpdateHit>> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let mut body = serde_json::json!({ "hashes": hashes, "algorithm": "sha1" });
    if let Some(game) = game {
        body["game_versions"] = serde_json::json!([game]);
    }
    if let Some(loader) = loader {
        body["loaders"] = serde_json::json!([loader.to_lowercase()]);
    }
    let mut out = Vec::new();
    for chunk in hashes.chunks(1000) {
        let mut chunk_body = body.clone();
        chunk_body["hashes"] = serde_json::json!(chunk);
        let response = client()?
            .post(format!("{API}/version_files/update"))
            .header("content-type", "application/json")
            .body(chunk_body.to_string())
            .send()
            .map_err(|error| CoreError::InvalidInput(format!("Modrinth 请求失败：{error}")))?;
        if !response.status().is_success() {
            return Err(CoreError::InvalidInput(format!(
                "Modrinth 返回 {}",
                response.status()
            )));
        }
        let text = response
            .text()
            .map_err(|error| CoreError::InvalidInput(format!("Modrinth 响应读取失败：{error}")))?;
        let value: Value = parse("update", &text)?;
        let Some(map) = value.as_object() else {
            continue;
        };
        for (sha1, version) in map {
            out.push(MrUpdateHit {
                sha1: sha1.clone(),
                project_id: text_of(version, "project_id"),
                version_number: text_of(version, "version_number"),
                file_name: text_of(&version["files"][0], "filename"),
                file_url: text_of(&version["files"][0], "url"),
                file_sha1: text_of(&version["files"][0]["hashes"], "sha1").to_lowercase(),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dependency_entries() {
        let value = serde_json::json!([
            { "version_id": null, "project_id": "P7dR8mSH", "file_name": null, "dependency_type": "required" },
            { "version_id": "abc", "project_id": null, "file_name": "x.jar", "dependency_type": "optional" }
        ]);
        let out = dependencies(&value);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].project_id.as_deref(), Some("P7dR8mSH"));
        assert_eq!(out[0].dependency_type, "required");
        assert_eq!(out[1].file_name.as_deref(), Some("x.jar"));
        assert_eq!(out[1].dependency_type, "optional");
    }
}
