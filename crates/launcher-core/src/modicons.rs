//! 给本地模组补「项目图标」。
//!
//! 模组 jar 里不一定带图标（常见文件名、`logoFile`、`icon` 都没有），
//! PCL 的做法是拿文件的哈希去 Modrinth / CurseForge 反查项目，用项目图。
//! 这里照做：SHA1 查 Modrinth，CurseForge 用文件指纹，结果按 SHA1 缓存在
//! 数据目录里（6 小时），查不到也记下来，免得每次进模组页都重新问一遍。
//!
//! 缓存记的是「这个文件长什么样」，所以重装、换版本后 SHA1 变了会自动重查。

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

const CACHE_FILE: &str = "mod-icons.json";
const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const CACHE_VERSION: u32 = 3;

/// 资源包/光影包里自带的图标（`pack.png`）读成 data URL。
///
/// PCL 就是这条路：本地文件加载时直接 `jar.GetEntry("pack.png")`，不联网。
/// 这里只解压一个几百字节的 entry，比「整包 SHA1 + 查平台 + 下载图标」快得多。
pub fn local_pack_icon(path: &Path) -> Option<String> {
    let bytes = read_pack_png(path)?;
    if bytes.is_empty() || bytes.len() > 4 * 1024 * 1024 {
        return None;
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(format!("data:image/png;base64,{encoded}"))
}

fn read_pack_png(path: &Path) -> Option<Vec<u8>> {
    // 正常路径：交给 zip 库按中央目录找。
    if let Ok(file) = fs::File::open(path) {
        if let Ok(mut archive) = zip::ZipArchive::new(file) {
            for index in 0..archive.len() {
                let Ok(mut entry) = archive.by_index(index) else {
                    continue;
                };
                // 有的整合包把条目名写成大写（`PACK.PNG`），按字节比会漏掉。
                if !entry.name().eq_ignore_ascii_case("pack.png") || entry.is_dir() {
                    continue;
                }
                let mut data = Vec::new();
                if entry.read_to_end(&mut data).is_ok() && !data.is_empty() {
                    return Some(data);
                }
            }
        }
    }
    // 兜底：有些工具生成的 zip 前面多了几个字节（`PKPK`），
    // 中央目录偏移整体错位，zip 库直接打不开。这时自己扫本地文件头。
    scan_pack_png(path)
}

fn scan_pack_png(path: &Path) -> Option<Vec<u8>> {
    // 走中央目录而不是扫本地头：有些整合包里的 zip 开头多塞了 4 个字节
    // （文件以 PK 起头），本地头字段会跟着错位，中央目录才是准的。
    let data = fs::read(path).ok()?;
    let eocd = (0..data.len().saturating_sub(3))
        .rev()
        .find(|&i| data[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])?;
    let count = u16::from_le_bytes([data[eocd + 10], data[eocd + 11]]) as usize;
    let cd = u32::from_le_bytes([
        data[eocd + 16],
        data[eocd + 17],
        data[eocd + 18],
        data[eocd + 19],
    ]) as usize;
    // 有些包在文件开头塞了 4 个字节，EOCD 里记的偏移就整体偏小了，按签名找一下真实位置。
    let mut shift = 0_usize;
    let mut cd = cd;
    if cd + 4 <= data.len() && data[cd..cd + 4] != [0x50, 0x4b, 0x01, 0x02] {
        if let Some(delta) = (1..=16).find(|&delta| {
            cd + delta + 4 <= data.len() && data[cd + delta..cd + delta + 4] == [0x50, 0x4b, 0x01, 0x02]
        }) {
            cd += delta;
            shift = delta;
        }
    }
    let mut offset = cd;
    for _ in 0..count {
        if offset + 46 > data.len() || data[offset..offset + 4] != [0x50, 0x4b, 0x01, 0x02] {
            break;
        }
        let flags = u16::from_le_bytes([data[offset + 8], data[offset + 9]]);
        let method = u16::from_le_bytes([data[offset + 10], data[offset + 11]]);
        let compressed = u32::from_le_bytes([
            data[offset + 20],
            data[offset + 21],
            data[offset + 22],
            data[offset + 23],
        ]) as usize;
        let plain = u32::from_le_bytes([
            data[offset + 24],
            data[offset + 25],
            data[offset + 26],
            data[offset + 27],
        ]) as usize;
        let name_len = u16::from_le_bytes([data[offset + 28], data[offset + 29]]) as usize;
        let extra_len = u16::from_le_bytes([data[offset + 30], data[offset + 31]]) as usize;
        let comment_len = u16::from_le_bytes([data[offset + 32], data[offset + 33]]) as usize;
        let local = u32::from_le_bytes([
            data[offset + 42],
            data[offset + 43],
            data[offset + 44],
            data[offset + 45],
        ]) as usize
            + shift;
        let name_start = offset + 46;
        let name_end = name_start + name_len;
        if name_end > data.len() {
            break;
        }
        let name = &data[name_start..name_end];
        if name.eq_ignore_ascii_case(b"pack.png") {
        }
        if name.eq_ignore_ascii_case(b"pack.png") && local + 30 <= data.len() {
            let local_extra = u16::from_le_bytes([data[local + 28], data[local + 29]]) as usize;
            // 这张表里以本地头为准：个别包的中央目录记的 csize/psz 是错的。
            let local_method = u16::from_le_bytes([data[local + 8], data[local + 9]]);
            let local_csize = u32::from_le_bytes([
                data[local + 18],
                data[local + 19],
                data[local + 20],
                data[local + 21],
            ]) as usize;
            let local_usize = u32::from_le_bytes([
                data[local + 22],
                data[local + 23],
                data[local + 24],
                data[local + 25],
            ]) as usize;
            let method = if local_method == 0 { method } else { local_method };
            let compressed = if local_csize > 0 { local_csize } else { compressed };
            let plain = if local_usize > 0 { local_usize } else { plain };
            let local_name_len = u16::from_le_bytes([data[local + 26], data[local + 27]]) as usize;
            let body_start = local + 30 + local_name_len + local_extra;
            let limit = if compressed > 0 || flags & 0x08 == 0 {
                (body_start + compressed).min(data.len())
            } else {
                data.len()
            };
            if body_start < limit {
                let body = &data[body_start..limit];
                let is_png = |bytes: &[u8]| bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47]);
                let mut candidates: Vec<Vec<u8>> = Vec::new();
                if method == 8 {
                    for slice in [body, &data[body_start..]] {
                        let mut out = Vec::new();
                        if flate2::read::DeflateDecoder::new(slice)
                            .read_to_end(&mut out)
                            .is_ok()
                            && is_png(&out)
                        {
                            candidates.push(out);
                            break;
                        }
                    }
                }
                if candidates.is_empty() {
                    for slice in [body, &data[body_start..]] {
                        if !slice.is_empty() && is_png(slice) {
                            candidates.push(slice[..plain.min(slice.len())].to_vec());
                            break;
                        }
                    }
                }
                let decoded = candidates.into_iter().next();
                if decoded.is_some() {
                    return decoded;
                }
            }
        }
        offset = name_end + extra_len + comment_len;
    }
    None
}

