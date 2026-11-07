//! CurseForge 搜索与下载。
//!
//! 照 PCL 的做法：key 编译进程序（构建时由 `CURSEFORGE_KEY` 注入），用户从不填写。
//! key 缺失或对某个端点没权限时，自动退回社区镜像 `mod.mcimirror.top/curseforge`
//! （PCL 也用它给下载加速），界面层不需要知道差别。

use std::{
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use reqwest::blocking::Client;
use serde_json::Value;

use crate::{
    modrinth::MrDependency,
    modrinth::{ContentCategory, MrProject, MrVersion, SearchPage},
    CoreError, Result,
};

const API: &str = "https://api.curseforge.com/v1";
const MIRROR: &str = "https://mod.mcimirror.top/curseforge/v1";

/// CurseForge 的 API key：优先构建期注入（`CURSEFORGE_KEY`），
/// 其次是运行期 `set_key` 塞进来的（数据目录的 `curseforge.key`）。
/// 为空就走镜像，官方接口只在有 key 时尝试。
static KEY: OnceLock<String> = OnceLock::new();

/// 启动时把数据目录里的 key 读进来（用户从不手填，key 不进版本库）。
pub fn set_key(value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        let _ = KEY.set(value.to_owned());
    }
}

fn key() -> &'static str {
    KEY.get_or_init(|| {
        option_env!("CURSEFORGE_KEY")
            .map(str::to_owned)
            .or_else(|| std::env::var("CURSEFORGE_KEY").ok())
            .unwrap_or_default()
    })
}

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

/// 官方被拒后先走镜像一段时间；过了这段时间再试官方（新 key 有几分钟生效延迟，
/// 不能因为一次 403 就一直挂在镜像上）。
const MIRROR_TTL: Duration = Duration::from_secs(600);

fn preferred_mirror() -> bool {
    static HIT: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let slot = HIT.get_or_init(|| Mutex::new(None));
    let guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    match *guard {
        Some(at) => at.elapsed() < MIRROR_TTL,
        None => false,
    }
}

fn remember_mirror() {
    static HIT: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let slot = HIT.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(Instant::now());
    }
}

fn clear_mirror() {
    static HIT: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let slot = HIT.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = None;
    }
}

/// 依次尝试官方（带 key）与镜像，返回第一个成功的结果。
fn get(path: &str) -> Result<Value> {
    call(path, None)
}

/// `POST` 版本：整合包里常常有上百个文件，逐个 `GET` 既慢又容易吃到限流。
fn post(path: &str, body: &Value) -> Result<Value> {
    call(path, Some(body))
}

fn call(path: &str, body: Option<&Value>) -> Result<Value> {
    let official_ok = !key().is_empty();
    let order: Vec<&'static str> = if !official_ok {
        vec![MIRROR]
    } else if preferred_mirror() {
        vec![MIRROR, API]
    } else {
        vec![API, MIRROR]
    };
    let mut last: Option<CoreError> = None;
    for base in order {
        if base == API && !official_ok {
            continue;
        }
        let url = format!("{base}{path}");
        let mut request = match body {
            Some(body) => client()?
                .post(&url)
                .header("content-type", "application/json")
                .body(body.to_string()),
            None => client()?.get(&url),
        };
        if base == API {
            request = request.header("x-api-key", key());
        }
        let response = match request.send() {
            Ok(response) => response,
            Err(error) => {
                last = Some(CoreError::InvalidInput(format!(
                    "CurseForge 请求失败：{error}"
                )));
                continue;
            }
        };
        if !response.status().is_success() {
            // 官方明确拒绝（key 刚建好还没生效、或被临时限流）时先改用镜像，
            // 十分钟后再回头试官方。
            if base == API {
                remember_mirror();
            }
            last = Some(CoreError::InvalidInput(format!(
                "CurseForge 返回 {}：{url}",
                response.status()
            )));
            continue;
        }
        let body = response.text().map_err(|error| {
            CoreError::InvalidInput(format!("CurseForge 响应读取失败：{error}"))
        })?;
        let value: Value = serde_json::from_str(&body).map_err(|error| {
            CoreError::InvalidInput(format!("CurseForge 响应解析失败：{error}"))
        })?;
        if base == API {
            clear_mirror();
        }
        return Ok(value);
    }
    Err(last.unwrap_or_else(|| CoreError::InvalidInput("CurseForge 没有可用的接口".into())))
}

