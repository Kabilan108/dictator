//! The desktop app, drawn with Rune: a sidebar of four panes over the same
//! backend worker the daemon's history, statistics and settings come from.

pub mod backend;
mod charts;
mod controls;
mod dictation;
mod history;
mod icons;
mod levels;
mod logic;
pub mod osd;
mod playback;
mod settings;
mod stats;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use rune::{
    Anim, AppSpec, Cx, El, Icon, KeyEvent, TextEdit, TextStyle, Tx, col, el, icon, row, text,
};

use crate::ipc::DaemonState;
use crate::provider::{CheckOutcome, ProviderCheck};

use backend::{
    Backend, ConnectionReport, Detail, Filter, Glossary, Microphone, Page, ProviderInfo, Recording,
    Reply, Request, Stats, Status,
};
use controls::{Paint, Tone};
use levels::LevelFeed;
use logic::{CompletedMutation, PendingMutation};
use playback::{AudioPlayer, waveform};

const APP_ID: &str = "org.dictator.Dictator";
const RECENT_LIMIT: usize = 6;
const LEVEL_HISTORY: usize = 96;
const WAVEFORM_BUCKETS: usize = 96;
/// The size the window opens at. It maps fixed at this size, which makes a
/// tiling compositor float it as is rather than hand it a column to shrink
/// out of; the user can resize it once it is up.
const WINDOW_SIZE: (f32, f32) = (940.0, 700.0);
const MIN_SIZE: (f32, f32) = (820.0, 560.0);
const TICK_IDLE: Duration = Duration::from_millis(120);
const TICK_LIVE: Duration = Duration::from_millis(33);
const SIDEBAR_WIDTH: f32 = 196.0;

#[derive(Clone, Copy, Debug)]
pub struct GuiOptions {
    /// Use isolated synthetic recordings, statistics and microphones.
    pub demo: bool,
}

pub fn run(options: GuiOptions) -> Result<()> {
    rune::set_app_id(APP_ID);
    rune::App::new(spec())
        .init(move || Ui::new(options.demo))
        .float_on_open()
        .try_run()
        .map_err(|error| anyhow!(error))
}

fn spec() -> AppSpec<Ui> {
    AppSpec {
        title: "Dictator",
        size: WINDOW_SIZE,
        min_size: Some(MIN_SIZE),
        init: || Ui::new(true),
        view,
        scenes: SCENES,
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        setup: None,
        poll: Some(poll),
        on_key: Some(on_key),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Pane {
    #[default]
    Dictation,
    History,
    Stats,
    Settings,
}

impl Pane {
    const ALL: [Pane; 4] = [Pane::Dictation, Pane::History, Pane::Stats, Pane::Settings];

    fn title(self) -> &'static str {
        match self {
            Pane::Dictation => "Dictation",
            Pane::History => "History",
            Pane::Stats => "Statistics",
            Pane::Settings => "Settings",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Pane::Dictation => "nav-dictation",
            Pane::History => "nav-history",
            Pane::Stats => "nav-stats",
            Pane::Settings => "nav-settings",
        }
    }

    fn glyph(self) -> &'static Icon {
        match self {
            Pane::Dictation => &icons::MIC,
            Pane::History => &icons::HISTORY,
            Pane::Stats => &icons::CHART,
            Pane::Settings => &icons::SLIDERS,
        }
    }
}

/// What the audio player was doing at the last poll. The view reads this
/// rather than the player, whose state checks reap its child process.
#[derive(Clone, Debug, Default)]
struct Playback {
    path: Option<PathBuf>,
    playing: bool,
    position: Duration,
}

struct Ui {
    backend: Backend,
    demo: bool,
    pane: Pane,
    tick_at: f64,

    status: Option<Status>,
    status_at: Instant,
    status_pending: bool,
    last_status_request: Instant,
    last_seen_recording_generation: Option<u64>,
    levels: LevelFeed,
    level_history: VecDeque<(f32, f32)>,
    recent: Vec<Recording>,
    connection: Option<ConnectionReport>,
    connection_pending: bool,
    /// Why the last start, stop, cancel or retry was refused.
    action_error: Option<String>,

    filter: Filter,
    page_index: usize,
    page: Option<Page>,
    loading: bool,
    detail: Option<Detail>,
    search: TextEdit,
    search_value: String,
    editor: TextEdit,
    editor_recording: Option<i64>,
    drafts: HashMap<i64, String>,
    selection_generation: u64,
    history_requests: VecDeque<u64>,
    detail_requests: VecDeque<(u64, i64)>,
    mutation_requests: VecDeque<PendingMutation>,
    reveal_id: Option<i64>,
    pin_detail_id: Option<i64>,
    confirm_delete: Option<i64>,
    delete_pending: Option<i64>,
    player: AudioPlayer,
    playback: Playback,
    waveform: Vec<f32>,

