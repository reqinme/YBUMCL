//! 收藏夹：存在数据目录的 `favorites.json`，跟实例数据一起搬走。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 收藏夹里的一条内容（来自 Modrinth 或未来的其它来源）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteEntry {
    pub project_id: String,
    pub title: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    /// mods / resourcepacks / shaders / datapacks ……
    #[serde(default)]
    pub kind: String,
    /// 加入时间（毫秒时间戳），列表默认按它倒序。
    #[serde(default)]
    pub added_at: i64,
    /// 这个项目支持的游戏版本（逐个版本存，不并成大版本），收藏夹里按版本筛选用。
    #[serde(default)]
    pub versions: Vec<String>,
    /// 支持的加载器显示名（Fabric / Forge ……），收藏夹里按加载器筛选用。
    #[serde(default)]
    pub loaders: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteFolder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub entries: Vec<FavoriteEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Favorites {
    #[serde(default)]
    pub folders: Vec<FavoriteFolder>,
}

const FILE: &str = "favorites.json";

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// 读收藏夹；文件不存在或损坏时给一个空结构，不影响启动。
pub fn load(data_dir: &Path) -> Favorites {
    match fs::read_to_string(path(data_dir)) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Favorites::default(),
    }
}

/// 写收藏夹：先写临时文件再改名，避免写一半断电把文件写坏。
pub fn save(data_dir: &Path, favorites: &Favorites) -> io::Result<()> {
    fs::create_dir_all(data_dir)?;
    let target = path(data_dir);
    let temp = target.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(favorites)
        .map_err(|error| io::Error::other(error.to_string()))?;
    fs::write(&temp, text)?;
    fs::rename(&temp, &target)
}