/// 一个模组文件的两种身份：Modrinth 认 SHA1，CurseForge 认指纹。
#[derive(Debug, Clone)]
pub struct ModKey {
    pub file_name: String,
    pub sha1: String,
    pub fingerprint: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    version: u32,
    /// SHA1 → 项目图标地址；`url: null` 表示查过但没有。
    #[serde(default)]
    entries: HashMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    url: Option<String>,
    /// 项目原始名（Modrinth 的 title / CurseForge 的 name）。
    #[serde(default)]
    title: Option<String>,
    /// 项目 slug，用来查内置中文名表。
    #[serde(default)]
    slug: Option<String>,
    /// 项目来自哪个站（modrinth / curseforge）与它的 id，界面拿去跳详情页。
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    at: u64,
}

/// 一个本地模组文件对应的平台项目：图标、原始名、中文名。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub url: Option<String>,
    pub title: Option<String>,
    pub slug: Option<String>,
    pub chinese: Option<String>,
    /// `modrinth` / `curseforge`，没匹配到项目时是 None。
    pub source: Option<String>,
    pub project_id: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_path(data_dir: &Path, cache_name: &str) -> PathBuf {
    data_dir.join(cache_name)
}

fn load_cache(data_dir: &Path, cache_name: &str) -> Cache {
    let Ok(text) = fs::read_to_string(cache_path(data_dir, cache_name)) else {
        return Cache::default();
    };
    let cache: Cache = serde_json::from_str(&text).unwrap_or_default();
    if cache.version != CACHE_VERSION {
        return Cache::default();
    }
    cache
}

