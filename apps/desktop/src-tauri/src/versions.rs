//! 版本即实例：一个版本（`versions/<id>`）就是一个可启动单元。
//!
//! 每个实例自己的设置存在实例文件夹里：`versions/<id>/ybmcl.json`（PCL 的
//! `versions/<id>/PCL/Setup.ini` 是同一个位置）。这样实例文件夹整个拷走、删掉，
//! 设置都跟着走，不会和别的游戏目录里同名实例串。
//!
//! 数据目录里的 `version-settings.json` 是旧版按名字整体存的格式，只在启动时
//! 一次性搬进各实例文件夹，搬完改名 `.bak`；`instances.json` 更早就退役了。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 旧版整体存储（只读一次，用来迁移）。
pub const STORE_FILE: &str = "version-settings.json";
/// 实例文件夹里的设置文件名。
pub const INSTANCE_FILE: &str = "ybmcl.json";
pub const LEGACY_REGISTRY: &str = "instances.json";
const SCHEMA_VERSION: u32 = 1;
const MAX_VERSIONS: usize = 512;
const MAX_ID_LEN: usize = 128;
/// 自定义内存的下限（MB）。
pub const MIN_MEMORY_MB: u32 = 512;
const MAX_MEMORY_MB: u32 = 32768;

fn store_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 整合包信息：从整合包装出来的实例才有，用来在概览页显示来源和版本。
///
/// 手动装的实例没有这一段，概览页就退化成「只有启动次数和 Minecraft」的形态。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PackInfo {
    /// 来源：`modrinth` / `curseforge` / `ftb` / `zip`。
    pub source: String,
    pub project_id: String,
    pub project_name: String,
    /// 整合包自己的版本号，例如 `15.0.0-beta.2`。
    pub version_label: String,
    pub summary: String,
    pub downloads: u64,
    pub tags: Vec<String>,
}

/// 单个版本的启动配置。字段缺省时由全局设置兜底。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VersionSettings {
    /// 隔离：mods/存档放在 `versions/<id>` 里，而不是共享的 `.minecraft`。
    pub isolated: bool,
    /// 空字符串表示"自动挑选"（由前端用 requiredJavaMajor + 已探测列表决定）。
    pub java_path: String,
    /// 0 表示"跟随全局设置"（旧字段，仅给老 json 兜底；新逻辑看 `memory_type`）。
    pub max_memory_mb: u32,
    /// 内存分配方式：0 自动、1 自定义、2 跟随全局。
    pub memory_type: u32,
    /// 自定义内存（MB），`memory_type == 1` 时生效。
    pub memory_custom_mb: u32,
    /// 初始堆（MB），0 表示和最大堆一样。
    pub memory_initial_mb: u32,
    /// 用户自己写的实例描述（概览页「修改实例描述」）。
    pub description: String,
    /// 图标：空字符串=自动，`builtin:<方块名>`=内置图，`custom`=实例目录里的 `icon.png`。
    pub icon: String,
    /// 启动次数，概览页显示「从未启动」或「N 次」。
    pub launches: u64,
    /// 整合包信息；手动装的实例为 `None`。
    pub pack: Option<PackInfo>,
    /// 「游戏窗口标题」：启动后把游戏窗口的标题改成这句，空=不改。
    pub window_title: String,
    /// 「自定义信息」：覆盖版本 json 的 `type`，进 `${version_type}`。空=用版本自己的。
    pub version_type: String,
    /// 「JVM 参数头部」：非空时整段替换版本自带的 JVM 参数。
    pub jvm_args: String,
    /// 「游戏参数尾部」：追加在版本参数之后。
    pub game_args: String,
    /// 「Classpath 头部附加」：插在 classpath 最前。
    pub classpath_head: String,
    /// 「启动前执行命令」。
    pub pre_launch: String,
    /// 启动前命令是否等它跑完再启动游戏。
    pub pre_launch_wait: bool,
    /// Java 选「自动选择」时为 true（忽略全局里指定的那套）。
    pub java_auto: bool,
    /// 「关闭文件校验」：启动前不逐个检查文件在不在。
    pub skip_file_check: bool,
    /// 「使用调试级别 log4j 配置文件」。
    pub debug_log4j: bool,
    /// 「忽略 Java 兼容性警告」。
    pub ignore_java_warning: bool,
    /// 「限制验证方式」：0 不限制、1 仅正版、2 仅离线模式、3 仅第三方登录。
    pub login_require: u32,
    /// 「自动进入服务器」：`host` 或 `host:port`，空表示不自动进服。
    pub server_entry: String,
}

