mod analysis;
mod chime;
mod indicator;
mod managers;
mod microphone;
mod models;
mod recording;
mod resampler;
mod settings;
mod shortcut;
mod storage;
mod tray;
mod typing;

#[cfg(test)]
mod tests;

use chime::{play_chime, DONE_CHIME};
use chrono::Local;
use cpal::traits::{DeviceTrait, StreamTrait};
use indicator::{
    hide_indicator_in, position_indicator, show_indicator, watch_indicator, INDICATOR_H,
    INDICATOR_W,
};
use managers::{ModelManager, SharedTranscriptionManager};
use microphone::{get_input_device, get_safe_input_config};
use recording::toggle_recording;
use settings::{load_prefs, save_prefs, set_debug_stats_everywhere, Prefs};
use shortcut::apply_shortcut;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use storage::{
    delete_recordings_in, get_recordings_dir, migrate_legacy_data_dir, redirect_output_to_log,
};
use tauri::{AppHandle, Emitter, State};
#[cfg(desktop)]
use tray::{tray_frames, watch_tray_icon};
#[cfg(target_os = "macos")]
use typing::accessibility_granted;
#[cfg(target_os = "linux")]
use zbus::interface;

// Application state for audio recording
pub struct AudioState {
    is_recording: Arc<Mutex<bool>>,
    recorded_samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: Arc<Mutex<Option<u32>>>,
    stop_signal: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>,
    model_manager: Arc<ModelManager>,
    transcription_manager: SharedTranscriptionManager,
    // Problems found at startup. Kept until a window asks: emitting them as
    // they happen is too early, no window is listening yet.
    startup_warnings: Arc<Mutex<Vec<String>>>,
    // The tray's tick for the debug line, so the settings switch can move it
    // too. Without this the two disagree until the next restart.
    debug_menu_item: Arc<Mutex<Option<tauri::menu::CheckMenuItem<tauri::Wry>>>>,
    // Every saved setting. One copy, in memory and on disk together.
    prefs: Arc<Mutex<Prefs>>,
    // When the indicator is due to leave the screen, if it is on it.
    indicator_hide_at: Arc<Mutex<Option<std::time::Instant>>>,
    // The menu-bar icon, kept so its picture can be changed while the app runs.
    #[cfg(desktop)]
    tray_icon: Arc<Mutex<Option<tauri::tray::TrayIcon<tauri::Wry>>>>,
}

impl AudioState {
    fn prefs(&self) -> Prefs {
        self.prefs.lock().unwrap().clone()
    }

    // Change a setting and write it out in the same breath, so what is on
    // screen and what is on disk can never drift apart.
    fn update_prefs(&self, change: impl FnOnce(&mut Prefs)) {
        let mut prefs = self.prefs.lock().unwrap();
        change(&mut prefs);
        save_prefs(&prefs);
    }
}

// D-Bus service for external control (Linux only)
#[cfg(target_os = "linux")]
struct OmegawhisperDBus {
    app_handle: AppHandle,
}

#[cfg(target_os = "linux")]
#[interface(name = "dev.omegawhisper")]
impl OmegawhisperDBus {
    async fn toggle_recording(&self) -> bool {
        toggle_recording(&self.app_handle);
        true
    }
}

// Emits "transcription-complete" however the transcription thread ends, panic
// included. Without it a crash inside the model leaves both windows stuck on
// "Transcribing..." until the app is restarted.
struct CompleteOnDrop(AppHandle);

impl Drop for CompleteOnDrop {
    fn drop(&mut self) {
        let _ = self.0.emit("transcription-complete", ());
    }
}

// Transcription event payload
#[derive(Clone, serde::Serialize)]
struct TranscriptionEvent {
    text: String,
    is_final: bool,
}

// Live microphone numbers, sent to the indicator window while recording.
// This is the audio the recording itself gets, not what the browser side
// hears, so it shows whether the recording is picking up any sound at all.
#[derive(Clone, serde::Serialize)]
struct MicLevel {
    // Loudest sample of the last chunk, 0.0 to 1.0.
    peak: f32,
    // Average level of the last chunk. Normal speech sits near 0.05.
    rms: f32,
    // Seconds since this recording started.
    seconds: f32,
    // Base frequency of the voice in Hz, 0 when it cannot be told.
    pitch: f32,
    // One value per frequency band, 0 to 1, for the bars the windows draw.
    bands: Vec<f32>,
}

