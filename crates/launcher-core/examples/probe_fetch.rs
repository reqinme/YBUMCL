//! 单个文件的下载冒烟测试：验证自适应分段（PCL 式速度闸门 + 40% 切块）、
//! HTTP/2 启用与按站点回退、连接预热。
//!
//! 用法：`cargo run -q -p launcher-core --example probe_fetch -- <url> <dest> [size]`
//! 只打印进度，不碰 .minecraft。
use launcher_core::download::{fetch_files_to, DownloadJob, DownloadStrategy};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("用法: probe_fetch <url> <dest> [size]");
        std::process::exit(2);
    }
    let size = args.get(2).and_then(|text| text.parse::<u64>().ok());
    let job = DownloadJob {
        urls: vec![args[0].clone()],
        dest: PathBuf::from(&args[1]),
        size,
        sha1: args.get(3).cloned(),
    };
    let started = Instant::now();
    let last = std::sync::Mutex::new(Instant::now() - Duration::from_secs(1));
    let outcome = fetch_files_to(
        &[job],
        &Default::default(),
        4,
        DownloadStrategy::Race,
        |progress| {
            let Ok(mut at) = last.lock() else { return };
            if at.elapsed() < Duration::from_millis(300) {
                return;
            }
            *at = Instant::now();
            drop(at);
            println!(
                "[{:>6.1}s] {} {} / {} · {} MB/s",
                started.elapsed().as_secs_f64(),
                progress.file,
                progress.bytes,
                progress.total_bytes,
                progress.speed / 1024 / 1024
            );
        },
    );
    match outcome {
        Ok((written, failures)) => {
            println!(
                "OK written={written} failures={} 耗时 {:?}",
                failures.len(),
                started.elapsed()
            );
            let meta = std::fs::metadata(&args[1]).unwrap();
            println!("落盘 {} 字节", meta.len());
        }
        Err(error) => {
            eprintln!("ERR {error}");
            std::process::exit(1);
        }
    }
}
