//! 单独跑 LabyMod 资源预取：`labymod_prefetch <目标目录> [production|snapshot]`
use std::path::PathBuf;

use launcher_core::download::prefetch_labymod;
use launcher_core::download::CancellationToken;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = match args.next() {
        Some(dir) => PathBuf::from(dir),
        None => {
            eprintln!("用法：labymod_prefetch <目标目录> [production|snapshot]");
            std::process::exit(2);
        }
    };
    let channel = args.next().unwrap_or_else(|| "production".into());
    let cancel = CancellationToken::default();
    let started = std::time::Instant::now();
    match prefetch_labymod(&dir, &channel, &cancel, |step| {
        println!(
            "{} {}/{} {} ({:.1}s)",
            step.stage,
            step.completed_files,
            step.total_files,
            step.path,
            started.elapsed().as_secs_f64()
        );
    }) {
        Ok(bytes) => println!(
            "ok {bytes} bytes in {:.1}s",
            started.elapsed().as_secs_f64()
        ),
        Err(error) => {
            eprintln!("failed: {error}");
            std::process::exit(1);
        }
    }
}
