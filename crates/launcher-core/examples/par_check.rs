//! 同进程内并行安装的回归检查：模拟「界面里同时点装原版和装加载器实例」。
//!
//! ```text
//! cargo run -p launcher-core --example par_check -- target/par2 1.21.1 neoforge 21.1.99 "C:\Java\bin\java.exe" "C:\cache\.minecraft"
//! ```

use launcher_core::download::{
    install_version, CancellationToken, DownloadStrategy, InstallRequest,
};
use launcher_core::instance::{install_instance, InstanceRequest, LoaderChoice};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 6 {
        eprintln!("usage: par_check <root> <game> <loader> <loader-version> <java|-> <cache-root>");
        std::process::exit(2);
    }
    let root = std::fs::canonicalize(&args[0]).unwrap_or_else(|_| {
        std::fs::create_dir_all(&args[0]).unwrap();
        std::fs::canonicalize(&args[0]).unwrap()
    });
    let root_text = root.to_string_lossy().to_string();
    let game = args[1].clone();
    let loader = args[2].clone();
    let loader_version = args[3].clone();
    let java = args[4].clone();
    let cache = args[5].clone();

    let vanilla_root = root_text.clone();
    let vanilla_game = game.clone();
    let vanilla = std::thread::spawn(move || {
        let summary = install_version(
            InstallRequest {
                game_dir: vanilla_root,
                version_id: vanilla_game,
                fabric_loader: None,
                quilt_loader: None,
                strategy: DownloadStrategy::Sequential,
                threads: Some(64),
                cache_roots: vec![std::env::args().nth(6).unwrap_or_default()],
            },
            CancellationToken::default(),
            |_| {},
        );
        match summary {
            Ok(done) => println!("vanilla ok: {} files", done.downloaded_files),
            Err(error) => println!("vanilla failed: {error}"),
        }
    });

    let inst_root = root_text.clone();
    let inst_game = game.clone();
    let game_for_print = game.clone();
    let cache_roots: Vec<String> = if cache.is_empty() || cache == "-" {
        Vec::new()
    } else {
        vec![cache]
    };
    let instance = std::thread::spawn(move || {
        let name = format!("{inst_game}-{loader}_par");
        let summary = install_instance(
            InstanceRequest {
                game_dir: inst_root,
                game_version: inst_game,
                instance_name: Some(name),
                loaders: vec![LoaderChoice {
                    loader,
                    version: loader_version,
                }],
                java_path: Some(java),
                strategy: DownloadStrategy::Sequential,
                threads: Some(64),
                cache_roots,
                overwrite: true,
            },
            CancellationToken::default(),
            |_| {},
        );
        match summary {
            Ok(done) => println!("instance ok: {}", done.version_id),
            Err(error) => println!("instance failed: {error}"),
        }
    });

    vanilla.join().unwrap();
    instance.join().unwrap();
    let base = Path::new(&root_text).join("versions").join(&game_for_print).join(format!("{game_for_print}.json"));
    println!("base json present: {}", base.is_file());
}
