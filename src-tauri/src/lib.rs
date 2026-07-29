mod audio;
mod managers;
mod resampler;

use audio::VadProcessor;
use chrono::{Local, Utc};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SupportedStreamConfig};
use managers::{ModelManager, ModelStatus, SharedTranscriptionManager, AVAILABLE_MODELS};
use resampler::AudioResampler;
use sha2::{Digest, Sha256};
use std::fs;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use url::Url;
#[cfg(target_os = "linux")]
use zbus::interface;

// Application state for audio recording
pub struct AudioState {
    is_recording: Arc<Mutex<bool>>,
    recorded_samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: Arc<Mutex<Option<u32>>>,
    stop_signal: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>,
    api_key: Arc<Mutex<Option<String>>>,
    // Hyperwhisper server settings
    use_hyperwhisper_server: Arc<Mutex<bool>>,
    hyperwhisper_server_url: Arc<Mutex<String>>,
    hyperwhisper_server_https: Arc<Mutex<bool>>,
    hyperwhisper_api_key: Arc<Mutex<Option<String>>>,
    // Real-time typing: type transcription as it streams in
    auto_type_transcription: Arc<Mutex<bool>>,
    // Selected audio input device ID from WirePlumber (None = auto-select)
    selected_device_id: Arc<Mutex<Option<u32>>>,
    // Local transcription settings
    use_local_transcription: Arc<Mutex<bool>>,
    local_model_path: Arc<Mutex<Option<String>>>,
    // Multi-model local transcription
    active_local_model_id: Arc<Mutex<Option<String>>>,
    model_manager: Arc<ModelManager>,
    transcription_manager: SharedTranscriptionManager,
    // VAD enabled flag
    use_vad: Arc<Mutex<bool>>,
    // show the live microphone numbers and the per-dictation line
    debug_stats: Arc<Mutex<bool>>,
    // Problems found at startup. Kept until a window asks: emitting them as
    // they happen is too early, no window is listening yet.
    startup_warnings: Arc<Mutex<Vec<String>>>,
    // The tray's tick for the debug line, so the settings switch can move it
    // too. Without this the two disagree until the next restart.
    debug_menu_item: Arc<Mutex<Option<tauri::menu::CheckMenuItem<tauri::Wry>>>>,
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
        // Emit event to frontend to toggle recording
        let _ = self.app_handle.emit("recording-toggled", ());
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

// Trial key API response types
#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct TrialProvisionResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub key_prefix: String,
    pub remaining_duration_seconds: f64,
    pub remaining_sessions: i64,
    pub max_session_duration_seconds: f64,
    pub expires_at: String,
    pub quota_exceeded: bool,
    pub expired: bool,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct TrialStatusResponse {
    pub active: bool,
    pub remaining_duration_seconds: f64,
    pub remaining_sessions: i64,
    pub expires_at: String,
    pub expired: bool,
    pub quota_exceeded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upgrade_url: Option<String>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct TrialUsageResponse {
    pub total_duration_seconds: f64,
    pub total_sessions: i64,
    pub remaining_duration_seconds: f64,
    pub remaining_sessions: i64,
    pub max_duration_seconds: f64,
    pub max_sessions: i64,
    pub max_session_duration_seconds: f64,
    pub quota_exceeded: bool,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct TrialError {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<TrialErrorDetails>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct TrialErrorDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upgrade_url: Option<String>,
}

// Generate a stable device fingerprint
fn generate_device_fingerprint() -> String {
    let mut hasher = Sha256::new();

    // Try to read machine-id (Linux standard)
    if let Ok(machine_id) = fs::read_to_string("/etc/machine-id") {
        hasher.update(machine_id.trim().as_bytes());
    } else if let Ok(machine_id) = fs::read_to_string("/var/lib/dbus/machine-id") {
        hasher.update(machine_id.trim().as_bytes());
    } else {
        // Fallback: use hostname and username
        if let Ok(hostname) = std::env::var("HOSTNAME")
            .or_else(|_| fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()))
        {
            hasher.update(hostname.as_bytes());
        }
        if let Ok(user) = std::env::var("USER") {
            hasher.update(user.as_bytes());
        }
    }

    // Add some hardware info if available
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        // Extract CPU model name for additional uniqueness
        for line in cpuinfo.lines() {
            if line.starts_with("model name") {
                hasher.update(line.as_bytes());
                break;
            }
        }
    }

    hex::encode(hasher.finalize())
}

// Get the base URL for the Hyperwhisper API
fn get_hyperwhisper_api_base(server_url: &str, use_https: bool) -> String {
    let protocol = if use_https { "https" } else { "http" };
    format!("{}://{}", protocol, server_url)
}

// Internal function to provision trial key (used by both command and auto-provision)
fn provision_trial_key_internal(
    server_url: &str,
    use_https: bool,
) -> Result<TrialProvisionResponse, String> {
    let fingerprint = generate_device_fingerprint();
    let base_url = get_hyperwhisper_api_base(server_url, use_https);
    let url = format!("{}/api/v1/trial/provision", base_url);

    let response = ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_json(serde_json::json!({
            "device_fingerprint": fingerprint
        }))
        .map_err(|e| {
            // Try to extract error message from response body
            if let ureq::Error::Status(code, response) = e {
                if let Ok(error_body) = response.into_json::<TrialError>() {
                    return format!("{}: {}", code, error_body.error);
                }
                return format!("{}: Request failed", code);
            }
            format!("Failed to provision trial key: {}", e)
        })?;

    let trial_response: TrialProvisionResponse = response
        .into_json()
        .map_err(|e| format!("Failed to parse trial response: {}", e))?;

    Ok(trial_response)
}

// Provision or retrieve a trial key
#[tauri::command]
async fn provision_trial_key(
    state: State<'_, AudioState>,
) -> Result<TrialProvisionResponse, String> {
    let server_url = state.hyperwhisper_server_url.lock().unwrap().clone();
    let use_https = *state.hyperwhisper_server_https.lock().unwrap();

    provision_trial_key_internal(&server_url, use_https)
}

// Check trial key status
#[tauri::command]
async fn get_trial_status(
    state: State<'_, AudioState>,
    api_key: String,
) -> Result<TrialStatusResponse, String> {
    let server_url = state.hyperwhisper_server_url.lock().unwrap().clone();
    let use_https = *state.hyperwhisper_server_https.lock().unwrap();

    let base_url = get_hyperwhisper_api_base(&server_url, use_https);
    let url = format!("{}/api/v1/trial/status", base_url);

    let response = ureq::get(&url)
        .set("X-API-Key", &api_key)
        .call()
        .map_err(|e| {
            if let ureq::Error::Status(code, response) = e {
                if let Ok(error_body) = response.into_json::<TrialError>() {
                    return format!("{}: {}", code, error_body.error);
                }
                return format!("{}: Request failed", code);
            }
            format!("Failed to get trial status: {}", e)
        })?;

    let status_response: TrialStatusResponse = response
        .into_json()
        .map_err(|e| format!("Failed to parse trial status: {}", e))?;

    Ok(status_response)
}

// Get trial usage statistics
#[tauri::command]
async fn get_trial_usage(
    state: State<'_, AudioState>,
    api_key: String,
) -> Result<TrialUsageResponse, String> {
    let server_url = state.hyperwhisper_server_url.lock().unwrap().clone();
    let use_https = *state.hyperwhisper_server_https.lock().unwrap();

    let base_url = get_hyperwhisper_api_base(&server_url, use_https);
    let url = format!("{}/api/v1/trial/usage", base_url);

    let response = ureq::get(&url)
        .set("X-API-Key", &api_key)
        .call()
        .map_err(|e| {
            if let ureq::Error::Status(code, response) = e {
                if let Ok(error_body) = response.into_json::<TrialError>() {
                    return format!("{}: {}", code, error_body.error);
                }
                return format!("{}: Request failed", code);
            }
            format!("Failed to get trial usage: {}", e)
        })?;

    let usage_response: TrialUsageResponse = response
        .into_json()
        .map_err(|e| format!("Failed to parse trial usage: {}", e))?;

    Ok(usage_response)
}

// Get the device fingerprint (for debugging/display purposes)
#[tauri::command]
fn get_device_fingerprint() -> String {
    generate_device_fingerprint()
}

// Get the recordings directory, creating it if necessary
fn get_recordings_dir() -> Result<PathBuf, String> {
    let data_dir =
        dirs::data_local_dir().ok_or_else(|| "Could not find local data directory".to_string())?;
    let recordings_dir = data_dir.join("omegawhisper").join("recordings");

    if !recordings_dir.exists() {
        fs::create_dir_all(&recordings_dir)
            .map_err(|e| format!("Failed to create recordings directory: {}", e))?;
    }

    Ok(recordings_dir)
}

// The tray menu has no browser storage behind it, so its choices are kept in a
// small file next to the models.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct TrayPrefs {
    /// Live microphone numbers and the per-dictation line. Off unless asked
    /// for; serde default keeps older settings files readable.
    #[serde(default)]
    debug_stats: bool,
}

fn tray_prefs_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("omegawhisper").join("tray-prefs.json"))
}

fn load_tray_prefs() -> TrayPrefs {
    let Some(path) = tray_prefs_path() else {
        return TrayPrefs::default();
    };
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!("Ignoring unreadable {}: {}", path.display(), e);
            TrayPrefs::default()
        }),
        // Missing file just means nothing has been chosen yet.
        Err(_) => TrayPrefs::default(),
    }
}

fn save_tray_prefs(prefs: &TrayPrefs) {
    let Some(path) = tray_prefs_path() else {
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
        Err(e) => eprintln!("Could not encode tray settings: {}", e),
    }
}

// The data folder was called "hyperwhisper" before the app was renamed to
// Omegawhisper. Move it to the new name once, so the downloaded models
// (several GB) and old recordings are kept instead of downloaded again.
// Must run before anything else touches the data folder.
fn migrate_legacy_data_dir() {
    let data_dir = match dirs::data_local_dir() {
        Some(d) => d,
        None => return,
    };
    let old_dir = data_dir.join("hyperwhisper");
    let new_dir = data_dir.join("omegawhisper");

    if !old_dir.is_dir() {
        return;
    }

    if let Ok(mut entries) = fs::read_dir(&new_dir) {
        if entries.next().is_some() {
            // Both folders hold data - do not touch either one.
            eprintln!(
                "Data folder migration skipped: {} already has files. Old data is still in {}",
                new_dir.display(),
                old_dir.display()
            );
            return;
        }
        // New folder exists but is empty: remove it so the rename can use the name.
        if let Err(e) = fs::remove_dir(&new_dir) {
            eprintln!("Could not remove empty {}: {}", new_dir.display(), e);
            return;
        }
    }

    match fs::rename(&old_dir, &new_dir) {
        Ok(()) => eprintln!("Moved {} to {}", old_dir.display(), new_dir.display()),
        Err(e) => eprintln!(
            "Failed to move {} to {}: {}",
            old_dir.display(),
            new_dir.display(),
            e
        ),
    }
}

#[tauri::command]
fn set_auto_type_transcription(state: State<'_, AudioState>, enabled: bool) {
    *state.auto_type_transcription.lock().unwrap() = enabled;
}

// WirePlumber device info with ID for selection
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct WpDevice {
    pub id: u32,
    pub name: String,
    pub is_default: bool,
}

#[tauri::command]
fn list_audio_devices() -> Result<Vec<WpDevice>, String> {
    // Use wpctl to get WirePlumber audio sources (input devices)
    let output = std::process::Command::new("wpctl")
        .args(["status"])
        .output()
        .map_err(|e| format!("Failed to run wpctl: {}", e))?;

    let status = String::from_utf8_lossy(&output.stdout);
    let mut devices = Vec::new();
    let mut in_audio_section = false;
    let mut in_sources_section = false;
    let mut in_filters_section = false;
    let mut in_devices_section = false;

    // First pass: collect device names from Devices section (for friendly Bluetooth names)
    let mut bluetooth_device_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for line in status.lines() {
        if line.starts_with("Audio") {
            in_audio_section = true;
            continue;
        }
        if line.starts_with("Video") || line.starts_with("Settings") {
            in_audio_section = false;
            in_devices_section = false;
            continue;
        }

        if !in_audio_section {
            continue;
        }

        if line.contains("├─ Devices:") || line.contains("└─ Devices:") {
            in_devices_section = true;
            continue;
        }

        if in_devices_section && (line.contains("├─") || line.contains("└─")) {
            in_devices_section = false;
            continue;
        }

        if in_devices_section {
            let trimmed = line.trim_start_matches([' ', '│', '├', '─', '*']);
            if let Some(dot_pos) = trimmed.find(". ") {
                let rest = &trimmed[dot_pos + 2..];
                // Check if it's a Bluetooth device
                if rest.contains("[bluez5]") {
                    let name = rest.replace("[bluez5]", "").trim().to_string();
                    // Store with lowercase for matching
                    bluetooth_device_names.insert(name.to_lowercase(), name);
                }
            }
        }
    }

    // Reset for second pass
    in_audio_section = false;

    for line in status.lines() {
        // Track when we enter/exit the Audio section
        if line.starts_with("Audio") {
            in_audio_section = true;
            continue;
        }
        if line.starts_with("Video") || line.starts_with("Settings") {
            in_audio_section = false;
            in_sources_section = false;
            in_filters_section = false;
            continue;
        }

        if !in_audio_section {
            continue;
        }

        // Look for the Sources section under Audio
        if line.contains("├─ Sources:") || line.contains("└─ Sources:") {
            in_sources_section = true;
            in_filters_section = false;
            continue;
        }

        // Look for the Filters section (contains Bluetooth audio sources)
        if line.contains("├─ Filters:") || line.contains("└─ Filters:") {
            in_filters_section = true;
            in_sources_section = false;
            continue;
        }

        // Exit sections when we hit another section
        if (in_sources_section || in_filters_section)
            && (line.contains("├─") || line.contains("└─"))
        {
            in_sources_section = false;
            in_filters_section = false;
            continue;
        }

        if in_sources_section {
            let trimmed = line.trim_start_matches([' ', '│', '├', '─']);

            if trimmed.is_empty() {
                continue;
            }

            let trimmed = trimmed.trim_start_matches(['*', ' ']);

            if let Some(dot_pos) = trimmed.find(". ") {
                if let Ok(id) = trimmed[..dot_pos].trim().parse::<u32>() {
                    let rest = &trimmed[dot_pos + 2..];
                    let name = if let Some(bracket_pos) = rest.rfind('[') {
                        rest[..bracket_pos].trim().to_string()
                    } else {
                        rest.trim().to_string()
                    };

                    if !name.is_empty() {
                        // Create user-friendly names
                        let name_lower = name.to_lowercase();
                        let is_builtin = name_lower.contains("digital microphone");
                        let is_stereo = name_lower.contains("stereo microphone");

                        let friendly_name = if is_builtin {
                            "Built-in Microphone".to_string()
                        } else if is_stereo {
                            "Stereo Microphone".to_string()
                        } else {
                            name.clone()
                        };

                        // Built-in microphone is the default
                        devices.push(WpDevice {
                            id,
                            name: friendly_name,
                            is_default: is_builtin,
                        });
                    }
                }
            }
        }

        if in_filters_section {
            let trimmed = line.trim_start_matches([' ', '│', '├', '─', '-']);

            if trimmed.is_empty() {
                continue;
            }

            // Look for Bluetooth audio sources: "146. bluez_input.XX:XX:XX [Audio/Source]"
            if trimmed.contains("[Audio/Source]") && trimmed.contains("bluez_input") {
                let is_default = trimmed.starts_with('*');
                let trimmed = trimmed.trim_start_matches(['*', ' ']);

                if let Some(dot_pos) = trimmed.find(". ") {
                    if let Ok(id) = trimmed[..dot_pos].trim().parse::<u32>() {
                        // Try to find a friendly name from the Devices section
                        let mut friendly_name = "Bluetooth Microphone".to_string();

                        for (key, value) in &bluetooth_device_names {
                            // The bluetooth device name should be in our map
                            if !key.is_empty() {
                                friendly_name = value.clone();
                                break;
                            }
                        }

                        devices.push(WpDevice {
                            id,
                            name: friendly_name,
                            is_default,
                        });
                    }
                }
            }
        }
    }

    Ok(devices)
}

