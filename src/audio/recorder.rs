use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{fs::OpenOptions, io::Write};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Local};
use tracing::{debug, info, warn};

use super::capture::{CaptureBackend, CaptureStream, LevelState, StreamOptions, on_capture_thread};
use super::devices::ResolvedMicrophone;
use super::timeline::{
    self, Captured, GATE_WINDOW, PREROLL, RING, TAIL, Verdict, duration_for, samples_for,
};
use super::warm::{Hold, WarmMic, WarmStatus};
use crate::utils::{AudioConfig, validate_audio_config};

pub use super::capture::{LevelObserver, LevelSample, calculate_level};

/// How long stop waits beyond the tail for late audio before finalizing.
const TAIL_GRACE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderState {
    Idle,
    Recording,
    Stopped,
}

/// Measurements of a finished recording.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureSummary {
    /// From the start command to the stop command.
    pub speech: Duration,
    /// Loudest 30 ms window before the tail, DC removed.
    pub loudest_dbfs: f64,
    pub preroll: Duration,
    pub tail: Duration,
    /// Everything kept: pre-roll, speech and tail.
    pub audio: Duration,
    /// Captured from the warm stream rather than a cold start.
    pub warm: bool,
}

pub enum StopOutcome {
    Recorded {
        wav: Vec<u8>,
        /// Wall-clock time of the start command, for naming the file.
        started: DateTime<Local>,
        summary: CaptureSummary,
    },
    /// Too short or silent; nothing was encoded or saved.
    Discarded {
        verdict: Verdict,
        summary: CaptureSummary,
    },
}

struct Session {
    stream: Arc<CaptureStream>,
    warm: bool,
    started_at: Instant,
    start_time: DateTime<Local>,
    error_taken: bool,
}

struct Inner {
    state: RecorderState,
    session: Option<Session>,
}

enum Choice {
    Warm(Arc<CaptureStream>),
    Cold(Option<ResolvedMicrophone>),
}

/// Records mono float32 audio from the highest-priority non-Bluetooth input
/// into memory and encodes it as 16-bit PCM WAV on stop.
pub struct Recorder {
    config: AudioConfig,
    options: StreamOptions,
    max_samples: usize,
    backend: CaptureBackend,
    control: Mutex<()>,
    inner: Mutex<Inner>,
    level: Arc<Mutex<LevelState>>,
    warm: WarmMic,
}

impl Recorder {
    pub fn new(config: AudioConfig) -> Result<Self> {
        Self::with_backend(config, CaptureBackend::system())
    }

