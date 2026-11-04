//! 中文搜索：把中文关键词换成英文再去请求资源站。
//!
//! 两个资源站的索引都是英文的，直接拿「钠」去搜是 0 结果。PCL 的做法是
//! 用本地那张「中文名 + 各站 slug + MC 百科浏览量」的表做模糊匹配，挑出最像的
//! 条目，再从它的英文名 / slug 里抽出词，拿这些词去搜。这里照同一套思路实现，
//! 数据来自 `data/mcmod-wiki.json.gz` 的 `e` 段（`scripts/build-wiki-entries.py` 生成）。

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// 判断一段文本里有没有汉字（与 PCL 的正则 `[\u4e00-\u9fbb]` 一致）。
pub fn has_chinese(text: &str) -> bool {
    text.chars().any(|c| ('\u{4e00}'..='\u{9fbb}').contains(&c))
}

/// 这次搜索最后要用的英文关键词。
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub keyword: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    CurseForge,
    Modrinth,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default, rename = "e")]
    entries: Vec<(Option<String>, Option<String>, String, u32)>,
}

/// 一条可搜索的条目。
struct Entry {
    slug: String,
    /// 中文名里括号前的部分（有的有 `/` 分隔的别名）。
    name: String,
    /// 中文名括号里的英文名（没有就是空）。
    english: String,
    popularity: u32,
}

/// 繁→简字表（OpenCC 的 TSCharacters，4113 条）。构建脚本用的是同一份文件，
/// 所以「表里存简体、查询也转简体」两边一定一致。
static T2S: OnceLock<HashMap<char, char>> = OnceLock::new();

fn t2s() -> &'static HashMap<char, char> {
    T2S.get_or_init(|| {
        let mut map = HashMap::new();
        for line in include_str!("../data/t2s.txt").lines() {
            let mut parts = line.split('\t');
            let (Some(trad), Some(simp)) = (parts.next(), parts.next()) else {
                continue;
            };
            if trad.chars().count() == 1 && simp.chars().count() == 1 {
                if let (Some(from), Some(to)) = (trad.chars().next(), simp.chars().next()) {
                    map.insert(from, to);
                }
            }
        }
        map
    })
}

/// 繁体转简体（只做字级：一个繁体字对应一个简体字）。
pub fn to_simplified(text: &str) -> String {
    let table = t2s();
    text.chars()
        .map(|c| table.get(&c).copied().unwrap_or(c))
        .collect()
}

static TABLE: OnceLock<Vec<(Option<String>, Option<String>, String, u32)>> = OnceLock::new();

fn table() -> &'static Vec<(Option<String>, Option<String>, String, u32)> {
    TABLE.get_or_init(|| {
        let raw = include_bytes!("../data/mcmod-wiki.json.gz");
        let mut decoder = flate2::read::GzDecoder::new(&raw[..]);
        let mut text = String::new();
        if std::io::Read::read_to_string(&mut decoder, &mut text).is_err() {
            return Vec::new();
        }
        match serde_json::from_str::<Raw>(&text) {
            Ok(parsed) => parsed.entries,
            Err(_) => Vec::new(),
        }
    })
}

/// 中文名拆成「括号前」和「括号里的英文名」。
fn split_name(name: &str) -> (String, String) {
    match name.find('(') {
        Some(at) => (
            name[..at].trim().to_owned(),
            name[at + 1..].trim_end_matches(')').trim().to_owned(),
        ),
        None => (name.trim().to_owned(), String::new()),
    }
}

fn entries(source: Source) -> Vec<Entry> {
    table()
        .iter()
        .filter_map(|(cf, mr, name, popularity)| {
            let slug = match source {
                Source::CurseForge => cf.clone(),
                Source::Modrinth => mr.clone(),
            }?;
            let (short, english) = split_name(name);
            Some(Entry {
                slug,
                name: short,
                english,
                popularity: *popularity,
            })
        })
        .collect()
}

