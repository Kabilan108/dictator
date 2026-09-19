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
    AnyWindowHandle, App, Application, Bounds, Context, Entity, Global, IntoElement, Subscription,
    Timer, Window, WindowBounds, WindowKind, WindowOptions, div, prelude::*, px, size,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Icon, IconName, Root, Theme, ThemeMode};

use crate::ipc::DaemonState;
use crate::tray::{TrayAction, TrayHandle};

use backend::{Backend, Detail, Filter, Microphone, Page, Reply, Request, Stats, Status};
use placement::place_popup;
use playback::{AudioPlayer, waveform};
use theme::*;

const APP_ID: &str = "org.dictator.Dictator";

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
                Cow::Borrowed(include_bytes!(
                    "../../assets/fonts/InstrumentSerif-Regular.ttf"
                )),
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
    inspected_revision: Option<i64>,
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
    Restore { id: i64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CompletedMutation {
    Save(String),
    Restore,
    Unexpected,
}

impl MainView {
    fn new(demo: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let backend = Backend::new(demo);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("search recordings"));
        let editor = cx.new(|cx| InputState::new(window, cx).multi_line(true));
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
            tab: Tab::History,
            filter: Filter::All,
            page_index: 0,
            page: None,
            detail: None,
            stats: None,
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
            inspected_revision: None,
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
        this
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
        self.inspected_revision = None;
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
                            CompletedMutation::Restore => {
                                let id = detail.recording.id;
                                let preserved_draft =
                                    normalize_draft(&mut self.drafts, id, &detail.recording.text);
                                if self.editor_recording == Some(id) {
                                    self.apply_detail(detail, None, window, cx);
                                }
                                self.notice = if preserved_draft {
                                    "Revision restored; unsaved draft preserved".to_string()
                                } else {
                                    "Revision restored".to_string()
                                };
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
                            let generation_changed = observe_recording_generation(
                                &mut self.last_seen_recording_generation,
                                status.last_recording_generation,
                            );
                            if generation_changed && let Some(id) = status.last_recording_id {
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
        match tab {
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
                    .text_size(px(18.))
                    .mr(px(10.))
                    .child("Dictator"),
            );
        for (tab, label) in [
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
                        .text_size(px(11.))
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
            for recording in &page.recordings {
                let id = recording.id;
                let selected = self
                    .detail
                    .as_ref()
                    .is_some_and(|detail| detail.recording.id == id);
                let text = if recording.failed {
                    recording.error.clone()
                } else {
                    recording.text.clone()
                };
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
                                .text_size(px(10.))
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
                                .text_size(px(12.))
                                .text_color(if recording.failed { RED } else { SUBTEXT })
                                .max_h(px(36.))
                                .overflow_hidden()
                                .child(text),
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
                            .text_size(px(10.))
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
            .text_size(px(10.))
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
                        .text_size(px(10.))
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
                                .text_size(px(25.))
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
                                    .text_size(px(10.))
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
                            .text_size(px(10.))
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
                        .text_size(px(10.))
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
        let baseline_revision = self
            .inspected_revision
            .and_then(|number| {
                detail
                    .revisions
                    .iter()
                    .find(|revision| revision.number == number)
            })
            .or_else(|| detail.revisions.first());
        let original = baseline_revision
            .map(|revision| revision.text.as_str())
            .unwrap_or(recording.text.as_str());
        let current_text = self.editor.read(cx).value();
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
                            .text_size(px(23.))
                            .child(format_date(recording.timestamp)),
                    )
                    .child(
                        div()
                            .ml(px(10.))
                            .text_size(px(10.))
                            .text_color(MUTED)
                            .child(format!("#{}", recording.id)),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(MUTED)
                            .child(format!("{} attempts", recording.attempts)),
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
                                    .text_size(px(10.))
                                    .text_color(MUTED)
                                    .child("TRANSCRIPT"),
                            )
                            .child(
                                div()
                                    .h(px(176.))
                                    .text_size(px(17.))
                                    .line_height(gpui::relative(1.5))
                                    .child(Input::new(&self.editor).h_full().appearance(false)),
                            )
                            .when_some(baseline_revision, |this, revision| {
                                this.child(div().text_size(px(10.)).text_color(MUTED).child(
                                    format!(
                                        "Comparing revision {} with the editable transcript",
                                        revision.number
                                    ),
                                ))
                            })
                            .child(self.render_diff(original, current_text.as_ref())),
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
                    .text_size(px(10.))
                    .text_color(if self.notice == "Saved" { GREEN } else { MUTED })
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
                        "Save revision",
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
        let mut bars = div().flex_1().h(px(44.)).flex().items_center().gap(px(2.));
        let bucket_count = self.waveform.len().max(1);
        for (index, peak) in self.waveform.iter().enumerate() {
            let target = total.mul_f64(index as f64 / bucket_count as f64);
            let seek_path = path.clone();
            bars = bars.child(
                div()
                    .id(("waveform", index))
                    .flex_1()
                    .h(px(3. + peak * 37.))
                    .rounded(px(1.))
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
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .mx(px(24.))
            .mt(px(10.))
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
            .child(bars)
            .child(div().text_size(px(11.)).text_color(MUTED).child(format!(
                "{} / {}{}",
                format_duration(position.as_millis() as i64),
                format_duration(duration_ms),
                if paused { " paused" } else { "" }
            )))
    }

    fn render_diff(&self, original: &str, current: &str) -> impl IntoElement {
        let original_words: Vec<_> = original.split_whitespace().collect();
        let current_words: Vec<_> = current.split_whitespace().collect();
        let prefix = original_words
            .iter()
            .zip(&current_words)
            .take_while(|(left, right)| left == right)
            .count();
        let suffix = original_words[prefix..]
            .iter()
            .rev()
            .zip(current_words[prefix..].iter().rev())
            .take_while(|(left, right)| left == right)
            .count();
        let mut diff = div()
            .flex()
            .flex_wrap()
            .gap(px(4.))
            .p(px(10.))
            .bg(MANTLE)
            .border_1()
            .border_color(LINE)
            .rounded(px(4.))
            .text_size(px(11.));
        for word in &original_words[..prefix] {
            diff = diff.child(div().text_color(SUBTEXT).child((*word).to_string()));
        }
        let original_end = original_words.len().saturating_sub(suffix);
        for word in &original_words[prefix..original_end] {
            diff = diff.child(
                div()
                    .text_color(RED)
                    .bg(alpha(RED, 0.1))
                    .child(format!("−{word}")),
            );
        }
        let current_end = current_words.len().saturating_sub(suffix);
        for word in &current_words[prefix..current_end] {
            diff = diff.child(
                div()
                    .text_color(GREEN)
                    .bg(alpha(GREEN, 0.1))
                    .child(format!("+{word}")),
            );
        }
        for word in &current_words[current_end..] {
            diff = diff.child(div().text_color(SUBTEXT).child((*word).to_string()));
        }
        diff
    }

    fn render_metadata(&self, detail: &Detail, cx: &mut Context<Self>) -> impl IntoElement {
        let recording = &detail.recording;
        let recording_id = recording.id;
        let recording_revision = recording.revision;
        let mut revisions = div().flex().flex_col().gap(px(2.));
        for revision in detail.revisions.iter().rev() {
            let number = revision.number;
            revisions = revisions.child(
                div()
                    .id(("revision", number as u64))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(7.))
                    .py(px(5.))
                    .rounded(px(4.))
                    .hover(|this| this.bg(SURFACE))
                    .cursor_pointer()
                    .when(self.inspected_revision == Some(number), |this| {
                        this.bg(SURFACE)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.inspected_revision = Some(number);
                        cx.notify();
                    }))
                    .child(div().size(px(7.)).rounded_full().bg(BLUE))
                    .child(div().flex_1().child(format!("Revision {number}")).child(
                        div().text_size(px(9.)).text_color(MUTED).child(format!(
                            "{} · {} · {} words",
                            format_time(revision.timestamp),
                            revision.source,
                            revision.text.split_whitespace().count()
                        )),
                    )),
            );
        }
        let mut attempts = div().flex().flex_col().gap(px(6.));
        for attempt in detail.attempts.iter().rev().take(4) {
            attempts =
                attempts.child(
                    div()
                        .px(px(7.))
                        .py(px(5.))
                        .bg(MANTLE)
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .child(format!("Attempt {} · {}", attempt.number, attempt.status))
                                .child(format_latency(attempt.latency_ms)),
                        )
                        .child(div().mt(px(3.)).text_size(px(9.)).text_color(MUTED).child(
                            format!(
                                "{} · {} → {}{}",
                                attempt.model,
                                format_time(attempt.started_at),
                                format_time(attempt.finished_at),
                                if attempt.error.is_empty() {
                                    String::new()
                                } else {
                                    format!(" · {}", attempt.error)
                                }
                            ),
                        )),
                );
        }
        div()
            .id("metadata-scroll")
            .w(px(220.))
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(18.))
            .text_size(px(11.))
            .child(section_heading("RECORDING"))
            .child(meta_row("Duration", format_duration(recording.duration_ms)))
            .child(meta_row("Model", recording.model.clone()))
            .child(meta_row(
                "Audio",
                recording
                    .audio_path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Not available".to_string()),
            ))
            .child(section_heading("ATTEMPTS"))
            .child(attempts)
            .child(section_heading("REVISIONS"))
            .child(revisions)
            .when_some(self.inspected_revision, |this, revision| {
                this.child(action_button(
                    "Restore selected revision",
                    cx.listener(move |view, _, _, cx| {
                        if view
                            .mutation_requests
                            .iter()
                            .any(|pending| pending_mutation_id(pending) == recording_id)
                        {
                            view.notice = "A transcript change is already in progress".to_string();
                        } else {
                            view.mutation_requests
                                .push_back(PendingMutation::Restore { id: recording_id });
                            view.backend.send(Request::RestoreRevision {
                                id: recording_id,
                                revision,
                                expected_revision: recording_revision,
                            });
                            view.notice = "Restoring revision...".to_string();
                        }
                        cx.notify();
                    }),
                ))
            })
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
        div()
            .id("stats-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(28.))
            .py(px(18.))
            .child(
                div()
                    .font_family(FONT_DISPLAY)
                    .text_size(px(27.))
                    .child("Your dictation"),
            )
            .child(
                div()
                    .mt(px(3.))
                    .text_size(px(11.))
                    .text_color(MUTED)
                    .child(format!(
                        "All history · {} recordings · {} failed · {} edited",
                        stats.total, stats.failed, stats.edited
                    )),
            )
            .child(
                div()
                    .flex()
                    .my(px(14.))
                    .border_y_1()
                    .border_color(LINE)
                    .child(metric(stats.words.to_string(), "words transcribed"))
                    .child(metric(format!("{wpm:.0}"), "words per minute"))
                    .child(metric(
                        format!("{:.1} h", stats.duration_ms as f64 / 3_600_000.0),
                        "recorded audio",
                    ))
                    .child(metric(format_latency(stats.p95), "server p95 latency")),
            )
            .child(
                div()
                    .flex()
                    .gap(px(24.))
                    .h(px(190.))
                    .child(render_daily_chart(stats))
                    .child(render_latency_chart(stats)),
            )
            .child(render_model_table(stats))
            .into_any_element()
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let settings = &self.backend.managed_settings;
        let mut microphones = div().flex().flex_col().max_w(px(850.));
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
                                .text_size(px(10.))
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
            .child(
                div()
                    .font_family(FONT_DISPLAY)
                    .text_size(px(28.))
                    .child("Settings"),
            )
            .child(section_title("MICROPHONE"))
            .child(
                div()
                    .max_w(px(850.))
                    .text_size(px(11.))
                    .line_height(gpui::relative(1.6))
                    .text_color(MUTED)
                    .child("Dictator uses the first connected input in this list. Disconnected inputs keep their place. Changes apply to the next recording."),
            )
            .child(microphones)
            .child(section_title("DAEMON CONFIGURATION"))
            .child(
                div()
                    .text_size(px(11.))
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
            .text_size(px(12.))
            .child(self.render_titlebar(cx))
            .when_some(recovery, |this, recovery| this.child(recovery))
            .child(match self.tab {
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
                    self.notice = action_notice(result);
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
            .text_size(px(11.))
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
                            .text_size(px(17.))
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
                        .text_size(px(10.))
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
                    .text_size(px(9.))
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
                            .text_size(px(10.))
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
    div().text_size(px(9.)).text_color(MUTED).child(label)
}

fn section_title(label: &'static str) -> impl IntoElement {
    div()
        .mt(px(24.))
        .mb(px(8.))
        .text_size(px(10.))
        .text_color(MUTED)
        .child(label)
}

fn meta_row(label: &'static str, value: String) -> impl IntoElement {
    div()
        .flex()
        .gap(px(8.))
        .child(div().w(px(62.)).text_color(MUTED).child(label))
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .text_color(SUBTEXT)
                .child(value),
        )
}

fn metric(value: String, label: &'static str) -> impl IntoElement {
    div()
        .flex_1()
        .px(px(16.))
        .py(px(12.))
        .border_r_1()
        .border_color(LINE)
        .child(
            div()
                .font_family(FONT_DISPLAY)
                .text_size(px(27.))
                .child(value),
        )
        .child(
            div()
                .mt(px(3.))
                .text_size(px(10.))
                .text_color(MUTED)
                .child(label),
        )
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
                .text_size(px(18.))
                .child(value),
        )
        .child(div().text_size(px(8.)).text_color(MUTED).child(label))
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

fn render_daily_chart(stats: &Stats) -> impl IntoElement {
    let max = stats
        .daily
        .iter()
        .map(|(_, count)| *count)
        .max()
        .unwrap_or(1) as f32;
    let mut bars = div()
        .flex_1()
        .h(px(130.))
        .flex()
        .items_end()
        .gap(px(7.))
        .border_b_1()
        .border_color(LINE);
    for (date, count) in &stats.daily {
        bars = bars.child(
            div()
                .flex_1()
                .h_full()
                .flex()
                .flex_col()
                .justify_end()
                .items_center()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(px(9.))
                        .text_color(MUTED)
                        .child(count.to_string()),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(((*count as f32 / max) * 92.).max(2.)))
                        .bg(alpha(BLUE, 0.65)),
                )
                .child(
                    div()
                        .text_size(px(8.))
                        .text_color(MUTED)
                        .child(date.format("%m-%d").to_string()),
                ),
        );
    }
    div()
        .w_1_2()
        .child(chart_heading("Recording count", "Last seven days"))
        .child(bars)
}

fn render_latency_chart(stats: &Stats) -> impl IntoElement {
    let max_value = stats
        .latency_samples
        .iter()
        .copied()
        .max()
        .unwrap_or(1)
        .max(1);
    let mut bins = [0_i64; 8];
    for sample in &stats.latency_samples {
        let index = ((*sample * bins.len() as i64) / (max_value + 1)) as usize;
        bins[index.min(bins.len() - 1)] += 1;
    }
    let max_bin = bins.iter().copied().max().unwrap_or(1).max(1) as f32;
    let mut bars = div()
        .h(px(102.))
        .flex()
        .items_end()
        .gap(px(3.))
        .border_b_1()
        .border_color(LINE);
    for (index, bin) in bins.into_iter().enumerate() {
        let upper_ms = max_value * (index as i64 + 1) / 8;
        bars = bars.child(
            div()
                .flex_1()
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_end()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(8.))
                        .text_color(MUTED)
                        .child(bin.to_string()),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(((bin as f32 / max_bin) * 72.).max(2.)))
                        .bg(alpha(BLUE, 0.55)),
                )
                .child(
                    div()
                        .text_size(px(7.))
                        .text_color(MUTED)
                        .child(format!("{upper_ms}ms")),
                ),
        );
    }
    div()
        .w_1_2()
        .child(chart_heading("Server latency", "Recent completed attempts"))
        .child(bars)
        .child(
            div()
                .flex()
                .justify_between()
                .mt(px(8.))
                .text_size(px(9.))
                .text_color(MUTED)
                .child(format!("p50 {}", format_latency(stats.p50)))
                .child(format!("p95 {}", format_latency(stats.p95)))
                .child(format!("p99 {}", format_latency(stats.p99)))
                .child(format!("max {}", format_latency(stats.max))),
        )
}