/// 按模组 id 批量取 `logo.thumbnailUrl`。
pub fn project_icons(mod_ids: &[u64]) -> Result<std::collections::HashMap<u64, String>> {
    use std::collections::HashMap;
    let mut out: HashMap<u64, String> = HashMap::new();
    if mod_ids.is_empty() {
        return Ok(out);
    }
    for chunk in mod_ids.chunks(100) {
        let body = serde_json::json!({ "modIds": chunk });
        let Ok(value) = post("/mods", &body) else {
            continue;
        };
        let Some(mods) = value["data"].as_array() else {
            continue;
        };
        for item in mods {
            let (Some(id), Some(icon)) = (
                item["id"].as_u64(),
                item["logo"]["thumbnailUrl"].as_str(),
            ) else {
                continue;
            };
            out.insert(id, icon.to_owned());
        }
    }
    Ok(out)
}

/// `/mods` 返回的项目元信息：名称、slug（用来查中文名表）与图标。
#[derive(Debug, Clone)]
pub struct CfProjectMeta {
    pub name: String,
    pub slug: String,
    pub icon: Option<String>,
}

/// 批量取项目元信息（每批 100 个）。
pub fn project_meta(mod_ids: &[u64]) -> Result<std::collections::HashMap<u64, CfProjectMeta>> {
    use std::collections::HashMap;
    let mut out: HashMap<u64, CfProjectMeta> = HashMap::new();
    if mod_ids.is_empty() {
        return Ok(out);
    }
    for chunk in mod_ids.chunks(100) {
        let body = serde_json::json!({ "modIds": chunk });
        let Ok(value) = post("/mods", &body) else {
            continue;
        };
        let Some(mods) = value["data"].as_array() else {
            continue;
        };
        for item in mods {
            let Some(id) = item["id"].as_u64() else {
                continue;
            };
            out.insert(
                id,
                CfProjectMeta {
                    name: item["name"].as_str().unwrap_or_default().to_owned(),
                    slug: item["slug"].as_str().unwrap_or_default().to_owned(),
                    icon: item["logo"]["thumbnailUrl"].as_str().map(str::to_owned),
                },
            );
        }
    }
    Ok(out)
}

/// 界面的内容分类 → CurseForge 的 classId。
fn class_id(kind: &str) -> Option<u32> {
    Some(match kind {
        "mods" | "mod" => 6,
        "modpacks" | "modpack" => 4471,
        "resourcepacks" | "resourcepack" => 12,
        "shaders" | "shader" => 6552,
        "worlds" | "world" => 17,
        "datapacks" | "datapack" => 6945,
        _ => return None,
    })
}

/// 排序字段：6=总下载、2=热度、11=发布日期、3=最近更新。
///
/// 官方文档里 4 是「相关性」，但镜像站点上 `sortField=4` 会忽略 `searchFilter`
/// 返回一堆不相干的项目（实测搜 `jei` 第一条是 `retroemi-forge`），所以「相关性」
/// 落到 2（热度）——结果至少和关键词相关。
fn sort_field(sort: &str) -> u32 {
    match sort {
        "downloads" => 6,
        "newest" => 11,
        "updated" => 3,
        _ => 2,
    }
}

/// 加载器 → modLoaderType（1 Forge、4 Fabric、5 Quilt、6 NeoForge）。
fn loader_type(loader: &str) -> Option<u32> {
    Some(match loader {
        "forge" => 1,
        "fabric" => 4,
        "quilt" => 5,
        "neoforge" => 6,
        _ => return None,
    })
}

fn number(value: &Value, key: &str) -> u64 {
    value[key].as_u64().unwrap_or(0)
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}

