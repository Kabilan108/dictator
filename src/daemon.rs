use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use tokio::sync::{Notify, watch};
use tokio::task::{JoinHandle, JoinSet};
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
    operation: Option<Operation>,
    next_operation_id: u64,
    revision: u64,
}

impl State {
    fn matches_operation(&self, operation_id: u64) -> bool {
        self.operation
            .as_ref()
            .is_some_and(|operation| operation.id == operation_id)
    }

    fn claim_recording(
        &mut self,
        expected_operation_id: Option<u64>,
        recording_duration: Duration,
    ) -> Option<(Operation, u64)> {
        if self.state != DaemonState::Recording {
            return None;
        }
        let operation = self.operation.as_ref()?.clone();
        if expected_operation_id.is_some_and(|expected| expected != operation.id) {
            return None;
        }
        self.state = DaemonState::Transcribing;
        self.recording_duration = recording_duration;
        self.revision = self.revision.wrapping_add(1);
        Some((operation, self.revision))
    }

    fn transition_operation(&mut self, operation_id: u64, state: DaemonState) -> Option<u64> {
        if !self.matches_operation(operation_id) {
            return None;
        }
        self.state = state;
        self.revision = self.revision.wrapping_add(1);
        Some(self.revision)
    }

    fn finish_operation(&mut self, operation_id: u64) -> Option<u64> {
        if !self.matches_operation(operation_id) {
            return None;
        }
        self.state = DaemonState::Idle;
        self.last_error = None;
        self.recording_duration = Duration::ZERO;
        self.operation = None;
        self.revision = self.revision.wrapping_add(1);
        Some(self.revision)
    }

    fn reset_error(&mut self, error_revision: u64) -> Option<u64> {
        if self.state != DaemonState::Error || self.revision != error_revision {
            return None;
        }
        self.state = DaemonState::Idle;
        self.last_error = None;
        self.recording_duration = Duration::ZERO;
        self.revision = self.revision.wrapping_add(1);
        Some(self.revision)
    }
}

#[derive(Clone)]
struct Operation {
    id: u64,
    cancel: CancellationToken,
}

struct PipelineTask {
    operation_id: u64,
    task: JoinHandle<()>,
}

#[derive(Clone, Copy)]
struct NotificationUpdate {
    state: DaemonState,
    revision: u64,
}

struct MeterPublisher {
    tx: watch::Sender<LevelSample>,
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
    db: Mutex<Option<Db>>,

    state: RwLock<State>,
    command_lock: Mutex<()>,
    start_time: Instant,
    stop_notify: Notify,
    shutdown_cancel: CancellationToken,
    notification_tx: watch::Sender<NotificationUpdate>,

    meter: Mutex<Option<MeterPublisher>>,
    recording_timeout: Mutex<Option<(u64, CancellationToken)>>,
    error_timeout: Mutex<Option<(u64, CancellationToken)>>,
    pipeline: Mutex<Option<PipelineTask>>,
    background_tasks: Mutex<JoinSet<()>>,
    runtime: tokio::runtime::Handle,
}

impl Daemon {
    pub async fn new(cfg: Config) -> Result<Arc<Self>> {
        let recorder = Recorder::new(cfg.audio.clone())
            .map_err(|e| anyhow!("failed to create recorder: {e}"))?;

        let transcriber = WhisperClient::new(&cfg.api)
            .map_err(|e| anyhow!("failed to create transcription client: {e}"))?;

        let notifier = if cfg.notifications == NotificationMode::Off {
            Notifier::Noop
        } else {
            Notifier::DBus(
                DBusNotifier::new()
                    .await
                    .map_err(|e| anyhow!("failed to create notifier: {e}"))?,
            )
        };

        let typer = Typer::new()
            .map_err(|e| anyhow!("failed to create typer: {e}"))?
            .with_config(cfg.typing.clone());

        let db = tokio::task::spawn_blocking(Db::new)
            .await
            .map_err(|e| anyhow!("failed to create database: {e}"))?
            .map_err(|e| anyhow!("failed to create database: {e}"))?;

        let (notification_tx, _) = watch::channel(NotificationUpdate {
            state: DaemonState::Idle,
            revision: 0,
        });

        Ok(Arc::new(Self {
            config: cfg,
            recorder,
            transcriber,
            notifier,
            visual_sink: RwLock::new(Sink::Noop),
            typer,
            db: Mutex::new(Some(db)),
            state: RwLock::new(State {
                state: DaemonState::Idle,
                last_error: None,
                recording_duration: Duration::ZERO,
                operation: None,
                next_operation_id: 0,
                revision: 0,
            }),
            command_lock: Mutex::new(()),
            start_time: Instant::now(),
            stop_notify: Notify::new(),
            shutdown_cancel: CancellationToken::new(),
            notification_tx,
            meter: Mutex::new(None),
            recording_timeout: Mutex::new(None),
            error_timeout: Mutex::new(None),
            pipeline: Mutex::new(None),
            background_tasks: Mutex::new(JoinSet::new()),
            runtime: tokio::runtime::Handle::current(),
        }))
    }

