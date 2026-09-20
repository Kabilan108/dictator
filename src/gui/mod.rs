pub mod backend;
mod placement;
mod playback;
mod theme;

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Local, Utc};
use gpui::{
    AnyWindowHandle, App, Application, Bounds, Context, Entity, Global, IntoElement, Rgba,
    Subscription, Timer, Window, WindowBounds, WindowKind, WindowOptions, div, prelude::*, px,
    size,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Icon, IconName, Root, Theme, ThemeMode};

use crate::ipc::DaemonState;
use crate::tray::{TrayAction, TrayHandle};

use backend::{
    Backend, ConnectionReport, Detail, Filter, Microphone, Page, Recording, Reply, Request, Stats,
    Status,
};
use placement::place_popup;
use playback::{AudioPlayer, waveform};
use theme::*;

const APP_ID: &str = "org.dictator.Dictator";
const RECENT_LIMIT: usize = 6;

struct GuiAssets;

impl gpui::AssetSource for GuiAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let data: &'static [u8] = match path {
            "icons/arrow-up.svg" => include_bytes!("../../assets/icons/arrow-up.svg"),
            "icons/arrow-down.svg" => include_bytes!("../../assets/icons/arrow-down.svg"),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(data)))
    }

    fn list(&self, path: &str) -> Result<Vec<gpui::SharedString>> {
        Ok(["icons/arrow-up.svg", "icons/arrow-down.svg"]
            .into_iter()
            .filter(|asset| asset.starts_with(path))
            .map(Into::into)
            .collect())
    }
}

#[derive(Default)]
struct WindowRegistry {
    main: Option<AnyWindowHandle>,
    popup: Option<AnyWindowHandle>,
}

impl Global for WindowRegistry {}

#[derive(Clone, Copy, Debug)]
pub struct GuiOptions {
    pub demo: bool,
    pub show_main: bool,
    pub tray: bool,
}

impl Default for GuiOptions {
    fn default() -> Self {
        Self {
            demo: false,
            show_main: true,
            tray: true,
        }
    }
}

