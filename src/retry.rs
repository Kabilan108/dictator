use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use tokio_util::sync::CancellationToken;

use crate::audio::{TranscriptionRequest, WhisperClient};
use crate::storage::Db;
use crate::utils;

// The recorder can emit 64 MiB of PCM plus its 44-byte WAV header.
const MAX_AUDIO_BYTES: u64 = 64 * 1024 * 1024 + 44;

pub struct RetryOutput {
    pub text: String,
    /// Keep recovered text available even if history could not be saved.
    pub save_error: Option<anyhow::Error>,
}

#[derive(Debug)]
struct LoadedAudio {
    data: Vec<u8>,
    duration_ms: i64,
    path: PathBuf,
}

/// Retries the newest failed transcription, or a specific WAV recording.
///
/// Callers should print the text even when saving history failed, and send
/// diagnostics to stderr so transcripts remain safe to pipe elsewhere.
pub async fn run(audio_file: Option<PathBuf>) -> Result<RetryOutput> {
    // A process-held advisory lock prevents two terminals from submitting the
    // same pending recording. The OS releases it even if the process crashes.
    let _retry_lock = tokio::task::spawn_blocking(|| {
        let db = Db::new()?;
        acquire_retry_lock(&db.path().with_file_name("retry.lock"))
    })
    .await
    .context("failed to join retry lock task")??;
    let explicit = audio_file.is_some();
    let selected = match audio_file {
        Some(path) => (path, None),
        None => tokio::task::spawn_blocking(|| {
            let failed = Db::new()?.get_last_failed_transcription()?;
            failed
                .map(|entry| (PathBuf::from(entry.audio_path), Some(entry.duration_ms)))
                .ok_or_else(|| anyhow!("no failed transcription is available to retry; pass a WAV path to retry an older recording"))
        })
        .await
        .context("failed to join database lookup task")??,
    };

    let requested_path = selected.0;
    // Use the original database key when retrying a pending entry. Resolving a
    // symlink here would otherwise leave that entry behind after success.
    let pending_path = (!explicit).then(|| requested_path.to_string_lossy().into_owned());
    let recorded_duration_ms = selected.1;
    let loaded = tokio::task::spawn_blocking(move || read_wav(&requested_path))
        .await
        .context("failed to join audio reader task")??;
    let duration_ms = recorded_duration_ms.unwrap_or(loaded.duration_ms);
    let audio_path = pending_path.unwrap_or_else(|| loaded.path.to_string_lossy().into_owned());

    let config = tokio::task::spawn_blocking(utils::get_config)
        .await
        .context("failed to join configuration reader task")??;
    let provider = config
        .api
        .providers
        .get(&config.api.active_provider)
        .ok_or_else(|| {
            anyhow!(
                "active provider '{}' is not configured",
                config.api.active_provider
            )
        })?;
    let model = if provider.model.is_empty() {
        "distil-large-v3".to_owned()
    } else {
        provider.model.clone()
    };
    let client = WhisperClient::new(&config.api).context("failed to configure transcription")?;
    let request = TranscriptionRequest {
        audio_data: loaded.data.into(),
        filename: audio_path.clone(),
        model: model.clone(),
        language: String::new(),
    };

    let cancel = CancellationToken::new();
    let transcription = client.transcribe(&cancel, &request);
    tokio::pin!(transcription);
    let result = tokio::select! {
        result = &mut transcription => result,
        signal = tokio::signal::ctrl_c() => {
            cancel.cancel();
            signal.context("failed to listen for Ctrl-C")?;
            transcription.await
        }
    };
    let response = match result {
        Ok(response) => response,
        Err(err) => {
            let err = err.context(format!("failed to transcribe {}", loaded.path.display()));
            if explicit && !cancel.is_cancelled() {
                let failure_path = audio_path.clone();
                let saved = tokio::task::spawn_blocking(move || {
                    Db::new()?.save_failed_transcription(duration_ms, &failure_path)
                })
                .await
                .context("failed to join retry persistence task")
                .and_then(|result| result);
                if let Err(save_err) = saved {
                    return Err(err.context(format!(
                        "could not track this failure for retry: {save_err:#}; audio file preserved"
                    )));
                }
            }
            return Err(err);
        }
    };

    let transcript = response.text;
    let saved_text = transcript.clone();
    let saved = tokio::task::spawn_blocking(move || {
        Db::new()?.save_retried_transcript(duration_ms, &saved_text, &audio_path, &model)
    })
    .await
    .context("failed to join transcript persistence task")
    .and_then(|result| result)
    .context("transcription succeeded but saving it failed; audio file preserved");

    Ok(RetryOutput {
        text: transcript,
        save_error: saved.err(),
    })
}

fn acquire_retry_lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .context("failed to open retry lock")?;
    if !file.metadata()?.is_file() {
        bail!("retry lock is not a regular file");
    }
    // SAFETY: file owns a valid descriptor for the duration of this call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::WouldBlock {
            bail!("another transcription retry is already running");
        }
        return Err(err).context("failed to lock transcription retry");
    }
    Ok(file)
}

