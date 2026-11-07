//! 模组更新检查：按文件哈希反查来源，再问来源有没有适配当前版本/加载器的新文件。
//!
//! 照 PCL 的做法：Modrinth 用 `POST /version_files/update` 一次问全，
//! CurseForge 用项目自带的 `latestFilesIndexes` + `POST /mods/files` 批量取，
//! 判定「要不要更新」只看**目标文件的 sha1 与本地是否相同**，不看版本号字符串。
//! 结果按「sha1 + 版本 + 加载器」缓存 6 小时，没查到的一个都不写。

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{curseforge, export, modrinth, CoreError};

/// 缓存有效期：6 小时后重新联网（PCL 同款）。
const CACHE_TTL_SECS: u64 = 6 * 60 * 60;

/// 缓存文件；换版本/换加载器就整份失效。
const CACHE_FILE: &str = "mod-updates.json";

/// 一条模组的更新情况；只有能识别出来源的模组才会出现。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModUpdate {
    /// 当前文件名（可能带 `.disabled`）。
    pub file_name: String,
    /// `modrinth` 或 `curseforge`。
    pub source: String,
    /// Modrinth 的 project id / CurseForge 的数字 id。
    pub project_id: String,
    /// 当前版本号（Modrinth 用版本号，CurseForge 用文件名）。
    pub current_version: String,
    /// 最新版本号。
    pub latest_version: String,
    pub latest_file_name: String,
    pub latest_url: String,
    /// 新文件的 SHA1（为空表示来源没给）。
    #[serde(default)]
    pub latest_sha1: String,
    pub can_update: bool,
}

/// 认领结果：这个 jar 在某个来源上对应哪个项目。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModProject {
    /// `modrinth` 或 `curseforge`。
    pub source: String,
    /// Modrinth 的 project id / CurseForge 的数字 id。
    pub project_id: String,
}

/// 一次检查的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReport {
    pub updates: Vec<ModUpdate>,
    /// 认出来的项目数（不含没命中任何来源的 jar）。
    pub identified: usize,
    /// 这次联网失败、下次还得重查的项目数。
    pub failed: usize,
    /// 全部命中缓存、这次没联网。
    pub cached: bool,
}

/// 进度回调：阶段名 + 已完成 + 总数。阶段名给界面直接显示。
pub type Progress<'a> = &'a mut dyn FnMut(&str, usize, usize);

/// 给单个模组文件认领项目：先查 Modrinth（sha1），查不到再问 CurseForge（指纹）。
pub fn identify(runtime_dir: &Path, file_name: &str) -> Result<Option<ModProject>, CoreError> {
    let dir = mods_dir(runtime_dir);
    let path = dir.join(file_name);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(crate::io_error(&path, error)),
    };
    let mut hasher = Sha1::new();
    hasher.update(&bytes);
    let sha1 = hex(&hasher.finalize());

    if let Ok(hits) = modrinth::lookup_sha1(&[sha1]) {
        if let Some(hit) = hits.into_iter().next() {
            return Ok(Some(ModProject {
                source: "modrinth".into(),
                project_id: hit.project_id,
            }));
        }
    }

    let murmur = export::curseforge_fingerprint(&bytes);
    if let Ok(hits) = curseforge::fingerprint_details(&[murmur]) {
        if let Some((_fingerprint, mod_id, _file_id)) = hits.into_iter().next() {
            return Ok(Some(ModProject {
                source: "curseforge".into(),
                project_id: mod_id.to_string(),
            }));
        }
    }
    Ok(None)
}

