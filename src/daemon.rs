use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::audio::{LevelSample, Recorder, TranscriptionRequest, WhisperClient, write_audio_data};
use crate::ipc::{self, CommandHandler, DaemonState, Server, StatusData};
use crate::notifier::{DBusNotifier, Notifier};
use crate::storage::Db;
use crate::typing::Typer;
use crate::utils::{Config, NotificationMode};
use crate::visual::{self, Sink, SocketSink, StateEvent, StateValue};

pub const ERROR_DISPLAY_DURATION: Duration = Duration::from_secs(5);
pub const OSD_METER_UPDATE_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 30);

struct State {
    state: DaemonState,
    last_error: Option<String>,
    recording_duration: Duration,
    operation_cancel: Option<CancellationToken>,
}

struct MeterPublisher {
    tx: mpsc::Sender<LevelSample>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

pub struct Daemon {
    config: Config,
    recorder: Recorder,
    transcriber: WhisperClient,
    notifier: Notifier,
    visual_sink: RwLock<Sink>,
    typer: Typer,
    ipc_server: Mutex<Option<Arc<Server>>>,
    db: Mutex<Option<Db>>,

    state: RwLock<State>,
    start_time: Instant,
    stop_notify: Notify,

    meter: Mutex<Option<MeterPublisher>>,
    runtime: tokio::runtime::Handle,
}

impl Daemon {
    pub async fn new(cfg: Config) -> Result<Arc<Self>> {
        let recorder = Recorder::new(cfg.audio.clone())
            .map_err(|e| anyhow!("failed to create recorder: {e}"))?;

        let transcriber = WhisperClient::new(&cfg.api);

        let notifier = if cfg.notifications == NotificationMode::Off {
            Notifier::Noop
        } else {
            Notifier::DBus(
                DBusNotifier::new()
                    .await
                    .map_err(|e| anyhow!("failed to create notifier: {e}"))?,
            )
        };

        let typer = Typer::new().map_err(|e| anyhow!("failed to create typer: {e}"))?;

        let db = Db::new().map_err(|e| anyhow!("failed to create database: {e}"))?;

        Ok(Arc::new(Self {
            config: cfg,
            recorder,
            transcriber,
            notifier,
            visual_sink: RwLock::new(Sink::Noop),
            typer,
            ipc_server: Mutex::new(None),
            db: Mutex::new(Some(db)),
            state: RwLock::new(State {
                state: DaemonState::Idle,
                last_error: None,
                recording_duration: Duration::ZERO,
                operation_cancel: None,
            }),
            start_time: Instant::now(),
            stop_notify: Notify::new(),
            meter: Mutex::new(None),
            runtime: tokio::runtime::Handle::current(),
        }))
    }

    pub async fn run(self: &Arc<Self>) -> Result<()> {
        debug!("starting dictator daemon");

        let handler: Arc<dyn CommandHandler> = Arc::new(Arc::clone(self));
        let server = Arc::new(Server::new(handler));
        server
            .start()
            .await
            .map_err(|e| anyhow!("failed to start IPC server: {e}"))?;
        *self.ipc_server.lock().unwrap() = Some(Arc::clone(&server));

        let result = self.run_inner().await;

        if let Err(err) = server.stop().await {
            error!(err = %err, "failed to stop IPC server");
        }
        result
    }

    async fn run_inner(self: &Arc<Self>) -> Result<()> {
        let initial = self.state.read().unwrap().state;
        self.update_notification_state(initial)
            .await
            .map_err(|e| anyhow!("failed to show initial notification: {e}"))?;

        self.start_osd();

        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

        info!("dictator daemon started successfully");

        tokio::select! {
            _ = sigint.recv() => debug!(signal = "SIGINT", "received signal"),
            _ = sigterm.recv() => debug!(signal = "SIGTERM", "received signal"),
            _ = self.stop_notify.notified() => debug!("daemon stop requested"),
        }

        self.shutdown().await
    }