#[tauri::command]
fn get_selected_device(state: State<'_, AudioState>) -> Option<u32> {
    *state.selected_device_id.lock().unwrap()
}

#[tauri::command]
fn set_selected_device(state: State<'_, AudioState>, device_id: Option<u32>) {
    *state.selected_device_id.lock().unwrap() = device_id;

    // Set the default source in WirePlumber
    // If no device selected, find and use the built-in microphone
    let id_to_set = if let Some(id) = device_id {
        Some(id)
    } else {
        // Find the built-in microphone (Digital Microphone) and set it as default
        find_builtin_microphone_id()
    };

    if let Some(id) = id_to_set {
        let _ = std::process::Command::new("wpctl")
            .args(["set-default", &id.to_string()])
            .status();
    }
}

// Helper to find the built-in microphone ID from wpctl status
fn find_builtin_microphone_id() -> Option<u32> {
    let output = std::process::Command::new("wpctl")
        .args(["status"])
        .output()
        .ok()?;

    let status = String::from_utf8_lossy(&output.stdout);
    let mut in_audio_section = false;
    let mut in_sources_section = false;

    for line in status.lines() {
        if line.starts_with("Audio") {
            in_audio_section = true;
            continue;
        }
        if line.starts_with("Video") || line.starts_with("Settings") {
            in_audio_section = false;
            in_sources_section = false;
            continue;
        }

        if !in_audio_section {
            continue;
        }

        if line.contains("├─ Sources:") || line.contains("└─ Sources:") {
            in_sources_section = true;
            continue;
        }

        if in_sources_section && (line.contains("├─") || line.contains("└─")) {
            in_sources_section = false;
            continue;
        }

        if in_sources_section {
            let trimmed = line.trim_start_matches([' ', '│', '├', '─', '*']);
            if let Some(dot_pos) = trimmed.find(". ") {
                if let Ok(id) = trimmed[..dot_pos].trim().parse::<u32>() {
                    let rest = &trimmed[dot_pos + 2..];
                    // Look for Digital Microphone (built-in)
                    if rest.to_lowercase().contains("digital microphone") {
                        return Some(id);
                    }
                }
            }
        }
    }
    None
}

// Legacy local transcription settings (kept for backward compatibility)
#[tauri::command]
fn set_use_local_transcription(state: State<'_, AudioState>, enabled: bool) {
    *state.use_local_transcription.lock().unwrap() = enabled;
}

#[tauri::command]
fn set_local_model_path(state: State<'_, AudioState>, path: String) {
    *state.local_model_path.lock().unwrap() = Some(path);
}

#[tauri::command]
fn get_local_model_path(state: State<'_, AudioState>) -> Option<String> {
    state.local_model_path.lock().unwrap().clone()
}

// ============================================================================
// Multi-Model Management Commands
// ============================================================================

/// Model info returned to frontend
#[derive(Clone, serde::Serialize)]
pub struct ModelInfoResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub engine_type: String,
    pub total_size_bytes: u64,
    pub accuracy_score: f32,
    pub speed_score: f32,
    pub status: String,
}

/// List all available models with their status
#[tauri::command]
fn list_available_models(state: State<'_, AudioState>) -> Vec<ModelInfoResponse> {
    AVAILABLE_MODELS
        .iter()
        .map(|m| {
            let status = state.model_manager.get_model_status(m.id);
            let status_str = match status {
                ModelStatus::NotDownloaded => "not_downloaded".to_string(),
                ModelStatus::Downloading { progress } => format!("downloading:{:.1}", progress),
                ModelStatus::Downloaded => "downloaded".to_string(),
                ModelStatus::Error { message } => format!("error:{}", message),
            };
            ModelInfoResponse {
                id: m.id.to_string(),
                name: m.name.to_string(),
                description: m.description.to_string(),
                engine_type: format!("{:?}", m.engine_type).to_lowercase(),
                total_size_bytes: m.total_size_bytes,
                accuracy_score: m.accuracy_score,
                speed_score: m.speed_score,
                status: status_str,
            }
        })
        .collect()
}

/// Get status of a specific model
#[tauri::command]
fn get_model_status(state: State<'_, AudioState>, model_id: String) -> Result<String, String> {
    let status = state.model_manager.get_model_status(&model_id);
    match status {
        ModelStatus::NotDownloaded => Ok("not_downloaded".to_string()),
        ModelStatus::Downloading { progress } => Ok(format!("downloading:{:.1}", progress)),
        ModelStatus::Downloaded => Ok("downloaded".to_string()),
        ModelStatus::Error { message } => Ok(format!("error:{}", message)),
    }
}

/// Download a model
#[tauri::command]
async fn download_model(
    state: State<'_, AudioState>,
    app_handle: AppHandle,
    model_id: String,
) -> Result<(), String> {
    let model_manager = state.model_manager.clone();

    // Run download in a blocking thread
    tokio::task::spawn_blocking(move || model_manager.download_model(&model_id, &app_handle))
        .await
        .map_err(|e| format!("Task join error: {}", e))?
}

/// Delete a model
#[tauri::command]
fn delete_model(state: State<'_, AudioState>, model_id: String) -> Result<(), String> {
    // Unload if this is the active model
    if state.transcription_manager.get_loaded_model_id().as_deref() == Some(&model_id) {
        state.transcription_manager.unload_model();
    }

    state.model_manager.delete_model(&model_id)
}

/// Set the active local model
#[tauri::command]
fn set_active_model(state: State<'_, AudioState>, model_id: String) -> Result<(), String> {
    // Verify model exists and is downloaded
    if !state.model_manager.is_model_downloaded(&model_id) {
        return Err(format!("Model {} is not downloaded", model_id));
    }

    *state.active_local_model_id.lock().unwrap() = Some(model_id);
    Ok(())
}

/// Get the active local model ID
#[tauri::command]
fn get_active_model(state: State<'_, AudioState>) -> Option<String> {
    state.active_local_model_id.lock().unwrap().clone()
}

/// Load the active model into memory (for pre-loading)
#[tauri::command]
fn load_active_model(state: State<'_, AudioState>) -> Result<(), String> {
    let model_id = state
        .active_local_model_id
        .lock()
        .unwrap()
        .clone()
        .ok_or("No active model set")?;

    state.transcription_manager.load_model(&model_id)
}

/// Unload the current model from memory
#[tauri::command]
fn unload_model(state: State<'_, AudioState>) {
    state.transcription_manager.unload_model();
}

/// Check if a model is currently loaded
#[tauri::command]
fn is_model_loaded(state: State<'_, AudioState>) -> bool {
    state.transcription_manager.is_model_loaded()
}

/// Get the loaded model ID
#[tauri::command]
fn get_loaded_model(state: State<'_, AudioState>) -> Option<String> {
    state.transcription_manager.get_loaded_model_id()
}

/// Set VAD enabled/disabled
#[tauri::command]
fn set_use_vad(state: State<'_, AudioState>, enabled: bool) {
    *state.use_vad.lock().unwrap() = enabled;
}

/// Get VAD enabled state
#[tauri::command]
fn get_use_vad(state: State<'_, AudioState>) -> bool {
    *state.use_vad.lock().unwrap()
}

// The one place the debug line is switched, so the tray tick, the settings
// switch, the saved file and both windows can never disagree.
fn set_debug_stats_everywhere(app: &AppHandle, enabled: bool) {
    use tauri::Manager;
    let state = app.state::<AudioState>();
    *state.debug_stats.lock().unwrap() = enabled;
    if let Some(item) = state.debug_menu_item.lock().unwrap().as_ref() {
        let _ = item.set_checked(enabled);
    }
    save_tray_prefs(&TrayPrefs {
        debug_stats: enabled,
    });
    let _ = app.emit("debug-stats-changed", enabled);
}

#[tauri::command]
fn set_debug_stats(app: AppHandle, enabled: bool) {
    set_debug_stats_everywhere(&app, enabled);
}

// Asked by each window when it opens; after that the tray sends
// "debug-stats-changed" when it is switched.
#[tauri::command]
fn get_debug_stats(state: State<'_, AudioState>) -> bool {
    *state.debug_stats.lock().unwrap()
}

// Deletes every recording in a folder. Only .wav files, so anything else that
// happens to be in there survives. Returns how many went.
fn delete_recordings_in(dir: &std::path::Path) -> Result<usize, String> {
    let entries =
        fs::read_dir(dir).map_err(|e| format!("Could not read {}: {}", dir.display(), e))?;
    let mut deleted = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("wav") {
            match fs::remove_file(&path) {
                Ok(()) => deleted += 1,
                Err(e) => eprintln!("Could not delete {}: {}", path.display(), e),
            }
        }
    }
    Ok(deleted)
}

// Hides the main window and drops back to a menu-bar app, the same as the tray
// item does. Closing it outright would end the app.
#[tauri::command]
fn hide_main_window(app: AppHandle) {
    use tauri::Manager;
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}

// A missing shortcut or permission is invisible otherwise: the app sits in the
// tray looking healthy and simply does nothing.
#[tauri::command]
fn get_startup_warnings(state: State<'_, AudioState>) -> Vec<String> {
    state.startup_warnings.lock().unwrap().clone()
}

// Legacy check for backward compatibility with old settings page
#[tauri::command]
fn check_local_model_status(state: State<'_, AudioState>) -> Result<serde_json::Value, String> {
    // Check if any model is downloaded
    let any_downloaded = AVAILABLE_MODELS
        .iter()
        .any(|m| state.model_manager.is_model_downloaded(m.id));

    let active_model = state.active_local_model_id.lock().unwrap().clone();
    let path = active_model.as_ref().map(|id| {
        state
            .model_manager
            .get_model_path(id)
            .to_string_lossy()
            .to_string()
    });

    Ok(serde_json::json!({
        "downloaded": any_downloaded,
        "path": path,
        "downloading": false
    }))
}

