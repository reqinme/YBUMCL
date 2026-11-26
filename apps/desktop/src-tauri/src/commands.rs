//! 启动器命令层：实例管理 + 版本扫描 + Java 检测 + 启动游戏。
//!
//! 约定：所有命令都返回 `Result<T, String>`，前端直接把字符串当错误提示展示。

use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read, Seek};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use launcher_core::download::{FabricLoaderVersion, list_fabric_loaders, list_quilt_loaders};
use launcher_core::instance::{InstanceRequest, install_instance};
use launcher_core::java::LoaderPresence;
use launcher_core::loader_catalog::{self, LoaderEntry};
use launcher_core::modloader::{
    ModLoader, ModLoaderInstallRequest, ModLoaderVersion, install_modloader,
    list_modloader_versions,
};
use launcher_core::{
    CancellationToken, InstallProgress, InstallRequest, InstallSummary, JavaInstallation,
    RemoteVersionManifest, VersionSummary, install_version,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::portable::Paths;

/// 游戏日志最多保留多少行，防止长时间挂机把内存吃满。
const MAX_LOG_LINES: usize = 2000;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub stream: String,
    pub text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameSnapshot {
    pub running: bool,
    pub pid: Option<u32>,
    pub last_exit: Option<i32>,
    pub logs: Vec<LogLine>,
}

/// 一次启动。同一实例可以同时开多份（PCL 也不拦），所以按「启动」而不是按实例名存。
pub struct Run {
    pub id: u64,
    pub version_id: String,
    child: Option<Child>,
    pid: Option<u32>,
    logs: VecDeque<LogLine>,
    last_exit: Option<i32>,
    /// 这次启动的落盘日志；进程一起就只能追加，别的地方不动它。
    log: Option<crate::runlog::RunLog>,
    /// 用户点了「结束实例」：之后的非零退出码不算崩溃。
    stopped: bool,
}

impl Run {
    fn new(id: u64, version_id: String) -> Self {
        Self {
            id,
            version_id,
            child: None,
            pid: None,
            logs: VecDeque::new(),
            last_exit: None,
            log: None,
            stopped: false,
        }
    }
}

/// 所有启动 + 最近一次启动的编号（`game_snapshot` 不带参数时用它）。
#[derive(Default)]
pub struct Sessions {
    runs: std::collections::HashMap<u64, Run>,
    next_id: u64,
    last: Option<u64>,
}

impl Sessions {
    /// 开一次新启动，先登记版本名，进程起来之后再挂上 child。
    fn begin(&mut self, version_id: &str) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.runs.insert(id, Run::new(id, version_id.to_owned()));
        self.last = Some(id);
        self.prune();
        id
    }

    fn running(&self) -> usize {
        self.runs.values().filter(|run| run.child.is_some()).count()
    }

    /// 已结束的启动只留最近 20 条，避免长时间挂机越攒越多。
    fn prune(&mut self) {
        let mut finished: Vec<u64> = self
            .runs
            .values()
            .filter(|run| run.child.is_none())
            .map(|run| run.id)
            .collect();
        if finished.len() <= 20 {
            return;
        }
        finished.sort_unstable();
        let drop_count = finished.len() - 20;
        for id in finished.into_iter().take(drop_count) {
            self.runs.remove(&id);
            if self.last == Some(id) {
                self.last = None;
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct GameRuntime(pub Arc<Mutex<Sessions>>);

/// 崩溃分析用的上下文：读游戏自己的 latest.log 尾部（有就带上）。
fn crash_tail(game_dir: &str, version_id: &str) -> Vec<String> {
    let path = std::path::Path::new(game_dir)
        .join("versions")
        .join(version_id)
        .join("logs")
        .join("latest.log");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(300);
    lines[start..]
        .iter()
        .map(|line| format!("[latest.log] {line}"))
        .collect()
}

fn publish(app: &AppHandle, runtime: &GameRuntime, run_id: u64, line: LogLine) {
    let mut version_id = String::new();
    if let Ok(mut sessions) = runtime.0.lock() {
        if let Some(run) = sessions.runs.get_mut(&run_id) {
            version_id = run.version_id.clone();
            if let Some(log) = run.log.as_mut() {
                log.push(&line.stream, &line.text);
            }
            run.logs.push_back(line.clone());
            while run.logs.len() > MAX_LOG_LINES {
                run.logs.pop_front();
            }
        }
    }
    let mut payload = serde_json::to_value(&line).unwrap_or(serde_json::Value::Null);
    if let Some(object) = payload.as_object_mut() {
        object.insert("runId".into(), serde_json::json!(run_id));
        object.insert("versionId".into(), serde_json::Value::String(version_id));
    }
    let _ = app.emit("game-log", &payload);
}

// ------------------------------------------------------------ 版本（即实例）

/// 前端列表项：版本本体 + 该版本合并后的启动配置。
#[derive(Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionView {
    pub id: String,
    pub kind: String,
    pub supported: bool,
    pub reason: Option<String>,
    pub isolated: bool,
    pub java_path: String,
    /// 0 表示还没配，前端用全局默认值展示。
    pub max_memory_mb: u32,
    /// 该版本实际使用的游戏目录（隔离时是 versions/<id>）。
    pub game_directory: String,
    /// 基础游戏版本（`1.21.10`、`26.3` …）；认不出来是 None。
    pub game_version: Option<String>,
    /// `release` / `snapshot` / `april` / `unknown`。
    pub channel: String,
    /// 加载器显示名，可多个（Forge + OptiFine）。
    pub loaders: Vec<String>,
    /// 加载器与版本号（从实例 json 的 `_ybmcl.loaders` 读回），旧实例可能为空。
    pub loaders_detail: Vec<launcher_core::instance::LoaderChoice>,
    /// 归属的分类 id。
    pub categories: Vec<String>,
    /// 用户写的实例描述。
    pub description: String,
    /// 图标设置：空=自动，`builtin:<方块名>`、`custom`。
    pub icon: String,
    /// 启动次数。
    pub launches: u64,
    /// 安装时间（unix 秒），排序「默认顺序」用；老实例回落到目录创建时间。
    #[serde(default)]
    pub installed_at: Option<u64>,
    /// 最近一次启动时间（`logs/runs/index.json` 里的 `started`），没启动过是 None。
    #[serde(default)]
    pub last_started: Option<String>,
    /// 整合包信息；手动装的实例是 None。
    pub pack: Option<crate::versions::PackInfo>,
    /// 内存分配方式：0 自动、1 自定义、2 跟随全局。
    pub memory_type: u32,
    /// 自定义内存（MB），`memory_type == 1` 时生效。
    pub memory_custom_mb: u32,
    /// 初始堆（MB），0 表示和最大堆一样。
    pub memory_initial_mb: u32,
    /// 启动前内存优化：0 跟随全局、1 开启、2 关闭。
    pub memory_optimize: u32,
    /// 「游戏窗口标题」，空=不改。
    pub window_title: String,
    /// 「自定义信息」：覆盖 `${version_type}`，空=用版本自己的。
    pub version_type: String,
    /// 「JVM 参数头部」：非空时整段替换版本自带的 JVM 参数。
    pub jvm_args: String,
    /// 「游戏参数尾部」：追加在版本参数之后。
    pub game_args: String,
    /// 「Classpath 头部附加」：插在 classpath 最前。
    pub classpath_head: String,
    /// 「启动前执行命令」。
    pub pre_launch: String,
    /// 启动前命令是否等它跑完。
    pub pre_launch_wait: bool,
    /// Java 选「自动选择」。
    pub java_auto: bool,
    /// 「关闭文件校验」。
    pub skip_file_check: bool,
    /// 「使用调试级别 log4j 配置文件」。
    pub debug_log4j: bool,
    /// 「忽略 Java 兼容性警告」。
    pub ignore_java_warning: bool,
    /// 「限制验证方式」：0 不限制、1 仅正版。
    pub login_require: u32,
    /// 「自动进入服务器」。
    pub server_entry: String,
}


/// 读实例 json 里的 `_ybmcl.loaders`（安装时写入）：旧实例没有这段，返回空。
fn instance_loader_choices(
    game_dir: &str,
    id: &str,
) -> Vec<launcher_core::instance::LoaderChoice> {
    let path = PathBuf::from(game_dir)
        .join("versions")
        .join(id)
        .join(format!("{id}.json"));
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    // 真相只有一个：实例 json 里实际挂了哪些加载器库。
    // 不信 `_ybmcl` 的记录，也不从实例 id 里的字样去猜。
    loaders_from_libraries(&json)
}

fn loaders_from_libraries(json: &serde_json::Value) -> Vec<launcher_core::instance::LoaderChoice> {
    let mut out: Vec<launcher_core::instance::LoaderChoice> = Vec::new();
    let Some(libraries) = json.get("libraries").and_then(|value| value.as_array()) else {
        return out;
    };
    // Legacy Fabric 的库清单里同时有 `net.legacyfabric:*` 和 `net.fabricmc:fabric-loader`，
    // 先扫一遍整份清单，有前者就说明这是 Legacy Fabric，Fabric 那条规则要跳过。
    let legacy = libraries.iter().any(|library| {
        library
            .get("name")
            .and_then(|value| value.as_str())
            .is_some_and(|name| name.starts_with("net.legacyfabric"))
    });
    for library in libraries {
        let Some(name) = library.get("name").and_then(|value| value.as_str()) else {
            continue;
        };
        let parts: Vec<&str> = name.split(':').collect();
        if parts.len() < 3 {
            continue;
        }
        let group = parts[0];
        let artifact = parts[1];
        let loader = match (group, artifact) {
            ("net.minecraftforge", "forge") | ("net.minecraftforge", "fmlloader") => "forge",
            ("net.neoforged", "neoforge") | ("net.neoforged", "fmlloader") => "neoforge",
            // Legacy Fabric 把 Fabric Loader 整个搬到了旧版本，库清单里同时有
            // `net.legacyfabric:intermediary` 和 `net.fabricmc:fabric-loader`。
            // 版本号要从 fabric-loader 那条拿（intermediary 带的是游戏版本），
            // 所以这里只认 fabric-loader，再按 legacy 分流。
            ("net.fabricmc", "fabric-loader") if legacy => "legacyfabric",
            ("net.fabricmc", "fabric-loader") => "fabric",
            ("org.quiltmc", "quilt-loader") => "quilt",
            ("com.mumfrey", "liteloader") => "liteloader",
            ("com.cleanroommc", "cleanroom") => "cleanroom",
            // LabyMod 的库全在 `net.labymod` 下面，没有单独的「加载器」坐标，
            // 版本号只能从安装时记下的 `_ybmcl.loaders` 里取。
            ("net.labymod", _) => "labymod",
            (_, "optifine") | (_, "OptiFine") => "optifine",
            // NeoForge 20.2+ 把 FML 拆到了 net.neoforged.fancymodloader，
            // 主组 net.neoforged 下还挂着 bus/coremods/accesstransformers 这些，靠它认。
            ("net.neoforged", _) | ("net.neoforged.fancymodloader", _) => "neoforge",
            _ => continue,
        };
        if out.iter().any(|item| item.loader == loader) {
            continue;
        }
        let raw = parts[2];
        let mut version = if loader == "forge" || loader == "neoforge" {
            // 老版本 Forge 的坐标是 `<游戏版本>-<Forge 版本>`，1.7.10 还多带一段
            // 游戏版本尾巴（`1.7.10-10.13.4.1614-1.7.10`）；现代 Forge 直接就是
            // 版本号（`47.4.28`）。统一取中间那段，别把游戏版本当加载器版本。
            if raw.starts_with("1.") {
                raw.split('-')
                    .nth(1)
                    .filter(|part| !part.is_empty())
                    .unwrap_or(raw)
                    .to_owned()
            } else {
                raw.to_owned()
            }
        } else {
            raw.to_owned()
        };
        // fancymodloader 那几项带的是 FML 自己的版本，不是 NeoForge 的版本号，
        // 先留空，等参数里的 --fml.neoForgeVersion 补上。
        if loader == "neoforge" && artifact != "neoforge" && artifact != "fmlloader" {
            version.clear();
        }
        // NeoForge 的正式版本号在启动参数里（--fml.neoForgeVersion 21.1.265），
        // libraries 里只有 FML 自己的版本（4.0.45），优先用参数里的那个。
        if loader == "neoforge" {
            if let Some(found) = argument_value(json, "--fml.neoForgeVersion") {
                version = found;
            }
        }
        // LabyMod 的库版本号是各个子库自己的（dnsjava 4.0.0 之类），不是加载器版本。
        if loader == "labymod" {
            version = recorded_loader_version(json, "labymod").unwrap_or_default();
        }
        out.push(launcher_core::instance::LoaderChoice {
            loader: loader.to_owned(),
            version,
        });
    }
    out
}

/// 安装时记在实例 json 里的加载器版本（`_ybmcl.loaders[].version`）。
fn recorded_loader_version(json: &serde_json::Value, loader: &str) -> Option<String> {
    json.get("_ybmcl")?
        .get("loaders")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("loader").and_then(|value| value.as_str()) == Some(loader))?
        .get("version")?
        .as_str()
        .map(str::to_owned)
}

/// 从 `arguments.game` 里取 `--flag value` 形式的参数值。
fn argument_value(json: &serde_json::Value, flag: &str) -> Option<String> {
    let game = json
        .get("arguments")
        .and_then(|value| value.get("game"))
        .and_then(|value| value.as_array())?;
    let mut previous: Option<&str> = None;
    for entry in game {
        let Some(text) = entry.as_str() else {
            previous = None;
            continue;
        };
        if previous == Some(flag) {
            return Some(text.to_owned());
        }
        previous = Some(text);
    }
    None
}

fn versions_root(game_dir: &Path) -> PathBuf {
    game_dir.join("versions")
}

/// 校验并返回版本目录，杜绝 `../` 之类越界删除。
fn version_dir(game_dir: &Path, id: &str) -> Result<PathBuf, String> {
    if !crate::versions::is_safe_version_id(id) {
        return Err(format!("非法的版本 id：{id}"));
    }
    Ok(versions_root(game_dir).join(id))
}

/// 记下当前目录的版本排序方式（每个文件夹各存一份）。
#[tauri::command]
pub fn set_version_order(
    paths: State<'_, Paths>,
    order: String,
) -> Result<crate::categories::CategorySnapshot, String> {
    let game_dir = crate::game_folders::current(&paths);
    crate::categories::set_order(&paths.data_dir(), &game_dir.to_string_lossy(), &order)
}

#[tauri::command]
pub async fn list_versions(
    paths: State<'_, Paths>,
    runtime: State<'_, GameRuntime>,
) -> Result<Vec<VersionView>, String> {
    let game_dir = crate::game_folders::current(&paths);
    let data_dir = paths.data_dir().to_owned();
    let log_dir = paths.log_dir();
    let running = runtime
        .0
        .lock()
        .map(|sessions| sessions.running() > 0)
        .unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        let _ = running;
        let mut views = versions_cached(&game_dir, &data_dir)?;
        // 「最近启动」排序要的时间不在版本缓存里（它来自启动记录），每次列表都现填。
        let mut latest: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for run in crate::runlog::history(&log_dir, 500) {
            latest.entry(run.version_id).or_insert(run.started);
        }
        for view in &mut views {
            view.last_started = latest.get(&view.id).cloned();
        }
        // 安装中的底稿 / 外壳也是真实目录，但还不是「版本」，先别显示出来。
        let busy = launcher_core::busy_installs();
        views.retain(|view| !busy.iter().any(|id| *id == view.id));
        Ok(views)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 版本列表：先看磁盘缓存（PCL 的 `InstanceCache` 那套），键对不上才算。
fn versions_cached(game_dir: &Path, data_dir: &Path) -> Result<Vec<VersionView>, String> {
    let key = crate::instance_cache::key(game_dir);
    if let Some(views) = crate::instance_cache::read::<Vec<VersionView>>(data_dir, &key) {
        return Ok(views);
    }
    let views = collect_views(game_dir, data_dir)?;
    crate::instance_cache::write(data_dir, &key, &views);
    Ok(views)
}

/// 启动时预热：把概述算好写进缓存，之后打开版本页 / 实例页就不用等。
pub fn warm_instance_cache(game_dir: &Path, data_dir: &Path) {
    let key = crate::instance_cache::key(game_dir);
    if crate::instance_cache::read::<Vec<VersionView>>(data_dir, &key).is_some() {
        return;
    }
    if let Ok(views) = collect_views(game_dir, data_dir) {
        crate::instance_cache::write(data_dir, &key, &views);
    }
}

/// 真正扫一遍所有实例，拼出列表页 / 实例页概述要的那份数据。
/// 实例的安装时间：优先读 json 里 `_ybmcl.installedAt`，老实例用目录创建时间兜底。
fn installed_at_of(game_dir: &Path, id: &str) -> Option<u64> {
    let folder = game_dir.join("versions").join(id);
    let json = folder.join(format!("{id}.json"));
    if let Ok(bytes) = std::fs::read(&json) {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Some(stamp) = value
                .get("_ybmcl")
                .and_then(|meta| meta.get("installedAt"))
                .and_then(serde_json::Value::as_u64)
            {
                return Some(stamp);
            }
        }
    }
    std::fs::metadata(&folder)
        .and_then(|meta| meta.created())
        .ok()
        .and_then(|created| created.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_secs())
}

fn collect_views(game_dir: &Path, data_dir: &Path) -> Result<Vec<VersionView>, String> {
    {
        let game_dir = game_dir.to_path_buf();
        let data_dir = data_dir.to_path_buf();
        let scanned = match launcher_core::scan_versions(&game_dir) {
            Ok(scanned) => scanned,
            // 首次运行还没建游戏目录，这是「还没装版本」，不是错误。
            Err(_) if !game_dir.is_dir() => Vec::new(),
            Err(error) => return Err(error.to_string()),
        };
        let ids: Vec<String> = scanned.iter().map(|version| version.id.clone()).collect();
        let metas = crate::version_meta::describe_all(&game_dir, &ids);
        let categories = crate::categories::snapshot(&data_dir, &game_dir.to_string_lossy());
        let mut out = Vec::with_capacity(scanned.len());
        for version in scanned {
            let settings = crate::versions::load_instance(&game_dir, &version.id);
            let meta = metas.get(&version.id).cloned().unwrap_or_default();
            let game_directory =
                match crate::versions::game_directory(&version.id, settings.isolated) {
                    Some(relative) => launcher_core::resolve_runtime_directory(
                        &game_dir.to_string_lossy(),
                        Some(&relative),
                    )
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| {
                        game_dir
                            .join("versions")
                            .join(&version.id)
                            .to_string_lossy()
                            .into_owned()
                    }),
                    None => game_dir.to_string_lossy().into_owned(),
                };
            let category_key = version.id.clone();
            let loaders_detail = instance_loader_choices(&game_dir.to_string_lossy(), &version.id);
            let installed_at = installed_at_of(&game_dir, &version.id);
            out.push(VersionView {
                id: version.id,
                kind: version.kind,
                supported: version.supported,
                reason: version.reason,
                isolated: settings.isolated,
                java_path: settings.java_path,
                max_memory_mb: settings.max_memory_mb,
                game_directory,
                game_version: meta.game_version,
                channel: meta.channel,
                loaders: meta.loaders,
                loaders_detail,
                categories: categories
                    .tags
                    .get(&category_key)
                    .cloned()
                    .unwrap_or_default(),
                description: settings.description.clone(),
                icon: settings.icon.clone(),
                launches: settings.launches,
                installed_at,
                last_started: None,
                pack: settings.pack.clone(),
                memory_type: settings.memory_type,
                memory_custom_mb: settings.memory_custom_mb,
                memory_initial_mb: settings.memory_initial_mb,
                memory_optimize: settings.memory_optimize,
                window_title: settings.window_title.clone(),
                version_type: settings.version_type.clone(),
                jvm_args: settings.jvm_args.clone(),
                game_args: settings.game_args.clone(),
                classpath_head: settings.classpath_head.clone(),
                pre_launch: settings.pre_launch.clone(),
                pre_launch_wait: settings.pre_launch_wait,
                java_auto: settings.java_auto,
                skip_file_check: settings.skip_file_check,
                debug_log4j: settings.debug_log4j,
                ignore_java_warning: settings.ignore_java_warning,
                login_require: settings.login_require,
                server_entry: settings.server_entry.clone(),
            });
        }
        Ok(out)
    }
}

#[tauri::command]
pub fn save_version_settings(
    paths: State<'_, Paths>,
    id: String,
    settings: crate::versions::VersionSettings,
) -> Result<(), String> {
    crate::versions::save_launch_fields(&crate::game_folders::current(&paths), &id, settings)
}

/// 删除一个版本：连同 `versions/<id>` 整目录（含存档、mods）一起删。
/// 前端负责二次确认，后端只做路径越界校验。
#[tauri::command]
pub async fn delete_version(
    paths: State<'_, Paths>,
    id: String,
    permanent: Option<bool>,
) -> Result<(), String> {
    let game_dir = crate::game_folders::current(&paths);
    let data_dir = paths.data_dir().to_owned();
    let permanent = permanent.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        let dir = version_dir(&game_dir, &id)?;
        if dir.starts_with(&game_dir) && dir.is_dir() {
            // 照 PCL CE：默认扔进回收站（还能捞回来），按住 Shift 才永久删。
            if permanent || !crate::recycle::to_recycle_bin(&dir) {
                std::fs::remove_dir_all(&dir).map_err(|e| format!("删除版本失败：{e}"))?;
            }
        }
        crate::categories::forget_version(&data_dir, &game_dir.to_string_lossy(), &id);
        crate::versions::forget(&data_dir, &id)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 打开版本目录 / 它的 mods / 它的存档。
#[tauri::command]
pub async fn open_version_folder(
    app: AppHandle,
    paths: State<'_, Paths>,
    id: String,
    which: Option<String>,
) -> Result<(), String> {
    let _ = &app;
    let game_dir = crate::game_folders::current(&paths);
    let settings = crate::versions::load_instance(&game_dir, &id);
    let root = match crate::versions::game_directory(&id, settings.isolated) {
        Some(relative) => {
            launcher_core::resolve_runtime_directory(&game_dir.to_string_lossy(), Some(&relative))
                .map_err(|e| e.to_string())?
        }
        None => game_dir.clone(),
    };
    let target = match which.as_deref() {
        Some("mods") => root.join("mods"),
        Some("saves") => root.join("saves"),
        _ => root.clone(),
    };
    std::fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    {
        use tauri_plugin_opener::OpenerExt;
        app.opener()
            .open_path(target.to_string_lossy().into_owned(), None::<&str>)
            .map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub fn default_game_dir(paths: State<'_, Paths>) -> String {
    crate::game_folders::current(&paths).to_string_lossy().into_owned()
}

/// 一个目录的「新鲜度」：改过没、最后改的时间、里面有多少个文件。
/// 挑默认下载位置时按 PCL 的口径排序用（改得越晚、文件越多越可能是你在用的那个实例）。
#[tauri::command]
pub fn ensure_dir(path: String) -> Result<(), String> {
    let target = Path::new(&path);
    if !target.is_absolute() {
        return Err("目录必须是绝对路径".into());
    }
    std::fs::create_dir_all(target).map_err(|error| format!("{path}: {error}"))
}

// ---------------------------------------------------------------- 版本 / Java

#[tauri::command]
pub async fn scan_versions(game_dir: String) -> Result<Vec<VersionSummary>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::scan_versions(Path::new(&game_dir)).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------- 版本图标

/// 最小 base64（不引第三方库，只用来拼 `data:` URL）。
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for group in bytes.chunks(3) {
        let chunk = [
            group[0],
            *group.get(1).unwrap_or(&0),
            *group.get(2).unwrap_or(&0),
        ];
        let bits = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | chunk[2] as u32;
        out.push(TABLE[(bits >> 18) as usize & 63] as char);
        out.push(TABLE[(bits >> 12) as usize & 63] as char);
        out.push(if group.len() > 1 {
            TABLE[(bits >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if group.len() > 2 {
            TABLE[bits as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn icon_data_url(path: &Path) -> Option<String> {
    let mime = match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => return None,
    };
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(format!("data:{mime};base64,{}", base64(&bytes)))
}

/// 从 PCL 的 `PCL/Setup.ini` 里取用户自己设过的图标。
/// PCL 把内置图写成 `pack://application:,,,...`，那个不是文件，跳过。
fn pcl_custom_icon(instance: &Path, base: &Path) -> Option<String> {
    let text = std::fs::read_to_string(instance.join("PCL").join("Setup.ini")).ok()?;
    let mut custom = false;
    let mut logo = String::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "LogoCustom" => custom = value.trim().eq_ignore_ascii_case("true"),
            "Logo" => logo = value.trim().to_owned(),
            _ => {}
        }
    }
    if !custom || logo.is_empty() || logo.starts_with("pack://") {
        return None;
    }
    let resolved = if let Some(rest) = logo.strip_prefix('$') {
        base.join(rest.trim_start_matches(['\\', '/']))
    } else {
        PathBuf::from(&logo)
    };
    icon_data_url(&resolved)
}

/// 版本图标：先认实例目录里的 `icon.png/jpg`，再认 PCL 设置过的自定义图标；
/// 都没有就返回 `None`，前端按「版本类型」用内置的方块图。
#[tauri::command]
pub fn version_icon(paths: State<'_, Paths>, id: String) -> Option<String> {
    let game_dir = crate::game_folders::current(&paths);
    let instance = game_dir.join("versions").join(&id);
    let icon = crate::versions::load_instance(&game_dir, &id).icon;
    if icon == "custom" {
        for name in ["icon.png", "icon.jpg", "icon.jpeg", "icon.webp", "icon.gif"] {
            if let Some(url) = icon_data_url(&instance.join(name)) {
                return Some(url);
            }
        }
    } else if icon.starts_with("builtin:") {
        return Some(icon);
    }
    for name in ["icon.png", "icon.jpg", "icon.jpeg", "icon.webp", "icon.gif"] {
        if let Some(url) = icon_data_url(&instance.join(name)) {
            return Some(url);
        }
    }
    // 别人用 PCL 装过的实例，图标就放在 PCL 自己的设置里，顺带认一下。
    pcl_custom_icon(&instance, paths.exe_dir())
}

/// 改实例图标。`icon`：空=自动，`builtin:<方块名>`、`custom`。
#[tauri::command]
pub fn set_version_icon(
    paths: State<'_, Paths>,
    id: String,
    icon: String,
) -> Result<crate::versions::VersionSettings, String> {
    crate::versions::set_icon(&crate::game_folders::current(&paths), &id, &icon)
}

/// 选一张图片当图标：弹选择框，拷成 `versions/<id>/icon.png`，图标改成自定义。
/// 用户取消返回 `None`。
#[tauri::command]
pub fn pick_version_icon(
    app: AppHandle,
    paths: State<'_, Paths>,
    id: String,
) -> Result<Option<crate::versions::VersionSettings>, String> {
    use tauri_plugin_dialog::DialogExt;
    let game_dir = crate::game_folders::current(&paths);
    let Some(picked) = app
        .dialog()
        .file()
        .set_title("选择图标")
        .add_filter(
            "常用图片文件",
            &["png", "jpeg", "jpg", "gif", "webp"],
        )
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let source = picked
        .into_path()
        .map_err(|_| "选中的不是本地文件".to_string())?;
    crate::versions::set_icon_image(&game_dir, &id, &source).map(Some)
}

/// 启动器起的是 Oracle javapath 的 `java.exe`，它会把真正的 JVM 再起一份：
/// 窗口属于孙子进程，所以先把整棵进程树捞出来。
#[cfg(windows)]
fn process_tree(root: u32) -> Vec<u32> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut pairs: Vec<(u32, u32)> = Vec::new();
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot.is_null() || snapshot as isize == -1 {
        return vec![root];
    }
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while ok {
        pairs.push((entry.th32ProcessID, entry.th32ParentProcessID));
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        ok = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };

    let mut tree = vec![root];
    loop {
        let mut added = false;
        for (pid, parent) in &pairs {
            if tree.contains(parent) && !tree.contains(pid) {
                tree.push(*pid);
                added = true;
            }
        }
        if !added {
            return tree;
        }
    }
}

/// 把游戏窗口的标题改成实例里配的那句。游戏要几秒才建出窗口，建好后进主菜单
/// 还可能把标题再写一遍，所以盯着改 45 秒。
#[cfg(windows)]
fn set_game_window_title(pid: u32, title: String) {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible,
        SetWindowTextW,
    };

    struct Target {
        pids: Vec<u32>,
        title: Vec<u16>,
    }

    unsafe extern "system" fn find(hwnd: HWND, lparam: isize) -> i32 {
        // SAFETY: lparam 就是上层传进来的 `&Target`，枚举期间一直活着。
        let target = unsafe { &*(lparam as *const Target) };
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        if !target.pids.contains(&owner) {
            return 1;
        }
        if unsafe { IsWindowVisible(hwnd) } == 0 || unsafe { GetWindowTextLengthW(hwnd) } == 0 {
            return 1;
        }
        unsafe { SetWindowTextW(hwnd, target.title.as_ptr()) };
        0
    }

    let mut wide: Vec<u16> = title.encode_utf16().collect();
    wide.push(0);
    for _ in 0..45 {
        let target = Target {
            pids: process_tree(pid),
            title: wide.clone(),
        };
        unsafe { EnumWindows(Some(find), &target as *const Target as isize) };
        std::thread::sleep(Duration::from_millis(1000));
    }
}

#[cfg(not(windows))]
fn set_game_window_title(_pid: u32, _title: String) {}

/// 窗口大小设置转成 `--width` / `--height`；`default` 返回 None，不动游戏自己的设置。
fn window_size_args(size: &str) -> Option<(String, String)> {
    let (width, height) = size.split_once('x')?;
    let width: u32 = width.parse().ok()?;
    let height: u32 = height.parse().ok()?;
    if width < 320 || height < 240 {
        return None;
    }
    Some((width.to_string(), height.to_string()))
}

/// 游戏进程起来之后改优先级。失败就算了，不影响玩。
#[cfg(windows)]
fn apply_process_priority(pid: u32, priority: &str) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS, HIGH_PRIORITY_CLASS,
        IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_SET_INFORMATION,
        REALTIME_PRIORITY_CLASS,
    };

    let class = match priority {
        "realtime" => REALTIME_PRIORITY_CLASS,
        "high" => HIGH_PRIORITY_CLASS,
        "above" => 0x0000_8000, // ABOVE_NORMAL_PRIORITY_CLASS
        "below" => BELOW_NORMAL_PRIORITY_CLASS,
        "idle" => IDLE_PRIORITY_CLASS,
        _ => NORMAL_PRIORITY_CLASS,
    };
    // SAFETY: pid 来自我们自己刚 spawn 的子进程，句柄用完立刻关。
    unsafe {
        let handle = OpenProcess(PROCESS_SET_INFORMATION, 0, pid);
        if handle.is_null() {
            return;
        }
        SetPriorityClass(handle, class);
        CloseHandle(handle);
    }
}

#[cfg(not(windows))]
fn apply_process_priority(_pid: u32, _priority: &str) {}

/// 进程还在不在：用来给「等游戏窗口」的循环收尾，进程没了就别再等了。
#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};
    const WAIT_TIMEOUT: u32 = 0x0000_0102;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let wait = unsafe { WaitForSingleObject(handle, 0) };
    unsafe { CloseHandle(handle) };
    wait == WAIT_TIMEOUT
}

#[cfg(not(windows))]
fn process_alive(_pid: u32) -> bool {
    false
}

/// 游戏窗口是不是已经出来了。Minecraft 的窗口类在 1.12 及更早是 `LWJGL`，
/// 1.13 以后是 `GLFW30`；控制台窗口（CREATE_NO_WINDOW 下本来也没有）和其它
/// 辅助窗口都不算。
#[cfg(windows)]
fn game_window_appeared(pids: &[u32]) -> bool {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowTextLengthW, GetWindowThreadProcessId,
        IsWindowVisible,
    };

    struct Probe {
        pids: Vec<u32>,
        found: bool,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: isize) -> i32 {
        // SAFETY: lparam 是上层传进来的 `&mut Probe`，枚举期间一直活着。
        let probe = unsafe { &mut *(lparam as *mut Probe) };
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        if !probe.pids.contains(&owner) {
            return 1;
        }
        if unsafe { IsWindowVisible(hwnd) } == 0 || unsafe { GetWindowTextLengthW(hwnd) } == 0 {
            return 1;
        }
        let mut class = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
        if len <= 0 {
            return 1;
        }
        let class = String::from_utf16_lossy(&class[..len as usize]);
        if class.starts_with("LWJGL") || class.starts_with("GLFW") {
            probe.found = true;
            return 0;
        }
        1
    }

    let mut probe = Probe {
        pids: pids.to_vec(),
        found: false,
    };
    unsafe { EnumWindows(Some(visit), &mut probe as *mut Probe as isize) };
    probe.found
}

#[cfg(not(windows))]
fn game_window_appeared(_pids: &[u32]) -> bool {
    false
}

/// 盯着游戏进程，等它真的把窗口画出来，然后广播 `game-ready`。
/// 前端拿这个事件把「启动中」的按钮解锁——窗口出现之前按钮一直锁着。
const READY_TIMEOUT: Duration = Duration::from_secs(900);

fn watch_game_window(app: tauri::AppHandle, pid: u32, run_id: u64, version_id: String) {
    let deadline = std::time::Instant::now() + READY_TIMEOUT;
    while std::time::Instant::now() < deadline {
        if game_window_appeared(&process_tree(pid)) {
            let _ = app.emit(
                "game-ready",
                serde_json::json!({ "runId": run_id, "versionId": version_id }),
            );
            return;
        }
        if !process_alive(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

/// 物理内存（MB）：自动配置内存和内存条都要用。
#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryInfo {
    pub total_mb: u64,
    pub available_mb: u64,
}

#[cfg(windows)]
pub fn memory_info_now() -> MemoryInfo {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return MemoryInfo {
            total_mb: 8192,
            available_mb: 4096,
        };
    }
    MemoryInfo {
        total_mb: status.ullTotalPhys / (1024 * 1024),
        available_mb: status.ullAvailPhys / (1024 * 1024),
    }
}

#[cfg(not(windows))]
pub fn memory_info_now() -> MemoryInfo {
    MemoryInfo {
        total_mb: 8192,
        available_mb: 4096,
    }
}

#[tauri::command]
pub fn memory_info() -> MemoryInfo {
    memory_info_now()
}

/// 把「一行参数」拆成参数表：按空白拆，成对的引号里的内容整段保留。
fn split_args(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in text.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// 跑实例的「启动前执行命令」；`wait` 为真则等它跑完，退出码非零算失败。
fn run_pre_launch(command: &str, dir: &str, wait: bool) -> Result<(), String> {
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/C", command])
        .current_dir(dir)
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    if !wait {
        return cmd
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("启动前命令跑不起来：{e}"));
    }
    let status = cmd
        .status()
        .map_err(|e| format!("启动前命令跑不起来：{e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "启动前命令失败（退出码 {}）",
            status.code().unwrap_or(-1)
        ))
    }
}

/// 照 PCL 的「自动配置内存」：按模组数量分档，再按可用内存逐段往上加。
fn auto_memory_mb(game_dir: &Path, id: &str, isolated: bool, info: MemoryInfo) -> u32 {
    let mods_dir = if isolated {
        game_dir.join("versions").join(id).join("mods")
    } else {
        game_dir.join("mods")
    };
    let mods = std::fs::read_dir(&mods_dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
                })
                .count() as f64
        })
        .unwrap_or(0.0);
    let available = info.available_mb as f64 / 1024.0;
    let optifine = id.to_ascii_lowercase().contains("optifine");
    let (ram_min, t1, t2, t3) = if mods == 0.0 {
        if optifine {
            (0.5, 1.5, 3.0, 5.0)
        } else {
            (0.5, 1.5, 2.5, 4.0)
        }
    } else {
        (
            0.5 + mods / 150.0,
            1.5 + mods / 90.0,
            2.7 + mods / 50.0,
            4.5 + mods / 25.0,
        )
    };
    let mut ram = available.min(t1);
    ram += (available * 0.7).min(t2 - t1);
    ram += (available * 0.4).min(t3 - t2);
    ram += (available * 0.15).min(t3);
    let ram = ram.max(ram_min).min(info.total_mb as f64 / 1024.0 * 0.9);
    ((ram * 1024.0).round() as u32).clamp(512, crate::settings::MAX_MEMORY_MB)
}