    pub fn stop(&self) {
        self.stop_notify.notify_one();
    }

    async fn shutdown(&self) -> Result<()> {
        debug!("shutting down daemon");

        if let Some(cancel) = self.state.read().unwrap().operation_cancel.clone() {
            cancel.cancel();
        }

        let mut last_err: Option<anyhow::Error> = None;

        if let Err(err) = self.recorder.close() {
            error!(err = %err, "failed to close recorder");
            last_err = Some(err);
        }

        if let Err(err) = self.notifier.close().await {
            error!(err = %err, "failed to close notifier");
            last_err = Some(err);
        }

        self.stop_osd_meter_publisher().await;
        let sink = self.visual_sink.read().unwrap().clone();
        if let Err(err) = sink.close().await {
            error!(err = %err, "failed to close OSD sink");
            last_err = Some(err);
        }

        // rusqlite closes on drop
        if let Some(db) = self.db.lock().unwrap().take() {
            drop(db);
        }

        info!("daemon shutdown complete");

        match last_err {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    // ---- state helpers -------------------------------------------------

    fn current_state(&self) -> DaemonState {
        self.state.read().unwrap().state
    }

    fn set_idle(&self) {
        let mut st = self.state.write().unwrap();
        st.state = DaemonState::Idle;
        st.last_error = None;
        st.recording_duration = Duration::ZERO;
    }

    fn spawn<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.runtime.spawn(fut);
    }

    // ---- notifications / OSD -------------------------------------------

    async fn update_notification_state(&self, state: DaemonState) -> Result<()> {
        match self.config.notifications {
            NotificationMode::All => self.notifier.update_state(state).await,
            NotificationMode::ErrorsOnly => {
                if state == DaemonState::Error {
                    self.notifier.update_state(state).await
                } else {
                    Ok(())
                }
            }
            NotificationMode::Off => Ok(()),
        }
    }

    async fn notify_state(&self, state: DaemonState) {
        if let Err(err) = self.update_notification_state(state).await {
            warn!(err = %err, "failed to update notification");
        }
    }

    fn osd_snapshot(&self) -> StateEvent {
        let (state, mut recording_duration, error_message) = {
            let st = self.state.read().unwrap();
            (
                st.state,
                st.recording_duration,
                st.last_error.clone().unwrap_or_default(),
            )
        };

        if state == DaemonState::Recording {
            recording_duration = self.recorder.get_recording_duration();
        }

        osd_state_event(state, recording_duration, &error_message)
    }

    fn publish_osd_state(
        &self,
        state: DaemonState,
        recording_duration: Option<Duration>,
        error_message: &str,
    ) {
        let duration = recording_duration.unwrap_or_default();
        let sink = self.visual_sink.read().unwrap().clone();
        sink.publish(osd_state_event(state, duration, error_message));
    }

    fn start_osd(self: &Arc<Self>) {
        if !self.config.enable_osd {
            return;
        }

        let snapshot_daemon = Arc::clone(self);
        let snapshot: visual::SnapshotFn = Arc::new(move || snapshot_daemon.osd_snapshot());

        let sink = match SocketSink::new(Some(snapshot)) {
            Ok(sink) => sink,
            Err(err) => {
                warn!(err = %err, "failed to start OSD event socket; continuing without OSD");
                return;
            }
        };

        *self.visual_sink.write().unwrap() = Sink::Socket(sink);
        self.start_osd_meter_publisher();

        let observer_daemon = Arc::clone(self);
        self.recorder.set_level_observer(
            Some(Arc::new(move |sample| {
                observer_daemon.enqueue_level_sample(sample)
            })),
            OSD_METER_UPDATE_INTERVAL,
        );
    }

    fn start_osd_meter_publisher(&self) {
        let cancel = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel::<LevelSample>(1);
        let sink = self.visual_sink.read().unwrap().clone();
        let task_cancel = cancel.clone();
        let task = self.runtime.spawn(async move {
            loop {
                tokio::select! {
                    _ = task_cancel.cancelled() => return,
                    sample = rx.recv() => match sample {
                        Some(sample) => sink.publish(visual::new_meter_event(sample.rms, sample.peak)),
                        None => return,
                    }
                }
            }
        });
        *self.meter.lock().unwrap() = Some(MeterPublisher { tx, cancel, task });
    }

    async fn stop_osd_meter_publisher(&self) {
        self.recorder.set_level_observer(None, Duration::ZERO);
        let publisher = self.meter.lock().unwrap().take();
        if let Some(publisher) = publisher {
            publisher.cancel.cancel();
            let _ = publisher.task.await;
        }
    }

    /// Called from the audio thread; keeps only the latest sample if the
    /// publisher is behind.
    fn enqueue_level_sample(&self, sample: LevelSample) {
        if self.current_state() != DaemonState::Recording {
            return;
        }
        let guard = self.meter.lock().unwrap();
        let Some(publisher) = guard.as_ref() else {
            return;
        };
        // if the channel is full the publisher task will drain it shortly; drop
        // this sample in favour of the next one (best-effort, like the Go version).
        let _ = publisher.tx.try_send(sample);
    }

    // ---- error handling ------------------------------------------------

    fn handle_error(self: &Arc<Self>, error_msg: String) {
        {
            let mut st = self.state.write().unwrap();
            st.state = DaemonState::Error;
            st.last_error = Some(error_msg.clone());
        }

        let daemon = Arc::clone(self);
        self.spawn(async move {
            if let Err(err) = daemon.update_notification_state(DaemonState::Error).await {
                warn!(err = %err, "failed to update error notification");
            }
            daemon.publish_osd_state(DaemonState::Error, None, &error_msg);

            // auto-return to idle after error display
            tokio::time::sleep(ERROR_DISPLAY_DURATION).await;
            {
                let mut st = daemon.state.write().unwrap();
                if st.state != DaemonState::Error {
                    return;
                }
                st.state = DaemonState::Idle;
                st.recording_duration = Duration::ZERO;
            }
            if let Err(err) = daemon.update_notification_state(DaemonState::Idle).await {
                warn!(err = %err, "failed to update notification after error");
            }
            daemon.publish_osd_state(DaemonState::Idle, None, "");
        });
    }

    // ---- transcription pipeline ---------------------------------------

    async fn transcribe_and_type(self: Arc<Self>) {
        let recording_duration = self.recorder.get_recording_duration();

        let (audio_data, audio_path) = match self.save_recording() {
            Ok(v) => v,
            Err(err) => {
                self.handle_error(format!("{}: {err:#}", ipc::ERR_RECORDING_FAILED));
                return;
            }
        };

        let cancel = self
            .state
            .read()
            .unwrap()
            .operation_cancel
            .clone()
            .unwrap_or_default();

        let text = match self.transcribe(&cancel, audio_data, &audio_path).await {
            Ok(text) => text,
            Err(err) => {
                if cancel.is_cancelled() {
                    debug!("transcription cancelled");
                    return;
                }
                self.handle_error(format!("{}: {err:#}", ipc::ERR_TRANSCRIPTION_FAILED));
                return;
            }
        };

        if self
            .type_and_save(&cancel, &text, recording_duration, &audio_path)
            .await
            .is_err()
        {
            return;
        }

        self.set_idle();
        self.notify_state(DaemonState::Idle).await;
        self.publish_osd_state(DaemonState::Idle, None, "");
    }

    fn save_recording(&self) -> Result<(Vec<u8>, String)> {
        let (audio_data, audio_path) = match self.recorder.stop() {
            Ok(v) => v,
            Err(err) => {
                error!(err = %err, "failed to stop recording");
                return Err(err);
            }
        };

        if let Err(err) = write_audio_data(&audio_path, &audio_data) {
            error!(err = %err, "failed to write audio file");
            return Err(err);
        }

        let audio_path = audio_path.to_string_lossy().into_owned();
        info!(filepath = %audio_path, "audio saved");
        Ok((audio_data, audio_path))
    }

    async fn transcribe(
        &self,
        cancel: &CancellationToken,
        audio_data: Vec<u8>,
        audio_path: &str,
    ) -> Result<String> {
        let model = self
            .config
            .api
            .providers
            .get(&self.config.api.active_provider)
            .map(|p| p.model.clone())
            .unwrap_or_default();

        let req = TranscriptionRequest {
            audio_data,
            filename: audio_path.to_string(),
            model,
            language: String::new(),
        };

        match self.transcriber.transcribe(cancel, &req).await {
            Ok(resp) => {
                info!("transcription complete");
                Ok(resp.text)
            }
            Err(err) => {
                error!(err = %err, "transcription failed");
                Err(err)
            }
        }
    }

    async fn type_and_save(
        self: &Arc<Self>,
        cancel: &CancellationToken,
        text: &str,
        duration: Duration,
        audio_path: &str,
    ) -> Result<()> {
        self.state.write().unwrap().state = DaemonState::Typing;
        self.notify_state(DaemonState::Typing).await;
        self.publish_osd_state(DaemonState::Typing, None, "");

        if let Err(err) = self.typer.type_text(cancel, text).await {
            if cancel.is_cancelled() {
                debug!("typing cancelled");
                self.set_idle();
                return Ok(());
            }
            error!(err = %err, "typing failed");
            self.handle_error(format!("{}: {err:#}", ipc::ERR_TYPING_FAILED));
            return Err(err);
        }

        info!("typing complete");

        let model = self
            .config
            .api
            .providers
            .get(&self.config.api.active_provider)
            .map(|p| p.model.clone())
            .unwrap_or_default();
        let duration_ms = duration.as_millis() as i64;

        let saved = match self.db.lock().unwrap().as_ref() {
            Some(db) => db.save_transcript(duration_ms, text, audio_path, &model),
            None => Err(anyhow!("database is closed")),
        };
        match saved {
            Ok(()) => debug!("transcript saved to database"),
            Err(err) => warn!(err = %err, "failed to save transcript to database"),
        }

        Ok(())
    }
}

// ---- CommandHandler ----------------------------------------------------

impl CommandHandler for Arc<Daemon> {
    fn handle_start(&self) -> Result<()> {
        {
            let st = self.state.read().unwrap();
            if st.state == DaemonState::Recording {
                bail!("{}", ipc::ERR_ALREADY_RECORDING);
            }
            if st.state != DaemonState::Idle {
                bail!("cannot start in current state: {}", st.state);
            }
        }

        debug!("starting recording");

        let cancel = CancellationToken::new();
        self.state.write().unwrap().operation_cancel = Some(cancel);

        if let Err(err) = self.recorder.start() {
            error!(err = %err, "failed to start recording");
            self.handle_error(format!("{}: {err:#}", ipc::ERR_RECORDING_FAILED));
            bail!("{}: {err:#}", ipc::ERR_RECORDING_FAILED);
        }

        {
            let mut st = self.state.write().unwrap();
            st.state = DaemonState::Recording;
            st.last_error = None;
            st.recording_duration = Duration::ZERO;
        }

        let daemon = Arc::clone(self);
        self.spawn(async move { daemon.notify_state(DaemonState::Recording).await });
        self.publish_osd_state(DaemonState::Recording, Some(Duration::ZERO), "");

        info!("recording started");
        Ok(())
    }

