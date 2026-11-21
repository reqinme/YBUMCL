//! 启动日志与运行日志的落盘。
//!
//! - 启动器日志按「启动次数」轮转，保留 5 份：`launcher.log`、`launcher.log.1`…`.4`。
//! - 每次启动游戏另写一份 `logs/runs/<时间>-<runId>-<实例>.log`，并在
//!   `logs/runs/index.json` 里登记元数据，供「历史运行」列表读取。
//! - 每次启动都覆盖写 `logs/LatestLaunch.bat`，双击即可复现。

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 保留几份启动器日志（含当前的 `launcher.log`）。
pub const LAUNCHER_LOG_KEEP: usize = 5;
/// 每个实例保留几份运行日志。
pub const RUNS_PER_VERSION: usize = 10;
/// 全部运行日志最多保留几份。
pub const RUNS_TOTAL: usize = 200;

const INDEX_VERSION: u32 = 1;

/// 启动器启动时轮转：`launcher.log` → `.1` → … → `.4`，多出来的删掉。
pub fn rotate_launcher_logs(dir: &Path) -> io::Result<()> {
    let current = dir.join("launcher.log");
    if !current.is_file() {
        return Ok(());
    }
    let mut slot = LAUNCHER_LOG_KEEP - 1;
    while slot > 1 {
        let from = dir.join(format!("launcher.log.{}", slot - 1));
        if from.is_file() {
            let _ = fs::rename(&from, dir.join(format!("launcher.log.{slot}")));
        }
        slot -= 1;
    }
    let _ = fs::rename(&current, dir.join("launcher.log.1"));
    Ok(())
}

fn local_stamp() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

fn readable_stamp() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

fn safe_name(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "instance".into()
    } else {
        cleaned
    }
}

/// 一份正在写入的运行日志。
pub struct RunLog {
    file: File,
    path: PathBuf,
    started: std::time::Instant,
}