    pub async fn run(self: &Arc<Self>) -> Result<()> {
        debug!("starting dictator daemon");

        let initial = self.state.read().unwrap().state;
        self.update_notification_state(initial)
            .await
            .map_err(|e| anyhow!("failed to show initial notification: {e}"))?;
        self.start_notification_worker();
        self.start_osd();

        let handler: Arc<dyn CommandHandler> = Arc::new(Arc::clone(self));
        let server = Arc::new(Server::new(handler));
        if let Err(err) = server.start().await {
            self.shutdown_cancel.cancel();
            let _ = self.shutdown().await;
            return Err(anyhow!("failed to start IPC server: {err}"));
        }

        let result = self.run_inner().await;

        self.shutdown_cancel.cancel();
        let stop_result = server
            .stop()
            .await
            .map_err(|err| anyhow!("failed to stop IPC server: {err}"));
        if let Err(err) = &stop_result {
            error!(err = %err, "failed to stop IPC server");
        }

        let shutdown_result = self.shutdown().await;
        result.and(stop_result).and(shutdown_result)
    }

    async fn run_inner(self: &Arc<Self>) -> Result<()> {
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

        info!("dictator daemon started successfully");

        tokio::select! {
            _ = sigint.recv() => debug!(signal = "SIGINT", "received signal"),
            _ = sigterm.recv() => debug!(signal = "SIGTERM", "received signal"),
            _ = self.stop_notify.notified() => debug!("daemon stop requested"),
        }

        Ok(())
    }

    pub fn stop(&self) {
        self.stop_notify.notify_one();
    }