/// 按实例设置拼启动请求（计划、导出脚本、测试游戏、真正启动都用它）。
fn launch_request(
    paths: &crate::portable::Paths,
    game_dir: &Path,
    id: &str,
    java_override: Option<&str>,
    ignore_override: Option<bool>,
    with_account: bool,
    quick_play: Option<String>,
) -> Result<launcher_core::LaunchRequest, String> {
    let version = crate::versions::load_instance(game_dir, id);
    let global = crate::settings::load(paths);
    // 账户页里选中的那个人；没选账户时退回设置里的玩家名（老行为）。
    let mut store = launcher_core::auth::Store::load(paths.data_dir());
    let mut account = if with_account {
        store.current().cloned()
    } else {
        None
    };
    if let Some(player) = account.as_mut() {
        if player.kind == launcher_core::auth::AccountKind::Thirdparty {
            let url = player.server.clone().unwrap_or_default();
            let server = store
                .auth_servers
                .iter()
                .find(|server| server.url == url)
                .cloned()
                .ok_or_else(|| {
                    format!("账户「{}」用的认证服务器已经删了，重新加一个再登录。", player.name)
                })?;
            launcher_core::auth::session(player, &server)
                .map_err(|error| format!("「{}」的登录过期了：{error}", player.name))?;
            if let Some(saved) = store.players.iter_mut().find(|saved| saved.id == player.id) {
                saved.access = player.access.clone();
                saved.refresh = player.refresh.clone();
                saved.access_expires = player.access_expires;
                let _ = store.save(paths.data_dir());
            }
        }
        if player.kind == launcher_core::auth::AccountKind::Microsoft {
            launcher_core::auth::ensure_fresh(player)
                .map_err(|error| format!("微软账户「{}」的登录状态不能用了：{error}", player.name))?;
            // 续期换来的新令牌要写回账户表，否则下次还得再续一遍。
            if let Some(saved) = store.players.iter_mut().find(|saved| saved.id == player.id) {
                saved.access = player.access.clone();
                saved.access_expires = player.access_expires;
                saved.refresh = player.refresh.clone();
                saved.avatar = player.avatar.clone();
                let _ = store.save(paths.data_dir());
            }
        }
    }
    let microsoft = account
        .as_ref()
        .is_some_and(|player| player.kind == launcher_core::auth::AccountKind::Microsoft);
    let third_party = account
        .as_ref()
        .is_some_and(|player| player.kind == launcher_core::auth::AccountKind::Thirdparty);
    // 第三方账户要把 authlib-injector 塞进 JVM 参数，游戏端才知道去哪验证。
    let authlib = if third_party {
        let player = account.as_ref().expect("third_party 只会在有账户时为真");
        let url = player.server.clone().unwrap_or_default();
        let server = store
            .auth_servers
            .iter()
            .find(|server| server.url == url)
            .cloned()
            .ok_or_else(|| "账户用的认证服务器已经被删了。".to_string())?;
        let jar = launcher_core::authlib::ensure_jar(paths.data_dir())
            .map_err(|error| format!("下载 authlib-injector 失败：{error}"))?;
        Some(launcher_core::AuthlibInject {
            jar: jar.to_string_lossy().into_owned(),
            server: server.url.clone(),
            metadata: server.metadata.clone(),
        })
    } else {
        None
    };
    // 离线账户带皮肤时，启动时要起本地 Yggdrasil 并注入 authlib-injector。
    let offline = account
        .as_ref()
        .is_some_and(|player| player.kind == launcher_core::auth::AccountKind::Offline);
    let offline_skin = account
        .as_ref()
        .filter(|player| player.kind == launcher_core::auth::AccountKind::Offline)
        .and_then(|player| player.skin.clone());
    let offline_skin_model = account
        .as_ref()
        .filter(|player| player.kind == launcher_core::auth::AccountKind::Offline)
        .and_then(|player| player.skin_model.clone());
    let offline_cape = account
        .as_ref()
        .filter(|player| player.kind == launcher_core::auth::AccountKind::Offline)
        .and_then(|player| player.cape.clone());
    let authlib_jar = if offline && offline_skin.is_some() {
        launcher_core::authlib::ensure_jar(paths.data_dir())
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
    } else {
        None
    };
    // 没有选中账户就不启动：登录态（username/uuid/token）全靠当前账户。
    if with_account && account.is_none() {
        return Err("还没有选中账户：先去「账户」页选一个再启动。".into());
    }
    let username = account
        .as_ref()
        .map(|player| player.name.clone())
        .unwrap_or_else(|| global.username.clone());
    let memory = match version.memory_type {
        0 => auto_memory_mb(game_dir, id, version.isolated, memory_info_now()),
        1 => version.memory_custom_mb.max(crate::versions::MIN_MEMORY_MB),
        _ => global.memory_mb,
    };
    let java_path = if let Some(path) = java_override.filter(|path| !path.trim().is_empty()) {
        path.to_owned()
    } else if !version.java_path.is_empty() {
        version.java_path.clone()
    } else if version.java_auto {
        pick_java(id, None, game_dir)?
    } else if !global.java_path.is_empty() {
        global.java_path.clone()
    } else {
        pick_java(id, None, game_dir)?
    };
    // 设置里存的路径可能混着 `/`（历史数据），跟探测出来的 `\` 路径对不上，
    // 后面按路径找 Java 大版本时就会找不到、跳过区间校验。
    let mut java_path = java_path.replace('/', "\\");
    match version.login_require {
        1 if !microsoft => {
            return Err(
                "这个实例限了「仅正版」：先去「账户」页登录一个微软账户并选中它，再启动。"
                    .into(),
            )
        }
        3 if !third_party => {
            return Err(
                "这个实例限了「仅第三方登录」：先添加一个第三方认证服务器并登录那里的账号，选中它再启动。"
                    .into(),
            )
        }
        _ => {}
    }
    let ignore_java = ignore_override.unwrap_or(version.ignore_java_warning);
    if !ignore_java {
        let major = launcher_core::detect_java()
            .into_iter()
            .find(|java| java.path.replace('/', "\\").eq_ignore_ascii_case(&java_path))
            .map(|java| java.major);
        if let Some(major) = major {
            let requirement = java_requirement(game_dir, id);
            if !requirement.accepts(major) {
                // PCL 的做法是「挑一个满足区间的」而不是拦住不给玩：
                // 这里换掉 java_path，并把换用记进启动器日志，界面照常启动。
                match pick_java_for(requirement.clone()) {
                    Ok(selected) => {
                        crate::versions::append_log(
                            paths.data_dir(),
                            &format!(
                                "[java] 实例 {id} 要 {}，原先的 Java {major} 不满足，已自动改用 {selected}",
                                requirement.describe()
                            ),
                        );
                        java_path = selected;
                    }
                    Err(_) => {
                        return Err(format!(
                            "这个实例要 {}，现在选的是 Java {major}，也没找到别的能用的 Java。换一套 Java，或者在「高级启动选项」里勾上「忽略 Java 兼容性警告」。",
                            requirement.describe()
                        ))
                    }
                }
            }
        }
    }
    // 自动档每次都重算，选完把「要什么 / 选了谁」写进启动器日志，出问题能直接看出来。
    let requirement = java_requirement(game_dir, id);
    crate::versions::append_log(
        paths.data_dir(),
        &format!(
            "[java] 实例 {id} 需要 {}，使用 {java_path}",
            requirement.describe()
        ),
    );
    let mut game_args = split_args(&version.game_args);
    // 「自动进入服务器」：1.20 起原版把 --server/--port 换成了 --quickPlayMultiplayer，
    // 旧参数会被直接忽略，所以要按基础版本挑一种写法。
    let server = version.server_entry.trim();
    if !server.is_empty() {
        let (host, port) = match server.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => (host, port),
            _ => (server, "25565"),
        };
        if !host.is_empty() {
            let meta = crate::version_meta::describe_all(game_dir, &[id.to_owned()]);
            let meta = meta.get(id);
            let modern = match meta {
                Some(meta) if meta.channel == "snapshot" => true,
                Some(meta) => wants_quick_play(meta.game_version.as_deref()),
                None => false,
            };
            if modern {
                game_args.push("--quickPlayMultiplayer".into());
                game_args.push(format!("{host}:{port}"));
            } else {
                game_args.push("--server".into());
                game_args.push(host.to_owned());
                game_args.push("--port".into());
                game_args.push(port.to_owned());
            }
        }
    }
    Ok(launcher_core::LaunchRequest {
        game_dir: game_dir.to_string_lossy().into_owned(),
        game_directory: crate::versions::game_directory(id, version.isolated),
        version_id: id.to_owned(),
        java_path,
        username,
        max_memory_mb: memory,
        initial_memory_mb: if version.memory_type == 1 {
            version.memory_initial_mb
        } else {
            0
        },
        access_token: account
            .as_ref()
            .filter(|player| {
                matches!(
                    player.kind,
                    launcher_core::auth::AccountKind::Microsoft
                        | launcher_core::auth::AccountKind::Thirdparty
                )
            })
            .and_then(|player| player.access.clone()),
        uuid: account.as_ref().map(|player| player.uuid.clone()),
        version_type: Some(version.version_type.clone()).filter(|text| !text.trim().is_empty()),
        jvm_args: split_args(&version.jvm_args),
        game_args,
        classpath_head: split_args(&version.classpath_head),
        skip_file_check: version.skip_file_check,
        allow_java_upgrade: true,
        log4j_debug: version.debug_log4j,
        authlib,
        skin: offline_skin,
        skin_model: offline_skin_model,
        cape: offline_cape,
        authlib_jar,
        quick_play,
    })
}

/// 1.20 起原版的 `--server/--port` 被 `--quickPlayMultiplayer` 取代了（旧参数会被忽略）。
fn wants_quick_play(game_version: Option<&str>) -> bool {
    let Some(version) = game_version else {
        return false;
    };
    let mut parts = version.split('.');
    let (Some(major), Some(minor)) = (parts.next(), parts.next()) else {
        return false;
    };
    let (Ok(major), Ok(minor)) = (major.parse::<u32>(), minor.parse::<u32>()) else {
        return false;
    };
    (major, minor) >= (1, 20)
}

/// 按实例设置拼一份启动计划（导出脚本、测试游戏都用它）。
fn plan_for(
    paths: &crate::portable::Paths,
    id: &str,
) -> Result<(launcher_core::LaunchPlan, String), String> {
    let game_dir = crate::game_folders::current(paths);
    // 导出的 .bat 不写访问令牌：脚本会留在磁盘上，令牌只该留在启动器里。
    let request = launch_request(paths, &game_dir, id, None, None, false, None)?;
    let plan = launcher_core::build_launch_plan(request).map_err(|e| e.to_string())?;
    Ok((plan, game_dir.to_string_lossy().into_owned()))
}

fn bat_quote(arg: &str) -> String {
    if arg.is_empty() || arg.contains(' ') || arg.contains('\t') {
        format!("\"{arg}\"")
    } else {
        arg.to_owned()
    }
}

/// 导出启动脚本：把当前实例的启动命令写成 .bat，双击就能起。
#[tauri::command]
pub fn export_launch_script(paths: State<'_, Paths>, id: String) -> Result<String, String> {
    let (plan, _) = plan_for(&paths, &id)?;
    let target = crate::game_folders::current(&paths)
        .join("downloads")
        .join(format!("启动 {id}.bat"));
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建不了下载目录：{e}"))?;
    }
    let mut text = String::from("@echo off\r\n");
    text.push_str(&format!(
        "cd /d \"{}\"\r\n",
        plan.working_directory.replace('"', "")
    ));
    text.push_str(&bat_quote(&plan.java_path));
    for arg in &plan.args {
        text.push(' ');
        text.push_str(&bat_quote(arg));
    }
    text.push_str("\r\npause\r\n");
    std::fs::write(&target, text).map_err(|e| format!("写不了启动脚本：{e}"))?;
    Ok(target.to_string_lossy().into_owned())
}

/// Cleanroom 装完还缺的三个可选模组（Fugue / Scalar / Scalar Legacy）以及各自的版本。
#[tauri::command]
pub async fn cleanroom_extras(game: String) -> Result<Vec<launcher_core::cleanroom::ExtraMod>, String> {
    tauri::async_runtime::spawn_blocking(move || launcher_core::cleanroom::extra_mods(&game))
        .await
        .map_err(|error| error.to_string())
}

