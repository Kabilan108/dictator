//! Transcribes a WAV file using the configured provider, to compare the Rust
//! Whisper client against transcripts produced by the Go daemon.
use dictator::audio::{TranscriptionRequest, WhisperClient};
use dictator::utils::get_config;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: transcribe_file <wav>");
    let cfg = get_config().expect("config");
    let provider = &cfg.api.providers[&cfg.api.active_provider];
    let client = WhisperClient::new(&cfg.api).expect("HTTP client");
    let audio_data = std::fs::read(&path).expect("read wav");
    let req = TranscriptionRequest {
        audio_data: audio_data.into(),
        filename: path.clone(),
        model: provider.model.clone(),
        language: String::new(),
    };
    let started = std::time::Instant::now();
    let resp = client
        .transcribe(&CancellationToken::new(), &req)
        .await
        .expect("transcribe");
    println!(
        "{}",
        serde_json::json!({
            "file": path,
            "model": provider.model,
            "elapsed_ms": started.elapsed().as_millis(),
            "text": resp.text,
        })
    );
}