impl Default for VersionSettings {
    fn default() -> Self {
        // 新建版本的默认值：隔离开启（与 PCL 一致，避免 mods/存档互相污染）。
        Self {
            isolated: true,
            java_path: String::new(),
            max_memory_mb: 0,
            memory_type: 2,
            memory_custom_mb: 0,
            memory_initial_mb: 0,
            description: String::new(),
            icon: String::new(),
            launches: 0,
            pack: None,
            window_title: String::new(),
            version_type: String::new(),
            jvm_args: String::new(),
            game_args: String::new(),
            classpath_head: String::new(),
            pre_launch: String::new(),
            pre_launch_wait: true,
            java_auto: false,
            skip_file_check: false,
            debug_log4j: false,
            ignore_java_warning: false,
            login_require: 0,
            server_entry: String::new(),
        }
    }
}

impl VersionSettings {
    fn sanitized(mut self) -> Self {
        // 老 json 只有 `max_memory_mb`：有值就当「自定义」，没值就是「跟随全局」。
        if self.max_memory_mb != 0 && self.memory_custom_mb == 0 && self.memory_type == 2 {
            self.memory_type = 1;
            self.memory_custom_mb = self.max_memory_mb;
        }
        self.memory_type = self.memory_type.min(2);
        if self.memory_custom_mb != 0 {
            self.memory_custom_mb = self.memory_custom_mb.clamp(MIN_MEMORY_MB, MAX_MEMORY_MB);
        }
        if self.memory_initial_mb != 0 {
            self.memory_initial_mb = self.memory_initial_mb.clamp(MIN_MEMORY_MB, MAX_MEMORY_MB);
        }
        self.max_memory_mb = if self.memory_type == 1 {
            self.memory_custom_mb
        } else {
            0
        };
        self.java_path = self.java_path.trim().to_string();
        crate::versions::trim_text(&mut self.description, 200);
        crate::versions::trim_text(&mut self.window_title, 128);
        crate::versions::trim_text(&mut self.version_type, 32);
        crate::versions::trim_text(&mut self.jvm_args, 4000);
        crate::versions::trim_text(&mut self.game_args, 2000);
        crate::versions::trim_text(&mut self.classpath_head, 2000);
        crate::versions::trim_text(&mut self.pre_launch, 500);
        crate::versions::trim_text(&mut self.server_entry, 200);
        self.login_require = self.login_require.min(3);
        self.icon = sanitize_icon(&self.icon);
        self.launches = self.launches.min(100_000_000);
        self.pack = self.pack.map(|mut pack| {
            crate::versions::trim_text(&mut pack.summary, 400);
            crate::versions::trim_text(&mut pack.project_name, 80);
            crate::versions::trim_text(&mut pack.version_label, 48);
            crate::versions::trim_text(&mut pack.project_id, 64);
            crate::versions::trim_text(&mut pack.source, 16);
            pack.tags.truncate(8);
            for tag in &mut pack.tags {
                crate::versions::trim_text(tag, 24);
            }
            pack.tags.retain(|tag| !tag.is_empty());
            pack
        });
        self
    }
}

/// 内置方块图的名字，和前端 `src/lib/version-icon.ts` 里的键一一对应。
pub const ICON_BUILTINS: [&str; 14] = [
    "CobbleStone",
    "CommandBlock",
    "GoldBlock",
    "Grass",
    "GrassPath",
    "Anvil",
    "RedstoneBlock",
    "RedstoneLampOn",
    "RedstoneLampOff",
    "Egg",
    "Fabric",
    "NeoForge",
    "Quilt",
    "Cleanroom",
];

