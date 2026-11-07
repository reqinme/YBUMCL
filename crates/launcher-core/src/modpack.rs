//! 整合包安装：`.mrpack`（Modrinth）与 CurseForge 整合包 zip。
//!
//! 照着 PCL 的做法来：先把清单读全（真实文件名、该落到哪个目录、校验值、
//! 备用地址），再交给并发下载器；缺几个文件不中断整个安装，装完把缺失项报给界面。
//!
//! 包里写明的游戏版本与加载器会交给 [`crate::instance::install_instance`] 建一个
//! 自包含实例，这一步与 mod 下载并行跑；`overrides/`（Modrinth 还有
//! `client-overrides/`）最后解压覆盖上去，包在 `saves/` 里的地图 zip 顺手解开。

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::thread;

use serde::Deserialize;

use crate::download::{self, CancellationToken, DownloadJob};
use crate::instance::{install_instance, InstanceRequest, LoaderChoice};
use crate::{world, CoreError, Result};

/// 从整合包文件里读出来的清单（更新时用来算差异）。
#[derive(Debug, Clone)]
pub struct PackFiles {
    pub name: String,
    pub version: String,
    pub game_version: String,
    pub loaders: Vec<(String, String)>,
    /// 相对路径、SHA1（CF 包没有）、下载地址、字节数。
    pub entries: Vec<(String, Option<String>, Vec<String>, Option<u64>)>,
    pub pack: PackOrigin,
}

/// 打开一个 `.mrpack` / CurseForge 整合包 zip，读出清单（不改动任何文件）。
pub fn read_pack_files(path: &Path) -> Result<PackFiles> {
    let file = fs::File::open(path).map_err(io_error)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| CoreError::InvalidInput(format!("打开整合包失败：{error}")))?;
    let root = pack_root(&mut zip).unwrap_or_default();
    let plan = read_plan(&mut zip, &root)?;
    Ok(PackFiles {
        name: plan.name,
        version: plan.version,
        game_version: plan.game_version,
        loaders: plan.loaders,
        entries: plan
            .files
            .into_iter()
            .map(|file| {
                (
                    file.relative.to_string_lossy().replace('\\', "/"),
                    file.sha1,
                    file.urls,
                    file.size,
                )
            })
            .collect(),
        pack: plan.pack,
    })
}

/// 一次整合包安装的请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModpackRequest {
    /// `.minecraft` 目录。
    pub game_dir: String,
    /// 整合包文件的下载地址（.mrpack 或 CurseForge 的 .zip）。
    #[serde(default)]
    pub url: Option<String>,
    /// 本地整合包文件（从「导入整合包」进来时用它，不下载）。
    #[serde(default)]
    pub path: Option<String>,
    /// 文件名，仅用于临时文件与取默认实例名。
    #[serde(default)]
    pub file_name: Option<String>,
    /// 用户指定的实例名，留空则用「整合包名 版本」。
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub java_path: Option<String>,
    /// 下载源策略（竞速/官方优先/镜像优先/仅镜像/仅官方）。
    #[serde(default)]
    pub strategy: crate::download::DownloadStrategy,
    /// 并发下载的线程数（1–256，留空用 64）。
    #[serde(default)]
    pub threads: Option<u32>,
    /// 资源站上这个整合包的图标地址；装完顺手存成实例图标。
    #[serde(default)]
    pub icon_url: Option<String>,
}

/// 安装结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModpackSummary {
    /// 建出来的实例名。
    pub version_id: String,
    /// 建出来的实例目录（`versions/<实例名>`），界面用来「打开文件夹 / 删除文件」。
    pub path: String,
    /// 整合包里声明的游戏版本。
    pub game_version: String,
    /// 加载器（可能为空：纯原版整合包）。
    pub loaders: Vec<String>,
    /// 写进实例的文件数。
    pub files: u64,
    /// 没下载成功的文件数（网络超时之类，可以重试）。
    pub failed: usize,
    /// 整合包要求、但资源站上已经找不到的条目（作者删了文件）。
    pub missing: Vec<String>,
    /// 这个包是从哪个项目装来的，写进实例的 `ybmcl.json` 供概览页显示。
    pub pack: Option<PackOrigin>,
}

