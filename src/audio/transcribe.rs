use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use crate::utils::ApiConfig;

pub const RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Default)]
pub struct TranscriptionRequest {
    pub audio_data: Vec<u8>,
    pub filename: String,
    /// optional, defaults to "distil-large-v3"
    pub model: String,
    /// optional
    pub language: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TranscriptionResponse {
    pub text: String,
}

pub struct WhisperClient {
    config: ApiConfig,
    http: reqwest::Client,
}

impl WhisperClient {
    pub fn new(config: &ApiConfig) -> Self {
        let timeout = Duration::from_secs(config.timeout.max(0) as u64);
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client");
        Self {
            config: config.clone(),
            http,
        }
    }

    pub async fn transcribe(
        &self,
        cancel: &CancellationToken,
        req: &TranscriptionRequest,
    ) -> Result<TranscriptionResponse> {
        debug!(filename = %req.filename, "starting transcription request");

        let Some(provider) = self.config.providers.get(&self.config.active_provider) else {
            bail!(
                "active provider '{}' not found",
                self.config.active_provider
            );
        };

        if provider.key.is_empty() {
            bail!(
                "API key is required but not configured for provider '{}'",
                self.config.active_provider
            );
        }

        let mut model = req.model.as_str();
        if model.is_empty() {
            model = provider.model.as_str();
        }
        if model.is_empty() {
            model = "distil-large-v3";
        }

        let url = normalize_endpoint(&provider.endpoint);

        let mut response = None;
        let mut last_err = None;

        for attempt in 0..2 {
            let form = build_form(req, model)?;
            debug!(url = %url, model = %model, attempt = attempt + 1, "sending request");

            let send = self
                .http
                .post(&url)
                .bearer_auth(&provider.key)
                .multipart(form)
                .send();

            let result = tokio::select! {
                _ = cancel.cancelled() => bail!("transcription cancelled"),
                result = send => result,
            };

            match result {
                Ok(resp) => {
                    response = Some(resp);
                    break;
                }
                Err(err) => {
                    last_err = Some(err);
                    if attempt == 0 {
                        warn!(attempt = attempt + 1, err = %last_err.as_ref().unwrap(), "request attempt failed, retrying");
                        tokio::select! {
                            _ = cancel.cancelled() => bail!("transcription cancelled"),
                            _ = tokio::time::sleep(RETRY_DELAY) => {}
                        }
                    }
                }
            }
        }

        let Some(resp) = response else {
            let err = last_err.expect("error recorded on failure");
            error!(err = %err, "all request attempts failed");
            bail!("request failed after 2 attempts: {err}");
        };

        let status = resp.status();
        if status != reqwest::StatusCode::OK {
            let body = resp.text().await.unwrap_or_default();
            let msg = format!(
                "API request failed with status {}: {}",
                status.as_u16(),
                body
            );
            error!(err = %msg, "api request failed");
            bail!(msg);
        }

        let parsed: TranscriptionResponse = match resp.json().await {
            Ok(parsed) => parsed,
            Err(err) => {
                error!(err = %err, "failed to decode response");
                bail!("failed to decode response: {err}");
            }
        };

        debug!(
            length = parsed.text.len(),
            "transcription completed successfully"
        );
        Ok(parsed)
    }
}

fn build_form(req: &TranscriptionRequest, model: &str) -> Result<Form> {
    let file = Part::bytes(req.audio_data.clone())
        .file_name(req.filename.clone())
        .mime_str("application/octet-stream")
        .map_err(|e| anyhow!("failed to create form file: {e}"))?;

    let mut form = Form::new()
        .part("file", file)
        .text("model", model.to_string());

    if !req.language.is_empty() {
        form = form.text("language", req.language.clone());
    }

    Ok(form)
}

pub fn normalize_endpoint(endpoint: &str) -> String {
    if endpoint.ends_with("/transcriptions") {
        return endpoint.to_string();
    }
    if endpoint.ends_with("/v1/audio/transcriptions") {
        return endpoint.to_string();
    }
    if endpoint.ends_with("/v1/audio") {
        return format!("{endpoint}/transcriptions");
    }
    if endpoint.ends_with("/v1") {
        return format!("{endpoint}/audio/transcriptions");
    }
    format!("{endpoint}/v1/audio/transcriptions")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_endpoints() {
        assert_eq!(
            normalize_endpoint("https://api.openai.com/v1/audio/transcriptions"),
            "https://api.openai.com/v1/audio/transcriptions"
        );
        assert_eq!(
            normalize_endpoint("https://x.dev/v1/audio"),
            "https://x.dev/v1/audio/transcriptions"
        );
        assert_eq!(
            normalize_endpoint("https://x.dev/v1"),
            "https://x.dev/v1/audio/transcriptions"
        );
        assert_eq!(
            normalize_endpoint("https://x.dev"),
            "https://x.dev/v1/audio/transcriptions"
        );
        assert_eq!(
            normalize_endpoint("https://x.dev/custom/transcriptions"),
            "https://x.dev/custom/transcriptions"
        );
    }
}