fn sanitize_icon(icon: &str) -> String {
    let icon = icon.trim();
    if icon.is_empty() {
        return String::new();
    }
    if icon == "custom" {
        return icon.to_owned();
    }
    match icon.strip_prefix("builtin:") {
        Some(name) if ICON_BUILTINS.contains(&name) => format!("builtin:{name}"),
        _ => String::new(),
    }
}

/// 截断到 `max` 个字符（不是字节），并去掉首尾空白。
pub fn trim_text(text: &mut String, max: usize) {
    *text = text.trim().chars().take(max).collect();
}

/// 启动成功后把实例的启动次数 +1。读不到或写不进就安静跳过，别影响启动。
pub fn bump_launches(game_dir: &Path, id: &str) {
    let mut settings = load_instance(game_dir, id);
    settings.launches = settings.launches.saturating_add(1);
    let _ = save_instance(game_dir, id, settings);
}

/// 改实例图标。`icon` 取值见 `sanitize_icon`；改成非 `custom` 时顺手删掉旧图。
pub fn set_icon(game_dir: &Path, id: &str, icon: &str) -> Result<VersionSettings, String> {
    let dir = instance_dir(game_dir, id);
    if !dir.is_dir() {
        return Err(format!("实例 {id} 不存在"));
    }
    let mut settings = load_instance(game_dir, id);
    settings.icon = sanitize_icon(icon).to_owned();
    if settings.icon != "custom" {
        remove_icon_files(&dir);
    }
    save_instance(game_dir, id, settings.clone())?;
    Ok(settings)
}

/// 把用户选好的图片拷成实例目录里的 `icon.png`，并把图标设成自定义。
pub fn set_icon_image(
    game_dir: &Path,
    id: &str,
    source: &Path,
) -> Result<VersionSettings, String> {
    let dir = instance_dir(game_dir, id);
    if !dir.is_dir() {
        return Err(format!("实例 {id} 不存在"));
    }
    let bytes = fs::read(source).map_err(|e| format!("读取图片失败：{e}"))?;
    if bytes.is_empty() {
        return Err("这个文件是空的".into());
    }
    remove_icon_files(&dir);
    let target = dir.join("icon.png");
    fs::write(&target, &bytes).map_err(|e| format!("写入图标失败：{e}"))?;
    let mut settings = load_instance(game_dir, id);
    settings.icon = "custom".into();
    save_instance(game_dir, id, settings.clone())?;
    Ok(settings)
}

fn remove_icon_files(dir: &Path) {
    for name in [
        "icon.png",
        "icon.jpg",
        "icon.jpeg",
        "icon.webp",
        "icon.gif",
    ] {
        let _ = fs::remove_file(dir.join(name));
    }
}

/// 只覆盖启动设置那四项，图标、启动次数、整合包信息原样保留。
pub fn save_launch_fields(
    game_dir: &Path,
    id: &str,
    patch: VersionSettings,
) -> Result<(), String> {
    // 设置页整张表单一起提交，所以这里整份替换；只有「启动次数」和「整合包来源」
    // 不归设置页管，永远以磁盘上的为准。
    let existing = load_instance(game_dir, id);
    let mut settings = patch;
    settings.launches = existing.launches;
    if settings.pack.is_none() {
        settings.pack = existing.pack;
    }
    save_instance(game_dir, id, settings)
}

/// 写入 / 清除整合包信息。
pub fn set_pack(game_dir: &Path, id: &str, pack: Option<PackInfo>) -> Result<VersionSettings, String> {
    let dir = instance_dir(game_dir, id);
    if !dir.is_dir() {
        return Err(format!("实例 {id} 不存在"));
    }
    let mut settings = load_instance(game_dir, id);
    settings.pack = pack;
    save_instance(game_dir, id, settings.clone())?;
    Ok(settings)
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Store {
    pub schema_version: u32,
    pub versions: BTreeMap<String, VersionSettings>,
}

/// 版本目录名能不能用：整合包名字常带空格、加号（例如 `Sodium Fps+`），
/// 所以只挡真正会让路径跑偏或写不出来的字符，不再限制成 ASCII 标识符。
pub fn is_safe_version_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_ID_LEN {
        return false;
    }
    if id != id.trim() || id.ends_with('.') {
        return false;
    }
    if id.contains("..") {
        return false;
    }
    if id
        .chars()
        .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return false;
    }
    // Windows 设备名（CON、NUL、COM1…）建不出目录。
    let stem = id.split('.').next().unwrap_or(id).to_ascii_uppercase();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    !RESERVED.contains(&stem.as_str())
}

