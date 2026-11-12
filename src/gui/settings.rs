//! The Settings pane. What can change while the daemon runs is editable
//! here: the transcription provider, the microphone order and the glossary.
//! The rest comes from `config.json` and is shown read-only.

use rune::{El, Key, KeyEvent, Tx, col, el, icon, row, text};
use rune_kit::{ButtonVariant, ControlSize, TextInput};

use crate::glossary::MAX_TERMS;

use super::backend::{Microphone, ProviderInfo, Request};
use super::controls::{self, GUTTER, Paint, ROW_PX, SECTION_GAP, Tone};
use super::{Ui, icons, logic};

/// Lists taller than this scroll inside their card instead of stretching
/// the page.
const MICROPHONES_MAX_H: f32 = 204.0;
const GLOSSARY_MAX_H: f32 = 176.0;
const CHIP_H: f32 = 28.0;

pub(super) fn pane(ui: &Ui, p: Paint, _layer: &mut Vec<El>) -> (Vec<El>, El) {
    let page = col()
        .key("settings")
        .px(GUTTER - ROW_PX)
        .pt(4.0)
        .pb(GUTTER)
        .gap(SECTION_GAP)
        .child(providers(ui, p))
        .child(microphones(ui, p))
        .child(glossary(ui, p))
        .child(daemon(ui, p));
    (
        Vec::new(),
        controls::scroll_fill(p, "settings-scroll", page),
    )
}

fn providers(ui: &Ui, p: Paint) -> El {
    let mut ordered: Vec<&ProviderInfo> = ui.providers.iter().collect();
    ordered.sort_by_key(|provider| (!provider.is_default, !provider.usable));
    let mut rows: Vec<El> = ordered
        .into_iter()
        .map(|provider| provider_row(ui, p, provider))
        .collect();
    if rows.is_empty() {
        let settings = &ui.backend.managed_settings;
        rows.push(controls::setting_row(
            p,
            "provider-default",
            &settings.provider,
            Some(&settings.model),
            controls::tone_tag(p, "Config default", Tone::Neutral),
        ));
    }
    rows.push(connection_row(ui, p));
    let mut body = col().children(controls::separated(p, rows));
    if !ui.provider_error.is_empty() {
        body = body.child(
            controls::note(
                p,
                "provider-error",
                &format!(
                    "Providers cannot be switched right now: {}",
                    ui.provider_error
                ),
                Tone::Warn,
            )
            .px(ROW_PX)
            .pb(12.0),
        );
    }
    controls::section(p, "providers", "Transcription provider", None, body)
}

fn provider_row(ui: &Ui, p: Paint, provider: &ProviderInfo) -> El {
    let name = provider.name.clone();
    let pending = ui.provider_pending.as_deref() == Some(provider.name.as_str());
    let status = if provider.active {
        controls::tone_tag(p, "In use", Tone::Good)
    } else if pending {
        controls::tone_tag(p, "Switching…", Tone::Accent)
    } else if !provider.usable {
        controls::tone_tag(p, "Unavailable", Tone::Warn)
    } else {
        el()
    };
    let detail = if provider.usable {
        format!("{} · {}", provider.model, host(&provider.endpoint))
    } else {
        provider.problem.clone()
    };
    let title = row()
        .gap(8.0)
        .center_y()
        .child(
            text(provider.name.clone())
                .font(p.k.ty.ui_medium)
                .color(if provider.usable { p.text() } else { p.faint() }),
        )
        .children(
            provider
                .is_default
                .then(|| text("default").font(p.k.ty.small).color(p.faint())),
        );
    let item = row()
        .key(format!("provider-{}", provider.name))
        .role("radio")
        .label(provider.name.clone())
        .description(detail.clone())
        .checked(provider.active)
        .disabled(!provider.usable)
        .focusable(provider.usable)
        .min_h(58.0)
        .px(ROW_PX)
        .py(8.0)
        .gap(12.0)
        .center_y()
        .child(radio_mark(p, provider.active, provider.usable))
        .child(
            col().grow(1.0).shrink().gap(2.0).child(title).child(
                text(detail)
                    .font(p.k.ty.small.lh(16.0))
                    .color(if provider.usable {
                        p.muted()
                    } else {
                        p.warning()
                    })
                    .truncate()
                    .a11y_hidden(),
            ),
        )
        .child(status);
    if !provider.usable || provider.active {
        return item;
    }
    item.hover(p.k.c.hover).on_press(move |ui: &mut Ui| {
        ui.provider_pending = Some(name.clone());
        ui.backend.send(Request::SetProvider(name.clone()));
    })
}