/// 检查目录里所有模组的更新情况。认不出来源的文件直接跳过。
///
/// `data_dir` 下写缓存；`force` 为真时忽略缓存整份重查；`cancel` 置位后尽快返回。
pub fn check(
    runtime_dir: &Path,
    game: &str,
    loader: &str,
    data_dir: &Path,
    force: bool,
    cancel: &AtomicBool,
    progress: Option<Progress<'_>>,
) -> Result<UpdateReport, CoreError> {
    let mut progress = progress;
    let mut tick = |stage: &str, done: usize, total: usize| {
        if let Some(callback) = progress.as_mut() {
            callback(stage, done, total);
        }
    };

    let dir = mods_dir(runtime_dir);
    let files = local_files(&dir, cancel, &mut tick)?;
    if cancel.load(Ordering::SeqCst) {
        // 取消不算失败：让界面保留上一次的结果，别用半份数据覆盖。
        return Err(CoreError::InvalidInput("已取消".into()));
    }
    if files.is_empty() {
        return Ok(UpdateReport {
            updates: Vec::new(),
            identified: 0,
            failed: 0,
            cached: false,
        });
    }

    let game = game.trim();
    let loader = loader.trim();
    let now = now_secs();
    let mut cache = load_cache(data_dir, game, loader);
    let mut fresh = HashMap::new();

    // 哪些还要联网：缓存里没有、或者过期 / 上次没查成功。
    let mut pending: Vec<&LocalFile> = Vec::new();
    for file in &files {
        let hit = cache.entries.get(&file.sha1);
        let usable = !force
            && hit.is_some_and(|entry| {
                entry.ok && now.saturating_sub(entry.at) < CACHE_TTL_SECS && !entry.url.is_empty()
            });
        if !usable {
            pending.push(file);
        }
    }
    if pending.is_empty() {
        let mut report = report_from_cache(&files, &cache);
        report.cached = true;
        tick("done", files.len(), files.len());
        return Ok(report);
    }

    tick("modrinth", 0, pending.len());
    if cancel.load(Ordering::SeqCst) {
        return Err(CoreError::InvalidInput("已取消".into()));
    }

    // ── Modrinth：先 sha1 换项目，再一次问「该升级到哪个文件」。
    let sha1s: Vec<String> = pending.iter().map(|file| file.sha1.clone()).collect();
    let mut looked_up: HashSet<String> = HashSet::new();
    // 只把「这一步真的报错」算成没查通；查到了但没有适配当前版本的更新是正常结果。
    let mut modrinth_error = false;
    let mut curse_error = false;
    if let Ok(hits) = modrinth::lookup_sha1(&sha1s) {
        for hit in &hits {
            looked_up.insert(hit.sha1.to_lowercase());
        }
        match modrinth::update_version_files(&sha1s, none_if_empty(game), none_if_empty(loader)) {
          Err(_) => modrinth_error = true,
          Ok(targets) => {
            for target in targets {
                let sha1 = target.sha1.to_lowercase();
                let Some(local) = pending.iter().find(|file| file.sha1 == sha1) else {
                    continue;
                };
                fresh.insert(
                    sha1.clone(),
                    CacheEntry {
                        source: "modrinth".into(),
                        project: target.project_id,
                        current: String::new(),
                        latest: target.version_number,
                        file_name: target.file_name,
                        url: target.file_url,
                        latest_sha1: target.file_sha1,
                        at: now,
                        ok: true,
                        local_file: local.file_name.clone(),
                    },
                );
            }
          }
        }
    } else {
        modrinth_error = true;
    }
    let modrinth_failed = if modrinth_error { looked_up.len() } else { 0 };
    tick("modrinth", pending.len(), pending.len());

    if cancel.load(Ordering::SeqCst) {
        return Err(CoreError::InvalidInput("已取消".into()));
    }

    // ── CurseForge：指纹 → 项目（自带 latestFilesIndexes）→ 批量取文件详情。
    tick("curseforge", 0, pending.len());
    let curse_pending: Vec<&LocalFile> = pending
        .iter()
        .copied()
        .filter(|file| !looked_up.contains(&file.sha1))
        .collect();
    if !curse_pending.is_empty() {
        let fingerprints: Vec<u32> = curse_pending.iter().map(|file| file.murmur).collect();
        let by_mod = curseforge::fingerprint_details(&fingerprints).unwrap_or_default();
        let mut mod_of: HashMap<u32, u64> = HashMap::new();
        let mut mod_ids: Vec<u64> = Vec::new();
        for (fingerprint, mod_id, file_id) in by_mod {
            mod_of.insert(fingerprint, mod_id);
            mod_ids.push(mod_id);
            let _ = fingerprint;
            let _ = file_id;
        }
        mod_ids.sort_unstable();
        mod_ids.dedup();

        let indexes = match curseforge::latest_file_indexes(&mod_ids) {
            Ok(indexes) => Some(indexes),
            Err(_) => {
                curse_error = true;
                None
            }
        };
        if let Some(indexes) = indexes {
            // 每个项目：只留「游戏版本 + 加载器」对得上的，取 fileId 最大的（后发的 id 更大）。
            let mut wanted: HashMap<u64, u64> = HashMap::new();
            for index in indexes {
                // 和 PCL 一样严格：游戏版本、加载器都要对上；
                // 加载器为空（通用 jar）不算命中，宁可多报「没查到」也别推错文件。
                if !game.is_empty() && index.game_version != game {
                    continue;
                }
                if !loader.is_empty() && !index.mod_loader.eq_ignore_ascii_case(loader) {
                    continue;
                }
                let slot = wanted.entry(index.mod_id).or_insert(index.file_id);
                if index.file_id > *slot {
                    *slot = index.file_id;
                }
            }
            let ids: Vec<u64> = wanted.values().copied().collect();
            if let Ok(files) = curseforge::files_by_ids(&ids) {
                let by_id: HashMap<u64, curseforge::PackFile> =
                    files.into_iter().map(|file| (file.id, file)).collect();
                for local in &curse_pending {
                    let Some(mod_id) = mod_of.get(&local.murmur).copied() else {
                        continue;
                    };
                    let Some(file_id) = wanted.get(&mod_id).copied() else {
                        continue;
                    };
                    let Some(file) = by_id.get(&file_id) else {
                        continue;
                    };
                    fresh.insert(
                        local.sha1.clone(),
                        CacheEntry {
                            source: "curseforge".into(),
                            project: mod_id.to_string(),
                            current: String::new(),
                            latest: file.file_name.clone(),
                            file_name: file.file_name.clone(),
                            url: file.url.clone(),
                            latest_sha1: file.sha1.clone().unwrap_or_default().to_lowercase(),
                            at: now,
                            ok: true,
                            local_file: local.file_name.clone(),
                        },
                    );
                }
            }
        }
    }
    tick("curseforge", pending.len(), pending.len());

    let found = fresh.len();
    merge(&mut cache, fresh);
    save_cache(data_dir, &cache);

    let curse_failed = if curse_error { curse_pending.len() } else { 0 };
    let mut report = report_from_cache(&files, &cache);
    report.failed = modrinth_failed + curse_failed;
    let _ = found;
    Ok(report)
}

