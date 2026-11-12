# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

Build and run the project (Rust, `cargo`; run everything through `nix develop -c`, which provides the toolchain, `pactl`/`parec` and the GUI libraries):
```bash
make build         # release build, copied to build/dictator
make run           # build and run daemon
make install       # cargo install --path . --locked
make test          # cargo test (unit + integration tests)
make check         # cargo fmt --check + cargo clippy -D warnings
make check-gui     # same with --features gui (needed when touching src/gui/)
make clean         # remove build artifacts
make deps          # cargo update
```

## Architecture

Dictator is a voice typing daemon for Linux using a client-server architecture:

- **Binary modes**: Single binary operates as both daemon and CLI client
- **IPC**: Unix socket communication at `$XDG_RUNTIME_DIR/dictator/dictator.sock`, with a private `/tmp/dictator-$UID/dictator.sock` fallback
- **State machine**: Daemon manages states: idle → recording → transcribing → typing → idle
- **Configuration**: JSON config at `~/.config/dictator/config.json`; `api.active_provider` is the default provider, overridden at runtime by `runtime.json` next to it

### Core Components

- **Daemon** (`src/daemon.rs`): Background service managing state transitions and orchestrating audio/transcription/typing
- **IPC** (`src/ipc/`): Unix socket protocol for client-daemon communication
- **Audio** (`src/audio/`): `parec` capture, microphone selection via `pactl`, and Whisper API transcription
- **Typing** (`src/typing.rs`): Clipboard + paste simulation (X11: xclip/xdotool, Wayland: wl-copy/wtype)
- **Notifier** (`src/notifier.rs`): D-Bus desktop notifications for state changes
- **Visual** (`src/visual/`): OSD event stream (newline-delimited JSON on a private unix socket). `state` events carry `recording_duration_ms` (recording, transcribing), `message` (error, sanitized by `public_osd_error_message`), `provider` (recording: the provider that recording will use) and `text` (typing: the text being pasted); empty fields are omitted, so clients must treat all of them as optional. The daemon builds every state event, and the snapshot a new client gets, from its `State` (`osd_state_event`): the provider and the typed text live on the current `Operation`.
- **Storage** (`src/storage.rs`): SQLite transcript history
- **Desktop app** (`src/gui/`, `gui` feature, `dictator-gui` binary): a Rune app (`rune`, `rune-kit`, `rune-theme-opencode`, pinned by git rev; bump the rev together with `cargoLock.outputHashes` in `flake.nix`). `mod.rs` holds the state, `poll` and the window frame; `dictation.rs`, `history.rs`, `stats.rs` and `settings.rs` are the panes; `controls.rs` and `charts.rs` the shared pieces; `logic.rs` the view-independent logic and its tests; `backend.rs` the worker thread that talks to the database and daemon. Views read kit tokens through `Paint`, never colour literals. Errors go through `logic::describe_error` (title, provider message, raw reply) and `controls::error_callout`. The app's mark is `icons::BRAND`, whose body `assets/dictator.svg` must contain (a test checks); the desktop entry and icon install under the app id `org.dictator.Dictator`. Don't nest scroll areas: Rune routes the wheel to the innermost one under the pointer and never chains to the parent. Check a visual change headlessly: `dictator-gui --demo --snap out.png --scene <name> [--theme light]` (`SCENES` in `mod.rs`).
- **OSD** (`src/gui/osd/`, `gui` feature, `dictator-osd` binary): the dictation island, a click-through Rune overlay that reads the OSD socket (`--socket`, `--placement floating|bottom|top`, default floating: a capsule above the bottom edge that opens from a dot; `--theme dark|light` overrides the system appearance, which Rune reports through `cx.system_dark()` and rereads on each show). Home Manager runs it as `services.dictator.osd`. `placement.rs` holds each placement as data (edge, gap, corners, fillets, seed size); motion and view are shared. `model.rs` is the state machine, level mapping and level history, `scope.rs` the dot-matrix visualizer (scrolls while recording; freezes, squeezes and sweeps while transcribing), `motion.rs` the springs, content layers and the scope's slide between layouts, `view.rs` the drawing, `feed.rs` the reconnecting socket reader. It shows the recording's `provider` and the typing state's `text` when the daemon sends them. Snapshot it headlessly like the app: `dictator-osd --snap out.png --scene <name> [--placement bottom|top] [--theme light]` (`SCENES` in `osd/mod.rs`).
- **Provider** (`src/provider.rs`): `ProviderSelector` holds the daemon's active provider. It starts from `$XDG_CONFIG_HOME/dictator/runtime.json` (`{"version": 1, "active_provider": "openai"}`), falling back to the config default with a warning when that names an unknown or unusable provider. The `providers` and `use_provider` IPC actions list and switch providers (`dictator provider [list|use <name>]`); the list travels as one JSON string in `Response.data["providers"]` and never carries keys. A provider is usable with a non-empty endpoint and a key with no unresolved `${env:...}`. Selecting the default deletes the override. The daemon snapshots the provider when a recording or retry starts (`Operation.provider`), so a switch applies from the next recording, and passes it in `TranscriptionRequest.provider`; the CLI `retry` reads the override itself (`effective_provider`). `check_provider` (`dictator provider check [name]`) sends a provider an authenticated request with no audio (`WhisperClient::check`): 400/422 means the key works, 401/403 that it was refused; the result is a `ProviderCheck` in `Response.data["provider_check"]`. The desktop app runs it on start, on a switch and from Settings, and falls back to an unauthenticated probe (`CheckOutcome::Reachable`) when the daemon is down or too old.
- **Glossary** (`src/glossary.rs`): vocabulary terms in `$XDG_DATA_HOME/dictator/glossary.json` (`{"version": 1, "terms": [...]}`), edited by the GUI and `dictator glossary` (`add`, `remove`, `rename`, `list`). `WhisperClient` sends them as the `vocabulary` form field when the active provider sets `send_vocabulary`, rereading the file when its inode, size or mtime changes. A missing file sends no field (server default); an empty list sends `[]` (boosting off). Invalid entries are dropped against siren's limits before sending.

