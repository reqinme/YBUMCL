//! 手动跑一次整合包安装，把每个进度事件打出来：
//!
//! ```text
//! cargo run -q -p launcher-core --example probe_pack -- <url> <game_dir> [size]
//! ```
//!
//! 用来查「任务卡一直 0 B」这类问题：能看清事件有没有来、停在哪个阶段。

use std::time::Instant;

use launcher_core::modpack::{install_modpack, ModpackRequest};
use launcher_core::CancellationToken;

fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().expect("url");
    let game_dir = args.next().expect("game_dir");
    let size = args.next().and_then(|raw| raw.parse::<u64>().ok());
    let started = Instant::now();
    let cancel = CancellationToken::default();
    let request = ModpackRequest {
        game_dir,
        url: Some(url),
        path: None,
        file_name: None,
        size,
        name: Some("probe-pack".into()),
        java_path: None,
        strategy: Default::default(),
        threads: Some(24),
        icon_url: None,
        source: None,
        project_id: None,
        project_name: None,
    };
    let result = install_modpack(request, cancel, |step| {
        println!(
            "[{:>6.1}s] {:<12} {}/{} {:>10}B /{:>10}B overall={:.1} speed={}KB/s {}",
            started.elapsed().as_secs_f64(),
            step.stage,
            step.done,
            step.total,
            step.bytes,
            step.total_bytes,
            step.overall,
            step.speed / 1024,
            step.file.unwrap_or(""),
        );
    });
    match result {
        Ok(summary) => println!("OK {} -> {} ({} files)", summary.version_id, summary.path, summary.files),
        Err(error) => println!("ERR {error}"),
    }
}