/// PCL 的加权模糊匹配：按最长公共片段加分，再按查询长度与源长度折算。
fn similarity(source: &str, query: &str) -> f64 {
    if source.is_empty() || query.is_empty() {
        return 0.0;
    }
    let mut text: Vec<char> = source.to_lowercase().replace(' ', "").chars().collect();
    let needle: Vec<char> = query.to_lowercase().replace(' ', "").chars().collect();
    if needle.is_empty() {
        return 0.0;
    }
    let source_len = text.len() as f64;
    let query_len = needle.len();
    let mut qp = 0usize;
    let mut total = 0.0f64;
    while qp < query_len {
        let mut sp = 0usize;
        let mut best_len = 0usize;
        let mut best_at = 0usize;
        while sp < text.len() {
            let mut len = 0usize;
            while qp + len < query_len && sp + len < text.len() && text[sp + len] == needle[qp + len] {
                len += 1;
            }
            if len > best_len {
                best_len = len;
                best_at = sp;
            }
            sp += if len > 0 { len } else { 1 };
        }
        if best_len > 0 {
            text.drain(best_at..best_at + best_len);
            let mut weight = 1.4f64.powi(3 + best_len as i32) - 3.6;
            weight *= 1.0 + 0.3 * (3.0 - (qp as f64 - best_at as f64).abs()).max(0.0);
            total += weight;
        }
        qp += if best_len > 0 { best_len } else { 1 };
    }
    (total / query_len as f64) * (3.0 / (source_len + 15.0).sqrt())
        * if query_len <= 2 {
            (3 - query_len) as f64
        } else {
            1.0
        }
}

/// 一段文本源：若干别名共用同一个权重，取其中最高的相似度。
fn best_of(aliases: &[&str], query: &str) -> f64 {
    aliases
        .iter()
        .map(|alias| similarity(alias, query))
        .fold(0.0, f64::max)
}

fn weighted(entry: &Entry, query: &str) -> f64 {
    let short_aliases: Vec<&str> = if entry.name.is_empty() {
        Vec::new()
    } else {
        entry.name.split('/').map(str::trim).filter(|s| !s.is_empty()).collect()
    };
    let mut sum = 0.0;
    let mut weight = 0.0;
    if !short_aliases.is_empty() {
        sum += best_of(&short_aliases, query) * 1.0;
        weight += 1.0;
    }
    let tail = format!("{}{}", entry.english, entry.slug);
    sum += best_of(&[tail.as_str()], query) * 0.5;
    weight += 0.5;
    if weight == 0.0 {
        0.0
    } else {
        sum / weight
    }
}

/// 查询按空格切分后，每一段都出现在某个别名里，就算完全匹配。
fn exact(entry: &Entry, query: &str) -> bool {
    let mut aliases: Vec<String> = Vec::new();
    if !entry.name.is_empty() {
        aliases.extend(entry.name.split('/').map(|s| s.replace(' ', "")));
    }
    aliases.push(format!("{}{}", entry.english, entry.slug).replace(' ', ""));
    let parts: Vec<String> = query
        .split_whitespace()
        .map(|part| part.to_lowercase().replace(' ', ""))
        .collect();
    !parts.is_empty()
        && parts.iter().all(|part| {
            aliases
                .iter()
                .any(|alias| alias.to_lowercase().contains(part.as_str()))
        })
}

/// 相似度门槛。PCL 用的是 0.25，但中文一两个常用字就能糊到一个不相关的模组上
/// （「瞎编的模组名」会因为共用的「模组」二字命中两千多条），这里抬高一些。
const MIN_SIMILARITY: f64 = 0.6;

const STOP_WORDS: &[&str] = &[
    "the", "of", "mod", "and", "forge", "fabric", "for", "quilt", "neoforge",
];

/// 从条目里抽可用的英文单词（PCL 的 `ExtractWords`）：去噪、去数字、去非 ASCII，
/// 再把「能由其它词拼出来」的组合词剔掉（`enderio` 会被 `ender io` 取代）。
fn extract_words(entry: &Entry) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    let push = |text: &str, out: &mut Vec<String>| {
        for word in text.split_whitespace() {
            let word = word
                .trim_start_matches(['{', '[', '('])
                .trim_end_matches(['}', ']', ')'])
                .to_lowercase();
            if word.len() <= 1
                || STOP_WORDS.contains(&word.as_str())
                || !word.chars().all(|c| c.is_ascii())
                || word.chars().all(|c| c.is_ascii_digit())
                || word.parse::<f64>().is_ok()
            {
                continue;
            }
            if !out.contains(&word) {
                out.push(word);
            }
        }
    };
    push(&entry.slug.replace(['-', '/'], " "), &mut candidates);
    let tail = entry
        .english
        .replace(['-', '/', ':', '(', ')'], " ");
    push(&tail, &mut candidates);

    // 如果某个词能由其它更短的词拼出来，就把它去掉，只留下拆开的版本。
    let list = candidates.clone();
    candidates.retain(|word| {
        !list.iter().any(|shorter| {
            shorter.len() < word.len() && word.starts_with(shorter.as_str()) && can_form(&list, &word[shorter.len()..])
        })
    });
    candidates
}

