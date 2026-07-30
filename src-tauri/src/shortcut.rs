// The key that starts and stops dictation, registered with the system so it
// works from any app.

use crate::AudioState;
use tauri::{AppHandle, Emitter, Manager, State};

// Register the shortcut, replacing whatever was registered before. Returns the
// reason it failed so the settings page can say why and keep the old key.
pub(crate) fn apply_shortcut(app: &AppHandle, accelerator: &str) -> Result<(), String> {
    use std::str::FromStr;
    use tauri::Manager;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

    let wanted = Shortcut::from_str(accelerator)
        .map_err(|e| format!("\"{}\" is not a key combination ({})", accelerator, e))?;

    // Let go of the old one first, or registering fails when it is the same key.
    let previous = app.state::<AudioState>().prefs().shortcut;
    if let Ok(old) = Shortcut::from_str(&previous) {
        let _ = app.global_shortcut().unregister(old);
    }

    if let Err(e) = app.global_shortcut().register(wanted) {
        // Put the old one back so the app is not left with no shortcut at all.
        if let Ok(old) = Shortcut::from_str(&previous) {
            let _ = app.global_shortcut().register(old);
        }
        return Err(format!(
            "{} could not be registered. Another app is probably using it. ({})",
            accelerator, e
        ));
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn get_shortcut(state: State<'_, AudioState>) -> String {
    state.prefs().shortcut
}

#[tauri::command]
pub(crate) fn set_shortcut(app: AppHandle, accelerator: String) -> Result<(), String> {
    let accelerator = accelerator.trim().to_string();
    if accelerator.is_empty() {
        return Err("No key was chosen.".to_string());
    }
    apply_shortcut(&app, &accelerator)?;

    app.state::<AudioState>()
        .update_prefs(|p| p.shortcut = accelerator);
    let _ = app.emit("shortcut-changed", ());
    Ok(())
}
