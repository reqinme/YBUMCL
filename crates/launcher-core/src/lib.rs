//! Install official vanilla Minecraft and plan vanilla/Fabric launches on Windows, without authentication.
//! `build_launch_plan` validates inputs and dependencies, then extracts native ZIPs into
//! `versions/<id>/launcher-natives`. It never starts Java. `detect_java` runs `java -version`.
//! Metadata and installed JARs must come from a trusted Minecraft installation.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
    process::Command,
    sync::Mutex,
};
use thiserror::Error;

/// 账户：离线档案与微软（设备代码）登录。
pub mod auth;
pub mod authlib;
pub mod cleanroom;
/// 崩溃报告诊断：认识几类高置信度的原因，认不出来就返回空。
pub mod crash;
pub mod curseforge;
pub mod dates;
pub mod download;
pub mod export;
pub mod icons;
mod inheritance;
/// 把多个版本 JSON 合并成 PCL2 风格的自包含实例。
pub mod instance;
pub mod java;
/// 六个加载器的版本号映射与清单来源。
pub mod loader_catalog;
pub mod merge;
/// Installer/profile support for modern Forge and NeoForge metadata.
pub mod modloader;
pub mod mod_updates;
pub mod modpack;
pub mod modrinth;
pub mod modicons;
pub mod mods;
pub mod names;
pub mod pack_update;
pub mod packages;
/// 读 `servers.dat`（服务器列表标签页用）。
pub mod servers;
/// 存档目录：图标、时间、游戏版本。
pub mod saves;
pub mod shaders;
/// 世界整包解压到 `saves`。
pub mod world;
pub mod ftb;
pub mod httpcache;
/// MC 百科条目表（slug → class id）。
pub mod wiki;
pub mod zhsearch;
pub use download::{
    install_version, list_remote_versions, CancellationToken, InstallProgress, InstallRequest,
    InstallSummary, RemoteVersion, RemoteVersionManifest,
};
pub use mods::{
    delete_mod, import_mods, inspect_mod_jar, resolve_runtime_directory, scan_mods,
    set_mod_enabled, ModEntry, ModImportOutcome, ModMetadata,
};

#[cfg(test)]
mod tests;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("Missing required file: {0}. Install or repair this version with an official launcher first.")]
    MissingFile(String),
    #[error("Unsupported version: {0}")]
    Unsupported(String),
    #[error("Invalid version metadata: {0}")]
    Metadata(String),
    #[error("File operation failed for {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("Invalid native archive: {0}")]
    Archive(String),
}

pub(crate) type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionSummary {
    pub id: String,
    pub kind: String,
    pub main_class: Option<String>,
    pub supported: bool,
    pub reason: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaInstallation {
    pub path: String,
    pub version: String,
    pub major: u32,
    /// 64 位还是 32 位：32 位 Java 只能给 2 GB 内存，装整合包容易直接 OOM。
    #[serde(default)]
    pub is_64: bool,
    /// 只有运行时没有 javac。PCL 安装 Forge 时会要求 JDK，这里用来提前说清楚。
    #[serde(default)]
    pub is_jre: bool,
    /// `java -version` 里 `(build 17.0.20+8-LTS)` 的括号内容，没有就是 None。
    #[serde(default)]
    pub revision: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRequest {
    /// Shared installation root containing versions, libraries, and assets.
    pub game_dir: String,
    /// Optional root-relative `instances/<safe-id>` runtime directory.
    /// Omitted requests retain the shared root as their runtime directory.
    #[serde(default)]
    pub game_directory: Option<String>,
    pub version_id: String,
    pub java_path: String,
    pub username: String,
    pub max_memory_mb: u32,
    /// 初始堆（MB），0 表示不传 `-Xms`。
    #[serde(default)]
    pub initial_memory_mb: u32,
    pub access_token: Option<String>,
    pub uuid: Option<String>,
    /// 实例设置里的「自定义信息」：覆盖版本 json 的 `type`，进 `${version_type}`。
    #[serde(default)]
    pub version_type: Option<String>,
    /// 实例设置里的「JVM 参数头部」：非空时整段替换版本自带的 JVM 参数。
    #[serde(default)]
    pub jvm_args: Vec<String>,
    /// 实例设置里的「游戏参数尾部」：追加在版本参数之后。
    #[serde(default)]
    pub game_args: Vec<String>,
    /// 实例设置里的「Classpath 头部附加」：插在 classpath 最前面。
    #[serde(default)]
    pub classpath_head: Vec<String>,
    /// 实例设置里的「关闭文件校验」：启动前不逐个检查文件在不在。
    #[serde(default)]
    pub skip_file_check: bool,
    /// 选中的 Java 比版本要求的旧时，允许自动改用探测到的合适 Java。
    /// 默认关闭，只有启动器界面打开；测试与其他调用方保持「报错」的老行为。
    #[serde(default)]
    pub allow_java_upgrade: bool,
    /// 实例设置里的「使用调试级别 log4j 配置文件」。
    #[serde(default)]
    pub log4j_debug: bool,
    /// 第三方认证服务器：非空时给 JVM 加 authlib-injector 的 agent 参数。
    #[serde(default)]
    pub authlib: Option<AuthlibInject>,
    /// 直接进存档：加 `--quickPlaySingleplayer <存档文件夹名>`（1.20+ 才有这个开关）。
    #[serde(default)]
    pub quick_play: Option<String>,
}

/// authlib-injector 的启动参数（由界面按当前账户的认证服务器算好传进来）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthlibInject {
    /// 数据目录里的 `authlib-injector.jar`。
    pub jar: String,
    /// 认证服务器接口地址。
    pub server: String,
    /// 服务器元数据（原文 JSON）；游戏端会预取，不再联网。
    #[serde(default)]
    pub metadata: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchPlan {
    pub java_path: String,
    pub working_directory: String,
    pub args: Vec<String>,
    pub version_id: String,
    pub warnings: Vec<String>,
}
impl LaunchPlan {
    pub fn redacted_args(&self) -> Vec<String> {
        let mut hide = false;
        self.args
            .iter()
            .map(|arg| {
                if hide {
                    hide = false;
                    return "[REDACTED]".into();
                }
                if arg == "--accessToken" {
                    hide = true;
                }
                if arg.starts_with("--accessToken=") {
                    "--accessToken=[REDACTED]".into()
                } else {
                    arg.clone()
                }
            })
            .collect()
    }
}

/// 实例要求「使用调试级别 log4j 配置文件」、而版本没带原件时写进 `log_configs` 的一份。
const DEBUG_LOG4J2: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Configuration status="WARN">
    <Appenders>
        <Console name="SysOut" target="SYSTEM_OUT">
            <PatternLayout pattern="[%d{HH:mm:ss}] [%t/%level] [%logger]: %msg%n"/>
        </Console>
    </Appenders>
    <Loggers>
        <Root level="debug">
            <AppenderRef ref="SysOut"/>
        </Root>
    </Loggers>
</Configuration>
"#;

fn io_error(path: &Path, source: io::Error) -> CoreError {
    CoreError::Io {
        path: path.display().to_string(),
        source,
    }
}
fn read_json(path: &Path) -> Result<Value> {
    required_file(path)?;
    let bytes = fs::read(path).map_err(|e| io_error(path, e))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| CoreError::Metadata(format!("{}: {e}", path.display())))
}
fn required_file(path: &Path) -> Result<()> {
    if !path.is_file() {
        // 路径多半是 canonicalize 过的，带 `\\?\` 前缀；给用户看的文案里抹掉。
        return Err(CoreError::MissingFile(crate::download::tidy_path(path)));
    }
    Ok(())
}
/// 版本 / 实例名能否直接当目录名用。
///
/// 允许中文和空格（PCL 的整合包实例就是这么命名的），只挡掉 Windows 不允许的
/// `< > : " / \ | ? *`、控制字符，以及结尾的点/空格和设备名。
/// 把任意名字改成能当文件夹名的一段：非法字符换成下划线、压缩空白、
/// 去掉结尾的点与空格、截到 120 字符、避开 CON/PRN 这类设备名。
///
/// 整合包名里常见 `™`、`:`、`|`（例如 `Prominence™ II: Hasturian Era`），
/// 直接拿来做目录名会失败，因此「从包名推实例名」的地方都先过这里。
pub(crate) fn sanitize_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_space = false;
    for c in value.chars() {
        let k = if (c as u32) < 0x20 || (r#"<>:"/\|?*"#).contains(c) {
            '_'
        } else {
            c
        };
        if k == '_' && out.ends_with('_') {
            continue;
        }
        if k.is_whitespace() {
            if last_space || out.is_empty() {
                continue;
            }
            out.push(' ');
            last_space = true;
            continue;
        }
        last_space = false;
        out.push(k);
    }
    let mut trimmed = out.trim().trim_end_matches(['.', ' ']).to_owned();
    if trimmed.chars().count() > 120 {
        trimmed = trimmed.chars().take(120).collect();
        trimmed = trimmed.trim_end_matches(['.', ' ']).to_owned();
    }
    if trimmed.is_empty() {
        return "整合包".into();
    }
    if is_device(&trimmed) {
        trimmed.push('_');
    }
    trimmed
}