    stats: Option<Stats>,

    microphones: Vec<Microphone>,
    microphone_error: String,
    providers: Vec<ProviderInfo>,
    provider_error: String,
    provider_pending: Option<String>,
    glossary: Option<Glossary>,
    glossary_input: TextEdit,
    glossary_error: String,
    /// Input values of adds awaiting a reply, oldest first.
    glossary_adds: VecDeque<String>,
    /// The glossary term being edited in place, as it was before the edit.
    term_edit: Option<String>,
    /// Lives for the whole session, so a focus event queued for a chip's
    /// field after its edit ended still finds an editor.
    term_field: TextEdit,

    notice: String,
}

impl Ui {
    fn new(demo: bool) -> Self {
        let backend = Backend::new(demo);
        backend.send(Request::Status);
        let mut ui = Self {
            backend,
            demo,
            pane: Pane::Dictation,
            tick_at: 0.0,
            status: None,
            status_at: Instant::now(),
            status_pending: true,
            last_status_request: Instant::now(),
            last_seen_recording_generation: None,
            levels: if demo {
                LevelFeed::default()
            } else {
                LevelFeed::start(crate::visual::default_socket_path())
            },
            level_history: VecDeque::with_capacity(LEVEL_HISTORY),
            recent: Vec::new(),
            connection: None,
            connection_pending: false,
            action_error: None,
            filter: Filter::All,
            page_index: 0,
            page: None,
            loading: true,
            detail: None,
            search: TextEdit::new(""),
            search_value: String::new(),
            editor: TextEdit::new(""),
            editor_recording: None,
            drafts: HashMap::new(),
            selection_generation: 0,
            history_requests: VecDeque::new(),
            detail_requests: VecDeque::new(),
            mutation_requests: VecDeque::new(),
            reveal_id: None,
            pin_detail_id: None,
            confirm_delete: None,
            delete_pending: None,
            player: AudioPlayer::default(),
            playback: Playback::default(),
            waveform: Vec::new(),
            stats: None,
            microphones: Vec::new(),
            microphone_error: String::new(),
            providers: Vec::new(),
            provider_error: String::new(),
            provider_pending: None,
            glossary: None,
            glossary_input: TextEdit::new(""),
            glossary_error: String::new(),
            glossary_adds: VecDeque::new(),
            term_edit: None,
            term_field: TextEdit::new(""),
            notice: String::new(),
        };
        ui.request_history();
        ui.request_recent();
        ui.backend.send(Request::Providers);
        ui.check_connection();
        ui
    }

    fn state(&self) -> Option<DaemonState> {
        self.status.as_ref().map(|status| status.state)
    }

    fn recording(&self) -> bool {
        self.state() == Some(DaemonState::Recording)
    }

    fn active_provider(&self) -> Option<&ProviderInfo> {
        self.providers.iter().find(|provider| provider.active)
    }

    /// How long the current recording has run, advanced between status
    /// replies so the timer does not step.
    fn elapsed_ms(&self) -> i64 {
        let Some(status) = &self.status else {
            return 0;
        };
        if status.state != DaemonState::Recording {
            return status.duration_ms;
        }
        status.duration_ms + self.status_at.elapsed().as_millis() as i64
    }

    fn tick(&mut self) -> Duration {
        let live = (self.recording() && self.pane == Pane::Dictation) || self.player.is_playing();
        if live { TICK_LIVE } else { TICK_IDLE }
    }

    fn request_status(&mut self) {
        if !self.status_pending {
            self.backend.send(Request::Status);
            self.status_pending = true;
            self.last_status_request = Instant::now();
        }
    }

    /// Sends a start, stop, cancel or retry, forgetting the last refusal.
    fn act(&mut self, request: Request) {
        self.action_error = None;
        self.backend.send(request);
    }

    fn request_recent(&mut self) {
        self.backend.send(Request::Recent {
            limit: RECENT_LIMIT,
        });
    }

    fn check_connection(&mut self) {
        if !self.connection_pending {
            self.connection_pending = true;
            self.backend.send(Request::CheckConnection);
        }
    }

    fn request_history(&mut self) {
        self.loading = true;
        self.history_requests.push_back(self.selection_generation);
        self.backend.send(Request::History {
            search: self.search_value.clone(),
            filter: self.filter,
            page: self.page_index,
        });
    }