/// 读配置；文件缺失/损坏都回落成空表，不让启动器因为配置坏了打不开。
pub fn load(data_dir: &Path) -> Store {
    let path = data_dir.join(STORE_FILE);
    let Ok(text) = fs::read_to_string(&path) else {
        return Store::default();
    };
    let mut store: Store = match serde_json::from_str(&text) {
        Ok(store) => store,
        Err(_) => return Store::default(),
    };
    if store.schema_version != SCHEMA_VERSION {
        return Store::default();
    }
    if store.versions.len() > MAX_VERSIONS {
        store.versions.clear();
    }
    store
}

/// 原子写：先写 `.part` 再 rename，避免掉电写坏。
pub fn save(data_dir: &Path, store: &Store) -> Result<(), String> {
    let _guard = store_lock().lock().map_err(|e| e.to_string())?;
    let path = data_dir.join(STORE_FILE);
    let mut out = Store {
        schema_version: SCHEMA_VERSION,
        versions: store.versions.clone(),
    };
    for (id, settings) in out.versions.iter_mut() {
        if !is_safe_version_id(id) {
            return Err(format!("非法的版本 id：{id}"));
        }
        *settings = settings.clone().sanitized();
    }
    let text = serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?;
    let part = path.with_extension("json.part");
    fs::write(&part, text).map_err(|e| format!("写入版本配置失败：{e}"))?;
    fs::rename(&part, &path).map_err(|e| format!("保存版本配置失败：{e}"))
}

/// 隔离模式下的游戏目录（相对 `.minecraft`）。不隔离返回 None = 用共享目录。
pub fn game_directory(id: &str, isolated: bool) -> Option<String> {
    isolated.then(|| format!("versions/{id}"))
}

/// 实例目录：`<game>/versions/<id>`。
pub fn instance_dir(game_dir: &Path, id: &str) -> PathBuf {
    game_dir.join("versions").join(id)
}

fn instance_path(game_dir: &Path, id: &str) -> PathBuf {
    instance_dir(game_dir, id).join(INSTANCE_FILE)
}

/// 读某个实例自己的设置；文件缺失或损坏都给默认值（不落盘）。
pub fn load_instance(game_dir: &Path, id: &str) -> VersionSettings {
    let Ok(text) = fs::read_to_string(instance_path(game_dir, id)) else {
        return VersionSettings::default();
    };
    serde_json::from_str::<VersionSettings>(&text)
        .map(VersionSettings::sanitized)
        .unwrap_or_default()
}

/// 写某个实例自己的设置。实例目录必须已经存在——不替用户凭空建实例文件夹。
pub fn save_instance(game_dir: &Path, id: &str, settings: VersionSettings) -> Result<(), String> {
    if !is_safe_version_id(id) {
        return Err(format!("非法的版本 id：{id}"));
    }
    let dir = instance_dir(game_dir, id);
    if !dir.is_dir() {
        return Err(format!("这个实例目录不存在：{}", dir.display()));
    }
    let path = dir.join(INSTANCE_FILE);
    let text = serde_json::to_string_pretty(&settings.sanitized()).map_err(|e| e.to_string())?;
    let part = dir.join(format!("{INSTANCE_FILE}.part"));
    fs::write(&part, text).map_err(|e| format!("写入实例设置失败（{}）：{e}", path.display()))?;
    fs::rename(&part, &path).map_err(|e| format!("保存实例设置失败（{}）：{e}", path.display()))
}

