//! The saved logins: per account, the `claudeAiOauth` tokens and the `oauthAccount` block exactly as
//! Claude Code wrote them, plus the last usage reading, in `vault.dat`: sealed with DPAPI on Windows
//! (this Windows user on this machine alone can open it); elsewhere plain JSON readable by this user
//! alone (0600), which is how Claude Code itself keeps `.credentials.json` on Linux.
//!
//! One rule keeps the tokens alive: an account's refresh token lives in one place at a time. While
//! an account is live, Claude Code owns (and rotates) its tokens and the vault copy is only a mirror
//! updated from disk; while it is not live, the vault copy is the only one and only we refresh it.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::claude;
#[cfg(windows)]
use crate::dpapi;
use crate::fsx;
use crate::usage::Usage;

/// DPAPI-sealed JSON (Windows).
const SEALED: &[u8] = b"CSUB1\n";
/// Plain JSON in a 0600 file (Linux and others).
const PLAIN: &[u8] = b"CSUB0\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Ok,
    /// The saved refresh token was refused: only a new `/login` with this account brings it back.
    NeedsLogin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// `oauthAccount.accountUuid`.
    pub id: String,
    pub email: String,
    pub oauth_account: Value,
    pub creds: Value,
    pub added_at: i64,
    pub captured_at: i64,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub status: Status,
    #[serde(default)]
    pub error: Option<String>,
    /// Unix ms of the last usage request (ours) for this account.
    #[serde(default)]
    pub last_poll: i64,
    #[serde(default)]
    pub backoff_until: i64,
    #[serde(default)]
    pub backoff_ms: i64,
}

impl Account {
    pub fn refresh_token(&self) -> Option<&str> {
        claude::refresh_token(&self.creds)
    }

    pub fn access_token(&self) -> Option<&str> {
        claude::access_token(&self.creds)
    }

    pub fn expires_at(&self) -> i64 {
        claude::expires_at(&self.creds)
    }

    pub fn scopes(&self) -> Vec<String> {
        self.creds
            .get("scopes")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())
            .unwrap_or_default()
    }

    fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
        v.get(key)?.as_str().filter(|s| !s.is_empty())
    }

    pub fn org_name(&self) -> Option<String> {
        Self::str_of(&self.oauth_account, "organizationName").map(str::to_owned)
    }

    pub fn display_name(&self) -> Option<String> {
        Self::str_of(&self.oauth_account, "displayName").map(str::to_owned)
    }

    /// "Team · Max 5x", from the subscription type and rate-limit tier.
    pub fn plan(&self) -> Option<String> {
        let sub = Self::str_of(&self.creds, "subscriptionType").map(title);
        let tier = Self::str_of(&self.creds, "rateLimitTier").and_then(|t| {
            let t = t.strip_prefix("default_claude_").unwrap_or(t);
            t.strip_prefix("max_").map(|m| format!("Max {m}"))
        });
        match (sub, tier) {
            (Some(s), Some(t)) if !s.eq_ignore_ascii_case("max") => Some(format!("{s} · {t}")),
            (_, Some(t)) => Some(t),
            (Some(s), None) => Some(s),
            (None, None) => None,
        }
    }

    pub fn login_expires_at(&self) -> Option<i64> {
        self.creds.get("refreshTokenExpiresAt").and_then(Value::as_i64)
    }
}

fn title(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Vault {
    pub accounts: Vec<Account>,
    /// The account whose tokens are in `.credentials.json` (as last seen).
    pub active: Option<String>,
}

impl Vault {
    pub fn get(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Account> {
        self.accounts.iter_mut().find(|a| a.id == id)
    }

    pub fn by_refresh_token(&self, rt: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.refresh_token() == Some(rt))
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.active.as_deref() == Some(id)
    }

    /// The vault at `path`, empty when there is none. `Unreadable` is a file that is not a vault
    /// or does not open (set aside by the caller); any other error is passing (a read that failed).
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Self::open(&bytes).map_err(|e| anyhow::Error::new(Unreadable(format!("{} {e:#}", path.display()))))
    }

    fn open(bytes: &[u8]) -> Result<Self> {
        let plain = if let Some(body) = bytes.strip_prefix(PLAIN) {
            body.to_vec()
        } else if let Some(sealed) = bytes.strip_prefix(SEALED) {
            unseal(sealed)?
        } else {
            bail!("is not a vault file");
        };
        serde_json::from_slice(&plain).context("reading the vault")
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let plain = serde_json::to_vec(self)?;
        fsx::write_atomic(path, &seal(&plain)?)
    }
}

/// A vault file that will not open: not ours, damaged, or sealed for another user or machine.
#[derive(Debug)]
pub struct Unreadable(pub String);

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unreadable {}

#[cfg(windows)]
fn seal(plain: &[u8]) -> Result<Vec<u8>> {
    let mut out = SEALED.to_vec();
    out.extend(dpapi::protect(plain)?);
    Ok(out)
}

#[cfg(not(windows))]
fn seal(plain: &[u8]) -> Result<Vec<u8>> {
    let mut out = PLAIN.to_vec();
    out.extend_from_slice(plain);
    Ok(out)
}

#[cfg(windows)]
fn unseal(sealed: &[u8]) -> Result<Vec<u8>> {
    dpapi::unprotect(sealed).context("unsealing the vault (DPAPI)")
}

#[cfg(not(windows))]
fn unseal(_: &[u8]) -> Result<Vec<u8>> {
    bail!("is sealed with Windows DPAPI; it opens on the Windows machine that wrote it")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn account(id: &str, rt: &str) -> Account {
        Account {
            id: id.into(),
            email: format!("{id}@x"),
            oauth_account: json!({"accountUuid": id, "organizationName": "Org"}),
            creds: json!({"refreshToken": rt, "subscriptionType": "team", "rateLimitTier": "default_claude_max_5x"}),
            added_at: 0,
            captured_at: 0,
            usage: None,
            status: Status::Ok,
            error: None,
            last_poll: 0,
            backoff_until: 0,
            backoff_ms: 0,
        }
    }

    #[test]
    fn sealed_round_trip_and_lookups() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.dat");
        let vault = Vault {
            accounts: vec![account("A", "ra"), account("B", "rb")],
            active: Some("A".into()),
        };
        vault.save(&path).unwrap();
        let raw = std::fs::read(&path).unwrap();
        #[cfg(windows)]
        assert!(!String::from_utf8_lossy(&raw).contains("rb"), "tokens are not readable at rest");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert!(raw.starts_with(PLAIN));
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "readable by this user alone");
        }
        let back = Vault::load(&path).unwrap();
        assert_eq!(back.by_refresh_token("rb").unwrap().id, "B");
        assert!(back.is_active("A"));
        assert_eq!(back.get("A").unwrap().plan().as_deref(), Some("Team · Max 5x"));
    }
}
