use chrono::Utc;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SupportedStreamConfig};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use url::Url;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};
use zbus::interface;

// STT service types
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SttService {
    Deepgram,
    Whisper,
}

// Application state for audio recording
pub struct AudioState {
    is_recording: Arc<Mutex<bool>>,
    recorded_samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: Arc<Mutex<Option<u32>>>,
    stop_signal: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>,
    api_key: Arc<Mutex<Option<String>>>,
    // LLM settings
    openrouter_api_key: Arc<Mutex<Option<String>>>,
    llm_prompt: Arc<Mutex<String>>,
    // STT service selection
    stt_service: Arc<Mutex<SttService>>,
    // Real-time typing: type transcription as it streams in
    auto_type_transcription: Arc<Mutex<bool>>,
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

// Get the models directory, creating it if necessary
fn get_models_dir() -> Result<PathBuf, String> {
    let data_dir = dirs::data_local_dir()
        .ok_or_else(|| "Could not find local data directory".to_string())?;
    let models_dir = data_dir.join("hyperwhisper").join("models");

    if !models_dir.exists() {
        fs::create_dir_all(&models_dir)
            .map_err(|e| format!("Failed to create models directory: {}", e))?;
    }

    Ok(models_dir)
}

// Get the path to the whisper base model
fn get_whisper_model_path() -> Result<PathBuf, String> {
    let models_dir = get_models_dir()?;
    Ok(models_dir.join("ggml-base.bin"))
}

const WHISPER_MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin";

// Download progress event payload
#[derive(Clone, serde::Serialize)]
struct DownloadProgressEvent {
    downloaded: u64,
    total: u64,
    percent: f64,
}

#[tauri::command]
fn check_whisper_model() -> Result<bool, String> {
    let model_path = get_whisper_model_path()?;
    Ok(model_path.exists())
}

#[tauri::command]
async fn download_whisper_model(app_handle: AppHandle) -> Result<(), String> {
    let model_path = get_whisper_model_path()?;

    // Check if model already exists
    if model_path.exists() {
        return Ok(());
    }

    // Download in a blocking thread
    let app_handle_clone = app_handle.clone();
    std::thread::spawn(move || {
        let result = download_model_internal(&model_path, &app_handle_clone);

        match result {
            Ok(()) => {
                let _ = app_handle_clone.emit("download-complete", true);
            }
            Err(e) => {
                let _ = app_handle_clone.emit("download-error", e);
            }
        }
    });

    Ok(())
}

fn download_model_internal(model_path: &PathBuf, app_handle: &AppHandle) -> Result<(), String> {
    // Create a temporary file path
    let temp_path = model_path.with_extension("bin.tmp");

    // Make the request
    let response = ureq::get(WHISPER_MODEL_URL)
        .call()
        .map_err(|e| format!("Failed to download model: {}", e))?;

    // Get content length for progress
    let total_size: u64 = response
        .header("Content-Length")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // Create the file
    let mut file = fs::File::create(&temp_path)
        .map_err(|e| format!("Failed to create model file: {}", e))?;

    // Read and write in chunks
    let mut reader = response.into_reader();
    let mut downloaded: u64 = 0;
    let mut buffer = [0u8; 8192];
    let mut last_progress_emit = std::time::Instant::now();

    loop {
        let bytes_read = reader
            .read(&mut buffer)
            .map_err(|e| format!("Failed to read download data: {}", e))?;

        if bytes_read == 0 {
            break;
        }

        file.write_all(&buffer[..bytes_read])
            .map_err(|e| format!("Failed to write model file: {}", e))?;

        downloaded += bytes_read as u64;

        // Emit progress every 100ms to avoid flooding
        if last_progress_emit.elapsed() >= Duration::from_millis(100) {
            let percent = if total_size > 0 {
                (downloaded as f64 / total_size as f64) * 100.0
            } else {
                0.0
            };

            let _ = app_handle.emit("download-progress", DownloadProgressEvent {
                downloaded,
                total: total_size,
                percent,
            });

            last_progress_emit = std::time::Instant::now();
        }
    }

    // Rename temp file to final path
    fs::rename(&temp_path, model_path)
        .map_err(|e| format!("Failed to finalize model file: {}", e))?;

    Ok(())
}

#[tauri::command]
fn set_stt_service(state: State<'_, AudioState>, service: SttService) {
    *state.stt_service.lock().unwrap() = service;
}

#[tauri::command]
fn get_stt_service(state: State<'_, AudioState>) -> SttService {
    *state.stt_service.lock().unwrap()
}

#[tauri::command]
fn set_auto_type_transcription(state: State<'_, AudioState>, enabled: bool) {
    *state.auto_type_transcription.lock().unwrap() = enabled;
}

// Helper function to get the default audio input device
fn get_input_device() -> Result<Device, String> {
    let host = cpal::default_host();

    // Log available input devices for debugging
    eprintln!("Available audio hosts: {:?}", cpal::available_hosts());
    eprintln!("Using host: {:?}", host.id());

    // On Linux with PipeWire, prefer the "pipewire" device over "default"
    // The "default" ALSA device can crash GNOME when Bluetooth audio is active
    if let Ok(devices) = host.input_devices() {
        let devices: Vec<_> = devices.collect();

        for device in &devices {
            if let Ok(name) = device.name() {
                eprintln!("  Available input device: {}", name);
            }
        }

        // Try to find "pipewire" device first - it handles Bluetooth better
        for device in devices {
            if let Ok(name) = device.name() {
                if name == "pipewire" {
                    eprintln!("Selected input device: {} (preferred for Bluetooth compatibility)", name);
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

// Resample audio to 16kHz (required by Whisper)
fn resample_to_16khz(samples: &[f32], source_rate: u32) -> Vec<f32> {
    if source_rate == 16000 {
        return samples.to_vec();
    }

    let ratio = source_rate as f64 / 16000.0;
    let new_len = (samples.len() as f64 / ratio).ceil() as usize;
    let mut resampled = Vec::with_capacity(new_len);

    for i in 0..new_len {
        let src_idx = (i as f64 * ratio) as usize;
        if src_idx < samples.len() {
            resampled.push(samples[src_idx]);
        }
    }

    resampled
}

// Transcribe audio using local Whisper model
fn transcribe_with_whisper(samples: &[f32], sample_rate: u32) -> Result<String, String> {
    let model_path = get_whisper_model_path()?;

    if !model_path.exists() {
        return Err("Whisper model not found. Please download it first.".to_string());
    }

    // Resample to 16kHz
    let resampled = resample_to_16khz(samples, sample_rate);

    // Create Whisper context
    let ctx = WhisperContext::new_with_params(
        model_path.to_str().ok_or("Invalid model path")?,
        WhisperContextParameters::default(),
    )
    .map_err(|e| format!("Failed to load Whisper model: {}", e))?;

    // Create a state for inference
    let mut state = ctx
        .create_state()
        .map_err(|e| format!("Failed to create Whisper state: {}", e))?;

    // Set up parameters for transcription
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("en"));
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);

    // Run inference
    state
        .full(params, &resampled)
        .map_err(|e| format!("Whisper inference failed: {}", e))?;

    // Collect all segments
    let num_segments = state.full_n_segments().map_err(|e| format!("Failed to get segments: {}", e))?;
    let mut transcription = String::new();

    for i in 0..num_segments {
        if let Ok(segment_text) = state.full_get_segment_text(i) {
            if !transcription.is_empty() {
                transcription.push(' ');
            }
            transcription.push_str(&segment_text);
        }
    }

    Ok(transcription.trim().to_string())
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

#[tauri::command]
fn set_openrouter_api_key(state: State<'_, AudioState>, api_key: String) {
    *state.openrouter_api_key.lock().unwrap() = Some(api_key);
}

#[tauri::command]
fn set_llm_prompt(state: State<'_, AudioState>, prompt: String) {
    *state.llm_prompt.lock().unwrap() = prompt;
}

// LLM response event payload
#[derive(Clone, serde::Serialize)]
struct LlmResponseEvent {
    text: String,
    is_error: bool,
}

#[tauri::command]
async fn process_with_llm(
    state: State<'_, AudioState>,
    app_handle: AppHandle,
    transcription: String,
    should_type: bool,
) -> Result<(), String> {
    let api_key = state.openrouter_api_key.lock().unwrap().clone()
        .ok_or_else(|| "OpenRouter API key not set".to_string())?;

    let prompt = state.llm_prompt.lock().unwrap().clone();

    if transcription.trim().is_empty() {
        return Ok(());
    }

    // Emit processing started event
    let _ = app_handle.emit("llm-processing", true);

    // Spawn blocking thread for HTTP request
    let app_handle_clone = app_handle.clone();
    std::thread::spawn(move || {
        let result = call_openrouter(&api_key, &prompt, &transcription);

        match result {
            Ok(response_text) => {
                // Emit response
                let _ = app_handle_clone.emit("llm-response", LlmResponseEvent {
                    text: response_text.clone(),
                    is_error: false,
                });

                // Type if requested
                if should_type && !response_text.is_empty() {
                    let _ = type_text_internal(&response_text);
                }
            }
            Err(e) => {
                let _ = app_handle_clone.emit("llm-response", LlmResponseEvent {
                    text: format!("Error: {}", e),
                    is_error: true,
                });
            }
        }

        // Emit processing done
        let _ = app_handle_clone.emit("llm-processing", false);
    });

    Ok(())
}

fn call_openrouter(api_key: &str, prompt: &str, transcription: &str) -> Result<String, String> {
    let request_body = serde_json::json!({
        "model": "openai/gpt-4.1-nano",
        "messages": [
            { "role": "system", "content": prompt },
            { "role": "user", "content": transcription }
        ]
    });

    let response = ureq::post("https://openrouter.ai/api/v1/chat/completions")
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {}", api_key))
        .send_json(&request_body)
        .map_err(|e| format!("Request failed: {}", e))?;

    let json: serde_json::Value = response.into_json()
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    let text = json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    Ok(text)
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

    // Get STT service
    let stt_service = *state.stt_service.lock().unwrap();

    // Get API key (only required for Deepgram)
    let api_key = if stt_service == SttService::Deepgram {
        Some(state.api_key.lock().unwrap().clone()
            .ok_or_else(|| "Deepgram API key not set".to_string())?)
    } else {
        None
    };

    // Clear previous recording
    *state.recorded_samples.lock().unwrap() = Vec::new();

    // Get audio device info in a blocking thread to avoid interfering with GTK main loop
    // This is critical for Bluetooth devices on PipeWire which can crash GNOME
    let (device, config) = tokio::task::spawn_blocking(|| {
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

    // Channel for sending audio chunks to WebSocket thread (only used for Deepgram)
    let (audio_tx, audio_rx) = std::sync::mpsc::channel::<Vec<f32>>();

    // Spawn WebSocket thread for Deepgram (only if using Deepgram)
    if stt_service == SttService::Deepgram {
        let api_key = api_key.unwrap();
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

            loop {
                // Check if we should stop
                if !*is_recording_ws.lock().unwrap() {
                    let _ = ws.send(Message::Text("{\"type\":\"CloseStream\"}".to_string()));
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
                                    let _ = app_handle_ws.emit("transcription", event);
                                }
                            }
                        }
                    }
                    Ok(Message::Close(_)) => {
                        break;
                    }
                    Err(tungstenite::Error::Io(ref e))
                        if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {
                        // Timeout - continue loop
                    }
                    Err(e) => {
                        eprintln!("WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        });
    }

    // Flag to determine if we should send audio to the WebSocket
    let use_deepgram = stt_service == SttService::Deepgram;

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
                            // Send to WebSocket thread (only for Deepgram)
                            if use_deepgram {
                                let _ = audio_tx.send(buffer);
                            }
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
                            if use_deepgram {
                                let _ = audio_tx.send(buffer);
                            }
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
                            if use_deepgram {
                                let _ = audio_tx.send(buffer);
                            }
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
async fn stop_recording(state: State<'_, AudioState>, app_handle: AppHandle) -> Result<String, String> {
    {
        let is_recording = state.is_recording.lock().unwrap();
        if !*is_recording {
            return Err("Not recording".to_string());
        }
    }

    // Get STT service before stopping
    let stt_service = *state.stt_service.lock().unwrap();

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

    // If using Whisper, run transcription now
    if stt_service == SttService::Whisper {
        let samples_clone = samples.clone();
        let app_handle_clone = app_handle.clone();

        // Run transcription in a thread to avoid blocking
        thread::spawn(move || {
            // Emit a processing event
            let _ = app_handle_clone.emit("whisper-processing", true);

            match transcribe_with_whisper(&samples_clone, sample_rate) {
                Ok(text) => {
                    if !text.is_empty() {
                        let event = TranscriptionEvent {
                            text,
                            is_final: true,
                        };
                        let _ = app_handle_clone.emit("transcription", event);
                    }
                }
                Err(e) => {
                    let _ = app_handle_clone.emit("transcription-error", e);
                }
            }

            let _ = app_handle_clone.emit("whisper-processing", false);
        });
    }

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
fn get_system_theme() -> String {
    if let Ok(output) = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
    {
        let result = String::from_utf8_lossy(&output.stdout);
        if result.contains("dark") {
            return "dark".to_string();
        }
    }
    "light".to_string()
}

#[tauri::command]
fn type_text(text: String) -> Result<(), String> {
    type_text_internal(&text)
}

#[tauri::command]
fn get_focused_window() -> Option<String> {
    // Query window-calls GNOME extension via gdbus
    let output = std::process::Command::new("gdbus")
        .args([
            "call", "--session",
            "--dest", "org.gnome.Shell",
            "--object-path", "/org/gnome/Shell/Extensions/Windows",
            "--method", "org.gnome.Shell.Extensions.Windows.List",
        ])
        .output()
        .ok()?;

    let result = String::from_utf8_lossy(&output.stdout);

    // The output format is: ([{'id': ..., 'wm_class': '...', 'focus': true, ...}, ...],)
    // We need to extract the JSON array from it
    let json_start = result.find('[')?;
    let json_end = result.rfind(']')? + 1;
    let json_str = &result[json_start..json_end];

    // Parse the JSON array
    let windows: Vec<serde_json::Value> = serde_json::from_str(json_str).ok()?;

    // Find the focused window
    for window in windows {
        if window.get("focus").and_then(|f| f.as_bool()) == Some(true) {
            // Return wm_class (app name) or title as fallback
            if let Some(wm_class) = window.get("wm_class").and_then(|c| c.as_str()) {
                if !wm_class.is_empty() {
                    return Some(wm_class.to_string());
                }
            }
            if let Some(title) = window.get("title").and_then(|t| t.as_str()) {
                return Some(title.to_string());
            }
        }
    }

    None
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let audio_state = AudioState {
        is_recording: Arc::new(Mutex::new(false)),
        recorded_samples: Arc::new(Mutex::new(Vec::new())),
        sample_rate: Arc::new(Mutex::new(None)),
        stop_signal: Arc::new(Mutex::new(None)),
        api_key: Arc::new(Mutex::new(None)),
        openrouter_api_key: Arc::new(Mutex::new(None)),
        llm_prompt: Arc::new(Mutex::new(String::new())),
        stt_service: Arc::new(Mutex::new(SttService::Deepgram)),
        auto_type_transcription: Arc::new(Mutex::new(false)),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(audio_state)
        .invoke_handler(tauri::generate_handler![
            start_recording,
            stop_recording,
            is_recording,
            get_system_theme,
            get_focused_window,
            set_api_key,
            set_openrouter_api_key,
            set_llm_prompt,
            process_with_llm,
            type_text,
            check_whisper_model,
            download_whisper_model,
            set_stt_service,
            get_stt_service,
            set_auto_type_transcription,
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
