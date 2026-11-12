use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Instant;

use chrono::{DateTime, Duration, NaiveDate, Utc};

use crate::ipc::{Client, DaemonState};
use crate::storage::{
    DailyActivity, Db, HistoryFilter, HistoryQuery, RecordingDetail, RecordingStats,
    RecordingStatus,
};

const PAGE_SIZE: usize = 50;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Edited,
    Failed,
}

#[derive(Clone, Debug)]
pub struct Recording {
    pub id: i64,
    pub timestamp: DateTime<Utc>,
    pub duration_ms: i64,
    pub text: String,
    pub audio_path: PathBuf,
    pub model: String,
    pub failed: bool,
    pub error: String,
    pub revision: i64,
    pub attempts: usize,
}

#[derive(Clone, Debug)]
pub struct Revision {
    pub number: i64,
    pub timestamp: DateTime<Utc>,
    pub text: String,
    pub source: String,
}

#[derive(Clone, Debug)]
pub struct Attempt {
    pub number: i64,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub latency_ms: Option<i64>,
    pub status: String,
    pub error: String,
    pub model: String,
}

#[derive(Clone, Debug)]
pub struct Detail {
    pub recording: Recording,
    pub revisions: Vec<Revision>,
    pub attempts: Vec<Attempt>,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub total: i64,
    pub successful: i64,
    pub failed: i64,
    pub edited: i64,
    pub words: i64,
    pub duration_ms: i64,
    pub today_words: i64,
    pub today_duration_ms: i64,
    pub daily: Vec<(NaiveDate, i64)>,
    pub daily_last_30: Vec<DailyActivity>,
    pub by_hour: [i64; 24],
    pub average_duration_ms: Option<i64>,
    pub median_duration_ms: Option<i64>,
    pub this_week_words: i64,
    pub last_week_words: i64,
    pub models: Vec<(String, i64, i64, Option<i64>)>,
    pub latency_samples: Vec<i64>,
    pub p50: Option<i64>,
    pub p95: Option<i64>,
    pub p99: Option<i64>,
    pub max: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct Microphone {
    pub id: String,
    pub name: String,
    pub connected: bool,
    pub is_default: bool,
    pub bluetooth: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderInfo {
    pub name: String,
    pub model: String,
    pub endpoint: String,
    pub usable: bool,
    /// Why it cannot be selected; empty when usable.
    pub problem: String,
    pub active: bool,
    pub is_default: bool,
    pub send_vocabulary: bool,
}

/// The read-only view of `config.json`.
#[derive(Clone, Debug, Default)]
pub struct ManagedSettings {
    pub managed_by_home_manager: bool,
    /// The config default provider. The daemon may be using another one;
    /// ask with [`Request::Providers`].
    pub provider: String,
    /// The config default provider's model.
    pub model: String,
    /// The config default provider's endpoint.
    pub endpoint: String,
    pub timeout_seconds: i64,
    pub max_duration_minutes: i64,
    pub paste_shortcut: String,
    pub notifications: String,
    /// Whether the config default provider receives the glossary.
    pub send_vocabulary: bool,
}

/// The glossary file as stored. A missing file (`exists == false`) sends no
/// vocabulary; an existing empty list turns boosting off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Glossary {
    pub exists: bool,
    pub terms: Vec<String>,
}

impl Glossary {
    fn from_stored(stored: Option<Vec<String>>) -> Self {
        Self {
            exists: stored.is_some(),
            terms: stored.unwrap_or_default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    pub recordings: Vec<Recording>,
    pub total: i64,
    pub has_next: bool,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub state: DaemonState,
    pub duration_ms: i64,
    pub error: String,
    pub recovered_text: String,
    pub uptime_seconds: u64,
    pub last_recording_id: Option<i64>,
    pub last_recording_generation: u64,
    pub audio_level_rms: f32,
    pub audio_level_peak: f32,
    /// The provider the daemon will use next; empty from a daemon that
    /// predates runtime switching.
    pub provider: String,
}

#[derive(Clone, Debug)]
pub struct ConnectionReport {
    pub daemon: Result<(), String>,
    pub provider: Result<String, String>,
    pub endpoint_host: String,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug)]
pub enum Request {
    History {
        search: String,
        filter: Filter,
        page: usize,
    },
    Detail(i64),
    SaveRevision {
        id: i64,
        expected_revision: i64,
        text: String,
    },
    RestoreRevision {
        id: i64,
        revision: i64,
        expected_revision: i64,
    },
    Stats,
    Microphones,
    ReorderMicrophones(Vec<String>),
    /// Answered with [`Reply::Microphones`].
    RemoveMicrophone(String),
    /// Answered with [`Reply::Providers`].
    Providers,
    /// Answered with [`Reply::Providers`].
    SetProvider(String),
    Status,
    Toggle,
    Cancel,
    Retry(i64),
    Delete(i64),
    CheckConnection,
    Recent {
        limit: usize,
    },
    Glossary,
    AddGlossaryTerm(String),
    RemoveGlossaryTerm(String),
    /// Answered with [`Reply::Glossary`].
    RenameGlossaryTerm {
        from: String,
        to: String,
    },
}

/// Replies cross an mpsc channel a few times per second, so the size spread
/// between `Stats` and the small variants does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Reply {
    History(Result<Page, String>),
    Detail(Result<Detail, String>),
    Saved(Result<Detail, String>),
    Stats(Result<Stats, String>),
    Microphones(Result<Vec<Microphone>, String>),
    Status(Result<Status, String>),
    Action(Result<(), String>),
    Deleted(Result<i64, String>),
    Connection(Result<ConnectionReport, String>),
    Recent(Result<Vec<Recording>, String>),
    Glossary(Result<Glossary, String>),
    /// Kept apart from `Glossary` so the view clears its input only after a
    /// successful add.
    GlossaryAdded(Result<Glossary, String>),
    /// Answers [`Request::Providers`] and [`Request::SetProvider`].
    Providers(Result<Vec<ProviderInfo>, String>),
}

pub struct Backend {
    request_tx: Sender<Request>,
    reply_rx: Receiver<Reply>,
    pub managed_settings: ManagedSettings,
}

impl Backend {
    pub fn new(demo: bool) -> Self {
        let (request_tx, request_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let managed_settings = read_managed_settings(demo);
        let default_endpoint = managed_settings.endpoint.clone();
        thread::Builder::new()
            .name("dictator-gui-backend".to_string())
            .spawn(move || {
                if demo {
                    run_demo_worker(request_rx, reply_tx);
                } else {
                    run_live_worker(request_rx, reply_tx, default_endpoint);
                }
            })
            .expect("failed to start GUI backend worker");
        Self {
            request_tx,
            reply_rx,
            managed_settings,
        }
    }

    pub fn send(&self, request: Request) {
        let _ = self.request_tx.send(request);
    }

    pub fn drain(&self) -> Vec<Reply> {
        let mut replies = Vec::new();
        while let Ok(reply) = self.reply_rx.try_recv() {
            replies.push(reply);
        }
        replies
    }
}

/// `default_endpoint` is probed by a connection check while the daemon is down.
fn run_live_worker(requests: Receiver<Request>, replies: Sender<Reply>, default_endpoint: String) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            run_failed_worker(requests, replies, error.to_string());
            return;
        }
    };
    let mut db = None;
    let mut db_error = String::new();
    retry_database(&mut db, &mut db_error, open_database);
    while let Ok(request) = requests.recv() {
        let kind = live_request_kind(&request);
        reopen_database_for_request(&request, &mut db, &mut db_error, open_database);
        let reply = if let Some(db) = db.as_ref() {
            handle_live(db, &runtime, &default_endpoint, request)
        } else if kind == LiveRequestKind::Service {
            handle_live_service(&runtime, &default_endpoint, request)
        } else {
            failed_reply(request, &db_error)
        };
        if replies.send(reply).is_err() {
            break;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LiveRequestKind {
    DatabaseRead,
    DatabaseMutation,
    Service,
}

fn live_request_kind(request: &Request) -> LiveRequestKind {
    match request {
        Request::History { .. } | Request::Detail(_) | Request::Stats | Request::Recent { .. } => {
            LiveRequestKind::DatabaseRead
        }
        Request::SaveRevision { .. } | Request::RestoreRevision { .. } | Request::Delete(_) => {
            LiveRequestKind::DatabaseMutation
        }
        Request::Microphones
        | Request::ReorderMicrophones(_)
        | Request::RemoveMicrophone(_)
        | Request::Providers
        | Request::SetProvider(_)
        | Request::RenameGlossaryTerm { .. }
        | Request::Status
        | Request::Toggle
        | Request::Cancel
        | Request::Retry(_)
        | Request::CheckConnection
        | Request::Glossary
        | Request::AddGlossaryTerm(_)
        | Request::RemoveGlossaryTerm(_) => LiveRequestKind::Service,
    }
}

fn open_database() -> Result<Db, String> {
    Db::new().map_err(|error| format!("Database unavailable: {error:#}"))
}

fn retry_database<T>(
    database: &mut Option<T>,
    error: &mut String,
    open: impl FnOnce() -> Result<T, String>,
) {
    match open() {
        Ok(opened) => {
            *database = Some(opened);
            error.clear();
        }
        Err(open_error) => {
            *database = None;
            *error = open_error;
        }
    }
}

fn reopen_database_for_request<T>(
    request: &Request,
    database: &mut Option<T>,
    error: &mut String,
    open: impl FnOnce() -> Result<T, String>,
) {
    if database.is_none() && live_request_kind(request) == LiveRequestKind::DatabaseRead {
        retry_database(database, error, open);
    }
}

fn run_failed_worker(requests: Receiver<Request>, replies: Sender<Reply>, message: String) {
    while let Ok(request) = requests.recv() {
        if replies.send(failed_reply(request, &message)).is_err() {
            break;
        }
    }
}

fn failed_reply(request: Request, message: &str) -> Reply {
    match request {
        Request::History { .. } => Reply::History(Err(message.to_string())),
        Request::Detail(_) => Reply::Detail(Err(message.to_string())),
        Request::SaveRevision { .. } | Request::RestoreRevision { .. } => {
            Reply::Saved(Err(message.to_string()))
        }
        Request::Stats => Reply::Stats(Err(message.to_string())),
        Request::Recent { .. } => Reply::Recent(Err(message.to_string())),
        Request::Microphones | Request::ReorderMicrophones(_) | Request::RemoveMicrophone(_) => {
            Reply::Microphones(Err(message.to_string()))
        }
        Request::Providers | Request::SetProvider(_) => Reply::Providers(Err(message.to_string())),
        Request::Status => Reply::Status(Err(message.to_string())),
        Request::Toggle | Request::Cancel | Request::Retry(_) => {
            Reply::Action(Err(message.to_string()))
        }
        Request::Delete(_) => Reply::Deleted(Err(message.to_string())),
        Request::CheckConnection => Reply::Connection(Err(message.to_string())),
        Request::Glossary | Request::RemoveGlossaryTerm(_) | Request::RenameGlossaryTerm { .. } => {
            Reply::Glossary(Err(message.to_string()))
        }
        Request::AddGlossaryTerm(_) => Reply::GlossaryAdded(Err(message.to_string())),
    }
}

fn handle_live(
    db: &Db,
    runtime: &tokio::runtime::Runtime,
    default_endpoint: &str,
    request: Request,
) -> Reply {
    match request {
        Request::History {
            search,
            filter,
            page,
        } => {
            let mut before = None;
            let mut result = None;
            for _ in 0..=page {
                let query = HistoryQuery {
                    search: (!search.trim().is_empty()).then(|| search.trim().to_string()),
                    filter: match filter {
                        Filter::All => HistoryFilter::All,
                        Filter::Edited => HistoryFilter::Edited,
                        Filter::Failed => HistoryFilter::Failed,
                    },
                    limit: PAGE_SIZE,
                    before,
                };
                match db.get_history(&query) {
                    Ok(page) => {
                        before = page.next_cursor.clone();
                        result = Some(page);
                        if before.is_none() {
                            break;
                        }
                    }
                    Err(error) => return Reply::History(Err(error.to_string())),
                }
            }
            let Some(page) = result else {
                return Reply::History(Err("history is unavailable".to_string()));
            };
            Reply::History(Ok(Page {
                recordings: page.recordings.into_iter().map(map_recording).collect(),
                total: page.total,
                has_next: page.next_cursor.is_some(),
            }))
        }
        Request::Detail(id) => Reply::Detail(
            db.get_recording(id)
                .and_then(|detail| detail.ok_or_else(|| anyhow::anyhow!("recording not found")))
                .map(map_detail)
                .map_err(|error| error.to_string()),
        ),
        Request::SaveRevision {
            id,
            expected_revision,
            text,
        } => Reply::Saved(
            db.append_revision(id, expected_revision, &text, "user")
                .and_then(|_| db.get_recording(id))
                .and_then(|detail| detail.ok_or_else(|| anyhow::anyhow!("recording not found")))
                .map(map_detail)
                .map_err(|error| error.to_string()),
        ),
        Request::RestoreRevision {
            id,
            revision,
            expected_revision,
        } => Reply::Saved(
            db.restore_revision(id, revision, expected_revision)
                .and_then(|_| db.get_recording(id))
                .and_then(|detail| detail.ok_or_else(|| anyhow::anyhow!("recording not found")))
                .map(map_detail)
                .map_err(|error| error.to_string()),
        ),
        Request::Stats => Reply::Stats(
            db.get_stats()
                .map(map_stats)
                .map_err(|error| error.to_string()),
        ),
        Request::Recent { limit } => Reply::Recent(
            db.get_history(&HistoryQuery {
                limit: limit.max(1),
                ..HistoryQuery::default()
            })
            .map(|page| page.recordings.into_iter().map(map_recording).collect())
            .map_err(|error| error.to_string()),
        ),
        Request::Delete(id) => Reply::Deleted(
            db.delete_recording(id)
                .map(|audio_path| {
                    if let Some(path) = audio_path {
                        remove_audio_file(&path);
                    }
                    id
                })
                .map_err(|error| error.to_string()),
        ),
        request => handle_live_service(runtime, default_endpoint, request),
    }
}

fn remove_audio_file(path: &std::path::Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "failed to remove recording audio");
        }
    }
}