/// 整合包的来源：Modrinth 能反查到项目，CurseForge 的 manifest 只有名字与版本号。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackOrigin {
    /// `modrinth` / `curseforge` / `ftb` / `zip`（认不出项目时用 `zip`）。
    pub source: String,
    pub project_id: String,
    pub project_name: String,
    /// 整合包自己的版本号。
    pub version_label: String,
    pub summary: String,
    pub downloads: u64,
    pub tags: Vec<String>,
}

impl PackOrigin {
    /// 认不出项目时只留名字与版本号，界面显示成「本地整合包」。
    fn local(name: &str, version: &str) -> Self {
        Self {
            source: "zip".into(),
            project_id: String::new(),
            project_name: name.trim().to_owned(),
            version_label: version.trim().to_owned(),
            summary: String::new(),
            downloads: 0,
            tags: Vec::new(),
        }
    }

    /// Modrinth 的项目：版本号用「版本 id 反查」拿到的版本号，拿不到就退回包里写的 id。
    fn modrinth(project: crate::modrinth::MrProject, version_number: &str, fallback: &str) -> Self {
        Self {
            source: "modrinth".into(),
            project_id: project.id,
            project_name: project.title,
            version_label: if version_number.trim().is_empty() {
                fallback.trim().to_owned()
            } else {
                version_number.trim().to_owned()
            },
            summary: project.description,
            downloads: project.downloads,
            tags: project.categories,
        }
    }
}

/// 安装过程中往外报的进度。
///
/// `stage` 是阶段名，`done/total` 是这一阶段完成的份数，`bytes` 是已下载字节，
/// `file` 是当前正在下的文件名（界面拿它显示，卡住时能看出卡在哪个文件）。
pub struct Step<'a> {
    pub stage: &'a str,
    pub done: u64,
    pub total: u64,
    pub bytes: u64,
    pub file: Option<&'a str>,
}

/// 只报阶段、不带文件列表的进度（下载包本身、装加载器、解压覆盖这几步）。
fn step(stage: &str, done: u64, total: u64) -> Step<'_> {
    Step {
        stage,
        done,
        total,
        bytes: 0,
        file: None,
    }
}

/// 下载（或读入本地文件）并安装一个整合包。
pub fn install_modpack(
    request: ModpackRequest,
    cancel: CancellationToken,
    progress: impl Fn(&Step) + Send + Sync,
) -> Result<ModpackSummary> {
    let game_dir = PathBuf::from(&request.game_dir);
    let result = install_modpack_inner(request, cancel.clone(), &progress);
    // 失败（取消、断网、校验不过都算；暂停除外）：把这次留下的暂存目录与半截 `.part` 清干净。
    if result.is_err() && !cancel.is_paused() {
        // 整块 `.ybmcl/modpack` 都清（暂存目录 + 已经下完但没装成的包 zip），
        // 不然取消一次会白留几百 MB。
        let _ = fs::remove_dir_all(game_dir.join(".ybmcl").join("modpack"));
        crate::merge::sweep_parts(&game_dir);
    }
    result
}

