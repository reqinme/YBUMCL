//! 安装包下载的探针：按 `packages` 的流程把文件抓到本地。
//!
//! 用法：`probe_pkg <kind> <game> <version> <dir>`

use std::path::PathBuf;

use launcher_core::download::CancellationToken;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!("usage: probe_pkg <kind> <game> <version> <dir>");
        std::process::exit(2);
    }
    let cancel = CancellationToken::default();
    let files =
        match launcher_core::packages::package_files(
            &args[0],
            &args[1],
            &args[2],
            launcher_core::download::DownloadStrategy::default(),
            &cancel,
        ) {
            Ok(files) => files,
            Err(error) => {
                eprintln!("package_files failed: {error}");
                std::process::exit(1);
            }
        };
    for file in &files {
        println!("file: {} {}", file.name, file.url);
    }
    let dir = PathBuf::from(&args[3]);
    match launcher_core::packages::download_into(&files, &dir, &cancel) {
        Ok(written) => {
            for path in &written {
                let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
                println!("ok: {path} ({size} bytes)");
            }
        }
        Err(error) => {
            eprintln!("download failed: {error}");
            std::process::exit(1);
        }
    }
}
