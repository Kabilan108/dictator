//! The History pane: a searchable list of recordings beside the selected
//! one's audio, editable transcript and metadata.

use std::time::Duration;

use chrono::Local;
use rune::{El, Span, TextStyle, Tx, col, rich, row, text};
use rune_kit::{
    ButtonVariant, ControlSize, IconButton, IconButtonVariant, SegmentedControl, TextArea,
    TextInput,
};

use super::backend::{Attempt, Detail, Filter, Recording, Request};
use super::controls::{self, GUTTER, Paint, Tone};
use super::logic::{self, DiffSegment};
use super::{Ui, charts, icons};

const LIST_WIDTH: f32 = 296.0;
const PAGE_SIZE: usize = 50;
const FILTERS: [(Filter, &str); 3] = [
    (Filter::All, "All"),
    (Filter::Edited, "Edited"),
    (Filter::Failed, "Failed"),
];
const DETAIL_TITLE: TextStyle = TextStyle::new(15.0, 600.0).lh(20.0);

pub(super) fn pane(ui: &Ui, p: Paint) -> (Vec<El>, El) {
    let body = row()
        .key("history")
        .grow(1.0)
        .shrink()
        .min_h(0.0)
        .pl(GUTTER - 10.0)
        .pr(GUTTER - controls::ROW_PX)
        .pb(GUTTER - controls::ROW_PX)
        .gap(14.0)
        .child(list_column(ui, p))
        .child(detail(ui, p));
    (Vec::new(), body)
}

fn list_column(ui: &Ui, p: Paint) -> El {
    let search = TextInput::new(&ui.search)
        .placeholder("Search recordings")
        .label("Search recordings")
        .leading_icon(&icons::SEARCH)
        .clear_button()
        .on_edit(|ui: &mut Ui| &mut ui.search)
        .on_change(|ui: &mut Ui, _: &mut Tx| ui.search_changed())
        .build(p.k)
        .key("search");
    let selected = FILTERS.iter().position(|(filter, _)| *filter == ui.filter);
    let labels: Vec<&str> = FILTERS.iter().map(|(_, label)| *label).collect();
    let filter = SegmentedControl::new(&labels, selected)
        .label("Filter")
        .width(None)
        .on_pick(|ui: &mut Ui, index: usize, tx: &mut Tx| {
            tx.instant();
            ui.set_filter(FILTERS[index].0);
        })
        .build(p.k)
        .key("filter");

    let list = match &ui.page {
        _ if ui.loading && ui.page.is_none() => {
            controls::empty_state(p, "list-loading", &icons::HISTORY, "Loading history…", "")
        }
        Some(page) if !page.recordings.is_empty() => {
            let selected = ui.detail.as_ref().map(|detail| detail.recording.id);
            let mut rows = Vec::new();
            let mut last_date = None;
            for recording in &page.recordings {
                let date = recording.timestamp.with_timezone(&Local).date_naive();
                if last_date != Some(date) {
                    last_date = Some(date);
                    rows.push(
                        text(logic::format_date(recording.timestamp))
                            .key(format!("date-{date}"))
                            .font(p.k.ty.small_medium)
                            .color(p.faint())
                            .px(10.0)
                            .pt(if rows.is_empty() { 4.0 } else { 14.0 })
                            .pb(4.0),
                    );
                }
                rows.push(list_row(p, recording, selected == Some(recording.id)));
            }
            controls::scroll_fill(
                p,
                "list-scroll",
                col()
                    .key("rows")
                    .role("list")
                    .gap(1.0)
                    .pr(8.0)
                    .children(rows),
            )
        }
        _ => {
            let (title, hint) = if ui.notice.is_empty() {
                ("No matching recordings", "Try another search or filter")
            } else {
                (ui.notice.as_str(), "")
            };
            controls::empty_state(p, "list-empty", &icons::SEARCH, title, hint)
        }
    };

    col()
        .key("list")
        .w(LIST_WIDTH)
        .min_h(0.0)
        .gap(10.0)
        .child(col().px(2.0).gap(8.0).child(search).child(filter))
        .child(list)
        .child(pager(ui, p))
}

