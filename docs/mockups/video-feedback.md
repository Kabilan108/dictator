# Video feedback, September 18

Historical prototype review. Native implementation and validation are documented
in `../gui-validation.md`; implementation-status statements below refer to the
prototype stage.

## Follow-up decisions

The next iteration supersedes the retyping and separate-indicator designs below.
Cursor is now the default logo. Removed re-type, tray status, and the separate
recording popup. Main activity is shown only while recording/transcribing, left
of search. Terminal outcomes reveal and select the recording, clearing search
and filters when necessary.

Added a Microphone section within Settings with reordered stable device identities, disconnected
entries retained in place, automatic/system/fixed selection, and next-recording
resolution. Modeled after Sotto commit
`93bc0b53321deda615b4c5709a01cecd0aad696a`,
`Sources/SottoCore/MicrophonePreferences.swift`. Preview changes are in memory.

Home Manager currently requires settings or configFile and owns config.json via
xdg.configFile. Settings are read-only in this iteration. Proposed convention:
Home Manager keeps daemon configuration; a separate app-owned preferences.json
holds explicitly mutable UI/device preferences, without duplicate fields. This
requires a new daemon preference-loading/reload design; no Nix change was made.

Current Rust retry inserts a new transcript with a new ID/current timestamp and
deletes the failed row. Proposed GUI retry preserves capture identity/time and
appends attempts. Both success and failure are simulated with attempt records.
That behavior requires a unified recording identity across failure and success;
it is not implemented in the Rust database.

Stats now includes p50/p95/p99/max for 60 recent successful requests and a small
latency graph. Request latency includes upload/network/queue/processing and
excludes capture/paste. Samples remain illustrative.

## Original video pass

Reviewed the full 4:07 recording using Siren transcription and ffmpeg frames.
Siren job: `job_a15c258d75114194b163db656f62c64e`.
Frames extracted at 15, 27, 58, 84, 101, 119, 140, 154, 184, 207, 219, and 239 seconds.
The transcript and frame files are temporary review material under `/tmp/dictator-feedback*`.

| Video | Request | Prototype change |
| --- | --- | --- |
| 0:09–0:44 | Remove non-button pills everywhere | Plain status, IDs, model names, and metadata; no badge borders or fills |
| 0:55–1:29 | Smaller recording control, waveform above it, no shortcut chips | 28px record button below waveform; Alt+Space and Escape handled while the preview is focused |
| 1:32–1:48 | No streak; words today, WPM, latency or duration | Words today, WPM, recorded duration, server p95; synthetic latency values |
| 1:48–2:09 | Five recent records; retain timer and open history | Five copyable recent entries, recording timer, daemon uptime, functioning history link |
| 2:14–2:23 | Show Settings and Stats with values | Populated tabs, editable in-memory example settings, derived synthetic statistics |
| 2:24–2:40 | Simulate hundreds or thousands of records | 2,400 deterministic synthetic records; search, filters, and 50-row pagination |
| 2:45–3:18 | Keep errors, edits, transcript and word count; try borderless player | Preserved those elements; removed player border/fill; simulated play/pause and seek |
| 3:19–3:31 | Copy metadata JSON | Original/current text, metadata and revisions; selectable fallback when clipboard access is denied |
| 3:35–3:55 | Explain revision storage and diff implementation | Read-only revision inspection, append-on-restore behavior; proposal below |
| 3:55–4:01 | Show retyping interface and states | Destination scenario, countdown, cancel, success, unavailable destination, changed focus and paste failure |

## Proposed native revision storage

The current SQLite schema in `src/storage.rs` has only one text field per
transcript and a separate failed-transcription table. No revision migration has
been applied in this prototype pass.

Keep `transcripts.text` as the latest saved text so the CLI and history previews
remain compatible. Add `current_revision` and a `transcript_revisions` table
containing `transcript_id`, `revision_no`, `text`, `created_at`, and `source`.
Revision 0 is the immutable model output. Backfill existing text into revision 0.

Use a versioned migration under `BEGIN IMMEDIATE`, reread the schema version
inside the transaction, and enable foreign keys on every connection. New
transcripts and their original revision must be created atomically. Saving an
edit appends a full text snapshot and updates current text/version in one
transaction, checking the expected prior version to prevent lost updates.
Restoring an earlier version appends another revision with source `restore`.

Compute word/punctuation diffs between selected snapshots when displayed;
do not persist HTML or diff operations. Preserve whitespace, debounce editor
updates, and run expensive comparisons off the UI thread with bounded work.
The HTML prototype currently uses a word-level LCS comparison for its short
sample texts, with a size/work cutoff that falls back to showing full versions.
That algorithm is not a performance claim for the native implementation.

For native history, add a compound `(timestamp DESC, id DESC)` index and keyset
pagination. Fetch roughly 50 summaries at a time and load full text/revisions
only for the selected recording. The HTML holds synthetic data in memory and
limits rendered rows; it does not benchmark SQLite or GPUI.

## Proposed native retyping

The existing implementation in `src/typing.rs` copies text and sends a paste
shortcut to the focused app. Niri app identity currently chooses the shortcut;
it does not verify a stable destination window ID.

Retyping should submit an exact saved transcript ID and revision number. The
daemon loads the text, acquires a destination window ID, and verifies that same
window immediately before pasting. Reject Dictator itself and abort on changed
focus. Offer Copy when the compositor cannot identify a destination reliably.
Serialize retyping with the daemon's other operations and suppress duplicate
requests. An IPC timeout means outcome unknown, so never automatically retry.

The prototype simulates the destination and insertion outcomes. It does not
change the clipboard except through explicit Copy actions, target other apps,
record audio, call providers, or change the real configuration/database.


## Microphone simplification

Removed the selection-mode dropdown and next-recording row/labels. The ordered
list is the single selection policy. Arrow controls are borderless SVG icon
buttons with keyboard-accessible labels.

Native persistence proposal: seed a saved device registry and priority order
once. Reconcile discovery by stable device identity, never the transient runtime
node ID or display name. Update names and availability; append previously unseen
devices at the end. Absence must never delete a saved entry or change its rank.
A reconnect reuses the existing entry. Bluetooth profile changes need identity
normalization during native implementation so one physical input is not treated
as a new device each time. Only an explicit forget action should delete a saved
entry. Persist registry/order in the app-owned preferences file; availability is
live state. The prototype simulates reconciliation in memory, not disk storage.
