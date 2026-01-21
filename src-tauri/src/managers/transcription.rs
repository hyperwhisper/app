use crate::managers::model::{EngineType, ModelManager, AVAILABLE_MODELS};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use transcribe_rs::engines::moonshine::MoonshineEngine;
use transcribe_rs::engines::parakeet::{ParakeetEngine, ParakeetModelParams, QuantizationType};
use transcribe_rs::engines::whisper::WhisperEngine;
use transcribe_rs::TranscriptionEngine;

/// Loaded transcription engine
enum LoadedEngine {
    Whisper(WhisperEngine),
    Parakeet(ParakeetEngine),
    Moonshine(MoonshineEngine),
}

impl LoadedEngine {
    /// Transcribe audio samples (expects 16kHz mono f32 audio)
    /// Writes to a temp WAV file and uses transcribe_file API
    fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
        // Create temp WAV file
        let temp_dir = std::env::temp_dir();
        let temp_path = temp_dir.join(format!("hyperwhisper_temp_{}.wav", std::process::id()));

        // Write WAV file
        write_wav_file(&temp_path, samples, 16000)
            .map_err(|e| format!("Failed to write temp WAV: {}", e))?;

        // Transcribe using the engine
        let result = match self {
            LoadedEngine::Whisper(engine) => {
                engine
                    .transcribe_file(&temp_path, None)
                    .map_err(|e| format!("Whisper transcription error: {}", e))
            }
            LoadedEngine::Parakeet(engine) => {
                engine
                    .transcribe_file(&temp_path, None)
                    .map_err(|e| format!("Parakeet transcription error: {}", e))
            }
            LoadedEngine::Moonshine(engine) => {
                engine
                    .transcribe_file(&temp_path, None)
                    .map_err(|e| format!("Moonshine transcription error: {}", e))
            }
        };

        // Clean up temp file
        let _ = std::fs::remove_file(&temp_path);

        result.map(|r| r.text)
    }
}

/// Write samples to a WAV file (16-bit PCM, mono, specified sample rate)
fn write_wav_file(path: &PathBuf, samples: &[f32], sample_rate: u32) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;

    // Convert f32 samples to i16
    let pcm_data: Vec<i16> = samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect();

    let data_size = (pcm_data.len() * 2) as u32;
    let file_size = 36 + data_size;

    // Write WAV header
    file.write_all(b"RIFF")?;
    file.write_all(&file_size.to_le_bytes())?;
    file.write_all(b"WAVE")?;

    // fmt chunk
    file.write_all(b"fmt ")?;
    file.write_all(&16u32.to_le_bytes())?; // chunk size
    file.write_all(&1u16.to_le_bytes())?; // audio format (PCM)
    file.write_all(&1u16.to_le_bytes())?; // num channels (mono)
    file.write_all(&sample_rate.to_le_bytes())?; // sample rate
    file.write_all(&(sample_rate * 2).to_le_bytes())?; // byte rate
    file.write_all(&2u16.to_le_bytes())?; // block align
    file.write_all(&16u16.to_le_bytes())?; // bits per sample

    // data chunk
    file.write_all(b"data")?;
    file.write_all(&data_size.to_le_bytes())?;

    // Write PCM data
    for sample in pcm_data {
        file.write_all(&sample.to_le_bytes())?;
    }

    Ok(())
}

/// Transcription manager handles loading and using transcription models
pub struct TranscriptionManager {
    loaded_engine: Option<LoadedEngine>,
    current_model_id: Option<String>,
    model_manager: Arc<ModelManager>,
}

impl TranscriptionManager {
    /// Create a new transcription manager
    pub fn new(model_manager: Arc<ModelManager>) -> Self {
        Self {
            loaded_engine: None,
            current_model_id: None,
            model_manager,
        }
    }

    /// Check if a model is currently loaded
    pub fn is_model_loaded(&self) -> bool {
        self.loaded_engine.is_some()
    }

    /// Get the currently loaded model ID
    pub fn get_loaded_model_id(&self) -> Option<&str> {
        self.current_model_id.as_deref()
    }

    /// Load a model by ID
    pub fn load_model(&mut self, model_id: &str) -> Result<(), String> {
        // Check if already loaded
        if self.current_model_id.as_deref() == Some(model_id) {
            return Ok(());
        }

        // Unload current model first
        self.unload_model();

        // Get model info
        let model_info = AVAILABLE_MODELS
            .iter()
            .find(|m| m.id == model_id)
            .ok_or_else(|| format!("Unknown model: {}", model_id))?;

        // Check if model is downloaded
        if !self.model_manager.is_model_downloaded(model_id) {
            return Err(format!("Model {} is not downloaded", model_id));
        }

        let model_path = self.model_manager.get_model_path(model_id);

        // Load the appropriate engine based on model type
        let engine = match model_info.engine_type {
            EngineType::Whisper => {
                let files = model_info.get_files();
                let model_file = files.first().ok_or("No model file defined")?;
                let full_path = model_path.join(model_file.filename);

                let mut whisper = WhisperEngine::new();
                whisper
                    .load_model(&full_path)
                    .map_err(|e| format!("Failed to load Whisper model: {}", e))?;

                LoadedEngine::Whisper(whisper)
            }
            EngineType::Parakeet => {
                let mut parakeet = ParakeetEngine::new();
                // Use int8 quantized models
                let params = ParakeetModelParams {
                    quantization: QuantizationType::Int8,
                };
                parakeet
                    .load_model_with_params(&model_path, params)
                    .map_err(|e| format!("Failed to load Parakeet model: {:?}", e))?;

                LoadedEngine::Parakeet(parakeet)
            }
            EngineType::Moonshine => {
                let mut moonshine = MoonshineEngine::new();
                moonshine
                    .load_model(&model_path)
                    .map_err(|e| format!("Failed to load Moonshine model: {}", e))?;

                LoadedEngine::Moonshine(moonshine)
            }
        };

        self.loaded_engine = Some(engine);
        self.current_model_id = Some(model_id.to_string());

        Ok(())
    }

    /// Unload the current model
    pub fn unload_model(&mut self) {
        if let Some(model_id) = self.current_model_id.take() {
            eprintln!("Unloading model {}", model_id);
        }
        self.loaded_engine = None;
    }

    /// Transcribe audio samples (expects 16kHz mono f32 audio)
    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
        let engine = self
            .loaded_engine
            .as_mut()
            .ok_or("No model loaded")?;

        engine.transcribe(samples)
    }

}

/// Thread-safe wrapper for TranscriptionManager
pub struct SharedTranscriptionManager(pub Arc<Mutex<TranscriptionManager>>);

impl SharedTranscriptionManager {
    pub fn new(model_manager: Arc<ModelManager>) -> Self {
        Self(Arc::new(Mutex::new(TranscriptionManager::new(model_manager))))
    }

    pub fn is_model_loaded(&self) -> bool {
        self.0.lock().map(|m| m.is_model_loaded()).unwrap_or(false)
    }

    pub fn get_loaded_model_id(&self) -> Option<String> {
        self.0
            .lock()
            .ok()
            .and_then(|m| m.get_loaded_model_id().map(|s| s.to_string()))
    }

    pub fn load_model(&self, model_id: &str) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|_| "Failed to lock transcription manager")?
            .load_model(model_id)
    }

    pub fn unload_model(&self) {
        if let Ok(mut manager) = self.0.lock() {
            manager.unload_model();
        }
    }
}
