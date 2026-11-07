//! Mod management for a runtime directory's `mods` folder.
//!
//! Mods are toggled by appending or stripping the `.disabled` suffix on the file
//! name; no file contents are ever deleted or rewritten by these helpers.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use crate::{io_error, runtime_directory, CoreError};

/// Descriptor entries are never read beyond this many bytes.
const DESCRIPTOR_LIMIT: usize = 256 * 1024;
/// Descriptions longer than this many characters are truncated.
const DESCRIPTION_LIMIT: usize = 500;
const FABRIC_DESCRIPTOR: &str = "fabric.mod.json";
const QUILT_DESCRIPTOR: &str = "quilt.mod.json";
const NEOFORGE_DESCRIPTOR: &str = "META-INF/neoforge.mods.toml";
const FORGE_DESCRIPTOR: &str = "META-INF/mods.toml";
const LEGACY_DESCRIPTOR: &str = "mcmod.info";

/// One mod file discovered in the `mods` directory of a runtime directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModEntry {
    /// Actual current file name, e.g. `opti.jar` or `opti.jar.disabled`.
    pub file_name: String,
    /// Display name with the trailing `.jar` or `.jar.disabled` stripped.
    pub mod_name: String,
    pub enabled: bool,
    pub size_bytes: u64,
    /// File mtime as Unix epoch millis; entries with stat errors use 0.
    pub modified_millis: i64,
    /// Short human-readable stat/read error; `None` when healthy.
    pub error: Option<String>,
    /// Metadata read from the jar's descriptor; `None` when unknown or unreadable.
    pub metadata: Option<ModMetadata>,
    /// Short human-readable metadata read error; `None` when metadata was read.
    pub metadata_error: Option<String>,
}

/// What a mod jar says about itself. Every field is optional because a jar may be
/// a plain library or a mod for a loader we cannot parse; such jars are still valid.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModMetadata {
    pub mod_id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    /// Empty when unknown.
    pub authors: Vec<String>,
    /// May be truncated to 500 characters.
    pub description: Option<String>,
    /// `"fabric"`, `"quilt"`, `"forge"`, `"neoforge"`, `"legacy"`, or `None`.
    pub loader: Option<String>,
    /// The descriptor declares an icon, or the jar ships an icon file.
    pub icon: bool,
    /// Descriptor entry name, e.g. `fabric.mod.json`.
    pub descriptor: Option<String>,
}

impl ModMetadata {
    /// Metadata for a jar that declares nothing about itself.
    fn empty() -> Self {
        Self {
            mod_id: None,
            name: None,
            version: None,
            authors: Vec::new(),
            description: None,
            loader: None,
            icon: false,
            descriptor: None,
        }
    }
}

/// Result of importing one source file into the `mods` directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModImportOutcome {
    pub source_path: String,
    /// `"imported"`, `"duplicate"`, or `"rejected"`.
    pub status: String,
    /// Final name inside `mods`, when known.
    pub file_name: Option<String>,
    /// User-facing reason for a `duplicate` or `rejected` result.
    pub message: Option<String>,
    pub metadata: Option<ModMetadata>,
}

/// Resolve the runtime directory for a launch request. This is a thin public
/// wrapper around the crate's private `runtime_directory` helper so that mod
/// management cannot invent its own resolution rules.
pub fn resolve_runtime_directory(
    game_dir: &str,
    game_directory: Option<&str>,
) -> Result<PathBuf, CoreError> {
    let requested = Path::new(game_dir);
    if !requested.is_dir() {
        return Err(CoreError::InvalidInput(format!(
            "Game directory does not exist: {game_dir}"
        )));
    }
    let root = fs::canonicalize(requested).map_err(|e| io_error(requested, e))?;
    runtime_directory(&root, game_directory)
}

/// Scan the `mods` directory of `runtime_dir` and return one entry per jar mod.
pub fn scan_mods(runtime_dir: &Path) -> Result<Vec<ModEntry>, CoreError> {
    if !runtime_dir.is_dir() {
        return Err(CoreError::InvalidInput(
            "Runtime directory does not exist".into(),
        ));
    }
    let mods_dir = runtime_dir.join("mods");
    if !mods_dir.exists() {
        // A runtime directory without a `mods` folder simply has no mods yet.
        return Ok(Vec::new());
    }
    if !mods_dir.is_dir() {
        return Err(CoreError::InvalidInput(format!(
            "Mods directory is not a directory: {}",
            mods_dir.display()
        )));
    }

    let mut entries = Vec::new();
    let read_dir = fs::read_dir(&mods_dir).map_err(|e| io_error(&mods_dir, e))?;
    for item in read_dir {
        let entry = item.map_err(|e| io_error(&mods_dir, e))?;

        // `entry.metadata()` does not follow symlinks, so reparse points and
        // other non-regular entries are classified by themselves and skipped.
        let Ok(link_metadata) = entry.metadata() else {
            continue;
        };
        if link_metadata.is_symlink() || !link_metadata.is_file() {
            continue;
        }

        let file_name = entry.file_name().to_string_lossy().into_owned();
        let lowercase = file_name.to_lowercase();
        if !(lowercase.ends_with(".jar") || lowercase.ends_with(".jar.disabled")) {
            continue;
        }

        let entry_path = entry.path();
        let (size_bytes, modified_millis, error) = match fs::metadata(&entry_path) {
            Ok(metadata) => (metadata.len(), modified_epoch_millis(&metadata), None),
            Err(error) => (0, 0, Some(format!("无法读取文件信息：{error}"))),
        };

        // Metadata inspection must never break the scan: failures are reported in
        // `metadata_error` and leave `metadata` empty. A disabled mod is inspected
        // in place: the ZIP reader does not care about the `.disabled` suffix, and
        // the suffix-stripped name does not exist on disk.
        let (metadata, metadata_error) = match inspect_mod_jar(&entry_path) {
            Ok(metadata) => (Some(metadata), None),
            Err(error) => (None, Some(error.to_string())),
        };

        entries.push(ModEntry {
            file_name: file_name.clone(),
            mod_name: mod_display_name(&file_name).to_owned(),
            enabled: lowercase.ends_with(".jar"),
            size_bytes,
            modified_millis,
            error,
            metadata,
            metadata_error,
        });
    }

    entries.sort_by_key(|entry| entry.file_name.to_lowercase());
    Ok(entries)
}

/// Toggle a mod's enabled state by renaming it.
pub fn set_mod_enabled(
    runtime_dir: &Path,
    file_name: &str,
    enabled: bool,
) -> Result<ModEntry, CoreError> {
    validate_mod_file_name(file_name)?;

    let is_jar = ascii_case_suffix_len(file_name, b".jar").is_some();
    let is_disabled_mod = ascii_case_suffix_len(file_name, b".jar.disabled").is_some();
    if !is_jar && !is_disabled_mod {
        return Err(CoreError::InvalidInput(
            "Only .jar mods can be toggled".into(),
        ));
    }
    if enabled != is_disabled_mod {
        return Err(CoreError::InvalidInput(format!(
            "Mod is already in the requested state: {file_name}"
        )));
    }

    let (mods_dir, canonical_mods) = canonical_mods_directory(runtime_dir)?;
    let target = verified_regular_file(&mods_dir, &canonical_mods, file_name)?;

    let new_name = if enabled {
        // `file_name` ends with ".jar.disabled", so stripping ".disabled" (9 bytes)
        // leaves a name ending in ".jar". The cut index is a char boundary because the
        // suffix is ASCII.
        file_name[..file_name.len() - b".disabled".len()].to_owned()
    } else {
        format!("{file_name}.disabled")
    };
    let new_target = mods_dir.join(&new_name);
    if fs::symlink_metadata(&new_target).is_ok() {
        return Err(CoreError::InvalidInput(format!(
            "Renaming {file_name} to {new_name} would overwrite an existing file"
        )));
    }

    fs::rename(&target, &new_target).map_err(|error| io_error(&target, error))?;

    match fs::metadata(&new_target) {
        Ok(metadata) => {
            let (mod_metadata, metadata_error) = match inspect_mod_jar(&new_target) {
                Ok(mod_metadata) => (Some(mod_metadata), None),
                Err(error) => (None, Some(error.to_string())),
            };
            Ok(ModEntry {
                file_name: new_name.clone(),
                mod_name: mod_display_name(&new_name).to_owned(),
                enabled,
                size_bytes: metadata.len(),
                modified_millis: modified_epoch_millis(&metadata),
                error: None,
                metadata: mod_metadata,
                metadata_error,
            })
        }
        Err(error) => Err(CoreError::InvalidInput(format!(
            "Renamed {file_name} to {new_name}, but reading the result failed: {error}"
        ))),
    }
}

