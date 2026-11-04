//! Blocking official Forge/NeoForge backend for any Minecraft version the
//! loader publishes builds for.
//! Call on a worker thread; serialize installations per root. Official Java code
//! is trusted, not OS-sandboxed. Vanilla installs into isolation with verified
//! read-only live-cache reuse via ordinary copies, never mutable hardlinks.
//! Only non-conflicting files merge; loader JSON is published last. User launcher
//! profiles are never changed. CLI verified in MinecraftForge/Installer ref=2.0
//! and neoforged/LegacyInstaller main SimpleInstaller.java: --installClient <File>
//! selects Actions.CLIENT and explicitly sets headless=true.
use crate::{
    download::DownloadError, io_error, safe_component, safe_relative, CancellationToken,
    InstallProgress, InstallRequest, InstallSummary,
};
use reqwest::{blocking::Client, redirect::Policy, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
    thread,
    time::{Duration, Instant},
};
pub type ModLoaderError = DownloadError;
type Result<T> = std::result::Result<T, ModLoaderError>;
const FORGE: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";
const NEO: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";
/// NeoForge 1.20.1 的 legacy 线在 `net/neoforged/forge` 下，版本号带 MC 前缀。
const NEO_LEGACY: &str = "https://maven.neoforged.net/releases/net/neoforged/forge";
/// BMCLAPI 的版本清单接口：直接 200 返回 JSON，比官方 maven-metadata.xml 快一两个数量级
/// （官方 Forge 目录 213 KB 要 19 秒，NeoForge 的 maven 在这边直接连不上）。
const BMCL_FORGE: &str = "https://bmclapi2.bangbang93.com/forge/minecraft";
const BMCL_NEOFORGE: &str =
    "https://bmclapi2.bangbang93.com/neoforge/meta/api/maven/details/releases/net/neoforged";
const META_LIMIT: u64 = 8 * 1024 * 1024;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ModLoader {
    Forge,
    NeoForge,
    Cleanroom,
    LiteLoader,
    OptiFine,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModLoaderVersion {
    pub version: String,
    /// Forge 的「推荐构建」：OptiFine 之类配最新版 Forge 会崩，界面默认选它。
    #[serde(default)]
    pub recommended: bool,
    /// 测试版（beta/alpha/snapshot）。界面把正式版和测试版分开列。
    #[serde(default)]
    pub beta: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModLoaderInstallRequest {
    pub game_dir: String,
    pub game_version: String,
    pub loader: ModLoader,
    pub loader_version: String,
    pub java_path: String,
    /// 下载源策略（竞速/官方优先/镜像优先/仅镜像/仅官方）。
    #[serde(default)]
    pub strategy: crate::download::DownloadStrategy,
    /// 并发下载数，1..=16，缺省用默认值。
    #[serde(default)]
    pub threads: Option<u32>,
}
fn invalid(message: impl Into<String>) -> DownloadError {
    DownloadError::Invalid(message.into())
}
fn check(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(DownloadError::Cancelled)
    } else {
        Ok(())
    }
}
fn catalog_prefix(loader: ModLoader, game: &str) -> Result<String> {
    if !safe_component(game) {
        return Err(invalid("Unsafe Minecraft version"));
    }
    match loader {
        ModLoader::Forge => Ok(format!("{game}-")),
        // NeoForge 的版本号不再带 MC 号（20.2 起）或带 legacy 前缀（1.20.1-），
        // 前缀只用来做粗筛，真正的判定交给 `loader_catalog::neoforge_game`。
        ModLoader::NeoForge => {
            if crate::loader_catalog::neoforge_legacy_game(game) {
                Ok(format!("{game}-"))
            } else {
                // NeoForge 只有 1.20.2 起才有构建；更早的版本直接告诉调用方没有。
                let mut parts = game.split('.').filter_map(|part| part.parse::<u32>().ok());
                let supported = matches!(
                    (parts.next(), parts.next(), parts.next()),
                    (Some(1), Some(minor), Some(patch)) if minor > 20 || (minor == 20 && patch >= 2)
                );
                if supported {
                    Ok(String::new())
                } else {
                    Err(invalid(format!(
                        "NeoForge has no builds for Minecraft {game}"
                    )))
                }
            }
        }
        // 这三个的版本号不嵌 Minecraft 号，前缀用不上。
        ModLoader::Cleanroom | ModLoader::LiteLoader | ModLoader::OptiFine => Ok(String::new()),
    }
}
/// 这个「游戏版本 + 加载器」需要的最低 Java 主版本，与 `java::requirement_with` 同一套规则。
pub fn required_java_for(game: &str, loader: ModLoader, version: &str) -> u32 {
    let mut presence = crate::java::LoaderPresence::default();
    match loader {
        ModLoader::Forge => presence.forge = Some(version),
        ModLoader::Cleanroom => presence.cleanroom = Some(version),
        ModLoader::LiteLoader => presence.liteloader = true,
        ModLoader::OptiFine => presence.optifine = true,
        ModLoader::NeoForge => {}
    }
    crate::java::requirement_with(game, None, &presence)
        .range
        .min
}

fn base(loader: ModLoader) -> &'static str {
    match loader {
        ModLoader::Forge => FORGE,
        ModLoader::NeoForge => NEO,
        ModLoader::Cleanroom | ModLoader::LiteLoader | ModLoader::OptiFine => "",
    }
}
fn trusted_host(host: &str) -> bool {
    host.ends_with(".sakura.sld.tw") || matches!(host, "sakura.sld.tw")
}
fn trusted_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|e| invalid(e.to_string()))?;
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !known_host(url.host_str().unwrap_or_default())
    {
        return Err(invalid(format!("Untrusted official installer URL: {raw}")));
    }
    Ok(url)
}
fn known_host(host: &str) -> bool {
    const HOSTS: &[&str] = &[
        "bmclapi2.bangbang93.com",
        "mirrors.hit.edu.cn",
        "mirrors.cernet.edu.cn",
        "maven.minecraftforge.net",
        "files.minecraftforge.net",
        "maven.neoforged.net",
        "github.com",
        "ghfast.top",
        "objects.githubusercontent.com",
        "release-assets.githubusercontent.com",
        "libraries.minecraft.net",
        "piston-meta.mojang.com",
        "piston-data.mojang.com",
        "launchermeta.mojang.com",
        "launcher.mojang.com",
        "resources.download.minecraft.net",
        "optifine.net",
        "www.optifine.net",
    ];
    !host.is_empty() && (trusted_host(host) || HOSTS.contains(&host))
}
/// 镜像会 302 到自家 CDN，只允许跳到白名单主机。
fn host_trusted(host: Option<&str>) -> bool {
    host.is_some_and(known_host)
}

