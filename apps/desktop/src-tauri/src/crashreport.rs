//! 打包「错误报告」zip：诊断结论 + 本次运行日志 + 启动器日志 + 游戏日志 + 崩溃报告。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::portable::Paths;

fn push(zip: &mut zip::ZipWriter<std::fs::File>, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file(name, options)?;
    zip.write_all(bytes)
}

fn add_file(zip: &mut zip::ZipWriter<std::fs::File>, name: &str, path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return false;
    }
    push(zip, name, &bytes).is_ok()
}

fn newest(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(time) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().map(|(at, _)| time > *at).unwrap_or(true) {
            best = Some((time, path));
        }
    }
    best.map(|(_, path)| path)
}

/// 写一份 zip。返回实际写入的路径。
pub fn write(
    target: &Path,
    paths: &Paths,
    game_dir: &str,
    version_id: &str,
    run_file: Option<&str>,
) -> Result<PathBuf, String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建不了目录：{e}"))?;
    }
    let log_dir = paths.log_dir();
    let file = std::fs::File::create(target).map_err(|e| format!("建不了 zip：{e}"))?;
    let mut zip = zip::ZipWriter::new(file);

    // 1. 本次运行日志（拿到了就当作诊断上下文）。
    let runs_dir = log_dir.join("runs");
    let run_path = run_file
        .map(|name| runs_dir.join(name))
        .filter(|path| path.is_file())
        .or_else(|| {
            let mut names: Vec<PathBuf> = std::fs::read_dir(&runs_dir)
                .map(|dir| {
                    dir.flatten()
                        .map(|entry| entry.path())
                        .filter(|path| {
                            path.extension().and_then(|e| e.to_str()) == Some("log")
                                && path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .map(|n| n.contains(version_id) || version_id.is_empty())
                                    .unwrap_or(false)
                        })
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names.pop()
        });
    let mut context: Vec<String> = Vec::new();
    if let Some(path) = run_path.as_ref() {
        if let Ok(text) = std::fs::read_to_string(path) {
            context.extend(text.lines().map(|line| line.to_string()));
        }
    }

    // 2. 游戏自己的 latest.log（尾部），再补进上下文。
    let latest = Path::new(game_dir)
        .join("versions")
        .join(version_id)
        .join("logs")
        .join("latest.log");
    if let Ok(text) = std::fs::read_to_string(&latest) {
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.len().saturating_sub(400);
        context.extend(lines[start..].iter().map(|line| line.to_string()));
    }

    // 3. 诊断文本。
    let hits = launcher_core::crashlog::analyze(&context);
    let mut diagnosis = String::new();
    diagnosis.push_str("YBUMCL 错误报告\n");
    diagnosis.push_str(&format!("实例：{version_id}\n"));
    diagnosis.push_str(&format!("时间：{}\n", crate::runlog::now_readable()));
    diagnosis.push_str(&format!(
        "内置规则：{} 条\n\n",
        launcher_core::crashlog::rule_count()
    ));
    if hits.is_empty() {
        diagnosis.push_str("没有命中已知原因。下面是本次收集到的文件。\n");
    } else {
        for (at, hit) in hits.iter().enumerate() {
            diagnosis.push_str(&format!("{}. {}\n", at + 1, hit.title));
            diagnosis.push_str(&format!("   {}\n", hit.detail));
            if !hit.fix.is_empty() {
                diagnosis.push_str(&format!("   建议：{}\n", hit.fix));
            }
            for line in &hit.evidence {
                diagnosis.push_str(&format!("   > {line}\n"));
            }
            diagnosis.push('\n');
        }
    }
    push(&mut zip, "诊断.txt", diagnosis.as_bytes()).map_err(|e| e.to_string())?;

    // 4. 各种日志与配置。
    if let Some(path) = run_path.as_ref() {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "run.log".into());
        add_file(&mut zip, &format!("运行日志/{name}"), path);
    }
    for slot in 0..crate::runlog::LAUNCHER_LOG_KEEP {
        let name = if slot == 0 {
            "launcher.log".to_string()
        } else {
            format!("launcher.log.{slot}")
        };
        add_file(&mut zip, &format!("启动器日志/{name}"), &log_dir.join(&name));
    }
    add_file(&mut zip, "游戏日志/latest.log", &latest);
    if let Some(report) = newest(&Path::new(game_dir).join("versions").join(version_id).join("crash-reports")).as_ref() {
        let name = report
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "crash.txt".into());
        add_file(&mut zip, &format!("崩溃报告/{name}"), report);
    }
    let version_json = Path::new(game_dir)
        .join("versions")
        .join(version_id)
        .join(format!("{version_id}.json"));
    add_file(&mut zip, "版本.json", &version_json);
    add_file(&mut zip, "LatestLaunch.bat", &log_dir.join("LatestLaunch.bat"));

    // 5. 系统与 Java 信息。
    let mut system = String::new();
    system.push_str(&format!("OS：{}\n", std::env::consts::OS));
    system.push_str(&format!("架构：{}\n", std::env::consts::ARCH));
    system.push_str(&format!("启动器：{}\n", env!("CARGO_PKG_VERSION")));
    system.push_str("Java：\n");
    for java in launcher_core::detect_java() {
        system.push_str(&format!(
            "  {}（{}，{}）\n",
            java.major, java.path, if java.is_64 { "64 位" } else { "32 位" }
        ));
    }
    push(&mut zip, "系统.txt", system.as_bytes()).map_err(|e| e.to_string())?;

    zip.finish().map_err(|e| e.to_string())?;
    Ok(target.to_path_buf())
}
