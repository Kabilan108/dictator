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
- **Configuration**: JSON config at `~/.config/dictator/config.json`

### Core Components

- **Daemon** (`src/daemon.rs`): Background service managing state transitions and orchestrating audio/transcription/typing
- **IPC** (`src/ipc/`): Unix socket protocol for client-daemon communication
- **Audio** (`src/audio/`): `parec` capture, microphone selection via `pactl`, and Whisper API transcription
- **Typing** (`src/typing.rs`): Clipboard + paste simulation (X11: xclip/xdotool, Wayland: wl-copy/wtype)
- **Notifier** (`src/notifier.rs`): D-Bus desktop notifications for state changes
- **Visual** (`src/visual/`): OSD event stream (newline-delimited JSON on a unix socket)
- **Storage** (`src/storage.rs`): SQLite transcript history

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
- `audio/devices.rs`: priority list and selection. Bluetooth sources are never selected.
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