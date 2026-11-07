//! Official vanilla installation into an explicitly supplied game directory.
//!
//! `list_remote_versions` fetches Mojang's manifest. `install_version` installs sequentially
//! (one bounded streaming transfer), verifies SHA1/size before publishing each file, and
//! reuses verified files. Callers should serialize installs targeting the same directory.
//! Progress totals grow after metadata/index discovery; bytes count network payload only.
//! Cancellation is cooperative between reads (HTTP operations have a 120 second timeout).
//! Fabric is installed from its JSON profile and Maven artifacts; no installer JAR is run.
//! No Java runtime or authentication is installed.
use crate::{inside, io_error, maven_path, rules_allow, safe_component, safe_relative, CoreError};
use reqwest::{blocking::Client, redirect::Policy, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

const FABRIC_META: &str = "https://meta.fabricmc.net/v2/versions/loader";
/// 同一份 Fabric 目录的国内镜像（直接 200 返回 JSON，不是 302）。
const FABRIC_META_BMCLAPI: &str = "https://bmclapi2.bangbang93.com/fabric-meta/v2/versions/loader";
/// Quilt 的元数据（列表与 profile 同形于 Fabric）。
const QUILT_META: &str = "https://meta.quiltmc.org/v3/versions/loader";
/// Legacy Fabric（1.13.2 及更早的 Fabric）的清单与 profile 都在这台服务器上。
const LEGACY_FABRIC_META: &str = "https://meta.legacyfabric.net/v2/versions/loader";
const MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const JSON_LIMIT: u64 = 32 * 1024 * 1024;
const FILE_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
/// Concurrent artifact transfers. Assets are thousands of small independent files, so
/// per-request latency dominates; a small pool keeps the link busy without flooding it.
/// 默认并发数：既要跑满小文件吞吐，又不至于把校园网出口打爆。
const PARALLEL_DOWNLOADS: usize = 16;
/// 上限：再高只会把 TLS 握手和 302 跳转的延迟放大。
/// 整合包并发下载的默认线程数（PCL 默认 64），实际值由设置项决定。
const MODPACK_PARALLEL: usize = 64;
/// 设置里「最大线程数」的上限（PCL 同款 1..=256）。
const MAX_PARALLEL_DOWNLOADS: u32 = 256;
/// 超过这个体积的文件切成多段并发 Range 拉取，避免一条连接吃不满带宽。
const CHUNK_THRESHOLD: u64 = 256 * 1024;
const CHUNK_MIN_PARTS: usize = 2;
const CHUNK_MAX_PARTS: usize = 8;
/// 单连接速率低于这个值并持续 `SLOW_WINDOW`，判定为源不通畅，换另一条源重试。
#[cfg(not(test))]
const SLOW_WINDOW: Duration = Duration::from_secs(4);
#[cfg(test)]
const SLOW_WINDOW: Duration = Duration::from_secs(1);
/// 低于这个速率判定为源不通畅。PCL 的 `NetTaskSpeedLimitLow` 起始就是 256 KB/s，
/// 但它同时受「实际限速」约束——限得比 256 KB/s 还低时，按限速本身算。
const SLOW_CEILING_BYTES_PER_SEC: u64 = 256 * 1024;

/// 限速档位总数：0..=42，和 PCL 的 `ToolDownloadSpeed` 一致。
pub const SPEED_TIER_COUNT: u32 = 43;

/// 「不限速」的档位。
pub const SPEED_TIER_UNLIMITED: u32 = 42;

/// 档位换算成字节/秒：0..=14 每档 0.1 MB/s（0.1–1.5），15..=31 每档 0.5 MB/s
/// （2–10），32..=41 每档 1 MB/s（11–20），42 不限速。
///
/// 公式照抄 PCL `NetTaskSpeedLimitHigh`（`pcl_ModNet.vb:416-433`）。
pub fn tier_limit(tier: u32) -> u64 {
    match tier {
        0..=14 => ((tier as f64 + 1.0) * 0.1 * 1024.0 * 1024.0).round() as u64,
        15..=31 => (tier - 11) as u64 * 512 * 1024,
        32..=41 => (tier - 21) as u64 * 1024 * 1024,
        _ => 0,
    }
}

/// 反查：字节/秒最接近哪个档位（老配置的 KB/s 迁过来时用）。
pub fn tier_for_limit(bytes_per_sec: u64) -> u32 {
    if bytes_per_sec == 0 {
        return SPEED_TIER_UNLIMITED;
    }
    (0..SPEED_TIER_COUNT)
        .min_by_key(|tier| {
            let limit = tier_limit(*tier);
            if limit == 0 {
                u64::MAX
            } else {
                limit.abs_diff(bytes_per_sec)
            }
        })
        .unwrap_or(SPEED_TIER_UNLIMITED)
}

/// 设置里的限速（字节/秒，0 = 不限）。每个下载任务创建 [`Rate`] 时读一次。
static SPEED_LIMIT_BPS: AtomicU64 = AtomicU64::new(0);

/// 保存限速设置（字节/秒，0 表示不限制）。启动器读设置和保存设置时都调它。
pub fn set_speed_limit(bytes_per_sec: u64) {
    SPEED_LIMIT_BPS.store(bytes_per_sec, Ordering::Relaxed);
}

/// 当前设置里的限速（字节/秒，0 = 不限）。
pub fn speed_limit() -> u64 {
    SPEED_LIMIT_BPS.load(Ordering::Relaxed)
}

/// 一个下载任务（一次批量下载 / 一次安装）专用的限速桶。
///
/// PCL 的 `NetTaskSpeedLimitLeft` 挂在任务上：同一任务的所有连接共享额度，不同任务
/// 各算各的（`pcl_ModNet.vb:438-440`、`1086-1092`）。这里同样让每个 `Http` 实例带一份，
/// 整批下载共用一个桶，不会把别的任务的额度吃掉。
pub(crate) struct Rate {
    /// 字节/秒，0 = 不限。
    limit: AtomicU64,
    /// 下一次可以放行的时刻；多个连接并行排队，所以只是记账不占锁睡觉。
    credit: Mutex<Instant>,
    /// 动态速度下限（字节/秒），PCL 会把它抬到「近 1 秒平均速度的 85%」。
    low: AtomicU64,
    /// 最近一次测得的聚合速度（字节/秒），用来判断是否已经跑满。
    speed: AtomicU64,
}

impl Rate {
    pub(crate) fn new(limit: u64) -> Self {
        Self {
            limit: AtomicU64::new(limit),
            credit: Mutex::new(Instant::now()),
            low: AtomicU64::new(initial_low(limit)),
            speed: AtomicU64::new(0),
        }
    }

    pub(crate) fn limit(&self) -> u64 {
        self.limit.load(Ordering::Relaxed)
    }

    /// 速度下限：低于它说明还没跑满，可以再开连接。
    pub(crate) fn low(&self) -> u64 {
        self.low.load(Ordering::Relaxed)
    }

    pub(crate) fn speed(&self) -> u64 {
        self.speed.load(Ordering::Relaxed)
    }

    pub(crate) fn set_speed(&self, speed: u64) {
        self.speed.store(speed, Ordering::Relaxed);
    }

    /// 已经跑到下限之上：不必再开新连接。
    pub(crate) fn saturated(&self) -> bool {
        self.speed() >= self.low()
    }

    /// 按「近 1 秒平均速度 × 0.85」抬高下限，只升不降（PCL `RefreshStat`）。
    pub(crate) fn raise_low(&self, average_speed: f64) {
        let target = (average_speed * 0.85).max(0.0) as u64;
        let mut low = self.low.load(Ordering::Relaxed);
        while target > low {
            match self
                .low
                .compare_exchange_weak(low, target, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(actual) => low = actual,
            }
        }
    }

    /// 领 `bytes` 字节的额度：额度不够就睡到够为止。`limit == 0` 时零开销。
    pub(crate) fn throttle(&self, bytes: u64) {
        let limit = self.limit();
        if limit == 0 || bytes == 0 {
            return;
        }
        let wait_until = {
            let Ok(mut credit) = self.credit.lock() else {
                return;
            };
            let now = Instant::now();
            let start = (*credit).max(now);
            let span = Duration::from_secs_f64(bytes as f64 / limit as f64);
            let ready_at = start + span;
            *credit = ready_at;
            ready_at
        };
        let now = Instant::now();
        if wait_until > now {
            thread::sleep(wait_until - now);
        }
    }
}

/// 起始下限：PCL 是 256 KB/s，但限速比它还低时按下限本身算，否则永不达标。
fn initial_low(limit: u64) -> u64 {
    if limit == 0 {
        SLOW_CEILING_BYTES_PER_SEC
    } else {
        limit.min(SLOW_CEILING_BYTES_PER_SEC)
    }
}

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("Download failed: {0}")]
    Network(String),
    #[error("Invalid download metadata: {0}")]
    Invalid(String),
    #[error("Integrity check failed: {0}")]
    Integrity(String),
    #[error("Download stalled: {0}")]
    Slow(String),
    #[error("Installation cancelled")]
    Cancelled,
}
impl DownloadError {
    /// 让界面能直接告诉用户「哪个文件失败了」，重试时也知道补齐什么。
    fn with_path(self, path: &str) -> Self {
        match self {
            DownloadError::Network(message) => DownloadError::Network(format!("{path}: {message}")),
            other => other,
        }
    }
}
type Result<T> = std::result::Result<T, DownloadError>;

/// 停止标记的三种状态：跑着、暂停（下次可续传）、取消（要清干净）。
const RUNNING: u8 = 0;
const PAUSED: u8 = 1;
const CANCELLED: u8 = 2;

#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicU8>);
impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(CANCELLED, Ordering::Relaxed);
    }
    /// 暂停：让下载停下，但保留下到一半的文件（`.part`），下次同名安装接着下。
    pub fn pause(&self) {
        let _ = self.0.compare_exchange(
            RUNNING,
            PAUSED,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed) != RUNNING
    }
    /// 是被「暂停」叫停的（而不是取消）。
    pub fn is_paused(&self) -> bool {
        self.0.load(Ordering::Relaxed) == PAUSED
    }
    pub fn is_cancel_requested(&self) -> bool {
        self.0.load(Ordering::Relaxed) == CANCELLED
    }
    fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(DownloadError::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteVersion {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    pub time: String,
    pub release_time: String,
    pub sha1: Option<String>,
    pub compliance_level: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteVersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<RemoteVersion>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRequest {
    pub game_dir: String,
    pub version_id: String,
    #[serde(default)]
    pub fabric_loader: Option<String>,
    #[serde(default)]
    pub quilt_loader: Option<String>,
    /// 下载源策略（缺省按竞速）。
    #[serde(default)]
    pub strategy: DownloadStrategy,
    /// 并发下载数，取值范围 1..=64，缺省用 PARALLEL_DOWNLOADS。
    #[serde(default)]
    pub threads: Option<u32>,
}
impl InstallRequest {
    /// PCL 的线程数 = 设置值 + 1（设置 1..=256 → 实际 2..=257），设置里那一个留给
    /// 「任务本身」，其余才是并发连接。
    fn thread_count(&self) -> usize {
        let asked = self.threads.unwrap_or(PARALLEL_DOWNLOADS as u32) as usize;
        asked.clamp(1, MAX_PARALLEL_DOWNLOADS as usize) + 1
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FabricLoaderVersion {
    pub version: String,
    /// Quilt 的元数据没有 stable 字段。
    #[serde(default)]
    pub stable: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallProgress {
    pub version_id: String,
    /// manifest, metadata, asset-index, downloading, fabric-profile, fabric-checksum,
    /// fabric-libraries, complete, cancelled, or failed.
    pub stage: String,
    pub path: String,
    pub completed_files: u64,
    pub total_files: u64,
    pub downloaded_bytes: u64,
    /// Sum of known sizes discovered so far, excluding files without a size.
    pub total_bytes: u64,
    /// 加权百分比（0–100）：PCL 把一次安装拆成「分析 / 支持库 / 资源」几段，
    /// 按段内完成度折算，进度条不会因为后面才发现大文件而后退。
    /// `None` 表示这条流程没算，界面按字节估算。
    #[serde(default)]
    pub percent: Option<f64>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallSummary {
    pub version_id: String,
    /// 这次装出来的目录（实例安装是 `versions/<实例名>`）。空串表示这条路不产出可打开的目录，
    /// 界面据此不给「打开文件夹 / 删除文件」。
    pub path: String,
    pub downloaded_files: u64,
    pub skipped_files: u64,
    pub downloaded_bytes: u64,
}

const MIRROR_HOST: &str = "bmclapi2.bangbang93.com";

/// 允许下载的主机：Mojang 官方 + Fabric + BMCLAPI 镜像及其跳转后端。
fn host_allowed(host: &str) -> bool {
    matches!(
        host,
        "piston-meta.mojang.com"
            | "piston-data.mojang.com"
            | "launchermeta.mojang.com"
            | "launcher.mojang.com"
            | "libraries.minecraft.net"
            | "resources.download.minecraft.net"
            | "meta.fabricmc.net"
            | "maven.fabricmc.net"
            | "meta.quiltmc.org"
            | "meta.legacyfabric.net"
            | "maven.legacyfabric.net"
            | "repo.legacyfabric.net"
            | "maven.quiltmc.org"
            | "maven.minecraftforge.net"
            | "maven.neoforged.net"
            | "files.minecraftforge.net"
            | "maven.cleanroommc.com"
            | "nexus.jsdu.cn"
            | "releases.r2.labymod.net"
            | "cdn.labymod.net"
            | "cdn.modrinth.com"
            | "edge.forgecdn.net"
            | "dist.modpacks.ch"
            | "api.modpacks.ch"
            | "media.forgecdn.net"
            | "mediafilez.forgecdn.net"
            | "mod.mcimirror.top"
            | "api.modrinth.com"
            | "repo.maven.apache.org"
            | "maven.aliyun.com"
            | "api.github.com"
            | "objects.githubusercontent.com"
            | "release-assets.githubusercontent.com"
            | "dl.liteloader.com"
            | MIRROR_HOST
            | "mirrors.hit.edu.cn"
            | "mirrors.cernet.edu.cn"
    ) || host.ends_with(".sakura.sld.tw")
}

fn trusted_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|e| DownloadError::Invalid(e.to_string()))?;
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !url.host_str().is_some_and(host_allowed)
    {
        return Err(DownloadError::Invalid(format!(
            "untrusted download URL: {raw}"
        )));
    }
    Ok(url)
}

/// Maven Central 的国内镜像：同一个坐标换个仓库前缀即可，命中率实测 118/118。
const ALIYUN_PREFIX: &str = "https://maven.aliyun.com/repository/public";
const CENTRAL_HOST: &str = "repo.maven.apache.org";
const CENTRAL_PREFIX: &str = "/maven2/";

/// 一条候选下载路径。同一份文件可能有官方、BMCLAPI、阿里云等几条路可走。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    Aliyun,
    Nexus,
    Bmclapi,
    Official,
}

impl Route {
    /// 还没测过时的先验速度（B/s），只为排出默认优先级。
    fn prior(self) -> f64 {
        match self {
            Route::Aliyun => 4_000_000.0,
            Route::Nexus => 3_000_000.0,
            Route::Bmclapi => 1_500_000.0,
            Route::Official => 200_000.0,
        }
    }
}

/// 同一份文件的所有候选地址；返回顺序只是默认优先级，实际顺序由测速决定。
fn route_candidates(original: &Url) -> Vec<(Route, Url)> {
    let host = original.host_str().unwrap_or_default();
    let path = original.path();
    // 校验文件（.sha1 等）镜像上普遍缺失，这类小文件只走官方。
    if [".sha1", ".sha256", ".md5"]
        .iter()
        .any(|ext| path.ends_with(ext))
    {
        return vec![(Route::Official, original.clone())];
    }
    let mut out = Vec::new();
    if host == CENTRAL_HOST {
        if let Some(rest) = path.strip_prefix(CENTRAL_PREFIX) {
            push_route(&mut out, Route::Aliyun, format!("{ALIYUN_PREFIX}/{rest}"));
        }
    }
    if matches!(
        host,
        "libraries.minecraft.net" | "maven.minecraftforge.net" | "maven.neoforged.net"
    ) {
        push_route(
            &mut out,
            Route::Bmclapi,
            format!("https://{MIRROR_HOST}/maven{path}"),
        );
    }
    // Fabric / Quilt 的 maven（加载器安装包与依赖库）。
    if matches!(
        host,
        "maven.fabricmc.net" | "maven.quiltmc.org" | "maven.legacyfabric.net"
    ) {
        push_route(
            &mut out,
            Route::Bmclapi,
            format!("https://{MIRROR_HOST}/maven{path}"),
        );
    }
    if host == "resources.download.minecraft.net" {
        push_route(
            &mut out,
            Route::Bmclapi,
            format!("https://{MIRROR_HOST}/assets{path}"),
        );
    }
    // Cleanroom 的 maven 本身就 307 到 nexus，直接打边缘省一跳。
    if host == "maven.cleanroommc.com" {
        push_route(
            &mut out,
            Route::Nexus,
            format!("https://nexus.jsdu.cn/repository{path}"),
        );
    }
    // 元数据（清单/版本 json）都很小、镜像反而绕远，只有官方一条路。
    out.push((Route::Official, original.clone()));
    out
}

fn push_route(out: &mut Vec<(Route, Url)>, route: Route, raw: String) {
    let Ok(url) = Url::parse(&raw) else {
        return;
    };
    if url.host_str().is_some_and(host_allowed) {
        out.push((route, url));
    }
}

/// 一组坐标共用一次测速结果：同一个仓库里的文件走同一条路。
fn family(url: &Url) -> &'static str {
    match url.host_str().unwrap_or_default() {
        CENTRAL_HOST => "central",
        "libraries.minecraft.net" | "maven.minecraftforge.net" | "maven.neoforged.net" => "maven",
        "resources.download.minecraft.net" => "assets",
        "maven.cleanroommc.com" | "nexus.jsdu.cn" => "cleanroom",
        _ => "other",
    }
}

/// 一组坐标的测速结果。
struct Ranked {
    speeds: Vec<(Route, f64)>,
    at: Instant,
}

/// 测速结果的有效期：过期后重新比一次，网络状况变了也能跟上。
const RANK_TTL: Duration = Duration::from_secs(600);
/// 单条路的测速只读这么点字节，够判断快慢又不浪费流量。
const PROBE_BYTES: usize = 64 * 1024;
/// 竞速时先读到这么多字节再定胜负：小文件一次读完，大文件只多花这么点。
const RACE_PREFETCH: usize = 64 * 1024;
/// 同时竞速的候选数。
const RACE_WIDTH: usize = 3;

/// 用一个很小的 Range 请求量这条路的实际速度，失败返回 None。
fn measure(client: &Client, url: &Url) -> Option<f64> {
    let started = Instant::now();
    let mut response = client
        .get(url.clone())
        .header(
            reqwest::header::RANGE,
            format!("bytes=0-{}", PROBE_BYTES - 1),
        )
        .timeout(Duration::from_secs(6))
        .send()
        .ok()?
        .error_for_status()
        .ok()?;
    let mut buffer = vec![0u8; PROBE_BYTES];
    let mut total = 0usize;
    while total < PROBE_BYTES {
        match response.read(&mut buffer[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(_) => break,
        }
    }
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    (total > 0).then(|| total as f64 / elapsed)
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}
trait Source {
    fn open(&self, url: &str) -> Result<Box<dyn Read>>;
    /// 尽量从 `offset` 开始续传；返回 (读取器, 是否确实从 offset 开始)。
    /// 默认实现忽略偏移量，等价于整文件重下。
    fn open_from(&self, url: &str, offset: u64) -> Result<(Box<dyn Read>, bool)> {
        let _ = offset;
        Ok((self.open(url)?, false))
    }
    /// 传输太慢时被调用，实现方可以据此把这条源降权、换一条。
    fn note_slow(&self, url: &str) {
        let _ = url;
    }
    /// 读到 `bytes` 字节后向「本任务」的限速桶记账。
    fn throttle(&self, bytes: u64) {
        let _ = bytes;
    }
    /// 本任务的速度下限（字节/秒）：低于它说明还没跑满。
    fn low(&self) -> u64 {
        SLOW_CEILING_BYTES_PER_SEC
    }
    /// 本任务已经跑满下限：不必再为同一个文件开新连接。
    fn saturated(&self) -> bool {
        false
    }
}
/// 下载源策略。判定规则只有一条：某个地址**有镜像路由**才按策略分流，
/// 没有镜像路由（版本清单、json 这类）永远只走官方，否则会直接 404。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DownloadStrategy {
    /// 官方与镜像同时下，谁先到用谁。
    #[default]
    Race,
    /// 先官方，失败或太慢再换镜像。
    OfficialFirst,
    /// 先镜像，失败或太慢再换官方。
    MirrorFirst,
    /// 只走镜像；没有镜像路由的地址仍然走官方。
    MirrorOnly,
    /// 只走官方。
    OfficialOnly,
}

impl DownloadStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Race => "race",
            Self::OfficialFirst => "official-first",
            Self::MirrorFirst => "mirror-first",
            Self::MirrorOnly => "mirror-only",
            Self::OfficialOnly => "official-only",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim() {
            "race" => Self::Race,
            "official-first" => Self::OfficialFirst,
            "mirror-first" => Self::MirrorFirst,
            "mirror-only" => Self::MirrorOnly,
            "official-only" => Self::OfficialOnly,
            _ => return None,
        })
    }
    /// 旧配置（下载源 + 竞速开关）映射到新枚举。
    pub fn from_legacy(source: &str, race: bool) -> Self {
        if race {
            Self::Race
        } else if source == "bmclapi" {
            Self::MirrorFirst
        } else {
            Self::OfficialFirst
        }
    }
    pub fn allows_mirror(self) -> bool {
        self != Self::OfficialOnly
    }
    fn code(self) -> u8 {
        match self {
            Self::Race => 0,
            Self::OfficialFirst => 1,
            Self::MirrorFirst => 2,
            Self::MirrorOnly => 3,
            Self::OfficialOnly => 4,
        }
    }
    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::OfficialFirst,
            2 => Self::MirrorFirst,
            3 => Self::MirrorOnly,
            4 => Self::OfficialOnly,
            _ => Self::Race,
        }
    }
}