// Legacy download function - now redirects to download default model
#[tauri::command]
async fn download_local_model(
    state: State<'_, AudioState>,
    app_handle: AppHandle,
) -> Result<String, String> {
    // Download moonshine-base as the default (smallest and fastest)
    let model_id = "moonshine-base";
    let model_manager = state.model_manager.clone();

    tokio::task::spawn_blocking(move || {
        model_manager.download_model(model_id, &app_handle)?;
        Ok(model_manager
            .get_model_path(model_id)
            .to_string_lossy()
            .to_string())
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

// Helper function to get the audio input device
// Uses WirePlumber's default device (set via wpctl set-default)
fn get_input_device() -> Result<Device, String> {
    let host = cpal::default_host();

    // Log available input devices for debugging
    eprintln!("Available audio hosts: {:?}", cpal::available_hosts());
    eprintln!("Using host: {:?}", host.id());

    if let Ok(devices) = host.input_devices() {
        let devices: Vec<_> = devices.collect();

        for device in &devices {
            if let Ok(name) = device.name() {
                eprintln!("  Available input device: {}", name);
            }
        }

        // Try to find "pipewire" device first - it uses WirePlumber's default source
        // and handles Bluetooth better than ALSA devices
        for device in devices {
            if let Ok(name) = device.name() {
                if name == "pipewire" {
                    eprintln!("Selected input device: {} (uses WirePlumber default)", name);
                    return Ok(device);
                }
            }
        }
    }

    // Fall back to default device
    let device = host
        .default_input_device()
        .ok_or_else(|| "No audio input device found".to_string())?;

    if let Ok(name) = device.name() {
        eprintln!("Selected default input device: {}", name);
    }

    Ok(device)
}

// Get a safe stream config that works with Bluetooth devices
// Bluetooth audio on Linux (especially with PipeWire) can crash GNOME when using
// certain buffer sizes or sample rates. This function tries to find a safer config.
fn get_safe_input_config(device: &Device) -> Result<SupportedStreamConfig, String> {
    // First, try to get supported configs and find one that's known to work well
    if let Ok(configs) = device.supported_input_configs() {
        let configs: Vec<_> = configs.collect();

        // Prefer 48000 Hz or 44100 Hz with F32 format - these are most compatible
        let preferred_rates = [48000u32, 44100, 16000, 32000, 96000];

        for rate in preferred_rates {
            for config in &configs {
                if config.min_sample_rate().0 <= rate
                    && config.max_sample_rate().0 >= rate
                    && config.sample_format() == SampleFormat::F32
                {
                    return Ok((*config).with_sample_rate(cpal::SampleRate(rate)));
                }
            }
            // If F32 not available at this rate, try I16
            for config in &configs {
                if config.min_sample_rate().0 <= rate
                    && config.max_sample_rate().0 >= rate
                    && config.sample_format() == SampleFormat::I16
                {
                    return Ok((*config).with_sample_rate(cpal::SampleRate(rate)));
                }
            }
        }
    }

    // Fall back to default config if no preferred config found
    device
        .default_input_config()
        .map_err(|e| format!("Failed to get input config: {}", e))
}

// Convert audio data to WAV format bytes
// Size of the spectrogram indicator window.
const INDICATOR_W: f64 = 460.0;
const INDICATOR_H: f64 = 200.0;

// Put the indicator at the bottom centre of the screen the mouse is on, so
// it appears on whichever display is being worked on.
//
// Screens share one coordinate space and only the main screen is guaranteed
// to start at zero, so the monitor's own origin has to be added or the window
// lands on another display. Called every time the indicator is shown rather
// than once at startup, so plugging a monitor in or out, or changing
// resolution, is picked up without restarting.
#[tauri::command]
fn position_indicator(app: AppHandle) {
    use tauri::Manager;

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

// Loudness of what was sent to the model: the loudest sample, the average
// level, and how much of it sits near silence. Quiet or mostly-silent audio
// is the usual reason a dictation comes back wrong or invented.
fn audio_stats(samples: &[f32]) -> (f32, f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0, 1.0);
    }
    let mut peak = 0.0f32;
    let mut sum_squares = 0.0f64;
    let mut quiet_samples = 0usize;
    for &s in samples {
        let level = s.abs();
        if level > peak {
            peak = level;
        }
        sum_squares += (s as f64) * (s as f64);
        // about -40 dBFS, below which speech is unlikely to be understood
        if level < 0.01 {
            quiet_samples += 1;
        }
    }
    let rms = (sum_squares / samples.len() as f64).sqrt() as f32;
    (peak, rms, quiet_samples as f32 / samples.len() as f32)
}

// How many samples the drawing maths looks at, and how many bars come out.
const FFT_SIZE: usize = 1024;
const BAND_COUNT: usize = 64;

// Loudness per frequency band, for the bars the windows draw.
//
// The bands are spaced logarithmically, so the voice range fills the width
// instead of being squeezed into the left edge, and the result is in decibels,
// because that is how loudness is heard. -90 dB comes out as 0 and -20 dB as 1.
fn frequency_bands(samples: &[f32], fft: &std::sync::Arc<dyn rustfft::Fft<f32>>) -> Vec<f32> {
    use rustfft::num_complex::Complex;

    if samples.len() < FFT_SIZE {
        return vec![0.0; BAND_COUNT];
    }
    let start = samples.len() - FFT_SIZE;

    // A Hann window: without it the ends of the slice act like a step change
    // and smear energy across every band.
    let mut buffer: Vec<Complex<f32>> = samples[start..]
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let w =
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos();
            Complex::new(s * w, 0.0)
        })
        .collect();
    fft.process(&mut buffer);

    let bins = FFT_SIZE / 2;
    let min_bin = 2.0f32;
    let max_bin = (bins as f32) * 0.5;

    (0..BAND_COUNT)
        .map(|i| {
            let pos = i as f32 / (BAND_COUNT - 1) as f32;
            let bin = (min_bin * (max_bin / min_bin).powf(pos)).round() as usize;
            let bin = bin.min(bins - 1);
            let magnitude = buffer[bin].norm() / (bins as f32);
            let db = 20.0 * (magnitude + 1e-9).log10();
            ((db + 90.0) / 70.0).clamp(0.0, 1.0)
        })
        .collect()
}

// Base frequency of the voice, found by looking for the shortest delay after
// which the wave repeats. 0 when the sound is too quiet or too noisy to tell -
// a wrong number is worse than none.
// Anything from 1.02 to 1.06 works; outside that it gets worse in one direction
// or the other. Do not raise it thinking bigger is safer.
const PITCH_MARGIN: f32 = 1.05;

fn detect_pitch(samples: &[f32], sample_rate: u32) -> f32 {
    let size = samples.len().min(2048);
    if size < 512 {
        return 0.0;
    }
    let window = &samples[samples.len() - size..];

    let energy: f32 = window.iter().map(|s| s * s).sum();
    let rms = (energy / size as f32).sqrt();
    if rms < 0.004 {
        return 0.0;
    }

    let min_lag = (sample_rate as f32 / 400.0) as usize; // highest voice
    let max_lag = ((sample_rate as f32 / 70.0) as usize).min(size - 1); // lowest voice
    let mut best_lag = 0usize;
    let mut best = 0.0f32;
    for lag in min_lag..=max_lag {
        let mut sum = 0.0f32;
        for i in 0..(size - lag) {
            sum += window[i] * window[i + lag];
        }
        let score = sum / (size - lag) as f32;
        // A delay of two or three repeats matches as well as one repeat, so a later
        // delay has to win clearly, not by rounding. Measured over 816 voices:
        // 1.0 is wrong 363 times, 1.05 is wrong 7, 1.1 is wrong 56 the other way.
        if score > best * PITCH_MARGIN {
            best = score;
            best_lag = lag;
        }
    }
    // The repeat has to be at least a third as strong as the sound itself.
    if best_lag == 0 || best < rms * rms * 0.33 {
        return 0.0;
    }
    sample_rate as f32 / best_lag as f32
}

// Keep the newest few thousand samples for the drawing maths. Called from the
// audio callback, so it does no more than a copy: enough for the frequency
// bands and for finding the pitch, and nothing older.
fn keep_recent(store: &Arc<Mutex<Vec<f32>>>, chunk: &[f32]) {
    const KEEP: usize = 4096;
    let Ok(mut recent) = store.lock() else { return };
    recent.extend_from_slice(chunk);
    if recent.len() > KEEP {
        let drop = recent.len() - KEEP;
        recent.drain(0..drop);
    }
}

// Loudest sample and average level of one chunk, for the live meter.
fn chunk_level(samples: &[f32]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let mut peak = 0.0f32;
    let mut sum_squares = 0.0f32;
    for &s in samples {
        peak = peak.max(s.abs());
        sum_squares += s * s;
    }
    (peak, (sum_squares / samples.len() as f32).sqrt())
}

// What the capture callback writes into.
struct CaptureTargets {
    is_recording: Arc<Mutex<bool>>,
    recorded_samples: Arc<Mutex<Vec<f32>>>,
    mic_level: Arc<Mutex<(f32, f32)>>,
    mic_recent: Arc<Mutex<Vec<f32>>>,
    audio_tx: std::sync::mpsc::Sender<Vec<f32>>,
    channels: u16,
}

fn i16_to_f32(sample: i16) -> f32 {
    sample as f32 / i16::MAX as f32
}

// Unsigned samples put silence in the middle of the range, so shift as well as scale.
fn u16_to_f32(sample: u16) -> f32 {
    (sample as f32 / u16::MAX as f32) * 2.0 - 1.0
}

// Stereo arrives as left, right, left, right.
fn mix_to_mono(buffer: Vec<f32>, channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return buffer;
    }
    let mut mono = Vec::with_capacity(buffer.len().div_ceil(channels as usize));
    for frame in buffer.chunks(channels as usize) {
        mono.push(frame.iter().sum::<f32>() / frame.len() as f32);
    }
    mono
}

// A microphone problem the user has to be told about. Clearing the flag matters
// as much as the message: leaving it set made the next shortcut press fail with
// "Already recording", with nothing on screen to explain it.
fn report_microphone_failure(app: &AppHandle, is_recording: &Arc<Mutex<bool>>, message: String) {
    eprintln!("{}", message);
    if let Ok(mut flag) = is_recording.lock() {
        *flag = false;
    }
    let _ = app.emit("transcription-error", message);
}

// Runs on the audio thread: store and hand off, never wait.
fn build_capture_stream<T>(
    device: &Device,
    config: &cpal::StreamConfig,
    targets: CaptureTargets,
    on_broken: impl Fn(String) + Send + 'static,
    to_f32: fn(T) -> f32,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + 'static,
{
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            if !*targets.is_recording.lock().unwrap() {
                return;
            }
            let buffer: Vec<f32> = data.iter().copied().map(to_f32).collect();
            let buffer = mix_to_mono(buffer, targets.channels);
            // Store for the WAV file
            targets
                .recorded_samples
                .lock()
                .unwrap()
                .extend_from_slice(&buffer);
            *targets.mic_level.lock().unwrap() = chunk_level(&buffer);
            keep_recent(&targets.mic_recent, &buffer);
            // Hand to the transcription thread
            let _ = targets.audio_tx.send(buffer);
        },
        // The microphone died mid-recording: unplugged, or taken by another app.
        // The sound from here on was never captured and cannot be recovered, so
        // say so and stop, which leaves what was already captured to transcribe.
        move |err| on_broken(format!("{}", err)),
        None,
    )
}

// How loud the spoken parts are, ignoring pauses and one-off bangs.
//
// The recording is cut into 30 ms pieces, each piece's loudness is measured,
// and the value 10% from the top is returned. Pauses sit at the bottom and a
// single door slam sits in that top 10%, so neither decides the answer.
// Normal close speech lands around 0.08 on this scale.
fn speech_level(samples: &[f32]) -> f32 {
    const FRAME: usize = 480; // 30 ms at 16 kHz

    let mut levels: Vec<f32> = samples
        .chunks(FRAME)
        .filter(|c| c.len() == FRAME)
        .map(|c| {
            let sum: f32 = c.iter().map(|s| s * s).sum();
            (sum / c.len() as f32).sqrt()
        })
        .collect();

    if levels.is_empty() {
        return 0.0;
    }

    levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Clamp, not wrap: wrapping would return the quietest frame.
    let index = ((levels.len() as f32 * 0.9) as usize).min(levels.len() - 1);
    levels[index]
}

// Wall-clock time for the log, so a recording that nobody meant to start can
// be matched against what was happening on screen at that moment.
fn now() -> String {
    Local::now().format("%H:%M:%S%.3f").to_string()
}

// Below these two, a recording holds no speech and must not be sent to the
// model. Measured on the microphone audio, before any level boost. Speech
// close to the microphone reaches a speech level of 0.08 and a peak near 0.5;
// an empty room measured 0.0014 and 0.02.
const MIN_SPEECH_LEVEL: f32 = 0.005;
const MIN_SPEECH_PEAK: f32 = 0.05;

// Whisper invents sentences from silence, and auto-type types them.
fn holds_speech(speech_level: f32, peak: f32) -> bool {
    speech_level >= MIN_SPEECH_LEVEL && peak >= MIN_SPEECH_PEAK
}

// Cut the quiet head and tail off, keeping everything in between.
//
// Pressing the shortcut and starting to speak takes a few seconds, and those
// seconds arrive as room noise. Whisper reads the recording as one block and
// a long quiet opening makes it lose the first words. Only the two ends are
// touched: a pause in the middle of a sentence is never cut, because cutting
// there would join words that were seconds apart.
//
// Returns how many seconds were removed from the front.
fn trim_quiet_edges(samples: &mut Vec<f32>) -> f32 {
    const FRAME: usize = 480; // 30 ms at 16 kHz
    const KEEP: usize = 16; // ~0.5 s of margin, so no word loses its start

    let level = speech_level(samples);
    if level <= 0.0 {
        return 0.0;
    }
    // A quarter of the speaking level: quiet enough to catch a soft word,
    // loud enough to ignore room noise.
    let threshold = (level * 0.25).max(0.004);

    let loud: Vec<bool> = samples
        .chunks(FRAME)
        .map(|c| {
            let sum: f32 = c.iter().map(|s| s * s).sum();
            (sum / c.len() as f32).sqrt() > threshold
        })
        .collect();

    let Some(first) = loud.iter().position(|&x| x) else {
        return 0.0;
    };
    let last = loud.iter().rposition(|&x| x).unwrap_or(loud.len() - 1);

    let start = first.saturating_sub(KEEP) * FRAME;
    let end = ((last + KEEP + 1) * FRAME).min(samples.len());
    if start >= end {
        return 0.0;
    }

    let cut_seconds = start as f32 / 16_000.0;
    *samples = samples[start..end].to_vec();
    cut_seconds
}

// Bring quiet recordings up to a level the model can work with.
//
// Whisper reads audio in 30 second windows and drops a whole window when it
// is not sure the window holds speech. A quiet microphone makes that happen
// again and again, so a long dictation comes back as a few sentences or as
// nothing. Scaling the whole recording up avoids it. The saved microphone
// recording is untouched; only what goes to the model is changed.
//
// The gain is chosen from the speech level, not the loudest sample, so long
// pauses do not shrink it and one loud bang does not cancel it. The peak
// still caps the gain, so nothing clips.
//
// Returns the gain applied, 1.0 meaning the audio was already loud enough.
fn boost_quiet_audio(samples: &mut [f32]) -> f32 {
    // Loudness of normal speech close to the microphone.
    const TARGET_LEVEL: f32 = 0.08;
    // Leave headroom so the loudest sample does not hit the ceiling.
    const MAX_PEAK: f32 = 0.95;
    // Past this the recording is being turned into something it was not.
    // 40x used to be allowed, and it raised an empty room to speech loudness,
    // which Whisper then read as sentences in Dutch.
    const MAX_GAIN: f32 = 10.0;

    let level = speech_level(samples);
    let peak = samples.iter().fold(0.0f32, |max, s| max.max(s.abs()));
    if level < MIN_SPEECH_LEVEL || peak <= 0.0 {
        return 1.0;
    }

    let gain = (TARGET_LEVEL / level).min(MAX_PEAK / peak).min(MAX_GAIN);
    if gain <= 1.0 {
        return 1.0;
    }

    for s in samples.iter_mut() {
        *s = (*s * gain).clamp(-1.0, 1.0);
    }
    gain
}