fn client() -> Result<Client> {
    Client::builder()
        .https_only(true)
        .redirect(Policy::custom(|attempt| {
            // BMCLAPI 会 302 到各家高校镜像（hit / cernet / ...），路径里保留了 /bmclapi/。
            let mirror_path = attempt.url().path().contains("/bmclapi/");
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if host_trusted(attempt.url().host_str()) || mirror_path {
                attempt.follow()
            } else {
                attempt.error("redirect to untrusted host")
            }
        }))
        .connect_timeout(Duration::from_secs(20))
        // 镜像慢时 7 MB 的安装器要 100 s 以上，给足余量再交给取消按钮。
        .timeout(Duration::from_secs(240))
        // 国内 IPv6 路由常常连上即被重置，优先用 IPv4。
        .local_address(if cfg!(windows) {
            Some(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
        } else {
            None
        })
        .user_agent("YBUMCL/0.1")
        .build()
        .map_err(|e| DownloadError::Network(e.to_string()))
}
fn fetch(client: &Client, url: &str, limit: u64, cancel: &CancellationToken) -> Result<Vec<u8>> {
    fetch_progress(client, url, limit, cancel, |_| {})
}
fn fetch_progress(
    client: &Client,
    url: &str,
    limit: u64,
    cancel: &CancellationToken,
    mut on_bytes: impl FnMut(u64),
) -> Result<Vec<u8>> {
    // 镜像的 302 偶尔会抽风（例如 bmclapi 的 optifine 下载），网络错误重试几次。
    let mut last = None;
    for attempt in 0..3 {
        match fetch_once(client, url, limit, cancel, &mut on_bytes) {
            Ok(bytes) => return Ok(bytes),
            Err(DownloadError::Cancelled) => return Err(DownloadError::Cancelled),
            Err(error @ DownloadError::Network(_)) => {
                last = Some(error);
                if attempt < 2 {
                    std::thread::sleep(std::time::Duration::from_millis(500 * (attempt + 1)));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Err(last.expect("at least one attempt"))
}

fn fetch_once(
    client: &Client,
    url: &str,
    limit: u64,
    cancel: &CancellationToken,
    on_bytes: &mut impl FnMut(u64),
) -> Result<Vec<u8>> {
    check(cancel)?;
    let mut response = client
        .get(trusted_url(url)?)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| DownloadError::Network(e.to_string()))?;
    let mut result = Vec::new();
    let mut buffer = [0; 65536];
    loop {
        check(cancel)?;
        let n = response
            .read(&mut buffer)
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        if result.len() as u64 + n as u64 > limit {
            return Err(invalid("Official response exceeds size limit"));
        }
        result.extend_from_slice(&buffer[..n]);
        on_bytes(result.len() as u64);
    }
    Ok(result)
}
fn parse_catalog(xml: &[u8], loader: ModLoader, game: &str) -> Result<Vec<ModLoaderVersion>> {
    let prefix = catalog_prefix(loader, game)?;
    let text = std::str::from_utf8(xml).map_err(|_| invalid("Maven catalog is not UTF-8"))?;
    if !text.contains("<metadata") || !text.contains("</metadata>") || text.contains("<!DOCTYPE") {
        return Err(invalid("Invalid Maven metadata"));
    }
    let regex = regex::Regex::new(r"<version>\s*([^<]+?)\s*</version>").expect("constant regex");
    let mut versions = BTreeSet::new();
    for capture in regex.captures_iter(text) {
        let value = capture[1].trim();
        if !value.starts_with(&prefix) || !safe_component(value) {
            continue;
        }
        // NeoForge 的版本号规则不是简单前缀（21.0.x、21.10.x、24+、快照线），按映射判归属。
        if loader == ModLoader::NeoForge
            && crate::loader_catalog::neoforge_game(value).as_deref() != Some(game)
        {
            continue;
        }
        let value = if loader == ModLoader::Forge {
            &value[prefix.len()..]
        } else {
            value
        };
        if safe_component(value) {
            versions.insert(value.to_owned());
        }
    }
    let mut versions: Vec<_> = versions.into_iter().collect();
    versions.sort_by_cached_key(|v| {
        let mut p = v.splitn(2, '-');
        let numbers = p
            .next()
            .unwrap_or("")
            .split('.')
            .map(|n| n.parse::<u32>().unwrap_or(0))
            .collect::<Vec<_>>();
        let suffix = p.next().unwrap_or("").to_owned();
        (numbers, suffix.is_empty(), suffix)
    });
    Ok(versions
        .into_iter()
        .rev()
        .map(|version| ModLoaderVersion {
            beta: crate::loader_catalog::prerelease(&version),
            version,
            recommended: false,
        })
        .collect())
}
/// Newest builds first. Forge versions omit the Minecraft coordinate prefix.
pub fn list_modloader_versions(
    loader: ModLoader,
    game_version: &str,
) -> Result<Vec<ModLoaderVersion>> {
    if !safe_component(game_version) {
        return Err(invalid("Unsafe Minecraft version"));
    }
    // 目录同一进程里不会变，缓存一份：来回切加载器不再重新拉。
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Vec<ModLoaderVersion>>>,
    > = std::sync::OnceLock::new();
    let key = format!("{loader:?}:{game_version}");
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return Ok(hit);
    }
    let loaded = load_modloader_versions(loader, game_version)
        .map(|versions| prefer_recommended_forge(loader, game_version, versions))
        .map(|versions| prefer_stable_neoforge(loader, versions));
    if let Ok(list) = &loaded {
        if let Ok(mut c) = cache.lock() {
            c.insert(key, list.clone());
        }
    }
    loaded
}

/// Forge 默认落到官网的「推荐构建」而不是最新版：OptiFine 这类加载器配最新版 Forge 会
/// `NoSuchMethodError: ResourceLocation.parse`，推荐版是官方认可的稳定组合，PCL 也是这么默认的。
fn prefer_recommended_forge(
    loader: ModLoader,
    game_version: &str,
    mut versions: Vec<ModLoaderVersion>,
) -> Vec<ModLoaderVersion> {
    if !matches!(loader, ModLoader::Forge) {
        return versions;
    }
    let Some(recommended) =
        crate::loader_catalog::forge_recommended(game_version, &CancellationToken::default())
    else {
        return versions;
    };
    if let Some(index) = versions.iter().position(|item| item.version == recommended) {
        let mut entry = versions.remove(index);
        entry.recommended = true;
        versions.insert(0, entry);
    }
    versions
}

/// NeoForge 没有 `promotions_slim.json` 这类官方推荐，且 20.4/1.20.6/21.4 这些线的最新一条
/// 常常是 `-beta`。照 PCL 的做法：默认落在「最新稳定版」上，并把它排到最前面。
fn prefer_stable_neoforge(
    loader: ModLoader,
    mut versions: Vec<ModLoaderVersion>,
) -> Vec<ModLoaderVersion> {
    if !matches!(loader, ModLoader::NeoForge) {
        return versions;
    }
    let Some(index) = versions
        .iter()
        .position(|item| !crate::loader_catalog::prerelease(&item.version))
    else {
        return versions;
    };
    let mut entry = versions.remove(index);
    entry.recommended = true;
    versions.insert(0, entry);
    versions
}

fn load_modloader_versions(loader: ModLoader, game_version: &str) -> Result<Vec<ModLoaderVersion>> {
    // 先问 BMCLAPI（快），失败再回落官方 maven 目录（慢，NeoForge 可能直接不通）。
    match bmcl_modloader_versions(loader, game_version) {
        Ok(versions) if !versions.is_empty() => Ok(versions),
        _ => {
            catalog_prefix(loader, game_version)?;
            parse_catalog(
                &fetch(
                    &client()?,
                    &format!("{}/maven-metadata.xml", base(loader)),
                    META_LIMIT,
                    &CancellationToken::default(),
                )?,
                loader,
                game_version,
            )
        }
    }
}

/// BMCLAPI 的加载器版本清单。两个接口都是「一整个目录的 JSON」：
/// Forge 返回该 Minecraft 版本下的所有构建，NeoForge 返回 org.neoforged 下的全部目录名。
/// 只有 Forge 系的清单会带 `<game>-` 前缀；LiteLoader 的 `1.12.2-SNAPSHOT` 是完整版本号，
/// 剥掉前缀就只剩一个 `SNAPSHOT`，列表会显示错。
fn strip_game_prefix(loader: ModLoader, game: &str, name: String) -> String {
    if matches!(loader, ModLoader::Forge | ModLoader::NeoForge) {
        let prefix = format!("{game}-");
        name.strip_prefix(&prefix).unwrap_or(&name).to_owned()
    } else {
        name
    }
}

fn bmcl_modloader_versions(loader: ModLoader, game: &str) -> Result<Vec<ModLoaderVersion>> {
    let client = client()?;
    let cancel = CancellationToken::default();
    let mut names: Vec<String> = match loader {
        ModLoader::Forge => {
            let body = fetch(
                &client,
                &format!("{BMCL_FORGE}/{}", game.replace('-', "_")),
                8 * 1024 * 1024,
                &cancel,
            )?;
            let entries: Vec<serde_json::Value> = serde_json::from_slice(&body)
                .map_err(|e| invalid(format!("Invalid Forge catalog: {e}")))?;
            entries
                .iter()
                .filter_map(|entry| entry["version"].as_str().map(str::to_owned))
                .collect()
        }
        ModLoader::NeoForge => {
            let mut found = Vec::new();
            for module in ["neoforge", "forge"] {
                let Ok(body) = fetch(
                    &client,
                    &format!("{BMCL_NEOFORGE}/{module}"),
                    8 * 1024 * 1024,
                    &cancel,
                ) else {
                    continue;
                };
                let value: serde_json::Value = match serde_json::from_slice(&body) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if let Some(files) = value["files"].as_array() {
                    for file in files {
                        if file["type"].as_str() != Some("DIRECTORY") {
                            continue;
                        }
                        if let Some(name) = file["name"].as_str() {
                            found.push(name.to_owned());
                        }
                    }
                }
            }
            found
        }
        ModLoader::Cleanroom | ModLoader::LiteLoader | ModLoader::OptiFine => {
            crate::loader_catalog::fetch(
                game,
                crate::loader_catalog::LoaderFilter::parse(&loader_key(loader))
                    .ok_or_else(|| invalid("Unknown loader"))?,
                &cancel,
            )?
            .into_iter()
            .map(|entry| entry.version)
            .collect()
        }
    };
    // 只留下属于这个 Minecraft 版本的构建。
    let forge_prefix = format!("{game}-");
    // Forge 的这个接口本身就是「某个 Minecraft 版本下的所有构建」，
    // version 字段已经是裸构建号（52.1.20），不需要再按前缀筛。
    // NeoForge 用统一的版本号→MC 版本映射来筛（legacy / 快照 / 24 之后的写法都不一样）。
    names.retain(|name| {
        if !safe_component(name) {
            return false;
        }
        match loader {
            ModLoader::Forge => true,
            ModLoader::NeoForge => {
                name.starts_with(&forge_prefix)
                    || crate::loader_catalog::neoforge_game(name).as_deref() == Some(game)
            }
            ModLoader::Cleanroom | ModLoader::LiteLoader | ModLoader::OptiFine => true,
        }
    });
    let mut versions: Vec<String> = names
        .into_iter()
        .map(|name| strip_game_prefix(loader, game, name))
        .collect();
    versions.sort_by_cached_key(|v| numeric_key(v));
    versions.dedup();
    versions.reverse();
    Ok(versions
        .into_iter()
        .map(|version| ModLoaderVersion {
            beta: crate::loader_catalog::prerelease(&version),
            version,
            recommended: false,
        })
        .collect())
}

fn numeric_key(version: &str) -> (Vec<u32>, bool, String) {
    let mut split = version.splitn(2, '-');
    let numbers = split
        .next()
        .unwrap_or("")
        .split('.')
        .map(|n| n.parse::<u32>().unwrap_or(0))
        .collect::<Vec<_>>();
    let suffix = split.next().unwrap_or("").to_owned();
    (numbers, suffix.is_empty(), suffix)
}
/// Forge 的 maven 在国内经常连不上，开了镜像就先走 BMCLAPI（它会再到国内 CDN）。
fn mirror_maven(url: &str, allow_mirror: bool) -> String {
    if !allow_mirror {
        return url.to_owned();
    }
    match mirrored_installer(url) {
        Some(mirrored) => mirrored,
        None => url.to_owned(),
    }
}
fn mirrored_installer(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    if host == "github.com" && parsed.path().contains("/releases/download/") {
        return Some(format!("https://ghfast.top/{url}"));
    }
    if !matches!(host, "maven.minecraftforge.net" | "maven.neoforged.net") {
        return None;
    }
    let path = parsed
        .path()
        .strip_prefix("/releases")
        .unwrap_or(parsed.path());
    let mut mirrored = Url::parse("https://bmclapi2.bangbang93.com").ok()?;
    mirrored.set_path(&format!("/maven{path}"));
    Some(mirrored.into())
}

/// Cleanroom 的安装器同样发布在 maven 上，国内直连 nexus 比 GitHub 代理快一个数量级。
fn cleanroom_mirror(version: &str) -> Option<String> {
    if !safe_component(version) {
        return None;
    }
    Some(format!(
        "https://nexus.jsdu.cn/repository/com/cleanroommc/cleanroom/{version}/cleanroom-{version}-installer.jar"
    ))
}

/// 只解析「该加载器的安装器下载地址」，不安装、不落盘（安装包页面用）。
pub fn installer_download(r: &ModLoaderInstallRequest) -> Result<String> {
    installer_url(r)
}
fn installer_url(r: &ModLoaderInstallRequest) -> Result<String> {
    if !safe_component(&r.game_version) || !safe_component(&r.loader_version) {
        return Err(invalid("Unsafe loader version"));
    }
    let (artifact, coordinate) = match r.loader {
        ModLoader::Forge => ("forge", format!("{}-{}", r.game_version, r.loader_version)),
        // 1.20.1 的 NeoForge 是 legacy 线，坐标在 `net/neoforged/forge/1.20.1-47.x/` 下，
        // 版本号要带 MC 前缀；20.2 以后才搬到 `net/neoforged/neoforge/`。
        ModLoader::NeoForge => {
            if crate::loader_catalog::neoforge_legacy_game(&r.game_version) {
                ("forge", format!("{}-{}", r.game_version, r.loader_version))
            } else {
                ("neoforge", r.loader_version.clone())
            }
        }
        ModLoader::Cleanroom | ModLoader::LiteLoader | ModLoader::OptiFine => {
            return loader_package(r).map(|package| package.url);
        }
    };
    let root = if artifact == "forge" && r.loader == ModLoader::NeoForge {
        NEO_LEGACY
    } else {
        base(r.loader)
    };
    Ok(mirror_maven(
        &format!("{root}/{coordinate}/{artifact}-{coordinate}-installer.jar"),
        r.strategy.allows_mirror(),
    ))
}

/// Forge 系以外的加载器要下载的那个包（OptiFine 是安装器，LiteLoader 直接就是修改过的版本 jar）。
struct LoaderPackage {
    url: String,
    /// 修改过的版本 jar 在游戏目录里的位置；安装器型为 None。
    path: Option<String>,
}

fn loader_package(r: &ModLoaderInstallRequest) -> Result<LoaderPackage> {
    let filter = crate::loader_catalog::LoaderFilter::parse(&loader_key(r.loader))
        .ok_or_else(|| invalid("Unsupported loader"))?;
    let url = crate::loader_catalog::installer_url(filter, &r.game_version, &r.loader_version)
        .ok_or_else(|| invalid("No download for this loader version"))?;
    let path = match r.loader {
        ModLoader::Forge | ModLoader::NeoForge | ModLoader::Cleanroom | ModLoader::OptiFine => None,
        ModLoader::LiteLoader => {
            let game = crate::loader_catalog::liteloader_game(&r.game_version)
                .ok_or_else(|| invalid("LiteLoader does not support this Minecraft version"))?;
            Some(crate::maven_path(&format!(
                "com.mumfrey:liteloader:{game}-SNAPSHOT"
            ))?)
        }
    };
    Ok(LoaderPackage { url, path })
}

fn loader_key(loader: ModLoader) -> String {
    match loader {
        ModLoader::Forge => "forge",
        ModLoader::NeoForge => "neoforge",
        ModLoader::Cleanroom => "cleanroom",
        ModLoader::LiteLoader => "liteloader",
        ModLoader::OptiFine => "optifine",
    }
    .to_owned()
}

fn valid_hash(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn checksum(bytes: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| invalid("Invalid SHA1"))?
        .trim();
    if !valid_hash(text) {
        return Err(invalid("Expected plain 40-digit SHA1"));
    }
    Ok(text.to_ascii_lowercase())
}
fn hash_bytes(data: &[u8]) -> String {
    format!("{:x}", Sha1::digest(data))
}
fn hash_file(path: &Path, cancel: &CancellationToken) -> Result<String> {
    let mut file = File::open(path).map_err(|e| io_error(path, e))?;
    let mut hash = Sha1::new();
    let mut buffer = [0; 65536];
    loop {
        check(cancel)?;
        let n = file.read(&mut buffer).map_err(|e| io_error(path, e))?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn no_links(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                #[cfg(windows)]
                let linked = {
                    use std::os::windows::fs::MetadataExt;
                    meta.file_type().is_symlink() || meta.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let linked = meta.file_type().is_symlink();
                if linked {
                    return Err(invalid(format!(
                        "Symlink/reparse point not allowed: {}",
                        ancestor.display()
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(io_error(ancestor, e).into()),
        }
    }
    Ok(())
}
fn root_path(raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    if !path.is_absolute()
        || path.components().any(|p| matches!(p, Component::ParentDir))
        || raw.contains('!')
    {
        return Err(invalid(
            "Game directory must be absolute without '..' or '!'",
        ));
    }
    no_links(&path)?;
    fs::create_dir_all(&path).map_err(|e| io_error(&path, e))?;
    fs::canonicalize(&path).map_err(|e| io_error(&path, e).into())
}
struct Stage(PathBuf);
impl Stage {
    fn new(root: &Path) -> Result<Self> {
        for _ in 0..100 {
            let path = root.join(format!(
                ".ybumcl-modloader-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io_error(&path, e).into()),
            }
        }
        Err(invalid("Cannot allocate staging directory"))
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn write_new(path: &Path, data: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| io_error(path, e))?;
    file.write_all(data)
        .and_then(|_| file.sync_all())
        .map_err(|e| io_error(path, e).into())
}
fn zip_bytes(zip: &mut zip::ZipArchive<File>, name: &str) -> Result<Vec<u8>> {
    let mut entry = zip
        .by_name(name)
        .map_err(|e| invalid(format!("Installer {name}: {e}")))?;
    if entry.size() > META_LIMIT {
        return Err(invalid("Installer entry exceeds size limit"));
    }
    let mut data = Vec::new();
    (&mut entry)
        .take(META_LIMIT + 1)
        .read_to_end(&mut data)
        .map_err(|e| invalid(e.to_string()))?;
    if data.len() as u64 > META_LIMIT {
        return Err(invalid("Installer entry exceeds size limit"));
    }
    Ok(data)
}
fn json(data: &[u8]) -> Result<Value> {
    serde_json::from_slice(data).map_err(|e| invalid(e.to_string()))
}
fn audit_urls(value: &Value) -> Result<()> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if key == "url" {
                    if let Some(url) = value.as_str().filter(|s| !s.is_empty()) {
                        // 老版本 Forge 安装器自带的 json 里，有些库的 url 只是「去哪下」的
                        // 说明页（比如论坛帖），我们并不会请求它，不该因此否决整份清单。
                        if !url.contains("minecraftforge.net/forum") {
                            trusted_url(url)?;
                        }
                    }
                }
                audit_urls(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                audit_urls(value)?;
            }
        }
        _ => (),
    }
    Ok(())
}
struct Profile {
    id: String,
    version: Value,
    install: Value,
    /// 安装器自己要下载的库：先用镜像替它备好，安装器校验 SHA1 后会直接跳过。
    libraries: Vec<InstallerLibrary>,
    /// true = 跑官方安装器（spec 1）；false = 老安装器，直接把自带的版本 json 摊开安装。
    headless: bool,
}

#[derive(Clone)]
struct InstallerLibrary {
    path: String,
    url: String,
    sha1: String,
}

fn installer_libraries(install: &Value) -> Vec<InstallerLibrary> {
    let mut libraries = Vec::new();
    let Some(entries) = install["libraries"].as_array() else {
        return libraries;
    };
    for entry in entries {
        let artifact = &entry["downloads"]["artifact"];
        let (Some(path), Some(url), Some(sha1)) = (
            artifact["path"].as_str(),
            artifact["url"].as_str(),
            artifact["sha1"].as_str(),
        ) else {
            continue;
        };
        if safe_relative(path).is_ok() && valid_hash(sha1) {
            libraries.push(InstallerLibrary {
                path: path.to_owned(),
                url: url.to_owned(),
                sha1: sha1.to_owned(),
            });
        }
    }
    libraries
}

/// 老安装器（1.12.2 及更早）把整份版本 json 塞在 install_profile.json 的 versionInfo 里，
/// 有些还额外给一个 install.json 指向安装后的 profile。
fn embedded_profile_version(install: &Value) -> Option<Value> {
    if let Some(info) = install.get("versionInfo") {
        if info.is_object() {
            return Some(info.clone());
        }
    }
    install
        .get("install")
        .and_then(|install| install.get("versionInfo"))
        .filter(|info| info.is_object())
        .cloned()
}

fn inspect_installer(path: &Path, r: &ModLoaderInstallRequest) -> Result<Profile> {
    let mut zip = zip::ZipArchive::new(File::open(path).map_err(|e| io_error(path, e))?)
        .map_err(|e| invalid(e.to_string()))?;
    let install = json(&zip_bytes(&mut zip, "install_profile.json")?)?;
    let declared = install
        .get("minecraft")
        .and_then(Value::as_str)
        .or_else(|| {
            install
                .get("install")
                .and_then(|i| i.get("minecraft"))
                .and_then(Value::as_str)
        });
    if let Some(declared) = declared {
        if declared != r.game_version {
            return Err(invalid("安装器里的 Minecraft 版本与所选版本不一致"));
        }
    }
    let headless = install["spec"].as_u64() == Some(1);
    let version = if headless {
        let name = install["json"]
            .as_str()
            .ok_or_else(|| invalid("Missing installer version JSON"))?
            .trim_start_matches('/');
        safe_relative(name)?;
        json(&zip_bytes(&mut zip, name)?)?
    } else if let Some(info) = embedded_profile_version(&install) {
        info
    } else {
        let name = install
            .get("json")
            .and_then(Value::as_str)
            .or_else(|| {
                install
                    .get("install")
                    .and_then(|i| i.get("json"))
                    .and_then(Value::as_str)
            })
            .ok_or_else(|| invalid("安装器里找不到版本信息，无法自动安装"))?
            .trim_start_matches('/');
        safe_relative(name)?;
        json(&zip_bytes(&mut zip, name)?)?
    };
    let id = version["id"]
        .as_str()
        .ok_or_else(|| invalid("Missing installed version id"))?
        .to_owned();
    if !safe_component(&id)
        || id == r.game_version
        || version["mainClass"].as_str().is_none()
        || version["inheritsFrom"].as_str() != Some(r.game_version.as_str())
    {
        return Err(invalid("Installer identity/inheritance mismatch"));
    }
    audit_urls(&install)?;
    audit_urls(&version)?;
    let libraries = installer_libraries(&install);
    Ok(Profile {
        id,
        version,
        install,
        libraries,
        headless,
    })
}
fn event(emit: &impl Fn(InstallProgress), version_id: &str, stage: &str, path: &str) {
    emit(InstallProgress {
        version_id: version_id.to_owned(),
        stage: stage.to_owned(),
        path: path.to_owned(),
        completed_files: 0,
        total_files: 0,
        downloaded_bytes: 0,
        total_bytes: 0,
    });
}

fn java_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(tail) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{tail}"));
    }
    if let Some(tail) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(tail.to_owned());
    }
    path.to_path_buf()
}

fn log_tail(path: &Path) -> String {
    let Ok(bytes) = fs::read(path) else {
        return String::new();
    };
    let start = bytes.len().saturating_sub(8192);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

fn kill_wait(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[allow(clippy::too_many_arguments)]
fn run_installer(
    java: &Path,
    jar: &Path,
    console_dir: &Path,
    cwd: &Path,
    args: &[String],
    env: Option<(&str, PathBuf)>,
    cancel: &CancellationToken,
    emit: impl Fn(&str),
) -> Result<()> {
    run_java(java, jar, None, console_dir, cwd, args, env, cancel, emit)
}

/// 跑安装器里的某个主类（OptiFine 的 `optifine.Patcher` 只能这样调，没有无界面参数）。
#[allow(clippy::too_many_arguments)]
/// 起一个 java 子进程的公共壳：headless、日志重定向、TMP 指到暂存目录、清掉外来环境变量。
fn java_command(java: &Path, console_dir: &Path) -> Result<(Command, PathBuf)> {
    let console_path = console_dir.join("java-console.log");
    let console = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&console_path)
        .map_err(|e| io_error(&console_path, e))?;
    let stderr = console
        .try_clone()
        .map_err(|e| io_error(&console_path, e))?;
    let mut command = Command::new(java_path(java));
    command.arg("-Djava.awt.headless=true");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(console))
        .stderr(Stdio::from(stderr));
    // 安装器（尤其 Forge 的 post-process）会在 %TEMP% 下建目录；%TEMP% 不可写时整次安装直接失败。
    // 统一指到暂存目录里，顺带避免清理软件扫掉安装中间文件。
    let tmp = console_dir.join("tmp");
    fs::create_dir_all(&tmp).map_err(|e| io_error(&tmp, e))?;
    command
        .env("TMP", java_path(&tmp))
        .env("TEMP", java_path(&tmp));
    for key in [
        "JAVA_TOOL_OPTIONS",
        "JDK_JAVA_OPTIONS",
        "_JAVA_OPTIONS",
        "CLASSPATH",
    ] {
        command.env_remove(key);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    Ok((command, console_path))
}

fn wait_java(
    mut child: std::process::Child,
    java: &Path,
    console_path: &Path,
    cwd: &Path,
    cancel: &CancellationToken,
    emit: impl Fn(&str),
) -> Result<()> {
    let start = Instant::now();
    let log = cwd.join("installer.jar.log");
    let status = loop {
        let reason = if cancel.is_cancelled() {
            Some(DownloadError::Cancelled)
        } else if start.elapsed() > Duration::from_secs(1800) {
            Some(invalid("Official installer timed out after 30 minutes"))
        } else if [&log, console_path].iter().any(|path| {
            fs::metadata(path)
                .map(|m| m.len() > META_LIMIT)
                .unwrap_or(false)
        }) {
            Some(invalid("Installer log exceeded 8 MiB"))
        } else {
            None
        };
        if let Some(error) = reason {
            kill_wait(&mut child);
            return Err(error);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if let Some(line) = tail_line(console_path) {
                    emit(&line);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => {
                kill_wait(&mut child);
                return Err(io_error(java, e).into());
            }
        }
    };
    check(cancel)?;
    if !status.success() {
        return Err(invalid(format!(
            "Official CLIENT installer failed ({status}):\nJava console:\n{}\nInstaller log:\n{}",
            log_tail(console_path),
            log_tail(&log)
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_java(
    java: &Path,
    jar: &Path,
    main: Option<&str>,
    console_dir: &Path,
    cwd: &Path,
    args: &[String],
    env: Option<(&str, PathBuf)>,
    cancel: &CancellationToken,
    emit: impl Fn(&str),
) -> Result<()> {
    let (mut command, console_path) = java_command(java, console_dir)?;
    match main {
        Some(main) => {
            command.arg("-cp").arg(java_path(jar)).arg(main).args(args);
        }
        None => {
            command.arg("-jar").arg(java_path(jar)).args(args);
        }
    }
    command.current_dir(java_path(cwd));
    if let Some((key, value)) = env {
        command.env(key, java_path(&value));
    }
    let child = command.spawn().map_err(|e| io_error(java, e))?;
    wait_java(child, java, &console_path, cwd, cancel, emit)
}

/// 直接跑某个 classpath 上的主类 —— Forge/NeoForge 的 processor 都是这么调的。
#[allow(clippy::too_many_arguments)]
fn run_java_classpath(
    java: &Path,
    classpath: &[PathBuf],
    main: &str,
    console_dir: &Path,
    cwd: &Path,
    args: &[String],
    cancel: &CancellationToken,
    emit: impl Fn(&str),
) -> Result<()> {
    let (mut command, console_path) = java_command(java, console_dir)?;
    let joined = classpath
        .iter()
        .map(|path| java_path(path).to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(";");
    command
        .arg("-cp")
        .arg(joined)
        .arg(main)
        .args(args)
        .current_dir(java_path(cwd));
    let child = command.spawn().map_err(|e| io_error(java, e))?;
    wait_java(child, java, &console_path, cwd, cancel, emit)
}

fn tail_line(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(4096)..]);
    let line = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())?
        .trim();
    if line.is_empty() {
        return None;
    }
    let short: String = line.chars().take(110).collect();
    Some(if short.len() < line.len() {
        format!("{short}…")
    } else {
        short
    })
}

/// 安装器自己也有一批库要下（默认走官方 maven，慢），先用镜像塞进它的目标目录。
/// 安装器 `processors` 里的一步：主类 jar + classpath + 参数。
struct ProcessorStep {
    jar: String,
    classpath: Vec<String>,
    args: Vec<String>,
}

/// 只留客户端要跑的那些步骤（`sides` 缺省表示两边都要）。
fn processor_steps(install: &Value) -> Vec<ProcessorStep> {
    let mut steps = Vec::new();
    let Some(list) = install["processors"].as_array() else {
        return steps;
    };
    for entry in list {
        if let Some(sides) = entry["sides"].as_array() {
            let client = sides.iter().any(|side| side.as_str() == Some("client"));
            if !client {
                continue;
            }
        }
        let Some(jar) = entry["jar"].as_str() else {
            continue;
        };
        let strings = |value: &Value| -> Vec<String> {
            value
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        steps.push(ProcessorStep {
            jar: jar.to_owned(),
            classpath: strings(&entry["classpath"]),
            args: strings(&entry["args"]),
        });
    }
    steps
}

/// `group:artifact:version[:classifier][@ext]` → `libraries` 下的相对路径。
fn processor_relative(coordinate: &str) -> Option<String> {
    let coordinate = coordinate.trim();
    let (body, ext) = match coordinate.rsplit_once('@') {
        Some((body, ext)) if !ext.is_empty() => (body, ext),
        _ => (coordinate, "jar"),
    };
    let parts: Vec<&str> = body.split(':').collect();
    if !(3..=4).contains(&parts.len()) || parts.iter().any(|part| part.is_empty()) {
        return None;
    }
    let classifier = parts
        .get(3)
        .map(|part| format!("-{part}"))
        .unwrap_or_default();
    Some(format!(
        "{}/{}/{}/{}-{}{}.{}",
        parts[0].replace('.', "/"),
        parts[1],
        parts[2],
        parts[1],
        parts[2],
        classifier,
        ext
    ))
}

/// 库文件按坐标找路径；缺了就按官方 maven（镜像优先）补下来。
/// 那些由 processor 生成的产物（slim/extra/srg/patched）本来就不存在，这里只返回路径。
fn processor_library(
    http: &Client,
    stage: &Path,
    coordinate: &str,
    mirror: bool,
    cancel: &CancellationToken,
) -> Result<PathBuf> {
    let relative = processor_relative(coordinate).ok_or_else(|| {
        invalid(format!(
            "invalid processor library coordinate: {coordinate}"
        ))
    })?;
    let path = stage.join("libraries").join(safe_relative(&relative)?);
    if fs::metadata(&path)
        .map(|meta| meta.is_file())
        .unwrap_or(false)
    {
        return Ok(path);
    }
    let official = format!("https://maven.minecraftforge.net/{relative}");
    let candidates = [
        format!("https://bmclapi2.bangbang93.com/maven/{relative}"),
        mirror_maven(&official, mirror),
        official,
        format!("https://maven.neoforged.net/releases/{relative}"),
    ];
    for candidate in candidates {
        match fetch_once(http, &candidate, 64 * 1024 * 1024, cancel, &mut |_| {}) {
            Ok(bytes) if !bytes.is_empty() => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
                }
                fs::write(&path, &bytes).map_err(|e| io_error(&path, e))?;
                return Ok(path);
            }
            Err(DownloadError::Cancelled) => return Err(DownloadError::Cancelled),
            _ => continue,
        }
    }
    Ok(path)
}

fn zip_entry_bytes(jar: &Path, entry: &str) -> Result<Vec<u8>> {
    let file = File::open(jar).map_err(|e| io_error(jar, e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| invalid(format!("installer: {e}")))?;
    let mut item = archive
        .by_name(entry)
        .map_err(|e| invalid(format!("installer entry {entry}: {e}")))?;
    let mut bytes = Vec::new();
    item.read_to_end(&mut bytes)
        .map_err(|e| invalid(format!("installer entry {entry}: {e}")))?;
    Ok(bytes)
}

/// install_profile 里 `data` 的取值：字面量、`[坐标]`、`/data/x`（安装器内嵌文件）。
fn processor_data(
    http: &Client,
    stage: &Path,
    installer: &Path,
    install: &Value,
    mirror: bool,
    cancel: &CancellationToken,
) -> Result<HashMap<String, String>> {
    let mut values = HashMap::new();
    let Some(data) = install["data"].as_object() else {
        return Ok(values);
    };
    for (key, entry) in data {
        let Some(raw) = entry["client"].as_str().or_else(|| entry.as_str()) else {
            continue;
        };
        let raw = raw.trim();
        let value = if let Some(literal) = raw
            .strip_prefix('\'')
            .and_then(|text| text.strip_suffix('\''))
        {
            literal.to_owned()
        } else if let Some(relative) = raw.strip_prefix('/') {
            let target = stage.join("data").join(safe_relative(relative)?);
            if !target.is_file() {
                let bytes = zip_entry_bytes(installer, relative)?;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
                }
                fs::write(&target, &bytes).map_err(|e| io_error(&target, e))?;
            }
            java_path(&target).to_string_lossy().into_owned()
        } else if raw.starts_with('[') && raw.ends_with(']') {
            let path = processor_library(http, stage, &raw[1..raw.len() - 1], mirror, cancel)?;
            java_path(&path).to_string_lossy().into_owned()
        } else {
            raw.to_owned()
        };
        values.insert(key.clone(), value);
    }
    Ok(values)
}

fn processor_arg(
    raw: &str,
    tokens: &HashMap<String, String>,
    http: &Client,
    stage: &Path,
    mirror: bool,
    cancel: &CancellationToken,
) -> Result<String> {
    let mut text = raw.to_owned();
    for (key, value) in tokens {
        let token = format!("{{{key}}}");
        if text.contains(&token) {
            text = text.replace(&token, value);
        }
    }
    let trimmed = text.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        let path = processor_library(http, stage, &trimmed[1..trimmed.len() - 1], mirror, cancel)?;
        return Ok(java_path(&path).to_string_lossy().into_owned());
    }
    Ok(text)
}

fn jar_main_class(jar: &Path) -> Result<String> {
    let manifest =
        String::from_utf8_lossy(&zip_entry_bytes(jar, "META-INF/MANIFEST.MF")?).into_owned();
    for line in manifest.lines() {
        if let Some(value) = line.strip_prefix("Main-Class:") {
            let value = value.trim();
            if !value.is_empty() {
                return Ok(value.to_owned());
            }
        }
    }
    Err(invalid(format!(
        "processor jar has no Main-Class: {}",
        jar.display()
    )))
}

/// 1.20.1 的 NeoForge 安装器没有 `--installClient`，客户端只能靠它自带的 processors 打补丁。
/// 这里照 install_profile 的描述把每一步 java 起来跑一遍，产物落在 staging 的 libraries 里。
#[allow(clippy::too_many_arguments)]
fn run_processors(
    java: &Path,
    stage: &Path,
    installer: &Path,
    install: &Value,
    game_version: &str,
    profile_id: &str,
    mirror: bool,
    cancel: &CancellationToken,
    progress: &(impl Fn(InstallProgress) + Sync),
) -> Result<()> {
    let steps = processor_steps(install);
    if steps.is_empty() {
        return Err(invalid(
            "installer has neither a client CLI nor client processors",
        ));
    }
    let http = client()?;
    let mut tokens = processor_data(&http, stage, installer, install, mirror, cancel)?;
    let minecraft_jar = stage
        .join("versions")
        .join(game_version)
        .join(format!("{game_version}.jar"));
    if !minecraft_jar.is_file() {
        return Err(invalid(format!(
            "missing vanilla jar for processors: {}",
            minecraft_jar.display()
        )));
    }
    let as_text = |path: &Path| java_path(path).to_string_lossy().into_owned();
    tokens.insert("MINECRAFT_JAR".into(), as_text(&minecraft_jar));
    tokens.insert("MINECRAFT_VERSION".into(), game_version.to_owned());
    tokens.insert("ROOT".into(), as_text(stage));
    tokens.insert("INSTALLER".into(), as_text(installer));
    tokens.insert("SIDE".into(), "client".into());
    tokens.insert("LIBRARY_DIR".into(), as_text(&stage.join("libraries")));
    let total = steps.len() as u64;
    for (index, step) in steps.into_iter().enumerate() {
        let mut classpath = Vec::new();
        for entry in &step.classpath {
            classpath.push(processor_library(&http, stage, entry, mirror, cancel)?);
        }
        let jar = processor_library(&http, stage, &step.jar, mirror, cancel)?;
        if !jar.is_file() {
            return Err(invalid(format!(
                "processor jar is missing: {}",
                jar.display()
            )));
        }
        classpath.push(jar.clone());
        let main = jar_main_class(&jar)?;
        let mut args = Vec::new();
        for raw in &step.args {
            args.push(processor_arg(raw, &tokens, &http, stage, mirror, cancel)?);
        }
        event(
            progress,
            profile_id,
            "modloader-install",
            &format!("processor {}/{total}: {main}", index + 1),
        );
        run_java_classpath(
            java,
            &classpath,
            &main,
            stage,
            stage,
            &args,
            cancel,
            |line| event(progress, profile_id, "modloader-install", line),
        )?;
    }
    if let Some(patched) = tokens.get("PATCHED") {
        let path = PathBuf::from(patched);
        if !path.is_file() {
            return Err(invalid(format!(
                "processors left no patched client jar: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

/// 老安装器只认 `--installClient`；NeoForge 1.20.1 那条线没有，只能看 help 输出判断。
fn installer_supports_client_cli(java: &Path, jar: &Path) -> bool {
    // 输出重定向到文件而不是管道：安装器偶尔会派生子进程，管道读端会一直等下去。
    let probe =
        std::env::temp_dir().join(format!("ybmcl-installer-probe-{}.log", std::process::id()));
    let Ok(file) = fs::File::create(&probe) else {
        return false;
    };
    let Ok(stdout) = file.try_clone() else {
        return false;
    };
    let stderr = file;
    let mut command = Command::new(java_path(java));
    command
        .arg("-Djava.awt.headless=true")
        .arg("-jar")
        .arg(java_path(jar))
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(100));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    let text = fs::read_to_string(&probe).unwrap_or_default();
    let _ = fs::remove_file(&probe);
    text.contains("installClient")
}

/// 拉库文件用的短超时客户端：库动辄几十上百个，单个请求卡 240 s 会把整次安装拖死。
fn library_client() -> Result<&'static Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .user_agent("YBUMCL/0.1")
                .connect_timeout(Duration::from_secs(3))
                // 安装器依赖都是几百 KB 的小文件；单次 10 s 足够，
                // 免得连不上的官方源把三轮预取拖成几十分钟。
                .timeout(Duration::from_secs(10))
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| invalid("cannot build the library http client"))
}

fn prefetch_installer_libraries(
    _http: &Client,
    stage: &Path,
    libraries: &[InstallerLibrary],
    mirror: bool,
    cancel: &CancellationToken,
    emit: &(impl Fn(InstallProgress) + Sync),
) -> Result<()> {
    let http = library_client()?;
    let total = libraries.len() as u64;
    let done = AtomicU64::new(0);
    let mut missing: Vec<&InstallerLibrary> = libraries.iter().collect();
    // 安装器只会去官方 maven 取没预取到的库，而那条线常常连不上；
    // 多跑几轮，直到每个库都落在 staging 里。
    for _pass in 0..3 {
        let pending = std::mem::take(&mut missing);
        let next = AtomicU64::new(0);
        // bmclapi 对高并发比较敏感，8 路常出现长时间挂起；4 路已经够快。
        let workers = pending.len().clamp(1, 4);
        thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| loop {
                    let index = next.fetch_add(1, Ordering::Relaxed) as usize;
                    let Some(library) = pending.get(index) else {
                        return;
                    };
                    if cancel.is_cancelled() {
                        return;
                    }
                    let target = stage.join("libraries").join(&library.path);
                    if fs::metadata(&target).is_ok() {
                        done.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    let source = mirror_maven(&library.url, mirror);
                    let mut fetched = None;
                    // bmclapi 的 maven 镜像最快（一个 jar 不到 1 s）；
                    // maven.neoforged.net 在这边完全连不上，排在最后只浪费时间。
                    let bmcl = format!("https://bmclapi2.bangbang93.com/maven/{}", library.path);
                    for candidate in std::iter::once(bmcl)
                        .chain(std::iter::once(source))
                        .chain(std::iter::once(library.url.clone()))
                    {
                        match fetch_once(http, &candidate, 64 * 1024 * 1024, cancel, &mut |_| {}) {
                            Ok(bytes) if hash_bytes(&bytes) == library.sha1 => {
                                fetched = Some(bytes);
                                break;
                            }
                            Ok(_) => continue,
                            Err(DownloadError::Cancelled) => return,
                            Err(_) => continue,
                        }
                    }
                    let Some(bytes) = fetched else { continue };
                    if let Some(parent) = target.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    if fs::write(&target, &bytes).is_err() {
                        continue;
                    }
                    let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                    emit(InstallProgress {
                        version_id: String::new(),
                        stage: "modloader-libraries".into(),
                        path: library.path.clone(),
                        completed_files: finished.min(total),
                        total_files: total,
                        downloaded_bytes: 0,
                        total_bytes: 0,
                    });
                });
            }
        });
        check(cancel)?;
        missing = pending
            .into_iter()
            .filter(|item| fs::metadata(stage.join("libraries").join(&item.path)).is_err())
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
    }
    let names: Vec<&str> = missing
        .iter()
        .take(3)
        .map(|item| item.path.as_str())
        .collect();
    Err(invalid(format!(
        "安装器依赖拉不下来（{} 这 {} 个库镜像和官方都超时）：{}",
        names.join(" / "),
        missing.len(),
        "可以检查网络后重试"
    )))
}

fn collect(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    no_links(directory)?;
    for entry in fs::read_dir(directory).map_err(|e| io_error(directory, e))? {
        let entry = entry.map_err(|e| io_error(directory, e))?;
        let path = entry.path();
        no_links(&path)?;
        let kind = entry.file_type().map_err(|e| io_error(&path, e))?;
        let relative = path
            .strip_prefix(root)
            .map_err(|_| invalid("Artifact escaped stage"))?;
        safe_relative(&relative.to_string_lossy())?;
        if kind.is_dir() {
            collect(root, &path, files)?;
        } else if kind.is_file() {
            files.push(relative.to_path_buf());
        } else {
            return Err(invalid("Non-regular artifact"));
        }
        if files.len() > 200_000 {
            return Err(invalid("Too many artifacts"));
        }
    }
    Ok(())
}
fn verify_libraries(stage: &Path, metadata: &Value, cancel: &CancellationToken) -> Result<()> {
    for library in metadata["libraries"].as_array().into_iter().flatten() {
        let artifact = &library["downloads"]["artifact"];
        let Some(path) = artifact["path"].as_str() else {
            continue;
        };
        let path = stage.join("libraries").join(safe_relative(path)?);
        no_links(&path)?;
        if path.is_file() {
            if let Some(hash) = artifact["sha1"].as_str().filter(|s| !s.is_empty()) {
                if !valid_hash(hash) || !hash_file(&path, cancel)?.eq_ignore_ascii_case(hash) {
                    return Err(DownloadError::Integrity(path.display().to_string()));
                }
            }
        } else if metadata.get("mainClass").is_some() && crate::rules_allow(library.get("rules"))? {
            return Err(invalid(format!(
                "Missing client library: {}",
                path.display()
            )));
        }
    }
    Ok(())
}
/// 安装器产物：这些位置本来就该用新文件覆盖旧的（PCL 也这么干）。
/// `assets/indexes` 会撞车是因为 stage 里的原版索引和游戏目录里那份可能来自不同
/// 下载源，内容等价但字节不同；`libraries`、`versions` 属于加载器自己写入的
/// 版本文件，覆盖是重装的正常部分。用户文件（mods/saves/options 等）仍然不覆盖。
fn replaceable(relative: &Path) -> bool {
    let mut parts = relative.components();
    match parts.next().and_then(|value| value.as_os_str().to_str()) {
        Some("libraries") | Some("versions") => true,
        Some("assets") => {
            matches!(
                parts.next().and_then(|value| value.as_os_str().to_str()),
                Some("indexes")
            )
        }
        _ => false,
    }
}

fn publish(
    stage: &Path,
    root: &Path,
    files: &[PathBuf],
    cancel: &CancellationToken,
) -> Result<u64> {
    // Preflight ALL conflicts before touching live files; never replace user data.
    for relative in files {
        check(cancel)?;
        let target = root.join(relative);
        no_links(&target)?;
        if replaceable(relative) {
            continue;
        }
        if target.exists()
            && (!target.is_file()
                || hash_file(&target, cancel)? != hash_file(&stage.join(relative), cancel)?)
        {
            return Err(invalid(format!(
                "Refusing to replace conflicting artifact: {}",
                target.display()
            )));
        }
    }
    let mut added = 0;
    for relative in files {
        check(cancel)?;
        let source = stage.join(relative);
        let target = root.join(relative);
        no_links(&target)?;
        let parent = target
            .parent()
            .ok_or_else(|| invalid("Artifact has no parent"))?;
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        no_links(&target)?;
        if replaceable(relative) && target.is_file() {
            // 安装器产物：先删再链，拿到的一定是这次安装写的那份。
            fs::remove_file(&target).map_err(|e| io_error(&target, e))?;
        }
        // Atomic complete-file publication without clobber (unlike POSIX rename).
        match fs::hard_link(&source, &target) {
            Ok(()) => added += 1,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                no_links(&target)?;
                if hash_file(&target, cancel)? != hash_file(&source, cancel)? {
                    return Err(invalid("Artifact changed during publication"));
                }
            }
            Err(e) => return Err(io_error(&target, e).into()),
        }
    }
    Ok(added)
}
/// Returns actual official profile ID. Counts cover vanilla/installer downloads,
/// not Java-internal transfers. HTTP cancellation latency is <=120 seconds.
pub fn install_modloader(
    request: ModLoaderInstallRequest,
    cancel: CancellationToken,
    progress: impl Fn(InstallProgress) + Sync,
) -> Result<InstallSummary> {
    let result = install_inner(&request, &cancel, &progress);
    // 加载器外壳的 json 是安装器（或我们自己）写的，没有 `_ybmcl`；
    // 补上之后实例改名也认得出基础版本与加载器版本，重装才能照原样装回同一套。
    if let Ok(summary) = &result {
        if let Ok(root) = root_path(&request.game_dir) {
            let key = match request.loader {
                ModLoader::Forge => "forge",
                ModLoader::NeoForge => "neoforge",
                ModLoader::Cleanroom => "cleanroom",
                ModLoader::LiteLoader => "liteloader",
                ModLoader::OptiFine => "optifine",
            };
            let _ = crate::merge::stamp_meta(
                &root,
                &summary.version_id,
                &request.game_version,
                &[(key, request.loader_version.as_str())],
            );
        }
    }
    if let Err(error) = &result {
        event(
            &progress,
            &request.game_version,
            if matches!(error, DownloadError::Cancelled) {
                "cancelled"
            } else {
                "failed"
            },
            "",
        );
    }
    result
}
fn install_inner(
    r: &ModLoaderInstallRequest,
    cancel: &CancellationToken,
    progress: &(impl Fn(InstallProgress) + Sync),
) -> Result<InstallSummary> {
    check(cancel)?;
    let java = checked_java(r)?;
    let root = root_path(&r.game_dir)?;
    match r.loader {
        ModLoader::Forge | ModLoader::NeoForge => {
            install_forge_family(r, &root, &java, cancel, progress)
        }
        ModLoader::Cleanroom => install_cleanroom(r, &root, &java, cancel, progress),
        ModLoader::LiteLoader => install_liteloader(r, &root, cancel, progress),
        ModLoader::OptiFine => install_optifine(r, &root, &java, cancel, progress),
    }
}

fn checked_java(r: &ModLoaderInstallRequest) -> Result<PathBuf> {
    let java = PathBuf::from(&r.java_path);
    if !java.is_absolute() || !java.is_file() {
        return Err(invalid("Select an absolute Java executable path"));
    }
    let java = fs::canonicalize(&java).map_err(|e| io_error(&java, e))?;
    // Reuse the core's bounded five-second Java probe before any downloads.
    let runtime = crate::java_version(&java)
        .ok_or_else(|| invalid("Could not determine the selected Java runtime version"))?;
    let mut presence = crate::java::LoaderPresence::default();
    match r.loader {
        ModLoader::OptiFine => presence.optifine = true,
        ModLoader::Cleanroom => presence.cleanroom = Some(&r.loader_version),
        ModLoader::LiteLoader => presence.liteloader = true,
        ModLoader::Forge | ModLoader::NeoForge => presence.forge = Some(&r.loader_version),
    }
    let requirement = crate::java::requirement_with(&r.game_version, None, &presence);
    if !requirement.accepts(runtime.major) {
        return Err(invalid(format!(
            "Minecraft {} 需要 {}，当前选中的是 Java {}",
            r.game_version,
            requirement.describe(),
            runtime.major
        )));
    }
    Ok(java)
}
fn install_forge_family(
    r: &ModLoaderInstallRequest,
    root: &Path,
    java: &Path,
    cancel: &CancellationToken,
    progress: &(impl Fn(InstallProgress) + Sync),
) -> Result<InstallSummary> {
    let url = installer_url(r)?;
    let stage = Stage::new(root)?;
    let http = client()?;
    event(progress, &r.game_version, "modloader-installer", "");
    let expected = match fetch(&http, &format!("{url}.sha1"), 1024, cancel) {
        Ok(body) => Some(checksum(&body)?),
        Err(DownloadError::Cancelled) => return Err(DownloadError::Cancelled),
        Err(_) => None,
    };
    // 安装器 jar 走下载层：有镜像路由时按设置里的策略竞速/优先，没镜像就只走官方。
    let bytes = crate::download::fetch_file_bytes(&url, cancel)?;
    if let Some(expected) = expected {
        if hash_bytes(&bytes) != expected {
            return Err(DownloadError::Integrity(
                "Official installer SHA1 mismatch".into(),
            ));
        }
    }
    let installer_bytes = bytes.len() as u64;
    let jar = stage.0.join("installer.jar");
    write_new(&jar, &bytes)?;
    let profile = inspect_installer(&jar, r)?;
    event(progress, &profile.id, "modloader-prepare", "");
    let mut summary = crate::download::install_version_with_cache(
        InstallRequest {
            game_dir: stage.0.to_string_lossy().into(),
            version_id: r.game_version.clone(),
            fabric_loader: None,
            quilt_loader: None,
            strategy: r.strategy,
            threads: r.threads,
        },
        cancel.clone(),
        |mut p| {
            p.version_id = profile.id.clone();
            if p.stage == "complete" {
                p.stage = "modloader-prepare".into();
            }
            progress(p);
        },
        root,
    )?;
    let relative = PathBuf::from(format!("versions/{0}/{0}.json", profile.id));
    let path = stage.0.join(&relative);
    if profile.headless {
        write_new(
            &stage.0.join("launcher_profiles.json"),
            b"{\"profiles\":{}}",
        )?;
        event(
            progress,
            &profile.id,
            "modloader-install",
            "Running official headless CLIENT installer",
        );
        // 安装器自己还会去官方 maven 取「安装后版本」里声明的库
        // （NeoForge 的 cpw.mods:bootstraplauncher 就在 version.json 里，不在 install_profile 里），
        // 那条线连不上就直接失败，所以两份清单一起预取。
        let mut preload = profile.libraries.clone();
        preload.extend(installer_libraries(&profile.version));
        prefetch_installer_libraries(
            &http,
            &stage.0,
            &preload,
            r.strategy.allows_mirror(),
            cancel,
            progress,
        )?;
        if installer_supports_client_cli(java, &jar) {
            run_installer(
                java,
                &jar,
                &stage.0,
                &stage.0,
                &[
                    "--installClient".into(),
                    java_path(&stage.0).to_string_lossy().into(),
                ],
                None,
                cancel,
                |line| event(progress, &profile.id, "modloader-install", line),
            )?;
        } else {
            // 这条线（NeoForge 1.20.1）的安装器只留了 GUI，客户端补丁得自己按 install_profile 跑。
            event(
                progress,
                &profile.id,
                "modloader-install",
                "Installer has no client CLI; running its processors",
            );
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
            }
            let serialized =
                serde_json::to_vec_pretty(&profile.version).map_err(|e| invalid(e.to_string()))?;
            write_new(&path, &serialized)?;
            // 版本 json 与它的库都由上面的 preload/prefetch 准备好了；
            // 这里只差按 install_profile 把客户端补丁跑出来。
            run_processors(
                java,
                &stage.0,
                &jar,
                &profile.install,
                &r.game_version,
                &profile.id,
                r.strategy.allows_mirror(),
                cancel,
                progress,
            )?;
        }
    } else {
        event(
            progress,
            &profile.id,
            "modloader-install",
            "Installing bundled loader profile",
        );
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        }
        let serialized =
            serde_json::to_vec_pretty(&profile.version).map_err(|e| invalid(e.to_string()))?;
        write_new(&path, &serialized)?;
        crate::download::install_version_with_cache(
            InstallRequest {
                game_dir: stage.0.to_string_lossy().into(),
                version_id: profile.id.clone(),
                fabric_loader: None,
                quilt_loader: None,
                strategy: r.strategy,
                threads: r.threads,
            },
            cancel.clone(),
            |mut p| {
                if p.stage == "complete" {
                    p.stage = "modloader-prepare".into();
                }
                progress(p);
            },
            root,
        )?;
    }
    event(progress, &profile.id, "modloader-verify", "");
    no_links(&path)?;
    let bytes = fs::read(&path).map_err(|e| io_error(&path, e))?;
    if bytes.len() as u64 > META_LIMIT || json(&bytes)? != profile.version {
        return Err(invalid(
            "Installed JSON differs from verified installer profile",
        ));
    }
    verify_libraries(&stage.0, &profile.version, cancel)?;
    verify_libraries(&stage.0, &profile.install, cancel)?;
    let mut files = Vec::new();
    for directory in ["libraries", "assets", "resources"] {
        let path = stage.0.join(directory);
        if path.exists() {
            collect(&stage.0, &path, &mut files)?;
        }
    }
    for id in [&r.game_version, &profile.id] {
        collect(&stage.0, &stage.0.join("versions").join(id), &mut files)?;
    }
    files.sort();
    files.retain(|p| p != &relative);
    files.push(relative);
    event(progress, &profile.id, "modloader-publish", "");
    publish(&stage.0, root, &files, cancel)?;
    summary.version_id = profile.id;
    summary.downloaded_files += 1;
    summary.downloaded_bytes += installer_bytes;
    progress(InstallProgress {
        version_id: summary.version_id.clone(),
        stage: "complete".into(),
        path: String::new(),
        completed_files: summary.downloaded_files + summary.skipped_files,
        total_files: summary.downloaded_files + summary.skipped_files,
        downloaded_bytes: summary.downloaded_bytes,
        total_bytes: summary.downloaded_bytes,
    });
    Ok(summary)
}

/// Cleanroom 的安装器不吃 `--installClient`，而是 `client --version <版本> -d <目录> -y`。
fn install_cleanroom(
    r: &ModLoaderInstallRequest,
    root: &Path,
    java: &Path,
    cancel: &CancellationToken,
    progress: &(impl Fn(InstallProgress) + Sync),
) -> Result<InstallSummary> {
    let package = loader_package(r)?;
    let stage = Stage::new(root)?;
    let http = client()?;
    event(progress, &r.game_version, "modloader-installer", "");
    // 优先 maven 上的同一份文件，其次 GitHub 加速代理，最后官方直连。
    let mut candidates = Vec::new();
    if let Some(mirror) = cleanroom_mirror(&r.loader_version) {
        candidates.push(mirror);
    }
    if let Some(proxy) = mirrored_installer(&package.url) {
        candidates.push(proxy);
    }
    candidates.push(package.url.clone());
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|url| seen.insert(url.clone()));
    let mut bytes = None;
    let last = candidates.len() - 1;
    for (index, candidate) in candidates.iter().enumerate() {
        match fetch_progress(&http, candidate, 64 * 1024 * 1024, cancel, |done| {
            event(
                progress,
                &r.game_version,
                "modloader-installer",
                &format!("下载官方安装器 · {:.1} MB", done as f64 / (1024.0 * 1024.0)),
            )
        }) {
            Ok(fetched) => {
                bytes = Some(fetched);
                break;
            }
            Err(DownloadError::Cancelled) => return Err(DownloadError::Cancelled),
            Err(error) if index < last => {
                let _ = error;
            }
            Err(error) => return Err(error),
        }
    }
    let bytes = bytes.expect("installer download attempted");
    let jar = stage.0.join("installer.jar");
    write_new(&jar, &bytes)?;
    // Cleanroom 的安装器自带完整版本 json（原版客户端 + 库都按自己的清单拉），
    // 不需要（也不需要等待）先装一遍原版，直接把 json 里的资产和库下载进暂存目录。
    let license = stage.0.join("license.txt");
    write_new(&license, b"Cleanroom installer")?;
    progress(InstallProgress {
        version_id: profile_label(r),
        stage: "modloader-prepare".into(),
        path: String::new(),
        completed_files: 0,
        total_files: 0,
        downloaded_bytes: 0,
        total_bytes: 0,
    });
    let mut summary = InstallSummary {
        version_id: profile_label(r),
        // 加载器安装器的产出由它自己决定落点，这里不给界面可打开/可删的路径。
        path: String::new(),
        downloaded_files: 0,
        skipped_files: 0,
        downloaded_bytes: 0,
    };
    event(
        progress,
        &profile_label(r),
        "modloader-install",
        "Running Cleanroom client installer",
    );
    let target = java_path(&stage.0);
    run_installer(
        java,
        &jar,
        &stage.0,
        &stage.0,
        &[
            "client".into(),
            "-d".into(),
            target.to_string_lossy().into(),
            "-y".into(),
        ],
        None,
        cancel,
        |line| event(progress, &profile_label(r), "modloader-install", line),
    )?;
    let label = profile_label(r);
    let profile = stage.0.join(format!("versions/{label}/{label}.json"));
    if profile.is_file() {
        summary = crate::download::install_profile_assets(
            &stage.0.to_string_lossy(),
            &label,
            r.strategy,
            r.threads.map(|n| n as usize),
            cancel.clone(),
            progress,
        )?;
    }
    finish_install(r, root, &stage, cancel, progress, &mut summary, &label)
}

/// LiteLoader 没有能用官方安装器走的通道：清单里只有快照 jar，里面已经是改过的版本。
fn install_liteloader(
    r: &ModLoaderInstallRequest,
    root: &Path,
    cancel: &CancellationToken,
    progress: &(impl Fn(InstallProgress) + Sync),
) -> Result<InstallSummary> {
    let package = loader_package(r)?;
    let jar_path = package
        .path
        .ok_or_else(|| invalid("LiteLoader package has no library path"))?;
    let id = format!("{}-LiteLoader", r.game_version);
    let mut summary = write_profile_loader(
        r,
        root,
        cancel,
        progress,
        &id,
        &package.url,
        &jar_path,
        liteloader_profile(&id, r)?,
    )?;
    // LiteLoader 只用旧式 launchwrapper 启动，profile 里的 launchwrapper / asm-all
    // 没有 downloads 描述，安装器只写 json 不会真的下载它们，缺了启动计划直接失败。
    summary.downloaded_bytes += fetch_legacy_libraries(root, &id, cancel, progress)?;
    summary.version_id = id;
    Ok(summary)
}

/// 补齐只有 maven 坐标、没有 downloads 的老库（launchwrapper、asm-all）。
fn fetch_legacy_libraries(
    root: &Path,
    id: &str,
    cancel: &CancellationToken,
    progress: &(impl Fn(InstallProgress) + Sync),
) -> Result<u64> {
    let http = client()?;
    let mut bytes = 0u64;
    for coordinate in [
        "net.minecraft:launchwrapper:1.12",
        "org.ow2.asm:asm-all:5.2",
    ] {
        let relative = crate::maven_path(coordinate)?;
        let target = root.join("libraries").join(&relative);
        if target.is_file() {
            continue;
        }
        let url = format!("https://bmclapi2.bangbang93.com/maven/{relative}");
        event(progress, id, "modloader-installer", "");
        let data = fetch_progress(&http, &url, 8 * 1024 * 1024, cancel, |done| {
            event(
                progress,
                id,
                "modloader-installer",
                &format!("下载 {coordinate} · {done} B"),
            )
        })?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        }
        write_new(&target, &data)?;
        bytes += data.len() as u64;
    }
    Ok(bytes)
}

/// OptiFine 的安装器是个 Swing 程序，没有无界面参数（-Djava.awt.headless 只会让它抛
/// HeadlessException）。照 HMCL 的做法：直接跑安装器里的 `optifine.Patcher` 打出补丁 jar，
/// 再自己写一份 launchwrapper + tweakClass 的版本 json。
/// OptiFine 官网要先打开 adloadx 页面拿到 downloadx 令牌，再带 Referer 取 jar。
/// 镜像站现在只有 25 KB/s 左右，官网这条能跑到 1.5 MB/s，所以官网优先、失败再退回镜像。
fn optifine_jar_bytes(
    installer_url: &str,
    http: &Client,
    cancel: &CancellationToken,
) -> Result<Option<Vec<u8>>> {
    let name = installer_url.rsplit('/').next().unwrap_or_default();
    if !name.starts_with("OptiFine_") || !name.ends_with(".jar") {
        return Ok(None);
    }
    // 官网偶尔长时间不回包，这条链路单独设超时，失败就退回镜像。
    // 45 s 是实测值：官网从国内经常要 20-40 s 才回第一个字节，25 s 会误判成失败。
    let short = std::time::Duration::from_secs(45);
    let page = format!("https://optifine.net/adloadx?f={name}");
    let Ok(response) = http
        .get(&page)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .timeout(short)
        .send()
    else {
        return Ok(None);
    };
    let Ok(html) = response.text() else {
        return Ok(None);
    };
    let Some(rest) = html.split("downloadx?").nth(1) else {
        return Ok(None);
    };
    let query: String = rest
        .chars()
        .take_while(|c| !matches!(c, '"' | '\'' | '<' | ' ' | '\n'))
        .collect();
    if query.is_empty() {
        return Ok(None);
    }
    if cancel.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }
    let url = format!("https://optifine.net/downloadx?{query}");
    let Ok(response) = http
        .get(&url)
        .header("Referer", &page)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .timeout(std::time::Duration::from_secs(180))
        .send()
    else {
        return Ok(None);
    };
    let Ok(bytes) = response.bytes() else {
        return Ok(None);
    };
    if !bytes.starts_with(b"PK") {
        return Ok(None);
    }
    Ok(Some(bytes.to_vec()))
}

/// OptiFine 安装器有两条来源：官网 `adloadx` 页（慢，但版本常新）和 BMCLAPI 镜像。
/// 按下载策略决定「竞速 / 谁先谁后」：竞速时两边同时开，谁先给出完整 jar 用谁，
/// 另一边立刻取消；串行时按策略顺序试，前一条拿不到再换另一条。
fn optifine_installer_bytes(
    strategy: crate::download::DownloadStrategy,
    mirrored: &str,
    http: &Client,
    cancel: &CancellationToken,
    progress: &(impl Fn(InstallProgress) + Sync),
    game: &str,
) -> Result<Vec<u8>> {
    use crate::download::DownloadStrategy;
    use std::sync::mpsc;

    let note = |text: &str| event(progress, game, "modloader-installer", text);
    let official = |token: &CancellationToken| optifine_jar_bytes(mirrored, http, token);
    let mirror = |token: &CancellationToken| -> Result<Option<Vec<u8>>> {
        match fetch_progress(http, mirrored, 64 * 1024 * 1024, token, |_| {}) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(DownloadError::Cancelled) => Err(DownloadError::Cancelled),
            // 403/超时/重定向失败都只说明「这条来源这次不行」，交给另一条。
            Err(_) => Ok(None),
        }
    };
    let missing = |text: &str| DownloadError::Invalid(text.into());

    match strategy {
        DownloadStrategy::OfficialOnly => match official(cancel)? {
            Some(bytes) => Ok(bytes),
            None => Err(missing(
                "OptiFine 官网没取到安装器，当前策略没开镜像下载",
            )),
        },
        DownloadStrategy::MirrorOnly => match mirror(cancel)? {
            Some(bytes) => Ok(bytes),
            None => Err(missing(
                "OptiFine 镜像没取到安装器，当前策略没开官方源下载",
            )),
        },
        DownloadStrategy::OfficialFirst => {
            if let Some(bytes) = official(cancel)? {
                return Ok(bytes);
            }
            note("OptiFine 官网没取到安装器，改从镜像下载（较慢）");
            mirror(cancel)?.ok_or_else(|| missing("OptiFine 安装器两边都没取到"))
        }
        DownloadStrategy::MirrorFirst => {
            if let Some(bytes) = mirror(cancel)? {
                return Ok(bytes);
            }
            note("OptiFine 镜像没取到安装器，改从官网下载（较慢）");
            official(cancel)?.ok_or_else(|| missing("OptiFine 安装器两边都没取到"))
        }
        DownloadStrategy::Race => {
            let official_token = CancellationToken::default();
            let mirror_token = CancellationToken::default();
            let (tx, rx) = mpsc::channel::<(&'static str, Result<Option<Vec<u8>>>)>();
            thread::scope(|scope| {
                {
                    let tx = tx.clone();
                    let token = official_token.clone();
                    scope.spawn(move || {
                        let out = optifine_jar_bytes(mirrored, http, &token);
                        let _ = tx.send(("官网", out));
                    });
                }
                {
                    let tx = tx.clone();
                    let token = mirror_token.clone();
                    scope.spawn(move || {
                        let out = match fetch_progress(http, mirrored, 64 * 1024 * 1024, &token, |_| {
                        }) {
                            Ok(bytes) => Ok(Some(bytes)),
                            Err(DownloadError::Cancelled) => Err(DownloadError::Cancelled),
                            Err(_) => Ok(None),
                        };
                        let _ = tx.send(("镜像", out));
                    });
                }
                drop(tx);
                let mut reason = String::from("OptiFine 安装器两边都没取到");
                for _ in 0..2 {
                    loop {
                        if cancel.is_cancelled() {
                            official_token.cancel();
                            mirror_token.cancel();
                            return Err(DownloadError::Cancelled);
                        }
                        match rx.recv_timeout(Duration::from_millis(120)) {
                            Ok((_, Ok(Some(bytes)))) => {
                                // 另一边还在下，直接掐掉，别让用户白等带宽。
                                official_token.cancel();
                                mirror_token.cancel();
                                return Ok(bytes);
                            }
                            Ok((which, Ok(None))) => {
                                reason = format!("{which}没取到安装器");
                                break;
                            }
                            Ok((which, Err(error))) => {
                                reason = format!("{which}: {error}");
                                break;
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                return Err(DownloadError::Invalid(reason));
                            }
                        }
                    }
                }
                Err(DownloadError::Invalid(reason))
            })
        }
    }
}

fn install_optifine(
    r: &ModLoaderInstallRequest,
    root: &Path,
    java: &Path,
    cancel: &CancellationToken,
    progress: &(impl Fn(InstallProgress) + Sync),
) -> Result<InstallSummary> {
    let package = loader_package(r)?;
    let stage = Stage::new(root)?;
    let http = client()?;
    let id = profile_label(r);
    event(progress, &r.game_version, "modloader-installer", "");
    let bytes = optifine_installer_bytes(r.strategy, &package.url, &http, cancel, progress, &r.game_version)?;
    event(
        progress,
        &r.game_version,
        "modloader-installer",
        &format!(
            "下载 OptiFine 安装器 · {:.1} MB",
            bytes.len() as f64 / (1024.0 * 1024.0)
        ),
    );
    let jar = stage.0.join("OptiFine-installer.jar");
    write_new(&jar, &bytes)?;
    event(progress, &id, "modloader-prepare", "");
    let mut summary = crate::download::install_version_with_cache(
        InstallRequest {
            game_dir: stage.0.to_string_lossy().into(),
            version_id: r.game_version.clone(),
            fabric_loader: None,
            quilt_loader: None,
            strategy: r.strategy,
            threads: r.threads,
        },
        cancel.clone(),
        |mut p| {
            if p.stage == "complete" {
                p.stage = "modloader-prepare".into();
            }
            progress(p);
        },
        root,
    )?;
    let base_jar = stage
        .0
        .join("versions")
        .join(&r.game_version)
        .join(format!("{}.jar", r.game_version));
    no_links(&base_jar)?;
    if !base_jar.is_file() {
        return Err(invalid("OptiFine 需要原版客户端 jar，请先安装原版"));
    }
    let listing = optifine_listing(&jar)?;
    let maven_version = format!("{}_{}", r.game_version, r.loader_version);
    let optifine_name = format!("optifine:OptiFine:{maven_version}");
    let patched = stage
        .0
        .join("libraries")
        .join(crate::maven_path(&optifine_name)?.as_str());
    if let Some(parent) = patched.parent() {
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    if listing.patcher {
        event(
            progress,
            &id,
            "modloader-install",
            "Running OptiFine patcher",
        );
        run_java(
            java,
            &jar,
            Some("optifine.Patcher"),
            &stage.0,
            &stage.0,
            &[
                java_path(&base_jar).to_string_lossy().into(),
                java_path(&jar).to_string_lossy().into(),
                java_path(&patched).to_string_lossy().into(),
            ],
            None,
            cancel,
            |line| event(progress, &id, "modloader-install", line),
        )?;
    } else {
        // 很老的安装器没有 Patcher 类，jar 本身就是补丁产物。
        write_new(&patched, &bytes)?;
    }
    if !patched.is_file() {
        return Err(invalid("OptiFine 补丁没有生成"));
    }
    let mut libraries = vec![serde_json::json!({ "name": optifine_name })];
    match &listing.launchwrapper {
        Some((entry, name)) => {
            let target = stage
                .0
                .join("libraries")
                .join(crate::maven_path(name)?.as_str());
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
            }
            let data = zip_entry(&jar, entry)?
                .ok_or_else(|| invalid("OptiFine 安装器里的 launchwrapper 读不出来"))?;
            write_new(&target, &data)?;
            libraries.push(serde_json::json!({ "name": name }));
        }
        None => libraries.push(serde_json::json!({ "name": "net.minecraft:launchwrapper:1.12" })),
    }
    let profile = serde_json::json!({
        "id": id,
        "inheritsFrom": r.game_version,
        "type": "release",
        "mainClass": "net.minecraft.launchwrapper.Launch",
        "libraries": libraries,
        "arguments": {"game": ["--tweakClass", "optifine.OptiFineTweaker"]},
    });
    let relative = PathBuf::from(format!("versions/{id}/{id}.json"));
    let json_path = stage.0.join(&relative);
    if let Some(parent) = json_path.parent() {
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    let serialized = serde_json::to_vec_pretty(&profile).map_err(|e| invalid(e.to_string()))?;
    write_new(&json_path, &serialized)?;
    event(progress, &id, "modloader-publish", "");
    finish_install(r, root, &stage, cancel, progress, &mut summary, &id)
}

struct OptifineListing {
    /// 安装器里有 `optifine/Patcher.class`，能直接打补丁。
    patcher: bool,
    /// 安装器自带的 launchwrapper（老版本是 `launchwrapper-2.0.jar`，
    /// 新版本是 `launchwrapper-of-<ver>.jar`），返回 (jar 内条目名, Maven 坐标)。
    launchwrapper: Option<(String, String)>,
}

fn optifine_listing(path: &Path) -> Result<OptifineListing> {
    let zip = zip::ZipArchive::new(File::open(path).map_err(|e| io_error(path, e))?)
        .map_err(|e| invalid(e.to_string()))?;
    let names: Vec<String> = zip.file_names().map(str::to_owned).collect();
    let patcher = names.iter().any(|name| name == "optifine/Patcher.class");
    let mut launchwrapper = None;
    if let Some(entry) = names
        .iter()
        .find(|name| name.starts_with("launchwrapper-of-") && name.ends_with(".jar"))
    {
        let version = entry
            .trim_start_matches("launchwrapper-of-")
            .trim_end_matches(".jar");
        if safe_component(version) {
            launchwrapper = Some((
                entry.clone(),
                format!("optifine:launchwrapper-of:{version}"),
            ));
        }
    } else if names.iter().any(|name| name == "launchwrapper-2.0.jar") {
        launchwrapper = Some((
            "launchwrapper-2.0.jar".to_owned(),
            "optifine:launchwrapper:2.0".to_owned(),
        ));
    }
    Ok(OptifineListing {
        patcher,
        launchwrapper,
    })
}

fn zip_entry(path: &Path, name: &str) -> Result<Option<Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(File::open(path).map_err(|e| io_error(path, e))?)
        .map_err(|e| invalid(e.to_string()))?;
    let mut entry = match zip.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(invalid(e.to_string())),
    };
    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|e| invalid(e.to_string()))?;
    Ok(Some(bytes))
}