fn install_modpack_inner(
    request: ModpackRequest,
    cancel: CancellationToken,
    progress: &(impl Fn(&Step) + Send + Sync),
) -> Result<ModpackSummary> {
    let game_dir = PathBuf::from(&request.game_dir);
    // 新拷出来的启动器还没有 `.minecraft`，安装时现建，别让用户看到「目录不存在」。
    fs::create_dir_all(&game_dir).map_err(io_error)?;
    let tmp = game_dir.join(".ybmcl").join("modpack");
    fs::create_dir_all(&tmp).map_err(io_error)?;
    let archive = tmp.join("pack.zip");

    match request.path.as_deref().filter(|path| !path.is_empty()) {
        Some(path) => {
            progress(&step("读取整合包", 0, 1));
            fs::copy(path, &archive).map_err(io_error)?;
            progress(&step("读取整合包", 1, 1));
        }
        None => {
            let url = request
                .url
                .clone()
                .filter(|url| !url.is_empty())
                .ok_or_else(|| CoreError::InvalidInput("没有整合包地址".into()))?;
            progress(&Step {
                stage: "下载整合包",
                done: 0,
                total: 1,
                bytes: 0,
                file: None,
            });
            let bytes = download::fetch_file_bytes(&url, &cancel)
                .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
            let mut file = fs::File::create(&archive).map_err(io_error)?;
            file.write_all(&bytes).map_err(io_error)?;
            drop(file);
            progress(&step("下载整合包", 1, 1));
        }
    }

    let mut zip = zip::ZipArchive::new(fs::File::open(&archive).map_err(io_error)?)
        .map_err(|e| CoreError::InvalidInput(format!("整合包不是有效的 zip：{e}")))?;

    // 有的整合包会把内容再套一层文件夹，PCL 叫它 ArchiveBaseFolder。
    let root = pack_root(&mut zip)
        .ok_or_else(|| CoreError::InvalidInput("不认识的整合包格式（没有 manifest.json 或 modrinth.index.json）".into()))?;
    let plan = read_plan(&mut zip, &root)?;

    let instance_name = unique_name(
        &game_dir,
        &request
            .name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| default_name(&plan.name, &plan.version, &request.file_name)),
    );
    let instance_dir = game_dir.join("versions").join(&instance_name);
    // 文件先下到暂存目录：`install_instance` 用「实例目录里有没有东西」判断重名，
    // 边装边往实例目录写会被它当成同名实例，直接报「已存在」。
    let staging = game_dir
        .join(".ybmcl")
        .join("modpack")
        .join("staging")
        .join(&instance_name);
    let _ = fs::remove_dir_all(&staging);

    // 目标路径先算出来，下载器和 install_instance 各写各的子目录，互不干扰。
    let mut jobs: Vec<DownloadJob> = Vec::new();
    for file in &plan.files {
        let dest = staging.join(&file.relative);
        let mut urls = file.urls.clone();
        if let Some(url) = file.urls.first().and_then(|url| download::mirror_url(url)) {
            if !urls.contains(&url) {
                urls.push(url);
            }
        }
        if urls.is_empty() {
            continue;
        }
        jobs.push(DownloadJob {
            urls,
            dest,
            size: file.size,
            sha1: file.sha1.clone(),
        });
    }

    let total = jobs.len() as u64;
    progress(&Step {
        stage: "下载整合包内容",
        done: 0,
        total,
        bytes: 0,
        file: None,
    });

    let install_request = InstanceRequest {
        game_dir: request.game_dir.clone(),
        game_version: plan.game_version.clone(),
        instance_name: Some(instance_name.clone()),
        loaders: plan
            .loaders
            .iter()
            .map(|(loader, version)| LoaderChoice {
                loader: loader.clone(),
                version: version.clone(),
            })
            .collect(),
        java_path: request.java_path.clone(),
        strategy: request.strategy,
        threads: None,
        overwrite: false,
    };

    // 装游戏（原版 + 加载器）和下载 mod 并行：PCL 的 LoaderCombo 也是这么排的，
    // 慢网络下能省掉一整段等待。
    let install_cancel = cancel.clone();
    let download_cancel = cancel.clone();
    let progress_ref = &progress;
    let (install_result, download_result) = thread::scope(|scope| {
        let install = scope.spawn(move || {
            install_instance(install_request, install_cancel, |item| {
                progress_ref(&Step {
                    stage: "安装游戏与加载器",
                    done: item.completed_files,
                    total: item.total_files.max(1),
                    bytes: item.downloaded_bytes,
                    file: Some(&item.path),
                });
            })
        });
        let threads = request.threads.unwrap_or(64) as usize;
        let download = download::fetch_files_to(&jobs, &download_cancel, threads, request.strategy, |batch| {
            progress(&Step {
                stage: "下载整合包内容",
                done: batch.done as u64,
                total: batch.total as u64,
                bytes: batch.bytes,
                file: Some(&batch.file),
            });
        });
        let install = install.join();
        (install, download)
    });
    let install = install_result
        .map_err(|_| CoreError::InvalidInput("安装游戏时线程异常退出".into()))?
        .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    let (written, failures) = download_result.map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    if cancel.is_cancelled() {
        return Err(CoreError::InvalidInput("安装已取消".into()));
    }
    let written = written as u64;
    if written == 0 && !jobs.is_empty() {
        return Err(CoreError::InvalidInput(format!(
            "整合包内容下载失败：{}",
            failures.first().cloned().unwrap_or_default()
        )));
    }
    progress(&step("安装游戏与加载器", 1, 1));
    // 下载完再搬进实例目录，此时 install_instance 已经把实例建好了。
    for file in &plan.files {
        let from = staging.join(&file.relative);
        if !from.is_file() {
            continue;
        }
        let to = instance_dir.join(&file.relative);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        fs::rename(&from, &to).or_else(|_| {
            fs::copy(&from, &to).map(|_| ()).and_then(|()| fs::remove_file(&from))
        })
        .map_err(io_error)?;
    }
    let _ = fs::remove_dir_all(&staging);

    // overrides 直接展开到实例目录，和前一步一样都是实例内的相对路径。
    progress(&step("解压覆盖文件", 0, 1));
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| CoreError::InvalidInput(format!("读取整合包条目失败：{e}")))?;
        let name = entry.name().to_string();
        let Some(relative) = override_path(&name, &root) else {
            continue;
        };
        if name.ends_with('/') {
            fs::create_dir_all(instance_dir.join(&relative)).map_err(io_error)?;
            continue;
        }
        let dest = instance_dir.join(&relative);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let mut buffer = Vec::new();
        entry.read_to_end(&mut buffer).map_err(io_error)?;
        fs::write(&dest, &buffer).map_err(io_error)?;
    }
    progress(&step("解压覆盖文件", 1, 1));

    // 整合包习惯把地图以 zip 形式放在 saves/ 里，PCL 装完会顺手解开。
    unpack_saves(&instance_dir);

    let _ = fs::remove_file(&archive);
    // 装完把 `.ybmcl/modpack` 整块删掉：`staging/` 与已装好的包 zip 都不该留在游戏目录里
    // （包 zip 可能几百 MB；空着的 `staging/` 也会让人以为没装完）。
    let _ = fs::remove_dir_all(&tmp);

    // 资源站上的整合包图标顺手落到实例目录里：`version_icon` 会优先认这些文件，
    // 版本列表就不用退回按版本类型画的方块图了。
    if let Some(url) = request.icon_url.as_deref() {
        save_pack_icon(url, &instance_dir);
    }

    // 记下这个实例的整合包身份与文件清单，之后「检查更新 / 增量更新 / 恢复」都靠它。
    {
        let entries: Vec<(String, Option<String>)> = plan
            .files
            .iter()
            .map(|file| {
                (
                    file.relative.to_string_lossy().replace('\\', "/"),
                    file.sha1.clone(),
                )
            })
            .collect();
        let pack_file = request
            .file_name
            .clone()
            .unwrap_or_else(|| plan.name.clone());
        // 清单里声明的版本号优先于文件名（CurseForge 从 zip 装时 version_label 就是文件名）。
        let version_label = if plan.version.trim().is_empty() {
            plan.pack.version_label.clone()
        } else {
            plan.version.trim().to_owned()
        };
        let _ = crate::pack_update::record_install_from_plan(
            &instance_dir,
            &plan.pack,
            &version_label,
            &plan.game_version,
            &plan.loaders,
            &entries,
            &pack_file,
        );
    }

    Ok(ModpackSummary {
        version_id: install.version_id,
        path: crate::download::tidy_path(&instance_dir),
        game_version: plan.game_version,
        loaders: plan
            .loaders
            .iter()
            .map(|(loader, _)| loader.clone())
            .collect(),
        files: written,
        failed: failures.len(),
        missing: plan.missing,
        pack: Some(plan.pack),
    })
}