// Save the exact audio handed to the model, at 16kHz, so it can be listened
// to afterwards. This is not the same as the saved recording: it is after
// resampling and after voice detection has cut pieces out, which is the
// whole point - it is what the model heard, not what the microphone heard.
fn save_model_input(samples: &[f32]) -> Option<PathBuf> {
    let dir = get_recordings_dir().ok()?;
    let path = dir.join(format!(
        "{}-model-input.wav",
        Utc::now().format("%Y-%m-%d_%H-%M-%S")
    ));
    fs::write(&path, to_wav_bytes(samples, 16_000, 1)).ok()?;
    Some(path)
}

fn to_wav_bytes(samples: &[f32], sample_rate: u32, channels: u16) -> Vec<u8> {
    let mut wav_data = Vec::new();

    let bytes_per_sample: u16 = 2; // 16-bit
    let byte_rate = sample_rate * channels as u32 * bytes_per_sample as u32;
    let block_align: u16 = channels * bytes_per_sample;
    let data_size = samples.len() * bytes_per_sample as usize;
    let file_size = 36 + data_size as u32;

    // RIFF header
    wav_data.extend_from_slice(b"RIFF");
    wav_data.extend_from_slice(&file_size.to_le_bytes());
    wav_data.extend_from_slice(b"WAVE");

    // fmt chunk
    wav_data.extend_from_slice(b"fmt ");
    wav_data.extend_from_slice(&16u32.to_le_bytes());
    wav_data.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav_data.extend_from_slice(&channels.to_le_bytes());
    wav_data.extend_from_slice(&sample_rate.to_le_bytes());
    wav_data.extend_from_slice(&byte_rate.to_le_bytes());
    wav_data.extend_from_slice(&block_align.to_le_bytes());
    wav_data.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    // data chunk
    wav_data.extend_from_slice(b"data");
    wav_data.extend_from_slice(&(data_size as u32).to_le_bytes());

    // Convert f32 samples to i16
    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let i16_sample = (clamped * i16::MAX as f32) as i16;
        wav_data.extend_from_slice(&i16_sample.to_le_bytes());
    }

    wav_data
}

// Convert f32 samples to linear16 PCM bytes for Deepgram
fn samples_to_linear16(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let i16_sample = (clamped * i16::MAX as f32) as i16;
        bytes.extend_from_slice(&i16_sample.to_le_bytes());
    }
    bytes
}

// Enum to hold either TLS or plain TCP WebSocket
enum WsStream {
    Tls(WebSocket<MaybeTlsStream<TcpStream>>),
    Plain(WebSocket<TcpStream>),
}

// tungstenite's own error, passed through. Boxing it would only move the cost.
#[allow(clippy::result_large_err)]
impl WsStream {
    fn send(&mut self, msg: Message) -> Result<(), tungstenite::Error> {
        match self {
            WsStream::Tls(ws) => ws.send(msg),
            WsStream::Plain(ws) => ws.send(msg),
        }
    }

    fn read(&mut self) -> Result<Message, tungstenite::Error> {
        match self {
            WsStream::Tls(ws) => ws.read(),
            WsStream::Plain(ws) => ws.read(),
        }
    }

    fn close(
        &mut self,
        _: Option<tungstenite::protocol::CloseFrame>,
    ) -> Result<(), tungstenite::Error> {
        match self {
            WsStream::Tls(ws) => ws.close(None),
            WsStream::Plain(ws) => ws.close(None),
        }
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        match self {
            WsStream::Tls(ws) => {
                if let MaybeTlsStream::NativeTls(ref tls) = ws.get_ref() {
                    tls.get_ref().set_read_timeout(timeout)
                } else {
                    Ok(())
                }
            }
            WsStream::Plain(ws) => ws.get_ref().set_read_timeout(timeout),
        }
    }
}

// Connect to Deepgram WebSocket (TLS)
fn connect_to_deepgram(api_key: &str, sample_rate: u32) -> Result<WsStream, String> {
    let url_str = format!(
        "wss://api.deepgram.com/v1/listen?model=nova-3&smart_format=true&interim_results=true&encoding=linear16&sample_rate={}&channels=1",
        sample_rate
    );

    let url = Url::parse(&url_str).map_err(|e| format!("Invalid URL: {}", e))?;

    // Create TLS connector
    let connector = native_tls::TlsConnector::new()
        .map_err(|e| format!("Failed to create TLS connector: {}", e))?;

    // Connect to the host
    let host = url.host_str().ok_or("No host in URL")?;
    let port = url.port().unwrap_or(443);
    let stream = TcpStream::connect(format!("{}:{}", host, port))
        .map_err(|e| format!("Failed to connect: {}", e))?;

    // Wrap with TLS
    let tls_stream = connector
        .connect(host, stream)
        .map_err(|e| format!("TLS handshake failed: {}", e))?;

    // Create WebSocket request with auth header
    let request = tungstenite::http::Request::builder()
        .uri(url_str)
        .header("Authorization", format!("Token {}", api_key))
        .header("Host", host)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .map_err(|e| format!("Failed to build request: {}", e))?;

    let (ws, _response) =
        tungstenite::client::client(request, MaybeTlsStream::NativeTls(tls_stream))
            .map_err(|e| format!("WebSocket handshake failed: {}", e))?;

    Ok(WsStream::Tls(ws))
}

// Connect to Hyperwhisper server WebSocket
fn connect_to_hyperwhisper_server(
    api_key: &str,
    sample_rate: u32,
    server_url: &str,
    use_https: bool,
) -> Result<WsStream, String> {
    let protocol = if use_https { "wss" } else { "ws" };
    let url_str = format!(
        "{}://{}/api/v1/deepgram/listen?model=nova-3&smart_format=true&interim_results=true&encoding=linear16&sample_rate={}&channels=1",
        protocol, server_url, sample_rate
    );

    let url = Url::parse(&url_str).map_err(|e| format!("Invalid URL: {}", e))?;
    let host = url.host_str().ok_or("No host in URL")?;
    let port = url.port().unwrap_or(if use_https { 443 } else { 80 });

    if use_https {
        // Connect with TLS
        let connector = native_tls::TlsConnector::new()
            .map_err(|e| format!("Failed to create TLS connector: {}", e))?;

        let stream = TcpStream::connect(format!("{}:{}", host, port))
            .map_err(|e| format!("Failed to connect to Hyperwhisper server: {}", e))?;

        let tls_stream = connector
            .connect(host, stream)
            .map_err(|e| format!("TLS handshake failed: {}", e))?;

        let request = tungstenite::http::Request::builder()
            .uri(&url_str)
            .header("X-API-Key", api_key)
            .header("Host", host)
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .map_err(|e| format!("Failed to build request: {}", e))?;

        let (ws, _response) =
            tungstenite::client::client(request, MaybeTlsStream::NativeTls(tls_stream))
                .map_err(|e| format!("WebSocket handshake failed: {}", e))?;

        Ok(WsStream::Tls(ws))
    } else {
        // Connect without TLS (plain TCP)
        let stream = TcpStream::connect(format!("{}:{}", host, port))
            .map_err(|e| format!("Failed to connect to Hyperwhisper server: {}", e))?;

        let request = tungstenite::http::Request::builder()
            .uri(&url_str)
            .header("X-API-Key", api_key)
            .header("Host", format!("{}:{}", host, port))
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .map_err(|e| format!("Failed to build request: {}", e))?;

        let (ws, _response) = tungstenite::client::client(request, stream)
            .map_err(|e| format!("WebSocket handshake failed: {}", e))?;

        Ok(WsStream::Plain(ws))
    }
}

#[tauri::command]
fn set_api_key(state: State<'_, AudioState>, api_key: String) {
    *state.api_key.lock().unwrap() = Some(api_key);
}

// Response for set_hyperwhisper_server_settings when auto-provisioning occurs
#[derive(Clone, serde::Serialize)]
pub struct ServerSettingsResponse {
    pub provisioned_key: Option<String>,
    pub trial_info: Option<TrialProvisionResponse>,
    pub error: Option<String>,
}

#[tauri::command]
fn set_hyperwhisper_server_settings(
    state: State<'_, AudioState>,
    use_hyperwhisper_server: bool,
    server_url: String,
    use_https: bool,
    api_key: Option<String>,
) -> ServerSettingsResponse {
    let server_url_clean = server_url.trim().to_string();
    let server_url_final = if server_url_clean.is_empty() {
        "hyperwhisper.dev".to_string()
    } else {
        server_url_clean
    };

    *state.use_hyperwhisper_server.lock().unwrap() = use_hyperwhisper_server;
    *state.hyperwhisper_server_url.lock().unwrap() = server_url_final.clone();
    *state.hyperwhisper_server_https.lock().unwrap() = use_https;

    // If using hyperwhisper server and no API key provided, auto-provision a trial key
    if use_hyperwhisper_server && api_key.as_ref().is_none_or(|k| k.trim().is_empty()) {
        match provision_trial_key_internal(&server_url_final, use_https) {
            Ok(response) => {
                if let Some(ref key) = response.key {
                    *state.hyperwhisper_api_key.lock().unwrap() = Some(key.clone());
                    return ServerSettingsResponse {
                        provisioned_key: Some(key.clone()),
                        trial_info: Some(response),
                        error: None,
                    };
                } else {
                    // Device already has a trial key on server but we don't have it
                    return ServerSettingsResponse {
                        provisioned_key: None,
                        trial_info: Some(response),
                        error: Some("Trial key exists for this device but was not returned. Please enter your API key manually.".to_string()),
                    };
                }
            }
            Err(e) => {
                eprintln!("Failed to auto-provision trial key: {}", e);
                return ServerSettingsResponse {
                    provisioned_key: None,
                    trial_info: None,
                    error: Some(e),
                };
            }
        }
    }

    // API key was provided
    *state.hyperwhisper_api_key.lock().unwrap() = api_key;
    ServerSettingsResponse {
        provisioned_key: None,
        trial_info: None,
        error: None,
    }
}

// How many UTF-16 units to put in one key event. A Unicode key event carries
// only a short string, so long text is sent in several events.
#[cfg(target_os = "macos")]
const CHUNK_UTF16_UNITS: usize = 20;

// Whether this app is allowed to control the computer (Accessibility).
// Without it the key events are created but the system drops them, so the
// text silently never appears.
#[cfg(target_os = "macos")]
fn accessibility_granted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

// The return separates the macOS path from the Linux one below it.
#[allow(clippy::needless_return)]
fn type_text_internal(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        // Post the text straight to the window server as Unicode key events.
        //
        // This replaces `osascript ... keystroke`, which spawned a process per
        // transcription, needed the Automation permission on top of
        // Accessibility, and typed through the current keyboard layout - so
        // any character the layout cannot produce (Cyrillic on a US layout)
        // came out wrong. Unicode key events do not use the layout.
        use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
        use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

        if !accessibility_granted() {
            return Err(
                "Accessibility permission is not granted, so text cannot be typed. \
                 Add Omegawhisper in System Settings > Privacy & Security > Accessibility."
                    .to_string(),
            );
        }

        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| "Failed to create a keyboard event source".to_string())?;

        // The shortcut that stops recording is F3, a function key. Give the
        // physical key time to be released and the target window time to take
        // focus back, otherwise the first characters land nowhere.
        thread::sleep(Duration::from_millis(120));

        // One event carries only a short Unicode string, so send the text in
        // small pieces. Split on character boundaries, never inside a
        // surrogate pair, or the character is corrupted.
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let mut start = 0;
        while start < utf16.len() {
            let mut end = std::cmp::min(start + CHUNK_UTF16_UNITS, utf16.len());
            // A leading surrogate at the end means the pair is split - keep it
            // with its trailing half in the next chunk.
            if end < utf16.len() && (0xD800..0xDC00).contains(&utf16[end - 1]) {
                end -= 1;
            }
            let chunk = String::from_utf16_lossy(&utf16[start..end]);

            for key_down in [true, false] {
                let event = CGEvent::new_keyboard_event(source.clone(), 0, key_down)
                    .map_err(|_| "Failed to create a keyboard event".to_string())?;
                // Events built from the live hardware state inherit whatever
                // modifiers are held right now. F3 sets the Fn modifier, and a
                // character carrying Fn (or Command) is read as a shortcut and
                // thrown away instead of typed. Send plain characters only.
                event.set_flags(CGEventFlags::CGEventFlagNull);
                event.set_string(&chunk);
                event.post(CGEventTapLocation::HID);
            }

            // Electron apps (Teams, VS Code) drop characters without a pause.
            thread::sleep(Duration::from_millis(2));
            start = end;
        }

        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Try ydotool first (works on both Wayland and X11 via uinput)
        // Use --key-delay 0 for fastest typing
        let ydotool_result = std::process::Command::new("ydotool")
            .args(["type", "--key-delay=0", "--", text])
            .status();

        if let Ok(status) = ydotool_result {
            if status.success() {
                return Ok(());
            }
        }

        // Try wtype (Wayland - requires compositor support)
        let wtype_result = std::process::Command::new("wtype").arg(text).status();

        if let Ok(status) = wtype_result {
            if status.success() {
                return Ok(());
            }
        }

        // Fall back to xdotool (X11)
        let xdotool_result = std::process::Command::new("xdotool")
            .args(["type", "--clearmodifiers", text])
            .status();

        if let Ok(status) = xdotool_result {
            if status.success() {
                return Ok(());
            }
        }

        Err("Failed to type text: ydotool, wtype, and xdotool all failed".to_string())
    }
}

