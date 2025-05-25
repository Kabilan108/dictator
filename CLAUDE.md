# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Dictator is a voice-to-text daemon for Linux that enables voice typing anywhere the cursor is positioned. The system uses a CLI/daemon architecture where a single binary operates in two modes:
- Daemon mode: Background service handling audio recording, transcription, and typing
- Client mode: CLI commands that communicate with the daemon via Unix socket IPC

## Build Commands

```bash
# Build the binary
make build

# Install to GOPATH/bin
make install

# Run the built binary
make run

# Update dependencies
make deps

# Clean build artifacts
make clean
```

## Development Commands

```bash
# Run daemon in foreground for development
go run . daemon

# Test CLI commands (requires daemon running)
go run . start
go run . stop
go run . toggle
go run . cancel
go run . status
```

## Architecture Overview

The application follows a modular design with these key packages:

- `internal/cmd/` - Cobra-based CLI command handlers with Viper configuration
- `internal/daemon/` - Main daemon implementation and state machine
- `internal/ipc/` - Unix socket IPC protocol for client-daemon communication
- `internal/audio/` - Audio recording using malgo (miniaudio bindings)
- `internal/transcribe/` - Whisper API client for speech-to-text
- `internal/typing/` - Keyboard input via xdotool
- `internal/notifier/` - Dunst integration for visual feedback
- `internal/config/` - Configuration management

## Key Dependencies

- `github.com/spf13/cobra` - CLI framework
- `github.com/spf13/viper` - Configuration management
- `github.com/fatih/color` - Terminal colors

Planned dependencies (from PLAN.md):
- `github.com/godbus/dbus/v5` - D-Bus for notifications
- `github.com/gen2brain/malgo` - Audio recording

## Configuration

Configuration file location: `~/.config/dictator/config.json`

Expected structure includes API keys for Whisper, audio settings, and behavior parameters.

## IPC Protocol

Uses Unix socket at `/tmp/dictator.sock` with JSON-based request/response protocol for command correlation between CLI and daemon.

## State Management

Daemon implements a state machine:
- Idle → Recording (start command)
- Recording → Transcribing (stop command)
- Transcribing → Typing (API response)
- Typing → Idle (completion)
- Any state → Idle (cancel command)

## External Dependencies

Required system packages:
- `xdotool` - Text input at cursor position
- `xclip` - Clipboard fallback
- `dunst` - Notification daemon
- Audio system (PulseAudio/PipeWire)

## Testing

Currently no test framework is configured. Check for test files or add testing setup as needed.
