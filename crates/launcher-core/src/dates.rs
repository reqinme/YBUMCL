//! 构建（加载器版本）的发行时间。
//!
//! 只有少数加载器公开了「逐构建的发布日期」：
//! * Forge：`files.minecraftforge.net/.../index_<mc>.html` 里每个构建都带 `download-time`；
//! * Cleanroom：GitHub Releases 的 `published_at`。
//!
//! 其他加载器（NeoForge / Fabric / Quilt / Legacy Fabric / LiteLoader / OptiFine / LabyMod）
//! 的清单里根本没有日期，镜像上的 `Last-Modified` 又是缓存或动态生成的时间，拿来当发行日期
//! 是假的。所以这里只报真实日期，界面拿不到就不显示。
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Duration,
};

const FORGE_INDEX: &str = "https://files.minecraftforge.net/net/minecraftforge/forge/index";
const CLEANROOM_RELEASES: &str =
    "https://api.github.com/repos/CleanroomMC/Cleanroom/releases?per_page=100";

fn cache() -> &'static Mutex<HashMap<String, HashMap<String, String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, HashMap<String, String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn client() -> Option<&'static reqwest::blocking::Client> {
    static CLIENT: OnceLock<Option<reqwest::blocking::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .user_agent("YBUMCL/0.1")
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(30))
                .build()
                .ok()
        })
        .as_ref()
}

fn get(url: &str, accept: Option<&str>) -> Option<String> {
    let mut request = client()?.get(url);
    if let Some(accept) = accept {
        request = request.header(reqwest::header::ACCEPT, accept);
    }
    let response = request.send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.text().ok()
}

/// `2026-10-28T21:40:59Z` → `2026-10-28`
fn day_of(timestamp: &str) -> Option<String> {
    let head = timestamp.get(..10)?;
    let mut parts = head.split('-');
    let year = parts.next()?;
    let month = parts.next()?;
    let day = parts.next()?;
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    if parts.next().is_some() {
        return None;
    }
    if !year.chars().all(|c| c.is_ascii_digit())
        || !month.chars().all(|c| c.is_ascii_digit())
        || !day.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    Some(head.to_owned())
}

/// 从 Forge 的 index 页里摘 `download-version` / `download-time` 这一对单元格。
fn forge_rows(html: &str) -> HashMap<String, String> {
    let mut dates = HashMap::new();
    let mut rest = html;
    while let Some(start) = rest.find("download-version") {
        rest = &rest[start + "download-version".len()..];
        let Some(open) = rest.find('>') else { break };
        let after = &rest[open + 1..];
        let Some(close) = after.find('<') else { break };
        let version = after[..close].trim().to_owned();
        let tail = &after[close..];
        rest = tail;
        if version.is_empty() {
            continue;
        }
        let Some(time) = tail.find("download-time") else {
            continue;
        };
        let after_time = &tail[time + "download-time".len()..];
        let Some(open) = after_time.find('>') else {
            continue;
        };
        let text = &after_time[open + 1..];
        let Some(close) = text.find('<') else {
            continue;
        };
        if let Some(date) = day_of(text[..close].trim()) {
            dates.insert(version, date);
        }
    }
    dates
}

fn forge_dates(game: &str) -> HashMap<String, String> {
    let url = format!("{FORGE_INDEX}_{game}.html");
    get(&url, None)
        .map(|html| forge_rows(&html))
        .unwrap_or_default()
}

/// Cleanroom 的 tag 就是构建版本号（`0.7.14-alpha`）。
fn cleanroom_dates() -> HashMap<String, String> {
    let Some(body) = get(CLEANROOM_RELEASES, Some("application/vnd.github+json")) else {
        return HashMap::new();
    };
    let Ok(releases) = serde_json::from_str::<serde_json::Value>(&body) else {
        return HashMap::new();
    };
    let mut dates = HashMap::new();
    for release in releases.as_array().into_iter().flatten() {
        if release
            .get("draft")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let (Some(tag), Some(published)) = (
            release.get("tag_name").and_then(|value| value.as_str()),
            release.get("published_at").and_then(|value| value.as_str()),
        ) else {
            continue;
        };
        if let Some(date) = day_of(published) {
            dates.insert(tag.trim_start_matches('v').to_owned(), date);
        }
    }
    dates
}

fn lookup(loader: &str, game: &str) -> HashMap<String, String> {
    let key = format!("{loader}:{game}");
    if let Some(hit) = cache().lock().ok().and_then(|map| map.get(&key).cloned()) {
        return hit;
    }
    let found = match loader {
        "forge" => forge_dates(game),
        "cleanroom" => cleanroom_dates(),
        _ => HashMap::new(),
    };
    if let Ok(mut map) = cache().lock() {
        map.insert(key, found.clone());
    }
    found
}

/// 有真实发行日期的构建；没日期的加载器返回空表。
pub fn dates(loader: &str, game: &str, versions: &[String]) -> Vec<(String, String)> {
    let table = lookup(loader, game);
    if table.is_empty() {
        return Vec::new();
    }
    versions
        .iter()
        .filter_map(|version| {
            table
                .get(version)
                .map(|date| (version.clone(), date.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_become_day_precision() {
        assert_eq!(
            day_of("2026-10-28T21:40:59Z").as_deref(),
            Some("2026-10-28")
        );
        assert_eq!(day_of("2026-10-28"), Some("2026-10-28".to_owned()));
        assert_eq!(day_of("garbage"), None);
    }

    #[test]
    fn forge_rows_are_read_from_the_index_page() {
        let html = r#"
            <tr><td class="download-version"> 47.4.9 </td>
            <td class="download-time" title="2025-09-19 19:09:00">2025-09-19</td></tr>
            <tr><td class="download-version"> 47.4.8 </td>
            <td class="download-time">2025-09-01</td></tr>
        "#;
        let dates = forge_rows(html);
        assert_eq!(dates.get("47.4.9").map(String::as_str), Some("2025-09-19"));
        assert_eq!(dates.get("47.4.8").map(String::as_str), Some("2025-09-01"));
    }
}
