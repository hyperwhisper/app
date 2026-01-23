mod audio;
mod managers;
mod resampler;

use audio::VadProcessor;
use chrono::Utc;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SupportedStreamConfig};
use managers::{
    ModelManager, ModelStatus, SharedTranscriptionManager, AVAILABLE_MODELS,
};
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
}

// D-Bus service for external control (Linux only)
#[cfg(target_os = "linux")]
struct HyperWhisperDBus {
    app_handle: AppHandle,
}

#[cfg(target_os = "linux")]
#[interface(name = "dev.hyperwhisper")]
impl HyperWhisperDBus {
    async fn toggle_recording(&self) -> bool {
        // Emit event to frontend to toggle recording
        let _ = self.app_handle.emit("recording-toggled", ());
        true
    }
}

// Transcription event payload
#[derive(Clone, serde::Serialize)]
struct TranscriptionEvent {
    text: String,
    is_final: bool,
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
        if let Ok(hostname) = std::env::var("HOSTNAME").or_else(|_| {
            fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string())
        }) {
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
fn provision_trial_key_internal(server_url: &str, use_https: bool) -> Result<TrialProvisionResponse, String> {
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
    let data_dir = dirs::data_local_dir()
        .ok_or_else(|| "Could not find local data directory".to_string())?;
    let recordings_dir = data_dir.join("hyperwhisper").join("recordings");

    if !recordings_dir.exists() {
        fs::create_dir_all(&recordings_dir)
            .map_err(|e| format!("Failed to create recordings directory: {}", e))?;
    }

    Ok(recordings_dir)
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
    let mut bluetooth_device_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();

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
            let trimmed = line.trim_start_matches(|c| c == ' ' || c == '│' || c == '├' || c == '─' || c == '*');
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
        if (in_sources_section || in_filters_section) && (line.contains("├─") || line.contains("└─")) {
            in_sources_section = false;
            in_filters_section = false;
            continue;
        }

        if in_sources_section {
            let trimmed = line.trim_start_matches(|c| c == ' ' || c == '│' || c == '├' || c == '─');

            if trimmed.is_empty() {
                continue;
            }

            let trimmed = trimmed.trim_start_matches(|c| c == '*' || c == ' ');

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
                        devices.push(WpDevice { id, name: friendly_name, is_default: is_builtin });
                    }
                }
            }
        }

        if in_filters_section {
            let trimmed = line.trim_start_matches(|c| c == ' ' || c == '│' || c == '├' || c == '─' || c == '-');

            if trimmed.is_empty() {
                continue;
            }

            // Look for Bluetooth audio sources: "146. bluez_input.XX:XX:XX [Audio/Source]"
            if trimmed.contains("[Audio/Source]") && trimmed.contains("bluez_input") {
                let is_default = trimmed.starts_with('*');
                let trimmed = trimmed.trim_start_matches(|c| c == '*' || c == ' ');

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

                        devices.push(WpDevice { id, name: friendly_name, is_default });
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
            let trimmed = line.trim_start_matches(|c| c == ' ' || c == '│' || c == '├' || c == '─' || c == '*');
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
    tokio::task::spawn_blocking(move || {
        model_manager.download_model(&model_id, &app_handle)
    })
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

// Legacy check for backward compatibility with old settings page
#[tauri::command]
fn check_local_model_status(state: State<'_, AudioState>) -> Result<serde_json::Value, String> {
    // Check if any model is downloaded
    let any_downloaded = AVAILABLE_MODELS
        .iter()
        .any(|m| state.model_manager.is_model_downloaded(m.id));

    let active_model = state.active_local_model_id.lock().unwrap().clone();
    let path = active_model
        .as_ref()
        .map(|id| state.model_manager.get_model_path(id).to_string_lossy().to_string());

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
        Ok(model_manager.get_model_path(model_id).to_string_lossy().to_string())
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
    let device = host.default_input_device()
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
                if config.min_sample_rate().0 <= rate && config.max_sample_rate().0 >= rate {
                    if config.sample_format() == SampleFormat::F32 {
                        return Ok(config.clone().with_sample_rate(cpal::SampleRate(rate)));
                    }
                }
            }
            // If F32 not available at this rate, try I16
            for config in &configs {
                if config.min_sample_rate().0 <= rate && config.max_sample_rate().0 >= rate {
                    if config.sample_format() == SampleFormat::I16 {
                        return Ok(config.clone().with_sample_rate(cpal::SampleRate(rate)));
                    }
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

    fn close(&mut self, _: Option<tungstenite::protocol::CloseFrame>) -> Result<(), tungstenite::Error> {
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
        .header("Sec-WebSocket-Key", tungstenite::handshake::client::generate_key())
        .body(())
        .map_err(|e| format!("Failed to build request: {}", e))?;

    let (ws, _response) = tungstenite::client::client(request, MaybeTlsStream::NativeTls(tls_stream))
        .map_err(|e| format!("WebSocket handshake failed: {}", e))?;

    Ok(WsStream::Tls(ws))
}

// Connect to Hyperwhisper server WebSocket
fn connect_to_hyperwhisper_server(api_key: &str, sample_rate: u32, server_url: &str, use_https: bool) -> Result<WsStream, String> {
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
            .header("Sec-WebSocket-Key", tungstenite::handshake::client::generate_key())
            .body(())
            .map_err(|e| format!("Failed to build request: {}", e))?;

        let (ws, _response) = tungstenite::client::client(request, MaybeTlsStream::NativeTls(tls_stream))
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
            .header("Sec-WebSocket-Key", tungstenite::handshake::client::generate_key())
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
    if use_hyperwhisper_server && api_key.as_ref().map_or(true, |k| k.trim().is_empty()) {
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

fn type_text_internal(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        // Use osascript with AppleScript on macOS
        // Escape backslashes and double quotes for AppleScript string
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!("tell application \"System Events\" to keystroke \"{}\"", escaped);

        let osascript_result = std::process::Command::new("osascript")
            .args(["-e", &script])
            .status();

        if let Ok(status) = osascript_result {
            if status.success() {
                return Ok(());
            }
        }

        return Err("Failed to type text: osascript failed. Ensure accessibility permissions are granted.".to_string());
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
        let wtype_result = std::process::Command::new("wtype")
            .arg(text)
            .status();

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
        state.hyperwhisper_api_key.lock().unwrap().clone()
            .ok_or_else(|| "Hyperwhisper API key not set".to_string())?
    } else {
        state.api_key.lock().unwrap().clone()
            .ok_or_else(|| "Deepgram API key not set".to_string())?
    };

    // Validate local model if using local transcription
    let active_model_id = if use_local {
        let model_id = state
            .active_local_model_id
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "No local model selected. Please select a model in settings.".to_string())?;

        if !state.model_manager.is_model_downloaded(&model_id) {
            return Err(format!("Model {} is not downloaded. Please download it first.", model_id));
        }

        Some(model_id)
    } else {
        None
    };

    // Get VAD setting
    let use_vad = *state.use_vad.lock().unwrap();

    // Clone transcription manager for the thread
    let transcription_manager = state.transcription_manager.0.clone();

    // Clear previous recording
    *state.recorded_samples.lock().unwrap() = Vec::new();

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
            let stop_recording_on_error = |is_recording: &Arc<Mutex<bool>>, stop_signal: &Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>| {
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
                        let _ = app_handle_ws.emit("transcription-error", format!("Failed to access transcription engine: {}", e));
                        stop_recording_on_error(&is_recording_ws, &stop_signal_ws);
                        return;
                    }
                };

                let currently_loaded = manager.get_loaded_model_id().map(|s| s.to_string());
                if currently_loaded.as_deref() != Some(&model_id) {
                    if let Err(e) = manager.load_model(&model_id) {
                        let _ = app_handle_ws.emit("transcription-error", format!("Failed to load model: {}", e));
                        stop_recording_on_error(&is_recording_ws, &stop_signal_ws);
                        return;
                    }
                }
            }

            // Create resampler to convert device sample rate to 16kHz
            let mut resampler = match AudioResampler::new(sample_rate) {
                Ok(r) => r,
                Err(e) => {
                    let _ = app_handle_ws.emit("transcription-error", format!("Failed to create resampler: {}", e));
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

            // Emit processing state
            let _ = app_handle_ws.emit("transcription-processing", ());

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
                let result = {
                    let mut manager = transcription_manager.lock().unwrap();
                    manager.transcribe(&all_audio)
                };

                match result {
                    Ok(text) => {
                        let text = text.trim().to_string();
                        if !text.is_empty() {
                            if auto_type {
                                let _ = type_text_internal(&format!("{} ", text));
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

            // Notify frontend that transcription processing is complete
            let _ = app_handle_ws.emit("transcription-complete", ());
        });
    } else {
        // Spawn WebSocket thread for Deepgram or Hyperwhisper server
        thread::spawn(move || {
            // Helper to stop recording on error
            let stop_recording_on_error = |is_recording: &Arc<Mutex<bool>>, stop_signal: &Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>| {
                *is_recording.lock().unwrap() = false;
                if let Some(stop_tx) = stop_signal.lock().unwrap().take() {
                    let _ = stop_tx.send(());
                }
            };

            // Connect to Hyperwhisper server or Deepgram
            let mut ws = if use_hyperwhisper {
                match connect_to_hyperwhisper_server(&api_key, sample_rate, &hyperwhisper_url, hyperwhisper_https) {
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
            let process_message = |ws: &mut WsStream, app_handle: &AppHandle, auto_type: bool| -> Option<bool> {
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
                                        let _ = type_text_internal(&text_to_type);
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
                        || e.kind() == std::io::ErrorKind::TimedOut => {
                        None // Timeout, no message
                    }
                    Err(_) => {
                        Some(false) // Error, stop
                    }
                    _ => Some(true)
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
                        Some(true) => {}, // Got a message, keep reading
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
            match process_message(&mut ws, &app_handle_ws, auto_type) {
                Some(false) => {
                    // Connection closed or error - still emit complete event
                    let _ = app_handle_ws.emit("transcription-complete", ());
                    break;
                }
                _ => {}
            }
        }
        });
    } // End of if use_local else block

    // Spawn audio recording thread
    thread::spawn(move || {
        let stream_result = match sample_format {
            SampleFormat::F32 => {
                let is_recording = is_recording_arc.clone();
                let recorded_samples = recorded_samples_arc.clone();
                let audio_tx = audio_tx.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if *is_recording.lock().unwrap() {
                            let mut buffer = data.to_vec();
                            // Convert to mono if stereo
                            if channels > 1 {
                                let mut mono_data = Vec::new();
                                for chunk in buffer.chunks(channels as usize) {
                                    let avg: f32 = chunk.iter().sum::<f32>() / chunk.len() as f32;
                                    mono_data.push(avg);
                                }
                                buffer = mono_data;
                            }
                            // Store for WAV file
                            recorded_samples.lock().unwrap().extend_from_slice(&buffer);
                            // Send to WebSocket thread
                            let _ = audio_tx.send(buffer);
                        }
                    },
                    move |err| {
                        eprintln!("Error in audio stream: {}", err);
                    },
                    None,
                )
            }
            SampleFormat::I16 => {
                let is_recording = is_recording_arc.clone();
                let recorded_samples = recorded_samples_arc.clone();
                let audio_tx = audio_tx.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if *is_recording.lock().unwrap() {
                            let mut buffer: Vec<f32> = data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                            if channels > 1 {
                                let mut mono_data = Vec::new();
                                for chunk in buffer.chunks(channels as usize) {
                                    let avg: f32 = chunk.iter().sum::<f32>() / chunk.len() as f32;
                                    mono_data.push(avg);
                                }
                                buffer = mono_data;
                            }
                            recorded_samples.lock().unwrap().extend_from_slice(&buffer);
                            let _ = audio_tx.send(buffer);
                        }
                    },
                    move |err| {
                        eprintln!("Error in audio stream: {}", err);
                    },
                    None,
                )
            }
            SampleFormat::U16 => {
                let is_recording = is_recording_arc.clone();
                let recorded_samples = recorded_samples_arc.clone();
                let audio_tx = audio_tx.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        if *is_recording.lock().unwrap() {
                            let mut buffer: Vec<f32> = data.iter().map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0).collect();
                            if channels > 1 {
                                let mut mono_data = Vec::new();
                                for chunk in buffer.chunks(channels as usize) {
                                    let avg: f32 = chunk.iter().sum::<f32>() / chunk.len() as f32;
                                    mono_data.push(avg);
                                }
                                buffer = mono_data;
                            }
                            recorded_samples.lock().unwrap().extend_from_slice(&buffer);
                            let _ = audio_tx.send(buffer);
                        }
                    },
                    move |err| {
                        eprintln!("Error in audio stream: {}", err);
                    },
                    None,
                )
            }
            _ => {
                return;
            }
        };

        let stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to build stream: {}", e);
                return;
            }
        };

        if let Err(e) = stream.play() {
            eprintln!("Failed to play stream: {}", e);
            return;
        }

        // Keep stream alive until stop signal
        let _ = stop_rx.recv();
    });

    Ok(())
}

