//! 导出整合包：照 PCL 的「导出」页那套逻辑。
//!
//! 页面上的勾选项只是一张规则表（glob，`!` 反选，靠后的行覆盖靠前的），
//! 执行时分三步：按规则把实例文件复制到 `overrides/`、联网把本地文件认成
//! 项目下载地址、写 `modrinth.index.json` 并压成 `.mrpack`（或连同启动器压成 zip）。

use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::SystemTime,
};

use serde::Serialize;
use serde_json::{json, Map, Value};
use sha1::{Digest as _, Sha1};
use sha2::Sha512;

use crate::{CoreError, Result};

/// 配置文件里分隔三段的那条线（与 PCL 一致，62 个等号）。
pub const SEPARATOR: &str =
    "==============================================================";

/// 子选项黑名单：这两个名字的文件/文件夹不进导出列表（PCL 同款）。
const BLACKLIST: [&str; 2] = ["Quark Programmer Art.zip", "+ EuphoriaPatches_"];

// ───────────────────────── 选项表 ─────────────────────────

struct Spec {
    id: &'static str,
    parent: Option<&'static str>,
    title: &'static str,
    description: &'static str,
    rules: &'static [&'static str],
    show_rules: &'static [&'static str],
    default_checked: bool,
    require_mod_loader: bool,
    require_optifine: bool,
    require_mod_loader_or_optifine: bool,
    /// `resourcepacks` / `shaderpacks` / `saves`：要按目录里的实际内容动态列子项。
    dynamic: Option<&'static str>,
}

/// 永远不导出的东西：日志、缓存、启动器自己的记录（PCL 与 HMCL 两边的并集）。
const DEFAULT_EXCLUDE: &[&str] = &[
    "!*.log",
    "!*.dat_old",
    "!*.old",
    "!*.BakaCoreInfo",
    "!hmclversion.cfg",
    "!log4j2.xml",
    "!usernamecache.json",
    "!usercache.json",
    "!launcher_profiles.json",
    "!launcher_accounts.json",
    "!launcher_cef_log.txt",
    "!launcher_log.txt",
    "!launcher_msa_credentials.bin",
    "!launcher_settings.json",
    "!launcher_ui_state.json",
    "!realms_persistence.json",
    "!treatment_tags.json",
    "!webcache2/",
    "!clientId.txt",
    "!PCL.ini",
    "!manifest.json",
    "!modrinth.index.json",
    "!.fabric/",
    "!.mixin.out/",
    "!.optifine/",
    "!backup/",
    "!cache/",
    "!*-natives/",
];

const MOD_PACK_DATA: &[&str] = &[
    "hotai/",
    "bansoukou/",
    "addons/",
    "multiblocked/",
    "modpack-update-checker/",
    "global_packs/",
    "global_resource_packs/",
    "global_data_packs/",
    "optional_data_packs/",
    "moonlight-global-datapacks/",
    "maps/",
    "icon.png",
    "mods-resourcepacks/",
    "matmos/",
    "resource_assorts/",
    "resource_assorts.json",
    "patchouli_books/",
    "datapacks/",
    "kubejs*/",
    "!kubejs*/probe/",
    "!kubejs*/exported/",
    "!kubejs*/jsconfig.json",
    "!kubejs*/README.txt",
    "openloader/",
    "worldshape/",
    "resources/",
    "scripts/",
    "structures/",
    "fontfiles/",
    "oresources/",
    "packmenu/",
    "craftpresence/",
    "pointblanks/",
    "template*/",
    "!template*/playerdata/",
    "!template*/stats/",
];

const MOD_CONFIG: &[&str] = &[
    "config/",
    "!config/jei/world/",
    "!config/worldedit/",
    "config/worldedit/worldedit.properties",
    "!config/spark/",
    "config/spark/config.json",
    "defaultconfigs/",
    "journeymap/config/",
    "journeymap/server/",
    "TrashSlotSaveState.json",
    "customfov.txt",
    "gg.essential.mod/",
    "essential/",
    "!essential/*/",
    "!essential/*.jar*",
    "!essential/screenshot-checksum-caches.json",
    "!essential/microsoft_accounts.json",
    "paragliderSettings.nbt",
    "local/client_config.json",
    "local/ftbl.json",
    "local/client/sidebar_buttons.json",
    "local/client/ftbutilities.cfg",
    "local/client/ftblib.cfg",
    "local/client/xencraft.cfg",
    "liteloader.properties",
    "default_reference.xml",
    "CustomSkinLoader/CustomSkinLoader.json",
    "!config/tacz/custom",
];

