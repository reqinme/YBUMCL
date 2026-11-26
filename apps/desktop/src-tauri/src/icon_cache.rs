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

/// 取一张图标：缓存新鲜就直接给，过期了先给旧的再后台刷，没缓存才同步下载。
pub fn load_or_fetch(dir: &Path, url: &str) -> Result<Option<String>, String> {
    prune(dir);
    let file = path_of(dir, url);
    if let Some((icon, fresh)) = read_entry(&file) {
        if fresh {
            return Ok(icon);
        }
        // 和 PCL 一样：旧图先顶上，后台重新下载，下好再覆盖，界面不会闪空。
        if let Some(icon) = icon {
            spawn_refresh(dir.to_path_buf(), url.to_owned());
            return Ok(Some(icon));
        }
        let _ = fs::remove_file(&file);
    }
    fetch_into(dir, url)
}

/// 后台刷新一张过期图标。
fn spawn_refresh(dir: PathBuf, url: String) {
    let _ = std::thread::Builder::new()
        .name("icon-refresh".into())
        .spawn(move || {
            let _ = fetch_into(&dir, &url);
        });
}

fn fetch_into(dir: &Path, url: &str) -> Result<Option<String>, String> {
    let file = path_of(dir, url);
    let gate = gate(url);
    let _guard = gate.lock().unwrap_or_else(|error| error.into_inner());
    // 等锁期间别人可能已经下好，再看一眼，别重复下载。
    if let Some((icon, fresh)) = read_entry(&file) {
        if fresh {
            return Ok(icon);
        }
    }
    let icon = fetch(url)?;
    // 只有真的拿到图片才落盘；失败、超时、拿到 HTML 都不写，
    // 否则一次网络抖动就会把「没有图标」锁死七天（PCL 也是失败即删）。
    if let Some(data) = icon.as_deref() {
        write(&file, data);
    }
    Ok(icon)
}

fn path_of(dir: &Path, url: &str) -> PathBuf {
    dir.join(format!("{}.txt", launcher_core::icons::url_key(url)))
}

/// 读缓存：返回内容 + 是否还在有效期内。空文件（旧版本留下的「没有图标」标记）直接删掉当没缓存。
fn read_entry(file: &Path) -> Option<(Option<String>, bool)> {
    let meta = fs::metadata(file).ok()?;
    let modified = meta.modified().ok()?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    let text = fs::read_to_string(file).ok()?;
    if text.is_empty() {
        let _ = fs::remove_file(file);
        return None;
    }
    Some((Some(text), age <= TTL))
}

fn write(file: &Path, icon: &str) {
    let Some(parent) = file.parent() else {
        return;
    };
    let _ = fs::create_dir_all(parent);
    // 先写临时文件再换名：中途退出不会留下半张图。
    let temp = file.with_extension("tmp");
    if fs::write(&temp, icon).is_err() {
        return;
    }
    let _ = fs::remove_file(file);
    let _ = fs::rename(&temp, file);
}

/// MCIM 的图片镜像：CDN 主机换掉、路径不变，官方图取不到时再试一次。
fn mirrored(url: &str) -> Option<String> {
    const HOSTS: [(&str, &str); 3] = [
        ("cdn.modrinth.com", "mod.mcimirror.top"),
        ("media.forgecdn.net", "mod.mcimirror.top"),
        ("edge.forgecdn.net", "mod.mcimirror.top"),
    ];
    HOSTS.iter().find_map(|(from, to)| {
        url.contains(&format!("//{from}/"))
            .then(|| url.replace(&format!("//{from}/"), &format!("//{to}/")))
    })
}

fn fetch(url: &str) -> Result<Option<String>, String> {
    let bytes = match launcher_core::download::fetch_icon_bytes(url) {
        Ok(bytes) => bytes,
        Err(primary) => match mirrored(url) {
            Some(backup) => launcher_core::download::fetch_icon_bytes(&backup)
                .map_err(|_| primary.to_string())?,
            None => return Err(primary.to_string()),
        },
    };
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
        // 过期文件留着当「旧图」用（读的时候会先显示再后台刷新）；
        // 空文件是旧版本留下的「没有图标」标记，直接清掉让它们重新下载。
        let empty = meta.len() == 0;
        if empty || age > TTL * 4 {
            let _ = fs::remove_file(entry.path());
        }
    }
}