#[tauri::command]
async fn stop_recording(state: State<'_, AudioState>, _app_handle: AppHandle) -> Result<String, String> {
    {
        let is_recording = state.is_recording.lock().unwrap();
        if !*is_recording {
            return Err("Not recording".to_string());
        }
    }

    // Stop recording
    *state.is_recording.lock().unwrap() = false;

    // Send stop signal
    let stop_tx = state.stop_signal.lock().unwrap().take();
    if let Some(stop_tx) = stop_tx {
        let _ = stop_tx.send(());
    }

    // Wait for buffers to flush
    std::thread::sleep(std::time::Duration::from_millis(300));

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

    fs::write(&file_path, &wav_bytes)
        .map_err(|e| format!("Failed to save recording: {}", e))?;

    // Encode as base64
    use base64::Engine;
    let base64_audio = base64::engine::general_purpose::STANDARD.encode(&wav_bytes);
    let data_url = format!("data:audio/wav;base64,{}", base64_audio);

    // Clear recorded samples
    *state.recorded_samples.lock().unwrap() = Vec::new();

    let response = serde_json::json!({
        "dataUrl": data_url,
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

/// Query dconf for the keybinding associated with hyperwhisper
/// Returns the keybinding string (e.g., "<Super>m") or None if not found
#[tauri::command]
fn get_keybinding() -> Option<String> {
    use std::process::Command;

    // Run dconf dump / to get all settings
    let output = Command::new("dconf")
        .args(["dump", "/"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let dump = String::from_utf8_lossy(&output.stdout);

    // Search patterns for hyperwhisper keybindings
    let search_patterns = [
        "hyperwhisper",
        "ToggleRecording",
        "dev.hyperwhisper",
    ];

    // Parse dconf dump format - it's INI-like with [path] sections
    // First, split into sections and find the one containing hyperwhisper
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

    // Find sections that contain hyperwhisper in any line
    for (section_name, lines) in &sections {
        let section_text = lines.join("\n");
        let has_pattern = search_patterns.iter().any(|p|
            section_name.to_lowercase().contains(&p.to_lowercase()) ||
            section_text.to_lowercase().contains(&p.to_lowercase())
        );

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize model manager
    let model_manager = Arc::new(
        ModelManager::new().expect("Failed to initialize model manager")
    );

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
        use_vad: Arc::new(Mutex::new(true)), // VAD enabled by default
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
        ])
        .setup(|app| {
            // Spawn D-Bus service for external control (Linux only)
            #[cfg(target_os = "linux")]
            {
                let handle = app.handle().clone();

                tauri::async_runtime::spawn(async move {
                    let service = HyperWhisperDBus { app_handle: handle };

                    match zbus::connection::Builder::session()
                        .and_then(|b| b.name("dev.hyperwhisper"))
                        .and_then(|b| b.serve_at("/dev/hyperwhisper", service))
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
