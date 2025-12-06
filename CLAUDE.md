# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

Build and run the project:
```bash
make build         # builds to build/dictator
make run           # build and run daemon
make install       # install to GOPATH/bin
make clean         # remove build artifacts
make deps          # tidy dependencies
```

No specific test or lint commands are defined in the makefile.

## Architecture

Dictator is a voice typing daemon for Linux using a client-server architecture:

- **Binary modes**: Single binary operates as both daemon and CLI client
- **IPC**: Unix socket communication at `/tmp/dictator.sock`
- **State machine**: Daemon manages states: idle → recording → transcribing → typing → idle
- **Configuration**: JSON config at `~/.config/dictator/config.json`

### Core Components

- **Daemon** (`internal/daemon/`): Background service managing state transitions and orchestrating audio/transcription/typing
- **IPC** (`internal/ipc/`): Unix socket protocol for client-daemon communication
- **Audio** (`internal/audio/`): PortAudio recording and Whisper API transcription
- **Typing** (`internal/typing/`): Clipboard + paste simulation (X11: xclip/xdotool, Wayland: wl-copy/wtype)
- **Notifier** (`internal/notifier/`): D-Bus desktop notifications for state changes

### State Flow

The daemon implements a linear state machine:
1. **Idle**: Waiting for commands
2. **Recording**: Audio capture active
3. **Transcribing**: Sending audio to Whisper API
4. **Typing**: Pasting text via clipboard
5. **Error**: Temporary error state (auto-returns to idle)

### Key Dependencies

- **Cobra/Viper**: CLI framework and configuration
- **PortAudio**: Cross-platform audio I/O
- **Whisper API**: Speech-to-text transcription
- **X11**: xclip (clipboard) + xdotool (paste keystroke)
- **Wayland**: wl-clipboard (clipboard) + wtype (paste keystroke)

### Project Structure

- `main.go`: CLI command definitions and entry point
- `internal/daemon/daemon.go`: Core daemon logic and state management
- `internal/ipc/protocol.go`: IPC message definitions and constants
- `internal/utils/config.go`: Configuration management
- `dictator.service`: systemd service template