    fn request_detail(&mut self, id: i64) {
        self.detail_requests
            .push_back((self.selection_generation, id));
        self.backend.send(Request::Detail(id));
    }

    fn bump_selection(&mut self) {
        self.selection_generation = self.selection_generation.wrapping_add(1);
        self.reveal_id = None;
        self.pin_detail_id = None;
    }

    fn select_recording(&mut self, id: i64) {
        self.bump_selection();
        self.confirm_delete = None;
        self.notice.clear();
        self.request_detail(id);
    }

    fn set_filter(&mut self, filter: Filter) {
        if self.filter == filter {
            return;
        }
        self.bump_selection();
        self.filter = filter;
        self.page_index = 0;
        self.request_history();
    }

    fn turn_page(&mut self, forward: bool) {
        self.bump_selection();
        if forward {
            self.page_index += 1;
        } else {
            self.page_index = self.page_index.saturating_sub(1);
        }
        self.request_history();
    }

    fn search_changed(&mut self) {
        let value = self.search.text.clone();
        if logic::accept_search_change(&mut self.search_value, &value).is_some() {
            self.bump_selection();
            self.page_index = 0;
            self.request_history();
        }
    }

    fn reveal_recording(&mut self, id: i64) {
        self.pane = Pane::History;
        self.notice.clear();
        self.bump_selection();
        self.reveal_id = Some(id);
        self.pin_detail_id = Some(id);
        self.filter = Filter::All;
        self.page_index = 0;
        self.search_value.clear();
        self.search.replace_text("");
        self.request_history();
        self.request_detail(id);
    }

    /// Selects a recording that just finished, without leaving the current
    /// pane or pulling the selection away from a transcript being edited.
    fn show_finished(&mut self, id: i64) {
        let editing = self
            .editor_recording
            .is_some_and(|current| self.drafts.contains_key(&current));
        if self.pane == Pane::History && editing {
            self.request_history();
            return;
        }
        let stay = self.pane;
        self.reveal_recording(id);
        self.pane = stay;
    }

    fn set_pane(&mut self, pane: Pane) {
        self.pane = pane;
        self.confirm_delete = None;
        self.term_edit = None;
        match pane {
            Pane::Dictation => self.request_recent(),
            Pane::History => {}
            Pane::Stats => self.backend.send(Request::Stats),
            Pane::Settings => {
                self.backend.send(Request::Microphones);
                self.backend.send(Request::Glossary);
                self.backend.send(Request::Providers);
                if self.connection.is_none() {
                    self.check_connection();
                }
            }
        }
    }

    fn editor_changed(&mut self) {
        let baseline = self
            .detail
            .as_ref()
            .map(|detail| (detail.recording.id, detail.recording.text.clone()));
        logic::track_editor_change(
            &mut self.drafts,
            self.editor_recording,
            baseline.as_ref().map(|(id, text)| (*id, text.as_str())),
            &self.editor.text,
        );
    }

    fn save_revision(&mut self) {
        let Some(detail) = &self.detail else {
            return;
        };
        let id = detail.recording.id;
        let text = self.editor.text.clone();
        if !logic::should_save(&detail.recording.text, &text, false) {
            self.notice = "No unsaved changes".to_string();
        } else if self
            .mutation_requests
            .iter()
            .any(|pending| logic::pending_mutation_id(pending) == id)
        {
            self.notice = "Save already in progress".to_string();
        } else {
            self.mutation_requests.push_back(PendingMutation::Save {
                id,
                text: text.clone(),
            });
            self.backend.send(Request::SaveRevision {
                id,
                expected_revision: detail.recording.revision,
                text,
            });
            self.notice = "Saving…".to_string();
        }
    }

    fn toggle_playback(&mut self) {
        let Some(path) = self
            .detail
            .as_ref()
            .map(|detail| detail.recording.audio_path.clone())
        else {
            return;
        };
        let result = if self.player.path() != Some(path.as_path()) {
            self.player.play(&path, Duration::ZERO)
        } else if self.player.is_playing() {
            self.player.pause()
        } else if self.player.is_paused() {
            self.player.resume()
        } else {
            self.player.play(&path, Duration::ZERO)
        };
        self.after_playback(result);
    }

    fn seek_playback(&mut self, position: Duration) {
        let Some(path) = self
            .detail
            .as_ref()
            .map(|detail| detail.recording.audio_path.clone())
        else {
            return;
        };
        let result = if self.player.path() == Some(path.as_path()) {
            self.player.seek(position)
        } else {
            self.player.play(&path, position)
        };
        self.after_playback(result);
    }

