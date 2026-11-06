//! The engine: keeps the vault in step with Claude Code's live login, reads each account's usage,
//! and switches the live login.
//!
//! * Capture. `.credentials.json` and `~/.claude.json` are watched (stat every 2 s). Tokens we have
//!   not seen are either Claude Code's refresh of the live account or a `/login`; the profile
//!   endpoint says whose they are, and they are saved under that account. So adding an account is
//!   just `/login` in any Claude Code window, and the vault always has the live account's latest
//!   tokens before anything can overwrite them.
//! * Usage. The live account: Claude Code's own cached reading when newer, else ours with its
//!   current access token (never refreshed by us). Idle accounts: their vault tokens, refreshed by
//!   us when expired (which also keeps their 30-day refresh tokens alive).
//! * Switch. Under Claude Code's refresh locks: save the outgoing tokens, then write the incoming
//!   `oauthAccount` and `claudeAiOauth` (see `claude.rs`).
//!
//! More than one process may run an engine: the tray app, and the command line (which the Claude
//! Code mod runs). Each change to the vault is made under `vault.lock` (a mkdir lock) on a copy
//! read again from disk when another process saved since, so no change is lost and an idle account
//! is never refreshed by two processes at once. Every snapshot is also written to `status.json`,
//! which the mod reads.

use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify, watch};

use crate::api::{self, Api, ApiError};
use crate::claude::{self, Live};
use crate::fsx;
use crate::lock::DirLock;
use crate::paths::Paths;
use crate::usage::{self, Usage, Window};
use crate::vault::{Account, Status, Unreadable, Vault};
use crate::when;

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Our own usage read for the live account, at most this often (Claude Code's cached reading
    /// counts too).
    pub poll_active_secs: u64,
    /// Usage reads for the other accounts.
    pub poll_idle_secs: u64,
    /// Refresh an idle account's expired access token to read its usage (and keep it alive).
    pub refresh_idle: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_active_secs: 300,
            poll_idle_secs: 900,
            refresh_idle: true,
        }
    }
}

impl Settings {
    /// `settings.json` in the data dir; written with the defaults when missing, so it can be edited.
    pub fn load(paths: &Paths) -> Self {
        let path = paths.settings();
        match fsx::read_json(&path) {
            Ok(Some(v)) => serde_json::from_value(v).unwrap_or_default(),
            Ok(None) => {
                let s = Self::default();
                if let Ok(text) = serde_json::to_string_pretty(&s) {
                    let _ = fsx::write_atomic(&path, text.as_bytes());
                }
                s
            }
            Err(e) => {
                tracing::warn!("settings unreadable, defaults used: {e:#}");
                Self::default()
            }
        }
    }
}

/// What the tray and the popover show.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Snapshot {
    pub active: Option<String>,
    pub accounts: Vec<AccountView>,
    /// The last switch failure, or a warning about the vault.
    pub notice: Option<String>,
    pub at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    pub id: String,
    pub email: String,
    /// The email's local part ("side"), or the whole email when two accounts share it.
    pub short: String,
    pub name: Option<String>,
    pub org: Option<String>,
    pub plan: Option<String>,
    pub active: bool,
    pub status: Status,
    pub error: Option<String>,
    pub session: Option<WindowView>,
    pub week: Option<WindowView>,
    pub scoped: Vec<WindowView>,
    pub usage_at: Option<i64>,
    pub usage_source: Option<String>,
    pub login_expires_at: Option<i64>,
}

/// A usage window as shown: its reset time already worded (see `when.rs`). Rebuilt every minute.
#[derive(Debug, Clone, Serialize)]
pub struct WindowView {
    pub label: String,
    pub percent: f64,
    pub resets_at: Option<i64>,
    /// "in 2h 05m", "tomorrow 11:00", "Sat 11:00".
    pub resets: Option<String>,
    /// "today 13:05", "Sat 7 Nov, 11:00": for hover.
    pub resets_exact: Option<String>,
}

impl WindowView {
    fn of(w: &Window, now: i64, clock: when::Clock) -> Self {
        Self {
            label: w.label.clone(),
            percent: w.percent,
            resets_at: w.resets_at,
            resets: w.resets_at.map(|at| when::reset_label(at, now, clock)),
            resets_exact: w.resets_at.map(|at| when::reset_exact(at, now, clock)),
        }
    }
}

#[cfg_attr(not(windows), allow(dead_code))] // the tray app's
impl Snapshot {
    pub fn active_view(&self) -> Option<&AccountView> {
        self.accounts.iter().find(|a| a.active)
    }
}

#[cfg_attr(not(windows), allow(dead_code))] // the tray app's
impl AccountView {
    pub fn headline(&self) -> Option<f64> {
        match (&self.session, &self.week) {
            (Some(s), Some(w)) => Some(s.percent.max(w.percent)),
            (Some(x), None) | (None, Some(x)) => Some(x.percent),
            (None, None) => None,
        }
    }
}

