//! 把安装过程各阶段的进度折成一条总进度。
//!
//! 一次安装分两段：先装原版（下载阶段自己已经算好了 0–100 的百分比），再装加载器
//! （安装器、库、processors，各阶段只报文件数）。以前界面直接显示后端的 `percent`，
//! 加载器阶段没有百分比，进度条就停在那儿不动。这里按「原版占大头」的比例把两段拼起来，
//! 并且只许涨不许跌，避免界面来回跳。

use crate::download::InstallProgress;

/// 原版阶段（下载器内部已经算好百分比）。
const VANILLA_WEIGHT: f64 = 80.0;
/// 加载器阶段（安装器 + 库 + processors + 收尾）。
const LOADER_WEIGHT: f64 = 20.0;

/// 加载器各阶段的相对位置（0..1），用来在没有百分比时粗略折算。
const LOADER_STEPS: &[(&str, f64)] = &[
    ("modloader-installer", 0.10),
    ("modloader-prepare", 0.20),
    ("modloader-install", 0.45),
    ("modloader-verify", 0.80),
    ("modloader-publish", 0.95),
    ("complete", 1.00),
];

#[derive(Debug)]
pub struct Tracker {
    has_loaders: bool,
    vanilla: f64,
    loader: f64,
    last: f64,
}

impl Tracker {
    pub fn new(has_loaders: bool) -> Self {
        Self {
            has_loaders,
            vanilla: 0.0,
            loader: 0.0,
            last: 0.0,
        }
    }

    /// 吃掉一条原始进度，返回补好 `percent` 的副本；未知阶段原样放过。
    pub fn observe(&mut self, mut progress: InstallProgress) -> InstallProgress {
        let stage = progress.stage.as_str();
        if matches!(stage, "complete" | "cancelled" | "failed") {
            if stage == "complete" {
                self.last = 100.0;
                progress.percent = Some(100.0);
            }
            return progress;
        }
        if stage.starts_with("modloader-") {
            if !self.has_loaders {
                return progress;
            }
            let fraction = loader_fraction(&progress);
            if fraction > self.loader {
                self.loader = fraction;
            }
        } else {
            let fraction = progress
                .percent
                .filter(|value| value.is_finite())
                .map(|value| (value / 100.0).clamp(0.0, 1.0))
                .unwrap_or_else(|| file_fraction(&progress));
            if fraction > self.vanilla {
                self.vanilla = fraction;
            }
        }
        if !self.has_loaders {
            let overall = self.vanilla * 100.0;
            return finish(progress, overall, &mut self.last);
        }
        let overall = (VANILLA_WEIGHT * self.vanilla + LOADER_WEIGHT * self.loader)
            / (VANILLA_WEIGHT + LOADER_WEIGHT)
            * 100.0;
        finish(progress, overall, &mut self.last)
    }
}

fn finish(mut progress: InstallProgress, overall: f64, last: &mut f64) -> InstallProgress {
    let next = overall.clamp(*last, 100.0);
    *last = next;
    progress.percent = Some(next);
    progress
}

fn file_fraction(progress: &InstallProgress) -> f64 {
    if progress.total_files == 0 {
        return 0.0;
    }
    (progress.completed_files as f64 / progress.total_files as f64).clamp(0.0, 1.0)
}

fn loader_fraction(progress: &InstallProgress) -> f64 {
    let step = LOADER_STEPS
        .iter()
        .find(|(stage, _)| *stage == progress.stage.as_str())
        .map(|(_, at)| *at)
        .unwrap_or(0.0);
    // 同一阶段里再按文件数往前蹭一点点，避免整段进度条完全不动。
    let within = if progress.total_files > 0 {
        file_fraction(progress)
    } else {
        0.0
    };
    let next = LOADER_STEPS
        .iter()
        .find(|(stage, _)| *stage == progress.stage.as_str())
        .map(|(stage, _)| stage)
        .and_then(|stage| LOADER_STEPS.iter().position(|(name, _)| name == stage))
        .and_then(|index| LOADER_STEPS.get(index + 1))
        .map(|(_, at)| *at)
        .unwrap_or(1.0);
    (step + (next - step) * within).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(stage: &str, percent: Option<f64>) -> InstallProgress {
        InstallProgress {
            version_id: "1.21.1".into(),
            stage: stage.into(),
            path: String::new(),
            completed_files: 0,
            total_files: 0,
            downloaded_bytes: 0,
            total_bytes: 0,
            percent,
        }
    }

    #[test]
    fn vanilla_only_install_passes_the_downloader_percent_through() {
        let mut tracker = Tracker::new(false);
        let out = tracker.observe(progress("downloading", Some(50.0)));
        assert_eq!(out.percent, Some(50.0));
    }

    #[test]
    fn loader_stages_advance_ahead_of_the_vanilla_half() {
        let mut tracker = Tracker::new(true);
        let vanilla = tracker.observe(progress("downloading", Some(100.0)));
        assert_eq!(vanilla.percent, Some(80.0));
        let installing = tracker.observe(progress("modloader-install", None));
        let installing = installing.percent.unwrap();
        assert!(installing > 80.0 && installing < 100.0, "{installing}");
        let done = tracker.observe(progress("complete", None));
        assert_eq!(done.percent, Some(100.0));
    }

    #[test]
    fn percent_never_goes_backwards() {
        let mut tracker = Tracker::new(true);
        let first = tracker
            .observe(progress("modloader-install", None))
            .percent
            .unwrap();
        let second = tracker.observe(progress("downloading", Some(5.0))).percent.unwrap();
        assert!(second >= first);
    }
}