    fn after_playback(&mut self, result: Result<()>) {
        match result {
            Ok(()) => self.notice.clear(),
            Err(error) => self.notice = format!("Playback failed: {error}"),
        }
        self.snapshot_playback();
    }

    /// True when the snapshot moved.
    fn snapshot_playback(&mut self) -> bool {
        let playing = self.player.is_playing();
        let position = self.player.position();
        let path = self.player.path().map(PathBuf::from);
        let moved = playing != self.playback.playing
            || position != self.playback.position
            || path != self.playback.path;
        self.playback = Playback {
            path,
            playing,
            position,
        };
        moved
    }

    fn request_delete(&mut self, id: i64) {
        if self.delete_pending.is_some() {
            self.notice = "A deletion is already in progress".to_string();
            return;
        }
        self.confirm_delete = None;
        self.delete_pending = Some(id);
        self.backend.send(Request::Delete(id));
        self.notice = "Deleting…".to_string();
    }

    fn add_glossary_term(&mut self) {
        let value = self.glossary_input.text.clone();
        match crate::glossary::normalize_term(&value) {
            Ok(term) => {
                self.glossary_error.clear();
                self.glossary_adds.push_back(value);
                self.backend.send(Request::AddGlossaryTerm(term));
            }
            Err(error) => self.glossary_error = error,
        }
    }

    fn begin_term_edit(&mut self, term: &str) {
        self.term_field.replace_text(term);
        self.term_field.set_selection(0, term.len());
        self.term_field.focused = true;
        self.glossary_error.clear();
        self.term_edit = Some(term.to_string());
    }

    fn commit_term_edit(&mut self) {
        let Some(original) = self.term_edit.take() else {
            return;
        };
        match crate::glossary::normalize_term(&self.term_field.text) {
            Ok(term) if term == original => {}
            Ok(term) => self.backend.send(Request::RenameGlossaryTerm {
                from: original,
                to: term,
            }),
            Err(error) => {
                self.glossary_error = error;
                self.term_edit = Some(original);
            }
        }
    }

    fn apply_detail(&mut self, detail: Detail, submitted_text: Option<&str>) {
        let id = detail.recording.id;
        if self.editor_recording != Some(id) {
            self.player.stop();
        }
        if let Some(submitted) = submitted_text
            && self.editor_recording == Some(id)
            && self.editor.text == submitted
        {
            self.drafts.remove(&id);
        }
        logic::normalize_draft(&mut self.drafts, id, &detail.recording.text);
        let text = logic::resolved_editor_text(
            self.drafts.get(&id).map(String::as_str),
            &detail.recording.text,
        );
        self.waveform = if self.demo {
            charts::demo_waveform(id, WAVEFORM_BUCKETS)
        } else {
            waveform(&detail.recording.audio_path, WAVEFORM_BUCKETS)
        };
        let pin_if_missing = self.pin_detail_id == Some(id);
        if pin_if_missing
            && let Some(page) = self.page.as_mut()
            && !page.recordings.iter().any(|recording| recording.id == id)
        {
            page.recordings.insert(0, detail.recording.clone());
        }
        if pin_if_missing {
            self.pin_detail_id = None;
            self.reveal_id = None;
        }
        self.detail = Some(detail);
        if self.editor_recording != Some(id) || self.editor.text != text {
            self.editor.replace_text(text);
            self.editor_recording = Some(id);
        }
    }

    /// Adopts everything the feed and the backend produced since the last
    /// frame. True when something shown changed.
    fn pump(&mut self) -> bool {
        let mut changed = false;
        let update = self.levels.take();
        for sample in update.samples {
            logic::push_level(&mut self.level_history, true, sample);
            changed = true;
        }
        // Only a change counts: the feed keeps its last state, and a retry
        // ends without an OSD idle event, so reapplying it every frame pinned
        // the app to "Transcribing" over the polled status.
        if update.state_changed
            && let Some(state) = update.state
            && let Some(status) = self.status.as_mut()
            && status.state != state
        {
            status.state = state;
            self.status_at = Instant::now();
            if state != DaemonState::Recording {
                self.level_history.clear();
            }
            if state == DaemonState::Error {
                self.check_connection();
            }
            changed = true;
        }
        if update.state_changed {
            self.request_status();
        }
        if self.demo && self.recording() {
            let t = self.status_at.elapsed().as_secs_f32() + self.level_history.len() as f32;
            logic::push_level(&mut self.level_history, true, charts::demo_level(t));
            changed = true;
        }
        for reply in self.backend.drain() {
            changed = true;
            self.adopt(reply);
        }
        changed |= self.snapshot_playback();
        if self.recording() {
            changed = true;
        }
        if !self.status_pending
            && self.last_status_request.elapsed() >= logic::status_interval(self.recording())
        {
            self.request_status();
        }
        changed
    }