const SPECS: &[Spec] = &[
    Spec {
        id: "basic",
        parent: None,
        title: "游戏本体",
        description: "",
        rules: &[],
        show_rules: &[],
        default_checked: true,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "options",
        parent: Some("basic"),
        title: "游戏本体设置",
        description: "键位、音量、视频设置等",
        rules: &["options.txt", "configureddefaults/"],
        show_rules: &[],
        default_checked: true,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "personal",
        parent: Some("basic"),
        title: "游戏本体个人信息",
        description: "命令历史、已保存的快捷栏",
        rules: &["hotbar.nbt", "command_history.txt"],
        show_rules: &[],
        default_checked: false,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "optifine_settings",
        parent: Some("basic"),
        title: "OptiFine 设置",
        description: "",
        rules: &["optionsof.txt", "optionsshaders.txt"],
        show_rules: &[],
        default_checked: true,
        require_mod_loader: false,
        require_optifine: true,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "mods",
        parent: None,
        title: "Mod",
        description: "模组",
        rules: &[
            "mods/",
            "!mods/*.disabled",
            "!mods/*.old",
            "!mods/.connector/",
            "coremods/",
            "lib/",
        ],
        show_rules: &[],
        default_checked: true,
        require_mod_loader: true,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "mods_disabled",
        parent: Some("mods"),
        title: "已禁用的 Mod",
        description: "",
        rules: &["mods/*.disabled", "mods/*.old"],
        show_rules: &[],
        default_checked: false,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "mods_data",
        parent: Some("mods"),
        title: "整合包重要数据",
        description: "脚本文件、内置资源包、数据包等",
        rules: MOD_PACK_DATA,
        show_rules: &[],
        default_checked: true,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "mods_config",
        parent: Some("mods"),
        title: "Mod 设置",
        description: "",
        rules: MOD_CONFIG,
        show_rules: &[],
        default_checked: true,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "tacz",
        parent: Some("mods"),
        title: "TaCZ 枪包",
        description: "",
        rules: &["tacz/", "config/tacz/custom/"],
        show_rules: &[],
        default_checked: true,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "paintings",
        parent: Some("mods"),
        title: "已上传的沉浸画",
        description: "",
        rules: &["immersive_paintings/"],
        show_rules: &[],
        default_checked: true,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "maps",
        parent: Some("mods"),
        title: "已绘制的地图",
        description: "地图类 Mod 为现有的存档、服务器记录的地图、路标点等",
        rules: &[
            "journeymap/data/",
            "xaero/",
            "XaeroWaypoints/",
            "XaeroWorldMap/",
        ],
        show_rules: &[],
        default_checked: false,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "jei",
        parent: Some("mods"),
        title: "JEI 个人信息",
        description: "物品收藏夹等",
        rules: &["config/jei/world/"],
        show_rules: &[],
        default_checked: false,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "emi",
        parent: Some("mods"),
        title: "EMI 个人信息",
        description: "物品收藏夹、默认配方、合成历史记录等",
        rules: &["emi.json"],
        show_rules: &[],
        default_checked: false,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "patchouli",
        parent: Some("mods"),
        title: "帕秋莉手册个人信息",
        description: "教程书的已读记录、书签、阅读历史记录等",
        rules: &["patchouli_data.json"],
        show_rules: &[],
        default_checked: false,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "resourcepacks",
        parent: None,
        title: "资源包",
        description: "纹理包/材质包",
        rules: &[],
        show_rules: &["resourcepacks/", "texturepacks/"],
        default_checked: true,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: Some("packs"),
    },
    Spec {
        id: "shaderpacks",
        parent: None,
        title: "光影包",
        description: "",
        rules: &[],
        show_rules: &["shaderpacks/"],
        default_checked: true,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: true,
        dynamic: Some("shaders"),
    },
    Spec {
        id: "screenshots",
        parent: None,
        title: "截图",
        description: "",
        rules: &["screenshots/"],
        show_rules: &[],
        default_checked: false,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "schematics",
        parent: None,
        title: "导出的结构",
        description: "schematics 文件夹",
        rules: &["schematics/"],
        show_rules: &[],
        default_checked: false,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "replays",
        parent: None,
        title: "录像回放",
        description: "Replay Mod 的录像文件",
        rules: &["replay_recordings/", "replay_videos/"],
        show_rules: &[],
        default_checked: false,
        require_mod_loader: true,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
    Spec {
        id: "saves",
        parent: None,
        title: "单人游戏存档",
        description: "世界/地图",
        rules: &[],
        show_rules: &["saves/"],
        default_checked: false,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: Some("saves"),
    },
    Spec {
        id: "servers",
        parent: None,
        title: "多人游戏服务器列表",
        description: "",
        rules: &["servers.dat"],
        show_rules: &[],
        default_checked: false,
        require_mod_loader: false,
        require_optifine: false,
        require_mod_loader_or_optifine: false,
        dynamic: None,
    },
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryView {
    pub label: String,
    pub hint: String,
    pub rule: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionView {
    pub id: String,
    pub parent: Option<String>,
    pub title: String,
    pub description: String,
    pub visible: bool,
    pub checked: bool,
    pub dynamic: bool,
    pub entries: Vec<EntryView>,
    pub rules: Vec<String>,
}

/// 选项表 + 每个选项该不该显示、默认勾不勾、动态子项（资源包/光影包/存档）。
pub fn layout(instance_dir: &Path, modable: bool, has_optifine: bool) -> Vec<OptionView> {
    let entries = collect_entries(instance_dir);
    let dynamic = dynamic_entries(instance_dir);
    let mut out = Vec::with_capacity(SPECS.len());
    for spec in SPECS {
        let visible = if spec.require_optifine && !has_optifine {
            false
        } else if spec.require_mod_loader && !modable {
            false
        } else if spec.require_mod_loader_or_optifine && !has_optifine && !modable {
            false
        } else if spec.rules.is_empty() && spec.show_rules.is_empty() {
            true
        } else {
            let probe = if spec.rules.is_empty() {
                spec.show_rules
            } else {
                spec.rules
            };
            has_match(instance_dir, &entries, probe)
        };
        let items = spec
            .dynamic
            .map(|kind| dynamic.get(kind).cloned().unwrap_or_default())
            .unwrap_or_default();
        let visible = visible && (spec.dynamic.is_none() || !items.is_empty());
        out.push(OptionView {
            id: spec.id.to_owned(),
            parent: spec.parent.map(str::to_owned),
            title: spec.title.to_owned(),
            description: spec.description.to_owned(),
            visible,
            checked: spec.default_checked && visible,
            dynamic: spec.dynamic.is_some(),
            entries: items,
            rules: spec.rules.iter().map(|rule| (*rule).to_owned()).collect(),
        });
    }
    out
}

/// 静态选项的规则（勾上就是这些行；空表示它只是分组标题或特殊项）。
pub fn rules_of(id: &str) -> &'static [&'static str] {
    SPECS
        .iter()
        .find(|spec| spec.id == id)
        .map(|spec| spec.rules)
        .unwrap_or(&[])
}

// ───────────────────────── 规则匹配 ─────────────────────────

/// 把 PCL 风格的 glob（`*` `?` `#` `[...]`）比成路径。
/// `*` 与 VB 的 `Like` 一样能跨分隔符，所以 `mods\*` 能吃掉 mods 下所有层级。
pub fn like_match(relative: &str, pattern: &str) -> bool {
    match_lower(&relative.to_lowercase(), &pattern.to_lowercase())
}

fn match_lower(text: &str, pattern: &str) -> bool {
    let bytes: Vec<char> = text.chars().collect();
    let pat: Vec<char> = pattern.chars().collect();
    wildcard(&bytes, 0, &pat, 0)
}

fn wildcard(text: &[char], mut ti: usize, pat: &[char], mut pi: usize) -> bool {
    while pi < pat.len() {
        match pat[pi] {
            '*' => {
                // 连续的 * 等于一个
                while pi < pat.len() && pat[pi] == '*' {
                    pi += 1;
                }
                if pi == pat.len() {
                    return true;
                }
                while ti <= text.len() {
                    if wildcard(text, ti, pat, pi) {
                        return true;
                    }
                    ti += 1;
                }
                return false;
            }
            '?' => {
                if ti >= text.len() {
                    return false;
                }
                ti += 1;
                pi += 1;
            }
            '#' => {
                if ti >= text.len() || !text[ti].is_ascii_digit() {
                    return false;
                }
                ti += 1;
                pi += 1;
            }
            '[' => {
                let mut end = pi + 1;
                while end < pat.len() && pat[end] != ']' {
                    end += 1;
                }
                if end == pat.len() {
                    return false;
                }
                if ti >= text.len() {
                    return false;
                }
                let mut negate = false;
                let mut start = pi + 1;
                if start < end && (pat[start] == '!' || pat[start] == '^') {
                    negate = true;
                    start += 1;
                }
                let mut hit = false;
                let mut k = start;
                while k < end {
                    if k + 2 < end && pat[k + 1] == '-' {
                        if text[ti] >= pat[k] && text[ti] <= pat[k + 2] {
                            hit = true;
                        }
                        k += 3;
                    } else {
                        if text[ti] == pat[k] {
                            hit = true;
                        }
                        k += 1;
                    }
                }
                if hit == negate {
                    return false;
                }
                ti += 1;
                pi = end + 1;
            }
            c => {
                if ti >= text.len() || text[ti] != c {
                    return false;
                }
                ti += 1;
                pi += 1;
            }
        }
    }
    ti == text.len()
}

/// 把文件名里的通配符转义掉（资源包、存档名字里可能有 `[` `]`）。
pub fn escape_like(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '[' | ']' | '?' | '*' | '#') {
            out.push('[');
            out.push(c);
            out.push(']');
        } else {
            out.push(c);
        }
    }
    out
}