fn project_from(value: &Value, kind: &str) -> MrProject {
    let authors = value["authors"].as_array().cloned().unwrap_or_default();
    let author = authors
        .first()
        .map(|item| text(item, "name"))
        .unwrap_or_default();
    let categories = value["categories"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| text(item, "name"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let slug = text(value, "slug");
    // CF 的搜索命中没有单独的版本/加载器数组，只能从 latestFilesIndexes 里收集。
    let mut loaders: Vec<String> = Vec::new();
    let mut game_versions: Vec<String> = Vec::new();
    if let Some(indexes) = value["latestFilesIndexes"].as_array() {
        for entry in indexes {
            if let Some(loader) = loader_name(entry["modLoader"].as_u64().unwrap_or(0)) {
                if !loaders.iter().any(|item| item == loader) {
                    loaders.push(loader.to_owned());
                }
            }
            let game = text(entry, "gameVersion");
            if !game.is_empty() && !game_versions.contains(&game) {
                game_versions.push(game);
            }
        }
    }
    MrProject {
        id: number(value, "id").to_string(),
        chinese_title: crate::names::chinese_name(&slug).map(str::to_owned),
        slug,
        title: text(value, "name"),
        description: text(value, "summary"),
        icon_url: value["logo"]["thumbnailUrl"].as_str().map(str::to_owned),
        downloads: number(value, "downloadCount"),
        author,
        categories,
        project_type: kind.trim_end_matches('s').to_owned(),
        loaders,
        game_versions,
    }
}

/// CurseForge 的 `modLoader` 数字 → 加载器名（0 表示不限定）。
fn loader_name(value: u64) -> Option<&'static str> {
    match value {
        1 => Some("Forge"),
        3 => Some("LiteLoader"),
        4 => Some("Fabric"),
        5 => Some("Quilt"),
        6 => Some("NeoForge"),
        _ => None,
    }
}

/// 搜索项目，`kind` 与 Modrinth 那边保持同一套取值。
pub fn search(
    kind: &str,
    query: &str,
    game: Option<&str>,
    loader: Option<&str>,
    sort: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<MrProject>> {
    let class = class_id(kind)
        .ok_or_else(|| CoreError::InvalidInput(format!("CurseForge 不支持分类 {kind}")))?;
    let mut url = format!(
        "/mods/search?gameId=432&classId={class}&sortOrder=desc&pageSize={}&index={offset}&sortField={}",
        limit.clamp(1, 50),
        sort_field(sort)
    );
    if !query.trim().is_empty() {
        url.push_str(&format!("&searchFilter={}", urlencode(query)));
    }
    if let Some(game) = game.filter(|value| !value.is_empty()) {
        url.push_str(&format!("&gameVersion={}", urlencode(game)));
    }
    if let Some(loader) = loader.and_then(loader_type) {
        url.push_str(&format!("&modLoaderType={loader}"));
    }
    let value = get(&url)?;
    Ok(value["data"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| project_from(item, kind))
        .collect())
}

/// 分页搜索：带总数与 `categoryId` 筛选（PCL 的 `/v1/mods/search` 同款参数）。
pub fn search_page(
    kind: &str,
    query: &str,
    game: Option<&str>,
    loader: Option<&str>,
    sort: &str,
    category: Option<&str>,
    limit: u32,
    offset: u32,
) -> Result<SearchPage> {
    let class = class_id(kind)
        .ok_or_else(|| CoreError::InvalidInput(format!("CurseForge 不支持分类 {kind}")))?;
    let mut url = format!(
        "/mods/search?gameId=432&classId={class}&sortOrder=desc&pageSize={}&index={offset}&sortField={}",
        limit.clamp(1, 50),
        sort_field(sort)
    );
    if !query.trim().is_empty() {
        url.push_str(&format!("&searchFilter={}", urlencode(query)));
    }
    if let Some(game) = game.filter(|value| !value.is_empty()) {
        url.push_str(&format!("&gameVersion={}", urlencode(game)));
    }
    if let Some(loader) = loader.and_then(loader_type) {
        url.push_str(&format!("&modLoaderType={loader}"));
    }
    if let Some(category) = category.filter(|value| !value.is_empty()) {
        url.push_str(&format!("&categoryId={}", urlencode(category)));
    }
    let value = get(&url)?;
    let total = value["pagination"]["totalCount"].as_u64().unwrap_or(0);
    let items = value["data"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| project_from(item, kind))
        .collect();
    Ok(SearchPage { items, total })
}

/// CurseForge 的分类标签：`/v1/categories?gameId=432&classId=…` 的 `id` 就是搜索时的 `categoryId`。
pub fn categories(kind: &str) -> Result<Vec<ContentCategory>> {
    let class = class_id(kind)
        .ok_or_else(|| CoreError::InvalidInput(format!("CurseForge 不支持分类 {kind}")))?;
    let value = get(&format!("/categories?gameId=432&classId={class}"))?;
    Ok(value["data"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| ContentCategory {
            source: "curseforge".into(),
            kind: kind.to_owned(),
            value: number(item, "id").to_string(),
            name: text(item, "name"),
            icon: item["iconUrl"].as_str().map(str::to_owned),
        })
        .collect())
}

/// CurseForge 的文件对象 → 我们的版本结构。`downloadUrl` 常为空，按 PCL 的规则拼。
fn version_from(value: &Value) -> MrVersion {
    let id = number(value, "id");
    let file_name = text(value, "fileName");
    let file_url = match value["downloadUrl"].as_str().filter(|url| !url.is_empty()) {
        Some(url) => url.to_owned(),
        None => edge_url(id, &file_name),
    };
    // CurseForge 把「游戏版本 + 加载器 + 客户端/服务端」混在 `gameVersions` 里，按名字拆开。
    let mut game_versions: Vec<String> = Vec::new();
    let mut loaders: Vec<String> = Vec::new();
    let mut environments: Vec<String> = Vec::new();
    for item in value["gameVersions"].as_array().cloned().unwrap_or_default() {
        let Some(name) = item.as_str() else { continue };
        match name.to_ascii_lowercase().as_str() {
            "client" => environments.push("client".to_owned()),
            "server" => environments.push("server".to_owned()),
            "fabric" | "forge" | "neoforge" | "quilt" | "liteloader" | "optifine" => {
                let id = name.to_ascii_lowercase();
                if !loaders.contains(&id) {
                    loaders.push(id);
                }
            }
            _ => {
                if !game_versions.iter().any(|item| item == name) {
                    game_versions.push(name.to_owned());
                }
            }
        }
    }
    environments.dedup();
    MrVersion {
        id: id.to_string(),
        name: text(value, "displayName"),
        version_number: text(value, "displayName"),
        game_versions,
        loaders,
        environments,
        date_published: text(value, "fileDate"),
        downloads: number(value, "downloadCount"),
        version_type: match number(value, "releaseType") {
            2 => "beta".to_owned(),
            3 => "alpha".to_owned(),
            _ => "release".to_owned(),
        },
        file_name,
        file_url,
        file_size: number(value, "fileLength"),
        dependencies: value["dependencies"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|item| number(item, "relationType") == 3)
            .map(|item| MrDependency {
                version_id: None,
                project_id: Some(number(item, "modId").to_string()),
                file_name: None,
                dependency_type: "required".to_owned(),
            })
            .collect(),
    }
}

/// `https://edge.forgecdn.net/files/{id 前 4 位}/{id 第 5 位起}/{文件名}`。
fn edge_url(id: u64, file_name: &str) -> String {
    let digits = id.to_string();
    let (head, tail) = digits.split_at(digits.len().min(4));
    format!(
        "https://edge.forgecdn.net/files/{head}/{tail}/{}",
        urlencode(file_name)
    )
}

/// 单个文件的下载直链：整合包 manifest 里只有 projectID/fileID，没有文件名。
pub fn file_download_url(project_id: u64, file_id: u64) -> Result<String> {
    let value = get(&format!("/mods/{project_id}/files/{file_id}"))?;
    let file = &value["data"];
    if let Some(url) = file["downloadUrl"].as_str().filter(|url| !url.is_empty()) {
        return Ok(url.to_owned());
    }
    let file_name = text(file, "fileName");
    if file_name.is_empty() {
        return Err(CoreError::InvalidInput(format!(
            "CurseForge 文件 {file_id} 没有下载地址"
        )));
    }
    Ok(edge_url(file_id, &file_name))
}

/// 整合包清单里的一个文件：`POST /mods/files` 批量返回的详情。
#[derive(Debug, Clone, Default)]
pub struct PackFile {
    pub id: u64,
    /// 站点上的真实文件名，落盘要用它，不是 projectID。
    pub file_name: String,
    pub url: String,
    pub size: u64,
    /// CurseForge 的 `hashes[]` 里 `algo == 1` 是 SHA1。
    pub sha1: Option<String>,
    /// `modules[].name`：判断该落到 `mods/` 还是 `resourcepacks/` 等。
    pub modules: Vec<String>,
}

/// 一次最多接受 1000 个 ID，留点余量分批。
const FILE_ID_BATCH: usize = 500;

/// 批量取文件详情：整合包 manifest 里只有 projectID/fileID，没有文件名和直链。
///
/// 一次 POST 拿全部，比逐个 `GET /mods/{project}/files/{file}` 快一个数量级；
/// 作者删掉的文件会被接口跳过（返回数量少于请求），调用方按缺失处理。
pub fn files_by_ids(ids: &[u64]) -> Result<Vec<PackFile>> {
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(FILE_ID_BATCH) {
        let body = serde_json::json!({ "fileIds": chunk });
        let value = post("/mods/files", &body)?;
        let data = value["data"].as_array().cloned().unwrap_or_default();
        for item in data {
            out.push(pack_file(&item));
        }
    }
    Ok(out)
}

fn pack_file(value: &Value) -> PackFile {
    let id = number(value, "id");
    let file_name = text(value, "fileName");
    let url = value["downloadUrl"]
        .as_str()
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| edge_url(id, &file_name));
    let sha1 = value["hashes"]
        .as_array()
        .and_then(|items| {
            items.iter().find(|item| {
                item["algo"].as_u64() == Some(1)
                    || item["algo"]
                        .as_str()
                        .is_some_and(|algo| algo.eq_ignore_ascii_case("sha1"))
            })
        })
        .map(|item| text(item, "value"))
        .filter(|hash| !hash.is_empty());
    let modules = value["modules"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["name"].as_str().map(|name| name.to_ascii_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    PackFile {
        id,
        file_name,
        url,
        size: number(value, "fileLength"),
        sha1,
        modules,
    }
}

/// 项目的文件列表，新的排前面。
///
/// 一次取全（`pageSize=10000`，PCL 也是这么干的）；个别特别大的项目会拒掉这么大的
/// 页，那就退回 50 条一页，至少先把最近的文件显示出来。
pub fn versions(project: &str, game: Option<&str>, loader: Option<&str>) -> Result<Vec<MrVersion>> {
    match versions_page_size(project, game, loader, 10000) {
        Ok(list) => Ok(list),
        Err(_) => versions_page_size(project, game, loader, 50),
    }
}

fn versions_page_size(
    project: &str,
    game: Option<&str>,
    loader: Option<&str>,
    page_size: u32,
) -> Result<Vec<MrVersion>> {
    let mut url = format!("/mods/{}/files?pageSize={page_size}", urlencode(project));
    if let Some(game) = game.filter(|value| !value.is_empty()) {
        url.push_str(&format!("&gameVersion={}", urlencode(game)));
    }
    if let Some(loader) = loader.and_then(loader_type) {
        url.push_str(&format!("&modLoaderType={loader}"));
    }
    let value = get(&url)?;
    let mut list: Vec<MrVersion> = value["data"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|item| item["isAvailable"].as_bool().unwrap_or(true))
        .map(version_from)
        .collect();
    list.sort_by(|left, right| right.date_published.cmp(&left.date_published));
    Ok(list)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_ids_follow_the_ui_categories() {
        assert_eq!(class_id("mods"), Some(6));
        assert_eq!(class_id("modpacks"), Some(4471));
        assert_eq!(class_id("shaders"), Some(6552));
        assert_eq!(class_id("resourcepacks"), Some(12));
        assert_eq!(class_id("worlds"), Some(17));
        assert_eq!(class_id("datapacks"), Some(6945));
        assert_eq!(class_id("minecraft"), None);
    }

    #[test]
    fn edge_url_splits_the_file_id_like_the_website() {
        assert_eq!(
            edge_url(9374421, "jei-26.3-neoforge-31.17.0.103.jar"),
            "https://edge.forgecdn.net/files/9374/421/jei-26.3-neoforge-31.17.0.103.jar"
        );
    }

    #[test]
    fn sort_fields_map_to_the_documented_values() {
        assert_eq!(sort_field("downloads"), 6);
        assert_eq!(sort_field("follows"), 2);
        assert_eq!(sort_field("newest"), 11);
        assert_eq!(sort_field("updated"), 3);
        assert_eq!(sort_field("relevance"), 2);
    }
}

/// 批量按文件指纹反查下载地址（导出整合包用）。
/// 返回 `(fingerprint, downloadUrl)`。
pub fn fingerprints(hashes: &[u32]) -> Result<Vec<(u32, String)>> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for chunk in hashes.chunks(1000) {
        let body = serde_json::json!({ "fingerprints": chunk });
        let value = post("/fingerprints/432", &body)?;
        let matches = value["data"]["exactMatches"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for item in matches {
            let Some(file) = item.get("file") else {
                continue;
            };
            let Some(url) = file["downloadUrl"].as_str() else {
                continue;
            };
            if url.is_empty() {
                continue;
            }
            if let Some(fingerprint) = file["fileFingerprint"]
                .as_u64()
                .map(|value| value as u32)
            {
                out.push((fingerprint, url.to_owned()));
            }
        }
    }
    Ok(out)
}

/// 按文件指纹批量反查，带上 modId / fileId（判断有没有新版本用）。
/// 返回 `(fingerprint, modId, fileId)`。
pub fn fingerprint_details(hashes: &[u32]) -> Result<Vec<(u32, u64, u64)>> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for chunk in hashes.chunks(1000) {
        let body = serde_json::json!({ "fingerprints": chunk });
        let value = post("/fingerprints/432", &body)?;
        let matches = value["data"]["exactMatches"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for item in matches {
            let file = &item["file"];
            let Some(fingerprint) = file["fileFingerprint"].as_u64() else {
                continue;
            };
            // 官方接口把模块 id 放在 `modId`，社区镜像只给 `id`——两个都认。
            let Some(mod_id) = item["modId"].as_u64().or_else(|| item["id"].as_u64()) else {
                continue;
            };
            let Some(file_id) = file["id"].as_u64() else {
                continue;
            };
            out.push((fingerprint as u32, mod_id, file_id));
        }
    }
    Ok(out)
}

/// 项目返回的 `latestFilesIndexes` 里的一条：某个游戏版本 + 加载器对应的最新文件。
#[derive(Debug, Clone)]
pub struct LatestFileIndex {
    pub mod_id: u64,
    pub file_id: u64,
    pub game_version: String,
    pub mod_loader: String,
}

/// 批量取「每个项目在每个版本/加载器下的最新文件 id」。
///
/// 项目详情里自带 `latestFilesIndexes`，不用再去 `/mods/{id}/files` 拉全部文件——
/// 这正是 PCL 的取法，一个请求就能覆盖上百个项目。
pub fn latest_file_indexes(mod_ids: &[u64]) -> Result<Vec<LatestFileIndex>> {
    let mut out = Vec::new();
    for chunk in mod_ids.chunks(100) {
        let body = serde_json::json!({ "modIds": chunk, "filterPcOnly": false });
        let value = post("/mods", &body)?;
        let data = value["data"].as_array().cloned().unwrap_or_default();
        for item in data {
            let mod_id = number(&item, "id");
            let Some(indexes) = item["latestFilesIndexes"].as_array() else {
                continue;
            };
            for index in indexes {
                out.push(LatestFileIndex {
                    mod_id,
                    file_id: number(index, "fileId"),
                    game_version: text(index, "gameVersion"),
                    // 接口里 `modLoader` 是数字（1 Forge / 3 LiteLoader / 4 Fabric / 5 Quilt / 6 NeoForge），
                    // 少数情况给字符串；统一成小写名字，方便和实例的加载器比。
                    mod_loader: match index["modLoader"].as_u64() {
                        Some(number) => loader_name(number)
                            .map(|name| name.to_lowercase())
                            .unwrap_or_default(),
                        None => text(index, "modLoader").to_lowercase(),
                    },
                });
            }
        }
    }
    Ok(out)
}