fn can_form(words: &[String], rest: &str) -> bool {
    if rest.is_empty() {
        return true;
    }
    words.iter().any(|word| {
        rest.starts_with(word.as_str()) && can_form(words, &rest[word.len()..])
    })
}

/// 把中文关键词换成英文关键词；换不出来就返回 `None`（界面按「没有结果」处理）。
pub fn plan(query: &str, source: Source) -> Option<Plan> {
    // 用户可能是从台网搜来的繁体名，先转成简体再和表里的名字比对。
    let raw: String = to_simplified(query.trim()).to_lowercase();
    if raw.is_empty() || !has_chinese(&raw) {
        return None;
    }
    let entries = entries(source);
    if entries.is_empty() {
        return None;
    }

    // 挑出相似度够高的候选，完全匹配的排在前面。
    let mut candidates: Vec<(bool, f64, &Entry)> = entries
        .iter()
        .filter_map(|entry| {
            let is_exact = exact(entry, &raw);
            let score = weighted(entry, &raw);
            if is_exact || score >= MIN_SIMILARITY {
                Some((is_exact, score, entry))
            } else {
                None
            }
        })
        .collect();
    candidates.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    candidates.truncate(100);
    if candidates.is_empty() {
        return None;
    }

    let keyword = match source {
        // CurseForge 的多词是 AND，只能挑一个目标：完全匹配 → 相似度 → 人气。
        Source::CurseForge => {
            let pool: Vec<&(bool, f64, &Entry)> = if candidates.iter().any(|item| item.0) {
                candidates.iter().filter(|item| item.0).collect()
            } else {
                let best = candidates.first().map(|item| item.1).unwrap_or(0.0);
                candidates
                    .iter()
                    .filter(|item| (item.1 - best).abs() < 1e-9)
                    .collect()
            };
            let target = pool
                .iter()
                .max_by_key(|item| item.2.popularity)
                .map(|item| item.2)?;
            let words = extract_words(target);
            if words.is_empty() {
                None
            } else {
                Some(words.join(" "))
            }
        }
        // Modrinth 支持多词，把候选里的词按「相似度 × 人气」加权，取权重最高的那个。
        Source::Modrinth => {
            let mut weights: Vec<(String, f64)> = Vec::new();
            for (is_exact, score, entry) in &candidates {
                let similarity = if *is_exact { 1000.0 } else { *score };
                for word in extract_words(entry) {
                    let add = similarity * entry.popularity as f64;
                    match weights.iter_mut().find(|item| item.0 == word) {
                        Some(slot) => slot.1 += add,
                        None => weights.push((word, add)),
                    }
                }
            }
            weights
                .into_iter()
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|item| item.0)
        }
    }?;

    Some(Plan { keyword })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_chinese_text() {
        assert!(has_chinese("钠"));
        assert!(has_chinese("机械动力 create"));
        assert!(!has_chinese("sodium"));
    }

    #[test]
    fn turns_chinese_names_into_english_keywords() {
        let sodium = plan("钠", Source::Modrinth).expect("钠 应该能换出 sodium");
        assert_eq!(sodium.keyword, "sodium");
        let create = plan("机械动力", Source::CurseForge).expect("机械动力 应该能换出 create");
        assert!(create.keyword.contains("create"), "{}", create.keyword);
    }

    #[test]
    fn converts_traditional_characters_to_simplified() {
        assert_eq!(to_simplified("鈉"), "钠");
        assert_eq!(to_simplified("機械動力"), "机械动力");
        assert_eq!(to_simplified("sodium"), "sodium");
    }

    #[test]
    fn accepts_traditional_queries() {
        let sodium = plan("鈉", Source::Modrinth).expect("鈉 应该能换出 sodium");
        assert_eq!(sodium.keyword, "sodium");
        let create = plan("機械動力", Source::CurseForge).expect("機械動力 应该能换出 create");
        assert!(create.keyword.contains("create"), "{}", create.keyword);
    }

    #[test]
    fn gives_up_on_unknown_chinese_names() {
        assert!(plan("完全不存在的模组名", Source::Modrinth).is_none());
        // 「瞎编的模组名」会因为共用的「模组」二字糊到不相关项目上，靠门槛挡住
        assert!(plan("瞎编的模组名", Source::Modrinth).is_none(), "{:?}", plan("瞎编的模组名", Source::Modrinth));
        assert!(plan("sodium", Source::Modrinth).is_none());
    }
}
