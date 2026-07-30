// Everything the app remembers between runs, in one file next to the models.
// These used to live in the hidden window's browser storage, which only exists
// while that window does.
//
// The file is still called tray-prefs.json: renaming it would throw away the
// dictation key and the debug switch that are already saved in it.

use crate::AudioState;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Prefs {
    /// Live microphone numbers and the per-dictation line. Off unless asked
    /// for; serde default keeps older settings files readable.
    #[serde(default)]
    pub(crate) debug_stats: bool,
    /// The key that starts and stops dictation, written the way Tauri parses
    /// it: "F3", "CommandOrControl+Shift+D".
    #[serde(default = "default_shortcut")]
    pub(crate) shortcut: String,
    #[serde(default)]
    pub(crate) active_local_model_id: Option<String>,
    /// Which microphone to record from, by the name the system gives it.
    /// None means whichever one the system has set as default.
    #[serde(default)]
    pub(crate) selected_microphone: Option<String>,
    /// Set once the settings held in the browser have been copied into here, so
    /// the copy happens exactly once and never overwrites a later change.
    #[serde(default)]
    pub(crate) migrated_from_browser: bool,
}

pub(crate) fn default_shortcut() -> String {
    "F3".to_string()
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            debug_stats: false,
            shortcut: default_shortcut(),
            active_local_model_id: None,
            selected_microphone: None,
            migrated_from_browser: false,
        }
    }
}

pub(crate) fn prefs_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("omegawhisper").join("tray-prefs.json"))
}

pub(crate) fn load_prefs() -> Prefs {
    let Some(path) = prefs_path() else {
        return Prefs::default();
    };
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!("Ignoring unreadable {}: {}", path.display(), e);
            Prefs::default()
        }),
        // Missing file just means nothing has been chosen yet.
        Err(_) => Prefs::default(),
    }
}

pub(crate) fn save_prefs(prefs: &Prefs) {
    let Some(path) = prefs_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!("Could not create {}: {}", parent.display(), e);
            return;
        }
    }
    match serde_json::to_string_pretty(prefs) {
        Ok(text) => {
            if let Err(e) = fs::write(&path, text) {
                eprintln!("Could not save {}: {}", path.display(), e);
            }
        }
        Err(e) => eprintln!("Could not encode settings: {}", e),
    }
}

// Everything the settings window shows. Read from here, not from the browser's
// own storage, so there is one answer to what a setting is.
#[tauri::command]
pub(crate) fn get_settings(state: State<'_, AudioState>) -> Prefs {
    state.prefs()
}

// What is left of the settings the hidden window kept in browser storage.
// Optional: a missing value means that window never saved it.
#[derive(serde::Deserialize)]
pub(crate) struct BrowserSettings {
    pub(crate) active_local_model_id: Option<String>,
}

// Copy the settings out of the window that used to hold them, once. Rust
// cannot read browser storage itself, so a window has to hand it over.
//
// Runs only while `migrated_from_browser` is false, and only fills in settings
// the browser actually had, so it can never wipe a later change made here.
pub(crate) fn apply_browser_settings(prefs: &mut Prefs, from: BrowserSettings) -> bool {
    if prefs.migrated_from_browser {
        return false;
    }
    prefs.migrated_from_browser = true;

    if let Some(id) = from.active_local_model_id.filter(|s| !s.is_empty()) {
        prefs.active_local_model_id = Some(id);
    }
    true
}

// Async so writing the settings file cannot freeze the screen: a plain command
// runs on the thread that draws it.
#[tauri::command]
pub(crate) async fn migrate_browser_settings(
    state: State<'_, AudioState>,
    values: BrowserSettings,
) -> Result<bool, String> {
    let mut migrated = false;
    state.update_prefs(|p| migrated = apply_browser_settings(p, values));
    if migrated {
        eprintln!("Settings copied out of the browser and saved to disk.");
    }
    Ok(migrated)
}

// The one place the debug line is switched, so the tray tick, the settings
// switch, the saved file and both windows can never disagree.
pub(crate) fn set_debug_stats_everywhere(app: &AppHandle, enabled: bool) {
    let state = app.state::<AudioState>();
    state.update_prefs(|p| p.debug_stats = enabled);
    if let Some(item) = state.debug_menu_item.lock().unwrap().as_ref() {
        let _ = item.set_checked(enabled);
    }
    let _ = app.emit("debug-stats-changed", enabled);
}

#[tauri::command]
pub(crate) fn set_debug_stats(app: AppHandle, enabled: bool) {
    set_debug_stats_everywhere(&app, enabled);
}

// Asked by each window when it opens; after that the tray sends
// "debug-stats-changed" when it is switched.
#[tauri::command]
pub(crate) fn get_debug_stats(state: State<'_, AudioState>) -> bool {
    state.prefs().debug_stats
}