    async fn shutdown(self: &Arc<Self>) -> Result<()> {
        debug!("shutting down daemon");

        self.shutdown_cancel.cancel();
        self.cancel_recording_timeout(None);
        self.cancel_error_timeout(None);
        let mut last_err: Option<anyhow::Error> = None;
        let shutdown_daemon = Arc::clone(self);
        let fenced = tokio::task::spawn_blocking(move || {
            let _command = shutdown_daemon.command_lock.lock().unwrap();
            let mut state = shutdown_daemon.state.write().unwrap();
            if let Some(operation) = state.operation.take() {
                operation.cancel.cancel();
            }
            state.revision = state.revision.wrapping_add(1);
        })
        .await;
        if let Err(err) = fenced {
            error!(err = %err, "failed to clear active operation during shutdown");
            last_err = Some(err.into());
        }

        let pipeline = self.pipeline.lock().unwrap().take();
        if let Some(pipeline) = pipeline
            && let Err(err) = pipeline.task.await
        {
            error!(operation_id = pipeline.operation_id, err = %err, "dictation pipeline task failed");
            last_err = Some(err.into());
        }

        self.drain_background_tasks().await;

        let close_daemon = Arc::clone(self);
        let recorder_result = tokio::task::spawn_blocking(move || {
            if close_daemon.recorder.is_recording() {
                close_daemon.recorder.cancel()?;
            }
            close_daemon.recorder.close()
        })
        .await;
        if let Err(err) = recorder_result
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
        {
            error!(err = %err, "failed to close recorder");
            last_err = Some(err);
        }

        if let Err(err) = self.notifier.close().await {
            error!(err = %err, "failed to close notifier");
            last_err = Some(err);
        }

        self.stop_osd_meter_publisher().await;
        let sink = std::mem::replace(&mut *self.visual_sink.write().unwrap(), Sink::Noop);
        if let Err(err) = sink.close().await {
            error!(err = %err, "failed to close OSD sink");
            last_err = Some(err);
        }

        let db = self.db.lock().unwrap().take();
        if let Some(db) = db
            && let Err(err) = tokio::task::spawn_blocking(move || drop(db)).await
        {
            error!(err = %err, "failed to close database");
            last_err = Some(err.into());
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

    fn state_matches_operation(&self, operation_id: u64) -> bool {
        self.state.read().unwrap().matches_operation(operation_id)
    }

    fn spawn_background<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut tasks = self.background_tasks.lock().unwrap();
        while let Some(result) = tasks.try_join_next() {
            if let Err(err) = result {
                warn!(err = %err, "background task failed");
            }
        }
        tasks.spawn_on(fut, &self.runtime);
    }

    async fn drain_background_tasks(&self) {
        let mut tasks = std::mem::take(&mut *self.background_tasks.lock().unwrap());
        while let Some(result) = tasks.join_next().await {
            if let Err(err) = result {
                warn!(err = %err, "background task failed during shutdown");
            }
        }
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

    fn start_notification_worker(self: &Arc<Self>) {
        let mut updates = self.notification_tx.subscribe();
        let daemon = Arc::downgrade(self);
        let shutdown = self.shutdown_cancel.clone();
        self.spawn_background(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => return,
                    changed = updates.changed() => {
                        if changed.is_err() {
                            return;
                        }
                    }
                }

                let update = *updates.borrow_and_update();
                let Some(daemon) = daemon.upgrade() else {
                    return;
                };
                {
                    let current = daemon.state.read().unwrap();
                    if current.revision != update.revision || current.state != update.state {
                        continue;
                    }
                }
                let result = tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => return,
                    result = daemon.update_notification_state(update.state) => result,
                };
                if let Err(err) = result {
                    warn!(err = %err, "failed to update notification");
                }
            }
        });
    }

    fn notify_state(&self, state: DaemonState, revision: u64) {
        self.notification_tx
            .send_replace(NotificationUpdate { state, revision });
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

        let snapshot_daemon = Arc::downgrade(self);
        let snapshot: visual::SnapshotFn = Arc::new(move || {
            snapshot_daemon.upgrade().map_or_else(
                || osd_state_event(DaemonState::Idle, Duration::ZERO, ""),
                |daemon| daemon.osd_snapshot(),
            )
        });

        let sink = match SocketSink::new(Some(snapshot)) {
            Ok(sink) => sink,
            Err(err) => {
                warn!(err = %err, "failed to start OSD event socket; continuing without OSD");
                return;
            }
        };

        *self.visual_sink.write().unwrap() = Sink::Socket(sink);
        self.start_osd_meter_publisher();

        let observer_daemon = Arc::downgrade(self);
        self.recorder.set_level_observer(
            Some(Arc::new(move |sample| {
                if let Some(daemon) = observer_daemon.upgrade() {
                    daemon.enqueue_level_sample(sample);
                }
            })),
            OSD_METER_UPDATE_INTERVAL,
        );
    }

    fn start_osd_meter_publisher(&self) {
        let cancel = self.shutdown_cancel.child_token();
        let (tx, mut rx) = watch::channel(LevelSample::default());
        let sink = self.visual_sink.read().unwrap().clone();
        let task_cancel = cancel.clone();
        let task = self.runtime.spawn(async move {
            loop {
                tokio::select! {
                    _ = task_cancel.cancelled() => return,
                    changed = rx.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        let sample = *rx.borrow_and_update();
                        sink.publish(visual::new_meter_event(sample.rms, sample.peak));
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
        publisher.tx.send_replace(sample);
    }

    // ---- error handling ------------------------------------------------

    fn handle_error_locked(self: &Arc<Self>, operation_id: Option<u64>, error_msg: String) {
        let revision = {
            let mut state = self.state.write().unwrap();
            if let Some(operation_id) = operation_id
                && !state.matches_operation(operation_id)
            {
                return;
            }
            if let Some(operation) = state.operation.take() {
                operation.cancel.cancel();
            }
            state.state = DaemonState::Error;
            state.last_error = Some(error_msg.clone());
            state.recording_duration = Duration::ZERO;
            state.revision = state.revision.wrapping_add(1);
            state.revision
        };

        self.cancel_recording_timeout(operation_id);
        self.cancel_error_timeout(None);
        self.publish_osd_state(DaemonState::Error, None, &error_msg);
        self.notify_state(DaemonState::Error, revision);

        let daemon = Arc::downgrade(self);
        let shutdown = self.shutdown_cancel.clone();
        let error_cancel = CancellationToken::new();
        *self.error_timeout.lock().unwrap() = Some((revision, error_cancel.clone()));
        self.spawn_background(async move {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => return,
                _ = error_cancel.cancelled() => return,
                _ = tokio::time::sleep(ERROR_DISPLAY_DURATION) => {}
            }

            let Some(daemon) = daemon.upgrade() else {
                return;
            };
            let transition_daemon = Arc::clone(&daemon);
            let transition = tokio::task::spawn_blocking(move || {
                let _command = transition_daemon.command_lock.lock().unwrap();
                let idle_revision = {
                    let mut state = transition_daemon.state.write().unwrap();
                    state.reset_error(revision)?
                };
                transition_daemon.publish_osd_state(DaemonState::Idle, None, "");
                Some(idle_revision)
            })
            .await;
            if let Ok(Some(idle_revision)) = transition {
                daemon.notify_state(DaemonState::Idle, idle_revision);
            }
        });
    }

    async fn handle_operation_error(self: &Arc<Self>, operation_id: u64, error_msg: String) {
        let daemon = Arc::clone(self);
        if let Err(err) = tokio::task::spawn_blocking(move || {
            let _command = daemon.command_lock.lock().unwrap();
            daemon.handle_error_locked(Some(operation_id), error_msg);
        })
        .await
        {
            error!(operation_id, err = %err, "failed to handle operation error");
        }
    }

    // ---- transcription pipeline ---------------------------------------

    async fn transcribe_and_type(
        self: Arc<Self>,
        operation_id: u64,
        cancel: CancellationToken,
        recording_duration: Duration,
    ) {
        if cancel.is_cancelled() {
            return;
        }

        let save_daemon = Arc::clone(&self);
        let saved = tokio::task::spawn_blocking(move || save_daemon.save_recording()).await;
        let (audio_data, audio_path) = match saved {
            Ok(Ok(value)) => value,
            Ok(Err(err)) => {
                self.handle_operation_error(
                    operation_id,
                    format!("{}: {err:#}", ipc::ERR_RECORDING_FAILED),
                )
                .await;
                return;
            }
            Err(err) => {
                self.handle_operation_error(
                    operation_id,
                    format!("{}: {err:#}", ipc::ERR_RECORDING_FAILED),
                )
                .await;
                return;
            }
        };

        if cancel.is_cancelled() || !self.state_matches_operation(operation_id) {
            return;
        }

        let text = match self.transcribe(&cancel, audio_data, &audio_path).await {
            Ok(text) => text,
            Err(err) => {
                if cancel.is_cancelled() {
                    debug!("transcription cancelled");
                    return;
                }
                self.handle_operation_error(
                    operation_id,
                    format!("{}: {err:#}", ipc::ERR_TRANSCRIPTION_FAILED),
                )
                .await;
                return;
            }
        };

        if self
            .type_and_save(
                operation_id,
                &cancel,
                &text,
                recording_duration,
                &audio_path,
            )
            .await
            .is_err()
        {
            return;
        }

        let finish_daemon = Arc::clone(&self);
        let finished = tokio::task::spawn_blocking(move || {
            let _command = finish_daemon.command_lock.lock().unwrap();
            let revision = finish_daemon
                .state
                .write()
                .unwrap()
                .finish_operation(operation_id)?;
            finish_daemon.publish_osd_state(DaemonState::Idle, None, "");
            Some(revision)
        })
        .await;
        if let Ok(Some(revision)) = finished {
            self.notify_state(DaemonState::Idle, revision);
        }
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
            audio_data: audio_data.into(),
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
        operation_id: u64,
        cancel: &CancellationToken,
        text: &str,
        duration: Duration,
        audio_path: &str,
    ) -> Result<()> {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let transition_daemon = Arc::clone(self);
        let revision = tokio::task::spawn_blocking(move || {
            let _command = transition_daemon.command_lock.lock().unwrap();
            let revision = transition_daemon
                .state
                .write()
                .unwrap()
                .transition_operation(operation_id, DaemonState::Typing)?;
            transition_daemon.publish_osd_state(DaemonState::Typing, None, "");
            Some(revision)
        })
        .await
        .map_err(anyhow::Error::from)?;
        let Some(revision) = revision else {
            return Ok(());
        };
        self.notify_state(DaemonState::Typing, revision);

        if let Err(err) = self.typer.type_text(cancel, text).await {
            if cancel.is_cancelled() {
                debug!("typing cancelled");
                return Ok(());
            }
            error!(err = %err, "typing failed");
            self.handle_operation_error(
                operation_id,
                format!("{}: {err:#}", ipc::ERR_TYPING_FAILED),
            )
            .await;
            return Err(err);
        }

        info!("typing complete");

        if cancel.is_cancelled() || !self.state_matches_operation(operation_id) {
            return Ok(());
        }

        let model = self
            .config
            .api
            .providers
            .get(&self.config.api.active_provider)
            .map(|p| p.model.clone())
            .unwrap_or_default();
        let duration_ms = duration.as_millis() as i64;

        let daemon = Arc::clone(self);
        let text = text.to_owned();
        let audio_path = audio_path.to_owned();
        let saved = tokio::task::spawn_blocking(move || match daemon.db.lock().unwrap().as_ref() {
            Some(db) => db.save_transcript(duration_ms, &text, &audio_path, &model),
            None => Err(anyhow!("database is closed")),
        })
        .await
        .map_err(anyhow::Error::from)?;
        match saved {
            Ok(()) => debug!("transcript saved to database"),
            Err(err) => warn!(err = %err, "failed to save transcript to database"),
        }

        Ok(())
    }

    fn reap_finished_pipeline(&self) -> bool {
        let mut pipeline = self.pipeline.lock().unwrap();
        if pipeline
            .as_ref()
            .is_some_and(|pipeline| pipeline.task.is_finished())
        {
            pipeline.take();
        }
        pipeline.is_some()
    }

    fn start_recording_locked(self: &Arc<Self>) -> Result<()> {
        if self.shutdown_cancel.is_cancelled() {
            bail!("daemon is shutting down");
        }
        if self.reap_finished_pipeline() {
            bail!("previous dictation operation is still stopping");
        }

        {
            let state = self.state.read().unwrap();
            if state.state == DaemonState::Recording {
                bail!("{}", ipc::ERR_ALREADY_RECORDING);
            }
            if state.state != DaemonState::Idle {
                bail!("cannot start in current state: {}", state.state);
            }
        }

        debug!("starting recording");
        if let Err(err) = self.recorder.start() {
            error!(err = %err, "failed to start recording");
            self.handle_error_locked(None, format!("{}: {err:#}", ipc::ERR_RECORDING_FAILED));
            bail!("{}: {err:#}", ipc::ERR_RECORDING_FAILED);
        }

        let cancel = self.shutdown_cancel.child_token();
        let (operation_id, revision) = {
            let mut state = self.state.write().unwrap();
            state.next_operation_id = state.next_operation_id.wrapping_add(1).max(1);
            let operation_id = state.next_operation_id;
            state.operation = Some(Operation {
                id: operation_id,
                cancel: cancel.clone(),
            });
            state.state = DaemonState::Recording;
            state.last_error = None;
            state.recording_duration = Duration::ZERO;
            state.revision = state.revision.wrapping_add(1);
            (operation_id, state.revision)
        };

        self.publish_osd_state(DaemonState::Recording, Some(Duration::ZERO), "");
        self.notify_state(DaemonState::Recording, revision);
        self.start_recording_timeout(operation_id);
        info!(operation_id, "recording started");
        Ok(())
    }

    fn stop_recording_locked(self: &Arc<Self>, expected_operation_id: Option<u64>) -> Result<()> {
        if self.shutdown_cancel.is_cancelled() {
            bail!("daemon is shutting down");
        }

        let recording_duration = self.recorder.get_recording_duration();
        let (operation_id, cancel, revision) = {
            let mut state = self.state.write().unwrap();
            let Some((operation, revision)) =
                state.claim_recording(expected_operation_id, recording_duration)
            else {
                bail!("{}", ipc::ERR_NOT_RECORDING);
            };
            (operation.id, operation.cancel, revision)
        };

        self.cancel_recording_timeout(Some(operation_id));
        self.publish_osd_state(DaemonState::Transcribing, Some(recording_duration), "");
        self.notify_state(DaemonState::Transcribing, revision);

        let daemon = Arc::clone(self);
        let task = self.runtime.spawn(async move {
            daemon
                .transcribe_and_type(operation_id, cancel, recording_duration)
                .await;
        });
        let mut pipeline = self.pipeline.lock().unwrap();
        if pipeline.is_some() {
            task.abort();
            bail!("dictation pipeline is already running");
        }
        *pipeline = Some(PipelineTask { operation_id, task });

        info!(
            operation_id,
            "stopping recording and starting transcription"
        );
        Ok(())
    }

    fn cancel_operation_locked(self: &Arc<Self>) -> Result<()> {
        if self.shutdown_cancel.is_cancelled() {
            bail!("daemon is shutting down");
        }
        debug!("canceling current operation");

        let (operation_id, revision) = {
            let mut state = self.state.write().unwrap();
            if state.state == DaemonState::Idle && state.operation.is_none() {
                return Ok(());
            }
            let operation_id = state.operation.as_ref().map(|operation| operation.id);
            if let Some(operation) = state.operation.take() {
                operation.cancel.cancel();
            }
            state.state = DaemonState::Idle;
            state.last_error = None;
            state.recording_duration = Duration::ZERO;
            state.revision = state.revision.wrapping_add(1);
            (operation_id, state.revision)
        };

        self.cancel_recording_timeout(operation_id);
        self.cancel_error_timeout(None);
        if self.recorder.is_recording()
            && let Err(err) = self.recorder.cancel()
        {
            error!(err = %err, "failed to stop recording during cancel");
        }

        self.publish_osd_state(DaemonState::Idle, None, "");
        self.notify_state(DaemonState::Idle, revision);
        info!(operation_id, "operation canceled");
        Ok(())
    }

    fn start_recording_timeout(self: &Arc<Self>, operation_id: u64) {
        let cancel = CancellationToken::new();
        *self.recording_timeout.lock().unwrap() = Some((operation_id, cancel.clone()));
        let daemon = Arc::downgrade(self);
        let shutdown = self.shutdown_cancel.clone();
        let max_duration = Duration::from_secs(
            u64::try_from(self.config.audio.max_duration_min)
                .unwrap_or(1)
                .saturating_mul(60),
        );
        self.spawn_background(async move {
            let deadline = tokio::time::sleep(max_duration);
            tokio::pin!(deadline);
            let mut errors = tokio::time::interval(Duration::from_millis(250));
            errors.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            errors.tick().await;

            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => return,
                    _ = cancel.cancelled() => return,
                    _ = &mut deadline => {
                        let Some(daemon) = daemon.upgrade() else {
                            return;
                        };
                        let transition = tokio::task::spawn_blocking(move || {
                            daemon.stop_recording_at_limit(operation_id)
                        }).await;
                        match transition {
                            Ok(Ok(())) => {}
                            Ok(Err(err)) => warn!(operation_id, err = %err, "failed to stop recording at duration limit"),
                            Err(err) => warn!(operation_id, err = %err, "duration-limit task failed"),
                        }
                        return;
                    }
                    _ = errors.tick() => {
                        let Some(daemon) = daemon.upgrade() else {
                            return;
                        };
                        let Some(recorder_error) = daemon.recorder.take_error() else {
                            continue;
                        };
                        let failure = tokio::task::spawn_blocking(move || {
                            let _command = daemon.command_lock.lock().unwrap();
                            if !daemon.state_matches_operation(operation_id)
                                || daemon.current_state() != DaemonState::Recording
                            {
                                return;
                            }
                            daemon.handle_error_locked(
                                Some(operation_id),
                                format!("{}: {recorder_error}", ipc::ERR_RECORDING_FAILED),
                            );
                            if daemon.recorder.is_recording()
                                && let Err(err) = daemon.recorder.cancel()
                            {
                                error!(operation_id, err = %err, "failed to stop recorder after backend error");
                            }
                        })
                        .await;
                        if let Err(err) = failure {
                            warn!(operation_id, err = %err, "recorder-error task failed");
                        }
                        return;
                    }
                }
            }
        });
    }

    fn cancel_recording_timeout(&self, operation_id: Option<u64>) {
        let mut timeout = self.recording_timeout.lock().unwrap();
        if timeout
            .as_ref()
            .is_some_and(|(id, _)| operation_id.is_none_or(|operation_id| *id == operation_id))
            && let Some((_, cancel)) = timeout.take()
        {
            cancel.cancel();
        }
    }

    fn cancel_error_timeout(&self, revision: Option<u64>) {
        let mut timeout = self.error_timeout.lock().unwrap();
        if timeout
            .as_ref()
            .is_some_and(|(current, _)| revision.is_none_or(|revision| *current == revision))
            && let Some((_, cancel)) = timeout.take()
        {
            cancel.cancel();
        }
    }

    fn stop_recording_at_limit(self: &Arc<Self>, operation_id: u64) -> Result<()> {
        let _command = self.command_lock.lock().unwrap();
        self.stop_recording_locked(Some(operation_id))
    }
}

