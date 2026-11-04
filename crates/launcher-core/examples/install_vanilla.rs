//! `cargo run -p launcher-core --example install_vanilla -- list`
//! `cargo run -p launcher-core --example install_vanilla -- <absolute-game-dir> <version-id>`
//! Always pass a disposable, isolated directory when smoke testing.
use launcher_core::{install_version, list_remote_versions, CancellationToken, InstallRequest};

fn mirror() -> Option<bool> {
    std::env::var("YBUMCL_MIRROR")
        .ok()
        .map(|value| value != "0" && !value.is_empty())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command] if command == "list" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&list_remote_versions(launcher_core::download::DownloadStrategy::default())?)?
            );
        }
        [game_dir, version_id] => {
            let result = install_version(
                InstallRequest {
                    game_dir: game_dir.clone(),
                    version_id: version_id.clone(),
                    fabric_loader: None,
                    quilt_loader: None,
                    strategy: launcher_core::download::DownloadStrategy::default(),
                    threads: None,
                },
                CancellationToken::default(),
                |progress| {
                    eprintln!(
                        "{}",
                        serde_json::to_string(&progress).expect("serializable progress")
                    );
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        _ => return Err("Usage: install_vanilla list | <absolute-game-dir> <version-id>".into()),
    }
    Ok(())
}