#[derive(Debug, Clone)]
struct Ident {
    uuid: String,
    email: String,
    org_uuid: Option<String>,
}

/// Which process runs the engine: said in `status.json`, so the mod knows whether anything keeps
/// the readings fresh without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The Windows tray app: watches and polls on its own.
    #[cfg_attr(not(windows), allow(dead_code))]
    Tray,
    /// One command, then exit.
    Command,
}

/// The in-memory vault and the disk stamp of the file it was read from.
struct Held {
    vault: Vault,
    seen: Option<(u128, u64)>,
}

/// The vault while this process may change it: the in-process mutex and `vault.lock`.
pub struct VaultGuard<'a> {
    held: tokio::sync::MutexGuard<'a, Held>,
    _lock: DirLock,
}

impl Deref for VaultGuard<'_> {
    type Target = Vault;
    fn deref(&self) -> &Vault {
        &self.held.vault
    }
}

impl DerefMut for VaultGuard<'_> {
    fn deref_mut(&mut self) -> &mut Vault {
        &mut self.held.vault
    }
}

/// The longest a holder keeps `vault.lock`: a token refresh (20 s timeout) plus Claude Code's
/// refresh locks (45 s at most) during a switch, with room to spare. An older lock is abandoned.
const VAULT_LOCK_STALE: Duration = Duration::from_secs(180);
const VAULT_LOCK_WAIT: Duration = Duration::from_secs(150);

/// What `status.json` holds: the snapshot, and how to reach the binary that wrote it.
#[derive(Serialize)]
struct StatusFile<'a> {
    format: u32,
    app_version: &'a str,
    driver: Role,
    exe: Option<PathBuf>,
    #[serde(flatten)]
    snapshot: &'a Snapshot,
}

pub struct Engine {
    pub paths: Paths,
    api: Api,
    settings: Settings,
    role: Role,
    vault: Mutex<Held>,
    tx: watch::Sender<Arc<Snapshot>>,
    poke: Notify,
    notice: std::sync::Mutex<Option<String>>,
    /// The last profile answer, by refresh token: the watcher retries without asking again.
    identified: std::sync::Mutex<Option<(String, Ident)>>,
}

