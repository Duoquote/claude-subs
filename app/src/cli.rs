//! The command line: what the Claude Code mod runs for its account band, and what a person runs
//! on a machine with no tray (Linux).
//!
//! Each command runs an engine once: it takes in the live login first (so a `/login` or a token
//! refresh since the last look is saved), does its work under the vault lock, and leaves
//! `status.json` up to date.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::engine::{AccountView, Engine, Role, WindowView};
use crate::fsx;
use crate::paths::Paths;
use crate::vault::Status;

const HELP: &str = "\
claude-subs: switch Claude Code between your accounts and see how much of each is used.

Usage: claude-subs [command]

Commands:
  status [--json]     the saved accounts and their usage (the default)
  switch <account>    make <account> the live login: its email, the part before the @,
                      or its number in `status`; running Claude Code sessions follow
  refresh             read every account's usage now
  forget <account>    remove a saved account (not the live one)
  sync                save the live login (after a /login), nothing else
  tick                what the Claude Code mod runs every minute: sync, then read what is due
  setup [--mod-dir <dir>] [--remove]
                      load (or stop loading) the Claude Code mod: names its folder in
                      CLAUDE_CODE_PLUGIN_DIRS in ~/.claude/settings.json
  version

Add an account by running /login in Claude Code; it is saved on its own. Never /logout:
that revokes the login, the saved copy included.
";

const COMMANDS: &[&str] = &[
    "status", "switch", "refresh", "forget", "sync", "tick", "setup", "version", "help", "--help", "-h", "--version",
];

pub fn is_command(arg: &str) -> bool {
    COMMANDS.contains(&arg)
}