pub(crate) fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && !value.ends_with(['.', ' '])
        && !value
            .chars()
            .any(|c| (c as u32) < 0x20 || (r#"<>:"/\|?*"#).contains(c))
        && value != "."
        && value != ".."
        && !is_device(value)
}
fn is_device(value: &str) -> bool {
    let stem = value.split('.').next().unwrap_or("").to_ascii_uppercase();
    ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
fn safe_relative(value: &str) -> Result<PathBuf> {
    let normalized = value.replace('\\', "/");
    if normalized.is_empty()
        || normalized.contains(':')
        || normalized.starts_with('/')
        || normalized.split('/').any(|p| {
            p.is_empty()
                || p == "."
                || p == ".."
                || p.ends_with(['.', ' '])
                || is_device(p)
                || p.chars().any(|c| c.is_control() || "<>\"|?*".contains(c))
        })
    {
        return Err(CoreError::Metadata(format!(
            "unsafe relative path: {value}"
        )));
    }
    let path = PathBuf::from(normalized);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(CoreError::Metadata(format!(
            "unsafe relative path: {value}"
        )));
    }
    Ok(path)
}
/// `fs::canonicalize` 在 Windows 上会返回带 `\\?\` 前缀的 verbatim 路径。
/// 用它拼出来的子路径再交给 `create_dir`，Windows 会以「拒绝访问」拒绝
/// （实测：盘符路径直接拼 verbatim 前缀建目录必定失败），所以统一去掉前缀。
fn plain(path: PathBuf) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest.to_string());
    }
    path
}

fn canonical_plain(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path)
        .map(plain)
        .map_err(|e| io_error(path, e))
}

fn inside(root: &Path, path: &Path) -> Result<()> {
    // root 可能来自 canonicalize（带 `\\?\` 前缀），real 是去前缀的普通路径，
    // 两边先归一化再比，否则永远不 starts_with。
    let root = plain(root.to_owned());
    let root = root.as_path();
    // Check existing ancestors too, including junctions/symlinks before creating a path.
    let mut ancestor = path;
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .ok_or_else(|| CoreError::InvalidInput("path has no existing ancestor".into()))?;
    }
    let real = canonical_plain(ancestor)?;
    if !real.starts_with(root) {
        return Err(CoreError::InvalidInput(format!(
            "path escapes game directory: {}",
            path.display()
        )));
    }
    Ok(())
}
/// Resolve the optional runtime directory without allowing arbitrary filesystem targets.
/// Existing junctions/symlinks must resolve to the exact managed location, not merely
/// somewhere else inside the shared installation.
fn runtime_directory(root: &Path, requested: Option<&str>) -> Result<PathBuf> {
    // 调用方给的 root 通常来自 canonicalize，即带 `\\?\` 前缀；带这个前缀去
    // 建子目录会被 Windows 判成「拒绝访问」，所以先还原成普通盘符路径。
    let root = plain(root.to_owned());
    let root = root.as_path();
    let Some(requested) = requested else {
        return Ok(root.to_owned());
    };
    let normalized = requested.replace('\\', "/");
    let parts: Vec<_> = normalized.split('/').collect();
    // 两种受管的隔离位置：`versions/<id>`（版本即实例，版本自带存档/模组）
    // 与历史上的 `instances/<id>`；都只允许一层安全 id，避免任意路径落盘。
    let managed_root = parts.len() == 2
        && matches!(parts[0], "versions" | "instances")
        && safe_component(parts[1]);
    if !managed_root {
        return Err(CoreError::InvalidInput(
            "Game directory must be a relative versions/<safe-id> path".into(),
        ));
    }
    let mut directory = root.to_owned();
    for part in parts {
        directory.push(part);
        inside(root, &directory)?;
        if !directory.exists() {
            // Create one component at a time so a redirected instances directory
            // cannot cause creation elsewhere before it has been validated.
            fs::create_dir(&directory).map_err(|e| io_error(&directory, e))?;
        }
        let canonical = canonical_plain(&directory)?;
        if canonical != directory || !canonical.is_dir() {
            return Err(CoreError::InvalidInput(format!(
                "Runtime directory must not be redirected: {}",
                directory.display()
            )));
        }
    }
    Ok(directory)
}

fn loader_main(main: &str) -> bool {
    matches!(
        main,
        "net.minecraftforge.bootstrap.ForgeBootstrap"
            // Forge 1.13–1.17 的入口（1.16.5-36.2.34 就是它）。
            | "cpw.mods.modlauncher.Launcher"
            | "net.neoforged.modlauncher.Launcher"
            | "com.cleanroommc.modlauncher.Launcher"
            | "cpw.mods.bootstraplauncher.BootstrapLauncher"
            | "net.neoforged.bootstraplauncher.BootstrapLauncher"
            | "com.cleanroommc.bootstraplauncher.BootstrapLauncher"
            | "net.minecraft.launchwrapper.Launch"
            | "cpw.mods.fml.relauncher.ServerLaunchWrapper"
    )
}

fn supported_main(main: &str) -> bool {
    matches!(
        main,
        "net.minecraft.client.main.Main"
            | "net.minecraft.client.Minecraft"
            | "net.fabricmc.loader.impl.launch.knot.KnotClient"
            | "net.fabricmc.loader.launch.knot.KnotClient"
            | "org.quiltmc.loader.impl.launch.knot.KnotClient"
    ) || loader_main(main)
}

fn is_loader_metadata(v: &Value) -> bool {
    v["mainClass"].as_str().is_some_and(loader_main)
}

fn unsupported(v: &Value, id: &str) -> Option<String> {
    let main = v["mainClass"].as_str().unwrap_or("");
    if !supported_main(main) && !foundation_main(main) {
        return Some(format!(
            "Main class '{main}' is not an allowed vanilla, Fabric, Quilt, Forge, NeoForge, Cleanroom, LiteLoader, or OptiFine entry point"
        ));
    }
    unsupported_configuration(v, id)
}

/// Cleanroom 的入口是它自带的 Foundation 引导器，类名不固定，靠顶层 token 认。
fn foundation_main(main: &str) -> bool {
    main.starts_with("top.outlands.foundation.")
}

fn unsupported_configuration(v: &Value, id: &str) -> Option<String> {
    // Cleanroom 这类自带完整 profile 的加载器会把 `jar` 指向自己的版本 id，
    // 落到 versions/<id>/<id>.jar，跟默认找法是同一个文件，不算覆盖入口。
    let custom_jar = v
        .get("jar")
        .and_then(Value::as_str)
        .is_some_and(|jar| jar != id || !safe_component(jar));
    if custom_jar
        || v.get("logging")
            .and_then(|l| l.get("client"))
            .and_then(|c| c.get("argument"))
            .and_then(Value::as_str)
            .is_some_and(|s| !s.starts_with("-Dlog4j.configurationFile="))
    {
        return Some("Custom JAR or logging launch configuration is not supported".into());
    }
    let loader = is_loader_metadata(v);
    // 老式 launchwrapper 版本（LiteLoader、OptiFine）本来就是靠 liteloader、
    // launchwrapper 这些库起作用的，不能一概拒绝；只有「不是加载器版本」的
    // JSON 夹带加载器库才可疑——那是伪造原版元数据的典型做法。
    for lib in v["libraries"].as_array().into_iter().flatten() {
        let name = lib["name"].as_str().unwrap_or("").to_ascii_lowercase();
        if !loader
            && [
                "forge",
                "neoforge",
                "liteloader",
                "launchwrapper",
                "optifine",
            ]
            .iter()
            .any(|s| name.contains(s))
        {
            return Some("Only vanilla, Fabric, Quilt, Forge, NeoForge, Cleanroom, LiteLoader, and OptiFine loader libraries are supported".into());
        }
    }
    None
}

pub fn scan_versions(game_dir: &Path) -> Result<Vec<VersionSummary>> {
    if !game_dir.is_dir() {
        return Err(CoreError::InvalidInput(
            "Game directory does not exist".into(),
        ));
    }
    let root = fs::canonicalize(game_dir).map_err(|e| io_error(game_dir, e))?;
    let versions = root.join("versions");
    if !versions.exists() {
        return Ok(vec![]);
    }
    inside(&root, &versions)?;
    let mut result = vec![];
    for entry in fs::read_dir(&versions).map_err(|e| io_error(&versions, e))? {
        let entry = entry.map_err(|e| io_error(&versions, e))?;
        if !entry.path().is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if !safe_component(&id) || merge::is_incomplete(&entry.path()) {
            continue;
        }
        // 没有 `<id>.json` 的目录不是版本：只可能是装到一半（暂停 / 被杀掉）留下的
        // 暂存目录，列出来就是一张「未知 / Missing required file」的坏卡。
        if !entry.path().join(format!("{id}.json")).is_file() {
            continue;
        }
        let parsed = inheritance::resolve_version(&root, &id).map(|resolved| resolved.metadata);
        let summary = match parsed {
            Ok(v) => {
                let reason = unsupported(&v, &id);
                VersionSummary {
                    id,
                    kind: v["type"].as_str().unwrap_or("unknown").into(),
                    main_class: v["mainClass"].as_str().map(str::to_owned),
                    supported: reason.is_none(),
                    reason,
                }
            }
            Err(e) => VersionSummary {
                id,
                kind: "unknown".into(),
                main_class: None,
                supported: false,
                reason: Some(e.to_string()),
            },
        };
        result.push(summary);
    }
    result.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(result)
}

/// `java -version` 里的 `(build 8.0.503)` / `(build 17.0.20+8-LTS)`。
fn build_revision(text: &str) -> Option<String> {
    let start = text.find("(build ")? + "(build ".len();
    let rest = &text[start..];
    let end = rest.find(')')?;
    let value = rest[..end].trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// 读 PE 头判断位数：`java.exe` 是 PE 文件，Machine 字段 0x8664/0xAA64 是 64 位。
/// 直接读文件比再跑一次 `java -XshowSettings` 便宜得多，也不用多等一个进程。
fn is_64_bit(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut file) = fs::File::open(path) else {
            return false;
        };
        let mut dos = [0u8; 0x40];
        if file.read_exact(&mut dos).is_err() {
            return false;
        }
        let offset = u32::from_le_bytes([dos[0x3c], dos[0x3d], dos[0x3e], dos[0x3f]]) as u64;
        if file.seek(SeekFrom::Start(offset)).is_err() {
            return false;
        }
        let mut pe = [0u8; 6];
        if file.read_exact(&mut pe).is_err() || &pe[..4] != b"PE\0\0" {
            return false;
        }
        let machine = u16::from_le_bytes([pe[4], pe[5]]);
        matches!(machine, 0x8664 | 0xAA64)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        true
    }
}

