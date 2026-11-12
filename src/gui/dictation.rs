//! The Dictation pane: the live meter and its one action, the last result and
//! the most recent recordings. What the daemon is doing shows in the
//! sidebar's status row, not here.

use rune::{El, TextStyle, Tx, col, row, text};
use rune_kit::ButtonVariant;

use crate::ipc::DaemonState;

use crate::provider::CheckOutcome;

use super::backend::{Recording, Request, Status};
use super::controls::{self, GUTTER, Paint, ROW_PX, SECTION_GAP, Tone};
use super::logic::{ErrorKind, ErrorSummary};
use super::{Pane, Ui, charts, icons, logic};

const TIMER: TextStyle = TextStyle::new(28.0, 500.0).lh(34.0);

pub(super) fn pane(ui: &Ui, p: Paint) -> (Vec<El>, El) {
    let mut sections = Vec::new();
    sections.extend(banners(ui, p));
    sections.push(stage(ui, p));
    if let Some(last) = ui.recent.first() {
        sections.push(last_result(p, last));
    }
    sections.push(recent(ui, p));
    let page = col()
        .key("dictation")
        .px(GUTTER - ROW_PX)
        .pt(4.0)
        .pb(GUTTER)
        .gap(SECTION_GAP)
        .children(sections);
    (
        Vec::new(),
        controls::scroll_fill(p, "dictation-scroll", page),
    )
}

fn banners(ui: &Ui, p: Paint) -> Vec<El> {
    let mut banners = Vec::new();
    match &ui.status {
        None => banners.push(controls::banner(
            p,
            "daemon-down",
            "The daemon is not running. Start it and the controls will return.",
            Tone::Bad,
            None,
        )),
        Some(status) => banners.extend(recovery(p, status)),
    }
    if ui.status.is_some()
        && let Some(summary) = provider_problem(ui)
    {
        banners.push(controls::error_callout(
            p,
            "provider-down",
            &summary,
            vec![settings_button(p, "Open settings")],
        ));
    }
    banners
}

/// The last provider check's failure, said as the reason recordings will
/// fail.
fn provider_problem(ui: &Ui) -> Option<ErrorSummary> {
    let check = &ui.connection.as_ref()?.check;
    let provider = &check.provider;
    let (title, kind) = match check.outcome {
        CheckOutcome::Ready | CheckOutcome::Reachable => return None,
        CheckOutcome::KeyRejected => (format!("{provider} rejected the API key"), ErrorKind::Key),
        CheckOutcome::Unreachable => (format!("Can't reach {provider}"), ErrorKind::Provider),
        CheckOutcome::Failed => (
            format!("{provider} is answering with errors"),
            ErrorKind::Provider,
        ),
        CheckOutcome::Unusable => (format!("{provider} is not set up"), ErrorKind::Provider),
    };
    let detail = if check.message.is_empty() {
        "Recordings will fail until this is fixed.".to_string()
    } else {
        format!(
            "{} Recordings will fail until this is fixed.",
            end_sentence(&check.message)
        )
    };
    Some(ErrorSummary {
        title,
        detail,
        kind,
        raw: None,
    })
}

fn end_sentence(text: &str) -> String {
    let text = text.trim_end();
    if text.ends_with(['.', '!', '?', '…']) {
        text.to_string()
    } else {
        format!("{text}.")
    }
}

fn settings_button(p: Paint, label: &str) -> El {
    controls::small_button(
        p,
        "open-settings",
        label,
        ButtonVariant::Neutral,
        |ui: &mut Ui, tx: &mut Tx| {
            tx.instant();
            ui.set_pane(Pane::Settings);
        },
    )
}

/// What fixes `summary`, when the app can take the user there.
pub(super) fn fix_actions(p: Paint, summary: &ErrorSummary) -> Vec<El> {
    match summary.kind {
        ErrorKind::Key | ErrorKind::Provider => vec![settings_button(p, "Settings")],
        ErrorKind::Other => Vec::new(),
    }
}