impl Engine {
    pub fn new(paths: Paths, role: Role) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&paths.data_dir)?;
        let mut notice = None;
        let seen = fsx::stamp(&paths.vault());
        let vault = match load_vault(&paths) {
            Ok(v) => v,
            Err(e) if e.downcast_ref::<Unreadable>().is_some() => {
                // Never overwrite a vault we cannot read: set it aside and start over.
                let aside = paths.vault().with_extension(format!("unreadable-{}", now_ms()));
                let _ = std::fs::rename(paths.vault(), &aside);
                tracing::error!("vault unreadable ({e:#}); moved to {}", aside.display());
                notice = Some(format!("The saved logins could not be read and were set aside ({}).", aside.display()));
                Vault::default()
            }
            Err(e) => return Err(e),
        };
        let settings = Settings::load(&paths);
        let version = claude_version(&paths, role);
        tracing::info!(
            "{role:?}: Claude Code {version}; data in {}; clock {:?}",
            paths.data_dir.display(),
            when::Clock::system()
        );
        let (tx, _) = watch::channel(Arc::new(Snapshot::default()));
        let engine = Arc::new(Self {
            api: Api::new(&version),
            paths,
            settings,
            role,
            vault: Mutex::new(Held { vault: Vault::default(), seen }),
            tx,
            poke: Notify::new(),
            notice: std::sync::Mutex::new(notice),
            identified: std::sync::Mutex::new(None),
        });
        let snap = engine.snapshot(&vault);
        engine.tx.send_replace(Arc::new(snap));
        engine.vault.try_lock().expect("nobody else has the engine yet").vault = vault;
        Ok(engine)
    }

    /// The vault to read: the in-memory copy, read again first when another process saved since.
    async fn peek(&self) -> tokio::sync::MutexGuard<'_, Held> {
        let mut held = self.vault.lock().await;
        self.reload_if_changed(&mut held);
        held
    }

    /// The vault to change: also holds `vault.lock`, so the copy stays the latest until dropped.
    /// Every change made through it is saved before it is dropped.
    async fn vault(&self) -> Result<VaultGuard<'_>> {
        let mut held = self.vault.lock().await;
        let lock = DirLock::acquire(&self.paths.vault_lock(), VAULT_LOCK_STALE, VAULT_LOCK_WAIT)
            .await
            .context("waiting for the vault (another claude-subs process holds it)")?;
        self.reload_if_changed(&mut held);
        Ok(VaultGuard { held, _lock: lock })
    }

    fn reload_if_changed(&self, held: &mut Held) {
        let now = fsx::stamp(&self.paths.vault());
        if now == held.seen {
            return;
        }
        match load_vault(&self.paths) {
            Ok(v) => {
                held.vault = v;
                held.seen = now;
            }
            // Keep what we have; the next look tries again.
            Err(e) => tracing::warn!("vault not read again: {e:#}"),
        }
    }

    /// The id of the saved account `who` names: its id, its email, its email's local part, or its
    /// place in the list (from 1).
    pub async fn resolve(&self, who: &str) -> Option<String> {
        let held = self.peek().await;
        let accounts = &held.vault.accounts;
        let who = who.trim();
        let lower = who.to_lowercase();
        if let Ok(n) = who.parse::<usize>()
            && n >= 1
            && n <= accounts.len()
        {
            return Some(accounts[n - 1].id.clone());
        }
        let exact = accounts
            .iter()
            .find(|a| a.id == who || a.email.to_lowercase() == lower)
            .map(|a| a.id.clone());
        exact.or_else(|| {
            let mut by_local = accounts
                .iter()
                .filter(|a| a.email.split('@').next().is_some_and(|l| l.to_lowercase() == lower));
            match (by_local.next(), by_local.next()) {
                (Some(a), None) => Some(a.id.clone()),
                _ => None,
            }
        })
    }

    #[cfg_attr(not(windows), allow(dead_code))] // the tray app's
    pub fn subscribe(&self) -> watch::Receiver<Arc<Snapshot>> {
        self.tx.subscribe()
    }

    pub fn current(&self) -> Arc<Snapshot> {
        self.tx.borrow().clone()
    }

    /// Starts the watcher and the usage poller on the current tokio runtime.
    #[cfg_attr(not(windows), allow(dead_code))] // the tray app's
    pub fn start(self: &Arc<Self>) {
        tokio::spawn(self.clone().watch_loop());
        tokio::spawn(self.clone().poll_loop());
    }

    /// Ask for fresh usage readings now (the popover's refresh button).
    #[cfg_attr(not(windows), allow(dead_code))] // the tray app's
    pub fn refresh_now(&self) {
        self.poke.notify_one();
    }

    /// Rebuilds the snapshot (a reset time may have passed) and publishes it.
    pub async fn republish(&self) {
        let held = self.peek().await;
        self.publish(&held.vault);
    }

    /// The snapshot to the tray (through `subscribe`) and to `status.json`.
    fn publish(&self, v: &Vault) {
        let snap = self.snapshot(v);
        let file = StatusFile {
            format: 1,
            app_version: env!("CARGO_PKG_VERSION"),
            driver: self.role,
            exe: std::env::current_exe().ok(),
            snapshot: &snap,
        };
        match serde_json::to_vec_pretty(&file) {
            Ok(bytes) => {
                if let Err(e) = fsx::write_atomic(&self.paths.status(), &bytes) {
                    tracing::warn!("status.json not written: {e:#}");
                }
            }
            Err(e) => tracing::warn!("status.json not built: {e}"),
        }
        self.tx.send_replace(Arc::new(snap));
    }

    fn save_and_publish(&self, v: &Vault) {
        if let Err(e) = v.save(&self.paths.vault()) {
            tracing::error!("vault not saved: {e:#}");
        }
        self.publish(v);
    }

    fn snapshot(&self, v: &Vault) -> Snapshot {
        let now = now_ms();
        let clock = when::Clock::system();
        let view = |w: &Window| WindowView::of(w, now, clock);
        let local = |email: &str| email.split('@').next().unwrap_or(email).to_owned();
        let shared = |a: &Account| v.accounts.iter().filter(|b| local(&b.email) == local(&a.email)).count() > 1;
        let accounts = v
            .accounts
            .iter()
            .map(|a| {
                let u: Option<Usage> = a.usage.as_ref().map(|u| u.as_of(now));
                AccountView {
                    id: a.id.clone(),
                    email: a.email.clone(),
                    short: if shared(a) { a.email.clone() } else { local(&a.email) },
                    name: a.display_name(),
                    org: a.org_name(),
                    plan: a.plan(),
                    active: v.is_active(&a.id),
                    status: a.status,
                    error: a.error.clone(),
                    session: u.as_ref().and_then(|u| u.session.as_ref().map(view)),
                    week: u.as_ref().and_then(|u| u.week.as_ref().map(view)),
                    scoped: u.as_ref().map(|u| u.scoped.iter().map(view).collect()).unwrap_or_default(),
                    usage_at: u.as_ref().map(|u| u.fetched_at),
                    usage_source: u.as_ref().map(|u| u.source.clone()),
                    login_expires_at: a.login_expires_at(),
                }
            })
            .collect();
        Snapshot {
            active: v.active.clone(),
            accounts,
            notice: self.notice.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            at: now,
        }
    }

    fn set_notice(&self, notice: Option<String>) {
        *self.notice.lock().unwrap_or_else(|p| p.into_inner()) = notice;
    }

    // ------------------------------------------------------------------ capture --

    #[cfg_attr(not(windows), allow(dead_code))] // the tray app's
    async fn watch_loop(self: Arc<Self>) {
        let stamps = |p: &Paths| (fsx::stamp(&p.credentials), fsx::stamp(&p.global_config));
        let mut last = stamps(&self.paths);
        // Another process (a command the mod ran) changed the vault: show it.
        let mut vault_seen = fsx::stamp(&self.paths.vault());
        if let Err(e) = self.sync(true).await {
            tracing::warn!("first sync: {e:#}");
        }
        // (first change, last change) while a change waits to be taken in.
        let mut pending: Option<(Instant, Instant)> = None;
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tick.tick().await;
            let vault_now = fsx::stamp(&self.paths.vault());
            if vault_now != vault_seen {
                vault_seen = vault_now;
                self.republish().await;
            }
            let now = stamps(&self.paths);
            if now != last {
                last = now;
                let t = Instant::now();
                pending = Some((pending.map_or(t, |(first, _)| first), t));
            }
            let Some((first, latest)) = pending else {
                continue;
            };
            let quiet = latest.elapsed();
            // A /login writes the tokens, then `oauthAccount` a moment later: wait for a quiet spell.
            if quiet < Duration::from_secs(3) && first.elapsed() < Duration::from_secs(15) {
                continue;
            }
            let settled = quiet >= Duration::from_secs(15) || first.elapsed() >= Duration::from_secs(60);
            match self.sync(settled).await {
                Ok(true) => pending = None,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!("sync: {e:#}");
                    pending = None;
                }
            }
        }
    }

    /// Takes in the live login. `false`: not decided yet (tried again on the next tick).
    async fn sync(&self, settled: bool) -> Result<bool> {
        let live = claude::read_live(&self.paths)?;
        let mut v = self.vault().await?;
        let done = self.absorb(&mut v, &live, settled).await;
        self.save_and_publish(&v);
        Ok(done)
    }

    /// One look at the live login, for a command: a `/login` still being written (a file changed
    /// in the last 15 s) is left for the next look.
    pub async fn sync_now(&self) -> Result<bool> {
        let quiet = |p: &std::path::Path| fsx::age(p).is_none_or(|age| age >= Duration::from_secs(15));
        let settled = quiet(&self.paths.credentials) && quiet(&self.paths.global_config);
        self.sync(settled).await
    }

    async fn absorb(&self, v: &mut Vault, live: &Live, settled: bool) -> bool {
        let now = now_ms();
        let Some(creds) = live.creds.as_ref() else {
            v.active = None;
            return true;
        };
        let Some(rt) = claude::refresh_token(creds) else {
            // Claude Code empties the tokens of a login whose refresh was refused.
            if let Some(id) = v.active.take()
                && let Some(a) = v.get_mut(&id)
            {
                a.status = Status::NeedsLogin;
                a.error = Some("Claude Code dropped this login. Run /login with this account to save it again.".into());
            }
            return true;
        };

        if let Some(id) = v.by_refresh_token(rt).map(|a| a.id.clone()) {
            let a = v.get_mut(&id).expect("found above");
            if a.creds != *creds {
                a.creds = creds.clone();
                a.captured_at = now;
            }
            if live.account_uuid() == Some(id.as_str())
                && let Some(acc) = &live.account
            {
                a.oauth_account = acc.clone();
            }
            v.active = Some(id.clone());
            absorb_usage_cache(v, live, &id);
            return true;
        }

        // Tokens we have not seen: Claude Code refreshed the live account, or someone ran /login.
        let Some(access) = claude::access_token(creds) else {
            return true;
        };
        let claimed = live.account_uuid().map(str::to_owned);
        let (id, email, oauth_account) = match self.identify(rt, access).await {
            Some(p) if claimed.as_deref() == Some(p.uuid.as_str()) => {
                (p.uuid, p.email, live.account.clone().unwrap_or_default())
            }
            // ~/.claude.json does not name this account yet: a /login is still being written.
            Some(_) if !settled => return false,
            Some(p) => {
                let block = v.get(&p.uuid).map(|a| a.oauth_account.clone()).unwrap_or_else(|| {
                    json!({"accountUuid": p.uuid, "emailAddress": p.email, "organizationUuid": p.org_uuid})
                });
                (p.uuid, p.email, block)
            }
            None if !settled => return false,
            // Profile unreachable: trust ~/.claude.json, which has been quiet for a while.
            None => {
                let (Some(uuid), Some(acc)) = (claimed, live.account.clone()) else {
                    return true;
                };
                let email = acc.get("emailAddress").and_then(Value::as_str).unwrap_or_default().to_owned();
                (uuid, email, acc)
            }
        };

        match v.get_mut(&id) {
            Some(a) => {
                a.creds = creds.clone();
                a.oauth_account = oauth_account;
                if !email.is_empty() {
                    a.email = email;
                }
                a.captured_at = now;
                a.status = Status::Ok;
                a.error = None;
                a.backoff_until = 0;
            }
            None => {
                tracing::info!("saved a new account: {email}");
                v.accounts.push(Account {
                    id: id.clone(),
                    email,
                    oauth_account,
                    creds: creds.clone(),
                    added_at: now,
                    captured_at: now,
                    usage: None,
                    status: Status::Ok,
                    error: None,
                    last_poll: 0,
                    backoff_until: 0,
                    backoff_ms: 0,
                });
            }
        }
        v.active = Some(id.clone());
        absorb_usage_cache(v, live, &id);
        true
    }

    /// Whose tokens these are, from the profile endpoint (remembered per refresh token).
    async fn identify(&self, rt: &str, access: &str) -> Option<Ident> {
        if let Some((known, ident)) = self.identified.lock().unwrap_or_else(|p| p.into_inner()).clone()
            && known == rt
        {
            return Some(ident);
        }
        let profile = match self.api.profile(access).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("profile check failed: {e}");
                return None;
            }
        };
        let text = |ptr: &str| profile.pointer(ptr).and_then(Value::as_str).map(str::to_owned);
        let ident = Ident {
            uuid: text("/account/uuid")?,
            email: text("/account/email").unwrap_or_default(),
            org_uuid: text("/organization/uuid"),
        };
        *self.identified.lock().unwrap_or_else(|p| p.into_inner()) = Some((rt.to_owned(), ident.clone()));
        Some(ident)
    }

    // -------------------------------------------------------------------- usage --

    #[cfg_attr(not(windows), allow(dead_code))] // the tray app's
    async fn poll_loop(self: Arc<Self>) {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let mut force = false;
        loop {
            self.poll_due(force).await;
            force = tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(30)) => false,
                _ = self.poke.notified() => true,
            };
        }
    }

    /// Reads the usage of every account that is due (all of them, at most every 30 s, with
    /// `force`).
    pub async fn poll_due(&self, force: bool) {
        let now = now_ms();
        let due: Vec<String> = {
            let held = self.peek().await;
            let v = &held.vault;
            v.accounts
                .iter()
                .filter(|a| {
                    if a.status == Status::NeedsLogin || a.backoff_until > now {
                        return false;
                    }
                    if force {
                        return now - a.last_poll > 30_000;
                    }
                    let every = if v.is_active(&a.id) {
                        self.settings.poll_active_secs
                    } else {
                        self.settings.poll_idle_secs
                    };
                    let latest = a.last_poll.max(a.usage.as_ref().map_or(0, |u| u.fetched_at));
                    now - latest >= i64::try_from(every).unwrap_or(i64::MAX).saturating_mul(1000)
                })
                .map(|a| a.id.clone())
                .collect()
        };
        for id in due {
            if let Err(e) = self.poll_one(&id).await {
                tracing::warn!("usage read skipped: {e:#}");
            }
        }
    }

    async fn poll_one(&self, id: &str) -> Result<()> {
        let now = now_ms();
        let (token, active) = {
            let mut v = self.vault().await?;
            let active = v.is_active(id);
            let Some(a) = v.get_mut(id) else {
                return Ok(());
            };
            // Another process read it a moment ago.
            if now - a.last_poll < 30_000 {
                return Ok(());
            }
            a.last_poll = now;
            let rt = a.refresh_token().map(str::to_owned);
            let token = if active {
                // Claude Code owns the live tokens: use its current access token, never refresh it.
                let live = claude::read_live(&self.paths)?;
                live.creds
                    .filter(|c| claude::refresh_token(c) == rt.as_deref())
                    .filter(|c| claude::expires_at(c) > now + 60_000)
                    .and_then(|c| claude::access_token(&c).map(str::to_owned))
            } else {
                if a.expires_at() < now + 5 * 60_000 {
                    if !self.settings.refresh_idle {
                        self.save_and_publish(&v);
                        return Ok(());
                    }
                    if let Err(e) = self.refresh_idle(&mut v, id).await {
                        self.save_and_publish(&v);
                        return Err(anyhow!("refresh: {e}"));
                    }
                }
                v.get(id).and_then(|a| a.access_token().map(str::to_owned))
            };
            self.save_and_publish(&v);
            (token, active)
        };
        let Some(token) = token else {
            return Ok(());
        };
        let answer = self.api.usage(&token).await;

        let mut v = self.vault().await?;
        let Some(a) = v.get_mut(id) else {
            return Ok(());
        };
        let now = now_ms();
        match answer {
            Ok(val) => {
                a.usage = Some(usage::parse(&val, now, "api"));
                a.error = None;
                a.backoff_ms = 0;
            }
            Err(ApiError::RateLimited(after)) => {
                a.backoff_ms = (a.backoff_ms * 2).clamp(5 * 60_000, 60 * 60_000);
                let wait = a.backoff_ms.max(after.map_or(0, |s| i64::try_from(s).unwrap_or(0) * 1000));
                a.backoff_until = now + wait;
                tracing::info!("usage endpoint rate limited; next try in {} min", wait / 60_000);
            }
            Err(ApiError::Unauthorized) if !active => {
                // Force a refresh before the next read.
                a.creds["expiresAt"] = json!(0);
            }
            Err(ApiError::Unauthorized) => {}
            Err(ApiError::Revoked) if !active => {
                a.status = Status::NeedsLogin;
                a.error = Some("This login was revoked. Run /login with this account to save it again.".into());
            }
            Err(e) => {
                a.error = Some(format!("Usage not read: {e}"));
                a.backoff_ms = (a.backoff_ms * 2).clamp(60_000, 30 * 60_000);
                a.backoff_until = now + a.backoff_ms;
            }
        }
        self.save_and_publish(&v);
        Ok(())
    }

    /// Refreshes an idle account's tokens (they are ours alone) and saves the vault at once: the
    /// old refresh token is spent the moment the server answers.
    async fn refresh_idle(&self, v: &mut Vault, id: &str) -> Result<(), ApiError> {
        let live_rt = claude::read_live(&self.paths)
            .ok()
            .and_then(|l| l.refresh_token().map(str::to_owned));
        let a = v.get(id).ok_or_else(|| ApiError::Http(0, "unknown account".into()))?;
        let rt = a.refresh_token().ok_or(ApiError::InvalidGrant)?.to_owned();
        if live_rt.as_deref() == Some(rt.as_str()) {
            // These tokens are live in Claude Code after all: never rotate them under it.
            v.active = Some(id.to_owned());
            return Err(ApiError::Http(0, "the account is live; Claude Code refreshes it".into()));
        }
        let scopes = a.scopes();
        let answer = self.api.refresh(&rt, &scopes).await;
        let a = v.get_mut(id).expect("checked above");
        match answer {
            Ok(resp) => {
                let now = now_ms();
                match api::merge_refreshed(&a.creds, &resp, now) {
                    Some(creds) => {
                        a.creds = creds;
                        a.captured_at = now;
                        a.status = Status::Ok;
                        a.error = None;
                    }
                    None => tracing::error!("a refresh answer without tokens"),
                }
                if let Err(e) = v.save(&self.paths.vault()) {
                    tracing::error!("vault not saved after a refresh: {e:#}");
                }
                Ok(())
            }
            Err(e @ (ApiError::InvalidGrant | ApiError::Revoked)) => {
                a.status = Status::NeedsLogin;
                a.error = Some("The saved login expired or was revoked. Run /login with this account to save it again.".into());
                Err(e)
            }
            Err(e) => {
                a.error = Some(format!("Login not refreshed: {e}"));
                Err(e)
            }
        }
    }

    // ------------------------------------------------------------------- switch --

    pub async fn switch(&self, id: &str) -> Result<()> {
        let result = match self.vault().await {
            Ok(mut v) => {
                let result = self.switch_inner(&mut v, id).await;
                match &result {
                    Ok(()) => self.set_notice(None),
                    Err(e) => {
                        tracing::warn!("switch failed: {e:#}");
                        self.set_notice(Some(format!("Switch failed: {e:#}")));
                    }
                }
                self.save_and_publish(&v);
                result
            }
            Err(e) => {
                self.set_notice(Some(format!("Switch failed: {e:#}")));
                self.republish().await;
                Err(e)
            }
        };
        if result.is_ok() {
            self.poke.notify_one();
        }
        result
    }

    async fn switch_inner(&self, v: &mut VaultGuard<'_>, id: &str) -> Result<()> {
        // Take in anything new first (a refresh or a /login since the last tick).
        let live = claude::read_live(&self.paths)?;
        self.absorb(v, &live, true).await;
        if v.is_active(id) {
            return Ok(());
        }
        let target = v.get(id).context("that account is not saved")?.clone();
        if target.status == Status::NeedsLogin {
            bail!("{} needs a fresh /login before it can be used", target.email);
        }
        if target.expires_at() < now_ms() + 10 * 60_000 {
            self.refresh_idle(v, id)
                .await
                .map_err(|e| anyhow!("refreshing {}'s login: {e}", target.email))?;
        }
        let target = v.get(id).context("that account is not saved")?.clone();

        let locks = claude::RefreshLocks::acquire(&self.paths).await?;
        // No refresh is in flight now: what is on disk is final for the outgoing account.
        let live = claude::read_live(&self.paths)?;
        self.absorb(v, &live, true).await;
        if v.is_active(id) {
            return Ok(());
        }
        v.save(&self.paths.vault()).context("saving the outgoing login")?;
        claude::backup_global_config(&self.paths).context("backing up ~/.claude.json")?;
        claude::write_oauth_account(&self.paths, &target.oauth_account).await?;
        if let Err(e) = claude::write_credentials(&self.paths, &target.creds) {
            if let Some(prev) = &live.account
                && let Err(undo) = claude::write_oauth_account(&self.paths, prev).await
            {
                tracing::error!("oauthAccount not restored after a failed switch: {undo:#}");
            }
            return Err(e.context("writing .credentials.json"));
        }
        drop(locks);

        tracing::info!("switched to {}", target.email);
        v.active = Some(id.to_owned());
        if let Some(a) = v.get_mut(id) {
            a.last_poll = 0;
        }
        Ok(())
    }

    /// Drops a saved account (not the live one).
    pub async fn forget(&self, id: &str) -> Result<()> {
        let mut v = self.vault().await?;
        if v.is_active(id) {
            bail!("this is the live login; switch to another account first");
        }
        v.accounts.retain(|a| a.id != id);
        self.save_and_publish(&v);
        Ok(())
    }
}

