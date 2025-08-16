# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

Build and run the project:
```bash
make build         # builds to build/dictator
make run           # build and run daemon (runs: ./build/dictator)
make install       # install to GOPATH/bin  
make clean         # remove build artifacts
make deps          # tidy dependencies
make release       # build Linux AMD64 release tarball
```

No specific test or lint commands are defined in the makefile. Consider using:
```bash
go vet ./...       # basic Go static analysis
go fmt ./...       # format code
```

## Architecture

Dictator is a voice typing daemon for Linux using a client-server architecture:

- **Binary modes**: Single binary operates as both daemon and CLI client
- **IPC**: Unix socket communication at `/tmp/dictator.sock`
- **State machine**: Daemon manages states: idle → recording → transcribing → typing → idle
- **Configuration**: JSON config at `~/.config/dictator/config.json`
- **Storage**: SQLite database at `~/.cache/dictator/database.db` for transcripts

### Core Components

- **Daemon** (`internal/daemon/daemon.go`): Background service managing state transitions and orchestrating audio/transcription/typing
- **IPC** (`internal/ipc/`): Unix socket protocol for client-daemon communication
- **Audio** (`internal/audio/`): PortAudio recording and Whisper API transcription
- **Typing** (`internal/typing/`): Clipboard + paste simulation (X11: xclip/xdotool, Wayland: wl-copy/wtype)
- **Notifier** (`internal/notifier/`): D-Bus desktop notifications for state changes
  - `protocol.go`: Message definitions and constants
  - `server.go`: Daemon-side socket handling
  - `client.go`: CLI-side socket communication
- **Storage** (`internal/storage/`):
  - `database.go`: SQLite database management
  - `transcripts.go`: Transcript persistence and retrieval

### State Flow

The daemon implements a linear state machine with these transitions:
1. **Idle**: Waiting for commands
2. **Recording**: Audio capture active (max 5 minutes by default)
3. **Transcribing**: Sending audio to Whisper API
4. **Typing**: Pasting text via clipboard
5. **Error**: Temporary error state (auto-returns to idle)

Commands affect state:
- `start`: idle → recording
- `stop`: recording → transcribing
- `toggle`: switches between idle ↔ recording
- `cancel`: any state → idle

### Key Dependencies

- **Cobra/Viper**: CLI framework and configuration
- **PortAudio**: Cross-platform audio I/O
- **Whisper API**: Speech-to-text transcription
- **X11**: xclip (clipboard) + xdotool (paste keystroke)
- **Wayland**: wl-clipboard (clipboard) + wtype (paste keystroke)
- **Cobra/Viper**: CLI framework and configuration management
- **PortAudio** (via gordonklaus/portaudio): Cross-platform audio I/O
- **Whisper API**: OpenAI or compatible speech-to-text service
- **xdotool**: X11 keyboard simulation (falls back to xclip if unavailable)
- **D-Bus/dunst**: Linux desktop notifications
- **SQLite**: Local transcript storage

### Project Structure

- `main.go`: CLI command definitions and entry point
- `internal/daemon/daemon.go`: Core daemon logic and state management
- `internal/ipc/protocol.go`: IPC message definitions and constants
- `internal/utils/config.go`: Configuration management with Viper
- `dictator.service`: systemd service template for user services

### Configuration

Config file at `~/.config/dictator/config.json` with these sections:
- **api**: Whisper endpoint, key, model, timeout
- **audio**: Sample rate (16000), channels (1), bit depth (16), buffer size
- **app**: Log level, recording limits, typing delay

## Development Notes

- Go version 1.24+ required
- System dependencies: xdotool, xclip, pulseaudio-utils, portaudio19-dev
- Audio files cached in `~/.cache/dictator/recordings/`
- Database stored in `~/.cache/dictator/database.db`
- No test suite currently implemented
- GitHub Actions configured for releases (see `.github/workflows/release.yml`)