/// Text the daemon transcribed but could not save or paste, so it is not
/// lost with the failed step.
fn recovery(p: Paint, status: &Status) -> Option<El> {
    if status.recovered_text.is_empty() {
        return None;
    }
    let recovered = status.recovered_text.clone();
    let copy = controls::small_button(
        p,
        "copy-recovered",
        "Copy text",
        ButtonVariant::Neutral,
        move |ui: &mut Ui, tx: &mut Tx| {
            tx.copy(recovered.clone());
            ui.notice = "Copied".to_string();
        },
    );
    let message = if status.error.is_empty() {
        format!("Recovered text: {}", status.recovered_text)
    } else {
        format!("{} Recovered text: {}", status.error, status.recovered_text)
    };
    Some(controls::banner(
        p,
        "recovered",
        &message,
        Tone::Warn,
        Some(copy),
    ))
}

/// The meter across the card, the timer and the action that fits the
/// daemon's state, and why the last action or recording failed.
fn stage(ui: &Ui, p: Paint) -> El {
    let state = ui.state();
    let recording = state == Some(DaemonState::Recording);
    let timer = text(logic::format_duration(ui.elapsed_ms()))
        .key("timer")
        .font(p.numeric(TIMER))
        .color(if recording { p.strong() } else { p.faint() });

    let mut actions = Vec::new();
    match state {
        Some(DaemonState::Recording) => {
            actions.push(controls::button(
                p,
                "dictation-cancel",
                "Cancel",
                ButtonVariant::Ghost,
                |ui: &mut Ui, _: &mut Tx| ui.act(Request::Cancel),
            ));
            actions.push(controls::button(
                p,
                "dictation-toggle",
                "Stop and transcribe",
                ButtonVariant::Contrast,
                |ui: &mut Ui, _: &mut Tx| ui.act(Request::Toggle),
            ));
        }
        Some(DaemonState::Transcribing) => actions.push(controls::button(
            p,
            "dictation-cancel",
            "Cancel",
            ButtonVariant::Neutral,
            |ui: &mut Ui, _: &mut Tx| ui.act(Request::Cancel),
        )),
        Some(DaemonState::Idle | DaemonState::Error) => actions.push(controls::button(
            p,
            "dictation-toggle",
            "Start dictation",
            ButtonVariant::Contrast,
            |ui: &mut Ui, _: &mut Tx| ui.act(Request::Toggle),
        )),
        Some(DaemonState::Typing) | None => {}
    }

    let failure = stage_failure(ui).map(|summary| {
        let actions = fix_actions(p, &summary);
        controls::error_callout(p, "error", &summary, actions)
    });

    controls::raised(p, "stage").child(
        col()
            .px(ROW_PX)
            .pt(20.0)
            .pb(16.0)
            .gap(16.0)
            .child(charts::level_meter(p, state, &ui.level_history))
            .child(
                row()
                    .key("controls")
                    .gap(16.0)
                    .center_y()
                    .child(
                        col()
                            .grow(1.0)
                            .shrink()
                            .min_w(0.0)
                            .gap(2.0)
                            .child(timer)
                            .child(source_line(ui, p)),
                    )
                    .child(
                        row()
                            .key("actions")
                            .min_h(32.0)
                            .gap(8.0)
                            .center_y()
                            .children(actions),
                    ),
            )
            .children(failure),
    )
}

/// A refused action, or the daemon's error while it is in its error state.
/// A failed transcription is left to the Last result card below, so the
/// same failure does not show twice.
fn stage_failure(ui: &Ui) -> Option<ErrorSummary> {
    if let Some(error) = &ui.action_error {
        return Some(logic::describe_error(error));
    }
    let status = ui.status.as_ref()?;
    if status.state != DaemonState::Error || status.error.is_empty() {
        return None;
    }
    let shown_below = ui.recent.first().is_some_and(|last| {
        last.failed && !last.error.is_empty() && status.error.ends_with(&last.error)
    });
    (!shown_below).then(|| logic::describe_error(&status.error))
}

/// Which provider the next recording goes to.
fn source_line(ui: &Ui, p: Paint) -> El {
    let line = match ui.active_provider() {
        Some(provider) => format!("{} · {}", provider.name, provider.model),
        None => {
            let settings = &ui.backend.managed_settings;
            format!("{} · {}", settings.provider, settings.model)
        }
    };
    text(line)
        .key("source")
        .font(p.k.ty.small)
        .color(p.faint())
        .truncate()
}

