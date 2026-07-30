use crate::managers::model::{EngineType, ModelManager, AVAILABLE_MODELS};
use std::sync::{Arc, Mutex};
use transcribe_rs::onnx::moonshine::{MoonshineModel, MoonshineParams, MoonshineVariant};
use transcribe_rs::onnx::parakeet::{ParakeetModel, ParakeetParams};
use transcribe_rs::onnx::Quantization;
use transcribe_rs::whisper_cpp::{WhisperEngine, WhisperInferenceParams, WhisperLoadParams};

/// Loaded transcription engine
enum LoadedEngine {
    Whisper(WhisperEngine),
    Parakeet(ParakeetModel),
    Moonshine(MoonshineModel),
}

// Whisper writes in whichever style it starts in, so long recordings often come
// back with no capitals and no punctuation. It copies the style of this text
// instead. Whisper reads it as background and never types it out.
//
// Deliberately about nothing: any subject in here becomes words Whisper expects
// to hear, and it would start finding them in unrelated speech.
const STYLE_PROMPT: &str = "Hello. This is an ordinary sentence, written the \
     normal way, with commas where they belong and a full stop at the end. On \
     Monday I told Maria that the work would be done by January. Do you see \
     how it reads? Yes, exactly like that.";

impl LoadedEngine {
    /// Transcribe audio samples (expects 16kHz mono f32 audio).
    /// Samples go straight to the engine - no temp WAV file needed.
    fn transcribe(&mut self, samples: &[f32], language: Option<String>) -> Result<String, String> {
        let result = match self {
            LoadedEngine::Whisper(engine) => {
                // no_speech_thold is whisper.cpp's own default of 0.6, not the
                // 0.2 that transcribe-rs sets. Whisper reads audio in 30 second
                // windows and throws a whole window away when the chance of it
                // being speech is below this number. At 0.2 a quiet microphone
                // loses most windows, which is why a long dictation came back
                // as only its last few sentences, or as nothing at all.
                let params = WhisperInferenceParams {
                    language,
                    no_speech_thold: 0.6,
                    initial_prompt: Some(STYLE_PROMPT.to_string()),
                    ..Default::default()
                };
                engine
                    .transcribe_with(samples, &params)
                    .map_err(|e| format!("Whisper transcription error: {}", e))
            }
            LoadedEngine::Parakeet(model) => {
                // transcribe_with prepends 250ms of silence itself: Parakeet's
                // mel spectrogram preprocessor weakens the start of the audio,
                // which drops the first words without that padding.
                model
                    .transcribe_with(samples, &ParakeetParams::default())
                    .map_err(|e| format!("Parakeet transcription error: {}", e))
            }
            LoadedEngine::Moonshine(model) => model
                .transcribe_with(samples, &MoonshineParams::default())
                .map_err(|e| format!("Moonshine transcription error: {}", e)),
        };

        result.map(|r| r.text)
    }
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

                // Load explicitly instead of WhisperEngine::load, whose defaults
                // turn flash attention on. Flash attention was off before the
                // transcribe-rs 0.3 upgrade and turning it on made Whisper
                // output repeated nonsense, sometimes in the wrong language.
                let params = WhisperLoadParams {
                    flash_attn: false,
                    ..Default::default()
                };
                let whisper = WhisperEngine::load_with_params(&full_path, params)
                    .map_err(|e| format!("Failed to load Whisper model: {}", e))?;

                LoadedEngine::Whisper(whisper)
            }
            EngineType::Parakeet => {
                // Int8 resolves to encoder-model.int8.onnx / decoder_joint-model.int8.onnx,
                // which is what we download.
                let model = ParakeetModel::load(&model_path, &Quantization::Int8)
                    .map_err(|e| format!("Failed to load Parakeet model: {}", e))?;

                LoadedEngine::Parakeet(model)
            }
            EngineType::Moonshine => {
                // FP32 means no quantization suffix: encoder_model.onnx /
                // decoder_model_merged.onnx, matching the downloaded files.
                let model =
                    MoonshineModel::load(&model_path, MoonshineVariant::Base, &Quantization::FP32)
                        .map_err(|e| format!("Failed to load Moonshine model: {}", e))?;

                LoadedEngine::Moonshine(model)
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

    /// Transcribe 16kHz mono f32 audio. `language` None = auto-detect.
    pub fn transcribe(
        &mut self,
        samples: &[f32],
        language: Option<String>,
    ) -> Result<String, String> {
        let engine = self.loaded_engine.as_mut().ok_or("No model loaded")?;

        engine.transcribe(samples, language)
    }
}

/// Thread-safe wrapper for TranscriptionManager
pub struct SharedTranscriptionManager(pub Arc<Mutex<TranscriptionManager>>);

impl SharedTranscriptionManager {
    pub fn new(model_manager: Arc<ModelManager>) -> Self {
        Self(Arc::new(Mutex::new(TranscriptionManager::new(
            model_manager,
        ))))
    }

    pub fn get_loaded_model_id(&self) -> Option<String> {
        self.0
            .lock()
            .ok()
            .and_then(|m| m.get_loaded_model_id().map(|s| s.to_string()))
    }

    pub fn unload_model(&self) {
        if let Ok(mut manager) = self.0.lock() {
            manager.unload_model();
        }
    }
}