/// JDK 的 `bin` 里有 `javac`，JRE 没有。老版本 8 的 JRE 也是这么分的。
fn javac_beside(path: &Path) -> bool {
    let name = if cfg!(windows) { "javac.exe" } else { "javac" };
    path.parent().map(|dir| dir.join(name).is_file()).unwrap_or(false)
}

/// 单个 Java 探测的超时：JVM 正常启动不到 1 秒，卡住的多半是坏安装或网络盘。
const JAVA_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

fn java_version(path: &Path) -> Option<JavaInstallation> {
    use std::io::Read;
    use std::time::Instant;

    let mut command = Command::new(path);
    command.arg("-version");
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    // 直接把 stderr 管道拿回来，不落临时文件：
    // 有些环境下 `std::env::temp_dir()` 不可写，落盘探测会静默失败（表现为一个 Java 都测不到）。
    // 但它可能是坏安装、可能是网络盘上的 exe，会一直卡住——所以自己盯着超时，别让整页等它。
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + JAVA_PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            Err(_) => return None,
        }
    }
    let mut stderr_text = String::new();
    let mut stdout_text = String::new();
    if let Some(mut stream) = child.stderr.take() {
        let _ = stream.read_to_string(&mut stderr_text);
    }
    if let Some(mut stream) = child.stdout.take() {
        let _ = stream.read_to_string(&mut stdout_text);
    }
    let mut text = stderr_text;
    if !text.contains("version") {
        text.push_str(&stdout_text);
    }
    let version = text.split('"').nth(1)?.trim().to_owned();
    if version.is_empty() {
        return None;
    }
    let mut numbers = version.split(['.', '-', '_']);
    let first: u32 = numbers.next()?.parse().ok()?;
    let major = if first == 1 {
        numbers.next()?.parse().ok()?
    } else {
        first
    };
    Some(JavaInstallation {
        // 候选路径是拼字符串出来的（`bin/java.exe`），可能是混着 `/` 的形式；
        // 设置里存的、界面上显示的都得是同一个写法，否则按路径查大版本永远对不上。
        path: path
            .to_string_lossy()
            .replace('/', std::path::MAIN_SEPARATOR_STR)
            .into(),
        revision: build_revision(&text),
        is_64: is_64_bit(path),
        is_jre: !javac_beside(path),
        version,
        major,
    })
}

/// 目录联接（junction）和符号链接会让「逐层下钻」绕回自己；Windows 的 junction
/// 不是 symlink，得看 FILE_ATTRIBUTE_REPARSE_POINT。
fn is_reparse_point(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return fs::symlink_metadata(path)
            .map(|meta| meta.file_attributes() & 0x400 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(windows))]
    {
        fs::symlink_metadata(path)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false)
    }
}

/// 常见的 JRE/JDK 安装位置。用户机器上往往装了 Zulu / Temurin / Oracle 好几套，
/// 只认 PATH 会漏掉绝大多数。
fn java_candidates() -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    let java_exe = if cfg!(windows) {
        "bin/java.exe"
    } else {
        "bin/java"
    };
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        paths.insert(PathBuf::from(home).join(java_exe));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            paths.insert(dir.join(if cfg!(windows) { "java.exe" } else { "java" }));
        }
    }
    let mut roots = Vec::new();
    for key in [
        "ProgramFiles",
        "ProgramFiles(x86)",
        "LOCALAPPDATA",
        "APPDATA",
    ] {
        if let Some(base) = std::env::var_os(key) {
            roots.push(PathBuf::from(base));
        }
    }
    let vendors = [
        "Java",
        "Zulu",
        "Eclipse Adoptium",
        "AdoptOpenJDK",
        "Microsoft",
        "Amazon Corretto",
        "BellSoft",
        "Semeru",
        "Liberica",
        "JetBrains",
        "Programs/Eclipse Adoptium",
    ];
    for base in &roots {
        for vendor in vendors {
            let Ok(entries) = fs::read_dir(base.join(vendor)) else {
                continue;
            };
            for entry in entries.flatten() {
                let dir = entry.path();
                if is_reparse_point(&dir) {
                    continue;
                }
                paths.insert(dir.join(java_exe));
                paths.insert(dir.join("bin/java.exe"));
            }
        }
    }
    // 启动器自带的 JRE（官方启动器下载到 .minecraft/runtime 下的那一套）。
    let mut runtimes = Vec::new();
    if let Some(base) = std::env::var_os("APPDATA") {
        runtimes.push(PathBuf::from(base).join(".minecraft/runtime"));
    }
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        runtimes.push(PathBuf::from(base).join("YBUMCL/runtime"));
    }
    for runtime in runtimes {
        let Ok(entries) = fs::read_dir(&runtime) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if is_reparse_point(&dir) {
                continue;
            }
            paths.insert(dir.join(java_exe));
            if let Ok(platforms) = fs::read_dir(&dir) {
                for platform in platforms.flatten() {
                    paths.insert(platform.path().join(java_exe));
                    if let Ok(inners) = fs::read_dir(platform.path()) {
                        for jre in inners.flatten() {
                            paths.insert(jre.path().join(java_exe));
                        }
                    }
                }
            }
        }
    }
    paths.into_iter().filter(|p| p.is_file()).collect()
}

/// 探测结果缓存：一次探测要起十几个进程、每个最多 1.5 秒，而启动一个实例
/// 内部会连着问三四次。60 秒足够挡掉重复劳动，又不会让「刚装完 JDK」看起来没生效。
static JAVA_CACHE: Mutex<Option<(std::time::Instant, Vec<JavaInstallation>)>> = Mutex::new(None);
const JAVA_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(60);

pub fn detect_java() -> Vec<JavaInstallation> {
    detect_java_cached(false)
}

/// `force` 为真时跳过缓存（工具页的「重新检测」用）。
pub fn detect_java_cached(force: bool) -> Vec<JavaInstallation> {
    let mut guard = match JAVA_CACHE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !force {
        if let Some((stamp, found)) = guard.as_ref() {
            if stamp.elapsed() < JAVA_CACHE_TTL {
                return found.clone();
            }
        }
    }
    let found = scan_java();
    *guard = Some((std::time::Instant::now(), found.clone()));
    found
}

/// 并行探测候选，避免一串 `java -version` 把界面堵住。
fn scan_java() -> Vec<JavaInstallation> {
    let candidates = java_candidates();
    // 一起开：每个探测都有 1.5 秒上限，最坏也就等一轮，不会因为某个坏 Java 拖成串行。
    let probes: Vec<_> = candidates
        .iter()
        .cloned()
        .map(|path| std::thread::spawn(move || java_version(&path)))
        .collect();
    let mut found = Vec::new();
    for probe in probes {
        if let Ok(Some(java)) = probe.join() {
            found.push(java);
        }
    }
    let mut seen = BTreeSet::new();
    // `C:\Program Files\Common Files\Oracle\Java\javapath\java.exe` 是个转发器，
    // 本身不带版本，探测出来的 major 取决于它指向谁；PCL 也会剔除这类 shim。
    found.retain(|java| {
        let lower = java.path.to_ascii_lowercase();
        !lower.contains("\\javapath\\") && seen.insert(java.path.clone())
    });
    // PCL 的 JavaSorter 给 8/17/21 三种「主流 JDK」更高权重，这里照做：
    // 同权重时大版本优先，再按路径稳定排序。
    found.sort_by(|a, b| {
        java_weight(b.major)
            .cmp(&java_weight(a.major))
            .then(b.major.cmp(&a.major))
            .then(a.path.cmp(&b.path))
    });
    found
}

/// PCL `JavaSorter` 的权重：8/17/21 是各年代的「标准 JDK」，优先于 11、25 这类。
pub fn java_weight(major: u32) -> u32 {
    match major {
        17 => 31,
        8 => 30,
        21 => 29,
        _ => major.min(28),
    }
}

/// 现代版本的元数据把各架构的原生库拆成了独立坐标（`...:natives-windows-arm64`），
/// 而它们的 rules 只写了 `os.name == "windows"`，不管 CPU 架构。若不额外按名字过滤，
/// x64 机器上会把 arm64 的 dll 一起塞进 classpath，LWJGL 会报 "Platform/architecture mismatch"。
fn arch_suffix_allows(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let classifier = lower.rsplit(':').next().unwrap_or("");
    let token = classifier.rsplit('-').next().unwrap_or("");
    match token {
        "arm64" | "aarch64" => cfg!(target_arch = "aarch64"),
        "x86" => cfg!(target_arch = "x86"),
        "x64" | "amd64" | "x86_64" => cfg!(target_arch = "x86_64"),
        _ => true,
    }
}

