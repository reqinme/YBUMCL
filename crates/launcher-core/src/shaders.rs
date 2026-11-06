//! 光影包的配置项：读 `shaders/shaders.properties`（含 `#include` 链）出选项表，
//! 读 `shaderpacks/<包名>.txt`（Iris 与 OptiFine 共用的 Properties 文件）回填当前值，
//! 存盘时按同样的 `key=value` 写回。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::CoreError;

/// 一个选项里的一个取值。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderValue {
    /// 写进配置文件的值（OptiFine/Iris 的字符串选项就是这么存的）。
    pub id: String,
    /// 界面上显示的文字：优先取语言文件，其次取 `option.<名>.<值>=<数字> <文字>` 里的文字。
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderOption {
    pub name: String,
    pub label: String,
    pub comment: Option<String>,
    pub values: Vec<ShaderValue>,
    /// 配置文件里的当前值；配置文件没写就是 `default`。
    pub current: String,
    pub default: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderScreen {
    pub name: String,
    pub label: String,
    pub columns: u32,
    pub options: Vec<ShaderOption>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderConfig {
    pub pack: String,
    /// 这个光影的 `shaders/shaders.properties`；为空表示光影里没有配置文件。
    pub source: Option<String>,
    /// 读写当前值的文件：`shaderpacks/<包名>.txt`。
    pub values_file: String,
    pub screens: Vec<ShaderScreen>,
}

/// `shaderpacks` 目录下这个包的入口：文件夹或 zip。
fn pack_root(root: &Path, pack: &str) -> Option<PathBuf> {
    let direct = root.join(pack);
    if direct.is_dir() {
        return Some(direct);
    }
    if direct.is_file() {
        return Some(direct);
    }
    let zip = root.join(format!("{pack}.zip"));
    if zip.is_file() {
        return Some(zip);
    }
    None
}

/// 把包里的一个文件读成字符串；`rel` 相对包根（形如 `shaders/shaders.properties`）。
fn read_pack_file(root: &Path, rel: &str) -> Option<String> {
    if root.is_dir() {
        let path = root.join(rel.replace('/', "\\"));
        let bytes = std::fs::read(path).ok()?;
        return String::from_utf8(bytes).ok();
    }

    let file = std::fs::File::open(root).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let name = rel.replace('\\', "/");
    let mut entry = archive.by_name(&name).ok()?;
    let mut text = String::new();
    entry.read_to_string(&mut text).ok()?;
    Some(text)
}

/// 包里的文件清单（只用于找出 `shaders/lang/*.lang`）。
fn list_pack_files(root: &Path, prefix: &str) -> Vec<String> {
    if root.is_dir() {
        let dir = root.join(prefix.replace('/', "\\"));
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(name) = path.file_name().and_then(|v| v.to_str()) {
                        out.push(format!("{prefix}/{name}"));
                    }
                }
            }
        }
        return out;
    }

    let mut out = Vec::new();
    if let Ok(file) = std::fs::File::open(root) {
        if let Ok(mut archive) = zip::ZipArchive::new(file) {
            let needle = format!("{prefix}/");
            for index in 0..archive.len() {
                let Ok(entry) = archive.by_index_raw(index) else {
                    continue;
                };
                let name = entry.name().to_string();
                if let Some(rest) = name.strip_prefix(&needle) {
                    if !rest.contains('/') && !rest.is_empty() {
                        out.push(name);
                    }
                }
            }
        }
    }
    out
}

/// 去掉 Minecraft 的颜色代码（`§a`、`§l` 这种），界面显示不需要。
fn strip_color(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '§' {
            chars.next();
            continue;
        }
        out.push(ch);
    }
    out.trim().to_string()
}

/// 解析 Properties 文本：`key=value`，`#`/`!` 开头是注释。
fn parse_properties(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        out.push((key.trim().to_string(), value.trim().to_string()));
    }
    out
}