    fn handle_stop(&self) -> Result<()> {
        if self.current_state() != DaemonState::Recording {
            bail!("{}", ipc::ERR_NOT_RECORDING);
        }

        info!("stopping recording and starting transcription");

        let recording_duration = self.recorder.get_recording_duration();
        {
            let mut st = self.state.write().unwrap();
            st.state = DaemonState::Transcribing;
            st.recording_duration = recording_duration;
        }

        let daemon = Arc::clone(self);
        self.spawn(async move { daemon.notify_state(DaemonState::Transcribing).await });
        self.publish_osd_state(DaemonState::Transcribing, Some(recording_duration), "");

        let daemon = Arc::clone(self);
        self.spawn(daemon.transcribe_and_type());

        Ok(())
    }

    fn handle_toggle(&self) -> Result<()> {
        match self.current_state() {
            DaemonState::Idle => self.handle_start(),
            DaemonState::Recording => self.handle_stop(),
            other => bail!("cannot toggle in current state: {other}"),
        }
    }

    fn handle_cancel(&self) -> Result<()> {
        debug!("canceling current operation");

        let was_recording = {
            let mut st = self.state.write().unwrap();
            if let Some(cancel) = st.operation_cancel.as_ref() {
                cancel.cancel();
            }
            let was_recording = st.state == DaemonState::Recording;
            st.state = DaemonState::Idle;
            st.last_error = None;
            st.recording_duration = Duration::ZERO;
            was_recording
        };

        if was_recording && let Err(err) = self.recorder.stop() {
            error!(err = %err, "failed to stop recording during cancel");
        }

        let daemon = Arc::clone(self);
        self.spawn(async move { daemon.notify_state(DaemonState::Idle).await });
        self.publish_osd_state(DaemonState::Idle, None, "");

        info!("operation canceled");
        Ok(())
    }

