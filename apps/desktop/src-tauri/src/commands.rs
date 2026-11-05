//! 启动器命令层：实例管理 + 版本扫描 + Java 检测 + 启动游戏。
//!
//! 约定：所有命令都返回 `Result<T, String>`，前端直接把字符串当错误提示展示。

use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read};
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
use tauri::{AppHandle, Emitter, State};

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

#[derive(Default)]
pub struct Session {
    child: Option<Child>,
    logs: VecDeque<LogLine>,
    last_exit: Option<i32>,
}

#[derive(Clone, Default)]
pub struct GameRuntime(pub Arc<Mutex<Session>>);

fn publish(app: &AppHandle, runtime: &GameRuntime, line: LogLine) {
    if let Ok(mut session) = runtime.0.lock() {
        session.logs.push_back(line.clone());
        while session.logs.len() > MAX_LOG_LINES {
            session.logs.pop_front();
        }
    }
    let _ = app.emit("game-log", &line);
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
    /// 整合包信息；手动装的实例是 None。
    pub pack: Option<crate::versions::PackInfo>,
    /// 内存分配方式：0 自动、1 自定义、2 跟随全局。
    pub memory_type: u32,
    /// 自定义内存（MB），`memory_type == 1` 时生效。
    pub memory_custom_mb: u32,
    /// 初始堆（MB），0 表示和最大堆一样。
    pub memory_initial_mb: u32,
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
            ("net.fabricmc", "fabric-loader") => "fabric",
            ("org.quiltmc", "quilt-loader") => "quilt",
            ("com.mumfrey", "liteloader") => "liteloader",
            ("com.cleanroommc", "cleanroom") => "cleanroom",
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
        out.push(launcher_core::instance::LoaderChoice {
            loader: loader.to_owned(),
            version,
        });
    }
    out
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