fn list_row(p: Paint, recording: &Recording, selected: bool) -> El {
    let id = recording.id;
    let snippet = logic::single_line(if recording.failed {
        &recording.error
    } else {
        &recording.text
    });
    let (snippet, color) = if recording.failed {
        (snippet, p.danger())
    } else if snippet.is_empty() {
        ("No speech was transcribed".to_string(), p.faint())
    } else {
        (snippet, if selected { p.text() } else { p.muted() })
    };
    let tag = if recording.failed {
        Some(("failed", p.danger()))
    } else if recording.revision > 0 {
        Some(("edited", p.accent()))
    } else {
        None
    };
    let meta = format!(
        "{} · {}",
        logic::format_time(recording.timestamp),
        logic::format_duration(recording.duration_ms)
    );
    let item = col()
        .key(format!("recording-{id}"))
        .role("listitem")
        .label(snippet.clone())
        .description(meta.clone())
        .focusable(true)
        .focus_selected(selected)
        .px(10.0)
        .py(8.0)
        .gap(3.0)
        .radius(p.k.radius.md)
        .child(
            row()
                .gap(8.0)
                .center_y()
                .child(
                    text(meta)
                        .font(p.numeric(p.k.ty.small))
                        .color(p.faint())
                        .grow(1.0)
                        .a11y_hidden(),
                )
                .children(tag.map(|(label, color)| text(label).font(p.k.ty.small).color(color))),
        )
        .child(
            text(snippet)
                .font(p.k.ty.ui.lh(18.0))
                .color(color)
                .wrap()
                .max_lines(2)
                .a11y_hidden(),
        );
    let item = if selected {
        item.fill(p.k.c.surface_raised_base)
    } else {
        item.hover(p.k.c.hover)
    };
    item.on_press_tx(move |ui: &mut Ui, tx: &mut Tx| {
        if !selected {
            tx.instant();
            ui.select_recording(id);
        }
    })
}

fn pager(ui: &Ui, p: Paint) -> El {
    let (total, has_next) = ui
        .page
        .as_ref()
        .map_or((0, false), |page| (page.total, page.has_next));
    let pages = (total.max(1) as usize).div_ceil(PAGE_SIZE).max(1);
    let step = |key: &str, glyph, label: &str, enabled: bool, forward: bool| {
        if enabled {
            controls::icon_button(p, key, glyph, label, move |ui: &mut Ui, tx: &mut Tx| {
                tx.instant();
                ui.turn_page(forward);
            })
        } else {
            controls::disabled_icon_button(p, key, glyph, label)
        }
    };
    row()
        .key("pager")
        .px(10.0)
        .gap(4.0)
        .center_y()
        .child(
            text(format!(
                "{} {}",
                logic::format_count(total),
                if total == 1 {
                    "recording"
                } else {
                    "recordings"
                }
            ))
            .font(p.numeric(p.k.ty.small))
            .color(p.faint())
            .grow(1.0),
        )
        .child(step(
            "page-previous",
            &icons::CHEVRON_LEFT,
            "Previous page",
            logic::can_go_previous(ui.page_index),
            false,
        ))
        .child(
            text(format!("{} / {pages}", ui.page_index + 1))
                .font(p.numeric(p.k.ty.small))
                .color(p.muted()),
        )
        .child(step(
            "page-next",
            &icons::CHEVRON_RIGHT,
            "Next page",
            has_next,
            true,
        ))
}

fn detail(ui: &Ui, p: Paint) -> El {
    let card = controls::raised(p, "detail")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .min_h(0.0);
    let Some(detail) = &ui.detail else {
        return card.child(controls::empty_state(
            p,
            "detail-empty",
            &icons::HISTORY,
            "Select a recording",
            "Its audio and transcript open here",
        ));
    };
    let recording = &detail.recording;
    let body = if recording.failed {
        failed_body(p, detail)
    } else {
        transcript_body(ui, p, detail)
    };
    card.child(detail_header(ui, p, detail))
        .child(controls::separator(p))
        .child(controls::scroll_fill(p, "detail-scroll", body))
        .child(controls::separator(p))
        .child(detail_footer(ui, p, detail))
}