// ---- CommandHandler ----------------------------------------------------

impl CommandHandler for Arc<Daemon> {
    fn handle_start(&self) -> Result<()> {
        let _command = self.command_lock.lock().unwrap();
        self.start_recording_locked()
    }

    fn handle_stop(&self) -> Result<()> {
        let _command = self.command_lock.lock().unwrap();
        self.stop_recording_locked(None)
    }

    fn handle_toggle(&self) -> Result<()> {
        let _command = self.command_lock.lock().unwrap();
        match self.current_state() {
            DaemonState::Idle => self.start_recording_locked(),
            DaemonState::Recording => self.stop_recording_locked(None),
            other => bail!("cannot toggle in current state: {other}"),
        }
    }

    fn handle_cancel(&self) -> Result<()> {
        let _command = self.command_lock.lock().unwrap();
        self.cancel_operation_locked()
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
    use std::sync::Barrier;

    use super::*;
    use crate::typing::Backend;
    use crate::utils::default_config;

    fn daemon_fixture() -> (Arc<Daemon>, tempfile::TempDir) {
        let mut config = default_config();
        config.enable_osd = false;
        config.notifications = NotificationMode::All;
        let recorder = Recorder::for_test(config.audio.clone());
        let transcriber = WhisperClient::new(&config.api).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.db")).unwrap();
        let (notification_tx, _) = watch::channel(NotificationUpdate {
            state: DaemonState::Idle,
            revision: 0,
        });
        let daemon = Arc::new(Daemon {
            config,
            recorder,
            transcriber,
            notifier: Notifier::Noop,
            visual_sink: RwLock::new(Sink::Noop),
            typer: Typer::with_backend(Backend::Wayland),
            db: Mutex::new(Some(db)),
            state: RwLock::new(State {
                state: DaemonState::Idle,
                last_error: None,
                recording_duration: Duration::ZERO,
                operation: None,
                next_operation_id: 0,
                revision: 0,
            }),
            command_lock: Mutex::new(()),
            start_time: Instant::now(),
            stop_notify: Notify::new(),
            shutdown_cancel: CancellationToken::new(),
            notification_tx,
            meter: Mutex::new(None),
            recording_timeout: Mutex::new(None),
            error_timeout: Mutex::new(None),
            pipeline: Mutex::new(None),
            background_tasks: Mutex::new(JoinSet::new()),
            runtime: tokio::runtime::Handle::current(),
        });
        (daemon, dir)
    }

    fn state_with_operation(operation_id: u64, state: DaemonState) -> State {
        State {
            state,
            last_error: None,
            recording_duration: Duration::from_secs(9),
            operation: Some(Operation {
                id: operation_id,
                cancel: CancellationToken::new(),
            }),
            next_operation_id: operation_id,
            revision: 3,
        }
    }

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

    #[test]
    fn stale_pipeline_cannot_mutate_new_operation() {
        let mut state = state_with_operation(2, DaemonState::Recording);

        assert_eq!(state.transition_operation(1, DaemonState::Typing), None);
        assert_eq!(state.finish_operation(1), None);
        assert!(!state.matches_operation(1));

        assert_eq!(state.state, DaemonState::Recording);
        assert_eq!(state.recording_duration, Duration::from_secs(9));
        assert_eq!(
            state.operation.as_ref().map(|operation| operation.id),
            Some(2)
        );
        assert_eq!(state.revision, 3);
    }

    #[test]
    fn stop_and_timeout_can_claim_recording_only_once() {
        let mut manual_first = state_with_operation(7, DaemonState::Recording);
        let claimed = manual_first
            .claim_recording(None, Duration::from_secs(10))
            .expect("manual stop claims recording");
        assert_eq!(claimed.0.id, 7);
        assert!(
            manual_first
                .claim_recording(Some(7), Duration::from_secs(10))
                .is_none()
        );

        let mut timeout_first = state_with_operation(8, DaemonState::Recording);
        let claimed = timeout_first
            .claim_recording(Some(8), Duration::from_secs(10))
            .expect("matching timeout claims recording");
        assert_eq!(claimed.0.id, 8);
        assert!(
            timeout_first
                .claim_recording(None, Duration::from_secs(10))
                .is_none()
        );
        assert_eq!(timeout_first.state, DaemonState::Transcribing);
    }

    #[test]
    fn stale_timeout_cannot_claim_later_recording() {
        let mut state = state_with_operation(12, DaemonState::Recording);
        assert!(
            state
                .claim_recording(Some(11), Duration::from_secs(10))
                .is_none()
        );
        assert_eq!(state.state, DaemonState::Recording);
        assert_eq!(
            state.operation.as_ref().map(|operation| operation.id),
            Some(12)
        );
    }

    #[test]
    fn error_reset_clears_the_error_when_returning_to_idle() {
        let mut state = State {
            state: DaemonState::Error,
            last_error: Some("transcription failed: provider token leaked".into()),
            recording_duration: Duration::from_secs(9),
            operation: None,
            next_operation_id: 4,
            revision: 12,
        };

        assert_eq!(state.reset_error(12), Some(13));
        assert_eq!(state.state, DaemonState::Idle);
        assert_eq!(state.last_error, None);
        assert_eq!(state.recording_duration, Duration::ZERO);
    }

    #[test]
    fn stale_error_reset_does_not_clear_a_newer_error() {
        let mut state = State {
            state: DaemonState::Error,
            last_error: Some("newer failure".into()),
            recording_duration: Duration::ZERO,
            operation: None,
            next_operation_id: 4,
            revision: 13,
        };

        assert_eq!(state.reset_error(12), None);
        assert_eq!(state.state, DaemonState::Error);
        assert_eq!(state.last_error.as_deref(), Some("newer failure"));
        assert_eq!(state.revision, 13);
    }

    #[tokio::test]
    async fn canceled_pipeline_cannot_mutate_replacement_operation() {
        let (daemon, _dir) = daemon_fixture();
        let operation_a = daemon.shutdown_cancel.child_token();
        {
            let mut state = daemon.state.write().unwrap();
            state.state = DaemonState::Transcribing;
            state.operation = Some(Operation {
                id: 1,
                cancel: operation_a.clone(),
            });
            state.next_operation_id = 1;
            state.revision = 1;
        }

        let started = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let task_daemon = Arc::clone(&daemon);
        let task_started = Arc::clone(&started);
        let task_resume = Arc::clone(&resume);
        let task = tokio::spawn(async move {
            task_started.notify_one();
            task_resume.notified().await;
            task_daemon
                .transcribe_and_type(1, operation_a, Duration::from_secs(4))
                .await;
        });

        started.notified().await;
        {
            let mut state = daemon.state.write().unwrap();
            state.operation.as_ref().unwrap().cancel.cancel();
            state.state = DaemonState::Recording;
            state.operation = Some(Operation {
                id: 2,
                cancel: daemon.shutdown_cancel.child_token(),
            });
            state.next_operation_id = 2;
            state.recording_duration = Duration::from_secs(17);
            state.revision = 2;
        }
        resume.notify_one();
        task.await.unwrap();

        let state = daemon.state.read().unwrap();
        assert_eq!(state.state, DaemonState::Recording);
        assert_eq!(state.recording_duration, Duration::from_secs(17));
        assert!(state.matches_operation(2));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timeout_and_manual_stop_start_only_one_pipeline() {
        let (daemon, _dir) = daemon_fixture();
        {
            let mut state = daemon.state.write().unwrap();
            state.state = DaemonState::Recording;
            state.operation = Some(Operation {
                id: 7,
                cancel: daemon.shutdown_cancel.child_token(),
            });
            state.next_operation_id = 7;
            state.revision = 1;
        }

        let barrier = Arc::new(Barrier::new(2));
        let manual_daemon = Arc::clone(&daemon);
        let manual_barrier = Arc::clone(&barrier);
        let manual = tokio::task::spawn_blocking(move || {
            manual_barrier.wait();
            let _command = manual_daemon.command_lock.lock().unwrap();
            manual_daemon.stop_recording_locked(None)
        });
        let timeout_daemon = Arc::clone(&daemon);
        let timeout = tokio::task::spawn_blocking(move || {
            barrier.wait();
            timeout_daemon.stop_recording_at_limit(7)
        });

        let manual_ok = manual.await.unwrap().is_ok();
        let timeout_ok = timeout.await.unwrap().is_ok();
        assert_ne!(manual_ok, timeout_ok);
        assert!(daemon.pipeline.lock().unwrap().is_some());

        daemon.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn notification_burst_uses_one_worker_and_shutdown_releases_daemon() {
        let (daemon, _dir) = daemon_fixture();
        daemon.start_notification_worker();
        for revision in 1..=10_000 {
            {
                let mut state = daemon.state.write().unwrap();
                state.state = DaemonState::Recording;
                state.revision = revision;
            }
            daemon.notify_state(DaemonState::Recording, revision);
        }

        assert_eq!(daemon.notification_tx.borrow().revision, 10_000);
        assert_eq!(daemon.background_tasks.lock().unwrap().len(), 1);

        let weak = Arc::downgrade(&daemon);
        daemon.shutdown().await.unwrap();
        drop(daemon);
        assert!(weak.upgrade().is_none());
    }

    #[tokio::test]
    async fn shutdown_root_cancels_active_operation_immediately() {
        let (daemon, _dir) = daemon_fixture();
        let operation = daemon.shutdown_cancel.child_token();
        {
            let mut state = daemon.state.write().unwrap();
            state.state = DaemonState::Transcribing;
            state.operation = Some(Operation {
                id: 1,
                cancel: operation.clone(),
            });
        }

        daemon.shutdown_cancel.cancel();
        assert!(operation.is_cancelled());
        daemon.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_lock_contention_does_not_block_the_runtime() {
        let (daemon, _dir) = daemon_fixture();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let lock_daemon = Arc::clone(&daemon);
        let owner = std::thread::spawn(move || {
            let _command = lock_daemon.command_lock.lock().unwrap();
            ready_tx.send(()).unwrap();
            // A synchronous runtime wait would prevent the timer below from
            // signaling us; the timeout keeps that regression from hanging CI.
            release_rx.recv_timeout(Duration::from_secs(2)).is_ok()
        });
        ready_rx.await.unwrap();
        let progress = async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let _ = release_tx.send(());
        };
        let (result, ()) = tokio::join!(daemon.shutdown(), progress);
        result.unwrap();
        assert!(
            owner.join().unwrap(),
            "shutdown starved an unrelated Tokio timer"
        );
    }
}