fn rules_allow(value: Option<&Value>) -> Result<bool> {
    let Some(value) = value else {
        return Ok(true);
    };
    let rules = value
        .as_array()
        .ok_or_else(|| CoreError::Metadata("rules must be an array".into()))?;
    let mut allow = false;
    for rule in rules {
        let mut matches = true;
        if let Some(os) = rule.get("os") {
            if let Some(name) = os.get("name").and_then(Value::as_str) {
                matches &= name == "windows";
            }
            if let Some(arch) = os.get("arch").and_then(Value::as_str) {
                matches &= match arch {
                    "x86" => cfg!(target_arch = "x86"),
                    // LWJGL 3 的原生库规则写的是 "x64"（不是 x86_64/amd64）；
                    // 认不出来会把整批 natives 条目过滤掉，启动时找不到 lwjgl.dll。
                    "x64" | "x86_64" | "amd64" => cfg!(target_arch = "x86_64"),
                    "aarch64" | "arm64" => cfg!(target_arch = "aarch64"),
                    _ => false,
                };
            }
            if let Some(pattern) = os.get("version").and_then(Value::as_str) {
                let re = regex::Regex::new(pattern)
                    .map_err(|e| CoreError::Metadata(format!("invalid OS version rule: {e}")))?;
                // Windows' kernel version is not Rust's OS name. Query it only when required.
                let version = windows_version();
                matches &= re.is_match(&version);
            }
        }
        if let Some(features) = rule.get("features") {
            for (_, expected) in features
                .as_object()
                .ok_or_else(|| CoreError::Metadata("features must be an object".into()))?
            {
                matches &= expected.as_bool() == Some(false); // all optional launcher features disabled
            }
        }
        let action = rule["action"]
            .as_str()
            .ok_or_else(|| CoreError::Metadata("rule action is missing".into()))?;
        if !["allow", "disallow"].contains(&action) {
            return Err(CoreError::Metadata(format!(
                "unknown rule action: {action}"
            )));
        }
        if matches {
            allow = action == "allow";
        }
    }
    Ok(allow)
}
fn windows_version() -> String {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if let Ok(out) = Command::new("cmd.exe")
            .args(["/D", "/C", "ver"])
            .creation_flags(0x08000000)
            .output()
        {
            if let Some(v) = String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .last()
            {
                return v.trim_end_matches(']').into();
            }
        }
    }
    String::new()
}
fn argument_values(value: &Value) -> Result<Vec<String>> {
    let mut out = vec![];
    for item in value
        .as_array()
        .ok_or_else(|| CoreError::Metadata("arguments must be arrays".into()))?
    {
        if let Some(s) = item.as_str() {
            out.push(s.into());
        } else if item.is_object() {
            if !rules_allow(item.get("rules"))? {
                continue;
            }
            match &item["value"] {
                Value::String(s) => out.push(s.clone()),
                Value::Array(a) => {
                    for s in a {
                        out.push(
                            s.as_str()
                                .ok_or_else(|| {
                                    CoreError::Metadata("argument value must be a string".into())
                                })?
                                .into(),
                        );
                    }
                }
                _ => {
                    return Err(CoreError::Metadata(
                        "argument value must be a string or array".into(),
                    ))
                }
            }
        } else {
            return Err(CoreError::Metadata("invalid argument entry".into()));
        }
    }
    Ok(out)
}
/// Legacy launcher syntax: whitespace separates words, single/double quotes group, backslash
/// escapes quotes or backslashes but remains literal before other characters (Windows paths).
fn split_legacy(s: &str) -> Result<Vec<String>> {
    let mut args = vec![];
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && chars
                .peek()
                .is_some_and(|c| *c == '\\' || *c == '"' || *c == '\'')
        {
            word.push(chars.next().unwrap());
            started = true;
        } else if c == '"' || c == '\'' {
            if quote == Some(c) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(c);
            } else {
                word.push(c);
            }
            started = true;
        } else if c.is_whitespace() && quote.is_none() {
            if started {
                args.push(std::mem::take(&mut word));
                started = false;
            }
        } else {
            word.push(c);
            started = true;
        }
    }
    if quote.is_some() {
        return Err(CoreError::Metadata(
            "unterminated quote in minecraftArguments".into(),
        ));
    }
    if started {
        args.push(word);
    }
    Ok(args)
}
fn maven_path(name: &str) -> Result<String> {
    let parts: Vec<_> = name.split(':').collect();
    if !(3..=4).contains(&parts.len()) || parts.iter().any(|s| s.is_empty()) {
        return Err(CoreError::Metadata(format!(
            "invalid library coordinate: {name}"
        )));
    }
    let classifier = parts.get(3).map(|s| format!("-{s}")).unwrap_or_default();
    Ok(format!(
        "{}/{}/{}/{}-{}{}.jar",
        parts[0].replace('.', "/"),
        parts[1],
        parts[2],
        parts[1],
        parts[2],
        classifier
    ))
}
/// 官方仓库里给老 LWJGL2 的 `<artifact>-platform` 是一个几十字节的空 jar，
/// 只用来占位；缺失时跳过它，别把整个启动判死。
const PLACEHOLDER_ARTIFACT_BYTES: u64 = 4096;
fn placeholder_artifact(artifact: &Value) -> bool {
    artifact["size"]
        .as_u64()
        .is_some_and(|size| size <= PLACEHOLDER_ARTIFACT_BYTES)
}
fn artifact_path(root: &Path, artifact: &Value, fallback: Option<&str>) -> Result<PathBuf> {
    let relative = artifact["path"]
        .as_str()
        .or(fallback)
        .ok_or_else(|| CoreError::Metadata("library artifact has no path".into()))?;
    let path = root.join("libraries").join(safe_relative(relative)?);
    inside(root, &path)?;
    required_file(&path)?;
    Ok(path)
}

/// 同一 group:artifact:classifier 只留版本最高的 jar。HMCL、PCL 装出来的实例常把
/// 同名库的多个版本都写进 JSON（例如 1.12.2 自带的 log4j 2.8.1 和 Cleanroom 的 2.26），
/// classpath 先到先得会让旧版本生效，加载器一调新 API 就 NoSuchMethodError。
fn dedupe_classpath(classpath: &mut Vec<PathBuf>, names: &[String]) {
    fn key(name: &str) -> Option<(String, String, String)> {
        let parts: Vec<&str> = name.split(':').collect();
        if parts.len() < 3 {
            return None;
        }
        Some((
            parts[0].to_ascii_lowercase(),
            parts[1].to_ascii_lowercase(),
            parts.get(3).copied().unwrap_or("").to_ascii_lowercase(),
        ))
    }
    fn version(name: &str) -> Vec<u64> {
        name.split(':')
            .nth(2)
            .unwrap_or_default()
            .split(|c: char| !c.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .map(|part| part.parse().unwrap_or(u64::MAX))
            .collect()
    }
    let mut best: std::collections::HashMap<(String, String, String), (Vec<u64>, usize)> =
        std::collections::HashMap::new();
    for (index, name) in names.iter().enumerate() {
        let Some(key) = key(name) else { continue };
        let version = version(name);
        match best.get(&key) {
            Some((current, _)) if *current >= version => {}
            _ => {
                best.insert(key, (version, index));
            }
        }
    }
    let mut index = 0;
    classpath.retain(|_| {
        let keep = match names.get(index).and_then(|name| key(name)) {
            Some(key) => best.get(&key).is_some_and(|(_, best)| *best == index),
            None => true,
        };
        index += 1;
        keep
    });
}
fn offline_uuid(username: &str) -> String {
    let mut bytes = md5::compute(format!("OfflinePlayer:{username}")).0;
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn normalize_uuid(uuid: &str) -> Result<String> {
    let compact = uuid.replace('-', "");
    if compact.len() != 32 || !compact.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CoreError::InvalidInput(
            "UUID must contain 32 hexadecimal digits".into(),
        ));
    }
    Ok(compact.to_ascii_lowercase())
}
fn extract_natives(
    root: &Path,
    destination: &Path,
    natives: &[(PathBuf, Vec<String>)],
) -> Result<()> {
    if natives.is_empty() {
        return Ok(());
    }
    inside(root, destination)?;
    // 必须用去 verbatim 前缀的路径：`\?\` 前缀下连建目录都会被 Windows 判成
    // 「拒绝访问」，所以一律先规范化再 create_dir_all。
    let destination = plain(destination.to_owned());
    fs::create_dir_all(&destination).map_err(|e| io_error(&destination, e))?;
    // 解压时也用去 verbatim 的规范路径，否则创建/写入 dll 会被判拒绝访问。
    let native_root = canonical_plain(&destination)?;
    // 两边都去掉 verbatim 前缀再比，否则调用方传进来的 `\?\...` 会永远「不相等」。
    if native_root != destination {
        return Err(CoreError::Archive(
            "native output directory must not be a symbolic link or junction".into(),
        ));
    }
    let mut total = 0u64;
    for (path, excludes) in natives {
        let file = fs::File::open(path).map_err(|e| io_error(path, e))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| CoreError::Archive(e.to_string()))?;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|e| CoreError::Archive(e.to_string()))?;
            let name = entry.name().replace('\\', "/");
            if name.starts_with("META-INF/") || excludes.iter().any(|e| name.starts_with(e)) {
                continue;
            }
            let relative = safe_relative(name.trim_end_matches('/'))
                .map_err(|e| CoreError::Archive(e.to_string()))?;
            if entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
            {
                return Err(CoreError::Archive("symbolic links are forbidden".into()));
            }
            total = total
                .checked_add(entry.size())
                .ok_or_else(|| CoreError::Archive("size overflow".into()))?;
            if total > 512 * 1024 * 1024 {
                return Err(CoreError::Archive("expanded natives exceed 512 MiB".into()));
            }
            // 用去 verbatim 的 native_root 拼接：`\?\` 路径下 Windows 会拒绝创建/写入文件。
            let target = native_root.join(relative);
            inside(&native_root, &target)?;
            if target.exists() {
                // 目录（可能是 junction）解析后必须仍是原路径；文件只要求解析结果不逃出
                // 原生库目录——Windows 的 canonicalize 会规范化大小写/短名，逐字比对会误判。
                let resolved = canonical_plain(&target)?;
                inside(&native_root, &resolved)?;
                if entry.is_dir() && resolved != target {
                    return Err(CoreError::Archive(
                        "native entry would follow a symbolic link or junction".into(),
                    ));
                }
            }
            // Reusing a previously created directory must never follow an external link.
            if entry.is_dir() {
                fs::create_dir_all(&target).map_err(|e| io_error(&target, e))?;
            } else {
                // 已经解压过且大小一致就直接复用：像 Minecraft 正在运行时 dll 被占用，
                // 这时重写会 PermissionDenied，但文件本身是可用的，没必要失败。
                let already_ok = fs::metadata(&target)
                    .map(|meta| meta.len() == entry.size())
                    .unwrap_or(false);
                if already_ok {
                    continue;
                }
                let parent = target.parent().unwrap();
                inside(root, parent)?;
                fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
                match fs::File::create(&target) {
                    Ok(mut output) => {
                        io::copy(&mut entry, &mut output).map_err(|e| io_error(&target, e))?;
                    }
                    Err(error)
                        if error.kind() == io::ErrorKind::PermissionDenied && target.exists() =>
                    {
                        // 被占用/只读：只要磁盘上已经有这个文件就继续，让启动照常进行。
                        continue;
                    }
                    Err(error) => return Err(io_error(&target, error)),
                }
            }
        }
    }
    Ok(())
}

