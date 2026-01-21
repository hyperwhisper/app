use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

/// Engine type for transcription models
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineType {
    Whisper,
    Parakeet,
    Moonshine,
}

/// Model file definition for multi-file models
#[derive(Debug, Clone)]
pub struct ModelFile {
    pub filename: &'static str,
    pub url: &'static str,
    pub size_bytes: u64,
}

/// Model information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub engine_type: EngineType,
    pub total_size_bytes: u64,
    pub is_directory: bool,
    pub accuracy_score: f32,
    pub speed_score: f32,
}

impl ModelInfo {
    /// Get the files needed for this model
    pub fn get_files(&self) -> Vec<ModelFile> {
        match self.id {
            "whisper-small" => vec![ModelFile {
                filename: "ggml-small.bin",
                url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
                size_bytes: 488_000_000,
            }],
            "whisper-medium" => vec![ModelFile {
                filename: "ggml-medium-q5_0.bin",
                url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium-q5_0.bin",
                size_bytes: 539_000_000,
            }],
            "whisper-turbo" => vec![ModelFile {
                filename: "ggml-large-v3-turbo.bin",
                url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
                size_bytes: 1_620_000_000,
            }],
            "whisper-large" => vec![ModelFile {
                filename: "ggml-large-v3-q5_0.bin",
                url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-q5_0.bin",
                size_bytes: 1_080_000_000,
            }],
            "parakeet-v3-int8" => vec![
                ModelFile {
                    filename: "encoder-model.int8.onnx",
                    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/encoder-model.int8.onnx",
                    size_bytes: 652_000_000,
                },
                ModelFile {
                    filename: "decoder_joint-model.int8.onnx",
                    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/decoder_joint-model.int8.onnx",
                    size_bytes: 18_000_000,
                },
                ModelFile {
                    filename: "nemo128.onnx",
                    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/nemo128.onnx",
                    size_bytes: 140_000,
                },
                ModelFile {
                    filename: "vocab.txt",
                    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/vocab.txt",
                    size_bytes: 94_000,
                },
                ModelFile {
                    filename: "config.json",
                    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/config.json",
                    size_bytes: 100,
                },
            ],
            "parakeet-v2-int8" => vec![
                ModelFile {
                    filename: "encoder-model.int8.onnx",
                    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main/encoder-model.int8.onnx",
                    size_bytes: 652_000_000,
                },
                ModelFile {
                    filename: "decoder_joint-model.int8.onnx",
                    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main/decoder_joint-model.int8.onnx",
                    size_bytes: 9_000_000,
                },
                ModelFile {
                    filename: "nemo128.onnx",
                    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main/nemo128.onnx",
                    size_bytes: 140_000,
                },
                ModelFile {
                    filename: "vocab.txt",
                    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main/vocab.txt",
                    size_bytes: 94_000,
                },
                ModelFile {
                    filename: "config.json",
                    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main/config.json",
                    size_bytes: 100,
                },
            ],
            "moonshine-base" => vec![
                ModelFile {
                    filename: "encoder_model.onnx",
                    url: "https://huggingface.co/UsefulSensors/moonshine/resolve/main/onnx/merged/base/float/encoder_model.onnx",
                    size_bytes: 80_800_000,
                },
                ModelFile {
                    filename: "decoder_model_merged.onnx",
                    url: "https://huggingface.co/UsefulSensors/moonshine/resolve/main/onnx/merged/base/float/decoder_model_merged.onnx",
                    size_bytes: 166_000_000,
                },
                ModelFile {
                    filename: "tokenizer.json",
                    url: "https://huggingface.co/UsefulSensors/moonshine/resolve/main/onnx/merged/base/float/tokenizer.json",
                    size_bytes: 3_760_000,
                },
            ],
            _ => vec![],
        }
    }
}