#[tauri::command]
async fn start_recording(
    state: State<'_, AudioState>,
    app_handle: AppHandle,
) -> Result<(), String> {
    // Check recording state
    {
        let is_recording = state.is_recording.lock().unwrap();
        if *is_recording {
            return Err("Already recording".to_string());
        }
    }
    eprintln!("[{}] recording started", now());

    // Check if using local transcription
    let use_local = *state.use_local_transcription.lock().unwrap();

    // Check if using Hyperwhisper server or direct Deepgram
    let use_hyperwhisper = *state.use_hyperwhisper_server.lock().unwrap();
    let hyperwhisper_url = state.hyperwhisper_server_url.lock().unwrap().clone();
    let hyperwhisper_https = *state.hyperwhisper_server_https.lock().unwrap();

    // Get the appropriate API key (not needed for local transcription)
    let api_key = if use_local {
        String::new() // Local transcription doesn't need API key
    } else if use_hyperwhisper {
        state
            .hyperwhisper_api_key
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "Hyperwhisper API key not set".to_string())?
    } else {
        state
            .api_key
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "Deepgram API key not set".to_string())?
    };

    // Validate local model if using local transcription
    let active_model_id = if use_local {
        let model_id = state
            .active_local_model_id
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| {
                "No local model selected. Please select a model in settings.".to_string()
            })?;

        if !state.model_manager.is_model_downloaded(&model_id) {
            return Err(format!(
                "Model {} is not downloaded. Please download it first.",
                model_id
            ));
        }

        Some(model_id)
    } else {
        None
    };

    // Get VAD setting
    let use_vad = *state.use_vad.lock().unwrap();

    // Clone transcription manager for the thread
    let transcription_manager = state.transcription_manager.0.clone();

    // Cleared, then given room below once the sample rate is known.
    state.recorded_samples.lock().unwrap().clear();

    // Get audio device info in a blocking thread to avoid interfering with GTK main loop
    // This is critical for Bluetooth devices on PipeWire which can crash GNOME
    // Note: Device selection is handled by WirePlumber via wpctl set-default
    let (device, config) = tokio::task::spawn_blocking(move || {
        let device = get_input_device()?;
        let config = get_safe_input_config(&device)?;
        Ok::<_, String>((device, config))
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
    .map_err(|e: String| e)?;

    let sample_format = config.sample_format();
    let sample_rate = config.sample_rate().0;
    let channels = config.channels();

    // Room for five minutes, set aside now. The audio callback appends to this
    // list and must never be slow: growing it there means copying every sample
    // recorded so far into a bigger block, which at two minutes is 23 MB, in the
    // one place that cannot afford to wait. The memory is handed back when the
    // recording stops.
    const RESERVE_SECONDS: usize = 300;
    state
        .recorded_samples
        .lock()
        .unwrap()
        .reserve(sample_rate as usize * RESERVE_SECONDS);

    // Use default buffer size - fixed sizes can cause issues with Bluetooth on PipeWire
    let stream_config: cpal::StreamConfig = config.into();

    // Store sample rate
    *state.sample_rate.lock().unwrap() = Some(sample_rate);

    let is_recording_arc = state.is_recording.clone();
    let recorded_samples_arc = state.recorded_samples.clone();

    // Set recording flag
    *state.is_recording.lock().unwrap() = true;

    // Create channel for stop signal
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    *state.stop_signal.lock().unwrap() = Some(stop_tx);

    // Channel for sending audio chunks to transcription thread
    let (audio_tx, audio_rx) = std::sync::mpsc::channel::<Vec<f32>>();

    // Spawn transcription thread (local or WebSocket-based)
    let app_handle_ws = app_handle.clone();
    let is_recording_ws = is_recording_arc.clone();
    let stop_signal_ws = state.stop_signal.clone();
    let auto_type = *state.auto_type_transcription.lock().unwrap();

    if use_local {
        // Spawn local transcription thread using multi-model transcription manager
        // Uses "transcribe on stop" mode - accumulate audio, transcribe at end
        let model_id = active_model_id.unwrap();

        thread::spawn(move || {
            // Helper to stop recording on error
            let stop_recording_on_error =
                |is_recording: &Arc<Mutex<bool>>,
                 stop_signal: &Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>| {
                    *is_recording.lock().unwrap() = false;
                    if let Some(stop_tx) = stop_signal.lock().unwrap().take() {
                        let _ = stop_tx.send(());
                    }
                };

            // Load the model if not already loaded
            {
                let mut manager = match transcription_manager.lock() {
                    Ok(m) => m,
                    Err(e) => {
                        let _ = app_handle_ws.emit(
                            "transcription-error",
                            format!("Failed to access transcription engine: {}", e),
                        );
                        stop_recording_on_error(&is_recording_ws, &stop_signal_ws);
                        return;
                    }
                };

                let currently_loaded = manager.get_loaded_model_id().map(|s| s.to_string());
                if currently_loaded.as_deref() != Some(&model_id) {
                    if let Err(e) = manager.load_model(&model_id) {
                        let _ = app_handle_ws.emit(
                            "transcription-error",
                            format!("Failed to load model: {}", e),
                        );
                        stop_recording_on_error(&is_recording_ws, &stop_signal_ws);
                        return;
                    }
                }
            }

            // Create resampler to convert device sample rate to 16kHz
            let mut resampler = match AudioResampler::new(sample_rate) {
                Ok(r) => r,
                Err(e) => {
                    let _ = app_handle_ws.emit(
                        "transcription-error",
                        format!("Failed to create resampler: {}", e),
                    );
                    stop_recording_on_error(&is_recording_ws, &stop_signal_ws);
                    return;
                }
            };

            // Optional VAD processor (energy-based, no model file needed)
            let mut vad_processor: Option<VadProcessor> = if use_vad {
                VadProcessor::new(std::path::Path::new(""), 16000).ok()
            } else {
                None
            };

            // Buffer for accumulating all audio (transcribe on stop mode)
            let mut all_audio: Vec<f32> = Vec::new();

            loop {
                // Check if we should stop
                if !*is_recording_ws.lock().unwrap() {
                    break;
                }

                // Receive audio data with timeout
                match audio_rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(samples) => {
                        // Resample to 16kHz
                        if let Ok(resampled) = resampler.process(&samples) {
                            if let Some(ref mut vad) = vad_processor {
                                // VAD filters to speech-only audio
                                let speech = vad.process(&resampled);
                                all_audio.extend(speech);
                            } else {
                                // No VAD - keep all audio
                                all_audio.extend(resampled);
                            }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // No data, continue checking
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        // Channel closed - process remaining audio
                        break;
                    }
                }
            }

            // Transcription happens here after loop exits (either from stop signal or channel disconnect)

            // Take whatever is still waiting in the channel. The loop above
            // leaves the moment recording stops, so without this the last
            // chunks captured - and any backlog, if resampling fell behind -
            // would never reach the model.
            while let Ok(samples) = audio_rx.try_recv() {
                if let Ok(resampled) = resampler.process(&samples) {
                    if let Some(ref mut vad) = vad_processor {
                        let speech = vad.process(&resampled);
                        all_audio.extend(speech);
                    } else {
                        all_audio.extend(resampled);
                    }
                }
            }

            // Emit processing state
            let _ = app_handle_ws.emit("transcription-processing", ());
            let _complete = CompleteOnDrop(app_handle_ws.clone());

            // Flush resampler
            if let Ok(final_samples) = resampler.flush() {
                if let Some(ref mut vad) = vad_processor {
                    let speech = vad.process(&final_samples);
                    all_audio.extend(speech);
                } else {
                    all_audio.extend(final_samples);
                }
            }

            // Flush VAD if used
            if let Some(ref mut vad) = vad_processor {
                let remaining = vad.flush();
                all_audio.extend(remaining);
            }

            // Transcribe all accumulated audio at once
            if !all_audio.is_empty() {
                // Transcription only starts once recording stops, so this time
                // is exactly how long the app looks frozen after pressing the
                // shortcut. Loading a model the first time is counted here too.
                let audio_seconds = all_audio.len() as f32 / 16_000.0;
                let (peak, rms, silence) = audio_stats(&all_audio);
                let level_before = speech_level(&all_audio);

                // Whisper does not return nothing when it is given silence -
                // it invents sentences, sometimes in a language nobody spoke,
                // and auto-type puts them straight into whatever app is in
                // front. So silence never reaches the model.
                if !holds_speech(level_before, peak) {
                    eprintln!(
                        "dictation: skipped {:.1}s, no speech in it (speech={:.4} peak={:.3})",
                        audio_seconds, level_before, peak
                    );
                    let _ = app_handle_ws.emit(
                        "dictation-stats",
                        DictationStats {
                            model: "skipped - no speech".to_string(),
                            seconds: audio_seconds,
                            level_before,
                            level_after: level_before,
                            gain: 1.0,
                            took: 0.0,
                            chars: 0,
                        },
                    );
                    let _ = app_handle_ws.emit(
                        "transcription-error",
                        format!(
                            "These {:.0} seconds held no speech, so nothing was typed. If you \
                             were speaking, the microphone is not reaching the app: check the \
                             input volume in System Settings > Sound > Input and close other \
                             apps holding the microphone (Teams, Zoom).",
                            audio_seconds
                        ),
                    );
                    return;
                }

                // Drop the quiet head and tail, then raise the level, then
                // save, so the saved file is exactly what the model was given.
                let trimmed = trim_quiet_edges(&mut all_audio);
                let gain = boost_quiet_audio(&mut all_audio);
                let level_after = speech_level(&all_audio);
                let saved_input = save_model_input(&all_audio);

                let started = std::time::Instant::now();
                // Panicking here would skip "transcription-complete" and hang both windows.
                let (result, model_id) = match transcription_manager.lock() {
                    Ok(mut manager) => {
                        let id = manager.get_loaded_model_id().unwrap_or("none").to_string();
                        let r = manager.transcribe(&all_audio, None);
                        (r, id)
                    }
                    Err(e) => {
                        let _ = app_handle_ws.emit(
                            "transcription-error",
                            format!(
                                "The transcription engine is in a broken state after an earlier \
                                 failure ({}). Restart Omegawhisper.",
                                e
                            ),
                        );
                        return;
                    }
                };
                let took = started.elapsed().as_secs_f32();

                // The same numbers as the log line below, sent to the main
                // window, so a result can be judged without opening a log.
                let _ = app_handle_ws.emit(
                    "dictation-stats",
                    DictationStats {
                        model: model_id.clone(),
                        seconds: audio_seconds,
                        level_before,
                        level_after,
                        gain,
                        took,
                        chars: result
                            .as_ref()
                            .map(|t| t.trim().chars().count())
                            .unwrap_or(0),
                    },
                );

                // Everything that differs between a good result and a bad one,
                // on one line, so two dictations can be compared directly.
                eprintln!(
                    "[{}] dictation: model={} vad={} audio={:.1}s \
                     peak={:.2} rms={:.3} silence={:.0}% trimmed={:.1}s \
                     speech={:.4}->{:.4} gain={:.1}x took={:.1}s",
                    now(),
                    model_id,
                    use_vad,
                    audio_seconds,
                    peak,
                    rms,
                    silence * 100.0,
                    trimmed,
                    level_before,
                    level_after,
                    gain,
                    took
                );
                if let Some(path) = saved_input {
                    eprintln!("  what the model heard: {}", path.display());
                }

                // A microphone this quiet cannot be rescued by raising the
                // level, and the model will return little or nothing. Say so,
                // otherwise a long dictation just disappears with no reason
                // given. 0.02 is about a quarter of normal speech loudness.
                if level_after < 0.02 {
                    let _ = app_handle_ws.emit(
                        "transcription-error",
                        format!(
                            "The microphone was almost silent for these {:.0} seconds, so most \
                             of the speech could not be read. Check the input volume in System \
                             Settings > Sound > Input, speak closer to the microphone, and close \
                             other apps holding the microphone (Teams, Zoom).",
                            audio_seconds
                        ),
                    );
                }

                match result {
                    Ok(text) => {
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            eprintln!("Transcription returned no text; nothing to type.");
                        }
                        if !text.is_empty() {
                            if auto_type {
                                eprintln!(
                                    "Auto-typing {} characters: {:?}",
                                    text.chars().count(),
                                    text
                                );
                                if let Err(e) = type_text_internal(&format!("{} ", text)) {
                                    eprintln!("Auto-type failed: {}", e);
                                    // Typing was refused, so hand the text to
                                    // the clipboard instead. A minute of speech
                                    // must never end up nowhere.
                                    let _ = app_handle_ws.emit("auto-type-failed", &text);
                                    let _ = app_handle_ws.emit("transcription-error", e);
                                }
                            } else {
                                eprintln!(
                                    "Auto-type is off: {} characters went to the window only.",
                                    text.chars().count()
                                );
                            }
                            let event = TranscriptionEvent {
                                text,
                                is_final: true,
                            };
                            let _ = app_handle_ws.emit("transcription", event);
                        }
                    }
                    Err(e) => {
                        let _ = app_handle_ws.emit("transcription-error", e);
                    }
                }
            }

            // "transcription-complete" is sent by _complete when this thread ends.
        });
    } else {
        // Spawn WebSocket thread for Deepgram or Hyperwhisper server
        thread::spawn(move || {
            // Helper to stop recording on error
            let stop_recording_on_error =
                |is_recording: &Arc<Mutex<bool>>,
                 stop_signal: &Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>| {
                    *is_recording.lock().unwrap() = false;
                    if let Some(stop_tx) = stop_signal.lock().unwrap().take() {
                        let _ = stop_tx.send(());
                    }
                };

            // Connect to Hyperwhisper server or Deepgram
            let mut ws = if use_hyperwhisper {
                match connect_to_hyperwhisper_server(
                    &api_key,
                    sample_rate,
                    &hyperwhisper_url,
                    hyperwhisper_https,
                ) {
                    Ok(ws) => ws,
                    Err(e) => {
                        eprintln!("Failed to connect to Hyperwhisper server: {}", e);
                        let _ = app_handle_ws.emit("transcription-error", e);
                        stop_recording_on_error(&is_recording_ws, &stop_signal_ws);
                        return;
                    }
                }
            } else {
                match connect_to_deepgram(&api_key, sample_rate) {
                    Ok(ws) => ws,
                    Err(e) => {
                        eprintln!("Failed to connect to Deepgram: {}", e);
                        let _ = app_handle_ws.emit("transcription-error", e);
                        stop_recording_on_error(&is_recording_ws, &stop_signal_ws);
                        return;
                    }
                }
            };

            // Set read timeout so we can check for stop signal and send audio
            let _ = ws.set_read_timeout(Some(Duration::from_millis(50)));

            // Helper closure to process incoming Deepgram messages
            let process_message =
                |ws: &mut WsStream, app_handle: &AppHandle, auto_type: bool| -> Option<bool> {
                    match ws.read() {
                        Ok(Message::Text(text)) => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                                if json.get("type").and_then(|t| t.as_str()) == Some("Results") {
                                    let transcript = json
                                        .get("channel")
                                        .and_then(|c| c.get("alternatives"))
                                        .and_then(|a| a.get(0))
                                        .and_then(|a| a.get("transcript"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");

                                    let is_final = json
                                        .get("is_final")
                                        .and_then(|f| f.as_bool())
                                        .unwrap_or(false);

                                    if !transcript.is_empty() {
                                        // Type final transcriptions in real-time if enabled
                                        if is_final && auto_type {
                                            // Add a space before the text (except potentially first word)
                                            let text_to_type = format!("{} ", transcript);
                                            eprintln!(
                                                "Auto-typing {} characters: {:?}",
                                                transcript.chars().count(),
                                                transcript
                                            );
                                            if let Err(e) = type_text_internal(&text_to_type) {
                                                eprintln!("Auto-type failed: {}", e);
                                            }
                                        }

                                        let event = TranscriptionEvent {
                                            text: transcript.to_string(),
                                            is_final,
                                        };
                                        let _ = app_handle.emit("transcription", event);
                                    }
                                }
                            }
                            Some(true) // Continue
                        }
                        Ok(Message::Close(_)) => {
                            Some(false) // Stop
                        }
                        Err(tungstenite::Error::Io(ref e))
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                || e.kind() == std::io::ErrorKind::TimedOut =>
                        {
                            None // Timeout, no message
                        }
                        Err(_) => {
                            Some(false) // Error, stop
                        }
                        _ => Some(true),
                    }
                };

            loop {
                // Check if we should stop
                if !*is_recording_ws.lock().unwrap() {
                    // Send CloseStream to Deepgram to signal end of audio
                    let _ = ws.send(Message::Text("{\"type\":\"CloseStream\"}".to_string()));

                    // Notify frontend that we're processing remaining transcriptions
                    let _ = app_handle_ws.emit("transcription-processing", ());

                    // Keep reading for pending transcription results (up to 5 seconds)
                    let drain_start = std::time::Instant::now();
                    while drain_start.elapsed() < Duration::from_secs(5) {
                        match process_message(&mut ws, &app_handle_ws, auto_type) {
                            Some(false) => break, // Close or error
                            Some(true) => {}      // Got a message, keep reading
                            None => {
                                // Timeout with no message - if we've waited at least 1 second, we're done
                                if drain_start.elapsed() > Duration::from_millis(1000) {
                                    break;
                                }
                            }
                        }
                    }

                    let _ = ws.close(None);

                    // Notify frontend that transcription processing is complete
                    let _ = app_handle_ws.emit("transcription-complete", ());

                    break;
                }

                // Send any pending audio data
                while let Ok(samples) = audio_rx.try_recv() {
                    let pcm_data = samples_to_linear16(&samples);
                    if let Err(e) = ws.send(Message::Binary(pcm_data)) {
                        eprintln!("Failed to send audio: {}", e);
                        return;
                    }
                }

                // Try to read messages from Deepgram (with timeout)
                if let Some(false) = process_message(&mut ws, &app_handle_ws, auto_type) {
                    // Connection closed or error - still emit complete event
                    let _ = app_handle_ws.emit("transcription-complete", ());
                    break;
                }
            }
        });
    } // End of if use_local else block

    // Newest microphone level and the most recent samples, written by the
    // capture callback and read by the thread below. The callback must not
    // wait on anything, so it only stores data here and never talks to the UI
    // itself, and never does the frequency maths.
    let mic_level = Arc::new(Mutex::new((0.0f32, 0.0f32)));
    let mic_recent = Arc::new(Mutex::new(Vec::<f32>::new()));

    // Everything the windows draw comes from here, 20 times a second. The
    // windows used to open the microphone themselves to draw with, which is
    // what made macOS wind the recording's gain up over the first seconds.
    {
        let mic_level = mic_level.clone();
        let mic_recent = mic_recent.clone();
        let is_recording = is_recording_arc.clone();
        let app_handle_level = app_handle.clone();
        thread::spawn(move || {
            let mut planner = rustfft::FftPlanner::<f32>::new();
            let fft = planner.plan_fft_forward(FFT_SIZE);
            let started = std::time::Instant::now();
            let mut tick = 0u32;
            let mut pitch = 0.0f32;

            while *is_recording.lock().unwrap() {
                let (peak, rms) = *mic_level.lock().unwrap();
                let recent = mic_recent.lock().unwrap().clone();

                let bands = frequency_bands(&recent, &fft);
                // Pitch costs more than the rest put together, so it runs 5
                // times a second rather than 20.
                if tick.is_multiple_of(4) {
                    pitch = detect_pitch(&recent, sample_rate);
                }
                tick += 1;

                let _ = app_handle_level.emit(
                    "mic-level",
                    MicLevel {
                        peak,
                        rms,
                        seconds: started.elapsed().as_secs_f32(),
                        pitch,
                        bands,
                    },
                );
                thread::sleep(Duration::from_millis(50));
            }
        });
    }

    // Spawn audio recording thread
    let app_handle_audio = app_handle.clone();
    let is_recording_audio = is_recording_arc.clone();
    thread::spawn(move || {
        // The microphone would not open at all.
        let fail = |message: String| {
            report_microphone_failure(&app_handle_audio, &is_recording_audio, message);
        };
        // It opened, then died part-way through: unplugged, or taken by another
        // app. The sound from that moment on was never captured and cannot be
        // recovered, so stop and keep what was already recorded.
        let broken = {
            let app = app_handle_audio.clone();
            let flag = is_recording_audio.clone();
            move |err: String| {
                report_microphone_failure(
                    &app,
                    &flag,
                    format!(
                        "The microphone stopped during the recording ({}). Whatever was \
                         recorded before it stopped has been kept.",
                        err
                    ),
                );
            }
        };
        let targets = CaptureTargets {
            is_recording: is_recording_arc.clone(),
            recorded_samples: recorded_samples_arc.clone(),
            mic_level: mic_level.clone(),
            mic_recent: mic_recent.clone(),
            audio_tx: audio_tx.clone(),
            channels,
        };
        let stream_result = match sample_format {
            SampleFormat::F32 => {
                build_capture_stream(&device, &stream_config, targets, broken, |s: f32| s)
            }
            SampleFormat::I16 => {
                build_capture_stream(&device, &stream_config, targets, broken, i16_to_f32)
            }
            SampleFormat::U16 => {
                build_capture_stream(&device, &stream_config, targets, broken, u16_to_f32)
            }
            _ => {
                fail(format!(
                    "This microphone sends audio in a format the app cannot read ({:?}).",
                    sample_format
                ));
                return;
            }
        };

        let stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                fail(format!(
                    "The microphone could not be opened: {}. Check that no other app \
                     is holding it, and that Omegawhisper is allowed in System \
                     Settings > Privacy & Security > Microphone.",
                    e
                ));
                return;
            }
        };

        if let Err(e) = stream.play() {
            fail(format!("The microphone opened but would not start: {}.", e));
            return;
        }

        // Keep stream alive until stop signal
        let _ = stop_rx.recv();
    });

    Ok(())
}