pub fn build_launch_plan(request: LaunchRequest) -> Result<LaunchPlan> {
    prepare(request, |path| {
        let probe = if path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("javaw.exe"))
        {
            path.with_file_name("java.exe")
        } else {
            path.to_owned()
        };
        java_version(&probe).map(|j| j.major).ok_or_else(|| {
            CoreError::InvalidInput(
                "Java could not report its version; select a working Java installation".into(),
            )
        })
    })
}

// Keep canonical paths for all filesystem validation, but do not send Win32
// verbatim prefixes to Java: its classpath/JAR URL handling rejects them.
fn java_launch_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(tail) = text.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{tail}");
        }
        if let Some(tail) = text.strip_prefix(r"\\?\") {
            return tail.to_owned();
        }
    }
    text.into_owned()
}

/// Windows 的单条命令行最多 32767 个字符，而加载器越多库就越多（LabyMod 一个实例
/// 就有 116 个 jar，拼起来超过 3 万）。超过阈值时把 classpath 写进一个清单 jar 的
/// `Class-Path`，命令行只留那一个 jar —— 这是 Java 8 也认的标准做法。
fn classpath_reference(root: &Path, id: &str, classpath: &[PathBuf]) -> Result<String> {
    // 命令行总长上限是 32767，java 路径和几十个参数还要占几千，这里留足余量。
    const LIMIT: usize = 16_000;
    let joined = classpath
        .iter()
        .map(|path| java_launch_path(path))
        .collect::<Vec<_>>()
        .join(";");
    if joined.len() <= LIMIT {
        return Ok(joined);
    }
    let entries = classpath
        .iter()
        .map(|path| {
            let text = java_launch_path(path).replace('\\', "/");
            let text = if text.starts_with('/') {
                text
            } else {
                format!("/{text}")
            };
            format!("file:{}", text.replace(' ', "%20"))
        })
        .collect::<Vec<_>>()
        .join(" ");
    // 清单规范：每行不超过 72 字节，续行以单个空格开头，续行内容直接接在前面之后，
    // 所以切分必须按字节切（路径是 ASCII，字符长度等于字节数）。
    let mut manifest = String::from("Manifest-Version: 1.0\r\nClass-Path: ");
    let mut remaining = entries.as_str();
    let mut capacity = 60;
    loop {
        let take = remaining.len().min(capacity);
        manifest.push_str(&remaining[..take]);
        remaining = &remaining[take..];
        if remaining.is_empty() {
            break;
        }
        manifest.push_str("\r\n ");
        capacity = 71;
    }
    manifest.push_str("\r\n\r\n");

    let folder = root.join("versions").join(id);
    inside(root, &folder)?;
    fs::create_dir_all(&folder).map_err(|e| io_error(&folder, e))?;
    let jar = folder.join(format!("{id}-classpath.jar"));
    let mut file = fs::File::create(&jar).map_err(|e| io_error(&jar, e))?;
    let mut writer = zip::ZipWriter::new(&mut file);
    let options = zip::write::SimpleFileOptions::default();
    let text = java_launch_path(&jar);
    writer
        .start_file("META-INF/MANIFEST.MF", options)
        .map_err(|e| CoreError::Io {
            path: text.clone(),
            source: std::io::Error::other(e.to_string()),
        })?;
    writer
        .write_all(manifest.as_bytes())
        .map_err(|e| io_error(&jar, e))?;
    writer.finish().map_err(|e| CoreError::Io {
        path: text,
        source: std::io::Error::other(e.to_string()),
    })?;
    Ok(java_launch_path(&jar))
}