/// 顺着 `#include <文件>` 把一个光影的配置摊平；同一文件只读一次。
fn collect_properties(root: &Path, rel: &str, out: &mut Vec<(String, String)>, seen: &mut HashSet<String>) {
    if !seen.insert(rel.to_string()) {
        return;
    }
    let Some(text) = read_pack_file(root, rel) else {
        return;
    };
    let dir = rel.rsplit_once('/').map(|(head, _)| head.to_string()).unwrap_or_default();
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("#include") {
            let name = rest.trim();
            if name.is_empty() {
                continue;
            }
            let next = if name.contains('/') {
                name.to_string()
            } else if dir.is_empty() {
                name.to_string()
            } else {
                format!("{dir}/{name}")
            };
            collect_properties(root, &next, out, seen);
        }
    }
    out.extend(parse_properties(&text));
}

/// 语言文件的键：`option.<名>`、`value.<名>.<值>`、`screen.<名>`。
fn load_lang(root: &Path) -> HashMap<String, String> {
    let files = list_pack_files(root, "shaders/lang");
    let mut names = BTreeMap::new();
    for file in files {
        let Some(name) = file.rsplit('/').next() else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".lang") {
            continue;
        }
        let code = lower.trim_end_matches(".lang").to_string();
        names.insert(code, file);
    }

    let mut map = HashMap::new();
    let ordered = [names.get("zh_cn"), names.get("zh_tw"), names.get("en_us")]
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    for file in ordered {
        if let Some(text) = read_pack_file(root, &file) {
            for (key, value) in parse_properties(&text) {
                map.entry(key).or_insert(value);
            }
        }
    }
    map
}

/// 读 `<shaderpacks>/<包名>.txt`，只认 `key=value`（Properties 的文件头注释会被忽略）。
fn read_values(root: &Path, pack: &str) -> HashMap<String, String> {
    let path = root.join(format!("{pack}.txt"));
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    parse_properties(&text).into_iter().collect()
}

