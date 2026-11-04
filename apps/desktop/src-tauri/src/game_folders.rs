//! 游戏目录表：版本页左栏的那一串文件夹。
//!
//! 两个目录是扫描出来的、不需要落设置：exe 同级的 `.minecraft`（「当前文件夹」）和
//! 官方启动器的 `%APPDATA%\.minecraft`（「官方启动器文件夹」）。用户「添加已有文件夹」
//! 的目录，以及被改过名的目录，才写进 `settings.json` 的 `gameFolders`。
//!
//! 显示顺序存在 `folderOrder`（绝对路径），改名的目录用 `name` 覆盖默认名字；
//! 当前选中的目录存在 `activeGameFolder`，所有下载/安装/启动都落在它里面。

use std::path::{Path, PathBuf};

use serde::Serialize;
use crate::portable::Paths;
use crate::settings::{self, FolderEntry, MAX_FOLDER_NAME};

pub const CURRENT_LABEL: &str = "当前文件夹";
pub const OFFICIAL_LABEL: &str = "官方启动器文件夹";

/// 一条目录，给前端左栏直接渲染。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameFolderView {
    pub path: String,
    pub name: String,
    /// `current` = 启动器自己的 `.minecraft`，`official` = 官方启动器目录，`custom` = 用户添加。
    pub kind: String,
    /// 目录还在不在（被挪走/删掉时为 false）。
    pub exists: bool,
    /// 是不是当前选中的目录。
    pub active: bool,
}

/// 去掉末尾分隔符与首尾空白；空串原样返回。
pub fn cleaned(path: &str) -> String {
    path.trim().trim_end_matches(['\\', '/']).to_owned()
}

/// 比较用的键：Windows 下路径不区分大小写，统一小写再比。
fn key(path: &str) -> String {
    cleaned(path).to_lowercase()
}

/// 目录的最后一段名字；末段是 `.minecraft` 时取上一级（PCL 同款规则）。
pub fn default_name(path: &str) -> String {
    let cleaned_path = cleaned(path);
    let path = Path::new(&cleaned_path);
    let mut name = path
        .file_name()
        .map(|part| part.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.eq_ignore_ascii_case(".minecraft") {
        if let Some(parent) = path.parent().and_then(|parent| parent.file_name()) {
            name = parent.to_string_lossy().into_owned();
        }
    }
    if name.is_empty() {
        name = cleaned(path.to_string_lossy().as_ref());
    }
    name.chars().take(MAX_FOLDER_NAME).collect()
}

fn official_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let dir = Path::new(&appdata).join(".minecraft");
    dir.is_dir().then_some(dir)
}

/// 当前游戏目录：设置的选中项，没设过就是 exe 同级 `.minecraft`。
pub fn current(paths: &Paths) -> PathBuf {
    let settings = settings::load(paths);
    let active = cleaned(&settings.active_game_folder);
    if active.is_empty() {
        return paths.game_dir();
    }
    PathBuf::from(active)
}

/// 当前目录（字符串形式），命令层用得最多。
pub fn current_string(paths: &Paths) -> String {
    current(paths).to_string_lossy().into_owned()
}

/// 给游戏目录补上 `launcher_profiles.json`：Forge 系的官方安装器找不到这个文件会直接退出。
///
/// 文件已存在（官方启动器用户自己有一份）就一个字都不动；只在缺失时写一个空表。
pub fn ensure_launcher_profiles(dir: &Path) {
    let path = dir.join("launcher_profiles.json");
    if path.exists() || !dir.is_dir() {
        return;
    }
    let _ = std::fs::write(&path, b"{\"profiles\":{}}");
}

