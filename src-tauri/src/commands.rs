// src-tauri/src/commands.rs
use crate::config::{save_config, DictatorConfig};
use crate::files::create_new_recording_file_path;
use crate::whisper::ModelInfo;
use crate::AppState;
use crate::CommandError;
use tauri::State;
// Remove unused: use std::sync::Arc;

#[derive(serde::Serialize)]
pub struct SimpleResult {
    success: bool,
    error: Option<String>,
}

#[derive(serde::Serialize)]
pub struct TranscriptionResult {
    success: bool,
    transcript: Option<String>,
    error: Option<String>,
}

#[tauri::command]
pub async fn start_recording(state: State<'_, AppState>) -> Result<SimpleResult, CommandError> {
    log::debug!("start_recording command invoked");
    // Access the inner Option<AudioRecorder> behind the Mutex
    let recorder_guard = state.recorder.lock().map_err(|_| CommandError {
        message: "Failed to lock recorder state".into(),
    })?;

    if let Some(recorder) = &*recorder_guard {
        // Call start_recording on the AudioRecorder instance
        recorder.start_recording().map_err(|e| {
            log::error!("Failed to start recording: {}", e);
            CommandError::from(e)
        })?;
        Ok(SimpleResult {
            success: true,
            error: None,
        })
    } else {
        log::error!("Audio recorder not initialized");
        Err(CommandError {
            message: "Audio recorder not initialized".into(),
        })
    }
}

#[tauri::command]
pub async fn stop_recording(
    state: State<'_, AppState>,
) -> Result<TranscriptionResult, CommandError> {
    log::debug!("stop_recording command invoked");
    let output_path = create_new_recording_file_path().map_err(CommandError::from)?;

    // --- Stop Recording ---
    {
        // Scope for recorder lock
        let recorder_guard = state.recorder.lock().map_err(|_| CommandError {
            message: "Failed to lock recorder state".into(),
        })?;
        if let Some(recorder) = &*recorder_guard {
            // Call stop_recording on the AudioRecorder instance
            recorder.stop_recording(output_path.clone()).map_err(|e| {
                log::error!("Failed to stop recording or write WAV: {}", e);
                CommandError::from(e)
            })?;
        } else {
            log::error!("Audio recorder not initialized");
            return Err(CommandError {
                message: "Audio recorder not initialized".into(),
            });
        }
    } // Recorder lock released

    // --- Transcribe ---
    log::info!("Transcribing file: {:?}", output_path);
    // Access client directly via state.client (Arc is Send+Sync)
    let client = state.client.clone();
    let transcription = client.transcribe(&output_path).await.map_err(|e| {
        log::error!("Transcription failed: {}", e);
        CommandError::from(e)
    })?;

    // --- Cleanup (Optional) ---
    // ...

    log::info!("Transcription successful: {}", transcription.text);
    Ok(TranscriptionResult {
        success: true,
        transcript: Some(transcription.text),
        error: None,
    })
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<DictatorConfig, CommandError> {
    log::debug!("get_settings command invoked");
    // Access config via state.config
    let config = state.config.lock().map_err(|_| CommandError {
        message: "Failed to lock config state".into(),
    })?;
    Ok(config.clone())
}

#[tauri::command]
pub async fn save_settings(
    settings: DictatorConfig,
    state: State<'_, AppState>,
) -> Result<SimpleResult, CommandError> {
    log::debug!("save_settings command invoked: {:?}", settings);
    save_config(&settings).map_err(|e| {
        log::error!("Failed to save config: {}", e);
        CommandError::from(e)
    })?;

    // Update the shared state via state.config
    let mut config_state = state.config.lock().map_err(|_| CommandError {
        message: "Failed to lock config state for update".into(),
    })?;
    *config_state = settings;

    Ok(SimpleResult {
        success: true,
        error: None,
    })
}

#[tauri::command]
pub async fn list_available_models(
    state: State<'_, AppState>,
) -> Result<Vec<ModelInfo>, CommandError> {
    log::debug!("list_available_models command invoked");
    // Access client via state.client
    let client = state.client.clone();
    client.list_models().await.map_err(|e| {
        log::error!("Failed to list models: {}", e);
        CommandError::from(e)
    })
}

#[tauri::command]
pub async fn supports_models_endpoint(state: State<'_, AppState>) -> Result<bool, CommandError> {
    log::debug!("supports_models_endpoint command invoked");
    // Access client via state.client
    let client = state.client.clone();
    client.supports_models_endpoint().await.map_err(|e| {
        log::error!("Failed to check models endpoint support: {}", e);
        CommandError::from(e)
    })
}