pub fn run(options: GuiOptions) -> Result<()> {
    Application::new().with_assets(GuiAssets).run(move |cx| {
        gpui_component::init(cx);
        if !options.tray {
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
        }
        Theme::change(ThemeMode::Dark, None, cx);
        cx.set_global(WindowRegistry::default());
        cx.text_system()
            .add_fonts(vec![
                Cow::Borrowed(include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf")),
                Cow::Borrowed(include_bytes!("../../assets/fonts/Geist-Medium.ttf")),
            ])
            .expect("failed to load Dictator fonts");

        let (tray_handle, tray_actions) = if options.tray {
            let (tx, rx) = std::sync::mpsc::channel();
            match crate::tray::spawn(tx) {
                Ok(handle) => (Some(handle), Some(rx)),
                Err(error) => {
                    eprintln!("dictator GUI: tray unavailable: {error:#}");
                    (None, None)
                }
            }
        } else {
            (None, None)
        };

        start_host(cx, options, tray_handle, tray_actions);
        if options.show_main {
            open_main_window(cx, options).expect("failed to open Dictator window");
        }
        cx.activate(true);
    });
    Ok(())
}

pub fn open_main_window(cx: &mut App, options: GuiOptions) -> Result<()> {
    if !cx.has_global::<WindowRegistry>() {
        cx.set_global(WindowRegistry::default());
    }
    if let Some(handle) = cx.global::<WindowRegistry>().main
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return Ok(());
    }
    let bounds = Bounds::centered(None, size(px(1120.), px(700.)), cx);
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some(APP_ID.to_string()),
                window_min_size: Some(size(px(920.), px(620.))),
                ..Default::default()
            },
            move |window, cx| {
                window.set_window_title("Dictator");
                let view = cx.new(|cx| MainView::new(options.demo, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .context("failed to open main window")?;
    cx.global_mut::<WindowRegistry>().main = Some(handle.into());
    Ok(())
}

pub fn open_tray_popup(cx: &mut App, options: GuiOptions) -> Result<()> {
    open_tray_popup_at(cx, options, None)
}

fn open_tray_popup_at(cx: &mut App, options: GuiOptions, anchor: Option<(i32, i32)>) -> Result<()> {
    if !cx.has_global::<WindowRegistry>() {
        cx.set_global(WindowRegistry::default());
    }
    if let Some(handle) = cx.global::<WindowRegistry>().popup
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        place_popup(anchor);
        return Ok(());
    }
    let bounds = Bounds::centered(None, size(px(340.), px(510.)), cx);
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                kind: WindowKind::PopUp,
                app_id: Some(APP_ID.to_string()),
                is_resizable: false,
                is_minimizable: false,
                ..Default::default()
            },
            move |window, cx| {
                window.set_window_title("Dictator quick controls");
                let view = cx.new(|cx| TrayView::new(options, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .context("failed to open tray popup")?;
    cx.global_mut::<WindowRegistry>().popup = Some(handle.into());
    place_popup(anchor);
    Ok(())
}

fn start_host(
    cx: &mut App,
    options: GuiOptions,
    tray: Option<TrayHandle>,
    actions: Option<Receiver<TrayAction>>,
) {
    let host = cx.new(|cx| HostView::new(options, tray, actions, cx));
    cx.set_global(HostController { _host: host });
}

struct HostController {
    _host: Entity<HostView>,
}

impl Global for HostController {}

struct HostView {
    options: GuiOptions,
    tray: Option<TrayHandle>,
    actions: Option<Receiver<TrayAction>>,
    backend: Backend,
    status_pending: bool,
    last_status_request: Instant,
}

impl HostView {
    fn new(
        options: GuiOptions,
        tray: Option<TrayHandle>,
        actions: Option<Receiver<TrayAction>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let backend = Backend::new(options.demo);
        backend.send(Request::Status);
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(250)).await;
                if !cx
                    .update(|cx| {
                        let Some(entity) = this.upgrade() else {
                            return false;
                        };
                        entity.update(cx, |this, cx| this.poll(cx));
                        true
                    })
                    .unwrap_or(false)
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            options,
            tray,
            actions,
            backend,
            status_pending: true,
            last_status_request: Instant::now(),
        }
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        if let Some(actions) = &self.actions {
            while let Ok(action) = actions.try_recv() {
                match action {
                    TrayAction::OpenHistory => {
                        let _ = open_main_window(cx, self.options);
                    }
                    TrayAction::OpenPopup => {
                        let _ = open_tray_popup(cx, self.options);
                    }
                    TrayAction::OpenPopupAt(x, y) => {
                        let _ = open_tray_popup_at(cx, self.options, Some((x, y)));
                    }
                    TrayAction::ToggleRecording => self.backend.send(Request::Toggle),
                    TrayAction::CancelRecording => self.backend.send(Request::Cancel),
                    TrayAction::Quit => cx.quit(),
                }
            }
        }
        for reply in self.backend.drain() {
            if let Reply::Status(result) = reply {
                self.status_pending = false;
                if let Ok(status) = result
                    && let Some(tray) = &self.tray
                {
                    tray.update(status.state.as_str());
                }
            }
        }
        if !self.status_pending && self.last_status_request.elapsed() >= Duration::from_secs(1) {
            self.backend.send(Request::Status);
            self.status_pending = true;
            self.last_status_request = Instant::now();
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Dictation,
    History,
    Stats,
    Settings,
}

struct MainView {
    backend: Backend,
    tab: Tab,
    filter: Filter,
    page_index: usize,
    page: Option<Page>,
    detail: Option<Detail>,
    stats: Option<Stats>,
    recent: Vec<Recording>,
    connection: Option<ConnectionReport>,
    connection_pending: bool,
    confirm_delete: Option<i64>,
    delete_pending: Option<i64>,
    microphones: Vec<Microphone>,
    status: Option<Status>,
    last_seen_recording_generation: Option<u64>,
    status_pending: bool,
    last_status_request: Instant,
    reveal_id: Option<i64>,
    pin_detail_id: Option<i64>,
    search: Entity<InputState>,
    search_value: String,
    editor: Entity<InputState>,
    editor_recording: Option<i64>,
    selection_generation: u64,
    history_requests: VecDeque<u64>,
    detail_requests: VecDeque<(u64, i64)>,
    mutation_requests: VecDeque<PendingMutation>,
    drafts: HashMap<i64, String>,
    setting_editor: bool,
    player: AudioPlayer,
    waveform: Vec<f32>,
    notice: String,
    loading: bool,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingMutation {
    Save { id: i64, text: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CompletedMutation {
    Save(String),
    Unexpected,
}

impl MainView {
    fn new(demo: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let backend = Backend::new(demo);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("search recordings"));
        let editor = cx.new(|cx| InputState::new(window, cx).auto_grow(3, 16));
        let search_subscription =
            cx.subscribe_in(&search, window, |this, input, event: &InputEvent, _, cx| {
                let value = input.read(cx).value().to_string();
                if matches!(event, InputEvent::Change)
                    && let Some(search) = accept_search_change(&mut this.search_value, &value)
                {
                    this.bump_selection();
                    this.page_index = 0;
                    this.request_history(search);
                    cx.notify();
                }
            });
        let editor_subscription =
            cx.subscribe_in(&editor, window, |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) && !this.setting_editor {
                    let editor_text = input.read(cx).value().to_string();
                    let baseline = this
                        .detail
                        .as_ref()
                        .map(|detail| (detail.recording.id, detail.recording.text.clone()));
                    track_editor_change(
                        &mut this.drafts,
                        this.editor_recording,
                        baseline.as_ref().map(|(id, text)| (*id, text.as_str())),
                        &editor_text,
                    );
                    cx.notify();
                }
            });
        backend.send(Request::Status);
        let weak = cx.weak_entity();
        window
            .spawn(cx, async move |cx| {
                loop {
                    Timer::after(Duration::from_millis(120)).await;
                    if !cx
                        .update(|window, cx| {
                            let Some(entity) = weak.upgrade() else {
                                return false;
                            };
                            entity.update(cx, |this, cx| this.poll(window, cx));
                            true
                        })
                        .unwrap_or(false)
                    {
                        break;
                    }
                }
            })
            .detach();
        let mut this = Self {
            backend,
            tab: Tab::Dictation,
            filter: Filter::All,
            page_index: 0,
            page: None,
            detail: None,
            stats: None,
            recent: Vec::new(),
            connection: None,
            connection_pending: false,
            confirm_delete: None,
            delete_pending: None,
            microphones: Vec::new(),
            status: None,
            last_seen_recording_generation: None,
            status_pending: true,
            last_status_request: Instant::now(),
            reveal_id: None,
            pin_detail_id: None,
            search,
            search_value: String::new(),
            editor,
            editor_recording: None,
            selection_generation: 0,
            history_requests: VecDeque::new(),
            detail_requests: VecDeque::new(),
            mutation_requests: VecDeque::new(),
            drafts: HashMap::new(),
            setting_editor: false,
            player: AudioPlayer::default(),
            waveform: vec![0.08; 64],
            notice: String::new(),
            loading: true,
            _subscriptions: vec![search_subscription, editor_subscription],
        };
        this.request_history(String::new());
        this.backend.send(Request::Recent {
            limit: RECENT_LIMIT,
        });
        this
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

    fn reveal_recording(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = Tab::History;
        self.notice.clear();
        self.bump_selection();
        self.reveal_id = Some(id);
        self.pin_detail_id = Some(id);
        self.filter = Filter::All;
        self.page_index = 0;
        self.search_value.clear();
        self.setting_editor = true;
        self.search.update(cx, |search, cx| {
            search.set_value("", window, cx);
        });
        self.setting_editor = false;
        self.request_history(String::new());
        self.request_detail(id);
    }

    fn request_delete(&mut self, id: i64) {
        if self.delete_pending.is_some() {
            self.notice = "A deletion is already in progress".to_string();
            return;
        }
        self.confirm_delete = None;
        self.delete_pending = Some(id);
        self.backend.send(Request::Delete(id));
        self.notice = "Deleting...".to_string();
    }

    fn request_history(&mut self, search: String) {
        self.loading = true;
        self.history_requests.push_back(self.selection_generation);
        self.backend.send(Request::History {
            search,
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

    fn apply_detail(
        &mut self,
        detail: Detail,
        submitted_text: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = detail.recording.id;
        if self.editor_recording != Some(id) {
            self.player.stop();
        }
        if let Some(submitted) = submitted_text {
            let current = self.editor.read(cx).value();
            if self.editor_recording == Some(id) && current.as_ref() == submitted {
                self.drafts.remove(&id);
            }
        }
        normalize_draft(&mut self.drafts, id, &detail.recording.text);
        let text = resolved_editor_text(
            self.drafts.get(&id).map(String::as_str),
            &detail.recording.text,
        );
        self.waveform = waveform(&detail.recording.audio_path, 64);
        let pin_if_missing = self.pin_detail_id == Some(id);
        if self
            .page
            .as_ref()
            .is_some_and(|page| !page.recordings.iter().any(|recording| recording.id == id))
            && pin_if_missing
            && let Some(page) = self.page.as_mut()
        {
            page.recordings.insert(0, detail.recording.clone());
        }
        if pin_if_missing {
            self.pin_detail_id = None;
            self.reveal_id = None;
        }
        self.detail = Some(detail);
        if self.editor_recording != Some(id) || self.editor.read(cx).value().as_ref() != text {
            self.setting_editor = true;
            self.editor.update(cx, |editor, cx| {
                editor.set_value(text, window, cx);
            });
            self.setting_editor = false;
            self.editor_recording = Some(id);
        }
    }

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for reply in self.backend.drain() {
            match reply {
                Reply::History(result) => {
                    let request_generation = self.history_requests.pop_front().unwrap_or_default();
                    if !accepts_reply(request_generation, self.selection_generation) {
                        continue;
                    }
                    match result {
                        Ok(page) => {
                            clear_database_notice(&mut self.notice);
                            let visible_ids = page
                                .recordings
                                .iter()
                                .map(|recording| recording.id)
                                .collect::<Vec<_>>();
                            let current_id = self.detail.as_ref().map(|detail| detail.recording.id);
                            let select = choose_history_selection(
                                &mut self.reveal_id,
                                &visible_ids,
                                current_id,
                            );
                            self.page = Some(page);
                            self.loading = false;
                            if let Some(id) = select {
                                self.request_detail(id);
                            } else {
                                self.detail = None;
                            }
                        }
                        Err(error) => {
                            apply_history_error(&mut self.loading, &mut self.notice, error)
                        }
                    }
                }
                Reply::Detail(result) => {
                    let Some((generation, requested_id)) = self.detail_requests.pop_front() else {
                        continue;
                    };
                    if !accepts_reply(generation, self.selection_generation) {
                        continue;
                    }
                    match result {
                        Ok(detail) if detail.recording.id == requested_id => {
                            self.apply_detail(detail, None, window, cx);
                        }
                        Ok(_) => {
                            if self.pin_detail_id == Some(requested_id) {
                                self.pin_detail_id = None;
                                self.reveal_id = None;
                            }
                            self.notice = "Received the wrong recording".to_string();
                        }
                        Err(error) => {
                            if self.pin_detail_id == Some(requested_id) {
                                self.pin_detail_id = None;
                                self.reveal_id = None;
                            }
                            self.notice = error;
                        }
                    }
                }
                Reply::Saved(result) => {
                    let pending = self.mutation_requests.pop_front();
                    match result {
                        Ok(detail) => match complete_mutation(pending, detail.recording.id) {
                            CompletedMutation::Save(text) => {
                                let id = detail.recording.id;
                                if self.editor_recording == Some(id) {
                                    self.apply_detail(detail, Some(&text), window, cx);
                                } else if self.drafts.get(&id).is_some_and(|draft| draft == &text) {
                                    self.drafts.remove(&id);
                                }
                                self.notice = "Saved".to_string();
                                let search = self.search.read(cx).value().to_string();
                                self.request_history(search);
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
                        clear_database_notice(&mut self.notice);
                    }
                    Err(error) => self.notice = error,
                },
                Reply::Microphones(result) => match result {
                    Ok(microphones) => self.microphones = microphones,
                    Err(error) => self.notice = error,
                },
                Reply::Status(result) => {
                    self.status_pending = false;
                    match result {
                        Ok(status) => {
                            if self.notice.starts_with("Daemon disconnected:") {
                                self.notice.clear();
                            }
                            let generation_changed = observe_recording_generation(
                                &mut self.last_seen_recording_generation,
                                status.last_recording_generation,
                            );
                            if generation_changed && let Some(id) = status.last_recording_id {
                                self.request_recent();
                                let stay = self.tab;
                                self.reveal_recording(id, window, cx);
                                if stay == Tab::Dictation {
                                    self.tab = Tab::Dictation;
                                }
                            }
                            self.status = Some(status);
                        }
                        Err(error) => {
                            self.status = None;
                            self.notice = disconnected_notice(&error);
                        }
                    }
                }
                Reply::Action(result) => {
                    self.notice = action_notice(result);
                    if !self.status_pending {
                        self.backend.send(Request::Status);
                        self.status_pending = true;
                        self.last_status_request = Instant::now();
                    }
                }
                Reply::Deleted(result) => {
                    let pending = self.delete_pending.take();
                    match result {
                        Ok(id) => {
                            if pending != Some(id) {
                                self.notice = "Received the wrong deleted recording".to_string();
                                continue;
                            }
                            self.drafts.remove(&id);
                            if self.detail.as_ref().is_some_and(|d| d.recording.id == id) {
                                self.player.stop();
                                self.detail = None;
                                self.editor_recording = None;
                            }
                            self.recent.retain(|recording| recording.id != id);
                            self.notice = "Recording deleted".to_string();
                            let search = self.search.read(cx).value().to_string();
                            self.request_history(search);
                        }
                        Err(error) => self.notice = format!("Delete failed: {error}"),
                    }
                }
                Reply::Connection(result) => {
                    self.connection_pending = false;
                    match result {
                        Ok(report) => self.connection = Some(report),
                        Err(error) => self.notice = error,
                    }
                }
                Reply::Recent(result) => match result {
                    Ok(recent) => {
                        self.recent = recent;
                        clear_database_notice(&mut self.notice);
                    }
                    Err(error) => self.notice = error,
                },
            }
        }
        if !self.status_pending && self.last_status_request.elapsed() >= Duration::from_secs(1) {
            self.backend.send(Request::Status);
            self.status_pending = true;
            self.last_status_request = Instant::now();
        }
        cx.notify();
    }

    fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.confirm_delete = None;
        match tab {
            Tab::Dictation => {
                self.request_recent();
                if self.connection.is_none() {
                    self.check_connection();
                }
            }
            Tab::History => {}
            Tab::Stats => self.backend.send(Request::Stats),
            Tab::Settings => self.backend.send(Request::Microphones),
        }
    }

    fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut bar = div()
            .flex()
            .items_center()
            .h(px(44.))
            .px(px(16.))
            .gap(px(10.))
            .bg(MANTLE)
            .border_b_1()
            .border_color(LINE)
            .child(cursor_logo())
            .child(
                div()
                    .font_family(FONT_DISPLAY)
                    .text_size(px(16.))
                    .mr(px(10.))
                    .child("Dictator"),
            );
        for (tab, label) in [
            (Tab::Dictation, "Dictation"),
            (Tab::History, "History"),
            (Tab::Stats, "Stats"),
            (Tab::Settings, "Settings"),
        ] {
            let selected = self.tab == tab;
            bar = bar.child(
                div()
                    .id(label)
                    .px(px(10.))
                    .py(px(5.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_color(if selected { TEXT } else { MUTED })
                    .when(selected, |this| this.bg(SURFACE_2))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_tab(tab);
                        cx.notify();
                    }))
                    .child(label),
            );
        }
        bar = bar.child(div().flex_1());
        if self.tab == Tab::History {
            if let Some(status) = &self.status
                && matches!(
                    status.state,
                    DaemonState::Recording | DaemonState::Transcribing
                )
            {
                let color = if status.state == DaemonState::Recording {
                    RED
                } else {
                    BLUE
                };
                bar = bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(12.))
                        .text_color(color)
                        .child(div().size(px(6.)).rounded_full().bg(color))
                        .child(status.state.as_str()),
                );
            }
            bar = bar.child(
                div()
                    .w(px(260.))
                    .h(px(30.))
                    .px(px(8.))
                    .bg(SURFACE)
                    .border_1()
                    .border_color(LINE)
                    .rounded(px(4.))
                    .child(Input::new(&self.search).appearance(false)),
            );
        }
        bar
    }

    fn render_history(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let page = self.page.clone().unwrap_or(Page {
            recordings: Vec::new(),
            total: 0,
            has_next: false,
        });
        let pages = (page.total.max(1) as usize).div_ceil(50).max(1);
        let mut list = div().id("history-scroll").flex_1().overflow_y_scroll();
        if self.loading {
            list = list.child(
                div()
                    .p(px(18.))
                    .text_color(MUTED)
                    .child("Loading history..."),
            );
        } else if page.recordings.is_empty() {
            list = list.child(
                div()
                    .p(px(18.))
                    .text_color(if self.notice.is_empty() { MUTED } else { RED })
                    .child(if self.notice.is_empty() {
                        "No matching recordings.".to_string()
                    } else {
                        self.notice.clone()
                    }),
            );
        } else {
            let mut last_date = None;
            for recording in &page.recordings {
                let id = recording.id;
                let selected = self
                    .detail
                    .as_ref()
                    .is_some_and(|detail| detail.recording.id == id);
                let date = recording.timestamp.with_timezone(&Local).date_naive();
                if last_date != Some(date) {
                    last_date = Some(date);
                    list = list.child(
                        div()
                            .px(px(14.))
                            .pt(px(14.))
                            .pb(px(4.))
                            .font_family(FONT_DISPLAY)
                            .text_size(px(14.))
                            .text_color(TEXT)
                            .child(format_date(recording.timestamp)),
                    );
                }
                let text = single_line(if recording.failed {
                    &recording.error
                } else {
                    &recording.text
                });
                let empty = text.is_empty();
                list = list.child(
                    div()
                        .id(("recording", id as u64))
                        .px(px(14.))
                        .py(px(10.))
                        .border_l_2()
                        .border_color(if selected { BLUE } else { MANTLE })
                        .when(selected, |this| this.bg(SURFACE))
                        .hover(|this| this.bg(SURFACE))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.bump_selection();
                            this.request_detail(id);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .text_size(px(11.))
                                .text_color(MUTED)
                                .child(format!(
                                    "{}  ·  {}{}",
                                    format_time(recording.timestamp),
                                    format_duration(recording.duration_ms),
                                    if recording.revision > 0 {
                                        "  ·  edited"
                                    } else {
                                        ""
                                    }
                                ))
                                .child(format!("#{}", recording.id)),
                        )
                        .child(
                            div()
                                .mt(px(5.))
                                .text_size(px(13.))
                                .line_clamp(2)
                                .text_color(if recording.failed {
                                    RED
                                } else if empty {
                                    MUTED
                                } else {
                                    SUBTEXT
                                })
                                .when(empty, |this| this.italic())
                                .child(if empty {
                                    "No speech was transcribed".to_string()
                                } else {
                                    text
                                }),
                        ),
                );
            }
        }
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .w(px(300.))
                    .flex()
                    .flex_col()
                    .bg(MANTLE)
                    .border_r_1()
                    .border_color(LINE)
                    .child(self.render_history_tools(page.total, cx))
                    .child(list)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .p(px(10.))
                            .border_t_1()
                            .border_color(LINE)
                            .text_size(px(11.))
                            .text_color(MUTED)
                            .child(self.page_button(
                                "Previous",
                                can_go_previous(self.page_index),
                                cx,
                            ))
                            .child(format!("{} / {pages}", self.page_index + 1))
                            .child(self.page_button("Next", page.has_next, cx)),
                    ),
            )
            .child(self.render_detail(cx))
            .into_any_element()
    }

    fn render_history_tools(&self, total: i64, cx: &mut Context<Self>) -> impl IntoElement {
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .p(px(10.))
            .border_b_1()
            .border_color(LINE)
            .text_size(px(11.))
            .text_color(MUTED)
            .child(format!("{total} recordings"))
            .child(div().flex_1());
        for (filter, label) in [
            (Filter::All, "All"),
            (Filter::Edited, "Edited"),
            (Filter::Failed, "Failed"),
        ] {
            let selected = self.filter == filter;
            row = row.child(
                div()
                    .id(label)
                    .px(px(6.))
                    .py(px(3.))
                    .rounded(px(3.))
                    .cursor_pointer()
                    .when(selected, |this| this.bg(SURFACE_2).text_color(TEXT))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.bump_selection();
                        this.filter = filter;
                        this.page_index = 0;
                        let search = this.search.read(cx).value().to_string();
                        this.request_history(search);
                        cx.notify();
                    }))
                    .child(label),
            );
        }
        row
    }

    fn page_button(
        &self,
        label: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(label)
            .px(px(8.))
            .py(px(4.))
            .border_1()
            .border_color(if enabled { LINE_STRONG } else { LINE })
            .rounded(px(4.))
            .text_color(if enabled { SUBTEXT } else { LINE_STRONG })
            .when(enabled, |this| {
                this.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.bump_selection();
                        if label == "Next" {
                            this.page_index += 1;
                        } else {
                            this.page_index = this.page_index.saturating_sub(1);
                        }
                        let search = this.search.read(cx).value().to_string();
                        this.request_history(search);
                        cx.notify();
                    }))
            })
            .child(label)
    }

    fn render_detail(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(detail) = self.detail.clone() else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(MUTED)
                .child("Select a recording")
                .into_any_element();
        };
        let recording = detail.recording.clone();
        if recording.failed {
            let id = recording.id;
            let retained_audio = !recording.audio_path.as_os_str().is_empty();
            let metadata_json = detail_json(&detail);
            let notice = self.notice.clone();
            let mut attempts = div().flex().flex_col().gap(px(6.));
            for attempt in detail.attempts.iter().rev() {
                attempts = attempts.child(
                    div()
                        .p(px(8.))
                        .bg(MANTLE)
                        .text_size(px(11.))
                        .child(format!(
                            "Attempt {} · {} · {} · {}",
                            attempt.number,
                            attempt.status,
                            attempt.model,
                            format_latency(attempt.latency_ms)
                        ))
                        .child(
                            div()
                                .mt(px(3.))
                                .text_color(RED)
                                .child(attempt.error.clone()),
                        ),
                );
            }
            return div()
                .flex_1()
                .flex()
                .flex_col()
                .p(px(26.))
                .gap(px(18.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .font_family(FONT_DISPLAY)
                                .text_size(px(22.))
                                .text_color(RED)
                                .child("Failed recording"),
                        )
                        .child(div().flex_1())
                        .when(retained_audio, |this| {
                            this.child(action_button(
                                "Retry",
                                cx.listener(move |view, _, _, cx| {
                                    view.backend.send(Request::Retry(id));
                                    view.notice = "Retry requested".to_string();
                                    cx.notify();
                                }),
                            ))
                        })
                        .when(!retained_audio, |this| {
                            this.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(MUTED)
                                    .child("Retry unavailable"),
                            )
                        }),
                )
                .when(!notice.is_empty(), |this| {
                    this.child(
                        div()
                            .px(px(10.))
                            .py(px(8.))
                            .bg(SURFACE)
                            .border_l_2()
                            .border_color(LINE_STRONG)
                            .text_size(px(11.))
                            .text_color(SUBTEXT)
                            .child(notice),
                    )
                })
                .child(
                    div()
                        .p(px(14.))
                        .bg(MANTLE)
                        .border_l_2()
                        .border_color(RED)
                        .text_color(RED)
                        .child(recording.error),
                )
                .child(div().text_color(MUTED).child(
                    if recording.audio_path.as_os_str().is_empty() {
                        "No audio was saved. Check the microphone and record again."
                    } else {
                        "The audio is retained and can be submitted again."
                    },
                ))
                .child(section_heading("ATTEMPTS"))
                .child(attempts)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .text_size(px(11.))
                        .children(self.render_delete_controls(id, cx))
                        .child(div().flex_1())
                        .child(action_button(
                            "Copy JSON",
                            cx.listener(move |view, _, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    metadata_json.clone(),
                                ));
                                view.notice = "Metadata copied".to_string();
                                cx.notify();
                            }),
                        )),
                )
                .into_any_element();
        }

        let id = recording.id;
        let expected_revision = recording.revision;
        let saved_text = recording.text.clone();
        let audio_path = recording.audio_path.clone();
        let metadata_json = detail_json(&detail);
        let original = detail
            .revisions
            .first()
            .map(|revision| revision.text.as_str())
            .unwrap_or(recording.text.as_str());
        let current_text = self.editor.read(cx).value();
        let changed = original != current_text.as_ref();
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .px(px(24.))
                    .pt(px(16.))
                    .child(
                        div()
                            .font_family(FONT_DISPLAY)
                            .text_size(px(20.))
                            .child(format_date(recording.timestamp)),
                    )
                    .child(
                        div()
                            .ml(px(10.))
                            .text_size(px(11.))
                            .text_color(MUTED)
                            .child(format!("#{}", recording.id)),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(MUTED)
                            .child(plural(recording.attempts, "attempt")),
                    ),
            )
            .child(self.render_player(audio_path, recording.duration_ms, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(24.))
                    .px(px(24.))
                    .py(px(14.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(MUTED)
                                    .child("TRANSCRIPT"),
                            )
                            .child(
                                div()
                                    .text_size(px(17.))
                                    .line_height(gpui::relative(1.5))
                                    .child(Input::new(&self.editor).appearance(false)),
                            )
                            .when(changed, |this| {
                                this.child(
                                    div()
                                        .mt(px(6.))
                                        .text_size(px(11.))
                                        .text_color(MUTED)
                                        .child("Changes from the model transcript"),
                                )
                                .child(self.render_diff(original, current_text.as_ref()))
                            }),
                    )
                    .child(self.render_metadata(&detail, cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(24.))
                    .py(px(10.))
                    .bg(MANTLE)
                    .border_t_1()
                    .border_color(LINE)
                    .text_size(px(11.))
                    .text_color(if self.notice == "Saved" { GREEN } else { MUTED })
                    .children(self.render_delete_controls(id, cx))
                    .child(self.notice.clone())
                    .child(div().flex_1())
                    .child(action_button(
                        "Copy",
                        cx.listener(|this, _, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                this.editor.read(cx).value().to_string(),
                            ));
                            this.notice = "Copied".to_string();
                            cx.notify();
                        }),
                    ))
                    .child(action_button(
                        "Copy JSON",
                        cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                metadata_json.clone(),
                            ));
                            this.notice = "Metadata copied".to_string();
                            cx.notify();
                        }),
                    ))
                    .child(action_button(
                        "Save",
                        cx.listener(move |this, _, _, cx| {
                            let text = this.editor.read(cx).value().to_string();
                            if !should_save(&saved_text, &text, false) {
                                this.notice = "No unsaved changes".to_string();
                            } else if this
                                .mutation_requests
                                .iter()
                                .any(|pending| pending_mutation_id(pending) == id)
                            {
                                this.notice = "Save already in progress".to_string();
                            } else {
                                this.mutation_requests.push_back(PendingMutation::Save {
                                    id,
                                    text: text.clone(),
                                });
                                this.backend.send(Request::SaveRevision {
                                    id,
                                    expected_revision,
                                    text,
                                });
                                this.notice = "Saving...".to_string();
                            }
                            cx.notify();
                        }),
                    )),
            )
            .into_any_element()
    }

    fn render_delete_controls(&self, id: i64, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        if self.confirm_delete == Some(id) {
            return vec![
                div()
                    .text_color(RED)
                    .child("Delete this recording and its audio?")
                    .into_any_element(),
                danger_button(
                    "Delete",
                    cx.listener(move |this, _, _, cx| {
                        this.request_delete(id);
                        cx.notify();
                    }),
                )
                .into_any_element(),
                action_button(
                    "Keep",
                    cx.listener(|this, _, _, cx| {
                        this.confirm_delete = None;
                        cx.notify();
                    }),
                )
                .into_any_element(),
            ];
        }
        vec![
            div()
                .id(("delete-recording", id as u64))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(4.))
                .text_color(MUTED)
                .cursor_pointer()
                .hover(|this| this.bg(alpha(RED, 0.12)).text_color(RED))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.confirm_delete = Some(id);
                    cx.notify();
                }))
                .child("Delete")
                .into_any_element(),
        ]
    }

    fn render_dictation(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let settings = self.backend.managed_settings.clone();
        let status = self.status.clone();
        let state = status.as_ref().map(|status| status.state);
        let recording = state == Some(DaemonState::Recording);
        let transcribing = state == Some(DaemonState::Transcribing);
        let busy = recording || transcribing;
        let connected = status.is_some();
        let (rms, peak) = status
            .as_ref()
            .map(|status| (status.audio_level_rms, status.audio_level_peak))
            .unwrap_or((0.0, 0.0));
        let elapsed = status
            .as_ref()
            .map(|status| status.duration_ms)
            .unwrap_or(0);
        let state_label = match state {
            Some(DaemonState::Recording) => format!("Recording · {}", format_duration(elapsed)),
            Some(DaemonState::Transcribing) => "Transcribing...".to_string(),
            Some(DaemonState::Typing) => "Pasting...".to_string(),
            Some(DaemonState::Error) => "Last recording failed".to_string(),
            Some(DaemonState::Idle) => "Ready".to_string(),
            None => "Daemon unavailable".to_string(),
        };
        let state_color = match state {
            Some(DaemonState::Recording) | Some(DaemonState::Error) => RED,
            Some(DaemonState::Transcribing) | Some(DaemonState::Typing) => BLUE,
            Some(DaemonState::Idle) => GREEN,
            None => MUTED,
        };
        let connection = self.connection.clone();
        let checking = self.connection_pending;
        let host = settings_host(&connection, &settings.endpoint);
        let daemon_line = match &status {
            Some(status) => format!("daemon up {}", format_uptime(status.uptime_seconds)),
            None => "daemon not running".to_string(),
        };
        let provider_line = match &connection {
            Some(report) => match &report.provider {
                Ok(detail) => format!("{host} · {detail}"),
                Err(error) => format!("{host} · {error}"),
            },
            None if checking => format!("{host} · checking..."),
            None => format!("{host} · not checked"),
        };
        let provider_ok = connection.as_ref().map(|report| report.provider.is_ok());
        let status_strip = div()
            .flex()
            .items_center()
            .gap(px(18.))
            .px(px(28.))
            .h(px(36.))
            .bg(MANTLE)
            .border_b_1()
            .border_color(LINE)
            .text_size(px(12.))
            .text_color(SUBTEXT)
            .child(status_pill(daemon_line, Some(connected)))
            .child(status_pill(
                format!("{} · {}", settings.provider, settings.model),
                None,
            ))
            .child(status_pill(provider_line, provider_ok))
            .child(div().flex_1())
            .child(
                div()
                    .id("check-connection")
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(4.))
                    .text_color(MUTED)
                    .cursor_pointer()
                    .hover(|this| this.bg(SURFACE_2).text_color(TEXT))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.check_connection();
                        cx.notify();
                    }))
                    .child(if checking {
                        "Checking..."
                    } else {
                        "Check connection"
                    }),
            );

        let hint = |value: &str| (!value.is_empty()).then(|| value.to_string());
        let toggle_hint = hint(&settings.shortcut_toggle);
        let cancel_hint = hint(&settings.shortcut_cancel);
        let record_panel = div()
            .w_full()
            .max_w(px(720.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(16.))
            .pt(px(28.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().size(px(9.)).rounded_full().bg(state_color))
                    .child(
                        div()
                            .font_family(FONT_DISPLAY)
                            .text_size(px(26.))
                            .child(state_label),
                    ),
            )
            .child(div().w_full().child(render_meter(recording, rms, peak)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .id("dictation-toggle")
                            .px(px(18.))
                            .py(px(9.))
                            .rounded(px(4.))
                            .text_size(px(13.))
                            .bg(if recording { SURFACE_2 } else { BLUE })
                            .text_color(if recording { TEXT } else { BG })
                            .when(!connected || transcribing, |this| this.opacity(0.5))
                            .when(connected && !transcribing, |this| {
                                this.cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.backend.send(Request::Toggle);
                                        cx.notify();
                                    }))
                            })
                            .child(if recording {
                                "Stop and transcribe"
                            } else {
                                "Start recording"
                            }),
                    )
                    .when_some(toggle_hint, |this, hint| this.child(key_hint(hint)))
                    .when(busy, |this| {
                        this.child(action_button(
                            "Cancel",
                            cx.listener(|this, _, _, cx| {
                                this.backend.send(Request::Cancel);
                                cx.notify();
                            }),
                        ))
                        .when_some(cancel_hint, |this, hint| this.child(key_hint(hint)))
                    }),
            );

        let latest = self
            .recent
            .iter()
            .find(|recording| !recording.failed)
            .cloned();
        let latest_id = latest.as_ref().map(|recording| recording.id);
        let last_result = latest.map(|recording| {
            let copy_text = recording.text.clone();
            let id = recording.id;
            div()
                .w_full()
                .max_w(px(720.))
                .flex()
                .items_start()
                .gap(px(12.))
                .px(px(14.))
                .py(px(12.))
                .bg(MANTLE)
                .border_1()
                .border_color(LINE)
                .rounded(px(6.))
                .child(
                    div()
                        .id("copy-last-result")
                        .flex_none()
                        .mt(px(2.))
                        .p(px(6.))
                        .rounded(px(4.))
                        .text_color(MUTED)
                        .cursor_pointer()
                        .hover(|this| this.bg(SURFACE_2).text_color(TEXT))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                copy_text.clone(),
                            ));
                            this.notice = "Copied".to_string();
                            cx.notify();
                        }))
                        .child(Icon::new(IconName::Copy).size(px(15.))),
                )
                .child(
                    div()
                        .id("last-result-text")
                        .flex_1()
                        .min_w_0()
                        .max_h(px(96.))
                        .overflow_y_scroll()
                        .text_size(px(14.))
                        .line_height(gpui::relative(1.5))
                        .text_color(TEXT)
                        .child(if recording.text.trim().is_empty() {
                            "No speech was transcribed.".to_string()
                        } else {
                            recording.text.clone()
                        }),
                )
                .child(
                    div()
                        .id("open-last-result")
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(MUTED)
                        .cursor_pointer()
                        .hover(|this| this.text_color(TEXT))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.reveal_recording(id, window, cx);
                            cx.notify();
                        }))
                        .child(format!("#{id}")),
                )
        });

        let recent: Vec<_> = self
            .recent
            .iter()
            .filter(|recording| Some(recording.id) != latest_id)
            .cloned()
            .collect();
        let mut recent_list = div()
            .w_full()
            .max_w(px(720.))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .pb(px(6.))
                    .child(section_heading("RECENT"))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("open-all-history")
                            .text_size(px(11.))
                            .text_color(MUTED)
                            .cursor_pointer()
                            .hover(|this| this.text_color(TEXT))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_tab(Tab::History);
                                cx.notify();
                            }))
                            .child("Open history"),
                    ),
            );
        if recent.is_empty() {
            recent_list = recent_list.child(div().text_color(MUTED).child("No other recordings."));
        }
        for recording in recent {
            let id = recording.id;
            let copy_text = recording.text.clone();
            let failed = recording.failed;
            let body = single_line(if failed {
                &recording.error
            } else {
                &recording.text
            });
            let body = if body.is_empty() {
                "No speech was transcribed".to_string()
            } else {
                body
            };
            recent_list = recent_list.child(
                div()
                    .id(("recent", id as u64))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(8.))
                    .py(px(7.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|this| this.bg(SURFACE))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.reveal_recording(id, window, cx);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(44.))
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(MUTED)
                            .child(format_time(recording.timestamp)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(if failed { RED } else { SUBTEXT })
                            .child(body),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(MUTED)
                            .child(format_duration(recording.duration_ms)),
                    )
                    .when(!failed, |this| {
                        this.child(
                            div()
                                .id(("recent-copy", id as u64))
                                .flex_none()
                                .p(px(4.))
                                .rounded(px(3.))
                                .text_color(MUTED)
                                .hover(|this| this.bg(SURFACE_2).text_color(TEXT))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        copy_text.clone(),
                                    ));
                                    this.notice = format!("Copied #{id}");
                                    cx.stop_propagation();
                                    cx.notify();
                                }))
                                .child(Icon::new(IconName::Copy).size(px(13.))),
                        )
                    }),
            );
        }

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(status_strip)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(20.))
                    .px(px(28.))
                    .pb(px(18.))
                    .child(record_panel)
                    .when_some(last_result, |this, result| this.child(result))
                    .child(recent_list),
            )
            .into_any_element()
    }

    fn render_player(
        &mut self,
        path: std::path::PathBuf,
        duration_ms: i64,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let playing = self.player.is_playing();
        let paused = self.player.is_paused();
        let position = self.player.position();
        let total = Duration::from_millis(duration_ms.max(0) as u64);
        let mut bars = div().flex_1().h(px(44.)).flex().items_center().gap(px(3.));
        let bucket_count = self.waveform.len().max(1);
        for (index, peak) in self.waveform.iter().enumerate() {
            let target = total.mul_f64(index as f64 / bucket_count as f64);
            let seek_path = path.clone();
            bars = bars.child(
                div()
                    .id(("waveform", index))
                    .flex_1()
                    .h(px(4. + peak * 36.))
                    .rounded_full()
                    .bg(if target <= position {
                        BLUE
                    } else {
                        alpha(MUTED, 0.6)
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let result = if this.player.path() == Some(seek_path.as_path()) {
                            this.player.seek(target)
                        } else {
                            this.player.play(&seek_path, target)
                        };
                        if let Err(error) = result {
                            this.notice = error.to_string();
                        }
                        cx.notify();
                    })),
            );
        }
        let button_path = path.clone();
        let readout = format!(
            "{} / {}{}",
            format_duration(position.as_millis() as i64),
            format_duration(duration_ms),
            if paused { " · paused" } else { "" }
        );
        div()
            .flex()
            .flex_col()
            .mx(px(24.))
            .mt(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .id("play-audio")
                            .size(px(34.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(BLUE)
                            .text_color(BG)
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let result = if this.player.path() != Some(button_path.as_path()) {
                                    this.player.play(&button_path, Duration::ZERO)
                                } else if this.player.is_playing() {
                                    this.player.pause()
                                } else if this.player.is_paused() {
                                    this.player.resume()
                                } else {
                                    this.player.play(&button_path, Duration::ZERO)
                                };
                                if let Err(error) = result {
                                    this.notice = error.to_string();
                                }
                                cx.notify();
                            }))
                            .child(if playing { "Ⅱ" } else { "▶" }),
                    )
                    .child(bars),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .mt(px(4.))
                    .text_size(px(11.))
                    .text_color(MUTED)
                    .child(readout),
            )
    }

    fn render_diff(&self, original: &str, current: &str) -> impl IntoElement {
        let mut diff = div()
            .flex()
            .flex_wrap()
            .items_end()
            .gap_x(px(4.))
            .gap_y(px(2.))
            .p(px(10.))
            .bg(MANTLE)
            .border_1()
            .border_color(LINE)
            .rounded(px(4.))
            .text_size(px(12.));
        for segment in word_diff(original, current) {
            diff = diff.child(match segment {
                DiffSegment::Same(word) => div().text_color(SUBTEXT).child(word),
                DiffSegment::Removed(word) => div()
                    .text_color(RED)
                    .bg(alpha(RED, 0.1))
                    .child(format!("−{word}")),
                DiffSegment::Added(word) => div()
                    .text_color(GREEN)
                    .bg(alpha(GREEN, 0.1))
                    .child(format!("+{word}")),
            });
        }
        diff
    }

    fn render_metadata(&self, detail: &Detail, cx: &mut Context<Self>) -> impl IntoElement {
        let recording = &detail.recording;
        let mut attempts = div().flex().flex_col().gap(px(6.));
        for attempt in detail.attempts.iter().rev().take(4) {
            attempts = attempts.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .bg(MANTLE)
                    .rounded(px(4.))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(format!("Attempt {} · {}", attempt.number, attempt.status))
                            .child(format_latency(attempt.latency_ms)),
                    )
                    .child(
                        div()
                            .mt(px(3.))
                            .text_size(px(10.))
                            .text_color(MUTED)
                            .truncate()
                            .child(attempt.model.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(MUTED)
                            .whitespace_nowrap()
                            .child(format!(
                                "{} → {}",
                                format_time(attempt.started_at),
                                format_time(attempt.finished_at)
                            )),
                    )
                    .when(!attempt.error.is_empty(), |this| {
                        this.child(
                            div()
                                .mt(px(3.))
                                .text_size(px(10.))
                                .text_color(RED)
                                .line_clamp(3)
                                .child(attempt.error.clone()),
                        )
                    }),
            );
        }
        let audio = recording
            .audio_path
            .file_name()
            .map(|name| middle_ellipsis(&name.to_string_lossy(), 28))
            .unwrap_or_else(|| "Not available".to_string());
        let audio_full = recording.audio_path.to_string_lossy().into_owned();
        let model_full = recording.model.clone();
        div()
            .id("metadata-scroll")
            .w(px(220.))
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(18.))
            .text_size(px(12.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(section_heading("RECORDING"))
                    .child(meta_row("Duration", format_duration(recording.duration_ms)))
                    .child(copyable_meta_row(
                        "Model",
                        recording.model.clone(),
                        model_full,
                        cx,
                    ))
                    .child(copyable_meta_row("Audio", audio, audio_full, cx))
                    .child(meta_row(
                        "Revision",
                        if recording.revision > 0 {
                            format!("Edited · revision {}", recording.revision)
                        } else {
                            "Model transcript".to_string()
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(section_heading("ATTEMPTS"))
                    .child(attempts),
            )
    }

    fn render_stats(&self) -> gpui::AnyElement {
        let Some(stats) = &self.stats else {
            return centered_message("Loading statistics...");
        };
        let minutes = stats.duration_ms as f64 / 60_000.0;
        let wpm = if minutes > 0.0 {
            stats.words as f64 / minutes
        } else {
            0.0
        };
        let week_delta = stats.this_week_words - stats.last_week_words;
        let week_note = if stats.last_week_words == 0 {
            "no words the week before".to_string()
        } else {
            format!(
                "{}{:.0}% vs previous week",
                if week_delta >= 0 { "+" } else { "−" },
                (week_delta.unsigned_abs() as f64 / stats.last_week_words as f64) * 100.0
            )
        };
        let active_days = stats
            .daily_last_30
            .iter()
            .filter(|day| day.recordings > 0)
            .count();
        let success_rate = if stats.total > 0 {
            format!(
                "{:.1}%",
                stats.successful as f64 / stats.total as f64 * 100.0
            )
        } else {
            "—".to_string()
        };
        let tiles = div()
            .flex()
            .border_y_1()
            .border_color(LINE)
            .child(stat_tile(
                format_count(stats.words),
                "words transcribed",
                format!("{} today", format_count(stats.today_words)),
            ))
            .child(stat_tile(
                format_count(stats.this_week_words),
                "words this week",
                week_note,
            ))
            .child(stat_tile(
                format!("{wpm:.0}"),
                "words per minute",
                format!("{} recorded", format_hours(stats.duration_ms)),
            ))
            .child(stat_tile(
                format_count(stats.total),
                "recordings",
                format!("{success_rate} succeeded · {} edited", stats.edited),
            ))
            .child(stat_tile(
                format_latency(stats.p50),
                "median latency",
                format!(
                    "p95 {} · p99 {}",
                    format_latency(stats.p95),
                    format_latency(stats.p99)
                ),
            ));

        div()
            .id("stats-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(28.))
            .py(px(16.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_end()
                    .child(
                        div()
                            .font_family(FONT_DISPLAY)
                            .text_size(px(22.))
                            .child("Your dictation"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(MUTED)
                            .child(format!("{active_days} active days in the last 30")),
                    ),
            )
            .child(tiles)
            .child(
                div()
                    .flex()
                    .gap(px(14.))
                    .child(
                        card()
                            .flex_1()
                            .min_w_0()
                            .child(chart_heading(
                                "Daily activity",
                                "Last 30 days · words per day",
                            ))
                            .child(render_activity_chart(stats)),
                    )
                    .child(
                        card()
                            .w(px(380.))
                            .flex_none()
                            .child(chart_heading("Time of day", "Recordings by hour"))
                            .child(render_hour_chart(stats)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(14.))
                    .child(
                        card()
                            .flex_1()
                            .min_w_0()
                            .child(chart_heading("Server latency", "Recent completed attempts"))
                            .child(render_latency_chart(stats)),
                    )
                    .child(
                        card()
                            .flex_1()
                            .min_w_0()
                            .child(chart_heading(
                                "Models",
                                format!(
                                    "Median length {} · average {}",
                                    format_duration(stats.median_duration_ms.unwrap_or(0)),
                                    format_duration(stats.average_duration_ms.unwrap_or(0))
                                ),
                            ))
                            .child(render_model_table(stats)),
                    ),
            )
            .into_any_element()
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let settings = &self.backend.managed_settings;
        let mut microphones = div().flex().flex_col();
        for (index, microphone) in self.microphones.iter().enumerate() {
            let up_ids = moved_ids(&self.microphones, index, -1);
            let down_ids = moved_ids(&self.microphones, index, 1);
            microphones = microphones.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(4.))
                    .py(px(12.))
                    .border_b_1()
                    .border_color(LINE)
                    .child(
                        div()
                            .w(px(22.))
                            .text_color(MUTED)
                            .child((index + 1).to_string()),
                    )
                    .child(
                        div().flex_1().child(microphone.name.clone()).child(
                            div()
                                .mt(px(4.))
                                .text_size(px(11.))
                                .text_color(if microphone.connected { GREEN } else { MUTED })
                                .child(if microphone.connected {
                                    if microphone.is_default {
                                        "Connected · system default"
                                    } else {
                                        "Connected"
                                    }
                                } else {
                                    "Disconnected"
                                }),
                        ),
                    )
                    .child(move_button(IconName::ArrowUp, index * 2, up_ids, cx))
                    .child(move_button(
                        IconName::ArrowDown,
                        index * 2 + 1,
                        down_ids,
                        cx,
                    )),
            );
        }
        div()
            .id("settings-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(36.))
            .py(px(24.))
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(820.))
                    .child(
                div()
                    .font_family(FONT_DISPLAY)
                    .text_size(px(24.))
                    .child("Settings"),
            )
            .child(section_title("MICROPHONE"))
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(gpui::relative(1.6))
                    .text_color(MUTED)
                    .child("Dictator uses the first connected input in this list. Disconnected inputs keep their place. Changes apply to the next recording."),
            )
            .child(microphones)
            .child(section_title("DAEMON CONFIGURATION"))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(MUTED)
                    .child(if settings.managed_by_home_manager {
                        "Managed by Home Manager. Configuration is read-only here. Provider keys are hidden."
                    } else {
                        "Configuration is read-only here. Provider keys are hidden."
                    }),
            )
            .child(setting_row("Provider", &settings.provider))
            .child(setting_row("Endpoint", &settings.endpoint))
            .child(setting_row("Model", &settings.model))
            .child(setting_row(
                "Request timeout",
                &if settings.timeout_seconds > 0 {
                    format!("{} seconds", settings.timeout_seconds)
                } else {
                    "Hidden".to_string()
                },
            ))
            .child(setting_row(
                "Maximum recording",
                &if settings.max_duration_minutes > 0 {
                    format!("{} minutes", settings.max_duration_minutes)
                } else {
                    "Hidden".to_string()
                },
            ))
            .child(setting_row("Paste shortcut", &settings.paste_shortcut))
            .child(setting_row("Notifications", &settings.notifications))
            )
            .into_any_element()
    }
}

impl Render for MainView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let recovery = self
            .status
            .as_ref()
            .and_then(|status| recovery_banner(status, false));
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(BG)
            .text_color(TEXT)
            .font_family(FONT_MONO)
            .text_size(px(13.))
            .child(self.render_titlebar(cx))
            .when_some(recovery, |this, recovery| this.child(recovery))
            .child(match self.tab {
                Tab::Dictation => self.render_dictation(cx),
                Tab::History => self.render_history(cx),
                Tab::Stats => self.render_stats(),
                Tab::Settings => self.render_settings(cx),
            })
    }
}

