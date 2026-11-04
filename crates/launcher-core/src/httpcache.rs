//! 极简的 HTTP 响应缓存，对应 PCL 用 CacheCow 存下来的那份。
//!
//! 只服务「拿 JSON」这类幂等 GET：响应体按 URL 哈希落盘，旁边记一份 ETag。
//! 逻辑很朴素——保鲜期内直接用，过期了带 `If-None-Match` 去问一次，
//! 304 就把本地那份当成新的。PCL 用的是完整的 CacheCow，行为上大差不差。

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// 由 Tauri 在启动时把数据目录接进来；没初始化就整个缓存退化成一堆 None。
pub fn init(root: PathBuf) {
    let _ = fs::create_dir_all(&root);
    let _ = ROOT.set(root);
}

/// URL → (正文路径, 元数据路径)。用 FNV-1a，够短且不需要额外依赖。
fn entry(url: &str) -> Option<(PathBuf, PathBuf)> {
    let root = ROOT.get()?;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1_0000_0001b3);
    }
    let base = root.join(format!("{hash:016x}"));
    Some((base.with_extension("body"), base.with_extension("meta")))
}

/// 还在保鲜期内就直接用，省掉一次往返。
pub fn fresh(url: &str, max_age: Duration) -> Option<Vec<u8>> {
    let (body_path, meta_path) = entry(url)?;
    let stored = fs::metadata(&meta_path).ok()?.modified().ok()?;
    if stored.elapsed().unwrap_or_default() > max_age {
        return None;
    }
    fs::read(body_path).ok()
}

/// 上次存下的 ETag（空文件表示服务端没给）。
pub fn etag(url: &str) -> Option<String> {
    let value = fs::read_to_string(entry(url)?.1).ok()?;
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

/// 缓存正文，不管保鲜期（304 时用）。
pub fn body(url: &str) -> Option<Vec<u8>> {
    fs::read(entry(url)?.0).ok()
}

/// 304 之后把时间戳刷新一下，相当于确认「本地这份还是最新的」。
pub fn refresh(url: &str) {
    if let Some((_, meta_path)) = entry(url) {
        if let Ok(etag) = fs::read_to_string(&meta_path) {
            let _ = fs::write(&meta_path, etag);
        }
    }
}

pub fn store(url: &str, body_bytes: &[u8], etag: Option<&str>) {
    let Some((body_path, meta_path)) = entry(url) else {
        return;
    };
    if let Some(parent) = body_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&body_path, body_bytes);
    let _ = fs::write(&meta_path, etag.unwrap_or_default());
}