/// Delete a mod file. Only `.jar` and `.jar.disabled` files are accepted.
///
/// The file goes to the recycle bin unless `permanent` is set.
pub fn delete_mod(runtime_dir: &Path, file_name: &str, permanent: bool) -> Result<(), CoreError> {
    validate_mod_file_name(file_name)?;
    if ascii_case_suffix_len(file_name, b".jar").is_none()
        && ascii_case_suffix_len(file_name, b".jar.disabled").is_none()
    {
        return Err(CoreError::InvalidInput(format!(
            "Only .jar or .jar.disabled mods can be deleted: {file_name}"
        )));
    }

    let (mods_dir, canonical_mods) = canonical_mods_directory(runtime_dir)?;
    let target = verified_regular_file(&mods_dir, &canonical_mods, file_name)?;
    if permanent {
        fs::remove_file(&target).map_err(|error| io_error(&target, error))
    } else {
        recycle_file(&target)
    }
}

/// 把任意路径（文件或文件夹）丢进回收站，给资源包/光影包/存档/截图删除用。
pub fn recycle_path(path: &Path) -> Result<(), CoreError> {
    recycle_file(path)
}

/// Put a file in the recycle bin. Non-Windows targets fall back to deleting it.
fn recycle_file(path: &Path) -> Result<(), CoreError> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::{
            SHFileOperationW, SHFILEOPSTRUCTW, FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION,
            FOF_NOERRORUI, FOF_SILENT,
        };

        let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
        from.push(0);
        from.push(0);
        // 结构体要求可空字段；用 0 初始化就等于全部为空。
        let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
        op.wFunc = FO_DELETE as u32;
        op.pFrom = from.as_ptr();
        op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16;
        let code = unsafe { SHFileOperationW(&mut op) };
        if code != 0 || op.fAnyOperationsAborted != 0 {
            return Err(io_error(
                path,
                io::Error::other(format!("recycle failed (code {code})")),
            ));
        }
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        fs::remove_file(path).map_err(|error| io_error(path, error))
    }
}

/// Read a mod's icon straight out of its jar, as a `data:` URL.
///
/// Saves callers from shipping their own zip reader; `None` means the jar has no
/// usable image.
pub fn mod_icon(runtime_dir: &Path, file_name: &str) -> Result<Option<String>, CoreError> {
    validate_mod_file_name(file_name)?;
    let (mods_dir, canonical_mods) = canonical_mods_directory(runtime_dir)?;
    let target = verified_regular_file(&mods_dir, &canonical_mods, file_name)?;
    let file = fs::File::open(&target).map_err(|error| io_error(&target, error))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| CoreError::Archive(format!("{}: {error}", target.display())))?;

    let mut wanted: Vec<String> = Vec::new();
    for name in [
        FABRIC_DESCRIPTOR,
        QUILT_DESCRIPTOR,
        NEOFORGE_DESCRIPTOR,
        FORGE_DESCRIPTOR,
        LEGACY_DESCRIPTOR,
    ] {
        if let Some(text) = read_text_entry(&mut archive, name) {
            wanted.extend(icon_paths(&text));
        }
    }
    if let Some(found) = wanted.iter().find_map(|path| read_image_entry(&mut archive, path)) {
        return Ok(Some(found));
    }
    // 描述符里没写就按常见文件名找一层。
    for path in ["icon.png", "logo.png", "pack.png", "assets/icon.png"] {
        if let Some(found) = read_image_entry(&mut archive, path) {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

/// Pull icon paths out of a descriptor without a full parse.
fn icon_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["\"icon\"", "logoFile", "logo_file"] {
        let mut rest = text;
        while let Some(index) = rest.find(key) {
            rest = &rest[index + key.len()..];
            let trimmed = rest.trim_start();
            if !(trimmed.starts_with(':') || trimmed.starts_with('=') || trimmed.starts_with(' '))
            {
                continue;
            }
            let trimmed = trimmed.trim_start_matches([':', '=', ' ', '\t']).trim_start();
            let Some(quote) = trimmed.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                continue;
            };
            let body = &trimmed[quote.len_utf8()..];
            if let Some(end) = body.find(quote) {
                let value = body[..end].trim();
                if !value.is_empty() {
                    out.push(value.to_owned());
                }
            }
        }
    }
    out
}

fn read_text_entry<R: Read + io::Seek>(archive: &mut zip::ZipArchive<R>, name: &str) -> Option<String> {
    let mut entry = archive.by_name(name).ok()?;
    if entry.size() as usize > DESCRIPTOR_LIMIT {
        return None;
    }
    let mut buffer = Vec::with_capacity(entry.size() as usize);
    entry.take(DESCRIPTOR_LIMIT as u64).read_to_end(&mut buffer).ok()?;
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

/// Read an entry as a base64 `data:` URL; images have to be small.
fn read_image_entry<R: Read + io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<String> {
    let name = name.trim_start_matches('/').replace('\\', "/");
    let mut entry = archive.by_name(&name).ok()?;
    let mime = match name.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => return None,
    };
    if entry.size() == 0 || entry.size() > 512 * 1024 {
        return None;
    }
    let mut buffer = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut buffer).ok()?;
    use base64::Engine as _;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(buffer)
    ))
}

/// Inspect a mod jar (or `.litemod`) and report what its descriptor says about it.
///
/// A jar without a known descriptor is not an error: it may be a plain library, so
/// an all-empty [`ModMetadata`] is returned. Only unreadable files and broken ZIP
/// containers produce an error.
pub fn inspect_mod_jar(path: &Path) -> Result<ModMetadata, CoreError> {
    let link_metadata = match fs::symlink_metadata(path) {
        Ok(link_metadata) => link_metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CoreError::MissingFile(path.display().to_string()));
        }
        Err(error) => return Err(io_error(path, error)),
    };
    if !link_metadata.is_file() {
        return Err(CoreError::MissingFile(path.display().to_string()));
    }

    let file = fs::File::open(path).map_err(|error| io_error(path, error))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| CoreError::Archive(format!("{}: {error}", path.display())))?;
    let archive_has_icon = jar_has_icon(&mut archive);

    if let Some(bytes) = read_descriptor(&mut archive, FABRIC_DESCRIPTOR) {
        let bytes = bytes.map_err(|error| io_error(path, error))?;
        return parse_fabric_descriptor(&bytes, archive_has_icon)
            .map_err(|error| CoreError::Archive(format!("{}: {error}", path.display())));
    }
    if let Some(bytes) = read_descriptor(&mut archive, QUILT_DESCRIPTOR) {
        let bytes = bytes.map_err(|error| io_error(path, error))?;
        return parse_quilt_descriptor(&bytes, archive_has_icon)
            .map_err(|error| CoreError::Archive(format!("{}: {error}", path.display())));
    }
    if let Some(bytes) = read_descriptor(&mut archive, NEOFORGE_DESCRIPTOR) {
        let bytes = bytes.map_err(|error| io_error(path, error))?;
        return Ok(parse_toml_descriptor(
            &bytes,
            "neoforge",
            NEOFORGE_DESCRIPTOR,
            archive_has_icon,
        ));
    }
    if let Some(bytes) = read_descriptor(&mut archive, FORGE_DESCRIPTOR) {
        let bytes = bytes.map_err(|error| io_error(path, error))?;
        return Ok(parse_toml_descriptor(
            &bytes,
            "forge",
            FORGE_DESCRIPTOR,
            archive_has_icon,
        ));
    }
    if let Some(bytes) = read_descriptor(&mut archive, LEGACY_DESCRIPTOR) {
        let bytes = bytes.map_err(|error| io_error(path, error))?;
        return parse_legacy_descriptor(&bytes, archive_has_icon)
            .map_err(|error| CoreError::Archive(format!("{}: {error}", path.display())));
    }

    Ok(ModMetadata::empty())
}