    pub(crate) fn with_backend(config: AudioConfig, backend: CaptureBackend) -> Result<Self> {
        validate_audio_config(&config)?;
        let sample_rate = u32::try_from(config.sample_rate)
            .map_err(|_| anyhow!("audio sample rate does not fit u32"))?;
        let frames_per_block = usize::try_from(config.frames_per_block)
            .map_err(|_| anyhow!("audio frames per block does not fit usize"))?;
        let edges = samples_for(PREROLL + TAIL, sample_rate) as usize;
        let max_samples = max_samples(&config)?
            .checked_add(edges)
            .ok_or_else(|| anyhow!("maximum audio sample count overflows usize"))?;

        debug!(
            sr = config.sample_rate,
            channels = config.channels,
            bit_depth = config.bit_depth,
            "recorder initialized"
        );

        Ok(Self {
            config,
            options: StreamOptions {
                sample_rate,
                frames_per_block,
                ring_capacity: samples_for(RING, sample_rate) as usize,
            },
            max_samples,
            backend,
            control: Mutex::new(()),
            inner: Mutex::new(Inner {
                state: RecorderState::Idle,
                session: None,
            }),
            level: Arc::new(Mutex::new(LevelState {
                observer: None,
                interval: Duration::ZERO,
                last_level_at: None,
            })),
            warm: WarmMic::new(),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(config: AudioConfig) -> Self {
        Self::new(config).expect("valid test audio config")
    }

    /// Opens the warm microphone and keeps it open until released or closed.
    pub fn start_warm_microphone(&self) -> Result<()> {
        self.warm
            .start(self.backend.clone(), self.options, Arc::clone(&self.level))
            .map_err(|err| anyhow!("failed to start warm microphone: {err}"))
    }

    /// Closes the warm stream until [`Recorder::resume_microphone`] and
    /// returns once it is closed. A recording in progress keeps capturing
    /// until it stops.
    pub fn release_microphone(&self) {
        self.warm.hold(Hold::Released, true);
    }

    pub fn resume_microphone(&self) {
        self.warm.unhold(Hold::Released, true);
    }

    /// Closes the warm stream across a system suspend. Does not block.
    pub fn set_asleep(&self, asleep: bool) {
        if asleep {
            self.warm.hold(Hold::Asleep, false);
        } else {
            self.warm.unhold(Hold::Asleep, false);
        }
    }

    pub fn microphone_status(&self) -> WarmStatus {
        self.warm.status()
    }

    pub fn set_level_observer(&self, observer: Option<LevelObserver>, interval: Duration) {
        let mut level = self.level.lock().unwrap();
        level.observer = observer;
        level.interval = interval;
        level.last_level_at = None;
    }

    pub fn get_state(&self) -> RecorderState {
        self.inner.lock().unwrap().state
    }

    pub fn is_recording(&self) -> bool {
        self.get_state() == RecorderState::Recording
    }

    /// Starts a recording for a start command that arrived at `at`. A live
    /// warm stream contributes the audio buffered before `at`; otherwise a
    /// new capture opens and the recording begins with its first sample.
    pub fn start(&self, at: Instant) -> Result<()> {
        self.reject_capture_thread_control()?;
        let _control = self.control.lock().unwrap();
        if self.inner.lock().unwrap().state == RecorderState::Recording {
            bail!("recorder is already recording");
        }

        debug!("starting audio recording");
        let (stream, warm) = match self.choose_stream() {
            Choice::Warm(stream) => (stream, true),
            Choice::Cold(microphone) => (self.open_cold(microphone)?, false),
        };
        let preroll = {
            let mut timeline = stream.shared().timeline.lock().unwrap();
            if warm {
                let limit = samples_for(PREROLL, self.options.sample_rate);
                timeline.begin_session(at, limit, self.max_samples)
            } else {
                timeline.begin_session_at_stream_start(self.max_samples);
                0
            }
        };
        self.level.lock().unwrap().last_level_at = None;

        let microphone = stream.microphone().name.clone();
        let behind = chrono::Duration::from_std(Instant::now().saturating_duration_since(at))
            .unwrap_or_default();
        let mut inner = self.inner.lock().unwrap();
        inner.session = Some(Session {
            stream,
            warm,
            started_at: at,
            start_time: Local::now() - behind,
            error_taken: false,
        });
        inner.state = RecorderState::Recording;
        drop(inner);

        info!(
            warm,
            microphone,
            preroll_ms = duration_for(preroll, self.options.sample_rate).as_millis() as u64,
            max_duration_min = self.config.max_duration_min,
            "recording started"
        );
        Ok(())
    }

    fn choose_stream(&self) -> Choice {
        let Some(warm) = self.warm.acquire(Instant::now()) else {
            return Choice::Cold(None);
        };
        if (self.backend.fingerprint)() == warm.fingerprint {
            return Choice::Warm(warm.stream);
        }
        // Priorities changed since the warm stream was resolved.
        self.warm.resolve_soon();
        match (self.backend.resolve)() {
            Ok(resolution)
                if resolution.microphone.source_name != warm.stream.microphone().source_name =>
            {
                Choice::Cold(Some(resolution.microphone))
            }
            Ok(_) => Choice::Warm(warm.stream),
            Err(err) => {
                // The live stream is a validated non-Bluetooth input; a
                // failed lookup should not cost the recording.
                warn!(err = %format!("{err:#}"), "microphone lookup failed; using the warm microphone");
                Choice::Warm(warm.stream)
            }
        }
    }

    fn open_cold(&self, microphone: Option<ResolvedMicrophone>) -> Result<Arc<CaptureStream>> {
        let microphone = match microphone {
            Some(microphone) => microphone,
            None => (self.backend.resolve)()?.microphone,
        };
        CaptureStream::open(
            &self.backend.capture,
            microphone,
            self.options,
            Arc::clone(&self.level),
            None,
        )
    }

    /// Stops a recording for a stop command that arrived at `at`, keeping
    /// [`TAIL`] of audio after it. Too short or silent recordings are
    /// discarded without encoding.
    pub fn stop(&self, at: Instant) -> Result<StopOutcome> {
        self.reject_capture_thread_control()?;
        let _control = self.control.lock().unwrap();
        let session = self.take_session()?;
        let captured = self.finish(&session, at);
        let sample_rate = self.options.sample_rate;
        let window = samples_for(GATE_WINDOW, sample_rate) as usize;
        let summary = CaptureSummary {
            speech: at.saturating_duration_since(session.started_at),
            loudest_dbfs: timeline::loudest_window_dbfs(
                &captured.samples[..captured.speech_end],
                window,
            ),
            preroll: duration_for(captured.preroll, sample_rate),
            tail: duration_for(captured.tail(), sample_rate),
            audio: duration_for(captured.samples.len(), sample_rate),
            warm: session.warm,
        };
        let start_time = session.start_time;
        drop(session);

        let verdict = timeline::gate(summary.speech, summary.loudest_dbfs);
        if verdict != Verdict::Keep {
            return Ok(StopOutcome::Discarded { verdict, summary });
        }
        let wav = self
            .encode_samples_to_wav(captured.samples)
            .map_err(|e| anyhow!("failed to encode to WAV: {e}"))?;
        info!(bytes = wav.len(), "recording stopped");
        Ok(StopOutcome::Recorded {
            wav,
            started: start_time,
            summary,
        })
    }

    fn finish(&self, session: &Session, at: Instant) -> Captured {
        let shared = session.stream.shared();
        let tail = samples_for(TAIL, self.options.sample_rate);
        let deadline = Instant::now().max(at + TAIL) + TAIL_GRACE;
        let mut timeline = shared.timeline.lock().unwrap();
        if let Some(end) = timeline.end_session(at, tail) {
            while !timeline.reached(end) {
                let now = Instant::now();
                if now >= deadline {
                    warn!("audio after the stop command did not arrive in time");
                    break;
                }
                timeline = shared
                    .progress
                    .wait_timeout(timeline, deadline - now)
                    .unwrap()
                    .0;
            }
        }
        timeline.take_session().unwrap_or_default()
    }

    fn take_session(&self) -> Result<Session> {
        let mut inner = self.inner.lock().unwrap();
        let Some(session) = inner.session.take() else {
            bail!("recorder is not recording");
        };
        inner.state = RecorderState::Stopped;
        Ok(session)
    }

    /// Stops the recording and discards its audio without encoding.
    pub fn cancel(&self) -> Result<()> {
        self.reject_capture_thread_control()?;
        let _control = self.control.lock().unwrap();
        let session = self.take_session()?;
        session
            .stream
            .shared()
            .timeline
            .lock()
            .unwrap()
            .take_session();
        Ok(())
    }

    pub fn get_recording_duration(&self) -> Duration {
        let inner = self.inner.lock().unwrap();
        match (&inner.state, &inner.session) {
            (RecorderState::Recording, Some(session)) => session.started_at.elapsed(),
            _ => Duration::ZERO,
        }
    }

    /// Time from the start command to `at`.
    pub fn duration_until(&self, at: Instant) -> Duration {
        let inner = self.inner.lock().unwrap();
        inner.session.as_ref().map_or(Duration::ZERO, |session| {
            at.saturating_duration_since(session.started_at)
        })
    }

    /// Takes the error that ended the active recording's capture, once.
    pub fn take_error(&self) -> Option<String> {
        let mut inner = self.inner.lock().unwrap();
        let session = inner.session.as_mut()?;
        if session.error_taken {
            return None;
        }
        let error = session
            .stream
            .shared()
            .timeline
            .lock()
            .unwrap()
            .error()?
            .to_owned();
        session.error_taken = true;
        Some(error)
    }

    pub fn encode_to_wav(&self, raw: &[u8]) -> Result<Vec<u8>> {
        if raw.is_empty() {
            bail!("no audio data to encode");
        }

        let mut out = self.wav_header(raw.len())?;
        out.extend_from_slice(raw);
        Ok(out)
    }

    fn encode_samples_to_wav(&self, samples: Vec<f32>) -> Result<Vec<u8>> {
        if samples.is_empty() {
            bail!("no audio data to encode");
        }
        let data_size = samples
            .len()
            .checked_mul(2)
            .ok_or_else(|| anyhow!("audio data exceeds the WAV size limit"))?;
        let mut out = self.wav_header(data_size)?;
        for sample in samples {
            let int_sample = (sample * 32767.0) as i16;
            out.extend_from_slice(&int_sample.to_le_bytes());
        }
        Ok(out)
    }

    fn wav_header(&self, data_len: usize) -> Result<Vec<u8>> {
        let num_channels = u16::try_from(self.config.channels)
            .map_err(|_| anyhow!("audio channel count does not fit u16"))?;
        let sample_rate = u32::try_from(self.config.sample_rate)
            .map_err(|_| anyhow!("audio sample rate does not fit u32"))?;
        let bits_per_sample = u16::try_from(self.config.bit_depth)
            .map_err(|_| anyhow!("audio bit depth does not fit u16"))?;
        let byte_rate = sample_rate
            .checked_mul(u32::from(num_channels))
            .and_then(|rate| rate.checked_mul(u32::from(bits_per_sample)))
            .map(|rate| rate / 8)
            .ok_or_else(|| anyhow!("WAV byte rate overflows u32"))?;
        let block_align = num_channels
            .checked_mul(bits_per_sample)
            .map(|align| align / 8)
            .ok_or_else(|| anyhow!("WAV block alignment overflows u16"))?;
        let data_size = u32::try_from(data_len)
            .map_err(|_| anyhow!("audio data exceeds the WAV size limit"))?;
        let riff_size = data_size
            .checked_add(36)
            .ok_or_else(|| anyhow!("audio data exceeds the WAV size limit"))?;

        let capacity = 44usize
            .checked_add(data_len)
            .ok_or_else(|| anyhow!("audio data exceeds the WAV size limit"))?;
        let mut out = Vec::with_capacity(capacity);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&num_channels.to_le_bytes());
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&byte_rate.to_le_bytes());
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&bits_per_sample.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_size.to_le_bytes());
        Ok(out)
    }

