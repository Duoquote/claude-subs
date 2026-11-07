//! The notification-area icon. Left click toggles the popover; right click opens the menu: the
//! saved accounts (click one to switch), Refresh, Start at login, Quit. The icon and tooltip follow
//! the live account; the tooltip lists the others too.

use std::sync::Arc;
use std::sync::Mutex;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_autostart::ManagerExt;

use crate::engine::{AccountView, Engine, Snapshot, WindowView};
use crate::icon::{self, Face};
use crate::views;

const TRAY_ID: &str = "claude-subs";

/// What the menu was last built from (accounts, active, autostart), so it is rebuilt only on change.
#[derive(Default)]
pub struct MenuKey(pub Mutex<String>);

pub fn create(app: &AppHandle, snap: &Snapshot) -> tauri::Result<()> {
    let face = face_of(snap);
    let size = icon::tray_size();
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::new_owned(icon::render(&face, size), size, size))
        .tooltip(tooltip(snap))
        .menu(&menu(app, snap)?)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                position,
                ..
            } = event
            {
                views::toggle_popover(tray.app_handle(), Some(position));
            }
        })
        .build(app)?;
    Ok(())
}

pub fn refresh(app: &AppHandle, snap: &Snapshot) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let size = icon::tray_size();
    let _ = tray.set_icon(Some(Image::new_owned(icon::render(&face_of(snap), size), size, size)));
    let _ = tray.set_tooltip(Some(tooltip(snap)));
    let key = menu_key(app, snap);
    let state = app.state::<MenuKey>();
    let mut last = state.0.lock().unwrap_or_else(|p| p.into_inner());
    if *last != key {
        match menu(app, snap) {
            Ok(m) => {
                let _ = tray.set_menu(Some(m));
                *last = key;
            }
            Err(e) => tracing::warn!("tray menu not rebuilt: {e}"),
        }
    }
}

fn face_of(snap: &Snapshot) -> Face {
    let light_taskbar = icon::light_taskbar();
    match snap.active_view() {
        Some(a) => Face {
            headline: a.headline(),
            session: a.session.as_ref().map(|w| w.percent),
            week: a.week.as_ref().map(|w| w.percent),
            light_taskbar,
            dim: a.status != crate::vault::Status::Ok,
        },
        None => Face {
            headline: None,
            session: None,
            week: None,
            light_taskbar,
            dim: true,
        },
    }
}