/// 一次性迁移：把数据目录里按名字存的旧配置搬进各实例文件夹。
/// 实例已经不在的条目直接丢掉；搬完把旧文件改名 `.bak`，以后不再读。
pub fn migrate_store_into_instances(data_dir: &Path, game_dir: &Path) {
    let legacy_path = data_dir.join(STORE_FILE);
    if !legacy_path.is_file() {
        return;
    }
    let store = load(data_dir);
    let mut moved = 0usize;
    let mut dropped = 0usize;
    for (id, settings) in store.versions.iter() {
        let dir = instance_dir(game_dir, id);
        if !dir.is_dir() || dir.join(INSTANCE_FILE).is_file() {
            dropped += 1;
            continue;
        }
        match save_instance(game_dir, id, settings.clone()) {
            Ok(()) => moved += 1,
            Err(_) => dropped += 1,
        }
    }
    let _ = fs::rename(&legacy_path, data_dir.join(format!("{STORE_FILE}.bak")));
    append_log(
        data_dir,
        &format!(
            "[migrate] 实例设置搬进实例文件夹：{moved} 条，丢弃 {dropped} 条（旧文件改名 {STORE_FILE}.bak）"
        ),
    );
}

/// 删除某版本在旧整体存储里的记录（删版本时顺手清掉，免得留下孤儿）。
pub fn forget(data_dir: &Path, id: &str) -> Result<(), String> {
    let mut store = load(data_dir);
    if store.versions.remove(id).is_some() {
        save(data_dir, &store)?;
    }
    Ok(())
}

/// 从旧的 `instances.json` 迁移：同一版本只留一条，优先保留隔离的那条。
/// 返回是否有迁移动作（用于日志）。
pub fn migrate_legacy(data_dir: &Path) -> Result<bool, String> {
    let legacy_path = data_dir.join(LEGACY_REGISTRY);
    if !legacy_path.is_file() {
        return Ok(false);
    }
    let modern_path = data_dir.join(STORE_FILE);
    if modern_path.is_file() {
        // 已经迁移过，只把旧文件留档。
        let _ = fs::rename(
            &legacy_path,
            data_dir.join(format!("{LEGACY_REGISTRY}.bak")),
        );
        return Ok(false);
    }

    let legacy = crate::instances::list_instances(data_dir)?;
    let mut store = Store {
        schema_version: SCHEMA_VERSION,
        versions: BTreeMap::new(),
    };
    let mut dropped = Vec::new();
    for instance in legacy {
        let candidate = VersionSettings {
            isolated: instance.isolated,
            java_path: instance.java_path.clone(),
            max_memory_mb: instance.max_memory_mb,
            ..VersionSettings::default()
        };
        match store.versions.get(&instance.version_id) {
            None => {
                store
                    .versions
                    .insert(instance.version_id.clone(), candidate);
            }
            Some(existing) => {
                // 同版本多条时保留隔离的（更安全）。
                if candidate.isolated && !existing.isolated {
                    store
                        .versions
                        .insert(instance.version_id.clone(), candidate);
                }
                dropped.push(format!("{}（{}）", instance.name, instance.version_id));
            }
        }
    }
    save(data_dir, &store)?;
    let _ = fs::rename(
        &legacy_path,
        data_dir.join(format!("{LEGACY_REGISTRY}.bak")),
    );
    if !dropped.is_empty() {
        append_log(
            data_dir,
            &format!(
                "[migrate] 同一版本多条实例，已合并并丢弃：{}",
                dropped.join("、")
            ),
        );
    }
    append_log(
        data_dir,
        &format!(
            "[migrate] 已把 instances.json 迁移为 {STORE_FILE}（{} 个版本），旧文件改名 {LEGACY_REGISTRY}.bak",
            store.versions.len()
        ),
    );
    Ok(true)
}

pub(crate) fn append_log(data_dir: &Path, line: &str) {
    use std::io::Write;
    let dir = data_dir.join("logs");
    let _ = fs::create_dir_all(&dir);
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("launcher.log"))
    {
        let _ = writeln!(file, "{line}");
    }
}