/// 把 `saves/*.zip` 解成存档文件夹。
fn unpack_saves(instance_dir: &Path) {
    let saves = instance_dir.join("saves");
    let Ok(entries) = fs::read_dir(&saves) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_zip = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
        if !is_zip {
            continue;
        }
        if world::extract_world(&path, &saves).is_ok() {
            let _ = fs::remove_file(&path);
        }
    }
}

/// 把整合包在资源站上的图标存成实例图标（原格式，最多 20 秒）。
fn save_pack_icon(url: &str, instance_dir: &Path) {
    // 已经有人手动换过图标（实例目录里有 icon.*）就别覆盖。
    if ["png", "jpg", "jpeg", "webp", "gif"]
        .iter()
        .any(|ext| instance_dir.join(format!("icon.{ext}")).exists())
    {
        return;
    }
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
    else {
        return;
    };
    let Ok(response) = client.get(url).send() else {
        return;
    };
    if !response.status().is_success() {
        return;
    }
    let Ok(bytes) = response.bytes() else {
        return;
    };
    let Some((mime, flat)) = crate::icons::inspect(&bytes) else {
        return;
    };
    if flat {
        return;
    }
    let extension = if mime == "jpeg" { "jpg" } else { mime };
    let _ = fs::write(instance_dir.join(format!("icon.{extension}")), &bytes);
}