/// 下载最新文件并替换掉旧文件。返回替换后的新文件名。
pub fn apply(runtime_dir: &Path, update: &ModUpdate) -> Result<String, CoreError> {
    if update.latest_url.is_empty() || update.latest_file_name.is_empty() {
        return Err(CoreError::InvalidInput("没有可下载的新版本".into()));
    }
    let dir = mods_dir(runtime_dir);
    // 沿用旧文件名风格，禁用状态也跟着走。
    let mut merged = crate::mods::merge_file_name(&update.file_name, &update.latest_file_name);
    if update.file_name.to_ascii_lowercase().ends_with(".disabled")
        && !merged.to_ascii_lowercase().ends_with(".disabled")
    {
        merged.push_str(".disabled");
    }
    let target = unique_name(&dir, &merged);
    let temp = dir.join(format!(".{merged}.ybmcl-new"));
    let _ = fs::remove_file(&temp);

    let mut urls = vec![update.latest_url.clone()];
    if let Some(mirror) = crate::download::mirror_url(&update.latest_url) {
        urls.push(mirror);
    }
    let job = crate::download::DownloadJob {
        urls,
        dest: temp.clone(),
        size: None,
        sha1: if update.latest_sha1.trim().is_empty() {
            None
        } else {
            Some(update.latest_sha1.to_ascii_lowercase())
        },
    };
    crate::download::fetch_files_to(
        &[job],
        &crate::download::CancellationToken::default(),
        4,
        crate::download::DownloadStrategy::default(),
        |_| {},
    )
    .map_err(|error| CoreError::InvalidInput(format!("下载新版本失败：{error}")))?;

    // 先把旧文件改名留底，再换上新的；任一步失败都复原，最后才把留底送进回收站。
    let old = update.file_name.clone();
    let old_path = dir.join(&old);
    let backup = dir.join(format!(".{old}.ybmcl-old"));
    let _ = fs::remove_file(&backup);
    let had_old = old_path.exists();
    if had_old {
        fs::rename(&old_path, &backup).map_err(|error| {
            let _ = fs::remove_file(&temp);
            crate::io_error(&old_path, error)
        })?;
    }
    if let Err(error) = fs::rename(&temp, &target) {
        let _ = fs::remove_file(&temp);
        if had_old {
            let _ = fs::rename(&backup, &old_path);
        }
        return Err(crate::io_error(&target, error));
    }
    if had_old {
        let _ = crate::mods::recycle_path(&backup);
    }
    Ok(target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| update.latest_file_name.clone()))
}

struct LocalFile {
    file_name: String,
    sha1: String,
    murmur: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheEntry {
    source: String,
    project: String,
    /// 本地版本号（Modrinth 才有，读的时候从文件里补）。
    #[serde(default)]
    current: String,
    latest: String,
    file_name: String,
    url: String,
    /// 目标文件的 sha1；和本地相同就说明已经是最新。
    #[serde(default)]
    latest_sha1: String,
    at: u64,
    ok: bool,
    /// 缓存里记下本地文件名，换了文件自然就查不到了。
    #[serde(default)]
    local_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CacheFile {
    #[serde(default)]
    game: String,
    #[serde(default)]
    loader: String,
    #[serde(default)]
    entries: HashMap<String, CacheEntry>,
}

fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CACHE_FILE)
}