struct TrayView {
    options: GuiOptions,
    backend: Backend,
    status: Status,
    page: Option<Page>,
    stats: Option<Stats>,
    notice: String,
    status_pending: bool,
    last_status_request: Instant,
    connected: bool,
}

impl TrayView {
    fn new(options: GuiOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let backend = Backend::new(options.demo);
        backend.send(Request::Status);
        backend.send(Request::History {
            search: String::new(),
            filter: Filter::All,
            page: 0,
        });
        backend.send(Request::Stats);
        let weak = cx.weak_entity();
        window
            .spawn(cx, async move |cx| {
                loop {
                    Timer::after(Duration::from_millis(180)).await;
                    if !cx
                        .update(|window, cx| {
                            let Some(entity) = weak.upgrade() else {
                                return false;
                            };
                            entity.update(cx, |this, cx| this.poll(window, cx));
                            true
                        })
                        .unwrap_or(false)
                    {
                        break;
                    }
                }
            })
            .detach();
        Self {
            options,
            backend,
            status: Status {
                state: DaemonState::Idle,
                duration_ms: 0,
                error: String::new(),
                recovered_text: String::new(),
                uptime_seconds: 0,
                last_recording_id: None,
                last_recording_generation: 0,
                audio_level_rms: 0.0,
                audio_level_peak: 0.0,
            },
            page: None,
            stats: None,
            notice: String::new(),
            status_pending: true,
            last_status_request: Instant::now(),
            connected: false,
        }
    }

