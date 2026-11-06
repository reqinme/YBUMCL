//! 版本元信息：这个实例是基于哪个游戏版本、属于正式版/快照/愚人节、装了哪些加载器。
//!
//! 我们的实例 json 是合并后的自包含文件：`type` 统一写成 release、没有
//! `inheritsFrom`，所以判断不出来源版本。能用的信号有三个，按可靠程度排：
//!
//! 1. 实例 json 里的 `_ybmcl.gameVersion`（以后新装的实例会写，见 install 流程）；
//! 2. 实例名：`1.21.11-Fabric_0.20.1` 这种前缀就是基础版本；
//! 3. 客户端 jar 指纹：合并后的 json 保留了基础版本的 `downloads.client.sha1`，
//!    和本机纯版本名实例（`1.21.10`、`26.3` …）对比就能认出来。整合包就靠这个。
//!
//! 都认不出来就老实给 `None`，前端只在「全部」里显示它。

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

/// 愚人节版本的 id（Mojang 那些一次性发布会混进快照里）。
const APRIL_FOOLS: &[&str] = &[
    "15w14a",
    "1.RV-Pre1",
    "3D Shareware v1.34",
    "20w14infinite",
    "22w13oneBlockAtATime",
    "23w13a_or_b",
    "24w14potato",
    "25w14craftmine",
];

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionMeta {
    /// 基础游戏版本（`1.21.10`、`26.3` …）；认不出来是 None。
    pub game_version: Option<String>,
    /// `release` / `snapshot` / `april` / `unknown`。
    pub channel: String,
    /// 加载器显示名，可多个（Forge + OptiFine 这种）。
    pub loaders: Vec<String>,
}

fn is_numbered(id: &str) -> bool {
    let mut parts = id.split('.');
    let first = parts.next().unwrap_or_default();
    if first.is_empty() || !first.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let rest: Vec<&str> = parts.collect();
    (1..=2).contains(&rest.len())
        && rest
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// 形如 `26w01a` 的快照 id。
fn is_snapshot_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.len() != 6 {
        return false;
    }
    bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && (bytes[2] == b'w' || bytes[2] == b'W')
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
        && bytes[5].is_ascii_lowercase()
}

fn channel_of(id: &str) -> &'static str {
    if APRIL_FOOLS.iter().any(|april| april.eq_ignore_ascii_case(id)) {
        return "april";
    }
    if is_numbered(id) {
        return "release";
    }
    let lower = id.to_ascii_lowercase();
    if is_snapshot_id(id) || lower.contains("-pre") || lower.contains("-rc") {
        return "snapshot";
    }
    "unknown"
}

/// 从实例名取基础版本：`1.21.11-Fabric_0.20.1` → `1.21.11`，`26.3` → `26.3`。
fn version_from_name(id: &str) -> Option<String> {
    if is_numbered(id) || is_snapshot_id(id) {
        return Some(id.to_owned());
    }
    let cut = id.find(['-', '_'])?;
    let prefix = &id[..cut];
    (is_numbered(prefix) || is_snapshot_id(prefix)).then(|| prefix.to_owned())
}

/// 从 mainClass 与 libraries 认加载器。
fn loaders_of(main_class: &str, libraries: &[String]) -> Vec<String> {
    let main = main_class.to_ascii_lowercase();
    let joined = libraries.join("
").to_ascii_lowercase();
    let fabric = main.contains("fabric") || joined.contains("net.fabricmc");
    let quilt = main.contains("quilt") || joined.contains("org.quiltmc");
    let neoforge = joined.contains("net.neoforged") || joined.contains("neoforge");
    let forge = !neoforge
        && (joined.contains("net.minecraftforge") || main.contains("modlauncher"));
    let optifine = joined.contains("optifine") || main.contains("optifine");
    let liteloader = joined.contains("liteloader");
    let labymod = joined.contains("labymod");

    let mut out: Vec<&str> = Vec::new();
    for (name, found) in [
        ("Fabric", fabric),
        ("Quilt", quilt),
        ("NeoForge", neoforge),
        ("Forge", forge),
        ("OptiFine", optifine),
        ("LiteLoader", liteloader),
        ("Labymod", labymod),
    ] {
        if found {
            out.push(name);
        }
    }
    if out.is_empty() {
        // 没有加载器痕迹 = 原版。
        out.push("原版");
    }
    out.into_iter().map(str::to_owned).collect()
}

/// 合并后的 json 里拿不到加载器痕迹时，退回实例名：我们自己装的 OptiFine 实例
/// 就叫 `1.20.1-Forge_47.4.28-OptiFine_HD_U_I6`，json 里一行痕迹都没有。
fn name_loaders(id: &str) -> Vec<&'static str> {
    let lower = id.to_ascii_lowercase();
    let mut out = Vec::new();
    if lower.contains("optifine") {
        out.push("OptiFine");
    }
    out
}

/// 一个实例 json 里摘出来的原始信号。
struct Probe {
    main_class: String,
    libraries: Vec<String>,
    client_sha1: String,
    recorded_version: Option<String>,
}