fn radio_mark(p: Paint, checked: bool, enabled: bool) -> El {
    let ring = if checked {
        p.strong()
    } else if enabled {
        p.k.c.border_strong
    } else {
        p.k.c.border_muted
    };
    row()
        .size(16.0, 16.0)
        .radius(8.0)
        .border(1.5, ring)
        .center()
        .a11y_hidden()
        .children(checked.then(|| el().size(8.0, 8.0).radius(4.0).fill(p.strong())))
}

fn connection_row(ui: &Ui, p: Paint) -> El {
    let (line, tone) = match &ui.connection {
        _ if ui.connection_pending => ("Checking…".to_string(), Tone::Neutral),
        Some(report) => match &report.provider {
            Ok(detail) => (
                format!("{} answered · {detail}", report.endpoint_host),
                Tone::Good,
            ),
            Err(error) => (format!("{} · {error}", report.endpoint_host), Tone::Bad),
        },
        None => ("Not checked yet".to_string(), Tone::Neutral),
    };
    controls::setting_row(
        p,
        "connection",
        "Connection",
        None,
        row()
            .gap(12.0)
            .center_y()
            .child(controls::tone_tag(p, &line, tone))
            .child(controls::small_button(
                p,
                "check-connection",
                "Check",
                ButtonVariant::Neutral,
                |ui: &mut Ui, _: &mut Tx| ui.check_connection(),
            )),
    )
}

