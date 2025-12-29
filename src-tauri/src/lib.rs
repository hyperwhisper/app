use chrono::Local;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SupportedStreamConfig};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::State;
use tokio::sync::mpsc;
use tokio::sync::RwLock;

// Application state for audio recording
pub struct AudioState {
    is_recording: Arc<Mutex<bool>>,
    recorded_data: Arc<RwLock<Vec<Vec<u8>>>>,
    sample_rate: Arc<Mutex<Option<u32>>>,
    stop_signal: Arc<Mutex<Option<mpsc::Sender<()>>>>,
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

    // WAV header
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
    wav_data.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    wav_data.extend_from_slice(&1u16.to_le_bytes()); // audio format (PCM)
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
        let i16_sample = (sample * i16::MAX as f32) as i16;
        wav_data.extend_from_slice(&i16_sample.to_le_bytes());
    }

    wav_data
}

#[tauri::command]
async fn start_recording(state: State<'_, AudioState>) -> Result<(), String> {
    // Check recording state and release lock before await
    {
        let is_recording = state.is_recording.lock().unwrap();
        if *is_recording {
            return Err("Already recording".to_string());
        }
    }

    // Clear previous recording
    *state.recorded_data.write().await = Vec::new();

    let device = get_input_device()?;
    let config = device
        .default_input_config()
        .map_err(|e| format!("Failed to get default input config: {}", e))?;

    let sample_format = config.sample_format();
    let sample_rate = config.sample_rate().0;
    let channels = config.channels();
    let config: SupportedStreamConfig = config.into();

    // Store sample rate for WAV conversion
    *state.sample_rate.lock().unwrap() = Some(sample_rate);

    let is_recording_arc = state.is_recording.clone();
    let recorded_data_arc = state.recorded_data.clone();

    // Set recording flag
    *state.is_recording.lock().unwrap() = true;

    // Create channel for stop signal
    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    *state.stop_signal.lock().unwrap() = Some(stop_tx);

    // Move to thread and keep stream alive there
    thread::spawn(move || {
        let stream_result = match sample_format {
            SampleFormat::F32 => {
                let is_recording = is_recording_arc.clone();
                let recorded_data = recorded_data_arc.clone();
                let is_recording_err = is_recording_arc.clone();
                device.build_input_stream(
                    &config.clone().into(),
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
                            let mut data_guard = recorded_data.blocking_write();
                            // Convert f32 buffer to bytes
                            let bytes: Vec<u8> = buffer
                                .iter()
                                .flat_map(|&s| s.to_le_bytes().to_vec())
                                .collect();
                            data_guard.push(bytes);
                        }
                    },
                    move |err| {
                        eprintln!("Error in audio stream: {}", err);
                        *is_recording_err.lock().unwrap() = false;
                    },
                    None,
                )
            }
            SampleFormat::I16 => {
                let is_recording = is_recording_arc.clone();
                let recorded_data = recorded_data_arc.clone();
                let is_recording_err = is_recording_arc.clone();
                device.build_input_stream(
                    &config.clone().into(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if *is_recording.lock().unwrap() {
                            let mut buffer: Vec<f32> = data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                            // Convert to mono if stereo
                            if channels > 1 {
                                let mut mono_data = Vec::new();
                                for chunk in buffer.chunks(channels as usize) {
                                    let avg: f32 = chunk.iter().sum::<f32>() / chunk.len() as f32;
                                    mono_data.push(avg);
                                }
                                buffer = mono_data;
                            }
                            let mut data_guard = recorded_data.blocking_write();
                            // Convert f32 buffer to bytes
                            let bytes: Vec<u8> = buffer
                                .iter()
                                .flat_map(|&s| s.to_le_bytes().to_vec())
                                .collect();
                            data_guard.push(bytes);
                        }
                    },
                    move |err| {
                        eprintln!("Error in audio stream: {}", err);
                        *is_recording_err.lock().unwrap() = false;
                    },
                    None,
                )
            }
            SampleFormat::U16 => {
                let is_recording = is_recording_arc.clone();
                let recorded_data = recorded_data_arc.clone();
                let is_recording_err = is_recording_arc.clone();
                device.build_input_stream(
                    &config.clone().into(),
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        if *is_recording.lock().unwrap() {
                            let mut buffer: Vec<f32> = data.iter().map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0).collect();
                            // Convert to mono if stereo
                            if channels > 1 {
                                let mut mono_data = Vec::new();
                                for chunk in buffer.chunks(channels as usize) {
                                    let avg: f32 = chunk.iter().sum::<f32>() / chunk.len() as f32;
                                    mono_data.push(avg);
                                }
                                buffer = mono_data;
                            }
                            let mut data_guard = recorded_data.blocking_write();
                            // Convert f32 buffer to bytes
                            let bytes: Vec<u8> = buffer
                                .iter()
                                .flat_map(|&s| s.to_le_bytes().to_vec())
                                .collect();
                            data_guard.push(bytes);
                        }
                    },
                    move |err| {
                        eprintln!("Error in audio stream: {}", err);
                        *is_recording_err.lock().unwrap() = false;
                    },
                    None,
                )
            }
            _ => {
                *is_recording_arc.lock().unwrap() = false;
                return Err(format!("Unsupported sample format: {:?}", sample_format));
            }
        };

        let stream = stream_result.map_err(|e| {
            *is_recording_arc.lock().unwrap() = false;
            format!("Failed to build stream: {}", e)
        })?;

        stream.play().map_err(|e| {
            *is_recording_arc.lock().unwrap() = false;
            format!("Failed to play stream: {}", e)
        })?;

        // Keep the stream alive until stop signal
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let _ = stop_rx.recv().await;
        });

        Ok::<(), String>(())
    });

    Ok(())
}

