//! 版本分类：每个游戏目录各一套，存在数据目录的 `categories.json` 里。
//!
//! 结构照收藏夹（`favorites.rs`）：一个目录一套分类列表，另有一张
//! 「版本 id → 分类 id 列表」的表。一个版本可以同时属于多个分类。
//! 删除分类只删分类本身，版本上的归类记录一起清，版本不受影响。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 分类名长度上限。
pub const MAX_NAME: usize = 20;
const SCHEMA_VERSION: u32 = 1;

/// 「标星实例」是内置分类：id 固定、名字固定，不能改名也不能删。
///
/// 星标就存在它的 tag 里（`tags[版本 id]` 含 `STARRED_ID` 就是打了星），
/// 这样星标和分类共用 `categories.json`，跟着游戏目录走。
pub const STARRED_ID: &str = "starred";
pub const STARRED_NAME: &str = "标星实例";

fn starred_category() -> Category {
    Category {
        id: STARRED_ID.to_owned(),
        name: STARRED_NAME.to_owned(),
        created_at: 0,
    }
}

/// 保证这一目录的分类表里有「标星实例」，缺了就补到最前面。
fn ensure_starred(dir: &mut DirCategories) -> bool {
    if dir.categories.iter().any(|category| category.id == STARRED_ID) {
        return false;
    }
    dir.categories.insert(0, starred_category());
    true
}

const FILE: &str = "categories.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub created_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DirCategories {
    pub categories: Vec<Category>,
    /// 版本 id → 分类 id（只记非空的）。
    pub tags: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Store {
    pub schema_version: u32,
    /// 游戏目录绝对路径（小写）→ 这个目录的分类。
    pub dirs: BTreeMap<String, DirCategories>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            dirs: BTreeMap::new(),
        }
    }
}

/// 给前端的一份快照：当前目录的分类 + 版本归类。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorySnapshot {
    pub categories: Vec<Category>,
    pub tags: BTreeMap<String, Vec<String>>,
}

fn store_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// 给前端的一份快照：当前目录的分类 + 版本归类。
///
/// 读的时候顺手把内置的「标星实例」补进 `categories.json`，这样它跟普通分类
/// 一样有落盘记录（用户要求「标星实例写进 categories.json」）。
pub fn snapshot(data_dir: &Path, game_dir: &str) -> CategorySnapshot {
    let mut store = load(data_dir);
    let key = dir_key(game_dir);
    let mut dir = store.dirs.remove(&key).unwrap_or_default();
    if ensure_starred(&mut dir) {
        store.dirs.insert(key, dir.clone());
        let _ = save(data_dir, &store);
    }
    CategorySnapshot {
        categories: dir.categories,
        tags: dir.tags,
    }
}

/// 目录键：小写、去末尾分隔符，Windows 下不区分大小写。
pub fn dir_key(game_dir: &str) -> String {
    game_dir
        .trim()
        .trim_end_matches(['\\', '/'])
        .to_lowercase()
}

pub fn load(data_dir: &Path) -> Store {
    let Ok(text) = fs::read_to_string(store_path(data_dir)) else {
        return Store::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save(data_dir: &Path, store: &Store) -> Result<(), String> {
    fs::create_dir_all(data_dir).map_err(|e| format!("{}: {e}", data_dir.display()))?;
    let target = store_path(data_dir);
    let temp = target.with_extension("json.tmp");
    // 老文件里没有版本号（默认 0），写回时一律按当前版本落盘。
    let store = Store {
        schema_version: SCHEMA_VERSION,
        dirs: store.dirs.clone(),
    };
    let text = serde_json::to_string_pretty(&store).map_err(|e| e.to_string())?;
    fs::write(&temp, text).map_err(|e| format!("{}: {e}", temp.display()))?;
    fs::rename(&temp, &target).map_err(|e| format!("{}: {e}", target.display()))
}

fn entry_mut<'a>(store: &'a mut Store, game_dir: &str) -> &'a mut DirCategories {
    let key = dir_key(game_dir);
    let dir = store.dirs.entry(key).or_default();
    ensure_starred(dir);
    dir
}

fn clean_name(name: &str) -> String {
    name.trim()
        .chars()
        .filter(|c| *c != '\n' && *c != '\r')
        .take(MAX_NAME)
        .collect()
}