/// All available models
pub static AVAILABLE_MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "whisper-small",
        name: "Whisper Small",
        description: "Fast, fairly accurate. Good for everyday use.",
        engine_type: EngineType::Whisper,
        total_size_bytes: 488_000_000,
        is_directory: false,
        accuracy_score: 0.7,
        speed_score: 0.9,
    },
    ModelInfo {
        id: "whisper-medium",
        name: "Whisper Medium (Q5)",
        description: "Good balance of speed and accuracy.",
        engine_type: EngineType::Whisper,
        total_size_bytes: 539_000_000,
        is_directory: false,
        accuracy_score: 0.8,
        speed_score: 0.7,
    },
    ModelInfo {
        id: "whisper-turbo",
        name: "Whisper Turbo",
        description: "Large v3 turbo - balanced speed and quality.",
        engine_type: EngineType::Whisper,
        total_size_bytes: 1_620_000_000,
        is_directory: false,
        accuracy_score: 0.9,
        speed_score: 0.6,
    },
    ModelInfo {
        id: "whisper-large",
        name: "Whisper Large (Q5)",
        description: "Best Whisper accuracy, slower.",
        engine_type: EngineType::Whisper,
        total_size_bytes: 1_080_000_000,
        is_directory: false,
        accuracy_score: 0.95,
        speed_score: 0.4,
    },
    ModelInfo {
        id: "parakeet-v3-int8",
        name: "Parakeet V3 (int8)",
        description: "English only. Fast and highly accurate.",
        engine_type: EngineType::Parakeet,
        total_size_bytes: 670_000_000,
        is_directory: true,
        accuracy_score: 0.92,
        speed_score: 0.85,
    },
    ModelInfo {
        id: "parakeet-v2-int8",
        name: "Parakeet V2 (int8)",
        description: "English only. Excellent accuracy.",
        engine_type: EngineType::Parakeet,
        total_size_bytes: 661_000_000,
        is_directory: true,
        accuracy_score: 0.9,
        speed_score: 0.85,
    },
    ModelInfo {
        id: "moonshine-base",
        name: "Moonshine Base",
        description: "Very fast, handles accents well.",
        engine_type: EngineType::Moonshine,
        total_size_bytes: 251_000_000, // encoder + decoder + tokenizer
        is_directory: true,
        accuracy_score: 0.75,
        speed_score: 0.95,
    },
];

/// Model download/ready status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    NotDownloaded,
    Downloading { progress: f64 },
    Downloaded,
    Error { message: String },
}

/// Download progress event payload
#[derive(Clone, Serialize)]
pub struct ModelDownloadProgress {
    pub model_id: String,
    pub file: String,
    pub progress: f64,
    pub total_files: usize,
    pub current_file: usize,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
}

/// Model manager for handling model downloads and lifecycle
pub struct ModelManager {
    models_dir: PathBuf,
    download_status: Arc<Mutex<std::collections::HashMap<String, ModelStatus>>>,
}

impl ModelManager {
    /// Create a new model manager
    pub fn new() -> Result<Self, String> {
        let data_dir = dirs::data_local_dir()
            .ok_or_else(|| "Could not find local data directory".to_string())?;
        let models_dir = data_dir.join("hyperwhisper").join("models");

        if !models_dir.exists() {
            fs::create_dir_all(&models_dir)
                .map_err(|e| format!("Failed to create models directory: {}", e))?;
        }

        Ok(Self {
            models_dir,
            download_status: Arc::new(Mutex::new(std::collections::HashMap::new())),
        })
    }

    /// Get the directory path for a specific model
    pub fn get_model_path(&self, model_id: &str) -> PathBuf {
        self.models_dir.join(model_id)
    }