    /// Discards any active recording and closes the warm microphone.
    pub fn close(&self) -> Result<()> {
        self.reject_capture_thread_control()?;
        let _control = self.control.lock().unwrap();
        if let Ok(session) = self.take_session() {
            warn!("recorder still active during close, discarding recording");
            session
                .stream
                .shared()
                .timeline
                .lock()
                .unwrap()
                .take_session();
        }
        self.warm.shutdown();
        debug!("audio recorder closed");
        Ok(())
    }

    fn reject_capture_thread_control(&self) -> Result<()> {
        if on_capture_thread() {
            bail!("cannot control recorder from its audio stream thread");
        }
        Ok(())
    }
}

fn max_samples(config: &AudioConfig) -> Result<usize> {
    let sample_rate = usize::try_from(config.sample_rate)
        .map_err(|_| anyhow!("audio sample rate does not fit usize"))?;
    let minutes = usize::try_from(config.max_duration_min)
        .map_err(|_| anyhow!("audio max duration does not fit usize"))?;
    sample_rate
        .checked_mul(60)
        .and_then(|per_minute| per_minute.checked_mul(minutes))
        .ok_or_else(|| anyhow!("maximum audio sample count overflows usize"))
}

/// Converts an owned float32 buffer to little-endian int16 PCM.
#[cfg(test)]
fn samples_to_pcm(samples: Vec<f32>) -> Vec<u8> {
    let mut data = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        // convert float32 (-1.0 to 1.0) to int16 (-32768 to 32767)
        let int_sample = (sample * 32767.0) as i16;
        data.extend_from_slice(&int_sample.to_le_bytes());
    }
    data
}