/// 新建分类；同目录下重名直接报错（与收藏夹一致）。
pub fn create(data_dir: &Path, game_dir: &str, name: &str) -> Result<CategorySnapshot, String> {
    let name = clean_name(name);
    if name.is_empty() {
        return Err("分类名不能是空的".into());
    }
    let mut store = load(data_dir);
    let dir = entry_mut(&mut store, game_dir);
    if name.eq_ignore_ascii_case(STARRED_NAME) {
        return Err(format!("「{STARRED_NAME}」是内置分类，换个名字吧"));
    }
    if dir
        .categories
        .iter()
        .any(|category| category.name.eq_ignore_ascii_case(&name))
    {
        return Err(format!("已经有这个分类了：{name}"));
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or_default();
    let id = format!("c{stamp:x}");
    dir.categories.push(Category {
        id,
        name,
        created_at: stamp,
    });
    save(data_dir, &store)?;
    Ok(snapshot(data_dir, game_dir))
}

pub fn rename(
    data_dir: &Path,
    game_dir: &str,
    id: &str,
    name: &str,
) -> Result<CategorySnapshot, String> {
    let name = clean_name(name);
    if name.is_empty() {
        return Err("分类名不能是空的".into());
    }
    if id == STARRED_ID {
        return Err(format!("「{STARRED_NAME}」是内置分类，不能改名"));
    }
    if name.eq_ignore_ascii_case(STARRED_NAME) {
        return Err(format!("「{STARRED_NAME}」是内置分类，换个名字吧"));
    }
    let mut store = load(data_dir);
    let dir = entry_mut(&mut store, game_dir);
    if dir
        .categories
        .iter()
        .any(|category| category.id != id && category.name.eq_ignore_ascii_case(&name))
    {
        return Err(format!("已经有这个分类了：{name}"));
    }
    let Some(category) = dir.categories.iter_mut().find(|category| category.id == id) else {
        return Err("没有这个分类".into());
    };
    category.name = name;
    save(data_dir, &store)?;
    Ok(snapshot(data_dir, game_dir))
}

pub fn delete(data_dir: &Path, game_dir: &str, id: &str) -> Result<CategorySnapshot, String> {
    if id == STARRED_ID {
        return Err(format!("「{STARRED_NAME}」是内置分类，不能删除"));
    }
    let mut store = load(data_dir);
    let dir = entry_mut(&mut store, game_dir);
    dir.categories.retain(|category| category.id != id);
    for ids in dir.tags.values_mut() {
        ids.retain(|tag| tag != id);
    }
    dir.tags.retain(|_, ids| !ids.is_empty());
    save(data_dir, &store)?;
    Ok(snapshot(data_dir, game_dir))
}

/// 设置某个版本属于哪些分类（传空数组 = 取消全部归类）。
pub fn set_version(
    data_dir: &Path,
    game_dir: &str,
    version: &str,
    ids: Vec<String>,
) -> Result<CategorySnapshot, String> {
    let mut store = load(data_dir);
    let dir = entry_mut(&mut store, game_dir);
    let known: Vec<String> = dir
        .categories
        .iter()
        .map(|category| category.id.clone())
        .collect();
    let mut cleaned: Vec<String> = Vec::new();
    for id in ids {
        if known.contains(&id) && !cleaned.contains(&id) {
            cleaned.push(id);
        }
    }
    if cleaned.is_empty() {
        dir.tags.remove(version);
    } else {
        dir.tags.insert(version.to_owned(), cleaned);
    }
    save(data_dir, &store)?;
    Ok(snapshot(data_dir, game_dir))
}

/// 实例改名的副产物：把旧的归属搬到新名字上。
pub fn rename_tag(
    data_dir: &Path,
    game_dir: &str,
    from: &str,
    to: &str,
) -> Result<CategorySnapshot, String> {
    let mut store = load(data_dir);
    let dir = entry_mut(&mut store, game_dir);
    if let Some(ids) = dir.tags.remove(from) {
        dir.tags.insert(to.to_owned(), ids);
        save(data_dir, &store)?;
    }
    Ok(snapshot(data_dir, game_dir))
}

/// 打星 / 取消打星：就是给「标星实例」这条内置分类加 tag / 去 tag。
pub fn set_starred(
    data_dir: &Path,
    game_dir: &str,
    version: &str,
    on: bool,
) -> Result<CategorySnapshot, String> {
    let mut store = load(data_dir);
    let dir = entry_mut(&mut store, game_dir);
    let mut ids = dir.tags.get(version).cloned().unwrap_or_default();
    if on {
        if !ids.iter().any(|id| id == STARRED_ID) {
            ids.insert(0, STARRED_ID.to_owned());
        }
    } else {
        ids.retain(|id| id != STARRED_ID);
    }
    if ids.is_empty() {
        dir.tags.remove(version);
    } else {
        dir.tags.insert(version.to_owned(), ids);
    }
    save(data_dir, &store)?;
    Ok(snapshot(data_dir, game_dir))
}

/// 老版本把星标存在 localStorage 里：第一次进某个目录时把这些 id 并进来。
///
/// 只收目录里还存在的实例（不存在的直接丢，免得表里堆幽灵 id）。
pub fn import_starred(
    data_dir: &Path,
    game_dir: &str,
    versions: Vec<String>,
) -> Result<CategorySnapshot, String> {
    let mut store = load(data_dir);
    let dir = entry_mut(&mut store, game_dir);
    let root = Path::new(game_dir).join("versions");
    let mut changed = false;
    for version in versions {
        if !root.join(&version).is_dir() {
            continue;
        }
        let ids = dir.tags.entry(version).or_default();
        if !ids.iter().any(|id| id == STARRED_ID) {
            ids.insert(0, STARRED_ID.to_owned());
            changed = true;
        }
    }
    if changed {
        save(data_dir, &store)?;
    }
    Ok(snapshot(data_dir, game_dir))
}

/// 实例被删掉时清它的分类归属（否则同名新实例会继承旧分类和星标）。
pub fn forget_version(data_dir: &Path, game_dir: &str, version: &str) {
    let mut store = load(data_dir);
    let dir = entry_mut(&mut store, game_dir);
    if dir.tags.remove(version).is_some() {
        let _ = save(data_dir, &store);
    }
}

/// 目录被移出列表时，把它的分类一起清掉（避免越攒越多）。
pub fn forget_dir(data_dir: &Path, game_dir: &str) -> io::Result<()> {
    let mut store = load(data_dir);
    if store.dirs.remove(&dir_key(game_dir)).is_some() {
        save(data_dir, &store).map_err(|error| io::Error::other(error))?;
    }
    Ok(())
}