/// 扫出来的两个固定目录 + 用户添加的目录，按保存的顺序排。
pub fn list(paths: &Paths) -> Vec<GameFolderView> {
    let settings = settings::load(paths);
    let active = key(&current_string(paths));

    let mut items: Vec<GameFolderView> = Vec::new();
    let mut push = |path: String, kind: &str, default_label: &str| {
        let keyed = key(&path);
        if items.iter().any(|item| key(&item.path) == keyed) {
            return;
        }
        let stored = settings
            .game_folders
            .iter()
            .find(|entry| key(&entry.path) == keyed);
        let name = match stored.map(|entry| entry.name.trim()) {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => default_label.to_owned(),
        };
        let exists = Path::new(&path).is_dir();
        if exists {
            // 顺手补齐官方安装器要的 `launcher_profiles.json`（只在缺失时写）。
            ensure_launcher_profiles(Path::new(&path));
        }
        items.push(GameFolderView {
            path: cleaned(&path),
            name,
            kind: kind.to_owned(),
            exists,
            active: keyed == active,
        });
    };

    push(
        paths.game_dir().to_string_lossy().into_owned(),
        "current",
        CURRENT_LABEL,
    );
    if let Some(dir) = official_dir() {
        push(
            dir.to_string_lossy().into_owned(),
            "official",
            OFFICIAL_LABEL,
        );
    }
    for entry in &settings.game_folders {
        if !entry.added {
            continue;
        }
        let label = default_name(&entry.path);
        push(entry.path.clone(), "custom", &label);
    }

    // 按保存的顺序排；没在顺序表里的留在原位（追加在已知项后面）。
    if !settings.folder_order.is_empty() {
        let order: Vec<String> = settings.folder_order.iter().map(|path| key(path)).collect();
        items.sort_by_key(|item| {
            order
                .iter()
                .position(|path| *path == key(&item.path))
                .unwrap_or(usize::MAX)
        });
    }
    items
}

/// 把一条目录写进设置（已存在则更新名字/added）。
fn upsert(settings_value: &mut settings::Settings, path: &str, name: &str, added: bool) {
    let keyed = key(path);
    if let Some(entry) = settings_value
        .game_folders
        .iter_mut()
        .find(|entry| key(&entry.path) == keyed)
    {
        entry.name = name.to_owned();
        entry.added = entry.added || added;
        return;
    }
    settings_value.game_folders.push(FolderEntry {
        path: cleaned(path),
        name: name.to_owned(),
        added,
    });
}

