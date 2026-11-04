//! 半成品的收尾：暂停登记的清理表、退出时删、启动时扫。
//!
//! 两条路分开记：
//! - **退出清理**：只在这一次进程里有效。用户点了「暂停」就把实例目录（或整合包的
//!   `.ybmcl/modpack`）记进来，正常关窗口时删掉，不留孤儿。
//! - **启动清扫**：硬杀（`taskkill /f`、崩溃、断电）跑不到退出清理，所以每次启动
//!   按「带我们标记就删」扫一遍：`versions/<名字>` 里有 `.ybumcl-version.json`
//!   （版本还没下完）或 `.pclignore`（实例还没装完）就整目录删掉；`.ybmcl/modpack`
//!   与 `.ybumcl-modloader-*` 这两类暂存目录天生一次性，见到就删。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use launcher_core::merge::IGNORE_FILE;

/// 版本还没下完时，`version_id` 目录里只有这个暂存 json。
const PENDING_FILE: &str = ".ybumcl-version.json";

fn paused() -> &'static Mutex<HashSet<String>> {
    static PAUSED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    PAUSED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn key(path: &Path) -> String {
    path.to_string_lossy().to_ascii_lowercase()
}

/// 暂停了哪个半成品：关窗口时删它。
pub fn mark_paused(path: &Path) {
    if let Ok(mut paused) = paused().lock() {
        paused.insert(key(path));
    }
}

/// 装了 / 取消了 / 又要重装了：这条不用等退出再删。
pub fn clear_paused(path: &Path) {
    if let Ok(mut paused) = paused().lock() {
        paused.remove(&key(path));
    }
}

/// 退出时删掉这次暂停留下的半成品，并清掉登记表。
pub fn cleanup_paused_on_exit() {
    let paths = {
        let Ok(mut paused) = paused().lock() else {
            return;
        };
        paused.drain().collect::<Vec<_>>()
    };
    for path in paths {
        let _ = fs::remove_dir_all(&path);
    }
}

fn has_child(dir: &Path, name: &str) -> bool {
    dir.join(name).exists()
}

/// 扫一个游戏目录，返回删掉的路径（给日志用）。
pub fn sweep_game_dir(game_dir: &Path) -> Vec<String> {
    let mut removed = Vec::new();
    if !game_dir.is_dir() {
        return removed;
    }
    // 1. versions/<名字>：带标记就是半成品，整目录删。
    let versions = game_dir.join("versions");
    if let Ok(entries) = fs::read_dir(&versions) {
        for entry in entries.flatten() {
            let folder = entry.path();
            if !folder.is_dir() {
                continue;
            }
            if !has_child(&folder, PENDING_FILE) && !has_child(&folder, IGNORE_FILE) {
                continue;
            }
            let _ = fs::remove_dir_all(&folder);
            removed.push(folder.to_string_lossy().into_owned());
        }
    }
    // 2. 整合包暂存：pack.zip + staging/<实例名>。
    let modpack = game_dir.join(".ybmcl").join("modpack");
    if modpack.is_dir() {
        let _ = fs::remove_dir_all(&modpack);
        removed.push(modpack.to_string_lossy().into_owned());
    }
    // 3. 加载器安装器的工作目录（`.ybumcl-modloader-<pid>-<n>`）：一次性的，启动时
    //    不可能有安装器还在写，见到就删。
    if let Ok(entries) = fs::read_dir(game_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(".ybumcl-modloader-") {
                continue;
            }
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let _ = fs::remove_dir_all(&path);
            removed.push(path.to_string_lossy().into_owned());
        }
    }
    removed
}

/// 扫所有登记过的游戏目录（后台线程里跑）。
pub fn sweep_all(dirs: Vec<PathBuf>) {
    let mut seen = HashSet::new();
    for dir in dirs {
        if !seen.insert(key(&dir)) {
            continue;
        }
        for path in sweep_game_dir(&dir) {
            eprintln!("swept half-finished: {path}");
        }
    }
}
