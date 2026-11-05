//! 启动器全局设置：落在数据目录的 `settings.json`，缺失或损坏时回落到默认值。
//!
//! 只保存「跨实例」的偏好；每个实例自己的内存/Java/玩家名仍在实例记录里。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::portable::Paths;

const SETTINGS_FILE: &str = "settings.json";
const SCHEMA_VERSION: u32 = 3;
/// 默认堆内存：现代版本 2 GB 起，低于 1 GB 很多模组包会直接 OOM。
pub const DEFAULT_MEMORY_MB: u32 = 4096;
pub const MAX_MEMORY_MB: u32 = 32768;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub schema_version: u32,
    /// 新建实例时的默认玩家名。
    pub username: String,
    /// 新建实例时的默认堆内存（MB）。
    pub memory_mb: u32,
    /// 新建实例时的默认 Java；空串表示每次自动挑最合适的。
    pub java_path: String,
    /// 是否播放启动动画。
    pub show_splash: bool,
    /// 下载源策略：`race`（两者竞速，默认）、`official-first`、`mirror-first`、
    /// `mirror-only`、`official-only`。没有镜像路由的文件永远只走官方。
    #[serde(default)]
    pub download_strategy: Option<String>,
    /// 最大线程数：同时下载的线程上限，1..=256（PCL 同款，默认 64）。
    pub concurrent_downloads: u32,
    /// 全局限速档位（0..=42，PCL 的 `ToolDownloadSpeed`）：42 表示不限速，
    /// 0..=14 每档 0.1 MB/s，15..=31 每档 0.5 MB/s，32..=41 每档 1 MB/s。
    #[serde(default = "default_speed_tier")]
    pub download_speed: u32,
    /// 旧字段：限速（KB/s，0 = 不限），仅用于读老配置；迁移到档位后不再保存。
    #[serde(default, skip_serializing)]
    pub speed_limit: u32,
    /// 旧字段：下载源，仅用于读老配置（下次保存后消失）。
    #[serde(default, skip_serializing)]
    pub download_source: String,
    /// 旧字段：是否竞速，仅用于读老配置（下次保存后消失）。
    #[serde(default, skip_serializing)]
    pub mirror_race: bool,
    /// 首页左侧实例栏是否收起。
    pub home_rail_collapsed: bool,
    /// 内容与实例的版本/加载器不匹配时是否给黄字提示（仍可下载）。
    pub compat_warning: bool,
    /// 下载社区资源时的文件名格式（0..=4，与 PCL 的 NameFormatV2 一致）。
    #[serde(default = "default_name_format")]
    pub file_name_format: u8,
    /// 内容列表每页条数（10..=100），与 PCL 的 compPageSize 类似，默认 40。
    pub page_size: u32,
    /// 内容页左栏宽度百分比（25..=65），默认 38。
    pub split_ratio: u32,
    /// 版本页左栏宽度百分比（12..=30），全局一份，默认 16。
    pub versions_split_ratio: u32,
    /// 分组时忽略 Quilt（PCL 的 `Config.Download.Comp.IgnoreQuilt`，默认关）。
    pub ignore_quilt: bool,
    /// 下载社区资源与安装包时是否弹系统「另存为」。
    #[serde(default = "default_true")]
    pub ask_save_location: bool,
    /// 整合包：true = 下载后直接装成实例，false = 只把 .mrpack 存下来。
    pub modpack_install: bool,
    /// 世界包：true = 解压进 `saves`，false = 只把 zip 存进 `downloads`。
    #[serde(default = "default_true")]
    pub world_extract: bool,
    /// 顶部导航切换时药丸滑动的时长（毫秒，0 = 不滑动，120..=600 常用）。
    #[serde(default = "default_nav_slide_ms")]
    pub nav_slide_ms: u32,
    /// 用户添加的游戏目录，以及被改过名的目录（`name` 为空串表示用默认名字）。
    #[serde(default)]
    pub game_folders: Vec<FolderEntry>,
    /// 版本页左栏的显示顺序（绝对路径）。不在表里的目录排在后面。
    #[serde(default)]
    pub folder_order: Vec<String>,
    /// 当前选中的游戏目录；空串表示用默认目录（exe 同级 `.minecraft`）。
    #[serde(default)]
    pub active_game_folder: String,
}

/// 版本页左栏里的一条游戏目录。自动发现的目录只在「改过名」时才落进设置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FolderEntry {
    /// 绝对路径，不含末尾分隔符。
    pub path: String,
    /// 显示名；空串表示用默认名字（当前文件夹 / 官方启动器文件夹 / 目录名）。
    pub name: String,
    /// true = 用户手动「添加已有文件夹」进来的。
    pub added: bool,
}

