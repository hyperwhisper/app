use chrono::Utc;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SupportedStreamConfig};
use std::fs;
use std::io::Write;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use url::Url;
use zbus::interface;

// Application state for audio recording
pub struct AudioState {
    is_recording: Arc<Mutex<bool>>,
    recorded_samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: Arc<Mutex<Option<u32>>>,
    stop_signal: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>,
    api_key: Arc<Mutex<Option<String>>>,
    // Real-time typing: type transcription as it streams in
    auto_type_transcription: Arc<Mutex<bool>>,
    // Selected audio input device ID from WirePlumber (None = auto-select)
    selected_device_id: Arc<Mutex<Option<u32>>>,
}

// D-Bus service for external control
struct HyperWhisperDBus {
    app_handle: AppHandle,
}

#[interface(name = "com.cc.hyperwhisper")]
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

    for line in status.lines() {
        // Track when we enter/exit the Audio section
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

        // Look for the Sources section under Audio
        if line.contains("├─ Sources:") || line.contains("└─ Sources:") {
            in_sources_section = true;
            continue;
        }

        // Exit sources section when we hit another section (Filters, Streams, etc.)
        if in_sources_section && (line.contains("├─") || line.contains("└─")) {
            in_sources_section = false;
            continue;
        }

        if in_sources_section {
            // Parse lines like: " │      59. Meteor Lake-P HD Audio Controller Stereo Microphone [vol: 1.00]"
            // or with asterisk: " │  *   60. Device Name [vol: 0.79]"
            let trimmed = line.trim_start_matches(|c| c == ' ' || c == '│' || c == '├' || c == '─');

            if trimmed.is_empty() {
                continue;
            }

            let is_default = trimmed.starts_with('*');
            let trimmed = trimmed.trim_start_matches(|c| c == '*' || c == ' ');

            // Parse ID and name: "59. Device Name [vol: 1.00]"
            if let Some(dot_pos) = trimmed.find(". ") {
                if let Ok(id) = trimmed[..dot_pos].trim().parse::<u32>() {
                    let rest = &trimmed[dot_pos + 2..];
                    // Remove the [vol: x.xx] suffix
                    let name = if let Some(bracket_pos) = rest.rfind('[') {
                        rest[..bracket_pos].trim().to_string()
                    } else {
                        rest.trim().to_string()
                    };

                    if !name.is_empty() {
                        devices.push(WpDevice { id, name, is_default });
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

    // Set the default source in WirePlumber if a device is selected
    if let Some(id) = device_id {
        let _ = std::process::Command::new("wpctl")
            .args(["set-default", &id.to_string()])
            .status();
    }
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

// Connect to Deepgram WebSocket
fn connect_to_deepgram(api_key: &str, sample_rate: u32) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, String> {
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

    Ok(ws)
}

#[tauri::command]
fn set_api_key(state: State<'_, AudioState>, api_key: String) {
    *state.api_key.lock().unwrap() = Some(api_key);
}

fn type_text_internal(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }

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

    // Get API key
    let api_key = state.api_key.lock().unwrap().clone()
        .ok_or_else(|| "Deepgram API key not set".to_string())?;

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

    // Channel for sending audio chunks to WebSocket thread
    let (audio_tx, audio_rx) = std::sync::mpsc::channel::<Vec<f32>>();

    // Spawn WebSocket thread for Deepgram
    let app_handle_ws = app_handle.clone();
    let is_recording_ws = is_recording_arc.clone();
    let auto_type = *state.auto_type_transcription.lock().unwrap();
    thread::spawn(move || {
        // Connect to Deepgram
        let mut ws = match connect_to_deepgram(&api_key, sample_rate) {
            Ok(ws) => ws,
            Err(e) => {
                eprintln!("Failed to connect to Deepgram: {}", e);
                let _ = app_handle_ws.emit("transcription-error", e);
                return;
            }
        };

        // Set read timeout so we can check for stop signal and send audio
        if let MaybeTlsStream::NativeTls(ref tls) = ws.get_ref() {
            let _ = tls.get_ref().set_read_timeout(Some(Duration::from_millis(50)));
        }

        // Helper closure to process incoming Deepgram messages
        let process_message = |ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, app_handle: &AppHandle, auto_type: bool| -> Option<bool> {
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
                Some(false) => break,
                _ => {}
            }
        }
    });

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let audio_state = AudioState {
        is_recording: Arc::new(Mutex::new(false)),
        recorded_samples: Arc::new(Mutex::new(Vec::new())),
        sample_rate: Arc::new(Mutex::new(None)),
        stop_signal: Arc::new(Mutex::new(None)),
        api_key: Arc::new(Mutex::new(None)),
        auto_type_transcription: Arc::new(Mutex::new(false)),
        selected_device_id: Arc::new(Mutex::new(None)),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(audio_state)
        .invoke_handler(tauri::generate_handler![
            start_recording,
            stop_recording,
            is_recording,
            set_api_key,
            type_text,
            set_auto_type_transcription,
            list_audio_devices,
            get_selected_device,
            set_selected_device,
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // Spawn D-Bus service for external control
            tauri::async_runtime::spawn(async move {
                let service = HyperWhisperDBus { app_handle: handle };

                match zbus::connection::Builder::session()
                    .and_then(|b| b.name("com.cc.hyperwhisper"))
                    .and_then(|b| b.serve_at("/com/cc/hyperwhisper", service))
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

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
