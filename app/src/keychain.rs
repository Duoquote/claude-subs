//! Claude Code's login on macOS: a generic password in the login keychain, not `.credentials.json`.
//!
//! Claude Code (2.1.x) keeps the same JSON there (`{"claudeAiOauth": {...}, ...}`) under the
//! service `Claude Code-credentials` (with `-<8 hex of sha256(config dir)>` when
//! `CLAUDE_CONFIG_DIR` is set) and the account `$USER`. It reads and writes it through
//! `/usr/bin/security`: `find-generic-password -w` to read, and `add-generic-password -U -X <hex>`
//! fed to `security -i` on stdin to write, so the secret is never in a process's arguments. We do
//! the same, which also keeps us within the item's access list (the `security` tool), so macOS
//! asks nothing. A running Claude Code reads the item again once its 30-second cache runs out,
//! which is how a switch reaches it.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};

const SECURITY: &str = "/usr/bin/security";
/// `security`'s exit code for an item that is not there.
const NOT_FOUND: i32 = 44;
/// `security`'s exit code when the keychain is locked to this session (`errSecInteractionNotAllowed`).
const LOCKED: i32 = 36;
/// `security -i` reads one command per line, up to this length (as Claude Code measures it).
const STDIN_LINE_MAX: usize = 4032;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub service: String,
    pub account: String,
}

impl Item {
    /// The item Claude Code uses, resolved from the environment the way Claude Code resolves it.
    pub fn claude_code() -> Self {
        let var = |name: &str| std::env::var(name).ok();
        // CLAUDE_SECURESTORAGE_CONFIG_DIR, when set, decides alone: empty means the default item.
        let hashed = match var("CLAUDE_SECURESTORAGE_CONFIG_DIR") {
            Some(dir) => (!dir.is_empty()).then_some(dir),
            None => var("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()),
        };
        let suffix = hashed.map_or_else(String::new, |dir| {
            let digest = Sha256::digest(dir.as_bytes());
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            format!("-{}", &hex[..8])
        });
        let account = var("USER")
            .filter(|u| !u.is_empty())
            .filter(|u| u.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)))
            .unwrap_or_else(|| "claude-code-user".into());
        Self {
            service: format!("Claude Code-credentials{suffix}"),
            account,
        }
    }

    /// The JSON in the item; `None` when there is no such item.
    pub fn read(&self) -> Result<Option<Value>> {
        let out = Command::new(SECURITY)
            .args(["find-generic-password", "-a", &self.account, "-w", "-s", &self.service])
            .stdin(Stdio::null())
            .output()
            .context("running security")?;
        match out.status.code() {
            Some(0) => {
                let text = String::from_utf8(out.stdout).context("the keychain item is not text")?;
                let value = serde_json::from_str(text.trim()).context("the keychain item is not JSON")?;
                Ok(Some(value))
            }
            Some(NOT_FOUND) => Ok(None),
            Some(LOCKED) => bail!(
                "the login keychain is locked to this session (macOS keeps it closed to SSH sessions); run this from a terminal on the Mac itself"
            ),
            code => bail!("security could not read the keychain item ({}): {}", show(code), stderr(&out.stderr)),
        }
    }

    /// Replaces the item's JSON (creating the item if it is missing).
    pub fn write(&self, value: &Value) -> Result<()> {
        let json = serde_json::to_string(value)?;
        let hex: String = json.bytes().map(|b| format!("{b:02x}")).collect();
        let line = format!(
            "add-generic-password -U -a \"{}\" -s \"{}\" -X \"{hex}\"\n",
            self.account, self.service
        );
        let out = if line.len() <= STDIN_LINE_MAX {
            let mut child = Command::new(SECURITY)
                .arg("-i")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("running security")?;
            child
                .stdin
                .take()
                .context("security's stdin")?
                .write_all(line.as_bytes())
                .context("writing to security")?;
            child.wait_with_output()?
        } else {
            // Too long for `security -i`: as Claude Code does, the hex goes in the arguments.
            tracing::warn!("keychain payload ({} B of JSON) is too long for security -i; using its arguments", json.len());
            Command::new(SECURITY)
                .args(["add-generic-password", "-U", "-a", &self.account, "-s", &self.service, "-X", &hex])
                .stdin(Stdio::null())
                .output()
                .context("running security")?
        };
        if !out.status.success() {
            bail!("security could not write the keychain item ({}): {}", show(out.status.code()), stderr(&out.stderr));
        }
        // `security -i` can report a failed command and still exit 0: read it back.
        match self.read()? {
            Some(back) if back == *value => Ok(()),
            _ => bail!("the keychain item did not take the new login: {}", stderr(&out.stderr)),
        }
    }

    /// When the item last changed (its `mdat`, to the second); `None` when there is no item.
    pub fn modified(&self) -> Option<SystemTime> {
        let out = Command::new(SECURITY)
            .args(["find-generic-password", "-a", &self.account, "-s", &self.service])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        parse_mdat(&String::from_utf8_lossy(&out.stdout))
    }

    /// How long ago the item last changed.
    pub fn age(&self) -> Option<Duration> {
        SystemTime::now().duration_since(self.modified()?).ok()
    }
}

/// `"mdat"<timedate>=0x3230...  "20261010175029Z\000"` → that time (UTC).
fn parse_mdat(attributes: &str) -> Option<SystemTime> {
    let line = attributes.lines().find(|l| l.trim_start().starts_with("\"mdat\""))?;
    let quoted = line.split('"').nth(3)?;
    let digits = quoted.get(..14)?;
    let t = chrono::NaiveDateTime::parse_from_str(digits, "%Y%m%d%H%M%S").ok()?.and_utc();
    Some(SystemTime::UNIX_EPOCH + Duration::from_millis(u64::try_from(t.timestamp_millis()).ok()?))
}

fn show(code: Option<i32>) -> String {
    code.map_or_else(|| "killed".into(), |c| format!("exit {c}"))
}

fn stderr(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn modified_time_from_the_attributes() {
        let attrs = "keychain: \"/Users/u/Library/Keychains/login.keychain-db\"\nattributes:\n    \"acct\"<blob>=\"u\"\n    \"mdat\"<timedate>=0x32303236313031303137353032395A00  \"20261010175029Z\\000\"\n";
        let t = parse_mdat(attrs).unwrap();
        let secs = t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(secs, chrono::DateTime::parse_from_rfc3339("2026-10-10T17:50:29Z").unwrap().timestamp() as u64);
    }

    /// `cargo test keychain_round_trip -- --ignored`, on a Mac: writes, reads and removes an item of
    /// its own (never Claude Code's).
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let item = Item {
            service: format!("claude-subs-test-{}", std::process::id()),
            account: Item::claude_code().account,
        };
        assert_eq!(item.read().unwrap(), None);
        let value = json!({"claudeAiOauth": {"refreshToken": "rt-test", "accessToken": "at-test"}, "mcpOAuth": {}});
        item.write(&value).unwrap();
        assert_eq!(item.read().unwrap(), Some(value.clone()));
        assert!(item.age().unwrap() < Duration::from_secs(120));
        let bigger = json!({"claudeAiOauth": {"refreshToken": "rt-2"}, "pad": "x".repeat(3000)});
        item.write(&bigger).unwrap();
        assert_eq!(item.read().unwrap(), Some(bigger));
        let status = Command::new(SECURITY)
            .args(["delete-generic-password", "-a", &item.account, "-s", &item.service])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }
}