    fn poll(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        for reply in self.backend.drain() {
            match reply {
                Reply::Status(result) => {
                    self.status_pending = false;
                    match result {
                        Ok(status) => {
                            self.status = status;
                            self.connected = true;
                            if self.notice.starts_with("Daemon disconnected:") {
                                self.notice.clear();
                            }
                        }
                        Err(error) => {
                            self.connected = false;
                            self.notice = disconnected_notice(&error);
                        }
                    }
                }
                Reply::History(result) => {
                    apply_tray_result(&mut self.page, result, &mut self.notice)
                }
                Reply::Stats(result) => {
                    apply_tray_result(&mut self.stats, result, &mut self.notice)
                }
                Reply::Action(result) => {
                    self.notice = match result {
                        Ok(()) => String::new(),
                        Err(error) => action_notice(Err(error)),
                    };
                    if !self.status_pending {
                        self.backend.send(Request::Status);
                        self.status_pending = true;
                        self.last_status_request = Instant::now();
                    }
                    self.backend.send(Request::History {
                        search: String::new(),
                        filter: Filter::All,
                        page: 0,
                    });
                }
                _ => {}
            }
        }
        if !self.status_pending && self.last_status_request.elapsed() >= Duration::from_secs(1) {
            self.backend.send(Request::Status);
            self.status_pending = true;
            self.last_status_request = Instant::now();
        }
        cx.notify();
    }
}