async fn probe_provider(endpoint: &str) -> Result<String, String> {
    if endpoint.trim().is_empty() {
        return Err("no provider endpoint configured".to_string());
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())?;
    let started = Instant::now();
    match client.get(endpoint.trim()).send().await {
        Ok(response) => Ok(format!(
            "HTTP {} in {} ms",
            response.status().as_u16(),
            started.elapsed().as_millis()
        )),
        Err(error) => Err(short_request_error(&error)),
    }
}

fn endpoint_host(endpoint: &str) -> String {
    reqwest::Url::parse(endpoint.trim())
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_default()
}

fn short_request_error(error: &reqwest::Error) -> String {
    let kind = if error.is_timeout() {
        "timed out"
    } else if error.is_connect() {
        "connection failed"
    } else if error.is_builder() {
        "invalid endpoint"
    } else {
        "request failed"
    };
    let mut source: &dyn std::error::Error = error;
    while let Some(next) = source.source() {
        source = next;
    }
    format!("{kind}: {source}")
}

/// The daemon's error, or a short note that it is not running.
fn daemon_error(error: anyhow::Error) -> String {
    let message = format!("{error:#}");
    if message.starts_with("failed to connect to daemon") {
        "The dictator daemon is not running".to_string()
    } else {
        message
    }
}

/// The endpoint of the provider the daemon is using, or `default_endpoint`
/// when the daemon cannot say.
async fn active_endpoint(client: &Client, default_endpoint: &str) -> String {
    client
        .providers()
        .await
        .ok()
        .and_then(|providers| providers.into_iter().find(|provider| provider.active))
        .map(|provider| provider.endpoint)
        .unwrap_or_else(|| default_endpoint.to_string())
}

fn map_provider(provider: crate::ipc::ProviderSummary) -> ProviderInfo {
    ProviderInfo {
        name: provider.name,
        model: provider.model,
        endpoint: provider.endpoint,
        usable: provider.usable,
        problem: provider.problem,
        active: provider.active,
        is_default: provider.is_default,
        send_vocabulary: provider.send_vocabulary,
    }
}

