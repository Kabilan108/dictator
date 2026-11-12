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
- **Visual** (`src/visual/`): OSD event stream (newline-delimited JSON on a unix socket)
- **Storage** (`src/storage.rs`): SQLite transcript history
- **Desktop app** (`src/gui/`, `gui` feature, `dictator-gui` binary): a Rune app (`rune`, `rune-kit`, `rune-theme-opencode`, pinned by git rev; bump the rev together with `cargoLock.outputHashes` in `flake.nix`). `mod.rs` holds the state, `poll` and the window frame; `dictation.rs`, `history.rs`, `stats.rs` and `settings.rs` are the panes; `controls.rs` and `charts.rs` the shared pieces; `logic.rs` the view-independent logic and its tests; `backend.rs` the worker thread that talks to the database and daemon. Views read kit tokens through `Paint`, never colour literals. Check a visual change headlessly: `dictator-gui --demo --snap out.png --scene <name> [--theme light]` (`SCENES` in `mod.rs`).
- **Provider** (`src/provider.rs`): `ProviderSelector` holds the daemon's active provider. It starts from `$XDG_CONFIG_HOME/dictator/runtime.json` (`{"version": 1, "active_provider": "openai"}`), falling back to the config default with a warning when that names an unknown or unusable provider. The `providers` and `use_provider` IPC actions list and switch providers (`dictator provider [list|use <name>]`); the list travels as one JSON string in `Response.data["providers"]` and never carries keys. A provider is usable with a non-empty endpoint and a key with no unresolved `${env:...}`. Selecting the default deletes the override. The pipeline snapshots the provider when a transcription or retry starts and passes it in `TranscriptionRequest.provider`; the CLI `retry` reads the override itself (`effective_provider`).
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
- `audio/devices.rs`: priority list (`preferences.json`) and selection. Bluetooth sources are never selected. Ids: `bluetooth:<address>`; USB `usb:<vendor>:<product>[:<serial>]` (port-independent; a second identical device connected at once gets `@<bus_path>`); otherwise `bus:<bus_path>`. `reconcile` first migrates old `bus:...-usb-...` entries (adopt by old bus path, else highest-ranked same name; collapse same-name leftovers) and is idempotent, so `Resolution::fingerprint` only changes on a real write. `remove_microphone` refuses connected devices.
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