// src-tauri/src/whisper.rs
use crate::config::DictatorConfig; // Use the config struct
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WhisperError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP Request Error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("API Error (Status: {status}): {message}")]
    ApiError { status: u16, message: String },
    #[error("JSON Error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("Model listing not supported by this API")]
    ModelListingNotSupported,
}

#[derive(Deserialize, Debug, Clone)]
pub struct WhisperResponse {
    pub text: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
}

#[derive(Deserialize, Debug)]
struct ModelsResponse {
    data: Vec<ModelInfo>,
}

#[derive(Clone)] // Make WhisperClient cloneable if needed in state
pub struct WhisperClient {
    client: Client,
    config: Arc<Mutex<DictatorConfig>>, // Share config state safely
}

// Needed for state management
use std::sync::{Arc, Mutex};

impl WhisperClient {
    pub fn new(config: Arc<Mutex<DictatorConfig>>) -> Self {
        WhisperClient {
            client: Client::builder()
                .timeout(Duration::from_secs(120)) // Adjust timeout as needed
                .build()
                .unwrap(), // Handle error properly in real app
            config,
        }
    }

    // Helper to get current config values safely
    fn get_api_details(&self) -> (String, String, String) {
        let config = self.config.lock().unwrap();
        (
            config.api_url.clone(),
            config.api_key.clone(),
            config.default_model.clone(),
        )
    }

    pub async fn transcribe(&self, file_path: &Path) -> Result<WhisperResponse, WhisperError> {
        if !file_path.exists() {
            return Err(WhisperError::FileNotFound(
                file_path.to_string_lossy().to_string(),
            ));
        }

        let (api_url, api_key, default_model) = self.get_api_details();
        let url = format!("{}/v1/audio/transcriptions", api_url);

        let mut file = File::open(file_path)?;
        let mut file_bytes = Vec::new();
        file.read_to_end(&mut file_bytes)?;

        let file_part = Part::bytes(file_bytes)
            .file_name(
                file_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "audio.wav".to_string()),
            )
            .mime_str("audio/wav")?; // Ensure correct MIME type

        let mut form = Form::new().part("file", file_part);

        if !default_model.is_empty() {
            form = form.text("model", default_model);
        }
        // Add other parameters like temperature if needed
        // form = form.text("temperature", "0.0");

        let mut request_builder = self.client.post(&url).multipart(form);

        if !api_key.is_empty() {
            request_builder = request_builder.bearer_auth(api_key);
        }

        let response = request_builder.send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            log::error!("API Error: Status {}, Body: {}", status, error_text);
            return Err(WhisperError::ApiError {
                status,
                message: error_text,
            });
        }

        let result: WhisperResponse = response.json().await?;
        Ok(result)
    }

    pub async fn supports_models_endpoint(&self) -> Result<bool, WhisperError> {
        let (api_url, api_key, _) = self.get_api_details();
        let url = format!("{}/v1/models", api_url);

        let mut request_builder = self.client.get(&url).timeout(Duration::from_secs(5)); // Shorter timeout for check

        if !api_key.is_empty() {
            request_builder = request_builder.bearer_auth(api_key);
        }

        match request_builder.send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(e) => {
                // Distinguish between network errors and non-200 responses
                if e.is_connect() || e.is_timeout() {
                    log::warn!("Could not connect to models endpoint: {}", e);
                    Ok(false) // Treat connection errors as "not supported"
                } else {
                    log::error!("Error checking models endpoint: {}", e);
                    Err(WhisperError::Reqwest(e)) // Other reqwest errors
                }
            }
        }
    }

    pub async fn list_models(&self) -> Result<Vec<ModelInfo>, WhisperError> {
        if !self.supports_models_endpoint().await? {
            return Err(WhisperError::ModelListingNotSupported);
        }

        let (api_url, api_key, _) = self.get_api_details();
        let url = format!("{}/v1/models", api_url);

        let mut request_builder = self.client.get(&url);

        if !api_key.is_empty() {
            request_builder = request_builder.bearer_auth(api_key);
        }

        let response = request_builder.send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            return Err(WhisperError::ApiError {
                status,
                message: error_text,
            });
        }

        let models_response: ModelsResponse = response.json().await?;
        Ok(models_response.data)
    }
}