fn autostart_on(app: &AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

fn menu_key(app: &AppHandle, snap: &Snapshot) -> String {
    let mut key = format!("{}|{:?}", autostart_on(app), snap.active);
    for a in &snap.accounts {
        key.push_str(&format!("|{}:{}:{:?}", a.id, a.email, a.status));
    }
    key
}

fn menu(app: &AppHandle, snap: &Snapshot) -> tauri::Result<Menu<Wry>> {
    let mut accounts = Vec::new();
    for a in &snap.accounts {
        let mut label = a.email.clone();
        if let Some(h) = a.headline() {
            label.push_str(&format!("  ({}%)", h.round()));
        }
        if a.status != crate::vault::Status::Ok {
            label.push_str("  — needs /login");
        }
        accounts.push(CheckMenuItem::with_id(
            app,
            format!("acct:{}", a.id),
            label,
            a.status == crate::vault::Status::Ok || a.active,
            a.active,
            None::<&str>,
        )?);
    }
    let none = MenuItem::with_id(app, "none", "No saved accounts yet — /login in Claude Code", false, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let open = MenuItem::with_id(app, "open", "Show accounts", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", "Refresh usage", true, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(app, "autostart", "Start at login", true, autostart_on(app), None::<&str>)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let mut items: Vec<&dyn IsMenuItem<Wry>> = Vec::new();
    if accounts.is_empty() {
        items.push(&none);
    }
    for item in &accounts {
        items.push(item);
    }
    items.extend([&sep1 as &dyn IsMenuItem<Wry>, &open, &refresh, &autostart, &sep2, &quit]);
    Menu::with_items(app, &items)
}

fn on_menu(app: &AppHandle, id: &str) {
    let engine = app.state::<Arc<Engine>>().inner().clone();
    if let Some(account) = id.strip_prefix("acct:") {
        let account = account.to_owned();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if engine.switch(&account).await.is_err() {
                // The popover shows why.
                views::popover(&app, None);
            }
        });
        return;
    }
    match id {
        "open" => views::popover(app, None),
        "refresh" => engine.refresh_now(),
        "autostart" => {
            let manager = app.autolaunch();
            let result = if manager.is_enabled().unwrap_or(false) {
                manager.disable()
            } else {
                manager.enable()
            };
            if let Err(e) = result {
                tracing::warn!("start at login not changed: {e}");
            }
            refresh(app, &engine.current());
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

// ----------------------------------------------------------------- tooltip --

/// "Session 90% · resets in 2h 05m", "Week 65% · resets tomorrow 11:00".
fn window_line(w: &WindowView) -> String {
    match &w.resets {
        Some(r) => format!("{} {}% · resets {r}", w.label, w.percent.round()),
        None => format!("{} {}%", w.label, w.percent.round()),
    }
}

fn short_name(a: &AccountView) -> String {
    a.short.clone()
}

fn pct(w: &Option<WindowView>) -> String {
    w.as_ref().map_or("–".to_owned(), |w| format!("{}", w.percent.round()))
}

/// The tooltip: the live account and its windows, then the others as "name s/w". Windows cuts
/// tooltips at 127 characters, so lines are dropped from the end to fit.
pub fn tooltip(snap: &Snapshot) -> String {
    let mut lines = Vec::new();
    match snap.active_view() {
        Some(a) => {
            lines.push(a.email.clone());
            if let Some(w) = &a.session {
                lines.push(window_line(w));
            }
            if let Some(w) = &a.week {
                lines.push(window_line(w));
            }
            if a.session.is_none() && a.week.is_none() {
                lines.push("No usage reading yet".into());
            }
        }
        None if snap.accounts.is_empty() => lines.push("Claude Subs: /login in Claude Code to add an account".into()),
        None => lines.push("Claude Subs: the live login is not a saved account".into()),
    }
    let others: Vec<String> = snap
        .accounts
        .iter()
        .filter(|a| !a.active)
        .map(|a| {
            if a.status != crate::vault::Status::Ok {
                format!("{} needs login", short_name(a))
            } else {
                format!("{} {}/{}", short_name(a), pct(&a.session), pct(&a.week))
            }
        })
        .collect();
    if !others.is_empty() {
        lines.push(others.join(" · "));
    }
    let mut text = lines.join("\n");
    while text.chars().count() > 127 {
        if lines.len() > 1 {
            lines.pop();
            text = lines.join("\n");
        } else {
            text = text.chars().take(126).collect::<String>() + "…";
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Status;

    fn window(label: &str, percent: f64, resets: Option<&str>) -> WindowView {
        WindowView {
            label: label.into(),
            percent,
            resets_at: resets.map(|_| 0),
            resets: resets.map(str::to_owned),
            resets_exact: None,
            resets_short: None,
        }
    }

    fn view(email: &str, active: bool, s: f64, w: f64) -> AccountView {
        AccountView {
            id: email.into(),
            email: email.into(),
            short: email.split('@').next().unwrap_or(email).into(),
            name: None,
            org: None,
            plan: None,
            active,
            status: Status::Ok,
            error: None,
            session: Some(window("Session", s, Some("in 1h 05m"))),
            week: Some(window("Week", w, Some("tomorrow 11:00"))),
            scoped: vec![],
            usage_at: None,
            usage_source: None,
            login_expires_at: None,
        }
    }

    #[test]
    fn tooltip_names_the_live_account_and_fits() {
        let snap = Snapshot {
            active: Some("work@example.com".into()),
            accounts: vec![
                view("work@example.com", true, 27.0, 54.0),
                view("side@example.com", false, 12.0, 30.0),
                view("client@example.com", false, 80.0, 95.0),
            ],
            notice: None,
            at: 0,
        };
        let t = tooltip(&snap);
        assert!(
            t.starts_with("work@example.com\nSession 27% · resets in 1h 05m\nWeek 54% · resets tomorrow 11:00"),
            "{t}"
        );
        assert!(t.ends_with("side 12/30 · client 80/95"), "{t}");
        assert!(t.chars().count() <= 127);
    }
}
