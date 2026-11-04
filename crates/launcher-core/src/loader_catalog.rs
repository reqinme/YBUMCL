//! 六个加载器的「版本号 ↔ Minecraft 版本」映射与版本清单来源。
//!
//! 每个加载器的版本号都跟 Minecraft 版本有一种别扭的对应关系：Forge 的构建号要配 MC 版本使用，
//! NeoForge 把 `1.` 去掉了（1.21.1 → 21.1.x），LiteLoader 的清单里 1.8 要补成 1.8.0，
//! OptiFine 的名字第一段才是 MC 版本。这些规则集中在这里，调用方只问「给我这个 MC 版本的
//! 构建列表」，不再各自判断。

use crate::download::{fetch_catalog_bytes, CancellationToken, DownloadError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

type Result<T> = std::result::Result<T, DownloadError>;

const FORGE_HTML: &str = "https://files.minecraftforge.net/maven/net/minecraftforge/forge";
const BMCL_FORGE: &str = "https://bmclapi2.bangbang93.com/forge/minecraft";
const NEO_VERSIONS: &str =
    "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";
const NEO_LEGACY_VERSIONS: &str =
    "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/forge";
const BMCL_NEO: &str =
    "https://bmclapi2.bangbang93.com/neoforge/meta/api/maven/details/releases/net/neoforged";
const CLEANROOM_RELEASES: &str =
    "https://api.github.com/repos/CleanroomMC/Cleanroom/releases?per_page=100";
/// Cleanroom 的 maven 元数据：GitHub API 在部分网络下不可达，这条国内可直连。
const CLEANROOM_METADATA: &str =
    "https://nexus.jsdu.cn/repository/com/cleanroommc/cleanroom/maven-metadata.xml";
const LITELOADER_JSON: &str = "https://dl.liteloader.com/versions/versions.json";
const BMCL_LITELOADER_JSON: &str =
    "https://bmclapi2.bangbang93.com/maven/com/mumfrey/liteloader/versions.json";
const BMCL_OPTIFINE: &str = "https://bmclapi2.bangbang93.com/optifine/versionList";
/// 下载地址和清单地址不同：清单多一层 `versionList`，jar 是 `/optifine/<mc>/<类型>/<尾段>`。
const BMCL_OPTIFINE_DOWNLOAD: &str = "https://bmclapi2.bangbang93.com/optifine";

/// Forge 的 1.8.9 分支历史上用这几个固定构建号发过包。
const FORGE_LEGACY_BUILDS: [&str; 3] = ["11.15.1.2318", "11.15.1.1902", "11.15.1.1890"];

/// 一个加载器版本，连同它所属的 Minecraft 版本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderEntry {
    pub version: String,
    pub game: String,
    #[serde(default)]
    pub stable: bool,
    /// 只有 OptiFine 会填：这个版本要求同版本的这个 Forge 构建。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forge: Option<String>,
}

impl LoaderEntry {
    fn new(version: impl Into<String>, game: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            game: game.into(),
            stable: true,
            forge: None,
        }
    }
}

fn sort_key(version: &str) -> (Vec<u32>, bool, String) {
    let mut split = version.splitn(2, '-');
    let numbers = split
        .next()
        .unwrap_or("")
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0))
        .collect::<Vec<_>>();
    let suffix = split.next().unwrap_or("").to_owned();
    (numbers, suffix.is_empty(), suffix)
}

fn newest_first(mut entries: Vec<LoaderEntry>) -> Vec<LoaderEntry> {
    entries.sort_by_cached_key(|entry| sort_key(&entry.version));
    entries.dedup_by(|a, b| a.version == b.version);
    entries.reverse();
    entries
}