    fn get_status(&self) -> StatusData {
        let st = self.state.read().unwrap();
        let recording_duration = if st.state == DaemonState::Recording {
            Some(self.recorder.get_recording_duration())
        } else {
            None
        };
        StatusData {
            state: st.state,
            recording_duration,
            last_error: st.last_error.clone(),
            uptime: self.start_time.elapsed(),
        }
    }
}

// ---- free helpers ------------------------------------------------------

/// Strips implementation details (paths, API responses) from an error before
/// it is shown on the OSD.
pub fn public_osd_error_message(error_message: &str) -> &'static str {
    if error_message.starts_with(ipc::ERR_RECORDING_FAILED) {
        ipc::ERR_RECORDING_FAILED
    } else if error_message.starts_with(ipc::ERR_TRANSCRIPTION_FAILED) {
        ipc::ERR_TRANSCRIPTION_FAILED
    } else if error_message.starts_with(ipc::ERR_TYPING_FAILED) {
        ipc::ERR_TYPING_FAILED
    } else {
        "dictation failed"
    }
}

fn osd_state_event(
    state: DaemonState,
    recording_duration: Duration,
    error_message: &str,
) -> StateEvent {
    match state {
        DaemonState::Recording => {
            visual::new_state_event(StateValue::Recording, Some(recording_duration), "")
        }
        DaemonState::Transcribing => {
            visual::new_state_event(StateValue::Transcribing, Some(recording_duration), "")
        }
        DaemonState::Typing => visual::new_state_event(StateValue::Typing, None, ""),
        DaemonState::Error => visual::new_state_event(
            StateValue::Error,
            None,
            public_osd_error_message(error_message),
        ),
        DaemonState::Idle => visual::new_state_event(StateValue::Idle, None, ""),
    }
}

pub fn not_running(err: anyhow::Error) -> anyhow::Error {
    anyhow!("can't connect to daemon: {err:#}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_osd_error_message_sanitizes_details() {
        let cases = [
            (
                format!("{}: /tmp/private/audio.wav", ipc::ERR_RECORDING_FAILED),
                ipc::ERR_RECORDING_FAILED,
            ),
            (
                format!(
                    "{}: API request failed with token abc123",
                    ipc::ERR_TRANSCRIPTION_FAILED
                ),
                ipc::ERR_TRANSCRIPTION_FAILED,
            ),
            (
                format!("{}: clipboard command failed", ipc::ERR_TYPING_FAILED),
                ipc::ERR_TYPING_FAILED,
            ),
            (
                "unexpected path /home/user/private".to_string(),
                "dictation failed",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(public_osd_error_message(&input), want);
        }
    }
}
