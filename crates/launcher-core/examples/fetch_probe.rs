use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

fn main() {
    let url = std::env::args()
        .nth(1)
        .expect("usage: fetch_probe <url> [--progress]");
    let progress = std::env::args().any(|arg| arg == "--progress");
    let cancel = launcher_core::download::CancellationToken::default();
    let started = Instant::now();
    let result = if progress {
        let received = AtomicU64::new(0);
        let known = AtomicU64::new(0);
        let reported = AtomicU64::new(0);
        let on_change = || {
            let bytes = received.load(Ordering::Relaxed);
            let total = known.load(Ordering::Relaxed);
            // 只打印每 1 MB 一档，验证计数不会超过总数。
            if bytes.saturating_sub(reported.load(Ordering::Relaxed)) >= 1024 * 1024 {
                reported.store(bytes, Ordering::Relaxed);
                println!("+ {bytes} / {total} B");
            }
        };
        let result = launcher_core::download::fetch_file_bytes_progress(
            &url,
            &cancel,
            &received,
            &known,
            &on_change,
        );
        println!(
            "final {} / {} B",
            received.load(Ordering::Relaxed),
            known.load(Ordering::Relaxed)
        );
        result
    } else {
        launcher_core::download::fetch_file_bytes(&url, &cancel)
    };
    match result {
        Ok(bytes) => println!(
            "ok {} bytes in {:.1}s ({:.0} KB/s)",
            bytes.len(),
            started.elapsed().as_secs_f64(),
            bytes.len() as f64 / 1024.0 / started.elapsed().as_secs_f64()
        ),
        Err(error) => println!("err after {:.1}s: {error}", started.elapsed().as_secs_f64()),
    }
}