#[tauri::command]
async fn stop_recording(state: State<'_, AudioState>) -> Result<String, String> {
    // Check and release lock immediately
    {
        let is_recording = state.is_recording.lock().unwrap();
        if !*is_recording {
            return Err("Not recording".to_string());
        }
    }

    // Stop recording
    *state.is_recording.lock().unwrap() = false;

    // Send stop signal to the thread (extract sender before await to avoid holding MutexGuard across await)
    let stop_tx = state.stop_signal.lock().unwrap().take();
    if let Some(stop_tx) = stop_tx {
        let _ = stop_tx.send(()).await;
    }

    // Give a small delay for the last buffer to be captured
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Collect all recorded data
    let data_chunks = state.recorded_data.read().await;
    if data_chunks.is_empty() {
        return Err("No audio data recorded".to_string());
    }

    // Combine all chunks into a single f32 vector
    let mut all_samples: Vec<f32> = Vec::new();
    for chunk in data_chunks.iter() {
        for chunk_bytes in chunk.chunks(4) {
            if chunk_bytes.len() == 4 {
                let sample = f32::from_le_bytes([chunk_bytes[0], chunk_bytes[1], chunk_bytes[2], chunk_bytes[3]]);
                all_samples.push(sample);
            }
        }
    }

    // Convert to WAV using the recorded sample rate
    let sample_rate = state.sample_rate.lock().unwrap().unwrap_or(48000);
    let wav_bytes = to_wav_bytes(&all_samples, sample_rate, 1);

    // Save to disk
    let recordings_dir = get_recordings_dir()?;
    let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let file_name = format!("{}.wav", timestamp);
    let file_path = recordings_dir.join(&file_name);

    fs::write(&file_path, &wav_bytes)
        .map_err(|e| format!("Failed to save recording: {}", e))?;

    // Encode as base64 for easy transmission
    use base64::Engine;
    let base64_audio = base64::engine::general_purpose::STANDARD.encode(&wav_bytes);

    // Create data URL
    let data_url = format!("data:audio/wav;base64,{}", base64_audio);

    // Clear the recorded data
    drop(data_chunks);
    *state.recorded_data.write().await = Vec::new();

    // Return JSON with both data URL and file path
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
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let audio_state = AudioState {
        is_recording: Arc::new(Mutex::new(false)),
        recorded_data: Arc::new(RwLock::new(Vec::new())),
        sample_rate: Arc::new(Mutex::new(None)),
        stop_signal: Arc::new(Mutex::new(None)),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(audio_state)
        .invoke_handler(tauri::generate_handler![
            greet,
            start_recording,
            stop_recording,
            is_recording,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