static ACTIVE_STRATEGY: AtomicU8 = AtomicU8::new(0);

/// 全局下载策略：设置页保存后由 Tauri 层写入一次，之后所有下载都按它分流。
pub fn set_strategy(strategy: DownloadStrategy) {
    ACTIVE_STRATEGY.store(strategy.code(), Ordering::Relaxed);
}

pub fn strategy() -> DownloadStrategy {
    DownloadStrategy::from_code(ACTIVE_STRATEGY.load(Ordering::Relaxed))
}

struct Http {
    client: Client,
    strategy: DownloadStrategy,
    /// 本任务的限速桶：整批下载共用一份，任务之间互不影响。
    rate: Rate,
    /// 正在安装的版本号（如 `1.20.5`）。原版 client/server jar 的镜像地址是
    /// `https://<mirror>/version/<id>/client`，光看文件 URL（`/v1/objects/<sha>/client.jar`）
    /// 是拿不到版本号的，所以由调用方在安装某个版本时把它放进来。
    version: Option<String>,
    rank: Mutex<HashMap<&'static str, Ranked>>,
    probing: Mutex<BTreeSet<&'static str>>,
    edge: Mutex<Option<String>>,
}
impl Http {
    /// 告诉这层「现在装的是哪个版本」，用来补原版 jar 的镜像地址。
    fn with_version(mut self, version: &str) -> Self {
        self.version = Some(version.to_owned());
        self
    }
    fn new(strategy: DownloadStrategy) -> Result<Self> {
        Self::build(strategy, Some(Duration::from_secs(120)))
    }
    /// 整合包这类「文件多、单个文件可能很大」的场景不设全局超时：
    /// 慢速链路上 7 MB 的 jar 要下两分钟，按总时长卡死会让它反复从头重试。
    /// 单文件的总时长在请求上按体积单独给（见 `transfer_timeout`）。
    fn streaming(strategy: DownloadStrategy) -> Result<Self> {
        Self::build(strategy, None)
    }
    /// 超时二选一：`timeout` 是整条请求的总时长（元数据、小文件），
    /// `read` 是「多久没收到数据算超时」（边下边写的大文件只该用这个）。
    fn build(strategy: DownloadStrategy, timeout: Option<Duration>) -> Result<Self> {
        let mut builder = Client::builder()
            .https_only(true)
            // 有的站（legacyfabric / labymod 这类 Cloudflare 边缘）对 HTTP/2 握手不友好，
            // 走 HTTP/1.1 稳定得多；我们并发靠线程池，不依赖 h2 的多路复用。
            .http1_only()
            // 部分 Cloudflare 边缘（legacyfabric 那几家）在 TLS 1.3 上会握手卡死，
            // 降到 1.2 一切正常；1.2 是我们的全部下载源都支持的版本。
            .max_tls_version(reqwest::tls::Version::TLS_1_2)
            // 允许跳转，但每一跳都必须落在白名单主机上（镜像会 302 到自家 CDN）。
            .redirect(Policy::custom(|attempt| {
                // BMCLAPI 会 302 到各家高校镜像，跳转后的路径里保留着 /bmclapi/。
                let mirror_path = attempt.url().path().contains("/bmclapi/");
                if attempt.previous().len() >= 5 {
                    attempt.error("too many redirects")
                } else if attempt.url().host_str().is_some_and(host_allowed) || mirror_path {
                    attempt.follow()
                } else {
                    attempt.error("redirect to untrusted host")
                }
            }))
            .connect_timeout(Duration::from_secs(20))
            // 国内 IPv6 路由常常连上即被重置，优先用 IPv4。
            .local_address(if cfg!(windows) {
                Some(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
            } else {
                None
            })
            // 复用连接：资源文件成千上万，握手开销比传输本身还贵。
            // keep-alive 池留久一点，HTTP/2 下同一条连接可以并行多条流。
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(MAX_PARALLEL_DOWNLOADS as usize)
            .tcp_nodelay(true)
            .user_agent("YBUMCL/0.1");
        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }
        builder
            .build()
            .map(|client| Self {
                client,
                strategy,
                rate: Rate::new(speed_limit()),
                version: None,
                rank: Mutex::new(HashMap::new()),
                probing: Mutex::new(BTreeSet::new()),
                edge: Mutex::new(None),
            })
            .map_err(|e| DownloadError::Network(e.to_string()))
    }
}
impl Http {
    fn request(
        &self,
        url: &str,
        range_start: Option<u64>,
    ) -> Result<(reqwest::blocking::Response, bool)> {
        self.request_with(url, range_start, None)
    }
    fn request_with(
        &self,
        url: &str,
        range_start: Option<u64>,
        timeout: Option<Duration>,
    ) -> Result<(reqwest::blocking::Response, bool)> {
        let target = self.route(trusted_url(url)?);
        let mut request = self.client.get(target.clone());
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        if let Some(offset) = range_start.filter(|offset| *offset > 0) {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let response = request
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        let partial = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        self.remember_edge(&target, &response);
        Ok((response, partial))
    }
    /// 闭区间分段请求（`Range: bytes=start-end`），分块下载用。
    ///
    /// 和 `request_with` 的唯一区别是 Range 头：那边是断点续传的开放区间
    /// （`bytes=offset-`），这里必须带结束位置，否则每块都会把后面全下下来。
    fn request_span(
        &self,
        url: &str,
        start: u64,
        end: u64,
    ) -> Result<(reqwest::blocking::Response, bool)> {
        let target = self.route(trusted_url(url)?);
        let response = self
            .client
            .get(target.clone())
            .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        let partial = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        self.remember_edge(&target, &response);
        Ok((response, partial))
    }
    /// 取文件总大小：先试 `Range: bytes=0-0` 读 `Content-Range` 的 `/total`，
    /// 服务器不认分段就退回 `Content-Length`；都拿不到就返回 None（不启用分块）。
    fn probe_size(&self, url: &str) -> Result<Option<u64>> {
        let target = self.route(trusted_url(url)?);
        let response = self
            .client
            .get(target.clone())
            .header(reqwest::header::RANGE, "bytes=0-0")
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        self.remember_edge(&target, &response);
        let total = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|text| text.rsplit('/').next())
            .and_then(|tail| tail.trim().parse::<u64>().ok());
        Ok(total.or_else(|| response.content_length()))
    }
    /// 镜像把请求 302 到边缘节点后，后续文件直接走那条连接，省掉每文件一次跳转。
    fn remember_edge(&self, requested: &Url, response: &reqwest::blocking::Response) {
        let Some(host) = response.url().host_str() else {
            return;
        };
        // 只认「镜像 302 到边缘节点」这一种：否则官方源的正常响应会把
        // 官方主机当成边缘节点缓存下来，之后所有镜像地址都会被改写到官方主机上（404）。
        if requested.host_str() != Some(MIRROR_HOST) || host.contains(MIRROR_HOST) {
            return;
        }
        let path = response.url().path();
        let Some(prefix) = path.strip_suffix(requested.path()) else {
            return;
        };
        if !host_allowed(host) {
            return;
        }
        let edge = format!("{}://{host}{}", response.url().scheme(), prefix);
        if let Ok(mut cached) = self.edge.lock() {
            if cached.as_deref() != Some(edge.as_str()) {
                *cached = Some(edge);
            }
        }
    }
    fn route(&self, target: Url) -> Url {
        if target.host_str() != Some(MIRROR_HOST) {
            return target;
        }
        let Some(edge) = self.edge.lock().ok().and_then(|cached| cached.clone()) else {
            return target;
        };
        let Ok(base) = Url::parse(&edge) else {
            return target;
        };
        let Some(host) = base.host_str() else {
            return target;
        };
        if !host_allowed(host) {
            return target;
        }
        let mut routed = target.clone();
        let prefix = base.path();
        if routed.set_host(Some(host)).is_err() || routed.set_scheme(base.scheme()).is_err() {
            return target;
        }
        routed.set_path(&format!("{prefix}{}", target.path()));
        routed
    }
    /// 这条地址跟到最后是哪台机器在出货（`None` = 探不到）。
    ///
    /// 判决「镜像」真假只能看 302 之后落到哪：`mod.mcimirror.top` 实测只是跳板，
    /// Modrinth 回 `cdn.modrinth.com`、CurseForge 回 `mediafilez.forgecdn.net`，
    /// 跟官方地址落在同一批机器上。HEAD 不校验状态码——405 也是有用信息。
    fn final_host(&self, url: &str) -> Result<Option<String>> {
        let target = self.route(trusted_url(url)?);
        let response = self
            .client
            .head(target)
            .send()
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        Ok(response.url().host_str().map(|host| host.to_ascii_lowercase()))
    }
    /// 这一组坐标当前各条路的实测速度；没有结果时用先验值。
    fn speeds(&self, key: &'static str) -> Vec<(Route, f64)> {
        self.rank
            .lock()
            .ok()
            .and_then(|rank| {
                rank.get(key)
                    .filter(|entry| entry.at.elapsed() < RANK_TTL)
                    .map(|entry| entry.speeds.clone())
            })
            .unwrap_or_default()
    }
    /// 按实测速度把候选地址排好序，没测过的排在后面（用先验值兜底）。
    fn arrange(&self, key: &'static str, mut all: Vec<(Route, Url)>) -> Vec<Url> {
        let speeds = self.speeds(key);
        let score = |route: Route| {
            speeds
                .iter()
                .find(|(candidate, _)| *candidate == route)
                .map_or(route.prior(), |(_, speed)| *speed)
        };
        all.sort_by(|a, b| score(b.0).total_cmp(&score(a.0)));
        all.into_iter().map(|(_, url)| url).collect()
    }
    /// 同时打每条路，谁先读完 `PROBE_BYTES` 谁快。同一组坐标只比一次，其余线程照常用先验顺序。
    fn probe(&self, key: &'static str, all: &[(Route, Url)]) {
        {
            let Ok(mut probing) = self.probing.lock() else {
                return;
            };
            if !probing.insert(key) {
                return;
            }
        }
        let speeds: Vec<(Route, f64)> = thread::scope(|scope| {
            let handles: Vec<_> = all
                .iter()
                .map(|(route, url)| {
                    let route = *route;
                    let url = url.clone();
                    let client = self.client.clone();
                    scope.spawn(move || (route, measure(&client, &url)))
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .filter_map(|(route, speed)| speed.map(|speed| (route, speed)))
                .collect()
        });
        if !speeds.is_empty() {
            if let Ok(mut rank) = self.rank.lock() {
                rank.insert(
                    key,
                    Ranked {
                        speeds,
                        at: Instant::now(),
                    },
                );
            }
        }
        if let Ok(mut probing) = self.probing.lock() {
            probing.remove(key);
        }
    }
    fn candidates(&self, url: &str) -> Result<Vec<Url>> {
        Ok(self
            .routed_candidates(url)?
            .into_iter()
            .map(|(_, url)| url)
            .collect())
    }
    /// 候选地址连同它们各自的路（竞速时要把赢家记进排名）。
    fn routed_candidates(&self, url: &str) -> Result<Vec<(Route, Url)>> {
        let original = trusted_url(url)?;
        let mut all = route_candidates(&original);
        if let Some(mirror) = self.version_mirror(&original) {
            if !all.iter().any(|(_, candidate)| *candidate == mirror) {
                all.push((Route::Bmclapi, mirror));
            }
        }
        if all.len() == 1 {
            return Ok(all);
        }
        match self.strategy {
            // 只走官方：镜像候选直接丢掉（没有镜像路由的本来就只剩官方）。
            DownloadStrategy::OfficialOnly => {
                all.retain(|(route, _)| *route == Route::Official);
            }
            // 只走镜像：有镜像路由就只留镜像，纯官方地址保留原样。
            DownloadStrategy::MirrorOnly => {
                all.retain(|(route, _)| *route != Route::Official);
                if all.is_empty() {
                    return Ok(route_candidates(&original));
                }
            }
            // 顺序固定的两种：镜像在前 / 官方在前。
            DownloadStrategy::MirrorFirst => {
                all.sort_by_key(|(route, _)| *route == Route::Official);
            }
            DownloadStrategy::OfficialFirst => {
                all.sort_by_key(|(route, _)| *route != Route::Official);
            }
            // 竞速：按测速结果排序，`race()` 会同时开这两条。
            DownloadStrategy::Race => {
                let key = family(&original);
                if self.speeds(key).is_empty() {
                    self.probe(key, &all);
                }
                let speeds = self.speeds(key);
                let score = |route: Route| {
                    speeds
                        .iter()
                        .find(|(candidate, _)| *candidate == route)
                        .map_or(route.prior(), |(_, speed)| *speed)
                };
                all.sort_by(|a, b| score(b.0).total_cmp(&score(a.0)));
            }
        }
        Ok(all)
    }
    /// 原版 client/server jar 的镜像地址：`/version/<版本号>/client`。
    ///
    /// 文件 URL 里只有对象哈希（`/v1/objects/<sha>/client.jar`），版本号是安装某个
    /// 版本时由 `with_version` 带进来的；没有版本号就没法拼，返回 `None`。
    fn version_mirror(&self, original: &Url) -> Option<Url> {
        let version = self.version.as_deref().filter(|id| safe_component(id))?;
        if original.host_str() != Some("piston-data.mojang.com") {
            return None;
        }
        let name = original.path().rsplit('/').next()?;
        let kind = match name {
            "client.jar" => "client",
            "server.jar" => "server",
            _ => return None,
        };
        Url::parse(&format!("https://{MIRROR_HOST}/version/{version}/{kind}")).ok()
    }
    /// 多源竞速：同时向每个候选地址发请求，谁先吐够首块谁赢。
    ///
    /// 小文件（大多数元数据、库小 jar）在第一块里就下载完了，等价于「全量竞速」；
    /// 大文件只用首块定胜负，剩下的继续走赢家，额外流量最多 64 KiB × 候选数。
    fn race(&self, url: &str, offset: u64) -> Option<Result<(Box<dyn Read>, bool)>> {
        if self.strategy != DownloadStrategy::Race {
            return None;
        }
        let candidates = self.routed_candidates(url).ok()?;
        if candidates.len() < 2 {
            return None;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        thread::scope(|scope| {
            for (route, candidate) in candidates.into_iter().take(RACE_WIDTH) {
                let tx = tx.clone();
                let client = self.client.clone();
                let route_url = self.route(candidate.clone());
                scope.spawn(move || {
                    let started = Instant::now();
                    let mut request = client.get(route_url.clone());
                    if offset > 0 {
                        request =
                            request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
                    }
                    let Ok(response) = request.send().and_then(|r| r.error_for_status()) else {
                        return;
                    };
                    let partial = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
                    let mut reader = response;
                    let mut head = Vec::with_capacity(RACE_PREFETCH);
                    let mut chunk = [0u8; 16 * 1024];
                    while head.len() < RACE_PREFETCH {
                        match reader.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(n) => head.extend_from_slice(&chunk[..n]),
                            Err(_) => return,
                        }
                    }
                    if head.is_empty() {
                        return;
                    }
                    let elapsed = started.elapsed().as_secs_f64().max(0.001);
                    let _ = tx.send((
                        route,
                        partial,
                        head,
                        elapsed,
                        Box::new(reader) as Box<dyn Read + Send>,
                    ));
                });
            }
            drop(tx);
        });
        let (winner, partial, head, elapsed, rest) = rx.recv().ok()?;
        // 赢家的实测速度记进排名，后面的文件可以直接按它排。
        if let Ok(parsed) = trusted_url(url) {
            let key = family(&parsed);
            let speed = head.len() as f64 / elapsed;
            if let Ok(mut rank) = self.rank.lock() {
                let entry = rank.entry(key).or_insert_with(|| Ranked {
                    speeds: vec![],
                    at: Instant::now(),
                });
                match entry.speeds.iter_mut().find(|(route, _)| *route == winner) {
                    Some((_, value)) => *value = speed,
                    None => entry.speeds.push((winner, speed)),
                }
                entry.at = Instant::now();
            }
        }
        let body = std::io::Cursor::new(head).chain(rest);
        Some(Ok((Box::new(body) as Box<dyn Read>, partial)))
    }

    fn send(&self, url: &str) -> Result<Box<dyn Read>> {
        if let Some(Ok((reader, _))) = self.race(url, 0) {
            return Ok(reader);
        }
        let mut last = DownloadError::Network("no download source available".into());
        for candidate in self.candidates(url)? {
            match self.request(candidate.as_str(), None) {
                Ok((response, _)) => return Ok(Box::new(response)),
                Err(error) => last = error,
            }
        }
        Err(last)
    }
}
impl Source for Http {
    fn open(&self, url: &str) -> Result<Box<dyn Read>> {
        self.send(url)
    }
    fn open_from(&self, url: &str, offset: u64) -> Result<(Box<dyn Read>, bool)> {
        if let Some(result) = self.race(url, offset) {
            return result;
        }
        if offset == 0 {
            return Ok((self.send(url)?, false));
        }
        let mut last = DownloadError::Network("no download source available".into());
        for candidate in self.candidates(url)? {
            match self.request(candidate.as_str(), Some(offset)) {
                Ok((response, partial)) => return Ok((Box::new(response), partial)),
                Err(error) => last = error,
            }
        }
        Err(last)
    }
    fn note_slow(&self, url: &str) {
        if !self.strategy.allows_mirror() {
            return;
        }
        let Ok(parsed) = trusted_url(url) else {
            return;
        };
        let key = family(&parsed);
        let Ok(mut rank) = self.rank.lock() else {
            return;
        };
        // 把当前最快的那条路压下去，下一次自然换源；不立刻重新测速，免得抖动时反复折腾。
        if let Some(entry) = rank.get_mut(key) {
            if let Some((_, fastest)) = entry.speeds.iter_mut().max_by(|a, b| a.1.total_cmp(&b.1)) {
                *fastest /= 100.0;
            }
            entry.at = Instant::now();
        }
    }
    fn throttle(&self, bytes: u64) {
        self.rate.throttle(bytes);
    }
    fn low(&self) -> u64 {
        self.rate.low()
    }
    fn saturated(&self) -> bool {
        self.rate.saturated()
    }
}
fn fetch_json(source: &impl Source, url: &str, cancel: &CancellationToken) -> Result<Vec<u8>> {
    fetch_limited(source, url, cancel, JSON_LIMIT)
}
/// 拉一份加载器清单（小的 JSON / HTML），走官方源。
pub fn fetch_catalog_bytes(url: &str, cancel: &CancellationToken) -> Result<Vec<u8>> {
    let source = Http::new(strategy())?;
    fetch_limited(&source, url, cancel, JSON_LIMIT)
}

/// 单个文件的可读流，外加服务端声明的总长度（没给就是 None，界面画成不确定进度）。
/// 逐块读、边读边写，大文件不用整个塞进内存。
pub fn open_file_stream(url: &str) -> Result<(Box<dyn Read>, Option<u64>)> {
    let source = Http::new(strategy())?;
    let mut last = DownloadError::Network("no download source available".into());
    for candidate in source.candidates(url)? {
        match source.request(candidate.as_str(), None) {
            Ok((response, _)) => {
                let total = response.content_length();
                return Ok((Box::new(response) as Box<dyn Read>, total));
            }
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// 单个文件的完整内容（客户端 jar、安装器这类，几十 MB 到几百 MB）。
/// 和元数据一样走多源竞速，只是上限放宽。
pub fn fetch_file_bytes(url: &str, cancel: &CancellationToken) -> Result<Vec<u8>> {
    const FILE_LIMIT: u64 = 512 * 1024 * 1024;
    let source = Http::new(strategy())?;
    fetch_limited(&source, url, cancel, FILE_LIMIT)
}

/// 同上，但带上版本号，让 `piston-data` 这类地址也能生成 BMCLAPI 镜像候选。
///
/// 打包原版客户端时原来用的是不带版本号的 `Http`，`version_mirror` 拿不到版本号就
/// 直接 None，客户端 jar 只剩官方一条路，竞速自然启动不了。
pub fn fetch_file_bytes_for_version(
    url: &str,
    version: &str,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    const FILE_LIMIT: u64 = 512 * 1024 * 1024;
    let source = Http::new(strategy())?.with_version(version);
    // 大文件先试 Range 分块（每块官方/镜像竞速）；服务器不认 Range 就退回单流。
    if let Some(bytes) = fetch_chunked_bytes(&source, url, cancel, FILE_LIMIT)? {
        return Ok(bytes);
    }
    fetch_limited(&source, url, cancel, FILE_LIMIT)
}

/// 带进度的版本：同一套分块竞速，只是每读到一段就把「已收字节」加进 `received`
/// 并调一次 `on_bytes`（节流交给调用方）。
///
/// 打包手动安装包时客户端 jar 占整包的九成，任务卡的百分比只能从这里拿。
pub fn fetch_file_bytes_for_version_progress(
    url: &str,
    version: &str,
    cancel: &CancellationToken,
    received: &AtomicU64,
    on_bytes: &(dyn Fn() + Sync),
) -> Result<Vec<u8>> {
    const FILE_LIMIT: u64 = 512 * 1024 * 1024;
    let source = Http::new(strategy())?.with_version(version);
    match fetch_chunked_bytes_counted(&source, url, cancel, FILE_LIMIT, received, on_bytes) {
        Ok(Some(bytes)) => return Ok(bytes),
        Err(DownloadError::Cancelled) => return Err(DownloadError::Cancelled),
        // 分块没成（服务器不认 Range）：退回单流，计数从头来过。
        Ok(None) | Err(_) => {
            received.store(0, Ordering::Relaxed);
        }
    }
    fetch_limited_counted(&source, url, cancel, FILE_LIMIT, received, on_bytes)
}
/// 一批文件下载到一半时的进度：已完成几个、一共几个，已经落盘多少字节，
/// 以及当前在下的文件名（界面拿它显示「正在下 xxx.jar」，卡住时一看就知道）。
#[derive(Debug, Clone, Default)]
pub struct BatchProgress {
    pub done: usize,
    pub total: usize,
    pub bytes: u64,
    pub file: String,
}

/// 一批文件里的一个：多个候选地址（官方 + 镜像）按顺序试，可选 SHA1 校验。
#[derive(Debug, Clone)]
pub struct DownloadJob {
    pub urls: Vec<String>,
    pub dest: PathBuf,
    /// 包里写明的字节数，用来放宽总超时。
    pub size: Option<u64>,
    /// 校验值：下完对不上就删掉，换下一个地址再试。
    pub sha1: Option<String>,
}

/// 给界面看的路径：`fs::canonicalize` 在 Windows 上会带出 `\\?\` 前缀，
/// 直接显示、直接塞进确认框都很难看，这里统一抹掉。
pub fn tidy_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => rest.to_owned(),
        None => text.into_owned(),
    }
}

/// PCL 那套镜像改写：官方地址换成 `mod.mcimirror.top` 上的同一路径。
///
/// 只认 Modrinth CDN 与 ForgeCDN 三个域名，其它（比如 Mojang 的）原样返回 `None`。
pub fn mirror_url(url: &str) -> Option<String> {
    let stripped = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = stripped.split_once('/')?;
    let mirror = match host.to_ascii_lowercase().as_str() {
        "cdn.modrinth.com"
        | "edge.forgecdn.net"
        | "mediafilez.forgecdn.net"
        | "media.forgecdn.net" => "mod.mcimirror.top",
        _ => return None,
    };
    Some(format!("https://{mirror}/{path}"))
}

/// 内存里这段字节的 SHA1（十六进制小写），修补核心时拿来对比。
pub fn bytes_sha1(bytes: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// 文件的 SHA1（十六进制小写）：整合包里的每个文件都带校验值。
pub(crate) fn file_sha1(path: &Path) -> Result<String> {
    use sha1::{Digest, Sha1};
    let mut file = File::open(path)
        .map_err(|error| DownloadError::Invalid(format!("读取 {} 失败：{error}", path.display())))?;
    let mut hasher = Sha1::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| DownloadError::Invalid(format!("读取 {} 失败：{error}", path.display())))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn host_of(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()?
        .host_str()
        .map(|host| host.to_ascii_lowercase())
}

/// 一条候选地址实际落到哪台机器；同一个域名只探一次，探不到就按原域名算。
fn resolved_host(
    source: &Http,
    url: &str,
    cache: &mut HashMap<String, Option<String>>,
) -> Option<String> {
    let host = host_of(url)?;
    if let Some(known) = cache.get(&host) {
        return known.clone();
    }
    let resolved = source
        .final_host(url)
        .ok()
        .flatten()
        .or_else(|| Some(host.clone()));
    cache.insert(host, resolved.clone());
    resolved
}

/// 把「落到同一台机器」的候选删掉，只留下第一条。
///
/// 竞速的全部价值在于两条路走不同的机器；实测 `mod.mcimirror.top` 只是 302 跳板，
/// Modrinth 回 `cdn.modrinth.com`，CurseForge 走 `edge.forgecdn.net` →
/// `mediafilez.forgecdn.net`——和官方地址是同一批机器。两条路同时下等于把同一个
/// 文件下一遍半，带宽有限的链路上整包直接慢一倍，进度里的字节也跟着翻倍。
///
/// 每个域名只发一次 HEAD（跟到底看 `Response::url`），探不到就保守保留。
fn drop_redirected_candidates(source: &Http, items: &[DownloadJob]) -> Vec<DownloadJob> {
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    items
        .iter()
        .map(|job| {
            let Some(first) = job.urls.first() else {
                return job.clone();
            };
            let reference = resolved_host(source, first, &mut cache);
            let first_routed = trusted_url(first).ok().map(|target| source.route(target));
            let urls: Vec<String> = job
                .urls
                .iter()
                .enumerate()
                .filter(|(index, url)| {
                    if *index == 0 {
                        return true;
                    }
                    if let (Some(routed), Ok(target)) = (&first_routed, trusted_url(url)) {
                        if source.route(target) == *routed {
                            return false;
                        }
                    }
                    match (&reference, resolved_host(source, url, &mut cache)) {
                        (Some(a), Some(b)) => a.as_str() != b.as_str(),
                        _ => true,
                    }
                })
                .map(|(_, url)| url.clone())
                .collect();
            DownloadJob {
                urls,
                ..job.clone()
            }
        })
        .collect()
}

/// 一批下载里每条候选地址的健康状况，对应 PCL 的 `SourcesOnce` / `IsFailed`。
///
/// PCL 的做法（`pcl_ModNet.vb:1160-1190`）：一条源出硬错误（不支持分段 / 404 /
/// 502 / 403 / 429）立刻停用；普通失败累计到 `clamp(线程数, 5, 30)` 次也停用，
/// 免得每个文件都去撞一次死镜像。停用只作用于这一批，下一批重新试。
struct Health {
    /// 普通失败累计到这个次数就停用。
    limit: usize,
    fails: Mutex<HashMap<String, usize>>,
    dead: Mutex<HashSet<String>>,
}

impl Health {
    fn new(workers: usize) -> Self {
        Self {
            limit: workers.clamp(5, 30),
            fails: Mutex::new(HashMap::new()),
            dead: Mutex::new(HashSet::new()),
        }
    }

    fn usable(&self, url: &str) -> bool {
        self.dead
            .lock()
            .map(|dead| !dead.contains(url))
            .unwrap_or(true)
    }

    fn stop_using(&self, url: &str) {
        if let Ok(mut dead) = self.dead.lock() {
            dead.insert(url.to_owned());
        }
    }

    /// 记一次失败；到达阈值或命中硬错误就停用这条地址。
    fn failed(&self, url: &str, error: &DownloadError) {
        if !self.usable(url) {
            return;
        }
        let message = error.to_string();
        let hard = is_hard_failure(&message, url);
        let Ok(mut fails) = self.fails.lock() else {
            return;
        };
        let count = {
            let entry = fails.entry(url.to_owned()).or_insert(0);
            *entry += 1;
            *entry
        };
        drop(fails);
        if hard || count >= self.limit {
            self.stop_using(url);
        }
    }
}

/// 硬错误：重试再多次也没用，直接停用这条地址。
fn is_hard_failure(message: &str, url: &str) -> bool {
    if message.contains("不支持分段") {
        return true;
    }
    // 404 / 416 / 502 对任何源都是「这个文件它没有」；403 / 429 在镜像上是限流，
    // 官方源上则多半是地址失效，和 PCL 一样只对非 bmclapi 停用。
    if ["404", "416", "502"].iter().any(|code| message.contains(code)) {
        return true;
    }
    let throttled = ["403", "429"].iter().any(|code| message.contains(code));
    throttled && !url.contains("bmclapi")
}

/// 把一批文件下到磁盘：复用同一个连接池、并行抓取，失败的文件只记下来不中断整批。
///
/// 整合包动辄上百个文件，逐个 `fetch_file_bytes` 会为每个文件重建 client（重新
/// DNS + TLS 握手 + 镜像探测），实测慢到 20 秒一个；这里共用 [`Http`] 并开
/// [`MODPACK_PARALLEL`] 个线程。
///
/// 走 [`fetch_into_part`] 而不是整包读进内存：单个文件边下边写 `<name>.part`，
/// 断了能接着下，且只设读超时——7 MB 的 jar 在慢速校园网要下两分钟，
/// 按总时长卡 45 秒只会让它一遍遍从头重试，看起来就是「装到一半不动了」。
pub fn fetch_files_to(
    items: &[DownloadJob],
    cancel: &CancellationToken,
    threads: usize,
    strategy: DownloadStrategy,
    progress: impl Fn(&BatchProgress) + Sync,
) -> Result<(usize, Vec<String>)> {
    if items.is_empty() {
        return Ok((0, Vec::new()));
    }
    let source = Http::streaming(strategy)?;
    // 竞速前先剔掉「跳回同一个主机」的候选，否则两条路抢的是同一份带宽。
    let trimmed;
    let jobs: &[DownloadJob] = if strategy == DownloadStrategy::Race
        && items.iter().any(|job| job.urls.len() > 1)
    {
        trimmed = drop_redirected_candidates(&source, items);
        &trimmed
    } else {
        items
    };
    let done = AtomicUsize::new(0);
    let bytes = AtomicU64::new(0);
    let last = Mutex::new(Instant::now() - Duration::from_secs(1));
    let failed: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());
    let total = jobs.len();
    // 线程数由设置决定（1–256，实际 2–257，默认 65）；比文件数多没意义。
    let workers = threads.clamp(1, 257).min(total).max(1);
    let health = Health::new(workers);

    // 一个文件里每读 64 KB 都会回调一次，全量转发会把 IPC 打爆；
    // 按 300 ms 节流，界面仍能看到字节在涨。
    let tick = |file: &str| {
        let due = last
            .lock()
            .map(|mut at| {
                if at.elapsed() >= Duration::from_millis(300) {
                    *at = Instant::now();
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if due {
            progress(&BatchProgress {
                done: done.load(Ordering::Relaxed),
                total,
                bytes: bytes.load(Ordering::Relaxed),
                file: file.to_owned(),
            });
        }
    };

    {
        // 一轮下载：`order` 里的下标交给 workers 抢着做。`count` 为 false 时不再累加
        // 「已完成个数」——补下轮不该让进度条从中间再走一遍。
        let run_pass = |order: &[usize], count: bool| {
            let next = AtomicUsize::new(0);
            // 正在处理的文件数：统计线程靠它判断这一轮是不是真的空了。
            let busy = AtomicUsize::new(0);
            let health = &health;
            thread::scope(|scope| {
                // 后台统计（PCL `RefreshStat`）：每 100 ms 采一次整批速度，
                // 速度下限取近 1 秒平均的 85%，只升不降。
                let counted = &bytes;
                scope.spawn(|| {
                    let mut samples: std::collections::VecDeque<u64> =
                        std::collections::VecDeque::with_capacity(31);
                    let mut last_bytes = counted.load(Ordering::Relaxed);
                    let mut last_at = Instant::now();
                    loop {
                        thread::sleep(Duration::from_millis(100));
                        if cancel.is_cancelled() {
                            return;
                        }
                        let now = Instant::now();
                        let current = counted.load(Ordering::Relaxed);
                        let elapsed = now.duration_since(last_at).as_secs_f64().max(0.001);
                        let speed = (current.saturating_sub(last_bytes) as f64 / elapsed) as u64;
                        last_bytes = current;
                        last_at = now;
                        source.rate.set_speed(speed);
                        samples.push_back(speed);
                        if samples.len() > 30 {
                            samples.pop_front();
                        }
                        if samples.len() >= 10 {
                            let recent = samples.len().min(10);
                            let sum: u64 = samples.iter().rev().take(recent).sum();
                            source.rate.raise_low(sum as f64 / recent as f64);
                        }
                        if next.load(Ordering::Relaxed) >= order.len()
                            && busy.load(Ordering::Relaxed) == 0
                        {
                            return;
                        }
                    }
                });
                for _ in 0..workers.min(order.len()) {
                    scope.spawn(|| loop {
                        if cancel.is_cancelled() {
                            return;
                        }
                        let slot = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&index) = order.get(slot) else {
                            return;
                        };
                        busy.fetch_add(1, Ordering::Relaxed);
                        let job = &jobs[index];
                        let name = job
                            .dest
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        tick(&name);
                        let result = fetch_job(
                            job,
                            cancel,
                            &source,
                            &bytes,
                            health,
                            &name,
                            strategy,
                            || tick(&name),
                        );
                        busy.fetch_sub(1, Ordering::Relaxed);
                        match result {
                            Ok(()) => {}
                            Err(DownloadError::Cancelled) => return,
                            Err(error) => {
                                let url = job.urls.first().cloned().unwrap_or_default();
                                if let Ok(mut list) = failed.lock() {
                                    list.push((index, format!("{url}: {error}")));
                                }
                            }
                        }
                        if count {
                            let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                            progress(&BatchProgress {
                                done: finished,
                                total,
                                bytes: bytes.load(Ordering::Relaxed),
                                file: name,
                            });
                        }
                    });
                }
            });
        };

        let order: Vec<usize> = (0..total).collect();
        run_pass(&order, true);

        // 整包几十上百个文件，一两个超时不该让用户从头再来：失败的那几个再补一轮，
        // 补下时进度只更新「正在下哪个文件」，不重复计数。
        let retry: Vec<usize> = failed
            .lock()
            .map(|list| list.iter().map(|(index, _)| *index).collect())
            .unwrap_or_default();
        if !retry.is_empty() && !cancel.is_cancelled() {
            if let Ok(mut list) = failed.lock() {
                list.clear();
            }
            run_pass(&retry, false);
        }
    }

    // `written` 只数真正下成的：之前把失败也算进去，调用方看数字会以为全好了。
    let failures: Vec<String> = failed
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .map(|(_, message)| message)
        .collect();
    let written = total.saturating_sub(failures.len());
    Ok((written, failures))
}

/// 两个地址同时下同一个文件，先完成的赢；输掉的立刻取消。
fn race_job(
    job: &DownloadJob,
    cancel: &CancellationToken,
    source: &Http,
    bytes: &AtomicU64,
    health: &Health,
    name: &str,
    tick: &(impl Fn() + Sync),
) -> Result<()> {
    let racers = job.urls.len().min(2);
    let winner: Mutex<Option<usize>> = Mutex::new(None);
    let tokens: Vec<CancellationToken> = (0..racers).map(|_| CancellationToken::default()).collect();
    let parts: Vec<std::path::PathBuf> = (0..racers)
        .map(|index| job.dest.with_extension(format!("race{index}.part")))
        .collect();

    thread::scope(|scope| {
        for (index, url) in job.urls.iter().take(racers).enumerate() {
            let token = tokens[index].clone();
            let part = parts[index].clone();
            let winner = &winner;
            let tokens = &tokens;
            scope.spawn(move || {
                let timeout = transfer_timeout(job.size);
                // 两条路各记各的字节：都往共享计数里加会让进度凭空翻倍，
                // 赢家定了才把这一个文件真正收到的字节并进批次。
                let mine = AtomicU64::new(0);
                let done = stream_to(url, &part, job.size, &token, source, &mine, timeout, tick)
                    .and_then(|()| match job.sha1.as_deref() {
                        Some(expected) => match file_sha1(&part) {
                            Ok(actual) if actual.eq_ignore_ascii_case(expected) => Ok(()),
                            Ok(actual) => Err(DownloadError::Integrity(format!(
                                "{name}: 校验不通过（期望 {expected}，实际 {actual}）"
                            ))),
                            Err(error) => Err(error),
                        },
                        None => Ok(()),
                    });
                match &done {
                    // 硬错误（404 / 不支持分段）说明这条地址对这个文件没戏了：整批停用。
                    Err(DownloadError::Cancelled) => {}
                    Err(error) => health.failed(url, error),
                    Ok(()) => {}
                }
                if done.is_ok() {
                    // 第一个成功的人点名，其他人立刻停手
                    if let Ok(mut slot) = winner.lock() {
                        if slot.is_none() {
                            *slot = Some(index);
                            bytes.fetch_add(mine.load(Ordering::Relaxed), Ordering::Relaxed);
                            for (other, token) in tokens.iter().enumerate() {
                                if other != index {
                                    token.cancel();
                                }
                            }
                        }
                    }
                }
            });
        }
    });

    let picked = winner.lock().ok().and_then(|slot| *slot);
    let Some(index) = picked else {
        for part in &parts {
            let _ = fs::remove_file(part);
        }
        return Err(DownloadError::Network(format!("{name}: 竞速的两条线路都没下成")));
    };
    if let Some(parent) = job.dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let moved = fs::rename(&parts[index], &job.dest)
        .map_err(|e| DownloadError::Invalid(format!("写入 {} 失败：{e}", job.dest.display())));
    for (other, part) in parts.iter().enumerate() {
        if other != index {
            let _ = fs::remove_file(part);
        }
    }
    cancel.check()?;
    moved
}

/// 一个文件：按候选地址顺序试，下完校验 SHA1；全失败才记一次失败。
fn fetch_job(
    job: &DownloadJob,
    cancel: &CancellationToken,
    source: &Http,
    bytes: &AtomicU64,
    health: &Health,
    name: &str,
    strategy: DownloadStrategy,
    tick: impl Fn() + Sync,
) -> Result<()> {
    // 已经有这个文件、而且 SHA1 对得上就直接跳过：重装整合包、断线重来都靠它省时间
    // （PCL 的 FileChecker 也是这个思路）。
    if let Some(expected) = job.sha1.as_deref() {
        if job.dest.is_file() {
            if let Ok(actual) = file_sha1(&job.dest) {
                if actual.eq_ignore_ascii_case(expected) {
                    if let Some(size) = job.size {
                        bytes.fetch_add(size, Ordering::Relaxed);
                    }
                    tick();
                    return Ok(());
                }
            }
        }
    }
    // 竞速策略下，凡是有两条候选（官方 + 镜像）的文件都同时开下，谁先到用谁。
    // 只串行重试的话，遇到「不报错但很慢」的线路要等满超时。
    let usable = job.urls.iter().filter(|url| health.usable(url)).count();
    if strategy == DownloadStrategy::Race && usable >= 2 {
        match race_job(job, cancel, source, bytes, health, name, &tick) {
            Ok(()) => return Ok(()),
            Err(DownloadError::Cancelled) => return Err(DownloadError::Cancelled),
            Err(_) => {} // 竞速没成功就退回串行，串行还有完整重试
        }
    }
    let mut last: Option<DownloadError> = None;
    for url in &job.urls {
        if !health.usable(url) {
            continue;
        }
        match download_to(url, &job.dest, job.size, cancel, source, bytes, || tick()) {
            Ok(()) => {
                if let Some(expected) = job.sha1.as_deref() {
                    match file_sha1(&job.dest) {
                        Ok(actual) if actual.eq_ignore_ascii_case(expected) => {}
                        Ok(actual) => {
                            discard(&job.dest);
                            last = Some(DownloadError::Integrity(format!(
                                "{name}: 校验不通过（期望 {expected}，实际 {actual}）"
                            )));
                            continue;
                        }
                        Err(error) => {
                            discard(&job.dest);
                            last = Some(error);
                            continue;
                        }
                    }
                }
                return Ok(());
            }
            Err(DownloadError::Cancelled) => return Err(DownloadError::Cancelled),
            Err(error) => {
                health.failed(url, &error);
                last = Some(error);
            }
        }
    }
    Err(last.unwrap_or_else(|| DownloadError::Network(format!("{name}: 没有可用的下载地址"))))
}

/// 单个整合包文件：边读边写到目标文件，只设读超时，网络错误重试几次。
///
/// 不用 `fetch_into_part` 那套：它的「太慢就换源」判定按单连接速率算，
/// 而整合包是十几个连接分带宽，单连接天然只有十来 KB/s，会一直误判。
fn download_to(
    url: &str,
    dest: &Path,
    size: Option<u64>,
    cancel: &CancellationToken,
    source: &Http,
    bytes: &AtomicU64,
    tick: impl Fn() + Sync,
) -> Result<()> {
    // 大文件先走 Range 分块（每块两条路竞速）；服务器不支持分段或失败就退回单流。
    // 整批速度已经到下限（PCL 的 Low）时不再分块：多开连接只是抢同一条带宽。
    if !source.saturated() {
        match fetch_chunked_to(url, dest, size, cancel, source, bytes, &tick) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(DownloadError::Cancelled) => return Err(DownloadError::Cancelled),
            Err(error) => {
                let _ = error;
            }
        }
    }
    let timeout = transfer_timeout(size);
    let part = part_path(dest);
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match stream_to(url, &part, size, cancel, source, bytes, timeout, &tick) {
            Ok(()) => {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(|e| DownloadError::Core(io_error(parent, e)))?;
                }
                return fs::rename(&part, dest).map_err(|e| {
                    DownloadError::Invalid(format!(
                        "写入 {} 失败：{e}",
                        dest.display()
                    ))
                });
            }
            Err(DownloadError::Cancelled) => return Err(DownloadError::Cancelled),
            Err(error) => {
                let network = matches!(error, DownloadError::Network(_));
                if !network || attempt >= MAX_ATTEMPTS {
                    return Err(error);
                }
                thread::sleep(Duration::from_millis(500u64 << (attempt - 1).min(3)));
            }
        }
    }
}

/// 每块内部同时试几条路（官方 / 镜像），先下完的那条算赢。
const CHUNK_RACERS: usize = 2;

/// 把文件切成互不重叠、覆盖 `[0, size)` 的闭区间。
///
/// 小于阈值返回空（表示不要分块）；块数按 `CHUNK_THRESHOLD` 折算并夹在
/// `CHUNK_MIN_PARTS`..`CHUNK_MAX_PARTS`，末块一定以 `size - 1` 结束。
pub(crate) fn plan_chunks(size: u64) -> Vec<(u64, u64)> {
    if size < CHUNK_THRESHOLD {
        return Vec::new();
    }
    let count = ((size / CHUNK_THRESHOLD) as usize).clamp(CHUNK_MIN_PARTS, CHUNK_MAX_PARTS);
    let step = size.div_ceil(count as u64);
    let mut chunks = Vec::with_capacity(count);
    let mut start = 0u64;
    while start < size {
        let end = (start + step).min(size) - 1;
        chunks.push((start, end));
        start = end + 1;
    }
    chunks
}

/// 下载一个闭区间到 `path`，长度必须正好是 `end - start + 1`。
fn span_to(
    source: &Http,
    url: &str,
    start: u64,
    end: u64,
    path: &Path,
    cancel: &CancellationToken,
) -> Result<u64> {
    let (mut response, partial) = source.request_span(url, start, end)?;
    if !partial {
        return Err(DownloadError::Invalid(format!(
            "{url}: 服务器不支持分段下载"
        )));
    }
    let mut file = File::create(path)
        .map_err(|e| DownloadError::Invalid(format!("写入 {} 失败：{e}", path.display())))?;
    let want = end - start + 1;
    let mut chunk = [0u8; 262144];
    let mut written = 0u64;
    loop {
        cancel.check()?;
        let read = response
            .read(&mut chunk)
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        if read == 0 {
            break;
        }
        source.throttle(read as u64);
        written += read as u64;
        if written > want {
            return Err(DownloadError::Integrity(url.to_owned()));
        }
        file.write_all(&chunk[..read])
            .map_err(|e| DownloadError::Invalid(format!("写入 {} 失败：{e}", path.display())))?;
    }
    file.flush()
        .map_err(|e| DownloadError::Invalid(format!("写入 {} 失败：{e}", path.display())))?;
    if written != want {
        return Err(DownloadError::Network(format!(
            "{url}: 分段只收到 {written}/{want} 字节"
        )));
    }
    Ok(written)
}

/// 分块临时文件：`<part>.c{块号}r{路号}`，成功后逐个删掉。
fn chunk_temp(part: &Path, index: usize, racer: usize) -> PathBuf {
    let name = match part.file_name().and_then(|name| name.to_str()) {
        Some(name) => format!("{name}.c{index}r{racer}"),
        None => format!("chunk.c{index}r{racer}"),
    };
    part.with_file_name(name)
}

/// 把一个闭区间读进内存（打包这类「不落盘」的调用用）。
fn span_bytes(
    source: &Http,
    url: &str,
    start: u64,
    end: u64,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    let (mut response, partial) = source.request_span(url, start, end)?;
    if !partial {
        return Err(DownloadError::Invalid(format!(
            "{url}: 服务器不支持分段下载"
        )));
    }
    let want = (end - start + 1) as usize;
    let mut out = Vec::with_capacity(want);
    let mut chunk = [0u8; 262144];
    loop {
        cancel.check()?;
        let read = response
            .read(&mut chunk)
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        if read == 0 {
            break;
        }
        source.throttle(read as u64);
        out.extend_from_slice(&chunk[..read]);
        if out.len() > want {
            return Err(DownloadError::Integrity(url.to_owned()));
        }
    }
    if out.len() != want {
        return Err(DownloadError::Network(format!(
            "{url}: 分段只收到 {}/{want} 字节",
            out.len()
        )));
    }
    Ok(out)
}

/// [`span_bytes`] 的带进度版本：每读到一段就把「这块已收的最大字节数」推到 `best`，
/// 只把比之前多出来的部分加进 `received`——同一块的两条路同时下，进度不能翻倍。
#[allow(clippy::too_many_arguments)]
fn span_bytes_counted(
    source: &Http,
    url: &str,
    start: u64,
    end: u64,
    cancel: &CancellationToken,
    best: &AtomicU64,
    received: &AtomicU64,
    on_bytes: &(dyn Fn() + Sync),
) -> Result<Vec<u8>> {
    let (mut response, partial) = source.request_span(url, start, end)?;
    if !partial {
        return Err(DownloadError::Invalid(format!("{url}: 服务器不支持分段下载")));
    }
    let want = (end - start + 1) as usize;
    let mut out = Vec::with_capacity(want);
    let mut chunk = [0u8; 262144];
    loop {
        cancel.check()?;
        let read = response
            .read(&mut chunk)
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        if read == 0 {
            break;
        }
        source.throttle(read as u64);
        out.extend_from_slice(&chunk[..read]);
        if out.len() > want {
            return Err(DownloadError::Integrity(url.to_owned()));
        }
        let mine = out.len() as u64;
        let earlier = best.fetch_max(mine, Ordering::Relaxed);
        if mine > earlier {
            received.fetch_add(mine - earlier, Ordering::Relaxed);
            on_bytes();
        }
    }
    if out.len() != want {
        return Err(DownloadError::Network(format!(
            "{url}: 分段只收到 {}/{want} 字节",
            out.len()
        )));
    }
    Ok(out)
}

/// 大文件分块读进内存：每块对前两条路（官方 / 镜像）竞速，全部到齐后按顺序拼接。
///
/// 返回 `Ok(None)` 表示「太大 / 服务器不认 Range / 只有一块」，调用方退回单流下载。
fn fetch_chunked_bytes(
    source: &Http,
    url: &str,
    cancel: &CancellationToken,
    limit: u64,
) -> Result<Option<Vec<u8>>> {
    let Some(total) = source.probe_size(url)? else {
        return Ok(None);
    };
    if total > limit {
        return Ok(None);
    }
    let chunks = plan_chunks(total);
    if chunks.len() < 2 {
        return Ok(None);
    }
    let candidates = source.candidates(url)?;
    if candidates.is_empty() {
        return Ok(None);
    }
    let racers = candidates.len().min(CHUNK_RACERS);
    let winners: Vec<AtomicUsize> = (0..chunks.len())
        .map(|_| AtomicUsize::new(usize::MAX))
        .collect();
    let pieces: Vec<Mutex<Option<Vec<u8>>>> =
        (0..chunks.len()).map(|_| Mutex::new(None)).collect();
    thread::scope(|scope| {
        for (index, (start, end)) in chunks.iter().copied().enumerate() {
            for racer in 0..racers {
                let winners = &winners;
                let pieces = &pieces;
                let url = candidates[racer].as_str().to_owned();
                scope.spawn(move || {
                    if winners[index].load(Ordering::Relaxed) != usize::MAX {
                        return;
                    }
                    let Ok(bytes) = span_bytes(source, &url, start, end, cancel) else {
                        return;
                    };
                    let won = winners[index]
                        .compare_exchange(
                            usize::MAX,
                            racer,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                        )
                        .is_ok();
                    if won {
                        if let Ok(mut slot) = pieces[index].lock() {
                            *slot = Some(bytes);
                        }
                    }
                });
            }
        }
    });
    cancel.check()?;
    if winners
        .iter()
        .any(|winner| winner.load(Ordering::Relaxed) == usize::MAX)
    {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(total as usize);
    for (index, slot) in pieces.into_iter().enumerate() {
        match slot.into_inner().ok().flatten() {
            Some(bytes) => out.extend_from_slice(&bytes),
            None => {
                let _ = index;
                return Ok(None);
            }
        }
    }
    if out.len() as u64 != total {
        return Ok(None);
    }
    Ok(Some(out))
}

/// [`fetch_chunked_bytes`] 的带进度版本：块内两条路只算领先那条的字节。
fn fetch_chunked_bytes_counted(
    source: &Http,
    url: &str,
    cancel: &CancellationToken,
    limit: u64,
    received: &AtomicU64,
    on_bytes: &(dyn Fn() + Sync),
) -> Result<Option<Vec<u8>>> {
    let Some(total) = source.probe_size(url)? else {
        return Ok(None);
    };
    if total > limit {
        return Ok(None);
    }
    let chunks = plan_chunks(total);
    if chunks.len() < 2 {
        return Ok(None);
    }
    let candidates = source.candidates(url)?;
    if candidates.is_empty() {
        return Ok(None);
    }
    let racers = candidates.len().min(CHUNK_RACERS);
    let winners: Vec<AtomicUsize> = (0..chunks.len())
        .map(|_| AtomicUsize::new(usize::MAX))
        .collect();
    let bests: Vec<AtomicU64> = (0..chunks.len()).map(|_| AtomicU64::new(0)).collect();
    let pieces: Vec<Mutex<Option<Vec<u8>>>> =
        (0..chunks.len()).map(|_| Mutex::new(None)).collect();
    thread::scope(|scope| {
        for (index, (start, end)) in chunks.iter().copied().enumerate() {
            for racer in 0..racers {
                let winners = &winners;
                let pieces = &pieces;
                let best = &bests[index];
                let url = candidates[racer].as_str().to_owned();
                scope.spawn(move || {
                    if winners[index].load(Ordering::Relaxed) != usize::MAX {
                        return;
                    }
                    let Ok(bytes) =
                        span_bytes_counted(source, &url, start, end, cancel, best, received, on_bytes)
                    else {
                        return;
                    };
                    let won = winners[index]
                        .compare_exchange(
                            usize::MAX,
                            racer,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                        )
                        .is_ok();
                    if won {
                        if let Ok(mut slot) = pieces[index].lock() {
                            *slot = Some(bytes);
                        }
                    }
                });
            }
        }
    });
    cancel.check()?;
    if winners
        .iter()
        .any(|winner| winner.load(Ordering::Relaxed) == usize::MAX)
    {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(total as usize);
    for slot in pieces {
        match slot.into_inner().ok().flatten() {
            Some(bytes) => out.extend_from_slice(&bytes),
            None => return Ok(None),
        }
    }
    if out.len() as u64 != total {
        return Ok(None);
    }
    Ok(Some(out))
}

/// 大文件分块下载：每块独立 Range 请求，块内对前两条路（官方 / 镜像）竞速，
/// 先下完的那条生效；全部块到齐后按偏移拼成 `part`，再改名成目标文件。
///
/// 返回 `Ok(true)` 表示已经写成 `dest`；`Ok(false)` 表示「不支持 / 失败」，
/// 调用方退回原来的单流下载（不报错，免得因为服务器不认 Range 就装不了）。
fn fetch_chunked_to(
    url: &str,
    dest: &Path,
    size: Option<u64>,
    cancel: &CancellationToken,
    source: &Http,
    bytes: &AtomicU64,
    tick: &(impl Fn() + Sync),
) -> Result<bool> {
    let total = match size {
        Some(size) if size >= CHUNK_THRESHOLD => size,
        _ => match source.probe_size(url)? {
            Some(size) if size >= CHUNK_THRESHOLD => size,
            _ => return Ok(false),
        },
    };
    let chunks = plan_chunks(total);
    if chunks.len() < 2 {
        return Ok(false);
    }
    let candidates = source.candidates(url)?;
    if candidates.is_empty() {
        return Ok(false);
    }
    let racers = candidates.len().min(CHUNK_RACERS);
    let part = part_path(dest);
    if let Some(parent) = part.parent() {
        fs::create_dir_all(parent).map_err(|e| DownloadError::Core(io_error(parent, e)))?;
    }
    let winners: Vec<AtomicUsize> = (0..chunks.len())
        .map(|_| AtomicUsize::new(usize::MAX))
        .collect();
    let failed = AtomicBool::new(false);
    thread::scope(|scope| {
        for (index, (start, end)) in chunks.iter().copied().enumerate() {
            for (racer, candidate) in candidates.iter().take(racers).enumerate() {
                let winners = &winners;
                let failed = &failed;
                let temp = chunk_temp(&part, index, racer);
                let url = candidate.as_str().to_owned();
                scope.spawn(move || {
                    if failed.load(Ordering::Relaxed) {
                        return;
                    }
                    match span_to(source, &url, start, end, &temp, cancel) {
                        Ok(written) => {
                            let won = winners[index]
                                .compare_exchange(
                                    usize::MAX,
                                    racer,
                                    Ordering::Relaxed,
                                    Ordering::Relaxed,
                                )
                                .is_ok();
                            if won {
                                // 只把赢下来的那条路的字节计进进度，输家不计，免得虚高。
                                bytes.fetch_add(written, Ordering::Relaxed);
                            } else {
                                let _ = fs::remove_file(&temp);
                            }
                        }
                        Err(_) => {
                            let _ = fs::remove_file(&temp);
                        }
                    }
                });
            }
        }
    });
    let cleanup = || {
        for index in 0..chunks.len() {
            for racer in 0..racers {
                let _ = fs::remove_file(chunk_temp(&part, index, racer));
            }
        }
    };
    if cancel.is_cancelled() {
        cleanup();
        let _ = fs::remove_file(&part);
        cancel.check()?;
    }
    if failed.load(Ordering::Relaxed)
        || winners
            .iter()
            .any(|winner| winner.load(Ordering::Relaxed) == usize::MAX)
    {
        cleanup();
        let _ = fs::remove_file(&part);
        return Ok(false);
    }
    let mut out = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&part)
        .map_err(|e| DownloadError::Invalid(format!("写入 {} 失败：{e}", part.display())))?;
    out.set_len(total)
        .map_err(|e| DownloadError::Invalid(format!("写入 {} 失败：{e}", part.display())))?;
    for (index, (start, _end)) in chunks.iter().enumerate() {
        let racer = winners[index].load(Ordering::Relaxed);
        let temp = chunk_temp(&part, index, racer);
        let mut chunk = File::open(&temp)
            .map_err(|e| DownloadError::Invalid(format!("读取 {} 失败：{e}", temp.display())))?;
        out.seek(SeekFrom::Start(*start))
            .map_err(|e| DownloadError::Invalid(format!("写入 {} 失败：{e}", part.display())))?;
        std::io::copy(&mut chunk, &mut out)
            .map_err(|e| DownloadError::Invalid(format!("写入 {} 失败：{e}", part.display())))?;
        let _ = fs::remove_file(&temp);
    }
    out.flush()
        .map_err(|e| DownloadError::Invalid(format!("写入 {} 失败：{e}", part.display())))?;
    drop(out);
    cleanup();
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| DownloadError::Core(io_error(parent, e)))?;
    }
    fs::rename(&part, dest)
        .map_err(|e| DownloadError::Invalid(format!("写入 {} 失败：{e}", dest.display())))?;
    tick();
    Ok(true)
}

/// 删掉目标文件和它的临时文件（校验不通过时用）。
fn discard(dest: &Path) {
    let _ = fs::remove_file(dest);
    let _ = fs::remove_file(part_path(dest));
}

/// 单个文件的总超时：留给「慢但一直在动」的链接，又不至于卡到天荒地老。
/// 按 10 KB/s 的保底速率折算（校园网到 cdn.modrinth.com 实测就只有 15–20 KB/s），
/// 多给 45 秒握手余量，最多 20 分钟；超时后靠 `.part` 断点续传接着下，不会白跑。
fn transfer_timeout(size: Option<u64>) -> Duration {
    const FLOOR: u64 = 10 * 1024;
    let extra = size.unwrap_or(64 * 1024 * 1024) / FLOOR;
    Duration::from_secs((45 + extra).min(1200))
}

fn stream_to(
    url: &str,
    part: &Path,
    size: Option<u64>,
    cancel: &CancellationToken,
    source: &Http,
    bytes: &AtomicU64,
    timeout: Duration,
    tick: &dyn Fn(),
) -> Result<()> {
    const LIMIT: u64 = 512 * 1024 * 1024;
    if let Some(parent) = part.parent() {
        fs::create_dir_all(parent).map_err(|e| DownloadError::Core(io_error(parent, e)))?;
    }
    // 上一次中断留下的半截文件：带了 Range 就从断点续，服务器不认就整份重下。
    let mut offset = fs::metadata(part).map(|meta| meta.len()).unwrap_or(0);
    if let Some(size) = size {
        if offset > size {
            let _ = fs::remove_file(part);
            offset = 0;
        }
        if offset > 0 && offset == size {
            return Ok(());
        }
    }
    let (mut response, partial) = match source.request_with(url, Some(offset), Some(timeout)) {
        Ok(pair) => pair,
        // 服务器不认这个断点：最常见的是上次中断留下的 `.part` 长度和这次响应对不上，
        // 带 Range 过去直接被 416 Range Not Satisfiable 打回来。丢掉半截文件整份重下，
        // 比让整个安装卡在这里报错强。
        Err(error) if offset > 0 => {
            let _ = fs::remove_file(part);
            offset = 0;
            match source.request_with(url, None, Some(timeout)) {
                Ok(pair) => pair,
                Err(_) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    };
    let resume = offset > 0 && partial;
    let mut written = if resume { offset } else { 0 };
    let mut file = if resume {
        OpenOptions::new()
            .append(true)
            .open(part)
            .map_err(|e| DownloadError::Invalid(format!("写入 {} 失败：{e}", part.display())))?
    } else {
        File::create(part)
            .map_err(|e| DownloadError::Invalid(format!("写入 {} 失败：{e}", part.display())))?
    };
    let mut chunk = [0u8; 262144];
    loop {
        cancel.check()?;
        let read = response
            .read(&mut chunk)
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        if read == 0 {
            break;
        }
        source.throttle(read as u64);
        written += read as u64;
        if written > LIMIT {
            return Err(DownloadError::Integrity(url.to_owned()));
        }
        file.write_all(&chunk[..read])
            .map_err(|e| DownloadError::Invalid(format!("写入 {} 失败：{e}", part.display())))?;
        bytes.fetch_add(read as u64, Ordering::Relaxed);
        tick();
    }
    cancel.check()?;
    // 这里以前每个文件都 sync_all()（fsync），一个整合包上千个文件就等于上千次刷盘，
    // 白等十几秒。写完交给操作系统的页缓存就行：真正的完整性靠后面的 SHA1 校验。
    file.flush()
        .map_err(|e| DownloadError::Invalid(format!("写入 {} 失败：{e}", part.display())))?;
    Ok(())
}
/// 元数据（清单/版本 json/校验和）很小但对稳定性最敏感，同样按 3 次重试。
fn fetch_limited(
    source: &impl Source,
    url: &str,
    cancel: &CancellationToken,
    limit: u64,
) -> Result<Vec<u8>> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match fetch_limited_once(source, url, cancel, limit) {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                if !matches!(error, DownloadError::Network(_)) || attempt >= MAX_ATTEMPTS {
                    return Err(error);
                }
                thread::sleep(Duration::from_millis(500u64 << (attempt - 1).min(3)));
            }
        }
    }
}
/// [`fetch_limited`] 的带进度版本：重试前把计数清零，免得同一份字节算两遍。
fn fetch_limited_counted(
    source: &impl Source,
    url: &str,
    cancel: &CancellationToken,
    limit: u64,
    received: &AtomicU64,
    on_bytes: &(dyn Fn() + Sync),
) -> Result<Vec<u8>> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match fetch_limited_once_counted(source, url, cancel, limit, received, on_bytes) {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                if !matches!(error, DownloadError::Network(_)) || attempt >= MAX_ATTEMPTS {
                    return Err(error);
                }
                received.store(0, Ordering::Relaxed);
                thread::sleep(Duration::from_millis(500u64 << (attempt - 1).min(3)));
            }
        }
    }
}
fn fetch_limited_once_counted(
    source: &impl Source,
    url: &str,
    cancel: &CancellationToken,
    limit: u64,
    received: &AtomicU64,
    on_bytes: &(dyn Fn() + Sync),
) -> Result<Vec<u8>> {
    trusted_url(url)?;
    cancel.check()?;
    let mut reader = source.open(url)?;
    let mut result = vec![];
    let mut chunk = [0u8; 65536];
    loop {
        cancel.check()?;
        let n = reader
            .read(&mut chunk)
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        source.throttle(n as u64);
        if result.len() as u64 + n as u64 > limit {
            return Err(DownloadError::Invalid("response exceeds size limit".into()));
        }
        result.extend_from_slice(&chunk[..n]);
        received.store(result.len() as u64, Ordering::Relaxed);
        on_bytes();
    }
    Ok(result)
}
fn fetch_limited_once(
    source: &impl Source,
    url: &str,
    cancel: &CancellationToken,
    limit: u64,
) -> Result<Vec<u8>> {
    trusted_url(url)?;
    cancel.check()?;
    let mut reader = source.open(url)?;
    let mut result = vec![];
    let mut chunk = [0u8; 65536];
    loop {
        cancel.check()?;
        let n = reader
            .read(&mut chunk)
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        source.throttle(n as u64);
        if result.len() as u64 + n as u64 > limit {
            return Err(DownloadError::Invalid("response exceeds size limit".into()));
        }
        result.extend_from_slice(&chunk[..n]);
    }
    Ok(result)
}
fn manifest(source: &impl Source, cancel: &CancellationToken) -> Result<RemoteVersionManifest> {
    let data = fetch_json(source, MANIFEST, cancel)?;
    let mut manifest: RemoteVersionManifest =
        serde_json::from_slice(&data).map_err(|e| DownloadError::Invalid(e.to_string()))?;
    // Historical April Fools IDs can contain spaces; omit IDs unsupported by our
    // strict path contract without failing the entire official catalog.
    manifest
        .versions
        .retain(|version| safe_component(&version.id));
    for version in &manifest.versions {
        trusted_url(&version.url)?;
        if version.sha1.as_deref().is_some_and(|h| !valid_hash(h)) {
            return Err(DownloadError::Invalid("invalid metadata SHA1".into()));
        }
    }
    Ok(manifest)
}
pub fn list_remote_versions(strategy: DownloadStrategy) -> Result<RemoteVersionManifest> {
    manifest(&Http::new(strategy)?, &CancellationToken::default())
}