fn last_result(p: Paint, last: &Recording) -> El {
    let id = last.id;
    let body = if last.failed {
        let summary = logic::describe_error(&last.error);
        let mut actions = Vec::new();
        if !last.audio_path.as_os_str().is_empty() {
            actions.push(controls::small_button(
                p,
                "retry-last",
                "Retry",
                ButtonVariant::Neutral,
                move |ui: &mut Ui, _: &mut Tx| ui.act(Request::Retry(id)),
            ));
        }
        actions.extend(fix_actions(p, &summary));
        controls::error_callout(p, "last-result-error", &summary, actions)
    } else if last.text.trim().is_empty() {
        text("No speech was transcribed")
            .key("last-result-text")
            .font(p.k.ty.body.lh(22.0))
            .color(p.faint())
    } else {
        text(last.text.clone())
            .key("last-result-text")
            .font(p.k.ty.body.lh(22.0))
            .color(p.text())
            .selectable(true)
            .selection_color(p.k.selection())
            .wrap()
            .max_lines(6)
    };
    let copied = last.text.clone();
    let mut actions = Vec::new();
    if !last.failed && !last.text.trim().is_empty() {
        actions.push(controls::icon_button(
            p,
            "copy-last-result",
            &icons::COPY,
            "Copy",
            move |ui: &mut Ui, tx: &mut Tx| {
                tx.copy(copied.clone());
                ui.notice = "Copied".to_string();
            },
        ));
    }
    let meta = format!(
        "{} · {}",
        logic::format_time(last.timestamp),
        logic::format_duration(last.duration_ms)
    );
    let accessory = controls::small_button(
        p,
        "open-last",
        "Open in history",
        ButtonVariant::Ghost,
        move |ui: &mut Ui, tx: &mut Tx| {
            tx.instant();
            ui.reveal_recording(id);
        },
    );
    controls::section(
        p,
        "last-result",
        "Last result",
        Some(accessory),
        col().px(ROW_PX).py(14.0).gap(10.0).child(body).child(
            row()
                .gap(8.0)
                .center_y()
                .child(
                    text(meta)
                        .font(p.numeric(p.k.ty.small))
                        .color(p.faint())
                        .grow(1.0),
                )
                .children(actions),
        ),
    )
}

fn recent(ui: &Ui, p: Paint) -> El {
    let rows: Vec<El> = ui
        .recent
        .iter()
        .skip(1)
        .map(|recording| recent_row(p, recording))
        .collect();
    let body = if rows.is_empty() {
        controls::empty_state(
            p,
            "recent-empty",
            &icons::HISTORY,
            "No other recordings",
            "Finished dictations appear here",
        )
    } else {
        col().children(controls::separated(p, rows))
    };
    let all = controls::small_button(
        p,
        "open-all-history",
        "All history",
        ButtonVariant::Ghost,
        |ui: &mut Ui, tx: &mut Tx| {
            tx.instant();
            ui.set_pane(Pane::History);
        },
    );
    controls::section(p, "recent", "Recent", Some(all), body)
}

fn recent_row(p: Paint, recording: &Recording) -> El {
    let id = recording.id;
    let snippet = if recording.failed {
        text(logic::describe_error(&recording.error).line()).color(p.danger())
    } else if recording.text.trim().is_empty() {
        text("No speech was transcribed").color(p.faint())
    } else {
        text(logic::single_line(&recording.text)).color(p.text())
    };
    let copied = recording.text.clone();
    row()
        .key(format!("recent-{id}"))
        .role("listitem")
        .min_h(controls::ROW_H)
        .px(ROW_PX)
        .gap(14.0)
        .center_y()
        .hover_group()
        .child(
            text(logic::format_time(recording.timestamp))
                .font(p.numeric(p.k.ty.small))
                .color(p.faint())
                .w(58.0),
        )
        .child(snippet.font(p.k.ty.ui).truncate().grow(1.0).shrink())
        .child(
            text(logic::format_duration(recording.duration_ms))
                .font(p.numeric(p.k.ty.small))
                .color(p.faint()),
        )
        .child(controls::icon_button(
            p,
            "recent-copy",
            &icons::COPY,
            "Copy",
            move |ui: &mut Ui, tx: &mut Tx| {
                tx.copy(copied.clone());
                ui.notice = format!("Copied #{id}");
            },
        ))
        .on_press_tx(move |ui: &mut Ui, tx: &mut Tx| {
            tx.instant();
            ui.reveal_recording(id);
        })
}