/// 选目录时用：没有 `versions\` 就往下一层找一个带 `versions\` 的子目录。
fn resolve_folder(target: &Path) -> Result<PathBuf, String> {
    if !target.is_dir() {
        return Err(format!("{} 不是一个文件夹", target.display()));
    }
    if target.join("versions").is_dir() {
        return Ok(target.to_path_buf());
    }
    let mut children: Vec<PathBuf> = std::fs::read_dir(target)
        .map_err(|error| format!("读不了 {}：{error}", target.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("versions").is_dir())
        .collect();
    children.sort();
    match children.into_iter().next() {
        Some(dir) => Ok(dir),
        None => Err(format!(
            "这个文件夹里没有 versions 目录：{}",
            target.display()
        )),
    }
}

#[tauri::command]
pub fn list_game_folders(paths: tauri::State<'_, Paths>) -> Vec<GameFolderView> {
    list(&paths)
}

/// 添加已有文件夹：校验 → 没有 versions 就往下钻一层 → 去重 → 设为当前目录。
#[tauri::command]
pub fn add_game_folder(
    paths: tauri::State<'_, Paths>,
    path: String,
) -> Result<Vec<GameFolderView>, String> {
    let resolved = resolve_folder(Path::new(&cleaned(&path)))?;
    let path = resolved.to_string_lossy().into_owned();
    let mut settings_value = settings::load(&paths);
    let known = list(&paths)
        .into_iter()
        .any(|item| key(&item.path) == key(&path));
    if !known {
        upsert(&mut settings_value, &path, "", true);
        if !settings_value
            .folder_order
            .iter()
            .any(|ordered| key(ordered) == key(&path))
        {
            settings_value.folder_order.push(cleaned(&path));
        }
    }
    settings_value.active_game_folder = cleaned(&path);
    settings::save(&paths, settings_value)?;
    Ok(list(&paths))
}

/// 移出列表：只对用户添加的目录生效（自动扫出来的目录下次扫描还会回来）。
#[tauri::command]
pub fn remove_game_folder(
    paths: tauri::State<'_, Paths>,
    path: String,
) -> Result<Vec<GameFolderView>, String> {
    let keyed = key(&path);
    let mut settings_value = settings::load(&paths);
    let before = settings_value.game_folders.len();
    settings_value
        .game_folders
        .retain(|entry| !(entry.added && key(&entry.path) == keyed));
    if settings_value.game_folders.len() == before {
        return Err("这个目录是自动扫出来的，不能移出列表".into());
    }
    settings_value
        .folder_order
        .retain(|ordered| key(ordered) != keyed);
    if key(&settings_value.active_game_folder) == keyed {
        settings_value.active_game_folder.clear();
    }
    settings::save(&paths, settings_value)?;
    let _ = crate::categories::forget_dir(paths.data_dir(), &path);
    Ok(list(&paths))
}

/// 重命名；`name` 为空串 = 复原默认名字。
#[tauri::command]
pub fn rename_game_folder(
    paths: tauri::State<'_, Paths>,
    path: String,
    name: String,
) -> Result<Vec<GameFolderView>, String> {
    let name: String = name.trim().chars().take(MAX_FOLDER_NAME).collect();
    let mut settings_value = settings::load(&paths);
    let known = list(&paths)
        .into_iter()
        .any(|item| key(&item.path) == key(&path));
    if !known {
        return Err(format!("列表里没有这个目录：{path}"));
    }
    upsert(&mut settings_value, &path, &name, false);
    if name.is_empty() {
        // 复原名字：自动目录不必留在表里，用户添加的要留着（只是为了名字）。
        settings_value.game_folders.retain(|entry| {
            !(key(&entry.path) == key(&path) && entry.name.is_empty() && !entry.added)
        });
    }
    settings::save(&paths, settings_value)?;
    Ok(list(&paths))
}

/// 拖动排序后保存显示顺序。
#[tauri::command]
pub fn set_folder_order(
    paths: tauri::State<'_, Paths>,
    order: Vec<String>,
) -> Result<Vec<GameFolderView>, String> {
    let mut settings_value = settings::load(&paths);
    settings_value.folder_order = order;
    settings::save(&paths, settings_value)?;
    Ok(list(&paths))
}

/// 切换当前游戏目录。
#[tauri::command]
pub fn set_active_game_folder(
    paths: tauri::State<'_, Paths>,
    path: String,
) -> Result<Vec<GameFolderView>, String> {
    let keyed = key(&path);
    if !list(&paths).into_iter().any(|item| key(&item.path) == keyed) {
        return Err(format!("列表里没有这个目录：{path}"));
    }
    let mut settings_value = settings::load(&paths);
    settings_value.active_game_folder = cleaned(&path);
    settings::save(&paths, settings_value)?;
    Ok(list(&paths))
}

/// 弹系统选目录框，返回选中的路径（取消返回 None）。
#[tauri::command]
pub fn pick_game_folder(app: tauri::AppHandle, paths: tauri::State<'_, Paths>) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let start = current(&paths);
    let picked = app
        .dialog()
        .file()
        .set_directory(start)
        .blocking_pick_folder()?;
    let path = picked.into_path().ok()?;
    Some(path.to_string_lossy().into_owned())
}

/// 启动时用：把自动目录补进 settings 的顺序表里，别让用户拖过的顺序每次都被清空。
pub fn remember_known(paths: &Paths) {
    let found = list(paths);
    let mut settings_value = settings::load(paths);
    let mut changed = false;
    for item in &found {
        if !settings_value
            .folder_order
            .iter()
            .any(|ordered| key(ordered) == key(&item.path))
        {
            settings_value.folder_order.push(item.path.clone());
            changed = true;
        }
    }
    if !settings_value.active_game_folder.is_empty()
        && !found
            .iter()
            .any(|item| key(&item.path) == key(&settings_value.active_game_folder))
    {
        settings_value.active_game_folder.clear();
        changed = true;
    }
    if changed {
        let _ = settings::save(paths, settings_value);
    }
}
