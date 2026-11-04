//! MC 百科条目表：资源站的 slug → mcmod.cn 的 class id。
//!
//! 数据来自 PCL 随包发布的 `WikiEntries.txt`（28,880 行），由
//! `scripts/build-wiki-entries.py` 转成「slug → 条目 id」编译进二进制，运行时按 slug 查表、
//! 不联网。PCL 的做法就是这样：有条目才在详情页显示「转到 MC 百科」，没有就不显示
//! （见 `PageDownloadCompDetail.xaml.vb` 里 `BtnIntroWiki.Visibility = If(Project.WikiId = 0, ...)`）。

use std::collections::HashMap;
use std::io::Read;
use std::sync::OnceLock;

/// 编译进二进制的条目表（gzip 的 JSON：`{"c": {slug: id}, "m": {slug: id}}`）。
const TABLE: &[u8] = include_bytes!("../data/mcmod-wiki.json.gz");

#[derive(Debug, Default, serde::Deserialize)]
struct Tables {
    /// CurseForge slug → 条目 id。
    #[serde(default)]
    c: HashMap<String, u32>,
    /// Modrinth slug → 条目 id。
    #[serde(default)]
    m: HashMap<String, u32>,
}

fn tables() -> &'static Tables {
    static CACHE: OnceLock<Tables> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(TABLE)
            .read_to_end(&mut raw)
            .ok()
            .and_then(|_| serde_json::from_slice::<Tables>(&raw).ok())
            .unwrap_or_default()
    })
}

/// 按平台（`modrinth` / `curseforge`）与 slug 找 MC 百科条目 id。
///
/// 返回 `None` 表示 MC 百科上没有这个条目，界面就不要显示按钮。
pub fn wiki_id(source: &str, slug: &str) -> Option<u32> {
    let slug = slug.trim().to_ascii_lowercase();
    if slug.is_empty() {
        return None;
    }
    let tables = tables();
    let table = if source.eq_ignore_ascii_case("modrinth") {
        &tables.m
    } else {
        &tables.c
    };
    table.get(&slug).copied().filter(|id| *id > 0)
}

/// 详情页按钮要打开的地址。
pub fn wiki_url(source: &str, slug: &str) -> Option<String> {
    wiki_id(source, slug).map(|id| format!("https://www.mcmod.cn/class/{id}.html"))
}