fn profile_label(r: &ModLoaderInstallRequest) -> String {
    match r.loader {
        ModLoader::Cleanroom => format!("Cleanroom-{}", r.loader_version),
        ModLoader::OptiFine => format!("{}-OptiFine_{}", r.game_version, r.loader_version),
        ModLoader::LiteLoader => format!("{}-LiteLoader", r.game_version),
        ModLoader::Forge | ModLoader::NeoForge => r.loader_version.clone(),
    }
}

fn finish_install(
    r: &ModLoaderInstallRequest,
    root: &Path,
    stage: &Stage,
    cancel: &CancellationToken,
    progress: &(impl Fn(InstallProgress) + Sync),
    summary: &mut InstallSummary,
    id: &str,
) -> Result<InstallSummary> {
    let relative = PathBuf::from(format!("versions/{id}/{id}.json"));
    let path = stage.0.join(&relative);
    no_links(&path)?;
    if !path.is_file() {
        return Err(invalid(format!(
            "安装器没有写出 versions/{id}/{id}.json，可能是该版本不适用于这个 Minecraft"
        )));
    }
    let value = {
        let bytes = fs::read(&path).map_err(|e| io_error(&path, e))?;
        json(&bytes)?
    };
    // Forge 系写出的是增量 profile（必须 inheritsFrom）；Cleanroom 的安装器写的是
    // 自带 assetIndex / downloads 的完整 profile，两种都接受。
    let inherits = value["inheritsFrom"].as_str();
    let standalone = inherits.is_none() && value.get("assetIndex").is_some();
    if inherits != Some(r.game_version.as_str()) && !standalone {
        return Err(invalid("安装器写出的版本不是基于所选 Minecraft 版本"));
    }
    verify_libraries(&stage.0, &value, cancel)?;
    let mut files = Vec::new();
    for directory in ["libraries", "assets", "resources"] {
        let directory = stage.0.join(directory);
        if directory.exists() {
            collect(&stage.0, &directory, &mut files)?;
        }
    }
    for dir in [&r.game_version, &id.to_owned()] {
        let directory = stage.0.join("versions").join(dir);
        if directory.exists() {
            collect(&stage.0, &directory, &mut files)?;
        }
    }
    let rel = relative.clone();
    files.sort();
    files.retain(|p| p != &rel);
    files.push(rel);
    event(progress, id, "modloader-publish", "");
    publish(&stage.0, root, &files, cancel)?;
    summary.version_id = id.to_owned();
    progress(InstallProgress {
        version_id: summary.version_id.clone(),
        stage: "complete".into(),
        path: String::new(),
        completed_files: summary.downloaded_files + summary.skipped_files,
        total_files: summary.downloaded_files + summary.skipped_files,
        downloaded_bytes: summary.downloaded_bytes,
        total_bytes: summary.downloaded_bytes,
    });
    Ok(summary.clone())
}

