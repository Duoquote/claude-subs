//! Claude Subs: switches Claude Code's login between saved accounts and shows how much of each
//! account's session and weekly limits is used.
//!
//! Accounts are saved by logging in with Claude Code (`/login`); the engine notices and keeps
//! their tokens. A switch rewrites the live login the way a sibling `/login` does, so running
//! Claude Code windows carry on with the new account.
//!
//! On Windows, run without arguments, it is a tray app. On every platform it is also a command
//! line (`claude-subs status`, `switch`, ...): what the Claude Code mod in `mod/` runs for its
//! account band, and all there is on Linux.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod api;
mod claude;
mod cli;
#[cfg(windows)]
mod dpapi;
mod engine;
mod fsx;
#[cfg(windows)]
mod gui;
#[cfg(windows)]
mod icon;
mod lock;
mod paths;
#[cfg(windows)]
mod tray;
mod usage;
mod vault;
#[cfg(windows)]
mod views;
mod when;

use std::process::ExitCode;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    if !args.first().is_some_and(|a| cli::is_command(a)) {
        return gui::main();
    }
    cli::main(&args)
}

/// Daily log files in `dir`, named by `prefix` (the tray app and the commands keep their own).
pub(crate) fn init_logging(dir: &std::path::Path, prefix: &str) -> Option<WorkerGuard> {
    let _ = std::fs::create_dir_all(dir);
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(prefix)
        .filename_suffix("log")
        .max_log_files(5)
        .build(dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env("CLAUDE_SUBS_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_timer(tracing_subscriber::fmt::time::ChronoLocal::rfc_3339())
        .with_writer(writer)
        .with_ansi(false)
        .init();
    Some(guard)
}