fn load_cache(data_dir: &Path, game: &str, loader: &str) -> CacheFile {
    let path = cache_path(data_dir);
    let Ok(text) = fs::read_to_string(&path) else {
        return CacheFile {
            game: game.to_owned(),
            loader: loader.to_owned(),
            ..Default::default()
        };
    };
    let Ok(mut cache) = serde_json::from_str::<CacheFile>(&text) else {
        return CacheFile {
            game: game.to_owned(),
            loader: loader.to_owned(),
            ..Default::default()
        };
    };
    // 换游戏版本或加载器，旧结果一律作废。
    if !cache.game.eq_ignore_ascii_case(game) || !cache.loader.eq_ignore_ascii_case(loader) {
        cache.entries.clear();
    }
    cache.game = game.to_owned();
    cache.loader = loader.to_owned();
    cache
}

fn save_cache(data_dir: &Path, cache: &CacheFile) {
    // 一条都没有就不写，别把「网络失败」缓存成「没有更新」。
    if cache.entries.is_empty() {
        return;
    }
    let path = cache_path(data_dir);
    let Ok(text) = serde_json::to_string_pretty(cache) else {
        return;
    };
    let temp = path.with_extension("json.tmp");
    if fs::write(&temp, text).is_ok() {
        let _ = fs::rename(&temp, &path);
    }
}

fn merge(cache: &mut CacheFile, fresh: HashMap<String, CacheEntry>) {
    for (sha1, entry) in fresh {
        cache.entries.insert(sha1, entry);
    }
}

/// 把缓存翻译成界面用的结果。
fn report_from_cache(files: &[LocalFile], cache: &CacheFile) -> UpdateReport {
    let mut updates = Vec::new();
    let mut identified = 0;
    for file in files {
        let Some(entry) = cache.entries.get(&file.sha1) else {
            continue;
        };
        if !entry.ok {
            continue;
        }
        identified += 1;
        // 目标文件就是本地这个文件 → 已经是最新。
        let same = !entry.latest_sha1.is_empty() && entry.latest_sha1.eq_ignore_ascii_case(&file.sha1);
        let current_version = if entry.source == "modrinth" && !entry.current.is_empty() {
            entry.current.clone()
        } else {
            strip_disabled(&file.file_name)
        };
        updates.push(ModUpdate {
            file_name: file.file_name.clone(),
            source: entry.source.clone(),
            project_id: entry.project.clone(),
            current_version,
            latest_version: entry.latest.clone(),
            latest_file_name: entry.file_name.clone(),
            latest_url: entry.url.clone(),
            latest_sha1: entry.latest_sha1.clone(),
            can_update: !same && !entry.url.is_empty(),
        });
    }
    UpdateReport {
        updates,
        identified,
        failed: 0,
        cached: false,
    }
}

fn strip_disabled(name: &str) -> String {
    name.strip_suffix(".disabled").unwrap_or(name).to_owned()
}

fn none_if_empty(value: &str) -> Option<&str> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

fn mods_dir(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("mods")
}

fn local_files(
    dir: &Path,
    cancel: &AtomicBool,
    tick: &mut dyn FnMut(&str, usize, usize),
) -> Result<Vec<LocalFile>, CoreError> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(error) => return Err(crate::io_error(dir, error)),
    };
    let names: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .map(|name| {
                        let lower = name.to_string_lossy().to_lowercase();
                        lower.ends_with(".jar") || lower.ends_with(".jar.disabled")
                    })
                    .unwrap_or(false)
        })
        .collect();
    let total = names.len();
    for (index, path) in names.into_iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let mut hasher = Sha1::new();
        hasher.update(&bytes);
        out.push(LocalFile {
            file_name: name,
            sha1: hex(&hasher.finalize()),
            murmur: export::curseforge_fingerprint(&bytes),
        });
        if index % 8 == 0 || index + 1 == total {
            tick("hash", index + 1, total);
        }
    }
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 目标文件名已经存在且不是同一个文件时，加序号避免覆盖别人的文件。
fn unique_name(dir: &Path, wanted: &str) -> PathBuf {
    let candidate = dir.join(wanted);
    if !candidate.exists() {
        return candidate;
    }
    let stem = wanted
        .strip_suffix(".jar")
        .or_else(|| wanted.strip_suffix(".JAR"))
        .unwrap_or(wanted);
    for index in 2..1000 {
        let candidate = dir.join(format!("{stem}-{index}.jar"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(wanted)
}
