//! 启动前算出这个版本到底要什么 Java。
//!
//! 版本 json 自带的 `javaVersion.majorVersion` 只是下限的一部分：老 Forge 有 Java 上限
//! （1.6–1.7.2 必须 7、1.12 及以前最高 8），LiteLoader 不能跑在 8 以上，OptiFine 在
//! 1.8–1.11 必须恰好 8，Cleanroom 0.5 以后要 25。只看 json 会挑到能起但会崩的 Java，
//! 所以这里按「基线 → json 声明 → 加载器收窄 → compatibleJavaMajors」叠出一个区间。

use serde::Serialize;
use serde_json::Value;
use std::path::Path;

const OPEN_MAX: u32 = 99;

/// 一个版本能接受的 Java 主版本区间，闭区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaRange {
    pub min: u32,
    pub max: u32,
}

impl JavaRange {
    const fn all() -> Self {
        Self {
            min: 7,
            max: OPEN_MAX,
        }
    }

    fn at_least(&mut self, major: u32) {
        self.min = self.min.max(major);
    }

    fn at_most(&mut self, major: u32) {
        self.max = self.max.min(major);
    }

    fn exactly(&mut self, major: u32) {
        self.min = major;
        self.max = major;
    }

    pub fn contains(&self, major: u32) -> bool {
        self.min <= major && major <= self.max
    }
}

/// 最终需求：区间，或者 json 明确列出的主版本集合（后者优先）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaRequirement {
    pub range: JavaRange,
    pub majors: Option<Vec<u32>>,
    /// 版本 json 声明的运行时组件（`java-runtime-epsilon` 这类）。挑本机 Java
    /// 用不上它，但「自动」要下载 Java 时它就是唯一正确的目标。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    /// 软上限：高于它的 Java 仍能选，但排序时排在满足的后面。
    ///
    /// 这些是「推荐上限」而不是硬规则（现代 Forge 遇到太新的 JDK 常报
    /// `Unsupported JVM module path`），硬写成排除会把唯一能用的 Java 判死。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_max: Option<u32>,
}

impl JavaRequirement {
    /// 高出软上限就记一次违规（不排除，只降权）。
    pub fn soft_penalty(&self, major: u32) -> u32 {
        match self.soft_max {
            Some(cap) if major > cap => 1,
            _ => 0,
        }
    }

    /// 满足区间时返回「比下限高出几档主版本」，越小越贴近版本真正需要的那一档。
    pub fn slack(&self, major: u32) -> Option<u32> {
        if !self.accepts(major) {
            return None;
        }
        Some(major.saturating_sub(self.range.min))
    }

    pub fn accepts(&self, major: u32) -> bool {
        match &self.majors {
            Some(majors) => majors.contains(&major),
            None => self.range.contains(major),
        }
    }

    /// 给用户看的「需要什么」。
    pub fn describe(&self) -> String {
        match &self.majors {
            Some(majors) => {
                let list: Vec<String> = majors.iter().map(u32::to_string).collect();
                format!("Java {}", list.join(" / "))
            }
            None if self.range.max >= OPEN_MAX => format!("Java {}+", self.range.min),
            None if self.range.min == self.range.max => format!("Java {}", self.range.min),
            None => format!("Java {}–{}", self.range.min, self.range.max),
        }
    }

    /// 找不到合适 Java 时的中文说明，带下载地址。
    pub fn missing_message(&self, installed: &[u32]) -> String {
        let mut have: Vec<u32> = installed.to_vec();
        have.sort_unstable();
        have.dedup();
        let current = if have.is_empty() {
            "没有检测到任何 Java".to_string()
        } else {
            let list: Vec<String> = have.iter().map(|major| format!("Java {major}")).collect();
            format!("当前检测到 {}", list.join("、"))
        };
        format!(
            "这个版本需要 {}，{current}。请先安装对应版本的 Java：https://adoptium.net/temurin/releases/?version={}",
            self.describe(),
            self.range.min
        )
    }
}