#[tauri::command]
pub async fn list_versions(
    paths: State<'_, Paths>,
    runtime: State<'_, GameRuntime>,
) -> Result<Vec<VersionView>, String> {
    let game_dir = crate::game_folders::current(&paths);
    let data_dir = paths.data_dir().to_owned();
    let running = runtime
        .0
        .lock()
        .map(|session| session.child.is_some())
        .unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        let _ = running;
        versions_cached(&game_dir, &data_dir)
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
                pack: settings.pack.clone(),
                memory_type: settings.memory_type,
                memory_custom_mb: settings.memory_custom_mb,
                memory_initial_mb: settings.memory_initial_mb,
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
) -> Result<launcher_core::LaunchRequest, String> {
    let version = crate::versions::load_instance(game_dir, id);
    let global = crate::settings::load(paths);
    // 实例里的玩家名已取消，统一用全局设置里那个。
    let username = global.username.clone();
    let memory = match version.memory_type {
        0 => auto_memory_mb(game_dir, id, version.isolated, memory_info_now()),
        1 => version.memory_custom_mb.max(crate::versions::MIN_MEMORY_MB),
        _ => global.memory_mb,
    };
    let java_path = if !version.java_path.is_empty() {
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
        1 => {
            return Err(
                "这个实例限了「仅正版」，启动器现在只有离线身份，把「限制验证方式」改成不限制或仅离线模式才能启动。"
                    .into(),
            )
        }
        3 => {
            return Err(
                "这个实例限了「仅第三方登录」，现在还没有第三方登录账户，把「限制验证方式」改成不限制或仅离线模式才能启动。"
                    .into(),
            )
        }
        _ => {}
    }
    if !version.ignore_java_warning {
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
        access_token: None,
        uuid: None,
        version_type: Some(version.version_type.clone()).filter(|text| !text.trim().is_empty()),
        jvm_args: split_args(&version.jvm_args),
        game_args,
        classpath_head: split_args(&version.classpath_head),
        skip_file_check: version.skip_file_check,
        allow_java_upgrade: true,
        log4j_debug: version.debug_log4j,
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
    let request = launch_request(paths, &game_dir, id)?;
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
        let text = String::from_utf8_lossy(&bytes);
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
pub async fn detect_java() -> Result<Vec<JavaInstallation>, String> {
    tauri::async_runtime::spawn_blocking(launcher_core::detect_java)
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
    // PCL 的 JavaSorter：8/17/21 权重更高，同权重再挑大版本。
    matching.sort_by_key(|java| (launcher_core::java_weight(java.major), java.major));
    match matching.last() {
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
    launcher_core::download::set_speed_limit(launcher_core::download::tier_limit(
        global.download_speed,
    ));
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

/// 启动一个版本。配置来自 `version-settings.json`，缺省字段用全局设置补。
#[tauri::command]
pub async fn launch_version(
    id: String,
    app: AppHandle,
    runtime: State<'_, GameRuntime>,
    paths: State<'_, crate::portable::Paths>,
) -> Result<u32, String> {
    let runtime = runtime.inner().clone();
    let log_dir = paths.log_dir();
    let game_dir = crate::game_folders::current(&paths);
    let (request, strategy) = {
        let request = launch_request(&paths, &game_dir, &id)?;
        let global = crate::settings::load(&paths);
        (
            request,
            strategy_of(global.download_strategy.as_deref()),
        )
    };
    let launch_dir = game_dir.clone();
    let pre_dir = launch_dir.clone();
    let launch_id = id.clone();
    let launched = tauri::async_runtime::spawn_blocking(move || {
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
        let mut session = runtime.0.lock().map_err(|e| e.to_string())?;
        if session.child.is_some() {
            return Err("已有游戏进程正在运行".into());
        }

        // 实例的「启动前执行命令」：在游戏目录里跑；失败就不启动。
        let settings = crate::versions::load_instance(&pre_dir, &version_id);
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
        let mut command = Command::new(&plan.java_path);
        command
            .args(&plan.args)
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
        if !settings.window_title.trim().is_empty() {
            let title = settings.window_title.clone();
            std::thread::spawn(move || set_game_window_title(pid, title));
        }
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        session.logs.clear();
        session.last_exit = None;
        session.child = Some(child);
        drop(session);

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
            std::thread::spawn(move || {
                let mut reader = BufReader::new(pipe);
                loop {
                    let mut bytes = Vec::new();
                    match reader.by_ref().take(16_384).read_until(b'\n', &mut bytes) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => publish(
                            &app,
                            &runtime,
                            LogLine {
                                stream: stream.into(),
                                text: String::from_utf8_lossy(&bytes).trim_end().to_string(),
                            },
                        ),
                    }
                }
            });
        }

        let app_exit = app.clone();
        let runtime_exit = runtime.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(300));
                let Ok(mut session) = runtime_exit.0.lock() else {
                    break;
                };
                let Some(child) = session.child.as_mut() else {
                    break;
                };
                match child.try_wait() {
                    Ok(Some(status)) => {
                        session.last_exit = status.code();
                        session.child = None;
                        drop(session);
                        publish(
                            &app_exit,
                            &runtime_exit,
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
                        let _ = app_exit.emit("game-exit", status.code());
                        break;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        drop(session);
                        publish(
                            &app_exit,
                            &runtime_exit,
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
pub fn game_snapshot(runtime: State<'_, GameRuntime>) -> Result<GameSnapshot, String> {
    let session = runtime.0.lock().map_err(|e| e.to_string())?;
    Ok(GameSnapshot {
        running: session.child.is_some(),
        pid: session.child.as_ref().map(Child::id),
        last_exit: session.last_exit,
        logs: session.logs.iter().cloned().collect(),
    })
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
                name,
                java_path,
                strategy,
                threads: None,
                icon_url,
            },
            token,
            move |step: &launcher_core::modpack::Step| {
                let _ = emitter.emit(
                    "modpack-progress",
                    serde_json::json!({
                        "stage": step.stage,
                        "done": step.done,
                        "total": step.total,
                        "bytes": step.bytes,
                        "file": step.file,
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
    tauri::async_runtime::spawn_blocking(move || {
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
    .map_err(|error| error.to_string())
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
#[tauri::command]
pub async fn fetch_icon(url: String) -> Result<Option<String>, String> {
    if !url.starts_with("https://") {
        return Err("Invalid input: icon url must be https".into());
    }
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        launcher_core::download::fetch_file_bytes(&url, &CancellationToken::default())
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    match launcher_core::icons::inspect(&bytes) {
        Some((_mime, true)) => Ok(None),
        Some((mime, false)) => Ok(Some(format!(
            "data:image/{mime};base64,{}",
            launcher_core::icons::base64(&bytes)
        ))),
        None => Ok(None),
    }
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
async fn write_with_progress(
    app: AppHandle,
    url: String,
    path: String,
    task_id: Option<u32>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let emitter = app.clone();
        launcher_core::packages::download_file_to_progress(
            &url,
            std::path::Path::new(&path),
            &launcher_core::download::CancellationToken::default(),
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
    .map_err(|error| error.to_string())?
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

#[tauri::command]
pub async fn install_game_version(
    app: AppHandle,
    runtime: State<'_, InstallRuntime>,
    install_id: Option<u64>,
    request: InstallRequest,
) -> Result<InstallSummary, String> {
    // 前端任务卡的 id 用不上时（版本页那种一次性安装）发一个匿名号，照样互不干扰。
    let install_id = install_id.unwrap_or_else(next_anonymous_install);
    let token = runtime.begin(install_id)?;
    let runtime_inner = runtime.inner().clone();
    // 装完要登记产出（任务卡的「打开文件夹 / 移除并删除文件」），得先留一份游戏目录。
    let version_dir = request.game_dir.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        let emitter = app.clone();
        install_version(request, token, move |progress: InstallProgress| {
            let _ = emitter.emit("install-progress", &progress);
        })
        .map_err(|e| e.to_string())
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
    result
}

#[tauri::command]
pub async fn install_game_modloader(
    app: AppHandle,
    runtime: State<'_, InstallRuntime>,
    install_id: Option<u64>,
    mut request: ModLoaderInstallRequest,
) -> Result<InstallSummary, String> {
    // 前端任务卡的 id 用不上时（版本页那种一次性安装）发一个匿名号，照样互不干扰。
    let install_id = install_id.unwrap_or_else(next_anonymous_install);
    let token = runtime.begin(install_id)?;
    let runtime_inner = runtime.inner().clone();
    // request 要搬进后台线程，装完登记产出还得知道游戏目录在哪。
    let loader_dir = request.game_dir.clone();

    {
        let loaders = request_loaders(&request);
        let requirement =
            launcher_core::java::requirement_with(&request.game_version, None, &loaders);
        let current = launcher_core::detect_java()
            .into_iter()
            .find(|java| java.path.eq_ignore_ascii_case(&request.java_path))
            .map(|java| java.major);
        if current.is_none_or(|major| !requirement.accepts(major)) {
            request.java_path = pick_java_for(requirement)?;
        }
    }

    let result = tauri::async_runtime::spawn_blocking(move || {
        let emitter = app.clone();
        install_modloader(request, token, move |progress: InstallProgress| {
            let _ = emitter.emit("install-progress", &progress);
        })
        .map_err(|e| e.to_string())
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
    result
}

#[tauri::command]
pub async fn install_game_instance(
    app: AppHandle,
    runtime: State<'_, InstallRuntime>,
    install_id: Option<u64>,
    request: InstanceRequest,
) -> Result<InstallSummary, String> {
    // 前端任务卡的 id 用不上时（版本页那种一次性安装）发一个匿名号，照样互不干扰。
    let install_id = install_id.unwrap_or_else(next_anonymous_install);
    let token = runtime.begin(install_id)?;
    let runtime_inner = runtime.inner().clone();
    // 暂停时半成品就是这个目录；装成功或用户取消都要把登记撤掉。
    let instance_path = launcher_core::instance::instance_target(&request);
    let probe = token.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        let emitter = app.clone();
        install_instance(request, token, move |progress: InstallProgress| {
            let _ = emitter.emit("install-progress", &progress);
        })
        .map_err(|e| e.to_string())
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
    runtime: State<'_, InstallRuntime>,
    install_id: Option<u64>,
    request: launcher_core::modpack::ModpackRequest,
) -> Result<launcher_core::modpack::ModpackSummary, String> {
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

    let result = tauri::async_runtime::spawn_blocking(move || {
        let emitter = app.clone();
        launcher_core::modpack::install_modpack(
            request,
            token,
            move |step: &launcher_core::modpack::Step| {
                let _ = emitter.emit(
                    "modpack-progress",
                    serde_json::json!({
                        "stage": step.stage,
                        "done": step.done,
                        "total": step.total,
                        "bytes": step.bytes,
                        "file": step.file,
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
        }
        Err(_) if probe.is_paused() => crate::sweep::mark_paused(&staging),
        Err(_) => crate::sweep::clear_paused(&staging),
    }
    result
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
    launcher_core::download::set_speed_limit(launcher_core::download::tier_limit(
        saved.download_speed,
    ));
    Ok(saved)
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

#[tauri::command]
pub async fn check_mod_updates(
    game_dir: String,
    game_directory: Option<String>,
    game_version: String,
    loader: String,
) -> Result<Vec<launcher_core::mod_updates::ModUpdate>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        launcher_core::mod_updates::check(&runtime, &game_version, &loader)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn apply_mod_update(
    game_dir: String,
    game_directory: Option<String>,
    update: launcher_core::mod_updates::ModUpdate,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let runtime = runtime_of(game_dir, game_directory)?;
        launcher_core::mod_updates::apply(&runtime, &update).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
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
            "game-cache",
            "下载缓存",
            "游戏目录下的 cache（PCL 等启动器留下的 SHA-1 缓存），删掉下次会重新下载",
            &game_dir.join("cache"),
        );
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

/// 读文本文件（日志、崩溃报告用）；超过 `limit` 字节只留开头。
#[tauri::command]
pub fn read_text_file(path: String, limit: Option<usize>) -> Result<String, String> {
    let file = PathBuf::from(&path);
    let max = limit.unwrap_or(512 * 1024);
    let mut bytes = Vec::new();
    std::fs::File::open(&file)
        .map_err(|error| format!("{path}: {error}"))?
        .take(max as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{path}: {error}"))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
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
