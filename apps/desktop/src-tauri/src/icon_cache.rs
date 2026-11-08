//! 网络图标的磁盘缓存。
//!
//! 对标 PCL 的 `MyImage`：下载过的图存到 `YBUMCL\icon-cache\<URL 的 sha1>.txt`，
//! 内容是 data URL；确认没有图标（纯白/纯透明/不是图片）的存成空文件。7 天过期。
//!
//! 之前只有进程内内存缓存（512 条），关掉启动器就全没了——每次打开模组页、
//! 下载页都要把几十上百张图重新下一遍，慢的正是这一段。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// 和 PCL 的 `FileCacheExpiredTime` 一致。
const TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// 同一个 URL 的并发请求（列表卡和详情面板会同时要同一张图）只让一个去下载。
fn inflight() -> &'static Mutex<HashMap<String, Arc<Mutex<()>>>> {
    static INFLIGHT: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    INFLIGHT.get_or_init(|| Mutex::new(HashMap::new()))
}

fn gate(url: &str) -> Arc<Mutex<()>> {
    let mut map = match inflight().lock() {
        Ok(map) => map,
        Err(error) => error.into_inner(),
    };
    // 缓存目录只留活着的 gate：图标请求很多，不清理会一直涨。
    if map.len() > 256 {
        map.retain(|_, entry| Arc::strong_count(entry) > 1);
    }
    map.entry(url.to_owned())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// 取一张图标：先看磁盘缓存，没有（或过期）才联网。
pub fn load_or_fetch(dir: &Path, url: &str) -> Result<Option<String>, String> {
    prune(dir);
    let file = path_of(dir, url);
    if let Some(hit) = read(&file) {
        return Ok(hit);
    }
    let gate = gate(url);
    let _guard = gate.lock().unwrap_or_else(|error| error.into_inner());
    // 等锁期间别人可能已经下好，再看一眼，别重复下载。
    if let Some(hit) = read(&file) {
        return Ok(hit);
    }
    let icon = fetch(url)?;
    write(&file, icon.as_deref());
    Ok(icon)
}

fn path_of(dir: &Path, url: &str) -> PathBuf {
    dir.join(format!("{}.txt", launcher_core::icons::url_key(url)))
}

fn read(file: &Path) -> Option<Option<String>> {
    let meta = fs::metadata(file).ok()?;
    let modified = meta.modified().ok()?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    if age > TTL {
        let _ = fs::remove_file(file);
        return None;
    }
    let text = fs::read_to_string(file).ok()?;
    Some(if text.is_empty() { None } else { Some(text) })
}

fn write(file: &Path, icon: Option<&str>) {
    let Some(parent) = file.parent() else {
        return;
    };
    let _ = fs::create_dir_all(parent);
    // 先写临时文件再换名：中途退出不会留下半张图。
    let temp = file.with_extension("tmp");
    if fs::write(&temp, icon.unwrap_or("")).is_err() {
        return;
    }
    let _ = fs::remove_file(file);
    let _ = fs::rename(&temp, file);
}

fn fetch(url: &str) -> Result<Option<String>, String> {
    let bytes = launcher_core::download::fetch_icon_bytes(url).map_err(|error| error.to_string())?;
    Ok(match launcher_core::icons::inspect(&bytes) {
        // 纯白、纯透明或根本不是图片：记成「没有图标」，界面显示项目名占位。
        None => None,
        Some((_mime, true)) => None,
        Some((mime, false)) => Some(format!(
            "data:image/{mime};base64,{}",
            launcher_core::icons::base64(&bytes)
        )),
    })
}

/// 每个进程只扫一遍：删掉过期文件，免得缓存目录越用越大。
fn prune(dir: &Path) {
    static DONE: OnceLock<()> = OnceLock::new();
    if DONE.get().is_some() {
        return;
    }
    if DONE.set(()).is_err() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let age = meta
            .modified()
            .ok()
            .and_then(|at| now.duration_since(at).ok())
            .unwrap_or_default();
        if age > TTL {
            let _ = fs::remove_file(entry.path());
        }
    }
}