/// Import mod jars from `sources` into the runtime directory's `mods` folder.
///
/// Every source is reported individually; a rejected or duplicated source never
/// aborts the batch. Imported mods are always enabled (plain `.jar` names).
pub fn import_mods(
    runtime_dir: &Path,
    sources: &[String],
    overwrite: bool,
) -> Result<Vec<ModImportOutcome>, CoreError> {
    if !runtime_dir.is_dir() {
        return Err(CoreError::InvalidInput(
            "Runtime directory does not exist".into(),
        ));
    }
    if sources.is_empty() {
        return Ok(Vec::new());
    }

    let mods_dir = runtime_dir.join("mods");
    if !mods_dir.exists() {
        fs::create_dir_all(&mods_dir).map_err(|error| io_error(&mods_dir, error))?;
    }
    // Reuse the same redirect/regularity guarantees the other mod helpers rely on.
    let (mods_dir, _) = canonical_mods_directory(runtime_dir)?;

    let mut imported: BTreeSet<String> = BTreeSet::new();
    let mut outcomes = Vec::with_capacity(sources.len());
    for source in sources {
        outcomes.push(import_one_mod(&mods_dir, source, overwrite, &mut imported));
    }
    Ok(outcomes)
}

fn import_one_mod(
    mods_dir: &Path,
    source: &str,
    overwrite: bool,
    imported: &mut BTreeSet<String>,
) -> ModImportOutcome {
    let reject = |message: String| ModImportOutcome {
        source_path: source.to_owned(),
        status: "rejected".into(),
        file_name: None,
        message: Some(message),
        metadata: None,
    };
    let duplicate = |file_name: String, message: String| ModImportOutcome {
        source_path: source.to_owned(),
        status: "duplicate".into(),
        file_name: Some(file_name),
        message: Some(message),
        metadata: None,
    };

    let path = Path::new(source);
    let link_metadata = match fs::symlink_metadata(path) {
        Ok(link_metadata) => link_metadata,
        Err(error) => {
            return reject(format!("无法读取源文件 {source}：{error}"));
        }
    };
    if link_metadata.is_symlink() {
        return reject("源文件是符号链接，已拒绝导入".to_owned());
    }
    if !link_metadata.is_file() {
        return reject("源路径不是普通文件，已拒绝导入".to_owned());
    }
    let size_bytes = link_metadata.len();
    if size_bytes == 0 {
        return reject("源文件为空，已拒绝导入".to_owned());
    }

    let Some(os_name) = path.file_name() else {
        return reject("源路径没有文件名，已拒绝导入".to_owned());
    };
    let file_name = os_name.to_string_lossy().into_owned();
    let lowercase = file_name.to_lowercase();
    if lowercase.ends_with(".jar.disabled") {
        return reject("已禁用的模组文件不能导入，请先启用它".to_owned());
    }
    if !(lowercase.ends_with(".jar") || lowercase.ends_with(".litemod")) {
        return reject(format!("只支持 .jar 或 .litemod 文件：{file_name}"));
    }
    if !is_safe_import_name(&file_name) {
        return reject(format!("文件名包含不安全字符或过长：{file_name}"));
    }

    // Reject anything that is not a readable ZIP container before touching `mods/`.
    if let Err(error) = open_zip_archive(path) {
        return reject(format!("不是有效的 JAR 文件：{file_name}（{error}）"));
    }

    let target = mods_dir.join(&file_name);
    let disabled_target = mods_dir.join(format!("{file_name}.disabled"));
    let key = lowercase;
    let already_present = target.exists() || disabled_target.exists();
    if (already_present || imported.contains(&key)) && !overwrite {
        return duplicate(
            file_name.clone(),
            format!("{file_name} 已存在于 mods 目录，已跳过"),
        );
    }
    if already_present && overwrite {
        if let Err(error) =
            remove_existing_mod(&target).and_then(|_| remove_existing_mod(&disabled_target))
        {
            return reject(format!("无法替换已有的 {file_name}：{error}"));
        }
    }

    let copied_bytes = match fs::copy(path, &target) {
        Ok(copied_bytes) => copied_bytes,
        Err(error) => {
            remove_quietly(&target);
            return reject(format!("复制 {file_name} 失败：{error}"));
        }
    };
    if copied_bytes != size_bytes {
        remove_quietly(&target);
        return reject(format!(
            "复制 {file_name} 后大小不一致（{copied_bytes} 字节，源文件 {size_bytes} 字节）"
        ));
    }

    imported.insert(key);
    ModImportOutcome {
        source_path: source.to_owned(),
        status: "imported".into(),
        file_name: Some(file_name),
        message: None,
        metadata: inspect_mod_jar(&target).ok(),
    }
}

/// Remove an existing `mods/<name>` entry, tolerating a missing file.
fn remove_existing_mod(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() || metadata.is_symlink() => fs::remove_file(path),
        Ok(_) => Err(io::Error::other("目标是目录，无法替换")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_quietly(path: &Path) {
    let _ = fs::remove_file(path);
}

fn open_zip_archive(path: &Path) -> Result<zip::ZipArchive<fs::File>, CoreError> {
    let file = fs::File::open(path).map_err(|error| io_error(path, error))?;
    zip::ZipArchive::new(file).map_err(|error| CoreError::Archive(error.to_string()))
}

/// A file name we are willing to write into `mods/`.
fn is_safe_import_name(file_name: &str) -> bool {
    const FORBIDDEN: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
    !file_name.is_empty()
        && file_name.len() <= 150
        && file_name != "."
        && file_name != ".."
        && !file_name
            .chars()
            .any(|character| FORBIDDEN.contains(&character) || character.is_control())
}

/// Read one descriptor entry, bounded to [`DESCRIPTOR_LIMIT`] bytes.
///
/// `None` means the entry is absent; `Some(Err(..))` means it exists but could not
/// be decompressed.
fn read_descriptor(
    archive: &mut zip::ZipArchive<fs::File>,
    name: &str,
) -> Option<io::Result<Vec<u8>>> {
    let entry = archive.by_name(name).ok()?;
    let mut buffer = Vec::new();
    let mut limited = entry.take(DESCRIPTOR_LIMIT as u64);
    match limited.read_to_end(&mut buffer) {
        Ok(_) => Some(Ok(buffer)),
        Err(error) => Some(Err(error)),
    }
}

/// Whether the jar ships an icon-looking file anywhere.
fn jar_has_icon(archive: &mut zip::ZipArchive<fs::File>) -> bool {
    archive.file_names().any(|name| {
        let base = name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase();
        base == "icon.png" || base == "logo.png"
    })
}

fn parse_fabric_descriptor(bytes: &[u8], archive_has_icon: bool) -> Result<ModMetadata, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| format!("{FABRIC_DESCRIPTOR}: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| format!("{FABRIC_DESCRIPTOR}: root must be an object"))?;

    let declared_icon = declares_icon(object.get("icon"));
    let metadata = ModMetadata {
        mod_id: json_scalar(object.get("id")),
        name: json_scalar(object.get("name")),
        version: json_scalar(object.get("version")),
        authors: json_names(object.get("authors")),
        description: json_scalar(object.get("description")),
        loader: Some("fabric".into()),
        icon: declared_icon || archive_has_icon,
        descriptor: Some(FABRIC_DESCRIPTOR.into()),
    };
    Ok(finalize_metadata(metadata))
}

fn parse_quilt_descriptor(bytes: &[u8], archive_has_icon: bool) -> Result<ModMetadata, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| format!("{QUILT_DESCRIPTOR}: {error}"))?;
    let root = value
        .as_object()
        .ok_or_else(|| format!("{QUILT_DESCRIPTOR}: root must be an object"))?;
    let loader = root
        .get("quilt_loader")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{QUILT_DESCRIPTOR}: missing quilt_loader object"))?;
    let quilt_metadata = loader.get("metadata").and_then(Value::as_object);
    let declared_icon = quilt_metadata.is_some_and(|m| declares_icon(m.get("icon")));
    let contributors = quilt_metadata
        .and_then(|m| m.get("contributors"))
        .or_else(|| loader.get("contributors"));

    let metadata = ModMetadata {
        mod_id: json_scalar(loader.get("id")),
        name: json_scalar(quilt_metadata.and_then(|m| m.get("name"))),
        version: json_scalar(loader.get("version")),
        authors: json_names(contributors),
        description: json_scalar(quilt_metadata.and_then(|m| m.get("description"))),
        loader: Some("quilt".into()),
        icon: declared_icon || archive_has_icon,
        descriptor: Some(QUILT_DESCRIPTOR.into()),
    };
    Ok(finalize_metadata(metadata))
}

