//! 真机安装用的手动调试入口（不进测试、不打进发布包）。
//!
//! ```text
//! cargo run -p launcher-core --example install -- java
//! cargo run -p launcher-core --example install -- list forge 1.20.1
//! cargo run -p launcher-core --example install -- vanilla target/e2e 1.20.1 --mirror
//! cargo run -p launcher-core --example install -- loader target/e2e 1.20.1 forge 47.3.0 "C:\Java\jdk17\bin\java.exe" --mirror
//! cargo run -p launcher-core --example install -- loader target/e2e 1.20.1 fabric 0.16.5 - --mirror
//! cargo run -p launcher-core --example install -- launch target/e2e 1.20.1-forge-47.4.9 "C:/Java/jdk17/bin/java.exe" 60
//! cargo run -p launcher-core --example install -- instance target/e2e 1.12.2 1.12.2-LiteLoader liteloader=1.12.2-SNAPSHOT --mirror --overwrite
//! ```

use launcher_core::{
    download::{
        list_fabric_loaders, list_legacy_fabric_loaders, list_quilt_loaders, CancellationToken,
        InstallRequest,
    },
    install_version,
    modloader::{install_modloader, list_modloader_versions, ModLoader, ModLoaderInstallRequest},
    InstallProgress, InstallSummary,
};
use std::path::PathBuf;