fn numeric(version: &str) -> Vec<u32> {
    version
        .split(['.', '-', '+', ' '])
        .take_while(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// NeoForge 的版本号怎么还原成 Minecraft 版本（规则沿用 PCL 的 `DlNeoForgeListEntry`）：
///
/// - legacy 线 `1.20.1-47.1.99` → `1.20.1`
/// - 快照线 `0.25w14craftmine.3-beta` → `25w14craftmine`
/// - `20.4.30-beta` / `21.1.265` → `1.20.4` / `1.21.1`，minor 是 0 时不带 `.0`（`21.0.167` → `1.21`）
/// - 24 起版本号本身就等于 MC 版本：`26.1.0.0-alpha.1+snapshot-1` → `26.1-snapshot-1`
pub fn neoforge_game(version: &str) -> Option<String> {
    let (bare, suffix) = match version.split_once('+') {
        Some((bare, suffix)) => (bare, Some(suffix)),
        None => (version, None),
    };
    let base = bare.split('-').next()?.trim();
    let mut segments = base.split('.');
    let major: u32 = segments.next()?.parse().ok()?;
    match major {
        // legacy：1.20.1-47.1.99
        1 => {
            let minor: u32 = segments.next()?.parse().ok()?;
            let patch: u32 = segments.next()?.parse().ok()?;
            Some(format!("1.{minor}.{patch}"))
        }
        // 快照线：0.25w14craftmine.3-beta
        0 => Some(segments.next()?.to_owned()),
        // 20–23：NeoForge 把 MC 的 `1.` 去掉了，minor 为 0 时不带 `.0`
        20..=23 => {
            let minor: u32 = segments.next()?.parse().ok()?;
            Some(if minor > 0 {
                format!("1.{major}.{minor}")
            } else {
                format!("1.{major}")
            })
        }
        // 24 起版本号本身就是 MC 版本号
        24.. => {
            let minor: u32 = segments.next()?.parse().ok()?;
            let build: u32 = segments
                .next()
                .and_then(|build| build.parse().ok())
                .unwrap_or(0);
            let mut game = if build > 0 {
                format!("{major}.{minor}.{build}")
            } else {
                format!("{major}.{minor}")
            };
            if let Some(suffix) = suffix {
                game.push('-');
                game.push_str(suffix);
            }
            Some(game)
        }
        _ => None,
    }
}

/// NeoForge 1.20.1 走的是 legacy 坐标：`net/neoforged/forge/1.20.1-47.x/forge-1.20.1-47.x-installer.jar`，
/// 而不是 `net/neoforged/neoforge/...`。
pub fn neoforge_legacy_game(game: &str) -> bool {
    game == "1.20.1"
}

/// NeoForge 没有 Forge 那种 `promotions_slim.json`，界面默认取「最新稳定版」。
pub fn prerelease(version: &str) -> bool {
    let lower = version.to_ascii_lowercase();
    lower.contains("beta") || lower.contains("alpha") || lower.contains("snapshot")
}

/// NeoForge 版本的 Java 下限（21.1 起要 21，1.20.x 需要 17）。
pub fn neoforge_java(version: &str) -> u32 {
    let parts = numeric(version);
    match parts.first().copied().unwrap_or(0) {
        0..=19 => 17,
        _ => 21,
    }
}

/// Cleanroom 只在 1.12.2 上工作，版本号取 `cleanroom-<tag>-installer.jar` 里的完整 tag
/// （`0.7.14-alpha` 这种带后缀的必须整段保留，release 的下载路径就是用 tag 拼的）。
pub fn cleanroom_api(name: &str) -> Option<String> {
    let rest = name.strip_prefix("cleanroom-")?;
    let tag = rest.strip_suffix("-installer.jar")?;
    if numeric(tag).is_empty() {
        return None;
    }
    Some(tag.to_owned())
}

/// LiteLoader 的清单里 1.8 / 1.9 用短号，实际目录名是 1.8.0 / 1.9.0。
pub fn liteloader_game(name: &str) -> Option<String> {
    let base = name.split('_').next()?.to_owned();
    let parts = numeric(&base);
    match parts.as_slice() {
        [1, 8] | [1, 9] => Some(format!("{base}.0")),
        [1, _, _] => Some(base),
        _ => None,
    }
}

pub fn liteloader_supported(game: &str) -> bool {
    let parts = numeric(game);
    let minor = parts.get(1).copied().unwrap_or(0);
    parts.len() >= 3 && minor != 5 && minor != 6
}

/// OptiFine 的版本名形如 `1.12.2_HD_U_G5`，第一段是 MC 版本。
pub fn optifine_game(name: &str) -> Option<String> {
    let base = name.split('_').next()?;
    if numeric(base).len() >= 2 {
        Some(base.to_owned())
    } else {
        None
    }
}

/// OptiFine 的下载直链（BMCLAPI）。目录是 `<mc>/<类型>/<尾段>`：
/// 稳定版 `HD_U_I6` → `.../1.20.1/HD_U/I6`，预览版 `HD_U_I6_pre6` → `.../1.20.1/HD_U_I6/pre6`；
/// 1.8/1.9 的目录名还要补一个 `.0`。
pub fn optifine_download(game: &str, version: &str) -> Option<String> {
    if numeric(game).is_empty() {
        return None;
    }
    let (kind, patch) = version.rsplit_once('_')?;
    if kind.is_empty() || patch.is_empty() {
        return None;
    }
    let game = match game {
        "1.8" | "1.9" => format!("{game}.0"),
        other => other.to_owned(),
    };
    Some(format!("{BMCL_OPTIFINE_DOWNLOAD}/{game}/{kind}/{patch}"))
}

/// 各加载器安装包的官方下载地址。
pub fn installer_url(filter: LoaderFilter, game: &str, version: &str) -> Option<String> {
    match filter {
        LoaderFilter::Cleanroom => Some(format!(
            "https://github.com/CleanroomMC/Cleanroom/releases/download/{version}/cleanroom-{version}-installer.jar"
        )),
        // LiteLoader 只发快照：目录和文件名都带 `-SNAPSHOT`。
        LoaderFilter::LiteLoader => Some(format!(
            "https://bmclapi2.bangbang93.com/maven/com/mumfrey/liteloader/{game}-SNAPSHOT/liteloader-{game}-SNAPSHOT.jar"
        )),
        LoaderFilter::OptiFine => optifine_download(game, version),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoaderFilter {
    Forge,
    NeoForge,
    Cleanroom,
    LiteLoader,
    OptiFine,
}

impl LoaderFilter {
    pub fn parse(loader: &str) -> Option<Self> {
        match loader {
            "forge" => Some(Self::Forge),
            "neoforge" => Some(Self::NeoForge),
            "cleanroom" => Some(Self::Cleanroom),
            "liteloader" => Some(Self::LiteLoader),
            "optifine" => Some(Self::OptiFine),
            _ => None,
        }
    }

    fn slot(self) -> u8 {
        match self {
            Self::Forge => 0,
            Self::NeoForge => 1,
            Self::Cleanroom => 2,
            Self::LiteLoader => 3,
            Self::OptiFine => 4,
        }
    }
}

fn parse_json(bytes: &[u8]) -> Option<serde_json::Value> {
    serde_json::from_slice(bytes).ok()
}

/// BMCLAPI / NeoForge 的「目录清单」：`{"files":[{"name":"…","type":"DIRECTORY"}]}`。
fn directory_names(bytes: &[u8]) -> Vec<String> {
    let Some(value) = parse_json(bytes) else {
        return Vec::new();
    };
    value["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|file| file["type"].as_str() == Some("DIRECTORY"))
        .filter_map(|file| file["name"].as_str().map(str::to_owned))
        .collect()
}

fn forge_entries(game: &str, bytes: &[u8]) -> Vec<LoaderEntry> {
    if let Some(value) = parse_json(bytes) {
        if let Some(items) = value.as_array() {
            return items
                .iter()
                .filter_map(|item| item["version"].as_str())
                .map(|version| LoaderEntry::new(version, game))
                .collect();
        }
    }
    // 官方 HTML 目录：只认 forge-<mc>-<build>-installer.jar 这类链接。
    let text = String::from_utf8_lossy(bytes);
    let regex = regex::Regex::new(r"forge-(\d[0-9.]*)-(\d[0-9.]*[\w.]*)-installer\.jar")
        .expect("constant regex");
    let mut found = BTreeSet::new();
    for capture in regex.captures_iter(&text) {
        if capture[1].trim_end_matches('.') == game {
            found.insert(capture[2].to_owned());
        }
    }
    found
        .into_iter()
        .map(|version| LoaderEntry::new(version, game))
        .collect()
}

fn neoforge_entries(game: &str, bytes: &[u8]) -> Vec<LoaderEntry> {
    let Some(value) = parse_json(bytes) else {
        return Vec::new();
    };
    let mut names: Vec<String> = directory_names(bytes);
    for key in ["versions", "files"] {
        if let Some(items) = value[key].as_array() {
            for item in items {
                if let Some(name) = item.as_str() {
                    names.push(name.to_owned());
                } else if let Some(name) = item["name"].as_str() {
                    names.push(name.to_owned());
                } else if let Some(name) = item["version"].as_str() {
                    names.push(name.to_owned());
                }
            }
        }
    }
    names
        .into_iter()
        .filter_map(|name| {
            let version = name.rsplit('/').next().unwrap_or(&name).to_owned();
            let game_found = neoforge_game(&version)?;
            if game_found != game {
                return None;
            }
            Some(LoaderEntry::new(version, game))
        })
        .collect()
}

fn cleanroom_entries(game: &str, bytes: &[u8]) -> Vec<LoaderEntry> {
    if game != "1.12.2" {
        return Vec::new();
    }
    if bytes.starts_with(b"<?xml") || bytes.windows(8).any(|window| window == b"<version") {
        return xml_tags(bytes, "version")
            .into_iter()
            .map(|version| {
                let mut entry = LoaderEntry::new(version, game);
                // Cleanroom 的构建多是 `0.7.14-alpha` 这类预发布，只有正式名（`0.4.0`）才算稳定版。
                entry.stable = !prerelease(&entry.version);
                entry
            })
            .collect();
    }
    let Some(value) = parse_json(bytes) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for release in value.as_array().into_iter().flatten() {
        for asset in release["assets"].as_array().into_iter().flatten() {
            let Some(name) = asset["name"].as_str() else {
                continue;
            };
            if !name.ends_with("-installer.jar") {
                continue;
            }
            if let Some(api) = cleanroom_api(name) {
                let mut entry = LoaderEntry::new(api, game);
                entry.stable = !prerelease(&entry.version);
                entries.push(entry);
            }
        }
    }
    entries
}

/// 取出 XML 里某个标签的全部内容（maven 元数据的版本列表够用，不引入 xml 依赖）。
fn xml_tags(bytes: &[u8], tag: &str) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = text.as_ref();
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else {
            break;
        };
        out.push(after[..end].trim().to_owned());
        rest = &after[end + close.len()..];
    }
    out
}

fn liteloader_entries(game: &str, bytes: &[u8]) -> Vec<LoaderEntry> {
    let Some(value) = parse_json(bytes) else {
        return Vec::new();
    };
    let Some(versions) = value["versions"].as_object() else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for (name, types) in versions {
        if liteloader_game(name).as_deref() != Some(game) {
            continue;
        }
        // 先看正式发布（artefacts），没有再退到快照构建。
        for group in ["artefacts", "snapshots"] {
            let Some(latest) = types[group]["com.mumfrey:liteloader"]["latest"].as_object() else {
                continue;
            };
            let Some(version) = latest["version"].as_str() else {
                continue;
            };
            let mut entry = LoaderEntry::new(version, game);
            // 快照构建（`1.12.2-SNAPSHOT`）不算稳定版，哪怕清单把它归在 artefacts 里。
            entry.stable = group == "artefacts" && !prerelease(&entry.version);
            entries.push(entry);
            break;
        }
    }
    entries
}

fn optifine_entries(game: &str, bytes: &[u8]) -> Vec<LoaderEntry> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        mcversion: String,
        #[serde(default)]
        filename: String,
        #[serde(default)]
        forge: String,
    }
    let Some(items) =
        parse_json(bytes).and_then(|value| serde_json::from_value::<Vec<Raw>>(value).ok())
    else {
        return Vec::new();
    };
    // 文件名才是权威：`OptiFine_<mc>_<type>_<ver>.jar`，preview 构建前面多一截 `preview_`。
    let regex = regex::Regex::new(r"^(?:preview_)?OptiFine_(.+?)_(.+?)_(.+?)\.jar$")
        .expect("constant regex");
    let mut by_version: std::collections::BTreeMap<String, LoaderEntry> =
        std::collections::BTreeMap::new();
    for item in items {
        let Some(capture) = regex.captures(&item.filename) else {
            continue;
        };
        let mut found = capture[1].to_owned();
        if found.is_empty() {
            found = item.mcversion.clone();
        }
        if found != game {
            continue;
        }
        let version = format!("{}_{}", &capture[2], &capture[3]);
        let preview = item.filename.starts_with("preview_");
        let forge = item
            .forge
            .trim()
            .trim_start_matches("Forge")
            .trim()
            .trim_start_matches('#')
            .trim()
            .to_owned();
        let entry = by_version.entry(version.clone()).or_insert_with(|| {
            let mut entry = LoaderEntry::new(version, game);
            entry.stable = !preview;
            entry
        });
        entry.stable &= !preview;
        if !forge.is_empty() && forge != "N/A" {
            entry.forge = Some(forge);
        }
    }
    let mut entries: Vec<LoaderEntry> = by_version.into_values().collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(optifine_rank(entry)));
    entries
}