    /// Get model info by ID
    pub fn get_model_info(&self, model_id: &str) -> Option<&'static ModelInfo> {
        AVAILABLE_MODELS.iter().find(|m| m.id == model_id)
    }

    /// Check if a model is downloaded
    pub fn is_model_downloaded(&self, model_id: &str) -> bool {
        let model = match self.get_model_info(model_id) {
            Some(m) => m,
            None => return false,
        };

        let model_path = self.get_model_path(model_id);
        if !model_path.exists() {
            return false;
        }

        // Check if all required files exist
        let files = model.get_files();
        files.iter().all(|f| model_path.join(f.filename).exists())
    }

    /// Get the status of a model
    pub fn get_model_status(&self, model_id: &str) -> ModelStatus {
        // Check if currently downloading
        if let Ok(status_map) = self.download_status.lock() {
            if let Some(status) = status_map.get(model_id) {
                return status.clone();
            }
        }

        // Check if downloaded
        if self.is_model_downloaded(model_id) {
            ModelStatus::Downloaded
        } else {
            ModelStatus::NotDownloaded
        }
    }

    /// Download a model with progress events
    pub fn download_model(
        &self,
        model_id: &str,
        app_handle: &AppHandle,
    ) -> Result<(), String> {
        let model = self
            .get_model_info(model_id)
            .ok_or_else(|| format!("Unknown model: {}", model_id))?;

        let model_path = self.get_model_path(model_id);

        // Create model directory
        if !model_path.exists() {
            fs::create_dir_all(&model_path)
                .map_err(|e| format!("Failed to create model directory: {}", e))?;
        }

        // Mark as downloading
        if let Ok(mut status_map) = self.download_status.lock() {
            status_map.insert(
                model_id.to_string(),
                ModelStatus::Downloading { progress: 0.0 },
            );
        }

        let files = model.get_files();
        let total_files = files.len();
        let agent = ureq::AgentBuilder::new().redirects(10).build();

        for (i, file) in files.iter().enumerate() {
            let file_path = model_path.join(file.filename);

            // Skip if file already exists
            if file_path.exists() {
                let _ = app_handle.emit(
                    "model-download-progress",
                    ModelDownloadProgress {
                        model_id: model_id.to_string(),
                        file: file.filename.to_string(),
                        progress: 100.0,
                        total_files,
                        current_file: i + 1,
                        bytes_downloaded: file.size_bytes,
                        total_bytes: file.size_bytes,
                    },
                );
                continue;
            }

            // Emit progress start
            let _ = app_handle.emit(
                "model-download-progress",
                ModelDownloadProgress {
                    model_id: model_id.to_string(),
                    file: file.filename.to_string(),
                    progress: 0.0,
                    total_files,
                    current_file: i + 1,
                    bytes_downloaded: 0,
                    total_bytes: file.size_bytes,
                },
            );

            // Download file
            eprintln!("Downloading {} from {}", file.filename, file.url);
            let response = agent.get(file.url).call().map_err(|e| {
                let err_msg = format!("Failed to download {}: {}", file.filename, e);
                if let Ok(mut status_map) = self.download_status.lock() {
                    status_map.insert(
                        model_id.to_string(),
                        ModelStatus::Error {
                            message: err_msg.clone(),
                        },
                    );
                }
                err_msg
            })?;

            let content_length = response
                .header("Content-Length")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(file.size_bytes);

            // Create temp file for download
            let temp_path = file_path.with_extension("tmp");
            let mut out_file = fs::File::create(&temp_path)
                .map_err(|e| format!("Failed to create file {}: {}", file.filename, e))?;

            let mut reader = response.into_reader();
            let mut buffer = [0u8; 8192];
            let mut downloaded: u64 = 0;
            let mut last_progress = 0.0;

            loop {
                let bytes_read = reader
                    .read(&mut buffer)
                    .map_err(|e| format!("Failed to read data for {}: {}", file.filename, e))?;

                if bytes_read == 0 {
                    break;
                }

                out_file
                    .write_all(&buffer[..bytes_read])
                    .map_err(|e| format!("Failed to write {}: {}", file.filename, e))?;

                downloaded += bytes_read as u64;

                // Emit progress every 1%
                let progress = (downloaded as f64 / content_length as f64) * 100.0;
                if progress - last_progress >= 1.0 {
                    last_progress = progress;

                    // Update status
                    if let Ok(mut status_map) = self.download_status.lock() {
                        let overall = ((i as f64 + progress / 100.0) / total_files as f64) * 100.0;
                        status_map.insert(
                            model_id.to_string(),
                            ModelStatus::Downloading { progress: overall },
                        );
                    }

                    let _ = app_handle.emit(
                        "model-download-progress",
                        ModelDownloadProgress {
                            model_id: model_id.to_string(),
                            file: file.filename.to_string(),
                            progress,
                            total_files,
                            current_file: i + 1,
                            bytes_downloaded: downloaded,
                            total_bytes: content_length,
                        },
                    );
                }
            }

            // Rename temp file to final file
            fs::rename(&temp_path, &file_path)
                .map_err(|e| format!("Failed to rename {}: {}", file.filename, e))?;

            // Emit completion for this file
            let _ = app_handle.emit(
                "model-download-progress",
                ModelDownloadProgress {
                    model_id: model_id.to_string(),
                    file: file.filename.to_string(),
                    progress: 100.0,
                    total_files,
                    current_file: i + 1,
                    bytes_downloaded: content_length,
                    total_bytes: content_length,
                },
            );
        }

        // Mark as downloaded
        if let Ok(mut status_map) = self.download_status.lock() {
            status_map.insert(model_id.to_string(), ModelStatus::Downloaded);
        }

        Ok(())
    }

    /// Delete a model
    pub fn delete_model(&self, model_id: &str) -> Result<(), String> {
        let model_path = self.get_model_path(model_id);

        if model_path.exists() {
            fs::remove_dir_all(&model_path)
                .map_err(|e| format!("Failed to delete model: {}", e))?;
        }

        // Clear status
        if let Ok(mut status_map) = self.download_status.lock() {
            status_map.remove(model_id);
        }

        Ok(())
    }
}
