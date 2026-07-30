// The menu-bar icon: which picture it shows, and how it moves while a
// recording is running.

use crate::AudioState;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Manager};

// Which picture the menu bar shows. One line to change; the frames for all
// three are in the repo, so nothing has to be generated to switch.
#[cfg(desktop)]
pub(crate) const TRAY_ICON: TrayIconStyle = TrayIconStyle::Key;

// Only one of these is picked at a time, so the other two always look unused.
#[cfg(desktop)]
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum TrayIconStyle {
    /// A key that goes down while the app is listening and comes back up when
    /// it stops - the same key you press to start.
    Key,
    /// A switch whose dot moves to the right while the app is listening.
    Switch,
    /// The plain letter. It has one frame, so it never moves.
    Omega,
}

// Every frame is a 36x36 PNG of black on clear, because macOS is told to treat
// the menu-bar icon as a template: it throws the colour away and keeps the
// shape, so it can draw it dark or light to match the bar. Order matters -
// first frame is idle, last frame is recording, and the ones between are only
// there so the change reads as movement.
#[cfg(desktop)]
pub(crate) const KEY_FRAMES: [&[u8]; 3] = [
    include_bytes!("../icons/tray/key-up.png"),
    include_bytes!("../icons/tray/key-mid.png"),
    include_bytes!("../icons/tray/key-down.png"),
];

#[cfg(desktop)]
pub(crate) const SWITCH_FRAMES: [&[u8]; 3] = [
    include_bytes!("../icons/tray/switch-off.png"),
    include_bytes!("../icons/tray/switch-mid.png"),
    include_bytes!("../icons/tray/switch-on.png"),
];

#[cfg(desktop)]
pub(crate) const OMEGA_FRAMES: [&[u8]; 1] = [include_bytes!("../icons/tray-icon.png")];

#[cfg(desktop)]
pub(crate) fn tray_frames() -> &'static [&'static [u8]] {
    match TRAY_ICON {
        TrayIconStyle::Key => &KEY_FRAMES,
        TrayIconStyle::Switch => &SWITCH_FRAMES,
        TrayIconStyle::Omega => &OMEGA_FRAMES,
    }
}

// Put one frame on the menu bar. Silent about failure on purpose: a picture
// that will not decode is not a reason to interrupt a dictation.
#[cfg(desktop)]
pub(crate) fn show_tray_frame(app: &AppHandle, frame: usize) {
    let frames = tray_frames();
    let Some(bytes) = frames.get(frame) else {
        return;
    };
    let Ok(image) = tauri::image::Image::from_bytes(bytes) else {
        return;
    };
    let state = app.state::<AudioState>();
    let tray = state.tray_icon.lock().unwrap();
    if let Some(tray) = tray.as_ref() {
        // Picture and template flag together: setting them one after the other
        // makes the icon visibly flicker on macOS.
        let _ = tray.set_icon_with_as_template(Some(image), true);
    }
}

// The menu-bar icon follows the recording state by watching it, rather than
// being told. Every way a recording can end - the key, the button, a
// microphone that dies halfway - already puts the flag back, so watching it is
// the only version of this that cannot be forgotten in a new code path.
#[cfg(desktop)]
pub(crate) fn watch_tray_icon(app: AppHandle) {
    if tray_frames().len() < 2 {
        return; // a still picture has nothing to play
    }
    thread::spawn(move || {
        let mut showing_recording = false;
        loop {
            thread::sleep(Duration::from_millis(50));
            let recording = *app.state::<AudioState>().is_recording.lock().unwrap();
            if recording == showing_recording {
                continue;
            }
            showing_recording = recording;
            // Forwards through the frames when a recording starts, backwards
            // when it ends. The first frame is already on screen, so the press
            // begins at the second one.
            let last = tray_frames().len() - 1;
            let frames: Vec<usize> = if recording {
                (1..=last).collect()
            } else {
                (0..last).rev().collect()
            };
            for frame in frames {
                show_tray_frame(&app, frame);
                thread::sleep(Duration::from_millis(45));
            }
        }
    });
}
