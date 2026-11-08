//! 关闭时清掉的缓存。
//!
//! 下载页「还原上次分类、筛选、上次选中的项目」、模组图标、模组更新、实例概述
//! 这些都在缓存里；关掉启动器就删，下次打开等同首次启动。
//!
//! 只删缓存：账号、设置、收藏、分类、日志、壁纸都不动；WebView2 的运行时组件
//! （WidevineCdm、component_crx_cache、Subresource Filter）也不是缓存，保留。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 主进程退出时用它把自己再拉起一次当清理器：WebView2 的 Local Storage 是
/// LevelDB，要等 `msedgewebview2.exe` 完全退出才解锁，在主进程里删只会碰到占用。
pub const CLEAN_FLAG: &str = "--clean-caches";

/// WebView2 的页面缓存与界面记忆，相对 `webview2\EBWebView`。
const WEBVIEW_DIRS: &[&str] = &[
    "Default/Local Storage/leveldb",
    "Default/Cache",
    "Default/Code Cache",
    "Default/GPUCache",
    "GrShaderCache",
    "ShaderCache",
];

/// 数据目录里的业务缓存文件：都靠 TTL 或缓存键自己失效。
const DATA_FILES: &[&str] = &[
    "mod-icons.json",
    "mod-updates.json",
    "mod-projects.json",
    "instance-cache.json",
    "resourcepacks-icons.json",
    "resourcepacks-hashes.json",
    "shaderpacks-icons.json",
    "shaderpacks-hashes.json",
];

/// 数据目录里的缓存目录。
const DATA_DIRS: &[&str] = &["http-cache", "cache"];

fn targets(data_dir: &Path) -> Vec<PathBuf> {
    let webview = data_dir.join("webview2").join("EBWebView");
    WEBVIEW_DIRS
        .iter()
        .map(|rel| webview.join(rel))
        .chain(DATA_DIRS.iter().map(|dir| data_dir.join(dir)))
        .chain(DATA_FILES.iter().map(|file| data_dir.join(file)))
        .collect()
}

/// 启动时补删：文件应该都是松的，等一秒多就够。
pub fn clear(data_dir: &Path) {
    clear_within(data_dir, Duration::from_secs(2));
}

/// 退出时先用它删一批没被占着的，删不掉的交给清理器。
pub fn clear_quick(data_dir: &Path) {
    for path in targets(data_dir) {
        let _ = remove_once(&path);
    }
}

/// 退出时拉起清理器：这时 WebView2 还没放锁，删不干净。
pub fn spawn_cleaner(data_dir: &Path) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = std::process::Command::new(exe)
        .arg(CLEAN_FLAG)
        .arg(data_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// 清理器进程的入口：等主进程把 WebView2 放掉再删，删完就返回。
pub fn run_cleaner(data_dir: &Path) {
    std::thread::sleep(Duration::from_millis(800));
    clear_within(data_dir, Duration::from_secs(8));
}

fn clear_within(data_dir: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    for path in targets(data_dir) {
        remove(&path, deadline);
    }
}

/// 扫掉下载留下的半截文件（`*.part`）。
///
/// 每个下载现在用自己的临时名（`<名>.<进程号>-<序号>.part`），崩溃或强杀之后
/// 没人再认领它们；启动时后台扫一遍，只删一天以前的，别碰正在下的。
pub fn sweep_parts(root: &Path, older_than: Duration) {
    let now = std::time::SystemTime::now();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                if depth < 6 {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            if !path.to_string_lossy().ends_with(".part") {
                continue;
            }
            let stale = meta
                .modified()
                .ok()
                .and_then(|time| now.duration_since(time).ok())
                .is_some_and(|age| age > older_than);
            if stale {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

fn remove_once(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// 被占用（退出瞬间常见）就过一会儿再试；本来就不存在也算完成。
fn remove(path: &Path, deadline: Instant) {
    loop {
        match remove_once(path) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                if Instant::now() >= deadline {
                    eprintln!("清理缓存失败 {}: {error}", path.display());
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}
