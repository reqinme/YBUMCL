//! 检查更新：读 GitHub 的最新 Release，和当前版本比一下。

use base64::Engine as _;
use serde::Serialize;
use std::time::Duration;

const RELEASES: &str = "https://api.github.com/repos/reqinme/YBUMCL/releases/latest";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    /// 最新版本号（去掉前缀 `v`）。
    pub latest: String,
    /// Release 页面地址。
    pub url: String,
    /// Release 正文，界面上可以截一段显示。
    pub notes: String,
    /// 当前版本比最新版本旧。
    pub newer: bool,
}

/// 读最新 Release；网络或解析失败时返回原因。
pub fn latest_release(current: &str) -> Result<ReleaseInfo, String> {
    let client = crate::proxy::apply(reqwest::blocking::Client::builder())
        .user_agent(crate::user_agent())
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("无法创建 HTTP 客户端：{error}"))?;
    let response = client
        .get(RELEASES)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|error| format!("连不上 GitHub：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("GitHub 返回 HTTP {}", response.status().as_u16()));
    }
    let value: serde_json::Value = response
        .json()
        .map_err(|error| format!("返回的不是 JSON：{error}"))?;
    let tag = value
        .get("tag_name")
        .and_then(|item| item.as_str())
        .unwrap_or_default();
    let latest = tag.trim_start_matches('v').trim().to_owned();
    if latest.is_empty() {
        return Err("Release 里没有版本号".into());
    }
    Ok(ReleaseInfo {
        newer: newer_than(current, &latest),
        latest,
        url: value
            .get("html_url")
            .and_then(|item| item.as_str())
            .unwrap_or("https://github.com/reqinme/YBUMCL/releases")
            .to_owned(),
        notes: value
            .get("body")
            .and_then(|item| item.as_str())
            .unwrap_or_default()
            .to_owned(),
    })
}

/// 逐段比较版本号：`0.2.6` < `0.3.0`，缺的段按 0 算。
fn newer_than(current: &str, latest: &str) -> bool {
    let parse = |text: &str| -> Vec<u64> {
        text.trim()
            .trim_start_matches('v')
            .split(['.', '-', '+'])
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let left = parse(current);
    let right = parse(latest);
    let len = left.len().max(right.len());
    for index in 0..len {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        if a != b {
            return b > a;
        }
    }
    false
}

/// 开发者头像：直接编译进程序，不联网。
/// 新贡献者头像放进 `assets/avatars/<login>.png`，加进下面的表即可。
const AVATARS: &[(&str, &[u8])] = &[(
    "reqinme",
    include_bytes!("../assets/avatars/reqinme.png"),
)];

/// 返回 `data:` URL；没有内置头像时给 `None`。
pub fn avatar_data(login: &str) -> Option<String> {
    let login = login.trim().to_ascii_lowercase();
    let bytes = AVATARS
        .iter()
        .find(|(name, _)| *name == login)
        .map(|(_, bytes)| *bytes)?;
    Some(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[cfg(test)]
mod tests {
    use super::newer_than;

    #[test]
    fn compares_by_segments() {
        assert!(newer_than("0.2.6", "0.2.7"));
        assert!(newer_than("0.2.9", "0.3"));
        assert!(!newer_than("0.3.0", "0.3"));
        assert!(!newer_than("1.0.0", "0.9.9"));
    }
}