impl Render for TrayView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.status.clone();
        let recording = status.state == DaemonState::Recording;
        let transcribing = status.state == DaemonState::Transcribing;
        let recent = self
            .page
            .as_ref()
            .map(|page| page.recordings.iter().take(5).cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let stats = self.stats.clone().unwrap_or_default();
        let today_minutes = stats.today_duration_ms as f64 / 60_000.0;
        let today_wpm = if today_minutes > 0.0 {
            stats.today_words as f64 / today_minutes
        } else {
            0.0
        };
        let options = self.options;
        let recovery = recovery_banner(&status, true);
        let notice = self.notice.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(BG)
            .text_color(TEXT)
            .font_family(FONT_MONO)
            .text_size(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .h(px(44.))
                    .bg(MANTLE)
                    .border_b_1()
                    .border_color(LINE)
                    .child(cursor_logo())
                    .child(
                        div()
                            .font_family(FONT_DISPLAY)
                            .text_size(px(15.))
                            .child("Dictator"),
                    ),
            )
            .when_some(recovery, |this, recovery| this.child(recovery))
            .when(!notice.is_empty(), |this| {
                this.child(
                    div()
                        .mx(px(12.))
                        .mt(px(8.))
                        .px(px(10.))
                        .py(px(7.))
                        .max_h(px(42.))
                        .overflow_hidden()
                        .bg(SURFACE)
                        .border_l_2()
                        .border_color(LINE_STRONG)
                        .text_size(px(11.))
                        .text_color(SUBTEXT)
                        .child(notice),
                )
            })
            .child(
                div()
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(render_meter(
                        recording,
                        status.audio_level_rms,
                        status.audio_level_peak,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(if transcribing {
                                "Transcribing...".to_string()
                            } else {
                                format_duration(status.duration_ms)
                            })
                            .child(
                                div()
                                    .id("tray-record")
                                    .size(px(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.))
                                    .when(recording, |this| this.bg(SURFACE_2))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.backend.send(Request::Toggle);
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .size(px(10.))
                                            .rounded(if recording { px(2.) } else { px(5.) })
                                            .bg(if transcribing { BLUE } else { RED }),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .border_y_1()
                    .border_color(LINE)
                    .child(tray_metric(stats.today_words.to_string(), "words today"))
                    .child(tray_metric(format!("{today_wpm:.0}"), "words / min")),
            )
            .child(
                div()
                    .flex()
                    .border_b_1()
                    .border_color(LINE)
                    .child(tray_metric(
                        format!("{:.1}m", stats.today_duration_ms as f64 / 60_000.0),
                        "audio today",
                    ))
                    .child(tray_metric(format_latency(stats.p95), "p95 latency")),
            )
            .child(
                div()
                    .px(px(16.))
                    .pt(px(12.))
                    .pb(px(5.))
                    .text_size(px(10.))
                    .text_color(MUTED)
                    .child("RECENT"),
            )
            .child(
                div()
                    .id("tray-recent-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(recent.into_iter().map(|recording| {
                        div()
                            .flex()
                            .items_center()
                            .h(px(32.))
                            .gap(px(9.))
                            .px(px(16.))
                            .py(px(7.))
                            .text_color(if recording.failed { RED } else { SUBTEXT })
                            .child(
                                div()
                                    .w(px(42.))
                                    .text_color(MUTED)
                                    .child(format_time(recording.timestamp)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(if recording.failed {
                                        recording.error
                                    } else {
                                        recording.text
                                    }),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(MUTED)
                                    .child(format_duration(recording.duration_ms)),
                            )
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .px(px(16.))
                    .h(px(42.))
                    .border_t_1()
                    .border_color(LINE)
                    .text_color(MUTED)
                    .child(if self.connected {
                        format!("daemon up {}", format_uptime(status.uptime_seconds))
                    } else {
                        "daemon disconnected".to_string()
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("open-history")
                            .cursor_pointer()
                            .text_color(SUBTEXT)
                            .on_click(move |_, window, cx| {
                                let _ = open_main_window(cx, options);
                                window.remove_window();
                            })
                            .child("open history"),
                    ),
            )
    }
}

fn card() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(14.))
        .bg(MANTLE)
        .border_1()
        .border_color(LINE)
        .rounded(px(6.))
}

fn key_hint(label: String) -> impl IntoElement {
    div()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(3.))
        .bg(SURFACE_2)
        .text_color(TEXT)
        .child(label)
}

fn status_pill(value: String, ok: Option<bool>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(7.))
        .child(div().size(px(7.)).flex_none().rounded_full().bg(match ok {
            Some(true) => GREEN,
            Some(false) => RED,
            None => LINE_STRONG,
        }))
        .child(div().truncate().child(value))
}

fn settings_host(connection: &Option<ConnectionReport>, endpoint: &str) -> String {
    connection
        .as_ref()
        .map(|report| report.endpoint_host.clone())
        .filter(|host| !host.is_empty())
        .or_else(|| {
            endpoint
                .split("//")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "Endpoint".to_string())
}

fn danger_button(
    label: &'static str,
    listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(label)
        .px(px(10.))
        .py(px(6.))
        .bg(alpha(RED, 0.15))
        .border_1()
        .border_color(RED)
        .rounded(px(4.))
        .text_color(RED)
        .cursor_pointer()
        .hover(|this| this.bg(alpha(RED, 0.3)))
        .on_click(listener)
        .child(label)
}

fn action_button(
    label: &'static str,
    listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(label)
        .px(px(10.))
        .py(px(6.))
        .bg(SURFACE)
        .border_1()
        .border_color(LINE_STRONG)
        .rounded(px(4.))
        .text_color(TEXT)
        .cursor_pointer()
        .hover(|this| this.bg(SURFACE_2))
        .on_click(listener)
        .child(label)
}

fn recovery_banner(status: &Status, compact: bool) -> Option<gpui::AnyElement> {
    if status.state != DaemonState::Error || status.recovered_text.is_empty() {
        return None;
    }

    let recovered_text = status.recovered_text.clone();
    let copy_text = recovered_text.clone();
    let error = if status.error.is_empty() {
        "The transcript could not be saved.".to_string()
    } else {
        status.error.clone()
    };
    Some(
        div()
            .mx(if compact { px(12.) } else { px(16.) })
            .my(px(8.))
            .p(px(if compact { 10. } else { 12. }))
            .bg(MANTLE)
            .border_l_2()
            .border_color(RED)
            .flex()
            .items_start()
            .gap(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_color(RED).child("Transcript was not saved"))
                    .child(
                        div()
                            .mt(px(3.))
                            .text_size(px(11.))
                            .text_color(MUTED)
                            .child(error),
                    )
                    .when(!recovered_text.is_empty(), |this| {
                        this.child(
                            div()
                                .id(if compact {
                                    "tray-recovered-text"
                                } else {
                                    "recovered-text"
                                })
                                .mt(px(7.))
                                .max_h(px(if compact { 38. } else { 64. }))
                                .overflow_y_scroll()
                                .text_color(SUBTEXT)
                                .child(recovered_text),
                        )
                    }),
            )
            .when(!copy_text.is_empty(), |this| {
                this.child(action_button("Copy recovered text", move |_, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_text.clone()));
                }))
            })
            .into_any_element(),
    )
}

