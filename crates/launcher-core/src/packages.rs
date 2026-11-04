//! 安装包：把某个加载器（或原版客户端）的文件单独下载到用户选的目录。
//!
//! 和 `instance` 的区别是这里不创建实例、不写 version json、不碰 `versions/`：
//! 用户只是想拿到那个 jar 自己放，或者留档、或者给别的启动器用。

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

use serde::Serialize;

use crate::{
    download::{self, CancellationToken, RemoteVersion},
    io_error,
    modloader::{self, ModLoader},
    CoreError, Result,
};

/// 一个待下载的安装包文件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageFile {
    /// 保存时用的文件名，例如 `forge-1.20.1-47.4.9-installer.jar`。
    pub name: String,
    pub url: String,
}

/// 下载错误与核心错误是两套 enum，这里统一折成「无效输入」，只为把信息带回界面。
fn dl(error: download::DownloadError) -> CoreError {
    CoreError::InvalidInput(error.to_string())
}

fn file_from_url(url: &str, fallback: &str) -> PackageFile {
    let name = url
        .rsplit('/')
        .next()
        .filter(|part| !part.is_empty() && part.contains('.'))
        .unwrap_or(fallback)
        .to_owned();
    PackageFile {
        name,
        url: url.to_owned(),
    }
}

/// 原版客户端：从官方（或镜像）清单里找到这个版本，再取 `downloads.client`。
fn minecraft_client(
    game: &str,
    strategy: download::DownloadStrategy,
    cancel: &CancellationToken,
) -> Result<PackageFile> {
    let manifest = download::list_remote_versions(strategy).map_err(dl)?;
    let entry: &RemoteVersion = manifest
        .versions
        .iter()
        .find(|item| item.id == game)
        .ok_or_else(|| CoreError::InvalidInput(format!("未知的游戏版本 {game}")))?;
    let raw = download::fetch_catalog_bytes(&entry.url, cancel).map_err(dl)?;
    let json: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|error| CoreError::InvalidInput(format!("版本清单格式不对：{error}")))?;
    let url = json["downloads"]["client"]["url"]
        .as_str()
        .ok_or_else(|| CoreError::InvalidInput(format!("版本 {game} 没有客户端下载地址")))?;
    Ok(PackageFile {
        name: format!("{game}.jar"),
        url: url.to_owned(),
    })
}

/// 从 meta 接口里取一个加载器官方安装器的下载地址。
///
/// Fabric / Legacy Fabric 的 `/v2/versions/installer` 直接给出 jar 的 url；
/// Quilt 没有这类接口，读 maven 的 `maven-metadata.xml` 里的版本号再拼。
fn official_installer_url(kind: &str, cancel: &CancellationToken) -> Result<String> {
    match kind {
        "fabric" | "legacyfabric" => {
            let meta = if kind == "fabric" {
                "https://meta.fabricmc.net/v2/versions/installer"
            } else {
                "https://meta.legacyfabric.net/v2/versions/installer"
            };
            let raw = download::fetch_catalog_bytes(meta, cancel).map_err(dl)?;
            let list: Vec<serde_json::Value> = serde_json::from_slice(&raw)
                .map_err(|error| CoreError::InvalidInput(format!("安装器清单格式不对：{error}")))?;
            list.iter()
                .find(|item| item["stable"].as_bool().unwrap_or(false))
                .or_else(|| list.first())
                .and_then(|item| item["url"].as_str())
                .map(str::to_owned)
                .ok_or_else(|| CoreError::InvalidInput(format!("{kind} 没有可用的安装器")))
        }
        "quilt" => {
            let meta = "https://maven.quiltmc.org/repository/release/org/quiltmc/quilt-installer/maven-metadata.xml";
            let raw = download::fetch_catalog_bytes(meta, cancel).map_err(dl)?;
            let text = String::from_utf8_lossy(&raw);
            let latest = text
                .split("<release>")
                .nth(1)
                .and_then(|rest| rest.split("</release>").next())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    text.rsplit("<version>")
                        .next()
                        .and_then(|rest| rest.split("</version>").next())
                        .map(str::trim)
                })
                .ok_or_else(|| CoreError::InvalidInput("Quilt 安装器版本号读不到".into()))?;
            Ok(format!(
                "https://maven.quiltmc.org/repository/release/org/quiltmc/quilt-installer/{latest}/quilt-installer-{latest}.jar"
            ))
        }
        other => Err(CoreError::InvalidInput(format!("{other} 没有官方安装器"))),
    }
}

