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

/// 一张从包里抠出来的图标：字节 + MIME。
struct PackImage {
    bytes: Vec<u8>,
    mime: &'static str,
}

/// 资源包/光影包里自带的图标（`pack.png`）读成 data URL。
///
/// PCL 就是这条路：本地文件加载时直接 `jar.GetEntry("pack.png")`，不联网。
/// 这里只解压一个几百字节的 entry，比「整包 SHA1 + 查平台 + 下载图标」快得多。
pub fn local_pack_icon(path: &Path) -> Option<String> {
    let image = read_pack_icon_image(path)?;
    if image.bytes.is_empty() || image.bytes.len() > 4 * 1024 * 1024 {
        return None;
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(image.bytes);
    Some(format!("data:{};base64,{}", image.mime, encoded))
}

/// 按顺序找图标，前一步拿到了就不走后面：
/// 1. `zip` 库按中央目录读（正常包）；
/// 2. 自己解析中央目录（开头多字节、条目名大写、中央目录尺寸写错）；
/// 3. 整文件里直接搜图片数据（流式 zip 记录 `csize=0`、尾部 descriptor、非 zip 容器等）。
fn read_pack_icon_image(path: &Path) -> Option<PackImage> {
    // zip 能正常打开，就只认里面的 `pack.png`：包里那些压缩过的贴图里
    // 偶尔会残留 PNG 魔数，硬切出来是半张图，`<img>` 只会显示空白。
    let opens_as_zip = fs::File::open(path)
        .ok()
        .map(|file| zip::ZipArchive::new(file).is_ok())
        .unwrap_or(false);
    if opens_as_zip {
        // 能打开但读不到条目（流式 zip 之类）时还可以按中央目录扫，
        // 但不再硬切文件——那是「半张图」的来源。
        return read_via_zip_lib(path).or_else(|| scan_pack_png(path));
    }
    if let Some(image) = scan_pack_png(path) {
        return Some(image);
    }
    carve_image(path)
}

/// PNG / WebP / JPEG 按魔数认，其余当 PNG 处理。
fn mime_of(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47]) {
        "image/png"
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else {
        "image/png"
    }
}

fn is_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47])
        || bytes.starts_with(&[0xff, 0xd8, 0xff])
        || (bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
}

fn wrap(bytes: Vec<u8>) -> Option<PackImage> {
    if bytes.is_empty() || !is_image(&bytes) || !looks_complete(&bytes) {
        return None;
    }
    let mime = mime_of(&bytes);
    Some(PackImage { bytes, mime })
}

/// 光有魔数不够：从压缩流里硬切出来的片段经常是「头对、身子断」，
/// 这种图 `<img>` 会静默失败，表现成空白方块。这里按格式再验一遍。
fn looks_complete(bytes: &[u8]) -> bool {
    // PNG：IHDR 紧跟签名，宽高必须像话。
    if bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47]) {
        if bytes.len() < 33 || &bytes[12..16] != b"IHDR" {
            return false;
        }
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return width > 0 && height > 0 && width <= 4096 && height <= 4096;
    }
    // WebP：RIFF 后面必须是 VP8/VP8L/VP8X 之一。
    if bytes.len() >= 16 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return matches!(&bytes[12..16], b"VP8 " | b"VP8L" | b"VP8X");
    }
    // JPEG：必须以 EOI 收尾。
    bytes.ends_with(&[0xff, 0xd9])
}

#[cfg(test)]
fn read_pack_png_bytes(path: &Path) -> Option<Vec<u8>> {
    read_pack_icon_image(path).map(|image| image.bytes)
}

fn read_via_zip_lib(path: &Path) -> Option<PackImage> {
    let file = fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    for index in 0..archive.len() {
        let Ok(mut entry) = archive.by_index(index) else {
            continue;
        };
        // 有的整合包把条目名写成大写（`PACK.PNG`），按字节比会漏掉。
        if !entry.name().eq_ignore_ascii_case("pack.png") || entry.is_dir() {
            continue;
        }
        let mut data = Vec::new();
        if entry.read_to_end(&mut data).is_ok() {
            if let Some(image) = wrap(data) {
                return Some(image);
            }
        }
    }
    None
}

