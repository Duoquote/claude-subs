//! Claude Code's live login: read it, and replace it the way a sibling `/login` does.
//!
//! A running Claude Code checks `.credentials.json` before its requests (mtime, or with the v5
//! store dev:ino:size:mtime) and on a change drops its cached token and adopts what is on disk
//! ("sibling login"). So an atomic replacement of `claudeAiOauth` switches every running instance.
//! `oauthAccount` in `~/.claude.json` is who the UI says is logged in; it is replaced first.
//!
//! Ordering with Claude Code's own refresh: it holds `.oauth_refresh.lock` (and the legacy lock)
//! while it POSTs the refresh and saves the result with a compare-and-swap on the refresh token. We
//! take the same locks, so no refresh is in flight while we read the outgoing tokens and write the
//! incoming ones; its lock order (refresh lock, then the config lock) is ours too.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use crate::fsx;
use crate::lock::DirLock;
use crate::paths::Paths;

/// What is logged in right now.
#[derive(Debug, Default, Clone)]
pub struct Live {
    /// `claudeAiOauth` from `.credentials.json`.
    pub creds: Option<Value>,
    /// `oauthAccount` from `~/.claude.json`.
    pub account: Option<Value>,
    /// `cachedUsageUtilization` from `~/.claude.json`: `{fetchedAtMs, accountUuid, utilization}`.
    pub usage_cache: Option<Value>,
}

impl Live {
    pub fn refresh_token(&self) -> Option<&str> {
        self.creds.as_ref().and_then(refresh_token)
    }

    pub fn account_uuid(&self) -> Option<&str> {
        self.account.as_ref()?.get("accountUuid")?.as_str()
    }
}

pub fn refresh_token(creds: &Value) -> Option<&str> {
    creds.get("refreshToken")?.as_str().filter(|s| !s.is_empty())
}

pub fn access_token(creds: &Value) -> Option<&str> {
    creds.get("accessToken")?.as_str().filter(|s| !s.is_empty())
}

pub fn expires_at(creds: &Value) -> i64 {
    creds.get("expiresAt").and_then(Value::as_i64).unwrap_or(0)
}

pub fn read_live(paths: &Paths) -> Result<Live> {
    let creds = fsx::read_json(&paths.credentials)?
        .and_then(|mut v| v.get_mut("claudeAiOauth").map(Value::take))
        .filter(|v| v.is_object());
    let (account, usage_cache) = match fsx::read_json(&paths.global_config)? {
        Some(mut cfg) => (
            cfg.get_mut("oauthAccount").map(Value::take).filter(Value::is_object),
            cfg.get_mut("cachedUsageUtilization").map(Value::take).filter(Value::is_object),
        ),
        None => (None, None),
    };
    Ok(Live {
        creds,
        account,
        usage_cache,
    })
}

/// Claude Code's refresh locks, held for the read-outgoing / write-incoming step of a switch.
pub struct RefreshLocks {
    _current: DirLock,
    _legacy: DirLock,
}

impl RefreshLocks {
    pub async fn acquire(paths: &Paths) -> Result<Self> {
        let stale = Duration::from_secs(60);
        let wait = Duration::from_secs(45);
        let current = DirLock::acquire(&paths.refresh_lock, stale, wait).await?;
        let legacy = DirLock::acquire(&paths.legacy_refresh_lock, stale, wait).await?;
        Ok(Self {
            _current: current,
            _legacy: legacy,
        })
    }
}

/// Sets `oauthAccount` in `~/.claude.json`, every other key kept as it is, under the config lock.
pub async fn write_oauth_account(paths: &Paths, account: &Value) -> Result<()> {
    let _lock = DirLock::acquire(&paths.config_lock, Duration::from_secs(10), Duration::from_secs(20)).await?;
    let mut cfg = fsx::read_json(&paths.global_config)?
        .with_context(|| format!("{} is missing", paths.global_config.display()))?;
    let Some(obj) = cfg.as_object_mut() else {
        bail!("{} is not a JSON object", paths.global_config.display());
    };
    obj.insert("oauthAccount".into(), account.clone());
    let mut text = serde_json::to_string_pretty(&cfg)?;
    text.push('\n');
    fsx::write_atomic(&paths.global_config, text.as_bytes())
}

/// Sets `claudeAiOauth` in `.credentials.json`; other entries (MCP server logins) are kept.
pub fn write_credentials(paths: &Paths, creds: &Value) -> Result<()> {
    let mut file = fsx::read_json(&paths.credentials)?.unwrap_or_else(|| Value::Object(Map::new()));
    let Some(obj) = file.as_object_mut() else {
        bail!("{} is not a JSON object", paths.credentials.display());
    };
    obj.insert("claudeAiOauth".into(), creds.clone());
    fsx::write_atomic(&paths.credentials, serde_json::to_string(&file)?.as_bytes())
}

/// Copies `~/.claude.json` into the backups folder before we write to it; keeps the last 10.
pub fn backup_global_config(paths: &Paths) -> Result<()> {
    let dir = paths.backups();
    std::fs::create_dir_all(&dir)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    std::fs::copy(&paths.global_config, dir.join(format!("claude.json.{stamp}")))?;
    let mut old: Vec<_> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("claude.json."))
        .map(|e| e.path())
        .collect();
    old.sort();
    let excess = old.len().saturating_sub(10);
    for path in old.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde_json::json;

    pub fn sandbox() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        let paths = Paths::new(claude_dir, dir.path().join(".claude.json"), dir.path().join("data"));
        (dir, paths)
    }

    #[tokio::test]
    async fn a_switch_write_keeps_everything_else() {
        let (_dir, paths) = sandbox();
        std::fs::write(
            &paths.credentials,
            json!({"claudeAiOauth": {"refreshToken": "a"}, "mcpOAuth": {"srv": {"t": 1}}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            &paths.global_config,
            json!({"numStartups": 5, "oauthAccount": {"accountUuid": "A"}, "projects": {"x": {}}}).to_string(),
        )
        .unwrap();

        let locks = RefreshLocks::acquire(&paths).await.unwrap();
        assert!(paths.refresh_lock.is_dir() && paths.legacy_refresh_lock.is_dir());
        write_oauth_account(&paths, &json!({"accountUuid": "B"})).await.unwrap();
        write_credentials(&paths, &json!({"refreshToken": "b"})).unwrap();
        drop(locks);
        assert!(!paths.refresh_lock.exists() && !paths.legacy_refresh_lock.exists());
        assert!(!paths.config_lock.exists());

        let live = read_live(&paths).unwrap();
        assert_eq!(live.refresh_token(), Some("b"));
        assert_eq!(live.account_uuid(), Some("B"));
        let creds = fsx::read_json(&paths.credentials).unwrap().unwrap();
        assert_eq!(creds["mcpOAuth"]["srv"]["t"], 1, "MCP logins survive");
        let cfg = fsx::read_json(&paths.global_config).unwrap().unwrap();
        assert_eq!(cfg["numStartups"], 5);
        let keys: Vec<_> = cfg.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["numStartups", "oauthAccount", "projects"], "key order kept");
    }
}