pub fn main(args: &[String]) -> ExitCode {
    #[cfg(windows)]
    attach_console();
    let paths = Paths::detect();
    let _log = crate::init_logging(&paths.logs(), "claude-subs-cli");
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("claude-subs: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(paths, args)) {
        Ok(code) => code,
        Err(e) => {
            tracing::warn!("{}: {e:#}", args.join(" "));
            eprintln!("claude-subs: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(paths: Paths, args: &[String]) -> Result<ExitCode> {
    let command = args.first().map_or("status", String::as_str);
    let rest = args.get(1..).unwrap_or_default();
    match command {
        "help" | "--help" | "-h" => {
            print!("{HELP}");
            return Ok(ExitCode::SUCCESS);
        }
        "version" | "--version" => {
            println!("claude-subs {}", env!("CARGO_PKG_VERSION"));
            return Ok(ExitCode::SUCCESS);
        }
        "setup" => return setup(&paths, rest),
        c if !is_command(c) => {
            eprint!("claude-subs: unknown command `{c}`\n\n{HELP}");
            return Ok(ExitCode::from(2));
        }
        _ => {}
    }

    let engine = Engine::new(paths, Role::Command)?;
    // A failed look (Claude Code mid-write, the profile endpoint down) is not fatal: the vault
    // still answers, and the next command looks again.
    let synced = engine.sync_now().await;
    if let Err(e) = &synced {
        tracing::warn!("sync: {e:#}");
    }
    match command {
        "sync" => {
            synced?;
        }
        "tick" => engine.poll_due(false).await,
        "status" => {
            engine.republish().await;
            if rest.iter().any(|a| a == "--json") {
                let text = std::fs::read_to_string(engine.paths.status()).context("reading status.json")?;
                println!("{}", text.trim_end());
            } else {
                print_status(&engine);
            }
        }
        "refresh" => {
            engine.poll_due(true).await;
            print_status(&engine);
        }
        "switch" | "forget" => {
            let Some(who) = rest.first() else {
                eprintln!("claude-subs: say which account: claude-subs {command} <email, name or number>");
                return Ok(ExitCode::from(2));
            };
            let Some(id) = engine.resolve(who).await else {
                bail!("no saved account matches `{who}` (see `claude-subs status`)");
            };
            let email = engine
                .current()
                .accounts
                .iter()
                .find(|a| a.id == id)
                .map_or_else(|| id.clone(), |a| a.email.clone());
            if command == "switch" {
                engine.switch(&id).await?;
                println!("Switched to {email}. Running Claude Code sessions use it from their next request.");
            } else {
                engine.forget(&id).await?;
                println!("Forgot {email}.");
            }
        }
        _ => unreachable!("checked above"),
    }
    Ok(ExitCode::SUCCESS)
}

fn print_status(engine: &Engine) {
    let snap = engine.current();
    let mut out = std::io::stdout().lock();
    if snap.accounts.is_empty() {
        let _ = writeln!(out, "No accounts saved yet. Run /login in Claude Code; each login is saved on its own.");
        return;
    }
    let width = snap.accounts.iter().map(|a| a.email.chars().count()).max().unwrap_or(0);
    for (n, a) in snap.accounts.iter().enumerate() {
        let mark = if a.active { "*" } else { " " };
        let _ = writeln!(out, "{mark} {}  {:<width$}  {}", n + 1, a.email, describe(a));
    }
    if let Some(notice) = &snap.notice {
        let _ = writeln!(out, "\n{notice}");
    }
}

fn describe(a: &AccountView) -> String {
    if a.status == Status::NeedsLogin {
        return "needs /login".into();
    }
    let window = |label: &str, w: &Option<WindowView>| match w {
        Some(w) => match &w.resets {
            Some(r) => format!("{label} {:>3.0}% ({r})", w.percent),
            None => format!("{label} {:>3.0}%", w.percent),
        },
        None => format!("{label}   -"),
    };
    let mut parts = vec![window("session", &a.session), window("week", &a.week)];
    if a.active {
        parts.push("live".into());
    }
    if let Some(e) = &a.error {
        parts.push(format!("({e})"));
    }
    parts.join("  ")
}

// -------------------------------------------------------------------- setup --

/// Names the mod's folder in `CLAUDE_CODE_PLUGIN_DIRS` in Claude Code's user settings (`env`), so
/// every new session loads it; `--remove` takes it out again.
fn setup(paths: &Paths, args: &[String]) -> Result<ExitCode> {
    let remove = args.iter().any(|a| a == "--remove");
    let explicit = args
        .iter()
        .position(|a| a == "--mod-dir")
        .map(|i| args.get(i + 1).map(PathBuf::from).context("--mod-dir needs a folder"))
        .transpose()?;
    let mod_dir = match explicit {
        Some(dir) => dir,
        None => find_mod_dir(paths).context("the mod's folder was not found; give it with --mod-dir <dir>")?,
    };
    let mod_dir = std::path::absolute(&mod_dir).unwrap_or(mod_dir);
    if !remove && !mod_dir.join(".claude-plugin").join("plugin.json").is_file() {
        bail!("{} holds no .claude-plugin/plugin.json", mod_dir.display());
    }

    let file = paths.claude_settings();
    let mut settings = fsx::read_json(&file)?.unwrap_or_else(|| json!({}));
    let root = settings.as_object_mut().context("settings.json is not a JSON object")?;
    let env = root.entry("env").or_insert_with(|| Value::Object(Map::new()));
    let env = env.as_object_mut().context("settings.json `env` is not an object")?;
    let sep = if cfg!(windows) { ';' } else { ':' };
    let ours = mod_dir.to_string_lossy().into_owned();
    let same = |entry: &str| {
        let a = entry.trim_end_matches(['/', '\\']);
        let b = ours.trim_end_matches(['/', '\\']);
        if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b }
    };
    let before = env.get("CLAUDE_CODE_PLUGIN_DIRS").and_then(Value::as_str).unwrap_or_default().to_owned();
    let mut dirs: Vec<&str> = before.split(sep).filter(|d| !d.is_empty() && !same(d)).collect();
    if !remove {
        dirs.push(&ours);
    }
    let after = dirs.join(&sep.to_string());
    if after == before {
        println!("Already {}: {}", if remove { "not loaded" } else { "loaded" }, mod_dir.display());
        return Ok(ExitCode::SUCCESS);
    }
    if after.is_empty() {
        env.remove("CLAUDE_CODE_PLUGIN_DIRS");
    } else {
        env.insert("CLAUDE_CODE_PLUGIN_DIRS".into(), Value::String(after));
    }
    backup(&file, &paths.backups())?;
    let mut text = serde_json::to_string_pretty(&settings)?;
    text.push('\n');
    fsx::write_atomic(&file, text.as_bytes())?;
    if remove {
        println!("The Claude Code mod is no longer loaded ({}).", file.display());
    } else {
        println!(
            "The Claude Code mod loads from {} in every new Claude Code session ({}).",
            mod_dir.display(),
            file.display()
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// The mod next to the binary: an install (`<data dir>/mod`, `<exe dir>/../mod`) or a build in the
/// repository (`target/release/../../mod`).
fn find_mod_dir(paths: &Paths) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let candidates = [
        exe_dir.join("mod"),
        exe_dir.join("..").join("mod"),
        exe_dir.join("..").join("..").join("mod"),
        paths.data_dir.join("mod"),
    ];
    candidates
        .into_iter()
        .find(|d| d.join(".claude-plugin").join("plugin.json").is_file())
        .map(|d| std::fs::canonicalize(&d).map(strip_verbatim).unwrap_or(d))
}

/// `\\?\C:\...` (what canonicalize gives on Windows) as `C:\...`.
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p,
    }
}

fn backup(file: &Path, dir: &Path) -> Result<()> {
    if !file.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let name = file.file_name().map_or_else(|| "settings.json".into(), |n| n.to_string_lossy().into_owned());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let bytes = std::fs::read(file)?;
    fsx::write_atomic(&dir.join(format!("{name}.{stamp}")), &bytes)
}

/// A tray build has no console of its own: print to the terminal it was started from.
#[cfg(windows)]
fn attach_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE};
    // SAFETY: plain Win32 calls with no pointers of ours.
    unsafe {
        let has_out = GetStdHandle(STD_OUTPUT_HANDLE).is_ok_and(|h| !h.is_invalid() && !h.0.is_null());
        if !has_out {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::tests::sandbox;

    #[test]
    fn setup_adds_the_mod_once_keeps_the_rest_and_removes_it() {
        let (dir, paths) = sandbox();
        let mod_dir = dir.path().join("mod");
        std::fs::create_dir_all(mod_dir.join(".claude-plugin")).unwrap();
        std::fs::write(mod_dir.join(".claude-plugin").join("plugin.json"), "{}").unwrap();
        let file = paths.claude_settings();
        std::fs::write(&file, r#"{"model": "opus", "env": {"A": "1", "CLAUDE_CODE_PLUGIN_DIRS": "/other"}}"#).unwrap();
        let args = |extra: &[&str]| {
            let mut v = vec!["--mod-dir".to_owned(), mod_dir.to_string_lossy().into_owned()];
            v.extend(extra.iter().map(|s| (*s).to_owned()));
            v
        };
        let sep = if cfg!(windows) { ';' } else { ':' };

        setup(&paths, &args(&[])).unwrap();
        setup(&paths, &args(&[])).unwrap();
        let s = fsx::read_json(&file).unwrap().unwrap();
        let ours = std::path::absolute(&mod_dir).unwrap().to_string_lossy().into_owned();
        assert_eq!(s["env"]["CLAUDE_CODE_PLUGIN_DIRS"], format!("/other{sep}{ours}"));
        assert_eq!(s["env"]["A"], "1");
        assert_eq!(s["model"], "opus");
        assert_eq!(s.as_object().unwrap().keys().next().map(String::as_str), Some("model"), "key order kept");

        setup(&paths, &args(&["--remove"])).unwrap();
        let s = fsx::read_json(&file).unwrap().unwrap();
        assert_eq!(s["env"]["CLAUDE_CODE_PLUGIN_DIRS"], "/other");
    }
}