/// 加载器的安装器。
fn loader_installer(
    kind: &str,
    game: &str,
    version: &str,
    cancel: &CancellationToken,
) -> Result<PackageFile> {
    let loader = match kind {
        "forge" => ModLoader::Forge,
        "neoforge" => ModLoader::NeoForge,
        "cleanroom" => ModLoader::Cleanroom,
        "liteloader" => ModLoader::LiteLoader,
        "optifine" => ModLoader::OptiFine,
        "labymod" => {
            let url = format!(
                "{}/labymod4/production/{version}.jar",
                download::LABYMOD_DOWNLOAD
            );
            return Ok(file_from_url(&url, "labymod-installer.jar"));
        }
        "fabric" | "legacyfabric" | "quilt" => {
            let url = official_installer_url(kind, cancel)?;
            return Ok(file_from_url(&url, "installer.jar"));
        }
        other => {
            return Err(CoreError::InvalidInput(format!(
                "{other} 没有可以单独下载的安装器"
            )))
        }
    };
    let request = modloader::ModLoaderInstallRequest {
        game_dir: String::new(),
        game_version: game.to_owned(),
        loader,
        loader_version: version.to_owned(),
        java_path: String::new(),
        strategy: download::DownloadStrategy::default(),
        threads: None,
    };
    let url = modloader::installer_download(&request).map_err(dl)?;
    Ok(file_from_url(&url, "installer.jar"))
}

/// 这个「类别 + 游戏版本 + 版本号」要下载的文件。
pub fn package_files(
    kind: &str,
    game: &str,
    version: &str,
    strategy: download::DownloadStrategy,
    cancel: &CancellationToken,
) -> Result<Vec<PackageFile>> {
    let file = if kind == "minecraft" {
        minecraft_client(game, strategy, cancel)?
    } else {
        loader_installer(kind, game, version, cancel)?
    };
    Ok(vec![file])
}

/// 原版客户端的「手动安装包」：`<版本>.jar` + `<版本>.json`，再按 PCL 的做法把
/// json 里 `relativeUrl` 指到的库文件也一起塞进去（放 `libraries/` 下）。
///
/// 返回整包字节，交给命令层写到用户选的位置。
pub fn vanilla_zip(
    game: &str,
    game_dir: &Path,
    strategy: download::DownloadStrategy,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    vanilla_zip_progress(game, game_dir, strategy, cancel, &|_, _| {})
}