#[tauri::command]
async fn stop_recording(
    state: State<'_, AudioState>,
    _app_handle: AppHandle,
) -> Result<String, String> {
    {
        let is_recording = state.is_recording.lock().unwrap();
        if !*is_recording {
            return Err("Not recording".to_string());
        }
    }

    eprintln!("[{}] recording stopped", now());

    // Stop recording
    *state.is_recording.lock().unwrap() = false;

    // Send stop signal
    let stop_tx = state.stop_signal.lock().unwrap().take();
    if let Some(stop_tx) = stop_tx {
        let _ = stop_tx.send(());
    }

    // Wait for buffers to flush. Awaited, not slept: this runs on the shared runtime.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Get recorded samples
    let samples = state.recorded_samples.lock().unwrap().clone();
    if samples.is_empty() {
        return Err("No audio data recorded".to_string());
    }

    // Convert to WAV
    let sample_rate = state.sample_rate.lock().unwrap().unwrap_or(48000);
    let wav_bytes = to_wav_bytes(&samples, sample_rate, 1);

    // Save to disk
    let recordings_dir = get_recordings_dir()?;
    let timestamp = Utc::now().format("%Y%m%d-%H%M%SUTC").to_string();
    let file_name = format!("{}.wav", timestamp);
    let file_path = recordings_dir.join(&file_name);

    fs::write(&file_path, &wav_bytes).map_err(|e| format!("Failed to save recording: {}", e))?;

    // Clear recorded samples
    *state.recorded_samples.lock().unwrap() = Vec::new();

    // The base64 copy of the recording that used to be returned here is gone.
    // A minute of audio made a 31 MB string on the shared runtime, and nothing
    // ever read it.
    let response = serde_json::json!({
        "filePath": file_path.to_string_lossy()
    });

    Ok(response.to_string())
}

#[tauri::command]
fn is_recording(state: State<'_, AudioState>) -> bool {
    *state.is_recording.lock().unwrap()
}

#[tauri::command]
fn type_text(text: String) -> Result<(), String> {
    type_text_internal(&text)
}

/// Query dconf for the keybinding associated with omegawhisper
/// Returns the keybinding string (e.g., "<Super>m") or None if not found
#[tauri::command]
fn get_keybinding() -> Option<String> {
    use std::process::Command;

    // Run dconf dump / to get all settings
    let output = Command::new("dconf").args(["dump", "/"]).output().ok()?;

    if !output.status.success() {
        return None;
    }

    let dump = String::from_utf8_lossy(&output.stdout);

    // Search patterns for omegawhisper keybindings.
    // The old hyperwhisper names stay so a keybinding made before the
    // rename is still found.
    let search_patterns = [
        "omegawhisper",
        "hyperwhisper",
        "ToggleRecording",
        "dev.omegawhisper",
        "dev.hyperwhisper",
    ];

    // Parse dconf dump format - it's INI-like with [path] sections
    // First, split into sections and find the one containing omegawhisper
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    let mut current_section = String::new();
    let mut current_lines: Vec<&str> = Vec::new();

    for line in dump.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            // Save previous section
            if !current_section.is_empty() {
                sections.push((current_section.clone(), current_lines.clone()));
            }
            current_section = line[1..line.len() - 1].to_string();
            current_lines = Vec::new();
        } else if !line.is_empty() {
            current_lines.push(line);
        }
    }
    // Don't forget the last section
    if !current_section.is_empty() {
        sections.push((current_section, current_lines));
    }

    // Find sections that contain omegawhisper in any line
    for (section_name, lines) in &sections {
        let section_text = lines.join("\n");
        let has_pattern = search_patterns.iter().any(|p| {
            section_name.to_lowercase().contains(&p.to_lowercase())
                || section_text.to_lowercase().contains(&p.to_lowercase())
        });

        if has_pattern {
            // Look for binding= in this section
            for line in lines {
                if let Some(eq_pos) = line.find('=') {
                    let key = line[..eq_pos].trim().to_lowercase();
                    let value = line[eq_pos + 1..].trim();

                    if key == "binding" {
                        let cleaned = value.trim_matches('\'').trim_matches('"');
                        if !cleaned.is_empty() && cleaned != "disabled" && cleaned != "[]" {
                            return Some(cleaned.to_string());
                        }
                    }
                }
            }
        }
    }

    None
}