fn save_cache(data_dir: &Path, cache_name: &str, cache: &Cache) {
    let path = cache_path(data_dir, cache_name);
    if fs::create_dir_all(data_dir).is_err() {
        return;
    }
    let temp = path.with_extension("json.tmp");
    if let Ok(text) = serde_json::to_string(cache) {
        if fs::write(&temp, text).is_ok() {
            let _ = fs::rename(&temp, &path);
        }
    }
}

/// 读一次文件，同时算 SHA1 与 CurseForge 指纹。
pub fn key_of(path: &Path, file_name: &str) -> Result<ModKey, std::io::Error> {
    use sha1::{Digest, Sha1};
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut bytes = Vec::new();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        bytes.extend_from_slice(&buffer[..read]);
    }
    let sha1 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(ModKey {
        file_name: file_name.to_owned(),
        sha1,
        fingerprint: crate::export::curseforge_fingerprint(&bytes),
    })
}

/// 按哈希查项目信息（图标、原名、中文名）：先 Modrinth，剩下的再问 CurseForge。
///
/// 返回「文件名 → 项目信息」，查不到的不会出现在结果里。
pub fn icons_for(data_dir: &Path, keys: &[ModKey]) -> HashMap<String, ProjectInfo> {
    icons_for_kind(data_dir, CACHE_FILE, keys)
}