/// OptiFine 的版本尾段是「字母 + 数字」（G5、J3、pre1），按字母再按数字排才是发布顺序。
fn optifine_rank(entry: &LoaderEntry) -> (bool, String, String, u32) {
    let mut letters = String::new();
    let mut number = 0u32;
    for part in entry.version.split('_').rev() {
        if part.starts_with("pre") {
            continue;
        }
        let alpha: String = part.chars().take_while(char::is_ascii_alphabetic).collect();
        let digits: String = part.chars().skip_while(char::is_ascii_alphabetic).collect();
        if !alpha.is_empty() && !digits.is_empty() {
            letters = alpha;
            number = digits.parse().unwrap_or(0);
            break;
        }
    }
    let style = entry.version.split('_').next().unwrap_or("").to_owned();
    (entry.stable, style, letters, number)
}

fn parse(filter: LoaderFilter, game: &str, bytes: &[u8]) -> Vec<LoaderEntry> {
    let entries = match filter {
        LoaderFilter::Forge => forge_entries(game, bytes),
        LoaderFilter::NeoForge => neoforge_entries(game, bytes),
        LoaderFilter::Cleanroom => cleanroom_entries(game, bytes),
        LoaderFilter::LiteLoader => liteloader_entries(game, bytes),
        LoaderFilter::OptiFine => optifine_entries(game, bytes),
    };
    let entries = match filter {
        LoaderFilter::Forge if game == "1.8.9" => entries
            .into_iter()
            .filter(|entry| FORGE_LEGACY_BUILDS.contains(&entry.version.as_str()))
            .collect(),
        LoaderFilter::LiteLoader => entries
            .into_iter()
            .filter(|_| liteloader_supported(game))
            .collect(),
        _ => entries,
    };
    match filter {
        // OptiFine 的版本尾段是字母，数字排序会把 G5 排到 J3 后面，顺序在解析时就定好了。
        LoaderFilter::OptiFine => entries,
        _ => newest_first(entries),
    }
}