fn section_heading(label: &'static str) -> impl IntoElement {
    div().text_size(px(10.)).text_color(MUTED).child(label)
}

fn section_title(label: &'static str) -> impl IntoElement {
    div()
        .mt(px(24.))
        .mb(px(8.))
        .text_size(px(11.))
        .text_color(MUTED)
        .child(label)
}

fn meta_row(label: &'static str, value: String) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(div().text_size(px(10.)).text_color(MUTED).child(label))
        .child(div().text_color(SUBTEXT).truncate().child(value))
}

fn copyable_meta_row(
    label: &'static str,
    shown: String,
    copy_text: String,
    cx: &mut Context<MainView>,
) -> impl IntoElement {
    let group = format!("meta-{label}");
    div()
        .group(group.clone())
        .flex()
        .items_end()
        .gap(px(6.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(px(10.)).text_color(MUTED).child(label))
                .child(div().text_color(SUBTEXT).truncate().child(shown)),
        )
        .child(
            div()
                .id(gpui::SharedString::from(format!("meta-copy-{label}")))
                .flex_none()
                .p(px(3.))
                .rounded(px(3.))
                .text_color(MUTED)
                .opacity(0.)
                .group_hover(group, |this| this.opacity(1.))
                .hover(|this| this.bg(SURFACE_2).text_color(TEXT))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_text.clone()));
                    this.notice = format!("{label} copied");
                    cx.notify();
                }))
                .child(Icon::new(IconName::Copy).size(px(12.))),
        )
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DiffSegment {
    Same(String),
    Removed(String),
    Added(String),
}

/// Splits on whitespace but keeps whitespace runs as their own tokens, so a
/// changed line break or doubled space shows up as a removed/added `⏎` or `␣`.
fn diff_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut gap = String::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
            gap.push(ch);
        } else {
            if !gap.is_empty() {
                tokens.push(std::mem::take(&mut gap));
            }
            word.push(ch);
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    if !gap.is_empty() {
        tokens.push(gap);
    }
    tokens
}

fn is_gap(token: &str) -> bool {
    token.chars().all(char::is_whitespace)
}

fn visible_gap(token: &str) -> String {
    token
        .chars()
        .map(|ch| match ch {
            '\n' => '⏎',
            '\t' => '⇥',
            _ => '␣',
        })
        .collect()
}