pub fn list_fabric_loaders(
    game_version: &str,
    strategy: DownloadStrategy,
) -> Result<Vec<FabricLoaderVersion>> {
    if !safe_component(game_version) {
        return Err(DownloadError::Invalid("unsafe game version".into()));
    }
    // 列表在同一进程里不会变，缓存起来：切换加载器/来回点页面就不用再等网络。
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<FabricLoaderVersion>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache
        .lock()
        .ok()
        .and_then(|cache| cache.get(game_version).cloned())
    {
        return Ok(hit);
    }
    // 官方源在国外、BMCLAPI 在国内，两边拿的是同一份数据。
    // 两个请求同时发，谁先成功用谁（PCL 的「下载源按优先级赛跑」思路）。
    // 策略说「只走官方/官方优先」就把官方排前面，「镜像优先」就反过来；
    // 两个请求仍然同时发，只是判定顺序不同。
    let official = format!("{FABRIC_META}/{game_version}");
    let mirrored = format!("{FABRIC_META_BMCLAPI}/{game_version}");
    let urls = match strategy {
        DownloadStrategy::OfficialOnly | DownloadStrategy::OfficialFirst => [official, mirrored],
        _ => [mirrored, official],
    };
    let (tx, rx) = mpsc::channel();
    for url in urls {
        let tx = tx.clone();
        thread::spawn(move || {
            let _ = tx.send(fabric_catalog(&url));
        });
    }
    drop(tx);
    let mut last_error = None;
    for result in rx {
        match result {
            Ok(loaders) => {
                if let Ok(mut cache) = cache.lock() {
                    cache.insert(game_version.to_owned(), loaders.clone());
                }
                return Ok(loaders);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .unwrap_or_else(|| DownloadError::Network("no Fabric catalog source responded".into())))
}

/// Quilt 的加载器列表。Quilt 的元数据只有官方一家，不做竞速。
pub fn list_quilt_loaders(game_version: &str) -> Result<Vec<FabricLoaderVersion>> {
    if !safe_component(game_version) {
        return Err(DownloadError::Invalid("unsafe game version".into()));
    }
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<FabricLoaderVersion>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache
        .lock()
        .ok()
        .and_then(|cache| cache.get(game_version).cloned())
    {
        return Ok(hit);
    }
    let source = Http::new(strategy())?;
    let cancel = CancellationToken::default();
    let url = format!("{QUILT_META}/{game_version}");
    // Quilt 的元数据服务经常几十秒才回，偶发超时；重试几次比直接报错友好。
    let mut attempt = 0;
    let loaders = loop {
        match fabric_catalog_from(&source, &url, &cancel) {
            Ok(loaders) => break loaders,
            Err(error) => {
                attempt += 1;
                if attempt >= 3 {
                    return Err(error);
                }
                std::thread::sleep(std::time::Duration::from_millis(800));
            }
        }
    };
    if let Ok(mut cache) = cache.lock() {
        cache.insert(game_version.to_owned(), loaders.clone());
    }
    Ok(loaders)
}

/// Legacy Fabric 的加载器列表。只有官方一家，服务器在国内访问较慢，多试几次。
pub fn list_legacy_fabric_loaders(game_version: &str) -> Result<Vec<FabricLoaderVersion>> {
    if !safe_component(game_version) {
        return Err(DownloadError::Invalid("unsafe game version".into()));
    }
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<FabricLoaderVersion>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache
        .lock()
        .ok()
        .and_then(|cache| cache.get(game_version).cloned())
    {
        return Ok(hit);
    }
    let source = Http::new(strategy())?;
    let cancel = CancellationToken::default();
    let url = format!("{LEGACY_FABRIC_META}/{game_version}");
    let mut attempt = 0;
    let loaders = loop {
        match fabric_catalog_from(&source, &url, &cancel) {
            Ok(loaders) => break loaders,
            Err(error) => {
                attempt += 1;
                if attempt >= 3 {
                    return Err(error);
                }
                std::thread::sleep(std::time::Duration::from_millis(800));
            }
        }
    };
    if let Ok(mut cache) = cache.lock() {
        cache.insert(game_version.to_owned(), loaders.clone());
    }
    Ok(loaders)
}

fn fabric_catalog(url: &str) -> Result<Vec<FabricLoaderVersion>> {
    let source = Http::new(strategy())?;
    let cancel = CancellationToken::default();
    fabric_catalog_from(&source, url, &cancel)
}

/// 从「Fabric 目录 URL」读取加载器列表；测试里也用这个入口喂假数据。
#[cfg(test)]
fn fabric_loaders(
    source: &impl Source,
    game: &str,
    cancel: &CancellationToken,
) -> Result<Vec<FabricLoaderVersion>> {
    if !safe_component(game) {
        return Err(DownloadError::Invalid("unsafe game version".into()));
    }
    fabric_catalog_from(source, &format!("{FABRIC_META}/{game}"), cancel)
}

fn fabric_catalog_from(
    source: &impl Source,
    url: &str,
    cancel: &CancellationToken,
) -> Result<Vec<FabricLoaderVersion>> {
    #[derive(Deserialize)]
    struct Entry {
        loader: FabricLoaderVersion,
    }
    let bytes = fetch_json(source, url, cancel)?;
    let entries: Vec<Entry> = serde_json::from_slice(&bytes)
        .map_err(|e| DownloadError::Invalid(format!("invalid Fabric loader catalog: {e}")))?;
    entries
        .into_iter()
        .map(|entry| {
            if !safe_component(&entry.loader.version) {
                return Err(DownloadError::Invalid(
                    "unsafe Fabric loader version".into(),
                ));
            }
            Ok(entry.loader)
        })
        .collect()
}
/// 「profile 型」加载器：拿一份官方 profile json，把它的库拉下来写成本地版本。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProfileKind {
    Fabric,
    Quilt,
    LegacyFabric,
}

impl ProfileKind {
    fn label(self) -> &'static str {
        match self {
            Self::Fabric => "Fabric",
            Self::Quilt => "Quilt",
            Self::LegacyFabric => "Legacy Fabric",
        }
    }
    fn stage(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::Quilt => "quilt",
            Self::LegacyFabric => "legacy-fabric",
        }
    }
    fn meta(self) -> &'static str {
        match self {
            Self::Fabric => FABRIC_META,
            Self::Quilt => QUILT_META,
            Self::LegacyFabric => LEGACY_FABRIC_META,
        }
    }
    fn main_classes(self) -> &'static [&'static str] {
        match self {
            Self::Fabric => &[
                "net.fabricmc.loader.impl.launch.knot.KnotClient",
                "net.fabricmc.loader.launch.knot.KnotClient",
            ],
            Self::Quilt => &["org.quiltmc.loader.impl.launch.knot.KnotClient"],
            Self::LegacyFabric => &[
                "net.fabricmc.loader.impl.launch.knot.KnotClient",
                "net.fabricmc.loader.launch.knot.KnotClient",
            ],
        }
    }
    fn hosts(self) -> &'static [&'static str] {
        match self {
            Self::Fabric => &["maven.fabricmc.net", "libraries.minecraft.net"],
            Self::Quilt => &[
                "maven.quiltmc.org",
                "maven.fabricmc.net",
                "libraries.minecraft.net",
            ],
            Self::LegacyFabric => &[
                "maven.legacyfabric.net",
                "maven.fabricmc.net",
                "libraries.minecraft.net",
            ],
        }
    }
}