/// 改实例名 = 改实例文件夹名，外加把文件夹里跟名字绑定的三样一起改：
/// `<旧名>.json`、`<旧名>.jar`、json 里的 `"id"`。分类归属（含星标）跟着搬。
///
/// 读版本 json 的 `merge::read_version` 认的是 `<versions>/<id>/<id>.json`，
/// 只改文件夹名会让实例读不到 json（启动、导出、补全全按这个路径）。
pub fn rename_instance(
    game_dir: &Path,
    data_dir: &Path,
    from: &str,
    to: &str,
) -> Result<(), String> {
    if !is_safe_version_id(from) || !is_safe_version_id(to) {
        return Err("实例名里不能有路径符号".into());
    }
    if from == to {
        return Ok(());
    }
    let dir = game_dir.join("versions");
    let source = dir.join(from);
    let target = dir.join(to);
    if !source.is_dir() {
        return Err(format!("实例 {from} 不存在"));
    }
    if target.exists() {
        return Err(format!("版本 {to} 已存在，请换一个名字"));
    }
    fs::rename(&source, &target).map_err(|e| format!("重命名失败：{e}"))?;
    if let Err(error) = retitle(&target, from, to) {
        // 文件没改干净就把目录名退回去，别留一个半改的实例。
        let _ = fs::rename(&target, &source);
        return Err(error);
    }
    save_instance(game_dir, to, load_instance(game_dir, to))?;
    let _ = crate::categories::rename_tag(data_dir, &game_dir.to_string_lossy(), from, to);
    Ok(())
}

/// 把实例文件夹里的 json / jar / `"id"` 改成新名字。
///
/// 老版本只改过目录名，文件夹里的文件名可能跟 `from` 对不上（磁盘上就有这种
/// 历史实例）：这时退而求其次，找文件夹里唯一的那份版本 json / client jar 来改。
fn retitle(folder: &Path, from: &str, to: &str) -> Result<(), String> {
    let json_from = folder.join(format!("{from}.json"));
    let json_source = if json_from.is_file() {
        json_from
    } else {
        find_single(folder, "json").ok_or_else(|| format!("实例里找不到版本 json（{from}.json）"))?
    };
    let bytes = fs::read(&json_source).map_err(|e| format!("{}: {e}", json_source.display()))?;
    let mut value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("版本 json 解析失败：{e}"))?;
    if let Some(object) = value.as_object_mut() {
        object.insert("id".into(), serde_json::Value::String(to.to_owned()));
        // 改名后 base 版本没法再从名字里猜了，补一份 `_ybmcl`：
        // 加载器外壳用 `inheritsFrom`，合并/原版实例用旧名字（旧名字就是它的版本号）。
        if !object.contains_key("_ybmcl") {
            let base = object
                .get("inheritsFrom")
                .and_then(|item| item.as_str())
                .unwrap_or(from)
                .to_owned();
            object.insert(
                "_ybmcl".into(),
                serde_json::json!({ "gameVersion": base, "loaders": [] }),
            );
        }
    }
    let json_to = folder.join(format!("{to}.json"));
    let text = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
    fs::write(&json_to, text).map_err(|e| format!("{}: {e}", json_to.display()))?;
    if json_source != json_to {
        let _ = fs::remove_file(&json_source);
    }

    let jar_from = folder.join(format!("{from}.jar"));
    let jar_source = if jar_from.is_file() {
        Some(jar_from)
    } else {
        find_single(folder, "jar")
    };
    if let Some(jar_source) = jar_source {
        let jar_to = folder.join(format!("{to}.jar"));
        if jar_source != jar_to {
            fs::rename(&jar_source, &jar_to)
                .map_err(|e| format!("{}: {e}", jar_source.display()))?;
        }
    }
    Ok(())
}

/// 文件夹第一层里指定扩展名的唯一文件（`ybmcl.json` 除外）。
fn find_single(folder: &Path, extension: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(folder).ok()?;
    let mut found: Option<PathBuf> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some(extension) {
            continue;
        }
        if path.file_name().and_then(|value| value.to_str()) == Some(INSTANCE_FILE) {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(path);
    }
    found
}

/// 恢复实例的启动设置与外观（不动 mods / saves / config）。
pub fn reset_instance(game_dir: &Path, id: &str) -> Result<VersionSettings, String> {
    let dir = instance_dir(game_dir, id);
    if !dir.is_dir() {
        return Err(format!("实例 {id} 不存在"));
    }
    let mut settings = VersionSettings::default();
    let previous = load_instance(game_dir, id);
    // 整合包信息和启动次数是事实数据，重置不该抹掉。
    settings.pack = previous.pack;
    settings.launches = previous.launches;
    remove_icon_files(&dir);
    save_instance(game_dir, id, settings.clone())?;
    Ok(settings)
}