fn word_diff(original: &str, current: &str) -> Vec<DiffSegment> {
    let left = diff_tokens(original);
    let right = diff_tokens(current);
    let prefix = left.iter().zip(&right).take_while(|(a, b)| a == b).count();
    let suffix = left[prefix..]
        .iter()
        .rev()
        .zip(right[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let mut out = Vec::new();
    let push_same = |token: &str, out: &mut Vec<DiffSegment>| {
        if !is_gap(token) {
            out.push(DiffSegment::Same(token.to_string()));
        }
    };
    let shown = |token: &str| -> Option<String> {
        if !is_gap(token) {
            Some(token.to_string())
        } else if token == " " {
            None
        } else {
            Some(visible_gap(token))
        }
    };
    for token in &left[..prefix] {
        push_same(token, &mut out);
    }
    for token in &left[prefix..left.len() - suffix] {
        if let Some(text) = shown(token) {
            out.push(DiffSegment::Removed(text));
        }
    }
    let right_end = right.len() - suffix;
    for token in &right[prefix..right_end] {
        if let Some(text) = shown(token) {
            out.push(DiffSegment::Added(text));
        }
    }
    for token in &right[right_end..] {
        push_same(token, &mut out);
    }
    out
}

fn middle_ellipsis(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars || max_chars < 5 {
        return value.to_string();
    }
    let keep_end = max_chars / 3;
    let keep_start = max_chars - keep_end - 1;
    let start: String = value.chars().take(keep_start).collect();
    let end: String = value.chars().skip(count - keep_end).collect();
    format!("{start}…{end}")
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn tray_metric(value: String, label: &'static str) -> impl IntoElement {
    div()
        .w_1_2()
        .px(px(16.))
        .py(px(9.))
        .border_r_1()
        .border_color(LINE)
        .child(
            div()
                .font_family(FONT_DISPLAY)
                .text_size(px(16.))
                .child(value),
        )
        .child(div().text_size(px(9.)).text_color(MUTED).child(label))
}

fn render_meter(active: bool, rms: f32, peak: f32) -> impl IntoElement {
    let mut meter = div().h(px(30.)).flex().items_center().gap(px(2.));
    for index in 0..36 {
        let height = if active {
            let level = if index % 6 == 0 { peak } else { rms };
            2. + level.clamp(0.0, 1.0) * 28.0
        } else {
            2.
        };
        meter = meter.child(div().flex_1().h(px(height)).bg(if active {
            BLUE
        } else {
            alpha(MUTED, 0.45)
        }));
    }
    meter
}

fn heat(fraction: f32) -> Rgba {
    let t = fraction.clamp(0.0, 1.0);
    if t < 0.5 {
        mix(TEAL, BLUE, t * 2.0)
    } else {
        mix(BLUE, MAUVE, (t - 0.5) * 2.0)
    }
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
}

fn chart_with_axis(
    max: i64,
    height: f32,
    format: impl Fn(i64) -> String,
    bars: impl IntoElement,
    footer: impl IntoElement,
) -> impl IntoElement {
    let ticks = [max, max / 2, 0];
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .flex()
                .items_end()
                .child(
                    div()
                        .w(px(46.))
                        .flex_none()
                        .h(px(height))
                        .flex()
                        .flex_col()
                        .justify_between()
                        .items_end()
                        .pr(px(6.))
                        .text_size(px(10.))
                        .text_color(MUTED)
                        .children(ticks.into_iter().map(|tick| div().child(format(tick)))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(height))
                        .border_l_1()
                        .border_b_1()
                        .border_color(LINE_STRONG)
                        .pl(px(4.))
                        .flex()
                        .items_end()
                        .child(bars),
                ),
        )
        .child(div().pl(px(50.)).child(footer))
}

fn bar(fraction: f32, height: f32, filled: bool, color: Rgba) -> gpui::Div {
    div()
        .flex_1()
        .h(px(if filled {
            (fraction * height).max(3.)
        } else {
            2.
        }))
        .rounded_t(px(2.))
        .bg(if filled { color } else { alpha(MUTED, 0.25) })
}

fn axis_footer(left: String, middle: String, right: String) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .text_size(px(11.))
        .text_color(MUTED)
        .child(left)
        .child(middle)
        .child(right)
}

fn render_activity_chart(stats: &Stats) -> impl IntoElement {
    const HEIGHT: f32 = 110.;
    let max_words = stats
        .daily_last_30
        .iter()
        .map(|day| day.words)
        .max()
        .unwrap_or(0)
        .max(1);
    let mut bars = div().flex_1().h_full().flex().items_end().gap(px(3.));
    for (index, day) in stats.daily_last_30.iter().enumerate() {
        let fraction = day.words as f32 / max_words as f32;
        bars = bars.child(
            bar(fraction, HEIGHT, day.words > 0, heat(fraction))
                .id(("activity-day", index))
                .hover(|this| this.bg(TEXT)),
        );
    }
    let label = |date: Option<chrono::NaiveDate>| {
        date.map(|date| date.format("%b %-d").to_string())
            .unwrap_or_default()
    };
    chart_with_axis(
        max_words,
        HEIGHT,
        format_count,
        bars,
        axis_footer(
            label(stats.daily_last_30.first().map(|day| day.date)),
            format!("peak {} words", format_count(max_words)),
            label(stats.daily_last_30.last().map(|day| day.date)),
        ),
    )
}

fn render_hour_chart(stats: &Stats) -> impl IntoElement {
    const HEIGHT: f32 = 110.;
    let max = stats.by_hour.iter().copied().max().unwrap_or(0).max(1);
    let peak_hour = stats
        .by_hour
        .iter()
        .enumerate()
        .max_by_key(|(_, count)| **count)
        .filter(|(_, count)| **count > 0)
        .map(|(hour, _)| hour);
    let mut bars = div().flex_1().h_full().flex().items_end().gap(px(2.));
    for count in stats.by_hour.iter() {
        let fraction = *count as f32 / max as f32;
        bars = bars.child(bar(fraction, HEIGHT, *count > 0, heat(fraction)));
    }
    chart_with_axis(
        max,
        HEIGHT,
        format_count,
        bars,
        axis_footer(
            "00:00".to_string(),
            peak_hour
                .map(|hour| format!("busiest {hour:02}:00–{:02}:00", (hour + 1) % 24))
                .unwrap_or_default(),
            "23:00".to_string(),
        ),
    )
}

fn render_latency_chart(stats: &Stats) -> impl IntoElement {
    const HEIGHT: f32 = 96.;
    let max_value = stats
        .latency_samples
        .iter()
        .copied()
        .max()
        .unwrap_or(1)
        .max(1);
    let mut bins = [0_i64; 12];
    for sample in &stats.latency_samples {
        let index = ((*sample * bins.len() as i64) / (max_value + 1)) as usize;
        bins[index.min(bins.len() - 1)] += 1;
    }
    let max_bin = bins.iter().copied().max().unwrap_or(0).max(1);
    let mut bars = div().flex_1().h_full().flex().items_end().gap(px(3.));
    for (index, bin) in bins.into_iter().enumerate() {
        let position = index as f32 / (bins.len() - 1) as f32;
        bars = bars.child(bar(
            bin as f32 / max_bin as f32,
            HEIGHT,
            bin > 0,
            heat(position),
        ));
    }
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(chart_with_axis(
            max_bin,
            HEIGHT,
            |value| value.to_string(),
            bars,
            axis_footer(
                "0 s".to_string(),
                format!("{} samples", stats.latency_samples.len()),
                format_latency(Some(max_value)),
            ),
        ))
        .child(
            div()
                .flex()
                .justify_between()
                .child(mini_stat(format_latency(stats.p50), "p50"))
                .child(mini_stat(format_latency(stats.p95), "p95"))
                .child(mini_stat(format_latency(stats.p99), "p99"))
                .child(mini_stat(format_latency(stats.max), "max")),
        )
}

fn chart_heading(title: &'static str, note: impl Into<String>) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .items_center()
        .child(div().text_size(px(14.)).child(title))
        .child(
            div()
                .text_size(px(11.))
                .text_color(MUTED)
                .child(note.into()),
        )
}

fn render_model_table(stats: &Stats) -> impl IntoElement {
    let mut table = div().mt(px(4.)).border_t_1().border_color(LINE);
    table = table.child(table_row("Model", "Recordings", "Audio", "p95", true));
    for (model, recordings, duration_ms, p95) in stats.models.iter().take(4) {
        table = table.child(table_row(
            model,
            &recordings.to_string(),
            &format_hours(*duration_ms),
            &format_latency(*p95),
            false,
        ));
    }
    table
}

fn table_row(a: &str, b: &str, c: &str, d: &str, heading: bool) -> impl IntoElement {
    div()
        .flex()
        .py(px(8.))
        .border_b_1()
        .border_color(LINE)
        .text_size(px(12.))
        .text_color(if heading { MUTED } else { SUBTEXT })
        .child(div().w_2_5().truncate().child(a.to_string()))
        .child(div().w_1_5().child(b.to_string()))
        .child(div().w_1_5().child(c.to_string()))
        .child(div().w_1_5().child(d.to_string()))
}

fn stat_tile(value: String, label: &'static str, note: String) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .px(px(14.))
        .py(px(8.))
        .border_r_1()
        .border_color(LINE)
        .child(
            div()
                .font_family(FONT_DISPLAY)
                .text_size(px(21.))
                .child(value),
        )
        .child(div().mt(px(2.)).text_size(px(12.)).child(label))
        .child(
            div()
                .mt(px(2.))
                .text_size(px(11.))
                .text_color(MUTED)
                .truncate()
                .child(note),
        )
}

fn mini_stat(value: String, label: &'static str) -> impl IntoElement {
    div()
        .child(
            div()
                .font_family(FONT_DISPLAY)
                .text_size(px(17.))
                .child(value),
        )
        .child(div().text_size(px(11.)).text_color(MUTED).child(label))
}

fn format_count(value: i64) -> String {
    let digits = value.abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if value < 0 { format!("−{out}") } else { out }
}

fn format_hours(milliseconds: i64) -> String {
    let minutes = milliseconds.max(0) as f64 / 60_000.0;
    if minutes >= 90.0 {
        format!("{:.1} h", minutes / 60.0)
    } else {
        format!("{minutes:.0} min")
    }
}

fn setting_row(label: &str, value: &str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(16.))
        .py(px(12.))
        .border_b_1()
        .border_color(LINE)
        .child(div().w(px(200.)).flex_none().child(label.to_string()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .p(px(7.))
                .bg(SURFACE)
                .border_1()
                .border_color(LINE)
                .rounded(px(4.))
                .text_color(MUTED)
                .child(value.to_string()),
        )
}

fn move_button(
    icon: IconName,
    id: usize,
    ids: Option<Vec<String>>,
    cx: &mut Context<MainView>,
) -> impl IntoElement {
    let enabled = ids.is_some();
    div()
        .id(("microphone-move", id))
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .text_color(if enabled { SUBTEXT } else { LINE_STRONG })
        .when_some(ids, |this, ids| {
            this.cursor_pointer()
                .hover(|this| this.bg(SURFACE_2))
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.backend.send(Request::ReorderMicrophones(ids.clone()));
                    cx.notify();
                }))
        })
        .child(Icon::new(icon).size(px(14.)))
}

fn cursor_logo() -> impl IntoElement {
    div()
        .w(px(20.))
        .h(px(20.))
        .flex()
        .items_center()
        .gap(px(2.))
        .child(div().w(px(2.)).h(px(8.)).rounded_full().bg(TEXT))
        .child(div().w(px(2.)).h(px(15.)).rounded_full().bg(TEXT))
        .child(div().w(px(2.)).h(px(8.)).rounded_full().bg(TEXT))
        .child(div().w(px(4.)))
        .child(div().w(px(2.)).h(px(18.)).rounded_full().bg(TEXT))
}

fn moved_ids(microphones: &[Microphone], index: usize, direction: isize) -> Option<Vec<String>> {
    let target = index as isize + direction;
    if target < 0 || target >= microphones.len() as isize {
        return None;
    }
    let mut ids: Vec<_> = microphones
        .iter()
        .map(|microphone| microphone.id.clone())
        .collect();
    ids.swap(index, target as usize);
    Some(ids)
}

fn centered_message(message: &'static str) -> gpui::AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(MUTED)
        .child(message)
        .into_any_element()
}

fn detail_json(detail: &Detail) -> String {
    let recording = &detail.recording;
    serde_json::to_string_pretty(&serde_json::json!({
        "id": recording.id,
        "captured_at": recording.timestamp,
        "duration_ms": recording.duration_ms,
        "status": if recording.failed { "failed" } else { "complete" },
        "model": recording.model,
        "text": recording.text,
        "error": recording.error,
        "current_revision": recording.revision,
        "original_text": detail.revisions.first().map(|revision| &revision.text),
        "revisions": detail.revisions.iter().map(|revision| serde_json::json!({
            "revision": revision.number,
            "created_at": revision.timestamp,
            "source": revision.source,
            "text": revision.text,
        })).collect::<Vec<_>>(),
        "attempts": detail.attempts.iter().map(|attempt| serde_json::json!({
            "attempt": attempt.number,
            "started_at": attempt.started_at,
            "finished_at": attempt.finished_at,
            "latency_ms": attempt.latency_ms,
            "status": attempt.status,
            "model": attempt.model,
            "error": attempt.error,
        })).collect::<Vec<_>>(),
    }))
    .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}"))
}