fn loader_maven_url(raw: &str, hosts: &[&str], label: &str) -> Result<Url> {
    let url = trusted_url(raw)?;
    if !url.host_str().is_some_and(|host| hosts.contains(&host)) || url.query().is_some() {
        return Err(DownloadError::Invalid(format!(
            "untrusted {label} Maven URL: {raw}"
        )));
    }
    Ok(url)
}
fn loader_plan(game: &str, profile: &Value, kind: ProfileKind) -> Result<(String, Vec<Artifact>)> {
    let label = kind.label();
    let id = profile["id"]
        .as_str()
        .filter(|id| safe_component(id) && *id != game)
        .ok_or_else(|| DownloadError::Invalid(format!("unsafe {label} profile ID")))?;
    let main_class = profile["mainClass"].as_str();
    if profile["inheritsFrom"].as_str() != Some(game)
        || !main_class.is_some_and(|name| kind.main_classes().contains(&name))
    {
        return Err(DownloadError::Invalid(format!(
            "{label} inheritance or launch class mismatch"
        )));
    }
    let libraries = profile["libraries"]
        .as_array()
        .ok_or_else(|| DownloadError::Invalid(format!("{label} libraries missing")))?;
    if libraries.is_empty() {
        return Err(DownloadError::Invalid(format!("{label} libraries empty")));
    }
    let mut jobs = Vec::new();
    let mut paths = BTreeSet::new();
    for library in libraries {
        let name = library["name"]
            .as_str()
            .ok_or_else(|| DownloadError::Invalid(format!("{label} library name missing")))?;
        // Maven coordinates are URL path segments as well as filesystem paths.
        // Reject URL metacharacters before joining so coordinates cannot change origin.
        if name.split(':').any(|part| !safe_component(part)) {
            return Err(DownloadError::Invalid(format!(
                "unsafe {label} library coordinate"
            )));
        }
        // Legacy Fabric 的 profile 会把 LWJGL2 的 natives 一起列出来（带 extract/natives），
        // 这些是「坐标 + classifier」的老式写法，只取 windows 那份 jar 下来。
        let classifier = if kind == ProfileKind::LegacyFabric {
            library["natives"]
                .as_object()
                .and_then(|map| map.get("windows"))
                .and_then(Value::as_str)
                .map(|value| value.to_owned())
        } else {
            None
        };
        let coordinate = match &classifier {
            Some(classifier) => format!("{name}:{classifier}"),
            None => name.to_owned(),
        };
        let relative = maven_path(&coordinate)?;
        safe_relative(&relative)?;
        // Fabric publishes an explicit Maven repository for every profile library.
        // Never silently substitute a repository, or trust a downloads override.
        let raw = library["url"]
            .as_str()
            .ok_or_else(|| DownloadError::Invalid(format!("{label} Maven URL missing")))?;
        let base = loader_maven_url(raw, kind.hosts(), label)?;
        if !base.path().ends_with('/')
            || library.get("downloads").is_some()
            || (kind != ProfileKind::LegacyFabric && library.get("natives").is_some())
            || library.get("rules").is_some()
        {
            return Err(DownloadError::Invalid(format!(
                "unsupported {label} Maven library"
            )));
        }
        let url = base
            .join(&relative)
            .map_err(|e| DownloadError::Invalid(e.to_string()))?;
        loader_maven_url(url.as_str(), kind.hosts(), label)?;
        let value = serde_json::json!({"url":url.as_str()});
        let mut artifact =
            Artifact::new(format!("libraries/{relative}"), &value, None, FILE_LIMIT)?;
        if let Some(hash) = library.get("sha1") {
            artifact.hash = Some(
                hash.as_str()
                    .filter(|h| valid_hash(h))
                    .ok_or_else(|| DownloadError::Invalid(format!("invalid {label} library SHA1")))?
                    .to_ascii_lowercase(),
            );
        }
        if !paths.insert(artifact.path.clone()) {
            return Err(DownloadError::Invalid(format!(
                "duplicate {label} library path"
            )));
        }
        jobs.push(artifact);
    }
    Ok((id.into(), jobs))
}

