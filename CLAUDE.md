# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

Build and run the project (Rust, `cargo`; `nix develop` provides the toolchain and `alsa-lib`):
```bash
make build         # release build, copied to build/dictator
make run           # build and run daemon
make install       # cargo install --path . --locked
make test          # cargo test (unit + integration tests)
make check         # cargo fmt --check + cargo clippy -D warnings
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
- **Audio** (`src/audio/`): cpal (ALSA/PipeWire) recording and Whisper API transcription
- **Typing** (`src/typing.rs`): Clipboard + paste simulation (X11: xclip/xdotool, Wayland: wl-copy/wtype)
- **Notifier** (`src/notifier.rs`): D-Bus desktop notifications for state changes
- **Visual** (`src/visual/`): OSD event stream (newline-delimited JSON on a unix socket)
- **Storage** (`src/storage.rs`): SQLite transcript history

### State Flow

The daemon implements a linear state machine:
1. **Idle**: Waiting for commands
2. **Recording**: Audio capture active
3. **Transcribing**: Sending audio to Whisper API
4. **Typing**: Pasting text via clipboard
5. **Error**: Temporary error state (auto-returns to idle)

### Key Dependencies

- **clap**: CLI framework (with `clap_complete` for shell completions)
- **tokio**: async runtime for IPC, OSD socket, HTTP and subprocesses
- **cpal**: Cross-platform audio input (ALSA backend on Linux, works through PipeWire)
- **reqwest** (rustls): Whisper API transcription
- **zbus**: D-Bus notifications
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