fn handle_live_service(
    runtime: &tokio::runtime::Runtime,
    default_endpoint: &str,
    request: Request,
) -> Reply {
    match request {
        Request::Microphones => Reply::Microphones(
            crate::audio::microphone_preferences()
                .map(|prefs| prefs.microphones.into_iter().map(map_microphone).collect())
                .map_err(|error| error.to_string()),
        ),
        Request::ReorderMicrophones(ids) => Reply::Microphones(
            crate::audio::reorder_microphones(&ids)
                .map(|prefs| prefs.microphones.into_iter().map(map_microphone).collect())
                .map_err(|error| error.to_string()),
        ),
        Request::RemoveMicrophone(id) => Reply::Microphones(
            crate::audio::remove_microphone(&id)
                .map(|prefs| prefs.microphones.into_iter().map(map_microphone).collect())
                .map_err(|error| error.to_string()),
        ),
        Request::Providers => Reply::Providers(
            runtime
                .block_on(Client::new().providers())
                .map(|providers| providers.into_iter().map(map_provider).collect())
                .map_err(daemon_error),
        ),
        Request::SetProvider(name) => Reply::Providers(
            runtime
                .block_on(Client::new().use_provider(&name))
                .map(|providers| providers.into_iter().map(map_provider).collect())
                .map_err(daemon_error),
        ),
        Request::Status => Reply::Status(runtime.block_on(async {
            Client::new()
                .status()
                .await
                .and_then(parse_status)
                .map_err(|error| error.to_string())
        })),
        Request::Toggle => Reply::Action(
            runtime
                .block_on(Client::new().toggle())
                .and_then(response_result)
                .map_err(|error| error.to_string()),
        ),
        Request::Cancel => Reply::Action(
            runtime
                .block_on(Client::new().cancel())
                .and_then(response_result)
                .map_err(|error| error.to_string()),
        ),
        Request::Retry(id) => Reply::Action(
            runtime
                .block_on(Client::new().send_command(crate::ipc::ACTION_RETRY, &[id.to_string()]))
                .and_then(response_result)
                .map_err(|error| error.to_string()),
        ),
        Request::CheckConnection => Reply::Connection(Ok(runtime.block_on(async {
            let daemon = Client::new()
                .status()
                .await
                .and_then(parse_status)
                .map(|_| ())
                .map_err(|error| error.to_string());
            let endpoint = if daemon.is_ok() {
                active_endpoint(&Client::new(), default_endpoint).await
            } else {
                default_endpoint.to_string()
            };
            let provider = probe_provider(&endpoint).await;
            ConnectionReport {
                daemon,
                provider,
                endpoint_host: endpoint_host(&endpoint),
                checked_at: Utc::now(),
            }
        }))),
        Request::Glossary => Reply::Glossary(
            crate::glossary::load(&crate::glossary::glossary_path())
                .map(Glossary::from_stored)
                .map_err(|error| format!("{error:#}")),
        ),
        Request::AddGlossaryTerm(term) => Reply::GlossaryAdded(
            crate::glossary::add(&crate::glossary::glossary_path(), &term)
                .map(|terms| Glossary::from_stored(Some(terms)))
                .map_err(|error| format!("{error:#}")),
        ),
        Request::RemoveGlossaryTerm(term) => Reply::Glossary(
            crate::glossary::remove(&crate::glossary::glossary_path(), &term)
                .map(|terms| Glossary::from_stored(Some(terms)))
                .map_err(|error| format!("{error:#}")),
        ),
        Request::RenameGlossaryTerm { from, to } => Reply::Glossary(
            crate::glossary::rename(&crate::glossary::glossary_path(), &from, &to)
                .map(|terms| Glossary::from_stored(Some(terms)))
                .map_err(|error| format!("{error:#}")),
        ),
        Request::History { .. }
        | Request::Detail(_)
        | Request::SaveRevision { .. }
        | Request::RestoreRevision { .. }
        | Request::Delete(_)
        | Request::Recent { .. }
        | Request::Stats => unreachable!("database request routed to service handler"),
    }
}

fn response_result(response: crate::ipc::Response) -> anyhow::Result<()> {
    if response.success {
        Ok(())
    } else {
        anyhow::bail!(response.error)
    }
}

fn parse_status(response: crate::ipc::Response) -> anyhow::Result<Status> {
    if !response.success {
        anyhow::bail!(response.error);
    }
    let state = match response
        .data
        .get(crate::ipc::DATA_KEY_STATE)
        .map(String::as_str)
        .unwrap_or("idle")
    {
        "idle" => DaemonState::Idle,
        "recording" => DaemonState::Recording,
        "transcribing" => DaemonState::Transcribing,
        "typing" => DaemonState::Typing,
        "error" => DaemonState::Error,
        value => anyhow::bail!("daemon returned unknown state {value:?}"),
    };
    Ok(Status {
        state,
        duration_ms: response
            .data
            .get(crate::ipc::DATA_KEY_RECORDING_DURATION)
            .map(|value| parse_go_duration_seconds(value) * 1000.0)
            .unwrap_or_default() as i64,
        error: response
            .data
            .get(crate::ipc::DATA_KEY_LAST_ERROR)
            .cloned()
            .unwrap_or_default(),
        recovered_text: response
            .data
            .get(crate::ipc::DATA_KEY_TEXT)
            .cloned()
            .unwrap_or_default(),
        uptime_seconds: response
            .data
            .get(crate::ipc::DATA_KEY_UPTIME)
            .map(|value| parse_go_duration_seconds(value) as u64)
            .unwrap_or_default(),
        last_recording_id: response
            .data
            .get(crate::ipc::DATA_KEY_LAST_RECORDING_ID)
            .and_then(|value| value.parse().ok()),
        last_recording_generation: response
            .data
            .get(crate::ipc::DATA_KEY_LAST_RECORDING_GENERATION)
            .and_then(|value| value.parse().ok())
            .unwrap_or_default(),
        audio_level_rms: response
            .data
            .get(crate::ipc::DATA_KEY_AUDIO_LEVEL_RMS)
            .and_then(|value| value.parse().ok())
            .unwrap_or_default(),
        audio_level_peak: response
            .data
            .get(crate::ipc::DATA_KEY_AUDIO_LEVEL_PEAK)
            .and_then(|value| value.parse().ok())
            .unwrap_or_default(),
        provider: response
            .data
            .get(crate::ipc::DATA_KEY_PROVIDER)
            .cloned()
            .unwrap_or_default(),
    })
}

fn parse_go_duration_seconds(value: &str) -> f64 {
    let mut total = 0.0;
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b'.') {
            index += 1;
        }
        let number = value[start..index].parse::<f64>().unwrap_or_default();
        let (factor, length) = if value[index..].starts_with("ms") {
            (0.001, 2)
        } else if value[index..].starts_with('h') {
            (3600.0, 1)
        } else if value[index..].starts_with('m') {
            (60.0, 1)
        } else if value[index..].starts_with('s') {
            (1.0, 1)
        } else {
            break;
        };
        total += number * factor;
        index += length;
    }
    total
}

fn map_recording(recording: crate::storage::RecordingSummary) -> Recording {
    Recording {
        id: recording.id,
        timestamp: recording.timestamp,
        duration_ms: recording.duration_ms,
        text: recording.text,
        audio_path: recording.audio_path.unwrap_or_default().into(),
        model: recording.model.unwrap_or_default(),
        failed: recording.status == RecordingStatus::Failed,
        error: recording.error.unwrap_or_default(),
        revision: recording.current_revision,
        attempts: recording.attempt_count.max(0) as usize,
    }
}

fn map_detail(detail: RecordingDetail) -> Detail {
    Detail {
        recording: map_recording(detail.recording),
        revisions: detail
            .revisions
            .into_iter()
            .map(|revision| Revision {
                number: revision.revision_no,
                timestamp: revision.created_at,
                text: revision.text,
                source: revision.source,
            })
            .collect(),
        attempts: detail
            .attempts
            .into_iter()
            .map(|attempt| Attempt {
                number: attempt.attempt_no,
                started_at: attempt.started_at,
                finished_at: attempt.finished_at,
                latency_ms: attempt.latency_ms,
                status: attempt.status,
                error: attempt.error.unwrap_or_default(),
                model: attempt.model.unwrap_or_default(),
            })
            .collect(),
    }
}

fn map_stats(stats: RecordingStats) -> Stats {
    Stats {
        total: stats.total_recordings,
        successful: stats.successful_recordings,
        failed: stats.failed_recordings,
        edited: stats.edited_recordings,
        words: stats.total_words,
        duration_ms: stats.total_duration_ms,
        today_words: stats.today_words,
        today_duration_ms: stats.today_duration_ms,
        daily: stats
            .daily_last_7
            .into_iter()
            .map(|day| (day.date, day.count))
            .collect(),
        daily_last_30: stats.daily_last_30,
        by_hour: stats.by_hour,
        average_duration_ms: stats.average_duration_ms,
        median_duration_ms: stats.median_duration_ms,
        this_week_words: stats.this_week_words,
        last_week_words: stats.last_week_words,
        models: stats
            .by_model
            .into_iter()
            .map(|model| {
                (
                    model.model,
                    model.recordings,
                    model.duration_ms,
                    model.latency_p95_ms,
                )
            })
            .collect(),
        latency_samples: stats.latency.recent_ms,
        p50: stats.latency.p50_ms,
        p95: stats.latency.p95_ms,
        p99: stats.latency.p99_ms,
        max: stats.latency.max_ms,
    }
}

