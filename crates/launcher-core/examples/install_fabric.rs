//! Fabric smoke tool. Always pass an isolated game directory.
//! `cargo run -p launcher-core --example install_fabric -- loaders 1.21.1`
//! `cargo run -p launcher-core --example install_fabric -- C:/Games/test 1.21.1 0.16.14`
use launcher_core::{install_version, CancellationToken, InstallRequest};

fn mirror() -> Option<bool> {
    std::env::var("YBUMCL_MIRROR")
        .ok()
        .map(|value| value != "0" && !value.is_empty())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command, game] if command == "loaders" => println!("{}", serde_json::to_string_pretty(&launcher_core::download::list_fabric_loaders(game, launcher_core::download::DownloadStrategy::default())?)?),
        [game_dir, version_id, loader] => {
            let summary = install_version(InstallRequest {
                game_dir: game_dir.clone(), version_id: version_id.clone(), fabric_loader: Some(loader.clone()),
                quilt_loader: None,
                strategy: launcher_core::download::DownloadStrategy::default(), threads: None,
            }, CancellationToken::default(), |event| {
                if event.completed_files == event.total_files || event.stage != "downloading" {
                    eprintln!("{}", serde_json::to_string(&event).expect("serializable progress"));
                }
            })?;
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        _ => return Err("Usage: install_fabric loaders <game-version> | <absolute-game-dir> <game-version> <loader-version>".into()),
    }
    Ok(())
}