/// `--mirror` 现在是「官方 + 镜像都试，谁快用谁」的策略。
fn strategy_from(mirror: bool) -> launcher_core::download::DownloadStrategy {
    if mirror {
        launcher_core::download::DownloadStrategy::Race
    } else {
        launcher_core::download::DownloadStrategy::OfficialOnly
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parts: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match parts.as_slice() {
        ["java", ..] => java(),
        ["list", loader, game] => list(loader, game),
        ["vanilla", dir, game, rest @ ..] => vanilla(dir, game, strategy_from(rest.contains(&"--mirror"))),
        ["loader", dir, game, loader, version, java_path, rest @ ..] => {
            loader_install(dir, game, loader, version, java_path, strategy_from(rest.contains(&"--mirror")))
        }
        ["plan", dir, version_id, java_path, out] => plan_dump(dir, version_id, java_path, out),
        ["repair", dir, version_id, rest @ ..] => repair(dir, version_id, strategy_from(rest.contains(&"--mirror"))),
        ["instance", dir, game, name, loaders, rest @ ..] => {
            instance(dir, game, name, loaders, strategy_from(rest.contains(&"--mirror")), rest.contains(&"--overwrite"))
        }
        ["launch", dir, version_id, java_path, seconds] => match seconds.parse() {
            Ok(seconds) => launch(dir, version_id, java_path, seconds),
            Err(_) => Err(format!("bad seconds: {seconds}")),
        },
        _ => Err(
            "usage: java | list <loader> <game> | vanilla <dir> <game> | loader <dir> <game> <loader> <version> <java|-> [--mirror] | launch <dir> <version> <java> <seconds>"
                .into(),
        ),
    };
    if let Err(error) = result {
        eprintln!("失败：{error}");
        std::process::exit(1);
    }
}

fn java() -> Result<(), String> {
    for java in launcher_core::detect_java() {
        println!("{}\t{}\t{}", java.major, java.path, java.version);
    }
    Ok(())
}

fn list(loader: &str, game: &str) -> Result<(), String> {
    if loader == "fabric" {
        for version in list_fabric_loaders(game, launcher_core::download::DownloadStrategy::default()).map_err(|e| e.to_string())? {
            println!("{}\tstable={}", version.version, version.stable);
        }
        return Ok(());
    }
    if loader == "legacyfabric" {
        for version in list_legacy_fabric_loaders(game).map_err(|e| e.to_string())? {
            println!("{}	stable={}", version.version, version.stable);
        }
        return Ok(());
    }
    if loader == "labymod" {
        for channel in ["production", "snapshot"] {
            for entry in launcher_core::download::labymod_versions(channel)
                .map_err(|e| e.to_string())?
                .iter()
                .take(6)
            {
                println!("{}	{}	{}", entry.channel, entry.version, entry.game);
            }
        }
        return Ok(());
    }
    if loader == "quilt" {
        for version in list_quilt_loaders(game).map_err(|e| e.to_string())? {
            println!("{}\tstable={}", version.version, version.stable);
        }
        return Ok(());
    }
    let mut versions =
        list_modloader_versions(parse_loader(loader)?, game).map_err(|e| e.to_string())?;
    versions.sort_by(|a, b| b.version.cmp(&a.version));
    for version in versions.iter().take(15) {
        println!("{}", version.version);
    }
    Ok(())
}

fn parse_loader(name: &str) -> Result<ModLoader, String> {
    Ok(match name {
        "forge" => ModLoader::Forge,
        "neoforge" => ModLoader::NeoForge,
        "cleanroom" => ModLoader::Cleanroom,
        "liteloader" => ModLoader::LiteLoader,
        "optifine" => ModLoader::OptiFine,
        other => return Err(format!("unknown loader {other}")),
    })
}

fn instance(
    dir: &str,
    game: &str,
    name: &str,
    loaders: &str,
    strategy: launcher_core::download::DownloadStrategy,
    overwrite: bool,
) -> Result<(), String> {
    let game_dir = prepare(dir)?;
    let choices: Vec<launcher_core::instance::LoaderChoice> = if loaders == "-" {
        Vec::new()
    } else {
        loaders
            .split(',')
            .map(|item| {
                let (loader, version) = item.split_once('=').unwrap_or((item, ""));
                launcher_core::instance::LoaderChoice {
                    loader: loader.to_owned(),
                    version: version.to_owned(),
                }
            })
            .collect()
    };
    let summary = launcher_core::instance::install_instance(
        launcher_core::instance::InstanceRequest {
            game_dir,
            game_version: game.to_owned(),
            instance_name: Some(name.to_owned()),
            loaders: choices,
            java_path: None,
            strategy,
            threads: Some(16),
            overwrite,
        },
        CancellationToken::default(),
        on_progress,
    )
    .map_err(|e| e.to_string())?;
    println!("instance ok: {summary:?}");
    Ok(())
}

fn prepare(dir: &str) -> Result<String, String> {
    let path = PathBuf::from(dir);
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

fn vanilla(dir: &str, game: &str, strategy: launcher_core::download::DownloadStrategy) -> Result<(), String> {
    let game_dir = prepare(dir)?;
    let summary = install_version(
        InstallRequest {
            game_dir,
            version_id: game.to_owned(),
            fabric_loader: None,
            quilt_loader: None,
            strategy,
            threads: Some(16),
        },
        CancellationToken::default(),
        on_progress,
    )
    .map_err(|e| e.to_string())?;
    println!("vanilla ok: {summary:?}");
    Ok(())
}

fn loader_install(
    dir: &str,
    game: &str,
    loader: &str,
    version: &str,
    java_path: &str,
    strategy: launcher_core::download::DownloadStrategy,
) -> Result<(), String> {
    let game_dir = prepare(dir)?;
    if matches!(loader, "fabric" | "quilt") {
        let summary = install_version(
            InstallRequest {
                game_dir,
                version_id: game.to_owned(),
                fabric_loader: (loader == "fabric").then(|| version.to_owned()),
                quilt_loader: (loader == "quilt").then(|| version.to_owned()),
                strategy,
                threads: Some(16),
            },
            CancellationToken::default(),
            on_progress,
        )
        .map_err(|e| e.to_string())?;
        println!("{loader} ok: {summary:?}");
        return Ok(());
    }
    let loader = parse_loader(loader)?;
    let java_path = if java_path == "-" {
        launcher_core::detect_java()
            .into_iter()
            .find(|java| {
                java.major >= launcher_core::modloader::required_java_for(game, loader, version)
            })
            .map(|java| java.path)
            .ok_or_else(|| "没有合适的 Java".to_string())?
    } else {
        java_path.to_owned()
    };
    println!("java: {java_path}");
    let summary = install_modloader(
        ModLoaderInstallRequest {
            game_dir,
            game_version: game.to_owned(),
            loader,
            loader_version: version.to_owned(),
            java_path,
            strategy,
            threads: Some(16),
        },
        CancellationToken::default(),
        on_progress,
    )
    .map_err(|e| e.to_string())?;
    print_summary(&summary);
    Ok(())
}

fn on_progress(progress: InstallProgress) {
    if matches!(
        progress.stage.as_str(),
        "complete" | "failed" | "cancelled" | "modloader-install" | "modloader-prepare"
    ) {
        eprintln!(
            "[{}] {} {}/{}",
            progress.stage, progress.version_id, progress.completed_files, progress.total_files
        );
    }
}

fn print_summary(summary: &InstallSummary) {
    println!(
        "ok: {} 下载 / {} 跳过 / {} 字节 -> {}",
        summary.downloaded_files,
        summary.skipped_files,
        summary.downloaded_bytes,
        summary.version_id
    );
}

/// 只把启动计划的 java 路径与参数落到文件，交给外部脚本拉起（排查环境问题时用）。
fn plan_dump(dir: &str, version_id: &str, java_path: &str, out: &str) -> Result<(), String> {
    let game_dir = prepare(dir)?;
    let plan = launcher_core::build_launch_plan(launcher_core::LaunchRequest {
        game_dir: game_dir.clone(),
        game_directory: None,
        version_id: version_id.to_owned(),
        java_path: java_path.to_owned(),
        username: "TestPlayer".to_owned(),
        max_memory_mb: 2048,
        access_token: None,
        uuid: None,
        ..launcher_core::LaunchRequest::default()
    })
    .map_err(|e| e.to_string())?;
    let mut text = plan.java_path.clone();
    text.push('\n');
    text.push_str(&plan.args.join(" "));
    text.push('\n');
    std::fs::write(out, text).map_err(|e| e.to_string())?;
    println!("wrote {out}: {} args", plan.args.len());
    Ok(())
}

/// 真机启动：按启动器的计划拉起游戏，抓 N 秒日志，看有没有进主菜单或崩溃。
fn launch(dir: &str, version_id: &str, java_path: &str, seconds: u64) -> Result<(), String> {
    let game_dir = prepare(dir)?;
    let plan = launcher_core::build_launch_plan(launcher_core::LaunchRequest {
        game_dir: game_dir.clone(),
        game_directory: None,
        version_id: version_id.to_owned(),
        java_path: java_path.to_owned(),
        username: "TestPlayer".to_owned(),
        max_memory_mb: 2048,
        access_token: None,
        uuid: None,
        ..launcher_core::LaunchRequest::default()
    })
    .map_err(|e| e.to_string())?;
    println!("java: {}", plan.java_path);
    println!("cwd: {}", plan.working_directory);
    println!("args: {}", plan.redacted_args().join(" "));
    for warning in &plan.warnings {
        println!("warning: {warning}");
    }
    let log = PathBuf::from(&game_dir).join(format!(".ybmcl-launch-{version_id}.log"));
    let file = std::fs::File::create(&log).map_err(|e| e.to_string())?;
    let errors = file.try_clone().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&plan.working_directory).map_err(|e| e.to_string())?;
    // 调试机上 %TEMP% 对子进程不可写，JNA / ImageIO 这类会把 dll 解到临时目录的库会直接炸。
    let temp = PathBuf::from(&game_dir).join(".ybmcl-tmp");
    std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new(&plan.java_path)
        .args(&plan.args)
        .current_dir(&plan.working_directory)
        .env("TMP", &temp)
        .env("TEMP", &temp)
        .stdin(std::process::Stdio::null())
        .stdout(file)
        .stderr(errors)
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let mut finished = None;
    while std::time::Instant::now() < deadline {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            finished = Some(status);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if finished.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let tail: Vec<&str> = text.lines().rev().take(40).collect();
    for line in tail.into_iter().rev() {
        println!("{line}");
    }
    let crashed = text.contains("---- Minecraft Crash Report ----")
        || text.contains("A fatal error has been detected")
        || text.contains("Exception in thread \"main\"")
        || text.contains("Failed to start game");
    let started = text.contains("Sound engine started")
        || text.contains("Created: 1024x512x4 minecraft:textures/atlas")
        || text.contains("Backend library: LWJGL");
    println!("--- log: {} ---", log.display());
    println!(
        "result: {} ({}s)",
        if crashed {
            "CRASHED"
        } else if finished.is_some() {
            "EXITED"
        } else if started {
            "RUNNING"
        } else {
            "NO-MENU"
        },
        seconds
    );
    if crashed || started {
        return Ok(());
    }
    Err("did not reach the main menu".into())
}

/// 补齐一个已存在 profile 缺的文件（别的启动器建的实例常常是「只写了 json」）。
fn repair(dir: &str, version_id: &str, strategy: launcher_core::download::DownloadStrategy) -> Result<(), String> {
    let cancel = CancellationToken::default();
    let summary = launcher_core::download::install_profile_assets(
        dir,
        version_id,
        strategy,
        None,
        cancel,
        |progress: InstallProgress| {
            if !progress.stage.is_empty() {
                println!("[{}] {}", progress.stage, progress.path);
            }
        },
    )
    .map_err(|e| e.to_string())?;
    println!(
        "ok: {} 下载 / {} 跳过 / {} 字节",
        summary.downloaded_files, summary.skipped_files, summary.downloaded_bytes
    );
    Ok(())
}