/// 保存：把用户改过的值写成 Properties（其它键原样保留）。
fn write_values(root: &Path, pack: &str, values: &HashMap<String, String>) -> Result<(), CoreError> {
    let path = root.join(format!("{pack}.txt"));
    if values.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    let mut text = String::from("# Configuration for the shader pack, written by YBUMCL\n");
    let mut keys = values.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    for key in keys {
        if let Some(value) = values.get(&key) {
            text.push_str(&format!("{key}={value}\n"));
        }
    }
    std::fs::write(&path, text).map_err(|source| CoreError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(())
}

/// 从着色器源码里找选项声明：OptiFine/Iris 的约定是
/// `#define 名字 值 //[可选值…]` 或 `const 类型 名字 = 值; //[可选值…]`。
#[derive(Debug, Clone)]
struct SourceOption {
    value: String,
    values: Vec<String>,
    comment: Option<String>,
}

/// 列举 `shaders/` 下的源码文件（相对包根）。
fn list_shader_sources(root: &Path) -> Vec<String> {
    const EXT: [&str; 6] = ["vsh", "fsh", "gsh", "glsl", "comp", "csh"];
    let mut out = Vec::new();
    if root.is_dir() {
        let mut stack = vec![root.join("shaders")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path
                    .extension()
                    .and_then(|v| v.to_str())
                    .is_some_and(|ext| EXT.contains(&ext))
                {
                    if let Ok(rel) = path.strip_prefix(root) {
                        out.push(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }
        return out;
    }

    if let Ok(file) = std::fs::File::open(root) {
        if let Ok(mut archive) = zip::ZipArchive::new(file) {
            for index in 0..archive.len() {
                let Ok(entry) = archive.by_index_raw(index) else {
                    continue;
                };
                let name = entry.name().to_string();
                if !name.starts_with("shaders/") || entry.is_dir() {
                    continue;
                }
                if name
                    .rsplit_once('.')
                    .is_some_and(|(_, ext)| EXT.contains(&ext))
                {
                    out.push(name);
                }
            }
        }
    }
    out
}

/// 解析一行声明，返回（选项名, 值, 可选值, 注释）。
fn parse_declaration(line: &str) -> Option<(String, String, Vec<String>, Option<String>)> {
    let text = line.trim();
    let (head, tail) = text.split_once("//").map_or((text, ""), |(a, b)| (a.trim_end(), b.trim()));
    let mut words = head.split_whitespace();
    let first = words.next()?;

    let (name, value) = if first == "#define" {
        let name = words.next()?.to_string();
        let value = words.next().unwrap_or("").to_string();
        (name, value)
    } else if first == "const" {
        let _kind = words.next()?;
        let name = words.next()?.to_string();
        let rest = words.collect::<Vec<_>>().join(" ");
        let value = rest
            .trim_start_matches('=')
            .trim()
            .trim_end_matches(';')
            .trim()
            .to_string();
        (name, value)
    } else {
        return None;
    };

    let mut values = Vec::new();
    let mut comment = None;
    if let Some(rest) = tail.strip_prefix('[') {
        if let Some((list, _)) = rest.split_once(']') {
            values = list.split_whitespace().map(|v| v.to_string()).collect();
        }
    } else if !tail.is_empty() {
        comment = Some(tail.to_string());
    }

    Some((name, value, values, comment))
}

fn scan_sources(root: &Path) -> BTreeMap<String, SourceOption> {
    let mut out: BTreeMap<String, SourceOption> = BTreeMap::new();
    for rel in list_shader_sources(root) {
        let Some(text) = read_pack_file(root, &rel) else {
            continue;
        };
        for line in text.lines() {
            let Some((name, value, values, comment)) = parse_declaration(line) else {
                continue;
            };
            // 同一个名字只保留第一处（后面多半是 #undef / 重新 define）。
            out.entry(name).or_insert(SourceOption {
                value,
                values,
                comment,
            });
        }
    }
    out
}

/// 读一个光影的完整配置（选项表 + 当前值）。
pub fn load(save_root: &Path, pack: &str) -> Result<ShaderConfig, CoreError> {
    let Some(root) = pack_root(save_root, pack) else {
        return Err(CoreError::InvalidInput(format!("找不到光影包：{pack}")));
    };

    let mut properties = Vec::new();
    let mut seen = HashSet::new();
    collect_properties(&root, "shaders/shaders.properties", &mut properties, &mut seen);

    let lang = load_lang(&root);
    let current = read_values(save_root, pack);

    // 先把 `option.<名>.xxx` 聚到一起，再按 screen 分组。
    let mut types = HashMap::new();
    let mut defaults = HashMap::new();
    let mut values_ids: HashMap<String, Vec<String>> = HashMap::new();
    let mut numeric_labels: HashMap<(String, String), String> = HashMap::new();
    let mut comments = HashMap::new();

    for (key, value) in &properties {
        if let Some(rest) = key.strip_prefix("option.") {
            let Some((name, field)) = rest.split_once('.') else {
                continue;
            };
            match field {
                "type" => {
                    types.insert(name.to_string(), value.clone());
                }
                "default" => {
                    defaults.insert(name.to_string(), value.clone());
                }
                "values" => {
                    values_ids.insert(
                        name.to_string(),
                        value.split_whitespace().map(|v| v.to_string()).collect(),
                    );
                }
                "comment" => {
                    comments.insert(name.to_string(), value.clone());
                }
                other => {
                    // `option.<名>.value.<值>=<文字>`，文字前面可能带一个数字，例如 `0 Off`。
                    if let Some(id) = other.strip_prefix("value.") {
                        let text = value
                            .split_once(' ')
                            .map(|(_, tail)| tail.trim().to_string())
                            .unwrap_or_else(|| value.clone());
                        numeric_labels.insert((name.to_string(), id.to_string()), text);
                    }
                }
            }
        }
    }

    let sliders = properties
        .iter()
        .filter(|(key, _)| key == "sliders")
        .flat_map(|(_, value)| value.split_whitespace().map(|v| v.to_string()))
        .collect::<HashSet<_>>();

    let sources = scan_sources(&root);

    // 选项表：声明了 type 的都算；profile 单独一类，这里不做编辑。
    let mut options: BTreeMap<String, ShaderOption> = BTreeMap::new();
    for (name, kind) in &types {
        if kind == "profile" {
            continue;
        }
        let ids = values_ids.get(name).cloned().unwrap_or_default();
        let values = if ids.is_empty() {
            // 没有 values 的字符串选项：通常是布尔开关（Iris 里存 true/false）。
            vec![
                ShaderValue { id: "false".into(), label: lang.get(&format!("value.{name}.false")).cloned().unwrap_or_else(|| "关".into()) },
                ShaderValue { id: "true".into(), label: lang.get(&format!("value.{name}.true")).cloned().unwrap_or_else(|| "开".into()) },
            ]
        } else {
            ids.iter()
                .map(|id| ShaderValue {
                    id: id.clone(),
                    label: strip_color(
                        &lang
                            .get(&format!("value.{name}.{id}"))
                            .cloned()
                            .or_else(|| numeric_labels.get(&(name.clone(), id.clone())).cloned())
                            .unwrap_or_else(|| id.clone()),
                    ),
                })
                .collect()
        };
        let default = defaults.get(name).cloned().unwrap_or_else(|| {
            values.first().map(|v| v.id.clone()).unwrap_or_else(|| "false".into())
        });
        let mut current_value = current.get(name).cloned().unwrap_or_else(|| default.clone());
        let mut values = values;
        if !current_value.is_empty() && !values.iter().any(|v| v.id == current_value) {
            values.push(ShaderValue { id: current_value.clone(), label: current_value.clone() });
        }
        if current_value.is_empty() {
            current_value = values.first().map(|v| v.id.clone()).unwrap_or_default();
        }
        options.insert(
            name.clone(),
            ShaderOption {
                name: name.clone(),
                label: strip_color(
                    &lang.get(&format!("option.{name}")).cloned().unwrap_or_else(|| name.clone()),
                ),
                comment: comments.get(name).cloned().or_else(|| lang.get(&format!("option.{name}.comment")).cloned()),
                values,
                current: current_value,
                default,
            },
        );
        let _ = sliders.contains(name);
    }

    for (name, source) in &sources {
        if types.contains_key(name) || name.starts_with("info") {
            continue;
        }
        let ids = if !source.values.is_empty() {
            source.values.clone()
        } else if matches!(source.value.as_str(), "0" | "1" | "true" | "false") {
            vec!["false".to_string(), "true".to_string()]
        } else {
            vec![source.value.clone()]
        };
        if ids.iter().all(|id| id.trim().is_empty()) {
            continue;
        }
        let values = ids
            .iter()
            .map(|id| ShaderValue {
                id: id.clone(),
                label: strip_color(
                    &lang
                        .get(&format!("value.{name}.{id}"))
                        .cloned()
                        .unwrap_or_else(|| id.clone()),
                ),
            })
            .collect::<Vec<_>>();
        let default = source.value.clone();
        let mut current_value = current.get(name).cloned().unwrap_or_else(|| default.clone());
        let mut values = values;
        if !current_value.is_empty() && !values.iter().any(|v| v.id == current_value) {
            values.push(ShaderValue { id: current_value.clone(), label: current_value.clone() });
        }
        if current_value.is_empty() {
            current_value = values.first().map(|v| v.id.clone()).unwrap_or_default();
        }
        options.insert(
            name.clone(),
            ShaderOption {
                name: name.clone(),
                label: strip_color(
                    &lang
                        .get(&format!("option.{name}"))
                        .cloned()
                        .unwrap_or_else(|| name.clone()),
                ),
                comment: source
                    .comment
                    .clone()
                    .or_else(|| lang.get(&format!("option.{name}.comment")).cloned()),
                values,
                current: current_value,
                default,
            },
        );
        let _ = sliders.contains(name);
    }

    // screen 分组：`screen=<名>` 列出子页面；`screen.<名>=<选项…>` 是页面内容；`*` 表示其余全部。
    let mut screen_names: Vec<String> = Vec::new();
    let mut screen_items: HashMap<String, Vec<String>> = HashMap::new();
    let mut screen_columns: HashMap<String, u32> = HashMap::new();
    for (key, value) in &properties {
        if key == "screen" {
            for name in value.split_whitespace() {
                let name = name.trim_matches(|c| c == '[' || c == ']' || c == '<' || c == '>');
                if name.is_empty() || name == "*" || name == "empty" || name == "profile" {
                    continue;
                }
                if (screen_items.contains_key(name) || name.chars().all(|c| c.is_ascii_uppercase() || c == '_'))
                    && !screen_names.contains(&name.to_string())
                {
                    screen_names.push(name.to_string());
                }
            }
        } else if let Some(name) = key.strip_prefix("screen.") {
            if let Some((sub, field)) = name.split_once('.') {
                if field == "columns" {
                    screen_columns.insert(sub.to_string(), value.parse().unwrap_or(2));
                    continue;
                }
            }
            let items = screen_items.entry(name.to_string()).or_default();
            for item in value.split_whitespace() {
                let item = item.trim_matches(|c| c == '[' || c == ']' || c == '<' || c == '>');
                if item.is_empty()
                    || item == "*"
                    || item == "empty"
                    || item == "profile"
                    || item == "columns"
                {
                    continue;
                }
                items.push(item.to_string());
            }
        }
    }

    let mut used = HashSet::new();
    let mut screens = Vec::new();
    for name in &screen_names {
        let items = screen_items.get(name).cloned().unwrap_or_default();
        let mut list = Vec::new();
        for item in items {
            if let Some(option) = options.get(&item) {
                used.insert(item.clone());
                list.push(option.clone());
            }
        }
        if list.is_empty() {
            continue;
        }
        screens.push(ShaderScreen {
            name: name.clone(),
            label: strip_color(
                &lang.get(&format!("screen.{name}")).cloned().unwrap_or_else(|| name.clone()),
            ),
            columns: screen_columns.get(name).copied().unwrap_or(2).clamp(1, 4),
            options: list,
        });
    }

    // 没被任何 screen 收进去的选项放在「其它」。
    let rest = options
        .into_iter()
        .filter(|(name, _)| !used.contains(name))
        .map(|(_, option)| option)
        .collect::<Vec<_>>();
    if !rest.is_empty() {
        screens.push(ShaderScreen {
            name: "_other".into(),
            label: "其它选项".into(),
            columns: 2,
            options: rest,
        });
    }

    Ok(ShaderConfig {
        pack: pack.to_string(),
        source: if properties.is_empty() {
            None
        } else {
            Some("shaders/shaders.properties".into())
        },
        values_file: format!("shaderpacks/{pack}.txt"),
        screens,
    })
}

/// 保存用户改过的值：只覆盖传进来的键，其余保留。
pub fn save(save_root: &Path, pack: &str, changed: HashMap<String, String>) -> Result<(), CoreError> {
    let mut values = read_values(save_root, pack);
    for (key, value) in changed {
        values.insert(key, value);
    }
    write_values(save_root, pack, &values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_options_and_screens() {
        let dir = std::env::temp_dir().join("ybumcl-shader-test");
        let shaders = dir.join("pack").join("shaders");
        std::fs::create_dir_all(&shaders).unwrap();
        std::fs::write(
            shaders.join("shaders.properties"),
            "screen=WATER\nscreen.WATER=SUN_PATH\noption.SUN_PATH.type=value\noption.SUN_PATH.values=0 1\noption.SUN_PATH.value.0=Off\noption.SUN_PATH.value.1=On\noption.SUN_PATH.default=1\n",
        )
        .unwrap();

        let config = load(&dir, "pack").unwrap();
        assert_eq!(config.screens.len(), 1);
        let option = &config.screens[0].options[0];
        assert_eq!(option.name, "SUN_PATH");
        assert_eq!(option.current, "1");
        assert_eq!(option.values.len(), 2);
        assert_eq!(option.values[0].label, "Off");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn writes_and_reads_values() {
        let dir = std::env::temp_dir().join("ybumcl-shader-test2");
        std::fs::create_dir_all(&dir).unwrap();
        let mut changed = HashMap::new();
        changed.insert("SUN_PATH".to_string(), "0".to_string());
        save(&dir, "pack", changed).unwrap();
        let values = read_values(&dir, "pack");
        assert_eq!(values.get("SUN_PATH").map(String::as_str), Some("0"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
