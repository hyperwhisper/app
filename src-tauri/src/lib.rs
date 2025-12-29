use chrono::Utc;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SupportedStreamConfig};
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

// Application state for audio recording
pub struct AudioState {
    is_recording: Arc<Mutex<bool>>,
    recorded_samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: Arc<Mutex<Option<u32>>>,
    stop_signal: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>,
    api_key: Arc<Mutex<Option<String>>>,
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

// Helper function to get the default audio input device
fn get_input_device() -> Result<Device, String> {
    let host = cpal::default_host();
    host.default_input_device()
        .ok_or_else(|| "No audio input device found".to_string())
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
        .ok_or_else(|| "API key not set".to_string())?;

    // Clear previous recording
    *state.recorded_samples.lock().unwrap() = Vec::new();

    let device = get_input_device()?;
    let config = device
        .default_input_config()
        .map_err(|e| format!("Failed to get default input config: {}", e))?;

    let sample_format = config.sample_format();
    let sample_rate = config.sample_rate().0;
    let channels = config.channels();
    let stream_config: SupportedStreamConfig = config.into();

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

    // Spawn audio recording thread
    thread::spawn(move || {
        let stream_result = match sample_format {
            SampleFormat::F32 => {
                let is_recording = is_recording_arc.clone();
                let recorded_samples = recorded_samples_arc.clone();
                let audio_tx = audio_tx.clone();
                device.build_input_stream(
                    &stream_config.clone().into(),
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
                    &stream_config.clone().into(),
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
                    &stream_config.clone().into(),
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
async fn stop_recording(state: State<'_, AudioState>) -> Result<String, String> {
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let audio_state = AudioState {
        is_recording: Arc::new(Mutex::new(false)),
        recorded_samples: Arc::new(Mutex::new(Vec::new())),
        sample_rate: Arc::new(Mutex::new(None)),
        stop_signal: Arc::new(Mutex::new(None)),
        api_key: Arc::new(Mutex::new(None)),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(audio_state)
        .invoke_handler(tauri::generate_handler![
            start_recording,
            stop_recording,
            is_recording,
            get_system_theme,
            set_api_key,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