/// 和 [`icons_for`] 一样，但缓存文件名可换（资源包、光影包各自一份）。
pub fn icons_for_kind(
    data_dir: &Path,
    cache_name: &str,
    keys: &[ModKey],
) -> HashMap<String, ProjectInfo> {
    let mut cache = load_cache(data_dir, cache_name);
    let mut out: HashMap<String, ProjectInfo> = HashMap::new();
    let mut stale: Vec<&ModKey> = Vec::new();

    for key in keys {
        match cache.entries.get(&key.sha1) {
            Some(entry) if now().saturating_sub(entry.at) < CACHE_TTL.as_secs() => {
                out.insert(key.file_name.clone(), info_of(entry));
            }
            _ => stale.push(key),
        }
    }
    if stale.is_empty() {
        return out;
    }

    // SHA1 → (url, title, slug, source, project_id)
    let mut found: HashMap<String, (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)> =
        HashMap::new();

    let sha1s: Vec<String> = stale.iter().map(|key| key.sha1.clone()).collect();
    if let Ok(hits) = crate::modrinth::lookup_sha1(&sha1s) {
        let mut projects: Vec<String> = hits.iter().map(|hit| hit.project_id.clone()).collect();
        projects.sort();
        projects.dedup();
        if let Ok(meta) = crate::modrinth::project_meta(&projects) {
            for hit in &hits {
                if let Some(project) = meta.get(&hit.project_id) {
                    found.insert(
                        hit.sha1.to_lowercase(),
                        (
                            project.icon.clone(),
                            Some(project.title.clone()).filter(|t| !t.is_empty()),
                            Some(project.slug.clone()).filter(|t| !t.is_empty()),
                            Some("modrinth".to_owned()),
                            Some(hit.project_id.clone()),
                        ),
                    );
                }
            }
        }
    }

    let left: Vec<&ModKey> = stale
        .iter()
        .copied()
        .filter(|key| !found.contains_key(&key.sha1))
        .collect();
    if !left.is_empty() {
        let fingerprints: Vec<u32> = left.iter().map(|key| key.fingerprint).collect();
        if let Ok(hits) = crate::curseforge::fingerprint_details(&fingerprints) {
            let mut by_fingerprint: HashMap<u32, u64> = HashMap::new();
            let mut mod_ids: Vec<u64> = Vec::new();
            for (fingerprint, mod_id, _file_id) in hits {
                by_fingerprint.insert(fingerprint, mod_id);
                mod_ids.push(mod_id);
            }
            mod_ids.sort();
            mod_ids.dedup();
            if let Ok(meta) = crate::curseforge::project_meta(&mod_ids) {
                for key in &left {
                    let Some(mod_id) = by_fingerprint.get(&key.fingerprint) else {
                        continue;
                    };
                    let Some(project) = meta.get(mod_id) else {
                        continue;
                    };
                    found.insert(
                        key.sha1.clone(),
                        (
                            project.icon.clone(),
                            Some(project.name.clone()).filter(|t| !t.is_empty()),
                            Some(project.slug.clone()).filter(|t| !t.is_empty()),
                            Some("curseforge".to_owned()),
                            Some(mod_id.to_string()),
                        ),
                    );
                }
            }
        }
    }

    let at = now();
    for key in stale {
        let hit = found.get(&key.sha1).cloned().unwrap_or_default();
        let entry = Entry {
            url: hit.0.clone(),
            title: hit.1,
            slug: hit.2,
            source: hit.3,
            project_id: hit.4,
            at,
        };
        out.insert(key.file_name.clone(), info_of(&entry));
        cache.entries.insert(key.sha1.clone(), entry);
    }
    // 太久没动过的条目顺手清掉，别让文件无限长。
    if cache.entries.len() > 4096 {
        let cutoff = 30 * 24 * 60 * 60;
        cache
            .entries
            .retain(|_, entry| now().saturating_sub(entry.at) < cutoff);
    }
    save_cache(
        data_dir,
        cache_name,
        &Cache {
            version: CACHE_VERSION,
            entries: cache.entries,
        },
    );
    out
}

/// 缓存条目 → 界面用的项目信息（顺便查中文名）。
fn info_of(entry: &Entry) -> ProjectInfo {
    ProjectInfo {
        url: entry.url.clone(),
        title: entry.title.clone(),
        slug: entry.slug.clone(),
        chinese: entry
            .slug
            .as_deref()
            .and_then(crate::names::chinese_name)
            .map(str::to_owned),
        source: entry.source.clone(),
        project_id: entry.project_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_with(entry_name: &str, payload: &[u8]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut out);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file(entry_name, options).unwrap();
            std::io::Write::write_all(&mut writer, payload).unwrap();
            writer.finish().unwrap();
        }
        out.into_inner()
    }

    fn write_temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("ybumcl-pack-icon-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn reads_lowercase_pack_png() {
        let bytes = zip_with("pack.png", b"PNGDATA");
        let path = write_temp("lower.zip", &bytes);
        assert_eq!(read_pack_png(&path).as_deref(), Some(&b"PNGDATA"[..]));
    }

    #[test]
    fn reads_uppercase_pack_png() {
        let bytes = zip_with("PACK.PNG", b"PNGDATA");
        let path = write_temp("upper.zip", &bytes);
        assert_eq!(read_pack_png(&path).as_deref(), Some(&b"PNGDATA"[..]));
    }

    #[test]
    fn reads_prefixed_zip() {
        let mut bytes = b"PK".to_vec();
        bytes.extend(zip_with("pack.png", b"PNGDATA"));
        let path = write_temp("prefixed.zip", &bytes);
        assert_eq!(read_pack_png(&path).as_deref(), Some(&b"PNGDATA"[..]));
    }

}
