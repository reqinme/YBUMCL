//! 同一基础版本（`versions/<game>`）的安装互斥与在用计数。
//!
//! 并行安装时几个任务都会去补同一份原版底稿：谁先装、谁跳过都行，但不能同时写
//! 同一个目录。另外合并式实例发布后会把底稿删掉，这就像在别人脚下抽走地毯，
//! 所以删除前要先问一句「还有人在用吗」。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::Duration;

use crate::download::{CancellationToken, DownloadError};

/// 进程内正在跑的安装数：只有一个的时候才能安全地扫掉 `.part` 残片。
static INSTALLS_ACTIVE: AtomicUsize = AtomicUsize::new(0);

static BASE_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
static BASE_ACTIVE: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();

pub(crate) fn base_lock(game: &str) -> Arc<Mutex<()>> {
    let map = BASE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|error| error.into_inner());
    guard
        .entry(game.to_owned())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn active_map() -> MutexGuard<'static, HashMap<String, usize>> {
    BASE_ACTIVE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// 等价于 `lock.lock()`，但等待期间一直看取消标记：等别人装底稿时也能按暂停 / 取消走人。
pub(crate) fn wait_lock<'a>(
    lock: &'a Mutex<()>,
    cancel: &CancellationToken,
) -> std::result::Result<MutexGuard<'a, ()>, DownloadError> {
    loop {
        match lock.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
            Err(TryLockError::WouldBlock) => {
                if cancel.is_cancelled() {
                    return Err(DownloadError::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(120));
            }
        }
    }
}

/// 这个基础版本是否还有别的安装任务在用（不含自己）。
pub(crate) fn base_busy(game: &str) -> bool {
    active_map().get(game).copied().unwrap_or(0) > 1
}

pub(crate) fn installs_active() -> usize {
    INSTALLS_ACTIVE.load(Ordering::Relaxed)
}

/// 「我正在用 `versions/<game>`」，离开作用域时自动减一。
pub(crate) struct BaseLease(String);

impl BaseLease {
    pub(crate) fn new(game: &str) -> Self {
        *active_map().entry(game.to_owned()).or_insert(0) += 1;
        Self(game.to_owned())
    }
}

impl Drop for BaseLease {
    fn drop(&mut self) {
        let mut map = active_map();
        if let Some(count) = map.get_mut(&self.0) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                map.remove(&self.0);
            }
        }
    }
}

/// 安装计数：只有最后一个安装收尾时才允许扫残片。
pub(crate) struct InstallLease;

impl InstallLease {
    pub(crate) fn new() -> Self {
        INSTALLS_ACTIVE.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for InstallLease {
    fn drop(&mut self) {
        INSTALLS_ACTIVE.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leases_count_concurrent_users_of_the_same_shell() {
        let id = "neoforge-21.11.48-test";
        assert!(!base_busy(id));
        let first = BaseLease::new(id);
        assert!(!base_busy(id), "只有一个使用时不算忙");
        let second = BaseLease::new(id);
        assert!(base_busy(id), "两个任务同时用同一个外壳才算忙");
        drop(second);
        assert!(!base_busy(id));
        drop(first);
        assert!(!base_busy(id));
    }

    #[test]
    fn waiting_for_a_shell_lock_can_be_cancelled() {
        let lock = base_lock("fabric-loader-0.20.1-1.21.11-test");
        let guard = lock.lock().unwrap();
        let token = CancellationToken::default();
        token.cancel();
        assert!(matches!(
            wait_lock(&lock, &token),
            Err(DownloadError::Cancelled)
        ));
        drop(guard);
    }
}