    fn adopt(&mut self, reply: Reply) {
        match reply {
            Reply::History(result) => {
                let generation = self.history_requests.pop_front().unwrap_or_default();
                if !logic::accepts_reply(generation, self.selection_generation) {
                    return;
                }
                match result {
                    Ok(page) => {
                        logic::clear_database_notice(&mut self.notice);
                        let visible: Vec<i64> = page
                            .recordings
                            .iter()
                            .map(|recording| recording.id)
                            .collect();
                        let current = self.detail.as_ref().map(|detail| detail.recording.id);
                        let select =
                            logic::choose_history_selection(&mut self.reveal_id, &visible, current);
                        self.page = Some(page);
                        self.loading = false;
                        match select {
                            Some(id) => self.request_detail(id),
                            None => self.detail = None,
                        }
                    }
                    Err(error) => {
                        logic::apply_history_error(&mut self.loading, &mut self.notice, error)
                    }
                }
            }
            Reply::Detail(result) => {
                let Some((generation, requested)) = self.detail_requests.pop_front() else {
                    return;
                };
                if !logic::accepts_reply(generation, self.selection_generation) {
                    return;
                }
                match result {
                    Ok(detail) if detail.recording.id == requested => {
                        self.apply_detail(detail, None)
                    }
                    other => {
                        if self.pin_detail_id == Some(requested) {
                            self.pin_detail_id = None;
                            self.reveal_id = None;
                        }
                        self.notice = match other {
                            Ok(_) => "Received the wrong recording".to_string(),
                            Err(error) => error,
                        };
                    }
                }
            }
            Reply::Saved(result) => {
                let pending = self.mutation_requests.pop_front();
                match result {
                    Ok(detail) => match logic::complete_mutation(pending, detail.recording.id) {
                        CompletedMutation::Save(text) => {
                            let id = detail.recording.id;
                            if self.editor_recording == Some(id) {
                                self.apply_detail(detail, Some(&text));
                            } else if self.drafts.get(&id).is_some_and(|draft| draft == &text) {
                                self.drafts.remove(&id);
                            }
                            self.notice = "Saved".to_string();
                            self.request_history();
                        }
                        CompletedMutation::Unexpected => {
                            self.notice = "Received the wrong saved recording".to_string();
                        }
                    },
                    Err(error) => self.notice = error,
                }
            }
            Reply::Stats(result) => match result {
                Ok(stats) => {
                    self.stats = Some(stats);
                    logic::clear_database_notice(&mut self.notice);
                }
                Err(error) => self.notice = error,
            },
            Reply::Microphones(result) => match result {
                Ok(microphones) => {
                    self.microphones = microphones;
                    self.microphone_error.clear();
                }
                Err(error) => self.microphone_error = error,
            },
            Reply::Status(result) => {
                self.status_pending = false;
                match result {
                    Ok(status) => {
                        if self.notice.starts_with("Daemon disconnected:") {
                            self.notice.clear();
                        }
                        let finished = logic::observe_recording_generation(
                            &mut self.last_seen_recording_generation,
                            status.last_recording_generation,
                        );
                        if finished && let Some(id) = status.last_recording_id {
                            self.request_recent();
                            self.show_finished(id);
                        }
                        if status.state != DaemonState::Recording {
                            self.level_history.clear();
                        }
                        // The feed may have reported the error first and
                        // started the check already.
                        let failed_now = status.state == DaemonState::Error
                            && self.state() != Some(DaemonState::Error);
                        if failed_now {
                            self.check_connection();
                        }
                        let known = self
                            .active_provider()
                            .map(|provider| provider.name.as_str());
                        let moved = !status.provider.is_empty()
                            && known != Some(status.provider.as_str())
                            && self.provider_pending.is_none();
                        if self.status.is_none() || moved {
                            self.backend.send(Request::Providers);
                        }
                        self.status = Some(status);
                        self.status_at = Instant::now();
                    }
                    Err(error) => {
                        self.status = None;
                        self.notice = logic::disconnected_notice(&error);
                    }
                }
            }
            Reply::Action(result) => {
                self.action_error = result.as_ref().err().cloned();
                self.notice = logic::action_notice(result);
                self.request_status();
            }
            Reply::Deleted(result) => {
                let pending = self.delete_pending.take();
                match result {
                    Ok(id) if pending == Some(id) => {
                        self.drafts.remove(&id);
                        if self
                            .detail
                            .as_ref()
                            .is_some_and(|detail| detail.recording.id == id)
                        {
                            self.player.stop();
                            self.detail = None;
                            self.editor_recording = None;
                        }
                        self.recent.retain(|recording| recording.id != id);
                        self.notice = "Recording deleted".to_string();
                        self.request_history();
                    }
                    Ok(_) => self.notice = "Received the wrong deleted recording".to_string(),
                    Err(error) => self.notice = format!("Delete failed: {error}"),
                }
            }
            Reply::Connection(result) => {
                self.connection_pending = false;
                match result {
                    Ok(report) if logic::stale_check(&report, self.active_provider()) => {
                        // Checks run beside other requests, so a switch can
                        // land while one is out: check the new provider.
                        self.backend.send(Request::Providers);
                        self.check_connection();
                    }
                    Ok(report) => self.connection = Some(report),
                    Err(error) => self.notice = error,
                }
            }
            Reply::Recent(result) => match result {
                Ok(recent) => {
                    self.recent = recent;
                    logic::clear_database_notice(&mut self.notice);
                }
                Err(error) => self.notice = error,
            },
            Reply::Glossary(result) => match result {
                Ok(glossary) => {
                    self.glossary = Some(glossary);
                    self.glossary_error.clear();
                }
                Err(error) => self.glossary_error = error,
            },
            Reply::GlossaryAdded(result) => {
                let unchanged = logic::submitted_input_unchanged(
                    &mut self.glossary_adds,
                    &self.glossary_input.text,
                );
                match result {
                    Ok(glossary) => {
                        self.glossary = Some(glossary);
                        self.glossary_error.clear();
                        if unchanged {
                            self.glossary_input.clear();
                        }
                    }
                    Err(error) => self.glossary_error = error,
                }
            }
            Reply::Providers(result) => {
                self.provider_pending = None;
                match result {
                    Ok(providers) => {
                        let before = self.active_provider().map(|provider| provider.name.clone());
                        self.providers = providers;
                        self.provider_error.clear();
                        let after = self.active_provider().map(|provider| provider.name.clone());
                        if before.is_some() && before != after {
                            self.connection = None;
                            self.check_connection();
                        }
                    }
                    Err(error) => self.provider_error = error,
                }
            }
        }
    }