/// 包根目录：根上就有清单就是 `""`，套了一层文件夹就是 `"folder/"`。
fn pack_root(zip: &mut zip::ZipArchive<fs::File>) -> Option<String> {
    const MARKERS: [&str; 2] = ["modrinth.index.json", "manifest.json"];
    for marker in MARKERS {
        if zip.by_name(marker).is_ok() {
            return Some(String::new());
        }
    }
    let mut names = Vec::new();
    for index in 0..zip.len() {
        if let Ok(entry) = zip.by_index(index) {
            names.push(entry.name().to_string());
        }
    }
    for name in names {
        let Some((dir, file)) = name.split_once('/') else {
            continue;
        };
        if !dir.is_empty() && !file.contains('/') && MARKERS.contains(&file) {
            return Some(format!("{dir}/"));
        }
    }
    None
}

/// 一个待下载条目（路径相对实例目录）。
struct PlanFile {
    relative: PathBuf,
    urls: Vec<String>,
    /// 包里写明的文件大小（mrpack 的 fileSize / CF 的 fileLength），下载超时按它放宽。
    size: Option<u64>,
    /// SHA1，下完校验。
    sha1: Option<String>,
}

/// 从包里读出来的安装计划。
struct Plan {
    name: String,
    version: String,
    game_version: String,
    loaders: Vec<(String, String)>,
    files: Vec<PlanFile>,
    /// 整合包要了、但资源站上没有的条目（作者删了文件）。
    missing: Vec<String>,
    /// 这个包是从哪个项目来的（自己导入、又反查不到项目时用「本地整合包」）。
    pack: PackOrigin,
}