#[derive(Clone)]
struct Artifact {
    path: String,
    url: String,
    hash: Option<String>,
    size: Option<u64>,
    limit: u64,
}
impl Artifact {
    fn new(path: String, value: &Value, fallback_url: Option<String>, limit: u64) -> Result<Self> {
        safe_relative(&path)?;
        let url = value["url"]
            .as_str()
            .map(String::from)
            .or(fallback_url)
            .ok_or_else(|| DownloadError::Invalid(format!("missing URL for {path}")))?;
        trusted_url(&url)?;
        let hash = match value.get("sha1") {
            None => None,
            Some(Value::String(h)) if valid_hash(h) => Some(h.to_ascii_lowercase()),
            _ => return Err(DownloadError::Invalid(format!("invalid SHA1 for {path}"))),
        };
        let size = match value.get("size") {
            None => None,
            Some(v) => Some(
                v.as_u64()
                    .ok_or_else(|| DownloadError::Invalid("invalid artifact size".into()))?,
            ),
        };
        if size.is_some_and(|n| n > limit) {
            return Err(DownloadError::Invalid(format!(
                "artifact exceeds size limit: {path}"
            )));
        }
        Ok(Self {
            path,
            url,
            hash,
            size,
            limit,
        })
    }
}
fn verify(artifact: &Artifact, hash: &str, size: u64) -> Result<()> {
    if artifact
        .hash
        .as_deref()
        .is_some_and(|h| !h.eq_ignore_ascii_case(hash))
        || artifact.size.is_some_and(|s| s != size)
    {
        return Err(DownloadError::Integrity(artifact.path.clone()));
    }
    Ok(())
}
fn existing_valid(path: &Path, artifact: &Artifact, cancel: &CancellationToken) -> Result<bool> {
    if !path.is_file() || artifact.hash.is_none() {
        return Ok(false);
    }
    let mut file = File::open(path).map_err(|e| io_error(path, e))?;
    let mut hash = Sha1::new();
    let mut size = 0;
    let mut chunk = [0u8; 65536];
    loop {
        cancel.check()?;
        let n = file.read(&mut chunk).map_err(|e| io_error(path, e))?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > artifact.limit {
            return Ok(false);
        }
        hash.update(&chunk[..n]);
    }
    Ok(verify(artifact, &format!("{:x}", hash.finalize()), size).is_ok())
}
// A cache is an untrusted optional source: never follow links (including Windows
// junctions), and never import anything except the artifact requested by metadata.
fn cache_file(
    root: &Path,
    relative: &str,
    a: &Artifact,
    cancel: &CancellationToken,
) -> Result<Option<File>> {
    let path = root.join(safe_relative(relative)?);
    if !root.is_absolute() || a.hash.is_none() {
        return Ok(None);
    }
    for component in path.ancestors() {
        let Ok(metadata) = fs::symlink_metadata(component) else {
            return Ok(None);
        };
        if metadata.file_type().is_symlink() {
            return Ok(None);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Ok(None);
            }
        }
    }
    match existing_valid(&path, a, cancel) {
        Ok(true) => Ok(File::open(&path).ok()),
        Err(DownloadError::Cancelled) => Err(DownloadError::Cancelled),
        _ => Ok(None),
    }
}
/// 目标文件旁边的 `<name>.part`：同一份文件在进程重启后仍可接着下。
fn part_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".part");
    path.with_file_name(name)
}
/// 单文件最多尝试次数（首次 + 2 次重试）。
const MAX_ATTEMPTS: u32 = 3;