    /// What the sidebar's status row says: the daemon's state while it
    /// works, otherwise whether the provider will take a recording.
    fn health(&self) -> Health {
        let provider = self
            .active_provider()
            .map(|provider| provider.name.clone())
            .unwrap_or_else(|| self.backend.managed_settings.provider.clone());
        let check = self.connection.as_ref().map(|report| &report.check);
        let mut help = check.map_or_else(
            || "The provider has not been checked yet".to_string(),
            |check| {
                let (_, verdict, message) = describe_check(check);
                let mut line = format!("{provider}: {verdict}");
                if !message.is_empty() {
                    line.push_str(&format!(". {message}"));
                }
                line
            },
        );
        help.push_str("\nClick to check again");
        let Some(status) = &self.status else {
            return Health {
                tone: Tone::Bad,
                label: "Daemon offline",
                provider,
                help: "The dictator daemon is not running".to_string(),
            };
        };
        let (tone, label) = match status.state {
            DaemonState::Recording => (Tone::Bad, "Recording"),
            DaemonState::Transcribing => (Tone::Accent, "Transcribing"),
            DaemonState::Typing => (Tone::Accent, "Pasting"),
            DaemonState::Error => (Tone::Warn, "Failed"),
            DaemonState::Idle => match check.map(|check| check.outcome) {
                None => (Tone::Neutral, "Checking…"),
                Some(CheckOutcome::Ready | CheckOutcome::Reachable) => (Tone::Good, "Ready"),
                Some(CheckOutcome::KeyRejected) => (Tone::Bad, "Key rejected"),
                Some(CheckOutcome::Unreachable) => (Tone::Bad, "Unreachable"),
                Some(CheckOutcome::Failed) => (Tone::Warn, "Provider error"),
                Some(CheckOutcome::Unusable) => (Tone::Warn, "Not set up"),
            },
        };
        Health {
            tone,
            label,
            provider,
            help,
        }
    }

    /// Scene helper: History with the newest successful recording selected.
    fn open_first_transcript(&mut self) {
        self.set_pane(Pane::History);
        self.settle();
        let first = self.page.as_ref().and_then(|page| {
            page.recordings
                .iter()
                .find(|recording| !recording.failed)
                .map(|recording| recording.id)
        });
        if let Some(id) = first {
            self.select_recording(id);
            self.settle();
        }
    }