fn probe(instance_dir: &Path, id: &str) -> Option<Probe> {
    let text = std::fs::read_to_string(instance_dir.join(id).join(format!("{id}.json"))).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let main_class = json
        .get("mainClass")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_owned();
    let libraries = json
        .get("libraries")
        .and_then(|value| value.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|item| item.get("name").and_then(|name| name.as_str()))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let client_sha1 = json
        .get("downloads")
        .and_then(|value| value.get("client"))
        .and_then(|value| value.get("sha1"))
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_owned();
    let recorded_version = json
        .get("_ybmcl")
        .and_then(|value| value.get("gameVersion"))
        .and_then(|value| value.as_str())
        // 整合包导入的实例没有 _ybmcl，但原版客户端版本写在 clientVersion 里。
        .or_else(|| {
            json.get("clientVersion")
                .and_then(|value| value.as_str())
        })
        .map(str::to_owned);
    Some(Probe {
        main_class,
        libraries,
        client_sha1,
        recorded_version,
    })
}

/// 一次扫描里给所有实例打标签：先收「客户端 jar 指纹 → 基础版本」的对照表，
/// 再逐个认。`ids` 是同一个目录里的全部实例 id。
pub fn describe_all(game_dir: &Path, ids: &[String]) -> BTreeMap<String, VersionMeta> {
    let root = game_dir.join("versions");
    let mut probes: BTreeMap<String, Probe> = BTreeMap::new();
    for id in ids {
        if let Some(found) = probe(&root, id) {
            probes.insert(id.clone(), found);
        }
    }
    // 指纹表只收「名字本身就是基础版本」的实例，免得互相污染。
    let mut by_client: BTreeMap<&str, &str> = BTreeMap::new();
    for (id, found) in &probes {
        if found.client_sha1.is_empty() {
            continue;
        }
        if is_numbered(id) || is_snapshot_id(id) {
            by_client.insert(found.client_sha1.as_str(), id.as_str());
        }
    }

    let mut out = BTreeMap::new();
    for id in ids {
        let Some(found) = probes.get(id) else {
            out.insert(
                id.clone(),
                VersionMeta {
                    game_version: version_from_name(id),
                    channel: "unknown".into(),
                    loaders: vec!["原版".into()],
                },
            );
            continue;
        };
        let game_version = found
            .recorded_version
            .clone()
            .or_else(|| version_from_name(id))
            .or_else(|| {
                by_client
                    .get(found.client_sha1.as_str())
                    .map(|base| (*base).to_owned())
            });
        let channel = game_version
            .as_deref()
            .map(channel_of)
            .unwrap_or("unknown")
            .to_owned();
        let mut loaders = loaders_of(&found.main_class, &found.libraries);
        for extra in name_loaders(id) {
            if !loaders.iter().any(|name| name == extra) {
                if loaders.len() == 1 && loaders[0] == "原版" {
                    loaders.clear();
                }
                loaders.push(extra.to_owned());
            }
        }
        out.insert(
            id.clone(),
            VersionMeta {
                game_version,
                channel,
                loaders,
            },
        );
    }
    out
}


#[cfg(test)]
mod tests {
    use super::{channel_of, is_numbered, is_snapshot_id, loaders_of, version_from_name};

    #[test]
    fn channel_matches_mojang_style_ids() {
        assert!(is_numbered("1.21.11"));
        assert!(is_numbered("26.3"));
        assert!(!is_numbered("1.20.1-Forge_47.4.28"));
        assert!(is_snapshot_id("26w01a"));
        assert_eq!(channel_of("1.21.11"), "release");
        assert_eq!(channel_of("26.3"), "release");
        assert_eq!(channel_of("26w01a"), "snapshot");
        assert_eq!(channel_of("25w14craftmine"), "april");
        assert_eq!(channel_of("Fabulously Optimized"), "unknown");
    }

    #[test]
    fn version_comes_from_name_prefix() {
        assert_eq!(version_from_name("1.21.11-Fabric_0.20.1").as_deref(), Some("1.21.11"));
        assert_eq!(version_from_name("1.20.1-Forge_47.4.28-OptiFine_HD_U_I6").as_deref(), Some("1.20.1"));
        assert_eq!(version_from_name("26.3").as_deref(), Some("26.3"));
        assert_eq!(version_from_name("Fabulously Optimized"), None);
    }

    #[test]
    fn loaders_are_recognised_from_libraries() {
        let forge = vec!["net.minecraftforge:forge:1.20.1-47.4.28".to_string()];
        assert_eq!(loaders_of("cpw.mods.bootstraplauncher.BootstrapLauncher", &forge), vec!["Forge"]);
        let optifine = vec![
            "net.minecraftforge:forge:1.20.1-47.4.28".to_string(),
            "optifine:OptiFine:1.20.1_HD_U_I6".to_string(),
        ];
        assert_eq!(loaders_of("x", &optifine), vec!["Forge", "OptiFine"]);
        assert_eq!(loaders_of("net.minecraft.client.main.Main", &[]), vec!["原版"]);
    }
}