/// 规则行标准化：去空白、`/`→`\`、文件夹规则补 `*`。
pub fn standardize(rules: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for line in rules {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('=') {
            continue;
        }
        let mut line = line.replace('/', "\\");
        if line.ends_with('\\') {
            line.push('*');
        }
        out.push(line);
    }
    out
}

/// 一组规则里最后命中的那条说了算。
fn keep(relative: &str, rules: &[String]) -> bool {
    let mut keep = false;
    for rule in rules {
        let (revert, pattern) = match rule.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, rule.as_str()),
        };
        if like_match(relative, pattern) {
            keep = !revert;
        }
    }
    keep
}

// ───────────────────────── 可见性探测 ─────────────────────────

fn collect_entries(instance_dir: &Path) -> Vec<String> {
    let mut all = Vec::new();
    let Ok(root) = fs::read_dir(instance_dir) else {
        return all;
    };
    for entry in root.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_blacklisted(&name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_file() {
            all.push(name);
            continue;
        }
        if !meta.is_dir() {
            continue;
        }
        all.push(format!("{name}\\"));
        let Ok(sub) = fs::read_dir(entry.path()) else {
            continue;
        };
        for item in sub.flatten() {
            let sub_name = item.file_name().to_string_lossy().into_owned();
            if is_blacklisted(&sub_name) {
                continue;
            }
            let Ok(sub_meta) = item.metadata() else { continue };
            if sub_meta.is_file() {
                all.push(format!("{name}\\{sub_name}"));
            } else if sub_meta.is_dir() {
                all.push(format!("{name}\\{sub_name}"));
            }
        }
    }
    all
}

