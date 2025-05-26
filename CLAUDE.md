# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Dictator is a voice-to-text daemon for Linux that enables voice typing anywhere the cursor is positioned. The system uses a CLI/daemon architecture where a single binary operates in two modes:
- Daemon mode: Background service handling audio recording, transcription, and typing
- Client mode: CLI commands that communicate with the daemon via Unix socket IPC

## Useful Devtools

### Code and file search: `dump`

If you want to read a number of files that match some filter quickly, use the `dump` tool in the bash shell. The command line flags available for dump are listed below.

```
usage: dump [options] [directories...]

  recursively dumps text files from specified directories,
  respecting .gitignore and custom ignore rules.

options:
  -d|--dir <value>       directory to scan (can be repeated)
  -g|--glob <value>      glob pattern to match (can be repeated)
  -f|--filter <string>   skip lines matching this regex
  -h|--help              display help message
  -i|--ignore <value>    glob pattern to ignore (can be repeated)
  -o|--out-fmt <string>  xml or md (default "xml")
  -l|--list              list file paths only
  --xml-tag <string>     custom XML tag name (only for xml output) (default "file")
```

Here are some examples of situations where you could use `dump`:

```
# Dump from specific directories
dump src/ tests/ docs/

# Dump with directory flags
dump -d src/ -d tests/

# List file paths only (no content). Helpful for refining your search filters before fetching the file contents
dump -l

# Include specific files using glob patterns
dump -g "**.go" -g "*.md"

# Add ignore patterns (can use multiple times)
dump -i "**.log" -i "node_modules"

# Filter out lines matching a regex pattern
dump -f "TODO|FIXME"

# Markdown output format instead of XML
dump -o md

# Custom XML tag name
dump --xml-tag source

# Combine options
dump src/ tests/ -g "*.go" -i "vendor" -f "^//.*"
```

By default, `dump` will print files to STDOUT formatted as XML in <file>...</file> tags. You can request markdown output by passing the `-o md` flag

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
