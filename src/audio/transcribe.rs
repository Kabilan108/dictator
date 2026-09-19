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
    pub audio_data: bytes::Bytes,
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
    pub fn new(config: &ApiConfig) -> Result<Self> {
        if config.timeout <= 0 {
            bail!("API timeout must be > 0");
        }
        let timeout = Duration::from_secs(config.timeout as u64);
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|err| anyhow!("failed to create HTTP client: {err}"))?;
        Ok(Self {
            config: config.clone(),
            http,
        })
    }

    pub async fn transcribe(
        &self,
        cancel: &CancellationToken,
        req: &TranscriptionRequest,
    ) -> Result<TranscriptionResponse> {
        if cancel.is_cancelled() {
            bail!("transcription cancelled");
        }
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
                biased;
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
        let limit = if status.is_success() {
            MAX_RESPONSE_BYTES
        } else {
            MAX_ERROR_BYTES
        };
        let body = read_response(cancel, resp, limit).await?;
        if !status.is_success() {
            bail!(
                "API request failed with status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            );
        }
        let parsed: TranscriptionResponse = serde_json::from_slice(&body)
            .map_err(|err| anyhow!("failed to decode response: {err}"))?;

        debug!(
            length = parsed.text.len(),
            "transcription completed successfully"
        );
        Ok(parsed)
    }
}

// Even a long dictation should fit comfortably within this limit. Bound both
// successful and failed provider replies before allocating the entire body.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_ERROR_BYTES: usize = 8 * 1024;

async fn read_response(
    cancel: &CancellationToken,
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|len| len > limit as u64)
    {
        bail!("API response exceeds {limit} bytes");
    }
    let mut body = Vec::new();
    loop {
        let chunk = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("transcription cancelled"),
            chunk = response.chunk() => chunk?,
        };
        let Some(chunk) = chunk else { return Ok(body) };
        if chunk.len() > limit - body.len() {
            bail!("API response exceeds {limit} bytes");
        }
        body.extend_from_slice(&chunk);
    }
}

fn build_form(req: &TranscriptionRequest, model: &str) -> Result<Form> {
    let file = Part::stream_with_length(req.audio_data.clone(), req.audio_data.len() as u64)
        .file_name(
            std::path::Path::new(&req.filename)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("recording.wav")
                .to_owned(),
        )
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
    let Ok(mut url) = reqwest::Url::parse(endpoint) else {
        // Request construction reports invalid URLs with reqwest's error.
        return endpoint.to_owned();
    };
    let path = url.path().trim_end_matches('/');
    let path = if path.ends_with("/transcriptions") {
        path.to_owned()
    } else if path.ends_with("/v1/audio") {
        format!("{path}/transcriptions")
    } else if path.ends_with("/v1") {
        format!("{path}/audio/transcriptions")
    } else {
        format!("{path}/v1/audio/transcriptions")
    };
    url.set_path(&path);
    url.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mock_provider(
        response: Vec<u8>,
        stall: bool,
    ) -> (
        WhisperClient,
        tokio::sync::oneshot::Receiver<()>,
        tokio::task::JoinHandle<Vec<u8>>,
    ) {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = crate::utils::default_config().api;
        let provider = config.providers.get_mut("openai").unwrap();
        provider.endpoint = format!("http://{}", listener.local_addr().unwrap());
        provider.key = "test-key".into();
        let (sent, received) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse::<usize>().unwrap();
                }
            }
            let mut request = vec![0; length];
            reader.read_exact(&mut request).await.unwrap();
            reader.get_mut().write_all(&response).await.unwrap();
            let _ = sent.send(());
            if stall {
                std::future::pending::<()>().await;
            }
            request
        });
        (WhisperClient::new(&config).unwrap(), received, task)
    }

    fn request() -> TranscriptionRequest {
        TranscriptionRequest {
            audio_data: bytes::Bytes::from_static(b"test audio"),
            filename: "sample.wav".into(),
            model: "test-model".into(),
            language: "en".into(),
        }
    }

    #[tokio::test]
    async fn multipart_roundtrip() {
        let body = br#"{"text":"hello"}"#;
        let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
        let (client, _, server) = mock_provider([response.as_bytes(), body].concat(), false).await;
        let result = client
            .transcribe(&CancellationToken::new(), &request())
            .await
            .unwrap();
        assert_eq!(result.text, "hello");
        let multipart = String::from_utf8(server.await.unwrap()).unwrap();
        for expected in [
            "test audio",
            "sample.wav",
            "test-model",
            "name=\"language\"",
            "en",
        ] {
            assert!(multipart.contains(expected), "missing {expected}");
        }
    }

    #[tokio::test]
    async fn cancellation_interrupts_stalled_success_and_error_bodies() {
        for status in ["200 OK", "500 Internal Server Error"] {
            let response = format!("HTTP/1.1 {status}\r\nContent-Length: 100\r\n\r\nx");
            let (client, sent, server) = mock_provider(response.into_bytes(), true).await;
            let cancel = CancellationToken::new();
            let task_cancel = cancel.clone();
            let task =
                tokio::spawn(async move { client.transcribe(&task_cancel, &request()).await });
            sent.await.unwrap();
            cancel.cancel();
            let result = tokio::time::timeout(Duration::from_millis(500), task)
                .await
                .unwrap()
                .unwrap();
            server.abort();
            let _ = server.await;
            assert!(result.unwrap_err().to_string().contains("cancelled"));
        }
    }

    #[tokio::test]
    async fn rejects_oversized_declared_and_streamed_bodies() {
        let declared = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            MAX_RESPONSE_BYTES + 1
        );
        let chunked = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            MAX_RESPONSE_BYTES + 1,
            "x".repeat(MAX_RESPONSE_BYTES + 1)
        );
        for response in [declared, chunked] {
            let (client, _, server) = mock_provider(response.into_bytes(), false).await;
            let result = client
                .transcribe(&CancellationToken::new(), &request())
                .await;
            assert!(result.unwrap_err().to_string().contains("exceeds"));
            let _ = server.await;
        }
    }

    #[tokio::test]
    async fn already_cancelled_request_does_not_connect() {
        let (client, _, server) = mock_provider(Vec::new(), false).await;
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(
            client
                .transcribe(&cancel, &request())
                .await
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        assert!(!server.is_finished());
        server.abort();
        let _ = server.await;
    }

    #[test]
    fn normalizes_endpoints() {
        assert_eq!(
            normalize_endpoint("https://x.dev/v1/?region=us"),
            "https://x.dev/v1/audio/transcriptions?region=us"
        );
        assert_eq!(
            normalize_endpoint("https://x.dev/custom/transcriptions/"),
            "https://x.dev/custom/transcriptions"
        );
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