/// 测试游戏：照启动计划跑一次，盯着看它会不会立刻崩。
#[tauri::command]
pub async fn test_instance(paths: State<'_, Paths>, id: String) -> Result<String, String> {
    let (plan, _) = plan_for(&paths, &id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut command = std::process::Command::new(&plan.java_path);
        command
            .args(&plan.args)
            .current_dir(&plan.working_directory)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().map_err(|e| format!("起不来 Java：{e}"))?;
        let pid = child.id();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut detail = String::new();
                    if let Some(mut stderr) = child.stderr.take() {
                        use std::io::Read;
                        let mut buffer = String::new();
                        let _ = stderr.read_to_string(&mut buffer);
                        detail = buffer
                            .lines()
                            .rev()
                            .take(3)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect::<Vec<_>>()
                            .join(" / ");
                    }
                    return Err(if detail.is_empty() {
                        format!("启动后就退出了，退出码 {status}")
                    } else {
                        format!("启动后就退出了（{status}）：{detail}")
                    });
                }
                Ok(None) => {}
                Err(error) => return Err(format!("读不到进程状态：{error}")),
            }
            if std::time::Instant::now() > deadline {
                return Ok(format!("起来了（PID {pid}），20 秒没崩，先当作能玩"));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 改实例描述（概览页「修改实例描述」）。
#[tauri::command]
pub fn set_version_description(
    paths: State<'_, Paths>,
    id: String,
    description: String,
) -> Result<crate::versions::VersionSettings, String> {
    let game_dir = crate::game_folders::current(&paths);
    let mut settings = crate::versions::load_instance(&game_dir, &id);
    settings.description = description;
    crate::versions::save_instance(&game_dir, &id, settings.clone())?;
    Ok(settings)
}

/// 改实例名：把 `versions/<旧名>` 整个改成 `<新名>`，分类归属一起搬。
#[tauri::command]
pub fn rename_instance(
    paths: State<'_, Paths>,
    from: String,
    to: String,
) -> Result<String, String> {
    let game_dir = crate::game_folders::current(&paths);
    let new_name = to.trim().to_owned();
    if new_name.is_empty() {
        return Err("实例名不能为空".into());
    }
    crate::versions::rename_instance(&game_dir, paths.data_dir(), &from, &new_name)?;
    Ok(new_name)
}

/// 重置实例：启动设置和图标恢复默认，mods / 存档 / 整合包信息不动。
#[tauri::command]
pub fn reset_instance(
    paths: State<'_, Paths>,
    id: String,
) -> Result<crate::versions::VersionSettings, String> {
    crate::versions::reset_instance(&crate::game_folders::current(&paths), &id)
}

/// 补全文件：把缺的客户端、库、资源重新校验并下回来。
///
/// 实例 json 是装好后合并过的自包含文件（客户端、加载器库、assetIndex 都在里面），
/// 所以实例本身单独补一遍；加载器库就写在实例的 libraries 里，不补这一遍的话
/// Forge/Fabric 的库丢了会一直启动失败。基础版本再补一遍，管两边共用的那些。
#[tauri::command]
pub async fn repair_instance_files(
    app: AppHandle,
    paths: State<'_, Paths>,
    id: String,
) -> Result<launcher_core::download::InstallSummary, String> {
    let game_dir = crate::game_folders::current(&paths);
    let meta = crate::version_meta::describe_all(&game_dir, std::slice::from_ref(&id))
        .remove(&id)
        .unwrap_or_default();
    let Some(base_version) = meta.game_version else {
        return Err("认不出这个实例的基础游戏版本，补全文件做不到".into());
    };
    let strategy = sync_global_strategy(&paths);
    let dir = game_dir.to_string_lossy().into_owned();
    tauri::async_runtime::spawn_blocking(move || {
        // 实例 json 有的字段决定能不能单独补：缺 assetIndex 的老增量 profile 只能补库。
        let metadata = std::fs::read_to_string(
            std::path::Path::new(&dir)
                .join("versions")
                .join(&id)
                .join(format!("{id}.json")),
        )
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        let has_client = metadata
            .as_ref()
            .is_some_and(|value| value.pointer("/downloads/client").is_some());
        let has_index = metadata
            .as_ref()
            .is_some_and(|value| value.pointer("/assetIndex/id").is_some());
        let mut targets: Vec<(&str, bool)> = Vec::new();
        if has_client && has_index {
            targets.push((id.as_str(), false));
        } else if metadata.is_some() {
            targets.push((id.as_str(), true));
        }
        if base_version != id {
            targets.push((base_version.as_str(), false));
        }
        let mut total: Option<launcher_core::download::InstallSummary> = None;
        for (target, libraries_only) in targets {
            let emitter = app.clone();
            let emit = move |progress: InstallProgress| {
                let _ = emitter.emit("install-progress", &progress);
            };
            let summary = if libraries_only {
                launcher_core::download::install_profile_libraries(
                    &dir,
                    target,
                    strategy,
                    None,
                    CancellationToken::default(),
                    emit,
                )
            } else {
                launcher_core::download::install_profile_assets(
                    &dir,
                    target,
                    strategy,
                    None,
                    CancellationToken::default(),
                    emit,
                )
            }
            .map_err(|e| e.to_string())?;
            total = Some(match total {
                None => summary,
                Some(mut summed) => {
                    summed.downloaded_files += summary.downloaded_files;
                    summed.skipped_files += summary.skipped_files;
                    summed.downloaded_bytes += summary.downloaded_bytes;
                    summed
                }
            });
        }
        total.ok_or_else(|| "这个实例没有可补的文件".to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 诊断一份崩溃报告：把原文里认得出来的原因翻成人话，认不出来就返回空。
#[tauri::command]
pub async fn diagnose_crash_report(
    dir: String,
    file: String,
) -> Result<Vec<launcher_core::crash::Diagnosis>, String> {
    // 只收文件名：面板传进来的就是列表里那一项，别让它顺手读到外面去。
    if file.is_empty() || file.contains(['/', '\\']) || file == "." || file == ".." {
        return Err("文件名不合法".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let path = std::path::Path::new(&dir).join(&file);
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let text = decode_text(&bytes);
        Ok(launcher_core::crash::diagnose(&text))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 修补核心：让用户挑一个 jar/zip，把里面的文件合并进实例的客户端 jar。
///
/// 对应 PCL CE 的 `GameCore.AddToCore`：用来临时换掉核心里的类（打补丁、换 mod 环境）。
#[tauri::command]
pub async fn patch_instance_core(
    app: AppHandle,
    paths: State<'_, Paths>,
    id: String,
) -> Result<usize, String> {
    use tauri_plugin_dialog::DialogExt;
    let game_dir = crate::game_folders::current(&paths);
    let Some(picked) = app
        .dialog()
        .file()
        .set_title("选择用于修补核心的文件")
        .add_filter("压缩文件", &["jar", "zip"])
        .blocking_pick_file()
    else {
        return Ok(0);
    };
    let source = picked
        .into_path()
        .map_err(|_| "选中的不是本地文件".to_string())?;
    let target = version_dir(&game_dir, &id)?.join(format!("{id}.jar"));
    if !target.is_file() {
        return Err(format!("找不到客户端核心 {}，先补全文件试试", target.display()));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let written = launcher_core::merge_core_jar(&target, &source).map_err(|e| e.to_string())?;
        Ok(written)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 重置实例前先把核心文件另存一份（照 PCL CE 存到 versions\install-backups\<实例>\）。
#[tauri::command]
pub async fn backup_instance_core(paths: State<'_, Paths>, id: String) -> Result<String, String> {
    let game_dir = crate::game_folders::current(&paths);
    tauri::async_runtime::spawn_blocking(move || {
        let dir = version_dir(&game_dir, &id)?;
        let backup = game_dir.join("versions").join("install-backups").join(&id);
        std::fs::create_dir_all(&backup).map_err(|e| format!("建备份目录失败：{e}"))?;
        for extension in ["json", "jar"] {
            let from = dir.join(format!("{id}.{extension}"));
            if from.is_file() {
                std::fs::copy(&from, backup.join(format!("{id}.{extension}")))
                    .map_err(|e| format!("备份 {} 失败：{e}", from.display()))?;
            }
        }
        Ok(backup.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 写入整合包信息（装整合包时由前端带上）；传 `None` 清掉。
#[tauri::command]
pub fn set_version_pack(
    paths: State<'_, Paths>,
    id: String,
    pack: Option<crate::versions::PackInfo>,
) -> Result<crate::versions::VersionSettings, String> {
    crate::versions::set_pack(&crate::game_folders::current(&paths), &id, pack)
}

// ---------------------------------------------------------------- 版本分类

#[tauri::command]
pub fn list_categories(paths: State<'_, Paths>) -> crate::categories::CategorySnapshot {
    crate::categories::snapshot(paths.data_dir(), &crate::game_folders::current_string(&paths))
}

#[tauri::command]
pub fn create_category(
    paths: State<'_, Paths>,
    name: String,
) -> Result<crate::categories::CategorySnapshot, String> {
    crate::categories::create(
        paths.data_dir(),
        &crate::game_folders::current_string(&paths),
        &name,
    )
}

#[tauri::command]
pub fn rename_category(
    paths: State<'_, Paths>,
    id: String,
    name: String,
) -> Result<crate::categories::CategorySnapshot, String> {
    crate::categories::rename(
        paths.data_dir(),
        &crate::game_folders::current_string(&paths),
        &id,
        &name,
    )
}

#[tauri::command]
pub fn delete_category(
    paths: State<'_, Paths>,
    id: String,
) -> Result<crate::categories::CategorySnapshot, String> {
    crate::categories::delete(
        paths.data_dir(),
        &crate::game_folders::current_string(&paths),
        &id,
    )
}

#[tauri::command]
pub fn set_version_categories(
    paths: State<'_, Paths>,
    version: String,
    categories: Vec<String>,
) -> Result<crate::categories::CategorySnapshot, String> {
    crate::categories::set_version(
        paths.data_dir(),
        &crate::game_folders::current_string(&paths),
        &version,
        categories,
    )
}

#[tauri::command]
pub fn set_starred(
    paths: State<'_, Paths>,
    version: String,
    starred: bool,
) -> Result<crate::categories::CategorySnapshot, String> {
    crate::categories::set_starred(
        paths.data_dir(),
        &crate::game_folders::current_string(&paths),
        &version,
        starred,
    )
}

#[tauri::command]
pub fn import_starred(
    paths: State<'_, Paths>,
    versions: Vec<String>,
) -> Result<crate::categories::CategorySnapshot, String> {
    crate::categories::import_starred(
        paths.data_dir(),
        &crate::game_folders::current_string(&paths),
        versions,
    )
}

#[tauri::command]
pub async fn detect_java(force: Option<bool>) -> Result<Vec<JavaInstallation>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::detect_java_cached(force.unwrap_or(false))
    })
        .await
        .map_err(|e| e.to_string())
}

/// 把失败原因追加到 `logs/launcher.log`，发布版没有控制台可看。
fn append_error(log_dir: &std::path::Path, message: &str) {
    use std::io::Write;
    let path = log_dir.join("launcher.log");
    // 超过 256KB 就滚动一次，避免日志无限增长把磁盘写满。
    if std::fs::metadata(&path)
        .map(|meta| meta.len() > 256 * 1024)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, log_dir.join("launcher.log.1"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "[{}] {message}", crate::portable::local_hhmm());
    }
}

// -------------------------------------------------------------- 日志查看

/// 读文件末尾若干行；超过上限时只保留尾部，避免把整个文件读进内存。
fn tail_lines(path: &std::path::Path, limit: usize) -> Result<String, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let lines: Vec<&str> = raw.lines().collect();
    let start = lines.len().saturating_sub(limit);
    Ok(lines[start..].join(
        "
",
    ))
}

#[tauri::command]
pub fn read_launcher_log(
    paths: tauri::State<'_, crate::portable::Paths>,
    limit: Option<usize>,
) -> Result<String, String> {
    let path = paths.log_dir().join("launcher.log");
    if !path.exists() {
        return Ok(String::new());
    }
    tail_lines(&path, limit.unwrap_or(2000))
}

/// 游戏自己的日志：`<game_dir>/logs/latest.log`（Minecraft 每次启动都会重写）。
#[tauri::command]
pub fn read_game_log(game_dir: String, limit: Option<usize>) -> Result<String, String> {
    let path = std::path::Path::new(&game_dir)
        .join("logs")
        .join("latest.log");
    if !path.exists() {
        return Ok(String::new());
    }
    tail_lines(&path, limit.unwrap_or(2000))
}

/// 导出错误报告：错误标题、说明、原始错误串，加上最近的启动器日志（必要时再加游戏日志），
/// 存成一份 txt，用户可以直接发给维护者。
#[tauri::command]
pub fn export_error_report(
    app: tauri::AppHandle,
    paths: tauri::State<'_, crate::portable::Paths>,
    title: String,
    message: String,
    detail: String,
    source: Option<String>,
    game_dir: Option<String>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let stamp = crate::portable::local_stamp();
    let compact: String = stamp
        .chars()
        .filter(char::is_ascii_digit)
        .take(12)
        .collect();
    let name = format!("ybumcl-error-{compact}.txt");
    let Some(target) = app
        .dialog()
        .file()
        .set_file_name(&name)
        .add_filter("文本文件", &["txt"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = target.into_path().map_err(|error| error.to_string())?;

    let mut report = String::new();
    report.push_str("YBUMCL 错误报告
");
    report.push_str(&format!("时间：{stamp}
"));
    report.push_str(&format!("启动器版本：{}
", env!("CARGO_PKG_VERSION")));
    report.push_str(&format!(
        "系统：{} {}
",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    report.push_str(&format!("
标题：{title}
"));
    if !message.trim().is_empty() {
        report.push_str(&format!("说明：{message}
"));
    }
    if !detail.trim().is_empty() {
        report.push_str(&format!("
详细信息：
{detail}
"));
    }

    let launcher_log = paths.log_dir().join("launcher.log");
    report.push_str("
—— 启动器日志（末尾 200 行）——
");
    report.push_str(
        &tail_lines(&launcher_log, 200)
            .unwrap_or_else(|error| format!("（读不到 {}：{error}）", launcher_log.display())),
    );

    if source.as_deref() == Some("game") {
        let game_log = game_dir
            .as_deref()
            .filter(|dir| !dir.trim().is_empty())
            .map(|dir| std::path::Path::new(dir).join("logs").join("latest.log"));
        report.push_str("
—— 游戏日志（末尾 200 行）——
");
        match game_log {
            Some(path) => report.push_str(
                &tail_lines(&path, 200)
                    .unwrap_or_else(|error| format!("（读不到 {}：{error}）", path.display())),
            ),
            None => report.push_str("（没有找到游戏目录）
"),
        }
    }

    std::fs::write(&path, report).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(Some(path.display().to_string()))
}

#[tauri::command]
pub fn launcher_log_path(paths: tauri::State<'_, crate::portable::Paths>) -> String {
    paths.log_dir().join("launcher.log").display().to_string()
}

/// 启动前确认游戏目录真的能写：Minecraft 一启动就要建 `logs/`、`crash-reports/`，
/// 建不了会抛一大坨 Java 堆栈，用户根本看不懂。这里提前探一次，失败就给中文提示。
fn ensure_game_dir_writable(dir: &std::path::Path) -> Result<(), String> {
    let mut failed = Vec::new();
    for name in ["logs", "crash-reports"] {
        let path = dir.join(name);
        if let Err(e) = std::fs::create_dir_all(&path) {
            failed.push(format!("{}（{e}）", path.display()));
        }
    }
    let probe = dir.join(".ybumcl-write-probe");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
        }
        Err(e) => failed.push(format!("{}（{e}）", probe.display())),
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "游戏目录不可写：{}
请检查该文件夹的权限，或把它移到普通目录（不要放在受保护的同步盘/沙箱目录里）。",
            failed.join("；")
        ))
    }
}

// ---------------------------------------------------------------- 启动游戏

/// 装加载器前先知道要装的是哪一类，好算它绑定的 Java 上限。
fn request_loaders(request: &ModLoaderInstallRequest) -> LoaderPresence<'_> {
    match request.loader {
        ModLoader::Forge | ModLoader::NeoForge => LoaderPresence {
            forge: Some(&request.loader_version),
            ..Default::default()
        },
        ModLoader::Cleanroom => LoaderPresence {
            cleanroom: Some(&request.loader_version),
            ..Default::default()
        },
        ModLoader::LiteLoader => LoaderPresence {
            liteloader: true,
            ..Default::default()
        },
        ModLoader::OptiFine => LoaderPresence {
            optifine: true,
            ..Default::default()
        },
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaPlan {
    /// 「Java 21+」这类给用户看的文字。
    pub description: String,
    /// 已装 Java 里能用的第一个（路径），没有则 null。
    pub path: Option<String>,
    pub major: Option<u32>,
    /// 自动挑的 Java 现成路径。
    pub message: Option<String>,
}

/// 前端要显示的「这个版本该用哪套 Java」。装原版和装加载器都用得到。
#[tauri::command]
pub fn plan_java(
    paths: State<'_, crate::portable::Paths>,
    game_version: String,
    loader: Option<String>,
    loader_version: Option<String>,
    version_id: Option<String>,
) -> JavaPlan {
    let requirement = match &version_id {
        Some(id) => java_requirement(&crate::game_folders::current(&paths), id),
        None => {
            let version = loader_version.as_deref().unwrap_or("");
            let presence = match loader.as_deref() {
                Some("forge") => LoaderPresence {
                    forge: Some(version),
                    ..Default::default()
                },
                Some("neoforge") => LoaderPresence::default(),
                Some("optifine") => LoaderPresence {
                    optifine: true,
                    ..Default::default()
                },
                Some("liteloader") => LoaderPresence {
                    liteloader: true,
                    ..Default::default()
                },
                Some("cleanroom") => LoaderPresence {
                    cleanroom: Some(version),
                    ..Default::default()
                },
                _ => LoaderPresence::default(),
            };
            launcher_core::java::requirement_with(&game_version, None, &presence)
        }
    };
    let found = launcher_core::detect_java();
    let mut matching: Vec<&JavaInstallation> = found
        .iter()
        .filter(|java| requirement.accepts(java.major))
        .collect();
    matching.sort_by_key(|java| java.major);
    let chosen = matching.first().copied();
    let majors: Vec<u32> = found.iter().map(|java| java.major).collect();
    JavaPlan {
        description: requirement.describe(),
        path: chosen.map(|java| java.path.clone()),
        major: chosen.map(|java| java.major),
        message: if chosen.is_none() {
            Some(requirement.missing_message(&majors))
        } else {
            None
        },
    }
}

/// 这个版本要什么 Java：按原版基线、json 声明、加载器收窄叠出区间（见 `launcher_core::java`）。
fn java_requirement(
    game_dir: &std::path::Path,
    version_id: &str,
) -> launcher_core::java::JavaRequirement {
    launcher_core::java::requirement_for_dir(game_dir, version_id).unwrap_or_else(|| {
        // 认不出这份 json 时退一步：拿实例认出来的基础版本算区间。
        // 实例名常常不是版本号（`Reverie Foundry`），直接把它当版本号会得到
        // 「任意 Java 都行」，于是全局默认的 Java 17 被放行、启动时才被
        // 核心以 `requires Java 21 or newer` 拦住。
        let base = crate::version_meta::describe_all(game_dir, &[version_id.to_owned()])
            .get(version_id)
            .and_then(|meta| meta.game_version.clone());
        launcher_core::java::requirement_with(
            base.as_deref().unwrap_or(version_id),
            None,
            &LoaderPresence::default(),
        )
    })
}

/// 启动前的 Java 兼容性检查。以前后端直接悄悄换一套，用户看不到「换过」；
/// 现在把结论交给界面，由用户决定换还是照旧启动。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaCheck {
    /// 「Java 21+」这种给人看的区间。
    pub required: String,
    pub accepted: bool,
    /// 这个实例本来就勾了「忽略兼容性警告」。
    pub ignored: bool,
    pub current_path: Option<String>,
    pub current_major: Option<u32>,
    pub suggested_path: Option<String>,
    pub suggested_major: Option<u32>,
}

#[tauri::command]
pub async fn check_java(
    paths: State<'_, crate::portable::Paths>,
    id: String,
) -> Result<JavaCheck, String> {
    let paths = paths.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let game_dir = crate::game_folders::current(&paths);
        let version = crate::versions::load_instance(&game_dir, &id);
        let global = crate::settings::load(&paths);
        let current = if !version.java_path.is_empty() {
            Some(version.java_path.replace('/', "\\"))
        } else if version.java_auto || global.java_path.is_empty() {
            pick_java(&id, None, &game_dir).ok()
        } else {
            Some(global.java_path.replace('/', "\\"))
        };
        let requirement = java_requirement(&game_dir, &id);
        let found = launcher_core::detect_java();
        let major_of = |path: &str| -> Option<u32> {
            found
                .iter()
                .find(|java| java.path.eq_ignore_ascii_case(path))
                .map(|java| java.major)
        };
        let current_major = current.as_deref().and_then(major_of);
        let accepted = version.ignore_java_warning
            || current_major
                .map(|major| requirement.accepts(major))
                .unwrap_or(false);
        let suggested = pick_java_for(requirement.clone()).ok();
        let suggested_major = suggested.as_deref().and_then(major_of);
        Ok(JavaCheck {
            required: requirement.describe(),
            accepted,
            ignored: version.ignore_java_warning,
            current_path: current,
            current_major,
            suggested_path: suggested,
            suggested_major,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 没配 Java 时自动挑一个：区间内取最小的大版本，避免拿 Java 25 跑只支持 17 的版本。
fn pick_java(
    version_id: &str,
    preferred: Option<&str>,
    game_dir: &std::path::Path,
) -> Result<String, String> {
    if let Some(path) = preferred {
        if !path.trim().is_empty() {
            return Ok(path.to_string());
        }
    }
    pick_java_for(java_requirement(game_dir, version_id))
}

fn pick_java_for(requirement: launcher_core::java::JavaRequirement) -> Result<String, String> {
    let found = launcher_core::detect_java();
    if found.is_empty() {
        return Err("没有检测到可用的 Java，请先在设置里指定，或安装：https://adoptium.net/temurin/releases/".into());
    }
    let mut matching: Vec<_> = found
        .iter()
        .filter(|java| requirement.accepts(java.major))
        .collect();
    // 先按「离下限最近」挑（1.20.1 的 17 优于 21，26.3 只认 25），
    // 同档优先 64 位、再取大版本。PCL 的权重表（8/17/21 偏好）只作最后的
    // 兜底：把它挪到排序末尾会让「1.20.1 挑到 21」这种能用但易崩的组合复现。
    matching.sort_by_key(|java| {
        (
            // 先满足软上限（建议区间），再贴近下限，最后才轮到 64 位与权重。
            requirement.soft_penalty(java.major),
            requirement.slack(java.major).unwrap_or(u32::MAX),
            !java.is_64,
            std::cmp::Reverse(launcher_core::java_weight(java.major)),
            std::cmp::Reverse(java.major),
        )
    });
    match matching.first() {
        Some(java) => Ok(java.path.clone()),
        None => {
            let majors: Vec<u32> = found.iter().map(|java| java.major).collect();
            Err(requirement.missing_message(&majors))
        }
    }
}

/// 别的启动器建的实例常常是「只写了 json」：库和 native 要等启动时才补。
/// YBUMCL 的启动计划是纯离线校验，所以发现缺文件时先按 profile 补齐，再重试一次。
/// 设置里的下载源策略文本 → 枚举（无法识别时回落到「两者竞速」）。
fn strategy_of(text: Option<&str>) -> launcher_core::download::DownloadStrategy {
    text.and_then(launcher_core::download::DownloadStrategy::parse)
        .unwrap_or_default()
}

/// 读取全局设置里的下载源策略与全局限速，并把进程级状态同步过去。
pub fn sync_global_strategy(paths: &crate::portable::Paths) -> launcher_core::download::DownloadStrategy {
    let global = crate::settings::load(paths);
    let strategy = strategy_of(global.download_strategy.as_deref());
    launcher_core::download::set_strategy(strategy);
    launcher_core::download::set_http2(global.use_http2);
    launcher_core::download::set_speed_limit(launcher_core::download::tier_limit(
        global.download_speed,
    ));
    launcher_core::proxy::configure(&global.proxy_mode, &global.proxy_url);
    crate::versions::set_isolation_mode(&global.isolation_mode);
    strategy
}

fn repair_before_launch(
    app: &AppHandle,
    game_dir: &str,
    version_id: &str,
    strategy: launcher_core::download::DownloadStrategy,
) -> Result<(), String> {
    // 增量 profile（Forge/Fabric/NeoForge 等）自己没有 assetIndex，资源和客户端
    // jar 都在它继承的基础版本里：先补基础版本，再按实例 json 补库。
    let base_version = crate::version_meta::describe_all(
        std::path::Path::new(game_dir),
        std::slice::from_ref(&version_id.to_owned()),
    )
    .remove(version_id)
    .and_then(|meta| meta.game_version);
    let Some(base_version) = base_version else {
        return Err("认不出这个实例的基础游戏版本，补齐缺失文件做不到".into());
    };
    if base_version == version_id {
        let emitter = app.clone();
        return launcher_core::download::install_profile_assets(
            game_dir,
            version_id,
            strategy,
            None,
            CancellationToken::default(),
            move |progress: InstallProgress| {
                let _ = emitter.emit("install-progress", &progress);
            },
        )
        .map(|_| ())
        .map_err(|error| format!("补齐缺失文件失败：{error}"));
    }
    let emitter = app.clone();
    launcher_core::download::install_profile_assets(
        game_dir,
        &base_version,
        strategy,
        None,
        CancellationToken::default(),
        move |progress: InstallProgress| {
            let _ = emitter.emit("install-progress", &progress);
        },
    )
    .map_err(|error| format!("补齐基础版本失败：{error}"))?;
    let emitter = app.clone();
    launcher_core::download::install_profile_libraries(
        game_dir,
        version_id,
        strategy,
        None,
        CancellationToken::default(),
        move |progress: InstallProgress| {
            let _ = emitter.emit("install-progress", &progress);
        },
    )
    .map(|_| ())
    .map_err(|error| format!("补齐缺失文件失败：{error}"))
}


/// 从游戏输出里认出「这次是 Java 选错了」，并给出该往哪个方向挪。
enum JavaFix {
    /// 需要更高主版本；`UnsupportedClassVersionError` 会带出 class file 版本。
    Up(Option<u32>),
    /// 需要更低主版本：模块/访问检查失败，常见于旧模组 + 新 JDK。
    Down,
    /// 同主版本换 build（JVM 原生崩溃）。
    SameBuild,
}

impl JavaFix {
    fn describe(&self) -> &'static str {
        match self {
            Self::Up(_) => "Java 版本过低",
            Self::Down => "Java 版本过高",
            Self::SameBuild => "JVM 自身崩溃",
        }
    }
}

/// 只认三类特征；其余失败（缺库、主类找不到、超时）不是换 Java 能解决的。
fn java_fix(logs: &[LogLine]) -> Option<JavaFix> {
    for line in logs.iter().rev().take(600) {
        let text = line.text.as_str();
        if text.contains("UnsupportedClassVersionError")
            || text.contains("compiled by a more recent version of the Java Runtime")
        {
            // `class file version 69.0` 对应 Java 25（class file 版本 = 主版本 + 44）。
            let needed = text
                .split("class file version ")
                .nth(1)
                .and_then(|rest| {
                    rest.split(|c: char| !c.is_ascii_digit())
                        .find(|part| !part.is_empty())
                })
                .and_then(|digits| digits.parse::<u32>().ok())
                .and_then(|file| file.checked_sub(44));
            return Some(JavaFix::Up(needed));
        }
        if text.contains("Unsupported JVM module path")
            || text.contains("module java.base does not")
            || text.contains("IllegalAccessError")
        {
            return Some(JavaFix::Down);
        }
        if text.contains("EXCEPTION_ACCESS_VIOLATION") || text.contains("hs_err_pid") {
            return Some(JavaFix::SameBuild);
        }
    }
    None
}

/// 换 Java 需要的上下文：只在「自动档 + 没有手动指定」时才自动重试。
struct RetryContext {
    paths: crate::portable::Paths,
    game_dir: std::path::PathBuf,
    version_id: String,
    java_path: String,
    auto_mode: bool,
    started: std::time::Instant,
}

fn java_retry_candidate(
    requirement: &launcher_core::java::JavaRequirement,
    used_major: u32,
    fix: &JavaFix,
) -> Option<launcher_core::JavaInstallation> {
    let mut candidates: Vec<launcher_core::JavaInstallation> = launcher_core::detect_java()
        .into_iter()
        .filter(|java| requirement.accepts(java.major) && java.major != used_major)
        .collect();
    candidates.sort_by_key(|java| {
        (
            requirement.soft_penalty(java.major),
            requirement.slack(java.major).unwrap_or(u32::MAX),
            !java.is_64,
            std::cmp::Reverse(java.major),
        )
    });
    match fix {
        JavaFix::Up(needed) => {
            let floor = needed.unwrap_or(used_major + 1);
            candidates.into_iter().find(|java| java.major >= floor)
        }
        JavaFix::Down => candidates.into_iter().find(|java| java.major < used_major),
        JavaFix::SameBuild => candidates
            .into_iter()
            .find(|java| java.major == used_major),
    }
}

/// 内存优化：已经是管理员就直接清；否则拉一个带 `--memory-optimize` 的自身进程并等它结束。
fn optimize_memory_now() -> Result<(), String> {
    if launcher_core::memory::is_elevated() {
        return launcher_core::memory::optimize();
    }
    let exe = std::env::current_exe().map_err(|e| format!("找不到启动器自身路径：{e}"))?;
    launcher_core::memory::optimize_elevated(&exe)
}

/// 启动一个版本。配置来自 `version-settings.json`，缺省字段用全局设置补。
#[tauri::command]
pub async fn launch_version(
    id: String,
    java: Option<String>,
    ignore_java: Option<bool>,
    quick_play: Option<String>,
    app: AppHandle,
    runtime: State<'_, GameRuntime>,
    paths: State<'_, crate::portable::Paths>,
) -> Result<u32, String> {
    let runtime = runtime.inner().clone();
    let log_dir = paths.log_dir();
    let game_dir = crate::game_folders::current(&paths);
    // 拼启动请求要读账户、必要时联网续期令牌，放进阻塞线程里做，别卡界面。
    let paths_for_request = (*paths).clone();
    let retry_paths = (*paths).clone();
    let retry_id = id.clone();
    let retry_dir = game_dir.clone();
    let request_id = id.clone();
    let launch_dir = game_dir.clone();
    let pre_dir = launch_dir.clone();
    let launch_id = id.clone();
    let launched = tauri::async_runtime::spawn_blocking(move || {
        let request = launch_request(
            &paths_for_request,
            &game_dir,
            &request_id,
            java.as_deref(),
            ignore_java,
            true,
            quick_play,
        )?;
        let app_settings = crate::settings::load(&paths_for_request);
        let strategy = strategy_of(app_settings.download_strategy.as_deref());
        let version_id = request.version_id.clone();
        let repair_request = request.clone();
        let plan = match launcher_core::build_launch_plan(request) {
            Ok(plan) => plan,
            Err(launcher_core::CoreError::MissingFile(path)) => {
                // 别的启动器建的实例常常缺库/native（它们启动时自己补），
                // 这里先按 profile 补齐一次，再重新算计划。
                append_error(&log_dir, &format!("缺少文件，启动前自动补齐：{path}"));
                let game_dir = repair_request.game_dir.clone();
                match repair_before_launch(&app, &game_dir, &version_id, strategy) {
                    Ok(()) => match launcher_core::build_launch_plan(repair_request) {
                        Ok(plan) => plan,
                        Err(error) => {
                            append_error(&log_dir, &format!("补齐后仍无法启动：{error}"));
                            return Err(error.to_string());
                        }
                    },
                    Err(message) => {
                        append_error(&log_dir, &message);
                        return Err(message);
                    }
                }
            }
            Err(error) => {
                // 界面上只显示一行错误，日志里留全量，方便事后查。
                append_error(&log_dir, &format!("启动构建失败：{error}"));
                return Err(error.to_string());
            }
        };
        // Cleanroom 首次启动会在渲染线程同步下载 35 MB 中文字体（单连接很慢，窗口
        // 会「未响应」十几分钟）。启动前用多连接把它塞进 Cleanroom 的全局缓存。
        if launcher_core::cleanroom::looks_like_cleanroom(&plan.args) {
            match launcher_core::cleanroom::ensure_font_cached() {
                Ok(Some(message)) => append_error(&log_dir, &message),
                Ok(None) => {}
                Err(message) => {
                    append_error(&log_dir, &format!("预置 Cleanroom 字体失败（不影响启动）：{message}"))
                }
            }
        }
        if let Err(message) =
            ensure_game_dir_writable(std::path::Path::new(&plan.working_directory))
        {
            append_error(&log_dir, &message);
            return Err(message);
        }
        // 同一实例可以同时开多份（PCL 也不拦），这里只开一次新的启动编号。
        let run_id = {
            let mut sessions = runtime.0.lock().map_err(|e| e.to_string())?;
            sessions.begin(&version_id)
        };

        // 实例的「启动前执行命令」：在游戏目录里跑；失败就不启动。
        let settings = crate::versions::load_instance(&pre_dir, &version_id);
        // 内存优化（PCL 的「启动游戏前进行内存优化」）：开了才跑，而且要等它跑完
        // 才继续启动。没拿到管理员权限就跳过，只提示一次，不阻断启动。
        let optimize = match settings.memory_optimize {
            1 => true,
            2 => false,
            _ => crate::settings::load(&paths_for_request).memory_optimize,
        };
        if optimize {
            match optimize_memory_now() {
                Ok(()) => append_error(&log_dir, "内存优化完成"),
                Err(message) => {
                    append_error(&log_dir, &format!("跳过内存优化：{message}"));
                    let _ = app.emit("memory-skip", message);
                }
            }
        }
        // 自动档（没手动指定 Java）才允许失败后自动换一个重试。
        let auto_mode =
            java.is_none() && (settings.java_auto || settings.java_path.trim().is_empty());
        if !settings.pre_launch.trim().is_empty() {
            match run_pre_launch(
                &settings.pre_launch,
                &plan.working_directory,
                settings.pre_launch_wait,
            ) {
                Ok(()) => append_error(
                    &log_dir,
                    &format!("启动前命令已执行：{}", settings.pre_launch),
                ),
                Err(message) => {
                    append_error(&log_dir, &message);
                    return Err(message);
                }
            }
        }
        let mut launch_args = plan.args.clone();
        if let Some((width, height)) = window_size_args(&app_settings.window_size) {
            launch_args.push("--width".into());
            launch_args.push(width);
            launch_args.push("--height".into());
            launch_args.push(height);
        }
        let mut command = Command::new(&plan.java_path);
        command
            .args(&launch_args)
            .current_dir(&plan.working_directory)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW：不给 Java 弹一个黑框控制台
            command.creation_flags(0x0800_0000);
        }

        append_error(
            &log_dir,
            &format!(
                "启动 {} | java={} | 目录={}",
                version_id, plan.java_path, plan.working_directory
            ),
        );
        // 落盘：这次的运行日志 + 可双击复现的启动脚本。
        let command_line = std::iter::once(plan.java_path.clone())
            .chain(launch_args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        if let Err(error) = crate::runlog::write_latest_batch(
            &log_dir,
            &plan.working_directory,
            &plan.java_path,
            &launch_args,
        ) {
            append_error(&log_dir, &format!("写 LatestLaunch.bat 失败：{error}"));
        }
        let started_at = crate::runlog::now_readable();
        match crate::runlog::RunLog::open(
            &log_dir,
            run_id,
            &version_id,
            [
                format!("启动：{started_at}"),
                format!("Java：{}", plan.java_path),
                format!("目录：{}", plan.working_directory),
                format!("命令行：{command_line}"),
            ],
        ) {
            Ok(mut log) => {
                log.push("system", &format!("启动 {version_id}"));
                if let Ok(mut sessions) = runtime.0.lock() {
                    if let Some(run) = sessions.runs.get_mut(&run_id) {
                        run.log = Some(log);
                    }
                }
            }
            Err(error) => append_error(&log_dir, &format!("开运行日志失败：{error}")),
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let message = format!("无法启动 Java：{error}");
                append_error(&log_dir, &message);
                return Err(message);
            }
        };
        let pid = child.id();
        append_error(&log_dir, &format!("已启动 {}，pid {pid}", version_id));
        let title = if settings.window_title.trim().is_empty() {
            app_settings.window_title.clone()
        } else {
            settings.window_title.clone()
        };
        if !title.trim().is_empty() {
            std::thread::spawn(move || set_game_window_title(pid, title));
        }
        apply_process_priority(pid, &app_settings.process_priority);
        if app_settings.launcher_visibility == "minimize" {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.minimize();
            }
        }
        {
            let ready_app = app.clone();
            let ready_version = version_id.clone();
            std::thread::spawn(move || watch_game_window(ready_app, pid, run_id, ready_version));
        }
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        {
            let mut sessions = runtime.0.lock().map_err(|e| e.to_string())?;
            if let Some(run) = sessions.runs.get_mut(&run_id) {
                run.pid = Some(pid);
                run.child = Some(child);
            }
        }

        let reader_run = run_id;
        for (stream, pipe) in [
            (
                "stdout",
                stdout.map(|p| Box::new(p) as Box<dyn Read + Send>),
            ),
            (
                "stderr",
                stderr.map(|p| Box::new(p) as Box<dyn Read + Send>),
            ),
        ] {
            let Some(pipe) = pipe else { continue };
            let app = app.clone();
            let runtime = runtime.clone();
            let reader_run = reader_run;
            std::thread::spawn(move || {
                let mut reader = BufReader::new(pipe);
                loop {
                    let mut bytes = Vec::new();
                    match reader.by_ref().take(16_384).read_until(b'\n', &mut bytes) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => publish(
                            &app,
                            &runtime,
                            reader_run,
                            LogLine {
                                stream: stream.into(),
                                text: decode_text(&bytes).trim_end().to_string(),
                            },
                        ),
                    }
                }
            });
        }

        let retry_ctx = RetryContext {
            paths: retry_paths,
            game_dir: retry_dir,
            version_id: retry_id,
            java_path: plan.java_path.clone(),
            auto_mode,
            started: std::time::Instant::now(),
        };
        let app_exit = app.clone();
        let runtime_exit = runtime.clone();
        let exit_run = run_id;
        let exit_version = version_id.clone();
        let log_dir_exit = log_dir.clone();
        let started_at_exit = started_at.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(300));
                let Ok(mut sessions) = runtime_exit.0.lock() else {
                    break;
                };
                let Some(run) = sessions.runs.get_mut(&exit_run) else {
                    break;
                };
                let Some(child) = run.child.as_mut() else {
                    break;
                };
                match child.try_wait() {
                    Ok(Some(status)) => {
                        run.last_exit = status.code();
                        run.child = None;
                        // 收尾：把退出码写进运行日志，并登记到 index.json。
                        let pid = run.pid;
                        let stopped_by_user = run.stopped;
                        let log_file = run.log.as_mut().map(|log| log.finish(status.code()));
                        run.log = None;
                        if let Some(path) = log_file {
                            let entry = crate::runlog::RunEntry {
                                run_id: exit_run,
                                version_id: exit_version.clone(),
                                pid,
                                started: started_at_exit.clone(),
                                ended: Some(crate::runlog::now_readable()),
                                exit_code: status.code(),
                                file: path
                                    .file_name()
                                    .map(|name| name.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                                crashed: status.code().unwrap_or(0) != 0 && !stopped_by_user,
                            };
                            if let Err(error) = crate::runlog::record(&log_dir_exit, entry) {
                                append_error(&log_dir_exit, &format!("写运行日志索引失败：{error}"));
                            }
                        }
                        let logs_snapshot: Vec<LogLine> = run.logs.iter().cloned().collect();
                        drop(sessions);
                        publish(
                            &app_exit,
                            &runtime_exit,
                            exit_run,
                            LogLine {
                                stream: "system".into(),
                                text: format!(
                                    "游戏已退出，退出码：{}",
                                    status
                                        .code()
                                        .map(|v| v.to_string())
                                        .unwrap_or_else(|| "未知".into())
                                ),
                            },
                        );
                        // 非正常退出：用规则表认一下原因，推给界面做红条提示。
                        // 用户自己点「结束实例」的不算崩溃。
                        if status.code().unwrap_or(0) != 0 && !stopped_by_user {
                            let mut context: Vec<String> = logs_snapshot
                                .iter()
                                .map(|line| format!("[{}] {}", line.stream, line.text))
                                .collect();
                            context.extend(crash_tail(&retry_ctx.game_dir.to_string_lossy(), &retry_ctx.version_id));
                            let hits = launcher_core::crashlog::analyze(&context);
                            if hits.is_empty() {
                                append_error(
                                    &retry_ctx.paths.log_dir(),
                                    &format!(
                                        "[crash] {} 退出码 {}，未识别出已知原因（内置 {} 条规则）",
                                        retry_ctx.version_id,
                                        status.code().unwrap_or(-1),
                                        launcher_core::crashlog::rule_count()
                                    ),
                                );
                            } else {
                                append_error(
                                    &retry_ctx.paths.log_dir(),
                                    &format!(
                                        "[crash] {} 退出码 {}：{}",
                                        retry_ctx.version_id,
                                        status.code().unwrap_or(-1),
                                        hits[0].summary()
                                    ),
                                );
                            }
                            let _ = app_exit.emit(
                                "game-crash",
                                serde_json::json!({
                                    "runId": exit_run,
                                    "versionId": retry_ctx.version_id,
                                    "code": status.code(),
                                    "hits": hits,
                                }),
                            );
                        }
                        // Java 选错的典型失败：退出码非 0、启动后 3 分钟内退出。
                        // 只做「建议 + 前端重试一次」，不改这里的启动状态。
                        if status.code().unwrap_or(0) != 0
                            && retry_ctx.auto_mode
                            && retry_ctx.started.elapsed() < Duration::from_secs(180)
                        {
                            if let Some(fix) = java_fix(&logs_snapshot) {
                                let requirement = java_requirement(&retry_ctx.game_dir, &retry_ctx.version_id);
                                let used = launcher_core::detect_java()
                                    .into_iter()
                                    .find(|java| java.path.replace('/', "\\").eq_ignore_ascii_case(&retry_ctx.java_path.replace('/', "\\")))
                                    .map(|java| java.major)
                                    .unwrap_or(0);
                                match java_retry_candidate(&requirement, used, &fix) {
                                    Some(next) => {
                                        append_error(
                                            &retry_ctx.paths.log_dir(),
                                            &format!(
                                                "[java] {} 用 Java {used} 启动失败（{}），改用 {} 重试",
                                                retry_ctx.version_id,
                                                fix.describe(),
                                                next.path
                                            ),
                                        );
                                        let _ = app_exit.emit(
                                            "java-retry",
                                            serde_json::json!({
                                                "versionId": retry_ctx.version_id,
                                                "javaPath": next.path,
                                                "reason": fix.describe(),
                                            }),
                                        );
                                    }
                                    None => append_error(
                                        &retry_ctx.paths.log_dir(),
                                        &format!(
                                            "[java] {} 启动失败（{}），没有更合适的 Java 可换：{}",
                                            retry_ctx.version_id,
                                            fix.describe(),
                                            requirement.describe()
                                        ),
                                    ),
                                }
                            }
                        }
                        let _ = app_exit.emit(
                            "game-exit",
                            serde_json::json!({
                                "runId": exit_run,
                                "versionId": exit_version,
                                "code": status.code(),
                            }),
                        );
                        break;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        drop(sessions);
                        publish(
                            &app_exit,
                            &runtime_exit,
                            exit_run,
                            LogLine {
                                stream: "system".into(),
                                text: format!("读取进程状态失败：{error}"),
                            },
                        );
                        break;
                    }
                }
            }
        });

        Ok(pid)
    })
    .await
    .map_err(|e| e.to_string())?;

    if let Ok(pid) = launched {
        // 游戏进程起来了才算启动一次。
        crate::versions::bump_launches(&launch_dir, &launch_id);
        Ok(pid)
    } else {
        launched.map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub fn game_snapshot(
    runtime: State<'_, GameRuntime>,
    version_id: Option<String>,
    run_id: Option<u64>,
) -> Result<GameSnapshot, String> {
    let sessions = runtime.0.lock().map_err(|e| e.to_string())?;
    let run = match run_id {
        Some(id) => sessions.runs.get(&id),
        None => {
            let id = sessions
                .last
                .filter(|id| sessions.runs.contains_key(id))
                .or_else(|| {
                    version_id
                        .as_ref()
                        .and_then(|version| {
                            sessions
                                .runs
                                .values()
                                .filter(|run| &run.version_id == version)
                                .max_by_key(|run| run.id)
                        })
                        .map(|run| run.id)
                });
            id.and_then(|id| sessions.runs.get(&id))
        }
    };
    Ok(GameSnapshot {
        running: run.map(|run| run.child.is_some()).unwrap_or(false),
        pid: run.and_then(|run| run.pid),
        last_exit: run.and_then(|run| run.last_exit),
        logs: run
            .map(|run| run.logs.iter().cloned().collect())
            .unwrap_or_default(),
    })
}

/// 「运行中实例」浮层里的一行。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub run_id: u64,
    pub version_id: String,
    pub pid: u32,
    /// 同名实例里的第几份，从 1 开始。
    pub index: u32,
}

#[tauri::command]
pub fn list_runs(runtime: State<'_, GameRuntime>) -> Result<Vec<RunView>, String> {
    let sessions = runtime.0.lock().map_err(|e| e.to_string())?;
    let mut live: Vec<&Run> = sessions
        .runs
        .values()
        .filter(|run| run.child.is_some() && run.pid.is_some())
        .collect();
    live.sort_by_key(|run| run.id);
    let mut seen: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    Ok(live
        .into_iter()
        .map(|run| {
            let index = seen.entry(run.version_id.as_str()).or_insert(0);
            *index += 1;
            RunView {
                run_id: run.id,
                version_id: run.version_id.clone(),
                pid: run.pid.unwrap_or(0),
                index: *index,
            }
        })
        .collect())
}

/// 结束某一次启动。只杀这一个进程，别的启动不受影响。
/// 导出错误报告 zip。`target` 由前端另存为对话框给出。
#[tauri::command]
pub fn export_crash_report(
    paths: State<'_, crate::portable::Paths>,
    target: String,
    version_id: String,
    run_file: Option<String>,
) -> Result<String, String> {
    let game_dir = crate::game_folders::current_string(&paths);
    let path = crate::crashreport::write(
        std::path::Path::new(&target),
        &paths,
        &game_dir,
        &version_id,
        run_file.as_deref(),
    )?;
    Ok(path.to_string_lossy().into_owned())
}

/// 最近几次启动记录（含已结束的），新的在前。
#[tauri::command]
pub fn run_history(
    paths: State<'_, crate::portable::Paths>,
    limit: Option<usize>,
) -> Result<Vec<crate::runlog::RunEntry>, String> {
    Ok(crate::runlog::history(&paths.log_dir(), limit.unwrap_or(20)))
}

#[tauri::command]
pub fn stop_run(runtime: State<'_, GameRuntime>, run_id: u64) -> Result<(), String> {
    let mut sessions = runtime.0.lock().map_err(|e| e.to_string())?;
    let Some(run) = sessions.runs.get_mut(&run_id) else {
        return Err("这次启动已经结束了".into());
    };
    run.stopped = true;
    if let Some(child) = run.child.as_mut() {
        child.kill().map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------------------------------------------------------------- 下载安装

/// 正在跑的安装任务，按前端任务卡的 id 登记。
///
/// PCL 的任务之间完全并行，这里也按任务 id 各管各的 token：同一个 id 重复进来才报错，
/// 不同实例、不同任务可以同时装。取消按 id 找对应的 token。
#[derive(Clone, Default)]
pub struct InstallRuntime(pub Arc<Mutex<std::collections::HashMap<u64, CancellationToken>>>);

/// 匿名安装号：从 1<<63 起，跟前端任务卡的 id 不会撞。
fn next_anonymous_install() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1 << 63);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl InstallRuntime {
    fn begin(&self, install_id: u64) -> Result<CancellationToken, String> {
        let mut running = self.0.lock().map_err(|e| e.to_string())?;
        if running.contains_key(&install_id) {
            return Err("这个任务已经在装了".into());
        }
        let token = CancellationToken::default();
        running.insert(install_id, token.clone());
        Ok(token)
    }

    fn end(&self, install_id: u64) {
        if let Ok(mut running) = self.0.lock() {
            running.remove(&install_id);
        }
    }
}

fn parse_loader(raw: &str) -> Result<ModLoader, String> {
    match raw {
        "forge" => Ok(ModLoader::Forge),
        "neoforge" => Ok(ModLoader::NeoForge),
        "cleanroom" => Ok(ModLoader::Cleanroom),
        "liteloader" => Ok(ModLoader::LiteLoader),
        "optifine" => Ok(ModLoader::OptiFine),
        other => Err(format!("未知的加载器：{other}")),
    }
}

/// Mojang 官方版本清单（联网）。
#[tauri::command]
pub async fn fetch_remote_versions(strategy: Option<String>) -> Result<RemoteVersionManifest, String> {
    let strategy = strategy_of(strategy.as_deref());
    tauri::async_runtime::spawn_blocking(move || launcher_core::list_remote_versions(strategy))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Fabric Meta 上的 loader 版本（联网）。
#[tauri::command]
pub async fn fetch_fabric_loaders(
    game_version: String,
    strategy: Option<String>,
) -> Result<Vec<FabricLoaderVersion>, String> {
    let strategy = strategy_of(strategy.as_deref());
    tauri::async_runtime::spawn_blocking(move || {
        list_fabric_loaders(&game_version, strategy).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// LabyMod 官方清单（联网）。`channel` 是 `production` 或 `snapshot`。
#[tauri::command]
pub async fn fetch_labymod_versions(
    channel: String,
) -> Result<Vec<launcher_core::download::LabymodVersion>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::download::labymod_versions(&channel).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Legacy Fabric Meta 上的 loader 版本（联网）。
#[tauri::command]
pub async fn fetch_legacy_fabric_loaders(
    game_version: String,
) -> Result<Vec<FabricLoaderVersion>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::download::list_legacy_fabric_loaders(&game_version)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Quilt Meta 上的 loader 版本（联网）。
#[tauri::command]
pub async fn fetch_quilt_loaders(game_version: String) -> Result<Vec<FabricLoaderVersion>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        list_quilt_loaders(&game_version).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Forge / NeoForge 的可用版本（联网，读 maven-metadata）。
#[tauri::command]
pub async fn fetch_modloader_versions(
    loader: String,
    game_version: String,
) -> Result<Vec<ModLoaderVersion>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let loader = parse_loader(&loader)?;
        list_modloader_versions(loader, &game_version).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Forge / NeoForge / Cleanroom / LiteLoader / OptiFine 的可用版本（联网，多源）。
/// 一个构建的发行日期（`YYYY-MM-DD`），拿不到就不出现在结果里。
#[derive(serde::Serialize)]
pub struct LoaderDate {
    version: String,
    date: String,
}

#[tauri::command]
pub async fn fetch_loader_dates(
    loader: String,
    game_version: String,
    versions: Vec<String>,
) -> Result<Vec<LoaderDate>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::dates::dates(&loader, &game_version, &versions)
            .into_iter()
            .map(|(version, date)| LoaderDate { version, date })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn fetch_loader_catalog(
    loader: String,
    game_version: String,
) -> Result<Vec<LoaderEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let filter = loader_catalog::LoaderFilter::parse(&loader)
            .ok_or_else(|| format!("未知的加载器：{loader}"))?;
        loader_catalog::fetch(&game_version, filter, &CancellationToken::default())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 安装包：把某个加载器（或原版客户端）的文件单独下载到用户选的目录。
/// 读收藏夹（favorites.json，跟数据目录一起搬）。
#[tauri::command]
pub async fn favorites_load(
    paths: tauri::State<'_, crate::portable::Paths>,
) -> Result<crate::favorites::Favorites, String> {
    Ok(crate::favorites::load(paths.data_dir()))
}

/// 写收藏夹。
#[tauri::command]
pub async fn favorites_save(
    paths: tauri::State<'_, crate::portable::Paths>,
    favorites: crate::favorites::Favorites,
) -> Result<(), String> {
    crate::favorites::save(paths.data_dir(), &favorites).map_err(|error| error.to_string())
}

/// Modrinth 搜索：返回项目列表。
#[tauri::command]
pub async fn modrinth_search(
    kind: String,
    query: String,
    game_version: Option<String>,
    loader: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<launcher_core::modrinth::MrProject>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::modrinth::search(
            &kind,
            &query,
            game_version.as_deref(),
            loader.as_deref(),
            limit.unwrap_or(20),
            offset.unwrap_or(0),
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Modrinth 集市：按排序拿一批项目（下载量最高 / 最新 / 最近更新）。
#[tauri::command]
pub async fn modrinth_browse(
    kind: String,
    sort: String,
    game_version: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<launcher_core::modrinth::MrProject>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::modrinth::browse(&kind, &sort, game_version.as_deref(), limit.unwrap_or(8))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Modrinth 项目版本列表。
#[tauri::command]
pub async fn modrinth_versions(
    project: String,
    game_version: Option<String>,
    loader: Option<String>,
) -> Result<Vec<launcher_core::modrinth::MrVersion>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::modrinth::versions(&project, game_version.as_deref(), loader.as_deref())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// CurseForge 搜索：和 Modrinth 返回同一套结构，界面不用区分。
#[tauri::command]
pub async fn curseforge_search(
    kind: String,
    query: String,
    game_version: Option<String>,
    loader: Option<String>,
    sort: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<launcher_core::modrinth::MrProject>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::curseforge::search(
            &kind,
            &query,
            game_version.as_deref(),
            loader.as_deref(),
            sort.as_deref().unwrap_or("relevance"),
            limit.unwrap_or(20),
            offset.unwrap_or(0),
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// CurseForge 项目的文件列表。
#[tauri::command]
pub async fn curseforge_versions(
    project: String,
    game_version: Option<String>,
    loader: Option<String>,
) -> Result<Vec<launcher_core::modrinth::MrVersion>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::curseforge::versions(&project, game_version.as_deref(), loader.as_deref())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// FTB 整合包搜索（详情是一条条取的，所以并发抓取在 launcher-core 里做）。
#[tauri::command]
pub async fn ftb_search(
    term: String,
    sort: Option<String>,
    page: Option<u32>,
    limit: Option<u32>,
) -> Result<launcher_core::modrinth::SearchPage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::ftb::search(
            &term,
            sort.as_deref().unwrap_or("relevance"),
            page.unwrap_or(0),
            limit.unwrap_or(40),
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// FTB 整合包的版本列表。
#[tauri::command]
pub async fn ftb_versions(
    project: String,
    game_version: Option<String>,
    loader: Option<String>,
) -> Result<Vec<launcher_core::modrinth::MrVersion>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::ftb::versions(&project, game_version.as_deref(), loader.as_deref())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 装一个 FTB 整合包：先把版本清单写成临时 mrpack，再走通用的整合包安装。
#[tauri::command]
pub async fn install_ftb_modpack(
    app: AppHandle,
    runtime: State<'_, InstallRuntime>,
    install_id: Option<u64>,
    game_dir: String,
    project: String,
    version: String,
    name: Option<String>,
    java_path: Option<String>,
    strategy: Option<String>,
    icon_url: Option<String>,
) -> Result<launcher_core::modpack::ModpackSummary, String> {
    let strategy = strategy_of(strategy.as_deref());
    // 前端任务卡的 id 用不上时（版本页那种一次性安装）发一个匿名号，照样互不干扰。
    let install_id = install_id.unwrap_or_else(next_anonymous_install);
    let token = runtime.begin(install_id)?;
    let runtime_inner = runtime.inner().clone();
    let staging = PathBuf::from(&game_dir).join(".ybmcl").join("modpack");
    let pack_dir = game_dir.clone();
    let app_announce = app.clone();
    crate::sweep::clear_paused(&staging);
    let probe = token.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        let plan = std::path::PathBuf::from(&game_dir)
            .join(".ybmcl")
            .join("modpack")
            .join(format!(
                "ftb-{}-{}.mrpack",
                project.replace(':', "-"),
                version
            ));
        launcher_core::ftb::write_plan_zip(&project, &version, &plan)
            .map_err(|error| error.to_string())?;
        let emitter = app.clone();
        let outcome = launcher_core::modpack::install_modpack(
            launcher_core::modpack::ModpackRequest {
                game_dir: game_dir.clone(),
                url: None,
                path: Some(plan.display().to_string()),
                file_name: None,
                size: None,
                name,
                java_path,
                strategy,
                threads: None,
                cache_roots: Vec::new(),
                icon_url,
                // FTB 的整合包由我们自己写 .mrpack，来源是确定的，直接带进实例记录。
                source: Some("ftb".into()),
                project_id: Some(project.clone()),
                project_name: None,
            },
            token,
            move |step: &launcher_core::modpack::Step| {
                let _ = emitter.emit(
                    "modpack-progress",
                    serde_json::json!({
                        "installId": install_id,
                        "stage": step.stage,
                        "done": step.done,
                        "total": step.total,
                        "bytes": step.bytes,
                        "totalBytes": step.total_bytes,
                        "file": step.file,
                        "overall": step.overall,
                        "speed": step.speed,
                    }),
                );
            },
        );
        let _ = std::fs::remove_file(&plan);
        outcome.map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?;

    runtime_inner.end(install_id);
    match &result {
        Ok(_) => crate::sweep::clear_paused(&staging),
        Err(_) if probe.is_paused() => crate::sweep::mark_paused(&staging),
        Err(_) => crate::sweep::clear_paused(&staging),
    }
    if let Ok(summary) = &result {
        remember_output(summary.path.clone());
        write_pack(&pack_dir, summary);
        announce_versions(&app_announce, &pack_dir);
    }
    result
}

/// 一页内容搜索结果（两个来源共用同一结构）。
#[tauri::command]
pub async fn content_search(
    source: String,
    kind: String,
    query: String,
    game_version: Option<String>,
    loader: Option<String>,
    sort: Option<String>,
    category: Option<String>,
    env: Option<String>,
    offset: Option<u32>,
    limit: Option<u32>,
    translate: Option<bool>,
) -> Result<launcher_core::modrinth::SearchPage, String> {
    let sort = sort.unwrap_or_else(|| "relevance".into());
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(40);
    let started = std::time::Instant::now();
    let tag = format!("{source}/{kind} q={query} offset={offset}");
    let page = tauri::async_runtime::spawn_blocking(move || {
        // 中文关键词：先在本地的「中文名 → 英文名」表里换一手，换不出来就当没结果。
        let mut query = query;
        if translate.unwrap_or(false) && launcher_core::zhsearch::has_chinese(&query) {
            let kind = if source == "curseforge" {
                launcher_core::zhsearch::Source::CurseForge
            } else {
                launcher_core::zhsearch::Source::Modrinth
            };
            match launcher_core::zhsearch::plan(&query, kind) {
                Some(plan) => query = plan.keyword,
                None => {
                    return Ok(launcher_core::modrinth::SearchPage {
                        items: Vec::new(),
                        total: 0,
                    })
                }
            }
        }
        if source == "curseforge" {
            launcher_core::curseforge::search_page(
                &kind,
                &query,
                game_version.as_deref(),
                loader.as_deref(),
                &sort,
                category.as_deref(),
                limit,
                offset,
            )
        } else {
            launcher_core::modrinth::search_page(
                &kind,
                &query,
                game_version.as_deref(),
                loader.as_deref(),
                &sort,
                category.as_deref(),
                env.as_deref(),
                limit,
                offset,
            )
        }
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    // 列表快慢全看这两个上游，把每次请求的耗时写进 launcher.log，方便对账。
    if let Ok(paths) = crate::portable::Paths::resolve() {
        append_error(
            &paths.log_dir(),
            &format!(
                "[搜索] {tag} 用时 {}ms {} 条",
                started.elapsed().as_millis(),
                page.items.len()
            ),
        );
    }
    Ok(page)
}

/// 某个内容分类下、某个来源的全部分类标签。
#[tauri::command]
pub async fn content_categories(
    source: String,
    kind: String,
) -> Result<Vec<launcher_core::modrinth::ContentCategory>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if source == "curseforge" {
            launcher_core::curseforge::categories(&kind)
        } else {
            launcher_core::modrinth::categories(&kind)
        }
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

/// 取一张图标并转成 data URL；纯白/纯透明的图返回 None（界面用首字母占位）。
///
/// CurseForge 的图片服务器不带 CORS 头，前端没法直接读像素，所以判断放在后端做。
/// 图标缓存：同一个地址只下载一次。
///
/// 列表和详情面板是同一个组件的两个实例，各取一次；以前两次都走网络，
/// 哪次慢过前端 6 秒就永远显示成「没有图标」（左边有、右边没有就是这么来的）。
static ICON_CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Option<String>>>> =
    std::sync::OnceLock::new();

fn icon_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, Option<String>>> {
    ICON_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[tauri::command]
pub async fn fetch_icon(paths: State<'_, Paths>, url: String) -> Result<Option<String>, String> {
    if !url.starts_with("https://") {
        return Err("Invalid input: icon url must be https".into());
    }
    if let Some(hit) = icon_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&url).cloned())
    {
        return Ok(hit);
    }
    // 磁盘缓存 + 联网都放到阻塞线程：解码图片和写文件都是 CPU/IO 活。
    let dir = paths.data_dir().join("icon-cache");
    let key = url.clone();
    let icon = tauri::async_runtime::spawn_blocking(move || crate::icon_cache::load_or_fetch(&dir, &key))
        .await
        .map_err(|error| error.to_string())??;
    if let Ok(mut cache) = icon_cache().lock() {
        // 图标都很小，缓存 2048 个足够翻很久；超了直接清空重来。
        if cache.len() > 2048 {
            cache.clear();
        }
        cache.insert(url, icon.clone());
    }
    Ok(icon)
}

/// 把 Modrinth 的文件下到指定目录。
#[tauri::command]
pub async fn modrinth_download(
    url: String,
    dir: String,
    file_name: String,
    slug: Option<String>,
    title: Option<String>,
    format: Option<u8>,
) -> Result<String, String> {
    let name = {
        let zh = slug
            .as_deref()
            .and_then(launcher_core::names::chinese_name)
            .map(str::to_owned);
        let project = zh.or(title);
        launcher_core::names::content_file_name(project.as_deref(), &file_name, format.unwrap_or(4))
    };
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::modrinth::download(&url, std::path::Path::new(&dir), &name)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 弹系统「另存为」，把安装包写到用户选的路径。取消返回 None。
/// 用户没写扩展名时补上默认的（对齐 PCL 的 `AddExtension = true`）。
fn ensure_extension(path: String, default_name: &str) -> String {
    let target = std::path::Path::new(&path);
    if target.extension().is_some() {
        return path;
    }
    match std::path::Path::new(default_name)
        .extension()
        .and_then(|part| part.to_str())
    {
        Some(extension) => format!("{path}.{extension}"),
        None => path,
    }
}

#[tauri::command]
pub async fn save_package(
    app: tauri::AppHandle,
    paths: State<'_, Paths>,
    kind: String,
    game_version: String,
    version: String,
    strategy: Option<String>,
    default_dir: Option<String>,
    task_id: Option<u32>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    // 原版客户端不再下单个 jar，而是打一份「手动安装包」：jar + json + 库文件。
    // 先把「另存为」弹出来，再打包，用户取消就不白下几十兆。
    let vanilla = kind == "minecraft";
    let strategy_value = strategy_of(strategy.as_deref());
    let file = if vanilla {
        None
    } else {
        let kind = kind.clone();
        let game_version = game_version.clone();
        let version = version.clone();
        let files = tauri::async_runtime::spawn_blocking(move || {
            launcher_core::packages::package_files(
                &kind,
                &game_version,
                &version,
                strategy_value,
                &launcher_core::download::CancellationToken::default(),
            )
            .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())??;
        Some(
            files
                .into_iter()
                .next()
                .ok_or_else(|| "没有可下载的文件".to_string())?,
        )
    };
    let file_name = match &file {
        Some(item) => item.name.clone(),
        None => format!("{game_version}.zip"),
    };
    let mut dialog = app.dialog().file().set_file_name(&file_name);
    if let Some(dir) = default_dir.filter(|dir| !dir.trim().is_empty()) {
        // 目录不存在时对话框会忽略起始位置、开在上次用过的文件夹，所以先建出来。
        let _ = std::fs::create_dir_all(&dir);
        dialog = dialog.set_directory(dir);
    }
    let dialog = if vanilla {
        dialog.add_filter("压缩包", &["zip"])
    } else {
        dialog.add_filter("Java 文件", &["jar", "zip"])
    };
    let Some(target) = dialog.blocking_save_file() else {
        return Ok(None);
    };
    let path = target
        .into_path()
        .map_err(|error| error.to_string())?
        .to_string_lossy()
        .into_owned();
    let path = ensure_extension(path, &file_name);
    if vanilla {
        let game = game_version.clone();
        let game_dir = crate::game_folders::current(&paths);
        let emitter = app.clone();
        let bytes = tauri::async_runtime::spawn_blocking(move || {
            let emit = |received: u64, total: u64| {
                if let Some(id) = task_id {
                    let _ = emitter.emit(
                        "download-progress",
                        serde_json::json!({ "id": id, "received": received, "total": total }),
                    );
                }
            };
            launcher_core::packages::vanilla_zip_progress(
                &game,
                &game_dir,
                strategy_value,
                &launcher_core::download::CancellationToken::default(),
                &emit,
            )
            .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())??;
        std::fs::write(&path, &bytes).map_err(|error| error.to_string())?;
        // 收尾补一条满进度：任务卡靠它落定，别停在上一次的百分比上。
        if let Some(id) = task_id {
            let _ = app.emit(
                "download-progress",
                serde_json::json!({ "id": id, "received": bytes.len(), "total": bytes.len() }),
            );
        }
        return Ok(Some(remember_output(path)));
    }
    let file = file.ok_or_else(|| "没有可下载的文件".to_string())?;
    let written = write_with_progress(app, file.url.clone(), path, task_id).await?;
    Ok(Some(remember_output(written)))
}

/// 弹系统「另存为」，把社区资源写到用户选的路径。取消返回 None。
#[tauri::command]
pub async fn save_modrinth(
    app: tauri::AppHandle,
    url: String,
    file_name: String,
    slug: Option<String>,
    title: Option<String>,
    format: Option<u8>,
    default_dir: Option<String>,
    task_id: Option<u32>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let name = {
        let zh = slug
            .as_deref()
            .and_then(launcher_core::names::chinese_name)
            .map(str::to_owned);
        let project = zh.or(title);
        launcher_core::names::content_file_name(project.as_deref(), &file_name, format.unwrap_or(4))
    };
    let mut dialog = app.dialog().file().set_file_name(&name);
    if let Some(dir) = default_dir.filter(|dir| !dir.trim().is_empty()) {
        // 目录不存在时对话框会忽略起始位置、开在上次用过的文件夹，所以先建出来。
        let _ = std::fs::create_dir_all(&dir);
        dialog = dialog.set_directory(dir);
    }
    let filter = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_else(|| "jar".into());
    let dialog = dialog.add_filter("文件", &[filter.as_str()]);
    let Some(target) = dialog.blocking_save_file() else {
        return Ok(None);
    };
    let path = target
        .into_path()
        .map_err(|error| error.to_string())?
        .to_string_lossy()
        .into_owned();
    let path = ensure_extension(path, &name);
    // Write beside the target first: a failed or aborted download must not eat an
    // already saved world that happens to live at the same path.
    let partial = format!("{path}.part");
    match write_with_progress(app, url, partial.clone(), task_id).await {
        Ok(_) => {
            std::fs::rename(&partial, &path).map_err(|error| error.to_string())?;
            Ok(Some(remember_output(path)))
        }
        Err(error) => {
            let _ = std::fs::remove_file(&partial);
            Err(error)
        }
    }
}

/// MC 百科条目地址：表里有这个 slug 才有，没有就返回 None，界面上不显示按钮。
///
/// PCL 也是这么做的（`BtnIntroWiki.Visibility`），链接形如
/// `https://www.mcmod.cn/class/<id>.html`。
#[tauri::command]
pub fn wiki_link(source: String, slug: String) -> Option<String> {
    launcher_core::wiki::wiki_url(&source, &slug)
}

/// 下载世界包：默认解压进 `<gameDir>\saves`，设置里可以改成只留压缩包。
///
/// 世界包的落地位置只能是 `saves`：CurseForge 那边它是整张存档，
/// 不是带 overrides 的整合包，装进 `versions` 反而会让版本列表多出一个空实例。
#[tauri::command]
pub async fn install_world(
    app: AppHandle,
    url: String,
    file_name: String,
    game_dir: String,
    extract: bool,
    // 解压到哪（不传就是 `<游戏目录>\saves`）；选了实例时前端传实例的 saves。
    saves_dir: Option<String>,
    task_id: Option<u32>,
) -> Result<String, String> {
    if game_dir.trim().is_empty() {
        return Err("还没有游戏目录，先在「版本」页装一个实例".into());
    }
    let root = PathBuf::from(&game_dir);
    let downloads = root.join("downloads");
    std::fs::create_dir_all(&downloads).map_err(|error| {
        format!("无法创建下载目录 {}：{error}", downloads.display())
    })?;
    let archive = downloads.join(world_file_name(&file_name, &url));
    write_with_progress(app, url, archive.display().to_string(), task_id).await?;

    if !extract {
        return Ok(remember_output(archive.display().to_string()));
    }

    let saves = match saves_dir.filter(|path| !path.trim().is_empty()) {
        Some(path) => PathBuf::from(path),
        None => root.join("saves"),
    };
    let source = archive.clone();
    let folder = tauri::async_runtime::spawn_blocking(move || {
        launcher_core::world::extract_world(&source, &saves)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    // 解压成功就把压缩包删掉，和 PCL 一样不在 saves 旁边留垃圾。
    let _ = std::fs::remove_file(&archive);
    Ok(remember_output(
        root.join("saves").join(folder).display().to_string(),
    ))
}

/// 世界包「只下载」：弹另存为，默认落在 `<gameDir>\downloads`，取消就什么都不留。
#[tauri::command]
pub async fn save_world(
    app: AppHandle,
    url: String,
    file_name: String,
    default_dir: Option<String>,
    task_id: Option<u32>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let name = world_file_name(&file_name, &url);
    let mut dialog = app.dialog().file().set_file_name(&name);
    if let Some(dir) = default_dir.filter(|dir| !dir.trim().is_empty()) {
        // 目录不存在时对话框会忽略起始位置、开在上次用过的文件夹，所以先建出来。
        let _ = std::fs::create_dir_all(&dir);
        dialog = dialog.set_directory(dir);
    }
    let dialog = dialog.add_filter("世界包", &["zip"]);
    let Some(target) = dialog.blocking_save_file() else {
        return Ok(None);
    };
    let path = target
        .into_path()
        .map_err(|error| error.to_string())?
        .to_string_lossy()
        .into_owned();
    let path = ensure_extension(path, &name);
    let written = write_with_progress(app, url, path, task_id).await?;
    Ok(Some(remember_output(written)))
}

/// 世界包的落盘文件名：站点给的名字优先，缺省从 URL 尾巴取，统一补 `.zip`。
fn world_file_name(file_name: &str, url: &str) -> String {
    let candidate = file_name.trim();
    let name = if candidate.is_empty() {
        url.split(['?', '#'])
            .next()
            .unwrap_or_default()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned()
    } else {
        candidate.to_owned()
    };
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_owned();
    if name.is_empty() {
        return "world.zip".into();
    }
    if name.to_ascii_lowercase().ends_with(".zip") {
        name
    } else {
        format!("{name}.zip")
    }
}

/// 把一个文件写到 `path`，边写边把进度推给界面（`download-progress` 事件）。
/// 正在下载的单个文件：任务号 → 取消令牌。
///
/// 以前这里每次都新建一个 `CancellationToken::default()`，任务面板上的
/// 「取消 / 暂停」对这种单文件下载完全没作用，进度会一直跑到结束。
static DOWNLOAD_TOKENS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<u32, launcher_core::download::CancellationToken>>,
> = std::sync::OnceLock::new();

fn download_token(task_id: u32) -> launcher_core::download::CancellationToken {
    let map =
        DOWNLOAD_TOKENS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = map.lock().expect("download token map poisoned");
    guard.entry(task_id).or_default().clone()
}

/// 任务面板的「暂停 / 取消」，针对单文件下载（模组、资源包、世界包）。
#[tauri::command]
pub fn cancel_download(task_id: u32, mode: Option<String>) -> bool {
    let token = download_token(task_id);
    if mode.as_deref() == Some("pause") {
        token.pause();
    } else {
        token.cancel();
    }
    true
}

fn forget_download(task_id: u32) {
    if let Some(map) = DOWNLOAD_TOKENS.get() {
        if let Ok(mut guard) = map.lock() {
            guard.remove(&task_id);
        }
    }
}

async fn write_with_progress(
    app: AppHandle,
    url: String,
    path: String,
    task_id: Option<u32>,
) -> Result<String, String> {
    let token = task_id.map(download_token).unwrap_or_default();
    let cleanup = task_id;
    let result = tauri::async_runtime::spawn_blocking(move || {
        let emitter = app.clone();
        launcher_core::packages::download_file_to_progress(
            &url,
            std::path::Path::new(&path),
            &token,
            |received, total| {
                if let Some(id) = task_id {
                    let _ = emitter.emit(
                        "download-progress",
                        serde_json::json!({ "id": id, "received": received, "total": total }),
                    );
                }
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?;
    if let Some(id) = cleanup {
        forget_download(id);
    }
    result
}

#[tauri::command]
pub async fn download_package(
    app: AppHandle,
    paths: State<'_, Paths>,
    kind: String,
    game_version: String,
    version: String,
    dir: String,
    strategy: Option<String>,
    task_id: Option<u32>,
) -> Result<Vec<String>, String> {
    let strategy = strategy_of(strategy.as_deref());
    let game_dir = crate::game_folders::current(&paths);
    let written = tauri::async_runtime::spawn_blocking(move || {
        let token = launcher_core::download::CancellationToken::default();
        let emit = |received: u64, total: u64| {
            if let Some(id) = task_id {
                let _ = app.emit(
                    "download-progress",
                    serde_json::json!({ "id": id, "received": received, "total": total }),
                );
            }
        };
        if kind == "minecraft" {
            let bytes =
                launcher_core::packages::vanilla_zip_progress(&game_version, &game_dir, strategy, &token, &emit)
                    .map_err(|error| error.to_string())?;
            let folder = std::path::Path::new(&dir);
            std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
            let target = folder.join(format!("{game_version}.zip"));
            std::fs::write(&target, &bytes).map_err(|error| error.to_string())?;
            emit(bytes.len() as u64, bytes.len() as u64);
            return Ok(vec![remember_output(target.to_string_lossy().into_owned())]);
        }
        let files = launcher_core::packages::package_files(
            &kind,
            &game_version,
            &version,
            strategy,
            &token,
        )
        .map_err(|error| error.to_string())?;
        launcher_core::packages::download_into(&files, std::path::Path::new(&dir), &token)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?;
    let written = written?;
    Ok(written.into_iter().map(remember_output).collect())
}

/// 其它游戏目录：这些目录里已有同 SHA1 的文件时直接复制，免得再下一遍。
///
/// 只把「用户登记过的目录」当来源（当前目录 + 已添加的文件夹），并且可以在设置里关掉。
fn reuse_cache_roots(paths: &Paths, game_dir: &str) -> Vec<String> {
    let settings = crate::settings::load(paths);
    if !settings.reuse_game_folders {
        return Vec::new();
    }
    let resolve = |path: &str| -> Option<String> {
        let path = path.trim();
        if path.is_empty() {
            return None;
        }
        let candidate = std::path::Path::new(path);
        let resolved = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.to_path_buf());
        Some(resolved.to_string_lossy().to_string())
    };
    let target = resolve(game_dir);
    let mut roots: Vec<String> = Vec::new();
    let mut push = |path: &str| {
        if let Some(resolved) = resolve(path) {
            if Some(&resolved) != target.as_ref() && !roots.contains(&resolved) {
                roots.push(resolved);
            }
        }
    };
    push(&settings.active_game_folder);
    for entry in &settings.game_folders {
        push(&entry.path);
    }
    for path in &settings.folder_order {
        push(path);
    }
    roots
}


/// 装完之后通知各页面重新读盘：版本页靠这个自动刷新列表，
/// 否则它会一直显示安装前的快照（用户删掉目录、装完新版本都看不到变化）。
fn announce_versions(app: &tauri::AppHandle, game_dir: &str) {
    use tauri::Emitter;
    let _ = app.emit("versions-changed", serde_json::json!({ "gameDir": game_dir }));
}

#[tauri::command]
pub async fn install_game_version(
    app: AppHandle,
    runtime: State<'_, InstallRuntime>,
    paths: State<'_, Paths>,
    install_id: Option<u64>,
    mut request: InstallRequest,
) -> Result<InstallSummary, String> {
    request.cache_roots = reuse_cache_roots(&paths, &request.game_dir);
    // 前端任务卡的 id 用不上时（版本页那种一次性安装）发一个匿名号，照样互不干扰。
    let install_id = install_id.unwrap_or_else(next_anonymous_install);
    let token = runtime.begin(install_id)?;
    let runtime_inner = runtime.inner().clone();
    // 装完要登记产出（任务卡的「打开文件夹 / 移除并删除文件」），得先留一份游戏目录。
    let version_dir = request.game_dir.clone();

    let watched = request.version_id.clone();
    let (_watch, emit) = crate::install_watch::watch(
        app.clone(),
        paths.log_dir(),
        token.clone(),
        false,
        install_id,
        &watched,
    );
    let result = tauri::async_runtime::spawn_blocking(move || {
        install_version(request, token, emit).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?;

    runtime_inner.end(install_id);
    let mut result = result;
    if let Ok(summary) = &mut result {
        if summary.path.is_empty() && !summary.version_id.is_empty() {
            summary.path = PathBuf::from(&version_dir)
                .join("versions")
                .join(&summary.version_id)
                .to_string_lossy()
                .into_owned();
        }
        // 原版版本目录本身也是一份实例：记下它对应哪个基础版本，
        // 目录被改名之后还认得出（下载那层不写，免得破坏 sha1 校验）。
        let _ = launcher_core::merge::stamp_meta(
            Path::new(&version_dir),
            &summary.version_id,
            &summary.version_id,
            &[],
        );
        remember_output(summary.path.clone());
    }
    if result.is_ok() {
        announce_versions(&app, &version_dir);
    }
    result
}

#[tauri::command]
pub async fn install_game_modloader(
    app: AppHandle,
    runtime: State<'_, InstallRuntime>,
    paths: State<'_, Paths>,
    install_id: Option<u64>,
    mut request: ModLoaderInstallRequest,
) -> Result<InstallSummary, String> {
    request.cache_roots = reuse_cache_roots(&paths, &request.game_dir);
    // 前端任务卡的 id 用不上时（版本页那种一次性安装）发一个匿名号，照样互不干扰。
    let install_id = install_id.unwrap_or_else(next_anonymous_install);
    let token = runtime.begin(install_id)?;
    let runtime_inner = runtime.inner().clone();
    // request 要搬进后台线程，装完登记产出还得知道游戏目录在哪。
    let loader_dir = request.game_dir.clone();
    let launch_dir_probe = std::path::PathBuf::from(&request.game_dir);

    {
        let loaders = request_loaders(&request);
        // 先读原版 json（它带着 `javaVersion` 与 `component`）：安装加载器时要跑
        // 处理器，选错 Java 会一路报「class file version 69.0」这类错。
        let requirement =
            launcher_core::java::requirement_for_dir(&launch_dir_probe, &request.game_version)
                .unwrap_or_else(|| {
                    launcher_core::java::requirement_with(&request.game_version, None, &loaders)
                });
        let current = launcher_core::detect_java()
            .into_iter()
            .find(|java| java.path.eq_ignore_ascii_case(&request.java_path))
            .map(|java| java.major);
        if current.is_none_or(|major| !requirement.accepts(major)) {
            let picked = pick_java_for(requirement.clone())?;
            crate::versions::append_log(
                paths.data_dir(),
                &format!(
                    "[java] 安装 {} 需要 {}，选用 {}",
                    request.game_version,
                    requirement.describe(),
                    picked
                ),
            );
            request.java_path = picked;
        }
    }

    let watched = request.game_version.clone();
    let (_watch, emit) = crate::install_watch::watch(
        app.clone(),
        paths.log_dir(),
        token.clone(),
        true,
        install_id,
        &watched,
    );
    let result = tauri::async_runtime::spawn_blocking(move || {
        install_modloader(request, token, emit).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?;

    runtime_inner.end(install_id);
    let mut result = result;
    if let Ok(summary) = &mut result {
        // 加载器外壳的产出就是 `versions/<外壳名>`：装完能直接打开、也能移除。
        if summary.path.is_empty() && !summary.version_id.is_empty() {
            summary.path = PathBuf::from(&loader_dir)
                .join("versions")
                .join(&summary.version_id)
                .to_string_lossy()
                .into_owned();
        }
        remember_output(summary.path.clone());
    }
    if result.is_ok() {
        announce_versions(&app, &loader_dir);
    }
    result
}

#[tauri::command]
pub async fn install_game_instance(
    app: AppHandle,
    paths: State<'_, Paths>,
    runtime: State<'_, InstallRuntime>,
    install_id: Option<u64>,
    mut request: InstanceRequest,
) -> Result<InstallSummary, String> {
    request.cache_roots = reuse_cache_roots(&paths, &request.game_dir);
    // 前端任务卡的 id 用不上时（版本页那种一次性安装）发一个匿名号，照样互不干扰。
    let install_id = install_id.unwrap_or_else(next_anonymous_install);
    let token = runtime.begin(install_id)?;
    let runtime_inner = runtime.inner().clone();
    // 暂停时半成品就是这个目录；装成功或用户取消都要把登记撤掉。
    let instance_path = launcher_core::instance::instance_target(&request);
    let game_dir_announce = request.game_dir.clone();
    let probe = token.clone();

    let has_loaders = !request.loaders.is_empty();
    // 实例名里带着游戏版本和加载器（`1.12.2-Forge_…`），拿它来判断要不要放宽看门狗。
    let watched = request
        .instance_name
        .clone()
        .unwrap_or_else(|| request.game_version.clone());
    let (_watch, emit) = crate::install_watch::watch(
        app.clone(),
        paths.log_dir(),
        token.clone(),
        has_loaders,
        install_id,
        &watched,
    );
    let result = tauri::async_runtime::spawn_blocking(move || {
        install_instance(request, token, emit).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?;

    runtime_inner.end(install_id);
    if let Some(path) = &instance_path {
        match &result {
            Ok(_) => crate::sweep::clear_paused(path),
            // 暂停：半成品留着续传，但关窗口时删掉它。
            Err(_) if probe.is_paused() => crate::sweep::mark_paused(path),
            Err(_) => crate::sweep::clear_paused(path),
        }
    }
    if let Ok(summary) = &result {
        remember_output(summary.path.clone());
    }
    if result.is_ok() {
        announce_versions(&app, &game_dir_announce);
    }
    result
}

/// 整合包装完把来源写进实例的 `ybmcl.json`：概览页据此显示「整合包版」那一套
/// （本地导入、反查不到项目的包写 `source = "zip"`，显示成「本地整合包」）。
fn write_pack(game_dir: &str, summary: &launcher_core::modpack::ModpackSummary) {
    let Some(pack) = &summary.pack else {
        return;
    };
    let info = crate::versions::PackInfo {
        source: pack.source.clone(),
        project_id: pack.project_id.clone(),
        project_name: pack.project_name.clone(),
        version_label: pack.version_label.clone(),
        summary: pack.summary.clone(),
        downloads: pack.downloads,
        tags: pack.tags.clone(),
    };
    let _ = crate::versions::set_pack(Path::new(game_dir), &summary.version_id, Some(info));
}

#[tauri::command]
pub async fn install_modpack(
    app: AppHandle,
    paths: State<'_, Paths>,
    runtime: State<'_, InstallRuntime>,
    install_id: Option<u64>,
    mut request: launcher_core::modpack::ModpackRequest,
) -> Result<launcher_core::modpack::ModpackSummary, String> {
    request.cache_roots = reuse_cache_roots(&paths, &request.game_dir);
    if request.threads.is_none() {
        request.threads = Some(crate::settings::load(&paths).concurrent_downloads);
    }
    // 前端任务卡的 id 用不上时（版本页那种一次性安装）发一个匿名号，照样互不干扰。
    let install_id = install_id.unwrap_or_else(next_anonymous_install);
    let token = runtime.begin(install_id)?;
    let runtime_inner = runtime.inner().clone();
    let staging = PathBuf::from(&request.game_dir)
        .join(".ybmcl")
        .join("modpack");
    let pack_dir = request.game_dir.clone();
    crate::sweep::clear_paused(&staging);
    let probe = token.clone();

    let app_announce = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let emitter = app.clone();
        launcher_core::modpack::install_modpack(
            request,
            token,
            move |step: &launcher_core::modpack::Step| {
                let _ = emitter.emit(
                    "modpack-progress",
                    serde_json::json!({
                        "installId": install_id,
                        "stage": step.stage,
                        "done": step.done,
                        "total": step.total,
                        "bytes": step.bytes,
                        "totalBytes": step.total_bytes,
                        "file": step.file,
                        "overall": step.overall,
                        "speed": step.speed,
                    }),
                );
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?;

    runtime_inner.end(install_id);
    match &result {
        Ok(summary) => {
            remember_output(summary.path.clone());
            write_pack(&pack_dir, summary);
            crate::sweep::clear_paused(&staging);
            announce_versions(&app_announce, &pack_dir);
        }
        Err(_) if probe.is_paused() => crate::sweep::mark_paused(&staging),
        Err(_) => crate::sweep::clear_paused(&staging),
    }
    result
}

/// 查这个实例的整合包有没有新版本。
#[tauri::command]
pub async fn pack_update_check(
    game_dir: String,
    game_directory: Option<String>,
) -> Result<Option<launcher_core::pack_update::UpdateInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        launcher_core::pack_update::check(&runtime).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 老实例没有来源记录时补一份（只写一次），补完只能走「重装更新」。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pack_update_backfill(
    game_dir: String,
    game_directory: Option<String>,
    source: String,
    project_id: String,
    version_label: String,
    game_version: String,
    loaders: Vec<String>,
) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        launcher_core::pack_update::backfill_meta(
            &runtime,
            &source,
            &project_id,
            &version_label,
            &game_version,
            &loaders,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 增量更新：备份 → 下载新增/替换 → 删除新包里没有的 → 写新记录。
#[tauri::command]
pub async fn pack_update_apply(
    app: AppHandle,
    game_dir: String,
    game_directory: Option<String>,
    keep_custom: bool,
) -> Result<launcher_core::pack_update::PackMeta, String> {
    let runtime = runtime_of(game_dir, game_directory)?;
    let instance_dir = runtime.clone();
    let info = tauri::async_runtime::spawn_blocking(move || {
        launcher_core::pack_update::check(&runtime)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "已经是最新版本了".to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    // 先把新包下载到实例内的暂存目录，再读它的清单算差异。
    let staging = instance_dir.join(".ybmcl").join("pack-update");
    let _ = std::fs::create_dir_all(&staging);
    let pack_path = staging.join(&info.latest_file_name);
    let download_url = info.latest_url.clone();
    let target = pack_path.clone();
    let total_bytes = info.latest_file_size;
    let size_hint = if total_bytes > 0 { Some(total_bytes) } else { None };
    let pack_emitter = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::download::fetch_pack_archive(
            &download_url,
            &target,
            size_hint,
            &launcher_core::download::CancellationToken::default(),
            move |progress: &launcher_core::download::BatchProgress| {
                let _ = pack_emitter.emit(
                    "pack-update-progress",
                    serde_json::json!({
                        "stage": "download-pack",
                        "bytes": progress.bytes,
                        "totalBytes": total_bytes,
                        "done": progress.done,
                        "total": progress.total,
                    }),
                );
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    let next = launcher_core::modpack::read_pack_files(&pack_path).map_err(|e| e.to_string())?;
    let meta = launcher_core::pack_update::read_meta_checked(&instance_dir)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "这个实例没有整合包更新记录".to_string())?;
    let latest_version = info.latest_version.clone();
    let emitter = app.clone();
    let dir = instance_dir.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let outcome = launcher_core::pack_update::apply(
            &dir,
            &meta,
            &next,
            &latest_version,
            keep_custom,
            &launcher_core::download::CancellationToken::default(),
            // 官方优先：镜像对 cdn.modrinth.com 只是 302 回 CDN，竞速反而可能抓住慢的那条。
            launcher_core::download::DownloadStrategy::OfficialFirst,
            &|stage: &str, done: u64, total: u64| {
                let _ = emitter.emit(
                    "pack-update-progress",
                    serde_json::json!({ "stage": stage, "done": done, "total": total }),
                );
            },
        )
        .map_err(|e| e.to_string());
        let _ = std::fs::remove_dir_all(dir.join(".ybmcl").join("pack-update"));
        outcome
    })
    .await
    .map_err(|e| e.to_string())?;
    result
}

/// 重装更新：用户数据挪到实例外，旧实例改名留底，按新包重建，再放回。
#[tauri::command]
pub async fn pack_reinstall(
    app: AppHandle,
    game_dir: String,
    game_directory: Option<String>,
    keep_custom: bool,
) -> Result<launcher_core::modpack::ModpackSummary, String> {
    let root = std::path::PathBuf::from(&game_dir);
    let instance_dir = runtime_of(game_dir.clone(), game_directory)?;
    let info = tauri::async_runtime::spawn_blocking({
        let instance_dir = instance_dir.clone();
        move || {
            launcher_core::pack_update::check(&instance_dir)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "已经是最新版本了".to_string())
        }
    })
    .await
    .map_err(|e| e.to_string())??;

    // 暂存与用户数据都放在游戏根目录下，绝不会被实例目录的删除波及。
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let workspace = root.join(".ybmcl-tmp").join(format!("reinstall-{stamp}"));
    std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
    let pack_path = workspace.join(&info.latest_file_name);
    let download_url = info.latest_url.clone();
    let target = pack_path.clone();
    let total_bytes = info.latest_file_size;
    let size_hint = if total_bytes > 0 { Some(total_bytes) } else { None };
    let pack_emitter = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::download::fetch_pack_archive(
            &download_url,
            &target,
            size_hint,
            &launcher_core::download::CancellationToken::default(),
            move |progress: &launcher_core::download::BatchProgress| {
                let _ = pack_emitter.emit(
                    "pack-update-progress",
                    serde_json::json!({
                        "stage": "download-pack",
                        "bytes": progress.bytes,
                        "totalBytes": total_bytes,
                        "done": progress.done,
                        "total": progress.total,
                    }),
                );
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    // 重装不保留自加内容：只搬存档、截图、选项、服务器列表。
    let _ = keep_custom;
    let moved = launcher_core::pack_update::reinstall_move_out(&instance_dir, &workspace)
        .map_err(|e| e.to_string())?;
    let backup = match launcher_core::pack_update::reinstall_swap_old(&instance_dir, stamp) {
        Ok(backup) => backup,
        Err(error) => {
            let _ = launcher_core::pack_update::reinstall_put_back(&instance_dir, &moved);
            let _ = std::fs::remove_dir_all(&workspace);
            return Err(error.to_string());
        }
    };

    let name = instance_dir
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("instance")
        .to_owned();
    // 重装是用新下载的包重新铺一遍：包里依然没有项目 id，沿用实例原来记的来源，
    // 否则重装一次「检查更新」就再也认不出这个包了。
    let kept = launcher_core::pack_update::read_meta(&instance_dir);
    let (kept_source, kept_project) = kept
        .as_ref()
        .map(|meta| (meta.source.clone(), meta.project_id.clone()))
        .unwrap_or_default();
    let request = launcher_core::modpack::ModpackRequest {
        game_dir: game_dir.clone(),
        url: None,
        path: Some(pack_path.to_string_lossy().to_string()),
        file_name: Some(info.latest_file_name.clone()),
        size: None,
        name: Some(name),
        java_path: None,
        strategy: Default::default(),
        threads: None,
        cache_roots: Vec::new(),
        icon_url: None,
        source: (!kept_source.trim().is_empty() && kept_source != "zip").then_some(kept_source),
        project_id: (!kept_project.trim().is_empty()).then_some(kept_project),
        project_name: None,
    };
    let emitter = app.clone();
    let install = tauri::async_runtime::spawn_blocking(move || {
        launcher_core::modpack::install_modpack(
            request,
            launcher_core::download::CancellationToken::default(),
            move |step: &launcher_core::modpack::Step| {
                let _ = emitter.emit(
                    "pack-update-progress",
                    serde_json::json!({
                        "stage": step.stage,
                        "done": step.done,
                        "total": step.total,
                    }),
                );
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?;

    let summary = match install {
        Ok(summary) => summary,
        Err(error) => {
            let _ = launcher_core::pack_update::reinstall_undo_old(&instance_dir, &backup);
            let _ = launcher_core::pack_update::reinstall_put_back(&instance_dir, &moved);
            let _ = std::fs::remove_dir_all(&workspace);
            return Err(error);
        }
    };

    let restored: std::path::PathBuf = summary.path.clone().into();
    launcher_core::pack_update::reinstall_put_back(&restored, &moved).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&backup);
    let _ = std::fs::remove_dir_all(&workspace);
    Ok(summary)
}

/// 恢复上次整合包更新。
#[tauri::command]
pub async fn pack_update_restore(
    game_dir: String,
    game_directory: Option<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        launcher_core::pack_update::restore(&runtime).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn instance_name_taken(game_dir: String, name: String) -> Result<bool, String> {
    launcher_core::instance::instance_name_taken(&game_dir, &name).map_err(|e| e.to_string())
}

/// 停下正在跑的安装。`mode = "pause"` 只停不清（`.part` 留着续传），
/// 其它值（含缺省）是「取消」：后台会把半成品与 `.part` 一起清掉。
#[tauri::command]
pub fn cancel_install(
    runtime: State<'_, InstallRuntime>,
    install_id: Option<u64>,
    mode: Option<String>,
) -> Result<bool, String> {
    let Some(install_id) = install_id else {
        return Ok(false);
    };
    let running = runtime.0.lock().map_err(|e| e.to_string())?;
    match running.get(&install_id) {
        Some(token) => {
            if mode.as_deref() == Some("pause") {
                token.pause();
            } else {
                token.cancel();
            }
            Ok(true)
        }
        None => Ok(false),
    }
}

// ---------------------------------------------------------------- 全局设置

#[tauri::command]
pub fn load_settings(paths: State<'_, Paths>) -> Result<crate::settings::Settings, String> {
    Ok(crate::settings::load(&paths))
}

#[tauri::command]
pub fn save_settings(
    paths: State<'_, Paths>,
    settings: crate::settings::Settings,
) -> Result<crate::settings::Settings, String> {
    let saved = crate::settings::save(&paths, settings)?;
    launcher_core::download::set_strategy(strategy_of(saved.download_strategy.as_deref()));
    launcher_core::download::set_http2(saved.use_http2);
    launcher_core::download::set_speed_limit(launcher_core::download::tier_limit(
        saved.download_speed,
    ));
    launcher_core::proxy::configure(&saved.proxy_mode, &saved.proxy_url);
    crate::versions::set_isolation_mode(&saved.isolation_mode);
    Ok(saved)
}

// ---------------------------------------------------------------- 账户

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountsView {
    pub players: Vec<PlayerView>,
    pub current: Option<String>,
    /// 每个来源档自己的顺序（`all` / `offline` / `microsoft` / 第三方服务器 URL）。
    pub orders: std::collections::BTreeMap<String, Vec<String>>,
    /// 加过的第三方认证服务器（只给界面显示名字与地址，元数据留在磁盘上）。
    pub servers: Vec<AuthServerView>,
}

/// 给界面看的账户：不带刷新令牌。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerView {
    pub id: String,
    pub name: String,
    pub uuid: String,
    pub kind: launcher_core::auth::AccountKind,
    pub avatar: Option<String>,
    pub server: Option<String>,
    /// 整张皮肤（data URL），账号页画全身预览用。
    pub skin: Option<String>,
    pub cape: Option<String>,
    /// `slim` 或 `classic`。
    pub skin_model: Option<String>,
    /// 选的是哪一款官方默认皮肤；自定义皮肤为 None。
    pub skin_official: Option<String>,
    /// 选的是哪一张内置披风；自选文件为 None。
    pub cape_official: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthServerView {
    pub url: String,
    pub name: String,
}

/// 读账户表；空表时把老设置里那个玩家名转成一个离线账户，避免升级后「一个账户都没有」。
/// 读账户表。
///
/// 以前这里会在表为空时「顺手补一个离线账户」，结果删掉最后一个账户后
/// 下一条命令又把它加回来，看起来就像删不掉。现在空表就是空表，
/// 界面自己显示「未登录 / 还没有账户」。
fn accounts_store(paths: &crate::portable::Paths) -> launcher_core::auth::Store {
    launcher_core::auth::Store::load(paths.data_dir())
}

fn accounts_view(store: &launcher_core::auth::Store) -> AccountsView {
    AccountsView {
        players: store
            .players
            .iter()
            .map(|player| PlayerView {
                id: player.id.clone(),
                name: player.name.clone(),
                uuid: player.uuid.clone(),
                kind: player.kind,
                avatar: player.avatar.clone(),
                server: player.server.clone(),
                skin: player.skin.clone(),
                cape: player.cape.clone(),
                skin_model: player.skin_model.clone(),
                skin_official: player.skin_official.clone(),
                cape_official: player.cape_official.clone(),
            })
            .collect(),
        current: store.current.clone(),
        orders: {
            let mut filters: Vec<String> = vec!["all".into(), "offline".into(), "microsoft".into()];
            for server in &store.auth_servers {
                filters.push(server.url.clone());
            }
            filters
                .into_iter()
                .map(|filter| {
                    let order = store.order_for(&filter);
                    (filter, order)
                })
                .collect()
        },
        servers: store
            .auth_servers
            .iter()
            .map(|server| AuthServerView {
                url: server.url.clone(),
                name: server.name.clone(),
            })
            .collect(),
    }
}

#[tauri::command]
pub fn list_accounts(paths: State<'_, Paths>) -> Result<AccountsView, String> {
    Ok(accounts_view(&accounts_store(&paths)))
}

#[tauri::command]
pub fn add_offline_account(
    paths: State<'_, Paths>,
    name: String,
    uuid: Option<String>,
) -> Result<AccountsView, String> {
    let mut store = accounts_store(&paths);
    let id = store.add_offline(&name, uuid.as_deref())?;
    store
        .select(&id)
        .map_err(|error| error.to_string())?;
    store
        .save(paths.data_dir())
        .map_err(|error| format!("账户表写不进去：{error}"))?;
    Ok(accounts_view(&store))
}

#[tauri::command]
pub fn rename_account(
    paths: State<'_, Paths>,
    id: String,
    name: String,
) -> Result<AccountsView, String> {
    let mut store = accounts_store(&paths);
    store.rename(&id, &name)?;
    store
        .save(paths.data_dir())
        .map_err(|error| format!("账户表写不进去：{error}"))?;
    Ok(accounts_view(&store))
}

/// 离线账户挑皮肤：`本地文件` 与 `LittleSkin` 两条路都会给出 data URL 与手臂模型。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkinPick {
    pub skin: String,
    pub model: String,
}

#[tauri::command]
pub async fn pick_skin_file(app: tauri::AppHandle) -> Result<Option<SkinPick>, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(picked) = app
        .dialog()
        .file()
        .add_filter("皮肤图片", &["png"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|error| error.to_string())?;
    let skin = tauri::async_runtime::spawn_blocking(move || {
        launcher_core::auth::local_skin(&path.to_string_lossy())
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(Some(SkinPick {
        skin,
        model: "classic".into(),
    }))
}

#[tauri::command]
pub async fn fetch_littleskin_skin(name: String) -> Result<SkinPick, String> {
    let (skin, model) = tauri::async_runtime::spawn_blocking(move || {
        launcher_core::auth::littleskin_skin(&name)
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(SkinPick { skin, model })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapePick {
    pub cape: String,
}

#[tauri::command]
pub async fn pick_cape_file(app: tauri::AppHandle) -> Result<Option<CapePick>, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(picked) = app
        .dialog()
        .file()
        .add_filter("披风图片", &["png"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|error| error.to_string())?;
    let cape = tauri::async_runtime::spawn_blocking(move || {
        launcher_core::auth::local_cape(&path.to_string_lossy())
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(Some(CapePick { cape }))
}

#[tauri::command]
pub fn set_offline_look(
    paths: State<'_, Paths>,
    id: String,
    skin: Option<String>,
    skin_model: Option<String>,
    skin_official: Option<String>,
    cape: Option<String>,
    cape_official: Option<String>,
) -> Result<AccountsView, String> {
    let mut store = accounts_store(&paths);
    store.set_offline_look(&id, skin, skin_model, skin_official, cape, cape_official)?;
    store
        .save(paths.data_dir())
        .map_err(|error| format!("账户表写不进去：{error}"))?;
    Ok(accounts_view(&store))
}

#[tauri::command]
pub fn remove_account(paths: State<'_, Paths>, id: String) -> Result<AccountsView, String> {
    let mut store = accounts_store(&paths);
    store.remove(&id);
    store
        .save(paths.data_dir())
        .map_err(|error| format!("账户表写不进去：{error}"))?;
    Ok(accounts_view(&store))
}

#[tauri::command]
pub fn select_account(paths: State<'_, Paths>, id: String) -> Result<AccountsView, String> {
    let mut store = accounts_store(&paths);
    store.select(&id)?;
    store
        .save(paths.data_dir())
        .map_err(|error| format!("账户表写不进去：{error}"))?;
    Ok(accounts_view(&store))
}

#[tauri::command]
pub fn reorder_accounts(
    paths: State<'_, Paths>,
    filter: String,
    ids: Vec<String>,
) -> Result<AccountsView, String> {
    let mut store = accounts_store(&paths);
    store.reorder(&filter, &ids)?;
    store
        .save(paths.data_dir())
        .map_err(|error| format!("账户表写不进去：{error}"))?;
    Ok(accounts_view(&store))
}

#[tauri::command]
pub fn clear_account(paths: State<'_, Paths>) -> Result<AccountsView, String> {
    let mut store = accounts_store(&paths);
    store.deselect();
    store
        .save(paths.data_dir())
        .map_err(|error| format!("账户表写不进去：{error}"))?;
    Ok(accounts_view(&store))
}

/// 微软登录一轮的结果：界面按 `state` 决定继续轮询还是收工。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MsLogin {
    pub state: String,
    pub error: Option<String>,
    pub accounts: Option<AccountsView>,
}

#[tauri::command]
pub async fn ms_login_start() -> Result<launcher_core::auth::DeviceCode, String> {
    tauri::async_runtime::spawn_blocking(launcher_core::auth::start_device_code)
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn ms_login_poll(
    paths: State<'_, Paths>,
    device_code: String,
) -> Result<MsLogin, String> {
    let data_dir = paths.data_dir().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let polled = launcher_core::auth::poll_device_code(&device_code)?;
        Ok::<_, String>(match polled {
            launcher_core::auth::Poll::Pending => MsLogin {
                state: "pending".into(),
                error: None,
                accounts: None,
            },
            launcher_core::auth::Poll::Failed(error) => MsLogin {
                state: "failed".into(),
                error: Some(error),
                accounts: None,
            },
            launcher_core::auth::Poll::Done(account) => {
                let mut store = launcher_core::auth::Store::load(&data_dir);
                store.absorb(*account);
                store
                    .save(&data_dir)
                    .map_err(|error| format!("账户表写不进去：{error}"))?;
                MsLogin {
                    state: "done".into(),
                    error: None,
                    accounts: Some(accounts_view(&store)),
                }
            }
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 手动续期当前账户（比如启动器开着放了一夜）。
#[tauri::command]
pub async fn refresh_account(
    paths: State<'_, Paths>,
    id: String,
) -> Result<AccountsView, String> {
    let data_dir = paths.data_dir().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let mut store = launcher_core::auth::Store::load(&data_dir);
        let account = store
            .players
            .iter_mut()
            .find(|player| player.id == id)
            .ok_or_else(|| "找不到这个账户。".to_string())?;
        launcher_core::auth::refresh(account)?;
        store
            .save(&data_dir)
            .map_err(|error| format!("账户表写不进去：{error}"))?;
        Ok::<_, String>(accounts_view(&store))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 加一个第三方认证服务器：读它的元数据（名字、签名公钥），存进账户表。
#[tauri::command]
pub async fn add_auth_server(paths: State<'_, Paths>, url: String) -> Result<AccountsView, String> {
    let data_dir = paths.data_dir().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let server = launcher_core::authlib::fetch_server(&url)?;
        let mut store = launcher_core::auth::Store::load(&data_dir);
        store.add_server(server);
        store
            .save(&data_dir)
            .map_err(|error| format!("账户表写不进去：{error}"))?;
        Ok::<_, String>(accounts_view(&store))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn remove_auth_server(paths: State<'_, Paths>, url: String) -> Result<AccountsView, String> {
    let mut store = accounts_store(&paths);
    store.remove_server(&url);
    store
        .save(paths.data_dir())
        .map_err(|error| format!("账户表写不进去：{error}"))?;
    Ok(accounts_view(&store))
}

/// 取服务器元数据：内置服务器只带了地址和名字，第一次登录时再补 metadata
/// （注入参数要用它，缺了 authlib-injector 会报错）。
fn ensure_server_meta(
    store: &mut launcher_core::auth::Store,
    data_dir: &std::path::Path,
    server: launcher_core::authlib::Server,
) -> Result<launcher_core::authlib::Server, String> {
    if server.metadata.is_some() {
        return Ok(server);
    }
    let mut fetched = launcher_core::authlib::fetch_server(&server.url)?;
    // 名字以我们写好的为准（服务器元数据里那串不一定和界面上的叫法一致）。
    if !server.name.trim().is_empty() && server.name != server.url {
        fetched.name = server.name;
    }
    store.add_server(fetched.clone());
    let _ = store.save(data_dir);
    Ok(fetched)
}

/// 用账号密码登录第三方认证服务器；一个账号下有多个角色时全部加进来。
#[tauri::command]
pub async fn add_third_party_account(
    paths: State<'_, Paths>,
    url: String,
    username: String,
    password: String,
) -> Result<AccountsView, String> {
    let data_dir = paths.data_dir().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let mut store = launcher_core::auth::Store::load(&data_dir);
        // 服务器可能被手动删过，登录前先确认元数据还在（不在就重新拉一次）。
        let saved = store.auth_servers.iter().find(|server| server.url == url).cloned();
        let server = match saved {
            Some(server) => ensure_server_meta(&mut store, &data_dir, server)?,
            None => {
                let fetched = launcher_core::authlib::fetch_server(&url)?;
                store.add_server(fetched.clone());
                fetched
            }
        };
        let profiles = launcher_core::authlib::login(&server, &username, &password)?;
        if profiles.is_empty() {
            return Err("这个账号里没有 Minecraft 角色。".into());
        }
        for profile in profiles {
            let mut account = launcher_core::auth::Account {
                id: String::new(),
                name: profile.name,
                uuid: profile.uuid.to_lowercase(),
                kind: launcher_core::auth::AccountKind::Thirdparty,
                avatar: profile.avatar,
                refresh: None,
                access: None,
                access_expires: 0,
                server: Some(server.url.clone()),
                oauth: None,
                skin: profile.skin,
                cape: profile.cape,
                skin_model: profile.skin_model,
                skin_official: None,
                cape_official: None,
            };
            // 一个账号下可能有多个角色，都加进来；最后添加的那个成为当前账户。
            launcher_core::auth::set_session(&mut account, &profile.token);
            store.add_third_party(account);
        }
        store
            .save(&data_dir)
            .map_err(|error| format!("账户表写不进去：{error}"))?;
        Ok::<_, String>(accounts_view(&store))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 外链登录的中间状态：设备码这种东西只在本进程里活几十秒，没必要落盘。
static OPENID_LOGINS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, (launcher_core::authlib::Server, launcher_core::authlib::DeviceLogin)>>,
> = std::sync::OnceLock::new();

fn openid_logins(
) -> &'static std::sync::Mutex<
    std::collections::HashMap<String, (launcher_core::authlib::Server, launcher_core::authlib::DeviceLogin)>,
> {
    OPENID_LOGINS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 外链登录第一段：拿到设备码，交给界面显示（用户到浏览器确认）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLoginView {
    pub session: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[tauri::command]
pub async fn start_openid_login(
    paths: State<'_, Paths>,
    url: String,
) -> Result<DeviceLoginView, String> {
    let data_dir = paths.data_dir().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let mut store = launcher_core::auth::Store::load(&data_dir);
        let saved = store.auth_servers.iter().find(|server| server.url == url).cloned();
        let server = match saved {
            Some(server) => ensure_server_meta(&mut store, &data_dir, server)?,
            None => {
                let fetched = launcher_core::authlib::fetch_server(&url)?;
                store.add_server(fetched.clone());
                let _ = store.save(&data_dir);
                fetched
            }
        };
        let login = launcher_core::authlib::start_device_login(&server)?;
        let session = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_millis())
                .unwrap_or_default()
        );
        {
            let mut live = openid_logins().lock().map_err(|_| "登录状态被锁住了".to_string())?;
            live.insert(session.clone(), (server, login.clone()));
        }
        Ok::<_, String>(DeviceLoginView {
            session,
            user_code: login.user_code,
            verification_uri: login.verification_uri,
            interval: login.interval,
            expires_in: login.expires_in,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 外链登录第二段：轮询一次。`state` 为 `pending` / `slow_down` / `denied` / `expired` / `done`。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicePollView {
    pub state: String,
    pub accounts: Option<AccountsView>,
}

#[tauri::command]
pub async fn poll_openid_login(
    paths: State<'_, Paths>,
    session: String,
) -> Result<DevicePollView, String> {
    let data_dir = paths.data_dir().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let (server, login) = {
            let live = openid_logins().lock().map_err(|_| "登录状态被锁住了".to_string())?;
            live.get(&session)
                .cloned()
                .ok_or_else(|| "这次登录已经过期了，重新点一次。".to_string())?
        };
        match launcher_core::authlib::poll_device_login(&server, &login)? {
            launcher_core::authlib::DevicePoll::Pending => Ok::<_, String>(DevicePollView {
                state: "pending".into(),
                accounts: None,
            }),
            launcher_core::authlib::DevicePoll::SlowDown => Ok(DevicePollView {
                state: "slow_down".into(),
                accounts: None,
            }),
            launcher_core::authlib::DevicePoll::Denied => {
                remove_login(&session);
                Ok(DevicePollView {
                    state: "denied".into(),
                    accounts: None,
                })
            }
            launcher_core::authlib::DevicePoll::Expired => {
                remove_login(&session);
                Ok(DevicePollView {
                    state: "expired".into(),
                    accounts: None,
                })
            }
            launcher_core::authlib::DevicePoll::Ready(profile) => {
                let mut store = launcher_core::auth::Store::load(&data_dir);
                let mut account = launcher_core::auth::Account {
                    id: String::new(),
                    name: profile.name,
                    uuid: profile.uuid.to_lowercase(),
                    kind: launcher_core::auth::AccountKind::Thirdparty,
                    avatar: profile.avatar,
                    refresh: None,
                    access: None,
                    access_expires: 0,
                    server: Some(server.url.clone()),
                    oauth: profile.refresh_token.as_deref().and_then(launcher_core::auth::protect),
                    skin: profile.skin,
                    cape: profile.cape,
                    skin_model: profile.skin_model,
                    skin_official: None,
                    cape_official: None,
                };
                launcher_core::auth::set_session(&mut account, &profile.token);
                store.add_third_party(account);
                store
                    .save(&data_dir)
                    .map_err(|error| format!("账户表写不进去：{error}"))?;
                remove_login(&session);
                Ok(DevicePollView {
                    state: "done".into(),
                    accounts: Some(accounts_view(&store)),
                })
            }
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

fn remove_login(session: &str) {
    if let Ok(mut live) = openid_logins().lock() {
        live.remove(session);
    }
}

// ---------------------------------------------------------------- 模组

/// 实例的运行目录：隔离实例是 `<游戏目录>/instances/<实例 id>`，否则就是游戏目录。
fn runtime_of(game_dir: String, game_directory: Option<String>) -> Result<PathBuf, String> {
    launcher_core::resolve_runtime_directory(&game_dir, game_directory.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_mods(
    game_dir: String,
    game_directory: Option<String>,
) -> Result<Vec<launcher_core::ModEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        launcher_core::scan_mods(&runtime).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 给「缺图标的模组」按哈希去 Modrinth / CurseForge 反查项目图标。
///
/// 只对 `files` 里点名的文件算哈希，返回「文件名 → 图标地址」。
#[tauri::command]
pub async fn mod_project_icons(
    paths: State<'_, Paths>,
    game_dir: String,
    game_directory: Option<String>,
    files: Vec<String>,
) -> Result<std::collections::HashMap<String, launcher_core::modicons::ProjectInfo>, String> {
    let data_dir = paths.data_dir().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        let mods_dir = runtime.join("mods");
        // 一次读盘同时算 SHA1 和指纹：文件大小/修改时间没变就走 `mods-hashes.json`
        // 的缓存，不必每次启动都把几百个 jar 整份重读一遍（665 MB 的整合包就是几秒）。
        let keys = launcher_core::modicons::keys_of(&data_dir, "mods-hashes.json", &mods_dir, &files);
        if keys.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        Ok(launcher_core::modicons::icons_for(&data_dir, &keys))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_mod_enabled(
    game_dir: String,
    game_directory: Option<String>,
    file_name: String,
    enabled: bool,
) -> Result<launcher_core::ModEntry, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        launcher_core::set_mod_enabled(&runtime, &file_name, enabled).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn remove_mod(
    game_dir: String,
    game_directory: Option<String>,
    file_name: String,
    permanent: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        launcher_core::delete_mod(&runtime, &file_name, permanent).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 认领模组对应的项目（先 Modrinth sha1，再 CurseForge 指纹），结果落缓存。
/// 认领不到返回 `None`，前端退回「只按显示名搜索」。
#[tauri::command]
pub async fn mod_project(
    paths: State<'_, Paths>,
    game_dir: String,
    game_directory: Option<String>,
    file_name: String,
) -> Result<Option<launcher_core::mod_updates::ModProject>, String> {
    let data_dir = paths.data_dir().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        if let Some(key) = crate::mod_projects::key(&runtime, &file_name) {
            if let Some(cached) = crate::mod_projects::get(&data_dir, &key) {
                return Ok(cached.map(|item| launcher_core::mod_updates::ModProject {
                    source: item.source,
                    project_id: item.project_id,
                }));
            }
            let found = launcher_core::mod_updates::identify(&runtime, &file_name)
                .map_err(|e| e.to_string())?;
            crate::mod_projects::put(
                &data_dir,
                &key,
                found.as_ref().map(|item| crate::mod_projects::CachedProject {
                    source: item.source.clone(),
                    project_id: item.project_id.clone(),
                }),
            );
            return Ok(found);
        }
        launcher_core::mod_updates::identify(&runtime, &file_name).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Mod 内嵌图标的 data URL；jar 里没有图标就是 `None`。
#[tauri::command]
pub async fn mod_icon(
    game_dir: String,
    game_directory: Option<String>,
    file_name: String,
) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        launcher_core::mods::mod_icon(&runtime, &file_name).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 取消正在跑的模组更新检查（下次开始时会自动复位）。
static MOD_UPDATE_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 批量更新/下载时的取消令牌，前端点「取消」就 cancel 它。
static MOD_UPDATE_TOKEN: std::sync::Mutex<Option<launcher_core::download::CancellationToken>> =
    std::sync::Mutex::new(None);

#[tauri::command]
pub fn cancel_mod_updates() {
    MOD_UPDATE_CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Ok(guard) = MOD_UPDATE_TOKEN.lock() {
        if let Some(token) = guard.as_ref() {
            token.cancel();
        }
    }
}

/// 查模组更新。打开模组页时自动跑一次，`force` 只给手动的「检查更新」用。
/// 进度用 `mod-updates-progress` 事件发出去，前端按阶段显示。
#[tauri::command]
pub async fn check_mod_updates(
    app: tauri::AppHandle,
    paths: State<'_, Paths>,
    game_dir: String,
    game_directory: Option<String>,
    game_version: String,
    loader: String,
    force: Option<bool>,
) -> Result<launcher_core::mod_updates::UpdateReport, String> {
    let data_dir = paths.data_dir().to_owned();
    MOD_UPDATE_CANCEL.store(false, std::sync::atomic::Ordering::SeqCst);
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        let emitter = app.clone();
        let mut progress = move |stage: &str, done: usize, total: usize| {
            let _ = emitter.emit(
                "mod-updates-progress",
                serde_json::json!({ "stage": stage, "done": done, "total": total }),
            );
        };
        launcher_core::mod_updates::check(
            &runtime,
            &game_version,
            &loader,
            &data_dir,
            force.unwrap_or(false),
            &MOD_UPDATE_CANCEL,
            Some(&mut progress),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn apply_mod_update(
    app: tauri::AppHandle,
    game_dir: String,
    game_directory: Option<String>,
    update: launcher_core::mod_updates::ModUpdate,
) -> Result<String, String> {
    MOD_UPDATE_CANCEL.store(false, std::sync::atomic::Ordering::SeqCst);
    let token = launcher_core::download::CancellationToken::default();
    if let Ok(mut guard) = MOD_UPDATE_TOKEN.lock() {
        *guard = Some(token.clone());
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        let emitter = app.clone();
        let progress = move |batch: &launcher_core::download::BatchProgress| {
            let _ = emitter.emit(
                "mod-updates-progress",
                serde_json::json!({
                    "stage": "download",
                    "done": batch.done,
                    "total": batch.total,
                    "bytes": batch.bytes,
                    "file": batch.file,
                }),
            );
        };
        launcher_core::mod_updates::apply(&runtime, &update, &token, progress)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?;
    if let Ok(mut guard) = MOD_UPDATE_TOKEN.lock() {
        *guard = None;
    }
    result
}

#[tauri::command]
pub async fn add_mods(
    game_dir: String,
    game_directory: Option<String>,
    sources: Vec<String>,
    overwrite: bool,
) -> Result<Vec<launcher_core::ModImportOutcome>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        launcher_core::import_mods(&runtime, &sources, overwrite).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------- 工具箱

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEntry {
    pub name: String,
    pub bytes: u64,
    pub files: u64,
}

/// 统计目录下每个顶层子目录占用（跳过符号链接/junction，避免顺着链接跑到别处）。
fn dir_size(root: &std::path::Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let meta = match entry.metadata() {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            let file_type = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                bytes += meta.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

/// 游戏目录里各子目录占多大——给「磁盘占用」用。
#[tauri::command]
pub async fn dir_usage(paths: State<'_, Paths>) -> Result<Vec<UsageEntry>, String> {
    let root = crate::game_folders::current(&paths);
    tauri::async_runtime::spawn_blocking(move || {
        if !root.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let entries = std::fs::read_dir(&root).map_err(|e| format!("{}: {e}", root.display()))?;
        for entry in entries.flatten() {
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => continue,
            };
            if kind.is_symlink() || !kind.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let (bytes, files) = dir_size(&entry.path());
            if bytes == 0 && files == 0 {
                continue;
            }
            out.push(UsageEntry { name, bytes, files });
        }
        out.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 用系统默认浏览器打开一个网址（社区资源详情页的「打开项目页」）。
#[tauri::command]
pub fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// 打开任意目录（不存在就建出来）。
#[tauri::command]
pub fn open_folder(app: AppHandle, path: String) -> Result<(), String> {
    let target = std::path::PathBuf::from(path);
    std::fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(target.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// 这次运行里后端确实产出过的路径。前端带着路径回来删文件时只认这里面有的，
/// 免得哪天传错一个路径（或者界面出了 bug）把别处的东西删了。
fn produced_paths() -> &'static Mutex<HashSet<String>> {
    static PRODUCED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    PRODUCED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 能 canonicalize 就用真实路径（Windows 会把大小写、短名、`..` 抹平）。
fn normalize_path(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|real| real.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_owned())
}

/// 记下一条产出路径，原样返回，方便 `Ok(Some(remember_output(path)))` 这样写。
fn remember_output(path: String) -> String {
    if let Ok(mut known) = produced_paths().lock() {
        known.insert(normalize_path(&path));
    }
    path
}

/// 在资源管理器里定位：文件选中它本身，目录打开上一级并选中它。
/// 路径已经不在了就直接报错——不替用户建一个空目录出来。
#[tauri::command]
pub fn reveal_path(app: AppHandle, path: String) -> Result<(), String> {
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err(format!("{} 已经不在了", target.display()));
    }
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .reveal_item_in_dir(&target)
        .map_err(|error| error.to_string())
}

/// 任务卡右下的「移除并删除文件」：只删这次运行产出过的文件或目录。
#[tauri::command]
pub fn delete_output(path: String) -> Result<(), String> {
    let known = produced_paths()
        .lock()
        .map_err(|_| "内部状态锁坏了".to_string())?;
    let target = normalize_path(&path);
    if !known.contains(&target) {
        return Err("这个路径不是本程序下载或安装出来的，不动它".into());
    }
    drop(known);
    let target = PathBuf::from(&target);
    let metadata = std::fs::symlink_metadata(&target)
        .map_err(|error| format!("{}：{error}", target.display()))?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(&target).map_err(|error| format!("{}：{error}", target.display()))
    } else {
        std::fs::remove_file(&target).map_err(|error| format!("{}：{error}", target.display()))
    }
}

/// 任务卡渲染前问一句「这东西还在不在」，不在就只留「移除」。
#[tauri::command]
pub fn path_exists(path: String) -> bool {
    Path::new(&path).exists()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanTarget {
    pub key: String,
    pub title: String,
    pub hint: String,
    pub path: String,
    pub bytes: u64,
    pub files: u64,
}

/// 清理项里有几个是「一堆散文件」（.part、旧日志），单独算一下再决定删不删。
fn part_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => continue,
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(entry.path());
            } else if entry.path().extension().is_some_and(|ext| ext == "part") {
                out.push(entry.path());
            }
        }
    }
    out
}

/// 工具页「清理」面板要列的东西：固定几项，不让前端传任意路径。
#[derive(serde::Serialize)]
pub struct CacheGroup {
    pub key: String,
    pub title: String,
    pub bytes: u64,
    pub files: u64,
    pub count: usize,
}

#[derive(serde::Serialize)]
pub struct CacheUsage {
    pub path: String,
    pub total_bytes: u64,
    pub total_files: u64,
    pub groups: Vec<CacheGroup>,
}

#[derive(serde::Serialize)]
pub struct CacheClear {
    pub freed: u64,
    pub removed: usize,
    pub skipped: usize,
}

const CACHE_GROUPS: &[(&str, &str)] = &[
    ("vanilla", "原版底稿"),
    ("forge", "加载器 · Forge"),
    ("neoforge", "加载器 · NeoForge"),
    ("fabric", "加载器 · Fabric"),
    ("quilt", "加载器 · Quilt"),
    ("legacyfabric", "加载器 · LegacyFabric"),
    ("cleanroom", "加载器 · Cleanroom"),
    ("liteloader", "加载器 · LiteLoader"),
    ("optifine", "加载器 · OptiFine"),
    ("labymod", "加载器 · LabyMod"),
];

/// 外壳目录名 → 分组。LegacyFabric 的外壳 id 和 Fabric 一样，只能再看一眼库名。
fn cache_group_of(dir: &std::path::Path, id: &str) -> &'static str {
    let lower = id.to_lowercase();
    if lower.starts_with("labymod-") {
        return "labymod";
    }
    if lower.contains("neoforge") {
        return "neoforge";
    }
    if lower.contains("cleanroom") {
        return "cleanroom";
    }
    if lower.contains("liteloader") {
        return "liteloader";
    }
    if lower.contains("optifine") {
        return "optifine";
    }
    if lower.contains("quilt-loader") {
        return "quilt";
    }
    if lower.contains("fabric-loader") {
        let json = dir.join(format!("{id}.json"));
        let legacy = std::fs::read(&json)
            .map(|bytes| String::from_utf8_lossy(&bytes).contains("legacyfabric"))
            .unwrap_or(false);
        return if legacy { "legacyfabric" } else { "fabric" };
    }
    if lower.contains("forge") {
        return "forge";
    }
    "other"
}

fn cache_dirs(game_dir: &std::path::Path, group: &str) -> Vec<std::path::PathBuf> {
    let root = game_dir.join("cache");
    if group == "vanilla" {
        return std::fs::read_dir(root.join("vanilla"))
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().is_dir())
                    .map(|entry| entry.path())
                    .collect()
            })
            .unwrap_or_default();
    }
    std::fs::read_dir(root.join("loaders"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .filter(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    cache_group_of(&entry.path(), &name) == group
                })
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default()
}

#[tauri::command]
pub async fn cache_usage(paths: State<'_, Paths>) -> Result<CacheUsage, String> {
    let game_dir = crate::game_folders::current(&paths);
    tauri::async_runtime::spawn_blocking(move || {
        let mut groups: Vec<CacheGroup> = CACHE_GROUPS
            .iter()
            .map(|(key, title)| CacheGroup {
                key: (*key).to_owned(),
                title: (*title).to_owned(),
                bytes: 0,
                files: 0,
                count: 0,
            })
            .collect();
        for (index, (key, _)) in CACHE_GROUPS.iter().enumerate() {
            for dir in cache_dirs(&game_dir, key) {
                let (bytes, files) = dir_size(&dir);
                groups[index].bytes += bytes;
                groups[index].files += files;
                groups[index].count += 1;
            }
        }
        let total_bytes = groups.iter().map(|group| group.bytes).sum();
        let total_files = groups.iter().map(|group| group.files).sum();
        Ok(CacheUsage {
            path: game_dir.join("cache").display().to_string(),
            total_bytes,
            total_files,
            groups,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 清缓存。`group` 为 None 表示全部；正在安装的目录跳过，不打断任何人。
#[tauri::command]
pub async fn clear_cache(paths: State<'_, Paths>, group: Option<String>) -> Result<CacheClear, String> {
    let game_dir = crate::game_folders::current(&paths);
    tauri::async_runtime::spawn_blocking(move || {
        let busy = launcher_core::busy_installs();
        let groups: Vec<&str> = match group.as_deref() {
            Some(key) => vec![key],
            None => CACHE_GROUPS.iter().map(|(key, _)| *key).collect(),
        };
        let mut freed = 0u64;
        let mut removed = 0usize;
        let mut skipped = 0usize;
        for key in groups {
            for dir in cache_dirs(&game_dir, key) {
                let name = dir
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
                if busy.iter().any(|live| live == &name) {
                    skipped += 1;
                    continue;
                }
                let (bytes, _) = dir_size(&dir);
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => {
                        freed += bytes;
                        removed += 1;
                    }
                    Err(_) => skipped += 1,
                }
            }
        }
        Ok(CacheClear {
            freed,
            removed,
            skipped,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn cleanup_targets(paths: State<'_, Paths>) -> Result<Vec<CleanTarget>, String> {
    let game_dir = crate::game_folders::current(&paths);
    let data_dir = paths.data_dir().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let mut out: Vec<CleanTarget> = Vec::new();

        let mut push_dir = |key: &str, title: &str, hint: &str, dir: &std::path::Path| {
            if !dir.is_dir() {
                return;
            }
            let (bytes, files) = dir_size(dir);
            if bytes == 0 && files == 0 {
                return;
            }
            out.push(CleanTarget {
                key: key.to_owned(),
                title: title.to_owned(),
                hint: hint.to_owned(),
                path: dir.display().to_string(),
                bytes,
                files,
            });
        };

        push_dir(
            "game-logs",
            "游戏日志",
            "游戏目录下的 logs，只影响历史日志",
            &game_dir.join("logs"),
        );
        push_dir(
            "crash-reports",
            "崩溃报告",
            "游戏目录下的 crash-reports",
            &game_dir.join("crash-reports"),
        );
        push_dir(
            "launcher-cache",
            "启动器缓存",
            "YBUMCL 自己的下载缓存目录",
            &data_dir.join("cache"),
        );

        // 半截文件：上次下载被中断留下的 *.part
        let parts = part_files(&game_dir);
        if !parts.is_empty() {
            let bytes = parts
                .iter()
                .filter_map(|path| std::fs::metadata(path).ok())
                .map(|meta| meta.len())
                .sum::<u64>();
            out.push(CleanTarget {
                key: "part-files".into(),
                title: "下载残留".into(),
                hint: "上次没下完的 .part 半截文件，删掉重装会重新下载".into(),
                path: game_dir.display().to_string(),
                bytes,
                files: parts.len() as u64,
            });
        }

        // 旧日志：launcher.log.1
        let log_dir = data_dir.join("logs");
        if let Ok(entries) = std::fs::read_dir(&log_dir) {
            let olds: Vec<_> = entries
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".log.1"))
                .collect();
            if !olds.is_empty() {
                let bytes = olds
                    .iter()
                    .filter_map(|entry| entry.metadata().ok())
                    .map(|meta| meta.len())
                    .sum::<u64>();
                out.push(CleanTarget {
                    key: "old-launcher-logs".into(),
                    title: "启动器旧日志".into(),
                    hint: "日志超限时轮转出来的 launcher.log.1".into(),
                    path: log_dir.display().to_string(),
                    bytes,
                    files: olds.len() as u64,
                });
            }
        }

        out.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 清理某一项，返回释放的字节数。key 必须是上面列出的那几个。
#[tauri::command]
pub async fn cleanup_run(paths: State<'_, Paths>, key: String) -> Result<u64, String> {
    let game_dir = crate::game_folders::current(&paths);
    let data_dir = paths.data_dir().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let removed_bytes = |paths: Vec<std::path::PathBuf>| -> Result<u64, String> {
            let mut freed = 0u64;
            let mut failed = Vec::new();
            for path in paths {
                let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                match std::fs::remove_file(&path) {
                    Ok(()) => freed += size,
                    Err(error) => failed.push(format!("{}: {error}", path.display())),
                }
            }
            if !failed.is_empty() {
                return Err(failed.join("；"));
            }
            Ok(freed)
        };

        match key.as_str() {
            "game-cache" => {
                let dir = game_dir.join("cache");
                let (bytes, _) = dir_size(&dir);
                std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                Ok(bytes)
            }
            "game-logs" => {
                let dir = game_dir.join("logs");
                let (bytes, _) = dir_size(&dir);
                std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                Ok(bytes)
            }
            "crash-reports" => {
                let dir = game_dir.join("crash-reports");
                let (bytes, _) = dir_size(&dir);
                std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                Ok(bytes)
            }
            "launcher-cache" => {
                let dir = data_dir.join("cache");
                let (bytes, _) = dir_size(&dir);
                std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                Ok(bytes)
            }
            "part-files" => removed_bytes(part_files(&game_dir)),
            "old-launcher-logs" => {
                let dir = data_dir.join("logs");
                let olds: Vec<_> = std::fs::read_dir(&dir)
                    .map_err(|e| format!("{}: {e}", dir.display()))?
                    .flatten()
                    .filter(|entry| entry.file_name().to_string_lossy().ends_with(".log.1"))
                    .map(|entry| entry.path())
                    .collect();
                removed_bytes(olds)
            }
            other => Err(format!("未知的清理项：{other}")),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------- 实例目录内容

/// 目录里的一项（标签页的列表用）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryInfo {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub modified_ms: u64,
}

/// 资源包/光影包自带图标的快速路径：只读 zip 里的 `pack.png`，不联网。
#[tauri::command]
pub async fn pack_local_icons(
    paths: State<'_, Paths>,
    game_dir: String,
    game_directory: Option<String>,
    subdir: String,
    files: Vec<String>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let _ = &paths;
    tauri::async_runtime::spawn_blocking(move || {
        let sub = match subdir.as_str() {
            "resourcepacks" => "resourcepacks",
            "shaderpacks" => "shaderpacks",
            _ => return Err("这个目录没有对应的项目信息".to_owned()),
        };
        let runtime = runtime_of(game_dir, game_directory)?;
        let dir = runtime.join(sub);
        let mut icons = std::collections::HashMap::new();
        for file_name in files {
            if file_name.contains('/') || file_name.contains('\\') || file_name.contains("..") {
                continue;
            }
            let path = dir.join(&file_name);
            if !path.is_file() {
                continue;
            }
            if let Some(icon) = launcher_core::modicons::local_pack_icon(&path) {
                icons.insert(file_name, icon);
            }
        }
        Ok(icons)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 按哈希查资源包 / 光影包对应的平台项目（名字与图标）。
#[tauri::command]
pub async fn pack_projects(
    paths: State<'_, Paths>,
    game_dir: String,
    game_directory: Option<String>,
    subdir: String,
    files: Vec<String>,
) -> Result<std::collections::HashMap<String, launcher_core::modicons::ProjectInfo>, String> {
    let data_dir = paths.data_dir().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let sub = match subdir.as_str() {
            "resourcepacks" => "resourcepacks",
            "shaderpacks" => "shaderpacks",
            _ => return Err("这个目录没有对应的项目信息".to_owned()),
        };
        let runtime = runtime_of(game_dir, game_directory)?;
        let dir = runtime.join(sub);
        let cache = format!("{sub}-icons.json");
        // 文件名 + 大小 + 修改时间没变就不重算整包哈希；剩下的 8 线程并行算。
        let hashes = format!("{sub}-hashes.json");
        let keys = launcher_core::modicons::keys_of(&data_dir, &hashes, &dir, &files);
        Ok(launcher_core::modicons::icons_for_kind(
            &data_dir, &cache, &keys,
        ))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 读一个光影包的配置项（`shaders/shaders.properties` + 当前值文件）。
#[tauri::command]
pub async fn shader_config(
    game_dir: String,
    game_directory: Option<String>,
    pack: String,
) -> Result<launcher_core::shaders::ShaderConfig, String> {
    if pack.contains('/') || pack.contains('\\') || pack.contains("..") {
        return Err("光影包名字不合法".to_owned());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        launcher_core::shaders::load(&runtime.join("shaderpacks"), &pack)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 保存光影配置：只写用户改过的值，其余原来的键保留。
#[tauri::command]
pub async fn save_shader_config(
    game_dir: String,
    game_directory: Option<String>,
    pack: String,
    values: std::collections::HashMap<String, String>,
) -> Result<(), String> {
    if pack.contains('/') || pack.contains('\\') || pack.contains("..") {
        return Err("光影包名字不合法".to_owned());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        launcher_core::shaders::save(&runtime.join("shaderpacks"), &pack, values)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 存档列表：图标、时间、游戏版本。
#[tauri::command]
pub async fn list_saves(
    game_dir: String,
    game_directory: Option<String>,
) -> Result<Vec<launcher_core::saves::SaveEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        Ok(launcher_core::saves::list(&runtime.join("saves")))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 这个实例能不能用 `--quickPlaySingleplayer`（1.20+）。
#[tauri::command]
pub fn quick_play_supported(game_dir: String, id: String) -> bool {
    let meta = crate::version_meta::describe_all(std::path::Path::new(&game_dir), &[id]);
    let version = meta.values().next().and_then(|meta| meta.game_version.clone());
    wants_quick_play(version.as_deref())
}

/// 把存档文件夹放进剪贴板（资源管理器里能直接 Ctrl+V）。
#[tauri::command]
pub fn copy_save(game_dir: String, game_directory: Option<String>, name: String) -> Result<(), String> {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err("存档名不合法".into());
    }
    let runtime = runtime_of(game_dir, game_directory)?;
    let path = runtime.join("saves").join(&name);
    if !path.is_dir() {
        return Err(format!("{} 已经不在了", path.display()));
    }
    crate::clipboard_files::set_files(&[path])
}

/// 把外部文件（资源包/光影包）拷进实例目录；同名文件跳过，返回 (导入数, 跳过数)。
#[tauri::command]
pub fn import_files(
    game_dir: String,
    game_directory: Option<String>,
    subdir: String,
    sources: Vec<String>,
) -> Result<(usize, usize), String> {
    if subdir != "resourcepacks" && subdir != "shaderpacks" {
        return Err("这个目录不支持从文件安装".into());
    }
    let runtime = runtime_of(game_dir, game_directory)?;
    let target = runtime.join(&subdir);
    std::fs::create_dir_all(&target).map_err(|error| format!("建目录失败：{error}"))?;
    let mut imported = 0_usize;
    let mut skipped = 0_usize;
    for source in sources {
        let path = std::path::PathBuf::from(&source);
        let Some(name) = path.file_name().map(|value| value.to_string_lossy().into_owned()) else {
            skipped += 1;
            continue;
        };
        let dest = target.join(&name);
        if dest.exists() {
            skipped += 1;
            continue;
        }
        match std::fs::copy(&path, &dest) {
            Ok(_) => imported += 1,
            Err(_) => skipped += 1,
        }
    }
    Ok((imported, skipped))
}

/// 把剪贴板里的文件夹复制进 `saves/`，返回新存档的文件夹名。
#[tauri::command]
pub fn paste_save(
    game_dir: String,
    game_directory: Option<String>,
) -> Result<Vec<String>, String> {
    let files = crate::clipboard_files::get_files()?;
    let runtime = runtime_of(game_dir, game_directory)?;
    let saves = runtime.join("saves");
    let mut imported = Vec::new();
    for source in files {
        if !source.is_dir() {
            continue;
        }
        let Some(name) = source.file_name() else {
            continue;
        };
        let name = name.to_string_lossy().into_owned();
        let mut target = saves.join(&name);
        let mut index = 2;
        while target.exists() {
            target = saves.join(format!("{name} ({index})"));
            index += 1;
        }
        copy_dir(&source, &target)?;
        imported.push(
            target
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or(name),
        );
    }
    if imported.is_empty() {
        return Err("剪贴板里没有存档文件夹".into());
    }
    Ok(imported)
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|error| format!("{}: {error}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))? {
        let entry = entry.map_err(|error| error.to_string())?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// 删掉实例子目录里的一个文件/文件夹（资源包、光影包、存档、截图共用）。
#[tauri::command]
pub fn delete_entry(
    game_dir: String,
    game_directory: Option<String>,
    subdir: String,
    name: String,
    permanent: bool,
) -> Result<(), String> {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err("文件名不合法".into());
    }
    let sub = match subdir.as_str() {
        "resourcepacks" | "shaderpacks" | "saves" | "screenshots" | "schematics" | "mods" => {
            subdir.as_str()
        }
        _ => return Err("这个目录不支持删除".to_owned()),
    };
    let runtime = runtime_of(game_dir, game_directory)?;
    let path = runtime.join(sub).join(&name);
    if !path.exists() {
        return Err(format!("{} 已经不在了", path.display()));
    }
    if permanent {
        if path.is_dir() {
            std::fs::remove_dir_all(&path).map_err(|error| error.to_string())
        } else {
            std::fs::remove_file(&path).map_err(|error| error.to_string())
        }
    } else {
        launcher_core::mods::recycle_path(&path).map_err(|error| error.to_string())
    }
}

/// 把任意文件放进剪贴板（资源管理器里能粘贴）。
#[tauri::command]
pub fn copy_file(game_dir: String, game_directory: Option<String>, subdir: String, name: String) -> Result<(), String> {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err("文件名不合法".into());
    }
    let runtime = runtime_of(game_dir, game_directory)?;
    let path = runtime.join(&subdir).join(&name);
    if !path.exists() {
        return Err(format!("{} 已经不在了", path.display()));
    }
    crate::clipboard_files::set_files(&[path])
}

fn instance_root_of(paths: &Paths, id: &str) -> Result<PathBuf, String> {
    let game_dir = crate::game_folders::current(paths);
    let settings = crate::versions::load_instance(&game_dir, id);
    match crate::versions::game_directory(id, settings.isolated) {
        Some(relative) => {
            launcher_core::resolve_runtime_directory(&game_dir.to_string_lossy(), Some(&relative))
                .map_err(|error| error.to_string())
        }
        None => Ok(game_dir),
    }
}

/// 实例实际用的根目录（隔离实例是 `versions/<id>`，否则是游戏目录）。
#[tauri::command]
pub fn instance_root(paths: State<'_, Paths>, id: String) -> Result<String, String> {
    instance_root_of(&paths, &id).map(|path| path.to_string_lossy().into_owned())
}

fn modified_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 列目录；目录不存在返回空列表（界面按空态处理）。
#[tauri::command]
pub fn list_entries(path: String) -> Result<Vec<EntryInfo>, String> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    for entry in std::fs::read_dir(&root).map_err(|error| format!("{path}: {error}"))? {
        let entry = entry.map_err(|error| format!("{path}: {error}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        items.push(EntryInfo {
            name,
            dir: meta.is_dir(),
            size: meta.len(),
            modified_ms: modified_ms(&meta),
        });
    }
    items.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then(b.modified_ms.cmp(&a.modified_ms))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(items)
}

/// 读文本文件（日志、崩溃报告用）。
///
/// - 默认只留开头 `limit` 字节；`tail = true` 时留**末尾**——日志的错误都在最后，
///   按开头截断等于把要看的那段扔了。
/// - `.gz`（MC 自动压缩的老日志）先解压再截断，解压上限 16 MB，防止压缩炸弹。
#[tauri::command]
pub fn read_text_file(
    path: String,
    limit: Option<usize>,
    tail: Option<bool>,
) -> Result<String, String> {
    let file = PathBuf::from(&path);
    if file.is_dir() {
        return Err(format!("{path}: 这是文件夹，不是文件"));
    }
    let max = limit.unwrap_or(512 * 1024);
    let tail = tail.unwrap_or(false);
    let bytes = if file
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gz"))
    {
        read_gzip(&file, max, tail)?
    } else if tail {
        read_tail(&file, max)?
    } else {
        read_head(&file, max)?
    };
    Ok(decode_text(&bytes))
}

/// 把日志/文本的字节解成字符串。
///
/// 不是所有日志都是 UTF-8：NeoForge 在中文 Windows 上按系统代码页（GBK）写，
/// 日期里的「月」之类会变成两个非法字节，直接 `from_utf8_lossy` 就显示成 `�`。
/// 所以先严格按 UTF-8 试，不行再按 GBK 解；顺带认 BOM（UTF-16 的日志）。
fn decode_text(bytes: &[u8]) -> String {
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        let (text, _) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
        return text.into_owned();
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_owned();
    }
    // 只读末尾时可能正好切在一个多字节字符中间：跳过开头的续字节再试一次，
    // 免得把 UTF-8 的日志误判成 GBK。
    let mut start = 0;
    while start < bytes.len() && start < 3 && bytes[start] & 0xC0 == 0x80 {
        start += 1;
    }
    if start > 0 {
        if let Ok(text) = std::str::from_utf8(&bytes[start..]) {
            return text.to_owned();
        }
    }
    let (text, _, _) = encoding_rs::GBK.decode(bytes);
    text.into_owned()
}

fn read_head(file: &Path, max: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(file)
        .map_err(|error| format!("{}: {error}", file.display()))?
        .take(max as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", file.display()))?;
    Ok(bytes)
}

fn read_tail(file: &Path, max: usize) -> Result<Vec<u8>, String> {
    let mut handle =
        std::fs::File::open(file).map_err(|error| format!("{}: {error}", file.display()))?;
    let len = handle
        .metadata()
        .map_err(|error| format!("{}: {error}", file.display()))?
        .len();
    let from = len.saturating_sub(max as u64);
    if from > 0 {
        handle
            .seek(std::io::SeekFrom::Start(from))
            .map_err(|error| format!("{}: {error}", file.display()))?;
    }
    let mut bytes = Vec::new();
    handle
        .take(max as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", file.display()))?;
    Ok(bytes)
}

fn read_gzip(file: &Path, max: usize, tail: bool) -> Result<Vec<u8>, String> {
    /// 解压上限：日志再大也不该超过这个数，超了说明不是正常日志。
    const CEILING: u64 = 16 * 1024 * 1024;
    let raw = std::fs::File::open(file).map_err(|error| format!("{}: {error}", file.display()))?;
    let mut text = Vec::new();
    flate2::read::GzDecoder::new(raw)
        .take(CEILING)
        .read_to_end(&mut text)
        .map_err(|error| format!("{}: {error}", file.display()))?;
    if text.len() > max {
        if tail {
            text = text.split_off(text.len() - max);
        } else {
            text.truncate(max);
        }
    }
    Ok(text)
}

/// 递归搜索的一条结果。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub modified_ms: u64,
}

/// 递归搜索的一次进度：同一 token 的多批结果按到达顺序追加。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchProgress {
    pub token: String,
    pub hits: Vec<SearchHit>,
    pub scanned: u32,
    pub done: bool,
}

/// 进行中的搜索，按 token 存停止标志。
static SEARCH_STOPS: OnceLock<Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>> =
    OnceLock::new();

fn search_stops() -> &'static Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>> {
    SEARCH_STOPS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// 在 `root` 下按文件名找 `needle`（不区分大小写），边找边用 `file-search` 事件推结果。
/// 只扫文件和目录，跳过 junction/符号链接防止成环，深度上限 8 层。
#[tauri::command]
pub fn search_files(
    app: AppHandle,
    token: String,
    root: String,
    needle: String,
    limit: Option<usize>,
) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let stop = Arc::new(AtomicBool::new(false));
    search_stops()
        .lock()
        .unwrap()
        .insert(token.clone(), stop.clone());
    let limit = limit.unwrap_or(500).max(1);
    let pattern = needle.trim().to_lowercase();
    tauri::async_runtime::spawn_blocking(move || {
        let mut hits: Vec<SearchHit> = Vec::new();
        let mut scanned = 0u32;
        let mut stack = vec![(PathBuf::from(&root), 0u32)];
        let mut last_flush = std::time::Instant::now();
        let mut finished = false;
        while let Some((dir, depth)) = stack.pop() {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut subdirs = Vec::new();
            for entry in entries.flatten() {
                if stop.load(Ordering::Relaxed) {
                    finished = true;
                    break;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = entry.path();
                if meta.is_dir() {
                    let linked = entry
                        .file_type()
                        .map(|kind| kind.is_symlink())
                        .unwrap_or(false);
                    if depth < 8 && !linked {
                        subdirs.push((path, depth + 1));
                    }
                    continue;
                }
                scanned += 1;
                if name.to_lowercase().contains(&pattern) {
                    hits.push(SearchHit {
                        path: path.to_string_lossy().into_owned(),
                        name,
                        size: meta.len(),
                        modified_ms: modified_ms(&meta),
                    });
                    if hits.len() >= limit {
                        finished = true;
                        break;
                    }
                }
            }
            // 目录名按字母序往下走，结果读起来更稳。
            subdirs.sort_by(|a, b| a.0.cmp(&b.0));
            for item in subdirs.into_iter().rev() {
                stack.push(item);
            }
            if finished {
                break;
            }
            if last_flush.elapsed() >= Duration::from_millis(150) {
                let _ = app.emit(
                    "file-search",
                    SearchProgress {
                        token: token.clone(),
                        hits: std::mem::take(&mut hits),
                        scanned,
                        done: false,
                    },
                );
                last_flush = std::time::Instant::now();
            }
        }
        let _ = app.emit(
            "file-search",
            SearchProgress {
                token: token.clone(),
                hits,
                scanned,
                done: true,
            },
        );
        search_stops().lock().unwrap().remove(&token);
    });
    Ok(())
}

/// 停掉一次进行中的搜索（关闭搜索或离开标签时调）。
#[tauri::command]
pub fn stop_search(token: String) {
    if let Some(stop) = search_stops().lock().unwrap().get(&token) {
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// 图片读成 `data:` URL（截图缩略图用）。
#[tauri::command]
pub fn image_data_url(path: String) -> Result<String, String> {
    let file = PathBuf::from(&path);
    icon_data_url(&file).ok_or_else(|| format!("{} 不是可显示的图片", file.display()))
}

/// 读 `servers.dat` 里的服务器列表。
#[tauri::command]
pub fn read_servers(path: String) -> Result<Vec<launcher_core::servers::ServerEntry>, String> {
    launcher_core::servers::read_servers(Path::new(&path)).map_err(|error| error.to_string())
}

/// 把整个实例目录打成 zip；`None` 表示用户在另存为里取消了。
#[tauri::command]
pub async fn export_instance_zip(
    app: tauri::AppHandle,
    paths: State<'_, Paths>,
    id: String,
    task_id: Option<u32>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let root = instance_root_of(&paths, &id)?;
    let game_dir = crate::game_folders::current(&paths);
    let game_version = crate::version_meta::describe_all(&game_dir, &[id.clone()])
        .get(&id)
        .and_then(|meta| meta.game_version.clone())
        .unwrap_or_default();
    let loaders = crate::version_meta::describe_all(&game_dir, &[id.clone()])
        .get(&id)
        .map(|meta| meta.loaders.clone())
        .unwrap_or_default();

    let file_name = format!("{id}.zip");
    let mut dialog = app.dialog().file().set_file_name(&file_name);
    let downloads = game_dir.join("downloads");
    let _ = std::fs::create_dir_all(&downloads);
    dialog = dialog.set_directory(downloads.to_string_lossy().into_owned());
    let dialog = dialog.add_filter("压缩包", &["zip"]);
    let Some(target) = dialog.blocking_save_file() else {
        return Ok(None);
    };
    let out = target
        .into_path()
        .map_err(|error| error.to_string())?
        .to_string_lossy()
        .into_owned();

    let name = id.clone();
    let out_for_zip = out.clone();
    let emitter = app.clone();
    let written = tauri::async_runtime::spawn_blocking(move || {
        let emit = |done: u64, total: u64| {
            if let Some(task) = task_id {
                let _ = emitter.emit(
                    "download-progress",
                    serde_json::json!({ "id": task, "received": done, "total": total }),
                );
            }
        };
        launcher_core::packages::export_instance(
            std::path::Path::new(&root),
            std::path::Path::new(&out_for_zip),
            &name,
            &game_version,
            &loaders,
            &launcher_core::download::CancellationToken::default(),
            emit,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;

    let _ = written;
    Ok(Some(out))
}

// ---- 整合包导出（规则/识别/索引，逻辑照 PCL 的「整合包制作」）----

fn export_game_dir(paths: &Paths) -> PathBuf {
    crate::game_folders::current(paths)
}

fn export_dependencies(paths: &Paths, id: &str) -> Vec<(String, String)> {
    let game_dir = export_game_dir(paths);
    let mut deps = Vec::new();
    if let Some(meta) = crate::version_meta::describe_all(&game_dir, &[id.to_owned()]).remove(id) {
        if let Some(game) = meta.game_version {
            if !game.is_empty() {
                deps.push(("minecraft".to_owned(), game));
            }
        }
    }
    for choice in instance_loader_choices(&game_dir.to_string_lossy(), id) {
        let key = match choice.loader.to_ascii_lowercase().as_str() {
            "fabric" => "fabric-loader",
            "quilt" => "quilt-loader",
            "forge" => "forge",
            "neoforge" => "neoforge",
            _ => continue,
        };
        deps.push((key.to_owned(), choice.version));
    }
    deps
}

/// 导出页的选项表；可见性和动态子项按实例目录探过。
#[tauri::command]
pub fn export_pack_layout(
    paths: State<'_, Paths>,
    id: String,
) -> Result<Vec<launcher_core::export::OptionView>, String> {
    let root = instance_root_of(&paths, &id)?;
    let meta = crate::version_meta::describe_all(&export_game_dir(&paths), &[id.clone()]);
    let loaders = meta
        .get(&id)
        .map(|item| item.loaders.clone())
        .unwrap_or_default();
    let modable = loaders.iter().any(|name| {
        !matches!(
            name.to_ascii_lowercase().as_str(),
            "原版" | "minecraft" | "optifine"
        )
    });
    let has_optifine = loaders
        .iter()
        .any(|name| name.to_ascii_lowercase().contains("optifine"));
    Ok(launcher_core::export::layout(&root, modable, has_optifine))
}

const EXPORT_TEMP_DIR: &str = ".ybmcl-export";

/// 第一阶段：按规则把文件挑进临时目录，算出待识别的哈希。
#[tauri::command]
pub async fn export_pack_begin(
    app: tauri::AppHandle,
    paths: State<'_, Paths>,
    id: String,
    rules: Vec<String>,
    task_id: Option<u32>,
) -> Result<u64, String> {
    let root = instance_root_of(&paths, &id)?;
    let temp = export_game_dir(&paths).join(EXPORT_TEMP_DIR);
    std::fs::create_dir_all(&temp).map_err(|error| error.to_string())?;
    let emitter = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let emit = |done: u64, total: u64| {
            if let Some(task) = task_id {
                let _ = emitter.emit(
                    "download-progress",
                    serde_json::json!({ "id": task, "received": done, "total": total }),
                );
            }
        };
        launcher_core::export::begin(&root, &rules, &temp, emit).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 第二阶段：联网识别（Modrinth + CurseForge）。返回 `(认出来的文件数, 待识别文件数)`。
#[tauri::command]
pub async fn export_pack_identify(token: u64, modrinth_only: bool) -> Result<(usize, usize), String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::export::identify(token, modrinth_only).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 第三阶段：写索引、删掉已识别的文件、打包。`target` 为空就用游戏目录下的 `downloads/`。
#[tauri::command]
pub async fn export_pack_finish(
    app: tauri::AppHandle,
    paths: State<'_, Paths>,
    id: String,
    token: u64,
    name: String,
    version: String,
    include_launcher: bool,
    task_id: Option<u32>,
    target: Option<String>,
) -> Result<Option<String>, String> {
    let game_dir = export_game_dir(&paths);
    let dependencies = export_dependencies(&paths, &id);
    let settings = crate::versions::load_instance(&game_dir, &id);
    let summary = settings.description.clone();
    let stem = if name.trim().is_empty() {
        id.clone()
    } else {
        name.trim().to_owned()
    };
    let suffix = if include_launcher { "zip" } else { "mrpack" };
    let file_name = format!("{stem}.{suffix}");
    let out = match target {
        Some(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => {
            let downloads = game_dir.join("downloads");
            std::fs::create_dir_all(&downloads).map_err(|error| error.to_string())?;
            downloads.join(file_name)
        }
    };
    let launcher = if include_launcher {
        std::env::current_exe().ok()
    } else {
        None
    };
    let emitter = app.clone();
    let opener = app.clone();
    let out_for_zip = out.clone();
    let written = tauri::async_runtime::spawn_blocking(move || {
        let emit = |done: u64, total: u64| {
            if let Some(task) = task_id {
                let _ = emitter.emit(
                    "download-progress",
                    serde_json::json!({ "id": task, "received": done, "total": total }),
                );
            }
        };
        launcher_core::export::finish(
            token,
            &out_for_zip,
            &name,
            &version,
            &summary,
            &dependencies,
            include_launcher,
            launcher.as_deref(),
            emit,
        )
        .map(|_| out_for_zip.to_string_lossy().into_owned())
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;
    // PCL 导出完会直接打开所在文件夹。
    use tauri_plugin_opener::OpenerExt;
    if let Some(parent) = Path::new(&written).parent() {
        let _ = opener
            .opener()
            .open_path(parent.to_string_lossy().into_owned(), None::<&str>);
    }
    Ok(Some(written))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportConfigView {
    pub config_path: String,
    pub name: String,
    pub version: String,
    pub include_launcher: bool,
    pub pack_resources: bool,
    pub modrinth_only: bool,
    pub pack_path: Option<String>,
    pub rules: Vec<String>,
    pub extra_files: Vec<String>,
}

/// 读一份 PCL 格式的导出配置；取消返回 `None`。
#[tauri::command]
pub async fn export_pack_load_config(
    app: tauri::AppHandle,
) -> Result<Option<ExportConfigView>, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(picked) = app
        .dialog()
        .file()
        .add_filter("整合包导出配置", &["txt"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|error| error.to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let config = launcher_core::export::parse_config(&text).map_err(|error| error.to_string())?;
    Ok(Some(ExportConfigView {
        config_path: path.to_string_lossy().into_owned(),
        name: config.name,
        version: config.version,
        include_launcher: config.include_launcher,
        pack_resources: config.pack_resources,
        modrinth_only: config.modrinth_only,
        pack_path: config.pack_path,
        rules: config.rules,
        extra_files: config.extra_files,
    }))
}

/// 把当前勾选写成 PCL 格式的导出配置；取消返回 `None`。
#[tauri::command]
pub async fn export_pack_save_config(
    app: tauri::AppHandle,
    paths: State<'_, Paths>,
    name: String,
    version: String,
    include_launcher: bool,
    pack_resources: bool,
    modrinth_only: bool,
    pack_path: Option<String>,
    rules: Vec<String>,
    extra_files: Vec<String>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let downloads = export_game_dir(&paths).join("downloads");
    std::fs::create_dir_all(&downloads).map_err(|error| error.to_string())?;
    let Some(picked) = app
        .dialog()
        .file()
        .set_file_name("export_config.txt")
        .set_directory(downloads.to_string_lossy().into_owned())
        .add_filter("整合包导出配置", &["txt"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|error| error.to_string())?;
    let mut rules = rules;
    rules.retain(|line| !line.trim().is_empty());
    let config = launcher_core::export::Config {
        name,
        version,
        include_launcher,
        pack_resources,
        modrinth_only,
        pack_path,
        rules,
        extra_files,
    };
    let text = launcher_core::export::config_text(&config);
    std::fs::write(&path, text).map_err(|error| error.to_string())?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
pub fn export_pack_cancel(token: u64) {
    launcher_core::export::cancel(token);
}

/// 让用户挑一个 `.mrpack` 的落点；不挑就返回空（前端就用默认位置）。
#[tauri::command]
pub async fn export_pack_pick_path(
    app: tauri::AppHandle,
    paths: State<'_, Paths>,
    file_name: String,
    include_launcher: bool,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let downloads = export_game_dir(&paths).join("downloads");
    std::fs::create_dir_all(&downloads).map_err(|error| error.to_string())?;
    let name = if file_name.trim().is_empty() {
        format!("modpack.{}", if include_launcher { "zip" } else { "mrpack" })
    } else {
        file_name
    };
    let picked = app
        .dialog()
        .file()
        .set_file_name(&name)
        .set_directory(downloads.to_string_lossy().into_owned())
        .add_filter(if include_launcher {
            "压缩文件 (*.zip)"
        } else {
            "Modrinth 整合包 (*.mrpack)"
        }, if include_launcher { &["zip"] } else { &["mrpack"] })
        .blocking_save_file();
    Ok(picked.map(|value| value.to_string()))
}

/// 已安装模组提供的 modid 列表（`mods/*.jar` 的 fabric.mod.json / mods.toml / mcmod.info）。
#[tauri::command]
pub async fn installed_mod_ids(
    game_dir: String,
    game_directory: Option<String>,
) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        let mods_dir = runtime.join("mods");
        let Ok(entries) = std::fs::read_dir(&mods_dir) else {
            return Ok(Vec::new());
        };
        let mut ids = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .map(|value| value.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if !name.ends_with(".jar") {
                continue;
            }
            if let Ok(meta) = launcher_core::mods::inspect_mod_jar(&path) {
                if let Some(id) = meta.mod_id.filter(|value| !value.trim().is_empty()) {
                    ids.push(id);
                }
            }
        }
        ids.sort();
        ids.dedup();
        Ok(ids)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 按 Modrinth 项目 id 批量取项目名/图标（前置列表用）。
/// 前置条目里要显示的信息：名字、简介、下载量、更新时间、来源。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefDetail {
    pub id: String,
    pub title: String,
    pub slug: String,
    pub icon: Option<String>,
    pub description: String,
    pub downloads: u64,
    pub updated: String,
    pub source: String,
}

/// 按来源批量取「前置项目」的展示信息。Modrinth 用字符串 id，CurseForge 用数字 modId。
/// 开发者头像：编译进程序里，不联网。
#[tauri::command]
pub fn developer_avatar(login: String) -> Option<String> {
    launcher_core::update::avatar_data(&login)
}

/// 检查更新：返回最新版本号、Release 地址与说明。
#[tauri::command]
pub async fn check_update(app: tauri::AppHandle) -> Result<launcher_core::update::ReleaseInfo, String> {
    let current = app.package_info().version.to_string();
    tauri::async_runtime::spawn_blocking(move || launcher_core::update::latest_release(&current))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn reference_details(source: String, ids: Vec<String>) -> Result<Vec<RefDetail>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        if source == "curseforge" {
            let nums: Vec<u64> = ids.iter().filter_map(|id| id.parse::<u64>().ok()).collect();
            let map = launcher_core::curseforge::project_meta(&nums).map_err(|e| e.to_string())?;
            for (id, meta) in map {
                out.push(RefDetail {
                    id: id.to_string(),
                    title: meta.name,
                    slug: meta.slug,
                    icon: meta.icon,
                    description: meta.summary,
                    downloads: meta.downloads,
                    updated: meta.updated,
                    source: "CurseForge".into(),
                });
            }
        } else {
            let map = launcher_core::modrinth::project_meta(&ids).map_err(|e| e.to_string())?;
            for (id, meta) in map {
                out.push(RefDetail {
                    id,
                    title: meta.title,
                    slug: meta.slug,
                    icon: meta.icon,
                    description: meta.description,
                    downloads: meta.downloads,
                    updated: meta.updated,
                    source: "Modrinth".into(),
                });
            }
        }
        out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        Ok(out)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 项目简介的中文翻译（MCIM 翻译库），没有就返回 null。
#[tauri::command]
pub async fn project_translation(source: String, id: String) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || launcher_core::modrinth::translation(&source, &id))
        .await
        .ok()
        .flatten()
}

#[tauri::command]
pub async fn dependency_projects(
    ids: Vec<String>,
) -> Result<std::collections::HashMap<String, launcher_core::modrinth::ProjectMeta>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher_core::modrinth::project_meta(&ids).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod text_decoding {
    use super::decode_text;

    #[test]
    fn utf8_stays_utf8() {
        assert_eq!(decode_text("日志 abc".as_bytes()), "日志 abc");
    }

    #[test]
    fn gbk_falls_back() {
        // 「月」的 GBK 两个字节，按 UTF-8 解不开——之前会显示成 `��`。
        let mut bytes = b"[0511".to_vec();
        bytes.extend_from_slice(&[0xD4, 0xC2]);
        bytes.extend_from_slice(b"2026 19:22:39.863]");
        assert_eq!(decode_text(&bytes), "[0511月2026 19:22:39.863]");
    }

    #[test]
    fn partial_head_is_not_mistaken_for_gbk() {
        let full = "日志".as_bytes();
        assert_eq!(decode_text(&full[1..]), "志");
    }
}


#[cfg(test)]
mod loader_label_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn labymod_label_uses_the_recorded_version() {
        let meta = json!({
            "libraries": [
                {"name": "net.labymod:dnsjava:4.0.0"},
                {"name": "net.labymod:labymod-api:3.3.4"}
            ],
            "_ybmcl": {"loaders": [{"loader": "labymod", "version": "50.3"}]}
        });
        let loaders = loaders_from_libraries(&meta);
        assert_eq!(loaders.len(), 1);
        assert_eq!(loaders[0].loader, "labymod");
        assert_eq!(loaders[0].version, "50.3");
    }

    #[test]
    fn neoforge_label_prefers_the_argument_version() {
        let meta = json!({
            "libraries": [
                {"name": "net.neoforged:neoforge:26.3.0.113"},
                {"name": "net.neoforged.fancymodloader:loader:12.0.12"}
            ],
            "arguments": {"game": ["--fml.neoForgeVersion", "26.3.0.113"]}
        });
        let loaders = loaders_from_libraries(&meta);
        assert!(loaders.iter().any(|item| item.loader == "neoforge"
            && item.version == "26.3.0.113"));
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    #[test]
    fn same_instance_can_start_more_than_once() {
        let mut sessions = Sessions::default();
        let first = sessions.begin("26.3-NeoForge");
        let second = sessions.begin("26.3-NeoForge");
        assert_ne!(first, second);
        assert_eq!(sessions.runs.len(), 2);
        // 还没挂进程，都算没在跑。
        assert_eq!(sessions.running(), 0);
        assert_eq!(sessions.last, Some(second));
    }

    #[test]
    fn finished_runs_are_pruned() {
        let mut sessions = Sessions::default();
        for _ in 0..25 {
            sessions.begin("26.3");
        }
        assert_eq!(sessions.runs.len(), 20);
    }
}

#[cfg(test)]
mod java_fix_tests {
    use super::*;

    fn line(text: &str) -> LogLine {
        LogLine {
            stream: "stderr".into(),
            text: text.into(),
        }
    }

    #[test]
    fn class_version_error_asks_for_a_newer_java() {
        let logs = vec![line(
            "java.lang.UnsupportedClassVersionError: net/minecraft/client/main/Main has been compiled by a more recent version of the Java Runtime (class file version 69.0)",
        )];
        assert!(matches!(java_fix(&logs), Some(JavaFix::Up(Some(25)))));
    }

    #[test]
    fn module_errors_ask_for_an_older_java() {
        for text in [
            "Unsupported JVM module path: java.base/java.lang.invoke",
            "java.lang.IllegalAccessError: module java.base does not \"opens java.lang\"",
        ] {
            assert!(matches!(java_fix(&[line(text)]), Some(JavaFix::Down)));
        }
    }

    #[test]
    fn jvm_crash_only_swaps_the_build() {
        let logs = vec![line(
            "# EXCEPTION_ACCESS_VIOLATION (0xc0000005) at pc=0x00007ffb",
        )];
        assert!(matches!(java_fix(&logs), Some(JavaFix::SameBuild)));
    }

    #[test]
    fn ordinary_crashes_do_not_blame_java() {
        let logs = vec![
            line("java.lang.ClassNotFoundException: cpw.mods.modlauncher.Launcher"),
            line("Error: Could not find or load main class net.minecraft.client.main.Main"),
        ];
        assert!(java_fix(&logs).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::loaders_from_libraries;

    #[test]
    fn legacy_fabric_is_not_reported_as_fabric() {
        let json = serde_json::json!({
            "libraries": [
                { "name": "net.legacyfabric:intermediary:1.12.2" },
                { "name": "net.fabricmc:fabric-loader:0.20.1" }
            ]
        });
        let loaders = loaders_from_libraries(&json);
        assert_eq!(loaders.len(), 1);
        assert_eq!(loaders[0].loader, "legacyfabric");
        assert_eq!(loaders[0].version, "0.20.1");
    }

    #[test]
    fn plain_fabric_still_reports_as_fabric() {
        let json = serde_json::json!({
            "libraries": [{ "name": "net.fabricmc:fabric-loader:0.20.1" }]
        });
        let loaders = loaders_from_libraries(&json);
        assert_eq!(loaders[0].loader, "fabric");
    }
}
