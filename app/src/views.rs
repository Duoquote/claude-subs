//! The popover: frameless, always on top, above the tray. Hides on blur; destroyed a while after it
//! hid so WebView2's memory is returned. (Same behaviour as Tasks' popover.)

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

const POPOVER: &str = "popover";
const SIZE: (f64, f64) = (380.0, 560.0);
const MARGIN: f64 = 8.0;
/// A tray click this soon after the popover hid on blur is the click that hid it.
const BLUR_GRACE: Duration = Duration::from_millis(400);
const KEEPALIVE: Duration = Duration::from_secs(120);

#[derive(Default)]
pub struct PopoverState {
    generation: AtomicU64,
    blurred_at: Mutex<Option<Instant>>,
    /// The popover has had focus since it was shown. Windows may refuse focus to a window that a
    /// relaunch (not a tray click) shows; such a popover must not hide on the blur that follows.
    focused: AtomicBool,
}

pub fn toggle_popover(app: &AppHandle, anchor: Option<PhysicalPosition<f64>>) {
    let visible = app
        .get_webview_window(POPOVER)
        .is_some_and(|w| w.is_visible().unwrap_or(false));
    if visible {
        hide_popover(app, false);
        return;
    }
    let state = app.state::<PopoverState>();
    let just_blurred = state
        .blurred_at
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_some_and(|t| t.elapsed() < BLUR_GRACE);
    if !just_blurred {
        popover(app, anchor);
    }
}

/// Shows the popover (created off the event loop: Tauri hands the work to the main thread).
pub fn popover(app: &AppHandle, anchor: Option<PhysicalPosition<f64>>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = show(&app, anchor) {
            tracing::warn!("popover not shown: {e}");
        }
    });
}

fn show(app: &AppHandle, anchor: Option<PhysicalPosition<f64>>) -> tauri::Result<()> {
    app.state::<PopoverState>().generation.fetch_add(1, Ordering::SeqCst);
    let window = match app.get_webview_window(POPOVER) {
        Some(w) => w,
        None => {
            let w = WebviewWindowBuilder::new(app, POPOVER, WebviewUrl::App("index.html".into()))
                .title("Claude Subs")
                .inner_size(SIZE.0, SIZE.1)
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .visible(false)
                .build()?;
            let handle = app.clone();
            w.on_window_event(move |event| {
                let state = handle.state::<PopoverState>();
                match event {
                    WindowEvent::Focused(true) => state.focused.store(true, Ordering::SeqCst),
                    WindowEvent::Focused(false) => {
                        let up = handle
                            .get_webview_window(POPOVER)
                            .is_some_and(|p| p.is_visible().unwrap_or(false));
                        if up && state.focused.swap(false, Ordering::SeqCst) {
                            hide_popover(&handle, true);
                        }
                    }
                    _ => {}
                }
            });
            w
        }
    };
    place(app, &window, anchor)?;
    window.show()?;
    window.set_focus()?;
    Ok(())
}

pub fn hide_popover(app: &AppHandle, by_blur: bool) {
    let state = app.state::<PopoverState>();
    if by_blur {
        *state.blurred_at.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
    }
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    state.focused.store(false, Ordering::SeqCst);
    if let Some(w) = app.get_webview_window(POPOVER) {
        let _ = w.hide();
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(KEEPALIVE).await;
        if app.state::<PopoverState>().generation.load(Ordering::SeqCst) != generation {
            return;
        }
        if let Some(w) = app.get_webview_window(POPOVER)
            && !w.is_visible().unwrap_or(false)
        {
            let _ = w.destroy();
        }
    });
}

/// Bottom-right of the work area of the monitor at `anchor` (or the cursor), centred on the
/// anchor's x when there is one; below the anchor when the taskbar is at the top.
fn place(app: &AppHandle, window: &WebviewWindow, anchor: Option<PhysicalPosition<f64>>) -> tauri::Result<()> {
    let at = match anchor {
        Some(p) => p,
        None => app.cursor_position()?,
    };
    let Some(monitor) = app.monitor_from_point(at.x, at.y)?.or(app.primary_monitor()?) else {
        return Ok(());
    };
    let work = monitor.work_area();
    let scale = monitor.scale_factor();
    let (w, h) = ((SIZE.0 * scale).round() as i32, (SIZE.1 * scale).round() as i32);
    let margin = (MARGIN * scale).round() as i32;
    let (left, top) = (work.position.x + margin, work.position.y + margin);
    let right = (work.position.x + work.size.width as i32 - w - margin).max(left);
    let bottom = (work.position.y + work.size.height as i32 - h - margin).max(top);
    let x = match anchor {
        Some(p) => (p.x as i32 - w / 2).clamp(left, right),
        None => right,
    };
    let y = if anchor.is_some_and(|p| (p.y as i32) < work.position.y + work.size.height as i32 / 2) {
        top
    } else {
        bottom
    };
    window.set_position(PhysicalPosition::new(x, y))
}