/// 把 `part` 原子发布到目标位置。Windows 的 rename 不能覆盖已存在文件。
fn publish(root: &Path, path: &Path, part: &Path) -> Result<()> {
    inside(root, path)?;
    if path.exists() {
        fs::remove_file(path).map_err(|e| io_error(path, e))?;
    }
    fs::rename(part, path).map_err(|e| io_error(path, e))?;
    Ok(())
}

/// 带重试与断点续传地把一个 artifact 写入 `part`。调用返回时文件已通过校验并落盘。
fn fetch_into_part<S: Source>(
    source: &S,
    a: &Artifact,
    part: &Path,
    cancel: &CancellationToken,
    on_downloaded: &(dyn Fn(u64) + Sync),
) -> Result<()> {
    let mut attempt = 0u32;
    let mut flipped = false;
    let mut tolerate_slow = false;
    loop {
        attempt += 1;
        cancel.check()?;
        match stream_once(source, a, part, cancel, on_downloaded, tolerate_slow) {
            Ok(()) => return Ok(()),
            Err(DownloadError::Slow(_)) => {
                if !flipped {
                    flipped = true;
                    source.note_slow(&a.url);
                    continue;
                }
                tolerate_slow = true;
                continue;
            }
            Err(error) => {
                // 只有网络错误重试；校验失败说明数据本身不对，重下也是同样结果。
                if matches!(error, DownloadError::Integrity(_)) {
                    let _ = fs::remove_file(part);
                }
                if !matches!(error, DownloadError::Network(_)) || attempt >= MAX_ATTEMPTS {
                    return Err(error);
                }
                thread::sleep(Duration::from_millis(500u64 << (attempt - 1).min(3)));
            }
        }
    }
}

/// 单次传输：已有 `.part` 时用 Range 续传，服务器不支持就从头重来。
fn stream_once<S: Source>(
    source: &S,
    a: &Artifact,
    part: &Path,
    cancel: &CancellationToken,
    on_downloaded: &(dyn Fn(u64) + Sync),
    tolerate_slow: bool,
) -> Result<()> {
    let existing = fs::metadata(part).map(|meta| meta.len()).unwrap_or(0);
    let resume = existing > 0 && !a.size.is_some_and(|size| existing >= size);
    let mut file = if resume {
        OpenOptions::new()
            .append(true)
            .open(part)
            .map_err(|e| io_error(part, e))?
    } else {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(part)
            .map_err(|e| io_error(part, e))?
    };
    let mut hash = Sha1::new();
    let mut size = 0u64;
    let mut chunk = [0u8; 65536];
    if resume {
        // 续传前先把已落盘的字节重新喂给哈希，收尾时才有完整的 SHA1。
        let mut done = File::open(part).map_err(|e| io_error(part, e))?;
        loop {
            let n = done.read(&mut chunk).map_err(|e| io_error(part, e))?;
            if n == 0 {
                break;
            }
            hash.update(&chunk[..n]);
            size += n as u64;
        }
    }
    let (mut reader, appended) = if size > 0 {
        match source.open_from(&a.url, size) {
            Ok(pair) => pair,
            // 同 fetch_into_part：断点对不上（416）时丢掉半截文件整份重下。
            Err(error) => {
                file.set_len(0).map_err(|e| io_error(part, e))?;
                hash = Sha1::new();
                size = 0;
                match source.open(&a.url) {
                    Ok(reader) => (reader, true),
                    Err(_) => return Err(error),
                }
            }
        }
    } else {
        (source.open(&a.url)?, true)
    };
    if !appended && size > 0 {
        file.set_len(0).map_err(|e| io_error(part, e))?;
        hash = Sha1::new();
        size = 0;
    }
    let mut window_start = Instant::now();
    let mut window_bytes = 0u64;
    loop {
        cancel.check()?;
        let n = reader
            .read(&mut chunk)
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        source.throttle(n as u64);
        size += n as u64;
        if size > a.limit || a.size.is_some_and(|expected| size > expected) {
            return Err(DownloadError::Integrity(a.path.clone()));
        }
        hash.update(&chunk[..n]);
        file.write_all(&chunk[..n]).map_err(|e| io_error(part, e))?;
        on_downloaded(n as u64);
        if !tolerate_slow && a.size.is_none_or(|expected| expected > 1024 * 1024) {
            window_bytes += n as u64;
            let elapsed = window_start.elapsed();
            if elapsed >= SLOW_WINDOW {
                let rate = window_bytes / elapsed.as_secs().max(1);
                if rate < source.low() {
                    return Err(DownloadError::Slow(format!("{} B/s", rate)));
                }
                window_start = Instant::now();
                window_bytes = 0;
            }
        }
    }
    verify(a, &format!("{:x}", hash.finalize()), size)?;
    cancel.check()?;
    file.sync_all().map_err(|e| io_error(part, e))?;
    Ok(())
}

/// 命中本地缓存时直接拷贝（无网络），同样先校验再发布。
fn copy_cached_into(
    cached: &mut File,
    part: &Path,
    a: &Artifact,
    cancel: &CancellationToken,
) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(part)
        .map_err(|e| io_error(part, e))?;
    let mut hash = Sha1::new();
    let mut size = 0u64;
    let mut chunk = [0u8; 65536];
    loop {
        cancel.check()?;
        let n = cached.read(&mut chunk).map_err(|e| io_error(part, e))?;
        if n == 0 {
            break;
        }
        size += n as u64;
        hash.update(&chunk[..n]);
        file.write_all(&chunk[..n]).map_err(|e| io_error(part, e))?;
    }
    verify(a, &format!("{:x}", hash.finalize()), size)?;
    file.sync_all().map_err(|e| io_error(part, e))?;
    Ok(())
}
enum WorkerMsg {
    Start { path: String },
    Progress,
    File { path: String, downloaded: bool },
    Failed(DownloadError),
}
fn segment_path(part: &Path, index: usize) -> PathBuf {
    let mut name = part.as_os_str().to_owned();
    name.push(format!(".seg{index}"));
    PathBuf::from(name)
}
/// Splits a large artifact into parallel Range requests. Returns false when the server
/// does not honour ranges (or the artifact is too small), leaving no temp files behind.
fn fetch_chunked<S: Source + Sync>(
    source: &S,
    a: &Artifact,
    part: &Path,
    cancel: &CancellationToken,
    on_downloaded: &(dyn Fn(u64) + Sync),
) -> Result<bool> {
    let Some(size) = a.size else {
        return Ok(false);
    };
    if size < CHUNK_THRESHOLD {
        return Ok(false);
    }
    let parts = ((size / CHUNK_THRESHOLD) as usize).clamp(CHUNK_MIN_PARTS, CHUNK_MAX_PARTS);
    let segment = size.div_ceil(parts as u64);
    let unsupported = AtomicBool::new(false);
    thread::scope(|scope| {
        for index in 0..parts {
            let start = index as u64 * segment;
            let end = (start + segment).min(size);
            if start >= end {
                continue;
            }
            let unsupported = &unsupported;
            scope.spawn(move || {
                if unsupported.load(Ordering::Relaxed) {
                    return;
                }
                match write_segment(
                    source,
                    a,
                    &segment_path(part, index),
                    start,
                    end,
                    cancel,
                    on_downloaded,
                ) {
                    Ok(true) => {}
                    Ok(false) => unsupported.store(true, Ordering::Relaxed),
                    Err(error) => {
                        if !matches!(error, DownloadError::Slow(_)) {
                            unsupported.store(true, Ordering::Relaxed);
                        }
                    }
                }
            });
        }
    });
    if unsupported.load(Ordering::Relaxed) {
        for index in 0..parts {
            let _ = fs::remove_file(segment_path(part, index));
        }
        return Ok(false);
    }
    join_segments(part, parts, a, cancel)?;
    Ok(true)
}
/// Streams one byte range into `target`. Returns false when the server ignored the Range header.
fn write_segment<S: Source>(
    source: &S,
    a: &Artifact,
    target: &Path,
    start: u64,
    end: u64,
    cancel: &CancellationToken,
    on_downloaded: &(dyn Fn(u64) + Sync),
) -> Result<bool> {
    let (mut reader, appended) = source.open_from(&a.url, start)?;
    // 第一段从 0 开始，服务器不会回 206，此时整包读取也是对的。
    if !appended && start > 0 {
        return Ok(false);
    }
    let mut file = File::create(target).map_err(|e| io_error(target, e))?;
    let mut chunk = vec![0u8; 65536];
    let mut read = start;
    while read < end {
        cancel.check()?;
        let want = (end - read).min(chunk.len() as u64) as usize;
        let n = reader
            .read(&mut chunk[..want])
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        if n == 0 {
            return Ok(false);
        }
        source.throttle(n as u64);
        file.write_all(&chunk[..n])
            .map_err(|e| io_error(target, e))?;
        read += n as u64;
        on_downloaded(n as u64);
    }
    file.sync_all().map_err(|e| io_error(target, e))?;
    Ok(true)
}
fn join_segments(
    part: &Path,
    parts: usize,
    a: &Artifact,
    cancel: &CancellationToken,
) -> Result<()> {
    let mut file = File::create(part).map_err(|e| io_error(part, e))?;
    let mut hash = Sha1::new();
    let mut size = 0u64;
    let mut chunk = vec![0u8; 65536];
    for index in 0..parts {
        let path = segment_path(part, index);
        if !path.is_file() {
            continue;
        }
        let mut segment = File::open(&path).map_err(|e| io_error(&path, e))?;
        loop {
            cancel.check()?;
            let n = segment.read(&mut chunk).map_err(|e| io_error(&path, e))?;
            if n == 0 {
                break;
            }
            size += n as u64;
            hash.update(&chunk[..n]);
            file.write_all(&chunk[..n]).map_err(|e| io_error(part, e))?;
        }
        let _ = fs::remove_file(&path);
    }
    file.sync_all().map_err(|e| io_error(part, e))?;
    verify(a, &format!("{:x}", hash.finalize()), size)
}
/// Downloads one artifact into place: stream to `.part`, verify, publish.
/// Returns whether network bytes were spent (false when an already-valid file was reused).
/// Safe to run concurrently for distinct artifacts of the same game directory.
fn transfer<S: Source + Sync>(
    source: &S,
    root: &Path,
    cache_root: Option<&Path>,
    cancel: &CancellationToken,
    a: &Artifact,
    bytes: &AtomicU64,
    tx: &mpsc::Sender<WorkerMsg>,
) -> Result<bool> {
    cancel.check()?;
    let path = root.join(safe_relative(&a.path)?);
    inside(root, &path)?;
    if existing_valid(&path, a, cancel)? {
        return Ok(false);
    }
    let parent = path
        .parent()
        .ok_or_else(|| DownloadError::Invalid(format!("artifact has no parent: {}", a.path)))?;
    fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    inside(root, &path)?;
    let part = part_path(&path);
    let cached = match cache_root {
        Some(cache_root) => cache_file(cache_root, &a.path, a, cancel)?,
        None => None,
    };
    let reused = cached.is_some();
    match cached {
        Some(mut cached_file) => copy_cached_into(&mut cached_file, &part, a, cancel)?,
        None => {
            let next_event = AtomicU64::new(512 * 1024);
            let notify = |added: u64| {
                let total = bytes.fetch_add(added, Ordering::Relaxed) + added;
                let due = next_event.load(Ordering::Relaxed);
                if total >= due
                    && next_event
                        .compare_exchange(
                            due,
                            total + 512 * 1024,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                        )
                        .is_ok()
                {
                    let _ = tx.send(WorkerMsg::Progress);
                }
            };
            // 速度已到下限就不再分块（PCL 只在 Speed < Low 时加连接）。
            let chunked = !source.saturated() && fetch_chunked(source, a, &part, cancel, &notify)?;
            if !chunked {
                fetch_into_part(source, a, &part, cancel, &notify)?;
            }
        }
    }
    publish(root, &path, &part)?;
    Ok(!reused)
}
/// 加权进度：`total` 是这次安装的总权重，`done` 是已经走完的权重。
///
/// 权重照 PCL `pcl_ModDownload.vb:59-118` 的分段：分析 1、下载支持库 15、
/// 资源索引 2、下载资源 25，合计 43；每个加载器阶段再占 6（profile 1 /
/// 校验和 1 / 支持库 4）。
#[derive(Debug, Clone, Copy, Default)]
struct Progress {
    total: f64,
    done: f64,
}

