//! 崩溃日志的规则引擎：规则表 + 匹配。
//!
//! 规则放在 `crash/rules.json`，编译期嵌进二进制；引擎本身只有几十行，
//! 之后加规则不用改代码。匹配是「不区分大小写的子串」，`any` 命中任意一条、
//! `all` 必须全部命中，两者同时存在时都要满足。
//!
//! 和 `crash.rs` 的区别：那边是从一份崩溃报告原文里认少数几类；
//! 这边扫的是「整段运行日志」，按规则表给出可能的原因，用于运行结束后的提示。

use serde::Deserialize;
use std::sync::OnceLock;

const RULES: &str = include_str!("../crash/rules.json");

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub id: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_severity")]
    pub severity: String,
    pub title: String,
    pub detail: String,
    #[serde(default)]
    pub fix: String,
    #[serde(default)]
    pub any: Vec<String>,
    #[serde(default)]
    pub all: Vec<String>,
}

fn default_severity() -> String {
    "error".into()
}

#[derive(Deserialize)]
struct RulesFile {
    #[serde(default)]
    rules: Vec<Rule>,
}

fn rules() -> &'static [Rule] {
    static CACHE: OnceLock<Vec<Rule>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let parsed: RulesFile = serde_json::from_str(RULES).unwrap_or(RulesFile { rules: Vec::new() });
        let mut rules = parsed.rules;
        rules.sort_by(|a, b| b.priority.cmp(&a.priority));
        rules
    })
}

/// 一条命中的诊断。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashHit {
    pub id: String,
    pub severity: String,
    pub title: String,
    pub detail: String,
    pub fix: String,
    /// 命中的原始日志行（最多几行，够定位就行）。
    pub evidence: Vec<String>,
}

impl CrashHit {
    /// 给界面/日志用的一段话。
    pub fn summary(&self) -> String {
        if self.fix.is_empty() {
            format!("{}：{}", self.title, self.detail)
        } else {
            format!("{}：{}（{}）", self.title, self.detail, self.fix)
        }
    }
}

/// 扫一遍日志行，按优先级返回最多 `limit` 条诊断。
pub fn analyze(lines: &[String]) -> Vec<CrashHit> {
    analyze_with(lines, 3)
}

pub fn analyze_with(lines: &[String], limit: usize) -> Vec<CrashHit> {
    let lowered: Vec<String> = lines.iter().map(|line| line.to_lowercase()).collect();
    let mut hits: Vec<CrashHit> = Vec::new();
    for rule in rules() {
        if hits.len() >= limit {
            break;
        }
        if rule.any.is_empty() && rule.all.is_empty() {
            continue;
        }
        let mut evidence: Vec<String> = Vec::new();
        if !rule.all.is_empty() {
            let mut every = true;
            for needle in &rule.all {
                match lowered.iter().position(|line| line.contains(&needle.to_lowercase())) {
                    Some(at) => evidence.push(lines[at].clone()),
                    None => {
                        every = false;
                        break;
                    }
                }
            }
            if !every {
                continue;
            }
        }
        if !rule.any.is_empty() {
            let mut found = false;
            for needle in &rule.any {
                if let Some(at) = lowered.iter().position(|line| line.contains(&needle.to_lowercase())) {
                    evidence.push(lines[at].clone());
                    found = true;
                    break;
                }
            }
            if !found {
                continue;
            }
        }
        evidence.sort();
        evidence.dedup();
        evidence.truncate(4);
        hits.push(CrashHit {
            id: rule.id.clone(),
            severity: rule.severity.clone(),
            title: rule.title.clone(),
            detail: rule.detail.clone(),
            fix: rule.fix.clone(),
            evidence,
        });
    }
    // 同一类问题只留一条：标题相同的后面几条丢掉。
    let mut seen = std::collections::HashSet::new();
    hits.retain(|hit| seen.insert(hit.title.clone()));
    hits
}

/// 规则条数（诊断界面/日志里报一句「内置 N 条规则」）。
pub fn rule_count() -> usize {
    rules().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rules_are_loaded() {
        assert!(rule_count() > 50, "规则表太小：{}", rule_count());
    }

    #[test]
    fn java_too_old_is_recognised() {
        let hits = analyze(&lines(&[
            "[stdout] hello",
            "[stderr] java.lang.UnsupportedClassVersionError: net/minecraft/client/main/Main has been compiled by a more recent version of the Java Runtime (class file version 69.0), this version of the Java Runtime only recognizes class file versions up to 65.0",
        ]));
        assert_eq!(hits[0].id, "java.too-old");
        assert!(hits[0].evidence[0].contains("UnsupportedClassVersionError"));
    }

    #[test]
    fn oom_is_recognised() {
        let hits = analyze(&lines(&["[stderr] java.lang.OutOfMemoryError: Java heap space"]));
        assert_eq!(hits[0].id, "java.out-of-memory");
    }

    #[test]
    fn duplicate_mods_is_recognised() {
        let hits = analyze(&lines(&[
            "[stdout] [main/ERROR] Duplicate mods found",
            "[stdout] mod file: create-0.5.1.jar",
        ]));
        assert!(hits.iter().any(|hit| hit.id == "fml.duplicate-mods"));
    }

    #[test]
    fn crash_report_header_is_recognised() {
        let hits = analyze(&lines(&["[stdout] ---- Minecraft Crash Report ----"]));
        assert!(hits.iter().any(|hit| hit.id == "mc.crash-report"));
    }

    #[test]
    fn access_violation_is_recognised() {
        let hits = analyze(&lines(&[
            "[stderr] # A fatal error has been detected by the Java Runtime Environment:",
            "[stderr] #  EXCEPTION_ACCESS_VIOLATION (0xc0000005) at pc=0x00007ffb",
        ]));
        assert_eq!(hits[0].id, "render.gpu-crash");
    }

    #[test]
    fn patched_jar_missing_is_recognised() {
        let hits = analyze(&lines(&[
            "[stdout] Failed to start FML: The patched Minecraft jar is missing. Please try to reinstall NeoForge.",
        ]));
        assert_eq!(hits[0].id, "fml.patched-jar-missing");
    }

    #[test]
    fn unknown_crash_returns_nothing() {
        let hits = analyze(&lines(&["[stdout] starting", "[stdout] done"]));
        assert!(hits.is_empty());
    }
}