fn read_plan(zip: &mut zip::ZipArchive<fs::File>, root: &str) -> Result<Plan> {
    let index_name = format!("{root}modrinth.index.json");
    if zip.by_name(&index_name).is_ok() {
        let mut raw = String::new();
        zip.by_name(&index_name)
            .map_err(|e| CoreError::InvalidInput(format!("读取 modrinth.index.json 失败：{e}")))?
            .read_to_string(&mut raw)
            .map_err(io_error)?;
        let index: MrIndex = serde_json::from_str(&raw)
            .map_err(|e| CoreError::InvalidInput(format!("modrinth.index.json 解析失败：{e}")))?;
        let game_version = index
            .dependencies
            .get("minecraft")
            .cloned()
            .ok_or_else(|| CoreError::InvalidInput("整合包没有写游戏版本".into()))?;
        let mut loaders = Vec::new();
        for (key, loader) in [
            ("fabric-loader", "fabric"),
            ("quilt-loader", "quilt"),
            ("forge", "forge"),
            ("neoforge", "neoforge"),
        ] {
            if let Some(version) = index.dependencies.get(key) {
                loaders.push((loader.to_string(), version.clone()));
            }
        }
        let mut files = Vec::new();
        for item in index.files {
            // 只给服务端的条目没必要装进客户端实例；optional 仍然装上，
            // 免得装完缺东西（PCL 会逐个问，这里先按「装上」处理）。
            let unsupported = item
                .env
                .as_ref()
                .and_then(|env| env.client.as_deref())
                .is_some_and(|client| client == "unsupported");
            if unsupported {
                continue;
            }
            let Some(relative) = safe_relative(&item.path) else {
                continue;
            };
            files.push(PlanFile {
                relative,
                urls: item.downloads,
                size: item.file_size,
                sha1: item.hashes.and_then(|hashes| hashes.sha1),
            });
        }
        let name = index.name.unwrap_or_else(|| "整合包".into());
        let version = index.version_id.unwrap_or_default();
        // 包里只有版本 id：能连上 Modrinth 就反查出项目，离线就只留名字（显示成「本地整合包」）。
        let pack = match version.trim() {
            "" => PackOrigin::local(&name, ""),
            id => match crate::modrinth::version_owner(id) {
                Some((project, number)) => PackOrigin::modrinth(project, &number, id),
                None => PackOrigin::local(&name, ""),
            },
        };
        return Ok(Plan {
            name,
            version,
            game_version,
            loaders,
            files,
            missing: Vec::new(),
            pack,
        });
    }

    let manifest_name = format!("{root}manifest.json");
    if zip.by_name(&manifest_name).is_ok() {
        let mut raw = String::new();
        zip.by_name(&manifest_name)
            .map_err(|e| CoreError::InvalidInput(format!("读取 manifest.json 失败：{e}")))?
            .read_to_string(&mut raw)
            .map_err(io_error)?;
        let manifest: CfManifest = serde_json::from_str(&raw)
            .map_err(|e| CoreError::InvalidInput(format!("manifest.json 解析失败：{e}")))?;
        let game_version = manifest.minecraft.version.clone();
        let mut loaders = Vec::new();
        for item in &manifest.minecraft.mod_loaders {
            if let Some((loader, version)) = split_loader_id(&item.id) {
                loaders.push((loader, version));
            }
        }
        let ids: Vec<u64> = manifest.files.iter().map(|file| file.file_id).collect();
        let details = crate::curseforge::files_by_ids(&ids)?;
        let by_id: HashMap<u64, crate::curseforge::PackFile> =
            details.into_iter().map(|file| (file.id, file)).collect();
        let mut files = Vec::new();
        let mut missing = Vec::new();
        for item in &manifest.files {
            let Some(detail) = by_id.get(&item.file_id) else {
                // 作者删了文件：PCL 会把名字列出来让用户决定，这里先继续装、把缺失报上去。
                missing.push(format!("项目 {} 的文件 {}", item.project_id, item.file_id));
                continue;
            };
            let folder = cf_folder(&detail.modules, &detail.file_name);
            let mut relative = PathBuf::from(folder);
            relative.push(&detail.file_name);
            let Some(relative) = safe_relative(&relative.to_string_lossy()) else {
                continue;
            };
            files.push(PlanFile {
                relative,
                urls: vec![detail.url.clone()],
                size: Some(detail.size).filter(|size| *size > 0).or(item.file_length),
                sha1: detail.sha1.clone(),
            });
        }
        // CurseForge 的 manifest 只有整合包名字与版本号，没有项目 id：先记下这两样，
        // 走搜索页装过来的那条线之后会用项目信息覆盖。
        let pack = PackOrigin::local(&manifest.name, &manifest.version);
        return Ok(Plan {
            name: manifest.name,
            version: manifest.version,
            game_version,
            loaders,
            files,
            missing,
            pack,
        });
    }

    Err(CoreError::InvalidInput(
        "只认得 Modrinth 与 CurseForge 整合包".into(),
    ))
}

/// CurseForge 只看得到 `modules`，按 PCL 的规则猜文件该放哪。
fn cf_folder(modules: &[String], file_name: &str) -> &'static str {
    let has = |name: &str| modules.iter().any(|module| module == name);
    if has("meta-inf")
        || has("mcmod.info")
        || has("fabric.mod.json")
        || file_name.to_ascii_lowercase().ends_with(".jar")
    {
        "mods"
    } else if has("pack.mcmeta") {
        "resourcepacks"
    } else if has("shaders") {
        "shaderpacks"
    } else if modules.is_empty() {
        "mods"
    } else {
        "saves"
    }
}

fn split_loader_id(id: &str) -> Option<(String, String)> {
    let (loader, version) = id.split_once('-')?;
    let loader = match loader {
        "forge" | "fabric" | "quilt" | "neoforge" => loader.to_string(),
        _ => return None,
    };
    Some((loader, version.to_string()))
}