fn is_blacklisted(name: &str) -> bool {
    BLACKLIST.iter().any(|bad| name.contains(bad))
}

/// 只要规则里有一条正向规则能命中，就算这个选项有东西可导。
fn has_match(instance_dir: &Path, entries: &[String], rules: &[&str]) -> bool {
    for rule in standardize(
        &rules
            .iter()
            .map(|item| item.to_string())
            .collect::<Vec<_>>(),
    ) {
        if rule.starts_with('!') {
            continue;
        }
        if entries.iter().any(|entry| like_match(entry, &rule)) {
            return true;
        }
        // 超过两级的路径直接查盘
        let plain = rule.trim_matches(|c| c == '*' || c == '?');
        if plain.matches('\\').count() >= 2 {
            let path = instance_dir.join(plain);
            if plain.ends_with('\\') {
                if fs::read_dir(&path).map(|mut d| d.next().is_some()).unwrap_or(false) {
                    return true;
                }
            } else if path.is_file() {
                return true;
            }
        }
    }
    false
}

fn dynamic_entries(instance_dir: &Path) -> HashMap<&'static str, Vec<EntryView>> {
    let mut out = HashMap::new();
    out.insert("packs", folder_entries(instance_dir, &["resourcepacks", "texturepacks"], true, true));
    out.insert("shaders", folder_entries(instance_dir, &["shaderpacks"], true, true));
    out.insert("saves", folder_entries(instance_dir, &["saves"], false, true));
    out
}