fn host(endpoint: &str) -> String {
    endpoint
        .split_once("://")
        .map_or(endpoint, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn microphones(ui: &Ui, p: Paint) -> El {
    let in_use = ui
        .microphones
        .iter()
        .position(|microphone| microphone.connected && !microphone.bluetooth);
    let rows: Vec<El> = ui
        .microphones
        .iter()
        .enumerate()
        .map(|(index, microphone)| microphone_row(ui, p, index, microphone, in_use == Some(index)))
        .collect();
    let mut body = if rows.is_empty() {
        col().child(controls::empty_state(
            p,
            "microphones-empty",
            &icons::MIC,
            "No microphones found",
            "",
        ))
    } else {
        col().child(controls::scroll_capped(
            p,
            "microphones-scroll",
            MICROPHONES_MAX_H,
            col().children(controls::separated(p, rows)),
        ))
    };
    if !ui.microphone_error.is_empty() {
        body = body.child(
            controls::note(p, "microphone-error", &ui.microphone_error, Tone::Bad)
                .px(ROW_PX)
                .py(10.0),
        );
    }
    let hint = text("The first connected one is used")
        .font(p.k.ty.small)
        .color(p.faint());
    controls::section(p, "microphones", "Microphones", Some(hint), body)
}

fn microphone_row(ui: &Ui, p: Paint, index: usize, microphone: &Microphone, in_use: bool) -> El {
    let status = if in_use {
        controls::tone_tag(p, "In use", Tone::Good)
    } else {
        let tone = if microphone.connected && !microphone.bluetooth {
            Tone::Accent
        } else {
            Tone::Neutral
        };
        controls::tone_tag(p, logic::microphone_status_label(microphone), tone)
    };
    let step = |key: &str, glyph, label: &str, direction: isize| match logic::moved_ids(
        &ui.microphones,
        index,
        direction,
    ) {
        Some(ids) => controls::icon_button(p, key, glyph, label, move |ui: &mut Ui, _: &mut Tx| {
            ui.backend.send(Request::ReorderMicrophones(ids.clone()))
        }),
        None => controls::disabled_icon_button(p, key, glyph, label),
    };
    let id = microphone.id.clone();
    let remove = row()
        .w(28.0)
        .center()
        .children((!microphone.connected).then(|| {
            controls::icon_button(
                p,
                "remove",
                &icons::TRASH,
                "Forget this microphone",
                move |ui: &mut Ui, _: &mut Tx| {
                    ui.backend.send(Request::RemoveMicrophone(id.clone()))
                },
            )
        }));
    let ink = if microphone.connected && !microphone.bluetooth {
        p.text()
    } else {
        p.muted()
    };
    row()
        .key(format!("microphone-{}", microphone.id))
        .role("listitem")
        .min_h(controls::ROW_H)
        .px(ROW_PX)
        .gap(10.0)
        .center_y()
        .child(
            text((index + 1).to_string())
                .font(p.numeric(p.k.ty.small))
                .color(p.faint())
                .w(16.0),
        )
        .children(
            microphone
                .bluetooth
                .then(|| icon(&icons::BLUETOOTH, 14.0).color(p.faint())),
        )
        .child(
            text(microphone.name.clone())
                .font(p.k.ty.ui)
                .color(ink)
                .truncate()
                .grow(1.0)
                .shrink(),
        )
        .child(status)
        .child(
            row()
                .gap(2.0)
                .center_y()
                .child(step("up", &icons::CHEVRON_UP, "Move up", -1))
                .child(step("down", &icons::CHEVRON_DOWN, "Move down", 1))
                .child(remove),
        )
}

fn glossary(ui: &Ui, p: Paint) -> El {
    let terms = ui
        .glossary
        .as_ref()
        .map(|glossary| glossary.terms.as_slice())
        .unwrap_or_default();
    let send_vocabulary = ui
        .active_provider()
        .map_or(ui.backend.managed_settings.send_vocabulary, |provider| {
            provider.send_vocabulary
        });
    let provider = ui
        .active_provider()
        .map_or(ui.backend.managed_settings.provider.as_str(), |provider| {
            provider.name.as_str()
        });

    let input = TextInput::new(&ui.glossary_input)
        .placeholder("Add a term, such as a name or a product")
        .label("New glossary term")
        .width(None)
        .invalid(!ui.glossary_error.is_empty() && ui.term_edit.is_none())
        .on_edit(|ui: &mut Ui| &mut ui.glossary_input)
        .on_change(|ui: &mut Ui, _: &mut Tx| {
            ui.glossary_error = logic::typing_glossary_error(&ui.glossary_input.text);
        })
        .on_submit(|ui: &mut Ui, _: &mut Tx| ui.add_glossary_term())
        .build(p.k)
        .key("glossary-input")
        .grow(1.0)
        .shrink();
    let add = rune_kit::Button::new("Add")
        .variant(ButtonVariant::Neutral)
        .disabled(ui.glossary_input.text.trim().is_empty())
        .on_press(|ui: &mut Ui| ui.add_glossary_term())
        .build(p.k)
        .key("glossary-add");

    let mut body = col()
        .px(ROW_PX)
        .py(14.0)
        .gap(12.0)
        .child(row().gap(8.0).center_y().child(input).child(add));
    if !ui.glossary_error.is_empty() {
        body = body.child(controls::note(
            p,
            "glossary-error",
            &ui.glossary_error,
            Tone::Bad,
        ));
    }
    if !terms.is_empty() {
        let chips = row()
            .key("terms")
            .role("list")
            .flex_wrap()
            .gaps(6.0, 6.0)
            .children(terms.iter().map(|term| chip(ui, p, term)));
        body = body.child(controls::scroll_capped(
            p,
            "terms-scroll",
            GLOSSARY_MAX_H,
            chips,
        ));
    }
    for (index, message) in logic::glossary_notes(ui.glossary.as_ref(), send_vocabulary, provider)
        .iter()
        .enumerate()
    {
        body = body.child(controls::note(
            p,
            &format!("glossary-note-{index}"),
            message,
            Tone::Neutral,
        ));
    }
    let count = text(format!("{} of {MAX_TERMS}", terms.len()))
        .font(p.numeric(p.k.ty.small))
        .color(p.faint());
    controls::section(p, "glossary", "Glossary", Some(count), body)
}

/// One term. Pressing it edits it in place; the cross removes it.
fn chip(ui: &Ui, p: Paint, term: &str) -> El {
    if ui.term_edit.as_deref() == Some(term) {
        return TextInput::new(&ui.term_field)
            .label("Edit term")
            .height(CHIP_H)
            .width(Some(190.0))
            .on_edit(|ui: &mut Ui| &mut ui.term_field)
            .on_submit(|ui: &mut Ui, _: &mut Tx| ui.commit_term_edit())
            .on_key(|ui: &mut Ui, event: &KeyEvent, _: &mut Tx| {
                if event.key == Key::Escape {
                    ui.term_edit = None;
                    ui.glossary_error.clear();
                    return true;
                }
                false
            })
            .build(p.k)
            .key(format!("term-edit-{term}"));
    }
    let (edited, removed) = (term.to_string(), term.to_string());
    row()
        .key(format!("term-{term}"))
        .role("listitem")
        .h(CHIP_H)
        .pl(10.0)
        .pr(2.0)
        .gap(2.0)
        .center_y()
        .radius(CHIP_H / 2.0)
        .fill(p.k.c.layer_02)
        .border(p.hair, p.k.c.border_muted)
        .child(
            text(term)
                .key("label")
                .font(p.k.ty.small_medium)
                .color(p.text())
                .cursor(rune::Cursor::Text)
                .help("Edit")
                .on_press(move |ui: &mut Ui| ui.begin_term_edit(&edited)),
        )
        .child(
            rune_kit::IconButton::new(&icons::CLOSE)
                .variant(rune_kit::IconButtonVariant::Ghost)
                .control_size(ControlSize::Mini)
                .label(format!("Remove {term}"))
                .on_press(move |ui: &mut Ui| {
                    ui.term_edit = None;
                    ui.backend
                        .send(Request::RemoveGlossaryTerm(removed.clone()));
                })
                .build(p.k)
                .key("remove"),
        )
}

fn daemon(ui: &Ui, p: Paint) -> El {
    let settings = &ui.backend.managed_settings;
    let shortcut = match settings.paste_shortcut.as_str() {
        "ctrl_v" => "Ctrl+V",
        "ctrl_shift_v" => "Ctrl+Shift+V",
        other => other,
    };
    let notifications = match settings.notifications.as_str() {
        "all" => "All",
        "errors_only" => "Errors only",
        "off" => "Off",
        other => other,
    };
    let uptime = ui
        .status
        .as_ref()
        .map(|status| logic::format_uptime(status.uptime_seconds))
        .unwrap_or_else(|| "Not running".to_string());
    let rows = vec![
        controls::setting_row(
            p,
            "max-duration",
            "Longest recording",
            None,
            controls::value_text(p, &format!("{} min", settings.max_duration_minutes)),
        ),
        controls::setting_row(
            p,
            "timeout",
            "Request timeout",
            None,
            controls::value_text(p, &format!("{} s", settings.timeout_seconds)),
        ),
        controls::setting_row(
            p,
            "paste",
            "Paste shortcut",
            None,
            controls::value_text(p, shortcut),
        ),
        controls::setting_row(
            p,
            "notifications",
            "Notifications",
            None,
            controls::value_text(p, notifications),
        ),
        controls::setting_row(
            p,
            "uptime",
            "Daemon uptime",
            None,
            controls::value_text(p, &uptime),
        ),
    ];
    let source = text(if settings.managed_by_home_manager {
        "Set in your Nix configuration"
    } else {
        "Set in config.json"
    })
    .font(p.k.ty.small)
    .color(p.faint());
    controls::section(
        p,
        "daemon",
        "Daemon",
        Some(source),
        col().children(controls::separated(p, rows)),
    )
}