fn detail_header(ui: &Ui, p: Paint, detail: &Detail) -> El {
    let recording = &detail.recording;
    let json = logic::detail_json(detail);
    let mut actions = Vec::new();
    if !recording.failed {
        actions.push(controls::icon_button(
            p,
            "copy-text",
            &icons::COPY,
            "Copy transcript",
            |ui: &mut Ui, tx: &mut Tx| {
                tx.copy(ui.editor.text.clone());
                ui.notice = "Copied".to_string();
            },
        ));
    }
    actions.push(controls::small_button(
        p,
        "copy-json",
        "Copy JSON",
        ButtonVariant::Ghost,
        move |ui: &mut Ui, tx: &mut Tx| {
            tx.copy(json.clone());
            ui.notice = "Metadata copied".to_string();
        },
    ));
    let subtitle = format!(
        "{} · {} · #{}",
        logic::format_time(recording.timestamp),
        logic::format_duration(recording.duration_ms),
        recording.id
    );
    let title = if recording.failed {
        text("Failed recording").color(p.danger())
    } else {
        text(logic::format_date(recording.timestamp)).color(p.strong())
    };
    let _ = ui;
    row()
        .key("detail-header")
        .px(controls::ROW_PX)
        .py(12.0)
        .gap(6.0)
        .center_y()
        .child(
            col()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(2.0)
                .child(title.role("heading").level(2).font(DETAIL_TITLE).truncate())
                .child(
                    text(subtitle)
                        .font(p.numeric(p.k.ty.small))
                        .color(p.muted())
                        .truncate(),
                ),
        )
        .children(actions)
}

fn transcript_body(ui: &Ui, p: Paint, detail: &Detail) -> El {
    let recording = &detail.recording;
    let original = detail
        .revisions
        .first()
        .map_or(recording.text.as_str(), |revision| revision.text.as_str());
    let editor = TextArea::new(&ui.editor)
        .label("Transcript")
        .placeholder("No speech was transcribed")
        .width(None)
        .auto_grow(4, 14)
        .on_edit(|ui: &mut Ui| &mut ui.editor)
        .on_change(|ui: &mut Ui, _: &mut Tx| ui.editor_changed())
        .build(p.k)
        .key("editor");
    let mut sections = vec![player(ui, p, recording), editor];
    if original != ui.editor.text {
        sections.push(
            col()
                .key("diff")
                .gap(6.0)
                .child(controls::section_label(
                    p,
                    "Changes from the model transcript",
                ))
                .child(diff(p, original, &ui.editor.text)),
        );
    }
    sections.push(facts(p, detail));
    col()
        .key("transcript")
        .px(controls::ROW_PX)
        .py(14.0)
        .gap(16.0)
        .children(sections)
}

fn player(ui: &Ui, p: Paint, recording: &Recording) -> El {
    let total = Duration::from_millis(recording.duration_ms.max(0) as u64);
    let here = ui.playback.path.as_deref() == Some(recording.audio_path.as_path());
    let position = if here {
        ui.playback.position
    } else {
        Duration::ZERO
    };
    let played = if total.is_zero() {
        0.0
    } else {
        (position.as_secs_f32() / total.as_secs_f32()).clamp(0.0, 1.0)
    };
    let playing = here && ui.playback.playing;
    let toggle = IconButton::new(if playing { &icons::PAUSE } else { &icons::PLAY })
        .variant(IconButtonVariant::Neutral)
        .control_size(ControlSize::Large)
        .label(if playing { "Pause" } else { "Play" })
        .on_press(|ui: &mut Ui| ui.toggle_playback())
        .build(p.k)
        .key("play");
    let clock = text(format!(
        "{} / {}",
        logic::format_duration(position.as_millis() as i64),
        logic::format_duration(recording.duration_ms)
    ))
    .font(p.numeric(p.k.ty.small))
    .color(p.muted());
    row()
        .key("player")
        .gap(12.0)
        .center_y()
        .child(toggle)
        .child(charts::waveform(
            p,
            &ui.waveform,
            if here { played } else { 0.0 },
            36.0,
            move |ui: &mut Ui, at: f32, _: &mut Tx| ui.seek_playback(total.mul_f32(at)),
        ))
        .child(clock)
}