fn folder_entries(
    instance_dir: &Path,
    folders: &[&str],
    compressed: bool,
    folders_too: bool,
) -> Vec<EntryView> {
    let mut items = Vec::new();
    for folder in folders {
        let dir = instance_dir.join(folder);
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = read.flatten().map(|item| item.path()).collect();
        paths.sort_by_key(|path| {
            fs::metadata(path)
                .and_then(|meta| meta.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH)
        });
        paths.reverse();
        for path in paths {
            let Some(name) = path.file_name().map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            if is_blacklisted(&name) {
                continue;
            }
            let Ok(meta) = fs::metadata(&path) else { continue };
            if meta.is_file() {
                if !compressed {
                    continue;
                }
                let ext = path
                    .extension()
                    .map(|ext| ext.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if ext != "zip" && ext != "rar" {
                    continue;
                }
                items.push(EntryView {
                    label: name.clone(),
                    hint: String::new(),
                    rule: format!("{folder}/{}", escape_like(&name)),
                });
            } else if meta.is_dir() && folders_too {
                let empty = fs::read_dir(&path)
                    .map(|mut items| items.next().is_none())
                    .unwrap_or(true);
                if empty {
                    continue;
                }
                let hint = meta
                    .modified()
                    .ok()
                    .map(format_time)
                    .unwrap_or_default();
                items.push(EntryView {
                    label: name.clone(),
                    hint,
                    rule: format!("{folder}/{}/", escape_like(&name)),
                });
            }
        }
    }
    items
}

// ───────────────────────── 导出任务 ─────────────────────────

struct Candidate {
    path: PathBuf,
    rel: String,
    sha1: String,
    sha512: String,
    fingerprint: u32,
    size: u64,
}

struct Job {
    instance: PathBuf,
    overrides: PathBuf,
    temp: PathBuf,
    candidates: Vec<Candidate>,
    downloads: HashMap<String, Vec<String>>,
}

fn jobs() -> &'static Mutex<HashMap<u64, Job>> {
    static JOBS: OnceLock<Mutex<HashMap<u64, Job>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_token() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// 第一步：按规则把实例文件复制进临时目录，并给「可能会联网识别」的压缩包算哈希。
pub fn begin(
    instance_dir: &Path,
    rules: &[String],
    temp_root: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<u64> {
    if !instance_dir.is_dir() {
        return Err(CoreError::InvalidInput(format!(
            "实例目录不存在：{}",
            instance_dir.display()
        )));
    }
    let token = next_token();
    let temp = temp_root.join(format!("export-{token}"));
    let overrides = temp.join("modpack").join("overrides");
    fs::create_dir_all(&overrides).map_err(|error| io_error(&overrides, error))?;

    let mut merged = rules.to_vec();
    merged.extend(DEFAULT_EXCLUDE.iter().map(|rule| (*rule).to_owned()));
    let normalized = standardize(&merged);
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    let mut stack = vec![instance_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            if meta.is_dir() {
                if dir == instance_dir
                    && matches!(name.as_str(), "assets" | "versions" | "libraries")
                {
                    continue;
                }
                if matches!(
                    name.as_str(),
                    "structureCacheV1" | ".fabric" | ".git" | "avatar-cache" | "cosmetic-cache"
                ) {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(instance_dir) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('/', "\\");
            if !keep(&rel, &normalized) {
                continue;
            }
            files.push((path, rel));
        }
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    let total = files.len() as u64;
    on_progress(0, total);

    let mut candidates = Vec::new();
    let mut claims = 0u64;
    for (path, rel) in files {
        let target = overrides.join(rel.replace('\\', "/"));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        fs::copy(&path, &target).map_err(|error| io_error(&target, error))?;
        claims += 1;
        on_progress(claims, total);

        let lower = rel.to_lowercase();
        let ext = Path::new(&rel)
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let interesting = matches!(ext.as_str(), "zip" | "rar" | "jar" | "disabled" | "old")
            && ["mods", "packs", "openloader", "resource"]
                .iter()
                .any(|needle| lower.contains(needle));
        if !interesting {
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| io_error(&path, error))?;
        candidates.push(Candidate {
            path: target,
            rel: rel.replace('\\', "/"),
            sha1: sha1_of(&bytes),
            sha512: sha512_of(&bytes),
            fingerprint: curseforge_fingerprint(&bytes),
            size: bytes.len() as u64,
        });
    }

    jobs().lock().unwrap().insert(
        token,
        Job {
            instance: instance_dir.to_path_buf(),
            overrides,
            temp,
            candidates,
            downloads: HashMap::new(),
        },
    );
    Ok(token)
}

/// 第二步：联网把文件认成项目下载地址。返回 `(识别成功数, 候选数)`；
/// 网络失败时返回错误，调用方可以再以「不识别」的方式继续。
pub fn identify(token: u64, modrinth_only: bool) -> Result<(usize, usize)> {
    let (hashes, fingerprints) = {
        let map = jobs().lock().unwrap();
        let job = map
            .get(&token)
            .ok_or_else(|| CoreError::InvalidInput("导出任务已过期".into()))?;
        (
            job.candidates
                .iter()
                .map(|item| item.sha1.clone())
                .collect::<Vec<_>>(),
            job.candidates
                .iter()
                .map(|item| item.fingerprint)
                .collect::<Vec<_>>(),
        )
    };
    let mut resolved: HashMap<String, Vec<String>> = HashMap::new();
    let mut failure: Option<String> = None;
    match crate::modrinth::files_by_sha1(&hashes) {
        Ok(list) => {
            for (sha1, url) in list {
                resolved.entry(sha1).or_default().push(url);
            }
        }
        Err(error) => failure = Some(error.to_string()),
    }
    if !modrinth_only {
        match crate::curseforge::fingerprints(&fingerprints) {
            Ok(list) => {
                for (fingerprint, url) in list {
                    if let Some(item) = {
                        let map = jobs().lock().unwrap();
                        map.get(&token)
                            .and_then(|job| {
                                job.candidates
                                    .iter()
                                    .find(|c| c.fingerprint == fingerprint)
                            })
                            .map(|c| c.sha1.clone())
                    } {
                        let entry = resolved.entry(item).or_default();
                        if !entry.iter().any(|had| had == &url) {
                            entry.push(url);
                        }
                    }
                }
            }
            Err(error) => {
                if failure.is_none() {
                    failure = Some(error.to_string());
                }
            }
        }
    }
    if let Some(error) = failure {
        return Err(CoreError::InvalidInput(format!("联网识别失败：{error}")));
    }
    for urls in resolved.values_mut() {
        // 与 PCL 一致：Modrinth 的地址排在后面，优先用 CurseForge。
        urls.sort_by_key(|url| url.contains("modrinth.com"));
    }
    let mut map = jobs().lock().unwrap();
    let job = map
        .get_mut(&token)
        .ok_or_else(|| CoreError::InvalidInput("导出任务已过期".into()))?;
    job.downloads = resolved;
    let left = job
        .candidates
        .iter()
        .filter(|c| !job.downloads.contains_key(&c.sha1))
        .count();
    Ok((job.candidates.len() - left, job.candidates.len()))
}

fn downloads_of(job: &Job, candidate: &Candidate) -> Vec<String> {
    job.downloads
        .get(&candidate.sha1)
        .cloned()
        .unwrap_or_default()
}

/// 第三步：写 `modrinth.index.json`，把已识别的文件从 overrides 里删掉，再压缩。
pub fn finish(
    token: u64,
    out: &Path,
    name: &str,
    version: &str,
    summary: &str,
    dependencies: &[(String, String)],
    include_launcher: bool,
    launcher_exe: Option<&Path>,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<u64> {
    let job = {
        let mut map = jobs().lock().unwrap();
        map.remove(&token)
            .ok_or_else(|| CoreError::InvalidInput("导出任务已过期".into()))?
    };
    let mut files = Vec::new();
    let mut dropped = Vec::new();
    for candidate in &job.candidates {
        let downloads = downloads_of(&job, candidate);
        if downloads.is_empty() {
            continue;
        }
        dropped.push(candidate.path.clone());
        files.push(json!({
            "path": candidate.rel,
            "hashes": { "sha1": candidate.sha1, "sha512": candidate.sha512 },
            "downloads": downloads,
            "fileSize": candidate.size,
        }));
    }
    for path in dropped {
        let _ = fs::remove_file(path);
    }
    let mut deps = Map::new();
    for (key, value) in dependencies {
        deps.insert(key.clone(), Value::String(value.clone()));
    }
    let index = json!({
        "game": "minecraft",
        "formatVersion": 1,
        "versionId": version,
        "name": name,
        "summary": summary,
        "files": files,
        "dependencies": deps,
    });
    let pack_dir = job.temp.join("modpack");
    let index_path = pack_dir.join("modrinth.index.json");
    fs::write(
        &index_path,
        serde_json::to_string_pretty(&index).unwrap_or_default(),
    )
    .map_err(|error| io_error(&index_path, error))?;

    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    }
    let _ = fs::remove_file(out);
    let weight = if include_launcher { 0.4 } else { 1.0 };
    let mut written = 0u64;
    if include_launcher {
        let inner = job.temp.join("modpack.mrpack");
        written += zip_dir(&pack_dir, &inner, &mut on_progress, 0.0, weight)?;
        let exe = launcher_exe
            .ok_or_else(|| CoreError::InvalidInput("拿不到当前启动器的路径".into()))?;
        let file = fs::File::create(out).map_err(|error| io_error(out, error))?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        let entries: [(&Path, &str); 2] = [(inner.as_path(), "modpack.mrpack"), (exe, "YBUMCL.exe")];
        for (path, entry_name) in entries {
            zip.start_file(entry_name, options)
                .map_err(|error| CoreError::InvalidInput(format!("写入 {entry_name} 失败：{error}")))?;
            let bytes = fs::read(path).map_err(|error| io_error(path, error))?;
            zip.write_all(&bytes)
                .map_err(|error| CoreError::InvalidInput(format!("写入 {entry_name} 失败：{error}")))?;
            written += bytes.len() as u64;
            on_progress(written, written.max(1));
        }
        zip.finish()
            .map_err(|error| CoreError::InvalidInput(format!("压缩包收尾失败：{error}")))?;
        let _ = fs::remove_file(&inner);
    } else {
        written += zip_dir(&pack_dir, out, &mut on_progress, 0.0, weight)?;
    }
    let _ = fs::remove_dir_all(&job.temp);
    Ok(written)
}

pub fn cancel(token: u64) {
    if let Some(job) = jobs().lock().unwrap().remove(&token) {
        let _ = fs::remove_dir_all(&job.temp);
    }
}

fn zip_dir(
    dir: &Path,
    out: &Path,
    on_progress: &mut impl FnMut(u64, u64),
    from: f64,
    to: f64,
) -> Result<u64> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        let Ok(read) = fs::read_dir(&folder) else { continue };
        for entry in read.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    let total = files.len().max(1) as f64;
    let file = fs::File::create(out).map_err(|error| io_error(out, error))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    let mut written = 0u64;
    for (index, path) in files.iter().enumerate() {
        let Ok(rel) = path.strip_prefix(dir) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        zip.start_file(&rel, options)
            .map_err(|error| CoreError::InvalidInput(format!("写入 {rel} 失败：{error}")))?;
        let bytes = fs::read(path).map_err(|error| io_error(path, error))?;
        zip.write_all(&bytes)
            .map_err(|error| CoreError::InvalidInput(format!("写入 {rel} 失败：{error}")))?;
        written += bytes.len() as u64;
        let ratio = (index as f64 + 1.0) / total;
        on_progress(
            (from * 1000.0 + ratio * (to - from) * 1000.0) as u64,
            1000,
        );
    }
    zip.finish()
        .map_err(|error| CoreError::InvalidInput(format!("压缩包收尾失败：{error}")))?;
    Ok(written)
}

// ───────────────────────── 配置文件 ─────────────────────────

pub struct Config {
    pub name: String,
    pub version: String,
    pub include_launcher: bool,
    pub pack_resources: bool,
    pub modrinth_only: bool,
    pub pack_path: Option<String>,
    pub rules: Vec<String>,
    pub extra_files: Vec<String>,
}

pub fn config_text(config: &Config) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Name:{}", config.name));
    lines.push(format!("Version:{}", config.version));
    lines.push(String::new());
    lines.push("# 是否打包启动器，以便没有启动器的玩家安装整合包。".into());
    lines.push(format!("IncludeLauncher:{}", config.include_launcher));
    lines.push(String::new());
    lines.push("# 是否将 Mod、资源包、光影包的文件直接放入整合包中，这样在导入时就无需联网下载它们。".into());
    lines.push("# 建议仅在无法稳定连接 CurseForge 或 Modrinth 时才考虑启用。".into());
    lines.push(format!("DontCheckHostedAssets:{}", config.pack_resources));
    lines.push(String::new());
    lines.push("# 若资源未在 Modrinth 上托管，则将其原始文件直接放入整合包中。".into());
    lines.push("# 此选项与 DontCheckHostedAssets 冲突。".into());
    lines.push(format!("ModrinthUploadMode:{}", config.modrinth_only));
    lines.push(String::new());
    lines.push("# 导出的文件的存放位置。".into());
    lines.push("# 若 IncludeLauncher 为 True，应以 .zip 结尾；若为 False，应以 .mrpack 结尾。".into());
    lines.push(format!(
        "PackPath:{}",
        config.pack_path.clone().unwrap_or_default()
    ));
    lines.push(String::new());
    lines.push(SEPARATOR.into());
    lines.extend(config.rules.iter().cloned());
    lines.push(SEPARATOR.into());
    lines.extend(config.extra_files.iter().cloned());
    lines.join("\r\n")
}

pub fn parse_config(text: &str) -> Result<Config> {
    let mut segments = text.split(SEPARATOR);
    let ini_text = segments.next().unwrap_or_default();
    let mut ini: HashMap<String, String> = HashMap::new();
    for line in ini_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('=') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            ini.insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    let collect = |block: &str| -> Vec<String> {
        let mut lines: Vec<String> = block
            .lines()
            .map(|line| line.trim_end_matches(char::is_whitespace).to_owned())
            .collect();
        while lines.first().is_some_and(|line| line.trim().is_empty()) {
            lines.remove(0);
        }
        while lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }
        lines
    };
    let rules = segments.next().map(collect).unwrap_or_default();
    let extra_files = segments.next().map(collect).unwrap_or_default();
    Ok(Config {
        name: ini.get("Name").cloned().unwrap_or_default(),
        version: ini.get("Version").cloned().unwrap_or_default(),
        include_launcher: truthy(ini.get("IncludeLauncher")),
        pack_resources: truthy(ini.get("DontCheckHostedAssets")),
        modrinth_only: truthy(ini.get("ModrinthUploadMode")),
        pack_path: ini
            .get("PackPath")
            .filter(|value| !value.is_empty())
            .cloned(),
        rules,
        extra_files,
    })
}

fn truthy(value: Option<&String>) -> bool {
    value
        .map(|text| matches!(text.trim().to_lowercase().as_str(), "true" | "1" | "yes"))
        .unwrap_or(false)
}

// ───────────────────────── 哈希 ─────────────────────────

fn sha1_of(bytes: &[u8]) -> String {
    let mut hash = Sha1::new();
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}

fn sha512_of(bytes: &[u8]) -> String {
    let mut hash = Sha512::new();
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}

/// CurseForge 的文件指纹：murmur2（seed 1），先剔掉空白字节。
///
/// CurseForge 的算法不把制表符、换行、回车和空格算进去（PCL 的
/// `LocalResourceFile.CurseForgeHash` 也是这么做的）。少了这一步，
/// 带空白的 jar 算出来的指纹永远对不上。
pub fn curseforge_fingerprint(data: &[u8]) -> u32 {
    let stripped: Vec<u8> = data
        .iter()
        .copied()
        .filter(|byte| !matches!(byte, 9 | 10 | 13 | 32))
        .collect();
    murmur2(&stripped)
}

/// CurseForge 的文件指纹（murmur2，seed 1）。
pub fn murmur2(data: &[u8]) -> u32 {
    const SEED: u32 = 1;
    let mut hash = SEED ^ (data.len() as u32);
    let mut chunks = data.chunks_exact(4);
    for chunk in &mut chunks {
        let mut k = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        k = k.wrapping_mul(0x5bd1_e995);
        k ^= k >> 24;
        k = k.wrapping_mul(0x5bd1_e995);
        hash = hash.wrapping_mul(0x5bd1_e995);
        hash ^= k;
    }
    let rest = chunks.remainder();
    match rest.len() {
        3 => {
            hash ^= (rest[2] as u32) << 16;
            hash ^= (rest[1] as u32) << 8;
            hash ^= rest[0] as u32;
            hash = hash.wrapping_mul(0x5bd1_e995);
        }
        2 => {
            hash ^= (rest[1] as u32) << 8;
            hash ^= rest[0] as u32;
            hash = hash.wrapping_mul(0x5bd1_e995);
        }
        1 => {
            hash ^= rest[0] as u32;
            hash = hash.wrapping_mul(0x5bd1_e995);
        }
        _ => {}
    }
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(0x5bd1_e995);
    hash ^= hash >> 15;
    hash
}

/// `SystemTime` → `yyyy/MM/dd HH:mm`（存档子项的描述用）。
fn format_time(time: SystemTime) -> String {
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0);
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (mut year, mut month, mut day) = (1970, 1, 1);
    let mut left = days;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let len = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            _ => {
                if leap {
                    29
                } else {
                    28
                }
            }
        };
        if left < len {
            day += left;
            break;
        }
        left -= len;
        month += 1;
        if month > 12 {
            month = 1;
            year += 1;
        }
    }
    format!(
        "{year}/{month:02}/{day:02} {:02}:{:02}",
        rest / 3600,
        (rest % 3600) / 60
    )
}