/// 各加载器清单的候选 URL，按优先级排列；任一成功即返回。
pub fn sources(filter: LoaderFilter, game: &str) -> Vec<String> {
    match filter {
        LoaderFilter::Forge => vec![
            format!("{BMCL_FORGE}/{}", game.replace('-', "_")),
            format!("{FORGE_HTML}/index_{game}.html"),
        ],
        LoaderFilter::NeoForge => vec![
            format!("{BMCL_NEO}/neoforge"),
            format!("{BMCL_NEO}/forge"),
            NEO_VERSIONS.to_owned(),
            NEO_LEGACY_VERSIONS.to_owned(),
        ],
        // GitHub 的 release 列表才是真正能下到安装器的那些版本；maven 元数据只当兜底，
        // 它里面有 `0.4.0` 这种 GitHub 上并不存在的 tag（装到一半会 404）。
        LoaderFilter::Cleanroom => {
            vec![CLEANROOM_RELEASES.to_owned(), CLEANROOM_METADATA.to_owned()]
        }
        LoaderFilter::LiteLoader => {
            vec![BMCL_LITELOADER_JSON.to_owned(), LITELOADER_JSON.to_owned()]
        }
        LoaderFilter::OptiFine => vec![BMCL_OPTIFINE.to_owned()],
    }
}