    /// Waits for the worker to answer everything asked so far, so a scene
    /// renders settled data on its first frame.
    fn settle(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut quiet = 0;
        while Instant::now() < deadline && quiet < 6 {
            if self.pump() {
                quiet = 0;
            } else {
                quiet += 1;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

struct Health {
    tone: Tone,
    label: &'static str,
    provider: String,
    help: String,
}

/// A provider check in words: its tone, a short verdict and the provider's
/// message (empty when there is nothing more to say).
fn describe_check(check: &ProviderCheck) -> (Tone, String, String) {
    let message = check.message.clone();
    match check.outcome {
        CheckOutcome::Ready => (
            Tone::Good,
            format!("Key accepted · {} ms", check.latency_ms),
            String::new(),
        ),
        CheckOutcome::Reachable => (
            Tone::Neutral,
            "Reachable · key not checked".to_string(),
            "The daemon could not check the key, so only the endpoint was tried.".to_string(),
        ),
        CheckOutcome::KeyRejected => (Tone::Bad, "API key rejected".to_string(), message),
        CheckOutcome::Unreachable => (Tone::Bad, "Unreachable".to_string(), message),
        CheckOutcome::Failed => (
            Tone::Warn,
            check.status.map_or_else(
                || "Error".to_string(),
                |status| format!("Error · HTTP {status}"),
            ),
            message,
        ),
        CheckOutcome::Unusable => (Tone::Warn, "Not set up".to_string(), message),
    }
}

/// A named fixture a headless render starts from.
type Scene = (&'static str, fn(&mut Ui));

const SCENES: &[Scene] = &[
    ("dictation", |ui| ui.settle()),
    ("recording", |ui| {
        ui.settle();
        ui.backend.send(Request::Toggle);
        ui.settle();
        for step in 0..LEVEL_HISTORY {
            logic::push_level(&mut ui.level_history, true, charts::demo_level(step as f32));
        }
        if let Some(status) = ui.status.as_mut() {
            status.duration_ms = 12_400;
        }
    }),
    ("dictation-error", |ui| {
        ui.settle();
        if let Some(last) = ui.recent.first_mut() {
            last.failed = true;
            last.text.clear();
            last.error = backend::demo_failure(0);
        }
        if let Some(status) = ui.status.as_mut() {
            status.state = DaemonState::Error;
            status.error = format!("transcription failed: {}", backend::demo_failure(0));
        }
    }),
    ("history", |ui| ui.open_first_transcript()),
    ("history-edited", |ui| {
        ui.open_first_transcript();
        let text = ui
            .editor
            .text
            .replace("after lunch", "before the standup tomorrow");
        ui.editor.replace_text(text);
        ui.editor_changed();
    }),
    ("history-delete", |ui| {
        ui.open_first_transcript();
        ui.confirm_delete = ui.detail.as_ref().map(|detail| detail.recording.id);
    }),
    ("history-failed", |ui| {
        ui.set_pane(Pane::History);
        ui.set_filter(Filter::Failed);
        ui.settle();
    }),
    ("stats", |ui| {
        ui.set_pane(Pane::Stats);
        ui.settle();
    }),
    ("settings", |ui| {
        ui.set_pane(Pane::Settings);
        ui.settle();
    }),
    ("settings-key-rejected", |ui| {
        ui.settle();
        ui.backend.send(Request::SetProvider("openai".to_string()));
        ui.settle();
        ui.set_pane(Pane::Settings);
        ui.settle();
    }),
    ("settings-editing", |ui| {
        ui.set_pane(Pane::Settings);
        ui.settle();
        if let Some(term) = ui
            .glossary
            .as_ref()
            .and_then(|glossary| glossary.terms.get(3).cloned())
        {
            ui.begin_term_edit(&term);
        }
    }),
];

fn poll(ui: &mut Ui, clock: f64, tx: &mut Tx) -> bool {
    if clock >= ui.tick_at {
        let tick = ui.tick();
        ui.tick_at = clock + tick.as_secs_f64();
        tx.wake_after(tick);
    }
    ui.pump()
}

fn on_key(ui: &mut Ui, event: &KeyEvent, _: &mut Tx) -> bool {
    if ui.pane == Pane::History && event.is_command('s') {
        ui.save_revision();
        return true;
    }
    for (index, pane) in Pane::ALL.into_iter().enumerate() {
        let digit = char::from(b'1' + index as u8);
        if event.is_command(digit) {
            ui.set_pane(pane);
            return true;
        }
    }
    false
}

fn view(ui: &Ui, cx: &mut Cx<Ui>) -> El {
    let p = Paint::new(cx.system_dark(), cx.env().scale);
    let mut layer = Vec::new();
    let (actions, body) = match ui.pane {
        Pane::Dictation => dictation::pane(ui, p),
        Pane::History => history::pane(ui, p),
        Pane::Stats => stats::pane(ui, p),
        Pane::Settings => settings::pane(ui, p, &mut layer),
    };
    let panel = col()
        .key("panel")
        .role("main")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .mt(8.0)
        .mr(8.0)
        .mb(8.0)
        .radius(p.k.radius.lg)
        .fill(p.k.c.bg_base)
        .border(p.hair, p.k.c.border_weak)
        .padding(p.hair)
        .clip_soft(0.0)
        .child(pane_header(p, ui.pane.title(), actions))
        .child(body);
    let window = row()
        .key("window")
        .role("none")
        .fill(p.k.c.bg_deep)
        .hover_motion_within(Anim::INSTANT, Anim::INSTANT)
        .child(sidebar(ui, p))
        .child(panel);
    rune_kit::root(window, layer)
}

const TITLE: TextStyle = TextStyle::new(18.0, 600.0).lh(24.0);

fn pane_header(p: Paint, title: &str, actions: Vec<El>) -> El {
    row()
        .key("pane-header")
        .h(52.0)
        .px(controls::GUTTER)
        .gap(8.0)
        .center_y()
        .child(
            text(title)
                .role("heading")
                .level(1)
                .font(TITLE)
                .color(p.strong())
                .grow(1.0),
        )
        .children(actions)
}

fn sidebar(ui: &Ui, p: Paint) -> El {
    let brand = row()
        .key("brand")
        .h(56.0)
        .px(10.0)
        .gap(9.0)
        .center_y()
        .child(icon(&icons::BRAND, 24.0).key("mark").a11y_hidden())
        .child(
            text("Dictator")
                .font(TextStyle::new(14.0, 600.0))
                .color(p.strong()),
        );
    let nav = col()
        .key("nav")
        .role("list")
        .focus_group()
        .gap(2.0)
        .children(Pane::ALL.into_iter().map(|pane| nav_row(ui, p, pane)));
    let footer = status_row(ui, p);
    col()
        .key("sidebar")
        .role("navigation")
        .w(SIDEBAR_WIDTH)
        .px(8.0)
        .pb(8.0)
        .child(brand)
        .child(nav)
        .child(rune::spacer())
        .child(footer)
}

/// One row: a haloed dot in the state's tone, what the app is doing or
/// whether it can record, and the provider recordings go to.
fn status_row(ui: &Ui, p: Paint) -> El {
    let health = ui.health();
    let ink = health.tone.color(p);
    let dot = row()
        .size(16.0, 16.0)
        .radius(8.0)
        .fill(ink.alpha(0.2))
        .center()
        .a11y_hidden()
        .child(el().size(8.0, 8.0).radius(4.0).fill(ink));
    row()
        .key("health")
        .role("status")
        .label(format!("{} · {}", health.label, health.provider))
        .h(40.0)
        .px(10.0)
        .gap(9.0)
        .center_y()
        .radius(p.k.radius.md)
        .fill(p.k.c.surface_raised_base)
        .border(p.hair, p.k.c.border_weak)
        .hover(p.k.c.hover)
        .help(health.help)
        .on_press(|ui: &mut Ui| ui.check_connection())
        .child(dot)
        .child(
            text(health.label)
                .font(p.k.ty.small_medium)
                .color(p.strong())
                .truncate()
                .grow(1.0)
                .shrink(),
        )
        .child(
            text(health.provider)
                .font(p.k.ty.small)
                .color(p.muted())
                .truncate()
                .shrink(),
        )
}

fn nav_row(ui: &Ui, p: Paint, pane: Pane) -> El {
    let selected = ui.pane == pane;
    let ink = if selected { p.strong() } else { p.muted() };
    let item = row()
        .key(pane.key())
        .role("listitem")
        .label(pane.title())
        .focusable(true)
        .focus_selected(selected)
        .h(32.0)
        .px(10.0)
        .gap(10.0)
        .center_y()
        .radius(p.k.radius.md)
        .child(icon(pane.glyph(), 16.0).color(ink))
        .child(
            text(pane.title())
                .font(if selected {
                    p.k.ty.ui_medium
                } else {
                    p.k.ty.ui
                })
                .color(ink)
                .a11y_hidden(),
        );
    let item = if selected {
        item.fill(p.k.c.surface_raised_base)
    } else {
        item.hover(p.k.c.hover)
    };
    item.on_press_tx(move |ui: &mut Ui, tx: &mut Tx| {
        if ui.pane != pane {
            tx.instant();
            ui.set_pane(pane);
        }
    })
}