struct Installer<'a, S, F> {
    source: &'a S,
    root: PathBuf,
    cache_root: Option<&'a Path>,
    cancel: &'a CancellationToken,
    threads: usize,
    quilt_loader: Option<String>,
    legacy_fabric: Option<String>,
    progress: InstallProgress,
    summary: InstallSummary,
    weights: Progress,
    emit: F,
}
impl<S: Source + Sync, F: FnMut(InstallProgress)> Installer<'_, S, F> {
    fn event(&mut self, stage: &str, path: &str) {
        self.progress.stage = stage.into();
        self.progress.path = path.into();
        (self.emit)(self.progress.clone());
    }
    /// 走完一段权重（PCL 的「分析 1 / 支持库 15 / 资源索引 2 / 资源 25」）。
    fn advance(&mut self, weight: f64) {
        self.weights.done += weight;
        self.progress.percent = Some(self.percent(0.0));
        (self.emit)(self.progress.clone());
    }
    /// 总进度百分比：`partial` 是当前这段里已经走完的比例（0..=1）。
    fn percent(&self, partial: f64) -> f64 {
        if self.weights.total <= 0.0 {
            return 0.0;
        }
        ((self.weights.done + partial) / self.weights.total * 100.0).clamp(0.0, 100.0)
    }
    fn add(&mut self, jobs: &[Artifact]) {
        self.progress.total_files += jobs.len() as u64;
        self.progress.total_bytes += jobs.iter().filter_map(|a| a.size).sum::<u64>();
    }
    fn download(&mut self, a: &Artifact, stage: &str) -> Result<PathBuf> {
        self.download_cached(a, stage, &a.path)
    }
    fn download_cached(&mut self, a: &Artifact, stage: &str, cache_path: &str) -> Result<PathBuf> {
        self.cancel.check()?;
        let path = self.root.join(safe_relative(&a.path)?);
        inside(&self.root, &path)?;
        self.event(stage, &a.path);
        self.cancel.check()?;
        if existing_valid(&path, a, self.cancel)? {
            self.summary.skipped_files += 1;
        } else {
            let parent = path.parent().ok_or_else(|| {
                DownloadError::Invalid(format!("artifact has no parent: {}", a.path))
            })?;
            fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
            inside(&self.root, &path)?;
            let part = part_path(&path);
            let cached = match self.cache_root {
                Some(root) => cache_file(root, cache_path, a, self.cancel)?,
                None => None,
            };
            let reused = cached.is_some();
            match cached {
                Some(mut cached_file) => copy_cached_into(&mut cached_file, &part, a, self.cancel)
                    .map_err(|error| error.with_path(&a.path))?,
                None => {
                    let source = self.source;
                    let cancel = self.cancel;
                    let downloaded = AtomicU64::new(self.progress.downloaded_bytes);
                    let notify = |added: u64| {
                        downloaded.fetch_add(added, Ordering::Relaxed);
                    };
                    fetch_into_part(source, a, &part, cancel, &notify)
                        .map_err(|error| error.with_path(&a.path))?;
                    self.progress.downloaded_bytes = downloaded.load(Ordering::Relaxed);
                    self.progress.stage = stage.into();
                    self.progress.path = a.path.clone();
                }
            }
            publish(&self.root, &path, &part)?;
            if reused {
                self.summary.skipped_files += 1;
            } else {
                self.summary.downloaded_files += 1;
            }
        }
        self.progress.completed_files += 1;
        self.summary.downloaded_bytes = self.progress.downloaded_bytes;
        self.event(stage, &a.path);
        Ok(path)
    }
    /// Fetches independent artifacts on a small worker pool. Each file is published only
    /// after its own SHA1/size check, so ordering between files is irrelevant; only the
    /// reported progress and the first error are observed by the caller.
    /// 下一组文件，`weight` 是这一组在总进度里占的权重（见 [`Progress`]）。
    fn download_many(&mut self, jobs: &[Artifact], stage: &str, weight: f64) -> Result<()> {
        if jobs.len() <= 1 || self.threads <= 1 {
            for artifact in jobs {
                self.download(artifact, stage)?;
                self.progress.percent = Some(self.percent(0.0));
                (self.emit)(self.progress.clone());
            }
            self.weights.done += weight;
            return Ok(());
        }
        let Installer {
            source,
            root,
            cache_root,
            cancel,
            threads,
            quilt_loader: _,
            legacy_fabric: _,
            progress,
            summary,
            weights,
            emit,
        } = self;
        let weight_base = weights.done;
        let weight_total = weights.total;
        let start_bytes = progress.downloaded_bytes;
        let expected_bytes: u64 = jobs.iter().filter_map(|a| a.size).sum();
        let batch_len = jobs.len() as f64;
        let source: &S = source;
        let root: &Path = root.as_path();
        let cache_root: Option<&Path> = cache_root.as_deref();
        let cancel: &CancellationToken = cancel;
        let workers = (*threads).min(jobs.len());
        let bytes = AtomicU64::new(progress.downloaded_bytes);
        let next = AtomicUsize::new(0);
        // A failed artifact stops new work but lets in-flight transfers publish, so the
        // install keeps every file it already validated (serial behavior).
        let stop = AtomicBool::new(false);
        let (tx, rx) = mpsc::channel::<WorkerMsg>();
        let mut failure: Option<DownloadError> = None;
        thread::scope(|scope| {
            for _ in 0..workers {
                let tx = tx.clone();
                let bytes = &bytes;
                let next = &next;
                let stop = &stop;
                scope.spawn(move || {
                    while !cancel.is_cancelled() && !stop.load(Ordering::Relaxed) {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(artifact) = jobs.get(index) else {
                            break;
                        };
                        if tx
                            .send(WorkerMsg::Start {
                                path: artifact.path.clone(),
                            })
                            .is_err()
                        {
                            break;
                        }
                        match transfer(source, root, cache_root, cancel, artifact, bytes, &tx) {
                            Ok(downloaded) => {
                                let path = artifact.path.clone();
                                if tx.send(WorkerMsg::File { path, downloaded }).is_err() {
                                    break;
                                }
                            }
                            Err(error) => {
                                let _ = tx.send(WorkerMsg::Failed(error.with_path(&artifact.path)));
                                // Do not cancel in-flight transfers; just stop queueing more.
                                stop.store(true, Ordering::Relaxed);
                                break;
                            }
                        }
                    }
                });
            }
            drop(tx);
            for message in rx {
                match message {
                    WorkerMsg::Start { path } => progress.path = path,
                    WorkerMsg::Progress => {}
                    WorkerMsg::File { path, downloaded } => {
                        progress.path = path;
                        progress.completed_files += 1;
                        if downloaded {
                            summary.downloaded_files += 1;
                        } else {
                            summary.skipped_files += 1;
                        }
                    }
                    WorkerMsg::Failed(error) => {
                        if failure.is_none() {
                            failure = Some(error);
                        }
                    }
                }
                progress.stage = stage.into();
                progress.downloaded_bytes = bytes.load(Ordering::Relaxed);
                summary.downloaded_bytes = progress.downloaded_bytes;
                // 段内完成度：有字节总数就按字节，没有就按文件个数。
                let done_bytes = progress
                    .downloaded_bytes
                    .saturating_sub(start_bytes) as f64;
                let fraction = if expected_bytes > 0 {
                    (done_bytes / expected_bytes as f64).clamp(0.0, 1.0)
                } else if batch_len > 0.0 {
                    (progress.completed_files as f64 / batch_len).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                if weight_total > 0.0 {
                    progress.percent = Some(
                        ((weight_base + weight * fraction) / weight_total * 100.0)
                            .clamp(0.0, 100.0),
                    );
                }
                (*emit)(progress.clone());
            }
            *weights = Progress {
                total: weight_total,
                done: weight_base + weight,
            };
        });
        if let Some(error) = failure {
            return Err(error);
        }
        cancel.check()?;
        Ok(())
    }
    fn run_profile(&mut self, id: &str) -> Result<()> {
        self.weights = Progress {
            total: 43.0,
            done: 0.0,
        };
        let metadata_path = self.root.join(format!("versions/{id}/{id}.json"));
        inside(&self.root, &metadata_path)?;
        let metadata = crate::read_json(&metadata_path)?;
        let index_id = metadata["assetIndex"]["id"]
            .as_str()
            .ok_or_else(|| DownloadError::Invalid("asset index ID missing".into()))?;
        if !safe_component(index_id) {
            return Err(DownloadError::Invalid("unsafe asset index ID".into()));
        }
        let index = Artifact::new(
            format!("assets/indexes/{index_id}.json"),
            &metadata["assetIndex"],
            None,
            JSON_LIMIT,
        )?;
        self.add(std::slice::from_ref(&index));
        let assets = crate::read_json(&self.download(&index, "asset-index")?)?;
        self.advance(3.0);
        let (libraries, assets) = split_plan(plan(id, &metadata, &assets)?);
        self.add(&libraries);
        self.add(&assets);
        self.download_many(&libraries, "downloading", 15.0)?;
        self.download_many(&assets, "downloading", 25.0)?;
        self.cancel.check()?;
        self.summary.version_id = id.to_owned();
        Ok(())
    }
    /// 只补 libraries / natives / logging。
    ///
    /// 老安装器写出的 `inheritsFrom` 增量 profile 没有 `assetIndex`，客户端 jar
    /// 也在继承的原版里；`run_profile` 要求的 assetIndex 与资源对它都不适用。
    fn run_libraries(&mut self, id: &str) -> Result<()> {
        self.weights = Progress {
            total: 15.0,
            done: 0.0,
        };
        let metadata_path = self.root.join(format!("versions/{id}/{id}.json"));
        inside(&self.root, &metadata_path)?;
        let metadata = crate::read_json(&metadata_path)?;
        let assets = serde_json::json!({ "objects": {} });
        let (libraries, assets) = split_plan(plan(id, &metadata, &assets)?);
        self.add(&libraries);
        self.add(&assets);
        self.download_many(&libraries, "downloading", 15.0)?;
        self.cancel.check()?;
        self.summary.version_id = id.to_owned();
        Ok(())
    }
    fn run(&mut self, game: &str, loader: Option<&str>) -> Result<InstallSummary> {
        // 加权进度：分析 3 + 支持库 15 + 资源 25；每个加载器再加 6
        // （profile 1 / 校验和 1 / 支持库 4），进度条不会在加载器阶段重走一遍。
        let loaders = [
            loader.is_some(),
            self.quilt_loader.is_some(),
            self.legacy_fabric.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count() as f64;
        self.weights = Progress {
            total: 43.0 + 6.0 * loaders,
            done: 0.0,
        };
        self.run_vanilla(game)?;
        if let Some(loader) = loader {
            self.install_profile_loader(game, loader, ProfileKind::Fabric)?;
        }
        if let Some(loader) = self.quilt_loader.clone() {
            self.install_profile_loader(game, &loader, ProfileKind::Quilt)?;
        }
        if let Some(loader) = self.legacy_fabric.clone() {
            self.install_profile_loader(game, &loader, ProfileKind::LegacyFabric)?;
        }
        self.cancel.check()?;
        self.event("complete", "");
        Ok(self.summary.clone())
    }
    fn install_profile_loader(
        &mut self,
        game: &str,
        loader: &str,
        kind: ProfileKind,
    ) -> Result<()> {
        let label = kind.label();
        let profile = Artifact::new(
            format!("versions/{game}/.ybumcl-{}.json", kind.stage()),
            &serde_json::json!({"url":format!("{}/{game}/{loader}/profile/json", kind.meta())}),
            None,
            JSON_LIMIT,
        )?;
        self.add(std::slice::from_ref(&profile));
        let staged = self.download(&profile, &format!("{}-profile", kind.stage()))?;
        self.advance(1.0);
        let metadata = crate::read_json(&staged)?;
        let (id, mut jobs) = loader_plan(game, &metadata, kind)?;
        self.progress.version_id = id.clone();
        self.add(&jobs);
        // Resolve every checksum before fetching any Fabric JAR. Sidecars are small,
        // bounded, cancellable transfers from the same allowlisted Maven endpoint.
        for job in &mut jobs {
            if job.hash.is_none() {
                self.event(&format!("{}-checksum", kind.stage()), &job.path);
                self.cancel.check()?;
                let bytes =
                    fetch_limited(self.source, &format!("{}.sha1", job.url), self.cancel, 1024)?;
                self.progress.downloaded_bytes += bytes.len() as u64;
                self.summary.downloaded_bytes = self.progress.downloaded_bytes;
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| DownloadError::Invalid(format!("invalid {label} SHA1 sidecar")))?;
                let hash = text.trim();
                if !valid_hash(hash) {
                    return Err(DownloadError::Invalid(format!(
                        "invalid {label} SHA1 sidecar"
                    )));
                }
                job.hash = Some(hash.to_ascii_lowercase());
            }
        }
        self.advance(1.0);
        self.download_many(&jobs, &format!("{}-libraries", kind.stage()), 4.0)?;
        self.cancel.check()?;
        let destination = self.root.join(format!("versions/{id}/{id}.json"));
        inside(&self.root, &destination)?;
        let parent = destination.parent().unwrap();
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        inside(&self.root, &destination)?;
        if destination.exists() {
            fs::remove_file(&destination).map_err(|e| io_error(&destination, e))?;
        }
        fs::rename(&staged, &destination).map_err(|e| io_error(&destination, e))?;
        // 这条线的实例 json 是加载器给的 profile（带 inheritsFrom），顺手记下基础版本。
        let key = match kind {
            ProfileKind::Fabric => "fabric",
            ProfileKind::Quilt => "quilt",
            ProfileKind::LegacyFabric => "legacyfabric",
        };
        let _ = crate::merge::stamp_meta(&self.root, &id, game, &[(key, loader)]);
        self.summary.version_id = id;
        Ok(())
    }
    fn run_vanilla(&mut self, version_id: &str) -> Result<()> {
        self.event("manifest", "");
        let remote = manifest(self.source, self.cancel)?
            .versions
            .into_iter()
            .find(|v| v.id == version_id)
            .ok_or_else(|| {
                DownloadError::Invalid(format!("unknown official version: {version_id}"))
            })?;
        self.advance(1.0);
        let meta = Artifact::new(
            format!("versions/{version_id}/{version_id}.json"),
            &serde_json::json!({"url":remote.url}),
            None,
            JSON_LIMIT,
        )?;
        let meta = Artifact {
            hash: remote.sha1,
            ..meta
        };
        self.add(std::slice::from_ref(&meta));
        let final_metadata = self.root.join(&meta.path);
        inside(&self.root, &final_metadata)?;
        // Keep a previously completed install visible; new/invalid metadata remains staged
        // until every required artifact succeeds, so scans cannot report a partial install.
        let metadata_artifact = if existing_valid(&final_metadata, &meta, self.cancel)? {
            meta.clone()
        } else {
            Artifact {
                path: format!("versions/{version_id}/.ybumcl-version.json"),
                ..meta.clone()
            }
        };
        let metadata_path = self.download_cached(&metadata_artifact, "metadata", &meta.path)?;
        let metadata = crate::read_json(&metadata_path)?;
        if metadata["id"].as_str() != Some(version_id) || metadata.get("inheritsFrom").is_some() {
            return Err(DownloadError::Invalid(
                "metadata ID mismatch or inherited version".into(),
            ));
        }
        self.advance(1.0);
        let index_id = metadata["assetIndex"]["id"]
            .as_str()
            .ok_or_else(|| DownloadError::Invalid("asset index ID missing".into()))?;
        if !safe_component(index_id) {
            return Err(DownloadError::Invalid("unsafe asset index ID".into()));
        }
        let index = Artifact::new(
            format!("assets/indexes/{index_id}.json"),
            &metadata["assetIndex"],
            None,
            JSON_LIMIT,
        )?;
        self.add(std::slice::from_ref(&index));
        let assets = crate::read_json(&self.download(&index, "asset-index")?)?;
        self.advance(1.0);
        let (libraries, assets) = split_plan(plan(version_id, &metadata, &assets)?);
        self.add(&libraries);
        self.add(&assets);
        self.download_many(&libraries, "downloading", 15.0)?;
        self.download_many(&assets, "downloading", 25.0)?;
        self.cancel.check()?;
        if metadata_path != final_metadata {
            inside(&self.root, &final_metadata)?;
            if final_metadata.exists() {
                fs::remove_file(&final_metadata).map_err(|e| io_error(&final_metadata, e))?;
            }
            fs::rename(&metadata_path, &final_metadata)
                .map_err(|e| io_error(&final_metadata, e))?;
        }
        // 原版目录本身也是一份实例，`_ybmcl` 由调用方（命令层 / 实例安装）补：
        // 这里不写，免得改掉 json 的 sha1，让缓存与「已装好」的校验对不上。
        Ok(())
    }
}
fn library_artifact(library: &Value, artifact: &Value, fallback: &str) -> Result<Artifact> {
    let relative = artifact["path"]
        .as_str()
        .or_else(|| library["downloads"]["artifact"]["path"].as_str())
        .unwrap_or(fallback);
    safe_relative(relative)?;
    let mut value = artifact.clone();
    if value["url"].as_str().is_none_or(str::is_empty) {
        // 不少库（尤其老版本 Forge 的 universal jar）只给了 `name` 和 `url`，没有
        // `downloads.artifact`；个别老安装器写的是 `http://files.minecraftforge.net/maven`，
        // 这个域名现在直接 404，得换成 https 的 maven.minecraftforge.net 才能下到。
        // PCL #5455：部分 Forge 的库（尤其 `:universal`）url 是空的，但文件就在
        // forge maven 上；按 group 猜的 libraries.minecraft.net 只会 404。
        let forge = library["name"]
            .as_str()
            .is_some_and(|name| name.starts_with("net.minecraftforge:forge:"));
        let base = if forge {
            "https://maven.minecraftforge.net".to_owned()
        } else {
            library_base(library, relative)
        };
        value["url"] = Value::String(format!("{base}/{relative}"));
    }
    if value["path"].as_str().is_none() {
        value["path"] = Value::String(relative.to_owned());
    }
    Artifact::new(format!("libraries/{relative}"), &value, None, FILE_LIMIT)
}

/// 库条目没写 `downloads.artifact.url` 时，按它的 `url`、再按 group 猜一个仓库根。
fn library_base(library: &Value, relative: &str) -> String {
    let declared = library["url"]
        .as_str()
        .map(|url: &str| url.trim_end_matches('/'))
        .filter(|url| !url.is_empty());
    if let Some(url) = declared {
        let url = upgrade_maven(url);
        if url.starts_with("https://") || url.starts_with("http://") {
            return url;
        }
    }
    // 没有 url 的库多半来自加载器自己的 maven：net.minecraftforge / net.neoforged 的
    // 文件在 Mojang 的 maven 上并不存在，落到 libraries.minecraft.net 只会 404。
    let group = relative.split('/').take(2).collect::<Vec<_>>().join("/");
    let base = match group.as_str() {
        "net/minecraftforge" | "cpw/mods" | "de/oceanlabs" => "https://maven.minecraftforge.net",
        "net/neoforged" => "https://maven.neoforged.net",
        _ => "https://libraries.minecraft.net",
    };
    base.to_owned()
}