const BMCL_FORGE_PROMOS: &str = "https://bmclapi2.bangbang93.com/forge/promotions_slim.json";
const FORGE_PROMOS: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";

/// Forge 的「推荐构建」。OptiFine 这类对 Forge 版本敏感的加载器要配推荐版而不是最新版
/// （新版 Forge 用到了 `ResourceLocation.parse`，OptiFine 的补丁类会把它盖掉），
/// 和 PCL 的默认行为保持一致。
pub fn forge_recommended(game: &str, cancel: &CancellationToken) -> Option<String> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Option<String>>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().ok().and_then(|map| map.get(game).cloned()) {
        return hit;
    }
    let found = forge_recommended_uncached(game, cancel);
    if let Ok(mut map) = cache.lock() {
        map.insert(game.to_owned(), found.clone());
    }
    found
}

/// 官网在 Cloudflare 后面，慢的时候能挂上一分钟；界面默认值等不了那么久，给个硬上限。
fn forge_recommended_uncached(game: &str, cancel: &CancellationToken) -> Option<String> {
    for url in [BMCL_FORGE_PROMOS, FORGE_PROMOS] {
        let Ok(client) = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(5))
            .build()
        else {
            continue;
        };
        let Ok(response) = client.get(url).send() else {
            continue;
        };
        if !response.status().is_success() || cancel.is_cancelled() {
            continue;
        }
        let Ok(bytes) = response.bytes() else {
            continue;
        };
        let Some(value) = parse_json(&bytes) else {
            continue;
        };
        let promos = &value["promos"];
        for key in [format!("{game}-recommended"), format!("{game}-latest")] {
            if let Some(version) = promos[key].as_str() {
                return Some(version.to_owned());
            }
        }
    }
    None
}

