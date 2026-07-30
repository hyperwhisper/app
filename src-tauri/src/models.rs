// The speech models: what is available, what is on disk, and which one runs.

use crate::managers::{ModelStatus, AVAILABLE_MODELS};
use crate::AudioState;
use tauri::{AppHandle, State};

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
pub(crate) fn list_available_models(state: State<'_, AudioState>) -> Vec<ModelInfoResponse> {
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

/// Download a model
#[tauri::command]
pub(crate) async fn download_model(
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
pub(crate) fn delete_model(state: State<'_, AudioState>, model_id: String) -> Result<(), String> {
    // Unload if this is the active model
    if state.transcription_manager.get_loaded_model_id().as_deref() == Some(&model_id) {
        state.transcription_manager.unload_model();
    }

    state.model_manager.delete_model(&model_id)
}

/// Set the active local model
#[tauri::command]
pub(crate) fn set_active_model(
    state: State<'_, AudioState>,
    model_id: String,
) -> Result<(), String> {
    // Verify model exists and is downloaded
    if !state.model_manager.is_model_downloaded(&model_id) {
        return Err(format!("Model {} is not downloaded", model_id));
    }

    state.update_prefs(|p| p.active_local_model_id = Some(model_id));
    Ok(())
}

/// Get the active local model ID
#[tauri::command]
pub(crate) fn get_active_model(state: State<'_, AudioState>) -> Option<String> {
    state.prefs().active_local_model_id
}