impl RunLog {
    /// 打开一份新的运行日志并写入头部。
    pub fn open(
        dir: &Path,
        run_id: u64,
        version_id: &str,
        header: impl IntoIterator<Item = String>,
    ) -> io::Result<Self> {
        let runs = dir.join("runs");
        fs::create_dir_all(&runs)?;
        let name = format!("{}-{}-{}.log", local_stamp(), run_id, safe_name(version_id));
        let path = runs.join(name);
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        writeln!(file, "==== YBUMCL run log ====")?;
        writeln!(file, "实例：{version_id}")?;
        writeln!(file, "runId：{run_id}")?;
        writeln!(file, "开始：{}", readable_stamp())?;
        for line in header {
            writeln!(file, "{line}")?;
        }
        writeln!(file, "---- 游戏输出 ----")?;
        file.flush()?;
        Ok(Self {
            file,
            path,
            started: std::time::Instant::now(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 追加一行游戏输出。写失败就算了，日志不能拖垮启动。
    pub fn push(&mut self, stream: &str, text: &str) {
        let _ = writeln!(self.file, "[{stream}] {text}");
    }

    /// 收尾：写退出码与耗时，flush 后返回日志路径。
    pub fn finish(&mut self, code: Option<i32>) -> PathBuf {
        let elapsed = self.started.elapsed();
        let _ = writeln!(
            self.file,
            "---- 结束：{} · 退出码 {} · 用时 {:.1} 秒 ----",
            readable_stamp(),
            code.map(|c| c.to_string()).unwrap_or_else(|| "未知".into()),
            elapsed.as_secs_f64()
        );
        let _ = self.file.flush();
        self.path.clone()
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

/// 一次运行在 `index.json` 里的记录。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEntry {
    pub run_id: u64,
    pub version_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub started: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub file: String,
    #[serde(default)]
    pub crashed: bool,
}

#[derive(Serialize, Deserialize)]
struct Index {
    version: u32,
    runs: Vec<RunEntry>,
}

/// 读 `index.json`；损坏或不存在都当成空表。
pub fn load_index(dir: &Path) -> Vec<RunEntry> {
    let path = dir.join("runs").join("index.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(index) = serde_json::from_str::<Index>(&text) else {
        return Vec::new();
    };
    if index.version != INDEX_VERSION {
        return Vec::new();
    }
    index.runs
}

fn save_index(dir: &Path, runs: &[RunEntry]) -> io::Result<()> {
    let runs_dir = dir.join("runs");
    fs::create_dir_all(&runs_dir)?;
    let index = Index {
        version: INDEX_VERSION,
        runs: runs.to_vec(),
    };
    let path = runs_dir.join("index.json");
    let temp = runs_dir.join("index.json.part");
    fs::write(&temp, serde_json::to_vec_pretty(&index)?)?;
    let _ = fs::remove_file(&path);
    fs::rename(&temp, &path)?;
    Ok(())
}

/// 登记一条运行记录，同时按保留策略删旧日志，返回最新列表。
pub fn record(dir: &Path, entry: RunEntry) -> io::Result<Vec<RunEntry>> {
    let mut runs = load_index(dir);
    runs.retain(|r| r.run_id != entry.run_id);
    runs.push(entry);
    runs.sort_by_key(|r| r.run_id);
    trim(dir, &mut runs);
    save_index(dir, &runs)?;
    Ok(runs)
}

/// 保留策略：每实例最近 `RUNS_PER_VERSION` 份、全局最近 `RUNS_TOTAL` 份。
fn trim(dir: &Path, runs: &mut Vec<RunEntry>) {
    let mut by_version: std::collections::HashMap<&str, Vec<usize>> = std::collections::HashMap::new();
    for (at, run) in runs.iter().enumerate() {
        by_version.entry(run.version_id.as_str()).or_default().push(at);
    }
    let mut drop_at = vec![false; runs.len()];
    for list in by_version.values_mut() {
        list.sort_by_key(|at| runs[*at].run_id);
        let excess = list.len().saturating_sub(RUNS_PER_VERSION);
        for at in list.iter().take(excess) {
            drop_at[*at] = true;
        }
    }
    let mut keep: Vec<RunEntry> = runs
        .iter()
        .enumerate()
        .filter(|(at, _)| !drop_at[*at])
        .map(|(_, run)| run.clone())
        .collect();
    keep.sort_by_key(|run| run.run_id);
    if keep.len() > RUNS_TOTAL {
        keep.drain(..keep.len() - RUNS_TOTAL);
    }
    let kept: std::collections::HashSet<&str> = keep.iter().map(|r| r.file.as_str()).collect();
    for run in runs.iter() {
        if !kept.contains(run.file.as_str()) {
            let _ = fs::remove_file(dir.join("runs").join(&run.file));
        }
    }
    *runs = keep;
}

/// 最近若干条运行记录，新的在前。
pub fn history(dir: &Path, limit: usize) -> Vec<RunEntry> {
    let mut runs = load_index(dir);
    runs.sort_by_key(|run| std::cmp::Reverse(run.run_id));
    runs.truncate(limit);
    runs
}

/// 把这次的启动命令写成 `logs/LatestLaunch.bat`。
pub fn write_latest_batch(
    dir: &Path,
    working_directory: &str,
    java: &str,
    args: &[String],
) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join("LatestLaunch.bat");
    let mut text = String::from("@echo off\r\nchcp 65001 >nul\r\n");
    text.push_str(&format!(
        "cd /d \"{}\"\r\n",
        working_directory.replace('"', "")
    ));
    text.push_str(&quote(java));
    for arg in args {
        text.push(' ');
        text.push_str(&quote(arg));
    }
    text.push_str("\r\npause\r\n");
    fs::write(&path, text)?;
    Ok(path)
}

fn quote(value: &str) -> String {
    if value.contains(' ') || value.contains('"') {
        format!("\"{}\"", value.replace('"', ""))
    } else {
        value.to_string()
    }
}

/// 启动时那一刻的可读时间，供日志头部用。
pub fn now_readable() -> String {
    readable_stamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        // 不用 %TEMP%：沙箱下它指向工作区外，create_dir_all 会被拒绝（os error 5）。
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        let dir = base.join(format!("ybumcl-runlog-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rotation_keeps_five_files() {
        let dir = temp("rotate");
        for n in 1..=7 {
            fs::write(dir.join(format!("launcher.log.{n}")), format!("old{n}")).unwrap();
        }
        fs::write(dir.join("launcher.log"), "current").unwrap();
        rotate_launcher_logs(&dir).unwrap();
        assert_eq!(fs::read_to_string(dir.join("launcher.log.1")).unwrap(), "current");
        assert!(!dir.join("launcher.log").exists());
        assert!(dir.join("launcher.log.4").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_log_records_header_and_footer() {
        let dir = temp("run");
        let mut log = RunLog::open(&dir, 7, "26.3-NeoForge", ["java=25".to_string()]).unwrap();
        log.push("stdout", "hello");
        let path = log.finish(Some(0));
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("java=25"));
        assert!(text.contains("[stdout] hello"));
        assert!(text.contains("退出码 0"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn index_keeps_per_version_limit() {
        let dir = temp("index");
        for run_id in 1..=13 {
            let entry = RunEntry {
                run_id,
                version_id: "v".into(),
                pid: None,
                started: "now".into(),
                ended: None,
                exit_code: None,
                file: format!("{run_id}.log"),
                crashed: false,
            };
            record(&dir, entry).unwrap();
        }
        let runs = load_index(&dir);
        assert_eq!(runs.len(), RUNS_PER_VERSION);
        assert_eq!(runs.first().unwrap().run_id, 4);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_quotes_arguments() {
        let dir = temp("bat");
        let path = write_latest_batch(
            &dir,
            r"C:\games\runs",
            r"C:\Program Files\Java\bin\java.exe",
            &["-Xmx2G".to_string(), "--version".to_string()],
        )
        .unwrap();
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains(r#"cd /d "C:\games\runs""#));
        assert!(text.contains(r#""C:\Program Files\Java\bin\java.exe" -Xmx2G --version"#));
        let _ = fs::remove_dir_all(&dir);
    }
}