fn parse_legacy_descriptor(bytes: &[u8], archive_has_icon: bool) -> Result<ModMetadata, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| format!("{LEGACY_DESCRIPTOR}: {error}"))?;
    let first = match &value {
        Value::Array(items) => items.first(),
        Value::Object(root) => root
            .get("modList")
            .and_then(Value::as_array)
            .and_then(|items| items.first()),
        _ => None,
    };

    let declared_icon = matches!(
        first.and_then(|item| item.get("logoFile")),
        Some(Value::String(_))
    );
    let metadata = ModMetadata {
        mod_id: first.and_then(|item| json_scalar(item.get("modid"))),
        name: first.and_then(|item| json_scalar(item.get("name"))),
        version: first.and_then(|item| json_scalar(item.get("version"))),
        authors: first
            .map(|item| json_names(item.get("authorList")))
            .unwrap_or_default(),
        description: first.and_then(|item| json_scalar(item.get("description"))),
        loader: Some("legacy".into()),
        icon: declared_icon || archive_has_icon,
        descriptor: Some(LEGACY_DESCRIPTOR.into()),
    };
    Ok(finalize_metadata(metadata))
}

/// Tolerant, line-based scan of a Forge/NeoForge `mods.toml`.
///
/// Only the first `[[mods]]` table is read; unparsable lines are ignored.
fn parse_toml_descriptor(
    bytes: &[u8],
    loader: &str,
    descriptor: &str,
    archive_has_icon: bool,
) -> ModMetadata {
    let text = String::from_utf8_lossy(bytes);
    let mut metadata = ModMetadata {
        loader: Some(loader.into()),
        descriptor: Some(descriptor.into()),
        icon: archive_has_icon,
        ..ModMetadata::empty()
    };
    let mut in_first_table = false;
    for raw_line in text.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            if in_first_table {
                // A second table ends the only table we care about.
                break;
            }
            in_first_table = line.starts_with("[[mods]]");
            continue;
        }
        if !in_first_table {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let Some(value) = toml_value(value) else {
            continue;
        };
        match key.trim() {
            "modId" => metadata.mod_id = Some(value),
            "displayName" => metadata.name = Some(value),
            "version" => metadata.version = Some(value),
            "description" => metadata.description = Some(value),
            "authors" => {
                metadata.authors = value
                    .split(',')
                    .map(str::trim)
                    .filter(|author| !author.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            "logoFile" => metadata.icon = true,
            _ => {}
        }
    }
    finalize_metadata(metadata)
}

/// Cut a TOML line at an unquoted `#`.
fn strip_toml_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (index, character) in line.char_indices() {
        match character {
            '"' | '\'' => {
                if quote == Some(character) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(character);
                }
            }
            '#' if quote.is_none() => return &line[..index],
            _ => {}
        }
    }
    line
}

/// Read a TOML scalar, tolerating quotes, trailing commas and stray whitespace.
fn toml_value(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let trimmed = trimmed.strip_suffix(',').unwrap_or(trimmed).trim();
    let unquoted = if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        // Both delimiters are ASCII, so these indices are char boundaries.
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    let value = unquoted.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Whether a descriptor's `icon` key declares an icon (string path or object map).
fn declares_icon(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::String(_)) | Some(Value::Object(_)))
}

/// Collect author names from a JSON string, array of strings/objects, or a map
/// whose keys are the names (Quilt contributors).
fn json_names(value: Option<&Value>) -> Vec<String> {
    let names: Vec<String> = match value {
        Some(Value::String(text)) => vec![text.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text.clone()),
                Value::Object(entry) => {
                    entry.get("name").and_then(Value::as_str).map(str::to_owned)
                }
                _ => None,
            })
            .collect(),
        Some(Value::Object(entries)) => entries.keys().cloned().collect(),
        _ => Vec::new(),
    };
    names
        .into_iter()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .collect()
}

fn json_scalar(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_owned())
        }
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// Truncate the description and keep everything else as parsed.
fn finalize_metadata(mut metadata: ModMetadata) -> ModMetadata {
    metadata.description = metadata
        .description
        .map(|description| truncate_chars(&description, DESCRIPTION_LIMIT));
    metadata
}

/// Truncate to `limit` characters without splitting a UTF-8 sequence.
fn truncate_chars(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((cut, _)) => text[..cut].to_owned(),
        None => text.to_owned(),
    }
}

fn canonical_mods_directory(runtime_dir: &Path) -> Result<(PathBuf, PathBuf), CoreError> {
    if !runtime_dir.is_dir() {
        return Err(CoreError::InvalidInput(
            "Runtime directory does not exist".into(),
        ));
    }
    let mods_dir = runtime_dir.join("mods");
    if !mods_dir.is_dir() {
        return Err(CoreError::InvalidInput(format!(
            "Mods directory does not exist: {}",
            mods_dir.display()
        )));
    }
    let canonical = crate::canonical_plain(&mods_dir)?;
    if canonical != mods_dir {
        return Err(CoreError::InvalidInput(format!(
            "Mods directory must not be redirected: {}",
            mods_dir.display()
        )));
    }
    Ok((mods_dir, canonical))
}

fn verified_regular_file(
    mods_dir: &Path,
    canonical_mods: &Path,
    file_name: &str,
) -> Result<PathBuf, CoreError> {
    let target = mods_dir.join(file_name);
    let metadata = match fs::symlink_metadata(&target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CoreError::InvalidInput(format!(
                "Mod file does not exist: {file_name}"
            )));
        }
        Err(error) => return Err(io_error(&target, error)),
    };
    if metadata.is_symlink() {
        return Err(CoreError::InvalidInput(format!(
            "Mod file must not be a symbolic link: {file_name}"
        )));
    }
    if !metadata.is_file() {
        return Err(CoreError::InvalidInput(format!(
            "Mod file must be a regular file: {file_name}"
        )));
    }
    let canonical = crate::canonical_plain(&target)?;
    if canonical != target || canonical.parent() != Some(canonical_mods) {
        return Err(CoreError::InvalidInput(format!(
            "Mod file must live inside the mods directory: {file_name}"
        )));
    }
    Ok(target)
}

fn validate_mod_file_name(file_name: &str) -> Result<(), CoreError> {
    const FORBIDDEN: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
    if file_name.is_empty()
        || file_name == "."
        || file_name == ".."
        || file_name.starts_with(['.', ' '])
        || file_name.ends_with(['.', ' '])
    {
        return Err(CoreError::InvalidInput(format!(
            "Invalid mod file name: {file_name:?}"
        )));
    }
    if file_name
        .chars()
        .any(|character| FORBIDDEN.contains(&character) || character.is_control())
    {
        return Err(CoreError::InvalidInput(format!(
            "Invalid mod file name: {file_name:?}"
        )));
    }
    Ok(())
}

/// Length of the prefix before `suffix`, when `file_name` ends with `suffix` ignoring
/// ASCII case. The suffix is ASCII, so a match guarantees the cut index is a char boundary.
fn ascii_case_suffix_len(file_name: &str, suffix: &[u8]) -> Option<usize> {
    let bytes = file_name.as_bytes();
    let cut = bytes.len().checked_sub(suffix.len())?;
    let tail = &bytes[cut..];
    if tail
        .iter()
        .zip(suffix.iter())
        .all(|(actual, expected)| actual.eq_ignore_ascii_case(expected))
    {
        Some(cut)
    } else {
        None
    }
}