/// The model's transcript against the edited one, word by word.
fn diff(p: Paint, original: &str, current: &str) -> El {
    let mut content = String::new();
    let mut spans = Vec::new();
    let mut strikes = Vec::new();
    for segment in logic::word_diff(original, current) {
        let start = content.len();
        match segment {
            DiffSegment::Same(word) => content.push_str(&word),
            DiffSegment::Removed(word) => {
                content.push_str(&word);
                spans.push(
                    Span::new(start, content.len())
                        .color(p.danger())
                        .bg(p.k.c.danger.bg, 1.0),
                );
                strikes.push((start, content.len(), Some(p.danger())));
            }
            DiffSegment::Added(word) => {
                content.push_str(&word);
                spans.push(
                    Span::new(start, content.len())
                        .color(p.success())
                        .bg(p.k.c.success.bg, 1.0),
                );
            }
        }
        content.push(' ');
    }
    rich(content.trim_end(), spans)
        .strikes(strikes)
        .font(p.k.ty.ui.lh(22.0))
        .color(p.muted())
        .wrap()
}

fn facts(p: Paint, detail: &Detail) -> El {
    let recording = &detail.recording;
    let audio = recording.audio_path.display().to_string();
    let mut rows = vec![
        fact(
            p,
            "Model",
            if recording.model.is_empty() {
                "Not available"
            } else {
                &recording.model
            },
        ),
        fact(
            p,
            "Latency",
            &logic::format_latency(
                detail
                    .attempts
                    .last()
                    .and_then(|attempt| attempt.latency_ms),
            ),
        ),
        fact(p, "Revision", &recording.revision.to_string()),
        fact(p, "Attempts", &logic::plural(recording.attempts, "attempt")),
    ];
    if !audio.is_empty() {
        let path = audio.clone();
        rows.push(
            row()
                .key("fact-audio")
                .gap(12.0)
                .center_y()
                .child(fact_label(p, "Audio"))
                .child(
                    text(audio)
                        .font(p.k.ty.small)
                        .color(p.muted())
                        .truncate_middle()
                        .grow(1.0)
                        .shrink(),
                )
                .child(controls::icon_button(
                    p,
                    "copy-audio-path",
                    &icons::COPY,
                    "Copy path",
                    move |ui: &mut Ui, tx: &mut Tx| {
                        tx.copy(path.clone());
                        ui.notice = "Path copied".to_string();
                    },
                )),
        );
    }
    col()
        .key("facts")
        .gap(6.0)
        .child(controls::section_label(p, "Recording"))
        .children(rows)
}

fn fact_label(p: Paint, label: &str) -> El {
    text(label).font(p.k.ty.small).color(p.faint()).w(72.0)
}

fn fact(p: Paint, label: &str, value: &str) -> El {
    row()
        .key(format!("fact-{label}"))
        .gap(12.0)
        .min_h(22.0)
        .center_y()
        .child(fact_label(p, label))
        .child(
            text(value)
                .font(p.numeric(p.k.ty.small))
                .color(p.muted())
                .selectable(true)
                .selection_color(p.k.selection()),
        )
}

