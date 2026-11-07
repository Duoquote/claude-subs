//! Where Claude Code keeps its login, and where this app keeps its own data.
//!
//! Claude Code (2.1.x) resolves its files the same way: the config dir is `CLAUDE_CONFIG_DIR` or
//! `~/.claude`; the global config is `<CLAUDE_CONFIG_DIR>/.claude.json` when that variable is set,
//! else `~/.claude.json`. Its token refresh holds two proper-lockfile (mkdir) locks:
//! `<config dir>/.oauth_refresh.lock` and the legacy `<config dir>.lock`; every global-config
//! save holds `<global config>.lock`.

use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Paths {
    pub claude_dir: PathBuf,
    /// `<claude_dir>/.credentials.json`: `{"claudeAiOauth": {...}, "mcpOAuth": {...}, ...}`.
    pub credentials: PathBuf,
    /// `~/.claude.json`: `oauthAccount` (who is logged in) and `cachedUsageUtilization`.
    pub global_config: PathBuf,
    pub refresh_lock: PathBuf,
    pub legacy_refresh_lock: PathBuf,
    pub config_lock: PathBuf,
    /// The vault, settings, status file and logs: `CLAUDE_SUBS_DATA_DIR`, else
    /// `%LOCALAPPDATA%\claude-subs` on Windows and `$XDG_DATA_HOME/claude-subs` (or
    /// `~/.local/share/claude-subs`) elsewhere.
    pub data_dir: PathBuf,
    /// macOS: the keychain item Claude Code keeps its login in (`.credentials.json` is its
    /// fallback). Set by `detect`; a sandbox (`new`) has none.
    #[cfg(target_os = "macos")]
    pub keychain: Option<crate::keychain::Item>,
}

impl Paths {
    pub fn detect() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        let claude_dir = config_dir.clone().unwrap_or_else(|| home.join(".claude"));
        let global_config = match &config_dir {
            Some(dir) => dir.join(".claude.json"),
            None => home.join(".claude.json"),
        };
        let set = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let data_dir = set("CLAUDE_SUBS_DATA_DIR").unwrap_or_else(|| {
            if cfg!(windows) {
                set("LOCALAPPDATA").map_or_else(|| home.join(".claude-subs"), |d| d.join("claude-subs"))
            } else {
                set("XDG_DATA_HOME")
                    .unwrap_or_else(|| home.join(".local").join("share"))
                    .join("claude-subs")
            }
        });
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut paths = Self::new(claude_dir, global_config, data_dir);
        #[cfg(target_os = "macos")]
        {
            paths.keychain = Some(crate::keychain::Item::claude_code());
        }
        paths
    }

    pub fn new(claude_dir: PathBuf, global_config: PathBuf, data_dir: PathBuf) -> Self {
        let mut legacy = claude_dir.clone().into_os_string();
        legacy.push(".lock");
        let mut config_lock = global_config.clone().into_os_string();
        config_lock.push(".lock");
        Self {
            credentials: claude_dir.join(".credentials.json"),
            refresh_lock: claude_dir.join(".oauth_refresh.lock"),
            legacy_refresh_lock: PathBuf::from(legacy),
            config_lock: PathBuf::from(config_lock),
            claude_dir,
            global_config,
            data_dir,
            #[cfg(target_os = "macos")]
            keychain: None,
        }
    }

    pub fn vault(&self) -> PathBuf {
        self.data_dir.join("vault.dat")
    }

    pub fn settings(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }

    pub fn backups(&self) -> PathBuf {
        self.data_dir.join("backups")
    }

    pub fn logs(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    /// Held (mkdir lock) by whichever process changes the vault: the tray app, or a command.
    pub fn vault_lock(&self) -> PathBuf {
        self.data_dir.join("vault.lock")
    }

    /// What the Claude Code mod reads: the accounts and their usage, no tokens.
    pub fn status(&self) -> PathBuf {
        self.data_dir.join("status.json")
    }

    /// Claude Code's user settings, where `setup` names the mod's folder.
    pub fn claude_settings(&self) -> PathBuf {
        self.claude_dir.join("settings.json")
    }
}