pub fn write_audio_data(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    if !dir.exists() {
        bail!("directory does not exist: {}", dir.display());
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);

    let mut file = options
        .open(path)
        .map_err(|e| anyhow!("failed to open audio file: {e}"))?;
    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|e| anyhow!("failed to restrict audio file permissions: {e}"))?;
    file.write_all(data)
        .map_err(|e| anyhow!("failed to write audio data: {e}"))
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::mpsc;

    use super::*;
    use crate::audio::capture::CaptureCommand;
    use crate::audio::capture::tests::{LIVE_SILENCE, microphone, shell_capture, wait_for};
    use crate::audio::devices::{Fingerprint, Resolution};

    fn cfg() -> AudioConfig {
        AudioConfig {
            sample_rate: 16000,
            channels: 1,
            bit_depth: 16,
            frames_per_block: 1024,
            max_duration_min: 5,
        }
    }

    /// A fake parec that writes 100 ms of a -6 dBFS square wave every 100 ms.
    fn loud_capture() -> (tempfile::TempDir, CaptureCommand) {
        let directory = tempfile::tempdir().unwrap();
        let chunk = directory.path().join("chunk.raw");
        let samples: Vec<u8> = (0..1600)
            .flat_map(|index| (if index % 2 == 0 { 0.5f32 } else { -0.5f32 }).to_le_bytes())
            .collect();
        std::fs::write(&chunk, samples).unwrap();
        let command = Arc::new(move |_: &str, _: u32| {
            let mut command = Command::new("/bin/sh");
            command
                .args(["-c", "while :; do cat \"$0\" || exit 1; sleep 0.1; done"])
                .arg(&chunk);
            command
        });
        (directory, command)
    }

    fn resolution(source_name: &str, fingerprint: Option<Fingerprint>) -> Resolution {
        Resolution {
            microphone: microphone(source_name),
            fingerprint,
        }
    }

    fn backend(capture: CaptureCommand) -> CaptureBackend {
        CaptureBackend {
            resolve: Arc::new(|| Ok(resolution("fake", None))),
            fingerprint: Arc::new(|| None),
            capture,
            subscribe: None,
        }
    }

    /// Returns once the warm stream has buffered at least `history` of audio.
    fn warm_recorder_with(capture: CaptureCommand, history: Duration) -> Recorder {
        let recorder = Recorder::with_backend(cfg(), backend(capture)).unwrap();
        recorder.start_warm_microphone().unwrap();
        let needed = samples_for(history, 16_000);
        wait_for(|| {
            recorder
                .warm
                .acquire(Instant::now())
                .is_some_and(|warm| warm.stream.shared().timeline.lock().unwrap().total() >= needed)
        });
        recorder
    }

    fn warm_recorder(capture: CaptureCommand) -> Recorder {
        warm_recorder_with(capture, Duration::ZERO)
    }

    #[test]
    fn warm_recording_includes_preroll_and_tail() {
        let (_directory, capture) = loud_capture();
        let recorder = warm_recorder_with(capture, Duration::from_millis(700));

        let started = Instant::now();
        recorder.start(started).unwrap();
        std::thread::sleep(Duration::from_millis(600));
        let outcome = recorder.stop(started + Duration::from_millis(600)).unwrap();

        let StopOutcome::Recorded { wav, summary, .. } = outcome else {
            panic!("loud recording was discarded");
        };
        assert!(summary.warm);
        assert_eq!(summary.preroll, PREROLL);
        assert_eq!(summary.speech, Duration::from_millis(600));
        assert_eq!(summary.tail, TAIL);
        // The fake writes 100 ms bursts, so the speech span is only as exact
        // as its pacing; the timeline tests check exact slicing.
        let edges = PREROLL + TAIL;
        assert!(
            summary.audio >= edges + Duration::from_millis(300),
            "{summary:?}"
        );
        assert!(
            summary.audio <= edges + Duration::from_millis(800),
            "{summary:?}"
        );
        assert!((summary.loudest_dbfs - -6.02).abs() < 0.1, "{summary:?}");
        let pcm_bytes = wav.len() - 44;
        assert_eq!(pcm_bytes, samples_for(summary.audio, 16_000) as usize * 2);
        recorder.close().unwrap();
    }

    #[test]
    fn silent_and_short_recordings_are_discarded() {
        let recorder = warm_recorder_with(shell_capture(LIVE_SILENCE), PREROLL);
        let started = Instant::now();
        recorder.start(started).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        let outcome = recorder.stop(started + Duration::from_millis(400)).unwrap();
        assert!(matches!(
            outcome,
            StopOutcome::Discarded {
                verdict: Verdict::Silent,
                ..
            }
        ));
        recorder.close().unwrap();

        let (_directory, capture) = loud_capture();
        let recorder = warm_recorder(capture);
        let started = Instant::now();
        recorder.start(started).unwrap();
        let outcome = recorder.stop(started + Duration::from_millis(120)).unwrap();
        let StopOutcome::Discarded { verdict, summary } = outcome else {
            panic!("short tap was kept");
        };
        assert_eq!(verdict, Verdict::TooShort);
        assert_eq!(summary.speech, Duration::from_millis(120));
        recorder.close().unwrap();
    }

    #[test]
    fn released_or_unstarted_warm_microphone_falls_back_to_a_cold_start() {
        let (_directory, capture) = loud_capture();
        let cold = Recorder::with_backend(cfg(), backend(Arc::clone(&capture))).unwrap();
        let started = Instant::now();
        cold.start(started).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let StopOutcome::Recorded { summary, .. } = cold.stop(Instant::now()).unwrap() else {
            panic!("cold recording was discarded");
        };
        assert!(!summary.warm);
        assert_eq!(summary.preroll, Duration::ZERO);

        let recorder = warm_recorder(capture);
        recorder.release_microphone();
        assert!(recorder.warm.acquire(Instant::now()).is_none());
        recorder.start(Instant::now()).unwrap();
        let session_warm = recorder
            .inner
            .lock()
            .unwrap()
            .session
            .as_ref()
            .unwrap()
            .warm;
        assert!(!session_warm);
        recorder.cancel().unwrap();
        assert_eq!(recorder.get_state(), RecorderState::Stopped);
        recorder.close().unwrap();
    }

    #[test]
    fn changed_priorities_reresolve_before_using_the_warm_stream() {
        let (_directory, capture) = loud_capture();
        let generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let selected = Arc::new(Mutex::new("fake"));
        let fingerprint_generation = Arc::clone(&generation);
        let resolve_selected = Arc::clone(&selected);
        let recorder = Recorder::with_backend(
            cfg(),
            CaptureBackend {
                resolve: Arc::new(move || Ok(resolution(*resolve_selected.lock().unwrap(), None))),
                // Each generation stands for one version of the preferences file.
                fingerprint: Arc::new(move || {
                    let generation =
                        fingerprint_generation.load(std::sync::atomic::Ordering::SeqCst);
                    (generation > 0).then(|| Fingerprint::for_test(generation))
                }),
                capture,
                subscribe: None,
            },
        )
        .unwrap();
        recorder.start_warm_microphone().unwrap();
        wait_for(|| recorder.warm.acquire(Instant::now()).is_some());

        // Same microphone after the change: the warm stream is still used.
        generation.store(1, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(recorder.choose_stream(), Choice::Warm(_)));

        // A different microphone after the change: cold start on the new one.
        *selected.lock().unwrap() = "usb";
        generation.store(2, std::sync::atomic::Ordering::SeqCst);
        match recorder.choose_stream() {
            Choice::Cold(Some(microphone)) => assert_eq!(microphone.source_name, "usb"),
            _ => panic!("expected a cold start on the newly selected microphone"),
        }
        recorder.close().unwrap();
    }

    /// A backend whose preferences file stays at version 1 and whose
    /// resolver succeeds only `successes` times.
    fn counting_backend(
        capture: CaptureCommand,
        successes: usize,
    ) -> (CaptureBackend, Arc<Mutex<usize>>) {
        let calls = Arc::new(Mutex::new(0));
        let counted = Arc::clone(&calls);
        let backend = CaptureBackend {
            resolve: Arc::new(move || {
                let mut calls = counted.lock().unwrap();
                *calls += 1;
                if *calls > successes {
                    anyhow::bail!("temporary pactl discovery failure");
                }
                Ok(resolution("fake", Some(Fingerprint::for_test(1))))
            }),
            fingerprint: Arc::new(|| Some(Fingerprint::for_test(1))),
            capture,
            subscribe: None,
        };
        (backend, calls)
    }

    #[test]
    fn unchanged_preferences_use_the_warm_stream_without_a_lookup() {
        let (_directory, capture) = loud_capture();
        let (backend, calls) = counting_backend(capture, 1);
        let recorder = Recorder::with_backend(cfg(), backend).unwrap();
        recorder.start_warm_microphone().unwrap();
        wait_for(|| recorder.warm.acquire(Instant::now()).is_some());

        // The resolver's own write (version 1) is what the warm stream holds.
        assert!(matches!(recorder.choose_stream(), Choice::Warm(_)));
        assert_eq!(*calls.lock().unwrap(), 1);
        recorder.close().unwrap();
    }

    #[test]
    fn failed_lookup_after_a_priority_change_keeps_the_warm_stream() {
        let (_directory, capture) = loud_capture();
        let (mut backend, calls) = counting_backend(capture, 1);
        backend.fingerprint = Arc::new(|| Some(Fingerprint::for_test(2)));
        let recorder = Recorder::with_backend(cfg(), backend).unwrap();
        recorder.start_warm_microphone().unwrap();
        wait_for(|| recorder.warm.acquire(Instant::now()).is_some());

        assert!(matches!(recorder.choose_stream(), Choice::Warm(_)));
        // The start path looked up once; the supervisor may follow after its
        // debounce.
        assert!(*calls.lock().unwrap() >= 2);
        recorder.close().unwrap();
    }

    #[test]
    fn capture_failure_during_recording_is_reported_once() {
        let recorder = Recorder::with_backend(
            cfg(),
            backend(shell_capture(
                "sleep 0.2; head -c 6400 /dev/zero; echo 'device unplugged' >&2; exit 1",
            )),
        )
        .unwrap();
        recorder.start(Instant::now()).unwrap();
        let mut error = None;
        wait_for(|| {
            error = recorder.take_error();
            error.is_some()
        });
        assert!(error.unwrap().contains("device unplugged"));
        assert!(recorder.take_error().is_none());
        recorder.cancel().unwrap();
    }

    #[test]
    fn observer_cannot_reentrantly_stop_or_discard_recording() {
        let (_directory, capture) = loud_capture();
        let recorder = Arc::new(Recorder::with_backend(cfg(), backend(capture)).unwrap());
        let (result_tx, result_rx) = mpsc::channel();
        let weak_recorder = Arc::downgrade(&recorder);
        let result_tx = Mutex::new(Some(result_tx));
        recorder.set_level_observer(
            Some(Arc::new(move |_| {
                let Some(result_tx) = result_tx.lock().unwrap().take() else {
                    return;
                };
                let recorder = weak_recorder.upgrade().expect("recorder remains alive");
                let stop = recorder.stop(Instant::now()).err().unwrap().to_string();
                let cancel = recorder.cancel().unwrap_err().to_string();
                result_tx.send((stop, cancel)).unwrap();
            })),
            Duration::ZERO,
        );

        recorder.start(Instant::now()).unwrap();
        let (stop, cancel) = result_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        assert_eq!(stop, "cannot control recorder from its audio stream thread");
        assert_eq!(cancel, stop);
        assert!(recorder.is_recording());
        recorder.set_level_observer(None, Duration::ZERO);
        recorder.cancel().unwrap();
    }

    #[test]
    fn dropping_active_recorder_stops_its_capture() {
        let recorder = Recorder::with_backend(cfg(), backend(shell_capture(LIVE_SILENCE))).unwrap();
        recorder.start(Instant::now()).unwrap();
        let shared = Arc::clone(
            recorder
                .inner
                .lock()
                .unwrap()
                .session
                .as_ref()
                .unwrap()
                .stream
                .shared(),
        );
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _inner = recorder.inner.lock().unwrap();
            panic!("poison recorder state");
        }));

        drop(recorder);

        assert!(shared.timeline.lock().unwrap().is_ended());
    }

    #[test]
    fn wav_header_matches_go_layout() {
        let recorder = Recorder::for_test(cfg());
        let wav = recorder.encode_to_wav(&[1, 0, 2, 0]).unwrap();
        assert_eq!(wav.len(), 48);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 40);
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(u32::from_le_bytes(wav[16..20].try_into().unwrap()), 16);
        assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16000);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 32000);
        assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
        assert_eq!(&wav[44..], &[1, 0, 2, 0]);
        assert!(recorder.encode_to_wav(&[]).is_err());
    }

    #[test]
    fn pcm_conversion() {
        let data = samples_to_pcm(vec![0.0, 1.0, -1.0]);
        assert_eq!(data, vec![0, 0, 0xFF, 0x7F, 0x01, 0x80]);
    }

    #[test]
    fn recording_buffer_is_not_reserved_up_front() {
        let mut config = cfg();
        config.max_duration_min = 15;
        let recorder =
            Recorder::with_backend(config, backend(shell_capture(LIVE_SILENCE))).unwrap();
        assert_eq!(recorder.max_samples, 15 * 60 * 16_000 + 7_200 + 3_200);
        recorder.start(Instant::now()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let inner = recorder.inner.lock().unwrap();
        let shared = Arc::clone(inner.session.as_ref().unwrap().stream.shared());
        drop(inner);
        let capacity = shared.timeline.lock().unwrap().session_capacity();
        assert!(capacity < 16_000 * 10, "reserved {capacity} samples");
        recorder.cancel().unwrap();
    }

    #[test]
    #[ignore = "live microphone probe"]
    fn live_capture_start_cancel_probe() {
        let recorder = Recorder::for_test(cfg());
        recorder.start(Instant::now()).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        let capture_error = recorder.take_error();
        let (StopOutcome::Discarded { summary, .. } | StopOutcome::Recorded { summary, .. }) =
            recorder.stop(Instant::now()).unwrap();
        assert!(capture_error.is_none(), "capture failed: {capture_error:?}");
        assert!(summary.audio > Duration::from_millis(500), "{summary:?}");
    }
}