fn chart_heading(title: &'static str, note: &'static str) -> impl IntoElement {
    div()
        .flex()
        .justify_between()
        .items_center()
        .mb(px(8.))
        .child(div().text_size(px(12.)).child(title))
        .child(div().text_size(px(9.)).text_color(MUTED).child(note))
}

fn render_model_table(stats: &Stats) -> impl IntoElement {
    let mut table = div().mt(px(10.)).border_t_1().border_color(LINE);
    table = table.child(table_row(
        "Model",
        "Recordings",
        "Audio",
        "p95 latency",
        true,
    ));
    for (model, recordings, duration_ms, p95) in &stats.models {
        table = table.child(table_row(
            model,
            &recordings.to_string(),
            &format!("{:.1} h", *duration_ms as f64 / 3_600_000.0),
            &format_latency(*p95),
            false,
        ));
    }
    table.child(
        div()
            .mt(px(8.))
            .text_size(px(9.))
            .text_color(MUTED)
            .child(format!("{} successful recordings", stats.successful)),
    )
}

fn table_row(a: &str, b: &str, c: &str, d: &str, heading: bool) -> impl IntoElement {
    div()
        .flex()
        .py(px(8.))
        .border_b_1()
        .border_color(LINE)
        .text_size(px(10.))
        .text_color(if heading { MUTED } else { SUBTEXT })
        .child(div().w_2_5().child(a.to_string()))
        .child(div().w_1_5().child(b.to_string()))
        .child(div().w_1_5().child(c.to_string()))
        .child(div().w_1_5().child(d.to_string()))
}

fn setting_row(label: &str, value: &str) -> impl IntoElement {
    div()
        .max_w(px(850.))
        .flex()
        .items_center()
        .py(px(12.))
        .border_b_1()
        .border_color(LINE)
        .child(div().flex_1().child(label.to_string()))
        .child(
            div()
                .w(px(360.))
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
        PendingMutation::Save { id, .. } | PendingMutation::Restore { id } => *id,
    }
}

fn complete_mutation(pending: Option<PendingMutation>, detail_id: i64) -> CompletedMutation {
    match pending {
        Some(PendingMutation::Save { id, text }) if id == detail_id => {
            CompletedMutation::Save(text)
        }
        Some(PendingMutation::Restore { id }) if id == detail_id => CompletedMutation::Restore,
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
    fn restore_reply_matches_its_pending_mutation() {
        assert_eq!(
            complete_mutation(Some(PendingMutation::Restore { id: 12 }), 12),
            CompletedMutation::Restore
        );
        assert_eq!(
            complete_mutation(Some(PendingMutation::Restore { id: 12 }), 13),
            CompletedMutation::Unexpected
        );
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
    fn restore_replaces_matching_false_draft_but_preserves_real_edit() {
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
