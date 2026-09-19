use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use chrono::{DateTime, Duration, NaiveDate, Utc};

use crate::ipc::{Client, DaemonState};
use crate::storage::{
    Db, HistoryFilter, HistoryQuery, RecordingDetail, RecordingStats, RecordingStatus,
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
}

#[derive(Clone, Debug, Default)]
pub struct ManagedSettings {
    pub managed_by_home_manager: bool,
    pub provider: String,
    pub model: String,
    pub endpoint: String,
    pub timeout_seconds: i64,
    pub max_duration_minutes: i64,
    pub paste_shortcut: String,
    pub notifications: String,
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
    Status,
    Toggle,
    Cancel,
    Retry(i64),
}

#[derive(Debug)]
pub enum Reply {
    History(Result<Page, String>),
    Detail(Result<Detail, String>),
    Saved(Result<Detail, String>),
    Stats(Result<Stats, String>),
    Microphones(Result<Vec<Microphone>, String>),
    Status(Result<Status, String>),
    Action(Result<(), String>),
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
        thread::Builder::new()
            .name("dictator-gui-backend".to_string())
            .spawn(move || {
                if demo {
                    run_demo_worker(request_rx, reply_tx);
                } else {
                    run_live_worker(request_rx, reply_tx);
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

fn run_live_worker(requests: Receiver<Request>, replies: Sender<Reply>) {
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
    let db = match Db::new() {
        Ok(db) => db,
        Err(error) => {
            run_failed_worker(requests, replies, error.to_string());
            return;
        }
    };
    while let Ok(request) = requests.recv() {
        let reply = handle_live(&db, &runtime, request);
        if replies.send(reply).is_err() {
            break;
        }
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
        Request::Microphones | Request::ReorderMicrophones(_) => {
            Reply::Microphones(Err(message.to_string()))
        }
        Request::Status => Reply::Status(Err(message.to_string())),
        Request::Toggle | Request::Cancel | Request::Retry(_) => {
            Reply::Action(Err(message.to_string()))
        }
    }
}

fn handle_live(db: &Db, runtime: &tokio::runtime::Runtime, request: Request) -> Reply {
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
    }
}

fn read_managed_settings(demo: bool) -> ManagedSettings {
    if demo {
        return ManagedSettings {
            managed_by_home_manager: true,
            provider: "Siren".to_string(),
            model: "whisper-large-v3-turbo".to_string(),
            endpoint: "https://siren.example.test/v1/audio/transcriptions".to_string(),
            timeout_seconds: 60,
            max_duration_minutes: 5,
            paste_shortcut: "ctrl_shift_v".to_string(),
            notifications: "errors_only".to_string(),
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
    })
}

fn run_demo_worker(requests: Receiver<Request>, replies: Sender<Reply>) {
    let mut recordings = demo_recordings();
    let mut revision_history: HashMap<i64, Vec<Revision>> = recordings
        .iter()
        .map(|recording| (recording.id, demo_initial_revisions(recording)))
        .collect();
    let mut microphones = vec![
        Microphone {
            id: "usb".to_string(),
            name: "USB microphone".to_string(),
            connected: true,
            is_default: false,
        },
        Microphone {
            id: "headset".to_string(),
            name: "Bluetooth headset".to_string(),
            connected: false,
            is_default: false,
        },
        Microphone {
            id: "builtin".to_string(),
            name: "Built-in microphone".to_string(),
            connected: true,
            is_default: true,
        },
    ];
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
            Request::Microphones => Reply::Microphones(Ok(microphones.clone())),
            Request::ReorderMicrophones(ids) => {
                microphones.sort_by_key(|microphone| {
                    ids.iter()
                        .position(|id| id == &microphone.id)
                        .unwrap_or(usize::MAX)
                });
                Reply::Microphones(Ok(microphones.clone()))
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
        };
        if replies.send(reply).is_err() {
            break;
        }
    }
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
                        "key": "must-not-enter-gui-state"
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
}