fn scan_pack_png(path: &Path) -> Option<PackImage> {
    // 走中央目录而不是扫本地头：有些整合包里的 zip 开头多塞了 4 个字节
    // （文件以 PK 05 06 起头），本地头字段会跟着错位，中央目录才是准的。
    let data = fs::read(path).ok()?;
    let eocd = (0..data.len().saturating_sub(3))
        .rev()
        .find(|&i| data[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])?;
    let count = u16::from_le_bytes([data[eocd + 10], data[eocd + 11]]) as usize;
    let mut cd = u32::from_le_bytes([
        data[eocd + 16],
        data[eocd + 17],
        data[eocd + 18],
        data[eocd + 19],
    ]) as usize;
    // 有些包在文件开头塞了 4 个字节，EOCD 里记的偏移就整体偏小了，按签名找一下真实位置。
    let mut shift = 0_usize;
    if cd + 4 <= data.len() && data[cd..cd + 4] != [0x50, 0x4b, 0x01, 0x02] {
        if let Some(delta) = (1..=16).find(|&delta| {
            cd + delta + 4 <= data.len()
                && data[cd + delta..cd + delta + 4] == [0x50, 0x4b, 0x01, 0x02]
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
        if data[name_start..name_end].eq_ignore_ascii_case(b"pack.png") && local + 30 <= data.len() {
            if let Some(image) = read_local_entry(&data, local, method, compressed, plain) {
                return Some(image);
            }
        }
        offset = name_end + extra_len + comment_len;
    }
    None
}

/// 从本地文件头解一个 entry：尺寸以本地头为准，流式 zip（`csize=0`）解到流结束。
fn read_local_entry(
    data: &[u8],
    local: usize,
    method_hint: u16,
    compressed_hint: usize,
    plain_hint: usize,
) -> Option<PackImage> {
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
    let name_len = u16::from_le_bytes([data[local + 26], data[local + 27]]) as usize;
    let extra_len = u16::from_le_bytes([data[local + 28], data[local + 29]]) as usize;
    let method = if local_method == 0 { method_hint } else { local_method };
    let compressed = if local_csize > 0 { local_csize } else { compressed_hint };
    let plain = if local_usize > 0 { local_usize } else { plain_hint };
    let body_start = local + 30 + name_len + extra_len;
    if body_start >= data.len() {
        return None;
    }
    let tail = &data[body_start..];
    if method == 8 {
        // 先按记录的长度解；解不开或不像图片，就当成流式压缩一路解到结束。
        if compressed > 0 {
            let end = (body_start + compressed).min(data.len());
            if let Some(image) = inflate(&data[body_start..end]) {
                return Some(image);
            }
        }
        return inflate(tail);
    }
    // stored：优先按记录的明文长度切，长度不可信时整段试。
    if plain > 0 {
        let end = (body_start + plain).min(data.len());
        if let Some(image) = wrap(data[body_start..end].to_vec()) {
            return Some(image);
        }
    }
    wrap(tail.to_vec())
}

/// deflate 解压；自带结束标记，尾部多余字节导致的错误忽略。
fn inflate(slice: &[u8]) -> Option<PackImage> {
    let mut out = Vec::new();
    let _ = flate2::read::DeflateDecoder::new(slice).read_to_end(&mut out);
    wrap(out)
}

/// 最后一步：不管容器，直接在文件里找图片本体。
fn carve_image(path: &Path) -> Option<PackImage> {
    let data = fs::read(path).ok()?;
    // PNG：从 89 50 4E 47 起，到 IEND + CRC 结束。
    const PNG: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    let mut at = 0_usize;
    while let Some(pos) = data[at..].windows(8).position(|w| w == PNG) {
        let start = at + pos;
        if let Some(iend) = data[start..].windows(4).position(|w| w == b"IEND") {
            let end = start + iend + 8;
            if end <= data.len() {
                if let Some(image) = wrap(data[start..end].to_vec()) {
                    return Some(image);
                }
            }
        }
        at = start + 8;
    }
    // WebP：RIFF + 4 字节长度 + WEBP。
    let mut at = 0_usize;
    while let Some(pos) = data[at..].windows(4).position(|w| w == b"RIFF") {
        let start = at + pos;
        if start + 12 <= data.len() && &data[start + 8..start + 12] == b"WEBP" {
            let size = u32::from_le_bytes([
                data[start + 4],
                data[start + 5],
                data[start + 6],
                data[start + 7],
            ]) as usize
                + 8;
            let end = start + size;
            if end > data.len() {
                at = start + 4;
                continue;
            }
            if let Some(image) = wrap(data[start..end].to_vec()) {
                return Some(image);
            }
        }
        at = start + 4;
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

/// 文件名 + 大小 + 修改时间没变，就不用重算整包 SHA1。
///
/// 光影包动辄几十 MB，串行算哈希是「图标慢」的第一段。
#[derive(Debug, Default, Serialize, Deserialize)]
struct HashCache {
    #[serde(default)]
    version: u32,
    /// `<目录>/<文件名>|<大小>|<修改时间毫秒>` → 哈希
    #[serde(default)]
    entries: std::collections::HashMap<String, HashEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HashEntry {
    sha1: String,
    fingerprint: u32,
    at: u64,
}

fn hash_cache_path(data_dir: &Path, cache_name: &str) -> PathBuf {
    data_dir.join(cache_name)
}

fn load_hash_cache(data_dir: &Path, cache_name: &str) -> HashCache {
    let path = hash_cache_path(data_dir, cache_name);
    let Ok(text) = fs::read_to_string(&path) else {
        return HashCache::default();
    };
    let mut cache: HashCache = serde_json::from_str(&text).unwrap_or_default();
    cache.version = 1;
    cache
}

fn save_hash_cache(data_dir: &Path, cache_name: &str, cache: &HashCache) {
    let path = hash_cache_path(data_dir, cache_name);
    if let Ok(text) = serde_json::to_string(cache) {
        let _ = fs::write(path, text);
    }
}

/// 给一批包算 SHA1/指纹：命中缓存的直接复用，剩下的 8 线程并行算。
pub fn keys_of(
    data_dir: &Path,
    cache_name: &str,
    dir: &Path,
    files: &[String],
) -> Vec<ModKey> {
    let mut cache = load_hash_cache(data_dir, cache_name);
    let mut out: Vec<Option<ModKey>> = Vec::with_capacity(files.len());
    let mut pending: Vec<(usize, PathBuf, String, String)> = Vec::new();
    let at = now();

    for (index, file_name) in files.iter().enumerate() {
        if file_name.contains('/') || file_name.contains('\\') || file_name.contains("..") {
            out.push(None);
            continue;
        }
        let path = dir.join(file_name);
        let Ok(meta) = fs::metadata(&path) else {
            out.push(None);
            continue;
        };
        if !meta.is_file() {
            out.push(None);
            continue;
        }
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let key = format!("{file_name}|{}|{modified}", meta.len());
        match cache.entries.get(&key) {
            Some(entry) if at.saturating_sub(entry.at) < 30 * 24 * 60 * 60 => {
                out.push(Some(ModKey {
                    file_name: file_name.clone(),
                    sha1: entry.sha1.clone(),
                    fingerprint: entry.fingerprint,
                }));
            }
            _ => {
                out.push(None);
                pending.push((index, path, file_name.clone(), key));
            }
        }
    }

    if !pending.is_empty() {
        let workers = pending.len().min(8);
        let queue = std::sync::Mutex::new(pending);
        let results = std::sync::Mutex::new(Vec::<(usize, ModKey, String)>::new());
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| loop {
                    let task = {
                        let mut guard = queue.lock().unwrap();
                        guard.pop()
                    };
                    let Some((index, path, file_name, key)) = task else {
                        break;
                    };
                    if let Ok(key_of) = key_of(&path, &file_name) {
                        results.lock().unwrap().push((index, key_of, key));
                    }
                });
            }
        });
        for (index, mod_key, key) in results.into_inner().unwrap() {
            cache.entries.insert(
                key,
                HashEntry {
                    sha1: mod_key.sha1.clone(),
                    fingerprint: mod_key.fingerprint,
                    at,
                },
            );
            out[index] = Some(mod_key);
        }
    }

    // 太老的条目清掉，别让文件无限长。
    if cache.entries.len() > 8192 {
        cache
            .entries
            .retain(|_, entry| at.saturating_sub(entry.at) < 30 * 24 * 60 * 60);
    }
    save_hash_cache(data_dir, cache_name, &cache);

    out.into_iter().flatten().collect()
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

    // 两个平台并行问：Modrinth 与 CurseForge 都是网络往返，串行要等两次。
    // SHA1 → (url, title, slug, source, project_id)
    let mut found: HashMap<
        String,
        (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    > = HashMap::new();

    let sha1s: Vec<String> = stale.iter().map(|key| key.sha1.clone()).collect();
    let fingerprints: Vec<u32> = stale.iter().map(|key| key.fingerprint).collect();
    let (modrinth_hits, curseforge_hits) = std::thread::scope(|scope| {
        let mr = scope.spawn(|| crate::modrinth::lookup_sha1(&sha1s));
        let cf = scope.spawn(|| crate::curseforge::fingerprint_details(&fingerprints));
        (
            mr.join().unwrap_or_else(|_| Ok(Vec::new())),
            cf.join().unwrap_or_else(|_| Ok(Vec::new())),
        )
    });

    if let Ok(hits) = modrinth_hits {
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

    // CurseForge 只补 Modrinth 没命中的，避免同一文件被两个平台的结果打架。
    if let Ok(hits) = curseforge_hits {
        let mut by_fingerprint: HashMap<u32, u64> = HashMap::new();
        let mut mod_ids: Vec<u64> = Vec::new();
        for (fingerprint, mod_id, _file_id) in hits {
            by_fingerprint.insert(fingerprint, mod_id);
            mod_ids.push(mod_id);
        }
        mod_ids.sort();
        mod_ids.dedup();
        if let Ok(meta) = crate::curseforge::project_meta(&mod_ids) {
            for key in stale.iter() {
                if found.contains_key(&key.sha1) {
                    continue;
                }
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


    fn png_bytes() -> Vec<u8> {
        let mut bytes = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.extend_from_slice(b"\x00\x00\x00\x0dIHDR");
        bytes.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(b"\x00\x00\x00\x00IEND\xaeB`\x82");
        bytes
    }

    #[test]
    fn reads_webp_pack_png() {
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&12u32.to_le_bytes());
        webp.extend_from_slice(b"WEBPVP8 ");
        let path = write_temp("webp.zip", &zip_with("pack.png", &webp));
        let url = local_pack_icon(&path).unwrap();
        assert!(url.starts_with("data:image/webp;base64,"), "{url}");
    }

    #[test]
    fn carves_png_outside_zip() {
        let mut raw = b"not a zip at all".to_vec();
        raw.extend_from_slice(&png_bytes());
        raw.extend_from_slice(b"trailing junk");
        let path = write_temp("notzip.zip", &raw);
        let url = local_pack_icon(&path).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
    }

    fn streamed_zip(name: &str, payload: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut encoder = flate2::write::DeflateEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        );
        encoder.write_all(payload).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut out = Vec::new();
        // 本地头：flag 0x08（尾部 descriptor）、csize/usize 都写 0。
        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0x0008u16.to_le_bytes());
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&compressed);
        out.extend_from_slice(&[0x50, 0x4b, 0x07, 0x08]);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        let cd_offset = out.len() as u32;
        // 中央目录：同样 flag 0x08、csize/usize 0，指向偏移 0。
        out.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0x0008u16.to_le_bytes());
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        let cd_size = out.len() as u32 - cd_offset;
        // EOCD
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn reads_streamed_zip_entry() {
        // 流式写入的 zip：本地头 csize=0，真正长度在尾部 descriptor 里。
        let raw = streamed_zip("pack.png", &png_bytes());
        let path = write_temp("streamed.zip", &raw);
        let url = local_pack_icon(&path).expect("streamed zip should still yield an icon");
        assert!(url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn reads_lowercase_pack_png() {
        let png = png_bytes();
        let path = write_temp("lower.zip", &zip_with("pack.png", &png));
        assert_eq!(read_pack_png_bytes(&path).as_deref(), Some(&png[..]));
    }

    #[test]
    fn reads_uppercase_pack_png() {
        let png = png_bytes();
        let path = write_temp("upper.zip", &zip_with("PACK.PNG", &png));
        assert_eq!(read_pack_png_bytes(&path).as_deref(), Some(&png[..]));
    }

    #[test]
    fn reads_prefixed_zip() {
        let png = png_bytes();
        let mut bytes = b"PK".to_vec();
        bytes.extend(zip_with("pack.png", &png));
        let path = write_temp("prefixed.zip", &bytes);
        assert_eq!(read_pack_png_bytes(&path).as_deref(), Some(&png[..]));
    }

}
