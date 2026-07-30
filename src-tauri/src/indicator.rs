// The little window with the spectrogram. Since the app has no main window it
// is the only thing the user ever sees, so Rust decides when it comes and goes
// - a hidden window used to do that, which is why errors went unseen.

use crate::AudioState;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

// Size of the spectrogram indicator window.
pub(crate) const INDICATOR_W: f64 = 460.0;

pub(crate) const INDICATOR_H: f64 = 200.0;

// Put the indicator at the bottom centre of the screen the mouse is on, so
// it appears on whichever display is being worked on.
//
// Screens share one coordinate space and only the main screen is guaranteed
// to start at zero, so the monitor's own origin has to be added or the window
// lands on another display. Called every time the indicator is shown rather
// than once at startup, so plugging a monitor in or out, or changing
// resolution, is picked up without restarting.
pub(crate) fn position_indicator(app: &AppHandle) {
    let Some(win) = app.get_webview_window("indicator") else {
        return;
    };

    // Fall back to the main screen if the pointer is somewhere with no
    // monitor, which happens briefly while displays are being rearranged.
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| win.primary_monitor().ok().flatten());
    let monitor = match monitor {
        Some(m) => m,
        None => return,
    };

    let scale = monitor.scale_factor();
    let origin_x = monitor.position().x as f64 / scale;
    let origin_y = monitor.position().y as f64 / scale;
    let screen_w = monitor.size().width as f64 / scale;
    let screen_h = monitor.size().height as f64 / scale;

    let x = origin_x + (screen_w - INDICATOR_W) / 2.0;
    let y = origin_y + screen_h - INDICATOR_H - 90.0;

    eprintln!(
        "indicator: screen {:?} {:.0}x{:.0} at ({:.0},{:.0}), scale {} -> window at ({:.0},{:.0})",
        monitor.name().map(|n| n.as_str()).unwrap_or("unnamed"),
        screen_w,
        screen_h,
        origin_x,
        origin_y,
        scale,
        x,
        y
    );

    let _ = win.set_position(tauri::LogicalPosition::new(x, y));
}

// Rust puts the indicator on screen and takes it away again. This used to be
// done by a hidden window watching the recording state, which is why an error
// nobody could see was the only sign anything had gone wrong.
pub(crate) fn show_indicator(app: &AppHandle) {
    *app.state::<AudioState>().indicator_hide_at.lock().unwrap() = None;
    let _ = app.emit("indicator-active", true);
    // Placed every time: a display may have been plugged in or resized since
    // the last dictation.
    position_indicator(app);
    if let Some(win) = app.get_webview_window("indicator") {
        let _ = win.show();
    }
}

// Take it away in a moment, leaving time to read what is on it. Only ever
// pushes the moment further out, so an error that needs ten seconds is not cut
// short by the dictation finishing right after it.
pub(crate) fn hide_indicator_in(app: &AppHandle, delay: Duration) {
    let due = std::time::Instant::now() + delay;
    let state = app.state::<AudioState>();
    let mut hide_at = state.indicator_hide_at.lock().unwrap();
    if hide_at.is_none_or(|current| due > current) {
        *hide_at = Some(due);
    }
}

pub(crate) fn hide_indicator_now(app: &AppHandle) {
    *app.state::<AudioState>().indicator_hide_at.lock().unwrap() = None;
    let _ = app.emit("indicator-active", false);
    if let Some(win) = app.get_webview_window("indicator") {
        let _ = win.hide();
    }
}

// One thread watches for the moment the indicator is due to go, so nothing
// that asks for it to be hidden has to wait around for that to happen.
pub(crate) fn watch_indicator(app: AppHandle) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(200));
        let due = *app.state::<AudioState>().indicator_hide_at.lock().unwrap();
        if due.is_some_and(|at| std::time::Instant::now() >= at) {
            hide_indicator_now(&app);
        }
    });
}

// Puts the indicator on screen long enough for a startup warning to be read.
// Called by the indicator itself, once it has asked what the warnings are.
#[tauri::command]
pub(crate) fn show_startup_warning(app: AppHandle) {
    show_indicator(&app);
    hide_indicator_in(&app, Duration::from_secs(20));
}