fn default_name(name: &str, version: &str, file_name: &Option<String>) -> String {
    let base = if name.trim().is_empty() {
        file_name
            .as_deref()
            .map(|value| value.trim_end_matches(".mrpack").trim_end_matches(".zip"))
            .unwrap_or("整合包")
            .to_string()
    } else {
        name.trim().to_string()
    };
    if version.trim().is_empty() {
        base
    } else {
        format!("{base} {version}")
    }
}

/// 重名时自动加序号（PCL 是弹框让用户改名，这里先不打断安装）。
fn unique_name(game_dir: &Path, base: &str) -> String {
    let versions = game_dir.join("versions");
    if !versions.join(base).exists() {
        return base.to_owned();
    }
    for index in 2..1000 {
        let candidate = format!("{base} {index}");
        if !versions.join(&candidate).exists() {
            return candidate;
        }
    }
    base.to_owned()
}

/// `overrides/foo/bar.txt` → `foo/bar.txt`；`client-overrides/` 同样处理。
fn override_path(name: &str, root: &str) -> Option<PathBuf> {
    let rest = name.strip_prefix(root)?;
    let rest = rest
        .strip_prefix("overrides/")
        .or_else(|| rest.strip_prefix("client-overrides/"))?;
    if rest.is_empty() {
        return None;
    }
    safe_relative(rest)
}

