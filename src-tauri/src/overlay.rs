//! The recording pill — creation, placement and state, in one place.
//!
//! Two separate bugs produced the same report ("the pop-up disappears"):
//!
//! 1. The pill's screen position was computed **once**, in `setup`, from the
//!    primary monitor's logical size. Dock a laptop, unplug a monitor, change
//!    scaling or swap which display is primary and the window is still sitting
//!    at the old coordinates — frequently off-screen. It was showing; it just
//!    was not anywhere Ben could see.
//! 2. If the window was ever destroyed (see `supervisor`), every later
//!    `get_webview_window("overlay")` returned `None` and `update_overlay`
//!    silently did nothing for the rest of the session.
//!
//! So: recreate on demand, and re-place on every show.

use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::applog;

pub const LABEL: &str = "overlay";
const WIDTH: f64 = 280.0;
const HEIGHT: f64 = 64.0;
const BOTTOM_MARGIN: f64 = 32.0;

/// Bottom-centre of the monitor the cursor is on, in **physical** pixels.
///
/// Physical, not logical: `WebviewWindowBuilder::position` takes logical units
/// but `set_position` here takes a `PhysicalPosition`, and mixing the two is
/// how a correct-looking calculation lands the pill half a screen away on a
/// 150%-scaled display.
fn target_position(app: &AppHandle) -> Option<PhysicalPosition<i32>> {
    // Prefer the monitor under the cursor — that is the screen Ben is actually
    // working on, which is not always the OS "primary" one.
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten())?;

    let size = monitor.size();
    let pos = monitor.position();
    let scale = monitor.scale_factor();

    let w_px = WIDTH * scale;
    let h_px = HEIGHT * scale;
    let margin_px = BOTTOM_MARGIN * scale;

    Some(PhysicalPosition::new(
        pos.x + ((size.width as f64 - w_px) / 2.0) as i32,
        pos.y + (size.height as f64 - h_px - margin_px) as i32,
    ))
}

/// Return the overlay window, building it if it is missing.
pub fn ensure(app: &AppHandle) -> Option<WebviewWindow> {
    if let Some(w) = app.get_webview_window(LABEL) {
        return Some(w);
    }

    applog::log("Overlay window missing — rebuilding");
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("src/overlay.html".into()))
        // The pill renders WebView2's own error page when this navigation
        // fails, which looks like "the app is broken" and says nothing about
        // why. Log the URL actually attempted so the next failure names
        // itself instead of needing a screenshot to find.
        .on_navigation(|url| {
            applog::log(&format!("Overlay navigating to: {}", url));
            true
        })
        .title("")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .shadow(false)
        .visible(false)
        .build();

    match built {
        Ok(w) => {
            applog::log(&format!(
                "Overlay built, url={}",
                w.url().map(|u| u.to_string()).unwrap_or_else(|_| "<unknown>".into())
            ));
            place(app, &w);
            Some(w)
        }
        Err(e) => {
            applog::log(&format!("Failed to build overlay: {}", e));
            None
        }
    }
}

/// Move the pill to the bottom-centre of the monitor in use right now.
pub fn place(app: &AppHandle, window: &WebviewWindow) {
    if let Some(p) = target_position(app) {
        let _ = window.set_position(p);
    }
}

/// Show the pill in `state`, re-placing and re-asserting always-on-top first.
///
/// `always_on_top` is re-applied on every show because another app going
/// full-screen (a game, a video call, a screen share) can leave a topmost
/// window stacked underneath it until it is set again.
pub fn show_state(app: &AppHandle, state: &str) {
    let Some(w) = ensure(app) else { return };
    place(app, &w);
    let _ = w.set_always_on_top(true);
    let _ = w.show();
    let _ = w.eval(&format!(
        "document.getElementById('bar')?.setAttribute('data-state', '{}');",
        state
    ));
}

pub fn hide(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
        let _ = w.eval("document.getElementById('bar')?.setAttribute('data-state', 'ready');");
    }
}

/// Flash an error on the pill, then hide it. Used when the pipeline fails, so
/// a failure is visible instead of silently pasting nothing.
pub fn flash_error(app: &AppHandle, message: &str) {
    let Some(w) = ensure(app) else { return };
    place(app, &w);
    let _ = w.set_always_on_top(true);
    let _ = w.show();
    let safe = message.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', " ");
    let _ = w.eval(&format!(
        "document.getElementById('bar')?.setAttribute('data-state','error');\
         window.__showError && window.__showError('{}');",
        safe
    ));

    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(2600));
        hide(&app);
    });
}