/// 把推荐构建标成 stable 并排到最前；列表里没有它就原样返回。
pub fn prefer_recommended(entries: &mut Vec<LoaderEntry>, recommended: &str) -> bool {
    if !entries.iter().any(|entry| entry.version == recommended) {
        return false;
    }
    for entry in entries.iter_mut() {
        entry.stable = entry.version == recommended;
    }
    // 稳定排序：推荐版排最前，其余顺序不变。
    entries.sort_by_key(|entry| !entry.stable);
    true
}

type CatalogCache = HashMap<(String, u8), Vec<LoaderEntry>>;

/// 带缓存地拉一份清单；同一进程里同一 (加载器, MC 版本) 只拉一次。
pub fn fetch(
    game: &str,
    filter: LoaderFilter,
    cancel: &CancellationToken,
) -> Result<Vec<LoaderEntry>> {
    static CACHE: OnceLock<Mutex<CatalogCache>> = OnceLock::new();
    let key = (game.to_owned(), filter.slot());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().ok().and_then(|map| map.get(&key).cloned()) {
        return Ok(hit);
    }
    let mut last = None;
    for url in sources(filter, game) {
        match fetch_catalog_bytes(&url, cancel) {
            Ok(bytes) => {
                let mut entries = parse(filter, game, &bytes);
                if entries.is_empty() {
                    continue;
                }
                if filter == LoaderFilter::Forge {
                    if let Some(recommended) = forge_recommended(game, cancel) {
                        prefer_recommended(&mut entries, &recommended);
                    }
                }
                if let Ok(mut map) = cache.lock() {
                    map.insert(key, entries.clone());
                }
                return Ok(entries);
            }
            Err(error) => last = Some(error),
        }
    }
    // 源都答了、只是没有这个 MC 版本的构建时，返回空表；连不上才报错。
    match last {
        Some(DownloadError::Cancelled) => Err(DownloadError::Cancelled),
        _ => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neoforge_versions_map_back_to_minecraft() {
        assert_eq!(neoforge_game("21.1.265").as_deref(), Some("1.21.1"));
        assert_eq!(neoforge_game("20.4.237").as_deref(), Some("1.20.4"));
        assert_eq!(neoforge_game("1.20.1-47.1.106").as_deref(), Some("1.20.1"));
        assert_eq!(neoforge_game("19.0.0-beta"), None);
    }

    #[test]
    fn liteloader_short_names_get_patch_zero() {
        assert_eq!(liteloader_game("1.8").as_deref(), Some("1.8.0"));
        assert_eq!(liteloader_game("1.12.2").as_deref(), Some("1.12.2"));
        assert!(liteloader_supported("1.12.2"));
        assert!(!liteloader_supported("1.6.4"));
    }

    #[test]
    fn cleanroom_versions_are_parsed_from_asset_names() {
        assert_eq!(
            cleanroom_api("cleanroom-0.7.14-alpha-installer.jar").as_deref(),
            Some("0.7.14-alpha")
        );
        assert_eq!(
            cleanroom_api("cleanroom-0.5.4-alpha-installer.jar").as_deref(),
            Some("0.5.4-alpha")
        );
        assert_eq!(cleanroom_api("sources.jar"), None);
    }

    #[test]
    fn optifine_names_carry_the_minecraft_version() {
        assert_eq!(optifine_game("1.12.2_HD_U_G5").as_deref(), Some("1.12.2"));
        assert_eq!(optifine_game("HD_U_G5"), None);
    }

    #[test]
    fn cleanroom_versions_are_read_from_maven_metadata() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
            <metadata><versioning><latest>0.7.14-alpha</latest><versions>
            <version>0.7.13-alpha</version><version>0.7.14-alpha</version>
            </versions></versioning></metadata>"#;
        let entries = parse(LoaderFilter::Cleanroom, "1.12.2", xml);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].version, "0.7.14-alpha");
        // `-alpha` 是预发布，界面要按测试版处理。
        assert!(entries.iter().all(|entry| !entry.stable));
    }

    #[test]
    fn cleanroom_releases_without_a_suffix_are_stable() {
        let xml = br#"<metadata><versioning><versions>
            <version>0.4.0</version><version>0.7.14-alpha</version>
            </versions></versioning></metadata>"#;
        let entries = parse(LoaderFilter::Cleanroom, "1.12.2", xml);
        let stable: Vec<_> = entries.iter().filter(|entry| entry.stable).collect();
        assert_eq!(stable.len(), 1);
        assert_eq!(stable[0].version, "0.4.0");
    }

    #[test]
    fn forge_html_catalog_keeps_only_the_requested_game() {
        let html = r#"
            <a href="net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0-installer.jar">x</a>
            <a href="net/minecraftforge/forge/1.19.2-43.3.0/forge-1.19.2-43.3.0-installer.jar">y</a>
        "#;
        let entries = parse(LoaderFilter::Forge, "1.20.1", html.as_bytes());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].version, "47.2.0");
    }

    #[test]
    fn optifine_downloads_split_the_kind_from_the_patch() {
        assert_eq!(
            optifine_download("1.20.1", "HD_U_I6").as_deref(),
            Some("https://bmclapi2.bangbang93.com/optifine/1.20.1/HD_U/I6")
        );
        // 三段版本号的 MC 版本也要能过（旧实现按「两段」判，1.20.1 会被拒）。
        assert_eq!(
            optifine_download("1.12.2", "HD_U_G5").as_deref(),
            Some("https://bmclapi2.bangbang93.com/optifine/1.12.2/HD_U/G5")
        );
        // 1.8/1.9 的目录名补 `.0`。
        assert_eq!(
            optifine_download("1.8", "HD_U_I8").as_deref(),
            Some("https://bmclapi2.bangbang93.com/optifine/1.8.0/HD_U/I8")
        );
        assert_eq!(optifine_download("1.20.1", "I6"), None);
    }

    #[test]
    fn optifine_preview_versions_keep_their_own_name() {
        let json = br#"[
            {"mcversion":"1.20.1","filename":"OptiFine_1.20.1_HD_U_I6.jar","type":"Release"},
            {"mcversion":"1.20.1","filename":"preview_OptiFine_1.20.1_HD_U_I6_pre6.jar","type":"Preview"}
        ]"#;
        let mut names: Vec<String> = parse(LoaderFilter::OptiFine, "1.20.1", json)
            .into_iter()
            .map(|entry| entry.version)
            .collect();
        names.sort();
        assert_eq!(names, vec!["HD_U_I6", "HD_U_I6_pre6"]);
    }

    #[test]
    fn liteloader_installer_lives_in_the_snapshot_directory() {
        assert_eq!(
            installer_url(LoaderFilter::LiteLoader, "1.12.2", "1.12.2-SNAPSHOT").as_deref(),
            Some(
                "https://bmclapi2.bangbang93.com/maven/com/mumfrey/liteloader/1.12.2-SNAPSHOT/liteloader-1.12.2-SNAPSHOT.jar"
            )
        );
    }
}

#[cfg(test)]
mod live {
    use super::*;
    #[test]
    #[ignore]
    fn probe_sources() {
        let c = CancellationToken::default();
        for (f, g) in [
            (LoaderFilter::Forge, "1.20.1"),
            (LoaderFilter::NeoForge, "1.21.1"),
            (LoaderFilter::Cleanroom, "1.12.2"),
            (LoaderFilter::LiteLoader, "1.12.2"),
            (LoaderFilter::OptiFine, "1.12.2"),
        ] {
            let e = fetch(g, f, &c).unwrap();
            println!("{f:?} {g} -> {} entries, first {:?}", e.len(), e.first());
        }
    }
}