fn map_microphone(microphone: crate::audio::Microphone) -> Microphone {
    Microphone {
        id: microphone.id,
        name: microphone.name,
        connected: microphone.connected,
        is_default: microphone.is_default,
        bluetooth: microphone.bluetooth,
    }
}

fn read_managed_settings(demo: bool) -> ManagedSettings {
    if demo {
        return ManagedSettings {
            managed_by_home_manager: true,
            provider: "siren".to_string(),
            model: "whisper-large-v3-turbo".to_string(),
            endpoint: "https://siren.example.test/v1/audio/transcriptions".to_string(),
            timeout_seconds: 60,
            max_duration_minutes: 5,
            paste_shortcut: "ctrl_shift_v".to_string(),
            notifications: "errors_only".to_string(),
            send_vocabulary: true,
        };
    }
    let config_path = crate::utils::CONFIG_DIR.join("config.json");
    let managed_by_home_manager = std::fs::symlink_metadata(&config_path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
        || std::fs::metadata(&config_path)
            .map(|metadata| metadata.permissions().readonly())
            .unwrap_or(false);
    std::fs::File::open(config_path)
        .ok()
        .and_then(|file| parse_managed_settings(file, managed_by_home_manager))
        .unwrap_or_else(|| ManagedSettings {
            managed_by_home_manager,
            provider: "openai".to_string(),
            model: "gpt-4o-transcribe".to_string(),
            endpoint: "https://api.openai.com/v1/audio/transcriptions".to_string(),
            timeout_seconds: 60,
            max_duration_minutes: 5,
            paste_shortcut: "ctrl_shift_v".to_string(),
            notifications: "errors_only".to_string(),
            send_vocabulary: false,
        })
}

fn parse_managed_settings(
    reader: impl std::io::Read,
    managed_by_home_manager: bool,
) -> Option<ManagedSettings> {
    #[derive(Default, serde::Deserialize)]
    struct ReadOnlyConfig {
        #[serde(default)]
        notifications: String,
        #[serde(default)]
        api: ReadOnlyApi,
        #[serde(default)]
        audio: ReadOnlyAudio,
        #[serde(default)]
        typing: ReadOnlyTyping,
    }
    #[derive(Default, serde::Deserialize)]
    struct ReadOnlyApi {
        #[serde(default)]
        active_provider: String,
        #[serde(default)]
        timeout: i64,
        #[serde(default)]
        providers: std::collections::BTreeMap<String, ReadOnlyProvider>,
    }
    #[derive(Default, serde::Deserialize)]
    struct ReadOnlyProvider {
        #[serde(default)]
        endpoint: String,
        #[serde(default)]
        model: String,
        #[serde(default)]
        send_vocabulary: bool,
    }
    #[derive(Default, serde::Deserialize)]
    struct ReadOnlyAudio {
        #[serde(default)]
        max_duration_min: i64,
    }
    #[derive(Default, serde::Deserialize)]
    struct ReadOnlyTyping {
        #[serde(default)]
        shortcut: String,
    }

    let config: ReadOnlyConfig = serde_json::from_reader(reader).ok()?;
    let provider_name = if config.api.active_provider.is_empty() {
        "openai".to_string()
    } else {
        config.api.active_provider
    };
    let provider = config.api.providers.get(&provider_name);
    Some(ManagedSettings {
        managed_by_home_manager,
        provider: provider_name,
        model: provider
            .map(|provider| provider.model.clone())
            .filter(|model| !model.is_empty())
            .unwrap_or_else(|| "gpt-4o-transcribe".to_string()),
        endpoint: provider
            .map(|provider| provider.endpoint.clone())
            .filter(|endpoint| !endpoint.is_empty())
            .unwrap_or_else(|| "https://api.openai.com/v1/audio/transcriptions".to_string()),
        timeout_seconds: if config.api.timeout > 0 {
            config.api.timeout
        } else {
            60
        },
        max_duration_minutes: if config.audio.max_duration_min > 0 {
            config.audio.max_duration_min
        } else {
            5
        },
        paste_shortcut: if config.typing.shortcut.is_empty() {
            "ctrl_shift_v".to_string()
        } else {
            config.typing.shortcut
        },
        notifications: if config.notifications.is_empty() {
            "errors_only".to_string()
        } else {
            config.notifications
        },
        send_vocabulary: provider.is_some_and(|provider| provider.send_vocabulary),
    })
}

fn run_demo_worker(requests: Receiver<Request>, replies: Sender<Reply>) {
    let mut recordings = demo_recordings();
    let mut revision_history: HashMap<i64, Vec<Revision>> = recordings
        .iter()
        .map(|recording| (recording.id, demo_initial_revisions(recording)))
        .collect();
    let mut microphones = demo_microphones();
    let mut providers = demo_providers();
    let mut glossary: Vec<String> = DEMO_GLOSSARY.map(str::to_string).to_vec();
    let mut status = Status {
        state: DaemonState::Idle,
        duration_ms: 0,
        error: String::new(),
        recovered_text: String::new(),
        uptime_seconds: 11_520,
        last_recording_id: None,
        last_recording_generation: 0,
        audio_level_rms: 0.0,
        audio_level_peak: 0.0,
        provider: "siren".to_string(),
    };
    while let Ok(request) = requests.recv() {
        let reply = match request {
            Request::History {
                search,
                filter,
                page,
            } => {
                let query = search.to_lowercase();
                let filtered: Vec<_> = recordings
                    .iter()
                    .filter(|recording| match filter {
                        Filter::All => true,
                        Filter::Edited => recording.revision > 0,
                        Filter::Failed => recording.failed,
                    })
                    .filter(|recording| {
                        query.is_empty()
                            || format!(
                                "{} {} {} {}",
                                recording.id, recording.text, recording.error, recording.model
                            )
                            .to_lowercase()
                            .contains(&query)
                    })
                    .cloned()
                    .collect();
                let start = page.saturating_mul(PAGE_SIZE);
                Reply::History(Ok(Page {
                    recordings: filtered
                        .iter()
                        .skip(start)
                        .take(PAGE_SIZE)
                        .cloned()
                        .collect(),
                    total: filtered.len() as i64,
                    has_next: start + PAGE_SIZE < filtered.len(),
                }))
            }
            Request::Detail(id) => Reply::Detail(
                recordings
                    .iter()
                    .find(|recording| recording.id == id)
                    .cloned()
                    .and_then(|recording| {
                        revision_history
                            .get(&id)
                            .cloned()
                            .map(|revisions| demo_detail(recording, revisions))
                    })
                    .ok_or_else(|| "recording not found".to_string()),
            ),
            Request::SaveRevision {
                id,
                expected_revision,
                text,
            } => {
                let result = recordings
                    .iter_mut()
                    .find(|recording| recording.id == id)
                    .ok_or_else(|| "recording not found".to_string())
                    .and_then(|recording| {
                        if recording.revision != expected_revision {
                            return Err("the transcript changed; reload before saving".to_string());
                        }
                        recording.text = text;
                        recording.revision += 1;
                        let revisions = revision_history.entry(id).or_default();
                        revisions.push(Revision {
                            number: recording.revision,
                            timestamp: Utc::now(),
                            text: recording.text.clone(),
                            source: "user".to_string(),
                        });
                        Ok(demo_detail(recording.clone(), revisions.clone()))
                    });
                Reply::Saved(result)
            }
            Request::RestoreRevision {
                id,
                revision,
                expected_revision,
            } => {
                let result = recordings
                    .iter_mut()
                    .find(|recording| recording.id == id)
                    .ok_or_else(|| "recording not found".to_string())
                    .and_then(|recording| {
                        if recording.revision != expected_revision {
                            return Err(
                                "the transcript changed; reload before restoring".to_string()
                            );
                        }
                        let revisions = revision_history.entry(id).or_default();
                        let restored_text = revisions
                            .iter()
                            .find(|item| item.number == revision)
                            .map(|item| item.text.clone())
                            .ok_or_else(|| "revision not found".to_string())?;
                        recording.text = restored_text;
                        recording.revision += 1;
                        revisions.push(Revision {
                            number: recording.revision,
                            timestamp: Utc::now(),
                            text: recording.text.clone(),
                            source: "restore".to_string(),
                        });
                        Ok(demo_detail(recording.clone(), revisions.clone()))
                    });
                Reply::Saved(result)
            }
            Request::Stats => Reply::Stats(Ok(demo_stats(&recordings))),
            Request::Recent { limit } => {
                Reply::Recent(Ok(recordings.iter().take(limit).cloned().collect()))
            }
            Request::Microphones => Reply::Microphones(Ok(microphones.clone())),
            Request::ReorderMicrophones(ids) => {
                microphones.sort_by_key(|microphone| {
                    ids.iter()
                        .position(|id| id == &microphone.id)
                        .unwrap_or(usize::MAX)
                });
                Reply::Microphones(Ok(microphones.clone()))
            }
            Request::RemoveMicrophone(id) => Reply::Microphones(
                match microphones
                    .iter()
                    .position(|microphone| microphone.id == id)
                {
                    Some(index) if microphones[index].connected => Err(format!(
                        "\"{}\" is connected; disconnect it before removing it, or it is added back on the next discovery",
                        microphones[index].name
                    )),
                    Some(index) => {
                        microphones.remove(index);
                        Ok(microphones.clone())
                    }
                    None => Err(format!("no saved microphone has the id {id:?}")),
                },
            ),
            Request::Providers => Reply::Providers(Ok(providers.clone())),
            Request::SetProvider(name) => {
                let result = demo_set_provider(&mut providers, &name);
                if let Some(active) = providers.iter().find(|provider| provider.active) {
                    status.provider.clone_from(&active.name);
                }
                Reply::Providers(result)
            }
            Request::Status => Reply::Status(Ok(status.clone())),
            Request::Toggle => {
                status.state = if status.state == DaemonState::Recording {
                    DaemonState::Transcribing
                } else {
                    DaemonState::Recording
                };
                status.duration_ms = if status.state == DaemonState::Recording {
                    7_400
                } else {
                    0
                };
                Reply::Action(Ok(()))
            }
            Request::Cancel => {
                status.state = DaemonState::Idle;
                status.duration_ms = 0;
                Reply::Action(Ok(()))
            }
            Request::Retry(_) => Reply::Action(Ok(())),
            Request::Delete(id) => {
                let position = recordings.iter().position(|recording| recording.id == id);
                Reply::Deleted(match position {
                    Some(index) => {
                        recordings.remove(index);
                        revision_history.remove(&id);
                        Ok(id)
                    }
                    None => Err("recording not found".to_string()),
                })
            }
            Request::CheckConnection => Reply::Connection(Ok(ConnectionReport {
                daemon: Ok(()),
                provider: Ok("HTTP 405 in 84 ms".to_string()),
                endpoint_host: providers
                    .iter()
                    .find(|provider| provider.active)
                    .map(|provider| endpoint_host(&provider.endpoint))
                    .unwrap_or_default(),
                checked_at: Utc::now(),
            })),
            Request::Glossary => Reply::Glossary(Ok(Glossary::from_stored(Some(glossary.clone())))),
            Request::AddGlossaryTerm(term) => Reply::GlossaryAdded(
                crate::glossary::insert_term(&mut glossary, &term)
                    .map(|()| Glossary::from_stored(Some(glossary.clone()))),
            ),
            Request::RemoveGlossaryTerm(term) => Reply::Glossary(
                crate::glossary::delete_term(&mut glossary, &term)
                    .map(|()| Glossary::from_stored(Some(glossary.clone()))),
            ),
            Request::RenameGlossaryTerm { from, to } => Reply::Glossary(
                crate::glossary::replace_term(&mut glossary, &from, &to)
                    .map(|()| Glossary::from_stored(Some(glossary.clone()))),
            ),
        };
        if replies.send(reply).is_err() {
            break;
        }
    }
}

const DEMO_GLOSSARY: [&str; 30] = [
    "Claude",
    "tmux",
    "Sietch",
    "home-manager",
    "NixOS",
    "PipeWire",
    "Wayland",
    "niri",
    "Tailscale",
    "Kubernetes",
    "PostgreSQL",
    "SQLite",
    "rustfmt",
    "clippy",
    "tokio",
    "serde",
    "GPUI",
    "Zellij",
    "Neovim",
    "ripgrep",
    "Grafana",
    "Prometheus",
    "OAuth",
    "WebSocket",
    "GraphQL",
    "TypeScript",
    "Vite",
    "pnpm",
    "Hyprland",
    "Whisper",
];

fn demo_microphones() -> Vec<Microphone> {
    let microphone = |id: &str, name: &str, connected, is_default, bluetooth| Microphone {
        id: id.to_string(),
        name: name.to_string(),
        connected,
        is_default,
        bluetooth,
    };
    vec![
        microphone(
            "usb:046d:0825:046d_C270_HD_WEBCAM_4F2C0660",
            "Webcam C270 Mono",
            true,
            false,
            false,
        ),
        microphone(
            "bus:pci-0000:c1:00.6",
            "Ryzen HD Audio Controller Analog Stereo",
            true,
            true,
            false,
        ),
        microphone(
            "bluetooth:88:C9:E8:16:C7:9C",
            "WH-1000XM4",
            true,
            false,
            true,
        ),
        microphone("usb:0d8c:0014", "BY-GM18CU Mono", false, false, false),
        microphone(
            "usb:b58e:9e84:REV8-2024",
            "Yeti Stereo Microphone",
            false,
            false,
            false,
        ),
        microphone(
            "bluetooth:F4:65:A6:0B:21:9E",
            "AirPods Pro",
            false,
            false,
            true,
        ),
        microphone(
            "bus:pci-0000:c3:00.4-usb-0:1.2:1.0",
            "USB Audio Device Mono",
            false,
            false,
            false,
        ),
    ]
}

fn demo_providers() -> Vec<ProviderInfo> {
    vec![
        ProviderInfo {
            name: "groq".to_string(),
            model: "whisper-large-v3".to_string(),
            endpoint: "https://api.groq.com/openai/v1/audio/transcriptions".to_string(),
            usable: false,
            problem: "GROQ_API_KEY is not set in the daemon's environment".to_string(),
            active: false,
            is_default: false,
            send_vocabulary: false,
        },
        ProviderInfo {
            name: "openai".to_string(),
            model: "gpt-4o-transcribe".to_string(),
            endpoint: "https://api.openai.com/v1/audio/transcriptions".to_string(),
            usable: true,
            problem: String::new(),
            active: false,
            is_default: false,
            send_vocabulary: false,
        },
        ProviderInfo {
            name: "siren".to_string(),
            model: "whisper-large-v3-turbo".to_string(),
            endpoint: "https://siren.example.test/v1/audio/transcriptions".to_string(),
            usable: true,
            problem: String::new(),
            active: true,
            is_default: true,
            send_vocabulary: true,
        },
    ]
}

/// Mirrors the daemon's checks and messages.
fn demo_set_provider(
    providers: &mut [ProviderInfo],
    name: &str,
) -> Result<Vec<ProviderInfo>, String> {
    let name = name.trim();
    let Some(selected) = providers.iter().find(|provider| provider.name == name) else {
        let known: Vec<&str> = providers
            .iter()
            .map(|provider| provider.name.as_str())
            .collect();
        return Err(format!(
            "unknown provider '{name}'; configured providers: {}",
            known.join(", ")
        ));
    };
    if !selected.usable {
        return Err(format!(
            "provider '{name}' cannot be used: {}",
            selected.problem
        ));
    }
    for provider in providers.iter_mut() {
        provider.active = provider.name == name;
    }
    Ok(providers.to_vec())
}

fn demo_recordings() -> Vec<Recording> {
    let samples = [
        "Draft the release notes and link the migration guide.",
        "Follow up on the meeting notes after lunch.",
        "The audio capture works, but the upload needs a clearer progress state.",
        "Please update the design review with the latest screenshots.",
        "Search the transcript history for the deployment checklist.",
    ];
    (0..2400)
        .map(|index| {
            let failed = index % 53 == 0;
            Recording {
                id: 10_000 + index,
                timestamp: Utc::now() - Duration::minutes(index * 37),
                duration_ms: 4_000 + (index % 29) * 700,
                text: if failed {
                    String::new()
                } else {
                    samples[index as usize % samples.len()].to_string()
                },
                audio_path: PathBuf::from(format!("/tmp/dictator-demo-{index}.wav")),
                model: if index % 3 == 0 {
                    "whisper-1".to_string()
                } else {
                    "whisper-large-v3-turbo".to_string()
                },
                failed,
                error: if failed {
                    "transcription failed: provider timeout".to_string()
                } else {
                    String::new()
                },
                revision: if !failed && index % 7 == 0 { 1 } else { 0 },
                attempts: if failed { 2 } else { 1 },
            }
        })
        .collect()
}

fn demo_initial_revisions(recording: &Recording) -> Vec<Revision> {
    let mut revisions = vec![Revision {
        number: 0,
        timestamp: recording.timestamp,
        text: demo_original_text(recording.id),
        source: "transcription".to_string(),
    }];
    if recording.revision > 0 {
        revisions.push(Revision {
            number: recording.revision,
            timestamp: recording.timestamp + Duration::seconds(42),
            text: recording.text.clone(),
            source: "user".to_string(),
        });
    }
    revisions
}

fn demo_detail(recording: Recording, revisions: Vec<Revision>) -> Detail {
    let attempts = (1..=recording.attempts)
        .map(|number| Attempt {
            number: number as i64,
            started_at: recording.timestamp + Duration::seconds((number - 1) as i64),
            finished_at: recording.timestamp
                + Duration::seconds((number - 1) as i64)
                + Duration::milliseconds(1_240),
            latency_ms: Some(1_240),
            status: if recording.failed {
                "error"
            } else {
                "complete"
            }
            .to_string(),
            error: recording.error.clone(),
            model: recording.model.clone(),
        })
        .collect();
    Detail {
        recording,
        revisions,
        attempts,
    }
}

fn demo_original_text(id: i64) -> String {
    let samples = [
        "Draft the release notes and link the migration guide.",
        "Follow up on the meeting notes after lunch.",
        "The audio capture works, but the upload needs a clearer progress state.",
        "Please update the design review with the latest screenshots.",
        "Search the transcript history for the deployment checklist.",
    ];
    let index = id.saturating_sub(10_000) as usize;
    if index.is_multiple_of(53) {
        String::new()
    } else {
        samples[index % samples.len()].to_string()
    }
}

fn demo_stats(recordings: &[Recording]) -> Stats {
    let words = recordings
        .iter()
        .map(|recording| recording.text.split_whitespace().count() as i64)
        .sum();
    let duration_ms = recordings
        .iter()
        .map(|recording| recording.duration_ms)
        .sum();
    let today = NaiveDate::from_ymd_opt(2026, 9, 18).expect("valid demo date");
    Stats {
        total: recordings.len() as i64,
        successful: recordings
            .iter()
            .filter(|recording| !recording.failed)
            .count() as i64,
        failed: recordings
            .iter()
            .filter(|recording| recording.failed)
            .count() as i64,
        edited: recordings
            .iter()
            .filter(|recording| recording.revision > 0)
            .count() as i64,
        words,
        duration_ms,
        today_words: 438,
        today_duration_ms: 186_000,
        daily: (0..7)
            .rev()
            .map(|days| (today - Duration::days(days), 16 + days * 7))
            .collect(),
        daily_last_30: (0..30)
            .rev()
            .map(|days| DailyActivity {
                date: today - Duration::days(days),
                recordings: 12 + (days * 5) % 17,
                words: 180 + (days * 71) % 260,
                duration_ms: 90_000 + (days * 13_000) % 120_000,
            })
            .collect(),
        by_hour: std::array::from_fn(|hour| match hour {
            9..=11 => 140 + hour as i64 * 9,
            12..=17 => 90 + hour as i64 * 4,
            18..=21 => 30,
            _ => 4,
        }),
        average_duration_ms: Some(9_800),
        median_duration_ms: Some(8_400),
        this_week_words: 2_140,
        last_week_words: 1_870,
        models: vec![
            (
                "whisper-large-v3-turbo".to_string(),
                1600,
                8_200_000,
                Some(1420),
            ),
            ("whisper-1".to_string(), 754, 4_100_000, Some(2180)),
        ],
        latency_samples: (0..60).map(|index| 350 + (index * 137) % 2500).collect(),
        p50: Some(990),
        p95: Some(2520),
        p99: Some(2790),
        max: Some(2850),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_preserves_recovered_text_after_persistence_failure() {
        let mut response = crate::ipc::Response {
            success: true,
            ..Default::default()
        };
        response
            .data
            .insert(crate::ipc::DATA_KEY_STATE.into(), "error".into());
        response.data.insert(
            crate::ipc::DATA_KEY_LAST_ERROR.into(),
            "failed to save recording".into(),
        );
        response.data.insert(
            crate::ipc::DATA_KEY_TEXT.into(),
            "Recovered provider transcript".into(),
        );

        let status = parse_status(response).expect("valid status");

        assert_eq!(status.state, DaemonState::Error);
        assert_eq!(status.error, "failed to save recording");
        assert_eq!(status.recovered_text, "Recovered provider transcript");
        assert_eq!(status.last_recording_id, None);
        assert_eq!(status.last_recording_generation, 0);
    }

    #[test]
    fn demo_keeps_each_saved_and_restored_revision() {
        let (request_tx, request_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_demo_worker(request_rx, reply_tx));

        request_tx
            .send(Request::SaveRevision {
                id: 10_001,
                expected_revision: 0,
                text: "first edit".into(),
            })
            .expect("demo worker is running");
        let Reply::Saved(Ok(first)) = reply_rx.recv().expect("first reply") else {
            panic!("expected first saved detail");
        };
        assert_eq!(first.revisions.len(), 2);
        assert_eq!(first.revisions[0].text, demo_original_text(10_001));
        assert_eq!(first.revisions[1].text, "first edit");

        request_tx
            .send(Request::SaveRevision {
                id: 10_001,
                expected_revision: 1,
                text: "second edit".into(),
            })
            .expect("demo worker is running");
        let Reply::Saved(Ok(second)) = reply_rx.recv().expect("second reply") else {
            panic!("expected second saved detail");
        };
        assert_eq!(second.revisions.len(), 3);
        assert_eq!(second.revisions[1].text, "first edit");
        assert_eq!(second.revisions[2].text, "second edit");

        request_tx
            .send(Request::RestoreRevision {
                id: 10_001,
                revision: 0,
                expected_revision: 2,
            })
            .expect("demo worker is running");
        let Reply::Saved(Ok(restored)) = reply_rx.recv().expect("restore reply") else {
            panic!("expected restored detail");
        };
        assert_eq!(restored.recording.revision, 3);
        assert_eq!(restored.revisions.len(), 4);
        assert_eq!(restored.revisions[3].source, "restore");
        assert_eq!(restored.revisions[3].text, demo_original_text(10_001));

        drop(request_tx);
        worker.join().expect("demo worker exits cleanly");
    }

    #[test]
    fn managed_settings_project_only_nonsecret_provider_fields() {
        let json = br#"{
            "notifications": "all",
            "api": {
                "active_provider": "local",
                "timeout": 42,
                "providers": {
                    "local": {
                        "endpoint": "http://127.0.0.1:8080/v1",
                        "model": "whisper-local",
                        "key": "must-not-enter-gui-state",
                        "send_vocabulary": true
                    }
                }
            },
            "audio": { "max_duration_min": 9 },
            "typing": { "shortcut": "ctrl_v" }
        }"#;

        let settings = parse_managed_settings(&json[..], true).expect("valid projection");

        assert!(settings.managed_by_home_manager);
        assert_eq!(settings.provider, "local");
        assert_eq!(settings.endpoint, "http://127.0.0.1:8080/v1");
        assert_eq!(settings.model, "whisper-local");
        assert_eq!(settings.timeout_seconds, 42);
        assert_eq!(settings.max_duration_minutes, 9);
        assert_eq!(settings.paste_shortcut, "ctrl_v");
        assert_eq!(settings.notifications, "all");
        assert!(settings.send_vocabulary);
        assert!(!format!("{settings:?}").contains("must-not-enter-gui-state"));
    }

    #[test]
    fn failed_worker_replies_in_kind_and_stays_alive() {
        let (request_tx, request_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            run_failed_worker(request_rx, reply_tx, "runtime unavailable".to_string())
        });

        request_tx.send(Request::Status).expect("status request");
        request_tx
            .send(Request::History {
                search: String::new(),
                filter: Filter::All,
                page: 0,
            })
            .expect("history request");
        request_tx
            .send(Request::SaveRevision {
                id: 7,
                expected_revision: 1,
                text: "edit".to_string(),
            })
            .expect("save request");
        request_tx.send(Request::Retry(7)).expect("retry request");

        assert!(
            matches!(reply_rx.recv(), Ok(Reply::Status(Err(error))) if error == "runtime unavailable")
        );
        assert!(
            matches!(reply_rx.recv(), Ok(Reply::History(Err(error))) if error == "runtime unavailable")
        );
        assert!(
            matches!(reply_rx.recv(), Ok(Reply::Saved(Err(error))) if error == "runtime unavailable")
        );
        assert!(
            matches!(reply_rx.recv(), Ok(Reply::Action(Err(error))) if error == "runtime unavailable")
        );

        drop(request_tx);
        worker.join().expect("failed worker exits cleanly");
    }

    #[test]
    fn database_reopens_only_for_a_later_read() {
        let mut database = None;
        let mut error = "Database unavailable: transient".to_string();
        let mut attempts = 0;

        let mutation = Request::SaveRevision {
            id: 7,
            expected_revision: 1,
            text: "edit".to_string(),
        };
        reopen_database_for_request(&mutation, &mut database, &mut error, || {
            attempts += 1;
            Ok(41)
        });
        assert_eq!(attempts, 0, "mutations must not trigger an implicit retry");
        assert_eq!(database, None);

        let read = Request::History {
            search: String::new(),
            filter: Filter::All,
            page: 0,
        };
        reopen_database_for_request(&read, &mut database, &mut error, || {
            attempts += 1;
            Ok(41)
        });
        assert_eq!(attempts, 1);
        assert_eq!(database, Some(41));
        assert!(error.is_empty());
    }

    #[test]
    fn service_requests_do_not_require_a_database() {
        assert_eq!(
            live_request_kind(&Request::Status),
            LiveRequestKind::Service
        );
        assert_eq!(
            live_request_kind(&Request::Toggle),
            LiveRequestKind::Service
        );
        assert_eq!(
            live_request_kind(&Request::Retry(7)),
            LiveRequestKind::Service
        );
        assert_eq!(
            live_request_kind(&Request::Microphones),
            LiveRequestKind::Service
        );
        assert_eq!(
            live_request_kind(&Request::AddGlossaryTerm("x".into())),
            LiveRequestKind::Service
        );
    }

    #[test]
    fn demo_glossary_validates_adds_and_removes() {
        let (request_tx, request_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_demo_worker(request_rx, reply_tx));

        request_tx.send(Request::Glossary).unwrap();
        let Reply::Glossary(Ok(initial)) = reply_rx.recv().unwrap() else {
            panic!("expected the glossary");
        };
        assert!(initial.exists);

        request_tx
            .send(Request::AddGlossaryTerm("  Arrakis ".into()))
            .unwrap();
        let Reply::GlossaryAdded(Ok(added)) = reply_rx.recv().unwrap() else {
            panic!("expected an added glossary");
        };
        assert_eq!(added.terms.last().map(String::as_str), Some("Arrakis"));

        request_tx
            .send(Request::AddGlossaryTerm("arrakis".into()))
            .unwrap();
        let Reply::GlossaryAdded(Err(error)) = reply_rx.recv().unwrap() else {
            panic!("expected a duplicate error");
        };
        assert!(error.contains("already in the glossary"), "{error}");

        request_tx
            .send(Request::RemoveGlossaryTerm("Arrakis".into()))
            .unwrap();
        let Reply::Glossary(Ok(removed)) = reply_rx.recv().unwrap() else {
            panic!("expected the glossary");
        };
        assert_eq!(removed, initial);

        drop(request_tx);
        worker.join().unwrap();
    }

    #[test]
    fn new_requests_route_to_services_and_fail_in_kind() {
        let requests = || {
            [
                Request::Providers,
                Request::SetProvider("openai".into()),
                Request::RemoveMicrophone("usb:1".into()),
                Request::RenameGlossaryTerm {
                    from: "a".into(),
                    to: "b".into(),
                },
            ]
        };
        for request in requests() {
            assert_eq!(live_request_kind(&request), LiveRequestKind::Service);
        }
        let replies: Vec<_> = requests()
            .into_iter()
            .map(|request| failed_reply(request, "down"))
            .collect();
        assert!(matches!(&replies[0], Reply::Providers(Err(error)) if error == "down"));
        assert!(matches!(&replies[1], Reply::Providers(Err(error)) if error == "down"));
        assert!(matches!(&replies[2], Reply::Microphones(Err(error)) if error == "down"));
        assert!(matches!(&replies[3], Reply::Glossary(Err(error)) if error == "down"));
    }

    #[test]
    fn daemon_errors_are_short_when_the_daemon_is_down() {
        assert_eq!(
            daemon_error(anyhow::anyhow!(
                "failed to connect to daemon: No such file or directory (os error 2)"
            )),
            "The dictator daemon is not running"
        );
        assert_eq!(
            daemon_error(anyhow::anyhow!("unknown provider 'x'")),
            "unknown provider 'x'"
        );
    }

    struct ProviderHandler;

    impl crate::ipc::CommandHandler for ProviderHandler {
        fn handle_start(&self, _received_at: Instant) -> anyhow::Result<()> {
            Ok(())
        }
        fn handle_stop(&self, _received_at: Instant) -> anyhow::Result<()> {
            Ok(())
        }
        fn handle_toggle(&self, _received_at: Instant) -> anyhow::Result<()> {
            Ok(())
        }
        fn handle_cancel(&self) -> anyhow::Result<()> {
            Ok(())
        }
        fn get_status(&self) -> crate::ipc::StatusData {
            crate::ipc::StatusData {
                state: DaemonState::Idle,
                recording_duration: None,
                last_error: None,
                uptime: std::time::Duration::ZERO,
            }
        }
        fn list_providers(&self) -> anyhow::Result<Vec<crate::ipc::ProviderSummary>> {
            Ok(vec![crate::ipc::ProviderSummary {
                name: "openai".into(),
                model: "gpt-4o-transcribe".into(),
                endpoint: "https://api.openai.com/v1/audio/transcriptions".into(),
                usable: true,
                problem: String::new(),
                active: true,
                is_default: false,
                send_vocabulary: false,
            }])
        }
    }

    #[tokio::test]
    async fn connection_check_probes_the_daemons_active_provider() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("dictator.sock");
        let default = "https://siren.example.test/v1";

        let missing = Client::with_path(dir.path().join("missing.sock"));
        assert_eq!(active_endpoint(&missing, default).await, default);

        let server =
            crate::ipc::Server::with_path(std::sync::Arc::new(ProviderHandler), socket.clone());
        server.start().await.unwrap();
        let endpoint = active_endpoint(&Client::with_path(socket), default).await;
        server.stop().await.unwrap();
        assert_eq!(endpoint, "https://api.openai.com/v1/audio/transcriptions");
        assert_eq!(endpoint_host(&endpoint), "api.openai.com");
    }

    #[test]
    fn demo_providers_switch_and_reject_unusable_ones() {
        let (request_tx, request_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_demo_worker(request_rx, reply_tx));
        let active = |providers: &[ProviderInfo]| {
            providers
                .iter()
                .filter(|provider| provider.active)
                .map(|provider| provider.name.clone())
                .collect::<Vec<_>>()
        };

        request_tx.send(Request::Providers).unwrap();
        let Reply::Providers(Ok(initial)) = reply_rx.recv().unwrap() else {
            panic!("expected providers");
        };
        let siren = initial.iter().find(|p| p.name == "siren").unwrap();
        assert!(siren.is_default && siren.active && siren.usable && siren.send_vocabulary);
        assert!(initial.iter().any(|p| p.name == "openai" && p.usable));
        assert!(
            initial
                .iter()
                .any(|p| !p.usable && !p.problem.is_empty() && p.problem.contains("not set"))
        );

        request_tx
            .send(Request::SetProvider("openai".into()))
            .unwrap();
        let Reply::Providers(Ok(switched)) = reply_rx.recv().unwrap() else {
            panic!("expected providers");
        };
        assert_eq!(active(&switched), ["openai"]);

        request_tx.send(Request::CheckConnection).unwrap();
        let Reply::Connection(Ok(report)) = reply_rx.recv().unwrap() else {
            panic!("expected a connection report");
        };
        assert_eq!(report.endpoint_host, "api.openai.com");

        for (name, expected) in [
            (
                "groq",
                "provider 'groq' cannot be used: GROQ_API_KEY is not set in the daemon's environment",
            ),
            (
                "nope",
                "unknown provider 'nope'; configured providers: groq, openai, siren",
            ),
        ] {
            request_tx.send(Request::SetProvider(name.into())).unwrap();
            let Reply::Providers(Err(error)) = reply_rx.recv().unwrap() else {
                panic!("expected an error for {name}");
            };
            assert_eq!(error, expected);
        }
        request_tx.send(Request::Providers).unwrap();
        let Reply::Providers(Ok(after)) = reply_rx.recv().unwrap() else {
            panic!("expected providers");
        };
        assert_eq!(
            active(&after),
            ["openai"],
            "a rejected switch changes nothing"
        );

        drop(request_tx);
        worker.join().unwrap();
    }

    #[test]
    fn demo_microphones_fill_a_list_and_remove_only_disconnected_ones() {
        let (request_tx, request_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_demo_worker(request_rx, reply_tx));

        request_tx.send(Request::Microphones).unwrap();
        let Reply::Microphones(Ok(initial)) = reply_rx.recv().unwrap() else {
            panic!("expected microphones");
        };
        assert!(initial.len() >= 5);
        assert!(initial.iter().any(|m| m.connected && !m.bluetooth));
        assert!(initial.iter().any(|m| !m.connected));
        assert!(initial.iter().any(|m| m.is_default));
        assert!(initial.iter().any(|m| m.bluetooth));

        let disconnected = initial.iter().find(|m| !m.connected).unwrap().clone();
        request_tx
            .send(Request::RemoveMicrophone(disconnected.id.clone()))
            .unwrap();
        let Reply::Microphones(Ok(removed)) = reply_rx.recv().unwrap() else {
            panic!("expected microphones");
        };
        assert_eq!(removed.len(), initial.len() - 1);
        assert!(removed.iter().all(|m| m.id != disconnected.id));

        let connected = initial.iter().find(|m| m.connected).unwrap().clone();
        request_tx
            .send(Request::RemoveMicrophone(connected.id.clone()))
            .unwrap();
        let Reply::Microphones(Err(error)) = reply_rx.recv().unwrap() else {
            panic!("expected an error");
        };
        assert!(error.contains("is connected"), "{error}");
        request_tx
            .send(Request::RemoveMicrophone(disconnected.id))
            .unwrap();
        assert!(matches!(
            reply_rx.recv().unwrap(),
            Reply::Microphones(Err(_))
        ));

        drop(request_tx);
        worker.join().unwrap();
    }

    #[test]
    fn demo_glossary_renames_with_the_real_rules() {
        let (request_tx, request_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_demo_worker(request_rx, reply_tx));

        request_tx.send(Request::Glossary).unwrap();
        let Reply::Glossary(Ok(initial)) = reply_rx.recv().unwrap() else {
            panic!("expected the glossary");
        };
        assert_eq!(initial.terms.len(), 30);
        assert_eq!(
            crate::glossary::sanitize(&initial.terms).1,
            Vec::<String>::new()
        );

        request_tx
            .send(Request::RenameGlossaryTerm {
                from: "tmux".into(),
                to: " Tmux ".into(),
            })
            .unwrap();
        let Reply::Glossary(Ok(renamed)) = reply_rx.recv().unwrap() else {
            panic!("expected the glossary");
        };
        assert_eq!(renamed.terms[1], "Tmux");
        assert_eq!(renamed.terms.len(), initial.terms.len());

        request_tx
            .send(Request::RenameGlossaryTerm {
                from: "Tmux".into(),
                to: "claude".into(),
            })
            .unwrap();
        let Reply::Glossary(Err(error)) = reply_rx.recv().unwrap() else {
            panic!("expected a duplicate error");
        };
        assert_eq!(error, "\"claude\" is already in the glossary");

        drop(request_tx);
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn provider_probe_counts_any_http_response_as_reachable() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/v1/audio/transcriptions",
            listener.local_addr().unwrap()
        );
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 1024];
            let _ = stream.read(&mut buffer).await;
            stream
                .write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });

        let report = probe_provider(&endpoint).await.expect("405 is reachable");
        assert!(report.starts_with("HTTP 405 in "), "{report}");
        assert!(report.ends_with(" ms"), "{report}");
    }

    #[tokio::test]
    async fn provider_probe_reports_closed_port_and_missing_endpoint() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1", closed.local_addr().unwrap());
        drop(closed);

        let error = probe_provider(&endpoint).await.unwrap_err();
        assert!(error.starts_with("connection failed: "), "{error}");
        assert_eq!(
            probe_provider("  ").await.unwrap_err(),
            "no provider endpoint configured"
        );
    }

    #[test]
    fn delete_and_connection_requests_route_like_their_peers() {
        assert_eq!(
            live_request_kind(&Request::Delete(7)),
            LiveRequestKind::DatabaseMutation
        );
        assert_eq!(
            live_request_kind(&Request::CheckConnection),
            LiveRequestKind::Service
        );
        assert!(matches!(
            failed_reply(Request::Delete(7), "down"),
            Reply::Deleted(Err(error)) if error == "down"
        ));
        assert!(matches!(
            failed_reply(Request::CheckConnection, "down"),
            Reply::Connection(Err(error)) if error == "down"
        ));
    }

    #[test]
    fn endpoint_host_extracts_the_host_or_nothing() {
        assert_eq!(
            endpoint_host("https://siren.example.test/v1/audio/transcriptions"),
            "siren.example.test"
        );
        assert_eq!(endpoint_host("http://127.0.0.1:8080/v1"), "127.0.0.1");
        assert_eq!(endpoint_host(""), "");
        assert_eq!(endpoint_host("not a url"), "");
    }

    #[test]
    fn recent_requests_read_the_database_and_fail_in_kind() {
        assert_eq!(
            live_request_kind(&Request::Recent { limit: 5 }),
            LiveRequestKind::DatabaseRead
        );
        assert!(matches!(
            failed_reply(Request::Recent { limit: 5 }, "down"),
            Reply::Recent(Err(error)) if error == "down"
        ));
    }

    #[test]
    fn live_recent_returns_newest_first_with_all_statuses() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.db")).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        db.save_transcript_attempt(1_000, "older", "/tmp/older.wav", "m", Some(10))
            .unwrap();
        let failed = db
            .save_failed_attempt(500, Some("/tmp/failed.wav"), "boom", Some("m"), None)
            .unwrap();

        let Reply::Recent(Ok(recent)) =
            handle_live(&db, &runtime, "", Request::Recent { limit: 1 })
        else {
            panic!("expected recent reply");
        };
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, failed);
        assert!(recent[0].failed);

        let Reply::Recent(Ok(all)) = handle_live(&db, &runtime, "", Request::Recent { limit: 0 })
        else {
            panic!("expected recent reply");
        };
        assert_eq!(all.len(), 1, "a zero limit is clamped to one");
    }

    #[test]
    fn demo_recent_returns_at_most_limit_recordings() {
        let (request_tx, request_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_demo_worker(request_rx, reply_tx));

        request_tx.send(Request::Recent { limit: 3 }).unwrap();
        let Reply::Recent(Ok(recent)) = reply_rx.recv().unwrap() else {
            panic!("expected recent reply");
        };
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].id, 10_000);

        drop(request_tx);
        worker.join().expect("demo worker exits cleanly");
    }

    #[test]
    fn live_delete_removes_the_audio_file_best_effort() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("app.db")).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let audio = dir.path().join("clip.wav");
        std::fs::write(&audio, b"wav").unwrap();
        let id = db
            .save_transcript_attempt(1_000, "text", audio.to_str().unwrap(), "m", Some(10))
            .unwrap();
        let missing_audio = db
            .save_transcript_attempt(
                1_000,
                "text",
                dir.path().join("gone.wav").to_str().unwrap(),
                "m",
                Some(10),
            )
            .unwrap();

        assert!(matches!(
            handle_live(&db, &runtime, "", Request::Delete(id)),
            Reply::Deleted(Ok(deleted)) if deleted == id
        ));
        assert!(!audio.exists());
        assert!(matches!(
            handle_live(&db, &runtime, "", Request::Delete(missing_audio)),
            Reply::Deleted(Ok(deleted)) if deleted == missing_audio
        ));
        assert!(
            matches!(
                handle_live(&db, &runtime, "", Request::Delete(id)),
                Reply::Deleted(Ok(deleted)) if deleted == id
            ),
            "deleting an already-deleted recording is idempotent"
        );
    }

    #[test]
    fn demo_delete_removes_the_recording() {
        let (request_tx, request_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let worker = thread::spawn(move || run_demo_worker(request_rx, reply_tx));

        request_tx.send(Request::Delete(10_001)).unwrap();
        assert!(matches!(reply_rx.recv(), Ok(Reply::Deleted(Ok(10_001)))));
        request_tx.send(Request::Detail(10_001)).unwrap();
        assert!(matches!(reply_rx.recv(), Ok(Reply::Detail(Err(_)))));
        request_tx.send(Request::Delete(10_001)).unwrap();
        assert!(matches!(reply_rx.recv(), Ok(Reply::Deleted(Err(_)))));

        drop(request_tx);
        worker.join().expect("demo worker exits cleanly");
    }
}