fn accepts_reply(request_generation: u64, current_generation: u64) -> bool {
    request_generation == current_generation
}

fn should_save(saved_text: &str, editor_text: &str, pending: bool) -> bool {
    !pending && saved_text != editor_text
}

fn resolved_editor_text(draft: Option<&str>, server_text: &str) -> String {
    draft.unwrap_or(server_text).to_string()
}

fn track_editor_change(
    drafts: &mut HashMap<i64, String>,
    editor_recording: Option<i64>,
    baseline: Option<(i64, &str)>,
    editor_text: &str,
) {
    let (Some(editor_id), Some((baseline_id, server_text))) = (editor_recording, baseline) else {
        return;
    };
    if editor_id != baseline_id {
        return;
    }
    if editor_text == server_text {
        drafts.remove(&editor_id);
    } else {
        drafts.insert(editor_id, editor_text.to_string());
    }
}

fn normalize_draft(
    drafts: &mut HashMap<i64, String>,
    recording_id: i64,
    server_text: &str,
) -> bool {
    if drafts
        .get(&recording_id)
        .is_some_and(|draft| draft == server_text)
    {
        drafts.remove(&recording_id);
    }
    drafts.contains_key(&recording_id)
}

fn accept_search_change(last_value: &mut String, incoming: &str) -> Option<String> {
    if *last_value == incoming {
        return None;
    }
    incoming.clone_into(last_value);
    Some(incoming.to_string())
}

fn apply_history_error(loading: &mut bool, notice: &mut String, error: String) {
    *loading = false;
    *notice = error;
}

fn can_go_previous(page_index: usize) -> bool {
    page_index > 0
}

fn apply_tray_result<T>(slot: &mut Option<T>, result: Result<T, String>, notice: &mut String) {
    match result {
        Ok(value) => {
            *slot = Some(value);
            clear_database_notice(notice);
        }
        Err(error) => {
            *slot = None;
            *notice = error;
        }
    }
}

fn clear_database_notice(notice: &mut String) {
    if notice.starts_with("Database unavailable:") {
        notice.clear();
    }
}

fn action_notice(result: Result<(), String>) -> String {
    match result {
        Ok(()) => "Request accepted".to_string(),
        Err(error) => format!("Request failed: {error}"),
    }
}

fn choose_history_selection(
    reveal_id: &mut Option<i64>,
    visible_ids: &[i64],
    current_id: Option<i64>,
) -> Option<i64> {
    if let Some(id) = reveal_id.take() {
        return Some(id);
    }
    current_id
        .filter(|id| visible_ids.contains(id))
        .or_else(|| visible_ids.first().copied())
}

fn pending_mutation_id(pending: &PendingMutation) -> i64 {
    match pending {
        PendingMutation::Save { id, .. } => *id,
    }
}

fn complete_mutation(pending: Option<PendingMutation>, detail_id: i64) -> CompletedMutation {
    match pending {
        Some(PendingMutation::Save { id, text }) if id == detail_id => {
            CompletedMutation::Save(text)
        }
        _ => CompletedMutation::Unexpected,
    }
}

fn disconnected_notice(error: &str) -> String {
    format!("Daemon disconnected: {error}")
}

fn observe_recording_generation(last_seen: &mut Option<u64>, incoming: u64) -> bool {
    let changed = last_seen.is_some_and(|previous| previous != incoming);
    *last_seen = Some(incoming);
    changed
}

fn format_time(timestamp: DateTime<Utc>) -> String {
    timestamp.with_timezone(&Local).format("%H:%M").to_string()
}

fn format_date(timestamp: DateTime<Utc>) -> String {
    timestamp
        .with_timezone(&Local)
        .format("%A, %B %-d")
        .to_string()
}

fn format_duration(milliseconds: i64) -> String {
    let seconds = milliseconds.max(0) as f64 / 1000.0;
    format!("{}:{:04.1}", (seconds / 60.0) as i64, seconds % 60.0)
}

fn format_latency(milliseconds: Option<i64>) -> String {
    milliseconds
        .map(|value| format!("{:.2} s", value as f64 / 1000.0))
        .unwrap_or_else(|| "—".to_string())
}

fn format_uptime(seconds: u64) -> String {
    if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    } else {
        format!("{}m", seconds / 60)
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    #[test]
    fn dirty_draft_wins_over_delayed_saved_reply() {
        assert_eq!(
            resolved_editor_text(Some("typed after save"), "submitted snapshot"),
            "typed after save"
        );
    }

    #[test]
    fn stale_selection_reply_is_rejected() {
        assert!(accepts_reply(7, 7));
        assert!(!accepts_reply(6, 7));
    }

    #[test]
    fn unchanged_or_pending_revision_is_not_saved() {
        assert!(!should_save("same", "same", false));
        assert!(!should_save("old", "new", true));
        assert!(should_save("old", "new", false));
    }

    #[test]
    fn poll_error_is_visible_as_disconnected() {
        assert_eq!(
            disconnected_notice("socket unavailable"),
            "Daemon disconnected: socket unavailable"
        );
    }

    #[test]
    fn reconnect_observes_terminal_generation_once() {
        let mut last_seen = None;
        assert!(!observe_recording_generation(&mut last_seen, 8));
        // A transport error does not clear the independent checkpoint.
        assert!(observe_recording_generation(&mut last_seen, 9));
        assert!(!observe_recording_generation(&mut last_seen, 9));
    }

    #[test]
    fn failed_history_request_finishes_loading_and_surfaces_error() {
        let mut loading = true;
        let mut notice = String::new();

        apply_history_error(
            &mut loading,
            &mut notice,
            "database unavailable".to_string(),
        );

        assert!(!loading);
        assert_eq!(notice, "database unavailable");
    }

    #[test]
    fn terminal_reveal_is_consumed_before_later_history_reload() {
        let mut reveal_id = Some(99);

        assert_eq!(
            choose_history_selection(&mut reveal_id, &[1, 2], Some(2)),
            Some(99)
        );
        assert_eq!(reveal_id, None);
        assert_eq!(
            choose_history_selection(&mut reveal_id, &[1, 2], Some(2)),
            Some(2)
        );
    }

    #[test]
    fn save_reply_matches_its_pending_mutation() {
        let pending = || {
            Some(PendingMutation::Save {
                id: 12,
                text: "edited".to_string(),
            })
        };
        assert_eq!(
            complete_mutation(pending(), 12),
            CompletedMutation::Save("edited".to_string())
        );
        assert_eq!(
            complete_mutation(pending(), 13),
            CompletedMutation::Unexpected
        );
    }

    #[test]
    fn diff_tracks_whitespace_changes() {
        use DiffSegment::*;
        assert_eq!(
            word_diff("a b", "a\n\nb"),
            vec![Same("a".into()), Added("⏎⏎".into()), Same("b".into())]
        );
        assert_eq!(
            word_diff("Bofo's at 64%.", "Bofo's at 64%"),
            vec![
                Same("Bofo's".into()),
                Same("at".into()),
                Removed("64%.".into()),
                Added("64%".into()),
            ]
        );
        assert_eq!(
            word_diff("same text", "same text"),
            vec![Same("same".into()), Same("text".into())]
        );
        assert_eq!(
            word_diff("x  y", "x y"),
            vec![Same("x".into()), Removed("␣␣".into()), Same("y".into())]
        );
    }

    #[test]
    fn counts_and_hours_are_human_readable() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(598_702), "598,702");
        assert_eq!(format_count(1_000_000), "1,000,000");
        assert_eq!(format_hours(45 * 60_000), "45 min");
        assert_eq!(format_hours(321_480_000), "89.3 h");
    }

    #[test]
    fn middle_ellipsis_keeps_both_ends() {
        assert_eq!(middle_ellipsis("short.wav", 28), "short.wav");
        let long = "09192026-194510-f5913661-a7fe-4cfa-b27c-a944fc5d9c20.wav";
        let shortened = middle_ellipsis(long, 28);
        assert_eq!(shortened.chars().count(), 28);
        assert!(shortened.starts_with("09192026-194510"));
        assert!(shortened.ends_with("c20.wav"));
        assert_eq!(single_line("a\n\n b\tc "), "a b c");
        assert_eq!(plural(1, "attempt"), "1 attempt");
        assert_eq!(plural(3, "attempt"), "3 attempts");
    }

    #[test]
    fn programmatic_editor_change_does_not_create_a_draft() {
        let mut drafts = HashMap::new();

        track_editor_change(
            &mut drafts,
            Some(7),
            Some((7, "server transcript")),
            "server transcript",
        );

        assert!(!drafts.contains_key(&7));
    }

    #[test]
    fn delayed_change_from_previous_selection_is_ignored() {
        let mut drafts = HashMap::new();

        track_editor_change(
            &mut drafts,
            Some(8),
            Some((7, "old transcript")),
            "old transcript",
        );

        assert!(drafts.is_empty());
    }

    #[test]
    fn matching_false_draft_is_dropped_but_real_edit_is_preserved() {
        let mut drafts = HashMap::from([(7, "restored transcript".to_string())]);
        assert!(!normalize_draft(&mut drafts, 7, "restored transcript"));
        assert!(!drafts.contains_key(&7));

        drafts.insert(7, "typed while restoring".to_string());
        assert!(normalize_draft(&mut drafts, 7, "restored transcript"));
        assert_eq!(
            drafts.get(&7).map(String::as_str),
            Some("typed while restoring")
        );
    }

    #[test]
    fn delayed_programmatic_search_clear_is_ignored() {
        let mut accepted_value = String::new();

        assert_eq!(accept_search_change(&mut accepted_value, ""), None);
        assert_eq!(accepted_value, "");
    }

    #[test]
    fn real_search_change_updates_the_accepted_value() {
        let mut accepted_value = String::new();

        assert_eq!(
            accept_search_change(&mut accepted_value, "deployment"),
            Some("deployment".to_string())
        );
        assert_eq!(accepted_value, "deployment");
        assert_eq!(
            accept_search_change(&mut accepted_value, "deployment"),
            None
        );
    }

    #[test]
    fn previous_page_is_enabled_after_first_page() {
        assert!(!can_go_previous(0));
        assert!(can_go_previous(1));
        assert!(can_go_previous(4));
    }

    #[test]
    fn tray_backend_failure_replaces_stale_data_and_is_visible() {
        let mut value = Some(41);
        let mut notice = String::new();

        apply_tray_result(
            &mut value,
            Err("history unavailable".to_string()),
            &mut notice,
        );

        assert_eq!(value, None);
        assert_eq!(notice, "history unavailable");

        notice = "Database unavailable: transient WAL error".to_string();
        apply_tray_result(&mut value, Ok(42), &mut notice);
        assert_eq!(value, Some(42));
        assert!(notice.is_empty());
    }

    #[test]
    fn rejected_retry_action_has_explicit_feedback() {
        assert_eq!(
            action_notice(Err("recording is no longer failed".to_string())),
            "Request failed: recording is no longer failed"
        );
    }
}