/// 只接受普通相对路径，挡住 `..` 与绝对路径（zip slip）。
fn safe_relative(path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

#[derive(Deserialize)]
struct MrIndex {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version_id: Option<String>,
    #[serde(default)]
    dependencies: HashMap<String, String>,
    #[serde(default)]
    files: Vec<MrFile>,
}

#[derive(Deserialize)]
struct MrFile {
    path: String,
    #[serde(default)]
    downloads: Vec<String>,
    #[serde(default, rename = "fileSize")]
    file_size: Option<u64>,
    #[serde(default)]
    env: Option<MrEnv>,
    #[serde(default)]
    hashes: Option<MrHashes>,
}

#[derive(Deserialize)]
struct MrHashes {
    #[serde(default)]
    sha1: Option<String>,
}

#[derive(Deserialize)]
struct MrEnv {
    #[serde(default)]
    client: Option<String>,
}

#[derive(Deserialize)]
struct CfManifest {
    name: String,
    #[serde(default)]
    version: String,
    minecraft: CfMinecraft,
    #[serde(default)]
    files: Vec<CfFile>,
}

#[derive(Deserialize)]
struct CfMinecraft {
    version: String,
    #[serde(default, rename = "modLoaders")]
    mod_loaders: Vec<CfLoader>,
}

#[derive(Deserialize)]
struct CfLoader {
    id: String,
}

#[derive(Deserialize)]
struct CfFile {
    #[serde(rename = "projectID")]
    project_id: u64,
    #[serde(rename = "fileID")]
    file_id: u64,
    /// CurseForge manifest 里的字节数，用来放宽下载超时（旧包可能没写）。
    #[serde(default, rename = "fileLength")]
    file_length: Option<u64>,
}

fn io_error(error: std::io::Error) -> CoreError {
    CoreError::InvalidInput(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write, path::PathBuf};

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ybumcl-modpack-{}-{tag}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_zip(path: &std::path::Path, files: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn cf_folder_follows_the_pcl_rules() {
        assert_eq!(cf_folder(&[], "whatever.pack"), "mods");
        assert_eq!(cf_folder(&[], "oneblock.zip"), "mods");
        assert_eq!(cf_folder(&[], "cool.jar"), "mods");
        assert_eq!(cf_folder(&["fabric.mod.json".into()], "x.pack"), "mods");
        assert_eq!(cf_folder(&["pack.mcmeta".into()], "x.pack"), "resourcepacks");
        assert_eq!(cf_folder(&["shaders".into()], "vibrant.pack"), "shaderpacks");
        assert_eq!(cf_folder(&["world".into()], "map.zip"), "saves");
    }

    #[test]
    fn unique_name_appends_a_number_instead_of_failing() {
        let dir = temp_dir("unique");
        let versions = dir.join("versions");
        fs::create_dir_all(versions.join("All the Mods 10")).unwrap();
        assert_eq!(unique_name(&dir, "Other Pack"), "Other Pack");
        assert_eq!(unique_name(&dir, "All the Mods 10"), "All the Mods 10 2");
        fs::create_dir_all(versions.join("All the Mods 10 2")).unwrap();
        assert_eq!(unique_name(&dir, "All the Mods 10"), "All the Mods 10 3");
    }

    #[test]
    fn pack_root_finds_the_folder_that_holds_the_index() {
        let dir = temp_dir("root");
        let flat = dir.join("flat.zip");
        write_zip(&flat, &[("manifest.json", b"{}"), ("overrides/mods/a.jar", b"x")]);
        let mut archive = zip::ZipArchive::new(fs::File::open(&flat).unwrap()).unwrap();
        assert_eq!(pack_root(&mut archive).as_deref(), Some(""));

        let nested = dir.join("nested.zip");
        write_zip(
            &nested,
            &[
                ("pack/modrinth.index.json", b"{}"),
                ("pack/overrides/mods/a.jar", b"x"),
            ],
        );
        let mut archive = zip::ZipArchive::new(fs::File::open(&nested).unwrap()).unwrap();
        assert_eq!(pack_root(&mut archive).as_deref(), Some("pack/"));

        let unknown = dir.join("unknown.zip");
        write_zip(&unknown, &[("readme.txt", b"hi")]);
        let mut archive = zip::ZipArchive::new(fs::File::open(&unknown).unwrap()).unwrap();
        assert!(pack_root(&mut archive).is_none());
    }

    #[test]
    fn curseforge_pack_keeps_its_name_and_version_as_the_origin() {
        let dir = temp_dir("origin");
        let archive = dir.join("pack.zip");
        write_zip(
            &archive,
            &[(
                "manifest.json",
                br#"{"minecraft":{"version":"1.20.1","modLoaders":[]},"name":"All the Mods 10","version":"1.2.3","files":[]}"#,
            )],
        );
        let mut zip = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
        let plan = read_plan(&mut zip, "").unwrap();
        // manifest 里没有项目 id：只能记成「本地整合包」，但名字与版本号要有。
        assert_eq!(plan.pack.source, "zip");
        assert!(plan.pack.project_id.is_empty());
        assert_eq!(plan.pack.project_name, "All the Mods 10");
        assert_eq!(plan.pack.version_label, "1.2.3");
        assert_eq!(plan.name, "All the Mods 10");
    }

    /// 两张色块拼成的小 PNG（纯色图会被 `icons::inspect` 当空白图，所以这里要两种颜色）。
    const ICON_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 1, 8,
        2, 0, 0, 0, 123, 64, 232, 221, 0, 0, 0, 15, 73, 68, 65, 84, 120, 156, 99, 96, 96, 96,
        248, 255, 255, 63, 0, 6, 1, 2, 254, 2, 178, 57, 174, 0, 0, 0, 0, 73, 69, 78, 68, 174,
        66, 96, 130,
    ];

    #[test]
    fn pack_icon_is_saved_with_the_original_format() {
        let dir = temp_dir("icon");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/icon.png", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = std::io::Read::read(&mut socket, &mut request);
            let header = format!(
                "HTTP/1.1 200 OK
Content-Type: image/png
Content-Length: {}
Connection: close

",
                ICON_PNG.len()
            );
            let _ = socket.write_all(header.as_bytes());
            let _ = socket.write_all(ICON_PNG);
        });
        save_pack_icon(&url, &dir);
        server.join().unwrap();
        // 原格式落盘，`version_icon` 认这个文件名。
        assert!(dir.join("icon.png").is_file());
        // 已经有人放过图标就别再覆盖（第二次调用不该改动文件内容）。
        let before = fs::read(dir.join("icon.png")).unwrap();
        save_pack_icon(&url, &dir);
        assert_eq!(fs::read(dir.join("icon.png")).unwrap(), before);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn override_path_strips_the_base_and_override_folder() {
        assert_eq!(
            override_path("pack/overrides/config/a.toml", "pack/"),
            Some(PathBuf::from("config/a.toml"))
        );
        assert_eq!(
            override_path("client-overrides/options.txt", ""),
            Some(PathBuf::from("options.txt"))
        );
        assert_eq!(override_path("pack/manifest.json", "pack/"), None);
    }
}