fn mod_display_name(file_name: &str) -> &str {
    if let Some(cut) = ascii_case_suffix_len(file_name, b".jar.disabled") {
        return &file_name[..cut];
    }
    if let Some(cut) = ascii_case_suffix_len(file_name, b".jar") {
        return &file_name[..cut];
    }
    file_name
}

fn modified_epoch_millis(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// 更新后沿用旧文件的命名风格：把新旧文件名按 `-` 拆段，
/// 剥掉前导与后导相同的段，只替换中间变掉的那一段。
///
/// `sodium-fabric-0.5.1.jar` + `sodium-fabric-0.5.2.jar` → `sodium-fabric-0.5.2.jar`
/// `x-1.0.jar.disabled` + `x-2.0.jar` → `x-2.0.jar.disabled`
pub fn merge_file_name(current: &str, newest: &str) -> String {
    fn strip_suffixes(value: &str) -> String {
        let mut text = value;
        loop {
            let mut trimmed = false;
            for suffix in [".jar", ".old", ".disabled"] {
                if let Some(rest) = text.strip_suffix(suffix) {
                    text = rest;
                    trimmed = true;
                }
            }
            if !trimmed {
                break;
            }
        }
        text.to_owned()
    }

    let current_stem = strip_suffixes(current);
    let newest_stem = strip_suffixes(newest);
    if current_stem.is_empty() || newest_stem.is_empty() {
        return newest.to_owned();
    }
    let left: Vec<&str> = current_stem.split('-').collect();
    let right: Vec<&str> = newest_stem.split('-').collect();
    let mut start = 0;
    while start < left.len() && start < right.len() && left[start] == right[start] {
        start += 1;
    }
    let mut left_end = left.len();
    let mut right_end = right.len();
    while left_end > start && right_end > start && left[left_end - 1] == right[right_end - 1] {
        left_end -= 1;
        right_end -= 1;
    }
    let current_diff = left[start..left_end].join("-");
    let newest_diff = right[start..right_end].join("-");
    if current_diff.is_empty() || newest_diff.is_empty() || !current.contains(&current_diff) {
        return newest.to_owned();
    }
    current.replacen(&current_diff, &newest_diff, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be valid")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("ybumcl-mods-{suffix}"));
            fs::create_dir_all(&path).expect("temporary directory should be created");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn canonical_runtime(temp: &TempDir) -> PathBuf {
        resolve_runtime_directory(temp.0.to_str().expect("utf-8 path"), None)
            .expect("runtime should resolve")
    }

    fn create_symlink_file(link: &Path, target: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link)
        }
    }

    #[test]
    fn mod_entry_serializes_with_camel_case_fields() {
        let entry = ModEntry {
            file_name: "opti.jar".to_owned(),
            mod_name: "opti".to_owned(),
            enabled: true,
            size_bytes: 7,
            modified_millis: 5,
            error: None,
            metadata: None,
            metadata_error: None,
        };
        let json = serde_json::to_value(&entry).expect("should serialize");
        assert!(json.get("fileName").is_some());
        assert!(json.get("modName").is_some());
        assert!(json.get("sizeBytes").is_some());
        assert!(json.get("modifiedMillis").is_some());
        assert!(json.get("file_name").is_none());
    }

    #[test]
    fn resolve_none_returns_canonical_root() {
        let temp = TempDir::new();
        let resolved = resolve_runtime_directory(temp.0.to_str().unwrap(), None).unwrap();
        assert_eq!(resolved, crate::canonical_plain(&temp.0).unwrap());
    }

    #[test]
    fn resolve_instances_creates_directory() {
        let temp = TempDir::new();
        let resolved =
            resolve_runtime_directory(temp.0.to_str().unwrap(), Some("instances/abc")).unwrap();
        let expected = temp.0.join("instances").join("abc");
        assert!(expected.is_dir());
        assert_eq!(resolved, crate::canonical_plain(&expected).unwrap());
    }

    #[test]
    fn resolve_rejects_absolute_and_traversal_paths() {
        let temp = TempDir::new();
        let root = temp.0.to_str().unwrap();
        assert!(resolve_runtime_directory(root, Some("C:/Games/abc")).is_err());
        assert!(resolve_runtime_directory(root, Some("instances/../x")).is_err());
        assert!(resolve_runtime_directory(root, Some("instances/abc/def")).is_err());
    }

    #[test]
    fn resolve_rejects_missing_game_dir() {
        let temp = TempDir::new();
        let missing = temp.0.join("does-not-exist");
        assert!(resolve_runtime_directory(missing.to_str().unwrap(), None).is_err());
    }

    #[test]
    fn scan_lists_jar_and_disabled_jar() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let mods = runtime.join("mods");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("opti.jar"), b"jar!").unwrap();
        fs::write(mods.join("silent.jar.disabled"), b"silent").unwrap();
        fs::write(mods.join("modB.JAR"), b"MODB").unwrap();
        fs::write(mods.join("readme.txt"), b"ignore").unwrap();
        fs::write(mods.join("worlds.zip"), b"ignore").unwrap();
        fs::create_dir_all(mods.join("subfolder")).unwrap();
        fs::write(mods.join("subfolder").join("nested.jar"), b"ignore").unwrap();

        let entries = scan_mods(&runtime).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].file_name, "modB.JAR");
        assert_eq!(entries[0].mod_name, "modB");
        assert!(entries[0].enabled);
        assert_eq!(entries[0].size_bytes, 4);
        assert!(entries[0].modified_millis > 0);
        assert!(entries[0].error.is_none());

        assert_eq!(entries[1].file_name, "opti.jar");
        assert_eq!(entries[1].mod_name, "opti");
        assert!(entries[1].enabled);

        assert_eq!(entries[2].file_name, "silent.jar.disabled");
        assert_eq!(entries[2].mod_name, "silent");
        assert!(!entries[2].enabled);
    }

    #[test]
    fn scan_on_empty_mods_dir_returns_empty_vec() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        fs::create_dir_all(runtime.join("mods")).unwrap();
        assert!(scan_mods(&runtime).unwrap().is_empty());
    }

    #[test]
    fn scan_ignores_jars_outside_mods_and_tolerates_missing_mods_dir() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        fs::write(runtime.join("loose.jar"), b"loose").unwrap();
        fs::create_dir_all(runtime.join("x")).unwrap();
        fs::write(runtime.join("x").join("x.jar"), b"nested").unwrap();

        // A runtime directory without `mods/` simply has no mods.
        assert!(scan_mods(&runtime).unwrap().is_empty());

        fs::create_dir_all(runtime.join("mods")).unwrap();
        let entries = scan_mods(&runtime).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn scan_rejects_non_directory_runtime() {
        let temp = TempDir::new();
        let missing = temp.0.join("missing-runtime");
        assert!(scan_mods(&missing).is_err());
    }

    #[test]
    fn scan_skips_symlinked_jars() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let mods = runtime.join("mods");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("plain.jar"), b"plain").unwrap();

        let outside = runtime.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("real.jar"), b"real").unwrap();
        let link = mods.join("linked.jar");
        if create_symlink_file(&link, &outside.join("real.jar")).is_err() {
            return;
        }
        let entries = scan_mods(&runtime).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_name, "plain.jar");
    }

    #[test]
    fn toggle_roundtrip_preserves_content() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let mods = runtime.join("mods");
        fs::create_dir_all(&mods).unwrap();
        let bytes = vec![1u8, 2, 3, 255, 0, 12];
        fs::write(mods.join("a.jar"), &bytes).unwrap();

        let disabled = set_mod_enabled(&runtime, "a.jar", false).unwrap();
        assert_eq!(disabled.file_name, "a.jar.disabled");
        assert_eq!(disabled.mod_name, "a");
        assert!(!disabled.enabled);
        assert_eq!(fs::read(mods.join("a.jar.disabled")).unwrap(), bytes);
        assert!(!mods.join("a.jar").exists());

        let enabled = set_mod_enabled(&runtime, "a.jar.disabled", true).unwrap();
        assert_eq!(enabled.file_name, "a.jar");
        assert_eq!(enabled.mod_name, "a");
        assert!(enabled.enabled);
        assert_eq!(fs::read(mods.join("a.jar")).unwrap(), bytes);
    }

    #[test]
    fn toggle_rejects_mismatched_suffixes() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let mods = runtime.join("mods");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("a.jar"), b"x").unwrap();
        fs::write(mods.join("b.jar.disabled"), b"y").unwrap();

        assert!(set_mod_enabled(&runtime, "a.jar", true).is_err());
        assert!(set_mod_enabled(&runtime, "b.jar.disabled", false).is_err());
        assert!(set_mod_enabled(&runtime, "c.txt", true)
            .unwrap_err()
            .to_string()
            .contains("Only .jar mods can be toggled"));
        assert!(set_mod_enabled(&runtime, "c.txt", false)
            .unwrap_err()
            .to_string()
            .contains("Only .jar mods can be toggled"));
    }

    #[test]
    fn toggle_conflict_keeps_source_intact() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let mods = runtime.join("mods");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("a.jar"), b"target").unwrap();
        fs::write(mods.join("a.jar.disabled"), b"source").unwrap();

        let result = set_mod_enabled(&runtime, "a.jar.disabled", true);
        assert!(result.is_err());
        assert_eq!(fs::read(mods.join("a.jar")).unwrap(), b"target");
        assert_eq!(fs::read(mods.join("a.jar.disabled")).unwrap(), b"source");
    }

    #[test]
    fn toggle_missing_file_errors() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        fs::create_dir_all(runtime.join("mods")).unwrap();
        assert!(set_mod_enabled(&runtime, "ghost.jar", false).is_err());
        assert!(set_mod_enabled(&runtime, "ghost.jar.disabled", true).is_err());
    }

    #[test]
    fn toggle_rejects_invalid_names() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        fs::create_dir_all(runtime.join("mods")).unwrap();
        let invalid = [
            "", ".", "..", "a.jar/", "a\\b.jar", "a:b.jar", "a*b.jar", "a?b.jar", "a\"b.jar",
            "a<b.jar", "a>b.jar", "a|b.jar", ".a.jar", "a.jar.", " a.jar", "a.jar ", "a\0.jar",
        ];
        for name in invalid {
            assert!(
                set_mod_enabled(&runtime, name, false).is_err(),
                "should reject {name:?}"
            );
        }
    }

    #[test]
    fn delete_removes_jar_and_disabled_jar() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let mods = runtime.join("mods");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("a.jar"), b"jar").unwrap();
        fs::write(mods.join("b.jar.disabled"), b"disabled").unwrap();

        delete_mod(&runtime, "a.jar", true).unwrap();
        assert!(!mods.join("a.jar").exists());
        delete_mod(&runtime, "b.jar.disabled", true).unwrap();
        assert!(!mods.join("b.jar.disabled").exists());
    }

    #[test]
    fn delete_refuses_non_jar_files() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let mods = runtime.join("mods");
        fs::create_dir_all(&mods).unwrap();
        for name in ["worlds.zip", "options.txt", "notes.txt", "notatoggl"] {
            fs::write(mods.join(name), b"data").unwrap();
            assert!(delete_mod(&runtime, name, true).is_err(), "should refuse {name}");
            assert!(mods.join(name).exists(), "should leave {name} intact");
        }
    }

    #[test]
    fn delete_refuses_directories_symlinks_and_traversal() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let mods = runtime.join("mods");
        fs::create_dir_all(&mods).unwrap();

        fs::create_dir_all(mods.join("d.jar")).unwrap();
        assert!(delete_mod(&runtime, "d.jar", true).is_err());
        assert!(mods.join("d.jar").is_dir());

        let outside = runtime.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("real.jar"), b"real").unwrap();
        let link = mods.join("s.jar");
        if create_symlink_file(&link, &outside.join("real.jar")).is_ok() {
            assert!(delete_mod(&runtime, "s.jar", true).is_err());
            assert!(link.exists() || link.is_symlink());
        }

        assert!(delete_mod(&runtime, "nested/x.jar", true).is_err());
        assert!(delete_mod(&runtime, "x.jar/../../y.jar", true).is_err());
        assert!(delete_mod(&runtime, "evil.jar\\..\\..\\outside.jar", true).is_err());
    }

    #[test]
    fn delete_missing_file_errors() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        fs::create_dir_all(runtime.join("mods")).unwrap();
        assert!(delete_mod(&runtime, "ghost.jar", true).is_err());
    }

    #[test]
    fn mods_directory_rejects_redirected_mods_dir() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let mods = runtime.join("mods");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("a.jar"), b"x").unwrap();

        let outside = runtime.join("outside-mods");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("a.jar"), b"y").unwrap();

        // Replace mods with a symlink to outside-mods if privileges allow.
        let _ = fs::remove_dir_all(&mods);
        if create_symlink_file(&mods, &outside).is_err() {
            // Windows may lack symlink privilege; skip gracefully.
            let _ = fs::create_dir_all(&mods);
            fs::write(mods.join("a.jar"), b"x").unwrap();
            return;
        }
        // Now mods is a symlink; operations must refuse.
        assert!(scan_mods(&runtime).is_err() || scan_mods(&runtime).unwrap().is_empty());
        assert!(set_mod_enabled(&runtime, "a.jar", false).is_err());
        assert!(delete_mod(&runtime, "a.jar", true).is_err());
    }

    /// Build a jar at `path` containing `entries`, using the `zip` writer.
    fn write_jar(path: &Path, entries: &[(&str, &str)]) {
        let file = fs::File::create(path).expect("jar file should be created");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            zip.start_file(*name, options)
                .expect("zip entry should start");
            std::io::Write::write_all(&mut zip, body.as_bytes())
                .expect("zip entry should be written");
        }
        zip.finish().expect("jar should finish");
    }

    fn source_strings(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn inspect_reads_fabric_descriptor() {
        let temp = TempDir::new();
        let jar = temp.0.join("fabric-mod.jar");
        write_jar(
            &jar,
            &[(
                "fabric.mod.json",
                r#"{
                    "id": "coolmod",
                    "name": "Cool Mod",
                    "version": "1.2.3",
                    "description": "Does cool things.",
                    "authors": ["Alice", { "name": "Bob" }, "Carol"],
                    "icon": "assets/coolmod/icon.png"
                }"#,
            )],
        );

        let metadata = inspect_mod_jar(&jar).expect("fabric jar should be inspected");
        assert_eq!(metadata.mod_id.as_deref(), Some("coolmod"));
        assert_eq!(metadata.name.as_deref(), Some("Cool Mod"));
        assert_eq!(metadata.version.as_deref(), Some("1.2.3"));
        assert_eq!(metadata.authors, vec!["Alice", "Bob", "Carol"]);
        assert_eq!(metadata.description.as_deref(), Some("Does cool things."));
        assert_eq!(metadata.loader.as_deref(), Some("fabric"));
        assert!(metadata.icon);
        assert_eq!(metadata.descriptor.as_deref(), Some("fabric.mod.json"));
    }

    #[test]
    fn inspect_accepts_single_author_string_and_missing_icon() {
        let temp = TempDir::new();
        let jar = temp.0.join("solo.jar");
        write_jar(
            &jar,
            &[(
                "fabric.mod.json",
                r#"{"id":"solo","authors":"Solo Dev","icon":{}}"#,
            )],
        );

        let metadata = inspect_mod_jar(&jar).unwrap();
        assert_eq!(metadata.authors, vec!["Solo Dev"]);
        assert_eq!(metadata.name, None);
        assert_eq!(metadata.version, None);
        assert_eq!(metadata.description, None);
        assert!(metadata.icon, "an icon object also declares an icon");
    }

    #[test]
    fn inspect_reads_quilt_descriptor() {
        let temp = TempDir::new();
        let jar = temp.0.join("quilt.jar");
        write_jar(
            &jar,
            &[(
                "quilt.mod.json",
                r#"{
                    "schema_version": 1,
                    "quilt_loader": {
                        "id": "quiltmod",
                        "version": "0.4.0",
                        "metadata": {
                            "name": "Quilt Mod",
                            "description": "Quilted.",
                            "icon": "icon.png",
                            "contributors": { "Dana": "owner", "Eve": "developer" }
                        }
                    }
                }"#,
            )],
        );

        let metadata = inspect_mod_jar(&jar).unwrap();
        assert_eq!(metadata.mod_id.as_deref(), Some("quiltmod"));
        assert_eq!(metadata.name.as_deref(), Some("Quilt Mod"));
        assert_eq!(metadata.version.as_deref(), Some("0.4.0"));
        assert_eq!(metadata.description.as_deref(), Some("Quilted."));
        assert_eq!(metadata.loader.as_deref(), Some("quilt"));
        assert!(metadata.icon);
        assert_eq!(metadata.descriptor.as_deref(), Some("quilt.mod.json"));
        let mut authors = metadata.authors;
        authors.sort();
        assert_eq!(authors, vec!["Dana", "Eve"]);
    }

    #[test]
    fn inspect_reads_forge_and_neoforge_toml() {
        let temp = TempDir::new();
        let forge = temp.0.join("forge.jar");
        write_jar(
            &forge,
            &[(
                "META-INF/mods.toml",
                "modLoader=\"javafml\"\n\
                 [[mods]] # the first table\n\
                 modId=\"forgemod\"\n\
                 displayName=\"Forge Mod\"\n\
                 version=\"2.0\"\n\
                 authors=\"Ann, Ben\"\n\
                 description='Forge description'\n\
                 logoFile=\"logo.png\"\n\
                 [[dependencies.forgemod]]\n\
                 modId=\"forge\"\n",
            )],
        );
        let metadata = inspect_mod_jar(&forge).unwrap();
        assert_eq!(metadata.loader.as_deref(), Some("forge"));
        assert_eq!(metadata.mod_id.as_deref(), Some("forgemod"));
        assert_eq!(metadata.name.as_deref(), Some("Forge Mod"));
        assert_eq!(metadata.version.as_deref(), Some("2.0"));
        assert_eq!(metadata.authors, vec!["Ann", "Ben"]);
        assert_eq!(metadata.description.as_deref(), Some("Forge description"));
        assert!(metadata.icon);
        assert_eq!(metadata.descriptor.as_deref(), Some("META-INF/mods.toml"));

        let neoforge = temp.0.join("neoforge.jar");
        write_jar(
            &neoforge,
            &[(
                "META-INF/neoforge.mods.toml",
                "[[mods]]\nmodId=\"neomod\"\ndisplayName=\"Neo Mod\"\nversion=\"3.0\"\n",
            )],
        );
        let metadata = inspect_mod_jar(&neoforge).unwrap();
        assert_eq!(metadata.loader.as_deref(), Some("neoforge"));
        assert_eq!(metadata.mod_id.as_deref(), Some("neomod"));
        assert_eq!(metadata.name.as_deref(), Some("Neo Mod"));
        assert_eq!(metadata.version.as_deref(), Some("3.0"));
        assert!(metadata.authors.is_empty());
        assert!(!metadata.icon);
        assert_eq!(
            metadata.descriptor.as_deref(),
            Some("META-INF/neoforge.mods.toml")
        );
    }

    #[test]
    fn neoforge_descriptor_wins_over_forge_descriptor() {
        let temp = TempDir::new();
        let jar = temp.0.join("both.jar");
        write_jar(
            &jar,
            &[
                (
                    "META-INF/mods.toml",
                    "[[mods]]\nmodId=\"forge-first\"\ndisplayName=\"Forge\"\n",
                ),
                (
                    "META-INF/neoforge.mods.toml",
                    "[[mods]]\nmodId=\"neo-first\"\ndisplayName=\"Neo\"\n",
                ),
            ],
        );
        let metadata = inspect_mod_jar(&jar).unwrap();
        assert_eq!(metadata.loader.as_deref(), Some("neoforge"));
        assert_eq!(metadata.mod_id.as_deref(), Some("neo-first"));
    }

    #[test]
    fn inspect_reads_legacy_mcmod_info() {
        let temp = TempDir::new();
        let jar = temp.0.join("legacy.jar");
        write_jar(
            &jar,
            &[(
                "mcmod.info",
                r#"[{
                    "modid": "legacymod",
                    "name": "Legacy Mod",
                    "version": "1.7.10-1",
                    "authorList": ["Old Timer"],
                    "description": "Ancient.",
                    "logoFile": "logo.png"
                }]"#,
            )],
        );
        let metadata = inspect_mod_jar(&jar).unwrap();
        assert_eq!(metadata.loader.as_deref(), Some("legacy"));
        assert_eq!(metadata.mod_id.as_deref(), Some("legacymod"));
        assert_eq!(metadata.name.as_deref(), Some("Legacy Mod"));
        assert_eq!(metadata.version.as_deref(), Some("1.7.10-1"));
        assert_eq!(metadata.authors, vec!["Old Timer"]);
        assert_eq!(metadata.description.as_deref(), Some("Ancient."));
        assert!(metadata.icon);
        assert_eq!(metadata.descriptor.as_deref(), Some("mcmod.info"));
    }

    #[test]
    fn plain_jar_reports_empty_metadata() {
        let temp = TempDir::new();
        let jar = temp.0.join("library.jar");
        write_jar(&jar, &[("com/example/Lib.class", "not really a class")]);

        let metadata = inspect_mod_jar(&jar).unwrap();
        assert_eq!(metadata, ModMetadata::empty());
        assert_eq!(metadata.descriptor, None);
        assert_eq!(metadata.loader, None);
        assert!(metadata.authors.is_empty());
        assert!(!metadata.icon);
    }

    #[test]
    fn inspect_rejects_non_zip_and_missing_paths() {
        let temp = TempDir::new();
        let bogus = temp.0.join("bogus.jar");
        fs::write(&bogus, b"this is not a zip archive").unwrap();
        assert!(matches!(
            inspect_mod_jar(&bogus).unwrap_err(),
            CoreError::Archive(_)
        ));
        assert!(matches!(
            inspect_mod_jar(&temp.0.join("ghost.jar")).unwrap_err(),
            CoreError::MissingFile(_)
        ));
        assert!(matches!(
            inspect_mod_jar(&temp.0).unwrap_err(),
            CoreError::MissingFile(_)
        ));
    }

    #[test]
    fn description_is_truncated_to_500_chars() {
        let temp = TempDir::new();
        let jar = temp.0.join("long.jar");
        let long = "龙".repeat(600);
        let body = serde_json::json!({ "id": "long", "description": long }).to_string();
        write_jar(&jar, &[("fabric.mod.json", &body)]);
        let metadata = inspect_mod_jar(&jar).unwrap();
        let description = metadata.description.expect("description should survive");
        assert_eq!(description.chars().count(), 500);
    }

    #[test]
    fn scan_enriches_metadata_and_survives_corrupt_descriptors() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let mods = runtime.join("mods");
        fs::create_dir_all(&mods).unwrap();
        write_jar(
            &mods.join("good.jar"),
            &[("fabric.mod.json", r#"{"id":"good","name":"Good"}"#)],
        );
        write_jar(
            &mods.join("disabled.jar.disabled"),
            &[("fabric.mod.json", r#"{"id":"off","name":"Off"}"#)],
        );
        write_jar(
            &mods.join("broken.jar"),
            &[("fabric.mod.json", "{ not json")],
        );
        fs::write(mods.join("notzip.jar"), b"garbage").unwrap();

        let entries = scan_mods(&runtime).unwrap();
        assert_eq!(entries.len(), 4);

        let good = entries.iter().find(|e| e.file_name == "good.jar").unwrap();
        assert_eq!(
            good.metadata.as_ref().and_then(|m| m.mod_id.as_deref()),
            Some("good")
        );
        assert!(good.metadata_error.is_none());

        let disabled = entries
            .iter()
            .find(|e| e.file_name == "disabled.jar.disabled")
            .unwrap();
        assert!(!disabled.enabled);
        assert_eq!(
            disabled.metadata.as_ref().and_then(|m| m.mod_id.as_deref()),
            Some("off")
        );

        for name in ["broken.jar", "notzip.jar"] {
            let entry = entries.iter().find(|e| e.file_name == name).unwrap();
            assert!(entry.metadata.is_none(), "{name} should have no metadata");
            assert!(
                entry.metadata_error.is_some(),
                "{name} should report a metadata error"
            );
        }
    }

    #[test]
    fn scan_without_mods_dir_is_empty() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        assert!(!runtime.join("mods").exists());
        assert!(scan_mods(&runtime).unwrap().is_empty());
    }

    #[test]
    fn import_creates_mods_directory_and_copies_metadata() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let source = temp.0.join("download").join("cool.jar");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        write_jar(
            &source,
            &[(
                "fabric.mod.json",
                r#"{"id":"cool","name":"Cool","version":"1"}"#,
            )],
        );
        assert!(!runtime.join("mods").exists());

        let outcomes = import_mods(&runtime, &source_strings(&[source]), false).unwrap();
        assert_eq!(outcomes.len(), 1);
        let outcome = &outcomes[0];
        assert_eq!(outcome.status, "imported");
        assert_eq!(outcome.file_name.as_deref(), Some("cool.jar"));
        assert!(outcome.message.is_none());
        assert_eq!(
            outcome.metadata.as_ref().and_then(|m| m.mod_id.as_deref()),
            Some("cool")
        );
        let imported = runtime.join("mods").join("cool.jar");
        assert!(imported.is_file());

        let entries = scan_mods(&runtime).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].enabled);
    }

    #[test]
    fn import_without_overwrite_reports_duplicate() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let source = temp.0.join("dup.jar");
        write_jar(&source, &[("a.txt", "one")]);
        let sources = source_strings(std::slice::from_ref(&source));

        let first = import_mods(&runtime, &sources, false).unwrap();
        assert_eq!(first[0].status, "imported");

        fs::write(&source, b"").unwrap();
        write_jar(&source, &[("a.txt", "two")]);
        let second = import_mods(&runtime, &sources, false).unwrap();
        assert_eq!(second[0].status, "duplicate");
        assert_eq!(second[0].file_name.as_deref(), Some("dup.jar"));
        let message = second[0].message.as_deref().unwrap();
        assert!(message.contains("dup.jar"), "message: {message}");
    }

    #[test]
    fn import_with_overwrite_replaces_existing_jar_and_disabled() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let mods = runtime.join("mods");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("dup.jar"), b"old").unwrap();
        fs::write(mods.join("other.jar.disabled"), b"old-disabled").unwrap();

        let source = temp.0.join("dup.jar");
        write_jar(&source, &[("new.txt", "new")]);
        let other = temp.0.join("other.jar");
        write_jar(&other, &[("other.txt", "other")]);

        let outcomes = import_mods(&runtime, &source_strings(&[source, other]), true).unwrap();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].status, "imported");
        assert_eq!(outcomes[1].status, "imported");

        assert!(mods.join("dup.jar").is_file());
        assert!(mods.join("other.jar").is_file());
        assert!(
            !mods.join("other.jar.disabled").exists(),
            "the disabled copy should be replaced by an enabled jar"
        );
        let entries = scan_mods(&runtime).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| entry.enabled));
    }

    #[test]
    fn import_rejects_unusable_sources_and_continues_batch() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let sdir = temp.0.join("sources");
        fs::create_dir_all(&sdir).unwrap();

        let not_jar = sdir.join("readme.txt");
        fs::write(&not_jar, b"hello").unwrap();
        let disabled = sdir.join("off.jar.disabled");
        write_jar(&disabled, &[("a.txt", "a")]);
        let empty = sdir.join("empty.jar");
        fs::write(&empty, b"").unwrap();
        let not_zip = sdir.join("notzip.jar");
        fs::write(&not_zip, b"garbage bytes").unwrap();
        let directory = sdir.join("folder.jar");
        fs::create_dir_all(&directory).unwrap();
        let missing = sdir.join("ghost.jar");

        let mut sources = vec![
            not_jar.to_string_lossy().into_owned(),
            disabled.to_string_lossy().into_owned(),
            empty.to_string_lossy().into_owned(),
            not_zip.to_string_lossy().into_owned(),
            directory.to_string_lossy().into_owned(),
            missing.to_string_lossy().into_owned(),
        ];
        let good = sdir.join("good.jar");
        write_jar(&good, &[("fabric.mod.json", r#"{"id":"ok"}"#)]);
        sources.push(good.to_string_lossy().into_owned());

        let outcomes = import_mods(&runtime, &sources, false).unwrap();
        assert_eq!(outcomes.len(), sources.len());
        for outcome in &outcomes[..sources.len() - 1] {
            assert_eq!(outcome.status, "rejected", "{}", outcome.source_path);
            assert!(
                outcome.message.as_deref().is_some_and(|m| !m.is_empty()),
                "rejection needs a message: {}",
                outcome.source_path
            );
            assert!(outcome.metadata.is_none());
        }
        let non_zip = &outcomes[3];
        assert!(
            non_zip
                .message
                .as_deref()
                .is_some_and(|m| m.contains("不是有效的 JAR 文件")),
            "message: {:?}",
            non_zip.message
        );
        let last = outcomes.last().unwrap();
        assert_eq!(
            last.status, "imported",
            "batch must continue after rejections"
        );
        assert_eq!(last.file_name.as_deref(), Some("good.jar"));
    }

    #[test]
    fn import_rejects_unsafe_file_names() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let long_name = format!("{}.jar", "a".repeat(160));
        let long_path = temp.0.join(&long_name);
        write_jar(&long_path, &[("a.txt", "a")]);
        let outcomes = import_mods(&runtime, &source_strings(&[long_path]), false).unwrap();
        assert_eq!(outcomes[0].status, "rejected");
        assert!(outcomes[0].file_name.is_none());
    }

    #[test]
    fn import_rejects_missing_runtime_and_empty_sources() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        assert!(import_mods(&runtime, &[], false).unwrap().is_empty());
        assert!(!runtime.join("mods").exists(), "no sources, no directory");

        let missing = temp.0.join("nowhere");
        assert!(import_mods(&missing, &["a.jar".to_owned()], false).is_err());
    }

    #[test]
    fn import_skips_repeat_of_same_basename_in_one_batch() {
        let temp = TempDir::new();
        let runtime = canonical_runtime(&temp);
        let first_dir = temp.0.join("one");
        let second_dir = temp.0.join("two");
        fs::create_dir_all(&first_dir).unwrap();
        fs::create_dir_all(&second_dir).unwrap();
        let first = first_dir.join("same.jar");
        let second = second_dir.join("same.jar");
        write_jar(&first, &[("a.txt", "a")]);
        write_jar(&second, &[("b.txt", "b")]);

        let outcomes = import_mods(&runtime, &source_strings(&[first, second]), false).unwrap();
        assert_eq!(outcomes[0].status, "imported");
        assert_eq!(outcomes[1].status, "duplicate");
    }
}

#[cfg(test)]
mod merge_tests {
    use super::merge_file_name;

    #[test]
    fn keeps_the_old_naming_style() {
        assert_eq!(
            merge_file_name("sodium-fabric-0.5.1.jar", "sodium-fabric-0.5.2.jar"),
            "sodium-fabric-0.5.2.jar"
        );
        assert_eq!(
            merge_file_name("jei-1.21.1-forge-19.0.0.jar", "jei-1.21.1-forge-19.1.0.jar"),
            "jei-1.21.1-forge-19.1.0.jar"
        );
    }

    #[test]
    fn keeps_the_disabled_suffix() {
        assert_eq!(
            merge_file_name("x-1.0.jar.disabled", "x-2.0.jar"),
            "x-2.0.jar.disabled"
        );
    }

    #[test]
    fn falls_back_to_the_new_name() {
        assert_eq!(merge_file_name("abc.jar", "def.jar"), "def.jar");
        assert_eq!(merge_file_name("1.2-forge-2.jar", "1.3-forge-3.jar"), "1.3-forge-3.jar");
    }
}