### State Flow

The daemon implements a linear state machine:
1. **Idle**: Waiting for commands
2. **Recording**: Audio capture active
3. **Transcribing**: Sending audio to Whisper API. A recording the gate discards (under 300 ms, or quieter than -50 dBFS) goes straight back to Idle with no upload and no history entry.
4. **Typing**: Pasting text via clipboard
5. **Error**: Temporary error state (auto-returns to idle)

### Audio capture

- `audio/warm.rs`: a supervisor thread keeps one warm `parec` stream open on the selected microphone. It closes the stream for `dictator mic release` and logind suspend, reopens it with backoff, and swaps it when `pactl subscribe` reports source changes. Discovery and resolution run here. On each re-resolution (for the warm stream and the active recording's stream) and in the start path before the warm buffer is used, the stream's source-output is looked up by `parec` PID; a stream the server moved to another source (PulseAudio does this on unplug) is ended with an error and never used.
- `audio/capture.rs`: one `parec` process feeding a `Timeline`. Streams are shared through `Arc`, so a recording keeps a released or swapped stream alive until it stops.
- `audio/timeline.rs`: the ring buffer, sample-index mapping, pre-roll/tail slicing and gate math. Each chunk re-anchors sample indices to `Instant`, and IPC passes each command's arrival `Instant` through `CommandHandler`, so pre-roll and tail are cut at the keypress rather than when the handler ran.
- `audio/devices.rs`: priority list (`preferences.json`) and selection. Bluetooth sources are never selected. Ids: `bluetooth:<address>`; USB `usb:<vendor>:<product>[:<serial>]` (port-independent; a second identical device connected at once gets `@<bus_path>`); otherwise `bus:<bus_path>`. `reconcile` first migrates old `bus:...-usb-...` entries (a connected device adopts the entry at its old bus path, else the highest-ranked one with its name; other old entries stay for the user to remove) and is idempotent, so `Resolution::fingerprint` only changes on a real write. `remove_microphone` refuses connected devices.
- Tests fake `parec`/`pactl` with `/bin/sh` scripts through `CaptureBackend`; pure timing logic is tested on `Timeline` with synthetic instants.

### Key Dependencies

- **clap**: CLI framework (with `clap_complete` for shell completions)
- **tokio**: async runtime for IPC, OSD socket, HTTP and subprocesses
- **parec/pactl** (PulseAudio tools, run against PipeWire's pulse server): audio capture and device discovery
- **reqwest** (rustls): Whisper API transcription
- **zbus**: D-Bus notifications and logind `PrepareForSleep`
- **rusqlite** (bundled): transcript storage
- **X11**: xclip (clipboard) + xdotool (paste keystroke)
- **Wayland**: wl-clipboard (clipboard) + wtype (paste keystroke)

### Project Structure

- `src/main.rs`: CLI command definitions and entry point
- `src/lib.rs`: library root exposing the modules below
- `src/daemon.rs`: Core daemon logic and state management
- `src/ipc/protocol.rs`: IPC message definitions and constants
- `src/utils/config.rs`: Configuration management
- `tests/`: integration tests (IPC round trip, OSD socket)
- `dictator.service`: systemd service template