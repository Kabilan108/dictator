use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use crate::glossary::{self, Vocabulary};
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
    /// configured provider to send to; empty uses the config default
    pub provider: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TranscriptionResponse {
    pub text: String,
}

pub struct WhisperClient {
    config: ApiConfig,
    http: reqwest::Client,
    vocabulary: Vocabulary,
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
            vocabulary: Vocabulary::new(glossary::glossary_path()),
        })
    }

    /// Reads the glossary from `path` instead of the data directory.
    pub fn with_glossary_path(mut self, path: std::path::PathBuf) -> Self {
        self.vocabulary = Vocabulary::new(path);
        self
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

        let provider_name = if req.provider.is_empty() {
            self.config.active_provider.as_str()
        } else {
            req.provider.as_str()
        };
        let Some(provider) = self.config.providers.get(provider_name) else {
            bail!("provider '{provider_name}' not found");
        };

        if let Some(problem) = crate::provider::provider_problem(provider) {
            bail!("provider '{provider_name}' cannot be used: {problem}");
        }

        let mut model = req.model.as_str();
        if model.is_empty() {
            model = provider.model.as_str();
        }
        if model.is_empty() {
            model = "distil-large-v3";
        }

        let url = normalize_endpoint(&provider.endpoint);
        let vocabulary = if provider.send_vocabulary {
            self.vocabulary.current()
        } else {
            None
        };
        let vocabulary = vocabulary
            .map(|terms| {
                debug!(terms = terms.len(), "sending glossary vocabulary");
                serde_json::to_string(&terms)
            })
            .transpose()
            .map_err(|err| anyhow!("failed to encode vocabulary: {err}"))?;

        let mut attempts = 0;
        let resp = loop {
            attempts += 1;
            let form = build_form(req, model, vocabulary.as_deref())?;
            debug!(url = %url, provider = provider_name, model = %model, attempt = attempts, "sending request");

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
                Ok(resp) => break resp,
                Err(err) if attempts == 1 && never_reached_server(&err) => {
                    warn!(attempt = attempts, err = %err, "could not connect, retrying");
                    tokio::select! {
                        _ = cancel.cancelled() => bail!("transcription cancelled"),
                        _ = tokio::time::sleep(RETRY_DELAY) => {}
                    }
                }
                Err(err) => {
                    error!(err = %err, attempts, "transcription request failed");
                    if attempts > 1 {
                        bail!("request failed after {attempts} attempts: {err}");
                    }
                    bail!("request failed: {err}");
                }
            }
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

/// Only a request that never reached the provider is safe to send again. After
/// a timeout or any failure once the body was sent, the provider may still be
/// transcribing, and a resend would queue a second inference behind the first.
fn never_reached_server(err: &reqwest::Error) -> bool {
    err.is_connect() && !err.is_timeout()
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

fn build_form(req: &TranscriptionRequest, model: &str, vocabulary: Option<&str>) -> Result<Form> {
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
    if let Some(vocabulary) = vocabulary {
        form = form.text("vocabulary", vocabulary.to_owned());
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
            provider: String::new(),
        }
    }

    #[tokio::test]
    async fn request_provider_overrides_the_config_default() {
        let body = br#"{"text":"routed"}"#;
        let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
        let (mut client, _, server) =
            mock_provider([response.as_bytes(), body].concat(), false).await;
        client.config.providers.insert(
            "unreachable".into(),
            crate::utils::Provider {
                endpoint: "http://127.0.0.1:1".into(),
                key: "k".into(),
                ..Default::default()
            },
        );
        client.config.providers.insert(
            "keyless".into(),
            crate::utils::Provider {
                endpoint: "http://127.0.0.1:1".into(),
                key: "${env:DICTATOR_TEST_UNSET_KEY}".into(),
                ..Default::default()
            },
        );
        client.config.active_provider = "unreachable".into();

        let mut routed = request();
        routed.provider = "openai".into();
        let result = client
            .transcribe(&CancellationToken::new(), &routed)
            .await
            .unwrap();
        assert_eq!(result.text, "routed");
        server.await.unwrap();

        for (provider, expected) in [
            (
                "keyless",
                "provider 'keyless' cannot be used: DICTATOR_TEST_UNSET_KEY is not set in the daemon's environment",
            ),
            ("gone", "provider 'gone' not found"),
        ] {
            let mut rejected = request();
            rejected.provider = provider.into();
            let error = client
                .transcribe(&CancellationToken::new(), &rejected)
                .await
                .unwrap_err()
                .to_string();
            assert_eq!(error, expected);
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

    fn form_field(multipart: &str, name: &str) -> Option<String> {
        let header = format!("name=\"{name}\"\r\n\r\n");
        let start = multipart.find(&header)? + header.len();
        let end = multipart[start..].find("\r\n")?;
        Some(multipart[start..start + end].to_string())
    }

    #[tokio::test]
    async fn vocabulary_field_follows_provider_flag_and_glossary_file() {
        /// name, send_vocabulary, stored terms (None: no file), expected field
        type Case = (
            &'static str,
            bool,
            Option<&'static [&'static str]>,
            Option<&'static str>,
        );
        let cases: &[Case] = &[
            ("flag off", false, Some(&["Claude"]), None),
            ("file missing", true, None, None),
            ("empty list disables boosting", true, Some(&[]), Some("[]")),
            (
                "terms in order",
                true,
                Some(&["Claude", "tmux"]),
                Some(r#"["Claude","tmux"]"#),
            ),
        ];
        for (name, send_vocabulary, terms, expected) in cases {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("glossary.json");
            if let Some(terms) = terms {
                std::fs::write(
                    &path,
                    serde_json::to_vec(&serde_json::json!({"version": 1, "terms": terms})).unwrap(),
                )
                .unwrap();
            }
            let body = br#"{"text":"hello"}"#;
            let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            let (mut client, _, server) =
                mock_provider([response.as_bytes(), body].concat(), false).await;
            client
                .config
                .providers
                .get_mut("openai")
                .unwrap()
                .send_vocabulary = *send_vocabulary;
            let client = client.with_glossary_path(path);

            client
                .transcribe(&CancellationToken::new(), &request())
                .await
                .unwrap();
            let multipart = String::from_utf8(server.await.unwrap()).unwrap();
            assert_eq!(
                form_field(&multipart, "vocabulary").as_deref(),
                *expected,
                "{name}"
            );
            assert_eq!(
                form_field(&multipart, "model").as_deref(),
                Some("test-model")
            );
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

    fn client_for(endpoint: String, timeout_secs: i64) -> WhisperClient {
        let mut config = crate::utils::default_config().api;
        config.timeout = timeout_secs;
        let provider = config.providers.get_mut("openai").unwrap();
        provider.endpoint = endpoint;
        provider.key = "test-key".into();
        WhisperClient::new(&config).unwrap()
    }

    /// Accepts connections, reads one request each, and never replies.
    async fn silent_provider() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&accepted);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buffer = vec![0; 64 * 1024];
                    while stream.read(&mut buffer).await.is_ok_and(|read| read > 0) {}
                });
            }
        });
        (endpoint, accepted)
    }

    #[tokio::test]
    async fn timeout_after_sending_is_not_retried() {
        let (endpoint, accepted) = silent_provider().await;
        let client = client_for(endpoint, 1);
        let started = std::time::Instant::now();
        let error = client
            .transcribe(&CancellationToken::new(), &request())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("request failed: "), "{error}");
        assert!(started.elapsed() < Duration::from_millis(1900));
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn closed_connection_after_sending_is_not_retried() {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            drop(reader);
            // A second connection would mean the request was sent again.
            tokio::time::timeout(Duration::from_millis(1500), listener.accept())
                .await
                .is_ok()
        });
        let error = client_for(endpoint, 10)
            .transcribe(&CancellationToken::new(), &request())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("request failed: "), "{error}");
        assert!(!server.await.unwrap(), "request was retried");
    }

    #[tokio::test]
    async fn refused_connection_is_retried_once() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        let reserved = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = reserved.local_addr().unwrap();
        drop(reserved);
        let client = client_for(format!("http://{address}"), 10);
        // The provider comes up during the retry delay.
        let server = tokio::spawn(async move {
            tokio::time::sleep(RETRY_DELAY / 3).await;
            let listener = tokio::net::TcpListener::bind(address).await.unwrap();
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
            let mut body = vec![0; length];
            reader.read_exact(&mut body).await.unwrap();
            let reply = br#"{"text":"second try"}"#;
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", reply.len());
            reader.get_mut().write_all(head.as_bytes()).await.unwrap();
            reader.get_mut().write_all(reply).await.unwrap();
        });
        let result = client
            .transcribe(&CancellationToken::new(), &request())
            .await
            .unwrap();
        assert_eq!(result.text, "second try");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connect_error_is_classified_as_never_sent() {
        let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = reserved.local_addr().unwrap();
        drop(reserved);
        let error = reqwest::Client::new()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap_err();
        assert!(never_reached_server(&error), "{error:?}");
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