/// Claude Code's own cached usage reading for the live account, when newer than ours.
fn absorb_usage_cache(v: &mut Vault, live: &Live, id: &str) {
    let Some(cache) = &live.usage_cache else {
        return;
    };
    if cache.get("accountUuid").and_then(Value::as_str) != Some(id) {
        return;
    }
    let fetched = cache.get("fetchedAtMs").and_then(Value::as_i64).unwrap_or(0);
    let Some(util) = cache.get("utilization").filter(|u| u.is_object()) else {
        return;
    };
    if let Some(a) = v.get_mut(id)
        && a.usage.as_ref().is_none_or(|u| u.fetched_at < fetched)
    {
        a.usage = Some(usage::parse(util, fetched, "claude"));
    }
}

/// The vault on disk; a failed read (not a damaged file) is tried again for a moment.
fn load_vault(paths: &Paths) -> Result<Vault> {
    let mut tries = 0;
    loop {
        match Vault::load(&paths.vault()) {
            Err(e) if e.downcast_ref::<Unreadable>().is_none() && tries < 5 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

/// The installed Claude Code version, for the User-Agent: the updater's last result, else
/// `claude --version` (remembered for a day: a command runs often), else a recent version.
fn claude_version(paths: &Paths, role: Role) -> String {
    let from_update = fsx::read_json(&paths.claude_dir.join(".last-update-result.json"))
        .ok()
        .flatten()
        .and_then(|v| v.get("version_to").and_then(Value::as_str).map(str::to_owned));
    if let Some(v) = from_update {
        return v;
    }
    let cache = paths.data_dir.join("claude-version.txt");
    if role == Role::Command
        && fsx::age(&cache).is_some_and(|age| age < Duration::from_secs(86_400))
        && let Ok(text) = std::fs::read_to_string(&cache)
        && !text.trim().is_empty()
    {
        return text.trim().to_owned();
    }
    let found = ask_claude_version();
    if let Some(v) = &found {
        let _ = fsx::write_atomic(&cache, v.as_bytes());
    }
    found.unwrap_or_else(|| "2.1.319".to_owned())
}

fn ask_claude_version() -> Option<String> {
    let mut cmd = std::process::Command::new("claude");
    cmd.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split_whitespace().next().map(str::to_owned))
        .filter(|s| s.chars().next().is_some_and(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::tests::sandbox;

    fn creds(rt: &str) -> Value {
        json!({"accessToken": format!("at-{rt}"), "refreshToken": rt,
               "expiresAt": now_ms() + 8 * 3_600_000, "scopes": ["user:inference"],
               "subscriptionType": "team", "rateLimitTier": "default_claude_max_5x"})
    }

    fn write_live(paths: &Paths, rt: &str, uuid: &str) {
        std::fs::write(&paths.credentials, json!({"claudeAiOauth": creds(rt)}).to_string()).unwrap();
        std::fs::write(
            &paths.global_config,
            json!({"numStartups": 1, "oauthAccount": {"accountUuid": uuid, "emailAddress": format!("{uuid}@x")}})
                .to_string(),
        )
        .unwrap();
    }

    /// A switch between two saved accounts, offline: the tokens are known, so no request is made.
    #[tokio::test]
    async fn switching_swaps_the_live_login_and_keeps_the_outgoing_tokens() {
        let (_dir, paths) = sandbox();
        write_live(&paths, "rt-a", "A");
        let engine = Engine::new(paths.clone(), Role::Tray).unwrap();
        {
            let v = &mut engine.vault.lock().await.vault;
            for (id, rt) in [("A", "rt-a"), ("B", "rt-b")] {
                v.accounts.push(Account {
                    id: id.into(),
                    email: format!("{id}@x"),
                    oauth_account: json!({"accountUuid": id, "emailAddress": format!("{id}@x")}),
                    creds: creds(rt),
                    added_at: 0,
                    captured_at: 0,
                    usage: None,
                    status: Status::Ok,
                    error: None,
                    last_poll: 0,
                    backoff_until: 0,
                    backoff_ms: 0,
                });
            }
        }
        // Claude Code refreshed A since we last looked: the switch must save A's new tokens.
        let mut refreshed = creds("rt-a2");
        refreshed["accessToken"] = json!("at-a2");
        {
            let v = &mut engine.vault.lock().await.vault;
            v.active = Some("A".into());
            // Pretend the profile check already ran for the new tokens.
            *engine.identified.lock().unwrap() = Some((
                "rt-a2".into(),
                Ident { uuid: "A".into(), email: "A@x".into(), org_uuid: None },
            ));
        }
        std::fs::write(&paths.credentials, json!({"claudeAiOauth": refreshed, "mcpOAuth": {"k": 1}}).to_string())
            .unwrap();

        engine.switch("B").await.unwrap();

        let live = claude::read_live(&paths).unwrap();
        assert_eq!(live.refresh_token(), Some("rt-b"));
        assert_eq!(live.account_uuid(), Some("B"));
        let held = engine.vault.lock().await;
        let v = &held.vault;
        assert_eq!(v.get("A").unwrap().refresh_token(), Some("rt-a2"), "the outgoing account's latest tokens");
        assert!(v.is_active("B"));
        assert!(!paths.refresh_lock.exists() && !paths.config_lock.exists(), "locks released");
        let file = fsx::read_json(&paths.credentials).unwrap().unwrap();
        assert_eq!(file["mcpOAuth"]["k"], 1);
        let status = fsx::read_json(&paths.status()).unwrap().unwrap();
        assert_eq!(status["active"], "B", "the mod's status file follows");
        assert!(!status.to_string().contains("rt-"), "no tokens in the status file");
        assert!(!paths.vault_lock().exists(), "vault lock released");
        drop(held);

        // And back.
        engine.switch("A").await.unwrap();
        assert_eq!(claude::read_live(&paths).unwrap().refresh_token(), Some("rt-a2"));
        let snap = engine.current();
        assert_eq!(snap.active.as_deref(), Some("A"));
        assert!(snap.notice.is_none());
    }

    /// The tray app and a command (the mod's) share the vault: each sees what the other saved.
    #[tokio::test]
    async fn two_processes_share_the_vault() {
        let (_dir, paths) = sandbox();
        write_live(&paths, "rt-a", "A");
        let tray = Engine::new(paths.clone(), Role::Tray).unwrap();
        {
            let mut v = tray.vault().await.unwrap();
            for (id, rt) in [("A", "rt-a"), ("B", "rt-b")] {
                v.accounts.push(Account {
                    id: id.into(),
                    email: format!("{id}@x"),
                    oauth_account: json!({"accountUuid": id, "emailAddress": format!("{id}@x")}),
                    creds: creds(rt),
                    added_at: 0,
                    captured_at: 0,
                    usage: None,
                    status: Status::Ok,
                    error: None,
                    last_poll: 0,
                    backoff_until: 0,
                    backoff_ms: 0,
                });
            }
            v.active = Some("A".into());
            tray.save_and_publish(&v);
        }

        let command = Engine::new(paths.clone(), Role::Command).unwrap();
        assert_eq!(command.resolve("b").await.as_deref(), Some("B"), "by the email's local part");
        assert_eq!(command.resolve("2").await.as_deref(), Some("B"), "by number");
        command.switch("B").await.unwrap();

        tray.republish().await;
        assert_eq!(tray.current().active.as_deref(), Some("B"), "the tray sees the command's switch");
        tray.forget("A").await.unwrap();
        command.republish().await;
        assert_eq!(command.current().accounts.len(), 1, "and the command sees the tray's change");
        let status = fsx::read_json(&paths.status()).unwrap().unwrap();
        assert_eq!(status["driver"], "command");
        assert_eq!(status["accounts"][0]["short"], "B");
    }

    #[tokio::test]
    async fn the_live_account_cannot_be_forgotten() {
        let (_dir, paths) = sandbox();
        write_live(&paths, "rt-a", "A");
        let engine = Engine::new(paths, Role::Tray).unwrap();
        engine.vault.lock().await.vault.active = Some("A".into());
        assert!(engine.forget("A").await.is_err());
    }
}