/// 同 [`vanilla_zip`]，但边下边报「已收字节 / 总量」。
///
/// 客户端 jar 是整包的大头（1.21 就 60 MB 往上），任务卡的百分比只能从这里拿。
/// 分母先用版本 json 里写明的 size 累出来（客户端 + 库文件），界面上从第一个字节
/// 起就有真实进度；本地已有的文件按 size 记成已完成，进度会合法地往前跳一截。
pub fn vanilla_zip_progress(
    game: &str,
    game_dir: &Path,
    strategy: download::DownloadStrategy,
    cancel: &CancellationToken,
    on_progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<Vec<u8>> {
    let manifest = download::list_remote_versions(strategy).map_err(dl)?;
    let entry: &RemoteVersion = manifest
        .versions
        .iter()
        .find(|item| item.id == game)
        .ok_or_else(|| CoreError::InvalidInput(format!("未知的游戏版本 {game}")))?;
    let raw = download::fetch_catalog_bytes(&entry.url, cancel).map_err(dl)?;
    let json: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|error| CoreError::InvalidInput(format!("版本清单格式不对：{error}")))?;
    let client_url = json["downloads"]["client"]["url"]
        .as_str()
        .ok_or_else(|| CoreError::InvalidInput(format!("版本 {game} 没有客户端下载地址")))?;
    // 本地已经有这个客户端（装过同版本，或在下载目录里存过）就不必重下几十兆。
    let expected_sha1 = json["downloads"]["client"]["sha1"].as_str();
    let local_client = ["versions", "downloads"]
        .iter()
        .map(|folder| game_dir.join(folder).join(format!("{game}.jar")))
        .find_map(|candidate| {
            let digest = download::file_sha1(&candidate).ok()?;
            if Some(digest.as_str()) == expected_sha1 {
                fs::read(&candidate).ok()
            } else {
                None
            }
        });

    let libraries = library_entries(&json);
    let total = json["downloads"]["client"]["size"].as_u64().unwrap_or(0)
        + libraries.iter().filter_map(|item| item.size).sum::<u64>();
    let mut done = 0u64;
    // 每读 64 KB 回调一次，全量转发会把 IPC 打爆；120 ms 节流，界面仍看得见在涨。
    let last = Mutex::new(Instant::now() - Duration::from_secs(1));
    let report = |done: u64, total: u64| {
        let due = last
            .lock()
            .map(|mut at| {
                if at.elapsed() >= Duration::from_millis(120) {
                    *at = Instant::now();
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if due {
            on_progress(done.min(total), total);
        }
    };

    let client = match local_client {
        Some(bytes) => {
            done += bytes.len() as u64;
            report(done, total);
            bytes
        }
        // 带上版本号，客户端 jar 才会同时生成官方和 BMCLAPI 两条候选（分块竞速要用）。
        None => {
            let received = AtomicU64::new(0);
            let base = done;
            let bytes = download::fetch_file_bytes_for_version_progress(
                client_url,
                game,
                cancel,
                &received,
                &|| report(base + received.load(Ordering::Relaxed), total),
            )
            .map_err(dl)?;
            done += bytes.len() as u64;
            report(done, total);
            bytes
        }
    };
    let library_files = library_files(&json, game_dir, cancel, |size| {
        done += size;
        report(done, total);
    })?;

    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        // 客户端和库是已经压过的 jar，deflate 收益有限（129 MB → 116 MB），
        // 只多两秒，省下 13 MB 还是划算，保持 Deflated。
        let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let zip_error = |error: zip::result::ZipError| CoreError::Io {
            path: format!("{game}.zip"),
            source: std::io::Error::other(error.to_string()),
        };
        zip.start_file(format!("{game}.json"), options)
            .map_err(zip_error)?;
        zip.write_all(&raw).map_err(|error| CoreError::Io {
            path: format!("{game}.json"),
            source: error,
        })?;
        zip.start_file(format!("{game}.jar"), options)
            .map_err(zip_error)?;
        zip.write_all(&client).map_err(|error| CoreError::Io {
            path: format!("{game}.jar"),
            source: error,
        })?;
        for (relative, bytes) in library_files {
            let entry_name = format!("libraries/{relative}");
            zip.start_file(&entry_name, options).map_err(zip_error)?;
            zip.write_all(&bytes).map_err(|error| CoreError::Io {
                path: entry_name.clone(),
                source: error,
            })?;
        }
        zip.finish().map_err(zip_error)?;
    }
    // 收尾补一条满进度：上面的总量是按 json 的 size 估的，缺 size 的条目会少算。
    let final_total = total.max(done);
    on_progress(final_total, final_total);
    Ok(buffer.into_inner())
}

/// 要打进包里的一个库文件：包内相对路径、缺货时的下载地址、输出大小。
struct LibraryEntry {
    relative: String,
    url: Option<String>,
    size: Option<u64>,
}

/// 从版本 json 里挑出 `relativeUrl`（或 `path`）指向的库文件。
fn library_entries(json: &serde_json::Value) -> Vec<LibraryEntry> {
    let mut wanted: Vec<LibraryEntry> = Vec::new();
    let mut push = |path: Option<&str>, url: Option<&str>, size: Option<u64>| {
        let Some(path) = path.filter(|value| !value.trim().is_empty()) else {
            return;
        };
        let path = path.trim_start_matches('/').to_owned();
        if wanted.iter().any(|entry| entry.relative == path) {
            return;
        }
        wanted.push(LibraryEntry {
            relative: path,
            url: url.map(str::to_owned),
            size,
        });
    };
    for library in json["libraries"].as_array().into_iter().flatten() {
        // 清单里 linux / macOS 的原生库是单独的 library 条目（name 以 `:natives-<os>` 结尾），
        // 按 rules 只收「在 Windows 上生效」的那些，免得把 40 个用不上的包也拉一遍。
        let allowed = match library["rules"].as_array() {
            None => true,
            Some(rules) => rules.iter().any(|rule| {
                rule["action"].as_str() == Some("allow")
                    && match rule["os"]["name"].as_str() {
                        None => true,
                        Some(name) => name == "windows",
                    }
            }),
        };
        if !allowed {
            continue;
        }
        let artifact = &library["downloads"]["artifact"];
        push(
            artifact["relativeUrl"].as_str(),
            artifact["url"].as_str(),
            artifact["size"].as_u64(),
        );
        push(
            artifact["path"].as_str(),
            artifact["url"].as_str(),
            artifact["size"].as_u64(),
        );
        push(library["relativeUrl"].as_str(), None, None);
        for (name, classifier) in library["downloads"]["classifiers"]
            .as_object()
            .into_iter()
            .flatten()
        {
            // 只带当前平台的原生库：linux/macos 的 natives 放进去也用不上，还会拖慢打包。
            let relative = classifier["relativeUrl"]
                .as_str()
                .or_else(|| classifier["path"].as_str())
                .unwrap_or_default();
            let windows = name.to_ascii_lowercase().contains("windows")
                || relative.to_ascii_lowercase().contains("windows");
            if !windows {
                continue;
            }
            push(
                classifier["relativeUrl"].as_str(),
                classifier["url"].as_str(),
                classifier["size"].as_u64(),
            );
            push(
                classifier["path"].as_str(),
                classifier["url"].as_str(),
                classifier["size"].as_u64(),
            );
        }
    }
    wanted
}

/// 逐个把库文件准备好（本地有就直接读，没有就按 url 补下来），每就位一个报一次字节数。
fn library_files(
    json: &serde_json::Value,
    game_dir: &Path,
    cancel: &CancellationToken,
    mut on_file: impl FnMut(u64),
) -> Result<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    for entry in library_entries(json) {
        let local = game_dir.join("libraries").join(&entry.relative);
        let bytes = match fs::read(&local) {
            Ok(bytes) => bytes,
            Err(_) => {
                let Some(url) = entry.url.as_deref() else {
                    continue;
                };
                download::fetch_file_bytes(url, cancel).map_err(dl)?
            }
        };
        on_file(bytes.len() as u64);
        files.push((entry.relative, bytes));
    }
    Ok(files)
}

/// 从版本 json 里挑出 `relativeUrl`（或 `path`）指向的库文件，本地有就直接读，
/// 没有就按 artifact 的 url 补下来。
#[cfg(test)]
pub(crate) fn relative_library_files(
    json: &serde_json::Value,
    game_dir: &Path,
    cancel: &CancellationToken,
) -> Result<Vec<(String, Vec<u8>)>> {
    library_files(json, game_dir, cancel, |_| {})
}

/// 把所有文件下载到 `dir`，返回实际写出的路径。
pub fn download_into(
    files: &[PackageFile],
    dir: &Path,
    cancel: &CancellationToken,
) -> Result<Vec<String>> {
    fs::create_dir_all(dir).map_err(|error| io_error(dir, error))?;
    let mut written = Vec::with_capacity(files.len());
    for file in files {
        let bytes = download::fetch_file_bytes(&file.url, cancel).map_err(dl)?;
        let target = dir.join(&file.name);
        fs::write(&target, &bytes).map_err(|error| io_error(&target, error))?;
        written.push(target.to_string_lossy().into_owned());
    }
    Ok(written)
}

/// 下载目录不存在时先建出来（界面上给默认路径用）。
/// 把整个实例目录打成 zip，附一份 `manifest.json` 说明来源。
///
/// 目录结构保持原样（`mods/`、`config/`、`saves/` 都进包），`manifest.json` 写在根上。
/// 进度按「已写文件数 / 总文件数」回调，调用方自己节流。
pub fn export_instance(
    instance_dir: &Path,
    out: &Path,
    name: &str,
    game_version: &str,
    loaders: &[String],
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<u64> {
    if !instance_dir.is_dir() {
        return Err(CoreError::InvalidInput(format!(
            "实例目录不存在：{}",
            instance_dir.display()
        )));
    }
    // 先数一遍文件，顺便得到总字节数，进度才有分母。
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    let mut stack = vec![instance_dir.to_path_buf()];
    let mut total = 0u64;
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| io_error(&dir, e))? {
            let entry = entry.map_err(|e| io_error(&dir, e))?;
            let path = entry.path();
            let meta = entry.metadata().map_err(|e| io_error(&path, e))?;
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let rel = path
                .strip_prefix(instance_dir)
                .map_err(|e| CoreError::InvalidInput(format!("{}: {e}", path.display())))?
                .to_string_lossy()
                .replace('\\', "/");
            total += meta.len();
            files.push((path, rel));
        }
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));

    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
    }
    let file = fs::File::create(out).map_err(|e| io_error(out, e))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    let manifest = serde_json::json!({
        "format": "ybmcl-instance",
        "name": name,
        "gameVersion": game_version,
        "loaders": loaders,
        "files": files.len(),
        "bytes": total,
    });
    zip.start_file("manifest.json", options)
        .map_err(|e| CoreError::InvalidInput(format!("写入清单失败：{e}")))?;
    zip.write_all(serde_json::to_string_pretty(&manifest).unwrap_or_default().as_bytes())
        .map_err(|e| CoreError::InvalidInput(format!("写入清单失败：{e}")))?;

    let count = files.len() as u64;
    let mut done = 0u64;
    on_progress(0, count);
    for (path, rel) in files {
        if cancel.is_cancelled() {
            return Err(CoreError::InvalidInput("导出已取消".into()));
        }
        let data = fs::read(&path).map_err(|e| io_error(&path, e))?;
        zip.start_file(&rel, options)
            .map_err(|e| CoreError::InvalidInput(format!("写入 {rel} 失败：{e}")))?;
        zip.write_all(&data)
            .map_err(|e| CoreError::InvalidInput(format!("写入 {rel} 失败：{e}")))?;
        done += 1;
        on_progress(done, count);
    }
    zip.finish()
        .map_err(|e| CoreError::InvalidInput(format!("收尾失败：{e}")))?;
    Ok(total)
}