fn failed_body(p: Paint, detail: &Detail) -> El {
    let recording = &detail.recording;
    let retained = !recording.audio_path.as_os_str().is_empty();
    let attempts = detail
        .attempts
        .iter()
        .rev()
        .map(|attempt| attempt_row(p, attempt));
    col()
        .key("failed")
        .px(controls::ROW_PX)
        .py(14.0)
        .gap(16.0)
        .child(controls::banner(
            p,
            "error",
            &recording.error,
            Tone::Bad,
            None,
        ))
        .child(controls::note(
            p,
            "retained",
            if retained {
                "The audio is kept and can be sent again."
            } else {
                "No audio was saved. Check the microphone and record again."
            },
            Tone::Neutral,
        ))
        .child(
            col()
                .key("attempts")
                .gap(8.0)
                .child(controls::section_label(p, "Attempts"))
                .children(attempts),
        )
}

fn attempt_row(p: Paint, attempt: &Attempt) -> El {
    col()
        .key(format!("attempt-{}", attempt.number))
        .gap(2.0)
        .child(
            text(format!(
                "Attempt {} · {} · {} · {}",
                attempt.number,
                attempt.status,
                if attempt.model.is_empty() {
                    "unknown model"
                } else {
                    &attempt.model
                },
                logic::format_latency(attempt.latency_ms)
            ))
            .font(p.numeric(p.k.ty.small))
            .color(p.muted()),
        )
        .children((!attempt.error.is_empty()).then(|| {
            text(attempt.error.clone())
                .font(p.k.ty.small.lh(16.0))
                .color(p.danger())
                .wrap()
        }))
}

fn detail_footer(ui: &Ui, p: Paint, detail: &Detail) -> El {
    let recording = &detail.recording;
    let id = recording.id;
    let mut left = Vec::new();
    if recording.failed {
        if !recording.audio_path.as_os_str().is_empty() {
            left.push(controls::small_button(
                p,
                "retry",
                "Retry",
                ButtonVariant::Contrast,
                move |ui: &mut Ui, _: &mut Tx| {
                    ui.backend.send(Request::Retry(id));
                    ui.notice = "Retry requested".to_string();
                },
            ));
        }
    } else {
        let dirty = logic::should_save(&recording.text, &ui.editor.text, false);
        left.push(
            rune_kit::Button::new("Save")
                .variant(if dirty {
                    ButtonVariant::Contrast
                } else {
                    ButtonVariant::Neutral
                })
                .control_size(ControlSize::Small)
                .disabled(!dirty)
                .shortcut("mod+s")
                .on_press(|ui: &mut Ui| ui.save_revision())
                .build(p.k)
                .key("save"),
        );
    }
    let notice_tone = match ui.notice.as_str() {
        "Saved" | "Copied" | "Metadata copied" | "Path copied" => Tone::Good,
        _ => Tone::Neutral,
    };
    let notice = (!ui.notice.is_empty())
        .then(|| controls::note(p, "notice", &ui.notice, notice_tone).max_lines(1));

    if ui.confirm_delete == Some(id) {
        // The question takes the whole row: beside Save it would not fit a
        // narrow window.
        return row()
            .key("detail-footer")
            .px(controls::ROW_PX)
            .py(10.0)
            .gap(10.0)
            .center_y()
            .child(
                controls::note(
                    p,
                    "delete-question",
                    "Delete the recording and its audio?",
                    Tone::Bad,
                )
                .max_lines(2)
                .grow(1.0)
                .shrink(),
            )
            .child(controls::small_button(
                p,
                "delete-keep",
                "Keep",
                ButtonVariant::Ghost,
                |ui: &mut Ui, _: &mut Tx| ui.confirm_delete = None,
            ))
            .child(controls::small_button(
                p,
                "delete-confirm",
                "Delete",
                ButtonVariant::Danger,
                move |ui: &mut Ui, _: &mut Tx| ui.request_delete(id),
            ));
    }
    let right = vec![controls::small_button(
        p,
        "delete",
        "Delete…",
        ButtonVariant::Ghost,
        move |ui: &mut Ui, _: &mut Tx| ui.confirm_delete = Some(id),
    )];
    row()
        .key("detail-footer")
        .px(controls::ROW_PX)
        .py(10.0)
        .gap(10.0)
        .center_y()
        .children(left)
        .child(col().grow(1.0).shrink().children(notice))
        .children(right)
}
