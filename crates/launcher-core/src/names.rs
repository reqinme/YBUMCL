//! 社区资源的中文名与下载文件名。
//!
//! 中文名表取自 PCL 随包发布的那份社区数据（`Resources/mcmod.buf`），转换后只保留
//! 「slug → 中文名」（11,603 个 slug，189 KB gzip），编译进二进制；运行时按 slug 查表，
//! 不联网。没有译名的项目就是空，文件名回退成原始文件名——与 PCL 的行为一致。

use std::collections::HashMap;
use std::io::Read;
use std::sync::OnceLock;

/// 编译进二进制的中文名表（gzip 的 JSON 对象）。
const TABLE: &[u8] = include_bytes!("../data/mcmod-names.json.gz");

fn table() -> &'static HashMap<String, String> {
    static CACHE: OnceLock<HashMap<String, String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut raw = Vec::new();
        let parsed = flate2::read::GzDecoder::new(TABLE)
            .read_to_end(&mut raw)
            .ok()
            .and_then(|_| serde_json::from_slice::<HashMap<String, String>>(&raw).ok());
        parsed.unwrap_or_default()
    })
}

/// 项目 slug（Modrinth 或 CurseForge 的都认，大小写不敏感）对应的中文名。
pub fn chinese_name(slug: &str) -> Option<&'static str> {
    table()
        .get(&slug.trim().to_ascii_lowercase())
        .map(String::as_str)
}

/// 文件名里不能出现的字符换成全角，其余控制字符与空白收尾去掉。
fn sanitize(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|ch| match ch {
            '<' => '＜',
            '>' => '＞',
            ':' => '：',
            '"' => '＂',
            '/' => '／',
            '\\' => '＼',
            '|' => '｜',
            '?' => '？',
            '*' => '＊',
            ch if (ch as u32) < 0x20 => ' ',
            ch => ch,
        })
        .collect();
    out = out.trim().to_string();
    if out.is_empty() {
        out.push_str("download");
    }
    out
}

fn split_name(raw: &str) -> (String, String) {
    match raw.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (raw.to_string(), String::new()),
    }
}

/// 按 PCL 的「文件名格式」拼出下载时的默认文件名。
///
/// `name` 是项目名（中文名优先，其次原始标题）；为空时无论 `format` 是哪一档都退回原始文件名。
/// 0..=4 与 PCL 的 `NameFormatV2` 一一对应，其余值按 4 处理。
pub fn content_file_name(name: Option<&str>, raw_name: &str, format: u8) -> String {
    let raw = raw_name.trim();
    let (stem, ext) = split_name(raw);
    let fallback = if raw.is_empty() {
        "download".to_string()
    } else {
        raw.to_string()
    };
    let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
        return sanitize(&fallback);
    };
    let composed = match format {
        0 => format!("【{name}】{fallback}"),
        1 => format!("[{name}] {fallback}"),
        2 => format!("{name}-{fallback}"),
        3 => {
            if stem.is_empty() {
                format!("{name}{ext}")
            } else {
                format!("{stem}-{name}{ext}")
            }
        }
        _ => fallback.clone(),
    };
    sanitize(&composed).replace('~', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = "sodium-fabric-0.9.3-beta.1+mc26.3.jar";

    #[test]
    fn pcl_formats_are_reproduced() {
        assert_eq!(
            content_file_name(Some("钠"), RAW, 0),
            format!("【钠】{RAW}")
        );
        assert_eq!(content_file_name(Some("钠"), RAW, 1), format!("[钠] {RAW}"));
        assert_eq!(content_file_name(Some("钠"), RAW, 2), format!("钠-{RAW}"));
        assert_eq!(
            content_file_name(Some("钠"), RAW, 3),
            "sodium-fabric-0.9.3-beta.1+mc26.3-钠.jar"
        );
        assert_eq!(content_file_name(Some("钠"), RAW, 4), RAW);
    }

    #[test]
    fn missing_name_falls_back_to_the_original_file() {
        assert_eq!(content_file_name(None, RAW, 0), RAW);
        assert_eq!(content_file_name(Some("  "), RAW, 2), RAW);
        assert_eq!(content_file_name(None, "", 1), "download");
    }

    #[test]
    fn illegal_characters_are_made_safe() {
        let name = content_file_name(Some("A:B/C?D"), "x.jar", 2);
        assert!(!name.contains(':') && !name.contains('/') && !name.contains('?'));
    }

    #[test]
    fn the_bundled_table_knows_common_mods() {
        assert_eq!(chinese_name("sodium"), Some("钠"));
        assert_eq!(chinese_name("Create"), Some("机械动力"));
        assert_eq!(chinese_name("no-such-mod-slug-anywhere"), None);
    }
}