pub fn ensure_dir(dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir).map_err(|error| io_error(dir, error))?;
    Ok(dir.to_path_buf())
}

/// 把一个文件直接写到用户选定的路径（另存为）。
pub fn download_file_to(url: &str, target: &Path, cancel: &CancellationToken) -> Result<String> {
    download_file_to_progress(url, target, cancel, |_, _| {})
}

/// 同上，但边写边回报 `(已写字节, 总字节)`，给任务的进度环用。
pub fn download_file_to_progress(
    url: &str,
    target: &Path,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<String> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    }
    let (mut reader, total) = download::open_file_stream(url).map_err(dl)?;
    let mut file = fs::File::create(target).map_err(|error| io_error(target, error))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut written = 0u64;
    let mut last = std::time::Instant::now();
    on_progress(0, total);
    loop {
        if cancel.is_cancelled() {
            return Err(dl(download::DownloadError::Cancelled));
        }
        let read = std::io::Read::read(&mut reader, &mut buffer)
            .map_err(|error| dl(download::DownloadError::Network(error.to_string())))?;
        if read == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buffer[..read])
            .map_err(|error| io_error(target, error))?;
        written += read as u64;
        if last.elapsed().as_millis() >= 120 {
            on_progress(written, total);
            last = std::time::Instant::now();
        }
    }
    std::io::Write::flush(&mut file).map_err(|error| io_error(target, error))?;
    on_progress(written, Some(written));
    Ok(target.to_string_lossy().into_owned())
}