fn io_error(path: &Path, error: std::io::Error) -> CoreError {
    CoreError::InvalidInput(format!("{}: {error}", path.display()))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matches_like_vb() {
        assert!(like_match("mods\\a.jar", "mods\\*"));
        assert!(like_match("mods\\sub\\a.jar", "mods\\*"));
        assert!(like_match("options.txt", "options.txt"));
        assert!(!like_match("mods\\a.jar.disable", "mods\\*.jar"));
        assert!(like_match("a1.txt", "a#.txt"));
        assert!(like_match("ab.txt", "a[bc].txt"));
        assert!(like_match("ad.txt", "a[!b].txt"));
        assert!(!like_match("ab.txt", "a[!b].txt"));
    }

    #[test]
    fn later_rules_win() {
        let rules = standardize(
            &["mods/", "!mods/*.disabled"]
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>(),
        );
        assert!(keep("mods\\a.jar", &rules));
        assert!(!keep("mods\\a.jar.disabled", &rules));
        let reordered = standardize(
            &["!mods/*.disabled", "mods/"]
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>(),
        );
        assert!(keep("mods\\a.jar.disabled", &reordered));
    }

    #[test]
    fn folder_rule_gets_star() {
        let rules = standardize(&["mods/".to_string()]);
        assert_eq!(rules, vec!["mods\\*".to_string()]);
    }

    #[test]
    fn murmur_matches_curseforge_vector() {
        // CurseForge 的指纹要剔掉空白：夹了制表/换行/回车的字节串应与纯字符串同值。
        let mut noisy: Vec<u8> = Vec::new();
        noisy.extend_from_slice(b"a b");
        noisy.push(10);
        noisy.extend_from_slice(b"c");
        noisy.push(13);
        noisy.push(9);
        noisy.extend_from_slice(b"d");
        assert_eq!(curseforge_fingerprint(b"abcd"), curseforge_fingerprint(&noisy));
        assert_ne!(curseforge_fingerprint(b"abcd"), curseforge_fingerprint(b"abce"));
        assert_ne!(murmur2(b"a"), murmur2(b"b"));
        assert_eq!(murmur2(b"a"), murmur2(b"a"));
        assert_ne!(murmur2(b""), 0);
    }

    #[test]
    fn escape_keeps_names_usable() {
        assert_eq!(escape_like("a[b].zip"), "a[[]b[]].zip");
    }

    #[test]
    fn config_round_trip() {
        let config = Config {
            name: "包".into(),
            version: "1.0.0".into(),
            include_launcher: true,
            pack_resources: false,
            modrinth_only: false,
            pack_path: Some("/tmp/a.zip".into()),
            rules: vec!["mods\\*".into(), "!*.log".into()],
            extra_files: vec![],
        };
        let text = config_text(&config);
        let back = parse_config(&text).unwrap();
        assert_eq!(back.name, "包");
        assert!(back.include_launcher);
        assert_eq!(back.rules.len(), 2);
    }

    #[test]
    fn layout_hides_loader_only_options_without_mods() {
        let dir = std::env::temp_dir().join("ybmcl-layout-test");
        let _ = fs::create_dir_all(&dir);
        let views = layout(&dir, false, false);
        assert!(views.iter().find(|v| v.id == "mods").unwrap().visible == false);
        assert!(views
            .iter()
            .find(|v| v.id == "optifine_settings")
            .unwrap()
            .visible
            == false);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn begin_finish_round_trip() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("ybmcl-export-{stamp}"));
        let instance = root.join("instance");
        let temp = root.join("temp");
        fs::create_dir_all(instance.join("mods")).unwrap();
        fs::create_dir_all(instance.join("config")).unwrap();
        fs::write(instance.join("options.txt"), b"fov:90").unwrap();
        fs::write(instance.join("mods").join("a.jar"), b"jar").unwrap();
        fs::write(instance.join("config").join("a.toml"), b"k=1").unwrap();
        fs::create_dir_all(&temp).unwrap();
        let rules = standardize(
            &["options.txt", "mods/", "config/"]
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>(),
        );
        let token = begin(&instance, &rules, &temp, |_, _| {}).unwrap();
        let out = root.join("pack.mrpack");
        finish(
            token,
            &out,
            "测试包",
            "1.0.0",
            "备注",
            &[("minecraft".into(), "1.20.1".into())],
            false,
            None,
            |_, _| {},
        )
        .unwrap();
        let bytes = fs::read(&out).unwrap();
        assert!(bytes.len() > 100);
        let _ = fs::remove_dir_all(&root);
    }
}