/// 老的安装器还会写 `http://files.minecraftforge.net/maven` 这种已经下线的地址。
fn upgrade_maven(url: &str) -> String {
    for (old, new) in [
        (
            "http://files.minecraftforge.net/maven",
            "https://maven.minecraftforge.net",
        ),
        (
            "https://files.minecraftforge.net/maven",
            "https://maven.minecraftforge.net",
        ),
        (
            "http://maven.minecraftforge.net",
            "https://maven.minecraftforge.net",
        ),
        (
            "http://maven.neoforged.net",
            "https://maven.neoforged.net",
        ),
        (
            "http://libraries.minecraft.net",
            "https://libraries.minecraft.net",
        ),
    ] {
        if let Some(rest) = url.strip_prefix(old) {
            return format!("{new}{rest}");
        }
    }
    url.to_owned()
}
fn plan(id: &str, metadata: &Value, assets: &Value) -> Result<Vec<Artifact>> {
    // Cleanroom 这类自带完整 profile 的加载器不写 downloads.client，客户端 jar 由库提供。
    let mut jobs = match metadata["downloads"]["client"]["url"].as_str() {
        Some(url) if !url.is_empty() => vec![Artifact::new(
            format!("versions/{id}/{id}.jar"),
            &metadata["downloads"]["client"],
            None,
            FILE_LIMIT,
        )?],
        _ => Vec::new(),
    };
    for library in metadata["libraries"]
        .as_array()
        .ok_or_else(|| DownloadError::Invalid("libraries missing".into()))?
    {
        if !rules_allow(library.get("rules"))? {
            continue;
        }
        let name = library["name"]
            .as_str()
            .ok_or_else(|| DownloadError::Invalid("library name missing".into()))?;
        let fallback = maven_path(name)?;
        if let Some(artifact) = library["downloads"].get("artifact") {
            jobs.push(library_artifact(library, artifact, &fallback)?);
        } else if library.get("downloads").is_none() {
            if library.get("url").is_some() {
                trusted_url(
                    library["url"]
                        .as_str()
                        .ok_or_else(|| DownloadError::Invalid("invalid library URL".into()))?,
                )?;
            }
            jobs.push(library_artifact(library, &Value::Null, &fallback)?);
        }
        if let Some(classifier) = library["natives"]["windows"].as_str() {
            let classifier = classifier.replace(
                "${arch}",
                if cfg!(target_pointer_width = "64") {
                    "64"
                } else {
                    "32"
                },
            );
            let fallback = maven_path(&format!("{name}:{classifier}"))?;
            let artifact = &library["downloads"]["classifiers"][&classifier];
            if artifact.is_null() && library.get("downloads").is_some() {
                return Err(DownloadError::Invalid(format!(
                    "native classifier missing: {classifier}"
                )));
            }
            jobs.push(library_artifact(library, artifact, &fallback)?);
        }
    }
    if let Some(logging) = metadata["logging"]["client"].get("file") {
        let name = logging["id"]
            .as_str()
            .ok_or_else(|| DownloadError::Invalid("logging ID missing".into()))?;
        if !safe_component(name) {
            return Err(DownloadError::Invalid("unsafe logging ID".into()));
        }
        jobs.push(Artifact::new(
            format!("assets/log_configs/{name}"),
            logging,
            None,
            JSON_LIMIT,
        )?);
    }
    let objects = assets["objects"]
        .as_object()
        .ok_or_else(|| DownloadError::Invalid("asset objects missing".into()))?;
    for (name, object) in objects {
        safe_relative(name)?;
        let hash = object["hash"]
            .as_str()
            .filter(|h| valid_hash(h))
            .ok_or_else(|| DownloadError::Invalid("invalid asset hash".into()))?
            .to_ascii_lowercase();
        let value = serde_json::json!({"sha1":hash,"size":object["size"],"url":format!("https://resources.download.minecraft.net/{}/{}", &hash[..2], hash)});
        jobs.push(Artifact::new(
            format!("assets/objects/{}/{}", &hash[..2], hash),
            &value,
            None,
            FILE_LIMIT,
        )?);
        // Old versions use a second, named layout. Downloading from the same content-addressed
        // URL preserves verification and avoids hardlinks to potentially mutable user files.
        if assets["virtual"].as_bool() == Some(true) {
            let index = metadata["assetIndex"]["id"].as_str().unwrap();
            jobs.push(Artifact::new(
                format!("assets/virtual/{index}/{name}"),
                &value,
                None,
                FILE_LIMIT,
            )?);
        }
        if assets["map_to_resources"].as_bool() == Some(true) {
            jobs.push(Artifact::new(
                format!("resources/{name}"),
                &value,
                None,
                FILE_LIMIT,
            )?);
        }
    }
    let mut paths = BTreeSet::new();
    jobs.retain(|a| paths.insert(a.path.clone()));
    Ok(jobs)
}

/// 按 PCL 的两段拆开：资源（资源对象 / 老版本 virtual 与 resources 布局）单独一段，
/// 其余（客户端 jar、libraries、natives、log 配置）算作「支持库」。
fn split_plan(jobs: Vec<Artifact>) -> (Vec<Artifact>, Vec<Artifact>) {
    let mut libraries = Vec::new();
    let mut assets = Vec::new();
    for job in jobs {
        let asset = job.path.starts_with("assets/objects/")
            || job.path.starts_with("assets/virtual/")
            || job.path.starts_with("resources/");
        if asset {
            assets.push(job);
        } else {
            libraries.push(job);
        }
    }
    (libraries, assets)
}
/// Installs official vanilla files beneath `request.game_dir`; never chooses `.minecraft`.
/// The caller supplies a token and receives owned, serializable progress snapshots.
pub fn install_version(
    request: InstallRequest,
    cancel: CancellationToken,
    emit: impl FnMut(InstallProgress),
) -> Result<InstallSummary> {
    let strategy = request.strategy;
    let version = request.version_id.clone();
    install_with(&Http::new(strategy)?.with_version(&version), request, &cancel, emit)
}
/// Installs using an optional, untrusted cache of matching game-relative artifacts.
/// Cache files must match metadata SHA1/size and are copied, never hardlinked.
/// Linked/reparse paths, missing files and invalid contents fall back to HTTP.
/// Only requested artifacts are examined; the cache is never modified.
pub fn install_version_with_cache(
    request: InstallRequest,
    cancel: CancellationToken,
    emit: impl FnMut(InstallProgress),
    cache_root: &Path,
) -> Result<InstallSummary> {
    let strategy = request.strategy;
    let threads = request.thread_count();
    let version = request.version_id.clone();
    install_with_cache(
        &Http::new(strategy)?.with_version(&version),
        request,
        &cancel,
        emit,
        Some(cache_root),
        threads,
    )
}
/// 装 Legacy Fabric（1.13.2 及更早的 Fabric 分支）：先按官方清单补齐原版，
/// 再从 meta.legacyfabric.net 拉它的 profile 并写进同一个实例。
pub fn install_legacy_fabric(
    game_dir: &str,
    game: &str,
    loader: &str,
    strategy: DownloadStrategy,
    threads: Option<usize>,
    cancel: CancellationToken,
    emit: impl FnMut(InstallProgress),
) -> Result<InstallSummary> {
    if !safe_component(game) || !safe_component(loader) {
        return Err(DownloadError::Invalid("unsafe version component".into()));
    }
    let root = PathBuf::from(game_dir);
    if !root.is_absolute() {
        return Err(DownloadError::Invalid(
            "game directory must be absolute".into(),
        ));
    }
    cancel.check()?;
    fs::create_dir_all(&root).map_err(|e| io_error(&root, e))?;
    let root = fs::canonicalize(&root).map_err(|e| io_error(&root, e))?;
    let source = Http::new(strategy)?.with_version(game);
    let mut installer = Installer {
        source: &source,
        root,
        cache_root: None,
        cancel: &cancel,
        threads: threads.unwrap_or(4).max(1),
        quilt_loader: None,
        legacy_fabric: Some(loader.to_owned()),
        progress: InstallProgress {
            version_id: game.to_owned(),
            stage: String::new(),
            path: String::new(),
            completed_files: 0,
            total_files: 0,
            downloaded_bytes: 0,
            total_bytes: 0,
            percent: None,
        },
        weights: Progress::default(),
        summary: InstallSummary {
            version_id: game.to_owned(),
            path: tidy_path(&PathBuf::from(game_dir).join("versions").join(game)),
            downloaded_files: 0,
            skipped_files: 0,
            downloaded_bytes: 0,
        },
        emit,
    };
    let result = installer.run(game, None);
    if let Err(ref error) = result {
        installer.event(
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
fn install_with(
    source: &(impl Source + Sync),
    request: InstallRequest,
    cancel: &CancellationToken,
    emit: impl FnMut(InstallProgress),
) -> Result<InstallSummary> {
    let threads = request.thread_count();
    install_with_cache(source, request, cancel, emit, None, threads)
}
/// 补齐一个已存在的完整版本 profile（自带 assetIndex 与库清单）缺的文件，
/// 用于 Cleanroom 这类安装器只写 json 与主 jar、其余靠自己清单下载的加载器。
/// LabyMod 的清单（production / snapshot 两个频道）。
/// LabyMod 客户端本体所在的下载前缀。
pub const LABYMOD_DOWNLOAD: &str = "https://releases.r2.labymod.net/api/v1/download";

pub const LABYMOD_MANIFEST: &str = "https://releases.r2.labymod.net/api/v1/manifest";

/// 清单里的一条构建：`version` 是 LabyMod 版本号，`game` 是它支持的 Minecraft 版本。
#[derive(Clone, Debug, Serialize)]
pub struct LabymodVersion {
    pub version: String,
    pub game: String,
    pub channel: String,
    pub commit: String,
    pub manifest_url: String,
    /// 客户端 jar 的 sha1 与大小（清单根上的字段）。
    pub sha1: String,
    pub size: u64,
}

/// 拉 LabyMod 的官方清单（`production` 或 `snapshot`）。
pub fn labymod_versions(channel: &str) -> Result<Vec<LabymodVersion>> {
    if channel != "production" && channel != "snapshot" {
        return Err(DownloadError::Invalid(format!(
            "unknown LabyMod channel {channel}"
        )));
    }
    let cancel = CancellationToken::default();
    let url = format!("{LABYMOD_MANIFEST}/{channel}/latest.json");
    let bytes = fetch_catalog_bytes(&url, &cancel)?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|e| DownloadError::Invalid(format!("bad LabyMod manifest: {e}")))?;
    let version = value["labyModVersion"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let commit = value["commitReference"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if version.is_empty() || commit.is_empty() {
        return Err(DownloadError::Invalid(
            "LabyMod manifest has no version".into(),
        ));
    }
    let sha1 = value["sha1"].as_str().unwrap_or_default().to_owned();
    let size = value["size"].as_u64().unwrap_or(0);
    let mut entries = vec![];
    for item in value["minecraftVersions"].as_array().into_iter().flatten() {
        let (Some(game), Some(url)) =
            (item["version"].as_str(), item["customManifestUrl"].as_str())
        else {
            continue;
        };
        entries.push(LabymodVersion {
            version: version.clone(),
            game: game.to_owned(),
            channel: channel.to_owned(),
            commit: commit.clone(),
            manifest_url: url.to_owned(),
            sha1: sha1.clone(),
            size,
        });
    }
    entries.sort_by(|a, b| b.game.cmp(&a.game));
    Ok(entries)
}

/// LabyMod 官方库清单（`libraries/{channel}.json`）：每个条目自带完整下载地址。
pub const LABYMOD_LIBRARIES: &str = "https://releases.r2.labymod.net/api/v1/libraries";

/// 把 LabyMod 的官方库（含 `net.minecraft:launchwrapper:4.1.3`、LabyMod 自己的 jar）
/// 追加进版本 json。它们的下载地址不在 maven，而是 LabyMod 自己的 CDN。
fn append_labymod_libraries(
    value: &Value,
    channel: &str,
    game: &str,
    commit: &str,
) -> Result<Value> {
    let cancel = CancellationToken::default();
    let url = format!("{LABYMOD_LIBRARIES}/{channel}.json");
    let bytes = fetch_catalog_bytes(&url, &cancel)?;
    let source: Value = serde_json::from_slice(&bytes)
        .map_err(|e| DownloadError::Invalid(format!("bad LabyMod libraries: {e}")))?;
    let mut merged = value.clone();
    let mut libraries = merged["libraries"].as_array().cloned().unwrap_or_default();
    for entry in source["libraries"].as_array().into_iter().flatten() {
        let (Some(name), Some(download)) = (entry["name"].as_str(), entry["url"].as_str()) else {
            continue;
        };
        let target = entry["minecraftVersion"].as_str().unwrap_or("all");
        if target != "all" && target != game {
            continue;
        }
        let Ok(path) = maven_path(name) else {
            continue;
        };
        libraries.push(serde_json::json!({
            "name": name,
            "downloads": {
                "artifact": {
                    "url": download,
                    "path": path,
                    "sha1": entry["sha1"].as_str().unwrap_or_default(),
                    "size": entry["size"].as_u64().unwrap_or(0),
                }
            }
        }));
    }
    // LabyMod 客户端本体（带 launchwrapper 插件的那个 jar）不在库清单里，单独一条。
    libraries.push(serde_json::json!({
        "name": "net.labymod:LabyMod:4",
        "downloads": {
            "artifact": {
                "url": format!("{LABYMOD_DOWNLOAD}/labymod4/{channel}/{commit}.jar"),
                "path": "net/labymod/LabyMod/4/LabyMod-4.jar",
            }
        }
    }));
    merged["libraries"] = Value::Array(libraries);
    Ok(merged)
}

/// 装一个 LabyMod 实例片段：把官方客户端 json 落成版本文件，再按普通版本装库与 assets。
pub fn install_labymod(
    game_dir: &str,
    game: &str,
    version: &str,
    channel: &str,
    strategy: DownloadStrategy,
    threads: Option<usize>,
    cancel: CancellationToken,
    emit: impl FnMut(InstallProgress),
) -> Result<InstallSummary> {
    let entry = labymod_versions(channel)?
        .into_iter()
        .find(|entry| entry.game == game)
        .ok_or_else(|| {
            DownloadError::Invalid(format!("LabyMod does not support Minecraft {game}"))
        })?;
    if entry.version != version {
        return Err(DownloadError::Invalid(format!(
            "LabyMod {channel} is {}, not {version}",
            entry.version
        )));
    }
    let id = format!("labymod-{}-{game}", entry.commit);
    if !safe_component(&id) {
        return Err(DownloadError::Invalid("unsafe LabyMod version ID".into()));
    }
    let bytes = fetch_catalog_bytes(&entry.manifest_url, &cancel)?;
    let mut value: Value = serde_json::from_slice(&bytes)
        .map_err(|e| DownloadError::Invalid(format!("bad LabyMod client json: {e}")))?;
    value = append_labymod_libraries(&value, channel, game, &entry.commit)?;
    value["id"] = Value::String(id.clone());
    if value.get("type").is_none() {
        value["type"] = Value::String("release".into());
    }
    let root = PathBuf::from(game_dir);
    if !root.is_absolute() {
        return Err(DownloadError::Invalid(
            "game directory must be absolute".into(),
        ));
    }
    let dir = root.join("versions").join(&id);
    fs::create_dir_all(&dir).map_err(|e| io_error(&dir, e))?;
    let path = dir.join(format!("{id}.json"));
    let pretty = serde_json::to_vec_pretty(&value)
        .map_err(|e| DownloadError::Invalid(format!("encode LabyMod json: {e}")))?;
    fs::write(&path, pretty).map_err(|e| io_error(&path, e))?;
    install_profile_assets(game_dir, &id, strategy, threads, cancel, emit)
}

pub fn install_profile_assets(
    game_dir: &str,
    version_id: &str,
    strategy: DownloadStrategy,
    threads: Option<usize>,
    cancel: CancellationToken,
    emit: impl FnMut(InstallProgress),
) -> Result<InstallSummary> {
    install_profile_part(game_dir, version_id, strategy, threads, cancel, emit, false)
}

/// 只按版本 json 补 libraries / natives / logging。
///
/// 不查 Mojang 清单，也不要求 `assetIndex`——给带 `inheritsFrom` 的增量 profile
/// （老版本 Forge 的 `1.12.2-forge-…` 之类）用，客户端与资源由它继承的版本负责。
pub fn install_profile_libraries(
    game_dir: &str,
    version_id: &str,
    strategy: DownloadStrategy,
    threads: Option<usize>,
    cancel: CancellationToken,
    emit: impl FnMut(InstallProgress),
) -> Result<InstallSummary> {
    install_profile_part(game_dir, version_id, strategy, threads, cancel, emit, true)
}

#[allow(clippy::too_many_arguments)]
fn install_profile_part(
    game_dir: &str,
    version_id: &str,
    strategy: DownloadStrategy,
    threads: Option<usize>,
    cancel: CancellationToken,
    emit: impl FnMut(InstallProgress),
    libraries_only: bool,
) -> Result<InstallSummary> {
    let root = PathBuf::from(game_dir);
    if !root.is_absolute() {
        return Err(DownloadError::Invalid(
            "game directory must be absolute".into(),
        ));
    }
    if !safe_component(version_id) {
        return Err(DownloadError::Invalid("unsafe version ID".into()));
    }
    // 游戏目录可能还不存在（新拷一份 exe 出来），先建再 canonicalize。
    fs::create_dir_all(&root).map_err(|e| io_error(&root, e))?;
    let root = fs::canonicalize(&root).map_err(|e| io_error(&root, e))?;
    let mut installer = Installer {
        source: &Http::new(strategy)?.with_version(version_id),
        root,
        cache_root: None,
        cancel: &cancel,
        threads: threads.unwrap_or(4).max(1),
        quilt_loader: None,
        legacy_fabric: None,
        progress: InstallProgress {
            version_id: version_id.to_owned(),
            stage: String::new(),
            path: String::new(),
            completed_files: 0,
            total_files: 0,
            downloaded_bytes: 0,
            total_bytes: 0,
            percent: None,
        },
        weights: Progress::default(),
        summary: InstallSummary {
            version_id: version_id.to_owned(),
            path: tidy_path(&PathBuf::from(game_dir).join("versions").join(version_id)),
            downloaded_files: 0,
            skipped_files: 0,
            downloaded_bytes: 0,
        },
        emit,
    };
    let result = if libraries_only {
        installer.run_libraries(version_id)
    } else {
        installer.run_profile(version_id)
    };
    if let Err(error) = result {
        installer.event(
            if matches!(error, DownloadError::Cancelled) {
                "cancelled"
            } else {
                "failed"
            },
            "",
        );
        return Err(error);
    }
    installer.event("complete", "");
    Ok(installer.summary.clone())
}

#[allow(clippy::too_many_arguments)]
fn install_with_cache(
    source: &(impl Source + Sync),
    request: InstallRequest,
    cancel: &CancellationToken,
    emit: impl FnMut(InstallProgress),
    cache_root: Option<&Path>,
    threads: usize,
) -> Result<InstallSummary> {
    if !safe_component(&request.version_id) || request.game_dir.trim().is_empty() {
        return Err(DownloadError::Invalid(
            "game directory and safe version ID required".into(),
        ));
    }
    if request
        .fabric_loader
        .as_deref()
        .is_some_and(|loader| !safe_component(loader))
    {
        return Err(DownloadError::Invalid(
            "unsafe Fabric loader version".into(),
        ));
    }
    let root = PathBuf::from(&request.game_dir);
    if !root.is_absolute() {
        return Err(DownloadError::Invalid(
            "game directory must be absolute".into(),
        ));
    }
    cancel.check()?;
    fs::create_dir_all(&root).map_err(|e| io_error(&root, e))?;
    let root = fs::canonicalize(&root).map_err(|e| io_error(&root, e))?;
    let mut installer = Installer {
        source,
        root,
        cache_root,
        cancel,
        threads,
        quilt_loader: request.quilt_loader.clone(),
        legacy_fabric: None,
        progress: InstallProgress {
            version_id: request.version_id.clone(),
            stage: String::new(),
            path: String::new(),
            completed_files: 0,
            total_files: 0,
            downloaded_bytes: 0,
            total_bytes: 0,
            percent: None,
        },
        weights: Progress::default(),
        summary: InstallSummary {
            version_id: request.version_id.clone(),
            path: tidy_path(
                &PathBuf::from(&request.game_dir)
                    .join("versions")
                    .join(&request.version_id),
            ),
            downloaded_files: 0,
            skipped_files: 0,
            downloaded_bytes: 0,
        },
        emit,
    };
    let result = installer.run(&request.version_id, request.fabric_loader.as_deref());
    if let Err(ref error) = result {
        installer.event(
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

#[cfg(test)]
mod tests;