// Everything the app prints goes to one file, whatever started it - Finder,
// the tray, or a terminal. Launched from Finder there is no terminal to print
// to, so a dictation that went wrong used to leave no trace at all.
#[cfg(unix)]
fn redirect_output_to_log() {
    let Some(dir) = dirs::data_local_dir() else {
        return;
    };
    let path = dir.join("omegawhisper").join("omegawhisper.log");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // Start over once the file gets big rather than growing without end.
    if fs::metadata(&path)
        .map(|m| m.len() > 5_000_000)
        .unwrap_or(false)
    {
        let _ = fs::remove_file(&path);
    }

    let Ok(file) = fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    use std::os::unix::io::AsRawFd;
    unsafe {
        libc::dup2(file.as_raw_fd(), libc::STDOUT_FILENO);
        libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
    }
    // The file must outlive this function: the two descriptors above now
    // point at it and closing it here would close them too.
    std::mem::forget(file);

    eprintln!(
        "\n===== started {} =====",
        Local::now().format("%F %H:%M:%S")
    );
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
        let device = match get_input_device() {
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

    // Language choice from the last run.
    let saved_prefs = load_tray_prefs();

    // Initialize model manager
    let model_manager = Arc::new(ModelManager::new().expect("Failed to initialize model manager"));

    // Initialize transcription manager
    let transcription_manager = SharedTranscriptionManager::new(model_manager.clone());

    let audio_state = AudioState {
        is_recording: Arc::new(Mutex::new(false)),
        recorded_samples: Arc::new(Mutex::new(Vec::new())),
        sample_rate: Arc::new(Mutex::new(None)),
        stop_signal: Arc::new(Mutex::new(None)),
        api_key: Arc::new(Mutex::new(None)),
        use_hyperwhisper_server: Arc::new(Mutex::new(true)),
        hyperwhisper_server_url: Arc::new(Mutex::new("hyperwhisper.dev".to_string())),
        hyperwhisper_server_https: Arc::new(Mutex::new(true)),
        hyperwhisper_api_key: Arc::new(Mutex::new(None)),
        auto_type_transcription: Arc::new(Mutex::new(false)),
        selected_device_id: Arc::new(Mutex::new(None)),
        use_local_transcription: Arc::new(Mutex::new(false)),
        local_model_path: Arc::new(Mutex::new(None)),
        active_local_model_id: Arc::new(Mutex::new(None)),
        model_manager,
        transcription_manager,
        // Keep speech only. Feeding silence to Whisper makes it invent text.
        use_vad: Arc::new(Mutex::new(true)),
        debug_stats: Arc::new(Mutex::new(saved_prefs.debug_stats)),
        startup_warnings: Arc::new(Mutex::new(startup_warnings)),
        debug_menu_item: Arc::new(Mutex::new(None)),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        // Global shortcut toggles recording from anywhere.
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    use tauri_plugin_global_shortcut::ShortcutState;
                    if event.state() == ShortcutState::Pressed {
                        eprintln!("[{}] F3 pressed", now());
                        let _ = app.emit("recording-toggled", ());
                    }
                })
                .build(),
        )
        .manage(audio_state)
        .invoke_handler(tauri::generate_handler![
            start_recording,
            stop_recording,
            is_recording,
            set_api_key,
            set_hyperwhisper_server_settings,
            type_text,
            set_auto_type_transcription,
            list_audio_devices,
            get_selected_device,
            set_selected_device,
            provision_trial_key,
            get_trial_status,
            get_trial_usage,
            get_device_fingerprint,
            set_use_local_transcription,
            set_local_model_path,
            get_local_model_path,
            check_local_model_status,
            download_local_model,
            // Multi-model management commands
            list_available_models,
            get_model_status,
            download_model,
            delete_model,
            set_active_model,
            get_active_model,
            load_active_model,
            unload_model,
            is_model_loaded,
            get_loaded_model,
            set_use_vad,
            get_use_vad,
            get_keybinding,
            position_indicator,
            get_debug_stats,
            set_debug_stats,
            get_startup_warnings,
            hide_main_window,
        ])
        .setup(|app| {
            // F3 toggles recording from anywhere.
            #[cfg(desktop)]
            {
                use tauri::Manager;
                use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut};
                if let Err(e) = app
                    .global_shortcut()
                    .register(Shortcut::new(None, Code::F3))
                {
                    eprintln!("Failed to register F3 shortcut: {}", e);
                    app.state::<AudioState>()
                        .startup_warnings
                        .lock()
                        .unwrap()
                        .push(format!(
                            "F3 could not be registered, so the shortcut will not work. \
                             Another app is probably using it. ({})",
                            e
                        ));
                }
            }

            // Menu-bar tray: open/hide window, pick language, quit.
            #[cfg(desktop)]
            {
                use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
                use tauri::tray::TrayIconBuilder;
                use tauri::Manager;

                let open_item = MenuItem::with_id(
                    app,
                    "open_settings",
                    "Open Omegawhisper",
                    true,
                    None::<&str>,
                )?;
                let settings_item = MenuItem::with_id(
                    app,
                    "open_settings_window",
                    "Settings...",
                    true,
                    None::<&str>,
                )?;
                let hide_item =
                    MenuItem::with_id(app, "hide_window", "Hide window", true, None::<&str>)?;
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
                let saved_debug = *app.state::<AudioState>().debug_stats.lock().unwrap();
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
                        &open_item,
                        &hide_item,
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
                        "open_settings" => {
                            // regular app so the window is focusable
                            #[cfg(target_os = "macos")]
                            let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
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
                                        // back to background agent when settings closes
                                        // and the main window is not on screen
                                        #[cfg(target_os = "macos")]
                                        {
                                            let handle = app.clone();
                                            w.on_window_event(move |event| {
                                                if let tauri::WindowEvent::Destroyed = event {
                                                    let main_visible = handle
                                                        .get_webview_window("main")
                                                        .and_then(|m| m.is_visible().ok())
                                                        .unwrap_or(false);
                                                    if !main_visible {
                                                        let _ = handle.set_activation_policy(
                                                            tauri::ActivationPolicy::Accessory,
                                                        );
                                                    }
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
                        "hide_window" => {
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.hide();
                            }
                            #[cfg(target_os = "macos")]
                            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                        }
                        "debug_stats" => {
                            let now_on = !*app.state::<AudioState>().debug_stats.lock().unwrap();
                            set_debug_stats_everywhere(app, now_on);
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    });

                // template icon renders white on the macOS menu bar
                if let Ok(icon) =
                    tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon.png"))
                {
                    tray = tray.icon(icon).icon_as_template(true);
                } else if let Some(icon) = app.default_window_icon().cloned() {
                    tray = tray.icon(icon).icon_as_template(true);
                }
                let _ = tray.build(app)?;
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
                        position_indicator(app.handle().clone());
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

// Tests. Plain maths on lists of numbers: no microphone, no model, no windows.
#[cfg(test)]
mod tests {
    use super::*;

    fn near(got: f32, want: f32, tolerance: f32, what: &str) {
        assert!(
            (got - want).abs() <= tolerance,
            "{what}: got {got}, wanted {want} (allowed {tolerance})"
        );
    }

    // Blocks of 480 samples (30 ms at 16 kHz), alternating +/- amplitude so the
    // measured loudness is exactly that amplitude.
    fn blocks(spec: &[(usize, f32)]) -> Vec<f32> {
        let mut out = Vec::new();
        for &(count, amplitude) in spec {
            for _ in 0..count {
                for i in 0..480 {
                    out.push(if i % 2 == 0 { amplitude } else { -amplitude });
                }
            }
        }
        out
    }

    fn sine(count: usize, hz: f32, sample_rate: f32, amplitude: f32) -> Vec<f32> {
        (0..count)
            .map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / sample_rate).sin() * amplitude)
            .collect()
    }

    // Repeatable stand-in for noise, so a failure can be reproduced.
    fn noise(count: usize, amplitude: f32) -> Vec<f32> {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        (0..count)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let unit = (state >> 32) as f32 / u32::MAX as f32;
                (unit * 2.0 - 1.0) * amplitude
            })
            .collect()
    }

    fn peak_of(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0f32, |max, s| max.max(s.abs()))
    }

    // ---- turning what the device sends into numbers between -1 and 1 -------

    #[test]
    fn sixteen_bit_signed_samples_convert() {
        // (raw value from the device, expected, what this case is for)
        let cases: &[(i16, f32, &str)] = &[
            (0, 0.0, "silence sits at zero"),
            (i16::MAX, 1.0, "the loudest positive value reaches 1.0"),
            (-i16::MAX, -1.0, "the loudest negative value reaches -1.0"),
            (16383, 0.5, "half loudness"),
            (-16383, -0.5, "half loudness, negative"),
        ];
        for &(raw, want, what) in cases {
            near(i16_to_f32(raw), want, 0.0001, what);
        }
        // One value below the loudest negative. It is allowed to land slightly
        // past -1.0; the check that follows every boost keeps it in range.
        assert!(i16_to_f32(i16::MIN) >= -1.001);
    }

    #[test]
    fn sixteen_bit_unsigned_samples_convert() {
        let cases: &[(u16, f32, &str)] = &[
            (32768, 0.0, "silence sits in the middle, not at zero"),
            (0, -1.0, "the bottom of the range is the loudest negative"),
            (
                u16::MAX,
                1.0,
                "the top of the range is the loudest positive",
            ),
            (49151, 0.5, "half loudness"),
            (16384, -0.5, "half loudness, negative"),
        ];
        for &(raw, want, what) in cases {
            near(u16_to_f32(raw), want, 0.0001, what);
        }
    }

    #[test]
    fn silence_from_an_unsigned_device_stays_silent() {
        // Scaling without shifting would turn a silent room into a steady tone.
        let quiet: Vec<f32> = vec![32768u16; 4800].into_iter().map(u16_to_f32).collect();
        near(peak_of(&quiet), 0.0, 0.0001, "silence must stay silent");
        assert!(!holds_speech(speech_level(&quiet), peak_of(&quiet)));
    }

    // ---- mixing several channels down to one ------------------------------

    #[test]
    fn channels_are_averaged_into_one() {
        // (channels, what the device sent, what should come out, why)
        let cases: &[(u16, &[f32], &[f32], &str)] = &[
            (
                1,
                &[0.1, 0.2, 0.3],
                &[0.1, 0.2, 0.3],
                "one channel passes through",
            ),
            (
                0,
                &[0.1, 0.2],
                &[0.1, 0.2],
                "a nonsense channel count changes nothing",
            ),
            (
                2,
                &[1.0, 0.0, 0.5, 0.5],
                &[0.5, 0.5],
                "two channels are averaged",
            ),
            (
                2,
                &[1.0, -1.0],
                &[0.0],
                "opposite channels cancel, they are not just dropped",
            ),
            (3, &[0.3, 0.6, 0.9], &[0.6], "three channels are averaged"),
            (
                2,
                &[1.0, 0.0, 1.0],
                &[0.5, 1.0],
                "a half-finished frame at the end is kept",
            ),
            (2, &[], &[], "nothing in, nothing out"),
        ];
        for &(channels, input, want, what) in cases {
            let got = mix_to_mono(input.to_vec(), channels);
            assert_eq!(got.len(), want.len(), "{what}: wrong length");
            for (i, (&g, &w)) in got.iter().zip(want.iter()).enumerate() {
                near(g, w, 0.0001, &format!("{what}, sample {i}"));
            }
        }
    }

    // ---- how loud is the speech -------------------------------------------

    #[test]
    fn speech_loudness_ignores_pauses_and_single_bangs() {
        // (recording, expected loudness, why this case exists)
        let cases: &[(Vec<f32>, f32, &str)] = &[
            (Vec::new(), 0.0, "nothing recorded"),
            (vec![0.5; 100], 0.0, "shorter than one 30 ms block"),
            (
                blocks(&[(1, 0.3)]),
                0.3,
                "exactly one block reports its own loudness",
            ),
            (
                blocks(&[(50, 0.08)]),
                0.08,
                "steady speech reports its loudness",
            ),
            (
                blocks(&[(99, 0.002), (1, 0.9)]),
                0.002,
                "one door slam in a quiet minute does not count as speech",
            ),
            (
                blocks(&[(80, 0.002), (20, 0.08)]),
                0.08,
                "real speech among pauses does count",
            ),
        ];
        for (recording, want, what) in cases {
            near(speech_level(recording), *want, 0.0001, what);
        }
    }

    #[test]
    fn a_single_block_does_not_wrap_to_the_quietest_one() {
        near(
            speech_level(&blocks(&[(1, 0.42)])),
            0.42,
            0.0001,
            "one block",
        );
    }

    // ---- the rule that silence must never reach the model -----------------

    #[test]
    fn only_real_speech_is_sent_to_the_model() {
        // (speech loudness, loudest sample, may it be sent, why)
        let cases: &[(f32, f32, bool, &str)] = &[
            (0.0, 0.0, false, "digital silence"),
            (
                0.0014,
                0.02,
                false,
                "an empty room, as measured on this Mac",
            ),
            (
                0.004,
                0.60,
                false,
                "one click: loud peak, but nothing is being said",
            ),
            (0.08, 0.02, false, "a steady hum: high average, no peaks"),
            (0.005, 0.05, true, "exactly on both thresholds"),
            (0.08, 0.50, true, "normal speech close to the microphone"),
            (0.30, 0.99, true, "loud speech"),
        ];
        for &(level, peak, want, what) in cases {
            assert_eq!(holds_speech(level, peak), want, "{what}");
        }
    }

    // ---- raising the level of a quiet recording ---------------------------

    #[test]
    fn quiet_recordings_are_raised_but_an_empty_room_is_not() {
        // (recording, expected gain, why)
        // A gain of 1.0 means the recording was left exactly as it was.
        let cases: &[(Vec<f32>, f32, &str)] = &[
            (Vec::new(), 1.0, "nothing recorded"),
            (blocks(&[(10, 0.0)]), 1.0, "digital silence is never raised"),
            (
                blocks(&[(10, 0.002)]),
                1.0,
                "an empty room stays quiet - raising it is how invented sentences got typed",
            ),
            (
                blocks(&[(10, 0.02)]),
                4.0,
                "quiet speech is brought up to normal",
            ),
            (
                blocks(&[(10, 0.006)]),
                10.0,
                "very quiet speech stops at ten times",
            ),
            (
                blocks(&[(10, 0.2)]),
                1.0,
                "speech that is already loud is left alone",
            ),
        ];
        for (recording, want, what) in cases {
            let mut audio = recording.clone();
            near(boost_quiet_audio(&mut audio), *want, 0.01, what);
        }
    }

    #[test]
    fn raising_the_level_never_pushes_a_sample_past_the_limit() {
        let recordings: &[(Vec<f32>, &str)] = &[
            (blocks(&[(10, 0.006)]), "very quiet speech"),
            (blocks(&[(10, 0.02)]), "quiet speech"),
            (
                blocks(&[(99, 0.006), (1, 0.9)]),
                "quiet speech with one loud bang in it",
            ),
            (sine(48_000, 220.0, 16_000.0, 0.01), "a quiet steady tone"),
            (noise(48_000, 0.01), "quiet noise"),
        ];
        for (recording, what) in recordings {
            let mut audio = recording.clone();
            boost_quiet_audio(&mut audio);
            let peak = peak_of(&audio);
            assert!(peak <= 1.0, "{what}: loudest sample reached {peak}");
        }
    }

    #[test]
    fn the_level_after_raising_matches_the_gain_reported() {
        let mut audio = blocks(&[(10, 0.02)]);
        let before = speech_level(&audio);
        let gain = boost_quiet_audio(&mut audio);
        let after = speech_level(&audio);
        near(
            after,
            before * gain,
            0.001,
            "reported gain matches the result",
        );
        near(
            after,
            0.08,
            0.001,
            "quiet speech ends up at normal speech loudness",
        );
    }

    // ---- cutting the quiet start and end off ------------------------------

    #[test]
    fn only_the_quiet_start_and_end_are_cut() {
        // 40 blocks of silence, 40 of speech, 40 of silence.
        let mut audio = blocks(&[(40, 0.0), (40, 0.1), (40, 0.0)]);
        let speech_samples_before = audio.iter().filter(|s| s.abs() > 0.05).count();

        let cut = trim_quiet_edges(&mut audio);

        near(cut, 0.72, 0.01, "seconds removed from the front");
        assert_eq!(audio.len(), 34_560, "length after trimming");
        assert_eq!(
            audio.iter().filter(|s| s.abs() > 0.05).count(),
            speech_samples_before,
            "every spoken sample survived the trim"
        );
    }

    #[test]
    fn a_pause_in_the_middle_of_a_sentence_is_never_cut() {
        // Cutting here would join words that were seconds apart.
        let mut audio = blocks(&[(20, 0.1), (40, 0.0), (20, 0.1)]);
        let length_before = audio.len();

        let cut = trim_quiet_edges(&mut audio);

        near(cut, 0.0, 0.0001, "nothing cut from the front");
        assert_eq!(audio.len(), length_before, "nothing cut at all");
        assert_eq!(
            audio.iter().filter(|s| s.abs() < 0.05).count(),
            40 * 480,
            "the pause is still there, at its full length"
        );
    }

    #[test]
    fn trimming_leaves_a_recording_with_no_speech_alone() {
        // (recording, why)
        let cases: &[(Vec<f32>, &str)] = &[
            (blocks(&[(20, 0.0)]), "digital silence"),
            (blocks(&[(20, 0.002)]), "an empty room"),
        ];
        for (recording, what) in cases {
            let mut audio = recording.clone();
            let length_before = audio.len();
            near(trim_quiet_edges(&mut audio), 0.0, 0.0001, what);
            assert_eq!(audio.len(), length_before, "{what}: nothing should be cut");
        }
    }

    #[test]
    fn a_quiet_tail_is_cut_without_touching_the_front() {
        let mut audio = blocks(&[(40, 0.1), (40, 0.0)]);
        let cut = trim_quiet_edges(&mut audio);
        near(cut, 0.0, 0.0001, "nothing cut from the front");
        assert_eq!(audio.len(), 26_880, "the tail was cut");
    }

    // ---- the numbers shown while recording --------------------------------

    #[test]
    fn chunk_loudness_reports_peak_and_average() {
        // (chunk, expected loudest, expected average, why)
        let cases: &[(&[f32], f32, f32, &str)] = &[
            (&[], 0.0, 0.0, "nothing recorded"),
            (&[0.0; 8], 0.0, 0.0, "silence"),
            (&[0.5, -0.5, 0.5, -0.5], 0.5, 0.5, "a steady tone"),
            (&[1.0, 0.0, 0.0, 0.0], 1.0, 0.5, "one spike among silence"),
            (
                &[-0.8, 0.1],
                0.8,
                0.5701,
                "the loudest sample can be negative",
            ),
        ];
        for &(chunk, want_peak, want_rms, what) in cases {
            let (peak, rms) = chunk_level(chunk);
            near(peak, want_peak, 0.001, &format!("{what}: loudest"));
            near(rms, want_rms, 0.001, &format!("{what}: average"));
        }
    }

    #[test]
    fn recording_statistics_report_how_much_was_near_silence() {
        // (recording, loudest, average, share near silence, why)
        let cases: &[(Vec<f32>, f32, f32, f32, &str)] = &[
            (
                Vec::new(),
                0.0,
                0.0,
                1.0,
                "nothing recorded counts as all silence",
            ),
            (vec![0.0; 100], 0.0, 0.0, 1.0, "silence"),
            (vec![0.5; 100], 0.5, 0.5, 0.0, "a steady loud signal"),
            (
                [vec![0.5; 50], vec![0.001; 50]].concat(),
                0.5,
                0.3536,
                0.5,
                "half loud, half near silence",
            ),
        ];
        for (recording, want_peak, want_rms, want_quiet, what) in cases {
            let (peak, rms, quiet) = audio_stats(recording);
            near(peak, *want_peak, 0.001, &format!("{what}: loudest"));
            near(rms, *want_rms, 0.001, &format!("{what}: average"));
            near(
                quiet,
                *want_quiet,
                0.001,
                &format!("{what}: share near silence"),
            );
        }
    }

    #[test]
    fn the_recent_sample_store_keeps_the_newest_and_stays_bounded() {
        let store = Arc::new(Mutex::new(Vec::<f32>::new()));

        // Less than the cap: everything is kept.
        keep_recent(&store, &[1.0, 2.0, 3.0]);
        assert_eq!(store.lock().unwrap().len(), 3);

        // Well past the cap, fed in pieces the way the microphone delivers it.
        let total = 6000usize;
        store.lock().unwrap().clear();
        for start in (0..total).step_by(1000) {
            let chunk: Vec<f32> = (start..start + 1000).map(|i| i as f32).collect();
            keep_recent(&store, &chunk);
        }
        let kept = store.lock().unwrap().clone();
        assert_eq!(kept.len(), 4096, "the store must not grow without limit");
        near(kept[4095], 5999.0, 0.5, "the newest sample is kept");
        near(
            kept[0],
            (total - 4096) as f32,
            0.5,
            "the oldest ones are dropped",
        );
    }

    // ---- the bars and the pitch the windows draw --------------------------

    #[test]
    fn frequency_bands_stay_in_range_and_follow_the_tone() {
        let mut planner = rustfft::FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);

        // Too short to measure: a full set of empty bars, not a crash.
        let short = frequency_bands(&[0.1; 100], &fft);
        assert_eq!(short.len(), BAND_COUNT);
        assert!(
            short.iter().all(|&b| b == 0.0),
            "too-short input gives no bars"
        );

        // Silence: every bar empty.
        let silent = frequency_bands(&vec![0.0; FFT_SIZE], &fft);
        assert!(silent.iter().all(|&b| b == 0.0), "silence gives no bars");

        // A single tone lights up one region, and no bar leaves the 0-to-1 range.
        let tone = sine(FFT_SIZE, 440.0, 16_000.0, 0.5);
        let bands = frequency_bands(&tone, &fft);
        assert_eq!(bands.len(), BAND_COUNT);
        assert!(
            bands.iter().all(|&b| (0.0..=1.0).contains(&b)),
            "a bar outside 0 to 1 would draw off the window"
        );
        let loudest = bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        assert!(
            (28..=42).contains(&loudest),
            "a 440 Hz tone should light up the middle of the display, not bar {loudest}"
        );
        assert!(
            bands[0] < bands[loudest] * 0.5,
            "the lowest bar should stay quiet"
        );
    }

    #[test]
    fn pitch_is_refused_when_it_cannot_be_told() {
        // 0.0 means "cannot be told" - better than a wrong number on screen.
        let cases: &[(Vec<f32>, u32, &str)] = &[
            (vec![0.0; 2048], 48_000, "silence"),
            (vec![0.1; 200], 48_000, "too little audio to measure"),
            (
                sine(2048, 200.0, 48_000.0, 0.001),
                48_000,
                "too quiet to measure",
            ),
            (noise(2048, 0.2), 48_000, "noise is not a voice"),
            (noise(2048, 0.5), 16_000, "loud noise is still not a voice"),
        ];
        for (audio, sample_rate, what) in cases {
            near(detect_pitch(audio, *sample_rate), 0.0, 0.0, what);
        }
    }

    #[test]
    fn pitch_is_found_for_a_voice() {
        // (audio, sample rate, expected pitch, allowed error, why)
        // The ignored test below covers the ones this gets wrong.
        let cases: &[(Vec<f32>, u32, f32, f32, &str)] = &[
            (
                sine(2048, 80.0, 48_000.0, 0.3),
                48_000,
                80.0,
                5.0,
                "a very low voice",
            ),
            (
                sine(2048, 120.0, 48_000.0, 0.3),
                48_000,
                120.0,
                6.0,
                "a low voice",
            ),
            (
                sine(2048, 200.0, 48_000.0, 0.3),
                48_000,
                200.0,
                10.0,
                "an average voice",
            ),
            (
                sine(2048, 400.0, 48_000.0, 0.3),
                48_000,
                400.0,
                20.0,
                "a high voice",
            ),
            (
                sine(2048, 120.0, 16_000.0, 0.3),
                16_000,
                120.0,
                6.0,
                "a low voice at 16 kHz",
            ),
            (
                sine(2048, 200.0, 16_000.0, 0.3),
                16_000,
                200.0,
                10.0,
                "an average voice at 16 kHz",
            ),
        ];
        for (audio, sample_rate, want, tolerance, what) in cases {
            near(detect_pitch(audio, *sample_rate), *want, *tolerance, what);
        }
    }

    #[test]
    fn pitch_should_never_be_reported_an_octave_too_low() {
        for hz in [150.0f32, 220.0, 250.0, 280.0, 300.0, 330.0, 350.0] {
            for sample_rate in [48_000u32, 16_000] {
                let audio = sine(2048, hz, sample_rate as f32, 0.3);
                let got = detect_pitch(&audio, sample_rate);
                assert!(
                    (got - hz).abs() / hz < 0.06,
                    "{hz} Hz at {sample_rate} Hz was reported as {got} Hz"
                );
            }
        }
    }

    // ---- writing the audio out --------------------------------------------

    #[test]
    fn wav_files_have_a_correct_header() {
        let wav = to_wav_bytes(&[0.0; 10], 16_000, 1);
        assert_eq!(wav.len(), 44 + 20, "44-byte header plus 2 bytes per sample");
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 20);
        assert_eq!(
            u16::from_le_bytes(wav[22..24].try_into().unwrap()),
            1,
            "channels"
        );
        assert_eq!(
            u32::from_le_bytes(wav[24..28].try_into().unwrap()),
            16_000,
            "sample rate"
        );
        assert_eq!(
            u16::from_le_bytes(wav[34..36].try_into().unwrap()),
            16,
            "bits per sample"
        );
        assert_eq!(
            u32::from_le_bytes(wav[40..44].try_into().unwrap()),
            20,
            "data size"
        );

        // An empty recording still produces a valid, empty file.
        assert_eq!(to_wav_bytes(&[], 16_000, 1).len(), 44);
    }

    #[test]
    fn samples_outside_the_allowed_range_are_pulled_back_in() {
        // (sample, expected 16-bit value, why)
        let cases: &[(f32, i16, &str)] = &[
            (0.0, 0, "silence"),
            (1.0, i16::MAX, "the loudest allowed value"),
            (-1.0, -i16::MAX, "the quietest allowed value"),
            (
                2.0,
                i16::MAX,
                "past the top: pulled back, not wrapped around",
            ),
            (
                -2.0,
                -i16::MAX,
                "past the bottom: pulled back, not wrapped around",
            ),
        ];
        for &(sample, want, what) in cases {
            let wav = to_wav_bytes(&[sample], 16_000, 1);
            let got = i16::from_le_bytes(wav[44..46].try_into().unwrap());
            assert_eq!(got, want, "{what}");

            let pcm = samples_to_linear16(&[sample]);
            assert_eq!(
                i16::from_le_bytes(pcm[0..2].try_into().unwrap()),
                want,
                "{what} (streaming path)"
            );
        }
        assert!(samples_to_linear16(&[]).is_empty());
        assert_eq!(
            samples_to_linear16(&[0.0; 5]).len(),
            10,
            "2 bytes per sample"
        );
    }

    // ---- settings that have to survive a restart --------------------------

    #[test]
    fn tray_settings_survive_a_restart() {
        // (stored text, expected debug line, why)
        let cases: &[(&str, bool, &str)] = &[
            (r#"{"debug_stats":true}"#, true, "switched on"),
            (r#"{"debug_stats":false}"#, false, "switched off"),
            (
                r#"{"language":"en","debug_stats":true}"#,
                true,
                "a file from when the language menu existed still loads",
            ),
            (r#"{}"#, false, "an empty settings file loads as defaults"),
        ];
        for &(stored, want_debug, what) in cases {
            let prefs: TrayPrefs = serde_json::from_str(stored).expect(what);
            assert_eq!(prefs.debug_stats, want_debug, "{what}");
        }

        let text = serde_json::to_string(&TrayPrefs { debug_stats: true }).unwrap();
        let loaded: TrayPrefs = serde_json::from_str(&text).unwrap();
        assert!(loaded.debug_stats, "written out and read back");

        // A damaged file is rejected rather than half-read, so the caller can
        // fall back to the defaults.
        assert!(serde_json::from_str::<TrayPrefs>("not json at all").is_err());
    }

    // ---- deleting recordings ----------------------------------------------

    #[test]
    fn deleting_recordings_removes_only_recordings() {
        let dir = std::env::temp_dir().join(format!("omegawhisper-delete-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // (file name, should it survive, why)
        let files: &[(&str, bool, &str)] = &[
            ("2026-01-01.wav", false, "a recording"),
            ("2026-01-01-model-input.wav", false, "what the model heard"),
            ("notes.txt", true, "someone else's file"),
            ("tray-prefs.json", true, "a settings file"),
            ("recording.wav.bak", true, "a backup, not a recording"),
            ("WAV", true, "no extension at all"),
        ];
        for (name, _, _) in files {
            fs::write(dir.join(name), b"x").unwrap();
        }
        // A folder must survive too - only files are considered.
        fs::create_dir(dir.join("old")).unwrap();
        fs::write(dir.join("old").join("kept.wav"), b"x").unwrap();

        let deleted = delete_recordings_in(&dir).unwrap();
        assert_eq!(deleted, 2, "only the two recordings should go");

        for (name, survives, why) in files {
            assert_eq!(dir.join(name).exists(), *survives, "{name}: {why}");
        }
        assert!(
            dir.join("old").join("kept.wav").exists(),
            "subfolders untouched"
        );
        assert!(dir.exists(), "the folder itself must stay");

        // Running it again on an empty folder is not an error.
        assert_eq!(delete_recordings_in(&dir).unwrap(), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_from_a_missing_folder_is_an_error_not_a_panic() {
        let missing = std::env::temp_dir().join("omegawhisper-does-not-exist-at-all");
        let _ = fs::remove_dir_all(&missing);
        assert!(delete_recordings_in(&missing).is_err());
    }

    #[test]
    fn server_addresses_pick_the_right_protocol() {
        // (address, secure, expected, why)
        let cases: &[(&str, bool, &str, &str)] = &[
            (
                "hyperwhisper.dev",
                true,
                "https://hyperwhisper.dev",
                "the normal case",
            ),
            (
                "localhost:8080",
                false,
                "http://localhost:8080",
                "a local test server",
            ),
        ];
        for &(address, secure, want, what) in cases {
            assert_eq!(get_hyperwhisper_api_base(address, secure), want, "{what}");
        }
    }
}
