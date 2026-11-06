//! The Windows tray app: the tray icon and menu, the popover, and the engine running in the
//! background (watching Claude Code's files and reading usage).

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use tauri::{App, AppHandle, Emitter, Manager, RunEvent, State};

use crate::engine::{Engine, Role, Snapshot};
use crate::paths::Paths;
use crate::{tray, views};

pub fn main() -> ExitCode {
    let paths = Paths::detect();
    let _log = crate::init_logging(&paths.logs(), "claude-subs");
    match run(paths) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("fatal: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(paths: Paths) -> anyhow::Result<()> {
    tauri::Builder::default()
        // First: a second start shows the running copy's popover and ends.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| views::popover(app, None)))
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .invoke_handler(tauri::generate_handler![snapshot, switch_account, forget_account, refresh_now, hide])
        .setup(move |app| start(app, paths).map_err(|e| -> Box<dyn std::error::Error> { e.into() }))
        .build(tauri::generate_context!())?
        // No window at idle is normal: closing the popover must not end the app; Quit has a code.
        .run(|_app, event| {
            if let RunEvent::ExitRequested { api, code, .. } = event
                && code.is_none()
            {
                api.prevent_exit();
            }
        });
    Ok(())
}

fn start(app: &App, paths: Paths) -> anyhow::Result<()> {
    let handle = app.handle().clone();
    let engine = Engine::new(paths, Role::Tray)?;
    app.manage(engine.clone());
    app.manage(views::PopoverState::default());
    app.manage(tray::MenuKey::default());
    tray::create(&handle, &engine.current())?;

    let runtime = tauri::async_runtime::handle();
    let _enter = runtime.inner().enter();
    engine.start();
    tauri::async_runtime::spawn(follow(handle, engine));
    Ok(())
}

/// Every snapshot goes to the tray and the popover; once a minute the snapshot is rebuilt anyway
/// (countdowns, windows that reset, a taskbar theme change).
async fn follow(app: AppHandle, engine: Arc<Engine>) {
    let mut rx = engine.subscribe();
    let mut minute = tokio::time::interval(Duration::from_secs(60));
    loop {
        tokio::select! {
            changed = rx.changed() => {
                if changed.is_err() {
                    return;
                }
                let snap = rx.borrow_and_update().clone();
                tray::refresh(&app, &snap);
                let _ = app.emit("snapshot", &*snap);
            }
            _ = minute.tick() => engine.republish().await,
        }
    }
}

// ---------------------------------------------------------------- commands --

#[tauri::command]
fn snapshot(engine: State<'_, Arc<Engine>>) -> Snapshot {
    (*engine.current()).clone()
}

#[tauri::command]
async fn switch_account(engine: State<'_, Arc<Engine>>, id: String) -> Result<Snapshot, String> {
    let engine = engine.inner().clone();
    engine.switch(&id).await.map_err(|e| format!("{e:#}"))?;
    Ok((*engine.current()).clone())
}

#[tauri::command]
async fn forget_account(engine: State<'_, Arc<Engine>>, id: String) -> Result<Snapshot, String> {
    let engine = engine.inner().clone();
    engine.forget(&id).await.map_err(|e| format!("{e:#}"))?;
    Ok((*engine.current()).clone())
}

#[tauri::command]
fn refresh_now(engine: State<'_, Arc<Engine>>) {
    engine.refresh_now();
}

#[tauri::command]
fn hide(app: AppHandle) {
    views::hide_popover(&app, false);
}
