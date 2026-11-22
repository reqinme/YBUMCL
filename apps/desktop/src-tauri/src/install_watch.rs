//! 安装进度的统一出口：写日志、折算总进度、盯住卡死。
//!
//! 发布版没有控制台，安装卡住时日志里必须留下「卡在哪一段」。同时这个模块给每条
//! 进度打上看门狗的时间戳，超过阈值没动静就主动中止，免得任务卡永远显示「进行中」。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use launcher_core::download::{CancellationToken, InstallProgress};
use launcher_core::progress::Tracker;
use tauri::{AppHandle, Emitter};

/// 多久没有任何进度就算「可疑」，写一条警告（新版本）。
const STALL_WARN: Duration = Duration::from_secs(90);
/// 多久没进度就中止安装（比警告长，避免慢速大文件被误杀）。
const STALL_ABORT: Duration = Duration::from_secs(300);
/// 老版本（1.7–1.16）、带 OptiFine/LiteLoader 的组合更容易长时间没进度：
/// 老 Forge 的处理器、OptiFine 的官网跳转都可能卡几分钟，阈值放宽一倍以上。
const LEGACY_WARN: Duration = Duration::from_secs(240);
const LEGACY_ABORT: Duration = Duration::from_secs(900);

/// 按版本名挑看门狗阈值。
fn stall_limits(version_id: &str) -> (Duration, Duration) {
    let lower = version_id.to_ascii_lowercase();
    let legacy_game = [
        "1.7.", "1.8.", "1.9.", "1.10.", "1.11.", "1.12.", "1.13.", "1.14.", "1.15.", "1.16.",
    ]
    .iter()
    .any(|prefix| lower.contains(prefix));
    let legacy_loader = lower.contains("optifine") || lower.contains("liteloader");
    if legacy_game || legacy_loader {
        (LEGACY_WARN, LEGACY_ABORT)
    } else {
        (STALL_WARN, STALL_ABORT)
    }
}

pub struct Reporter {
    app: AppHandle,
    log_dir: std::path::PathBuf,
    /// 前端任务卡的 id：几张卡同时装的时候靠它把进度写回各自的卡。
    install_id: u64,
    tracker: Mutex<Tracker>,
    state: Mutex<State>,
    finished: AtomicBool,
}

#[derive(Default)]
struct State {
    last_seen: Option<Instant>,
    stage: String,
}

impl Reporter {
    fn emit(&self, progress: InstallProgress) {
        if self.finished.load(Ordering::Relaxed) {
            return;
        }
        let progress = {
            let mut tracker = self.tracker.lock().unwrap_or_else(|e| e.into_inner());
            tracker.observe(progress)
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let stage_changed = state.stage != progress.stage;
        if stage_changed {
            state.stage = progress.stage.clone();
            append_log(
                &self.log_dir,
                &format!(
                    "安装 {} · {} · 文件 {}/{} · {} / {}",
                    progress.version_id,
                    stage_label(&progress.stage),
                    progress.completed_files,
                    progress.total_files,
                    bytes_label(progress.downloaded_bytes),
                    bytes_label(progress.total_bytes),
                ),
            );
        }
        state.last_seen = Some(Instant::now());
        drop(state);
        if matches!(progress.stage.as_str(), "complete" | "cancelled" | "failed") {
            self.finished.store(true, Ordering::Relaxed);
        }
        // 事件带 installId：下载页可以同时开好几个安装，前端按它把进度写回各自的卡。
        let _ = self.app.emit(
            "install-progress",
            InstallProgressEvent {
                install_id: self.install_id,
                progress,
            },
        );
    }
}

/// `install-progress` 的事件体：`InstallProgress` 的字段照旧平铺，另加任务卡 id。
/// 老监听只认原来的字段，多出来这一个不会影响它们。
#[derive(Clone, serde::Serialize)]
struct InstallProgressEvent {
    #[serde(flatten)]
    progress: InstallProgress,
    #[serde(rename = "installId")]
    install_id: u64,
}

/// 造一个进度出口，同时起一个看门狗线程；返回的闭包交给安装函数当 `emit`。
pub fn watch(
    app: AppHandle,
    log_dir: std::path::PathBuf,
    token: CancellationToken,
    has_loaders: bool,
    install_id: u64,
    version_id: &str,
) -> (Arc<Reporter>, impl Fn(InstallProgress) + Send + Sync + 'static) {
    let (stall_warn, stall_abort) = stall_limits(version_id);
    let reporter = Arc::new(Reporter {
        app,
        log_dir: log_dir.clone(),
        install_id,
        tracker: Mutex::new(Tracker::new(has_loaders)),
        state: Mutex::new(State::default()),
        finished: AtomicBool::new(false),
    });
    let guard = reporter.clone();
    std::thread::spawn(move || {
        let mut warned = false;
        loop {
            std::thread::sleep(Duration::from_secs(10));
            if guard.finished.load(Ordering::Relaxed) {
                return;
            }
            let idle = {
                let state = guard.state.lock().unwrap_or_else(|e| e.into_inner());
                state.last_seen.map(|at| at.elapsed())
            };
            let Some(idle) = idle else { continue };
            if idle >= stall_abort {
                append_log(
                    &log_dir,
                    &format!(
                        "看门狗：{} 秒没有收到进度，已中止这次安装（阶段 {}）",
                        idle.as_secs(),
                        guard
                            .state
                            .lock()
                            .map(|state| stage_label(&state.stage))
                            .unwrap_or_default()
                    ),
                );
                token.cancel();
                return;
            }
            if idle >= stall_warn && !warned {
                warned = true;
                append_log(
                    &log_dir,
                    &format!("安装可能卡住：{} 秒没有进度（阶段 {}）", idle.as_secs(), {
                        guard
                            .state
                            .lock()
                            .map(|state| stage_label(&state.stage))
                            .unwrap_or_default()
                    }),
                );
            }
        }
    });
    let handle = reporter.clone();
    let callback = move |progress: InstallProgress| handle.emit(progress);
    (reporter, callback)
}

/// 阶段名 → 用户看得懂的一句话（日志用，界面上用的是另一张表）。
pub fn stage_label(stage: &str) -> &'static str {
    match stage {
        "manifest" => "读取版本清单",
        "metadata" => "读取版本信息",
        "asset-index" => "获取资源索引",
        "jar" => "下载客户端",
        "libraries" => "下载支持库",
        "downloading" => "下载游戏资源",
        "fabric-profile" | "fabric-libraries" | "fabric-checksum" => "安装 Fabric",
        "modloader-installer" => "下载加载器",
        "modloader-prepare" => "准备加载器依赖",
        "modloader-install" => "运行加载器处理器",
        "modloader-verify" => "校验加载器产物",
        "modloader-publish" => "写入版本信息",
        "complete" => "完成",
        "cancelled" => "已取消",
        "failed" => "失败",
        _ => "安装中",
    }
}

fn bytes_label(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / MB)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn append_log(dir: &std::path::Path, message: &str) {
    use std::io::Write;
    let path = dir.join("launcher.log");
    let _ = std::fs::create_dir_all(dir);
    if std::fs::metadata(&path)
        .map(|meta| meta.len() > 512 * 1024)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, dir.join("launcher.log.1"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "[{}] {message}", crate::portable::local_hhmm());
    }
}