fn prepare(
    request: LaunchRequest,
    validate_java: impl FnOnce(&Path) -> Result<u32>,
) -> Result<LaunchPlan> {
    if !safe_component(&request.version_id) {
        return Err(CoreError::InvalidInput(
            "Version ID contains unsafe path characters".into(),
        ));
    }
    if request.username.is_empty()
        || request.username.len() > 16
        || !request
            .username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(CoreError::InvalidInput(
            "Username must contain 1–16 ASCII letters, digits or underscores".into(),
        ));
    }
    if !(512..=65536).contains(&request.max_memory_mb) {
        return Err(CoreError::InvalidInput(
            "Memory must be between 512 and 65536 MiB".into(),
        ));
    }
    let root = PathBuf::from(&request.game_dir);
    if !root.is_dir() {
        return Err(CoreError::InvalidInput(
            "Game directory does not exist".into(),
        ));
    }
    let root = fs::canonicalize(&root).map_err(|e| io_error(&root, e))?;
    let runtime = runtime_directory(&root, request.game_directory.as_deref())?;
    let java = PathBuf::from(&request.java_path);
    required_file(&java)?;
    if !matches!(
        java.file_name()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("java.exe" | "javaw.exe" | "java" | "javaw")
    ) {
        return Err(CoreError::InvalidInput(
            "Select a Java executable (java.exe or javaw.exe)".into(),
        ));
    }
    let java = fs::canonicalize(&java).map_err(|e| io_error(&java, e))?;
    let version_dir = root.join("versions").join(&request.version_id);
    let resolved = inheritance::resolve_version(&root, &request.version_id)?;
    let v = resolved.metadata;
    let mut java = java;
    let mut java_major = validate_java(&java)?;
    let required_java = v["javaVersion"]["majorVersion"].as_u64();
    if let (Some(required), true) = (required_java, request.allow_java_upgrade) {
        if (java_major as u64) < required {
            // PCL 的做法是挑一个满足要求的 Java 直接用，而不是拦住不让玩。
            // 这里在探测到的列表里挑满足下限的最小大版本（Java 21 可用就不
            // 硬塞 Java 25），挑不到才报错。
            let mut candidates: Vec<JavaInstallation> = detect_java()
                .into_iter()
                .filter(|item| item.major as u64 >= required)
                .collect();
            candidates.sort_by_key(|item| item.major);
            let mut picked = None;
            for item in candidates {
                let Ok(path) = fs::canonicalize(&item.path) else {
                    continue;
                };
                let Some(runtime) = java_version(&path) else {
                    continue;
                };
                if (runtime.major as u64) >= required {
                    picked = Some((path, runtime.major));
                    break;
                }
            }
            let Some((path, major)) = picked else {
                return Err(CoreError::InvalidInput(format!(
                    "This version requires Java {required} or newer; selected Java is {java_major}"
                )));
            };
            java = path;
            java_major = major;
        }
    }
    if let Some(required) = required_java {
        if (java_major as u64) < required {
            return Err(CoreError::InvalidInput(format!(
                "This version requires Java {required} or newer; selected Java is {java_major}"
            )));
        }
    }
    if let Some(reason) = unsupported(&v, &request.version_id) {
        return Err(CoreError::Unsupported(reason));
    }
    if v["id"].as_str() != Some(&request.version_id) {
        return Err(CoreError::Metadata(
            "Version metadata ID does not match the selected directory".into(),
        ));
    }
    let jar = root
        .join("versions")
        .join(&resolved.client_id)
        .join(format!("{}.jar", resolved.client_id));
    inside(&root, &jar)?;
    // Cleanroom 之类自带完整 profile 的加载器不分发客户端 jar，主 jar 在库里。
    let client_jar = !v["downloads"]["client"]["url"]
        .as_str()
        .unwrap_or_default()
        .is_empty();
    if client_jar {
        required_file(&jar)?;
    }
    let mut classpath = vec![];
    let mut classpath_names: Vec<String> = vec![];
    let mut natives = vec![];
    // Cleanroom 换了 LWJGL3 自己接管渲染，但它的版本 JSON 里同时留着原版 1.12.2 的
    // LWJGL2；两套都有 `org.lwjgl.PointerBuffer`（2.x 继承 PointerWrapper，3.x 继承
    // CustomBuffer），classpath 里先前那套会让 3.x 的 libffi 直接 VerifyError。
    let lwjgl3 = v["libraries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|library| library["name"].as_str())
        .any(|name| name.starts_with("org.lwjgl:lwjgl:3."));
    // 同一份 profile 里还同时留着原版 1.12.2 的 `com.ibm.icu:icu4j-core-mojang:51.2`
    // 和 Cleanroom 的新 ICU（当前是 `com.ibm.icu:icu4j:78.3`）。classpath 先到先得，
    // 51 的类会被按 78 编译的代码调用，文本/语言初始化时 ICU 资源包查找无限递归，
    // 直接 StackOverflowError（`UResourceBundle.instantiateBundle`）。有新版就丢掉老的。
    let icu_modern = v["libraries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|library| library["name"].as_str())
        .any(|name| name.starts_with("com.ibm.icu:icu4j:"));
    for library in v["libraries"]
        .as_array()
        .ok_or_else(|| CoreError::Metadata("libraries array is missing".into()))?
    {
        if !rules_allow(library.get("rules"))? {
            continue;
        }
        if let Some(name) = library["name"].as_str() {
            if !arch_suffix_allows(name) {
                continue;
            }
        }
        let fallback = maven_path(
            library["name"]
                .as_str()
                .ok_or_else(|| CoreError::Metadata("library name is missing".into()))?,
        )?;
        let coordinate = library["name"].as_str().unwrap_or_default().to_owned();
        if lwjgl3 && coordinate.starts_with("org.lwjgl.lwjgl:") {
            continue;
        }
        if icu_modern && coordinate.starts_with("com.ibm.icu:icu4j-core-mojang:") {
            continue;
        }
        let downloads = library.get("downloads");
        if let Some(artifact) = downloads.and_then(|d| d.get("artifact")) {
            match artifact_path(&root, artifact, Some(&fallback)) {
                Ok(path) => {
                    classpath.push(path);
                    classpath_names.push(coordinate);
                }
                // 老 LWJGL2 的 lwjgl-platform 在官方仓库里是 22 字节的空占位 jar，
                // 有的启动器（PCL）干脆不下；里面没有任何类，跳过不影响启动。
                Err(CoreError::MissingFile(_)) if placeholder_artifact(artifact) => {}
                Err(error) => return Err(error),
            }
        } else {
            // 有的启动器（HMCL、PCL）只写 maven 坐标，`downloads` 要么整个缺省，
            // 要么是空对象；这种库按坐标在 libraries 下找，找不到再当纯原生库跳过。
            match artifact_path(&root, &Value::Null, Some(&fallback)) {
                Ok(path) => {
                    classpath.push(path);
                    classpath_names.push(coordinate);
                }
                Err(CoreError::MissingFile(_)) if library.get("natives").is_some() => {}
                Err(error) => return Err(error),
            }
        }
        if let Some(classifier) = library["natives"]["windows"].as_str() {
            let classifier = classifier.replace(
                "${arch}",
                if cfg!(target_pointer_width = "64") {
                    "64"
                } else {
                    "32"
                },
            );
            let native_fallback = maven_path(&format!(
                "{}:{classifier}",
                library["name"].as_str().unwrap_or_default()
            ))?;
            let native = if let Some(a) = library["downloads"]["classifiers"].get(&classifier) {
                // LWJGL 3 的 classifier 只写 url/sha1，没有 path，按坐标补全。
                artifact_path(&root, a, Some(&native_fallback))?
            } else if downloads.is_none() {
                artifact_path(&root, &Value::Null, Some(&native_fallback))?
            } else {
                return Err(CoreError::Metadata(format!(
                    "Missing native classifier {classifier}"
                )));
            };
            let excludes = library["extract"]["exclude"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.replace('\\', "/"))
                        .collect()
                })
                .unwrap_or_default();
            natives.push((native, excludes));
        }
    }
    let client_jar_name = jar
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    if client_jar {
        classpath.push(jar);
        classpath_names.push(String::new());
    }
    dedupe_classpath(&mut classpath, &classpath_names);
    let mut ignore_list = vec![];
    if let Some(names) = v["arguments"]["jvm"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find_map(|arg| arg.strip_prefix("-DignoreList="))
    {
        for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            ignore_list.push(if name == "${version_name}.jar" {
                client_jar_name.clone()
            } else {
                name.to_owned()
            });
        }
    }
    let mut cp = classpath_reference(&root, &request.version_id, &classpath)?;
    if !request.classpath_head.is_empty() {
        // 「Classpath 头部附加」：按分号拼在各库前面，和 PCL 一样插到最前。
        cp = format!("{};{cp}", request.classpath_head.join(";"));
    }
    // 实例设置里的「关闭文件校验」：跳过 assets 是否齐全的逐个检查，启动时由
    // 游戏自己报缺什么。
    let check_file = |path: &Path| -> Result<()> {
        if request.skip_file_check {
            Ok(())
        } else {
            required_file(path)
        }
    };
    let assets = root.join("assets");
    inside(&root, &assets)?;
    let asset_id = v["assetIndex"]["id"]
        .as_str()
        .or_else(|| v["assets"].as_str())
        .ok_or_else(|| CoreError::Metadata("asset index ID is missing".into()))?;
    if !safe_component(asset_id) {
        return Err(CoreError::Metadata("unsafe asset index ID".into()));
    }
    let index_path = assets.join("indexes").join(format!("{asset_id}.json"));
    inside(&root, &index_path)?;
    let index = read_json(&index_path)?;
    let objects = index["objects"]
        .as_object()
        .ok_or_else(|| CoreError::Metadata("asset index objects are missing".into()))?;
    for object in objects.values() {
        let hash = object["hash"]
            .as_str()
            .ok_or_else(|| CoreError::Metadata("asset hash missing".into()))?;
        if hash.len() != 40 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(CoreError::Metadata("invalid asset SHA1".into()));
        }
        let path = assets.join("objects").join(&hash[..2]).join(hash);
        inside(&root, &path)?;
        check_file(&path)?;
    }
    let game_assets = if index["virtual"].as_bool() == Some(true) {
        assets.join("virtual").join(asset_id)
    } else {
        assets.clone()
    };
    if index["virtual"].as_bool() == Some(true) {
        for name in objects.keys() {
            let path = game_assets.join(safe_relative(name)?);
            inside(&root, &path)?;
            check_file(&path)?;
        }
    }
    if index["map_to_resources"].as_bool() == Some(true) {
        for (name, object) in objects {
            let path = runtime.join("resources").join(safe_relative(name)?);
            inside(&runtime, &path)?;
            if runtime != root && !path.exists() {
                // Old clients load resources relative to their working directory.
                // Copy immutable asset objects rather than linking writable runtime
                // files back to the shared installation or requiring a reinstall.
                let parent = path.parent().unwrap();
                fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
                inside(&runtime, &path)?;
                let hash = object["hash"].as_str().unwrap();
                let source = assets.join("objects").join(&hash[..2]).join(hash);
                let mut source_file = fs::File::open(&source).map_err(|e| io_error(&source, e))?;
                let mut target = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|e| io_error(&path, e))?;
                io::copy(&mut source_file, &mut target).map_err(|e| io_error(&path, e))?;
            }
            check_file(&path)?;
        }
    }
    let offline = request.access_token.as_deref().is_none_or(str::is_empty);
    let token = request
        .access_token
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("0");
    if token.chars().any(char::is_control) {
        return Err(CoreError::InvalidInput(
            "Access token contains control characters".into(),
        ));
    }
    let uuid =
        if offline {
            offline_uuid(&request.username)
        } else {
            normalize_uuid(request.uuid.as_deref().ok_or_else(|| {
                CoreError::InvalidInput("Authenticated launch requires UUID".into())
            })?)?
        };
    let natives_dir = version_dir.join("launcher-natives");
    inside(&root, &natives_dir)?;
    let runtime_string = java_launch_path(&runtime);
    let replacements = vec![
        ("auth_player_name", request.username.clone()),
        ("version_name", request.version_id.clone()),
        ("game_directory", runtime_string.clone()),
        ("assets_root", java_launch_path(&assets)),
        ("assets_index_name", asset_id.into()),
        ("auth_uuid", uuid),
        ("auth_access_token", token.into()),
        (
            "auth_session",
            format!(
                "token:{token}:{}",
                if offline {
                    offline_uuid(&request.username)
                } else {
                    normalize_uuid(request.uuid.as_deref().unwrap())?
                }
            ),
        ),
        ("user_type", if offline { "legacy" } else { "msa" }.into()),
        (
            "version_type",
            request
                .version_type
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .or_else(|| v["type"].as_str().map(str::to_owned))
                .unwrap_or_else(|| "release".into()),
        ),
        ("natives_directory", java_launch_path(&natives_dir)),
        ("launcher_name", "YBUMCL".into()),
        ("launcher_version", env!("CARGO_PKG_VERSION").into()),
        ("classpath", cp.clone()),
        ("classpath_separator", ";".into()),
        (
            "library_directory",
            java_launch_path(&root.join("libraries")),
        ),
        ("user_properties", "{}".into()),
        ("game_assets", java_launch_path(&game_assets)),
        ("clientid", String::new()),
        ("auth_xuid", String::new()),
    ];
    let substitute = |arg: String| -> Result<String> {
        let mut out = arg;
        for (key, value) in &replacements {
            out = out.replace(&format!("${{{key}}}"), value);
        }
        if out.contains("${") {
            return Err(CoreError::Metadata(format!(
                "Unresolved launch placeholder in {out}"
            )));
        }
        if out.contains('\0') {
            return Err(CoreError::Metadata("NUL in launch argument".into()));
        }
        Ok(out)
    };
    let mut jvm = if let Some(j) = v["arguments"].get("jvm") {
        argument_values(j)?
    } else {
        vec![
            "-Djava.library.path=${natives_directory}".into(),
            "-cp".into(),
            "${classpath}".into(),
        ]
    };
    // Loader paths must be symbolic paths rooted in the managed library directory,
    // never arbitrary absolute paths, wildcards, or argument-file indirections.
    // 实例设置里的「JVM 参数头部」：整段替换版本自带的参数（PCL 的
    // VersionAdvanceJvm 就是这么做的）。用户自己填的东西和下载来的 metadata
    // 不是一个信任级别，所以先展开占位符、等下面那段 metadata 白名单校验跑完
    // 再整段换上去（换替身见后面的 `if !user_jvm.is_empty()`）。
    let user_jvm = request
        .jvm_args
        .iter()
        .map(|arg| substitute(arg.clone()))
        .collect::<Result<Vec<_>>>()?;
    if is_loader_metadata(&v) {
        let mut module_path = false;
        for arg in &jvm {
            let path_list = if module_path {
                module_path = false;
                Some(arg.as_str())
            } else {
                arg.strip_prefix("--module-path=")
            };
            if let Some(paths) = path_list {
                for entry in paths.split("${classpath_separator}") {
                    let relative =
                        entry.strip_prefix("${library_directory}/").ok_or_else(|| {
                            CoreError::Metadata("Module path must use ${library_directory}/".into())
                        })?;
                    let path = root.join("libraries").join(safe_relative(relative)?);
                    inside(&root, &path)?;
                    required_file(&path)?;
                    if !classpath.contains(&path) {
                        return Err(CoreError::Metadata(
                            "Module path must reference a declared library".into(),
                        ));
                    }
                }
            }
            if matches!(arg.as_str(), "-p" | "--module-path") {
                module_path = true;
            }
            if arg.starts_with("-DlegacyClassPath=") && arg != "-DlegacyClassPath=${classpath}" {
                return Err(CoreError::Metadata(
                    "legacyClassPath must use ${classpath}".into(),
                ));
            }
            if arg.starts_with("-DlibraryDirectory=")
                && arg != "-DlibraryDirectory=${library_directory}"
            {
                return Err(CoreError::Metadata(
                    "libraryDirectory must use ${library_directory}".into(),
                ));
            }
        }
        if module_path {
            return Err(CoreError::Metadata(
                "JVM module path option has no value".into(),
            ));
        }
    }
    jvm = jvm.into_iter().map(&substitute).collect::<Result<_>>()?;
    let mut loader_path_operand = false;
    let mut cp_operand = false;
    let mut exports_operand = false;
    let mut module_operand: Option<&'static str> = None;
    for arg in &mut jvm {
        if loader_path_operand {
            loader_path_operand = false;
            continue;
        }
        if is_loader_metadata(&v) && module_operand.is_none() && !cp_operand && !exports_operand {
            if arg == "--add-exports" {
                module_operand = Some("--add-exports");
                continue;
            }
            if arg == "--add-modules" {
                module_operand = Some("--add-modules");
                continue;
            }
            if let Some((key, value)) = arg.strip_prefix("-D").and_then(|s| s.split_once('=')) {
                if key == "ignoreList" && !ignore_list.is_empty() {
                    *arg = format!("-DignoreList={}", ignore_list.join(","));
                    continue;
                }
                if matches!(
                    key,
                    "ignoreList"
                        | "mergeModules"
                        | "fml.systemMods"
                        | "bootstraplauncher.ignoreList"
                ) && !value.is_empty()
                    && value.len() <= 4096
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-._,;".contains(&b))
                {
                    continue;
                }
            }
            if matches!(arg.as_str(), "-p" | "--module-path") {
                loader_path_operand = true;
                continue;
            }
            if arg.starts_with("--module-path=")
                || matches!(
                    arg.as_str(),
                    "--add-modules=ALL-MODULE-PATH" | "-Djava.net.preferIPv6Addresses=system"
                )
            {
                continue;
            }
            if arg.starts_with("-DlegacyClassPath=") || arg.starts_with("-DlibraryDirectory=") {
                continue;
            }
        }
        if let Some(expected) = module_operand.take() {
            let valid = match expected {
                "--add-opens" => matches!(
                    arg.as_str(),
                    "java.base/java.util.jar=cpw.mods.securejarhandler"
                        | "java.base/java.lang.invoke=cpw.mods.securejarhandler"
                        | "java.base/java.lang=ALL-UNNAMED"
                        | "java.base/java.util=ALL-UNNAMED"
                        | "java.base/sun.nio.ch=ALL-UNNAMED"
                        | "java.base/java.io=ALL-UNNAMED"
                ),
                "--add-exports" => matches!(
                    arg.as_str(),
                    "java.base/jdk.internal.misc=ALL-UNNAMED"
                        | "java.base/sun.security.util=cpw.mods.securejarhandler"
                        | "jdk.naming.dns/com.sun.jndi.dns=java.naming"
                ),
                "--add-modules" => arg == "ALL-MODULE-PATH",
                _ => false,
            };
            if !valid {
                return Err(CoreError::Unsupported(format!(
                    "Unsupported JVM module path: {arg}"
                )));
            }
            continue;
        }
        if arg == "--add-opens" && is_loader_metadata(&v) {
            module_operand = Some("--add-opens");
            continue;
        }
        if exports_operand {
            if arg != "java.base/jdk.internal.misc=ALL-UNNAMED" {
                return Err(CoreError::Unsupported(format!(
                    "Unsupported JVM export in version metadata: {arg}"
                )));
            }
            exports_operand = false;
            continue;
        }
        if arg == "--add-exports" {
            exports_operand = true;
            continue;
        }
        if arg == "--enable-native-access=ALL-UNNAMED" {
            continue;
        }
        if let Some(pages) = arg.strip_prefix("-XX:StackShadowPages=") {
            if pages.is_empty()
                || !pages.bytes().all(|b| b.is_ascii_digit())
                || !pages.parse::<u16>().is_ok_and(|n| (1..=512).contains(&n))
            {
                return Err(CoreError::Metadata(
                    "StackShadowPages must be a decimal integer between 1 and 512".into(),
                ));
            }
            continue;
        }
        if cp_operand {
            *arg = cp.clone();
            cp_operand = false;
            continue;
        }
        if ["-cp", "-classpath", "--class-path"].contains(&arg.as_str()) {
            cp_operand = true;
            continue;
        }
        if let Some(value) = arg.strip_prefix("-Djava.library.path=") {
            // canonicalize returns a verbatim Windows path, whose parser treats `/`
            // literally; metadata appends forward slashes to this placeholder.
            let normalized = if cfg!(windows) {
                value.replace('/', "\\")
            } else {
                value.to_owned()
            };
            let native_path = Path::new(&normalized);
            let java_natives_dir = PathBuf::from(java_launch_path(&natives_dir));
            if !native_path.starts_with(&java_natives_dir) {
                return Err(CoreError::Metadata(
                    "Java native library path must remain inside the version natives directory"
                        .into(),
                ));
            }
            let suffix = native_path.strip_prefix(&java_natives_dir).unwrap();
            if !suffix.as_os_str().is_empty() {
                safe_relative(&suffix.to_string_lossy())?;
            }
            inside(&root, native_path)?;
            continue;
        }
        if let Some(main) = arg.strip_prefix("-DFabricMcEmu=") {
            if !v["mainClass"].as_str().is_some_and(|main| {
                matches!(
                    main,
                    "net.fabricmc.loader.impl.launch.knot.KnotClient"
                        | "net.fabricmc.loader.launch.knot.KnotClient"
                )
            }) || !matches!(
                main.trim(),
                "net.minecraft.client.main.Main" | "net.minecraft.client.Minecraft"
            ) {
                return Err(CoreError::Unsupported(format!(
                    "Unsupported Fabric emulated main class: {main}"
                )));
            }
            continue;
        }
        let property = arg
            .strip_prefix("-D")
            .and_then(|p| p.split_once('='))
            .map(|(k, _)| k);
        let allowed_property = property.is_some_and(|key| {
            [
                "java.library.path",
                "org.lwjgl.librarypath",
                "org.lwjgl.system.SharedLibraryExtractPath",
                "io.netty.native.workdir",
                "jna.tmpdir",
                "minecraft.launcher.brand",
                "minecraft.launcher.version",
                "java.net.preferIPv4Stack",
                "java.net.preferIPv6Addresses",
                "file.encoding",
                "stdout.encoding",
                "stderr.encoding",
                "sun.stdout.encoding",
                "sun.stderr.encoding",
                "os.name",
                "os.version",
                "log4j2.formatMsgNoLookups",
                // Forge / NeoForge 自己的模块层开关，值只是路径列表。
                "ignoreList",
                "mergeModules",
                "libraryDirectory",
            ]
            .contains(&key)
        });
        // 加载器还可以塞自己命名空间下的属性（fml.*、forge.*、neoforged.*），
        // 这些是它们内部约定的键，不会改变 JVM 的类加载方式。
        let loader_property = property.is_some_and(|key| {
            [
                "fml.",
                "forge.",
                "neoforged.",
                "fabric.",
                "quilt.",
                "net.labymod.",
                "labymod.",
            ]
            .iter()
            .any(|prefix| key.starts_with(prefix))
        });
        // 模块系统的开放/导出开关：加载器（LabyMod、Forge 系）都要靠它反射 JDK 内部类，
        // 只允许 `模块/包=ALL-UNNAMED` 这种形式。
        // 目标模块不一定叫 ALL-UNNAMED：Forge 1.16.5 就写
        // `--add-exports=jdk.naming.dns/com.sun.jndi.dns=java.naming`。
        // 这类开关只放宽模块可见性、不加载代码，值里只要没有怪字符就照发。
        let module_flag = ["--add-opens=", "--add-exports=", "--add-reads=", "--add-modules="]
            .iter()
            .find_map(|prefix| arg.strip_prefix(prefix))
            .is_some_and(|rest| {
                !rest.is_empty()
                    && rest
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.$/*+=-".contains(&b))
            });
        let allowed_xx = [
            "-XX:+UseG1GC",
            "-XX:+UnlockExperimentalVMOptions",
            "-XX:+DisableExplicitGC",
            "-XX:+AlwaysPreTouch",
            "-XX:+UseStringDeduplication",
        ]
        .contains(&arg.as_str())
            || [
                "-XX:G1NewSizePercent=",
                "-XX:G1ReservePercent=",
                "-XX:MaxGCPauseMillis=",
                "-XX:G1HeapRegionSize=",
                "-XX:HeapDumpPath=",
            ]
            .iter()
            .any(|p| arg.starts_with(p));
        // -XX 只是虚拟机开关，不会加载代码；和 -agentlib/-javaagent 这类分开看。
        let xx_flag =
            arg.starts_with("-XX:+") || arg.starts_with("-XX:-") || arg.starts_with("-XX:");
        let memory = ["-Xmx", "-Xms", "-Xss"].iter().any(|p| {
            arg.strip_prefix(p).is_some_and(|v| {
                !v.is_empty()
                    && v.bytes()
                        .all(|b| b.is_ascii_digit() || b"kKmMgG".contains(&b))
            })
        });
        if !allowed_property
            && !loader_property
            && !module_flag
            && !allowed_xx
            && !xx_flag
            && !memory
        {
            return Err(CoreError::Unsupported(format!(
                "Unsupported JVM option in version metadata: {arg}"
            )));
        }
    }
    if exports_operand {
        return Err(CoreError::Metadata(
            "JVM --add-exports option has no value".into(),
        ));
    }
    if module_operand.is_some() {
        return Err(CoreError::Metadata(
            "JVM --add-opens option has no value".into(),
        ));
    }
    if cp_operand {
        return Err(CoreError::Metadata(
            "JVM classpath option has no value".into(),
        ));
    }
    if !user_jvm.is_empty() {
        // 用户填的参数照发，只剔掉 classpath 和内存这两类：它们由启动器自己接管，
        // 用户填进来会把 `-cp`、`-Xmx` 拼乱。
        let mut replaced = Vec::with_capacity(user_jvm.len());
        let mut skip_operand = false;
        for arg in user_jvm {
            if skip_operand {
                skip_operand = false;
                continue;
            }
            if ["-cp", "-classpath", "--class-path"].contains(&arg.as_str()) {
                skip_operand = true;
                continue;
            }
            if ["-cp=", "-classpath=", "--class-path="]
                .iter()
                .any(|prefix| arg.starts_with(prefix))
            {
                continue;
            }
            if arg.starts_with("-Xmx") || arg.starts_with("-Xms") {
                continue;
            }
            replaced.push(arg);
        }
        jvm = replaced;
    }
    jvm.retain(|a| !a.starts_with("-Xmx") && !a.starts_with("-Xms"));
    if !jvm
        .iter()
        .any(|a| a == "-cp" || a == "-classpath" || a == "--class-path")
    {
        jvm.extend(["-cp".into(), cp]);
    }
    if !jvm.iter().any(|a| a.starts_with("-Djava.library.path=")) {
        jvm.push(format!(
            "-Djava.library.path={}",
            java_launch_path(&natives_dir)
        ));
    }
    // PCL 的 log4shell 防御参数：不管版本 json 有没有写，都补上。
    if !jvm
        .iter()
        .any(|a| a.starts_with("-Dlog4j2.formatMsgNoLookups="))
    {
        jvm.push("-Dlog4j2.formatMsgNoLookups=true".into());
    }
    // 有些版本的 json 会带 `-XX:MaxDirectMemorySize=256M`，现代加载器会因此崩，
    // PCL 也是直接剔掉（#3511）。
    jvm.retain(|arg| arg != "-XX:MaxDirectMemorySize=256M");
    // 编码参数按 Java 版本给（PCL 同款）：Java 8 不加，9+ 才加 stdout/stderr，
    // 18+ 用 COMPAT，免得老 mod 的中文反而乱码。
    let mut encodings: Vec<&str> = vec![];
    if java_major > 8 {
        encodings.extend(["-Dstdout.encoding=UTF-8", "-Dstderr.encoding=UTF-8"]);
    }
    if java_major >= 18 {
        encodings.push("-Dfile.encoding=COMPAT");
    }
    for flag in encodings {
        if !jvm
            .iter()
            .any(|a| a.starts_with(flag.split('=').next().unwrap_or(flag)))
        {
            jvm.push(flag.into());
        }
    }
    if let Some(logging) = v["logging"]["client"].as_object() {
        let id = logging
            .get("file")
            .and_then(|f| f.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::Metadata("logging file ID missing".into()))?;
        if !safe_component(id) {
            return Err(CoreError::Metadata("unsafe logging file ID".into()));
        }
        let path = assets.join("log_configs").join(id);
        inside(&root, &path)?;
        // 别的启动器（PCL）装出来的实例常见没下 log_configs/*.xml：缺它只影响
        // 日志格式，log4j 会退回默认配置，不值得因此拒绝启动。
        if path.is_file() {
            let arg = logging
                .get("argument")
                .and_then(Value::as_str)
                .ok_or_else(|| CoreError::Metadata("logging argument missing".into()))?;
            jvm.push(substitute(
                arg.replace("${path}", &java_launch_path(&path)),
            )?);
        }
    }
    let game = if let Some(game) = v["arguments"].get("game") {
        argument_values(game)?
    } else {
        split_legacy(
            v["minecraftArguments"]
                .as_str()
                .ok_or_else(|| CoreError::Metadata("game arguments missing".into()))?,
        )?
    };
    let mut game = game
        .into_iter()
        .map(substitute)
        .collect::<Result<Vec<_>>>()?;
    if !request.game_args.is_empty() {
        // 实例设置里的「游戏参数尾部」：接在版本参数后面。
        game.extend(
            request
                .game_args
                .iter()
                .map(|arg| substitute(arg.clone()))
                .collect::<Result<Vec<_>>>()?,
        );
    }
    if request.log4j_debug {
        // 「使用调试级别 log4j 配置文件」：照 Mojang 的客户端配置改一份 debug 级别
        // 的放进 log_configs，缺原件时用内置的一份。
        let path = root
            .join("log_configs")
            .join(format!("debug-{}.xml", request.version_id));
        inside(&root, &path)?;
        let base = v["logging"]["client"]
            .get("file")
            .and_then(|file| file.get("id"))
            .and_then(Value::as_str)
            .map(|id| assets.join("log_configs").join(id))
            .filter(|path| path.is_file())
            .and_then(|path| fs::read_to_string(path).ok());
        let text = match base {
            Some(text) => text.replace("level=\"info\"", "level=\"debug\""),
            None => DEBUG_LOG4J2.to_owned(),
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        }
        fs::write(&path, text).map_err(|e| io_error(&path, e))?;
        jvm.push(format!(
            "-Dlog4j.configurationFile={}",
            java_launch_path(&path)
        ));
    }
    let mut args = vec![format!("-Xmx{}M", request.max_memory_mb)];
    if request.initial_memory_mb > 0 {
        args.push(format!("-Xms{}M", request.initial_memory_mb));
    }
    // 第三方登录：authlib-injector 的 agent 要排在所有 JVM 参数之后、main class
    // 之前（它靠 agentmain 改 classloader，位置错了游戏端还是走正版验证）。
    if let Some(authlib) = request.authlib.as_ref() {
        let metadata = authlib
            .metadata
            .as_deref()
            .ok_or_else(|| CoreError::InvalidInput("认证服务器缺少元数据，重新添加一次。".into()))?;
        use base64::Engine;
        let prefetched = base64::engine::general_purpose::STANDARD.encode(metadata.as_bytes());
        jvm.push(format!("-javaagent:{}={}", authlib.jar, authlib.server));
        jvm.push("-Dauthlibinjector.side=client".into());
        jvm.push(format!("-Dauthlibinjector.yggdrasil.prefetched={prefetched}"));
    }
    if let Some(save) = request.quick_play.as_deref() {
        if !save.is_empty() {
            game.push("--quickPlaySingleplayer".into());
            game.push(save.to_owned());
        }
    }
    args.extend(jvm);
    args.push(v["mainClass"].as_str().unwrap().into());
    args.extend(game);
    extract_natives(&root, &natives_dir, &natives)?;
    let warnings = if offline {
        vec![
            "Offline identity: online authentication and multiplayer access are not provided."
                .into(),
        ]
    } else {
        vec!["Access token is supplied by the caller; launcher-core does not authenticate or refresh it.".into()]
    };
    Ok(LaunchPlan {
        java_path: java_launch_path(&java),
        working_directory: runtime_string,
        args,
        version_id: request.version_id,
        warnings,
    })
}

/// 把 `extra`（用户选的 jar/zip）里的条目合并进 `target`（实例的客户端 jar）。
///
/// 照 PCL CE 的 `GameCore.AddToCore`：同名条目以 `extra` 为准，跳过目录项和
/// `META-INF` 里的签名文件（否则改过的 jar 会带着失效签名，Java 直接拒载）。
/// 返回写进 `target` 的条目数。
pub fn merge_core_jar(target: &Path, extra: &Path) -> Result<usize> {
    use std::io::{Read, Write};

    let mut source = zip::ZipArchive::new(
        std::fs::File::open(extra).map_err(|e| io_error(extra, e))?,
    )
    .map_err(|e| CoreError::Archive(format!("{}: {e}", extra.display())))?;
    let mut names = Vec::with_capacity(source.len());
    for index in 0..source.len() {
        let entry = source
            .by_index(index)
            .map_err(|e| CoreError::Archive(e.to_string()))?;
        let name = entry.name().to_owned();
        if entry.is_dir() || name.starts_with("META-INF/") {
            continue;
        }
        names.push(name);
    }
    if names.is_empty() {
        return Err(CoreError::InvalidInput(
            "这个压缩包里没有可用的文件（是不是选错了？）".into(),
        ));
    }

    let handle = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(target)
        .map_err(|e| io_error(target, e))?;
    let mut writer = zip::ZipWriter::new_append(handle)
        .map_err(|e| CoreError::Archive(format!("{}: {e}", target.display())))?;
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut written = 0usize;
    for name in &names {
        let mut entry = source
            .by_name(name)
            .map_err(|e| CoreError::Archive(e.to_string()))?;
        let mut buffer = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut buffer)
            .map_err(|e| CoreError::Archive(e.to_string()))?;
        writer
            .start_file(name.clone(), options)
            .map_err(|e| CoreError::Archive(e.to_string()))?;
        writer
            .write_all(&buffer)
            .map_err(|e| CoreError::Archive(e.to_string()))?;
        written += 1;
    }
    writer
        .finish()
        .map_err(|e| CoreError::Archive(e.to_string()))?;
    Ok(written)
}

