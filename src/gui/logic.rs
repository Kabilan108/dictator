//! View-independent logic for the desktop app: reply bookkeeping, draft
//! tracking, the word diff and display formatting.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use chrono::{DateTime, Local, Utc};

use super::LEVEL_HISTORY;
use super::backend::{Detail, Glossary, Microphone};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PendingMutation {
    Save { id: i64, text: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CompletedMutation {
    Save(String),
    Unexpected,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum DiffSegment {
    Same(String),
    Removed(String),
    Added(String),
}

/// Splits on whitespace but keeps whitespace runs as their own tokens, so a
/// changed line break or doubled space shows up as a removed/added `⏎` or `␣`.
pub(super) fn diff_tokens(text: &str) -> Vec<String> {
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

pub(super) fn is_gap(token: &str) -> bool {
    token.chars().all(char::is_whitespace)
}

pub(super) fn visible_gap(token: &str) -> String {
    token
        .chars()
        .map(|ch| match ch {
            '\n' => '⏎',
            '\t' => '⇥',
            _ => '␣',
        })
        .collect()
}

pub(super) fn word_diff(original: &str, current: &str) -> Vec<DiffSegment> {
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

pub(super) fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

pub(super) fn push_level(history: &mut VecDeque<(f32, f32)>, recording: bool, sample: (f32, f32)) {
    if !recording {
        history.clear();
        return;
    }
    if history.len() == LEVEL_HISTORY {
        history.pop_front();
    }
    history.push_back(sample);
}

pub(super) fn status_interval(recording: bool) -> Duration {
    if recording {
        Duration::from_millis(250)
    } else {
        Duration::from_secs(1)
    }
}

pub(super) fn smoothed(levels: &[f32], index: usize) -> f32 {
    let get = |i: isize| -> f32 {
        if i < 0 || i as usize >= levels.len() {
            0.0
        } else {
            levels[i as usize]
        }
    };
    let i = index as isize;
    ((get(i - 1) + 2.0 * get(i) + get(i + 1)) / 4.0).clamp(0.0, 1.0)
}

pub(super) fn format_count(value: i64) -> String {
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

pub(super) fn format_hours(milliseconds: i64) -> String {
    let minutes = milliseconds.max(0) as f64 / 60_000.0;
    if minutes >= 90.0 {
        format!("{:.1} h", minutes / 60.0)
    } else {
        format!("{minutes:.0} min")
    }
}

pub(super) fn microphone_status_label(microphone: &Microphone) -> &'static str {
    match (
        microphone.bluetooth,
        microphone.connected,
        microphone.is_default,
    ) {
        (true, true, _) => "Bluetooth · not used",
        (true, false, _) => "Bluetooth · disconnected · not used",
        (false, true, true) => "Connected · system default",
        (false, true, false) => "Connected",
        (false, false, _) => "Disconnected",
    }
}

pub(super) fn moved_ids(
    microphones: &[Microphone],
    index: usize,
    direction: isize,
) -> Option<Vec<String>> {
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

/// Explains what the daemon will send, since a missing file and an empty
/// list behave differently.
pub(super) fn glossary_notes(
    glossary: Option<&Glossary>,
    send_vocabulary: bool,
    provider: &str,
) -> Vec<String> {
    let mut notes = Vec::new();
    if !send_vocabulary {
        notes.push(format!(
            "Not sent: provider \"{provider}\" does not set send_vocabulary."
        ));
    }
    match glossary {
        Some(glossary) if !glossary.exists => notes.push(
            "No glossary yet, so the server uses its default list. Adding a term creates one."
                .to_string(),
        ),
        Some(glossary) if glossary.terms.is_empty() => {
            notes.push("The glossary is empty, so boosting is off.".to_string())
        }
        _ => {}
    }
    notes
}

/// Takes the oldest pending add, whose reply just arrived, and reports
/// whether the input still holds what was submitted. Clearing it otherwise
/// would erase a term typed while the add was queued.
pub(super) fn submitted_input_unchanged(pending: &mut VecDeque<String>, current: &str) -> bool {
    pending
        .pop_front()
        .is_some_and(|submitted| submitted == current)
}

/// Flags a term while it is typed, but not an empty field.
pub(super) fn typing_glossary_error(value: &str) -> String {
    if value.trim().is_empty() {
        return String::new();
    }
    crate::glossary::normalize_term(value)
        .err()
        .unwrap_or_default()
}

pub(super) fn detail_json(detail: &Detail) -> String {
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

pub(super) fn accepts_reply(request_generation: u64, current_generation: u64) -> bool {
    request_generation == current_generation
}

pub(super) fn should_save(saved_text: &str, editor_text: &str, pending: bool) -> bool {
    !pending && saved_text != editor_text
}

pub(super) fn resolved_editor_text(draft: Option<&str>, server_text: &str) -> String {
    draft.unwrap_or(server_text).to_string()
}

pub(super) fn track_editor_change(
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

pub(super) fn normalize_draft(
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

pub(super) fn accept_search_change(last_value: &mut String, incoming: &str) -> Option<String> {
    if *last_value == incoming {
        return None;
    }
    incoming.clone_into(last_value);
    Some(incoming.to_string())
}

pub(super) fn apply_history_error(loading: &mut bool, notice: &mut String, error: String) {
    *loading = false;
    *notice = error;
}

pub(super) fn can_go_previous(page_index: usize) -> bool {
    page_index > 0
}

pub(super) fn clear_database_notice(notice: &mut String) {
    if notice.starts_with("Database unavailable:") {
        notice.clear();
    }
}

pub(super) fn action_notice(result: Result<(), String>) -> String {
    match result {
        Ok(()) => "Request accepted".to_string(),
        Err(error) => format!("Request failed: {error}"),
    }
}

pub(super) fn choose_history_selection(
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

pub(super) fn pending_mutation_id(pending: &PendingMutation) -> i64 {
    match pending {
        PendingMutation::Save { id, .. } => *id,
    }
}

pub(super) fn complete_mutation(
    pending: Option<PendingMutation>,
    detail_id: i64,
) -> CompletedMutation {
    match pending {
        Some(PendingMutation::Save { id, text }) if id == detail_id => {
            CompletedMutation::Save(text)
        }
        _ => CompletedMutation::Unexpected,
    }
}

pub(super) fn disconnected_notice(error: &str) -> String {
    format!("Daemon disconnected: {error}")
}

pub(super) fn observe_recording_generation(last_seen: &mut Option<u64>, incoming: u64) -> bool {
    let changed = last_seen.is_some_and(|previous| previous != incoming);
    *last_seen = Some(incoming);
    changed
}

pub(super) fn format_time(timestamp: DateTime<Utc>) -> String {
    timestamp.with_timezone(&Local).format("%H:%M").to_string()
}

pub(super) fn format_date(timestamp: DateTime<Utc>) -> String {
    timestamp
        .with_timezone(&Local)
        .format("%A, %B %-d")
        .to_string()
}

pub(super) fn format_duration(milliseconds: i64) -> String {
    let seconds = milliseconds.max(0) as f64 / 1000.0;
    format!("{}:{:04.1}", (seconds / 60.0) as i64, seconds % 60.0)
}

pub(super) fn format_latency(milliseconds: Option<i64>) -> String {
    milliseconds
        .map(|value| format!("{:.2} s", value as f64 / 1000.0))
        .unwrap_or_else(|| "—".to_string())
}

pub(super) fn format_uptime(seconds: u64) -> String {
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
    fn smoothing_averages_neighbours_and_clamps() {
        let levels = [0.0, 1.0, 0.0, 2.0];
        assert!((smoothed(&levels, 1) - 0.5).abs() < f32::EPSILON);
        assert!((smoothed(&levels, 0) - 0.25).abs() < f32::EPSILON);
        assert_eq!(smoothed(&levels, 3), 1.0);
    }

    #[test]
    fn level_history_is_bounded_and_clears_when_idle() {
        let mut history = VecDeque::new();
        for index in 0..(LEVEL_HISTORY + 5) {
            push_level(&mut history, true, (index as f32, 0.0));
        }
        assert_eq!(history.len(), LEVEL_HISTORY);
        assert_eq!(history.front().map(|(rms, _)| *rms as usize), Some(5));
        push_level(&mut history, false, (1.0, 1.0));
        assert!(history.is_empty());
        assert!(status_interval(true) < status_interval(false));
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
    fn rejected_retry_action_has_explicit_feedback() {
        assert_eq!(
            action_notice(Err("recording is no longer failed".to_string())),
            "Request failed: recording is no longer failed"
        );
    }

    #[test]
    fn glossary_notes_separate_missing_empty_and_unsent() {
        let missing = Glossary::default();
        let empty = Glossary {
            exists: true,
            terms: Vec::new(),
        };
        let filled = Glossary {
            exists: true,
            terms: vec!["Claude".to_string()],
        };

        assert!(glossary_notes(Some(&filled), true, "siren").is_empty());
        assert!(glossary_notes(None, true, "siren").is_empty());
        assert_eq!(
            glossary_notes(Some(&missing), true, "siren"),
            ["No glossary yet, so the server uses its default list. Adding a term creates one."]
        );
        assert_eq!(
            glossary_notes(Some(&empty), true, "siren"),
            ["The glossary is empty, so boosting is off."]
        );
        assert_eq!(
            glossary_notes(Some(&filled), false, "openai"),
            ["Not sent: provider \"openai\" does not set send_vocabulary."]
        );
    }

    #[test]
    fn add_reply_keeps_input_typed_after_submission() {
        let mut pending = VecDeque::from(["tmux".to_string(), "Claude".to_string()]);
        assert!(!submitted_input_unchanged(&mut pending, "Claude"));
        assert!(submitted_input_unchanged(&mut pending, "Claude"));
        assert!(pending.is_empty());
        assert!(!submitted_input_unchanged(&mut pending, ""));
    }

    #[test]
    fn typing_feedback_ignores_an_empty_field() {
        assert_eq!(typing_glossary_error("   "), "");
        assert_eq!(typing_glossary_error("tmux"), "");
        assert!(typing_glossary_error(&"x".repeat(65)).contains("at most 64"));
    }
}