fn read_wav(path: &Path) -> Result<LoadedAudio> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| format!("failed to open audio file {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect audio file {}", path.display()))?;
    let file_type = metadata.file_type();
    if !file_type.is_file() {
        bail!("audio path is not a regular file: {}", path.display());
    }
    if metadata.len() > MAX_AUDIO_BYTES {
        bail!(
            "audio file {} exceeds the {} MiB limit",
            path.display(),
            MAX_AUDIO_BYTES / (1024 * 1024)
        );
    }

    let mut data = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.by_ref()
        .take(MAX_AUDIO_BYTES + 1)
        .read_to_end(&mut data)
        .with_context(|| format!("failed to read audio file {}", path.display()))?;
    if data.len() as u64 > MAX_AUDIO_BYTES {
        bail!(
            "audio file {} exceeds the {} MiB limit",
            path.display(),
            MAX_AUDIO_BYTES / (1024 * 1024)
        );
    }

    let duration_ms =
        wav_duration_ms(&data).with_context(|| format!("invalid WAV file {}", path.display()))?;
    let path = std::path::absolute(path)
        .with_context(|| format!("failed to resolve audio file {}", path.display()))?;
    Ok(LoadedAudio {
        data,
        duration_ms,
        path,
    })
}

fn wav_duration_ms(data: &[u8]) -> Result<i64> {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        bail!("missing RIFF/WAVE header");
    }
    let riff_len = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let end = riff_len
        .checked_add(8)
        .ok_or_else(|| anyhow!("RIFF length overflows"))?;
    if end > data.len() {
        bail!("truncated RIFF data");
    }

    let mut offset = 12usize;
    let mut byte_rate = None;
    let mut data_bytes = 0u64;
    while offset
        .checked_add(8)
        .is_some_and(|header_end| header_end <= end)
    {
        let chunk_len =
            u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let body_start = offset + 8;
        let body_end = body_start
            .checked_add(chunk_len)
            .ok_or_else(|| anyhow!("WAV chunk length overflows"))?;
        if body_end > end {
            bail!("truncated WAV chunk");
        }
        match &data[offset..offset + 4] {
            b"fmt " => {
                if chunk_len < 16 {
                    bail!("WAV format chunk is too short");
                }
                let format =
                    u16::from_le_bytes(data[body_start..body_start + 2].try_into().unwrap());
                if format != 1 && format != 3 {
                    bail!("unsupported WAV format {format}; expected PCM or IEEE float");
                }
                let rate =
                    u32::from_le_bytes(data[body_start + 8..body_start + 12].try_into().unwrap());
                if rate == 0 {
                    bail!("WAV byte rate is zero");
                }
                byte_rate = Some(rate as u64);
            }
            b"data" => {
                data_bytes = data_bytes
                    .checked_add(chunk_len as u64)
                    .ok_or_else(|| anyhow!("WAV audio length overflows"))?;
            }
            _ => {}
        }
        offset = body_end
            .checked_add(chunk_len & 1)
            .ok_or_else(|| anyhow!("WAV chunk padding overflows"))?;
    }

    let byte_rate = byte_rate.ok_or_else(|| anyhow!("missing WAV format chunk"))?;
    if data_bytes == 0 {
        bail!("WAV contains no audio data");
    }
    let millis = data_bytes
        .checked_mul(1000)
        .ok_or_else(|| anyhow!("WAV duration overflows"))?
        / byte_rate;
    i64::try_from(millis).map_err(|_| anyhow!("WAV duration does not fit in milliseconds"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm_wav(data_len: u32) -> Vec<u8> {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&16_000u32.to_le_bytes());
        wav.extend_from_slice(&32_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.resize(44 + data_len as usize, 0);
        wav
    }

    #[test]
    fn parses_pcm_wav_duration() {
        assert_eq!(wav_duration_ms(&pcm_wav(32_000)).unwrap(), 1000);
    }

    #[test]
    fn rejects_truncated_and_non_wav_data() {
        assert!(wav_duration_ms(b"not a wav").is_err());
        let mut wav = pcm_wav(32_000);
        wav.truncate(100);
        assert!(wav_duration_ms(&wav).is_err());
    }

    #[test]
    fn rejects_non_regular_inputs_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("audio.wav");
        let path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let err = read_wav(&fifo).unwrap_err();
        assert!(err.to_string().contains("not a regular file"));
    }

    #[test]
    fn retry_lock_rejects_concurrent_retry_and_releases_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retry.lock");
        let first = acquire_retry_lock(&path).unwrap();
        assert!(
            acquire_retry_lock(&path)
                .unwrap_err()
                .to_string()
                .contains("already running")
        );
        drop(first);
        assert!(acquire_retry_lock(&path).is_ok());
    }

    #[test]
    fn rejects_oversized_file_before_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.wav");
        File::create(&path)
            .unwrap()
            .set_len(MAX_AUDIO_BYTES + 1)
            .unwrap();
        assert!(read_wav(&path).unwrap_err().to_string().contains("exceeds"));
    }
}