/// MC 版本号拆成数字段，`1.12.2` → `[1, 12, 2]`，`26.3` → `[26, 3]`。
fn numeric(version: &str) -> Vec<u32> {
    version
        .split(['.', '-', '+', ' '])
        .take_while(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// 原版基线：只看 MC 版本本身。
fn base_range(game: &str) -> JavaRange {
    let mut range = JavaRange::all();
    let parts = numeric(game);
    let Some(&first) = parts.first() else {
        return range;
    };
    if first >= 2 {
        // 26.1 起换成年份版本号，最低 Java 25（PCL 的分段表同此）。
        range.at_least(25);
        return range;
    }
    let minor = parts.get(1).copied().unwrap_or(0);
    let patch = parts.get(2).copied().unwrap_or(0);
    if minor >= 21 || (minor == 20 && patch >= 5) {
        range.at_least(21);
    } else if minor >= 18 {
        range.at_least(17);
    } else if minor == 17 {
        range.at_least(16);
    } else if minor >= 12 {
        range.at_least(8);
    } else if minor <= 5 {
        range.at_most(8);
    }
    range
}

/// 版本 json 声明的下限。
///
/// 只有 `majorVersion >= 22` 才当硬下限——PCL 同样只在这个区间信任 json
/// （见 `pcl_ModLaunch.vb`：`JavaVersion.Major >= 22` 时才 `MinVersion` 跟随）。
/// 1.13–1.21 的 json 常写着 8，照着抬会把「1.7.10/1.12.2 用 Java 7 也行」判死。
fn declared_json(value: &Value, range: &mut JavaRange) -> Option<String> {
    if let Some(major) = value
        .get("javaVersion")
        .and_then(|java| java.get("majorVersion"))
        .and_then(Value::as_u64)
        .and_then(|major| u32::try_from(major).ok())
    {
        if major >= 22 {
            range.at_least(major);
            return value
                .get("javaVersion")
                .and_then(|java| java.get("component"))
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
    }
    None
}

fn forge_range(game: &str, forge: &str, range: &mut JavaRange, soft_max: &mut Option<u32>) {
    let parts = numeric(game);
    let minor = parts.get(1).copied().unwrap_or(0);
    // 1.7.10 的 Forge 既能跑 Java 7 也能跑 Java 8（安装器自己写的
    // `javaVersion` 通常就是 8），只有 1.6.4 及更早才锁死 7。
    if minor == 7 {
        range.at_least(7);
        range.at_most(8);
        return;
    }
    if minor == 6 {
        range.exactly(7);
        return;
    }
    if minor <= 12 {
        range.at_most(8);
        return;
    }
    match minor {
        13 | 14 => {
            range.at_least(8);
            range.at_most(10);
        }
        15 => range.at_most(15),
        16 => {
            let build = numeric(forge).get(1).copied().unwrap_or(0);
            if (34..=36).contains(&build) {
                if build <= 36 {
                    // 34.0.0–36.2.25 里有一批在 Java 9+ 上直接崩，这条保留硬上限。
                    range.at_most(8);
                }
            }
        }
        _ => {}
    }
    // 现代 Forge 带 OptiFine 或 45.0.66–47.4.8：官方文档建议不超过 21，但 22–25
    // 通常也能起，只降权不排除。
    if minor >= 18 && forge.starts_with("4") {
        let build = numeric(forge).first().copied().unwrap_or(0);
        if (45..=47).contains(&build) {
            soften(soft_max, 21);
        }
    }
    if minor >= 18 {
        soften(soft_max, 23);
    }
}

/// 记录一个软上限，多条规则取最严格的那条。
fn soften(soft_max: &mut Option<u32>, cap: u32) {
    *soft_max = Some(soft_max.map_or(cap, |value| value.min(cap)));
}

fn optifine_range(game: &str, range: &mut JavaRange, soft_max: &mut Option<u32>) {
    let minor = numeric(game).get(1).copied().unwrap_or(0);
    if minor < 7 {
        range.at_most(8);
    } else if (8..=11).contains(&minor) {
        range.exactly(8);
    } else if minor == 12 {
        range.at_most(8);
    } else if minor == 18 {
        soften(soft_max, 18);
    }
}

fn fabric_range(game: &str, range: &mut JavaRange) {
    let minor = numeric(game).get(1).copied().unwrap_or(0);
    if (15..=16).contains(&minor) {
        range.at_least(8);
    } else if minor >= 18 {
        range.at_least(17);
    }
}

fn cleanroom_range(version: &str, range: &mut JavaRange) {
    if numeric(version).get(1).copied().unwrap_or(0) < 5 {
        range.at_least(21);
    } else {
        range.at_least(25);
    }
}

/// 版本 json 里能看出装了哪些加载器。
#[derive(Debug, Default)]
pub struct LoaderPresence<'a> {
    pub forge: Option<&'a str>,
    pub neoforge: Option<&'a str>,
    pub cleanroom: Option<&'a str>,
    pub fabric: bool,
    pub liteloader: bool,
    pub optifine: bool,
}

impl<'a> LoaderPresence<'a> {
    pub fn detect(value: &'a Value) -> Self {
        let mut presence = Self::default();
        let main = value
            .get("mainClass")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        if main.contains("fabric") || main.contains("quilt") {
            presence.fabric = true;
        }
        for library in value
            .get("libraries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = library.get("name").and_then(Value::as_str).unwrap_or("");
            let lower = name.to_ascii_lowercase();
            if lower.contains("optifine") {
                presence.optifine = true;
            }
            if lower.contains("liteloader") || lower.contains("launchwrapper") {
                presence.liteloader = true;
            }
            if lower.contains("cleanroom") {
                presence
                    .cleanroom
                    .get_or_insert_with(|| maven_version(name));
            }
            if lower.contains("net.minecraftforge:forge:") {
                presence.forge.get_or_insert_with(|| maven_version(name));
            }
            if lower.starts_with("net.neoforged:") {
                presence
                    .neoforge
                    .get_or_insert_with(|| maven_version(name));
            }
        }
        presence
    }
}

fn maven_version(name: &str) -> &str {
    name.split(':').nth(2).unwrap_or("")
}

/// 从已合并的版本 json 算出完整需求。`game` 是它所基于的原版版本号。
pub fn requirement(game: &str, value: &Value) -> JavaRequirement {
    requirement_with(game, Some(value), &LoaderPresence::detect(value))
}

/// 装加载器之前用：原版 json 还没带上加载器的库，所以加载器信息由调用方直接给。
pub fn requirement_with(
    game: &str,
    value: Option<&Value>,
    loaders: &LoaderPresence<'_>,
) -> JavaRequirement {
    let mut range = base_range(game);
    let mut component = None;
    let mut soft_max = None;
    if let Some(value) = value {
        component = declared_json(value, &mut range);
    }
    if let Some(forge) = loaders.forge {
        forge_range(game, forge, &mut range, &mut soft_max);
    }
    if let Some(neoforge) = loaders.neoforge {
        // 1.20.1 的 NeoForge 官方建议 ≤21（22+ 的模块检查改动会炸），但 22 也能起。
        if game.starts_with("1.20.1") || numeric(neoforge).get(1).copied().unwrap_or(0) <= 20 {
            soften(&mut soft_max, 21);
        }
    }
    if loaders.optifine {
        optifine_range(game, &mut range, &mut soft_max);
    }
    if loaders.fabric {
        fabric_range(game, &mut range);
    }
    if loaders.liteloader {
        range.at_most(8);
    }
    if let Some(cleanroom) = loaders.cleanroom {
        cleanroom_range(cleanroom, &mut range);
    }

    // 硬区间被软规则挤空时（例如老版本 json 写着 8、加载器建议 ≤7），以硬区间为准。
    let majors = value
        .and_then(|value| value.get("compatibleJavaMajors"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_u64)
                .filter_map(|item| u32::try_from(item).ok())
                .filter(|major| *major > 0)
                .collect::<Vec<_>>()
        })
        .filter(|majors| !majors.is_empty());

    if let Some(majors) = &majors {
        // 集合说了算：旧版本的上限规则可能把它整个挤空，这时以集合为准。
        if !majors.iter().any(|major| range.contains(*major)) {
            range.min = majors.iter().copied().min().unwrap_or(range.min);
            range.max = majors.iter().copied().max().unwrap_or(range.max);
        }
    }
    if range.max < range.min {
        range.max = OPEN_MAX;
    }
    let soft_max = soft_max.filter(|cap| *cap >= range.min);
    JavaRequirement {
        range,
        majors,
        component,
        soft_max,
    }
}

/// 读游戏目录里某个版本的 json（含继承合并）判断它要什么 Java。
pub fn requirement_for_dir(game_dir: &Path, version_id: &str) -> Option<JavaRequirement> {
    // 游戏目录可能是指到别处的 junction/符号链接，而 inside() 是拿 canonical
    // 路径跟 root 比；不先 canonicalize 会一律判越界，这里返回 None，调用方
    // 退回「任意 Java」，自动挑 Java 就会挑到 7 这种明显不对的版本。
    let root = std::fs::canonicalize(game_dir)
        .map(crate::plain)
        .unwrap_or_else(|_| crate::plain(game_dir.to_path_buf()));
    let resolved = crate::inheritance::resolve_version(&root, version_id).ok()?;
    // 合并后的 json 已经丢了 inheritsFrom，链条最底层那个 id 就是原版版本号。
    Some(requirement(&resolved.client_id, &resolved.metadata))
}
