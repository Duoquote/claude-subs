//! Locks that Claude Code respects: proper-lockfile's scheme. A lock is a directory created with
//! `mkdir` (atomic: exactly one creator wins); its holder touches its mtime every few seconds, and a
//! lock whose mtime is older than `stale` is abandoned and may be removed. Release is `rmdir`.
//!
//! Claude Code's token refresh uses `stale` 60 s (update 5 s) on `.oauth_refresh.lock` and the
//! legacy `<config dir>.lock`; its global-config saves use proper-lockfile's default, 10 s. We hold
//! these for well under a second, so we never touch the mtime.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::fsx;

#[derive(Debug)]
pub struct DirLock {
    path: PathBuf,
}

impl DirLock {
    /// Takes the lock at `path`, waiting up to `wait` for a live holder. A lock older than `stale`
    /// is removed and retaken.
    pub async fn acquire(path: &Path, stale: Duration, wait: Duration) -> Result<Self> {
        let started = Instant::now();
        loop {
            match std::fs::create_dir(path) {
                Ok(()) => {
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    if fsx::age(path).is_some_and(|age| age > stale) {
                        tracing::warn!("removing a stale lock: {}", path.display());
                        let _ = std::fs::remove_dir(path);
                        continue;
                    }
                    if started.elapsed() > wait {
                        bail!(
                            "{} is held by another process (Claude Code refreshing or saving); try again in a moment",
                            path.display()
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    let parent = path.parent().context("lock path has no parent")?;
                    std::fs::create_dir_all(parent)?;
                }
                Err(e) => return Err(e).with_context(|| format!("taking {}", path.display())),
            }
        }
    }
}

impl Drop for DirLock {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir(&self.path) {
            tracing::warn!("lock {} not released: {e}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn second_taker_waits_then_times_out_and_release_frees_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.lock");
        let held = DirLock::acquire(&path, Duration::from_secs(60), Duration::ZERO).await.unwrap();
        let second = DirLock::acquire(&path, Duration::from_secs(60), Duration::from_millis(250)).await;
        assert!(second.is_err(), "a live lock is not taken");
        drop(held);
        assert!(!path.exists(), "release removes the directory");
        let again = DirLock::acquire(&path, Duration::from_secs(60), Duration::ZERO).await;
        assert!(again.is_ok());
    }

    #[tokio::test]
    async fn a_stale_lock_is_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("y.lock");
        std::fs::create_dir(&path).unwrap();
        std::thread::sleep(Duration::from_millis(60));
        let lock = DirLock::acquire(&path, Duration::from_millis(20), Duration::from_secs(1)).await;
        assert!(lock.is_ok(), "older than `stale`: abandoned");
    }
}