// What one finished local dictation did, shown in the main window so the
// numbers behind a bad result are visible without reading a log file.
#[derive(Clone, serde::Serialize)]
struct DictationStats {
    model: String,
    // Length of the audio handed to the model.
    seconds: f32,
    // Loudness of the spoken parts, before and after the level boost.
    level_before: f32,
    level_after: f32,
    gain: f32,
    // Seconds spent inside the model.
    took: f32,
    // Characters of text it returned. 0 means it returned nothing.
    chars: usize,
}

// ============================================================================
// Multi-Model Management Commands
// ============================================================================

// A missing shortcut or permission is invisible otherwise: the app sits in the
// tray looking healthy and simply does nothing.
#[tauri::command]
fn get_startup_warnings(state: State<'_, AudioState>) -> Vec<String> {
    state.startup_warnings.lock().unwrap().clone()
}

// Wall-clock time for the log, so a recording that nobody meant to start can
// be matched against what was happening on screen at that moment.
fn now() -> String {
    Local::now().format("%H:%M:%S%.3f").to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(unix)]
    redirect_output_to_log();

    // Rename the old data folder before any code reads or creates it.
    migrate_legacy_data_dir();

    // Say at startup whether text can be typed into other apps. This is
    // granted per bundle identifier, so it is lost whenever the app is
    // renamed or reinstalled under a new identifier.
    let mut startup_warnings: Vec<String> = Vec::new();

    #[cfg(target_os = "macos")]
    if accessibility_granted() {
        eprintln!("Accessibility permission: granted (auto-type can work).");
    } else {
        eprintln!(
            "Accessibility permission: NOT granted. Auto-type will do nothing. \
             Add Omegawhisper in System Settings > Privacy & Security > Accessibility."
        );
        startup_warnings.push(
            "Text cannot be typed into other apps: Omegawhisper is not allowed in \
             System Settings > Privacy & Security > Accessibility."
                .to_string(),
        );
    }

    // Ask for the microphone now, not at the first F3.
    //
    // macOS asks the moment an app first opens the microphone. That used to be
    // in the middle of the first dictation: the permission window appeared,
    // took the keyboard away, and the first seconds of speech were lost. This
    // opens the microphone for a moment at startup so the question is asked
    // and answered before any recording. The permission is tied to the app's
    // signature, so a rebuilt app is a new app to macOS and is asked again.
    thread::spawn(|| {
        let device = match get_input_device(None) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Microphone check: no input device ({})", e);
                return;
            }
        };
        let config = match get_safe_input_config(&device) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Microphone check: no usable input format ({})", e);
                return;
            }
        };
        let stream = device.build_input_stream_raw(
            &config.config(),
            config.sample_format(),
            |_, _| {},
            |e| eprintln!("Microphone check: {}", e),
            None,
        );
        match stream {
            Ok(s) => {
                let _ = s.play();
                thread::sleep(Duration::from_millis(300));
                eprintln!("Microphone permission: granted (recording can work).");
            }
            Err(e) => eprintln!(
                "Microphone permission: NOT granted or device unusable ({}). \
                 Allow Omegawhisper in System Settings > Privacy & Security > Microphone.",
                e
            ),
        }
    });

    // Everything chosen on the last run.
    let saved_prefs = load_prefs();

    // Initialize model manager
    let model_manager = Arc::new(ModelManager::new().expect("Failed to initialize model manager"));

    // Initialize transcription manager
    let transcription_manager = SharedTranscriptionManager::new(model_manager.clone());

    let audio_state = AudioState {
        is_recording: Arc::new(Mutex::new(false)),
        recorded_samples: Arc::new(Mutex::new(Vec::new())),
        sample_rate: Arc::new(Mutex::new(None)),
        stop_signal: Arc::new(Mutex::new(None)),
        model_manager,
        transcription_manager,
        startup_warnings: Arc::new(Mutex::new(startup_warnings)),
        debug_menu_item: Arc::new(Mutex::new(None)),
        prefs: Arc::new(Mutex::new(saved_prefs)),
        indicator_hide_at: Arc::new(Mutex::new(None)),
        #[cfg(desktop)]
        tray_icon: Arc::new(Mutex::new(None)),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        // Global shortcut toggles recording from anywhere.
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    use tauri_plugin_global_shortcut::ShortcutState;
                    if event.state() == ShortcutState::Pressed {
                        eprintln!("[{}] shortcut pressed", now());
                        toggle_recording(app);
                    }
                })
                .build(),
        )
        .manage(audio_state)
        .invoke_handler(tauri::generate_handler![
            recording::start_recording,
            recording::stop_recording,
            microphone::list_audio_devices,
            microphone::set_selected_device,
            settings::get_settings,
            settings::migrate_browser_settings,
            models::list_available_models,
            models::download_model,
            models::delete_model,
            models::set_active_model,
            models::get_active_model,
            indicator::show_startup_warning,
            settings::get_debug_stats,
            settings::set_debug_stats,
            shortcut::get_shortcut,
            shortcut::set_shortcut,
            get_startup_warnings,
        ])
        .setup(|app| {
            // The saved key toggles recording from anywhere.
            #[cfg(desktop)]
            {
                use tauri::Manager;
                let handle = app.handle().clone();
                let wanted = handle.state::<AudioState>().prefs().shortcut;
                if let Err(e) = apply_shortcut(&handle, &wanted) {
                    eprintln!("{}", e);
                    handle
                        .state::<AudioState>()
                        .startup_warnings
                        .lock()
                        .unwrap()
                        .push(format!(
                            "{} The shortcut will not work until it is changed in Settings.",
                            e
                        ));
                }
            }

            // The indicator is the only thing on screen, so Rust decides when
            // it comes and goes. A hidden window used to do this.
            #[cfg(desktop)]
            {
                use tauri::Listener;
                watch_indicator(app.handle().clone());

                let handle = app.handle().clone();
                app.listen_any("transcription-complete", move |_| {
                    play_chime(&DONE_CHIME);
                    hide_indicator_in(&handle, Duration::from_millis(400));
                });

                // Errors can arrive after the indicator has already gone, so
                // bring it back rather than only delaying its exit.
                let handle = app.handle().clone();
                app.listen_any("transcription-error", move |_| {
                    show_indicator(&handle);
                    hide_indicator_in(&handle, Duration::from_secs(10));
                });
            }

            // Menu-bar tray: recordings, debug line, settings, quit.
            #[cfg(desktop)]
            {
                use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
                use tauri::tray::TrayIconBuilder;
                use tauri::Manager;

                let settings_item = MenuItem::with_id(
                    app,
                    "open_settings_window",
                    "Settings...",
                    true,
                    None::<&str>,
                )?;
                let recordings_open =
                    MenuItem::with_id(app, "open_recordings", "Open Folder", true, None::<&str>)?;
                let recordings_delete = MenuItem::with_id(
                    app,
                    "delete_recordings",
                    "Delete Recordings",
                    true,
                    None::<&str>,
                )?;
                let recordings_item = Submenu::with_items(
                    app,
                    "Recordings",
                    true,
                    &[&recordings_open, &recordings_delete],
                )?;

                // Live microphone numbers on the indicator and the line under
                // the text. Useful when a dictation goes wrong, noise the rest
                // of the time, so it stays off until asked for.
                let saved_debug = app.state::<AudioState>().prefs().debug_stats;
                let debug_item = CheckMenuItem::with_id(
                    app,
                    "debug_stats",
                    "Show debug stats",
                    true,
                    saved_debug,
                    None::<&str>,
                )?;

                let quit_item =
                    MenuItem::with_id(app, "quit", "Quit Omegawhisper", true, None::<&str>)?;
                let sep = PredefinedMenuItem::separator(app)?;
                let menu = Menu::with_items(
                    app,
                    &[
                        &recordings_item,
                        &debug_item,
                        &settings_item,
                        &sep,
                        &quit_item,
                    ],
                )?;

                *app.state::<AudioState>().debug_menu_item.lock().unwrap() =
                    Some(debug_item.clone());

                let mut tray = TrayIconBuilder::new()
                    .menu(&menu)
                    .tooltip("Omegawhisper — press F3 to dictate")
                    .on_menu_event(move |app, event| match event.id.as_ref() {
                        "open_settings_window" => {
                            // regular app so the window is focusable
                            #[cfg(target_os = "macos")]
                            let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
                            if let Some(w) = app.get_webview_window("settings") {
                                let _ = w.show();
                                let _ = w.set_focus();
                            } else {
                                use tauri::{WebviewUrl, WebviewWindowBuilder};
                                // same window shape as the in-app settings button
                                match WebviewWindowBuilder::new(
                                    app,
                                    "settings",
                                    WebviewUrl::App("settings".into()),
                                )
                                .title("Settings")
                                .inner_size(450.0, 550.0)
                                .decorations(false)
                                .transparent(true)
                                .resizable(false)
                                .center()
                                .build()
                                {
                                    Ok(w) => {
                                        let _ = w.set_focus();
                                        // back to a background menu-bar agent
                                        // once settings closes
                                        #[cfg(target_os = "macos")]
                                        {
                                            let handle = app.clone();
                                            w.on_window_event(move |event| {
                                                if let tauri::WindowEvent::Destroyed = event {
                                                    let _ = handle.set_activation_policy(
                                                        tauri::ActivationPolicy::Accessory,
                                                    );
                                                }
                                            });
                                        }
                                    }
                                    Err(e) => eprintln!("Failed to create settings window: {}", e),
                                }
                            }
                        }
                        "open_recordings" => {
                            // Every dictation leaves two WAV files here and
                            // nothing removes them, so make the folder reachable.
                            match get_recordings_dir() {
                                Ok(dir) => {
                                    if let Err(e) =
                                        tauri_plugin_opener::open_path(&dir, None::<&str>)
                                    {
                                        eprintln!("Could not open {}: {}", dir.display(), e);
                                    }
                                }
                                Err(e) => eprintln!("No recordings folder: {}", e),
                            }
                        }
                        "delete_recordings" => {
                            // Irreversible, so it asks first. show() takes a
                            // callback rather than blocking: the tray handler
                            // runs on the main thread, and waiting for a window
                            // there would freeze the app.
                            use tauri_plugin_dialog::{
                                DialogExt, MessageDialogButtons, MessageDialogKind,
                            };
                            let handle = app.clone();
                            app.dialog()
                                .message(
                                    "This action will delete all LOCAL recordings from your \
                                     hard drive.",
                                )
                                .title("Delete recordings")
                                .kind(MessageDialogKind::Warning)
                                .buttons(MessageDialogButtons::OkCancelCustom(
                                    "Delete".to_string(),
                                    "Cancel".to_string(),
                                ))
                                .show(move |confirmed| {
                                    if !confirmed {
                                        return;
                                    }
                                    match get_recordings_dir()
                                        .and_then(|dir| delete_recordings_in(&dir))
                                    {
                                        Ok(count) => {
                                            eprintln!("Deleted {} recordings", count);
                                            let _ = handle.emit("recordings-deleted", count);
                                        }
                                        Err(e) => {
                                            eprintln!("Could not delete recordings: {}", e);
                                            let _ = handle.emit("transcription-error", e);
                                        }
                                    }
                                });
                        }
                        "debug_stats" => {
                            let now_on = !app.state::<AudioState>().prefs().debug_stats;
                            set_debug_stats_everywhere(app, now_on);
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    });

                // The idle frame. Template icon renders white on the macOS
                // menu bar; watch_tray_icon changes the frame from here on.
                if let Ok(icon) = tauri::image::Image::from_bytes(tray_frames()[0]) {
                    tray = tray.icon(icon).icon_as_template(true);
                } else if let Some(icon) = app.default_window_icon().cloned() {
                    tray = tray.icon(icon).icon_as_template(true);
                }
                *app.state::<AudioState>().tray_icon.lock().unwrap() = Some(tray.build(app)?);
                watch_tray_icon(app.handle().clone());
            }

            // Start as a background menu-bar agent (no Dock icon, no Cmd-Tab).
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Hidden indicator window (the recording waterfall spectrogram).
            #[cfg(desktop)]
            {
                use tauri::{WebviewUrl, WebviewWindowBuilder};

                let ind_w = INDICATOR_W;
                let ind_h = INDICATOR_H;
                match WebviewWindowBuilder::new(
                    app,
                    "indicator",
                    WebviewUrl::App("indicator".into()),
                )
                .title("Omegawhisper indicator")
                .inner_size(ind_w, ind_h)
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                // focused(false) only covers the moment it is built. focusable
                // (false) is what stops it taking the keyboard away from the
                // app you are dictating into every time it is shown.
                .focused(false)
                .focusable(false)
                .resizable(false)
                .shadow(false)
                .visible(false)
                .build()
                {
                    Ok(_) => {
                        // Placed by position_indicator, which runs again every
                        // time the window is shown.
                        position_indicator(app.handle());
                    }
                    Err(e) => eprintln!("Failed to create indicator window: {}", e),
                }
            }

            // Spawn D-Bus service for external control (Linux only)
            #[cfg(target_os = "linux")]
            {
                let handle = app.handle().clone();

                tauri::async_runtime::spawn(async move {
                    let service = OmegawhisperDBus { app_handle: handle };

                    match zbus::connection::Builder::session()
                        .and_then(|b| b.name("dev.omegawhisper"))
                        .and_then(|b| b.serve_at("/dev/omegawhisper", service))
                    {
                        Ok(builder) => {
                            match builder.build().await {
                                Ok(_conn) => {
                                    // Keep connection alive
                                    std::future::pending::<()>().await;
                                }
                                Err(e) => eprintln!("Failed to build D-Bus connection: {}", e),
                            }
                        }
                        Err(e) => eprintln!("Failed to setup D-Bus service: {}", e),
                    }
                });
            }

            let _ = app; // Silence unused warning on non-Linux
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
