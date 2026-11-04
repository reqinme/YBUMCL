//! Official loader smoke tool. Pass a dedicated test directory, never a live game library.
use launcher_core::{
    modloader::{self, ModLoader, ModLoaderInstallRequest},
    CancellationToken,
};

fn mirror() -> Option<bool> {
    std::env::var("YBUMCL_MIRROR")
        .ok()
        .map(|value| value != "0" && !value.is_empty())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let loader = match args.first().map(String::as_str) {
        Some("forge") => ModLoader::Forge,
        Some("neoforge") => ModLoader::NeoForge,
        _ => return Err("First argument must be forge or neoforge".into()),
    };
    match &args[1..] {
        [command, game] if command == "list" => {
            println!("{}", serde_json::to_string_pretty(&modloader::list_modloader_versions(loader, game)?)?);
        }
        [game_dir, game_version, loader_version, java_path] => {
            let result = modloader::install_modloader(ModLoaderInstallRequest {
                game_dir: game_dir.clone(), game_version: game_version.clone(), loader,
                loader_version: loader_version.clone(), java_path: java_path.clone(),
                strategy: launcher_core::download::DownloadStrategy::default(), threads: None,
            }, CancellationToken::default(), |event| {
                if event.stage != "downloading" || event.completed_files == event.total_files {
                    eprintln!("{}", serde_json::to_string(&event).expect("progress serialization"));
                }
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        _ => return Err("Usage: install_modloader forge|neoforge list <game> | <isolated-game-dir> <game> <loader-version> <java-path>".into()),
    }
    Ok(())
}