/// LiteLoader 的版本 json：在 launchwrapper 里挂一个 tweakClass。
fn liteloader_profile(id: &str, r: &ModLoaderInstallRequest) -> Result<Value> {
    let game = crate::loader_catalog::liteloader_game(&r.game_version)
        .ok_or_else(|| invalid("LiteLoader does not support this Minecraft version"))?;
    Ok(serde_json::json!({
        "id": id,
        "inheritsFrom": r.game_version,
        "type": "release",
        "mainClass": "net.minecraft.launchwrapper.Launch",
        "libraries": [
            {"name": format!("com.mumfrey:liteloader:{game}-SNAPSHOT"), "url": "https://bmclapi2.bangbang93.com/maven/"},
            {"name": "net.minecraft:launchwrapper:1.12"},
            {"name": "org.ow2.asm:asm-all:5.2"},
        ],
        // 只补 tweakClass：继承的原版 1.12.2 已经带齐 --username/--gameDir 这些参数，
        // 再写一遍会让 launchwrapper 报 MultipleArgumentsForOptionException。
        "minecraftArguments": "--tweakClass com.mumfrey.liteloader.launch.LiteLoaderTweaker",
    }))
}

#[allow(clippy::too_many_arguments)]
fn write_profile_loader(
    r: &ModLoaderInstallRequest,
    root: &Path,
    cancel: &CancellationToken,
    progress: &(impl Fn(InstallProgress) + Sync),
    id: &str,
    package_url: &str,
    package_path: &str,
    profile: Value,
) -> Result<InstallSummary> {
    if !safe_component(id) || id == r.game_version {
        return Err(invalid("Unsafe loader version id"));
    }
    let stage = Stage::new(root)?;
    let http = client()?;
    event(progress, &r.game_version, "modloader-installer", "");
    let bytes = fetch_progress(&http, package_url, 64 * 1024 * 1024, cancel, |done| {
        event(
            progress,
            id,
            "modloader-installer",
            &format!("下载模组加载器 · {:.1} MB", done as f64 / (1024.0 * 1024.0)),
        )
    })?;
    event(progress, id, "modloader-prepare", "");
    let mut summary = crate::download::install_version_with_cache(
        InstallRequest {
            game_dir: stage.0.to_string_lossy().into(),
            version_id: r.game_version.clone(),
            fabric_loader: None,
            quilt_loader: None,
            strategy: r.strategy,
            threads: r.threads,
        },
        cancel.clone(),
        |mut p| {
            if p.stage == "complete" {
                p.stage = "modloader-prepare".into();
            }
            progress(p);
        },
        root,
    )?;
    let target = stage.0.join("libraries").join(safe_relative(package_path)?);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    write_new(&target, &bytes)?;
    summary.downloaded_files += 1;
    summary.downloaded_bytes += bytes.len() as u64;
    let relative = PathBuf::from(format!("versions/{id}/{id}.json"));
    let json_path = stage.0.join(&relative);
    if let Some(parent) = json_path.parent() {
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    let serialized = serde_json::to_vec_pretty(&profile).map_err(|e| invalid(e.to_string()))?;
    write_new(&json_path, &serialized)?;
    event(progress, id, "modloader-verify", "");
    verify_libraries(&stage.0, &profile, cancel)?;
    let mut files = Vec::new();
    for directory in ["libraries", "assets", "resources"] {
        let directory = stage.0.join(directory);
        if directory.exists() {
            collect(&stage.0, &directory, &mut files)?;
        }
    }
    for dir in [&r.game_version, &id.to_owned()] {
        let directory = stage.0.join("versions").join(dir);
        if directory.exists() {
            collect(&stage.0, &directory, &mut files)?;
        }
    }
    files.sort();
    files.retain(|p| p != &relative);
    files.push(relative);
    summary.version_id = id.to_owned();
    event(progress, id, "modloader-publish", "");
    publish(&stage.0, root, &files, cancel)?;
    progress(InstallProgress {
        version_id: id.to_owned(),
        stage: "complete".into(),
        path: String::new(),
        completed_files: summary.downloaded_files + summary.skipped_files,
        total_files: summary.downloaded_files + summary.skipped_files,
        downloaded_bytes: summary.downloaded_bytes,
        total_bytes: summary.downloaded_bytes,
    });
    Ok(summary)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installer_libraries_read_the_installed_version_json() {
        // NeoForge 的 bootstraplauncher 只出现在 version.json 里，不在 install_profile 里；
        // 安装器会自己按它的 url 去官方 maven 取，所以这份清单要一起预取到镜像里。
        let version = serde_json::json!({
            "libraries": [
                {"name": "cpw.mods:bootstraplauncher:2.0.2", "downloads": {"artifact": {
                    "path": "cpw/mods/bootstraplauncher/2.0.2/bootstraplauncher-2.0.2.jar",
                    "url": "https://maven.neoforged.net/releases/cpw/mods/bootstraplauncher/2.0.2/bootstraplauncher-2.0.2.jar",
                    "sha1": "0123456789abcdef0123456789abcdef01234567"
                }}},
                {"name": "broken", "downloads": {"artifact": {
                    "path": "../escape.jar",
                    "url": "https://example.com/escape.jar",
                    "sha1": "0123456789abcdef0123456789abcdef01234567"
                }}},
                {"name": "no-hash", "downloads": {"artifact": {
                    "path": "a/b.jar",
                    "url": "https://example.com/b.jar",
                    "sha1": "not-a-hash"
                }}}
            ]
        });
        let libraries = installer_libraries(&version);
        assert_eq!(libraries.len(), 1);
        assert_eq!(
            libraries[0].path,
            "cpw/mods/bootstraplauncher/2.0.2/bootstraplauncher-2.0.2.jar"
        );
    }

    #[test]
    fn publish_replaces_installer_artifacts_but_not_user_files() {
        use std::fs;
        let base = std::env::temp_dir().join(format!(
            "ybumcl-publish-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let stage = base.join("stage");
        let root = base.join("root");
        for path in [
            stage.join("assets/indexes/5.json"),
            root.join("assets/indexes/5.json"),
            stage.join("libraries/a/b.jar"),
            root.join("libraries/a/b.jar"),
            stage.join("versions/x/x.json"),
            root.join("versions/x/x.json"),
            stage.join("mods/local.jar"),
            root.join("mods/local.jar"),
        ] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
        }
        for (path, data) in [
            (stage.join("assets/indexes/5.json"), "from-mirror"),
            (root.join("assets/indexes/5.json"), "from-official"),
            (stage.join("libraries/a/b.jar"), "new-library"),
            (root.join("libraries/a/b.jar"), "old-library"),
            (stage.join("versions/x/x.json"), "new-profile"),
            (root.join("versions/x/x.json"), "old-profile"),
            (stage.join("mods/local.jar"), "installer-jar"),
            (root.join("mods/local.jar"), "user-jar"),
        ] {
            fs::write(path, data).unwrap();
        }
        let cancel = CancellationToken::default();
        let files = vec![
            PathBuf::from("assets/indexes/5.json"),
            PathBuf::from("libraries/a/b.jar"),
            PathBuf::from("versions/x/x.json"),
        ];
        let added = publish(&stage, &root, &files, &cancel).unwrap();
        assert_eq!(added, 3);
        for (path, want) in [
            (root.join("assets/indexes/5.json"), "from-mirror"),
            (root.join("libraries/a/b.jar"), "new-library"),
            (root.join("versions/x/x.json"), "new-profile"),
        ] {
            assert_eq!(fs::read_to_string(path).unwrap(), want);
        }
        // 用户自己的东西（mods 等）仍然不许覆盖。
        let conflict = publish(&stage, &root, &[PathBuf::from("mods/local.jar")], &cancel);
        assert!(conflict.is_err());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn only_forge_family_strips_the_game_prefix() {
        assert_eq!(
            strip_game_prefix(ModLoader::Forge, "1.20.1", "1.20.1-47.4.9".to_owned()),
            "47.4.9"
        );
        assert_eq!(
            strip_game_prefix(ModLoader::NeoForge, "1.21.1", "1.21.1-21.1.99".to_owned()),
            "21.1.99"
        );
        // LiteLoader 只发快照，`1.12.2-SNAPSHOT` 本身就是完整版本号。
        assert_eq!(
            strip_game_prefix(
                ModLoader::LiteLoader,
                "1.12.2",
                "1.12.2-SNAPSHOT".to_owned()
            ),
            "1.12.2-SNAPSHOT"
        );
        assert_eq!(
            strip_game_prefix(ModLoader::OptiFine, "1.20.1", "HD_U_I6".to_owned()),
            "HD_U_I6"
        );
    }

    #[test]
    fn cleanroom_installer_mirror_follows_the_version() {
        assert_eq!(
            cleanroom_mirror("0.5.4-alpha").as_deref(),
            Some("https://nexus.jsdu.cn/repository/com/cleanroommc/cleanroom/0.5.4-alpha/cleanroom-0.5.4-alpha-installer.jar")
        );
        // 版本号来自远端清单，落盘前必须挡住路径穿越。
        assert_eq!(cleanroom_mirror("../installer"), None);
        assert_eq!(cleanroom_mirror(""), None);
    }
    fn fixture() -> Stage {
        Stage::new(&std::env::temp_dir()).unwrap()
    }
    #[cfg(windows)]
    #[test]
    fn java_paths_remove_verbatim_prefix() {
        assert_eq!(
            java_path(Path::new(r"\\?\C:\game\installer.jar")),
            PathBuf::from(r"C:\game\installer.jar")
        );
        assert_eq!(
            java_path(Path::new(r"\\?\UNC\server\share\game")),
            PathBuf::from(r"\\server\share\game")
        );
    }
    #[test]
    #[ignore = "requires YBUMCL_TEST_JAVA pointing to a local Java executable; no network"]
    fn real_java_startup_failure_is_visible() {
        let java =
            PathBuf::from(std::env::var_os("YBUMCL_TEST_JAVA").expect("set YBUMCL_TEST_JAVA"));
        let stage = fixture();
        let root = fs::canonicalize(&stage.0).unwrap();
        let error = run_installer(
            &java,
            &root.join("missing.jar"),
            &root,
            &root,
            &["--installClient".into()],
            None,
            &CancellationToken::default(),
            |_| {},
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("Unable to access jarfile"), "{error}");
        assert!(error.contains("Java console:"), "{error}");
        let jar_stage = fixture();
        let root = fs::canonicalize(&jar_stage.0).unwrap();
        let jar = root.join("installer.jar");
        let mut zip = zip::ZipWriter::new(File::create(&jar).unwrap());
        zip.start_file(
            "META-INF/MANIFEST.MF",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"Manifest-Version: 1.0\r\nMain-Class: MissingTestMain\r\n\r\n")
            .unwrap();
        zip.finish().unwrap();
        let error = run_installer(
            &java,
            &jar,
            &root,
            &root,
            &["--installClient".into()],
            None,
            &CancellationToken::default(),
            |_| {},
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("Could not find or load main class MissingTestMain"),
            "{error}"
        );
    }
    #[test]
    fn serde_contract() {
        assert_eq!(
            serde_json::to_string(&ModLoader::NeoForge).unwrap(),
            "\"neoforge\""
        );
    }
    #[test]
    fn catalog_sorts_filters_deduplicates() {
        let xml = b"<metadata><version>1.21.1-52.0.9</version><version>1.21.1-52.0.10</version><version>1.21.1-52.0.10</version><version>1.21-51.0.9</version></metadata>";
        let values = parse_catalog(xml, ModLoader::Forge, "1.21.1").unwrap();
        assert_eq!(
            values
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>(),
            ["52.0.10", "52.0.9"]
        );
        let xml = b"<metadata><version>21.1.9</version><version>21.1.10-beta</version><version>21.1.10</version><version>21.10.9</version></metadata>";
        let values = parse_catalog(xml, ModLoader::NeoForge, "1.21.1").unwrap();
        assert_eq!(
            values
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>(),
            ["21.1.10", "21.1.10-beta", "21.1.9"]
        );
        let xml = b"<metadata><version>1.20.1-47.2.0</version><version>1.20.1-47.2.10</version><version>1.20.2-48.0.1</version></metadata>";
        let values = parse_catalog(xml, ModLoader::Forge, "1.20.1").unwrap();
        assert_eq!(
            values
                .iter()
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>(),
            ["47.2.10", "47.2.0"]
        );
        assert!(catalog_prefix(ModLoader::Forge, "..").is_err());
        assert!(catalog_prefix(ModLoader::NeoForge, "1.7.10").is_err());
        assert_eq!(required_java_for("1.20.1", ModLoader::Forge, "47.2.0"), 17);
        assert_eq!(required_java_for("1.20.5", ModLoader::Forge, "51.0.1"), 21);
        assert_eq!(
            required_java_for("1.12.2", ModLoader::Forge, "14.23.5.2859"),
            8
        );
        assert_eq!(required_java_for("26.3", ModLoader::NeoForge, "66.0.1"), 21);
    }
    #[test]
    fn trust_boundaries() {
        for url in [
            "http://maven.minecraftforge.net/x",
            "https://evil.example/x",
            "https://maven.minecraftforge.net.evil/x",
            "https://user@maven.minecraftforge.net/x",
            "https://maven.neoforged.net:8443/x",
        ] {
            assert!(trusted_url(url).is_err());
        }
        assert!(trusted_url(&format!("{NEO}/maven-metadata.xml")).is_ok());
        assert!(checksum(b"invalid").is_err());
        assert_eq!(
            checksum(b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n").unwrap(),
            "a".repeat(40)
        );
    }
    #[test]
    fn publication_conflict_is_preflighted() {
        let staging = fixture();
        let live = fixture();
        fs::write(staging.0.join("a"), b"new").unwrap();
        fs::write(staging.0.join("b"), b"new").unwrap();
        fs::write(live.0.join("b"), b"old").unwrap();
        assert!(publish(
            &staging.0,
            &live.0,
            &[PathBuf::from("a"), PathBuf::from("b")],
            &CancellationToken::default()
        )
        .is_err());
        assert!(!live.0.join("a").exists());
        assert_eq!(fs::read(live.0.join("b")).unwrap(), b"old");
    }
    #[test]
    fn publication_reuses_equal_files_preserves_profiles_and_cancels() {
        let staging = fixture();
        let live = fixture();
        fs::write(staging.0.join("artifact"), b"bytes").unwrap();
        fs::write(live.0.join("launcher_profiles.json"), b"user profiles").unwrap();
        let files = vec![PathBuf::from("artifact")];
        assert_eq!(
            publish(&staging.0, &live.0, &files, &CancellationToken::default()).unwrap(),
            1
        );
        assert_eq!(
            publish(&staging.0, &live.0, &files, &CancellationToken::default()).unwrap(),
            0
        );
        assert_eq!(
            fs::read(live.0.join("launcher_profiles.json")).unwrap(),
            b"user profiles"
        );
        let cancel = CancellationToken::default();
        cancel.cancel();
        assert!(matches!(
            publish(&staging.0, &live.0, &files, &cancel),
            Err(DownloadError::Cancelled)
        ));
    }
    #[test]
    fn library_integrity() {
        let stage = fixture();
        fs::create_dir(stage.0.join("libraries")).unwrap();
        fs::write(stage.0.join("libraries/a.jar"), b"artifact").unwrap();
        let mut value = serde_json::json!({"mainClass":"test", "libraries":[{"downloads":{"artifact":{"path":"a.jar","sha1":hash_bytes(b"artifact")}}}]});
        verify_libraries(&stage.0, &value, &CancellationToken::default()).unwrap();
        value["libraries"][0]["downloads"]["artifact"]["sha1"] = Value::String("0".repeat(40));
        assert!(verify_libraries(&stage.0, &value, &CancellationToken::default()).is_err());
    }
    #[test]
    fn installer_fixture_identity_and_headless_capability() {
        fn write_jar(path: &Path, install: &Value, version: &Value) {
            let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            for (name, data) in [
                ("install_profile.json", serde_json::to_vec(install).unwrap()),
                ("version.json", serde_json::to_vec(version).unwrap()),
            ] {
                zip.start_file(name, options).unwrap();
                zip.write_all(&data).unwrap();
            }
            zip.finish().unwrap();
        }
        let stage = fixture();
        let path = stage.0.join("installer.jar");
        let request = ModLoaderInstallRequest {
            game_dir: String::new(),
            game_version: "1.21.1".into(),
            loader: ModLoader::Forge,
            loader_version: "52.0.10".into(),
            java_path: String::new(),
            strategy: crate::download::DownloadStrategy::default(),
            threads: None,
        };
        let modern_version = serde_json::json!({"id":"official-id","inheritsFrom":"1.21.1","mainClass":"cpw.mods.bootstraplauncher.BootstrapLauncher"});
        let modern = serde_json::json!({"spec":1,"minecraft":"1.21.1","version":"official-id","json":"/version.json"});
        write_jar(&path, &modern, &modern_version);
        let profile = inspect_installer(&path, &request).unwrap();
        assert_eq!(profile.id, "official-id");
        assert!(profile.headless);

        let legacy_version = serde_json::json!({"id":"1.12.2-forge1.12.2-14.23.5.2864","inheritsFrom":"1.12.2","mainClass":"net.minecraft.launchwrapper.Launch","libraries":[]});
        let legacy = serde_json::json!({"install":{"minecraft":"1.12.2"},"versionInfo":legacy_version.clone()});
        let legacy_request = ModLoaderInstallRequest {
            game_version: "1.12.2".into(),
            ..request.clone()
        };
        write_jar(&path, &legacy, &legacy_version);
        let profile = inspect_installer(&path, &legacy_request).unwrap();
        assert_eq!(profile.id, "1.12.2-forge1.12.2-14.23.5.2864");
        assert!(!profile.headless);

        let mismatched = serde_json::json!({"spec":1,"minecraft":"1.21","version":"official-id","json":"/version.json"});
        write_jar(&path, &mismatched, &modern_version);
        assert!(inspect_installer(&path, &request).is_err());
    }
    #[test]
    fn cancellation_and_failure_leave_commit_marker_absent() {
        let stage = fixture();
        let live = fixture();
        fs::write(stage.0.join("library"), b"new").unwrap();
        fs::write(stage.0.join("version.json"), b"{}").unwrap();
        let files = vec![PathBuf::from("library"), PathBuf::from("version.json")];
        let cancel = CancellationToken::default();
        cancel.cancel();
        assert!(matches!(
            publish(&stage.0, &live.0, &files, &cancel),
            Err(DownloadError::Cancelled)
        ));
        assert!(!live.0.join("version.json").exists());
        fs::write(live.0.join("library"), b"existing").unwrap();
        assert!(publish(&stage.0, &live.0, &files, &CancellationToken::default()).is_err());
        assert!(!live.0.join("version.json").exists());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_dangling_symlinks() {
        let stage = fixture();
        std::os::unix::fs::symlink(stage.0.join("missing"), stage.0.join("link")).unwrap();
        assert!(no_links(&stage.0.join("link/file")).is_err());
    }
    #[test]
    fn liteloader_profile_only_adds_the_tweak_class() {
        // 原版 1.12.2 的 arguments.game 已经带齐 --username/--gameDir，profile 再写一遍
        // 会让 launchwrapper 直接抛 MultipleArgumentsForOptionException。
        let request = ModLoaderInstallRequest {
            game_dir: String::new(),
            game_version: "1.12.2".into(),
            loader: ModLoader::LiteLoader,
            loader_version: "1.12.2-SNAPSHOT".into(),
            java_path: String::new(),
            strategy: crate::download::DownloadStrategy::default(),
            threads: None,
        };
        let profile = liteloader_profile("1.12.2-LiteLoader", &request).unwrap();
        assert_eq!(
            profile["minecraftArguments"].as_str(),
            Some("--tweakClass com.mumfrey.liteloader.launch.LiteLoaderTweaker")
        );
        assert!(!profile["minecraftArguments"]
            .as_str()
            .unwrap()
            .contains("--gameDir"));
    }
}