/// 目录显示名的长度上限（与 PCL 一致：1–30 字）。
pub const MAX_FOLDER_NAME: usize = 30;

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            username: "Steve".into(),
            memory_mb: DEFAULT_MEMORY_MB,
            java_path: String::new(),
            show_splash: true,
            download_strategy: Some("race".into()),
            download_source: String::new(),
            concurrent_downloads: 64,
            download_speed: launcher_core::download::SPEED_TIER_UNLIMITED,
            speed_limit: 0,
            mirror_race: true,
            home_rail_collapsed: false,
            page_size: default_page_size(),
            split_ratio: default_split_ratio(),
            versions_split_ratio: default_versions_split_ratio(),
            ignore_quilt: false,
            compat_warning: true,
            file_name_format: 4,
            ask_save_location: true,
            modpack_install: true,
            world_extract: true,
            nav_slide_ms: default_nav_slide_ms(),
            game_folders: Vec::new(),
            folder_order: Vec::new(),
            active_game_folder: String::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_speed_tier() -> u32 {
    launcher_core::download::SPEED_TIER_UNLIMITED
}

fn default_name_format() -> u8 {
    4
}

fn default_nav_slide_ms() -> u32 {
    220
}

impl Settings {
    /// 把越界/空白的值拉回可用范围，避免手改 json 把启动搞坏。
    pub fn sanitized(mut self) -> Self {
        if self.username.trim().is_empty() {
            self.username = "Steve".into();
        }
        self.username = self.username.trim().chars().take(24).collect();
        self.memory_mb = self.memory_mb.clamp(512, MAX_MEMORY_MB);
        self.concurrent_downloads = self.concurrent_downloads.clamp(1, 256);
        // 老配置（KB/s）迁到档位；新配置只认档位。
        if self.schema_version < 3 && self.speed_limit > 0 {
            self.download_speed = launcher_core::download::tier_for_limit(self.speed_limit as u64 * 1024);
        }
        self.speed_limit = 0;
        self.download_speed = self.download_speed.min(launcher_core::download::SPEED_TIER_UNLIMITED);
        self.page_size = self.page_size.clamp(10, 50);
        self.split_ratio = self.split_ratio.clamp(25, 65);
        self.versions_split_ratio = self.versions_split_ratio.clamp(12, 30);
        self.nav_slide_ms = self.nav_slide_ms.min(600);
        if self.file_name_format > 4 {
            self.file_name_format = 4;
        }
        // 新配置优先；老配置（download_source + mirror_race）映射过来。
        let parsed = self
            .download_strategy
            .as_deref()
            .and_then(launcher_core::download::DownloadStrategy::parse);
        let strategy = parsed.unwrap_or_else(|| {
            launcher_core::download::DownloadStrategy::from_legacy(
                &self.download_source,
                self.mirror_race,
            )
        });
        self.download_strategy = Some(strategy.as_str().to_owned());
        self.download_source = String::new();
        self.mirror_race = false;
        // 游戏目录表：去掉空路径与重复项（Windows 下按不区分大小写去重）。
        let mut seen: Vec<String> = Vec::new();
        self.game_folders.retain(|entry| {
            let path = entry.path.trim().trim_end_matches(['\\', '/']).to_owned();
            if path.is_empty() || seen.iter().any(|kept| kept.eq_ignore_ascii_case(&path)) {
                return false;
            }
            seen.push(path);
            true
        });
        for entry in self.game_folders.iter_mut() {
            entry.path = entry.path.trim().trim_end_matches(['\\', '/']).to_owned();
            entry.name = entry.name.trim().chars().take(MAX_FOLDER_NAME).collect();
        }
        let mut order: Vec<String> = Vec::new();
        self.folder_order.retain(|path| {
            let path = path.trim().trim_end_matches(['\\', '/']).to_owned();
            if path.is_empty() || order.iter().any(|kept| kept.eq_ignore_ascii_case(&path)) {
                return false;
            }
            order.push(path);
            true
        });
        self.folder_order = order;
        self.active_game_folder = self
            .active_game_folder
            .trim()
            .trim_end_matches(['\\', '/'])
            .to_owned();
        self.schema_version = SCHEMA_VERSION;
        self
    }
}

pub fn settings_path(paths: &Paths) -> PathBuf {
    paths.data_dir().join(SETTINGS_FILE)
}

/// 读取设置；文件不存在、损坏或不是合法 JSON 时都返回默认值而不是报错。
pub fn load(paths: &Paths) -> Settings {
    read_file(&settings_path(paths))
        .unwrap_or_default()
        .sanitized()
}

pub fn save(paths: &Paths, settings: Settings) -> Result<Settings, String> {
    let settings = settings.sanitized();
    let path = settings_path(paths);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| io_message(parent, e))?;
    }
    let json =
        serde_json::to_string_pretty(&settings).map_err(|e| format!("无法序列化设置：{e}"))?;
    let temporary = path.with_extension("json.part");
    fs::write(&temporary, json).map_err(|e| io_message(&temporary, e))?;
    fs::rename(&temporary, &path).map_err(|e| io_message(&path, e))?;
    Ok(settings)
}

fn read_file(path: &Path) -> Option<Settings> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn io_message(path: &Path, error: io::Error) -> String {
    format!("{}: {error}", path.display())
}

/// 内容列表每页条数的默认值。
fn default_page_size() -> u32 {
    40
}

/// 左栏宽度的默认百分比。
fn default_split_ratio() -> u32 {
    38
}

fn default_versions_split_ratio() -> u32 {
    16
}
